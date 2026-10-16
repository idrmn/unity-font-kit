use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const HOME: &str = "ufk";

/// A per-user folder, so another copy of the program still finds the backups of patched games.
pub fn find_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("UFK_DATA").filter(|home| !home.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    let base = dirs::data_local_dir().context("Cannot find the folder for program data")?;
    Ok(base.join(HOME))
}

pub fn hash_files(paths: &[PathBuf]) -> Result<String> {
    let mut digest = Sha256::new();
    for path in paths {
        let file = File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
        let mut reader = BufReader::with_capacity(1 << 20, file);
        let mut block = vec![0; 1 << 20];
        loop {
            let read = reader.read(&mut block)?;
            if read == 0 {
                break;
            }
            digest.update(&block[..read]);
        }
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub fn fetch(url: &str, sha256: &str, target: &Path, purpose: &str) -> Result<PathBuf> {
    if is_intact(target, sha256) {
        return Ok(target.to_path_buf());
    }
    let name = file_name(target);
    let dir = target.parent().context("No folder to download into")?;
    std::fs::create_dir_all(dir).with_context(|| format!("Cannot create {}", dir.display()))?;
    let mut partial = tempfile::Builder::new()
        .prefix(&format!("{name}-"))
        .tempfile_in(dir)?;
    let mut response = make_agent()
        .get(url)
        .call()
        .with_context(|| format!("Cannot download {name} ({purpose}) from {url}"))?;
    std::io::copy(&mut response.body_mut().as_reader(), partial.as_file_mut())
        .with_context(|| format!("Cannot download {url}"))?;
    if !is_intact(partial.path(), sha256) {
        bail!("The downloaded {name} has a wrong checksum, try again later");
    }
    make_executable(partial.path())?;
    publish_file(partial, target, sha256)?;
    Ok(target.to_path_buf())
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

fn make_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(CONNECT_TIMEOUT))
        .timeout_recv_body(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into()
}

pub fn hidden_command(program: &Path) -> Command {
    hide_window(Command::new(program))
}

#[cfg(windows)]
fn hide_window(mut command: Command) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(not(windows))]
fn hide_window(command: Command) -> Command {
    command
}

pub fn is_intact(path: &Path, sha256: &str) -> bool {
    path.is_file() && hash_files(&[path.to_path_buf()]).is_ok_and(|hash| hash == sha256)
}

fn publish_file(partial: tempfile::NamedTempFile, target: &Path, sha256: &str) -> Result<()> {
    match partial.persist(target) {
        Ok(_) => Ok(()),
        // Another run may have put the same file in place and be running it now.
        Err(_) if is_intact(target, sha256) => Ok(()),
        Err(error) => {
            Err(error.error).with_context(|| format!("Cannot write {}", target.display()))
        }
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}
