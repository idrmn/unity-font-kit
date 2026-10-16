mod dialogs;
mod filters;
mod game;
mod icons;
mod sidebar;
mod tasks;
mod text_area;
pub mod theme;

use std::collections::HashMap;

use freya::prelude::{
    ChildrenExt, Component, ContainerPositionExt, ContainerSizeExt, ContainerWithContentExt,
    Content, Event, EventHandlersExt, IntoElement, LaunchConfig, Layer, LayerExt, Position,
    PressEventData, Size, State, StyleExt, TextStyleExt, WindowConfig, WritableUtils, launch, rect,
    spawn, use_hook, use_init_theme, use_side_effect, use_state,
};

use crate::session::{self, FontKey, FontOption, GameView, Letters, Library, Progress};
use theme::{CONTENT, TEXT};

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
    /// None until something is picked; Some(None) keeps the game's own font.
    pub picked: State<Option<Option<std::path::PathBuf>>>,
    /// Fonts ticked in the dialog that saves font files.
    pub saved_fonts: State<Vec<FontKey>>,
    pub found_games: State<Vec<std::path::PathBuf>>,
    pub search: State<String>,
    /// The letters being edited in the Custom dialog.
    pub letters_draft: State<String>,
    /// The game's own letters, kept while a preset is chosen.
    pub custom_letters: State<String>,
    /// Fonts ticked in the Mark dialog.
    pub marked: State<Vec<FontKey>>,
    pub sort: State<filters::Sort>,
    pub statuses: State<HashMap<String, session::GameStatus>>,
}

pub fn run() {
    launch(
        theme::FONTS
            .iter()
            .fold(LaunchConfig::new(), |config, (name, data)| {
                config.with_font(*name, *data)
            })
            .with_default_font(theme::FONT)
            .with_window(
                WindowConfig::new(app)
                    .with_title("ufk")
                    .with_size(1180., 700.)
                    .with_min_size(1180., 560.),
            ),
    )
}

pub fn app() -> impl IntoElement {
    use_init_theme(theme::build);
    let ui = use_ui();
    use_side_effect(move || recheck_letters(ui));
    use_side_effect(move || note_statuses(ui));
    rect()
        .expanded()
        .horizontal()
        .content(Content::flex())
        .background(CONTENT)
        .color(TEXT)
        .font_family(theme::FONT)
        .font_size(theme::BODY)
        .child(sidebar::view(ui))
        .child(game::view(ui))
        .child(dialogs::view(ui))
        .child(Blocker { ui })
}

/// While a task runs nothing in the window changes; only Stop sits above this layer.
#[derive(Clone, PartialEq)]
struct Blocker {
    ui: Ui,
}

pub const BLOCKER_LAYER: u8 = 1;

impl Component for Blocker {
    fn render(&self) -> impl IntoElement {
        let busy = self.ui.busy.read().is_some();
        rect()
            .layer(Layer::OverlayLevel(BLOCKER_LAYER))
            .position(Position::new_global().top(0.).left(0.))
            .maybe_child(busy.then(|| {
                rect()
                    .width(Size::window_percent(100.))
                    .height(Size::window_percent(100.))
                    .on_press(|event: Event<PressEventData>| event.stop_propagation())
            }))
    }
}

/// The open game's status comes from its view; the others are read once, without hashing files.
fn note_statuses(ui: Ui) {
    let games = ui.library.read().games.clone();
    let open = ui
        .view
        .read()
        .as_ref()
        .map(|view| (view.record.id.clone(), session::status_of(view)));
    let mut statuses = ui.statuses;
    let known = statuses.peek().clone();
    let mut next: HashMap<String, session::GameStatus> = games
        .iter()
        .map(|game| {
            let status = known
                .get(&game.id)
                .copied()
                .unwrap_or_else(|| session::read_status(game));
            (game.id.clone(), status)
        })
        .collect();
    next.extend(open);
    if next != known {
        statuses.set(next);
    }
}

impl std::fmt::Debug for Ui {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ui").finish_non_exhaustive()
    }
}

impl Ui {
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
        picked: use_state(|| None),
        saved_fonts: use_state(Vec::new),
        found_games: use_state(Vec::new),
        search: use_state(String::new),
        letters_draft: use_state(String::new),
        custom_letters: use_state(String::new),
        marked: use_state(Vec::new),
        sort: use_state(filters::Sort::default),
        statuses: use_state(HashMap::new),
    }
}

// A scan or apply in progress rewrites the plan, so the letters wait for it to end.
const BUSY_CHECK: std::time::Duration = std::time::Duration::from_millis(300);

fn recheck_letters(ui: Ui) {
    let typed = (ui.preset.read().clone(), ui.chars.read().clone());
    spawn(async move {
        while ui.busy.peek().is_some() {
            blocking::unblock(move || std::thread::sleep(BUSY_CHECK)).await;
        }
        let now = (ui.preset.peek().clone(), ui.chars.peek().clone());
        let Some(game) = ui.view.peek().clone() else {
            return;
        };
        let checked = (
            game.letters.preset.clone(),
            session::join_text(&game.letters),
        );
        if now != typed || now == checked || game.plan.is_none() {
            return;
        }
        if !game.rechecks {
            game::scan(ui, false);
            return;
        }
        let letters = session::split_text(&now.1, &now.0);
        let record = game.record.clone();
        let mut view = ui.view;
        tasks::run(
            ui,
            "Checking the letters",
            move |_| session::recheck(&record, &letters),
            move |checked| view.set(Some(checked)),
        );
    });
}
