mod dialogs;
mod game;
mod sidebar;
mod tasks;
mod text_area;
pub mod theme;

use freya::prelude::{
    ChildrenExt, ContainerSizeExt, ContainerWithContentExt, Content, IntoElement, LaunchConfig,
    State, StyleExt, TextStyleExt, WindowConfig, launch, rect, spawn, use_hook, use_init_theme,
    use_side_effect, use_state,
};

use crate::session::{self, FontKey, FontOption, GameView, Letters, Library, Progress};
use theme::{CONTENT, TEXT};

#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    None,
    Picker(FontKey),
    Folders,
    Export,
    Updated(Vec<String>),
    Notice(String),
    RemoveGame(session::GameRecord),
    SaveFonts,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Ui {
    pub library: State<Library>,
    pub selected: State<Option<String>>,
    pub view: State<Option<GameView>>,
    pub preset: State<String>,
    pub chars: State<String>,
    pub busy: State<Option<Progress>>,
    pub error: State<Option<String>>,
    pub done: State<Option<String>>,
    pub dialog: State<Dialog>,
    pub options: State<Option<Vec<FontOption>>>,
    pub picked: State<Option<std::path::PathBuf>>,
    pub export_fonts: State<Vec<FontKey>>,
    /// Fonts ticked in the dialog that saves font files.
    pub saved_fonts: State<Vec<FontKey>>,
    pub readme: State<String>,
    pub found_games: State<Vec<std::path::PathBuf>>,
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
                    .with_size(1180., 760.)
                    .with_min_size(900., 560.),
            ),
    )
}

pub fn app() -> impl IntoElement {
    use_init_theme(theme::build);
    let ui = use_ui();
    use_side_effect(move || rescan_for_letters(ui));
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
        error: use_state(|| loaded.clone().err()),
        done: use_state(|| None),
        dialog: use_state(|| Dialog::None),
        options: use_state(|| None),
        picked: use_state(|| None),
        export_fonts: use_state(Vec::new),
        saved_fonts: use_state(Vec::new),
        readme: use_state(String::new),
        found_games: use_state(Vec::new),
    }
}

// Waits for typing to pause, so a scan does not start on every key.
const TYPING_PAUSE: std::time::Duration = std::time::Duration::from_millis(700);

/// Coverage is measured during a scan, so other letters need a new one.
fn rescan_for_letters(ui: Ui) {
    let typed = (ui.preset.read().clone(), ui.chars.read().clone());
    spawn(async move {
        blocking::unblock(move || std::thread::sleep(TYPING_PAUSE)).await;
        let now = (ui.preset.peek().clone(), ui.chars.peek().clone());
        let Some(game) = ui.view.peek().clone() else {
            return;
        };
        let scanned = (
            game.letters.preset.clone(),
            session::join_text(&game.letters),
        );
        if now != typed || now == scanned || game.plan.is_none() {
            return;
        }
        game::scan(ui, game.deep_scan);
    });
}
