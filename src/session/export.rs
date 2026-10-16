//! Built in its own project folder so the applied state stays as it is.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::game::folder;
use crate::game::typetrees;
use crate::progress::{Progress, Report};
use crate::rebuild::inject::Options;
use crate::work::build;
use crate::work::export as players;
use crate::work::plan::{self, Entry, Plan};
use crate::work::project::{self, Project};

use super::GameState;
use super::scanning::prepare_generator;
use super::state::{detect_state, list_wanted, open_work, to_choices};
use super::{FontKey, GameView};

const EXPORT_DIR: &str = "export";
const STAMP: &str = "stamp.json";

pub fn default_readme(view: &GameView) -> String {
    let folder = folder::find_data_dir(&view.record.folder)
        .ok()
        .and_then(|dir| Some(dir.file_name()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| format!("{}_Data", view.record.name));
    players::default_readme(&view.record.name, &folder).unwrap_or_default()
}

/// An empty README means the default one for this version.
pub fn export(
    view: &GameView,
    fonts: &[FontKey],
    readme: &str,
    target: &Path,
    progress: Report,
) -> Result<PathBuf> {
    let Some(plan) = &view.plan else {
        bail!("Scan the game first");
    };
    if fonts.is_empty() {
        bail!("Pick at least one font for the patch");
    }
    let game = folder::locate(&view.record.folder)?;
    let work = open_work(&view.home, &view.record)?;
    if let GameState::Updated { .. } = detect_state(&work, &game.data_dir)? {
        bail!("The game changed since its fonts were applied: click Use new game files first");
    }
    let wanted = list_wanted(plan, &work.plan);
    if let Some(key) = fonts.iter().find(|key| !wanted.contains_key(*key)) {
        bail!("{}: choose a replacement font first", key.font);
    }
    let copy = select_fonts(plan, &work.plan, fonts);
    let side = open_export_project(&work)?;
    let stamp = serde_json::to_string_pretty(&(
        plan::to_json(&copy)?,
        to_choices(&list_wanted(&copy, &side.plan)),
    ))?;
    if !is_fresh(&side, &game.data_dir, &stamp)? {
        std::fs::create_dir_all(&side.work)?;
        project::write_text(&side.plan, &plan::to_json(&copy)?)?;
        let (shared, _) = build::share_family_sources(copy);
        let charset = plan::read_charset(&shared, &side.plan)?;
        let generator = prepare_generator(&game, progress)?;
        let options = Options {
            raw_note: typetrees::describe_raw_reading(&game),
        };
        build::build(
            &shared,
            &game,
            &side,
            generator.as_ref(),
            &charset,
            &options,
            progress,
        )?;
        project::write_text(&side.work.join(STAMP), &stamp)?;
    }
    progress(Progress {
        done: 0,
        total: 1,
        step: "Making the zip".to_string(),
    });
    let readme = match readme.trim().is_empty() {
        true => default_readme(view),
        false => readme.to_string(),
    };
    players::export_to(&side, &game.data_dir, &readme, target)
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
fn open_export_project(work: &Project) -> Result<Project> {
    let name = work.plan.file_name().unwrap_or_default();
    let side = project::open_project(&work.work.join(EXPORT_DIR).join(name))?;
    Ok(Project {
        backup: work.backup.clone(),
        installed: work.installed.clone(),
        ..side
    })
}

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
