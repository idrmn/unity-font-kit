use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::progress::{self, Report};
use rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};
use unicode_general_category::{GeneralCategory, get_general_category};

pub const MANAGED: &str = "Managed";
const MARKERS: [&str; 3] = ["globalgamemanagers", "mainData", "data.unity3d"];
const TOP_LEVEL: [&str; 4] = [
    "globalgamemanagers.assets",
    "resources.assets",
    "maindata",
    "data.unity3d",
];
const SKIP_SUFFIXES: [&str; 22] = [
    ".ress",
    ".resource",
    ".dll",
    ".so",
    ".json",
    ".txt",
    ".xml",
    ".png",
    ".jpg",
    ".ogg",
    ".wav",
    ".wem",
    ".bnk",
    ".bank",
    ".mp4",
    ".webm",
    ".manifest",
    ".stringtable",
    ".conversation",
    ".csv",
    ".orig",
    ".bak",
];
// Bundle entries with these extensions hold raw data, not serialized files.
pub(crate) const NOT_SERIALIZED: [&str; 5] = [".resS", ".resource", ".config", ".xml", ".dat"];
const DEFAULT_VERSION: &str = "2.5.0f5";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanDepth {
    Fast,
    Deep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backend {
    Mono,
    Il2cpp,
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Mono => "Mono",
            Self::Il2cpp => "IL2CPP",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub data_dir: PathBuf,
    pub backend: Backend,
    pub unity_version: String,
}

impl Game {
    #[must_use]
    pub fn managed_dir(&self) -> PathBuf {
        self.data_dir.join(MANAGED)
    }
}

pub fn locate(path: &Path) -> Result<Game> {
    let data_dir = find_data_dir(&resolve(path))?;
    Ok(Game {
        backend: detect_backend(&data_dir),
        unity_version: read_unity_version(&data_dir)?,
        data_dir,
    })
}

#[must_use]
pub fn resolve(path: &Path) -> PathBuf {
    dunce::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

pub fn find_data_dir(path: &Path) -> Result<PathBuf> {
    if !path.is_dir() {
        bail!("Not a folder: {}", path.display());
    }
    if is_data_dir(path) {
        return Ok(path.to_path_buf());
    }
    list_sorted(path)?
        .into_iter()
        .find(|child| is_data_dir(child))
        .with_context(|| {
            format!(
                "No Unity data folder (with {}) in {}",
                MARKERS.join(" / "),
                path.display()
            )
        })
}

#[must_use]
pub fn is_data_dir(path: &Path) -> bool {
    path.is_dir() && MARKERS.iter().any(|marker| path.join(marker).is_file())
}

#[must_use]
pub fn detect_backend(data_dir: &Path) -> Backend {
    let game_dir = data_dir.parent().unwrap_or(data_dir);
    if game_dir.join("GameAssembly.dll").exists() || data_dir.join("il2cpp_data").is_dir() {
        return Backend::Il2cpp;
    }
    Backend::Mono
}

pub fn list_asset_files(game: &Game, depth: ScanDepth, report: Report) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in list_sorted(&game.data_dir)? {
        if path.is_file() && is_top_level(&path, depth)? {
            files.push(path);
        }
    }
    let mut nested = Vec::new();
    walk(&game.data_dir, &mut nested)?;
    sort_paths(&mut nested);
    nested.retain(|path| is_bundle_candidate(path, &game.data_dir, depth));
    for path in progress::track(&nested, LISTING, |path| file_name(path), report) {
        if is_unity_file(path)? {
            files.push(path.clone());
        }
    }
    progress::check(report, LISTING)?;
    Ok(files)
}

const LISTING: &str = "Looking for game files";

fn is_named_like_font(path: &Path, data_dir: &Path) -> bool {
    format_relative(path, data_dir)
        .to_lowercase()
        .contains("font")
}

fn is_top_level(path: &Path, depth: ScanDepth) -> Result<bool> {
    let name = file_name(path);
    if is_top_level_name(&name) {
        return Ok(true);
    }
    if is_scene_name(&name) {
        return Ok(depth == ScanDepth::Deep);
    }
    if is_skipped(path) || (depth == ScanDepth::Fast && !name.to_lowercase().contains("font")) {
        return Ok(false);
    }
    is_unity_file(path)
}

fn is_bundle_candidate(path: &Path, data_dir: &Path, depth: ScanDepth) -> bool {
    if path.parent() == Some(data_dir) || !path.is_file() || is_skipped(path) {
        return false;
    }
    if path.starts_with(data_dir.join(MANAGED)) || path.starts_with(data_dir.join("il2cpp_data")) {
        return false;
    }
    depth == ScanDepth::Deep
        || is_top_level_name(&file_name(path))
        || is_named_like_font(path, data_dir)
}

/// Scene files (levelN) hold scene objects; the fonts they use are serialized into sharedassetsN.
fn is_top_level_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    TOP_LEVEL.contains(&lower.as_str()) || is_numbered(&lower, "sharedassets", ".assets")
}

