use std::path::PathBuf;

use freya::prelude::{
    Alignment, Button, Checkbox, ChildrenExt, Color, ContainerExt, ContainerSizeExt,
    ContainerWithContentExt, Content, Element, EventHandlersExt, FontWeight, IntoElement, Popup,
    PopupThemePartialExt, Rect, ScrollView, Size, StyleExt, TextStyleExt, WritableUtils, rect,
    spawn,
};

use super::text_area::TextArea;
use super::theme::{self, ACCENT_TINT, CAPTION, OK, SECONDARY, TERTIARY, WARN};
use super::{Dialog, Ui, sidebar, tasks};
use crate::session::{self, FontKey, FontOption, Origin};

pub fn view(ui: Ui) -> impl IntoElement {
    let dialog = ui.dialog.read().clone();
    let mut close = ui.dialog;
    let (width, title, body, buttons): (f32, String, Element, Element) = match dialog {
        Dialog::None => return Popup::new().on_close_request(move |_| close.set(Dialog::None)),
        Dialog::Picker(key) => (
            520.,
            format!("Replace {}", key.font),
            picker_body(ui).into(),
            picker_buttons(ui, key).into(),
        ),
        Dialog::Folders => (
            560.,
            "Find games".to_string(),
            folders_body(ui).into(),
            close_button(ui, "Done").into(),
        ),
        Dialog::Export => (
            520.,
            "Export patch".to_string(),
            export_body(ui).into(),
            export_buttons(ui).into(),
        ),
        Dialog::Updated(files) => (
            460.,
            "The game was updated".to_string(),
            updated_body(&files).into(),
            updated_buttons(ui).into(),
        ),
        Dialog::RemoveGame(record) => (
            400.,
            format!("Remove {} from the list?", record.name),
            theme::text("The game files stay as they are.", theme::BODY, SECONDARY).into(),
            remove_buttons(ui, record).into(),
        ),
        Dialog::SaveFonts => (
            480.,
            "Save fonts".to_string(),
            save_fonts_body(ui).into(),
            save_fonts_buttons(ui).into(),
        ),
        Dialog::Notice(message) => (380., message, rect().into(), close_button(ui, "OK").into()),
    };
    Popup::new()
        .width(Size::px(width))
        .on_close_request(move |_| close.set(Dialog::None))
        .child(
            theme::modal(rect())
                .width(Size::fill())
                .padding((20., 22., 18., 22.))
                .spacing(16.)
                .child(theme::strong(title, theme::DIALOG_TITLE))
                .child(body)
                .child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .main_align(Alignment::End)
                        .child(buttons),
                ),
        )
}

fn close_button(ui: Ui, label: &str) -> Button {
    let mut dialog = ui.dialog;
    theme::primary(label).on_press(move |_| dialog.set(Dialog::None))
}

pub fn open_picker(ui: Ui, key: FontKey) {
    let Some(game) = ui.view.read().clone() else {
        return;
    };
    let current = game
        .replacements
        .get(&key)
        .map(|chosen| chosen.path.clone());
    let (mut dialog, mut options, mut picked) = (ui.dialog, ui.options, ui.picked);
    let letters = ui.current_letters();
    dialog.set(Dialog::Picker(key));
    picked.set(current.clone());
    options.set(None);
    spawn(async move {
        let wanted = current.clone();
        let (listed, matching) = blocking::unblock(move || {
            let listed = session::list_options(&game, &letters).unwrap_or_default();
            let matching = match &wanted {
                Some(path) => find_same_file(&listed, path),
                None => listed
                    .iter()
                    .find(|o| o.origin == Origin::Game && o.missing.is_empty())
                    .map(|o| o.path.clone()),
            };
            (listed, matching)
        })
        .await;
        picked.set(matching.or(current));
        options.set(Some(listed));
    });
}

/// The plan may name a copy of a listed font, so equal content counts as the same font.
fn find_same_file(options: &[FontOption], path: &std::path::Path) -> Option<PathBuf> {
    let digest = crate::work::project::hash_file(path).ok()?;
    options
        .iter()
        .find(|option| {
            crate::work::project::hash_file(&option.path).is_ok_and(|other| other == digest)
        })
        .map(|option| option.path.clone())
}

