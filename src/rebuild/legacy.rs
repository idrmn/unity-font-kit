use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use skrifa::raw::{FileRef, FontRef, TableProvider};

use crate::fonts::font_file;
use crate::game::assets;
use crate::game::tmp_schema::{Tree, number};

#[derive(Debug)]
pub struct LineMetrics {
    pub ascent: f64,
    pub descent: f64,
    pub line_spacing: f64,
}

pub fn swap_font_data(tree: &Tree, data: &[u8]) -> Result<Tree> {
    let legacy_bounds = tree
        .get("m_UseLegacyBoundsCalculation")
        .is_some_and(is_truthy);
    let size = tree.get("m_FontSize").map_or(0.0, number);
    let metrics = read_line_metrics(data, size, legacy_bounds)?;
    let mut swapped = tree.clone();
    swapped.insert("m_FontData".to_string(), assets::bytes_value(data));
    let updates = [
        ("m_Ascent", metrics.ascent),
        ("m_Descent", metrics.descent),
        ("m_LineSpacing", metrics.line_spacing),
    ];
    for (key, value) in updates {
        if tree.contains_key(key) {
            swapped.insert(key.to_string(), json!(value));
        }
    }
    if tree.contains_key("m_KerningValues") {
        // Kerning pairs were measured on the old font.
        swapped.insert("m_KerningValues".to_string(), json!([]));
    }
    Ok(swapped)
}

pub fn list_lost(old: &[u8], new: &[u8]) -> Result<String> {
    let Ok(had) = font_file::load_cmap(old) else {
        return Ok(String::new());
    };
    let kept = font_file::load_cmap(new)?;
    Ok(had
        .keys()
        .filter(|cp| **cp >= 0x20 && !kept.contains_key(cp))
        .filter_map(|cp| char::from_u32(*cp))
        .collect())
}

/// Measured on untouched game fonts: Unity scales hhea by size / em.
/// The head bounds for the legacy mode rest on one font only.
pub fn read_line_metrics(data: &[u8], size: f64, legacy_bounds: bool) -> Result<LineMetrics> {
    let unusable = |error: String| anyhow!("Not a usable TrueType/OpenType font: {error}");
    let font = open_first(data).map_err(unusable)?;
    let head = font.head().map_err(|error| unusable(error.to_string()))?;
    let hhea = font.hhea().map_err(|error| unusable(error.to_string()))?;
    let scale = size / f64::from(head.units_per_em());
    let (ascent, descent) = if legacy_bounds {
        (i32::from(head.y_max()), i32::from(head.y_min()))
    } else {
        (
            i32::from(hhea.ascender().to_i16()),
            i32::from(hhea.descender().to_i16()),
        )
    };
    let gap = i32::from(hhea.ascender().to_i16()) - i32::from(hhea.descender().to_i16())
        + i32::from(hhea.line_gap().to_i16());
    Ok(LineMetrics {
        ascent: f64::from(ascent) * scale,
        descent: f64::from(descent) * scale,
        line_spacing: f64::from(gap) * scale,
    })
}

fn open_first(data: &[u8]) -> std::result::Result<FontRef<'_>, String> {
    match FileRef::new(data) {
        Ok(FileRef::Font(font)) => Ok(font),
        Ok(FileRef::Collection(collection)) => collection.get(0).map_err(|e| e.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::Null => false,
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn swap_puts_the_new_font_in_with_its_line_metrics() {
        let tree = json!({"m_Name": "Body", "m_FontSize": 20.0, "m_FontData": [1, 2, 3], "m_Ascent": 1.0,
            "m_Descent": -1.0, "m_LineSpacing": 2.0, "m_KerningValues": [{"first": 1}],
            "m_UseLegacyBoundsCalculation": false});
        let tree = tree.as_object().unwrap();
        let data = fixture("make-font-AB.ttf");
        let swapped = swap_font_data(tree, &data).unwrap();
        assert_eq!(assets::read_bytes(swapped.get("m_FontData")), data);
        let metrics =
            ["m_Ascent", "m_Descent", "m_LineSpacing"].map(|key| swapped[key].as_f64().unwrap());
        assert_eq!(metrics, [16.0, -4.0, 20.0]);
        assert_eq!(swapped["m_KerningValues"], json!([]));
        assert_eq!(tree["m_FontData"], json!([1, 2, 3]));
    }

    #[test]
    fn legacy_bounds_take_ascent_and_descent_from_the_glyph_box() {
        let metrics = read_line_metrics(&fixture("make-font-AB.ttf"), 20.0, true).unwrap();
        assert_eq!((metrics.ascent, metrics.descent), (14.0, 0.0));
    }

    #[test]
    fn characters_the_new_font_lacks_are_reported_as_lost() {
        let lost = list_lost(&fixture("make-font-ABC.ttf"), &fixture("make-font-AB.ttf")).unwrap();
        assert_eq!(lost, "C");
    }
}
