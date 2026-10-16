use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_general_category::{GeneralCategory as G, get_general_category};

use crate::fonts::charsets::{self, Charset};
use crate::fonts::coverage::{LegacyCoverage, TmpCoverage};
use crate::fonts::font_file::{self, FontCheck};
use crate::game::folder;
use crate::game::scan::Ref;

// Font names often carry the script they cover, the only clue when a dynamic font has few characters yet.
const SCRIPT_HINTS: [(&str, &[&str]); 17] = [
    ("cjk", &["CJK"]),
    ("sc", &["CJK"]),
    ("tc", &["CJK"]),
    ("jp", &["CJK", "HIRAGANA", "KATAKANA"]),
    ("kr", &["HANGUL"]),
    ("thai", &["THAI"]),
    ("arabic", &["ARABIC"]),
    ("hebrew", &["HEBREW"]),
    ("bengali", &["BENGALI"]),
    ("devanagari", &["DEVANAGARI"]),
    ("tamil", &["TAMIL"]),
    ("georgian", &["GEORGIAN"]),
    ("armenian", &["ARMENIAN"]),
    ("khmer", &["KHMER"]),
    ("lao", &["LAO"]),
    ("myanmar", &["MYANMAR"]),
    ("sinhala", &["SINHALA"]),
];
pub const OTHER_SCRIPT_NOTE: &str =
    "font for another writing system, skipped; choose a font to rebake it anyway";
pub const MARKS_NOTE: &str =
    "has all your letters but lacks some marks, skipped; choose a font to add them";
pub const COMPLETE_NOTE: &str = "has all your letters and marks";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    #[default]
    #[serde(rename = "tmp")]
    Tmp,
    #[serde(rename = "font")]
    Legacy,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Place {
    pub file: String,
    pub path_id: i64,
    /// A bundle can hold several serialized files; empty matches any of them.
    #[serde(default)]
    pub cab: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub font: String,
    pub family: String,
    pub missing: String,
    pub source: String,
    pub note: String,
    pub places: Vec<Place>,
    pub skip: bool,
    pub suggested_source: String,
    pub suggested_skip: bool,
    pub kind: Kind,
    /// Dynamic TMP fonts that draw letters from this Font; build clears their cached letters.
    pub resets: Vec<Place>,
    /// How many texts in the game files use this font, so people start with the busiest one.
    pub texts: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skipped {
    pub font: String,
    pub reason: String,
    pub files: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub game: String,
    pub language: String,
    #[serde(default)]
    pub chars: Option<String>,
    pub fonts: Vec<Entry>,
    #[serde(default)]
    pub not_fixable: Vec<Skipped>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub path: String,
    pub name: String,
    pub family: String,
    pub missing: String,
    pub marks: String,
}

#[derive(Clone, Debug, PartialEq)]
struct Need {
    kind: Kind,
    name: String,
    family: String,
    letters: String,
    marks: String,
    file: PathBuf,
    path_id: i64,
    other_script: bool,
    cab: String,
    resets: Vec<Place>,
    texts: usize,
}

#[derive(Clone, Copy, Debug)]
pub enum Lacking<'a> {
    Tmp(&'a TmpCoverage),
    Legacy(&'a LegacyCoverage),
}

impl Lacking<'_> {
    fn reference(&self) -> &Ref {
        match self {
            Self::Tmp(item) => &item.font.reference,
            Self::Legacy(item) => &item.font.reference,
        }
    }
}

pub fn normalize(name: &str) -> String {
    let kept: String = name
        .to_lowercase()
        .chars()
        .filter(|ch| is_word(*ch))
        .collect();
    kept.strip_suffix("sdf")
        .map_or(kept.clone(), str::to_string)
}

fn is_word(ch: char) -> bool {
    is_alpha(ch)
        || matches!(
            get_general_category(ch),
            G::DecimalNumber | G::LetterNumber | G::OtherNumber
        )
}

fn is_alpha(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        G::UppercaseLetter
            | G::LowercaseLetter
            | G::TitlecaseLetter
            | G::ModifierLetter
            | G::OtherLetter
    )
}

/// `NotoSansCJK3` gives noto, sans, cjk, 3: script hints hide inside camel-case names.
pub fn split_words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.replace("CJK", " CJK ").chars().collect();
    let mut words = Vec::new();
    let mut idx = 0;
    while idx < chars.len() {
        let length = match_word(&chars[idx..]);
        if length == 0 {
            idx += 1;
            continue;
        }
        let word: String = chars[idx..idx + length].iter().collect();
        words.push(word.to_lowercase());
        idx += length;
    }
    words
}

fn match_word(chars: &[char]) -> usize {
    let count = |test: fn(&char) -> bool, from: usize| -> usize {
        chars[from.min(chars.len())..]
            .iter()
            .take_while(|ch| test(ch))
            .count()
    };
    let upper = count(char::is_ascii_uppercase, 0);
    let lower_after = chars.get(upper).is_some_and(char::is_ascii_lowercase);
    if upper > 0 && !lower_after {
        return upper;
    }
    if upper > 1 {
        return upper - 1;
    }
    let lower = count(char::is_ascii_lowercase, upper);
    if lower > 0 {
        return upper + lower;
    }
    count(|ch| get_general_category(*ch) == G::DecimalNumber, 0)
}

