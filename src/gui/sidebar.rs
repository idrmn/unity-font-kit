use freya::prelude::{
    Alignment, ChildrenExt, Color, Component, ContainerExt, ContainerSizeExt,
    ContainerWithContentExt, Content, Cursor, CursorIcon, Event, EventHandlersExt, FontWeight,
    IntoElement, PressEventData, Rect, ScrollView, Size, StyleExt, SvgViewer, TextStyleExt,
    WritableUtils, rect, spawn, use_state,
};

use super::theme::{self, CAPTION, ERROR, ERROR_TINT, SECONDARY, SELECTION, TERTIARY, WINDOW};
use super::{Dialog, Ui, tasks};
use crate::session::{self, GameRecord};

pub fn view(ui: Ui) -> impl IntoElement {
    let games = ui.library.read().games.clone();
    rect()
        .width(Size::px(290.))
        .height(Size::fill())
        .background(WINDOW)
        .border(theme::edge((false, true, false, false)))
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
                    theme::primary("Add game…")
                        .expanded()
                        .on_press(move |_| add_game(ui)),
                )
                .child(
                    theme::secondary("Find games…")
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
                .child(theme::text(subtitle, CAPTION, SECONDARY)),
        )
        .child(remove_button(ui, game))
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
        let hovered = *hovering.read();
        rect()
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
                dialog.set(Dialog::RemoveGame(game.clone()));
            })
            .child(
                SvgViewer::new(("trash", TRASH))
                    .width(Size::px(17.))
                    .height(Size::px(17.))
                    .stroke(if hovered { ERROR } else { TERTIARY }),
            )
    }
}

const TRASH: &[u8] = include_bytes!("../../assets/icons/trash.svg");

/// A game with ufk's fonts in it must be restored first, or its backup would be lost.
pub fn remove_game(ui: Ui, record: &GameRecord) {
    let (mut library, mut view, mut selected, mut dialog, mut error) =
        (ui.library, ui.view, ui.selected, ui.dialog, ui.error);
    let applied = session::has_backup(record).unwrap_or(true);
    if applied {
        dialog.set(Dialog::Notice(format!(
            "Restore the original files of {} first.",
            record.name
        )));
        return;
    }
    dialog.set(Dialog::None);
    let current = library.read().clone();
    let removed = session::remove_game(&current, &record.id).and_then(|next| {
        session::save_library(&next)?;
        Ok(next)
    });
    match removed {
        Ok(next) => {
            library.set(next);
            if selected.read().as_deref() == Some(record.id.as_str()) {
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
    let Some(record) = ui.library.read().games.iter().find(|g| g.id == id).cloned() else {
        return;
    };
    if ui.busy.read().is_some() {
        return;
    }
    let (mut selected, mut view, mut dialog) = (ui.selected, ui.view, ui.dialog);
    tasks::run(
        ui,
        "Opening the game",
        move |_| session::open_game(&record),
        move |opened| {
            selected.set(Some(opened.record.id.clone()));
            if let session::GameState::Updated { files } = &opened.state {
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
pub fn add_to_library(ui: Ui, path: std::path::PathBuf) {
    let (mut library, mut error) = (ui.library, ui.error);
    spawn(async move {
        let current = library.peek().clone();
        let added = blocking::unblock(move || {
            let (next, _) = session::add_game(&current, &path)?;
            session::save_library(&next)?;
            anyhow::Ok(next)
        })
        .await;
        match added {
            Ok(next) => library.set(next),
            Err(failure) => error.set(Some(format!("{failure:#}"))),
        }
    });
}

pub fn add_game_folder(ui: Ui, path: std::path::PathBuf) {
    let mut library = ui.library;
    let current = library.read().clone();
    tasks::run(
        ui,
        "Reading the game",
        move |_| {
            let (next, added) = session::add_game(&current, &path)?;
            session::save_library(&next)?;
            Ok((next, added))
        },
        move |(next, added)| {
            library.set(next);
            open_game(ui, &added.id);
        },
    );
}
