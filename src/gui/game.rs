use freya::prelude::{
    Alignment, AttachedPosition, Button, ChildrenExt, ContainerExt, ContainerSizeExt,
    ContainerWithContentExt, Content, Element, Event, EventHandlersExt, FontWeight, Input,
    IntoElement, Layer, LayerExt, MenuItem, Overflow, PressEventData, ProgressBar, Rect,
    ScrollView, Select, Size, StyleExt, TextStyleExt, Tooltip, TooltipContainer, WritableUtils,
    rect,
};

use std::collections::BTreeSet;

use super::filters::{self, Sort, SortKey};
use super::icons;
use super::theme::{
    self, ACCENT, CAPTION, ERROR, ERROR_TINT, OK, PANEL_RADIUS, SECONDARY, TERTIARY, WARN, WINDOW,
};
use super::{Dialog, Ui, tasks};
use crate::session::{self, FontKey, FontStatus, GameState, GameView};
use crate::work::plan::Entry;

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
        .child(
            theme::primary_with(icons::ADD_GAME, "Add game…")
                .on_press(move |_| super::sidebar::add_game(ui)),
        )
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
            theme::secondary_with(icons::FAST_SCAN, "Fast scan").on_press(move |_| scan(ui, false)),
        ))
        .child(theme::dimmed(
            idle,
            theme::secondary_with(icons::DEEP_SCAN, "Deep scan").on_press(move |_| scan(ui, true)),
        ))
}

fn letters_bar(ui: Ui) -> Rect {
    let mut preset = ui.preset;
    let mut chars = ui.chars;
    let (mut dialog, mut draft, custom_letters) = (ui.dialog, ui.letters_draft, ui.custom_letters);
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
            let custom = custom_letters.read().clone();
            draft.set(match custom.is_empty() {
                true => chars.read().clone(),
                false => custom,
            });
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
    rect().child(
        Input::new(ui.search)
            .placeholder("Search fonts or files")
            .width(Size::px(160.)),
    )
}

fn clear_button(ui: Ui, game: &GameView) -> TooltipContainer {
    let idle = ui.busy.read().is_none();
    // Chosen files that went missing have no replacement entry, yet they still need clearing.
    let chosen_any = game
        .plan
        .iter()
        .flat_map(|plan| plan.fonts.iter())
        .any(|entry| !entry.skip && !entry.source.is_empty());
    TooltipContainer::new(Tooltip::new(
        "Sets every font back to Keep original. A new scan does not choose again.",
    ))
    .child(theme::dimmed(
        chosen_any && idle,
        theme::secondary_with(icons::CLEAR, "Clear replacements")
            .on_press(move |_| clear_choices(ui)),
    ))
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
    let sort = ui.sort.read().in_effect(filters::has_counts(game));
    let shown = filters::select_entries(game, &search, sort);
    let nothing = shown.is_empty();
    let columns = list_columns(game);
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
                    .child(table_head(ui, &columns, sort))
                    .child(theme::divider())
                    .maybe_child(nothing.then(|| {
                        rect().padding(16.).child(theme::text(
                            "No font matches the search or filter.",
                            theme::BODY,
                            SECONDARY,
                        ))
                    }))
                    .child(
                        ScrollView::new().height(Size::flex(1.)).child(
                            rect().width(Size::fill()).children(
                                filters::group_entries(shown, sort)
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, (heading, entries))| {
                                        group(ui, game, &columns, sort, (index, heading), &entries)
                                            .into()
                                    }),
                            ),
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
    "Marked fonts show their numbers in the game. Start it to look, then press Unmark.";

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

#[derive(Clone, Copy)]
enum Column {
    Font,
    UsedIn,
    Status,
    Replacement,
}

impl Column {
    fn title(self) -> &'static str {
        match self {
            Column::Font => "Font",
            Column::UsedIn => "Used in",
            Column::Status => "Status",
            Column::Replacement => "Replacement",
        }
    }

    fn sort_key(self) -> Option<SortKey> {
        match self {
            Column::Font => Some(SortKey::Name),
            Column::UsedIn => Some(SortKey::Used),
            Column::Status => Some(SortKey::Status),
            Column::Replacement => None,
        }
    }

    fn width(self) -> f32 {
        match self {
            Column::Font => 3.0,
            Column::UsedIn => 1.0,
            Column::Status => 3.4,
            Column::Replacement => 2.3,
        }
    }
}

/// Without counted texts the column would say nothing.
fn list_columns(game: &GameView) -> Vec<Column> {
    let counted = filters::has_counts(game);
    [
        Column::Font,
        Column::UsedIn,
        Column::Status,
        Column::Replacement,
    ]
    .into_iter()
    .filter(|column| counted || !matches!(column, Column::UsedIn))
    .collect()
}

