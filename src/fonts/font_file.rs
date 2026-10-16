use std::collections::{BTreeMap, HashSet};

use anyhow::{Result, anyhow, bail};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::{LocationRef, Size};
use skrifa::raw::tables::cmap::{Cmap4, CmapSubtable};
use skrifa::raw::{FileRef, FontRef, TableProvider};
use skrifa::{GlyphId, MetadataProvider};

use super::charsets::{self, Charset};

// Symbol cmaps (3, 0) map private-use codes, not letters, so they are left out.
const CMAP_PREFERENCES: [(u16, u16); 8] = [
    (3, 10),
    (0, 6),
    (0, 4),
    (3, 1),
    (0, 3),
    (0, 2),
    (0, 1),
    (0, 0),
];
const FAMILY_NAME: u16 = 1;
const STYLE_NAME: u16 = 2;

pub type Cmap = BTreeMap<u32, GlyphId>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontCheck {
    pub family: String,
    pub style: String,
    pub glyphs: usize,
    pub missing_letters: String,
    pub missing_typography: String,
    pub empty: String,
    pub aliased: String,
}

pub fn check(data: &[u8], charset: &Charset) -> Result<FontCheck> {
    let font = open(data)?;
    if font.glyf().is_err() && font.cff().is_err() && font.cff2().is_err() {
        return Err(unusable(anyhow!("Font contains no outlines")));
    }
    let cmap = best_cmap(&font).map_err(unusable)?;
    let have: HashSet<u32> = cmap.keys().copied().collect();
    Ok(FontCheck {
        family: debug_name(&font, FAMILY_NAME).unwrap_or_else(|| "?".to_string()),
        style: debug_name(&font, STYLE_NAME).unwrap_or_else(|| "?".to_string()),
        glyphs: cmap.len(),
        missing_letters: charsets::find_missing(&have, &charset.letters),
        missing_typography: charsets::find_missing(&have, &charset.typography),
        empty: find_empty(&font, &cmap, &charset.all()),
        aliased: find_aliases(&cmap, &charset.lookalikes),
    })
}

pub fn load_cmap(data: &[u8]) -> Result<Cmap> {
    best_cmap(&open(data)?).map_err(unusable)
}

fn unusable(error: anyhow::Error) -> anyhow::Error {
    anyhow!("Not a usable TrueType/OpenType font: {error}")
}

/// A font collection is read by its first font.
fn open(data: &[u8]) -> Result<FontRef<'_>> {
    let font = match FileRef::new(data) {
        Ok(FileRef::Font(font)) => Ok(font),
        Ok(FileRef::Collection(collection)) => collection.get(0),
        Err(error) => Err(error),
    };
    font.map_err(|error| anyhow!("Not a usable TrueType/OpenType font: {error}"))
}

fn best_cmap(font: &FontRef) -> Result<Cmap> {
    let Ok(cmap) = font.cmap() else {
        return Ok(Cmap::new());
    };
    let records = cmap.encoding_records();
    let Some(record) = CMAP_PREFERENCES.iter().find_map(|&(platform, encoding)| {
        records
            .iter()
            .find(|r| r.platform_id() as u16 == platform && r.encoding_id() == encoding)
    }) else {
        return Ok(Cmap::new());
    };
    match record.subtable(cmap.offset_data()) {
        Ok(CmapSubtable::Format4(table)) => map_format4(&table),
        Ok(subtable) => Ok(subtable
            .iter()
            .filter(|(_, glyph)| *glyph != GlyphId::NOTDEF)
            .collect()),
        Err(_) => Ok(Cmap::new()),
    }
}

