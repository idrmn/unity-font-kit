use std::collections::{BTreeSet, HashMap};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::downloads;
use crate::fonts::charsets::Charset;
use crate::fonts::{coverage, extract};
use crate::game::folder::{self, Game, ScanDepth};
use crate::game::scan::{self, Ref, ScanResult};
use crate::game::typetrees::{self, Generator};
use crate::progress::{self, Progress, Report};
use crate::work::plan::{self, Extracted, Scanned};
use crate::work::project::{self, Project};

use super::state::{self, Saved};
use super::{GameRecord, GameState, GameView, Letters, letters};

const SCAN_FILE: &str = "scan.json";

#[derive(Debug, Serialize, Deserialize)]
struct ScanCache {
    result: ScanResult,
    extracted: Vec<SavedFont>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
// scan.json keeps the old tuple shape, so caches from earlier versions still load.
#[serde(from = "SavedFontWire", into = "SavedFontWire")]
struct SavedFont {
    file: PathBuf,
    reference: Ref,
    path: String,
}

type SavedFontWire = ((PathBuf, Ref), String);

impl From<SavedFontWire> for SavedFont {
    fn from(((file, reference), path): SavedFontWire) -> Self {
        SavedFont {
            file,
            reference,
            path,
        }
    }
}

impl From<SavedFont> for SavedFontWire {
    fn from(font: SavedFont) -> Self {
        ((font.file, font.reference), font.path)
    }
}

pub fn scan(
    record: &GameRecord,
    depth: ScanDepth,
    letters: &Letters,
    progress: Report,
) -> Result<GameView> {
    scan_in(&downloads::find_home()?, record, depth, letters, progress)
}

pub fn recheck(record: &GameRecord, letters: &Letters) -> Result<GameView> {
    recheck_in(&downloads::find_home()?, record, letters)
}

pub(crate) fn scan_in(
    home: &Path,
    record: &GameRecord,
    depth: ScanDepth,
    letters: &Letters,
    progress: Report,
) -> Result<GameView> {
    let charset = letters::to_charset(letters)?;
    let (game, work) = prepare(home, record)?;
    let generator = prepare_generator(&game, progress)?;
    let listed = folder::list_asset_files(&game, depth, progress)?;
    let files = add_known_font_files(listed, &game, &work)?;
    let result = scan_originals(&game, &work, &files, generator.as_ref(), progress);
    progress::check(progress, "Saving the fonts")?;
    let extracted = save_game_fonts(&result, &game, &work)?;
    let cache = ScanCache { extracted, result };
    // The cache first: a plan is always rebuilt from it, never the other way round.
    state::write_json(&work.work.join(SCAN_FILE), &cache)?;
    write_plan(record, &game, &work, &cache, &charset)?;
    let saved = state::load_saved(&work)?;
    state::store_saved(
        &work,
        &Saved {
            scan_errors: cache.result.errors.clone(),
            ..saved
        },
    )?;
    state::open_game_in(home, record)
}

pub(crate) fn recheck_in(home: &Path, record: &GameRecord, letters: &Letters) -> Result<GameView> {
    let charset = letters::to_charset(letters)?;
    let game = folder::locate(&record.folder)?;
    let work = state::open_work(home, record)?;
    let path = work.work.join(SCAN_FILE);
    if !path.is_file() {
        bail!("No fonts read yet: run a scan first");
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    let mut cache: ScanCache =
        serde_json::from_str(&text).with_context(|| format!("Cannot read {}", path.display()))?;
    load_saved_fonts(&mut cache, &work)?;
    write_plan(record, &game, &work, &cache, &charset)?;
    state::open_game_in(home, record)
}

pub(crate) fn has_scan_cache(work: &Project) -> bool {
    work.work.join(SCAN_FILE).is_file()
}

pub(crate) fn forget_scan(work: &Project) -> Result<()> {
    match std::fs::remove_file(work.work.join(SCAN_FILE)) {
        Err(error) if error.kind() != ErrorKind::NotFound => {
            Err(error).with_context(|| format!("Cannot delete {}", work.work.display()))
        }
        _ => Ok(()),
    }
}

/// A fast scan also reads the files where the plan already has fonts, so fonts a deep scan found stay.
fn add_known_font_files(listed: Vec<PathBuf>, game: &Game, work: &Project) -> Result<Vec<PathBuf>> {
    if !work.plan.is_file() {
        return Ok(listed);
    }
    let known: BTreeSet<PathBuf> = plan::load(&work.plan)?
        .fonts
        .iter()
        .flat_map(|entry| &entry.places)
        .map(|place| game.data_dir.join(&place.file))
        .filter(|path| path.is_file())
        .collect();
    let mut files = listed;
    let extra: Vec<PathBuf> = known
        .into_iter()
        .filter(|path| !files.contains(path))
        .collect();
    files.extend(extra);
    Ok(files)
}

fn prepare(home: &Path, record: &GameRecord) -> Result<(Game, Project)> {
    let game = folder::locate(&record.folder)?;
    let work = state::open_work(home, record)?;
    folder::reject_output_inside(&game.data_dir, &work.work)?;
    reject_updated(&work, &game)?;
    std::fs::create_dir_all(&work.work)
        .with_context(|| format!("Cannot create {}", work.work.display()))?;
    Ok((game, work))
}

fn write_plan(
    record: &GameRecord,
    game: &Game,
    work: &Project,
    cache: &ScanCache,
    charset: &Charset,
) -> Result<()> {
    let previous = match work.plan.is_file() {
        true => Some(plan::load(&work.plan)?),
        false => None,
    };
    let chars = letters::store_letters(work, charset)?;
    let extracted: Extracted = cache
        .extracted
        .iter()
        .map(|font| {
            (
                (font.file.clone(), font.reference.clone()),
                font.path.clone(),
            )
        })
        .collect();
    let raw_note = typetrees::describe_raw_reading(game);
    let scanned = Scanned {
        game_path: &record.folder,
        data_dir: &game.data_dir,
        extracted: &extracted,
        raw_note: &raw_note,
        tmp: coverage::check_tmp_fonts(&cache.result, charset),
        legacy: coverage::check_legacy_fonts(&cache.result, charset),
    };
    let made = plan::make_plan(scanned, charset, &charset.name, chars.as_deref());
    let made = match &previous {
        Some(previous) => plan::keep_user_choices(made, previous),
        None => made,
    };
    project::write_text(&work.plan, &plan::to_json(&made)?)
}

fn load_saved_fonts(cache: &mut ScanCache, work: &Project) -> Result<()> {
    let base = work.plan.parent().unwrap_or(Path::new("")).to_path_buf();
    let paths: HashMap<(&PathBuf, &Ref), &String> = cache
        .extracted
        .iter()
        .map(|saved| ((&saved.file, &saved.reference), &saved.path))
        .collect();
    for font in &mut cache.result.legacy {
        if let Some(relative) = paths.get(&(&font.file, &font.reference)) {
            let path = base.join(relative);
            font.data =
                std::fs::read(&path).with_context(|| format!("Cannot read {}", path.display()))?;
        }
    }
    Ok(())
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
    let moves: HashMap<PathBuf, PathBuf> = pairs.into_iter().collect();
    scan::relocate(result, &moves)
}

fn save_game_fonts(result: &ScanResult, game: &Game, work: &Project) -> Result<Vec<SavedFont>> {
    let saved = extract::plan_files(&result.legacy, &game.data_dir);
    extract::write_files(&saved, &work.fonts)?;
    let base = work.plan.parent().unwrap_or(Path::new(""));
    Ok(saved
        .iter()
        .filter_map(|item| {
            let path = work.fonts.join(item.path.as_ref()?);
            let relative = path.strip_prefix(base).ok()?;
            let text = folder::format_relative(relative, Path::new(""));
            Some(SavedFont {
                file: item.font.file.clone(),
                reference: item.font.reference.clone(),
                path: text,
            })
        })
        .collect())
}
