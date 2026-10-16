use std::path::Path;

use anyhow::{Context, Result, bail};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::fonts::charsets;
use crate::game::folder::{self, Game};
use crate::game::typetrees;
use crate::progress::{Progress, Report};
use crate::rebuild::inject::{Done, Options};
use crate::work::build::{self, Built};
use crate::work::plan::{self, Entry, Kind, Plan};
use crate::work::project::{self, Project};

use super::scanning::prepare_generator;
use super::state::{
    self, FontResult, Saved, Wanted, detect_state, list_wanted, open_game_in, open_work,
    result_key, to_choices, to_wanted,
};
use super::{FontKey, GameRecord, GameState, GameView};

/// Proton games show the .exe as an argument, not as the program.
pub fn is_game_running(record: &GameRecord) -> bool {
    let folder = folder::resolve(&record.folder);
    let mut system = System::new();
    let wanted = ProcessRefreshKind::nothing()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet);
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, wanted);
    let own = sysinfo::get_current_pid().ok();
    system.processes().iter().any(|(pid, process)| {
        let program = process.exe().is_some_and(|exe| exe.starts_with(&folder));
        let argument = process
            .cmd()
            .first()
            .is_some_and(|arg| Path::new(arg).starts_with(&folder));
        Some(*pid) != own && (program || argument)
    })
}

pub fn apply(view: &GameView, progress: Report) -> Result<GameView> {
    let (game, work) = prepare_change(view)?;
    let Some(plan) = view.plan.as_ref() else {
        bail!("Scan the game first");
    };
    let saved = state::load_saved(&work)?;
    let wanted = list_wanted(plan, &work.plan);
    let applied_now = detect_state(&work, &game.data_dir)? == GameState::Applied;
    if is_unchanged(&wanted, &saved, applied_now) {
        return open_game_in(&view.home, &view.record);
    }
    reject_unwritable(&game.data_dir)?;
    if wanted.is_empty() {
        return restore(view);
    }
    let mut failed: Vec<FontResult> = Vec::new();
    loop {
        let trying = without_fonts(plan, &failed);
        match build_and_install(&trying, &game, &work, progress) {
            Ok(results) => {
                let kept: Wanted = wanted
                    .iter()
                    .filter(|(key, _)| !failed.iter().any(|f| result_key(f) == **key))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                let applied = Saved {
                    applied: to_choices(&kept),
                    results: [results, failed].concat(),
                    ..saved
                };
                state::store_saved(&work, &applied)?;
                return open_game_in(&view.home, &view.record);
            }
            Err(error) => {
                let blamed = blame(&trying, &wanted, &format!("{error:#}"));
                let named = blamed.len() < count_wanted(&trying, &wanted);
                if !named || blamed.is_empty() {
                    let stopped = Saved {
                        results: [blamed, failed].concat(),
                        ..saved
                    };
                    state::store_saved(&work, &stopped)?;
                    return Err(error);
                }
                failed.extend(blamed);
            }
        }
    }
}

fn without_fonts(plan: &Plan, failed: &[FontResult]) -> Plan {
    let fonts = plan
        .fonts
        .iter()
        .map(|entry| Entry {
            skip: entry.skip || failed.iter().any(|f| result_key(f) == FontKey::of(entry)),
            ..entry.clone()
        })
        .collect();
    Plan {
        fonts,
        ..plan.clone()
    }
}

fn count_wanted(plan: &Plan, wanted: &Wanted) -> usize {
    plan.fonts
        .iter()
        .filter(|entry| !entry.skip && wanted.contains_key(&FontKey::of(entry)))
        .count()
}

pub fn restore(view: &GameView) -> Result<GameView> {
    let (game, work) = prepare_change(view)?;
    if !project::list_files(&work.backup)?.is_empty() {
        reject_unwritable(&game.data_dir)?;
        project::restore(&work, &game.data_dir)?;
    }
    let saved = state::load_saved(&work)?;
    state::store_saved(
        &work,
        &Saved {
            applied: Vec::new(),
            results: Vec::new(),
            ..saved
        },
    )?;
    open_game_in(&view.home, &view.record)
}

/// The changed files become the originals, so their old backups are deleted.
pub fn accept_update(view: &GameView) -> Result<GameView> {
    let game = folder::locate(&view.record.folder)?;
    let work = open_work(&view.home, &view.record)?;
    let updated = state::find_updated_files(&work, &game.data_dir)?;
    for file in &updated {
        remove_file(&work.backup.join(file))?;
    }
    let installed: project::Hashes = project::read_installed(&work)?
        .into_iter()
        .filter(|(file, _)| !updated.contains(file))
        .collect();
    match installed.is_empty() {
        true => remove_file(&work.installed)?,
        false => project::write_installed(&work, &installed)?,
    }
    // The last build read the old originals; the next apply builds again from the new ones.
    remove_dir(&work.patched)?;
    remove_file(&work.built)?;
    super::forget_scan(&work)?;
    let saved = state::load_saved(&work)?;
    state::store_saved(
        &work,
        &Saved {
            applied: Vec::new(),
            results: Vec::new(),
            ..saved
        },
    )?;
    open_game_in(&view.home, &view.record)
}

