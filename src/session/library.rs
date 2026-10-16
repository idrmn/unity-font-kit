use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::downloads;
use crate::game::folder::{self, Game};

use super::state;
use super::{GameRecord, Library};

const LIBRARY_FILE: &str = "games.json";
const DATA_SUFFIX: &str = "_Data";
// Long enough to tell game folders apart, short enough for Windows' 260-character paths.
const ID_HASH: usize = 8;
const ID_NAME: usize = 40;

pub fn load_library() -> Result<Library> {
    read_library(&downloads::find_home()?)
}

pub fn save_library(library: &Library) -> Result<()> {
    write_library(&downloads::find_home()?, library)
}

pub(crate) fn read_library(home: &Path) -> Result<Library> {
    let path = home.join(LIBRARY_FILE);
    if !path.exists() {
        return Ok(Library::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("Cannot read {}", path.display()))
}

pub(crate) fn write_library(home: &Path, library: &Library) -> Result<()> {
    state::write_json(&home.join(LIBRARY_FILE), library)
}

pub fn add_game(library: &Library, path: &Path) -> Result<(Library, GameRecord)> {
    Ok(add_record(library, read_game(path)?))
}

/// Reading a game takes a while; the library it goes into is taken only once it is read.
pub fn read_game(path: &Path) -> Result<GameRecord> {
    Ok(make_record(&folder::locate(path)?))
}

pub fn add_record(library: &Library, record: GameRecord) -> (Library, GameRecord) {
    if let Some(known) = library.games.iter().find(|game| game.id == record.id) {
        return (library.clone(), known.clone());
    }
    let mut games = library.games.clone();
    games.push(record.clone());
    (
        Library {
            games,
            ..library.clone()
        },
        record,
    )
}

/// The project stays, so adding the game again brings its choices and backups back.
pub fn remove_game(library: &Library, id: &str) -> Result<Library> {
    if !library.games.iter().any(|game| game.id == id) {
        bail!("No game {id} in the list");
    }
    Ok(Library {
        games: library
            .games
            .iter()
            .filter(|game| game.id != id)
            .cloned()
            .collect(),
        ..library.clone()
    })
}

// Steam keeps games three levels below the library folder: steamapps/common/<game>.
const SEARCH_DEPTH: usize = 3;

pub fn find_games(folders: &[PathBuf]) -> Vec<PathBuf> {
    let mut games: Vec<PathBuf> = folders
        .iter()
        .flat_map(|root| find_below(root, SEARCH_DEPTH))
        .collect();
    games.sort();
    games.dedup();
    games
}

/// A game folder is not searched further: its own folders are not games.
fn find_below(dir: &Path, depth: usize) -> Vec<PathBuf> {
    if depth == 0 {
        return Vec::new();
    }
    list_dirs(dir)
        .into_iter()
        .flat_map(|child| match is_game_folder(&child) {
            true => vec![child],
            false => find_below(&child, depth - 1),
        })
        .collect()
}

fn list_dirs(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

fn is_game_folder(dir: &Path) -> bool {
    !folder::is_data_dir(dir) && folder::find_data_dir(dir).is_ok()
}

pub(crate) fn make_record(game: &Game) -> GameRecord {
    let folder = game
        .data_dir
        .parent()
        .unwrap_or(&game.data_dir)
        .to_path_buf();
    let name = name_game(&game.data_dir, &folder);
    GameRecord {
        id: make_id(&name, &folder),
        folder,
        name,
        unity_version: game.unity_version.clone(),
        backend: game.backend,
    }
}

/// The data folder is named after the executable; the game folder's name reads as people write it.
fn name_game(data_dir: &Path, folder: &Path) -> String {
    let named = file_name(folder);
    if !named.is_empty() {
        return named;
    }
    let data = file_name(data_dir);
    data.strip_suffix(DATA_SUFFIX).unwrap_or(&data).to_string()
}

pub(crate) fn make_id(name: &str, folder: &Path) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(ID_NAME)
        .collect();
    let digest: String = Sha256::digest(folder.to_string_lossy().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let hash = &digest[..ID_HASH];
    match slug.is_empty() {
        true => format!("game-{hash}"),
        false => format!("{slug}-{hash}"),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_file_is_an_empty_library() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(read_library(home.path()).unwrap(), Library::default());
    }
}
