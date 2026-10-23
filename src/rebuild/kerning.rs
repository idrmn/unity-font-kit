//! Kerning pairs for a TMP font asset, read the way Unity's TMP reads them.

use std::collections::HashSet;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use skrifa::GlyphId;
use skrifa::raw::tables::gpos::{
    PairPos, PairPosFormat1, PairPosFormat2, PositionSubtables, ValueRecord,
};
use skrifa::raw::tables::kern::SubtableKind;
use skrifa::raw::{FontRef, TableProvider};

/// In font units as read from the font, in pixels once scaled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shift<T> {
    pub x_placement: T,
    pub y_placement: T,
    pub x_advance: T,
    pub y_advance: T,
}

impl Shift<i32> {
    fn scaled(self, scale: f64) -> Shift<f64> {
        let scale = |part: i32| f64::from(part) * scale;
        Shift {
            x_placement: scale(self.x_placement),
            y_placement: scale(self.y_placement),
            x_advance: scale(self.x_advance),
            y_advance: scale(self.y_advance),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphPair {
    pub first: u32,
    pub second: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adjustment {
    pub first: Shift<i32>,
    pub second: Shift<i32>,
}

const NO_SHIFT: Shift<i32> = Shift {
    x_placement: 0,
    y_placement: 0,
    x_advance: 0,
    y_advance: 0,
};
const NO_ADJUSTMENT: Adjustment = Adjustment {
    first: NO_SHIFT,
    second: NO_SHIFT,
};

/// A pair record with values scaled to the atlas point size.
#[derive(Debug, Clone, PartialEq)]
pub struct PairRecord {
    pub first: u32,
    pub second: u32,
    pub first_shift: Shift<f64>,
    pub second_shift: Shift<f64>,
}

type Pairs = IndexMap<GlyphPair, Adjustment>;

#[must_use]
pub fn is_before_unity6(unity_version: &str) -> bool {
    let major: Option<u32> = unity_version
        .split('.')
        .next()
        .and_then(|part| part.parse().ok());
    major.is_some_and(|major| major < 6000)
}

pub fn read_kerning(
    font: &[u8],
    placed: &HashSet<u32>,
    point_size: f64,
    unity_version: &str,
) -> Result<Vec<PairRecord>> {
    let font = FontRef::new(font).context("cannot read font")?;
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
        .filter(|(GlyphPair { first, second }, _)| {
            *first < glyph_count
                && *second < glyph_count
                && placed.contains(first)
                && placed.contains(second)
        })
        .map(|(pair, adjustment)| describe_pair(pair, adjustment, scale))
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
                let shift = Shift {
                    x_advance: value,
                    ..NO_SHIFT
                };
                pairs.insert(
                    GlyphPair {
                        first: pair.left().to_u32(),
                        second: pair.right().to_u32(),
                    },
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
                GlyphPair {
                    first: u32::from(first.to_u16()),
                    second: u32::from(record.second_glyph().to_u16()),
                },
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
            .entry(class2.get(GlyphId::new(glyph)))
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
                let pair = GlyphPair {
                    first: u32::from(first.to_u16()),
                    second,
                };
                pairs.insert(pair, adjustment);
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

fn read_shift(value: &ValueRecord) -> Shift<i32> {
    let read = |part: Option<i16>| i32::from(part.unwrap_or(0));
    Shift {
        x_placement: read(value.x_placement()),
        y_placement: read(value.y_placement()),
        x_advance: read(value.x_advance()),
        y_advance: read(value.y_advance()),
    }
}

fn describe_pair(pair: GlyphPair, adjustment: Adjustment, scale: f64) -> PairRecord {
    PairRecord {
        first: pair.first,
        second: pair.second,
        first_shift: adjustment.first.scaled(scale),
        second_shift: adjustment.second.scaled(scale),
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
        assert_eq!((second.y_placement, second.x_advance), (0.4, -2.0));
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
        let firsts: Vec<(u32, f64)> = pairs
            .iter()
            .map(|p| (p.first, p.first_shift.x_advance))
            .collect();
        assert_eq!(firsts, [(1, -2.0), (1, -2.0)]);
    }
}
