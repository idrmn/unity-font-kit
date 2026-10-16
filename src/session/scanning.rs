use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::downloads;
use crate::fonts::{coverage, extract};
use crate::game::folder::{self, Game};
use crate::game::scan::{self, ScanResult};
use crate::game::typetrees::{self, Generator};
use crate::progress::{Progress, Report};
use crate::work::plan::{self, Extracted};
use crate::work::project::{self, Project};

use super::GameState;
use super::letters::{store_letters, to_charset};
use super::state::{self, open_game_in, open_work};
use super::{GameRecord, GameView, Letters};

pub fn scan(
    record: &GameRecord,
    full: bool,
    letters: &Letters,
    progress: Report,
) -> Result<GameView> {
    scan_in(&downloads::find_home()?, record, full, letters, progress)
}

pub(crate) fn scan_in(
    home: &Path,
    record: &GameRecord,
    full: bool,
    letters: &Letters,
    progress: Report,
) -> Result<GameView> {
    let game = folder::locate(&record.folder)?;
    let work = open_work(home, record)?;
    folder::reject_output_inside(&game.data_dir, &work.work)?;
    reject_updated(&work, &game)?;
    let charset = to_charset(letters)?;
    let previous = match work.plan.is_file() {
        true => Some(plan::load(&work.plan)?),
        false => None,
    };
    std::fs::create_dir_all(&work.work)
        .with_context(|| format!("Cannot create {}", work.work.display()))?;
    let chars = store_letters(&work, &charset)?;
    let generator = prepare_generator(&game, progress)?;
    let every = folder::list_asset_files(&game)?;
    let files = match full {
        true => every,
        false => folder::select_quick(&every, &game.data_dir),
    };
    let result = scan_originals(&game, &work, &files, generator.as_ref(), progress);
    let extracted = save_game_fonts(&result, &game, &work)?;
    let raw_note = typetrees::describe_raw_reading(&game);
    let scanned = plan::Scanned {
        game_path: &record.folder,
        data_dir: &game.data_dir,
        extracted: &extracted,
        raw_note: &raw_note,
        tmp: coverage::check_tmp_fonts(&result, &charset),
        legacy: coverage::check_legacy_fonts(&result, &charset),
    };
    let made = plan::make_plan(scanned, &charset, (&charset.name, chars.as_deref()));
    let made = match &previous {
        Some(previous) => plan::keep_user_choices(made, previous),
        None => made,
    };
    project::write_text(&work.plan, &plan::to_json(&made)?)?;
    let saved = state::load_saved(&work)?;
    state::store_saved(
        &work,
        &state::Saved {
            scan_errors: result.errors,
            deep_scan: full,
            ..saved
        },
    )?;
    open_game_in(home, record)
}

fn reject_updated(work: &Project, game: &Game) -> Result<()> {
    if let GameState::Updated { files } = state::detect_state(work, &game.data_dir)? {
        bail!(
            "The game changed since its fonts were applied ({}): click Use new game files first",
            files.join(", ")
        );
    }
    Ok(())
}

/// IL2CPP games need Cpp2IL once per game version; that step can take minutes.
pub(crate) fn prepare_generator(game: &Game, progress: Report) -> Result<Option<Generator>> {
    progress(Progress {
        done: 0,
        total: 1,
        step: "Reading the game code".to_string(),
    });
    typetrees::build_generator(&game.unity_version, typetrees::find_dll_dir(game)?)
}

/// After an apply the game files are patched; the scan reads the originals kept in the backup.
fn scan_originals(
    game: &Game,
    work: &Project,
    files: &[PathBuf],
    generator: Option<&Generator>,
    progress: Report,
) -> ScanResult {
    let pairs: Vec<(PathBuf, PathBuf)> = files
        .iter()
        .map(|path| {
            let relative = folder::format_relative(path, &game.data_dir);
            let original = project::original_file(Some(&work.backup), &game.data_dir, &relative);
            (original, path.clone())
        })
        .collect();
    let sources: Vec<PathBuf> = pairs.iter().map(|(source, _)| source.clone()).collect();
    let homes: HashMap<PathBuf, PathBuf> = pairs
        .iter()
        .map(|(source, path)| {
            let home = path.parent().unwrap_or(&game.data_dir).to_path_buf();
            (source.clone(), home)
        })
        .collect();
    let raw_note = typetrees::describe_raw_reading(game);
    let result = scan::scan_files(&sources, generator, &raw_note, &homes, progress);
    scan::relocate(result, &pairs.into_iter().collect())
}

fn save_game_fonts(result: &ScanResult, game: &Game, work: &Project) -> Result<Extracted> {
    let saved = extract::plan_files(&result.legacy, &game.data_dir);
    extract::write_files(&saved, &work.fonts)?;
    let base = work.plan.parent().unwrap_or(Path::new(""));
    Ok(saved
        .iter()
        .filter_map(|item| {
            let path = work.fonts.join(item.path.as_ref()?);
            let relative = path.strip_prefix(base).ok()?;
            let text = folder::format_relative(relative, Path::new(""));
            Some(((item.font.file.clone(), item.font.reference.clone()), text))
        })
        .collect())
}
