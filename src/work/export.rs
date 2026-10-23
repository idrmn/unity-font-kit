use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde_json::Value;
use sha2::Sha256;
use tar::Archive;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipArchive, ZipWriter};

use crate::downloads;
use crate::work::project::{self, Project};

pub const XDELTA_VERSION: &str = "3.2.2";

#[derive(Debug)]
pub struct Xdelta {
    pub system: &'static str,
    pub archive: &'static str,
    pub sha256: &'static str,
    pub encoder: &'static str,
    pub decoder: &'static str,
}

pub const XDELTA: [Xdelta; 2] = [
    Xdelta {
        system: "Windows",
        archive: "xdelta3-3.2.2-windows-x86_64.zip",
        sha256: "d48f7651d86b78fdca397057b46601f89131f37219675df736db70af44ec3379",
        encoder: "xdelta3.exe",
        decoder: "xdelta3decode.exe",
    },
    Xdelta {
        system: "Linux",
        archive: "xdelta3-3.2.2-linux-x86_64.tar.gz",
        sha256: "a8ce7d2c943ae2b3d308942643cb36d4805a1f16775abe1be64fee292d7c2506",
        encoder: "xdelta3",
        decoder: "xdelta3decode",
    },
];
// xdelta3 refuses a smaller source window.
const MIN_WINDOW: u64 = 16384;
const PATCH_DIR: &str = "ufk-patch";
const README: &str = "ufk-fonts-README.txt";
const TOP_FILES: [&str; 5] = [
    README,
    "Install fonts.bat",
    "Restore original files.bat",
    "install.sh",
    "restore.sh",
];
const PATCH_FILES: [&str; 3] = ["patch.ps1", "patch.sh", "LICENSE-xdelta.txt"];

pub fn export_to(
    project: &Project,
    data_dir: &Path,
    readme: &str,
    target: &Path,
) -> Result<PathBuf> {
    export_with(
        project,
        data_dir,
        readme,
        target,
        fetch_xdelta,
        encode_delta,
    )
}

pub fn default_readme(game_name: &str, folder: &str) -> Result<String> {
    let text = translate_newlines(player_source(README)?);
    format_fields(&text, &[("game", game_name), ("folder", folder)])
}

