use freya::prelude::{
    Alignment, ArrowIcon, AttachedPosition, ChildrenExt, ContainerExt, ContainerSizeExt,
    ContainerWithContentExt, Content, Element, Event, EventHandlersExt, FontWeight, Input,
    IntoElement, MenuItem, Overflow, Preference, PressEventData, ProgressBar, Rect, ScrollView,
    Select, SelectThemePartial, Size, StyleExt, TextStyleExt, Tooltip, TooltipContainer,
    WritableUtils, rect,
};

use std::collections::BTreeSet;

use super::filters::{self, Sort};
use super::theme::{
    self, ACCENT, CAPTION, ERROR, ERROR_TINT, OK, PANEL_RADIUS, SECONDARY, TERTIARY, WARN, WINDOW,
};
use super::{Dialog, Ui, tasks};
use crate::session::{self, FontKey, FontStatus, GameState, GameView};
use crate::work::plan::Entry;

const COLUMNS: [f32; 5] = [3.0, 1.0, 1.8, 2.3, 2.3];

pub fn view(ui: Ui) -> impl IntoElement {
    let current = ui.view.read().clone();
    let body: Element = match current {
        Some(game) => screen(ui, game).into(),
        None => empty(ui).into(),
    };
    rect()
        .width(Size::flex(1.))
        .height(Size::fill())
        .content(Content::flex())
        .child(body)
}

fn empty(ui: Ui) -> Rect {
    let failure = ui.error.read().clone();
    rect()
        .expanded()
        .center()
        .spacing(14.)
        .child(theme::text(
            "Pick a game on the left, or add one.",
            15.,
            SECONDARY,
        ))
        .child(theme::primary("Add game…").on_press(move |_| super::sidebar::add_game(ui)))
        .maybe_child(failure.map(|message| theme::text(message, theme::BODY, ERROR)))
}

fn screen(ui: Ui, game: GameView) -> Rect {
    rect()
        .expanded()
        .content(Content::flex())
        .child(letters_bar(ui))
        .child(theme::divider())
        .child(fonts(ui, &game))
        .child(footer(ui, &game))
}

fn scan_buttons(ui: Ui) -> Rect {
    let idle = ui.busy.read().is_none();
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(8.)
        .child(theme::dimmed(
            idle,
            theme::secondary("Fast scan").on_press(move |_| scan(ui, false)),
        ))
        .child(theme::dimmed(
            idle,
            theme::secondary("Deep scan").on_press(move |_| scan(ui, true)),
        ))
}

fn letters_bar(ui: Ui) -> Rect {
    let mut preset = ui.preset;
    let mut chars = ui.chars;
    let (mut dialog, mut draft) = (ui.dialog, ui.letters_draft);
    let presets = [("uk", "Ukrainian"), ("pl", "Polish")];
    // Edited preset text is the user's own set.
    let untouched = session::letters_from_preset(&preset.read())
        .is_ok_and(|filled| session::join_text(&filled) == *chars.read());
    let chosen = presets
        .iter()
        .find(|(id, _)| untouched && *id == preset.read().as_str())
        .map_or("Custom", |(_, label)| label);
    let count = chars
        .read()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .count();
    let custom = MenuItem::new()
        .selected(chosen == "Custom")
        .on_press(move |_| {
            draft.set(chars.read().clone());
            dialog.set(Dialog::Letters);
        })
        .child("Custom…");
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .padding((20., 28., 18., 28.))
        .spacing(10.)
        .child(
            Select::new().selected_item(chosen.to_string()).children(
                presets
                    .iter()
                    .map(|(id, label)| {
                        let id = id.to_string();
                        MenuItem::new()
                            .selected(chosen == *label)
                            .on_press(move |_| {
                                if let Ok(filled) = session::letters_from_preset(&id) {
                                    chars.set(session::join_text(&filled));
                                }
                                preset.set(id.clone());
                            })
                            .child(label.to_string())
                            .into()
                    })
                    .chain([custom.into()]),
            ),
        )
        .child(theme::text(
            format!("{count} characters"),
            CAPTION + 1.,
            SECONDARY,
        ))
        .child(rect().width(Size::flex(1.)))
        .child(scan_buttons(ui))
}

pub fn scan(ui: Ui, full: bool) {
    let Some(record) = ui.view.read().as_ref().map(|game| game.record.clone()) else {
        return;
    };
    let letters = ui.current_letters();
    let mut view = ui.view;
    let label = if full {
        "Reading every game file"
    } else {
        "Looking for fonts"
    };
    tasks::run_stoppable(
        ui,
        label,
        move |progress| session::scan(&record, full, &letters, progress),
        move |scanned| view.set(Some(scanned)),
    );
}

