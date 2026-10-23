use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use unicode_general_category::{GeneralCategory as G, get_general_category};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Charset {
    pub name: String,
    pub letters: String,
    pub typography: String,
    /// Pairs of a letter and the letter a font may draw in its place, such as Ґ and Г.
    pub lookalikes: Vec<(char, char)>,
}

impl Charset {
    #[must_use]
    pub fn all(&self) -> String {
        format!("{}{}", self.letters, self.typography)
    }
}

pub const LANGUAGES: [&str; 2] = ["uk", "pl"];

pub fn get(lang: &str) -> Result<Charset> {
    match lang {
        "uk" => Ok(ukrainian()),
        "pl" => Ok(polish()),
        _ => bail!("Unknown language '{lang}', known: {}", LANGUAGES.join(", ")),
    }
}

fn ukrainian() -> Charset {
    Charset {
        name: "uk".to_string(),
        // The apostrophe is part of Ukrainian words, so a font without it cannot show the text.
        letters: "АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯабвгґдеєжзиіїйклмнопрстуфхцчшщьюя’".to_string(),
        typography: "«»„“—–…№".to_string(),
        lookalikes: vec![
            ('Ґ', 'Г'),
            ('ґ', 'г'),
            ('Ї', 'І'),
            ('ї', 'і'),
            ('Є', 'Е'),
            ('є', 'е'),
        ],
    }
}

fn polish() -> Charset {
    Charset {
        name: "pl".to_string(),
        letters: "ĄĆĘŁŃÓŚŹŻąćęłńóśźż".to_string(),
        typography: "„”«»—–…".to_string(),
        lookalikes: Vec::new(),
    }
}

pub fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let text = bytes.strip_prefix("\u{feff}".as_bytes()).unwrap_or(&bytes);
    match std::str::from_utf8(text) {
        Ok(text) => Ok(text.to_string()),
        Err(_) => bail!(
            "{} is not UTF-8: in Notepad use Save as, Encoding UTF-8",
            file_name(path)
        ),
    }
}

/// Letters, then a blank line, then marks, so a report can tell them apart for any language.
pub fn load(path: &Path) -> Result<Charset> {
    let text = read_text(path)?;
    let (first, rest) = split_at_blank_line(text.trim_matches(is_space));
    let letters = collect_chars(first);
    if letters.is_empty() {
        bail!("{} has no characters", file_name(path));
    }
    let marks = collect_chars(rest)
        .chars()
        .filter(|ch| !letters.contains(*ch))
        .collect();
    Ok(Charset {
        name: file_name(path),
        letters,
        typography: marks,
        lookalikes: Vec::new(),
    })
}

fn split_at_blank_line(text: &str) -> (&str, &str) {
    for (start, _) in text.match_indices('\n') {
        let after = &text[start + 1..];
        let gap = after.len() - after.trim_start_matches(is_space).len();
        if let Some(end) = after[..gap].rfind('\n') {
            return (&text[..start], &after[end + 1..]);
        }
    }
    (text, "")
}

#[must_use]
pub fn collect_chars(text: &str) -> String {
    let mut seen = HashSet::new();
    text.chars()
        .filter(|ch| !is_space(*ch) && seen.insert(*ch))
        .collect()
}

fn is_space(ch: char) -> bool {
    ch.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&ch)
}

#[must_use]
pub fn find_missing(have: &HashSet<u32>, wanted: &str) -> String {
    wanted
        .chars()
        .filter(|ch| !have.contains(&u32::from(*ch)))
        .collect()
}

#[must_use]
pub fn format_chars(chars: &str) -> String {
    chars
        .chars()
        .map(|ch| {
            if is_visible(ch) {
                ch.to_string()
            } else {
                format!("U+{:04X}", u32::from(ch))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_visible(ch: char) -> bool {
    !matches!(
        get_general_category(ch),
        G::Control
            | G::Format
            | G::Unassigned
            | G::PrivateUse
            | G::Surrogate
            | G::SpaceSeparator
            | G::LineSeparator
            | G::ParagraphSeparator
    )
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn letters_file_replaces_the_built_in_language() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "polish.txt", "\u{feff}ĄĆ ąć\nĄ".as_bytes());
        let charset = load(&path).unwrap();
        assert_eq!(
            (
                charset.name.as_str(),
                charset.letters.as_str(),
                charset.typography.as_str()
            ),
            ("polish.txt", "ĄĆąć", "")
        );
    }

    #[test]
    fn marks_after_a_blank_line_are_reported_apart_from_letters() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "polish.txt", "ĄĆ ąć\r\n\r\n„” Ą\r\n".as_bytes());
        let charset = load(&path).unwrap();
        assert_eq!(
            (charset.letters.as_str(), charset.typography.as_str()),
            ("ĄĆąć", "„”")
        );
    }
}