fn picker_body(ui: Ui) -> Rect {
    let loaded = ui.options.read().clone();
    let options = loaded.clone().unwrap_or_default();
    let picked = ui.picked.read().clone();
    let groups = [(Origin::Game, "In this game"), (Origin::Added, "Added")];
    let mut list = rect().width(Size::fill()).spacing(2.);
    for (origin, title) in groups {
        let items: Vec<&FontOption> = options.iter().filter(|o| o.origin == origin).collect();
        if items.is_empty() {
            continue;
        }
        list = list
            .child(
                rect()
                    .padding((10., 10., 4., 10.))
                    .child(theme::caption(title)),
            )
            .children(
                items
                    .into_iter()
                    .map(|option| option_row(ui, option, picked.as_ref()).into()),
            );
    }
    let hint = match &loaded {
        None => Some("Reading the fonts…"),
        Some(listed) if listed.is_empty() => {
            Some("No font in this game has all your letters. Add a font file that has them.")
        }
        Some(_) => None,
    };
    rect()
        .width(Size::fill())
        .spacing(10.)
        .child(
            rect()
                .width(Size::fill())
                .height(Size::px(300.))
                .maybe_child(hint.map(|hint| {
                    rect()
                        .padding(10.)
                        .child(theme::text(hint, theme::BODY, SECONDARY))
                }))
                .child(ScrollView::new().child(list)),
        )
        .child(theme::secondary("Add font file…").on_press(move |_| add_font(ui)))
}

fn option_row(ui: Ui, option: &FontOption, picked: Option<&PathBuf>) -> Rect {
    let chosen = picked == Some(&option.path);
    let path = option.path.clone();
    let mut pick = ui.picked;
    let letters = if option.missing.is_empty() {
        theme::text("all letters", CAPTION, OK)
    } else {
        theme::text(format!("missing {}", option.missing), CAPTION, WARN)
    };
    rect()
        .width(Size::fill())
        .padding((8., 10.))
        .corner_radius(theme::corners())
        .background(if chosen {
            ACCENT_TINT
        } else {
            Color::TRANSPARENT
        })
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .on_press(move |_| pick.set(Some(path.clone())))
        .child(
            theme::text(option.label.clone(), theme::BODY, theme::TEXT)
                .font_weight(if chosen {
                    FontWeight::SEMI_BOLD
                } else {
                    FontWeight::NORMAL
                })
                .width(Size::flex(1.)),
        )
        .child(letters)
}

fn add_font(ui: Ui) {
    let (mut options, mut picked, mut error) = (ui.options, ui.picked, ui.error);
    let letters = ui.current_letters();
    spawn(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title("Add font file")
            .add_filter("Fonts", &["ttf", "otf", "ttc"])
            .pick_file()
            .await
        else {
            return;
        };
        let path = file.path().to_path_buf();
        match blocking::unblock(move || session::add_font_file(&path, &letters)).await {
            Ok(added) => {
                picked.set(Some(added.path.clone()));
                options.write().get_or_insert_with(Vec::new).push(added);
            }
            Err(failure) => error.set(Some(format!("{failure:#}"))),
        }
    });
}

fn remove_buttons(ui: Ui, record: session::GameRecord) -> Rect {
    let mut dialog = ui.dialog;
    rect()
        .horizontal()
        .spacing(8.)
        .child(
            Button::new()
                .flat()
                .on_press(move |_| dialog.set(Dialog::None))
                .child("Cancel"),
        )
        .child(theme::destructive("Remove").on_press(move |_| sidebar::remove_game(ui, &record)))
}

/// Checking the chosen file's letters reads it, so the choice is saved off the window's thread.
fn choose_source(ui: Ui, key: FontKey, source: Option<PathBuf>) {
    let Some(current) = ui.view.read().clone() else {
        return;
    };
    let mut view = ui.view;
    tasks::run(
        ui,
        "Saving your choice",
        move |_| session::set_source(&current, &key, source),
        move |next| view.set(Some(next)),
    );
}

fn picker_buttons(ui: Ui, key: FontKey) -> Rect {
    let mut dialog = ui.dialog;
    let picked = ui.picked.read().clone();
    let clear_key = key.clone();
    rect()
        .horizontal()
        .spacing(8.)
        .child(
            Button::new()
                .flat()
                .on_press(move |_| dialog.set(Dialog::None))
                .child("Cancel"),
        )
        .child(
            Button::new()
                .on_press(move |_| {
                    dialog.set(Dialog::None);
                    choose_source(ui, clear_key.clone(), None);
                })
                .child("Keep original"),
        )
        .child(
            theme::primary("Choose")
                .enabled(picked.is_some())
                .on_press(move |_| {
                    dialog.set(Dialog::None);
                    choose_source(ui, key.clone(), picked.clone());
                }),
        )
}