pub fn visible_entries(game: &GameView) -> Vec<Entry> {
    let mut shown: Vec<Entry> = game
        .plan
        .iter()
        .flat_map(|plan| plan.fonts.clone())
        .collect();
    shown.sort_by_key(|entry| std::cmp::Reverse(entry.texts));
    shown
}

fn table_tools(ui: Ui) -> Rect {
    let mut sort = ui.sort;
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(8.)
        .child(
            Input::new(ui.search)
                .placeholder("Search fonts or files")
                .width(Size::px(160.)),
        )
        .child(
            narrow_select(sort.read().label()).children(Sort::ALL.iter().map(|option| {
                let option = *option;
                MenuItem::new()
                    .selected(*sort.read() == option)
                    .on_press(move |_| sort.set(option))
                    .child(option.label().to_string())
                    .into()
            })),
        )
}

fn clear_button(ui: Ui, game: &GameView) -> TooltipContainer {
    let idle = ui.busy.read().is_none();
    let chosen_any = !game.replacements.is_empty();
    TooltipContainer::new(Tooltip::new(
        "Sets every font back to Keep original. A new scan does not choose again.",
    ))
    .child(theme::dimmed(
        chosen_any && idle,
        theme::secondary("Clear replacements").on_press(move |_| clear_choices(ui)),
    ))
}

// Fixed, so the toolbar stays put; the open list shows the full names.
const SELECT_WIDTH: f32 = 140.;

fn narrow_select(chosen: &str) -> Select {
    Select::new()
        .theme(SelectThemePartial {
            width: Some(Preference::Specific(Size::px(SELECT_WIDTH))),
            ..Default::default()
        })
        .selected_item(
            theme::line(chosen, theme::BODY, theme::TEXT).width(Size::px(SELECT_WIDTH - 56.)),
        )
}

fn clear_choices(ui: Ui) {
    let Some(game) = ui.view.read().clone() else {
        return;
    };
    let mut view = ui.view;
    tasks::run(
        ui,
        "Clearing your choices",
        move |_| session::clear_choices(&game),
        move |cleared| view.set(Some(cleared)),
    );
}

fn fonts(ui: Ui, game: &GameView) -> Rect {
    let search = ui.search.read().clone();
    let shown = filters::select_entries(game, &search, *ui.sort.read());
    let toolbar = rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .spacing(12.)
        .padding((12., 28., 0., 28.))
        .maybe_child(game.plan.is_some().then(|| table_tools(ui)))
        .child(rect().width(Size::flex(1.)))
        .maybe_child(scan_errors(&game.scan_errors))
        .maybe_child(game.plan.is_some().then(|| clear_button(ui, game)));
    let table: Element = if game.plan.is_none() {
        let waiting = if ui.busy.read().is_some() {
            "Looking for fonts…"
        } else {
            "No fonts read yet. Press Fast scan."
        };
        rect()
            .expanded()
            .center()
            .child(theme::text(waiting, theme::BODY + 1., SECONDARY))
            .into()
    } else {
        rect()
            .expanded()
            .padding((16., 28., 16., 28.))
            .child(
                theme::raised(rect())
                    .expanded()
                    .corner_radius(theme::rounded(PANEL_RADIUS))
                    .overflow(Overflow::Clip)
                    .content(Content::flex())
                    .child(table_head())
                    .child(theme::divider())
                    .maybe_child(shown.is_empty().then(|| {
                        rect().padding(16.).child(theme::text(
                            "No font matches the search or filter.",
                            theme::BODY,
                            SECONDARY,
                        ))
                    }))
                    .child(
                        ScrollView::new().height(Size::flex(1.)).child(
                            rect()
                                .width(Size::fill())
                                .children(shown.iter().enumerate().map(|(index, entry)| {
                                    rect()
                                        .width(Size::fill())
                                        .maybe_child((index > 0).then(theme::divider))
                                        .child(row(ui, game, entry))
                                        .into()
                                })),
                        ),
                    ),
            )
            .into()
    };
    rect()
        .width(Size::fill())
        .height(Size::flex(1.))
        .content(Content::flex())
        .child(toolbar)
        .child(
            rect()
                .width(Size::fill())
                .height(Size::flex(1.))
                .child(table),
        )
        .maybe_child(status_line(ui, game))
}

const NUMBERS_HINT: &str =
    "The fonts show their numbers in the game. Start it to look, then press Restore.";

