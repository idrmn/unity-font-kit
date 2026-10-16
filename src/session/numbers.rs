//! Built in its own project folder, so a second Find in game after Restore only copies the files again.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::fonts::numbers;
use crate::game::folder::Game;
use crate::game::typetrees;
use crate::progress::{Progress, Report};
use crate::rebuild::inject::Options;
use crate::work::build;
use crate::work::plan::{self, Entry, Plan};
use crate::work::project::{self, Project};

use super::apply::{prepare_change, reject_unwritable};
use super::scanning::prepare_generator;
use super::state::{self, Numbered, Saved, detect_state, open_game_in};
use super::{FontKey, GameState, GameView};

const NUMBERS_DIR: &str = "numbers";
const STAMP: &str = "stamp.json";

/// Every font draws its row number, so one game start shows which row each text uses.
pub fn show_numbers(view: &GameView, progress: Report) -> Result<GameView> {
    let (game, work) = prepare_change(view)?;
    let Some(plan) = view.plan.as_ref() else {
        bail!("Scan the game first");
    };
    // On top of applied fonts, Restore would bring back the originals and drop them unseen.
    if detect_state(&work, &game.data_dir)? != GameState::Original {
        bail!("Restore the original fonts first");
    }
    reject_unwritable(&game.data_dir)?;
    let side = open_side_project(&work, NUMBERS_DIR)?;
    let numbered = number_fonts(plan, &work, &side)?;
    build_once(&numbered, &game, &side, progress)?;
    progress(Progress {
        done: 0,
        total: 1,
        step: "Copying the new files into the game".to_string(),
    });
    project::install(&side, &game.data_dir)?;
    let saved = state::load_saved(&work)?;
    state::store_saved(
        &work,
        &Saved {
            applied: Vec::new(),
            results: Vec::new(),
            numbers: list_numbers(plan),
            ..saved
        },
    )?;
    open_game_in(&view.home, &view.record)
}

fn build_once(numbered: &Plan, game: &Game, side: &Project, progress: Report) -> Result<()> {
    let stamp = format!(
        "{}
{}",
        env!("CARGO_PKG_VERSION"),
        plan::to_json(numbered)?
    );
    if is_fresh(side, &game.data_dir, &stamp)? {
        return Ok(());
    }
    project::write_text(&side.plan, &plan::to_json(numbered)?)?;
    let charset = plan::read_charset(numbered, &side.plan)?;
    let generator = prepare_generator(game, progress)?;
    let options = Options {
        raw_note: typetrees::describe_raw_reading(game),
    };
    build::build(
        numbered,
        game,
        side,
        generator.as_ref(),
        &charset,
        &options,
        progress,
    )?;
    project::write_text(&side.work.join(STAMP), &stamp)
}

fn list_numbers(plan: &Plan) -> Vec<Numbered> {
    plan.fonts
        .iter()
        .enumerate()
        .map(|(index, entry)| Numbered {
            kind: entry.kind,
            font: entry.font.clone(),
            family: entry.family.clone(),
            number: index + 1,
        })
        .collect()
}

/// Every font gets its own number font, with paths the side project can read.
fn number_fonts(plan: &Plan, work: &Project, side: &Project) -> Result<Plan> {
    let folder = side.work.join("fonts");
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("Cannot create {}", folder.display()))?;
    let fonts = plan
        .fonts
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let path = folder.join(format!("{}.ttf", index + 1));
            std::fs::write(&path, numbers::make_number_font(index + 1)?)
                .with_context(|| format!("Cannot write {}", path.display()))?;
            Ok(Entry {
                source: path.display().to_string(),
                skip: false,
                ..entry.clone()
            })
        })
        .collect::<Result<Vec<Entry>>>()?;
    let every: Vec<_> = fonts.iter().map(FontKey::of).collect();
    let chosen = Plan {
        fonts,
        ..plan.clone()
    };
    Ok(select_fonts(&chosen, &work.plan, &every))
}

fn select_fonts(plan: &Plan, plan_path: &Path, fonts: &[FontKey]) -> Plan {
    let (shared, _) = build::share_family_sources(plan.clone());
    let base = plan_path.parent().unwrap_or(Path::new(""));
    let entries = shared
        .fonts
        .iter()
        .map(|entry| Entry {
            skip: entry.skip || !fonts.contains(&FontKey::of(entry)),
            source: absolute_source(entry, plan_path),
            ..entry.clone()
        })
        .collect();
    Plan {
        fonts: entries,
        chars: shared
            .chars
            .as_ref()
            .map(|chars| base.join(chars).display().to_string()),
        ..shared
    }
}

fn absolute_source(entry: &Entry, plan_path: &Path) -> String {
    match build::resolve_source(entry, plan_path) {
        Ok(path) if !entry.source.is_empty() => path.display().to_string(),
        _ => entry.source.clone(),
    }
}

/// Uses the game's real backup, since the originals are read from there.
fn open_side_project(work: &Project, dir: &str) -> Result<Project> {
    let name = work.plan.file_name().unwrap_or_default();
    let side = project::open_project(&work.work.join(dir).join(name))?;
    Ok(Project {
        backup: work.backup.clone(),
        installed: work.installed.clone(),
        ..side
    })
}

/// The last build of this side project is reused while its plan and the game's originals stay the same.
fn is_fresh(side: &Project, data_dir: &Path, stamp: &str) -> Result<bool> {
    let stamp_path = side.work.join(STAMP);
    let same =
        std::fs::read_to_string(&stamp_path).is_ok_and(|text| text.replace("\r\n", "\n") == stamp);
    let files = project::list_files(&side.patched)?;
    if !same || files.is_empty() {
        return Ok(false);
    }
    let built = project::read_build_sources(side)?;
    let read_now = project::hash_originals(side, data_dir, &files)?;
    Ok(files
        .iter()
        .all(|file| built.get(file) == read_now.get(file)))
}