pub fn script_of(codepoint: u32) -> String {
    let name = char::from_u32(codepoint)
        .and_then(unicode_names2::name)
        .map_or_else(|| "UNKNOWN".to_string(), |name| name.to_string());
    name.split(' ').next().unwrap_or_default().to_string()
}

fn find_hinted_scripts(names: &[&str]) -> HashSet<&'static str> {
    names
        .iter()
        .flat_map(|name| split_words(name))
        .flat_map(|word| {
            SCRIPT_HINTS
                .iter()
                .filter(move |(hint, _)| *hint == word)
                .flat_map(|(_, scripts)| scripts.iter().copied())
        })
        .collect()
}

pub fn is_other_script(
    names: &[&str],
    codepoints: impl IntoIterator<Item = u32>,
    charset: &Charset,
) -> bool {
    let mut wanted: HashSet<String> = charset
        .letters
        .chars()
        .map(|ch| script_of(u32::from(ch)))
        .collect();
    wanted.insert("LATIN".to_string());
    let hinted = find_hinted_scripts(names);
    if !hinted.is_empty() && !hinted.iter().any(|script| wanted.contains(*script)) {
        return true;
    }
    let (inside, total) = codepoints
        .into_iter()
        .filter(|cp| char::from_u32(*cp).is_some_and(is_alpha))
        .fold((0, 0), |(inside, total), cp| {
            (
                inside + usize::from(wanted.contains(&script_of(cp))),
                total + 1,
            )
        });
    total - inside > inside
}

pub fn is_tmp_other_script(item: &TmpCoverage, charset: &Charset) -> bool {
    let family = item.font.family.as_deref().unwrap_or("");
    is_other_script(
        &[&item.font.name, family],
        item.font.codepoints.iter().copied(),
        charset,
    )
}

pub fn is_legacy_other_script(item: &LegacyCoverage, charset: &Charset) -> bool {
    let family = item
        .check
        .as_ref()
        .map_or("", |check| check.family.as_str());
    is_other_script(
        &[&item.font.name, family],
        read_cmap(&item.font.data),
        charset,
    )
}

fn read_cmap(data: &[u8]) -> Vec<u32> {
    font_file::load_cmap(data).map_or_else(|_| Vec::new(), |cmap| cmap.into_keys().collect())
}

pub fn to_candidate(item: &LegacyCoverage, path: &str) -> Option<Candidate> {
    let check = item.check.as_ref()?;
    if path.is_empty() {
        return None;
    }
    Some(Candidate {
        path: path.to_string(),
        name: item.font.name.clone(),
        family: format!("{} {}", check.family, check.style),
        missing: format!("{}{}{}", check.missing_letters, check.empty, check.aliased),
        marks: check.missing_typography.clone(),
    })
}

pub fn pick_candidate<'a>(
    font: &str,
    family: &str,
    candidates: &'a [Candidate],
) -> Option<&'a Candidate> {
    let wanted: HashSet<String> = [normalize(font), normalize(family)]
        .into_iter()
        .filter(|name| !name.is_empty())
        .collect();
    candidates
        .iter()
        .find(|c| wanted.contains(&normalize(&c.name)) || wanted.contains(&normalize(&c.family)))
}

pub fn describe_source(candidate: Option<&Candidate>) -> (String, String) {
    let Some(candidate) = candidate else {
        return (
            String::new(),
            "no matching font in the game: choose a TTF/OTF with your letters".to_string(),
        );
    };
    if candidate.missing.is_empty() {
        return (
            candidate.path.clone(),
            format!("game font {} has all your letters", candidate.name),
        );
    }
    (
        String::new(),
        format!(
            "game font {} lacks {}: add them in a font editor and choose the edited file, or choose another font",
            candidate.path,
            shorten(&candidate.missing)
        ),
    )
}

pub fn shorten(chars: &str) -> String {
    let count = chars.chars().count();
    if count <= 12 {
        return chars.to_string();
    }
    let head: String = chars.chars().take(8).collect();
    format!("{count} letters ({head}…)")
}

pub fn skip_reason(item: &TmpCoverage, raw_note: &str, shared: &HashSet<Ref>) -> Option<String> {
    let font = &item.font;
    if font.point_size.is_none() {
        return Some(format!("settings unreadable, {raw_note}"));
    }
    let atlases = font.atlases.as_deref().unwrap_or_default();
    let [atlas] = atlases else {
        return Some(format!(
            "{} atlas textures, ufk can rebake only fonts with one",
            atlases.len()
        ));
    };
    let Some(atlas) = atlas else {
        return Some("its atlas texture is missing, ufk cannot rebake it".to_string());
    };
    if atlas.0 != font.reference.0 {
        return Some("its atlas is in another game file, ufk cannot rebake it".to_string());
    }
    if shared.contains(atlas) {
        return Some("its atlas is shared with another font, ufk cannot rebake it".to_string());
    }
    None
}