fn group(
    ui: Ui,
    game: &GameView,
    columns: &[Column],
    sort: Sort,
    (index, heading): (usize, String),
    entries: &[Entry],
) -> Rect {
    let titled = !heading.is_empty();
    rect()
        .width(Size::fill())
        .maybe_child((index > 0).then(theme::divider))
        .maybe_child(titled.then(|| {
            rect()
                .width(Size::fill())
                .padding((8., 20.))
                .background(theme::CONTENT)
                .child(theme::caption(heading))
        }))
        .children(entries.iter().enumerate().map(|(position, entry)| {
            rect()
                .width(Size::fill())
                .maybe_child((position > 0 || titled).then(theme::divider))
                .child(row(ui, game, entry, columns, sort.key != SortKey::File))
                .into()
        }))
}

fn cell(column: Column) -> Rect {
    rect().width(Size::flex(column.width())).padding((0., 8.))
}

/// A click on a heading sorts by it; the arrow shows the order in use.
fn table_head(ui: Ui, columns: &[Column], sort: Sort) -> Rect {
    cells()
        .height(Size::px(38.))
        .children(columns.iter().map(|column| {
            let Some(key) = column.sort_key() else {
                return cell(*column).child(theme::caption(column.title())).into();
            };
            let arrow = match (sort.key == key, sort.descending) {
                (true, false) => " ↑",
                (true, true) => " ↓",
                (false, _) => "",
            };
            let mut chosen = ui.sort;
            cell(*column)
                .on_press(move |_| chosen.set(sort.after_click(key)))
                .child(theme::caption(format!("{}{arrow}", column.title())))
                .into()
        }))
}

