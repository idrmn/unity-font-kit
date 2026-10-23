use std::path::PathBuf;

use freya::prelude::{
    Alignment, ChildrenExt, Color, Component, ContainerExt, ContainerSizeExt,
    ContainerWithContentExt, Content, Cursor, CursorIcon, Event, EventHandlersExt, FontWeight,
    IntoElement, PressEventData, Rect, ScrollView, Size, StyleExt, TextStyleExt, WritableUtils,
    rect, spawn, use_state,
};

use super::icons;
use super::tasks;
use super::theme::{
    self, ACCENT, CAPTION, ERROR, ERROR_TINT, OK, SECONDARY, SELECTION, Side, TERTIARY, WARN,
    WINDOW,
};
use super::ui::{Dialog, Ui};
use crate::session::{self, GameRecord, GameState, GameStatus};

pub fn view(ui: Ui) -> impl IntoElement {
    let games = ui.library.read().games.clone();
    rect()
        .width(Size::px(290.))
        .height(Size::fill())
        .background(WINDOW)
        .border(theme::edge(Side::Right))
        .content(Content::flex())
        .child(
            rect()
                .padding((20., 20., 8., 20.))
                .child(theme::caption("Games")),
        )
        .child(
            ScrollView::new().height(Size::flex(1.)).child(
                rect()
                    .width(Size::fill())
                    .padding((0., 10.))
                    .spacing(1.)
                    .children(games.into_iter().map(|game| item(ui, game).into())),
            ),
        )
        .child(
            rect()
                .width(Size::fill())
                .padding(12.)
                .spacing(6.)
                .child(
                    theme::secondary_with(icons::ADD_GAME, "Add game…")
                        .expanded()
                        .on_press(move |_| add_game(ui)),
                )
                .child(
                    theme::secondary_with(icons::FIND_GAMES, "Find games…")
                        .expanded()
                        .on_press(move |_| open_find_games(ui)),
                ),
        )
}

fn item(ui: Ui, game: GameRecord) -> Rect {
    let chosen = ui.selected.read().as_deref() == Some(game.id.as_str());
    let subtitle = format!("Unity {} · {}", game.unity_version, game.backend);
    let id = game.id.clone();
    let waiting = ui.busy.read().is_some() && !chosen;
    let status = ui
        .statuses
        .read()
        .get(&game.id)
        .copied()
        .unwrap_or(GameStatus::Original);
    rect()
        .width(Size::fill())
        .opacity(if waiting { 0.5 } else { 1. })
        .padding((10., 12.))
        .corner_radius(theme::corners())
        .background(if chosen {
            SELECTION
        } else {
            Color::TRANSPARENT
        })
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .on_press(move |_| open_game(ui, &id))
        .child(
            rect()
                .width(Size::flex(1.))
                .spacing(2.)
                .child(
                    theme::strong(game.name.clone(), theme::BODY).font_weight(FontWeight::MEDIUM),
                )
                .child(theme::text(subtitle, CAPTION, SECONDARY))
                .child(status_line(status)),
        )
        .child(remove_button(ui, game))
}

fn status_line(status: GameStatus) -> Rect {
    let color = match status {
        GameStatus::Original => TERTIARY,
        GameStatus::Applied => OK,
        GameStatus::Numbers => ACCENT,
        GameStatus::Updated => WARN,
    };
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(6.)
        .child(theme::dot(color))
        .child(theme::text(status.label(), CAPTION, color))
}

fn remove_button(ui: Ui, game: GameRecord) -> TrashButton {
    TrashButton { ui, game }
}

#[derive(Clone, PartialEq)]
struct TrashButton {
    ui: Ui,
    game: GameRecord,
}

impl Component for TrashButton {
    fn render(&self) -> impl IntoElement {
        let mut hovering = use_state(|| false);
        let mut dialog = self.ui.dialog;
        let game = self.game.clone();
        let idle = self.ui.busy.read().is_none();
        let hovered = *hovering.read() && idle;
        rect()
            .opacity(if idle { 1. } else { 0.4 })
            .padding(6.)
            .corner_radius(theme::corners())
            .background(if hovered {
                ERROR_TINT
            } else {
                Color::TRANSPARENT
            })
            .on_pointer_over(move |_| hovering.set(true))
            .on_pointer_out(move |_| hovering.set_if_modified(false))
            .on_pointer_enter(move |_| Cursor::set(CursorIcon::Pointer))
            .on_pointer_leave(move |_| Cursor::set(CursorIcon::default()))
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                if idle {
                    dialog.set(Dialog::RemoveGame(game.clone()));
                }
            })
            .child(icons::draw(
                icons::TRASH,
                if hovered { ERROR } else { TERTIARY },
            ))
    }
}