/// Fonts in other game files can point at the same atlas, so references are compared across files.
pub fn find_shared_atlases(tmp: &[TmpCoverage]) -> HashSet<Ref> {
    let mut used: HashMap<&Ref, usize> = HashMap::new();
    for atlas in tmp
        .iter()
        .flat_map(|item| item.font.atlases.iter().flatten().flatten())
    {
        *used.entry(atlas).or_insert(0) += 1;
    }
    used.into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(atlas, _)| atlas.clone())
        .collect()
}

fn to_need(item: Lacking, charset: &Charset, resets: &[Place]) -> Need {
    match item {
        Lacking::Tmp(item) => Need {
            kind: Kind::Tmp,
            name: item.font.name.clone(),
            family: item.font.family.clone().unwrap_or_default(),
            letters: item.missing_letters.clone(),
            marks: item.missing_typography.clone(),
            file: item.font.file.clone(),
            path_id: item.font.reference.1,
            other_script: is_tmp_other_script(item, charset),
            cab: item.font.reference.0.clone(),
            resets: Vec::new(),
            texts: item.texts,
        },
        Lacking::Legacy(item) => {
            let check = item.check.as_ref();
            Need {
                kind: Kind::Legacy,
                name: item.font.name.clone(),
                family: check.map_or_else(String::new, |c| {
                    format!("{} {}", c.family, c.style).trim().to_string()
                }),
                letters: check.map_or_else(String::new, read_unusable),
                marks: check.map_or_else(String::new, |c| c.missing_typography.clone()),
                file: item.font.file.clone(),
                path_id: item.font.reference.1,
                other_script: is_legacy_other_script(item, charset),
                cab: item.font.reference.0.clone(),
                resets: resets.to_vec(),
                texts: item.texts,
            }
        }
    }
}

pub fn group_entries(
    fixable: &[Lacking],
    candidates: &[Candidate],
    data_dir: &Path,
    charset: &Charset,
    resets: &HashMap<Ref, Vec<Place>>,
) -> Vec<Entry> {
    let mut groups: IndexMap<(Kind, String, String), Vec<Need>> = IndexMap::new();
    for item in fixable {
        let places = resets.get(item.reference()).map_or(&[][..], Vec::as_slice);
        let need = to_need(*item, charset, places);
        groups
            .entry((need.kind, need.name.clone(), need.family.clone()))
            .or_default()
            .push(need);
    }
    let mut entries: Vec<Entry> = groups
        .values()
        .map(|needs| build_entry(needs, candidates, data_dir))
        .collect();
    entries.sort_by_key(|entry| -entry.texts);
    entries
}

fn build_entry(needs: &[Need], candidates: &[Candidate], data_dir: &Path) -> Entry {
    let first = needs
        .iter()
        .find(|need| !need.letters.is_empty())
        .unwrap_or(&needs[0]);
    let marks_only = first.letters.is_empty();
    let complete = needs
        .iter()
        .all(|need| need.letters.is_empty() && need.marks.is_empty());
    let picked = (!first.other_script)
        .then(|| pick_candidate(&first.name, &first.family, candidates))
        .flatten();
    let marks: String = needs
        .iter()
        .flat_map(|need| need.marks.chars())
        .collect::<IndexSet<char>>()
        .into_iter()
        .collect();
    let letters: String = needs
        .iter()
        .flat_map(|need| need.letters.chars())
        .collect::<IndexSet<char>>()
        .into_iter()
        .collect();
    let lacking_marks: String = marks
        .chars()
        .filter(|ch| picked.is_some_and(|c| c.marks.contains(*ch)))
        .collect();
    let candidate = picked.map(|c| match marks_only && !lacking_marks.is_empty() {
        true => Candidate {
            missing: c.missing.clone() + &lacking_marks,
            ..c.clone()
        },
        false => c.clone(),
    });
    let (source, note) = describe_source(candidate.as_ref());
    let skip_note = match (first.other_script, complete, marks_only) {
        (true, _, _) => Some(OTHER_SCRIPT_NOTE),
        (false, true, _) => Some(COMPLETE_NOTE),
        (false, false, true) => Some(MARKS_NOTE),
        _ => None,
    };
    let source = if first.other_script || complete {
        String::new()
    } else {
        source
    };
    Entry {
        font: first.name.clone(),
        family: first.family.clone(),
        missing: if marks_only { marks } else { letters },
        source: source.clone(),
        note: skip_note.map_or(note, str::to_string),
        places: needs
            .iter()
            .map(|need| Place {
                file: folder::format_relative(&need.file, data_dir),
                path_id: need.path_id,
                cab: need.cab.clone(),
            })
            .collect(),
        skip: skip_note.is_some(),
        suggested_source: source,
        suggested_skip: skip_note.is_some(),
        kind: first.kind,
        resets: needs
            .iter()
            .flat_map(|need| need.resets.iter().cloned())
            .collect::<IndexSet<Place>>()
            .into_iter()
            .collect(),
        texts: needs.iter().map(|need| need.texts as i64).sum(),
    }
}

/// Blank letters and letters drawn with another letter's shape need a new font as much as missing ones.
pub fn read_unusable(check: &FontCheck) -> String {
    format!("{}{}{}", check.missing_letters, check.empty, check.aliased)
        .chars()
        .collect::<IndexSet<char>>()
        .into_iter()
        .collect()
}

