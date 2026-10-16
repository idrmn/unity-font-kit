mod apply;
mod atlas;
mod export;
mod font_files;
mod letters;
mod library;
mod numbers;
mod picker;
mod scanning;
mod state;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use crate::game::folder::Backend;
pub use crate::progress::Progress;
use crate::work::plan::Plan;
pub use apply::{accept_update, apply, can_write, is_game_running, remove_game_data, restore};
pub use atlas::atlas_png;
pub use export::{can_export, export};
pub use font_files::save_font_files;
pub use letters::{join_text, letters_from_preset, read_custom_letters, split_text};
pub use library::{
    add_game, add_record, find_games, load_library, read_game, remove_game, save_library,
};
pub use numbers::{mark_fonts, unmark_fonts};
pub use picker::{add_font_file, list_options};
pub(crate) use scanning::{forget_scan, has_scan_cache};
pub use scanning::{recheck, scan};
pub use state::{GameStatus, clear_choices, open_game, read_status, set_source, status_of};

/// Kind, font name, family.
pub use crate::work::plan::FontKey;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameRecord {
    pub id: String,
    /// The folder that holds the game's data folder.
    pub folder: PathBuf,
    pub name: String,
    pub unity_version: String,
    pub backend: Backend,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub games: Vec<GameRecord>,
    pub folders: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameState {
    Original,
    Applied,
    /// Files ufk installed changed since, usually by a game update.
    Updated {
        files: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontStatus {
    Unchanged,
    Chosen,
    Applied,
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameView {
    pub record: GameRecord,
    pub state: GameState,
    pub plan: Option<Plan>,
    pub statuses: HashMap<FontKey, FontStatus>,
    pub notes: HashMap<FontKey, Vec<String>>,
    pub replacements: HashMap<FontKey, Replacement>,
    pub scan_errors: Vec<String>,
    /// Letters can be checked against the last scan instead of reading the game again.
    pub rechecks: bool,
    /// The number each font draws in the game; empty when the game shows letters.
    pub numbers: HashMap<FontKey, usize>,
    pub letters: Letters,
    /// The data folder this view was read from.
    pub home: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    pub path: PathBuf,
    /// Letters and marks the file lacks; None when it cannot be read.
    pub lacks: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Letters {
    pub preset: String,
    pub letters: String,
    pub marks: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Game,
    Added,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontOption {
    pub label: String,
    pub path: PathBuf,
    pub origin: Origin,
    pub missing: String,
}
