use std::collections::HashSet;

// m_CharacterTable is an int32 count followed by (elementType=1, unicode, glyphIndex, float scale) records.
const RECORD_SIZE: usize = 16;
const MAX_GLYPH_INDEX: u32 = 1 << 20;
const MAX_CHARACTERS: i32 = 1 << 18;

pub fn extract_codepoints(raw: &[u8]) -> HashSet<u32> {
    let mut longest: Vec<u32> = Vec::new();
    for offset in (0..raw.len().saturating_sub(4)).step_by(4) {
        let table = read_table(raw, offset);
        if table.len() > longest.len() {
            longest = table;
        }
    }
    longest.into_iter().collect()
}

fn read_table(raw: &[u8], offset: usize) -> Vec<u32> {
    let count = i32::from_le_bytes(read_word(raw, offset));
    let start = offset + 4;
    if !(1..=MAX_CHARACTERS).contains(&count) {
        return Vec::new();
    }
    let count = count as usize;
    if start + count * RECORD_SIZE > raw.len() {
        return Vec::new();
    }
    let mut codepoints = Vec::with_capacity(count);
    for index in 0..count {
        let at = start + index * RECORD_SIZE;
        let Some(codepoint) = read_character(raw, at) else {
            return Vec::new();
        };
        codepoints.push(codepoint);
    }
    codepoints
}

fn read_character(raw: &[u8], at: usize) -> Option<u32> {
    let word = |n: usize| u32::from_le_bytes(read_word(raw, at + 4 * n));
    let (element_type, codepoint, glyph_index) = (word(0), word(1), word(2));
    let scale = f32::from_bits(word(3));
    let valid = element_type == 1
        && (1..0x110000).contains(&codepoint)
        && glyph_index < MAX_GLYPH_INDEX
        && scale.is_finite()
        && scale > 0.0;
    valid.then_some(codepoint)
}

fn read_word(raw: &[u8], at: usize) -> [u8; 4] {
    raw.get(at..at + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .unwrap_or([0; 4])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character(codepoint: u32, glyph: u32) -> Vec<u8> {
        [1, codepoint, glyph]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .chain(1.0f32.to_le_bytes())
            .collect()
    }

    #[test]
    fn reads_the_counted_character_table_and_ignores_lookalike_glyph_records() {
        // A glyph rect at y=1 followed by scale 1.0 looks like one character record on its own.
        let noise: Vec<u8> = [1u32, 1, 40, 52]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .chain(1.0f32.to_le_bytes())
            .collect();
        let raw = [
            noise,
            2i32.to_le_bytes().to_vec(),
            character('A' as u32, 36),
            character('Ї' as u32, 900),
        ]
        .concat();
        assert_eq!(
            extract_codepoints(&raw),
            HashSet::from(['A' as u32, 'Ї' as u32])
        );
    }
}