pub fn legacy_skip_reason(item: &LegacyCoverage) -> Option<String> {
    if item.font.data.is_empty() {
        return Some(
            "legacy Font without font data, the game takes it from the system".to_string(),
        );
    }
    if !item.font.dynamic {
        return Some(
            "legacy Font with letters drawn in advance, ufk cannot replace it".to_string(),
        );
    }
    if item.check.is_none() {
        let error = item.error.as_deref().unwrap_or("None");
        return Some(format!("legacy Font with unreadable font data, {error}"));
    }
    None
}

fn group_skipped(rows: Vec<(String, String, String)>) -> Vec<Skipped> {
    let mut groups: IndexMap<(String, String), Vec<Value>> = IndexMap::new();
    for (font, reason, file) in rows {
        groups
            .entry((font, reason))
            .or_default()
            .push(Value::String(file));
    }
    groups
        .into_iter()
        .map(|((font, reason), files)| Skipped {
            font,
            reason,
            files,
        })
        .collect()
}

pub fn find_drawn_from_font(
    tmp: &[TmpCoverage],
    legacy: &[LegacyCoverage],
    data_dir: &Path,
) -> HashMap<Ref, Vec<Place>> {
    let usable: HashSet<&Ref> = legacy
        .iter()
        .filter(|item| legacy_skip_reason(item).is_none())
        .map(|item| &item.font.reference)
        .collect();
    let mut drawn: HashMap<Ref, Vec<Place>> = HashMap::new();
    for item in tmp {
        let Some(source) = &item.font.source_font else {
            continue;
        };
        if item.font.dynamic != Some(true) || !usable.contains(source) {
            continue;
        }
        drawn.entry(source.clone()).or_default().push(Place {
            file: folder::format_relative(&item.font.file, data_dir),
            path_id: item.font.reference.1,
            cab: item.font.reference.0.clone(),
        });
    }
    drawn
}

/// A dynamic font draws new letters from its Font while the game runs, so fixing that Font fixes it.
fn is_drawn_from_font(item: &TmpCoverage, drawn: &HashMap<Ref, Vec<Place>>) -> bool {
    item.font.dynamic == Some(true)
        && item
            .font
            .source_font
            .as_ref()
            .is_some_and(|source| drawn.contains_key(source))
}

fn add_drawn_texts(
    legacy: Vec<LegacyCoverage>,
    tmp: &[TmpCoverage],
    drawn: &HashMap<Ref, Vec<Place>>,
) -> Vec<LegacyCoverage> {
    let drawing: Vec<&TmpCoverage> = tmp
        .iter()
        .filter(|item| is_drawn_from_font(item, drawn))
        .collect();
    legacy
        .into_iter()
        .map(|item| {
            let extra: usize = drawing
                .iter()
                .filter(|t| t.font.source_font.as_ref() == Some(&item.font.reference))
                .map(|t| t.texts)
                .sum();
            LegacyCoverage {
                texts: item.texts + extra,
                ..item
            }
        })
        .collect()
}

fn is_lacking_tmp(item: &TmpCoverage) -> bool {
    !item.missing_letters.is_empty() || !item.missing_typography.is_empty()
}

fn is_lacking_legacy(item: &LegacyCoverage) -> bool {
    if item.font.data.is_empty() {
        return false;
    }
    item.check.as_ref().is_none_or(|check| {
        !read_unusable(check).is_empty() || !check.missing_typography.is_empty()
    })
}

/// Where each legacy Font's file was saved, by game file and reference.
pub type Extracted = HashMap<(PathBuf, Ref), String>;

#[derive(Debug)]
pub struct Scanned<'a> {
    pub game_path: &'a Path,
    pub data_dir: &'a Path,
    pub extracted: &'a Extracted,
    pub raw_note: &'a str,
    pub tmp: Vec<TmpCoverage>,
    pub legacy: Vec<LegacyCoverage>,
}

