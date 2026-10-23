use std::path::{Path, PathBuf};

use freya::prelude::{
    Alignment, Button, Checkbox, ChildrenExt, Color, ContainerExt, ContainerSizeExt,
    ContainerWithContentExt, Content, Element, EventHandlersExt, FontWeight, IntoElement, Popup,
    PopupThemePartialExt, Rect, ScrollView, Size, StyleExt, TextStyleExt, WritableUtils, rect,
    spawn,
};

use super::icons;
use super::text_area::TextArea;
use super::theme::{self, ACCENT_TINT, CAPTION, OK, SECONDARY, TERTIARY, WARN};
use super::ui::{Dialog, Pick, Ui};
use super::{game, sidebar, tasks};
use crate::session::{self, FontKey, FontOption, GameStatus, Origin, ScanDepth};
use crate::work::project;

pub fn view(ui: Ui) -> impl IntoElement {
    let dialog = ui.dialog.read().clone();
    let mut close = ui.dialog;
    let (width, title, body, buttons): (Size, String, Element, Element) = match dialog {
        Dialog::None => return Popup::new().on_close_request(move |_| close.set(Dialog::None)),
        Dialog::Picker(key) => (
            Size::px(520.),
            format!("Replace {}", key.font),
            picker_body(ui).into(),
            picker_buttons(ui, key).into(),
        ),
        Dialog::Folders => (
            Size::px(560.),
            "Find games".to_string(),
            folders_body(ui).into(),
            close_button(ui, "Done").into(),
        ),
        Dialog::Updated(files) => (
            Size::px(460.),
            "The game was updated".to_string(),
            updated_body(&files).into(),
            updated_buttons(ui).into(),
        ),
        Dialog::RemoveGame(record) => (
            Size::px(460.),
            format!("Remove {} from the list?", record.name),
            remove_body(ui, &record).into(),
            remove_buttons(ui, record).into(),
        ),
        Dialog::SaveFonts => (
            Size::px(480.),
            "Save fonts".to_string(),
            save_fonts_body(ui).into(),
            save_fonts_buttons(ui).into(),
        ),
        Dialog::Notice(message) => (
            Size::px(380.),
            message,
            rect().into(),
            close_button(ui, "OK").into(),
        ),
        Dialog::Mark => (
            Size::px(480.),
            "Mark fonts".to_string(),
            mark_body(ui).into(),
            mark_buttons(ui).into(),
        ),
        Dialog::Letters => (
            Size::window_percent(80.),
            "Your letters".to_string(),
            letters_body(ui).into(),
            letters_buttons(ui).into(),
        ),
    };
    Popup::new()
        .width(width)
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

fn mark_body(ui: Ui) -> Rect {
    let Some(game) = ui.view.read().clone() else {
        return rect();
    };
    let ticked = ui.marked.read().clone();
    let mut marked = ui.marked;
    let every: Vec<FontKey> = game
        .plan
        .iter()
        .flat_map(|plan| plan.fonts.iter())
        .map(FontKey::of)
        .collect();
    let rows: Vec<Element> = game
        .plan
        .iter()
        .flat_map(|plan| plan.fonts.iter())
        .enumerate()
        .map(|(index, entry)| {
            let key = FontKey::of(entry);
            let selected = ticked.contains(&key);
            rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::Center)
                .content(Content::flex())
                .spacing(10.)
                .on_press(move |_| {
                    let mut next = marked.read().clone();
                    if selected {
                        next.retain(|kept| kept != &key);
                    } else {
                        next.push(key.clone());
                    }
                    marked.set(next);
                })
                .child(Checkbox::new().selected(selected))
                .child(
                    theme::text(format!("{}", index + 1), CAPTION, TERTIARY).width(Size::px(22.)),
                )
                .child(
                    theme::text(entry.font.clone(), theme::BODY, theme::TEXT).width(Size::flex(1.)),
                )
                .maybe_child((entry.texts > 0).then(|| {
                    let used = match entry.texts {
                        1 => "1 text".to_string(),
                        count => format!("{count} texts"),
                    };
                    theme::text(used, CAPTION, SECONDARY)
                }))
                .into()
        })
        .collect();
    rect()
        .width(Size::fill())
        .spacing(10.)
        .child(theme::text(
            "The picked fonts show their numbers in the game. Unmark puts back what the game had.",
            theme::BODY,
            SECONDARY,
        ))
        .child(
            rect()
                .horizontal()
                .spacing(8.)
                .child(theme::secondary("Select all").on_press(move |_| marked.set(every.clone())))
                .child(theme::secondary("Select none").on_press(move |_| marked.set(Vec::new()))),
        )
        .child(
            rect().width(Size::fill()).max_height(Size::px(320.)).child(
                ScrollView::new().child(rect().width(Size::fill()).spacing(8.).children(rows)),
            ),
        )
}

