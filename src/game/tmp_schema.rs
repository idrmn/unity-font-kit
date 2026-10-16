//! TMP 1.4+ stores m_CharacterTable and m_GlyphTable; older TMP stores m_glyphInfoList.

use serde_json::{Map, Value, json};

pub type Tree = Map<String, Value>;

pub fn is_new_schema(tree: &Tree) -> bool {
    tree.contains_key("m_CharacterTable")
}

pub fn read_atlas_refs(tree: &Tree) -> Vec<Value> {
    if is_new_schema(tree) {
        return list(tree, "m_AtlasTextures");
    }
    match tree.get("atlas") {
        Some(atlas) if atlas.get("m_PathID").is_some_and(is_truthy) => vec![atlas.clone()],
        _ => Vec::new(),
    }
}

pub fn to_glyph_info(codepoint: &Value, glyph: &Value, atlas_height: i64) -> Value {
    let (rect, metrics) = (&glyph["m_GlyphRect"], &glyph["m_Metrics"]);
    let rect_height = number(&rect["m_Height"]);
    let y = if rect_height != 0.0 {
        (atlas_height as f64 - number(&rect["m_Y"])) - rect_height
    } else {
        0.0
    };
    json!({
        "id": codepoint,
        "x": number(&rect["m_X"]),
        "y": y,
        "width": number(&metrics["m_Width"]),
        "height": number(&metrics["m_Height"]),
        "xOffset": number(&metrics["m_HorizontalBearingX"]),
        "yOffset": number(&metrics["m_HorizontalBearingY"]),
        "xAdvance": number(&metrics["m_HorizontalAdvance"]),
        "scale": glyph.get("m_Scale").map_or(1.0, number),
    })
}

pub fn to_glyph_infos(baked: &Tree) -> Vec<Value> {
    let atlas_height = baked
        .get("atlas_height")
        .map_or(0, |value| number(value) as i64);
    let glyphs: std::collections::HashMap<i64, &Value> = list_ref(baked, "glyphs")
        .iter()
        .map(|glyph| (number(&glyph["m_Index"]) as i64, glyph))
        .collect();
    list_ref(baked, "characters")
        .iter()
        .filter_map(|character| {
            let glyph = glyphs.get(&(number(&character["m_GlyphIndex"]) as i64))?;
            Some(to_glyph_info(&character["m_Unicode"], glyph, atlas_height))
        })
        .collect()
}

pub fn to_font_info(face: &Value, baked: &Tree) -> Value {
    let get = |key: &str, default: Value| face.get(key).cloned().unwrap_or(default);
    json!({
        "Name": get("m_FamilyName", json!("")),
        "PointSize": get("m_PointSize", json!(0)),
        "Scale": get("m_Scale", json!(1.0)),
        "CharacterCount": list_ref(baked, "characters").len(),
        "LineHeight": get("m_LineHeight", json!(0)),
        "Baseline": get("m_Baseline", json!(0)),
        "Ascender": get("m_AscentLine", json!(0)),
        "CapHeight": get("m_CapLine", json!(0)),
        "Descender": get("m_DescentLine", json!(0)),
        "CenterLine": get("m_MeanLine", json!(0)),
        "SuperscriptOffset": get("m_SuperscriptOffset", json!(0)),
        "SubscriptOffset": get("m_SubscriptOffset", json!(0)),
        "SubSize": get("m_SubscriptSize", json!(0.5)),
        "Underline": get("m_UnderlineOffset", json!(0)),
        "UnderlineThickness": get("m_UnderlineThickness", json!(0)),
        "strikethrough": get("m_StrikethroughOffset", json!(0)),
        "strikethroughThickness": get("m_StrikethroughThickness", json!(0)),
        "TabWidth": get("m_TabWidth", json!(0)),
        "Padding": baked.get("atlas_padding").map_or(0.0, number),
        "AtlasWidth": baked.get("atlas_width").map_or(0.0, number),
        "AtlasHeight": baked.get("atlas_height").map_or(0.0, number),
    })
}

pub fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::Bool(flag) => *flag,
        Value::Null => false,
        _ => true,
    }
}

fn list(tree: &Tree, key: &str) -> Vec<Value> {
    list_ref(tree, key).to_vec()
}

fn list_ref<'a>(tree: &'a Tree, key: &str) -> &'a [Value] {
    tree.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_glyph_converts_to_an_old_glyph_measured_from_the_top() {
        let glyph = json!({
            "m_GlyphRect": {"m_X": 801, "m_Y": 270, "m_Width": 45, "m_Height": 47},
            "m_Metrics": {"m_Width": 44.6, "m_Height": 46.1, "m_HorizontalBearingX": 0.6,
                "m_HorizontalBearingY": 45.8, "m_HorizontalAdvance": 46.3},
        });
        let old = to_glyph_info(&json!(u32::from('Ї')), &glyph, 1024);
        let fields = ["x", "y", "width", "xAdvance"].map(|key| old[key].as_f64().unwrap());
        assert_eq!(fields, [801.0, 707.0, 44.6, 46.3]);
    }
}