fn is_scene_name(name: &str) -> bool {
    is_numbered(&name.to_lowercase(), "level", "")
}

fn is_numbered(name: &str, prefix: &str, suffix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .is_some_and(|digits| {
            !digits.is_empty()
                && digits
                    .chars()
                    .all(|c| get_general_category(c) == GeneralCategory::DecimalNumber)
        })
}

fn is_skipped(path: &Path) -> bool {
    SKIP_SUFFIXES.contains(&file_suffix(&file_name(path)).to_lowercase().as_str())
}

fn file_suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot < name.len() - 1 => &name[dot..],
        _ => "",
    }
}

pub fn is_unity_file(path: &Path) -> Result<bool> {
    let mut file = File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let length = file.metadata()?.len();
    let mut head = Vec::with_capacity(48);
    file.by_ref().take(48).read_to_end(&mut head)?;
    Ok(is_asset_head(&head, length))
}

/// Gzipped web files are not detected.
#[must_use]
pub(crate) fn is_asset_head(head: &[u8], length: u64) -> bool {
    if length < 20 {
        return false;
    }
    let signature = head.split(|b| *b == 0).next().unwrap_or_default();
    if [&b"UnityWeb"[..], b"UnityRaw", b"UnityFS"].contains(&signature) {
        return true;
    }
    if signature.starts_with(b"UnityWebData") || signature.starts_with(b"TuanjieWebData") {
        return true;
    }
    if signature == b"PK\x03\x04" || length < 128 {
        return false;
    }
    if head.get(0x20..0x26) == Some(b"brotli") {
        return true;
    }
    is_serialized_header(head, length)
}

fn is_serialized_header(head: &[u8], length: u64) -> bool {
    let u32_at = |at: usize| i64::from(u32::from_be_bytes(read_array(head, at)));
    let i64_at = |at: usize| i64::from_be_bytes(read_array(head, at));
    let version = u32_at(8);
    let (metadata_size, file_size, data_offset) = if version >= 22 {
        (u32_at(20), i64_at(24), i64_at(32))
    } else {
        (u32_at(0), u32_at(4), u32_at(12))
    };
    let length = i64::try_from(length).unwrap_or(i64::MAX);
    let in_file = [file_size, metadata_size, version, data_offset]
        .iter()
        .all(|value| (0..=length).contains(value));
    version <= 100 && in_file && file_size >= metadata_size && file_size >= data_offset
}

fn read_array<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    bytes
        .get(at..at + N)
        .and_then(|slice| slice.try_into().ok())
        .unwrap_or([0; N])
}

pub fn read_unity_version(data_dir: &Path) -> Result<String> {
    for marker in MARKERS.map(|name| data_dir.join(name)) {
        if !marker.is_file() {
            continue;
        }
        if let Some(version) = read_file_version(&marker)? {
            return Ok(version);
        }
    }
    bail!(
        "No file to read the Unity version from in {}",
        data_dir.display()
    )
}

fn read_file_version(path: &Path) -> Result<Option<String>> {
    let file = File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let length = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    let mut head = Vec::with_capacity(48);
    reader.by_ref().take(48).read_to_end(&mut head)?;
    reader.seek(SeekFrom::Start(0))?;
    if head.starts_with(b"UnityFS\0") {
        return read_bundle_version(reader)
            .with_context(|| format!("Cannot read {}", path.display()));
    }
    if head.starts_with(b"UnityWeb\0") || head.starts_with(b"UnityRaw\0") {
        bail!("Cannot read {}: old bundle format", path.display());
    }
    if !is_asset_head(&head, length) {
        return Ok(None);
    }
    read_serialized_version(&mut reader).with_context(|| format!("Cannot read {}", path.display()))
}

fn read_bundle_version(reader: impl Read + Seek) -> Result<Option<String>> {
    let mut bundle = BundleFileReader::from_reader(reader, &ExtractionConfig::default())?;
    while let Some(mut entry) = bundle.next() {
        if NOT_SERIALIZED
            .iter()
            .any(|suffix| entry.path.ends_with(suffix))
        {
            continue;
        }
        let data = entry.read()?;
        let length = data.len() as u64;
        if !is_asset_head(&data[..data.len().min(48)], length) {
            continue;
        }
        if let Some(version) = read_serialized_version(&mut Cursor::new(data))? {
            return Ok(Some(version));
        }
    }
    Ok(None)
}

