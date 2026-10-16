use freya::prelude::{
    Alignment, ArrowIcon, ChildrenExt, ContainerExt, ContainerSizeExt, ContainerWithContentExt,
    Content, Element, Event, EventHandlersExt, FontWeight, Input, IntoElement, MenuItem, Overflow,
    PressEventData, ProgressBar, Rect, ScrollView, Select, Size, StyleExt, TextStyleExt, Tooltip,
    TooltipContainer, WritableUtils, rect,
};

use super::theme::{
    self, ACCENT, CAPTION, ERROR, ERROR_TINT, OK, OK_TINT, PANEL_RADIUS, SECONDARY, SUNKEN,
    TERTIARY, WARN, WARN_TINT, WINDOW,
};
use super::{Dialog, Ui, tasks};
use crate::session::{self, FontKey, FontStatus, GameState, GameView};
use crate::work::plan::Entry;

const COLUMNS: [f32; 4] = [3., 2., 2., 3.];

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
        .child(header(ui, &game))
        .child(letters_bar(ui))
        .child(theme::divider())
        .child(fonts(ui, &game))
        .child(footer(ui, &game))
}

fn header(ui: Ui, game: &GameView) -> Rect {
    let record = &game.record;
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .padding((24., 28., 16., 28.))
        .spacing(12.)
        .child(
            rect().spacing(4.).child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(10.)
                    .child(theme::strong(record.name.clone(), theme::TITLE))
                    .child(state_pill(&game.state)),
            ),
        )
        .child(rect().width(Size::flex(1.)))
        .child(
            TooltipContainer::new(Tooltip::new(
                "Also reads scenes and asset bundles, not only the main files and files named like fonts. Takes longer.",
            ))
            .child(
                theme::secondary("Deep scan")
                    .enabled(ui.busy.read().is_none())
                    .on_press(move |_| scan(ui, true)),
            ),
        )
}

fn state_pill(state: &GameState) -> Rect {
    match state {
        GameState::Original => theme::pill("Original", SECONDARY, SUNKEN),
        GameState::Applied => theme::pill("Fonts applied", OK, OK_TINT),
        GameState::Updated { .. } => theme::pill("Game updated", WARN, WARN_TINT),
    }
}

fn letters_bar(ui: Ui) -> Rect {
    let mut preset = ui.preset;
    let mut chars = ui.chars;
    let presets = [("uk", "Ukrainian"), ("pl", "Polish"), ("custom", "Custom")];
    // Edited preset text is the user's own set.
    let untouched = session::letters_from_preset(&preset.read())
        .is_ok_and(|filled| session::join_text(&filled) == *chars.read());
    let chosen = presets
        .iter()
        .find(|(id, _)| untouched && *id == preset.read().as_str())
        .map_or("Custom", |(_, label)| label);
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::flex())
        .padding((0., 28., 18., 28.))
        .spacing(10.)
        .child(
            Select::new()
                .selected_item(chosen.to_string())
                .children(presets.iter().map(|(id, label)| {
                    let id = id.to_string();
                    MenuItem::new()
                        .selected(*preset.read() == id)
                        .on_press(move |_| {
                            if let Ok(filled) = session::letters_from_preset(&id) {
                                chars.set(session::join_text(&filled));
                            }
                            preset.set(id.clone());
                        })
                        .child(label.to_string())
                        .into()
                })),
        )
        .child(Input::new(chars).width(Size::flex(1.)))
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
    tasks::run(
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

fn fonts(ui: Ui, game: &GameView) -> Rect {
    let shown = visible_entries(game);
    let toolbar = scan_errors(&game.scan_errors).map(|errors| {
        rect()
            .width(Size::fill())
            .horizontal()
            .main_align(Alignment::End)
            .padding((12., 28., 0., 28.))
            .child(errors)
    });
    let table: Element = if game.plan.is_none() {
        let waiting = if ui.busy.read().is_some() {
            "Looking for fonts…"
        } else {
            "No fonts read yet."
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
        .maybe_child(toolbar)
        .child(
            rect()
                .width(Size::fill())
                .height(Size::flex(1.))
                .child(table),
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
    let headings = ["Font", "State", "Letters", "Replacement"];
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
        .child(cell(0).child(font_name(entry)))
        .child(cell(1).child(status_label(&status, &notes)))
        .child(cell(2).child(letters_label(entry, replacement)))
        .child(
            cell(3).child(replacement_field(entry, replacement).on_press(
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

fn font_name(entry: &Entry) -> Element {
    let name =
        theme::text(entry.font.clone(), theme::BODY, theme::TEXT).font_weight(FontWeight::MEDIUM);
    let used = match entry.texts {
        0 => return name.into(),
        1 => "Used in 1 text".to_string(),
        count => format!("Used in {count} texts"),
    };
    TooltipContainer::new(Tooltip::new(used)).child(name).into()
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
    let busy = ui.busy.read().clone();
    let failure = ui.error.read().clone();
    let mut dialog = ui.dialog;
    let ready = !game.replacements.is_empty();
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
    let status: Element = match (busy, failure) {
        (Some(progress), _) => progress_line(&progress).into(),
        (None, Some(message)) => rect()
            .horizontal()
            .padding((6., 10.))
            .corner_radius(theme::corners())
            .background(ERROR_TINT)
            .child(theme::text(message, CAPTION, ERROR))
            .into(),
        (None, None) => match ui.done.read().clone() {
            Some(message) => theme::text(message, CAPTION + 1., OK).into(),
            None => rect().into(),
        },
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
        .child(theme::dimmed(
            ready && idle,
            theme::secondary("Export patch…").on_press(move |_| {
                let mut chosen = ui.export_fonts;
                let ready: Vec<FontKey> = ui
                    .view
                    .read()
                    .iter()
                    .flat_map(|game| game.plan.iter().flat_map(|plan| plan.fonts.iter()))
                    .filter(|entry| !entry.skip && !entry.source.is_empty())
                    .map(FontKey::of)
                    .collect();
                chosen.set(ready);
                if let Some(game) = ui.view.read().as_ref() {
                    let mut readme = ui.readme;
                    readme.set(session::default_readme(game));
                }
                dialog.set(Dialog::Export);
            }),
        ))
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
        .child(
            rect()
                .width(Size::flex(1.))
                .padding((0., 0., 0., 12.))
                .child(status),
        )
}

// A fixed bar keeps still while step names of different lengths pass by.
const PROGRESS_WIDTH: f32 = 220.;

fn progress_line(progress: &session::Progress) -> Rect {
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
