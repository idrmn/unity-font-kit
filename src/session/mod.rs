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

pub use crate::game::folder::{Backend, ScanDepth};
pub use crate::progress::Progress;
pub use apply::{accept_update, apply, can_write, is_game_running, remove_game_data, restore};
pub use atlas::atlas_png;
pub use export::{can_export, export};
pub use font_files::save_font_files;
pub use letters::{Letters, join_text, letters_from_preset, read_custom_letters, split_text};
pub use library::{
    GameRecord, Library, add_game, add_record, find_games, load_library, read_game, remove_game,
    save_library,
};
pub use numbers::{mark_fonts, unmark_fonts};
pub use picker::{FontOption, Origin, add_font_file, list_options};
pub(crate) use scanning::{forget_scan, has_scan_cache};
pub use scanning::{recheck, scan};
pub use state::{
    FontStatus, GameState, GameStatus, GameView, Replacement, clear_choices, open_game,
    read_status, set_source, status_of,
};

/// Kind, font name, family.
pub use crate::work::plan::FontKey;
