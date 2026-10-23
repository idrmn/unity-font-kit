//! Built in its own project folder, so marking the same fonts again only copies the files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::fonts::numbers;
use crate::game::folder::Game;
use crate::game::typetrees;
use crate::progress::{self, Report};
use crate::rebuild::inject::Options;
use crate::work::build;
use crate::work::plan::{self, Entry, Plan};
use crate::work::project::{self, Project};

use super::state::{self, Numbered, Saved, SourceFile, Wanted};
use super::{FontKey, GameView, apply, scanning};

const NUMBERS_DIR: &str = "numbers";
const STAMP: &str = "stamp.json";

/// The marked fonts draw their row numbers; the others keep what the game has, original or applied.
pub fn mark_fonts(view: &GameView, marked: &[FontKey], progress: Report) -> Result<GameView> {
    let (game, work) = apply::prepare_change(view)?;
    let Some(plan) = view.plan.as_ref() else {
        bail!("Scan the game first");
    };
    if marked.is_empty() {
        bail!("Pick at least one font to mark");
    }
    apply::reject_unwritable(&game.data_dir)?;
    let saved = state::load_saved(&work)?;
    let side = open_side_project(&work, NUMBERS_DIR)?;
    let numbered = number_fonts(plan, marked, &saved, &work, &side)?;
    build_once(&numbered, &game, &side, progress)?;
    progress::check(progress, "Copying the new files into the game")?;
    project::install(&side, &game.data_dir)?;
    state::store_saved(
        &work,
        &Saved {
            numbers: list_numbers(plan, marked),
            ..saved
        },
    )?;
    state::open_game_in(&view.home, &view.record)
}

/// Puts back what the game had before the marks: the applied fonts, or the originals.
pub fn unmark_fonts(view: &GameView) -> Result<GameView> {
    let (game, work) = apply::prepare_change(view)?;
    let saved = state::load_saved(&work)?;
    if saved.applied.is_empty() {
        return apply::restore(view);
    }
    apply::reject_unwritable(&game.data_dir)?;
    project::install(&work, &game.data_dir)?;
    state::store_saved(
        &work,
        &Saved {
            numbers: Vec::new(),
            ..saved
        },
    )?;
    state::open_game_in(&view.home, &view.record)
}

fn build_once(numbered: &Plan, game: &Game, side: &Project, progress: Report) -> Result<()> {
    // The letters live in their own file, so its text goes into the stamp, not just its path.
    let letters = match &numbered.chars {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read {path}"))?
            .replace("\r\n", "\n"),
        None => String::new(),
    };
    // Paths stay the same when a font file changes, so its content goes in too.
    let sources = numbered
        .fonts
        .iter()
        .filter(|entry| !entry.skip && !entry.source.is_empty())
        .map(|entry| project::hash_file(Path::new(&entry.source)))
        .collect::<Result<Vec<_>>>()?;
    let stamp = [
        env!("CARGO_PKG_VERSION").to_string(),
        letters,
        sources.join(" "),
        plan::to_json(numbered)?,
    ]
    .join("\n");
    if is_fresh(side, &game.data_dir, &stamp)? {
        return Ok(());
    }
    project::write_text(&side.plan, &plan::to_json(numbered)?)?;
    let charset = plan::read_charset(numbered, &side.plan)?;
    let generator = scanning::prepare_generator(game, progress)?;
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

/// Where the marked build keeps its files, so an install of them stopped midway is recognised.
pub(crate) fn marked_files(work: &Project) -> Result<PathBuf> {
    Ok(open_side_project(work, NUMBERS_DIR)?.patched)
}

fn list_numbers(plan: &Plan, marked: &[FontKey]) -> Vec<Numbered> {
    plan.fonts
        .iter()
        .enumerate()
        .filter(|(_, entry)| marked.contains(&FontKey::of(entry)))
        .map(|(index, entry)| Numbered {
            kind: entry.kind,
            font: entry.font.clone(),
            family: entry.family.clone(),
            number: index + 1,
        })
        .collect()
}

/// Marked fonts get their number font; the rest keep the applied choice, or stay original.
fn number_fonts(
    plan: &Plan,
    marked: &[FontKey],
    saved: &Saved,
    work: &Project,
    side: &Project,
) -> Result<Plan> {
    let folder = side.work.join("fonts");
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("Cannot create {}", folder.display()))?;
    let applied: Wanted = state::to_wanted(&saved.applied);
    let mut fonts: Vec<Entry> = Vec::new();
    for (index, entry) in plan.fonts.iter().enumerate() {
        let key = FontKey::of(entry);
        if marked.contains(&key) {
            let path = folder.join(format!("{}.ttf", index + 1));
            std::fs::write(&path, numbers::make_number_font(index + 1)?)
                .with_context(|| format!("Cannot write {}", path.display()))?;
            fonts.push(Entry {
                source: path.display().to_string(),
                skip: false,
                ..entry.clone()
            });
            continue;
        }
        fonts.push(match applied.get(&key) {
            Some(SourceFile { source, .. }) => Entry {
                source: source.clone(),
                skip: false,
                ..entry.clone()
            },
            None => Entry {
                skip: true,
                ..entry.clone()
            },
        });
    }
    let every: Vec<_> = fonts.iter().map(FontKey::of).collect();
    let chosen = Plan {
        fonts,
        ..plan.clone()
    };
    Ok(select_fonts(&chosen, &work.plan, &every))
}

fn select_fonts(plan: &Plan, plan_path: &Path, fonts: &[FontKey]) -> Plan {
    let shared = build::share_family_sources(plan.clone()).plan;
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