/// Progress, the last error or result, or the reminder that the game shows numbers.
fn status_line(ui: Ui, game: &GameView) -> Option<Rect> {
    let busy = ui.busy.read().clone();
    let failure = ui.error.read().clone();
    let done = ui.done.read().clone();
    let shown: Element = match (busy, failure, done) {
        (Some(progress), _, _) => progress_line(ui, &progress).into(),
        (None, Some(message), _) => rect()
            .width(Size::fill())
            .padding((6., 10.))
            .corner_radius(theme::corners())
            .background(ERROR_TINT)
            .child(theme::text(message, CAPTION + 1., ERROR))
            .into(),
        (None, None, Some(message)) => theme::text(message, CAPTION + 1., OK).into(),
        (None, None, None) if !game.numbers.is_empty() => {
            theme::text(NUMBERS_HINT, CAPTION + 1., ACCENT).into()
        }
        (None, None, None) => return None,
    };
    Some(
        rect()
            .width(Size::fill())
            .padding((0., 28., 14., 28.))
            .child(shown),
    )
}

// The tooltip lists the first files only; a broken game can have hundreds.
const LISTED_ERRORS: usize = 8;

/// Files the scan could not read may hold fonts, so they are never hidden.
fn scan_errors(errors: &[String]) -> Option<Element> {
    let count = errors.len();
    if count == 0 {
        return None;
    }
    let label = match count {
        1 => "1 file could not be read".to_string(),
        count => format!("{count} files could not be read"),
    };
    let mut listed: Vec<String> = errors.iter().take(LISTED_ERRORS).cloned().collect();
    if count > LISTED_ERRORS {
        listed.push(format!("and {} more", count - LISTED_ERRORS));
    }
    let shown = rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(7.)
        .child(theme::dot(WARN))
        .child(theme::text(label, CAPTION + 1., WARN));
    Some(
        TooltipContainer::new(Tooltip::new(listed.join("\n")))
            .child(shown)
            .into(),
    )
}

fn cells() -> Rect {
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .padding((0., 12.))
}

fn cell(index: usize) -> Rect {
    let width = COLUMNS[index];
    rect().width(Size::flex(width)).padding((0., 8.))
}

fn table_head() -> Rect {
    let headings = ["Font", "Used in", "State", "Letters", "Replacement"];
    cells().height(Size::px(38.)).children(
        headings
            .iter()
            .enumerate()
            .map(|(index, title)| cell(index).child(theme::caption(title.to_string())).into()),
    )
}

fn row(ui: Ui, game: &GameView, entry: &Entry) -> Rect {
    let key = FontKey::of(entry);
    let status = game
        .statuses
        .get(&key)
        .cloned()
        .unwrap_or(FontStatus::Unchanged);
    let notes = game.notes.get(&key).cloned().unwrap_or_default();
    let replacement = game.replacements.get(&key);
    let pick_key = key.clone();
    cells()
        .height(Size::px(52.))
        .child(cell(0).child(font_name(game, entry)))
        .child(cell(1).child(texts_label(entry)))
        .child(cell(2).child(match game.numbers.get(&key) {
            Some(number) => numbered_label(*number),
            None => status_label(&status, &notes),
        }))
        .child(cell(3).child(letters_label(entry, replacement)))
        .child(
            cell(4).child(replacement_field(entry, replacement).on_press(
                move |event: Event<PressEventData>| {
                    event.stop_propagation();
                    // A scan rewrites the plan when it ends, which would drop a choice made meanwhile.
                    if ui.busy.read().is_none() {
                        super::dialogs::open_picker(ui, pick_key.clone());
                    }
                },
            )),
        )
}

/// The number the font draws in the game while font numbers are shown.
fn font_number(game: &GameView, entry: &Entry) -> String {
    let key = FontKey::of(entry);
    if !game.numbers.is_empty() {
        return game
            .numbers
            .get(&key)
            .map_or_else(String::new, ToString::to_string);
    }
    game.plan
        .iter()
        .flat_map(|plan| plan.fonts.iter())
        .position(|other| FontKey::of(other) == key)
        .map_or_else(String::new, |index| (index + 1).to_string())
}

fn font_name(game: &GameView, entry: &Entry) -> Element {
    let name =
        theme::line(entry.font.clone(), theme::BODY, theme::TEXT).font_weight(FontWeight::MEDIUM);
    let files: BTreeSet<&str> = entry
        .places
        .iter()
        .map(|place| place.file.as_str())
        .collect();
    let shown = files.into_iter().collect::<Vec<_>>().join(", ");
    let named = rect()
        .width(Size::flex(1.))
        .spacing(2.)
        .child(name)
        .maybe_child((!shown.is_empty()).then(|| {
            TooltipContainer::new(Tooltip::new(shown.clone()))
                .child(theme::line(shown, CAPTION, TERTIARY))
        }));
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .child(
            theme::line(font_number(game, entry), CAPTION, TERTIARY).width(Size::px(NUMBER_WIDTH)),
        )
        .child(named)
        .into()
}