fn prepare_change(view: &GameView) -> Result<(Game, Project)> {
    if is_game_running(&view.record) {
        bail!("Close the game first");
    }
    let game = folder::locate(&view.record.folder)?;
    let work = open_work(&view.home, &view.record)?;
    if let GameState::Updated { files } = detect_state(&work, &game.data_dir)? {
        bail!(
            "The game changed since its fonts were applied ({}): click Use new game files first",
            files.join(", ")
        );
    }
    Ok((game, work))
}

pub(crate) fn is_unchanged(wanted: &Wanted, saved: &Saved, applied_now: bool) -> bool {
    let clean = saved.results.iter().all(|result| result.error.is_none());
    clean && applied_now != wanted.is_empty() && *wanted == to_wanted(&saved.applied)
}

/// On Windows a game under Program Files needs administrator rights.
pub fn can_write(record: &GameRecord) -> bool {
    folder::locate(&record.folder).is_ok_and(|game| tempfile::tempfile_in(&game.data_dir).is_ok())
}

fn reject_unwritable(data_dir: &Path) -> Result<()> {
    match tempfile::tempfile_in(data_dir) {
        Ok(_) => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "ufk cannot change the game files in {}. Move the game to a folder you can write to, \
                 or start ufk as administrator",
                data_dir.display()
            )
        }),
    }
}

fn build_and_install(
    plan: &Plan,
    game: &Game,
    work: &Project,
    progress: Report,
) -> Result<Vec<FontResult>> {
    let (shared, borrowed) = build::share_family_sources(plan.clone());
    let charset = plan::read_charset(&shared, &work.plan)?;
    let flaws = build::check_sources(&build::select_entries(&shared)?, &work.plan, &charset)?;
    let generator = prepare_generator(game, progress)?;
    let options = Options {
        raw_note: typetrees::describe_raw_reading(game),
    };
    let built = build::build(
        &shared,
        game,
        work,
        generator.as_ref(),
        &charset,
        &options,
        progress,
    )?;
    progress(Progress {
        done: 0,
        total: 1,
        step: "Copying the new files into the game".to_string(),
    });
    project::install(work, &game.data_dir)?;
    Ok(collect_results(
        &shared,
        work,
        &built,
        &[borrowed, flaws].concat(),
    ))
}

fn collect_results(
    plan: &Plan,
    work: &Project,
    built: &Built,
    lines: &[String],
) -> Vec<FontResult> {
    plan.fonts
        .iter()
        .filter(|entry| !entry.skip && !entry.source.is_empty())
        .map(|entry| {
            let source = build::resolve_source(entry, &work.plan)
                .map(|path| file_name(&path))
                .unwrap_or_default();
            let notes = built
                .injected
                .iter()
                .flat_map(|done| describe_done(entry, done))
                .chain(describe_not_baked(entry, built))
                .chain(pick_lines(lines, &[&entry.font, &source]))
                .collect();
            let FontKey { kind, font, family } = FontKey::of(entry);
            FontResult {
                kind,
                font,
                family,
                error: None,
                notes,
            }
        })
        .collect()
}

fn describe_done(entry: &Entry, done: &Done) -> Vec<String> {
    let in_places = |file: &str| entry.places.iter().any(|place| place.file == file);
    match done {
        Done::Injected(item) if item.font == entry.font && in_places(&item.file) => [
            describe_chars("lost characters the game font had", &item.lost),
            (item.resized && item.materials == 0).then(|| {
                "the atlas changed size and no material uses it; check the text in the game"
                    .to_string()
            }),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Done::Swapped(item) if item.font == entry.font && in_places(&item.file) => {
            describe_chars("lost characters the game font had", &item.lost)
                .into_iter()
                .collect()
        }
        Done::Cleared(item)
            if entry.kind == Kind::Legacy
                && entry.resets.iter().any(|place| place.file == item.file) =>
        {
            vec![format!(
                "{} now draws its letters from the new font",
                item.font
            )]
        }
        _ => Vec::new(),
    }
}

/// Control characters never show in game text, so losing them is not worth a note.
fn describe_chars(what: &str, chars: &str) -> Option<String> {
    let shown: String = chars.chars().filter(|c| !c.is_control()).collect();
    (!shown.is_empty()).then(|| format!("{what}: {}", charsets::format_chars(&shown)))
}

fn describe_not_baked(entry: &Entry, built: &Built) -> Option<String> {
    let chars = built.not_baked.get(&entry.font)?;
    describe_chars("the new font has no glyph for", chars)
}

/// Build notes start with the font or the source file they are about.
fn pick_lines(lines: &[String], names: &[&str]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| {
            names
                .iter()
                .filter(|name| !name.is_empty())
                .find_map(|name| line.strip_prefix(&format!("{name}: ")))
                .map(str::to_string)
        })
        .collect()
}

