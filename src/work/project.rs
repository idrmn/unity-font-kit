use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::downloads;
use crate::game::addressables;

pub const SUFFIX: &str = ".ufk.json";
const PARTIAL: &str = ".partial";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub plan: PathBuf,
    pub work: PathBuf,
    pub fonts: PathBuf,
    pub recipes: PathBuf,
    pub patched: PathBuf,
    pub backup: PathBuf,
    pub installed: PathBuf,
    pub built: PathBuf,
}

pub type Hashes = Map<String, Value>;

pub fn open_project(plan: &Path) -> Result<Project> {
    let name = file_name(plan);
    let Some(stem) = name.strip_suffix(SUFFIX) else {
        bail!("{name} is not a ufk plan, its name must end with {SUFFIX}");
    };
    let work = plan
        .parent()
        .unwrap_or(Path::new(""))
        .join(format!("{stem}.ufk"));
    Ok(Project {
        plan: plan.to_path_buf(),
        fonts: work.join("fonts"),
        recipes: work.join("recipes"),
        patched: work.join("patched"),
        backup: work.join("backup"),
        installed: work.join("installed.json"),
        built: work.join("built.json"),
        work,
    })
}

pub fn list_files(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_files(root, "", &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files(dir: &Path, prefix: &str, files: &mut Vec<String>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("Cannot read {}", dir.display())),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let relative = format!("{prefix}{}", entry.file_name().to_string_lossy());
        // Links to folders are skipped: they could loop or lead outside the folder.
        if entry.file_type()?.is_dir() {
            collect_files(&path, &format!("{relative}/"), files)?;
        } else if path.is_file() && !is_partial(&path) {
            files.push(relative);
        }
    }
    Ok(())
}

fn is_partial(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == &PARTIAL[1..])
}

pub fn hash_file(path: &Path) -> Result<String> {
    if !path.is_file() {
        return Ok(String::new());
    }
    downloads::hash_files(&[path])
}

pub fn read_installed(project: &Project) -> Result<Hashes> {
    read_hashes(&project.installed)
}

fn read_hashes(path: &Path) -> Result<Hashes> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    match serde_json::from_str(&text).with_context(|| format!("Cannot read {}", path.display()))? {
        Value::Object(hashes) => Ok(hashes),
        _ => bail!("{} is not a list of file hashes", path.display()),
    }
}

fn is_recorded(hashes: &Hashes, file: &str, digest: &str) -> bool {
    hashes.get(file).and_then(Value::as_str) == Some(digest)
}

pub fn reject_changed_game(project: &Project, data_dir: &Path) -> Result<()> {
    let installed = read_installed(project)?;
    let mut changed = Vec::new();
    for file in list_files(&project.backup)? {
        let current = hash_file(&data_dir.join(&file))?;
        if !is_recorded(&installed, &file, &current)
            && current != hash_file(&project.backup.join(&file))?
            // An install stopped between copying and recording leaves the game with the patched file.
            && current != hash_file(&project.patched.join(&file))?
        {
            changed.push(file);
        }
    }
    if !changed.is_empty() {
        bail!(
            "Game files changed since the last install, by a game update or another tool: {}. \
             Choose Use new game files… to take them as originals",
            changed.join(", ")
        );
    }
    Ok(())
}

pub fn hash_originals(project: &Project, data_dir: &Path, files: &[String]) -> Result<Hashes> {
    files
        .iter()
        .map(|file| {
            let original = original_file(Some(&project.backup), data_dir, file);
            Ok((file.clone(), Value::String(hash_file(&original)?)))
        })
        .collect()
}

pub fn write_build_sources(project: &Project, sources: &Hashes) -> Result<()> {
    let built = list_files(&project.patched)?
        .into_iter()
        .map(|file| match sources.get(&file) {
            Some(digest) => Ok((file, digest.clone())),
            None => bail!("No hash of the original {file}"),
        })
        .collect::<Result<Hashes>>()?;
    write_text(&project.built, &format_hashes(&built)?)
}

pub fn read_build_sources(project: &Project) -> Result<Hashes> {
    read_hashes(&project.built)
}

pub fn reject_changed_since_build(
    project: &Project,
    data_dir: &Path,
    files: &[String],
) -> Result<()> {
    let sources = read_build_sources(project)?;
    let mut changed = Vec::new();
    for file in files {
        // Files with a backup are guarded by reject_changed_game; the rest must still be what build read.
        if !project.backup.join(file).exists()
            && !is_recorded(&sources, file, &hash_file(&data_dir.join(file))?)
        {
            changed.push(file.as_str());
        }
    }
    if !changed.is_empty() {
        bail!(
            "Game files changed after the fonts were built, for example by a translation tool: {}. \
             Apply to game again",
            changed.join(", ")
        );
    }
    Ok(())
}

pub fn original_file(backup: Option<&Path>, data_dir: &Path, file: &str) -> PathBuf {
    let backed_up = backup.map(|dir| dir.join(file));
    match backed_up {
        Some(path) if path.is_file() => path,
        _ => data_dir.join(file),
    }
}