fn row(ui: Ui, game: &GameView, entry: &Entry, columns: &[Column], show_file: bool) -> Rect {
    let key = FontKey::of(entry);
    let pick_key = key.clone();
    let replacement = game.replacements.get(&key);
    cells()
        .height(Size::px(52.))
        .children(columns.iter().map(|column| {
            let shown: Element = match column {
                Column::Font => font_name(game, entry, show_file),
                Column::UsedIn => texts_label(entry),
                Column::Status => status_cell(game, entry),
                Column::Replacement => {
                    let pick_key = pick_key.clone();
                    replacement_field(entry, replacement)
                        .on_press(move |event: Event<PressEventData>| {
                            event.stop_propagation();
                            // A scan rewrites the plan when it ends, which would drop a choice made meanwhile.
                            if !tasks::reject_busy(ui) {
                                super::dialogs::open_picker(ui, pick_key.clone());
                            }
                        })
                        .into()
                }
            };
            cell(*column).child(shown).into()
        }))
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

/// Under a file heading the file name would only repeat it.
fn font_name(game: &GameView, entry: &Entry, show_file: bool) -> Element {
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
        .maybe_child((show_file && !shown.is_empty()).then(|| {
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

/// A button, not a select: it opens the font picker.
fn replacement_field(entry: &Entry, replacement: Option<&session::Replacement>) -> Button {
    let (label, color) = match (replacement, entry.skip || entry.source.is_empty()) {
        (Some(chosen), _) => (file_stem(&chosen.path), theme::TEXT),
        (None, true) => ("Keep original".to_string(), TERTIARY),
        (None, false) => (format!("{} (file not found)", entry.source), ERROR),
    };
    Button::new()
        .expanded()
        .child(theme::line(label, theme::BODY, color))
}

fn file_stem(path: &std::path::Path) -> String {
    path.file_stem().map_or_else(
        || path.display().to_string(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// One line on what the game will show: the change, then what is still missing.
fn status_cell(game: &GameView, entry: &Entry) -> Element {
    let key = FontKey::of(entry);
    if let Some(number) = game.numbers.get(&key) {
        return status_text(format!("Shows {number}"), String::new(), ACCENT);
    }
    let status = game
        .statuses
        .get(&key)
        .cloned()
        .unwrap_or(FontStatus::Unchanged);
    if let FontStatus::Error(reason) = &status {
        return status_text(reason.clone(), reason.clone(), ERROR);
    }
    let notes = game.notes.get(&key).cloned().unwrap_or_default();
    let lacking = match game.replacements.get(&key) {
        Some(chosen) => chosen.lacks.clone().unwrap_or_default(),
        None => entry.missing.clone(),
    };
    let change = match status {
        FontStatus::Chosen => Some("Ready to apply".to_string()),
        FontStatus::Applied => Some("Applied".to_string()),
        _ => None,
    };
    let parts: Vec<String> = change
        .into_iter()
        .chain((!lacking.is_empty()).then(|| describe_lacking(&lacking)))
        .chain(notes.iter().cloned())
        .collect();
    let color = match (&status, lacking.is_empty() && notes.is_empty()) {
        (_, false) => WARN,
        (FontStatus::Chosen, true) => ACCENT,
        _ => OK,
    };
    let details = [
        (!lacking.is_empty()).then(|| format!("Lacks {lacking}")),
        (!notes.is_empty()).then(|| notes.join("; ")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(". ");
    let label = match parts.is_empty() {
        true => "All letters".to_string(),
        false => parts.join(" · "),
    };
    status_text(label, details, color)
}

fn status_text(label: String, details: String, color: freya::prelude::Color) -> Element {
    let shown = rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .spacing(7.)
        .child(theme::dot(color))
        .child(theme::line(label, theme::BODY, color).width(Size::flex(1.)));
    if details.is_empty() {
        return shown.into();
    }
    TooltipContainer::new(Tooltip::new(details))
        .child(shown)
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
            theme::primary_with(icons::APPLY, &apply_label).on_press(move |_| apply(ui)),
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
            theme::secondary_with(icons::RESTORE, "Restore").on_press(move |_| restore(ui)),
        ))
        .child(theme::separator())
        .child(
            TooltipContainer::new(Tooltip::new(match game.numbers.is_empty() {
                true => "The fonts you pick show their table numbers in the game.",
                false => "Puts back what the game had before the marks.",
            }))
            .position(AttachedPosition::Top)
            .child(mark_button(ui, game, idle)),
        )
        .child(theme::separator())
        .child(
            TooltipContainer::new(Tooltip::new(match session::can_export(game) {
                true => "A zip that puts the fonts now in the game into the same game version.",
                false => "Apply your fonts and check them in the game first.",
            }))
            .position(AttachedPosition::Top)
            .child(theme::dimmed(
                session::can_export(game) && idle,
                theme::secondary_with(icons::EXPORT, "Export patch…")
                    .on_press(move |_| super::dialogs::save_zip(ui)),
            )),
        )
        .child(theme::dimmed(
            game.plan.is_some() && idle,
            theme::secondary_with(icons::SAVE, "Save fonts…").on_press(move |_| {
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
        .maybe_child(ui.stoppable.read().then(|| {
            rect()
                .layer(Layer::OverlayLevel(super::BLOCKER_LAYER + 1))
                .child(theme::secondary_with(icons::STOP, "Stop").on_press(|_| tasks::stop()))
        }))
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
            done.set(Some(describe_apply(&applied)));
            view.set(Some(applied));
        },
    );
}

/// Apply also restores when no font is chosen, and it leaves out fonts that fail.
fn describe_apply(applied: &GameView) -> String {
    if applied.state == GameState::Original {
        return "The original files are back.".to_string();
    }
    let failed = applied
        .statuses
        .values()
        .filter(|status| matches!(status, FontStatus::Error(_)))
        .count();
    match failed {
        0 => "Fonts applied. Start the game to check them.".to_string(),
        1 => "Fonts applied, except 1 font: see its row.".to_string(),
        count => format!("Fonts applied, except {count} fonts: see their rows."),
    }
}

fn mark_button(ui: Ui, game: &GameView, idle: bool) -> Rect {
    let usable = game.plan.is_some() && !matches!(game.state, GameState::Updated { .. }) && idle;
    if !game.numbers.is_empty() {
        return theme::dimmed(
            usable,
            theme::secondary_with(icons::UNMARK, "Unmark").on_press(move |_| unmark(ui)),
        );
    }
    let (mut dialog, mut marked) = (ui.dialog, ui.marked);
    let lacking: Vec<FontKey> = game
        .plan
        .iter()
        .flat_map(|plan| plan.fonts.iter())
        .filter(|entry| !entry.missing.is_empty())
        .map(FontKey::of)
        .collect();
    theme::dimmed(
        usable,
        theme::secondary_with(icons::MARK, "Mark…").on_press(move |_| {
            marked.set(lacking.clone());
            dialog.set(Dialog::Mark);
        }),
    )
}

pub fn mark(ui: Ui, keys: Vec<FontKey>) {
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
        "Marking the fonts",
        move |progress| session::mark_fonts(&game, &keys, progress),
        move |marked| {
            let mut done = ui.done;
            done.set(Some(NUMBERS_HINT.to_string()));
            view.set(Some(marked));
        },
    );
}

fn unmark(ui: Ui) {
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
        "Taking the marks out",
        move |_| session::unmark_fonts(&game),
        move |unmarked| view.set(Some(unmarked)),
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