fn texts_label(entry: &Entry) -> Element {
    let shown = match entry.texts {
        0 => "–".to_string(),
        1 => "1 text".to_string(),
        count => format!("{count} texts"),
    };
    theme::text(shown, theme::BODY, SECONDARY).into()
}

fn replacement_field(entry: &Entry, replacement: Option<&session::Replacement>) -> Rect {
    let (label, color) = match (replacement, entry.skip || entry.source.is_empty()) {
        (Some(chosen), _) => (file_stem(&chosen.path), theme::TEXT),
        (None, true) => ("Keep original".to_string(), TERTIARY),
        (None, false) => (format!("{} (file not found)", entry.source), ERROR),
    };
    rect()
        .width(Size::fill())
        .height(Size::px(36.))
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .padding((0., 10.))
        .spacing(8.)
        .corner_radius(theme::corners())
        .background(theme::RAISED)
        .border(theme::hairline(theme::CONTROL_BORDER))
        .child(theme::line(label, theme::BODY, color).width(Size::flex(1.)))
        .child(ArrowIcon::new().fill(SECONDARY))
}

fn file_stem(path: &std::path::Path) -> String {
    path.file_stem().map_or_else(
        || path.display().to_string(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

fn numbered_label(number: usize) -> Element {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(7.)
        .child(theme::dot(ACCENT))
        .child(theme::line(format!("shows {number}"), theme::BODY, ACCENT))
        .into()
}

fn status_label(status: &FontStatus, notes: &[String]) -> Element {
    let (label, color) = match status {
        FontStatus::Unchanged => ("original".to_string(), TERTIARY),
        FontStatus::Chosen => ("ready to apply".to_string(), ACCENT),
        FontStatus::Applied if notes.is_empty() => ("applied".to_string(), OK),
        FontStatus::Applied => (format!("applied, {}", notes.join(", ")), WARN),
        FontStatus::Error(reason) => (reason.clone(), ERROR),
    };
    let shown = rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .spacing(7.)
        .child(theme::dot(color))
        .child(theme::line(label.clone(), theme::BODY, color).width(Size::flex(1.)));
    // The cell cuts long notes; the whole text shows on hover.
    if label.chars().count() < 28 {
        return shown.into();
    }
    TooltipContainer::new(Tooltip::new(label))
        .child(shown)
        .into()
}

/// What the game will lack: the chosen font's gaps, or the game font's when nothing replaces it.
fn letters_label(entry: &Entry, replacement: Option<&session::Replacement>) -> Element {
    let (lacking, color) = match replacement {
        Some(chosen) => match &chosen.lacks {
            Some(lacks) => (lacks.clone(), WARN),
            None => return theme::text("—", theme::BODY, TERTIARY).into(),
        },
        None => (entry.missing.clone(), WARN),
    };
    if lacking.is_empty() {
        return theme::text("All letters", theme::BODY, OK).into();
    }
    TooltipContainer::new(Tooltip::new(format!("Lacks {lacking}")))
        .child(theme::line(describe_lacking(&lacking), theme::BODY, color))
        .into()
}

// More than a few characters do not fit the column; the tooltip lists them all.
const SHOWN_CHARS: usize = 5;

fn describe_lacking(lacking: &str) -> String {
    let count = lacking.chars().count();
    if count <= SHOWN_CHARS {
        let spaced: Vec<String> = lacking.chars().map(String::from).collect();
        return format!("Lacks {}", spaced.join(" "));
    }
    format!("Lacks {count} characters")
}

fn footer(ui: Ui, game: &GameView) -> Rect {
    let mut dialog = ui.dialog;
    let changes = game
        .statuses
        .values()
        .filter(|status| matches!(status, FontStatus::Chosen | FontStatus::Error(_)))
        .count();
    let apply_label = match changes {
        0 => "Apply".to_string(),
        1 => "Apply 1 change".to_string(),
        count => format!("Apply {count} changes"),
    };
    let applied = matches!(game.state, GameState::Applied);
    let idle = ui.busy.read().is_none();
    let main_action = match &game.state {
        GameState::Updated { files } => {
            let files = files.clone();
            theme::dimmed(
                idle,
                theme::primary("Use new game files…")
                    .on_press(move |_| dialog.set(Dialog::Updated(files.clone()))),
            )
        }
        _ => theme::dimmed(
            changes > 0 && idle,
            theme::primary(&apply_label).on_press(move |_| apply(ui)),
        ),
    };
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .padding((14., 28.))
        .spacing(8.)
        .background(WINDOW)
        .border(theme::edge((true, false, false, false)))
        .child(main_action)
        .child(theme::dimmed(
            applied && idle,
            theme::secondary("Restore").on_press(move |_| restore(ui)),
        ))
        .child(
            TooltipContainer::new(Tooltip::new(match game.state {
                GameState::Original => {
                    "Every letter shows its font's number from the table, until Restore."
                }
                _ => "Restore the original fonts first.",
            }))
            .position(AttachedPosition::Top)
            .child(theme::dimmed(
                game.plan.is_some() && game.state == GameState::Original && idle,
                theme::secondary("Find in game").on_press(move |_| show_numbers(ui)),
            )),
        )
        .child(
            TooltipContainer::new(Tooltip::new(match session::can_export(game) {
                true => "A zip that puts the fonts now in the game into the same game version.",
                false => "Apply your fonts and check them in the game first.",
            }))
            .position(AttachedPosition::Top)
            .child(theme::dimmed(
                session::can_export(game) && idle,
                theme::secondary("Export patch…").on_press(move |_| super::dialogs::save_zip(ui)),
            )),
        )
        .child(theme::dimmed(
            game.plan.is_some() && idle,
            theme::secondary("Save fonts…").on_press(move |_| {
                let mut ticked = ui.saved_fonts;
                let shown = ui
                    .view
                    .read()
                    .as_ref()
                    .map(visible_entries)
                    .unwrap_or_default();
                ticked.set(shown.iter().map(FontKey::of).collect());
                dialog.set(Dialog::SaveFonts);
            }),
        ))
}

const NUMBER_WIDTH: f32 = 24.;

// A fixed bar keeps still while step names of different lengths pass by.
const PROGRESS_WIDTH: f32 = 220.;

fn progress_line(ui: Ui, progress: &session::Progress) -> Rect {
    let share = if progress.total == 0 {
        0.
    } else {
        progress.done as f32 / progress.total as f32 * 100.
    };
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .spacing(12.)
        .child(ProgressBar::new(share).width(Size::px(PROGRESS_WIDTH)))
        .child(theme::line(progress.step.clone(), CAPTION + 1., SECONDARY).width(Size::flex(1.)))
        .maybe_child(
            ui.stoppable
                .read()
                .then(|| theme::secondary("Stop").on_press(|_| tasks::stop())),
        )
}

fn blocker(record: &session::GameRecord) -> Option<String> {
    if session::is_game_running(record) {
        return Some("Close the game first.".to_string());
    }
    if !session::can_write(record) {
        let how = if cfg!(windows) {
            "Close ufk, then right-click it and choose Run as administrator."
        } else {
            "Give your user write access to the game folder."
        };
        return Some(format!(
            "ufk cannot change the files in this game folder. {how}"
        ));
    }
    None
}

fn apply(ui: Ui) {
    let Some(game) = ui.view.read().clone() else {
        return;
    };
    let mut dialog = ui.dialog;
    if let Some(blocker) = blocker(&game.record) {
        dialog.set(Dialog::Notice(blocker));
        return;
    }
    let mut view = ui.view;
    tasks::run(
        ui,
        "Applying fonts",
        move |progress| session::apply(&game, progress),
        move |applied| {
            let mut done = ui.done;
            done.set(Some(
                "Fonts applied. Start the game to check them.".to_string(),
            ));
            view.set(Some(applied));
        },
    );
}

fn show_numbers(ui: Ui) {
    let Some(game) = ui.view.read().clone() else {
        return;
    };
    let mut dialog = ui.dialog;
    if let Some(blocker) = blocker(&game.record) {
        dialog.set(Dialog::Notice(blocker));
        return;
    }
    let mut view = ui.view;
    tasks::run_stoppable(
        ui,
        "Putting font numbers into the game",
        move |progress| session::show_numbers(&game, progress),
        move |numbered| {
            let mut done = ui.done;
            done.set(Some(NUMBERS_HINT.to_string()));
            view.set(Some(numbered));
        },
    );
}

fn restore(ui: Ui) {
    let Some(game) = ui.view.read().clone() else {
        return;
    };
    let mut dialog = ui.dialog;
    if let Some(blocker) = blocker(&game.record) {
        dialog.set(Dialog::Notice(blocker));
        return;
    }
    let mut view = ui.view;
    tasks::run(
        ui,
        "Restoring the original files",
        move |_| session::restore(&game),
        move |restored| {
            let mut done = ui.done;
            done.set(Some("The original files are back.".to_string()));
            view.set(Some(restored));
        },
    );
}
