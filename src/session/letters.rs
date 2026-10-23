use std::path::Path;

use anyhow::{Result, bail};

use crate::fonts::charsets::{self, Charset};
use crate::work::plan::{self, Plan};
use crate::work::project::{self, Project};

use super::GameView;

pub const PRESETS: [&str; 2] = charsets::LANGUAGES;
const CUSTOM: &str = "custom";
pub(crate) const LETTERS_FILE: &str = "letters.txt";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Letters {
    pub preset: String,
    pub letters: String,
    pub marks: String,
}

/// People type every character into one field; letters (and what a language counts as one, like ’)
/// must be in a font, everything else (₴, «», №) is a mark that only gets noted.
#[must_use]
pub fn split_text(text: &str, preset: &str) -> Letters {
    let language_letters: String = PRESETS
        .iter()
        .filter_map(|name| charsets::get(name).ok())
        .map(|charset| charset.letters)
        .collect();
    let chars = charsets::collect_chars(text);
    let is_letter = |ch: &char| ch.is_alphabetic() || language_letters.contains(*ch);
    Letters {
        preset: preset.to_string(),
        letters: chars.chars().filter(is_letter).collect(),
        marks: chars.chars().filter(|ch| !is_letter(ch)).collect(),
    }
}

#[must_use]
pub fn join_text(letters: &Letters) -> String {
    format!("{}{}", letters.letters, letters.marks)
}

/// The custom letters stay in their file after a switch to a preset, so Custom can offer them again.
pub fn read_custom_letters(view: &GameView) -> Option<String> {
    let work = super::state::open_work(&view.home, &view.record).ok()?;
    let text = std::fs::read_to_string(work.work.join(LETTERS_FILE)).ok()?;
    Some(charsets::collect_chars(&text))
}

pub fn letters_from_preset(name: &str) -> Result<Letters> {
    let charset = charsets::get(name)?;
    Ok(Letters {
        preset: charset.name,
        letters: charset.letters,
        marks: charset.typography,
    })
}

pub fn to_charset(letters: &Letters) -> Result<Charset> {
    if let Some(preset) = find_preset(letters) {
        return Ok(preset);
    }
    let chars = charsets::collect_chars(&letters.letters);
    if chars.is_empty() {
        bail!("Type the letters the game needs");
    }
    let marks = charsets::collect_chars(&letters.marks)
        .chars()
        .filter(|ch| !chars.contains(*ch))
        .collect();
    Ok(Charset {
        name: LETTERS_FILE.to_string(),
        letters: chars,
        typography: marks,
        lookalikes: Vec::new(),
    })
}

/// A preset counts only while its letters are untouched; its lookalike checks fit only its own letters.
fn find_preset(letters: &Letters) -> Option<Charset> {
    let preset = charsets::get(&letters.preset).ok()?;
    let same = charsets::collect_chars(&letters.letters) == preset.letters
        && charsets::collect_chars(&letters.marks) == preset.typography;
    same.then_some(preset)
}

fn to_custom(charset: &Charset) -> Letters {
    Letters {
        preset: CUSTOM.to_string(),
        letters: charset.letters.clone(),
        marks: charset.typography.clone(),
    }
}

pub(crate) fn read_letters(plan: Option<&Plan>, plan_path: &Path) -> Result<Letters> {
    let Some(plan) = plan else {
        return letters_from_preset(PRESETS[0]);
    };
    let charset = plan::read_charset(plan, plan_path)?;
    if plan.chars.as_deref().unwrap_or("").is_empty() {
        return Ok(Letters {
            preset: charset.name,
            letters: charset.letters,
            marks: charset.typography,
        });
    }
    Ok(to_custom(&charset))
}

pub(crate) fn store_letters(work: &Project, charset: &Charset) -> Result<Option<String>> {
    if charsets::get(&charset.name).is_ok() {
        return Ok(None);
    }
    let text = format!("{}\n\n{}\n", charset.letters, charset.typography);
    project::write_text(&work.work.join(LETTERS_FILE), &text)?;
    let folder = work.work.file_name().unwrap_or_default().to_string_lossy();
    Ok(Some(format!("{folder}/{LETTERS_FILE}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_field_splits_into_letters_and_marks_keeping_the_apostrophe_a_letter() {
        let split = split_text("Аб’₴«Аб", "custom");
        assert_eq!(
            (split.letters.as_str(), split.marks.as_str()),
            ("Аб’", "₴«")
        );
    }
}