/// A game with ufk's fonts in it must be restored first, or its backup would be lost.
pub fn remove_game(ui: Ui, record: &GameRecord) {
    if tasks::reject_busy(ui) {
        return;
    }
    let mut dialog = ui.dialog;
    dialog.set(Dialog::None);
    let removed = record.clone();
    tasks::run(
        ui,
        "Removing the game",
        move |_| session::remove_game_data(&removed),
        {
            let id = record.id.clone();
            move |()| drop_from_library(ui, &id)
        },
    );
}

fn drop_from_library(ui: Ui, id: &str) {
    let (mut library, mut view, mut selected, mut error) =
        (ui.library, ui.view, ui.selected, ui.error);
    let removed = session::remove_game(&library.peek(), id).and_then(|next| {
        session::save_library(&next)?;
        Ok(next)
    });
    match removed {
        Ok(next) => {
            library.set(next);
            if selected.peek().as_deref() == Some(id) {
                view.set(None);
                selected.set(None);
            }
        }
        Err(failure) => error.set(Some(format!("{failure:#}"))),
    }
}

fn open_find_games(ui: Ui) {
    let (mut dialog, mut found_games) = (ui.dialog, ui.found_games);
    dialog.set(Dialog::Folders);
    let folders = ui.library.read().folders.clone();
    spawn(async move {
        found_games.set(blocking::unblock(move || session::find_games(&folders)).await);
    });
}

pub fn open_game(ui: Ui, id: &str) {
    let Some(record) = ui
        .library
        .read()
        .games
        .iter()
        .find(|game| game.id == id)
        .cloned()
    else {
        return;
    };
    if ui.busy.read().is_some() {
        return;
    }
    let (mut selected, mut view, mut dialog) = (ui.selected, ui.view, ui.dialog);
    tasks::run(
        ui,
        "Opening the game",
        move |_| {
            let opened = session::open_game(&record)?;
            let custom = session::read_custom_letters(&opened).unwrap_or_default();
            Ok((opened, custom))
        },
        move |(opened, custom)| {
            let mut custom_letters = ui.custom_letters;
            custom_letters.set(custom);
            selected.set(Some(opened.record.id.clone()));
            if let GameState::Updated { files } = &opened.state {
                dialog.set(Dialog::Updated(files.clone()));
            }
            let (mut preset, mut chars) = (ui.preset, ui.chars);
            preset.set(opened.letters.preset.clone());
            chars.set(session::join_text(&opened.letters));
            view.set(Some(opened));
        },
    );
}

pub fn add_game(ui: Ui) {
    let mut error = ui.error;
    spawn(async move {
        let Some(picked) = rfd::AsyncFileDialog::new()
            .set_title("Add game")
            .pick_folder()
            .await
        else {
            return;
        };
        add_game_folder(ui, picked.path().to_path_buf());
        error.set(None);
    });
}

/// Adds without opening, so several found games can be added in a row while another task runs.
pub fn add_to_library(ui: Ui, path: PathBuf) {
    let mut error = ui.error;
    spawn(async move {
        let read = blocking::unblock(move || session::read_game(&path)).await;
        if let Err(failure) = read.and_then(|record| store_record(ui, record).map(|_| ())) {
            error.set(Some(format!("{failure:#}")));
        }
    });
}

pub fn add_game_folder(ui: Ui, path: PathBuf) {
    let mut error = ui.error;
    tasks::run(
        ui,
        "Reading the game",
        move |_| session::read_game(&path),
        move |record| match store_record(ui, record) {
            Ok(added) => open_game(ui, &added.id),
            Err(failure) => error.set(Some(format!("{failure:#}"))),
        },
    );
}

/// Added to the library as it is now, so games added meanwhile are kept.
fn store_record(ui: Ui, record: GameRecord) -> anyhow::Result<GameRecord> {
    let mut library = ui.library;
    let (next, added) = session::add_record(&library.peek(), record);
    session::save_library(&next)?;
    library.set(next);
    Ok(added)
}
