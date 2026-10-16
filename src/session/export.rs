use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::game::folder;
use crate::progress::{Progress, Report};
use crate::work::export as players;

use super::state::open_work;
use super::{FontStatus, GameState, GameView};

fn default_readme(view: &GameView) -> String {
    let folder = folder::find_data_dir(&view.record.folder)
        .ok()
        .and_then(|dir| Some(dir.file_name()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| format!("{}_Data", view.record.name));
    players::default_readme(&view.record.name, &folder).unwrap_or_default()
}

/// Only what is in the game now, so the patch holds fonts that were seen there.
pub fn can_export(view: &GameView) -> bool {
    let pending = view
        .statuses
        .values()
        .any(|status| matches!(status, FontStatus::Chosen | FontStatus::Error(_)));
    view.state == GameState::Applied && view.numbers.is_empty() && !pending
}

pub fn export(view: &GameView, target: &Path, progress: Report) -> Result<PathBuf> {
    if !can_export(view) {
        bail!("Apply your fonts first and check them in the game");
    }
    let game = folder::locate(&view.record.folder)?;
    let work = open_work(&view.home, &view.record)?;
    progress(Progress {
        done: 0,
        total: 1,
        step: "Making the zip".to_string(),
    });
    players::export_to(&work, &game.data_dir, &default_readme(view), target)
}
