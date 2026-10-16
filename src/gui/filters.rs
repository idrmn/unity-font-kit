use std::cmp::Reverse;

use crate::session::GameView;
use crate::work::plan::Entry;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    MostUsed,
    Name,
    File,
}

impl Sort {
    pub const ALL: [Sort; 3] = [Sort::MostUsed, Sort::Name, Sort::File];

    pub fn label(self) -> &'static str {
        match self {
            Sort::MostUsed => "Most used",
            Sort::Name => "Name",
            Sort::File => "File",
        }
    }
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
    sort_entries(&mut shown, sort);
    shown
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

fn sort_entries(entries: &mut [Entry], sort: Sort) {
    match sort {
        Sort::MostUsed => entries.sort_by_key(|entry| Reverse(entry.texts)),
        Sort::Name => entries.sort_by_key(|entry| entry.font.to_lowercase()),
        Sort::File => entries.sort_by(|a, b| {
            (first_file(a), a.font.to_lowercase()).cmp(&(first_file(b), b.font.to_lowercase()))
        }),
    }
}
