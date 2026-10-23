use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use freya::prelude::{
    ChildrenExt, Component, ContainerPositionExt, ContainerSizeExt, ContainerWithContentExt,
    Content, Event, EventHandlersExt, IntoElement, LaunchConfig, Layer, LayerExt, Position,
    PressEventData, Size, StyleExt, TextStyleExt, WindowConfig, WritableUtils, launch, rect, spawn,
    use_init_theme, use_side_effect,
};

use super::theme::{self, CONTENT, TEXT};
use super::ui::{self, Ui};
use super::{dialogs, game, sidebar, tasks};
use crate::session::{self, ScanDepth};

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
    let ui = ui::use_ui();
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

// A scan or apply in progress rewrites the plan, so the letters wait for it to end.
const BUSY_CHECK: Duration = Duration::from_millis(300);

fn recheck_letters(ui: Ui) {
    let typed = (ui.preset.read().clone(), ui.chars.read().clone());
    spawn(async move {
        while ui.busy.peek().is_some() {
            blocking::unblock(move || thread::sleep(BUSY_CHECK)).await;
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
            game::scan(ui, ScanDepth::Fast);
            return;
        }
        let (preset, chars) = now;
        let letters = session::split_text(&chars, &preset);
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