fn mark_buttons(ui: Ui) -> Rect {
    let mut dialog = ui.dialog;
    let keys = ui.marked.read().clone();
    let ready = !keys.is_empty();
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
            theme::primary("Generate").on_press(move |_| {
                dialog.set(Dialog::None);
                game::mark(ui, keys.clone());
            }),
        ))
}

fn letters_body(ui: Ui) -> Rect {
    rect()
        .width(Size::fill())
        .spacing(10.)
        .child(theme::text(
            "Every letter and mark your texts use. ufk checks each font against them.",
            theme::BODY,
            SECONDARY,
        ))
        .child(TextArea::new(ui.letters_draft).min_height(200.))
}

fn letters_buttons(ui: Ui) -> Rect {
    let mut dialog = ui.dialog;
    let (mut chars, mut preset, draft) = (ui.chars, ui.preset, ui.letters_draft);
    let mut custom_letters = ui.custom_letters;
    rect()
        .horizontal()
        .spacing(8.)
        .child(
            Button::new()
                .flat()
                .on_press(move |_| dialog.set(Dialog::None))
                .child("Cancel"),
        )
        .child(theme::primary("Use these letters").on_press(move |_| {
            custom_letters.set(draft.read().clone());
            chars.set(draft.read().clone());
            preset.set("custom".to_string());
            dialog.set(Dialog::None);
        }))
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
    let start = current.clone().map_or(Pick::Original, Pick::File);
    dialog.set(Dialog::Picker(key.clone()));
    picked.set(start.clone());
    options.set(None);
    spawn(async move {
        let wanted = current.clone();
        let (listed, matching) = blocking::unblock(move || {
            let listed = session::list_options(&game, &letters).unwrap_or_default();
            let matching = wanted.and_then(|path| find_same_file(&listed, &path));
            (listed, matching)
        })
        .await;
        // The list may arrive after the user moved on to another font or picked one already.
        if *dialog.peek() != Dialog::Picker(key) {
            return;
        }
        if let Some(same) = matching.filter(|_| *picked.peek() == start) {
            picked.set(Pick::File(same));
        }
        let added: Vec<FontOption> = options
            .peek()
            .iter()
            .flatten()
            .filter(|option| !listed.iter().any(|known| known.path == option.path))
            .cloned()
            .collect();
        let listed = [listed, added].concat();
        options.set(Some(listed));
    });
}

/// The plan may name a copy of a listed font, so equal content counts as the same font.
fn find_same_file(options: &[FontOption], path: &Path) -> Option<PathBuf> {
    let digest = project::hash_file(path).ok()?;
    options
        .iter()
        .find(|option| project::hash_file(&option.path).is_ok_and(|other| other == digest))
        .map(|option| option.path.clone())
}

fn picker_body(ui: Ui) -> Rect {
    let loaded = ui.options.read().clone();
    let options = loaded.clone().unwrap_or_default();
    let picked = ui.picked.read().clone();
    let groups = [(Origin::Game, "In this game"), (Origin::Added, "Added")];
    let mut list = rect()
        .width(Size::fill())
        .spacing(2.)
        .child(original_row(ui, picked == Pick::Original));
    for (origin, title) in groups {
        let items: Vec<&FontOption> = options
            .iter()
            .filter(|option| option.origin == origin)
            .collect();
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
                    .map(|option| option_row(ui, option, picked.file()).into()),
            );
    }
    let hint = match &loaded {
        None => Some("Reading the fonts…"),
        Some(listed) if listed.is_empty() => {
            Some("No font in this game has all your letters. Add a font file that has them.")
        }
        Some(_) => None,
    };
    rect().width(Size::fill()).spacing(10.).child(
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
}

