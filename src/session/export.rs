use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::game::folder;
use crate::progress::{Progress, Report};
use crate::work::export;

use super::{FontStatus, GameState, GameView, state};

fn default_readme(view: &GameView) -> Result<String> {
    let folder = folder::find_data_dir(&view.record.folder)
        .ok()
        .and_then(|dir| Some(dir.file_name()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| format!("{}_Data", view.record.name));
    export::default_readme(&view.record.name, &folder)
}

/// Only what is in the game now, so the patch holds fonts that were seen there.
#[must_use]
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
    let work = state::open_work(&view.home, &view.record)?;
    progress(Progress {
        done: 0,
        total: 1,
        step: "Making the zip".to_string(),
    });
    let readme = default_readme(view)?;
    export::export_to(&work, &game.data_dir, &readme, target)
}
