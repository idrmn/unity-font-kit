//! Kerning pairs for a TMP font asset, read the way Unity's TMP reads them.

use std::collections::HashSet;

use anyhow::{Context, Result, anyhow};
use indexmap::IndexMap;
use skrifa::raw::tables::gpos::{
    PairPos, PairPosFormat1, PairPosFormat2, PositionSubtables, ValueRecord,
};
use skrifa::raw::tables::kern::SubtableKind;
use skrifa::raw::{FontRef, TableProvider};

/// XPlacement, YPlacement, XAdvance, YAdvance in font units.
pub type Shift = [i32; 4];
pub type Pair = (u32, u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adjustment {
    pub first: Shift,
    pub second: Shift,
}

const NO_SHIFT: Shift = [0; 4];
const NO_ADJUSTMENT: Adjustment = Adjustment {
    first: NO_SHIFT,
    second: NO_SHIFT,
};

/// A pair record with values scaled to the atlas point size.
#[derive(Debug, Clone, PartialEq)]
pub struct PairRecord {
    pub first: u32,
    pub second: u32,
    pub first_shift: [f64; 4],
    pub second_shift: [f64; 4],
}

type Pairs = IndexMap<Pair, Adjustment>;

pub fn is_before_unity6(unity_version: &str) -> bool {
    let major = unity_version
        .split('.')
        .next()
        .and_then(|part| part.parse::<u32>().ok());
    major.is_some_and(|major| major < 6000)
}

pub fn read_kerning(
    font: &[u8],
    placed: &HashSet<u32>,
    point_size: f64,
    unity_version: &str,
) -> Result<Vec<PairRecord>> {
    let font = FontRef::new(font).map_err(|error| anyhow!("cannot read font: {error}"))?;
    let scale = point_size / f64::from(font.head().context("no head table")?.units_per_em());
    let glyph_count = u32::from(font.maxp().context("no maxp table")?.num_glyphs());
    let kern = read_kern_pairs(&font);
    // Measured: TMP before Unity 6 reads only the kern table; Unity 6 also reads GPOS pairs.
    let pairs = if is_before_unity6(unity_version) {
        kern
    } else {
        let mut seconds: Vec<u32> = placed
            .iter()
            .copied()
            .filter(|glyph| *glyph < glyph_count)
            .collect();
        seconds.sort_unstable();
        merge(read_gpos_pairs(&font, &seconds)?, kern)
    };
    Ok(pairs
        .into_iter()
        .filter(|((a, b), _)| {
            *a < glyph_count && *b < glyph_count && placed.contains(a) && placed.contains(b)
        })
        .map(|((first, second), adjustment)| describe_pair(first, second, adjustment, scale))
        .collect())
}

/// GPOS order first, kern values win, kern-only pairs after.
fn merge(mut gpos: Pairs, kern: Pairs) -> Pairs {
    gpos.extend(kern);
    gpos
}

fn read_kern_pairs(font: &FontRef) -> Pairs {
    let mut pairs = Pairs::new();
    let Ok(kern) = font.kern() else {
        return pairs;
    };
    for subtable in kern.subtables().flatten() {
        let Ok(SubtableKind::Format0(table)) = subtable.kind() else {
            continue;
        };
        for pair in table.pairs() {
            let value = i32::from(pair.value());
            if value != 0 {
                let shift = [0, 0, value, 0];
                pairs.insert(
                    (pair.left().to_u32(), pair.right().to_u32()),
                    Adjustment {
                        first: shift,
                        second: NO_SHIFT,
                    },
                );
            }
        }
    }
    pairs
}

fn read_gpos_pairs(font: &FontRef, seconds: &[u32]) -> Result<Pairs> {
    let mut pairs = Pairs::new();
    let Ok(gpos) = font.gpos() else {
        return Ok(pairs);
    };
    let Ok(lookups) = gpos.lookup_list() else {
        return Ok(pairs);
    };
    for lookup in lookups.lookups().iter() {
        let Ok(PositionSubtables::Pair(subtables)) = lookup?.subtables() else {
            continue;
        };
        for subtable in subtables.iter() {
            for (pair, adjustment) in read_pair_subtable(&subtable?, seconds)? {
                pairs.entry(pair).or_insert(adjustment);
            }
        }
    }
    Ok(pairs)
}

fn read_pair_subtable(subtable: &PairPos, seconds: &[u32]) -> Result<Pairs> {
    match subtable {
        PairPos::Format1(table) => read_glyph_pairs(table),
        PairPos::Format2(table) => read_class_pairs(table, seconds),
    }
}

fn read_glyph_pairs(table: &PairPosFormat1) -> Result<Pairs> {
    let mut found = Pairs::new();
    let firsts = table.coverage()?.iter();
    for (first, pair_set) in firsts.zip(table.pair_sets().iter()) {
        for record in pair_set?.pair_value_records().iter() {
            let record = record?;
            let adjustment = read_adjustment(record.value_record1(), record.value_record2());
            found.insert(
                (
                    u32::from(first.to_u16()),
                    u32::from(record.second_glyph().to_u16()),
                ),
                adjustment,
            );
        }
    }
    found.retain(|_, adjustment| *adjustment != NO_ADJUSTMENT);
    Ok(found)
}

fn read_class_pairs(table: &PairPosFormat2, seconds: &[u32]) -> Result<Pairs> {
    let (class1, class2) = (table.class_def1()?, table.class_def2()?);
    // Glyphs the class table leaves out belong to class 0.
    let mut by_class: IndexMap<u16, Vec<u32>> = IndexMap::new();
    for &glyph in seconds {
        by_class
            .entry(class2.get(skrifa::GlyphId::new(glyph)))
            .or_default()
            .push(glyph);
    }
    let rows = table.class1_records();
    let mut pairs = Pairs::new();
    for first in table.coverage()?.iter() {
        let row = rows.get(usize::from(class1.get(first)))?;
        for (class, record) in row.class2_records().iter().enumerate() {
            let record = record?;
            let adjustment = read_adjustment(record.value_record1(), record.value_record2());
            let Some(glyphs) = by_class.get(&u16::try_from(class)?) else {
                continue;
            };
            if adjustment == NO_ADJUSTMENT {
                continue;
            }
            for &second in glyphs {
                pairs.insert((u32::from(first.to_u16()), second), adjustment);
            }
        }
    }
    Ok(pairs)
}

fn read_adjustment(first: &ValueRecord, second: &ValueRecord) -> Adjustment {
    Adjustment {
        first: read_shift(first),
        second: read_shift(second),
    }
}

fn read_shift(value: &ValueRecord) -> Shift {
    [
        value.x_placement(),
        value.y_placement(),
        value.x_advance(),
        value.y_advance(),
    ]
    .map(|part| i32::from(part.unwrap_or(0)))
}

fn describe_pair(first: u32, second: u32, adjustment: Adjustment, scale: f64) -> PairRecord {
    let scaled = |shift: Shift| shift.map(|part| f64::from(part) * scale);
    PairRecord {
        first,
        second,
        first_shift: scaled(adjustment.first),
        second_shift: scaled(adjustment.second),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn gpos_pair_keeps_the_shift_of_the_second_glyph() {
        let pairs = read_kerning(
            &fixture("gpos-second-shift.ttf"),
            &HashSet::from([1, 2]),
            40.0,
            "6000.3.12f1",
        )
        .unwrap();
        let second = pairs[0].second_shift;
        assert_eq!((second[1], second[2]), (0.4, -2.0));
    }

    #[test]
    fn gpos_class_pair_covers_second_glyphs_of_the_unlisted_class() {
        let pairs = read_kerning(
            &fixture("gpos-class-zero.ttf"),
            &HashSet::from([1, 2]),
            40.0,
            "6000.3.12f1",
        )
        .unwrap();
        let firsts: Vec<(u32, f64)> = pairs.iter().map(|p| (p.first, p.first_shift[2])).collect();
        assert_eq!(firsts, [(1, -2.0), (1, -2.0)]);
    }
}
