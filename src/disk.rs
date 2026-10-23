use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};

#[must_use]
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// Deletes a file; a file that is already gone counts as deleted.
pub fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => {
            Err(error).with_context(|| format!("Cannot delete {}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Deletes a folder with its contents; a folder that is already gone counts as deleted.
pub fn remove_dir(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => {
            Err(error).with_context(|| format!("Cannot delete {}", path.display()))
        }
        _ => Ok(()),
    }
}
