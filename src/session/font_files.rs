use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::work::plan::Kind;

use super::atlas::{atlas_png, find_entry, read_legacy_data};
use super::{FontKey, GameView};

pub fn save_font_files(view: &GameView, keys: &[FontKey], folder: &Path) -> Result<Vec<PathBuf>> {
    let mut taken = HashSet::new();
    let mut written = Vec::new();
    for key in keys {
        let Some((data, extension)) = read_font_file(view, key)? else {
            continue;
        };
        let target = pick_name(folder, &key.font, extension, &mut taken);
        std::fs::write(&target, data)
            .with_context(|| format!("Cannot write {}", target.display()))?;
        written.push(target);
    }
    Ok(written)
}

fn read_font_file(view: &GameView, key: &FontKey) -> Result<Option<(Vec<u8>, &'static str)>> {
    if key.kind == Kind::Tmp {
        return Ok(atlas_png(view, key)?.map(|png| (png, "png")));
    }
    let Some(entry) = find_entry(view, key) else {
        return Ok(None);
    };
    Ok(read_legacy_data(view, entry)?.map(|data| {
        let extension = if data.starts_with(b"OTTO") {
            "otf"
        } else {
            "ttf"
        };
        (data, extension)
    }))
}

/// Font names may hold characters a file name cannot; files already on disk are never overwritten.
fn pick_name(folder: &Path, font: &str, extension: &str, taken: &mut HashSet<PathBuf>) -> PathBuf {
    let stem: String = font
        .chars()
        .map(|ch| {
            if r#"/\:*?"<>|"#.contains(ch) || ch.is_control() {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let first = folder.join(format!("{stem}.{extension}"));
    std::iter::once(first.clone())
        .chain((2..).map(|number| folder.join(format!("{stem} {number}.{extension}"))))
        .find(|candidate| !candidate.exists() && taken.insert(candidate.clone()))
        .unwrap_or(first)
}
