use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::game::scan::LegacyFont;

const SIGNATURES: [(&[u8], &str); 2] = [(b"OTTO", ".otf"), (b"ttcf", ".ttc")];

#[derive(Debug, Clone, PartialEq)]
pub struct Extracted {
    pub font: LegacyFont,
    pub path: Option<PathBuf>,
}

#[must_use]
pub fn plan_files(fonts: &[LegacyFont], data_dir: &Path) -> Vec<Extracted> {
    let repeats = count_repeats(fonts);
    fonts
        .iter()
        .map(|font| Extracted {
            font: font.clone(),
            path: (!font.data.is_empty()).then(|| place_file(font, data_dir, &repeats)),
        })
        .collect()
}

fn count_repeats(fonts: &[LegacyFont]) -> HashMap<(PathBuf, String), usize> {
    let mut repeats = HashMap::new();
    for font in fonts.iter().filter(|font| !font.data.is_empty()) {
        *repeats
            .entry((font.file.clone(), safe_stem(&font.name)))
            .or_insert(0) += 1;
    }
    repeats
}

fn place_file(
    font: &LegacyFont,
    data_dir: &Path,
    repeats: &HashMap<(PathBuf, String), usize>,
) -> PathBuf {
    let mut stem = safe_stem(&font.name);
    if repeats
        .get(&(font.file.clone(), stem.clone()))
        .is_some_and(|count| *count > 1)
    {
        stem = format!("{stem} {}", font.reference.path_id);
    }
    let folder = match font.file.strip_prefix(data_dir) {
        Ok(relative) => relative.to_path_buf(),
        Err(_) => PathBuf::from(font.file.file_name().unwrap_or_default()),
    };
    folder.join(stem + detect_suffix(&font.data))
}

fn detect_suffix(data: &[u8]) -> &'static str {
    SIGNATURES
        .iter()
        .find(|(signature, _)| data.get(..4) == Some(*signature))
        .map_or(".ttf", |(_, suffix)| suffix)
}

#[must_use]
pub fn safe_stem(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|ch| {
            if "<>:\"/\\|?*".contains(ch) || ch < '\u{20}' {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let stem = replaced.trim_matches([' ', '.']);
    if stem.is_empty() {
        "font".to_string()
    } else {
        stem.to_string()
    }
}

pub fn write_files(planned: &[Extracted], out_dir: &Path) -> Result<()> {
    for item in planned {
        let Some(path) = &item.path else { continue };
        let target = out_dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        std::fs::write(&target, &item.font.data)
            .with_context(|| format!("cannot write {}", target.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = "/game/Demo_Data";
    const TTF: &[u8] = b"\x00\x01\x00\x00";

    fn legacy(name: &str, path_id: i64, data: &[u8], file: &str) -> LegacyFont {
        LegacyFont {
            file: Path::new(DATA).join(file),
            reference: ("CAB-1".to_string(), path_id).into(),
            name: name.to_string(),
            data: data.to_vec(),
            os_names: Vec::new(),
            dynamic: false,
        }
    }

    #[test]
    fn plan_mirrors_game_files_and_skips_empty_fonts() {
        let fonts = [
            legacy("Body", 1, b"OTTO", "sharedassets0.assets"),
            legacy("Body", 2, TTF, "StreamingAssets/fonts_th"),
            legacy("Title/Bold", 3, TTF, "sharedassets0.assets"),
            legacy("Title/Bold", 5, TTF, "sharedassets0.assets"),
            legacy("Arial", 4, b"", "resources.assets"),
        ];
        let paths: Vec<_> = plan_files(&fonts, Path::new(DATA))
            .into_iter()
            .map(|item| item.path)
            .collect();
        assert_eq!(
            paths,
            [
                Some(PathBuf::from("sharedassets0.assets/Body.otf")),
                Some(PathBuf::from("StreamingAssets/fonts_th/Body.ttf")),
                Some(PathBuf::from("sharedassets0.assets/Title_Bold 3.ttf")),
                Some(PathBuf::from("sharedassets0.assets/Title_Bold 5.ttf")),
                None,
            ]
        );
    }
}
