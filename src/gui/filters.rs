use std::cmp::Ordering;

use crate::session::{FontKey, FontStatus, GameView};
use crate::work::plan::Entry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// Rows under their file's heading.
    File,
    Name,
    Used,
    Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Sort {
            key: SortKey::Used,
            descending: true,
        }
    }
}

impl Sort {
    /// Used means nothing where no texts are counted, so those games group by file.
    pub fn in_effect(self, counted: bool) -> Sort {
        match (self.key, counted) {
            (SortKey::Used, false) => Sort {
                key: SortKey::File,
                descending: false,
            },
            _ => self,
        }
    }

    /// A click on the Font heading switches between name order and file groups;
    /// a second click on another heading turns its order round.
    pub fn after_click(self, clicked: SortKey) -> Sort {
        match (clicked, self.key) {
            (SortKey::Name | SortKey::File, SortKey::Name) => Sort {
                key: SortKey::File,
                descending: false,
            },
            (SortKey::Name | SortKey::File, _) => Sort {
                key: SortKey::Name,
                descending: false,
            },
            (clicked, current) if clicked == current => Sort {
                descending: !self.descending,
                ..self
            },
            (SortKey::Used, _) => Sort {
                key: SortKey::Used,
                descending: true,
            },
            (clicked, _) => Sort {
                key: clicked,
                descending: false,
            },
        }
    }
}

/// Games drawn without uGUI or TextMeshPro texts count none.
pub fn has_counts(game: &GameView) -> bool {
    game.plan
        .iter()
        .flat_map(|plan| plan.fonts.iter())
        .any(|entry| entry.texts > 0)
}

pub fn select_entries(game: &GameView, search: &str, sort: Sort) -> Vec<Entry> {
    let wanted = search.trim().to_lowercase();
    let mut shown: Vec<Entry> = game
        .plan
        .iter()
        .flat_map(|plan| plan.fonts.iter())
        .filter(|entry| wanted.is_empty() || matches_search(entry, &wanted))
        .cloned()
        .collect();
    shown.sort_by(|a, b| {
        let order = compare(game, a, b, sort.key);
        match sort.descending {
            true => order.reverse(),
            false => order,
        }
    });
    shown
}

fn compare(game: &GameView, a: &Entry, b: &Entry, key: SortKey) -> Ordering {
    let name = |entry: &Entry| entry.font.to_lowercase();
    match key {
        SortKey::File => (first_file(a), name(a)).cmp(&(first_file(b), name(b))),
        SortKey::Name => name(a).cmp(&name(b)),
        SortKey::Used => a.texts.cmp(&b.texts),
        SortKey::Status => (rank_status(game, a), name(a)).cmp(&(rank_status(game, b), name(b))),
    }
}

/// What needs attention first: errors, missing letters, choices not yet applied, then done.
fn rank_status(game: &GameView, entry: &Entry) -> u8 {
    let key = FontKey::of(entry);
    let lacking = match game.replacements.get(&key) {
        Some(chosen) => chosen
            .lacks
            .as_deref()
            .is_some_and(|lacks| !lacks.is_empty()),
        None => !entry.missing.is_empty(),
    };
    match game.statuses.get(&key) {
        Some(FontStatus::Error(_)) => 0,
        _ if lacking => 1,
        Some(FontStatus::Chosen) => 2,
        Some(FontStatus::Applied) => 3,
        _ => 4,
    }
}

fn first_file(entry: &Entry) -> &str {
    entry
        .places
        .iter()
        .map(|place| place.file.as_str())
        .min()
        .unwrap_or("")
}

fn matches_search(entry: &Entry, wanted: &str) -> bool {
    entry.font.to_lowercase().contains(wanted)
        || entry
            .places
            .iter()
            .any(|place| place.file.to_lowercase().contains(wanted))
}

/// Rows under their file's heading when grouped by file; otherwise one list without headings.
pub fn group_entries(entries: Vec<Entry>, sort: Sort) -> Vec<(String, Vec<Entry>)> {
    let mut groups: Vec<(String, Vec<Entry>)> = Vec::new();
    for entry in entries {
        let heading = match sort.key {
            SortKey::File => first_file(&entry).to_string(),
            _ => String::new(),
        };
        match groups.last_mut() {
            Some((last, members)) if *last == heading => members.push(entry),
            _ => groups.push((heading, vec![entry])),
        }
    }
    groups
}