pub fn make_plan(scanned: Scanned, charset: &Charset, charset_spec: (&str, Option<&str>)) -> Plan {
    let Scanned {
        game_path,
        data_dir,
        extracted,
        raw_note,
        tmp,
        legacy,
    } = scanned;
    let candidates: Vec<Candidate> = legacy
        .iter()
        .filter_map(|item| {
            let key = (item.font.file.clone(), item.font.reference.clone());
            to_candidate(item, extracted.get(&key).map_or("", String::as_str))
        })
        .collect();
    let drawn = find_drawn_from_font(&tmp, &legacy, data_dir);
    let legacy = add_drawn_texts(legacy, &tmp, &drawn);
    let tmp: Vec<TmpCoverage> = tmp
        .into_iter()
        .filter(|item| !is_drawn_from_font(item, &drawn))
        .collect();
    let shared = find_shared_atlases(&tmp);
    let lacking: Vec<&TmpCoverage> = tmp.iter().filter(|item| is_lacking_tmp(item)).collect();
    let relative = |file: &Path| folder::format_relative(file, data_dir);
    // Fonts that already have every letter are listed too: a translator may still swap them.
    let fixable = tmp
        .iter()
        .filter(|item| skip_reason(item, raw_note, &shared).is_none())
        .map(Lacking::Tmp);
    let unfixable_tmp = lacking.iter().filter_map(|item| {
        let reason = skip_reason(item, raw_note, &shared)?;
        let other = is_tmp_other_script(item, charset);
        (!other).then(|| (item.font.name.clone(), reason, relative(&item.font.file)))
    });
    let lacking_legacy: Vec<&LegacyCoverage> = legacy
        .iter()
        .filter(|item| is_lacking_legacy(item))
        .collect();
    let fixable_legacy = legacy
        .iter()
        .filter(|item| !item.font.data.is_empty() && legacy_skip_reason(item).is_none())
        .map(Lacking::Legacy);
    let unfixable_legacy = lacking_legacy.iter().filter_map(|item| {
        let reason = legacy_skip_reason(item)?;
        let other = item.check.is_some() && is_legacy_other_script(item, charset);
        (!other).then(|| (item.font.name.clone(), reason, relative(&item.font.file)))
    });
    let fixable: Vec<Lacking> = fixable.chain(fixable_legacy).collect();
    let unfixable: Vec<_> = unfixable_tmp.chain(unfixable_legacy).collect();
    let (language, chars) = charset_spec;
    Plan {
        game: game_path.display().to_string(),
        language: language.to_string(),
        chars: chars.map(str::to_string),
        fonts: group_entries(&fixable, &candidates, data_dir, charset, &drawn),
        not_fixable: group_skipped(unfixable),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontKey {
    pub kind: Kind,
    pub font: String,
    pub family: String,
}

impl FontKey {
    pub fn of(entry: &Entry) -> FontKey {
        FontKey {
            kind: entry.kind,
            font: entry.font.clone(),
            family: entry.family.clone(),
        }
    }
}

pub fn entry_key(entry: &Entry) -> (Kind, &str, &str) {
    (entry.kind, &entry.font, &entry.family)
}

pub fn keep_user_choices(new: Plan, old: &Plan) -> Plan {
    let by_key: HashMap<_, &Entry> = old
        .fonts
        .iter()
        .map(|entry| (entry_key(entry), entry))
        .collect();
    let fonts = new
        .fonts
        .iter()
        .map(|entry| merge_choice(entry, by_key.get(&entry_key(entry)).copied()))
        .collect();
    Plan { fonts, ..new }
}

pub fn merge_choice(entry: &Entry, previous: Option<&Entry>) -> Entry {
    let Some(previous) = previous else {
        return entry.clone();
    };
    let chose_source = previous.source != previous.suggested_source;
    let chose_skip = previous.skip != previous.suggested_skip;
    if !chose_source && !chose_skip {
        return entry.clone();
    }
    Entry {
        source: if chose_source {
            previous.source.clone()
        } else {
            entry.source.clone()
        },
        skip: if chose_skip {
            previous.skip
        } else {
            entry.skip
        },
        note: "kept your choice from the last scan".to_string(),
        ..entry.clone()
    }
}

pub fn to_json(plan: &Plan) -> Result<String> {
    Ok(serde_json::to_string_pretty(plan)?)
}

pub fn load(path: &Path) -> Result<Plan> {
    let text = charsets::read_text(path)?;
    serde_json::from_str(&text).with_context(|| format!("{} is not a ufk plan", path.display()))
}

pub fn read_charset(plan: &Plan, plan_path: &Path) -> Result<Charset> {
    match &plan.chars {
        Some(chars) if !chars.is_empty() => {
            charsets::load(&plan_path.parent().unwrap_or(Path::new("")).join(chars))
        }
        _ => charsets::get(&plan.language),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::game::scan::{LegacyFont, TmpFont};

    const DATA: &str = "/game/Demo_Data";

    fn uk() -> Charset {
        charsets::get("uk").unwrap()
    }

    fn tmp_font(name: &str, family: &str, file: &str, path_id: i64) -> TmpCoverage {
        let font = TmpFont {
            file: Path::new(DATA).join(file),
            reference: ("CAB-1".to_string(), path_id),
            name: name.to_string(),
            codepoints: "ABC".chars().map(u32::from).collect::<BTreeSet<u32>>(),
            fallbacks: Vec::new(),
            family: Some(family.to_string()),
            point_size: Some(30.into()),
            padding: Some(3),
            atlas: Some((512, 512)),
            render_mode: None,
            dynamic: Some(false),
            atlases: None,
            source: "typetree".to_string(),
            source_font: None,
        };
        TmpCoverage {
            font,
            missing_letters: "ҐЄ".to_string(),
            missing_typography: String::new(),
            missing_letters_with_fallbacks: "ҐЄ".to_string(),
            chain: Vec::new(),
            texts: 0,
        }
    }

    fn candidate(name: &str, family: &str) -> Candidate {
        Candidate {
            path: format!("fonts/{name}.ttf"),
            name: name.to_string(),
            family: family.to_string(),
            missing: String::new(),
            marks: String::new(),
        }
    }

    fn check(empty: &str, aliased: &str, missing: &str) -> FontCheck {
        FontCheck {
            family: "Body".to_string(),
            style: "Regular".to_string(),
            glyphs: 100,
            missing_letters: missing.to_string(),
            missing_typography: String::new(),
            empty: empty.to_string(),
            aliased: aliased.to_string(),
        }
    }

    fn legacy_font(file: &str, path_id: i64) -> LegacyFont {
        LegacyFont {
            file: Path::new(DATA).join(file),
            reference: ("CAB-1".to_string(), path_id),
            name: "Body".to_string(),
            data: b"x".to_vec(),
            os_names: Vec::new(),
            dynamic: true,
        }
    }

    fn group(items: &[TmpCoverage], candidates: &[Candidate]) -> Vec<Entry> {
        let lacking: Vec<Lacking> = items.iter().map(Lacking::Tmp).collect();
        group_entries(
            &lacking,
            candidates,
            Path::new(DATA),
            &uk(),
            &HashMap::new(),
        )
    }

    fn entry(font: &str, family: &str, missing: &str, source: &str, places: &[Place]) -> Entry {
        Entry {
            font: font.to_string(),
            family: family.to_string(),
            missing: missing.to_string(),
            source: source.to_string(),
            note: String::new(),
            places: places.to_vec(),
            skip: false,
            suggested_source: String::new(),
            suggested_skip: false,
            kind: Kind::Tmp,
            resets: Vec::new(),
            texts: 0,
        }
    }

    fn plan_of(fonts: Vec<Entry>) -> Plan {
        Plan {
            game: "game".to_string(),
            language: "uk".to_string(),
            chars: None,
            fonts,
            not_fixable: Vec::new(),
        }
    }

    fn place(file: &str, path_id: i64) -> Place {
        Place {
            file: file.to_string(),
            path_id,
            cab: String::new(),
        }
    }

    fn make(tmp: Vec<TmpCoverage>, legacy: Vec<LegacyCoverage>) -> Plan {
        let data = Path::new(DATA);
        let parent = data.parent().unwrap();
        let extracted = Extracted::new();
        let scanned = Scanned {
            game_path: parent,
            data_dir: data,
            extracted: &extracted,
            raw_note: "",
            tmp,
            legacy,
        };
        make_plan(scanned, &uk(), ("uk", None))
    }

    #[test]
    fn source_font_must_match_name_or_family_exactly() {
        let candidates = [
            candidate("NotoSansThai-Regular", "Noto Sans Thai Regular"),
            candidate("NotoSerif-Regular", "Noto Serif Regular"),
        ];
        assert!(pick_candidate("NotoSans-Regular SDF", "Noto Sans Regular", &candidates).is_none());
        let picked = pick_candidate("NotoSerif-Regular SDF", "Noto Serif Regular", &candidates);
        assert_eq!(picked.unwrap().name, "NotoSerif-Regular");
    }

    #[test]
    fn fonts_for_other_writing_systems_are_recognised_by_name_or_characters() {
        let cps = |text: &str| text.chars().map(u32::from).collect::<Vec<_>>();
        assert!(is_other_script(
            &["NotoSansThai-Regular SDF"],
            cps("ABC"),
            &uk()
        ));
        assert!(is_other_script(&["Body SDF"], cps("漢字仮名AB"), &uk()));
        assert!(!is_other_script(&["Body SDF"], cps("ABCабв"), &uk()));
        let japanese = Charset {
            name: "ja.txt".to_string(),
            letters: "漢字".to_string(),
            typography: String::new(),
            lookalikes: Vec::new(),
        };
        assert!(!is_other_script(
            &["NotoSansCJK-Regular SDF"],
            cps("漢字"),
            &japanese
        ));
    }

    #[test]
    fn copies_of_one_font_become_one_entry_with_all_places() {
        let fonts = [
            tmp_font("Body SDF", "Body Regular", "fonts", 1),
            tmp_font("Body SDF", "Body Regular", "sharedassets0.assets", 2),
        ];
        let entries = group(&fonts, &[]);
        let files: Vec<(&str, Vec<&str>)> = entries
            .iter()
            .map(|e| {
                (
                    e.font.as_str(),
                    e.places.iter().map(|p| p.file.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(files, [("Body SDF", vec!["fonts", "sharedassets0.assets"])]);
    }

    #[test]
    fn cjk_font_names_are_recognised_inside_camel_case() {
        let cps = "AB".chars().map(u32::from);
        assert!(is_other_script(&["NotoSansCJKsc-Regular SDF"], cps, &uk()));
    }

    #[test]
    fn game_font_with_blank_or_aliased_letters_is_not_offered_as_source() {
        let item = LegacyCoverage {
            font: legacy_font("a.assets", 1),
            check: Some(check("Ж", "", "")),
            error: None,
            texts: 0,
        };
        let found = to_candidate(&item, "fonts/Body.ttf");
        assert_eq!(describe_source(found.as_ref()).0, "");
    }

    #[test]
    fn replan_keeps_user_sources_and_skips_but_not_old_auto_choices() {
        let places = [place("fonts", 1)];
        let auto = Entry {
            note: "game font has all".to_string(),
            suggested_source: "fonts/Body.ttf".to_string(),
            ..entry("Body SDF", "Body", "ҐЄ", "fonts/Body.ttf", &places)
        };
        let old_auto = plan_of(vec![Entry {
            skip: true,
            suggested_skip: true,
            ..auto.clone()
        }]);
        let old_user = plan_of(vec![Entry {
            source: "my.ttf".to_string(),
            skip: true,
            ..auto.clone()
        }]);
        let new = plan_of(vec![Entry {
            source: "fonts/Other.ttf".to_string(),
            suggested_source: "fonts/Other.ttf".to_string(),
            ..auto
        }]);
        let auto_kept = keep_user_choices(new.clone(), &old_auto).fonts.remove(0);
        let user_kept = keep_user_choices(new, &old_user).fonts.remove(0);
        assert_eq!(
            (auto_kept.source.as_str(), auto_kept.skip),
            ("fonts/Other.ttf", false)
        );
        assert_eq!(
            (user_kept.source.as_str(), user_kept.skip),
            ("my.ttf", true)
        );
    }

    #[test]
    fn plan_lists_the_fonts_used_by_most_texts_first() {
        let rare = TmpCoverage {
            texts: 3,
            ..tmp_font("Rare SDF", "Rare", "sharedassets0.assets", 1)
        };
        let body = TmpCoverage {
            texts: 5,
            ..tmp_font("Body SDF", "Body", "sharedassets0.assets", 2)
        };
        let body_copy = TmpCoverage {
            texts: 4,
            ..tmp_font("Body SDF", "Body", "sharedassets1.assets", 2)
        };
        let entries = group(&[rare, body, body_copy], &[]);
        let order: Vec<(&str, i64)> = entries.iter().map(|e| (e.font.as_str(), e.texts)).collect();
        assert_eq!(order, [("Body SDF", 9), ("Rare SDF", 3)]);
    }

    #[test]
    fn fonts_whose_atlas_ufk_cannot_replace_are_not_fixable() {
        let with_atlases = |path_id: i64, atlases: Vec<Option<(&str, i64)>>, cab: &str| {
            let mut item = tmp_font(
                &format!("Font{path_id} SDF"),
                "Font",
                "sharedassets0.assets",
                path_id,
            );
            item.font.reference = (cab.to_string(), path_id);
            item.font.atlases = Some(
                atlases
                    .into_iter()
                    .map(|a| a.map(|(c, id)| (c.to_string(), id)))
                    .collect(),
            );
            item
        };
        let fonts = [
            with_atlases(1, vec![Some(("CAB-1", 10))], "CAB-1"),
            with_atlases(2, vec![Some(("CAB-1", 20))], "CAB-1"),
            with_atlases(3, vec![Some(("CAB-1", 20))], "CAB-1"),
            with_atlases(4, vec![Some(("CAB-2", 30))], "CAB-1"),
            with_atlases(5, vec![Some(("CAB-1", 40)), None], "CAB-1"),
            with_atlases(6, vec![Some(("CAB-1", 50))], "CAB-1"),
            with_atlases(7, vec![Some(("CAB-1", 50))], "CAB-3"),
            with_atlases(8, vec![None], "CAB-1"),
        ];
        let shared = find_shared_atlases(&fonts);
        let reasons: Vec<String> = fonts
            .iter()
            .map(|item| skip_reason(item, "", &shared).unwrap_or_default())
            .collect();
        assert_eq!(reasons[0], "");
        assert!([1, 2, 5].iter().all(|i| reasons[*i].contains("shared")));
        assert!(reasons[3].contains("another game file"));
        assert!(reasons[4].starts_with("2 atlas textures"));
        assert!(reasons[7].contains("missing"));
    }

    fn marks_only(marks: &str) -> TmpCoverage {
        TmpCoverage {
            missing_letters: String::new(),
            missing_typography: marks.to_string(),
            ..tmp_font("Body SDF", "Body Regular", "sharedassets0.assets", 1)
        }
    }

    #[test]
    fn font_lacking_only_marks_is_listed_but_skipped() {
        let entry = group(&[marks_only("№")], &[]).remove(0);
        assert_eq!(
            (
                entry.missing.as_str(),
                entry.skip,
                entry.suggested_skip,
                entry.note.as_str()
            ),
            ("№", true, true, MARKS_NOTE)
        );
    }

    #[test]
    fn replan_takes_the_new_skip_suggestion_when_only_the_source_was_chosen() {
        let item = tmp_font("Body SDF", "Body Regular", "sharedassets0.assets", 1);
        let old = Entry {
            source: "C:/Fonts/my.ttf".to_string(),
            ..group(&[item], &[]).remove(0)
        };
        let merged = merge_choice(&group(&[marks_only("№")], &[])[0], Some(&old));
        assert_eq!(
            (merged.source.as_str(), merged.skip),
            ("C:/Fonts/my.ttf", true)
        );
    }

    #[test]
    fn marks_only_entry_does_not_suggest_a_game_font_lacking_those_marks() {
        let lacking = Candidate {
            marks: "№«»".to_string(),
            ..candidate("Body", "Body Regular")
        };
        let having = Candidate {
            marks: "«»".to_string(),
            ..candidate("Body", "Body Regular")
        };
        assert_eq!(group(&[marks_only("№")], &[lacking])[0].source, "");
        assert_eq!(
            group(&[marks_only("№")], &[having])[0].source,
            "fonts/Body.ttf"
        );
    }

    #[test]
    fn marks_only_entry_checks_the_marks_of_every_copy() {
        let lacking_quote = Candidate {
            marks: "«".to_string(),
            ..candidate("Body", "Body Regular")
        };
        let entry = group(&[marks_only("№"), marks_only("«")], &[lacking_quote]).remove(0);
        assert_eq!((entry.missing.as_str(), entry.source.as_str()), ("№«", ""));
    }

    #[test]
    fn replan_keeps_choices_apart_for_a_tmp_font_and_a_font_with_one_name() {
        let places = [place("resources.assets", 1)];
        let tmp = entry("Body", "Body", "Ґ", "", &places);
        let font = Entry {
            kind: Kind::Legacy,
            ..tmp.clone()
        };
        let old = plan_of(vec![
            Entry {
                source: "tmp.ttf".to_string(),
                ..tmp.clone()
            },
            Entry {
                source: "font.ttf".to_string(),
                ..font.clone()
            },
        ]);
        let kept = keep_user_choices(plan_of(vec![tmp, font]), &old).fonts;
        let pairs: Vec<(Kind, &str)> = kept.iter().map(|e| (e.kind, e.source.as_str())).collect();
        assert_eq!(pairs, [(Kind::Tmp, "tmp.ttf"), (Kind::Legacy, "font.ttf")]);
    }

    #[test]
    fn font_whose_letters_are_blank_or_drawn_as_others_gets_an_entry() {
        let item = LegacyCoverage {
            font: legacy_font("a.assets", 1),
            check: Some(check("", "Ґ", "")),
            error: None,
            texts: 0,
        };
        let made = make(Vec::new(), vec![item]);
        let found: Vec<(Kind, &str)> = made
            .fonts
            .iter()
            .map(|e| (e.kind, e.missing.as_str()))
            .collect();
        assert_eq!(found, [(Kind::Legacy, "Ґ")]);
    }

    fn dynamic_font(source: Option<i64>) -> TmpCoverage {
        let mut item = tmp_font("Body SDF", "Body", "resources.assets", 5);
        item.font.dynamic = Some(true);
        item.font.source_font = source.map(|id| ("CAB-1".to_string(), id));
        item.font.atlases = Some(vec![Some(("CAB-1".to_string(), 50))]);
        item
    }

    fn body_font(missing: &str) -> LegacyCoverage {
        LegacyCoverage {
            font: legacy_font("resources.assets", 9),
            check: Some(check("", "", missing)),
            error: None,
            texts: 0,
        }
    }

    #[test]
    fn dynamic_font_is_fixed_through_its_source_font_or_rebaked_without_one() {
        let fix = |tmp: Vec<TmpCoverage>, legacy: Vec<LegacyCoverage>| {
            make(tmp, legacy)
                .fonts
                .into_iter()
                .map(|e| {
                    let resets: Vec<(String, i64)> =
                        e.resets.into_iter().map(|p| (p.file, p.path_id)).collect();
                    (e.kind, e.font, resets)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            fix(vec![dynamic_font(Some(9))], vec![body_font("Ґ")]),
            [(
                Kind::Legacy,
                "Body".to_string(),
                vec![("resources.assets".to_string(), 5)]
            )]
        );
        let complete = make(vec![dynamic_font(Some(9))], vec![body_font("")]).fonts;
        assert_eq!(
            complete
                .iter()
                .map(|e| (e.kind, e.skip, e.note.as_str()))
                .collect::<Vec<_>>(),
            [(Kind::Legacy, true, COMPLETE_NOTE)]
        );
        assert_eq!(
            fix(vec![dynamic_font(None)], Vec::new()),
            [(Kind::Tmp, "Body SDF".to_string(), Vec::new())]
        );
    }

    #[test]
    fn a_font_counts_the_texts_of_the_dynamic_fonts_drawn_from_it() {
        let drawing = TmpCoverage {
            texts: 200,
            ..dynamic_font(Some(9))
        };
        let made = make(vec![drawing], vec![body_font("Ґ")]);
        assert_eq!(made.fonts[0].texts, 200);
    }

    #[test]
    fn replan_keeps_choices_apart_for_fonts_with_one_name_and_two_families() {
        let places = [place("resources.assets", 1)];
        let serif = Entry {
            kind: Kind::Legacy,
            ..entry("Body", "Serif", "Ґ", "", &places)
        };
        let sans = Entry {
            family: "Sans".to_string(),
            ..serif.clone()
        };
        let old = plan_of(vec![
            Entry {
                source: "serif.ttf".to_string(),
                ..serif.clone()
            },
            Entry {
                source: "sans.ttf".to_string(),
                ..sans.clone()
            },
        ]);
        let kept = keep_user_choices(plan_of(vec![serif, sans]), &old).fonts;
        let sources: Vec<&str> = kept.iter().map(|e| e.source.as_str()).collect();
        assert_eq!(sources, ["serif.ttf", "sans.ttf"]);
    }
}