/// The 0xFFFF end-marker segment is skipped, later segments override earlier ones,
/// and glyph 0 never enters the map.
fn map_format4(table: &Cmap4) -> Result<Cmap> {
    let (starts, ends) = (table.start_code(), table.end_code());
    let (deltas, offsets, glyphs) = (
        table.id_delta(),
        table.id_range_offsets(),
        table.glyph_id_array(),
    );
    let segments = starts
        .len()
        .min(ends.len())
        .min(deltas.len())
        .min(offsets.len());
    let mut cmap = Cmap::new();
    for segment in 0..segments.saturating_sub(1) {
        let (start, end) = (
            u32::from(starts[segment].get()),
            u32::from(ends[segment].get()),
        );
        let delta = i64::from(deltas[segment].get());
        let range_offset = i64::from(offsets[segment].get());
        let partial = range_offset / 2 - i64::from(start) + segment as i64 - offsets.len() as i64;
        for code in start..=end {
            let glyph = if range_offset == 0 {
                (i64::from(code) + delta) & 0xFFFF
            } else {
                let index = i64::from(code) + partial;
                let Some(raw) = usize::try_from(index).ok().and_then(|i| glyphs.get(i)) else {
                    bail!(
                        "cmap format 4 subtable: glyph index array offset {index} out of range in segment {segment}"
                    );
                };
                match raw.get() {
                    0 => 0,
                    raw => (i64::from(raw) + delta) & 0xFFFF,
                }
            };
            if glyph != 0 {
                cmap.insert(code, GlyphId::new(glyph as u32));
            }
        }
    }
    Ok(cmap)
}

/// The Windows English name, else the first Mac English one, else the last one that decodes.
fn debug_name(font: &FontRef, name_id: u16) -> Option<String> {
    let name = font.name().ok()?;
    let data = name.string_data();
    let (mut mac_english, mut some) = (None, None);
    for record in name.name_record() {
        if record.name_id().to_u16() != name_id {
            continue;
        }
        let (platform, language) = (record.platform_id(), record.language_id());
        let Some(text) = decode_name(platform, record.encoding_id(), language, record, data) else {
            continue;
        };
        if (platform, language) == (3, 0x409) {
            if !text.is_empty() {
                return Some(text);
            }
            some = Some(text);
            break;
        }
        if (platform, language) == (1, 0) && mac_english.is_none() {
            mac_english = Some(text.clone());
        }
        some = Some(text);
    }
    [mac_english, some]
        .into_iter()
        .flatten()
        .find(|text| !text.is_empty())
}

fn decode_name(
    platform: u16,
    encoding: u16,
    language: u16,
    record: &skrifa::raw::tables::name::NameRecord,
    data: skrifa::raw::FontData,
) -> Option<String> {
    let bytes = record.string(data).ok()?;
    let is_utf16 = platform == 0 || (platform == 3 && matches!(encoding, 0 | 1 | 10));
    let is_mac_roman = platform == 1 && encoding == 0 && language == 0;
    if !is_utf16 && !is_mac_roman {
        return None;
    }
    let text: String = bytes.chars().collect();
    // A broken UTF-16 record is skipped rather than shown with replacement marks.
    (!text.contains(char::REPLACEMENT_CHARACTER)).then_some(text)
}

fn find_empty(font: &FontRef, cmap: &Cmap, wanted: &str) -> String {
    let outlines = font.outline_glyphs();
    wanted
        .chars()
        .filter(|ch| {
            cmap.get(&u32::from(*ch)).is_some_and(|glyph| {
                outlines.get(*glyph).is_none_or(|outline| {
                    let mut pen = AnyPoint::default();
                    let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
                    outline.draw(settings, &mut pen).is_err() || !pen.drawn
                })
            })
        })
        .collect()
}

fn find_aliases(cmap: &Cmap, pairs: &[(char, char)]) -> String {
    pairs
        .iter()
        .filter(|(ch, other)| {
            let glyph = cmap.get(&u32::from(*ch));
            glyph.is_some() && glyph == cmap.get(&u32::from(*other))
        })
        .map(|(ch, _)| *ch)
        .collect()
}

#[derive(Default)]
struct AnyPoint {
    drawn: bool,
}

impl OutlinePen for AnyPoint {
    fn move_to(&mut self, _: f32, _: f32) {
        self.drawn = true;
    }
    fn line_to(&mut self, _: f32, _: f32) {
        self.drawn = true;
    }
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
        self.drawn = true;
    }
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
        self.drawn = true;
    }
    fn close(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_missing_empty_and_aliased_letters() {
        let data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/empty-aliased.ttf"
        ))
        .unwrap();
        let result = check(&data, &charsets::get("uk").unwrap()).unwrap();
        assert_eq!(result.missing_letters, "Щщ");
        assert_eq!(result.empty, "Ж");
        assert_eq!(result.aliased, "Ґ");
    }
}