fn original_row(ui: Ui, chosen: bool) -> Rect {
    let mut pick = ui.picked;
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
        .on_press(move |_| pick.set(Pick::Original))
        .child(
            theme::text("Keep original", theme::BODY, theme::TEXT)
                .font_weight(if chosen {
                    FontWeight::SEMI_BOLD
                } else {
                    FontWeight::NORMAL
                })
                .width(Size::flex(1.)),
        )
        .child(theme::text("the game's own font", CAPTION, TERTIARY))
}

fn option_row(ui: Ui, option: &FontOption, picked_file: Option<&Path>) -> Rect {
    let chosen = picked_file == Some(option.path.as_path());
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
        .on_press(move |_| pick.set(Pick::File(path.clone())))
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
            .add_filter("Fonts", &["ttf", "otf"])
            .pick_file()
            .await
        else {
            return;
        };
        let path = file.path().to_path_buf();
        match blocking::unblock(move || session::add_font_file(&path, &letters)).await {
            Ok(added) => {
                picked.set(Pick::File(added.path.clone()));
                options.write().get_or_insert_with(Vec::new).push(added);
            }
            Err(failure) => error.set(Some(format!("{failure:#}"))),
        }
    });
}

/// A game with ufk's changes gets its original files back before its data goes.
fn has_changes(ui: Ui, record: &session::GameRecord) -> bool {
    ui.statuses
        .read()
        .get(&record.id)
        .is_some_and(|status| *status != GameStatus::Original)
}

fn remove_body(ui: Ui, record: &session::GameRecord) -> Rect {
    let text = match has_changes(ui, record) {
        true => {
            "ufk puts the original game files back, then deletes everything it kept for this game."
        }
        false => "ufk deletes everything it kept for this game. The game files stay as they are.",
    };
    theme::text(text, theme::BODY, SECONDARY)
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
        .child(
            theme::destructive(match has_changes(ui, &record) {
                true => "Restore and remove",
                false => "Remove",
            })
            .on_press(move |_| sidebar::remove_game(ui, &record)),
        )
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
    rect()
        .width(Size::fill())
        .horizontal()
        .content(Content::flex())
        .spacing(8.)
        .child(
            theme::secondary_with(icons::ADD_FONT, "Add font file…")
                .on_press(move |_| add_font(ui)),
        )
        .child(rect().width(Size::flex(1.)))
        .child(
            Button::new()
                .flat()
                .on_press(move |_| dialog.set(Dialog::None))
                .child("Cancel"),
        )
        .child(
            theme::primary("Choose")
                .enabled(picked != Pick::Unset)
                .on_press(move |_| {
                    dialog.set(Dialog::None);
                    choose_source(ui, key.clone(), picked.file().map(Path::to_path_buf));
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
        .map(|game| game.folder.clone())
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
                            next.folders.retain(|kept| kept != &gone);
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
                            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
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

fn save_fonts_body(ui: Ui) -> Rect {
    let Some(game) = ui.view.read().clone() else {
        return rect();
    };
    let ticked = ui.saved_fonts.read().clone();
    let mut saved_fonts = ui.saved_fonts;
    let shown = game::visible_entries(&game);
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
                                    next.retain(|kept| kept != &key);
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

pub fn save_zip(ui: Ui) {
    let Some(game) = ui.view.read().clone() else {
        return;
    };
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
        tasks::run(
            ui,
            "Making the zip",
            move |progress| session::export(&game, &target, progress),
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
            let letters = ui.current_letters();
            tasks::run(
                ui,
                "Taking the new game files",
                move |progress| {
                    let accepted = session::accept_update(&game)?;
                    // The update may move fonts inside the files, so they are found again first.
                    let scanned =
                        session::scan(&accepted.record, ScanDepth::Fast, &letters, progress)?;
                    session::apply(&scanned, progress)
                },
                move |applied| view.set(Some(applied)),
            );
        }))
}