fn read_serialized_version(reader: &mut (impl BufRead + Seek)) -> Result<Option<String>> {
    let mut header = [0; 16];
    reader.read_exact(&mut header)?;
    let field = |at: usize| u64::from(u32::from_be_bytes(read_array(&header, at)));
    let (metadata_size, file_size, version) = (field(0), field(4), field(8));
    let position = match version {
        0..7 => return Ok(Some(DEFAULT_VERSION.to_string())),
        7..9 => file_size.saturating_sub(metadata_size) + 1,
        9..22 => 20,
        _ => 48,
    };
    reader.seek(SeekFrom::Start(position))?;
    let mut text = Vec::new();
    reader.read_until(0, &mut text)?;
    if text.last() == Some(&0) {
        text.pop();
    }
    let text = String::from_utf8_lossy(&text).into_owned();
    Ok((!text.is_empty()).then_some(text))
}

pub fn reject_output_inside(data_dir: &Path, out_dir: &Path) -> Result<()> {
    let (data, out) = (resolve(data_dir), resolve(out_dir));
    if out.starts_with(&data) || data.starts_with(&out) {
        bail!(
            "The work folder {} overlaps the game folder, pick a folder outside it",
            out_dir.display()
        );
    }
    Ok(())
}

#[must_use]
pub fn format_relative(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) => relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.display().to_string(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

fn list_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = std::fs::read_dir(dir)
        .with_context(|| format!("Cannot read {}", dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    sort_paths(&mut paths);
    Ok(paths)
}

/// Symlinked folders are not followed, so a link loop cannot hang the scan.
fn walk(dir: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("Cannot read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walk(&path, found)?;
        }
        found.push(path);
    }
    Ok(())
}

/// Case-insensitive on Windows, like its file system.
fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort_by_cached_key(|path| {
        path.components()
            .map(|part| {
                let text = part.as_os_str().to_string_lossy();
                if cfg!(windows) {
                    text.to_lowercase()
                } else {
                    text.into_owned()
                }
            })
            .collect::<Vec<_>>()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE_HEADER: &[u8] = b"UnityFS\0";

    fn make_game(root: &Path) -> Game {
        let data = root.join("Demo_Data");
        std::fs::create_dir_all(data.join("StreamingAssets/fonts")).unwrap();
        let blank = [
            "globalgamemanagers",
            "globalgamemanagers.assets",
            "resources.assets",
            "sharedassets3.assets",
            "level0",
            "sharedassets3.assets.resS",
        ];
        for name in blank {
            std::fs::write(data.join(name), [0; 256]).unwrap();
        }
        let bundle = [BUNDLE_HEADER, &[0; 200]].concat();
        for name in [
            "StreamingAssets/fonts/ui",
            "fonts.bundle",
            "StreamingAssets/scenes",
        ] {
            std::fs::write(data.join(name), &bundle).unwrap();
        }
        let text = b"plain text, not a unity file".repeat(10);
        std::fs::write(data.join("StreamingAssets/fonts/readme"), text).unwrap();
        std::fs::write(root.join("GameAssembly.dll"), b"").unwrap();
        let data_dir = find_data_dir(root).unwrap();
        Game {
            backend: detect_backend(&data_dir),
            unity_version: "2019.4.26f1".to_string(),
            data_dir,
        }
    }

    fn names(files: &[PathBuf], data: &Path) -> Vec<String> {
        files
            .iter()
            .map(|path| format_relative(path, data))
            .collect()
    }

    #[test]
    fn locates_data_dir_and_lists_every_unity_file() {
        let root = tempfile::tempdir().unwrap();
        let found = make_game(root.path());
        assert_eq!(found.backend, Backend::Il2cpp);
        assert_eq!(
            names(
                &list_asset_files(&found, ScanDepth::Deep, &progress::silent).unwrap(),
                &found.data_dir
            ),
            [
                "fonts.bundle",
                "globalgamemanagers",
                "globalgamemanagers.assets",
                "level0",
                "resources.assets",
                "sharedassets3.assets",
                "StreamingAssets/fonts/ui",
                "StreamingAssets/scenes",
            ]
        );
    }

    #[test]
    fn quick_reading_keeps_main_files_and_font_bundles() {
        let root = tempfile::tempdir().unwrap();
        let found = make_game(root.path());
        let quick = list_asset_files(&found, ScanDepth::Fast, &progress::silent).unwrap();
        assert_eq!(
            names(&quick, &found.data_dir),
            [
                "fonts.bundle",
                "globalgamemanagers.assets",
                "resources.assets",
                "sharedassets3.assets",
                "StreamingAssets/fonts/ui",
            ]
        );
    }
}