fn folder_name(data_dir: &Path) -> String {
    data_dir
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn translate_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

// xdelta3 cannot patch files of 2 GiB and more.
const MAX_PATCHED_SIZE: u64 = (1 << 31) - 1;

fn reject_huge_files(originals: &[PathBuf]) -> Result<()> {
    for path in originals {
        if std::fs::metadata(path)?.len() > MAX_PATCHED_SIZE {
            bail!(
                "{} is over 2 GB, too big for a patch",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }
    Ok(())
}

fn export_with(
    project: &Project,
    data_dir: &Path,
    readme: &str,
    target: &Path,
    fetch: impl Fn(&Xdelta) -> Result<PathBuf>,
    encode: impl Fn(&Path, &Path, &Path, &Path) -> Result<PathBuf>,
) -> Result<PathBuf> {
    let files = project::list_files(&project.patched)?;
    if files.is_empty() {
        bail!(
            "Nothing to export in {}, apply fonts to the game first",
            project.patched.display()
        );
    }
    let originals: Vec<PathBuf> = files
        .iter()
        .map(|file| project::original_file(Some(&project.backup), data_dir, file))
        .collect();
    reject_huge_files(&originals)?;
    // Hashed once: the patch must describe the very files that were checked.
    let hashes = originals
        .iter()
        .map(|path| project::hash_file(path))
        .collect::<Result<Vec<_>>>()?;
    reject_changed_originals(project, &files, &hashes)?;
    let tools = XDELTA
        .iter()
        .map(|tool| Ok((tool, fetch(tool)?)))
        .collect::<Result<Vec<_>>>()?;
    let temp = tempfile::tempdir()?;
    let host = find_host()?;
    let encoder = unpack_encoder(&tools, host, temp.path())?;
    let deltas = files
        .iter()
        .zip(&originals)
        .map(|(file, original)| {
            let delta = temp.path().join(name_delta(file));
            encode(&encoder, original, &project.patched.join(file), &delta)
        })
        .collect::<Result<Vec<_>>>()?;
    let manifest = format_manifest(&files, &hashes, &project.patched)?;
    write_zip(
        target,
        readme,
        &folder_name(data_dir),
        &manifest,
        &deltas,
        &tools,
    )?;
    Ok(target.to_path_buf())
}

fn reject_changed_originals(project: &Project, files: &[String], hashes: &[String]) -> Result<()> {
    let sources = project::read_build_sources(project)?;
    let changed: Vec<&str> = files
        .iter()
        .zip(hashes)
        .filter(|(file, digest)| sources.get(file.as_str()).and_then(Value::as_str) != Some(digest))
        .map(|(file, _)| file.as_str())
        .collect();
    if !changed.is_empty() {
        bail!(
            "Game files changed after the fonts were built: {}. Apply to game again, then export",
            changed.join(", ")
        );
    }
    Ok(())
}

fn find_host() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("Windows"),
        ("linux", "x86_64") => Ok("Linux"),
        (system, machine) => {
            bail!("Export works on 64-bit Windows and Linux, not {system} {machine}")
        }
    }
}

fn fetch_xdelta(tool: &Xdelta) -> Result<PathBuf> {
    let url = format!(
        "https://github.com/jmacd/xdelta/releases/download/v{XDELTA_VERSION}/{}",
        tool.archive
    );
    let target = downloads::find_home()?.join("xdelta").join(tool.archive);
    downloads::fetch(&url, tool.sha256, &target, "it makes the patches")
}

fn read_member(archive: &Path, binary: &str) -> Result<Vec<u8>> {
    let name = archive
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let folder = name.strip_suffix(".zip").unwrap_or(&name);
    let folder = folder.strip_suffix(".tar.gz").unwrap_or(folder);
    let member = format!("{folder}/{binary}");
    let bytes =
        std::fs::read(archive).with_context(|| format!("Cannot read {}", archive.display()))?;
    if name.ends_with(".zip") {
        return read_zip_member(&bytes, &member);
    }
    read_tar_member(&bytes, &member)
        .with_context(|| format!("Cannot read {}", archive.display()))?
        .with_context(|| format!("No {member} in {}", archive.display()))
}

fn read_tar_member(bytes: &[u8], member: &str) -> Result<Option<Vec<u8>>> {
    let mut archive = Archive::new(GzDecoder::new(bytes));
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.to_string_lossy() != member {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Ok(None);
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        return Ok(Some(data));
    }
    bail!("filename '{member}' not found")
}

fn unpack_encoder(tools: &[(&Xdelta, PathBuf)], host: &str, folder: &Path) -> Result<PathBuf> {
    let Some((tool, archive)) = tools.iter().find(|(tool, _)| tool.system == host) else {
        bail!("No xdelta3 for {host}");
    };
    let encoder = folder.join(tool.encoder);
    std::fs::write(&encoder, read_member(archive, tool.encoder)?)
        .with_context(|| format!("Cannot write {}", encoder.display()))?;
    make_executable(&encoder)?;
    Ok(encoder)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn encode_delta(encoder: &Path, original: &Path, patched: &Path, delta: &Path) -> Result<PathBuf> {
    // A window as big as the original finds data that rewriting the game file moved far from its old place.
    let window = std::fs::metadata(original)?.len().max(MIN_WINDOW);
    // The small decoder players get reads only patches without secondary compression.
    let output = downloads::hidden_command(encoder)
        .args(["-e", "-f", "-S", "none", "-B", &window.to_string(), "-s"])
        .args([original, patched, delta])
        .output()
        .with_context(|| format!("Cannot run {}", encoder.display()))?;
    if !output.status.success() {
        let name = patched
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        bail!(
            "xdelta3 could not make a patch for {name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(delta.to_path_buf())
}

fn format_manifest(files: &[String], hashes: &[String], patched: &Path) -> Result<String> {
    files
        .iter()
        .zip(hashes)
        .map(|(file, original)| {
            let digest = project::hash_file(&patched.join(file))?;
            Ok(format!(
                "{original}\t{digest}\tdeltas/{}\t{file}\n",
                name_delta(file)
            ))
        })
        .collect()
}

/// Players' backups take this name, so it must stay the same for a file in every later patch.
#[must_use]
pub fn name_delta(file: &str) -> String {
    use sha2::Digest;
    let digest: String = Sha256::digest(file.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{}.vcdiff", &digest[..16])
}

fn player_source(name: &str) -> Result<&'static str> {
    Ok(match name {
        "ufk-fonts-README.txt" => {
            include_str!("../../assets/player/ufk-fonts-README.txt")
        }
        "Install fonts.bat" => {
            include_str!("../../assets/player/Install fonts.bat")
        }
        "Restore original files.bat" => {
            include_str!("../../assets/player/Restore original files.bat")
        }
        "install.sh" => include_str!("../../assets/player/install.sh"),
        "restore.sh" => include_str!("../../assets/player/restore.sh"),
        "patch.ps1" => include_str!("../../assets/player/patch.ps1"),
        "patch.sh" => include_str!("../../assets/player/patch.sh"),
        "LICENSE-xdelta.txt" => {
            include_str!("../../assets/player/LICENSE-xdelta.txt")
        }
        _ => bail!("No player file {name}"),
    })
}

fn read_player_file(name: &str, readme: &str) -> Result<String> {
    let text = if name == README {
        translate_newlines(readme)
    } else {
        translate_newlines(player_source(name)?)
    };
    // cmd and Notepad expect CRLF; sh breaks on it.
    if name.ends_with(".sh") {
        return Ok(text);
    }
    Ok(text.replace('\n', "\r\n"))
}

fn format_fields(text: &str, fields: &[(&str, &str)]) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['{', '}']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        let Some(end) = tail.find('}').filter(|_| tail.starts_with('{')) else {
            bail!("Single '}}' encountered in format string");
        };
        let key = &tail[1..end];
        let Some((_, value)) = fields.iter().find(|(name, _)| *name == key) else {
            bail!("'{key}'");
        };
        out.push_str(value);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn write_zip(
    target: &Path,
    readme: &str,
    folder: &str,
    manifest: &str,
    deltas: &[PathBuf],
    tools: &[(&Xdelta, PathBuf)],
) -> Result<()> {
    let mut entries = Vec::new();
    for name in TOP_FILES {
        entries.push((
            name.to_string(),
            read_player_file(name, readme)?.into_bytes(),
        ));
    }
    for name in PATCH_FILES {
        entries.push((
            format!("{PATCH_DIR}/{name}"),
            read_player_file(name, readme)?.into_bytes(),
        ));
    }
    entries.push((
        format!("{PATCH_DIR}/folder.txt"),
        format!("{folder}\n").into_bytes(),
    ));
    entries.push((
        format!("{PATCH_DIR}/files.tsv"),
        manifest.as_bytes().to_vec(),
    ));
    for (tool, archive) in tools {
        entries.push((
            format!("{PATCH_DIR}/{}", tool.decoder),
            read_member(archive, tool.decoder)?,
        ));
    }
    for delta in deltas {
        let name = delta
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let data =
            std::fs::read(delta).with_context(|| format!("Cannot read {}", delta.display()))?;
        entries.push((format!("{PATCH_DIR}/deltas/{name}"), data));
    }
    let partial = target.with_file_name(format!(
        "{}.partial",
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&partial, encode_zip(&entries)?)
        .with_context(|| format!("Cannot write {}", partial.display()))?;
    std::fs::rename(&partial, target).with_context(|| format!("Cannot write {}", target.display()))
}

fn encode_zip(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    use std::io::Write;
    let date = DateTime::from_date_and_time(2020, 1, 1, 0, 0, 0)?;
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(date)
            // Written on Windows the entries would be marked DOS, and unzip would drop the modes.
            .system(System::Unix)
            .unix_permissions(unix_mode(name));
        writer.start_file(name.as_str(), options)?;
        writer.write_all(data)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// Unix unzip tools restore the executable bit only from unix attributes.
fn unix_mode(name: &str) -> u32 {
    if name.ends_with(".sh") || name.ends_with("/xdelta3decode") {
        0o755
    } else {
        0o644
    }
}

fn read_zip_member(bytes: &[u8], member: &str) -> Result<Vec<u8>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut file = archive
        .by_name(member)
        .with_context(|| format!("There is no item named '{member}' in the archive"))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::project::tests::{build_patch, make_game};

    fn make_tools(root: &Path) -> Vec<PathBuf> {
        XDELTA
            .iter()
            .map(|tool| {
                let archive = root.join(tool.archive);
                let folder = tool.archive.strip_suffix(".zip").unwrap_or(tool.archive);
                let folder = folder.strip_suffix(".tar.gz").unwrap_or(folder);
                let binaries = [tool.encoder, tool.decoder];
                let bytes = if tool.archive.ends_with(".zip") {
                    let entries: Vec<_> = binaries
                        .iter()
                        .map(|binary| (format!("{folder}/{binary}"), binary.as_bytes().to_vec()))
                        .collect();
                    encode_zip(&entries).unwrap()
                } else {
                    let gz =
                        flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                    let mut stream = tar::Builder::new(gz);
                    for binary in binaries {
                        let mut header = tar::Header::new_gnu();
                        header.set_size(binary.len() as u64);
                        header.set_cksum();
                        stream
                            .append_data(
                                &mut header,
                                format!("{folder}/{binary}"),
                                binary.as_bytes(),
                            )
                            .unwrap();
                    }
                    stream.into_inner().unwrap().finish().unwrap()
                };
                std::fs::write(&archive, bytes).unwrap();
                archive
            })
            .collect()
    }

    fn fake_export(work: &Project, data: &Path, game: &str) -> Result<Vec<u8>> {
        let tools = make_tools(&work.plan.parent().unwrap().join("tools"));
        let fetch = |tool: &Xdelta| {
            let at = XDELTA
                .iter()
                .position(|known| known.system == tool.system)
                .unwrap();
            Ok(tools[at].clone())
        };
        let encode = |_: &Path, original: &Path, patched: &Path, delta: &Path| {
            let mut bytes = std::fs::read(original)?;
            bytes.push(b'>');
            bytes.extend(std::fs::read(patched)?);
            std::fs::write(delta, bytes)?;
            Ok(delta.to_path_buf())
        };
        let readme = default_readme(game, "Game_Data")?;
        let target = work.work.join(format!("{game} fonts.zip"));
        let target = export_with(work, data, &readme, &target, fetch, encode)?;
        Ok(std::fs::read(target)?)
    }

    fn setup(root: &Path) -> (PathBuf, Project) {
        std::fs::create_dir(root.join("tools")).unwrap();
        make_game(root)
    }

    fn read(zip: &[u8], name: &str) -> Vec<u8> {
        read_zip_member(zip, name).unwrap()
    }

    fn read_text(zip: &[u8], name: &str) -> String {
        String::from_utf8(read(zip, name)).unwrap()
    }

    fn read_mode(zip: &[u8], member: &str) -> u32 {
        let mut archive = ZipArchive::new(Cursor::new(zip)).unwrap();
        archive.by_name(member).unwrap().unix_mode().unwrap() & 0o777
    }

    #[test]
    fn export_after_install_patches_from_the_originals() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = setup(tmp.path());
        build_patch(&work, &data, &["resources.assets"]);
        project::install(&work, &data).unwrap();
        let zip = fake_export(&work, &data, "Game").unwrap();
        let rows = read_text(&zip, "ufk-patch/files.tsv");
        let row: Vec<&str> = rows.trim_end_matches('\n').split('\t').collect();
        assert_eq!(
            (row[0], row[1], row[3]),
            (
                project::hash_file(&work.backup.join("resources.assets"))
                    .unwrap()
                    .as_str(),
                project::hash_file(&work.patched.join("resources.assets"))
                    .unwrap()
                    .as_str(),
                "resources.assets"
            )
        );
        assert_eq!(
            read(&zip, &format!("ufk-patch/{}", row[2])),
            b"original resources.assets>patched"
        );
        assert_eq!(read(&zip, "ufk-patch/folder.txt"), b"Game_Data\n");
        assert_eq!(read_mode(&zip, "ufk-patch/xdelta3decode"), 0o755);
        assert!(!read_text(&zip, "ufk-patch/patch.sh").contains("\r\n"));
        assert!(read_text(&zip, "Install fonts.bat").contains("\r\n"));
    }

    #[test]
    fn export_refuses_game_files_changed_after_build() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = setup(tmp.path());
        build_patch(&work, &data, &["resources.assets"]);
        std::fs::write(data.join("resources.assets"), "updated").unwrap();
        let error = fake_export(&work, &data, "Game").unwrap_err().to_string();
        assert!(
            error.contains("changed after the fonts were built"),
            "{error}"
        );
    }

    #[test]
    fn a_file_keeps_its_patch_name_when_the_patch_gains_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, work) = setup(tmp.path());
        build_patch(&work, &data, &["sharedassets0.assets"]);
        let zip = fake_export(&work, &data, "Game").unwrap();
        let alone = read_text(&zip, "ufk-patch/files.tsv")
            .split('\t')
            .nth(2)
            .unwrap()
            .to_string();
        build_patch(&work, &data, &["resources.assets", "sharedassets0.assets"]);
        let zip = fake_export(&work, &data, "Game").unwrap();
        let rows = read_text(&zip, "ufk-patch/files.tsv");
        let named = rows
            .lines()
            .map(|row| row.split('\t').collect::<Vec<_>>())
            .find(|row| row[3] == "sharedassets0.assets")
            .unwrap()[2]
            .to_string();
        assert_eq!(named, alone);
    }
}