fn folders_body(ui: Ui) -> Rect {
    let folders = ui.library.read().folders.clone();
    let found = ui.found_games.read().clone();
    let known: Vec<PathBuf> = ui
        .library
        .read()
        .games
        .iter()
        .map(|g| g.folder.clone())
        .collect();
    let mut library = ui.library;
    let mut found_games = ui.found_games;
    rect()
        .width(Size::fill())
        .spacing(10.)
        .children(folders.iter().map(|folder| {
            let gone = folder.clone();
            rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::Center)
                .content(Content::flex())
                .padding((6., 4.))
                .child(
                    theme::text(folder.display().to_string(), theme::BODY, theme::TEXT)
                        .width(Size::flex(1.)),
                )
                .child(
                    Button::new()
                        .flat()
                        .compact()
                        .on_press(move |_| {
                            let mut next = library.read().clone();
                            next.folders.retain(|f| f != &gone);
                            let _ = session::save_library(&next);
                            found_games.set(session::find_games(&next.folders));
                            library.set(next);
                        })
                        .child("Remove"),
                )
                .into()
        }))
        .child(theme::secondary("Add folder…").on_press(move |_| add_folder(ui)))
        .child(theme::divider())
        .child(theme::caption("Games found"))
        .child(
            rect().width(Size::fill()).height(Size::px(220.)).child(
                ScrollView::new().child(rect().width(Size::fill()).spacing(2.).children(
                    found.into_iter().map(|path| {
                        let added = known.contains(&path);
                        let target = path.clone();
                        let name = path
                            .file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .content(Content::flex())
                            .padding((6., 4.))
                            .child(
                                theme::text(name, theme::BODY, theme::TEXT).width(Size::flex(1.)),
                            )
                            .child(if added {
                                theme::text("added", CAPTION, TERTIARY).into()
                            } else {
                                Element::from(
                                    Button::new()
                                        .compact()
                                        .on_press(move |_| {
                                            sidebar::add_to_library(ui, target.clone());
                                        })
                                        .child("Add"),
                                )
                            })
                            .into()
                    }),
                )),
            ),
        )
}

fn add_folder(ui: Ui) {
    let (mut library, mut found_games, mut error) = (ui.library, ui.found_games, ui.error);
    spawn(async move {
        let Some(picked) = rfd::AsyncFileDialog::new()
            .set_title("Add a folder with games")
            .pick_folder()
            .await
        else {
            return;
        };
        let mut next = library.read().clone();
        let folder = picked.path().to_path_buf();
        if !next.folders.contains(&folder) {
            next.folders.push(folder);
        }
        if let Err(failure) = session::save_library(&next) {
            error.set(Some(format!("{failure:#}")));
        }
        let folders = next.folders.clone();
        library.set(next);
        found_games.set(blocking::unblock(move || session::find_games(&folders)).await);
    });
}

fn export_body(ui: Ui) -> Rect {
    let Some(game) = ui.view.read().clone() else {
        return rect();
    };
    let chosen = ui.export_fonts.read().clone();
    let mut export_fonts = ui.export_fonts;
    let entries: Vec<_> = game
        .plan
        .iter()
        .flat_map(|plan| plan.fonts.iter())
        .filter(|entry| !entry.skip && !entry.source.is_empty())
        .cloned()
        .collect();
    rect()
        .width(Size::fill())
        .spacing(10.)
        .child(theme::caption("Fonts"))
        .child(
            rect().width(Size::fill()).max_height(Size::px(220.)).child(
                ScrollView::new().child(rect().width(Size::fill()).spacing(8.).children(
                    entries.into_iter().map(|entry| {
                        let key = FontKey::of(&entry);
                        let selected = chosen.contains(&key);
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(10.)
                            .on_press(move |_| {
                                let mut next = export_fonts.read().clone();
                                if selected {
                                    next.retain(|k| k != &key);
                                } else {
                                    next.push(key.clone());
                                }
                                export_fonts.set(next);
                            })
                            .child(Checkbox::new().selected(selected))
                            .child(theme::text(entry.font.clone(), theme::BODY, theme::TEXT))
                            .into()
                    }),
                )),
            ),
        )
        .child(theme::caption("README"))
        .child(TextArea::new(ui.readme).min_height(140.))
}