/// An error that names a font belongs to it; any other error stops every font that was to change.
fn blame(plan: &Plan, wanted: &Wanted, error: &str) -> Vec<FontResult> {
    let named: Vec<&Entry> = plan
        .fonts
        .iter()
        .filter(|entry| error.starts_with(&format!("{}: ", entry.font)))
        .collect();
    let failed: Vec<&Entry> = match named.is_empty() {
        true => plan
            .fonts
            .iter()
            .filter(|entry| wanted.contains_key(&FontKey::of(entry)))
            .collect(),
        false => named,
    };
    failed
        .into_iter()
        .map(|entry| {
            let FontKey { kind, font, family } = FontKey::of(entry);
            FontResult {
                kind,
                font,
                family,
                error: Some(error.to_string()),
                notes: Vec::new(),
            }
        })
        .collect()
}

fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("Cannot delete {}", path.display()))
        }
        _ => Ok(()),
    }
}

fn remove_dir(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("Cannot delete {}", path.display()))
        }
        _ => Ok(()),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::library::make_id;
    use crate::session::state::tests::{entry, plan_of};
    use crate::session::{Backend, GameRecord};
    use serde_json::Value;
    use std::path::PathBuf;

    const FONT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/make-font-ABC.ttf");

    /// A serialized-file header (format 17) that names the Unity version.
    fn fake_globalgamemanagers() -> Vec<u8> {
        let mut head = vec![0u8; 256];
        for (at, value) in [(0, 100u32), (4, 256), (8, 17), (12, 200)] {
            head[at..at + 4].copy_from_slice(&value.to_be_bytes());
        }
        head[20..32].copy_from_slice(b"2020.3.12f1\0");
        head
    }

    fn patched_game(
        root: &Path,
        files: &[(&str, &str)],
    ) -> (PathBuf, GameRecord, PathBuf, Project) {
        let (home, folder) = (root.join("ufk-data"), root.join("Demo"));
        let data = folder.join("Demo_Data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("globalgamemanagers"), fake_globalgamemanagers()).unwrap();
        let record = GameRecord {
            id: make_id("Demo", &folder),
            folder: folder.clone(),
            name: "Demo".to_string(),
            unity_version: "2020.3.12f1".to_string(),
            backend: Backend::Mono,
        };
        let work = open_work(&home, &record).unwrap();
        std::fs::create_dir_all(&work.backup).unwrap();
        let mut installed = project::Hashes::new();
        for (file, original) in files {
            std::fs::write(data.join(file), "patched").unwrap();
            std::fs::write(work.backup.join(file), original).unwrap();
            let digest = project::hash_file(&data.join(file)).unwrap();
            installed.insert(file.to_string(), Value::String(digest));
        }
        project::write_installed(&work, &installed).unwrap();
        (home, record, data, work)
    }

    #[test]
    fn apply_does_nothing_when_the_game_has_the_chosen_fonts() {
        let tmp = tempfile::tempdir().unwrap();
        let (home, record, data, work) =
            patched_game(tmp.path(), &[("resources.assets", "original")]);
        let place = plan::Place {
            file: "resources.assets".to_string(),
            path_id: 1,
            cab: String::new(),
        };
        let body = Entry {
            places: vec![place],
            ..entry("Body SDF", FONT, false)
        };
        let plan = plan_of(vec![body]);
        project::write_text(&work.plan, &plan::to_json(&plan).unwrap()).unwrap();
        let wanted = list_wanted(&plan, &work.plan);
        let saved = Saved {
            applied: to_choices(&wanted),
            ..Saved::default()
        };
        state::store_saved(&work, &saved).unwrap();
        let view = open_game_in(&home, &record).unwrap();
        // resources.assets holds no font, so a build would fail: only a skipped apply succeeds.
        let after = apply(&view, &crate::progress::silent).unwrap();
        assert_eq!(after, view);
        assert_eq!(
            std::fs::read_to_string(data.join("resources.assets")).unwrap(),
            "patched"
        );
    }

    #[test]
    fn accepting_an_update_keeps_the_new_files_and_the_originals_of_files_still_patched() {
        let tmp = tempfile::tempdir().unwrap();
        let files = [
            ("resources.assets", "old original"),
            ("sharedassets0.assets", "original"),
        ];
        let (home, record, data, work) = patched_game(tmp.path(), &files);
        std::fs::write(data.join("resources.assets"), "updated").unwrap();
        let view = open_game_in(&home, &record).unwrap();
        assert!(matches!(view.state, GameState::Updated { .. }));
        accept_update(&view).unwrap();
        project::restore(&work, &data).unwrap();
        let read = |file: &str| std::fs::read_to_string(data.join(file)).unwrap();
        assert_eq!(
            (read("resources.assets"), read("sharedassets0.assets")),
            ("updated".to_string(), "original".to_string())
        );
    }
}