pub fn copy_atomically(source: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    let partial = partial_path(target);
    let copied = copy_durably(source, &partial).and_then(|()| std::fs::rename(&partial, target));
    let Err(error) = copied else {
        return Ok(());
    };
    // A failed copy may leave a partial file as big as the original in the game folder.
    let _ = std::fs::remove_file(&partial);
    if is_permission_error(&error) {
        return Err(error).with_context(|| {
            format!(
                "Cannot write {}: close the game and try again. \
                 If the game is in Program Files, run ufk as administrator",
                target.display()
            )
        });
    }
    Err(error).with_context(|| format!("Cannot write {}", target.display()))
}

/// On Windows a file the running game holds open gives a sharing or lock violation.
fn is_permission_error(error: &io::Error) -> bool {
    const SHARING_VIOLATION: i32 = 32;
    const LOCK_VIOLATION: i32 = 33;
    error.kind() == ErrorKind::PermissionDenied
        || (cfg!(windows)
            && matches!(
                error.raw_os_error(),
                Some(SHARING_VIOLATION | LOCK_VIOLATION)
            ))
}

/// Contents reach the disk before the rename, so a power loss leaves the old file or the new one.
fn copy_durably(source: &Path, target: &Path) -> io::Result<()> {
    let mut from = File::open(source)?;
    let mut to = File::create(target)?;
    io::copy(&mut from, &mut to)?;
    to.set_modified(from.metadata()?.modified()?)?;
    to.sync_all()?;
    drop(to);
    std::fs::set_permissions(target, from.metadata()?.permissions())
}

fn partial_path(path: &Path) -> PathBuf {
    path.with_file_name(format!("{}{PARTIAL}", file_name(path)))
}

pub fn write_installed(project: &Project, installed: &Hashes) -> Result<()> {
    write_text(&project.installed, &format_hashes(installed)?)
}

pub fn install(project: &Project, data_dir: &Path) -> Result<Vec<String>> {
    install_with(project, data_dir, copy_atomically)
}

fn install_with(
    project: &Project,
    data_dir: &Path,
    copy: impl Fn(&Path, &Path) -> Result<()>,
) -> Result<Vec<String>> {
    let files = catalogs_first(list_files(&project.patched)?);
    if files.is_empty() {
        bail!("Nothing to install in {}", project.patched.display());
    }
    reject_changed_game(project, data_dir)?;
    reject_changed_since_build(project, data_dir, &files)?;
    let mut installed = read_installed(project)?;
    for file in &files {
        // Only the first install backs up: later the game file is already patched.
        if !project.backup.join(file).exists() {
            copy(&data_dir.join(file), &project.backup.join(file))?;
        }
        copy(&project.patched.join(file), &data_dir.join(file))?;
        // Recorded per file so an interrupted install is not mistaken for a game update.
        let digest = hash_file(&project.patched.join(file))?;
        installed.insert(file.clone(), Value::String(digest));
        write_installed(project, &installed)?;
    }
    let patched: BTreeSet<&String> = files.iter().collect();
    for file in list_files(&project.backup)? {
        if !patched.contains(&file) {
            copy(&project.backup.join(&file), &data_dir.join(&file))?;
        }
    }
    let kept: Hashes = files
        .iter()
        .map(|file| {
            (
                file.clone(),
                installed.get(file).cloned().unwrap_or_default(),
            )
        })
        .collect();
    write_installed(project, &kept)?;
    Ok(files)
}

pub fn restore(project: &Project, data_dir: &Path) -> Result<Vec<String>> {
    let mut files = catalogs_first(list_files(&project.backup)?);
    if files.is_empty() {
        bail!(
            "No backup in {}, nothing to restore",
            project.backup.display()
        );
    }
    reject_changed_game(project, data_dir)?;
    files.reverse();
    for file in &files {
        copy_atomically(&project.backup.join(file), &data_dir.join(file))?;
    }
    remove_if_present(&project.installed)?;
    // The game holds the originals again; an old backup would block later changes by other tools.
    std::fs::remove_dir_all(&project.backup)
        .with_context(|| format!("Cannot delete {}", project.backup.display()))?;
    Ok(files)
}

fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => {
            Err(error).with_context(|| format!("Cannot delete {}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Text files get the system's line ends; the old file stays whole until the new one is on disk.
pub fn write_text(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let text = if cfg!(windows) {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    };
    let partial = partial_path(path);
    let written = File::create(&partial)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&partial, path));
    written.with_context(|| format!("Cannot write {}", path.display()))
}

fn catalogs_first(files: Vec<String>) -> Vec<String> {
    let (catalogs, rest): (Vec<String>, Vec<String>) = files
        .into_iter()
        .partition(|file| addressables::is_catalog_file(file));
    [catalogs, rest].concat()
}

pub fn format_hashes(hashes: &Hashes) -> Result<String> {
    Ok(serde_json::to_string_pretty(hashes)?)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn make_game(root: &Path) -> (PathBuf, Project) {
        let data = root.join("Game_Data");
        std::fs::create_dir(&data).unwrap();
        for name in ["resources.assets", "sharedassets0.assets"] {
            std::fs::write(data.join(name), format!("original {name}")).unwrap();
        }
        let work = open_project(&root.join("Game.ufk.json")).unwrap();
        std::fs::create_dir_all(&work.patched).unwrap();
        (data, work)
    }

    pub(crate) fn build_patch(work: &Project, data: &Path, names: &[&str]) {
        let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        let read_hashes = hash_originals(work, data, &names).unwrap();
        for name in &names {
            std::fs::write(work.patched.join(name), "patched").unwrap();
        }
        write_build_sources(work, &read_hashes).unwrap();
    }

    fn read(path: PathBuf) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn install_backs_up_the_original_once_and_restore_brings_it_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = make_game(tmp.path());
        build_patch(&work, &data, &["resources.assets"]);
        install(&work, &data).unwrap();
        install(&work, &data).unwrap();
        assert_eq!(
            read(work.backup.join("resources.assets")),
            "original resources.assets"
        );
        restore(&work, &data).unwrap();
        assert_eq!(
            read(data.join("resources.assets")),
            "original resources.assets"
        );
    }

    #[test]
    fn install_puts_back_files_no_longer_patched() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = make_game(tmp.path());
        build_patch(&work, &data, &["resources.assets", "sharedassets0.assets"]);
        install(&work, &data).unwrap();
        std::fs::remove_file(work.patched.join("resources.assets")).unwrap();
        install(&work, &data).unwrap();
        assert_eq!(
            read(data.join("resources.assets")),
            "original resources.assets"
        );
    }

    #[test]
    fn an_interrupted_install_can_be_run_again() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = make_game(tmp.path());
        build_patch(&work, &data, &["resources.assets", "sharedassets0.assets"]);
        let fail_on_second_game_file = |source: &Path, target: &Path| {
            if target == data.join("sharedassets0.assets") {
                bail!("disk full");
            }
            copy_atomically(source, target)
        };
        assert!(install_with(&work, &data, fail_on_second_game_file).is_err());
        install(&work, &data).unwrap();
        assert_eq!(read(data.join("sharedassets0.assets")), "patched");
    }

    #[test]
    fn install_writes_catalogs_before_the_bundles_they_check() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = make_game(tmp.path());
        let names = [
            "StreamingAssets/aa/StandaloneWindows64/fonts.bundle",
            "StreamingAssets/aa/catalog.json",
        ];
        for name in names {
            std::fs::create_dir_all(data.join(name).parent().unwrap()).unwrap();
            std::fs::create_dir_all(work.patched.join(name).parent().unwrap()).unwrap();
            std::fs::write(data.join(name), "original").unwrap();
        }
        build_patch(&work, &data, &names);
        let order = std::cell::RefCell::new(Vec::new());
        let record = |source: &Path, target: &Path| {
            if target.starts_with(&data) {
                order
                    .borrow_mut()
                    .push(target.strip_prefix(&data).unwrap().to_path_buf());
            }
            copy_atomically(source, target)
        };
        install_with(&work, &data, record).unwrap();
        assert_eq!(
            order.into_inner(),
            [PathBuf::from(names[1]), PathBuf::from(names[0])]
        );
    }

    #[test]
    fn a_game_update_after_install_stops_the_next_install() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = make_game(tmp.path());
        build_patch(&work, &data, &["resources.assets"]);
        install(&work, &data).unwrap();
        std::fs::write(data.join("resources.assets"), "updated by the game").unwrap();
        let error = install(&work, &data).unwrap_err().to_string();
        assert!(error.contains("game update"), "{error}");
    }

    #[test]
    fn a_game_file_changed_after_build_stops_the_first_install() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = make_game(tmp.path());
        build_patch(&work, &data, &["resources.assets"]);
        std::fs::write(data.join("resources.assets"), "translated").unwrap();
        let error = install(&work, &data).unwrap_err().to_string();
        assert!(error.contains("after the fonts were built"), "{error}");
        assert_eq!(read(data.join("resources.assets")), "translated");
    }

    #[test]
    fn a_failed_reinstall_is_not_taken_for_a_game_update() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = make_game(tmp.path());
        build_patch(&work, &data, &["resources.assets"]);
        install(&work, &data).unwrap();
        std::fs::write(work.patched.join("resources.assets"), "patched again").unwrap();
        let locked = |_: &Path, _: &Path| -> Result<()> { bail!("locked") };
        assert!(install_with(&work, &data, locked).is_err());
        install(&work, &data).unwrap();
        assert_eq!(read(data.join("resources.assets")), "patched again");
    }

    #[test]
    fn after_restore_other_tools_may_change_the_game() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = make_game(tmp.path());
        build_patch(&work, &data, &["resources.assets"]);
        install(&work, &data).unwrap();
        restore(&work, &data).unwrap();
        std::fs::write(data.join("resources.assets"), "translated").unwrap();
        reject_changed_game(&work, &data).unwrap();
    }
}
