use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use freya::prelude::{State, use_hook, use_state};

use super::sort::Sort;
use crate::session::{self, FontKey, FontOption, GameView, Letters, Library, Progress};

#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    None,
    Picker(FontKey),
    Folders,
    Updated(Vec<String>),
    Notice(String),
    RemoveGame(session::GameRecord),
    SaveFonts,
    Letters,
    Mark,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Pick {
    #[default]
    Unset,
    Original,
    File(PathBuf),
}

impl Pick {
    #[must_use]
    pub fn file(&self) -> Option<&Path> {
        match self {
            Pick::File(path) => Some(path),
            Pick::Unset | Pick::Original => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Ui {
    pub library: State<Library>,
    pub selected: State<Option<String>>,
    pub view: State<Option<GameView>>,
    pub preset: State<String>,
    pub chars: State<String>,
    pub busy: State<Option<Progress>>,
    pub stoppable: State<bool>,
    pub error: State<Option<String>>,
    pub done: State<Option<String>>,
    pub dialog: State<Dialog>,
    pub options: State<Option<Vec<FontOption>>>,
    pub picked: State<Pick>,
    /// Fonts ticked in the dialog that saves font files.
    pub saved_fonts: State<Vec<FontKey>>,
    pub found_games: State<Vec<PathBuf>>,
    pub search: State<String>,
    /// The letters being edited in the Custom dialog.
    pub letters_draft: State<String>,
    /// The game's own letters, kept while a preset is chosen.
    pub custom_letters: State<String>,
    /// Fonts ticked in the Mark dialog.
    pub marked: State<Vec<FontKey>>,
    pub sort: State<Sort>,
    pub statuses: State<HashMap<String, session::GameStatus>>,
}

impl fmt::Debug for Ui {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Ui").finish_non_exhaustive()
    }
}

impl Ui {
    #[must_use]
    pub fn current_letters(&self) -> Letters {
        session::split_text(&self.chars.read(), &self.preset.read())
    }
}

pub fn use_ui() -> Ui {
    let loaded = use_hook(|| session::load_library().map_err(|error| format!("{error:#}")));
    let uk = use_hook(|| {
        session::letters_from_preset("uk").unwrap_or(Letters {
            preset: "uk".to_string(),
            letters: String::new(),
            marks: String::new(),
        })
    });
    Ui {
        library: use_state(|| loaded.clone().unwrap_or_default()),
        selected: use_state(|| None),
        view: use_state(|| None),
        preset: use_state(|| "uk".to_string()),
        chars: use_state(|| session::join_text(&uk)),
        busy: use_state(|| None),
        stoppable: use_state(|| false),
        error: use_state(|| loaded.clone().err()),
        done: use_state(|| None),
        dialog: use_state(|| Dialog::None),
        options: use_state(|| None),
        picked: use_state(Pick::default),
        saved_fonts: use_state(Vec::new),
        found_games: use_state(Vec::new),
        search: use_state(String::new),
        letters_draft: use_state(String::new),
        custom_letters: use_state(String::new),
        marked: use_state(Vec::new),
        sort: use_state(Sort::default),
        statuses: use_state(HashMap::new),
    }
}