fn save_fonts_body(ui: Ui) -> Rect {
    let Some(game) = ui.view.read().clone() else {
        return rect();
    };
    let ticked = ui.saved_fonts.read().clone();
    let mut saved_fonts = ui.saved_fonts;
    let shown = super::game::visible_entries(&game);
    rect()
        .width(Size::fill())
        .spacing(10.)
        .child(theme::text(
            "A font gets its TTF or OTF file, a TextMeshPro font its atlas as PNG.",
            theme::BODY,
            SECONDARY,
        ))
        .child(
            rect().width(Size::fill()).max_height(Size::px(320.)).child(
                ScrollView::new().child(rect().width(Size::fill()).spacing(8.).children(
                    shown.into_iter().map(|entry| {
                        let key = FontKey::of(&entry);
                        let selected = ticked.contains(&key);
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(10.)
                            .on_press(move |_| {
                                let mut next = saved_fonts.read().clone();
                                if selected {
                                    next.retain(|k| k != &key);
                                } else {
                                    next.push(key.clone());
                                }
                                saved_fonts.set(next);
                            })
                            .child(Checkbox::new().selected(selected))
                            .child(theme::text(entry.font.clone(), theme::BODY, theme::TEXT))
                            .into()
                    }),
                )),
            ),
        )
}

fn save_fonts_buttons(ui: Ui) -> Rect {
    let mut dialog = ui.dialog;
    let ready = !ui.saved_fonts.read().is_empty();
    rect()
        .horizontal()
        .spacing(8.)
        .child(
            Button::new()
                .flat()
                .on_press(move |_| dialog.set(Dialog::None))
                .child("Cancel"),
        )
        .child(theme::dimmed(
            ready,
            theme::primary("Save to folder…").on_press(move |_| save_fonts(ui)),
        ))
}

fn save_fonts(ui: Ui) {
    let Some(game) = ui.view.read().clone() else {
        return;
    };
    let keys = ui.saved_fonts.read().clone();
    let mut dialog = ui.dialog;
    spawn(async move {
        let Some(folder) = rfd::AsyncFileDialog::new()
            .set_title("Save fonts")
            .pick_folder()
            .await
        else {
            return;
        };
        let folder = folder.path().to_path_buf();
        dialog.set(Dialog::None);
        let shown = folder.display().to_string();
        tasks::run(
            ui,
            "Saving font files",
            move |_| session::save_font_files(&game, &keys, &folder),
            move |written| {
                let noun = if written.len() == 1 { "file" } else { "files" };
                dialog.set(Dialog::Notice(format!(
                    "Saved {} {noun} to {shown}",
                    written.len()
                )));
            },
        );
    });
}

fn export_buttons(ui: Ui) -> Rect {
    let mut dialog = ui.dialog;
    let ready = !ui.export_fonts.read().is_empty();
    rect()
        .horizontal()
        .spacing(8.)
        .child(
            Button::new()
                .flat()
                .on_press(move |_| dialog.set(Dialog::None))
                .child("Cancel"),
        )
        .child(
            theme::primary("Save zip…")
                .enabled(ready)
                .on_press(move |_| save_zip(ui)),
        )
}

fn save_zip(ui: Ui) {
    let Some(game) = ui.view.read().clone() else {
        return;
    };
    let fonts = ui.export_fonts.read().clone();
    let readme = ui.readme.read().clone();
    let mut dialog = ui.dialog;
    spawn(async move {
        let Some(target) = rfd::AsyncFileDialog::new()
            .set_title("Save patch")
            .set_file_name(format!("{} fonts.zip", game.record.name))
            .save_file()
            .await
        else {
            return;
        };
        let target = target.path().to_path_buf();
        dialog.set(Dialog::None);
        tasks::run(
            ui,
            "Making the zip",
            move |progress| session::export(&game, &fonts, &readme, &target, progress),
            move |saved| dialog.set(Dialog::Notice(format!("Saved {}", saved.display()))),
        );
    });
}

fn updated_body(files: &[String]) -> Rect {
    let count = files.len();
    let noun = if count == 1 { "file" } else { "files" };
    rect()
        .width(Size::fill())
        .spacing(8.)
        .child(theme::text(
            format!("{count} {noun} changed since your fonts were applied. Use the new files as originals and apply your fonts again?"),
            theme::BODY,
            theme::TEXT,
        ))
        .child(theme::text("Your font choices stay.", CAPTION, SECONDARY))
}

fn updated_buttons(ui: Ui) -> Rect {
    let mut dialog = ui.dialog;
    let mut view = ui.view;
    rect()
        .horizontal()
        .spacing(8.)
        .child(
            Button::new()
                .flat()
                .on_press(move |_| dialog.set(Dialog::None))
                .child("Not now"),
        )
        .child(theme::primary("Use new files").on_press(move |_| {
            let Some(game) = view.read().clone() else {
                return;
            };
            dialog.set(Dialog::None);
            tasks::run(
                ui,
                "Taking the new game files",
                move |progress| {
                    let accepted = session::accept_update(&game)?;
                    session::apply(&accepted, progress)
                },
                move |applied| view.set(Some(applied)),
            );
        }))
}
