//! IL2CPP games keep no DLLs; Cpp2IL writes dummy DLLs with the script fields, once per game version.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::downloads;
use crate::game::folder::Game;
use crate::game::typetrees::has_dlls;

pub const CPP2IL_VERSION: &str = "2022.1.0-pre-release.21";
const RELEASES: &str = "https://github.com/SamboyCoding/Cpp2IL/releases/download";
const CPP2IL_WINDOWS: (&str, &str) = (
    "Windows.exe",
    "663fb432433b4371fd1ee0ebc321a8fff2a9aac5ac4230c843f9e03ddee4e04c",
);
const CPP2IL_LINUX: (&str, &str) = (
    "Linux",
    "526998e593c52c029c5a6215c5c6c9f9d963706bfc409fc9ff80a95c4c500349",
);
pub const DONE: &str = "ufk-done.txt";

pub fn find_binary(game_dir: &Path) -> Result<PathBuf> {
    ["GameAssembly.dll", "GameAssembly.so"]
        .map(|name| game_dir.join(name))
        .into_iter()
        .find(|path| path.is_file())
        .with_context(|| {
            format!(
                "No GameAssembly.dll or GameAssembly.so in {}",
                game_dir.display()
            )
        })
}

pub fn find_metadata(data_dir: &Path) -> Result<PathBuf> {
    let path = data_dir
        .join("il2cpp_data")
        .join("Metadata")
        .join("global-metadata.dat");
    if !path.is_file() {
        let shown = data_dir
            .parent()
            .and_then(|game_dir| path.strip_prefix(game_dir).ok());
        bail!("No {}", shown.unwrap_or(&path).display());
    }
    Ok(path)
}

pub fn find_dll_dir(game: &Game) -> Result<PathBuf> {
    let game_dir = game.data_dir.parent().unwrap_or(&game.data_dir);
    let (binary, metadata) = (find_binary(game_dir)?, find_metadata(&game.data_dir)?);
    let hash = downloads::hash_files(&[binary.clone(), metadata.clone()])?;
    let out = downloads::find_home()?.join("il2cpp").join(&hash[..16]);
    if !out.join(DONE).is_file() {
        let tool = fetch_cpp2il()?;
        dump_dlls(&out, |target| {
            run_cpp2il(&tool, &binary, &metadata, &game.unity_version, target)
        })?;
    }
    Ok(out)
}

/// Each run writes into its own folder, so two runs for one game never delete each other's work.
pub fn dump_dlls(out: &Path, run_into: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let parent = out.parent().context("No folder for the game code")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Cannot create {}", parent.display()))?;
    let name = out.file_name().unwrap_or_default().to_string_lossy();
    let partial = tempfile::Builder::new()
        .prefix(&format!("{name}-"))
        .tempdir_in(parent)?;
    run_into(partial.path())?;
    std::fs::write(partial.path().join(DONE), CPP2IL_VERSION)?;
    publish_dir(partial.path(), out)
}

fn run_cpp2il(
    tool: &Path,
    binary: &Path,
    metadata: &Path,
    unity_version: &str,
    target: &Path,
) -> Result<()> {
    let output = downloads::hidden_command(tool)
        .arg(format!("--force-binary-path={}", binary.display()))
        .arg(format!("--force-metadata-path={}", metadata.display()))
        .arg(format!("--force-unity-version={unity_version}"))
        // Without this processor the DLLs lack private [SerializeField] fields, and no font can be read.
        .arg("--use-processor=attributeanalyzer")
        .arg("--output-as=dummydll")
        .arg(format!("--output-to={}", target.display()))
        .output()
        .with_context(|| format!("Cannot run {}", tool.display()))?;
    if output.status.success() && has_dlls(target) {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(15)..].join("\n");
    bail!("Cpp2IL could not read the game code:\n{tail}")
}

fn publish_dir(partial: &Path, out: &Path) -> Result<()> {
    if out.join(DONE).is_file() {
        return Ok(());
    }
    // A failed earlier run may have left an incomplete folder.
    let _ = std::fs::remove_dir_all(out);
    match std::fs::rename(partial, out) {
        Ok(()) => Ok(()),
        Err(_) if out.join(DONE).is_file() => Ok(()),
        Err(error) => Err(error).with_context(|| format!("Cannot write {}", out.display())),
    }
}

fn fetch_cpp2il() -> Result<PathBuf> {
    let (platform, sha256) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => CPP2IL_WINDOWS,
        ("linux", "x86_64") => CPP2IL_LINUX,
        (os, arch) => {
            bail!("Reading IL2CPP games works on 64-bit Windows and Linux, not {os} {arch}")
        }
    };
    let name = format!("Cpp2IL-{CPP2IL_VERSION}-{platform}");
    let target = downloads::find_home()?.join("cpp2il").join(&name);
    downloads::fetch(
        &format!("{RELEASES}/{CPP2IL_VERSION}/{name}"),
        sha256,
        &target,
        "it reads the code of IL2CPP games",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_dll(target: &Path) -> Result<()> {
        std::fs::write(target.join("Assembly-CSharp.dll"), b"dll")?;
        Ok(())
    }

    #[test]
    fn two_runs_for_one_game_do_not_delete_each_others_work() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("il2cpp").join("game");
        dump_dlls(&out, |target| {
            write_dll(target)?;
            dump_dlls(&out, write_dll)
        })
        .unwrap();
        assert!(out.join(DONE).is_file() && out.join("Assembly-CSharp.dll").is_file());
        let left: Vec<_> = std::fs::read_dir(out.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["game"]);
    }
}
