use std::collections::{HashMap, HashSet};

use crate::fonts::charsets::{self, Charset};
use crate::fonts::font_file::{self, FontCheck};
use crate::game::scan::{LegacyFont, Ref, ScanResult, TmpFont};

#[derive(Clone, Debug, PartialEq)]
pub struct TmpCoverage {
    pub font: TmpFont,
    pub missing_letters: String,
    pub missing_typography: String,
    pub missing_letters_with_fallbacks: String,
    pub chain: Vec<String>,
    pub texts: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LegacyCoverage {
    pub font: LegacyFont,
    pub check: Option<FontCheck>,
    pub error: Option<String>,
    pub texts: usize,
}

pub fn check_tmp_fonts(result: &ScanResult, charset: &Charset) -> Vec<TmpCoverage> {
    let index: HashMap<&Ref, &TmpFont> = result
        .tmp
        .iter()
        .map(|font| (&font.reference, font))
        .collect();
    let global_fallbacks = collect_settings_fallbacks(result);
    let texts = count_texts(result);
    result
        .tmp
        .iter()
        .map(|font| TmpCoverage {
            texts: texts.get(&font.reference).copied().unwrap_or(0),
            ..check_tmp_font(font, &index, &global_fallbacks, charset)
        })
        .collect()
}

fn collect_settings_fallbacks(result: &ScanResult) -> Vec<Ref> {
    result
        .settings
        .iter()
        .flat_map(|settings| {
            settings
                .fallbacks
                .iter()
                .cloned()
                .chain(settings.default_font.clone())
        })
        .collect()
}

fn count_texts(result: &ScanResult) -> HashMap<&Ref, usize> {
    let mut texts = HashMap::new();
    for text in &result.uses {
        *texts.entry(&text.font).or_insert(0) += 1;
    }
    texts
}

fn check_tmp_font(
    font: &TmpFont,
    index: &HashMap<&Ref, &TmpFont>,
    global_fallbacks: &[Ref],
    charset: &Charset,
) -> TmpCoverage {
    let start: Vec<&Ref> = std::iter::once(&font.reference)
        .chain(global_fallbacks)
        .collect();
    let chain = collect_fallback_chain(&start, index);
    let own: HashSet<u32> = font.codepoints.iter().copied().collect();
    let all: HashSet<u32> = chain
        .iter()
        .flat_map(|f| f.codepoints.iter().copied())
        .collect();
    TmpCoverage {
        font: font.clone(),
        missing_letters: charsets::find_missing(&own, &charset.letters),
        missing_typography: charsets::find_missing(&own, &charset.typography),
        missing_letters_with_fallbacks: charsets::find_missing(&all, &charset.letters),
        chain: chain[1..].iter().map(|f| f.name.clone()).collect(),
        texts: 0,
    }
}

/// Depth-first through each font's fallbacks, in TMP's order; every font once, so cycles end.
fn collect_fallback_chain<'a>(
    start: &[&Ref],
    index: &HashMap<&Ref, &'a TmpFont>,
) -> Vec<&'a TmpFont> {
    let mut seen: HashSet<&Ref> = HashSet::new();
    let mut order = Vec::new();
    let mut stack: Vec<&Ref> = start.iter().rev().copied().collect();
    while let Some(reference) = stack.pop() {
        if seen.contains(reference) {
            continue;
        }
        let Some((key, font)) = index.get_key_value(reference) else {
            continue;
        };
        seen.insert(key);
        order.push(*font);
        stack.extend(font.fallbacks.iter().rev());
    }
    order
}

pub fn check_legacy_fonts(result: &ScanResult, charset: &Charset) -> Vec<LegacyCoverage> {
    let texts = count_texts(result);
    result
        .legacy
        .iter()
        .map(|font| LegacyCoverage {
            texts: texts.get(&font.reference).copied().unwrap_or(0),
            ..check_legacy_font(font, charset)
        })
        .collect()
}

fn check_legacy_font(font: &LegacyFont, charset: &Charset) -> LegacyCoverage {
    let (check, error) = if font.data.is_empty() {
        (None, Some("no embedded font data".to_string()))
    } else {
        match font_file::check(&font.data, charset) {
            Ok(check) => (Some(check), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        }
    };
    LegacyCoverage {
        font: font.clone(),
        check,
        error,
        texts: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::game::scan::TmpSettings;

    fn font(path_id: i64, chars: &str, fallbacks: &[i64]) -> TmpFont {
        TmpFont {
            file: PathBuf::from("resources.assets"),
            reference: ("resources.assets".to_string(), path_id),
            name: format!("font{path_id}"),
            codepoints: chars.chars().map(u32::from).collect(),
            fallbacks: fallbacks
                .iter()
                .map(|f| ("resources.assets".to_string(), *f))
                .collect(),
            family: None,
            point_size: None,
            padding: None,
            atlas: None,
            render_mode: None,
            dynamic: None,
            atlases: None,
            source: "typetree".to_string(),
            source_font: None,
        }
    }

    #[test]
    fn fallback_chain_follows_font_then_global_settings_and_survives_cycles() {
        let charset = charsets::get("uk").unwrap();
        let letters = &charset.letters;
        let result = ScanResult {
            tmp: vec![
                font(1, "ABC", &[2]),
                font(2, &letters.replace('ґ', ""), &[1]),
                font(3, "ґ", &[]),
            ],
            settings: vec![TmpSettings {
                file: PathBuf::from("resources.assets"),
                default_font: Some(("resources.assets".to_string(), 3)),
                fallbacks: Vec::new(),
            }],
            ..ScanResult::default()
        };
        let covered = &check_tmp_fonts(&result, &charset)[0];
        assert_eq!(covered.chain, ["font2", "font3"]);
        assert_eq!(
            covered.missing_letters.chars().count(),
            letters.chars().count()
        );
        assert_eq!(covered.missing_letters_with_fallbacks, "");
    }
}
