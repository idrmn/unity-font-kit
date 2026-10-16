use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::downloads;
use crate::fonts::font_file;
use crate::game::folder;
use crate::work::build;
use crate::work::plan::{self, Entry, Kind, Plan};
use crate::work::project::{self, Project};

use super::letters::{read_letters, to_charset};
use super::{FontKey, FontStatus, GameRecord, GameState, GameView, Letters, Replacement};

const PROJECTS: &str = "projects";
const STATE_FILE: &str = "state.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Saved {
    pub applied: Vec<Choice>,
    pub results: Vec<FontResult>,
    pub scan_errors: Vec<String>,
    #[serde(default)]
    pub deep_scan: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Choice {
    pub kind: Kind,
    pub font: String,
    pub family: String,
    pub source: String,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FontResult {
    pub kind: Kind,
    pub font: String,
    pub family: String,
    pub error: Option<String>,
    pub notes: Vec<String>,
}

pub(crate) type Wanted = HashMap<FontKey, (String, String)>;

pub fn open_game(record: &GameRecord) -> Result<GameView> {
    open_game_in(&downloads::find_home()?, record)
}

pub(crate) fn open_game_in(home: &Path, record: &GameRecord) -> Result<GameView> {
    let work = open_work(home, record)?;
    let game = folder::locate(&record.folder)?;
    let state = detect_state(&work, &game.data_dir)?;
    let plan = match work.plan.is_file() {
        true => Some(plan::load(&work.plan)?),
        false => None,
    };
    let saved = load_saved(&work)?;
    let letters = read_letters(plan.as_ref(), &work.plan)?;
    let view = GameView {
        record: record.clone(),
        state,
        plan,
        statuses: HashMap::new(),
        notes: HashMap::new(),
        replacements: HashMap::new(),
        scan_errors: saved.scan_errors.clone(),
        deep_scan: saved.deep_scan,
        letters,
        home: home.to_path_buf(),
    };
    with_statuses(view, &work, &saved)
}

/// None keeps the game's own font.
pub fn set_source(view: &GameView, key: &FontKey, source: Option<PathBuf>) -> Result<GameView> {
    let work = open_work(&view.home, &view.record)?;
    let text = source.map_or_else(String::new, |path| {
        name_source(&path, &work.plan, &view.home)
    });
    let chosen = !text.is_empty();
    change_entry(view, &work, key, |entry| Entry {
        source: text.clone(),
        skip: !chosen,
        ..entry.clone()
    })
}

fn change_entry(
    view: &GameView,
    work: &Project,
    key: &FontKey,
    change: impl Fn(&Entry) -> Entry,
) -> Result<GameView> {
    let Some(plan) = &view.plan else {
        bail!("Scan the game first");
    };
    if !plan.fonts.iter().any(|entry| FontKey::of(entry) == *key) {
        bail!(
            "{} is no longer in the font list, scan the game again",
            key.font
        );
    }
    let fonts = plan
        .fonts
        .iter()
        .map(|entry| match FontKey::of(entry) == *key {
            true => change(entry),
            false => entry.clone(),
        })
        .collect();
    let plan = Plan {
        fonts,
        ..plan.clone()
    };
    project::write_text(&work.plan, &plan::to_json(&plan)?)?;
    let view = GameView {
        plan: Some(plan),
        ..view.clone()
    };
    with_statuses(view, work, &load_saved(work)?)
}

pub(crate) fn open_work(home: &Path, record: &GameRecord) -> Result<Project> {
    let folder = home.join(PROJECTS).join(&record.id);
    project::open_project(&folder.join(format!("{}{}", record.id, project::SUFFIX)))
}

/// Paths inside the data folder are kept relative to the plan, so the folder can move.
pub(crate) fn name_source(path: &Path, plan_path: &Path, home: &Path) -> String {
    let base = plan_path.parent().unwrap_or(Path::new(""));
    let (Ok(inside), Ok(depth)) = (path.strip_prefix(home), base.strip_prefix(home)) else {
        return path.display().to_string();
    };
    let ups = depth
        .components()
        .filter(|part| matches!(part, Component::Normal(_)))
        .count();
    match inside.strip_prefix(depth) {
        Ok(below) => to_posix(below),
        Err(_) => format!("{}{}", "../".repeat(ups), to_posix(inside)),
    }
}

fn to_posix(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn with_statuses(view: GameView, work: &Project, saved: &Saved) -> Result<GameView> {
    let Some(plan) = &view.plan else {
        return Ok(view);
    };
    let wanted = list_wanted(plan, &work.plan);
    let statuses = compute_statuses(plan, &wanted, saved, view.state == GameState::Applied);
    let replacements = find_replacements(plan, &work.plan, &view.letters);
    let notes = saved
        .results
        .iter()
        .map(|result| (result_key(result), result.notes.clone()))
        .collect();
    Ok(GameView {
        statuses,
        notes,
        replacements,
        ..view
    })
}

fn find_replacements(
    plan: &Plan,
    plan_path: &Path,
    letters: &Letters,
) -> HashMap<FontKey, Replacement> {
    let charset = to_charset(letters).ok();
    let (shared, _) = build::share_family_sources(plan.clone());
    shared
        .fonts
        .iter()
        .filter(|entry| !entry.skip && !entry.source.is_empty())
        .filter_map(|entry| {
            let path = build::resolve_source(entry, plan_path).ok()?;
            let lacks = charset.as_ref().and_then(|charset| {
                let checked = font_file::check(&std::fs::read(&path).ok()?, charset).ok()?;
                Some(format!(
                    "{}{}",
                    checked.missing_letters, checked.missing_typography
                ))
            });
            Some((FontKey::of(entry), Replacement { path, lacks }))
        })
        .collect()
}

/// A missing source file gets an empty hash.
pub(crate) fn list_wanted(plan: &Plan, plan_path: &Path) -> Wanted {
    let (shared, _) = build::share_family_sources(plan.clone());
    shared
        .fonts
        .iter()
        .filter(|entry| !entry.skip && !entry.source.is_empty())
        .map(|entry| {
            let hash = build::resolve_source(entry, plan_path)
                .and_then(|path| project::hash_file(&path))
                .unwrap_or_default();
            (FontKey::of(entry), (entry.source.clone(), hash))
        })
        .collect()
}

pub(crate) fn compute_statuses(
    plan: &Plan,
    wanted: &Wanted,
    saved: &Saved,
    applied_now: bool,
) -> HashMap<FontKey, FontStatus> {
    let applied = to_wanted(&saved.applied);
    let errors: HashMap<FontKey, &String> = saved
        .results
        .iter()
        .filter_map(|result| Some((result_key(result), result.error.as_ref()?)))
        .collect();
    plan.fonts
        .iter()
        .map(|entry| {
            let key = FontKey::of(entry);
            let in_game = applied_now.then(|| applied.get(&key)).flatten();
            let status = match (errors.get(&key), wanted.get(&key), in_game) {
                (Some(error), _, _) => FontStatus::Error((*error).clone()),
                (None, None, None) => FontStatus::Unchanged,
                (None, Some(choice), Some(now)) if choice == now => FontStatus::Applied,
                _ => FontStatus::Chosen,
            };
            (key, status)
        })
        .collect()
}

pub(crate) fn to_wanted(choices: &[Choice]) -> Wanted {
    choices
        .iter()
        .map(|choice| {
            (
                FontKey {
                    kind: choice.kind,
                    font: choice.font.clone(),
                    family: choice.family.clone(),
                },
                (choice.source.clone(), choice.hash.clone()),
            )
        })
        .collect()
}

pub(crate) fn to_choices(wanted: &Wanted) -> Vec<Choice> {
    let mut choices: Vec<Choice> = wanted
        .iter()
        .map(|(FontKey { kind, font, family }, (source, hash))| Choice {
            kind: *kind,
            font: font.clone(),
            family: family.clone(),
            source: source.clone(),
            hash: hash.clone(),
        })
        .collect();
    choices.sort_by(|a, b| (&a.font, &a.family).cmp(&(&b.font, &b.family)));
    choices
}

pub(crate) fn result_key(result: &FontResult) -> FontKey {
    FontKey {
        kind: result.kind,
        font: result.font.clone(),
        family: result.family.clone(),
    }
}

pub(crate) fn detect_state(work: &Project, data_dir: &Path) -> Result<GameState> {
    let updated = find_updated_files(work, data_dir)?;
    if !updated.is_empty() {
        return Ok(GameState::Updated { files: updated });
    }
    match project::read_installed(work)?.is_empty() {
        true => Ok(GameState::Original),
        false => Ok(GameState::Applied),
    }
}

/// Installed files that no longer hold what ufk put in, and backed-up files that are no longer the original.
pub(crate) fn find_updated_files(work: &Project, data_dir: &Path) -> Result<Vec<String>> {
    let installed = project::read_installed(work)?;
    let files: BTreeSet<String> = project::list_files(&work.backup)?
        .into_iter()
        .chain(installed.keys().cloned())
        .collect();
    let mut updated = Vec::new();
    for file in files {
        let expected = match installed.get(&file).and_then(Value::as_str) {
            Some(digest) => digest.to_string(),
            None => project::hash_file(&work.backup.join(&file))?,
        };
        if project::hash_file(&data_dir.join(&file))? != expected {
            updated.push(file);
        }
    }
    Ok(updated)
}

/// Reads no game file, so it is quick.
pub fn has_backup(record: &GameRecord) -> Result<bool> {
    let work = open_work(&downloads::find_home()?, record)?;
    has_applied_files(&work)
}

pub(crate) fn has_applied_files(work: &Project) -> Result<bool> {
    Ok(work.installed.exists() || !project::list_files(&work.backup)?.is_empty())
}

pub(crate) fn load_saved(work: &Project) -> Result<Saved> {
    let path = work.work.join(STATE_FILE);
    if !path.exists() {
        return Ok(Saved::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("Cannot read {}", path.display()))
}

pub(crate) fn store_saved(work: &Project, saved: &Saved) -> Result<()> {
    write_json(&work.work.join(STATE_FILE), saved)
}

/// Written next to the target and renamed, so a crash never leaves half a file.
pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let folder = path.parent().context("No folder to write into")?;
    std::fs::create_dir_all(folder)
        .with_context(|| format!("Cannot create {}", folder.display()))?;
    let partial = path.with_extension("json.partial");
    project::write_text(&partial, &serde_json::to_string_pretty(value)?)?;
    std::fs::rename(&partial, path).with_context(|| format!("Cannot write {}", path.display()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn entry(font: &str, source: &str, skip: bool) -> Entry {
        Entry {
            font: font.to_string(),
            family: String::new(),
            missing: "Ґ".to_string(),
            source: source.to_string(),
            note: String::new(),
            places: Vec::new(),
            skip,
            suggested_source: String::new(),
            suggested_skip: false,
            kind: Kind::Tmp,
            resets: Vec::new(),
            texts: 0,
        }
    }

    pub(crate) fn plan_of(fonts: Vec<Entry>) -> Plan {
        Plan {
            game: String::new(),
            language: "uk".to_string(),
            chars: None,
            fonts,
            not_fixable: Vec::new(),
        }
    }

    fn key(font: &str) -> FontKey {
        FontKey {
            kind: Kind::Tmp,
            font: font.to_string(),
            family: String::new(),
        }
    }

    fn choice(font: &str, source: &str, hash: &str) -> Choice {
        Choice {
            kind: Kind::Tmp,
            font: font.to_string(),
            family: String::new(),
            source: source.to_string(),
            hash: hash.to_string(),
        }
    }

    #[test]
    fn a_font_is_applied_only_with_the_same_source_file_in_the_game() {
        let plan = plan_of(vec![
            entry("Same", "a.ttf", false),
            entry("Edited", "a.ttf", false),
            entry("Other", "b.ttf", false),
            entry("Dropped", "", false),
            entry("Never", "", true),
            entry("Failed", "a.ttf", false),
        ]);
        let wanted: Wanted = [
            (key("Same"), ("a.ttf".to_string(), "h1".to_string())),
            (key("Edited"), ("a.ttf".to_string(), "h2".to_string())),
            (key("Other"), ("b.ttf".to_string(), "h3".to_string())),
            (key("Failed"), ("a.ttf".to_string(), "h1".to_string())),
        ]
        .into();
        let saved = Saved {
            applied: ["Same", "Edited", "Other", "Dropped"]
                .iter()
                .map(|font| choice(font, "a.ttf", "h1"))
                .collect(),
            results: vec![FontResult {
                kind: Kind::Tmp,
                font: "Failed".to_string(),
                family: String::new(),
                error: Some("Failed: broken".to_string()),
                notes: Vec::new(),
            }],
            ..Default::default()
        };
        let statuses = compute_statuses(&plan, &wanted, &saved, true);
        let shown: Vec<FontStatus> = plan
            .fonts
            .iter()
            .map(|entry| statuses[&FontKey::of(entry)].clone())
            .collect();
        assert_eq!(
            shown,
            [
                FontStatus::Applied,
                FontStatus::Chosen,
                FontStatus::Chosen,
                FontStatus::Chosen,
                FontStatus::Unchanged,
                FontStatus::Error("Failed: broken".to_string()),
            ]
        );
        let restored = compute_statuses(&plan, &wanted, &saved, false);
        assert_eq!(restored[&key("Same")], FontStatus::Chosen);
    }

    #[test]
    fn installed_files_changed_by_an_update_are_found() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = project::tests::make_game(tmp.path());
        project::tests::build_patch(&work, &data, &["resources.assets"]);
        assert_eq!(detect_state(&work, &data).unwrap(), GameState::Original);
        project::install(&work, &data).unwrap();
        assert_eq!(detect_state(&work, &data).unwrap(), GameState::Applied);
        std::fs::write(data.join("resources.assets"), "updated").unwrap();
        assert_eq!(
            detect_state(&work, &data).unwrap(),
            GameState::Updated {
                files: vec!["resources.assets".to_string()]
            }
        );
    }
}
