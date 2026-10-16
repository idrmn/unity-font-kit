use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::downloads;
use crate::fonts::charsets::Charset;
use crate::fonts::font_file::{self, FontCheck};
use crate::work::project;

use super::letters::to_charset;
use super::state::open_work;
use super::{FontOption, GameView, Letters, Origin};

const ADDED_FONTS: &str = "fonts";

pub fn list_options(view: &GameView, letters: &Letters) -> Result<Vec<FontOption>> {
    let charset = to_charset(letters)?;
    let work = open_work(&view.home, &view.record)?;
    let game = list_game_fonts(&work.fonts, &charset)?;
    let added = list_fonts(&view.home.join(ADDED_FONTS), &charset, Origin::Added)?;
    Ok([game, added].concat())
}

pub fn add_font_file(path: &Path, letters: &Letters) -> Result<FontOption> {
    add_font_file_in(&downloads::find_home()?, path, letters)
}

pub(crate) fn add_font_file_in(home: &Path, path: &Path, letters: &Letters) -> Result<FontOption> {
    let data = std::fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let checked = font_file::check(&data, &to_charset(letters)?)
        .with_context(|| format!("{} is not a font ufk can use", path.display()))?;
    let folder = home.join(ADDED_FONTS);
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("Cannot create {}", folder.display()))?;
    let target = pick_target(&folder, path, &data)?;
    if !target.exists() {
        std::fs::write(&target, &data)
            .with_context(|| format!("Cannot write {}", target.display()))?;
    }
    Ok(to_option(target, &checked, Origin::Added))
}

fn pick_target(folder: &Path, path: &Path, data: &[u8]) -> Result<PathBuf> {
    let name = path.file_name().context("The font has no file name")?;
    let stem = Path::new(name)
        .file_stem()
        .unwrap_or(name)
        .to_string_lossy();
    let suffix = Path::new(name)
        .extension()
        .map_or_else(String::new, |ext| format!(".{}", ext.to_string_lossy()));
    let candidates = std::iter::once(folder.join(name))
        .chain((2..).map(|number| folder.join(format!("{stem} {number}{suffix}"))));
    candidates
        .into_iter()
        .find(|candidate| std::fs::read(candidate).map_or(true, |existing| existing == data))
        .context("No free file name for the font")
}

/// Games repeat one font in many files, so each file content is listed once.
fn list_game_fonts(folder: &Path, charset: &Charset) -> Result<Vec<FontOption>> {
    let mut seen = HashSet::new();
    let options = list_fonts(folder, charset, Origin::Game)?
        .into_iter()
        .filter(|option| option.missing.is_empty())
        .filter(|option| project::hash_file(&option.path).is_ok_and(|digest| seen.insert(digest)))
        .collect();
    Ok(options)
}

fn list_fonts(folder: &Path, charset: &Charset, origin: Origin) -> Result<Vec<FontOption>> {
    let mut options: Vec<FontOption> = project::list_files(folder)?
        .into_iter()
        .map(|file| folder.join(file))
        .filter_map(|path| {
            let data = std::fs::read(&path).ok()?;
            let checked = font_file::check(&data, charset).ok()?;
            Some(to_option(path, &checked, origin))
        })
        .collect();
    options.sort_by(|a, b| (&a.label, &a.path).cmp(&(&b.label, &b.path)));
    Ok(options)
}

fn to_option(path: PathBuf, checked: &FontCheck, origin: Origin) -> FontOption {
    FontOption {
        label: format!("{} {}", checked.family, checked.style),
        path,
        origin,
        missing: checked.missing_letters.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::letters::letters_from_preset;
    use crate::session::{GameRecord, GameState};
    use std::collections::HashMap;

    const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

    #[test]
    fn game_fonts_need_every_letter_and_added_fonts_follow() {
        let home = tempfile::tempdir().unwrap();
        let record = GameRecord {
            id: "demo-00000000".to_string(),
            folder: PathBuf::from("/games/Demo"),
            name: "Demo".to_string(),
            unity_version: "2020.3.12f1".to_string(),
            backend: crate::session::Backend::Mono,
        };
        let work = open_work(home.path(), &record).unwrap();
        std::fs::create_dir_all(work.fonts.join("sub")).unwrap();
        let abc = std::fs::read(format!("{DATA}/make-font-ABC.ttf")).unwrap();
        let ab = std::fs::read(format!("{DATA}/make-font-AB.ttf")).unwrap();
        std::fs::write(work.fonts.join("abc.ttf"), &abc).unwrap();
        std::fs::write(work.fonts.join("sub/abc copy.ttf"), &abc).unwrap();
        std::fs::write(work.fonts.join("ab.ttf"), &ab).unwrap();
        std::fs::write(work.fonts.join("notes.txt"), "not a font").unwrap();
        let letters = Letters {
            preset: "custom".to_string(),
            letters: "ABC".to_string(),
            marks: String::new(),
        };
        let added = add_font_file_in(home.path(), &work.fonts.join("ab.ttf"), &letters).unwrap();
        assert_eq!(added.missing, "C");
        assert!(add_font_file_in(home.path(), &work.fonts.join("notes.txt"), &letters).is_err());
        let view = GameView {
            record,
            state: GameState::Original,
            plan: None,
            statuses: HashMap::new(),
            notes: HashMap::new(),
            replacements: HashMap::new(),
            scan_errors: Vec::new(),
            rechecks: false,
            numbers: HashMap::new(),
            letters: letters_from_preset("uk").unwrap(),
            home: home.path().to_path_buf(),
        };
        let options = list_options(&view, &letters).unwrap();
        let shown: Vec<(Origin, PathBuf)> = options
            .iter()
            .map(|option| (option.origin, option.path.clone()))
            .collect();
        assert_eq!(
            shown,
            [
                (Origin::Game, work.fonts.join("abc.ttf")),
                (Origin::Added, home.path().join("fonts/ab.ttf")),
            ]
        );
    }
}
