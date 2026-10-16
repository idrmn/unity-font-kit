use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use rabex::UnityVersion;
use rabex::files::SerializedFile;
use rabex::files::bundlefile::{
    BundleFileHeader, BundleFileReader, CompressionType, ExtractionConfig, write_bundle,
};
use rabex::files::rawbundle;
use rabex::files::serializedfile::{
    ObjectInfo, build_common_offset_map, write_serialized_with_objects,
};
use rabex::files::unityfile::FileEntry;
use rabex::objects::ClassId;
use rabex::serde_typetree;
use rabex::tpk::TpkTypeTreeBlob;
use rabex::typetree::typetree_cache::sync::TypeTreeCache;
use rabex::typetree::{NullTypeTreeProvider, TypeTreeNode, TypeTreeProvider};
use serde_json::Value;

use crate::game::folder::{NOT_SERIALIZED, is_asset_head};

pub type Tpk = TypeTreeCache<TpkTypeTreeBlob>;
/// Keyed by (serialized file name, path id).
pub type Changed = HashMap<(String, i64), Vec<u8>>;

const COMPRESSION_MASK: u32 = 0x3f;

pub struct Unit {
    pub name: String,
    pub data: Vec<u8>,
    pub file: SerializedFile,
    pub version: Option<UnityVersion>,
}

pub struct Script {
    pub class: String,
    pub namespace: String,
    pub assembly: String,
}

pub struct Source {
    pub units: Vec<Rc<Unit>>,
    entries: Vec<(String, Option<Rc<Unit>>)>,
    resources: Vec<(String, Vec<u8>)>,
    deferred: Vec<String>,
    path: PathBuf,
    dir: PathBuf,
    loaded: RefCell<HashMap<String, Option<Rc<Unit>>>>,
    walked: RefCell<Option<Vec<PathBuf>>>,
}

/// `home` replaces the file's own folder when externals are looked up (a backup copy outside the game).
pub fn open(path: &Path, home: Option<&Path>) -> Result<Source> {
    let (entries, resources, deferred) = match is_unityfs(path)? {
        true => read_bundle(path)?,
        false => {
            let bytes =
                std::fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;
            read_entries(bytes, &file_name(path))?
        }
    };
    let units = entries
        .iter()
        .filter_map(|(_, unit)| unit.clone())
        .collect();
    let dir = match home {
        Some(home) => home.to_path_buf(),
        None => path.parent().map_or_else(PathBuf::new, Path::to_path_buf),
    };
    Ok(Source {
        units,
        entries,
        resources,
        deferred,
        path: path.to_path_buf(),
        dir,
        loaded: RefCell::default(),
        walked: RefCell::default(),
    })
}

type Entries = (
    Vec<(String, Option<Rc<Unit>>)>,
    Vec<(String, Vec<u8>)>,
    Vec<String>,
);

fn is_unityfs(path: &Path) -> Result<bool> {
    let mut head = [0; 8];
    let file = File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let read = file.take(8).read(&mut head)?;
    Ok(head[..read] == *b"UnityFS\0")
}

fn read_entries(bytes: Vec<u8>, name: &str) -> Result<Entries> {
    if rawbundle::is_raw_bundle(&bytes) {
        return read_raw_bundle(&bytes);
    }
    let length = bytes.len() as u64;
    if !is_asset_head(&bytes[..bytes.len().min(48)], length) {
        return Ok((Vec::new(), Vec::new(), Vec::new()));
    }
    let unit = read_unit(name.to_string(), bytes, None)?;
    Ok((
        vec![(name.to_string(), Some(Rc::new(unit)))],
        Vec::new(),
        Vec::new(),
    ))
}

fn read_raw_bundle(bytes: &[u8]) -> Result<Entries> {
    let bundle = rawbundle::read_raw_bundle(bytes)?;
    let engine = bundle.header.engine_version.parse::<UnityVersion>().ok();
    let mut entries = Vec::new();
    let mut resources = Vec::new();
    for (path, data) in bundle.files {
        let (entry, resource) = split_entry(path, data, engine.clone())?;
        entries.push(entry);
        resources.extend(resource);
    }
    Ok((entries, resources, Vec::new()))
}

type Split = ((String, Option<Rc<Unit>>), Option<(String, Vec<u8>)>);

fn split_entry(path: String, data: Vec<u8>, engine: Option<UnityVersion>) -> Result<Split> {
    let serialized = !NOT_SERIALIZED.iter().any(|suffix| path.ends_with(suffix))
        && is_asset_head(&data[..data.len().min(48)], data.len() as u64);
    if !serialized {
        return Ok(((path.clone(), None), Some((path, data))));
    }
    let unit = read_unit(base_name(&path), data, engine)?;
    Ok(((path, Some(Rc::new(unit))), None))
}

fn read_bundle(path: &Path) -> Result<Entries> {
    let file = File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let mut bundle =
        BundleFileReader::from_reader(BufReader::new(file), &ExtractionConfig::default())?;
    let engine = bundle.header().unity_revision.clone();
    let mut entries = Vec::new();
    let mut resources = Vec::new();
    let mut deferred = Vec::new();
    while let Some(mut entry) = bundle.next() {
        let path = entry.path.clone();
        if NOT_SERIALIZED.iter().any(|suffix| path.ends_with(suffix)) {
            entries.push((path.clone(), None));
            deferred.push(path);
            continue;
        }
        let data = entry.read_owned()?;
        let (entry, resource) = split_entry(path, data, engine.clone())?;
        entries.push(entry);
        resources.extend(resource);
    }
    Ok((entries, resources, deferred))
}

fn read_deferred(path: &Path, entry: &str) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let bundle = BundleFileReader::from_reader(Cursor::new(bytes), &ExtractionConfig::default())?;
    let file = bundle
        .files()
        .iter()
        .find(|file| file.path == entry)
        .with_context(|| format!("{entry} is not in {}", path.display()))?;
    Ok(bundle.read_at_entry(file)?)
}

/// UnityPy takes the bundle's engine version when a serialized file's own version is blank.
fn read_unit(name: String, data: Vec<u8>, engine: Option<UnityVersion>) -> Result<Unit> {
    let file = SerializedFile::from_reader(&mut Cursor::new(&data))?;
    let version = file.m_UnityVersion.clone().or(engine);
    Ok(Unit {
        name,
        data,
        file,
        version,
    })
}

pub fn node_for<'a>(
    unit: &'a Unit,
    info: &ObjectInfo,
    tpk: &'a Tpk,
) -> Option<Cow<'a, TypeTreeNode>> {
    if let Ok(embedded) = unit.file.get_typetree_for(info, &NullTypeTreeProvider) {
        return Some(embedded);
    }
    tpk_node(unit, info.m_ClassID, tpk)
}

pub fn has_embedded_node(unit: &Unit, info: &ObjectInfo) -> bool {
    unit.file
        .get_typetree_for(info, &NullTypeTreeProvider)
        .is_ok()
}

pub fn tpk_node<'a>(unit: &Unit, class: ClassId, tpk: &'a Tpk) -> Option<Cow<'a, TypeTreeNode>> {
    tpk.get_typetree_node(class, unit.version.as_ref()?)
}

/// `exact` rejects a node that does not cover every byte.
pub fn read_value(
    unit: &Unit,
    info: &ObjectInfo,
    node: &TypeTreeNode,
    exact: bool,
) -> Result<Value> {
    let data = raw_data(unit, info)?;
    let mut reader = Cursor::new(data);
    let value: Value =
        serde_typetree::from_reader_endianed(&mut reader, node, unit.file.m_Header.m_Endianess)?;
    if exact && reader.position() != data.len() as u64 {
        bail!(
            "Expected to read {} bytes, but only read {} bytes",
            data.len(),
            reader.position()
        );
    }
    Ok(value)
}

/// A byte list as read: one base64 string from rabex, or numbers in hand-made trees.
pub fn read_bytes(value: Option<&Value>) -> Vec<u8> {
    match value {
        Some(Value::String(text)) => serde_typetree::decode_bytes(text)
            .and_then(Result::ok)
            .unwrap_or_default(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_u64().and_then(|byte| u8::try_from(byte).ok()))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn bytes_value(data: &[u8]) -> Value {
    Value::String(serde_typetree::encode_bytes(data))
}

pub fn raw_data<'a>(unit: &'a Unit, info: &ObjectInfo) -> Result<&'a [u8]> {
    let start = usize::try_from(info.m_Offset)?;
    unit.data
        .get(start..start + info.m_Size as usize)
        .context("object data is outside the file")
}

pub fn read_head(unit: &Unit, info: &ObjectInfo, tpk: &Tpk) -> Result<Value> {
    let node = tpk_node(unit, ClassId::MonoBehaviour, tpk).context("no MonoBehaviour typetree")?;
    read_value(unit, info, &node, false)
}

impl Source {
    pub fn follow(&self, unit: &Rc<Unit>, pptr: &Value) -> Option<(Rc<Unit>, i64)> {
        let (file_id, path_id) = (int_field(pptr, "m_FileID"), int_field(pptr, "m_PathID"));
        if path_id == 0 {
            return None;
        }
        if file_id == 0 {
            return Some((Rc::clone(unit), path_id));
        }
        let external = unit
            .file
            .m_Externals
            .get(usize::try_from(file_id - 1).ok()?)?;
        Some((self.find_external(&external.pathName)?, path_id))
    }

    pub fn read_script(&self, unit: &Rc<Unit>, head: &Value, tpk: &Tpk) -> Option<Script> {
        let (target, path_id) = self.follow(unit, head.get("m_Script")?)?;
        let info = target.file.get_object_info(path_id)?;
        let node = node_for(&target, info, tpk)?;
        let script = read_value(&target, info, &node, true).ok()?;
        let text = |key: &str| script.get(key)?.as_str().map(str::to_string);
        Some(Script {
            class: text("m_ClassName")?,
            namespace: text("m_Namespace")?,
            assembly: text("m_AssemblyName")?,
        })
    }

    /// A `.resS` entry of the same bundle first, else a file in the home folder.
    pub fn read_resource(&self, path: &str, offset: u64, size: u64) -> Result<Vec<u8>> {
        let base = base_name(path);
        let stem = base
            .rsplit_once('.')
            .map_or(base.as_str(), |(stem, _)| stem);
        let names = [
            base.clone(),
            format!("{stem}.resource"),
            format!("{stem}.assets.resS"),
            format!("{stem}.resS"),
        ];
        let start = usize::try_from(offset)?;
        let end = start + usize::try_from(size)?;
        if let Some(data) = self.find_bundle_resource(&names) {
            return Ok(data
                .get(start..end)
                .context("resource data is outside the file")?
                .to_vec());
        }
        if let Some(entry) = self.find_deferred(&names) {
            return Ok(read_deferred(&self.path, entry)?
                .get(start..end)
                .context("resource data is outside the file")?
                .to_vec());
        }
        self.read_home_resource(&names, &base, offset, size)
    }

    fn find_deferred(&self, names: &[String]) -> Option<&String> {
        names.iter().find_map(|name| {
            self.deferred
                .iter()
                .find(|path| base_name(path).to_lowercase() == name.to_lowercase())
        })
    }

    fn find_bundle_resource(&self, names: &[String]) -> Option<&[u8]> {
        names.iter().find_map(|name| {
            self.resources
                .iter()
                .find(|(path, _)| base_name(path).to_lowercase() == name.to_lowercase())
                .map(|(_, data)| data.as_slice())
        })
    }

    /// A `.resS` file can be gigabytes; only the object's own bytes are read.
    fn read_home_resource(
        &self,
        names: &[String],
        base: &str,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>> {
        let found = names
            .iter()
            .find_map(|name| find_case_insensitive(&self.dir, name));
        let path = found.with_context(|| format!("Resource file {base} not found"))?;
        let mut file =
            File::open(&path).with_context(|| format!("Cannot read {}", path.display()))?;
        file.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0; usize::try_from(size)?];
        file.read_exact(&mut data)
            .context("resource data is outside the file")?;
        Ok(data)
    }

    fn find_external(&self, path: &str) -> Option<Rc<Unit>> {
        let clean = clean_external(path);
        if let Some((_, unit)) = self
            .entries
            .iter()
            .find(|(name, _)| name.to_lowercase() == clean)
        {
            return unit.clone();
        }
        if let Some(found) = self.loaded.borrow().get(&clean) {
            return found.clone();
        }
        let found = self
            .locate_file(&clean)
            .and_then(|path| load_unit(&path, &clean));
        self.loaded.borrow_mut().insert(clean, found.clone());
        found
    }

    /// Next to the file first, then any file under that folder whose path ends with the name.
    fn locate_file(&self, clean: &str) -> Option<PathBuf> {
        let direct = self.dir.join(clean);
        if direct.exists() {
            return Some(direct);
        }
        let mut walked = self.walked.borrow_mut();
        let files = walked.get_or_insert_with(|| list_files(&self.dir));
        let ends_with = |path: &PathBuf| path.to_string_lossy().ends_with(clean);
        let simple = |path: &PathBuf| {
            let parent = path
                .parent()
                .map(|dir| dir.to_string_lossy())
                .unwrap_or_default();
            format!("{parent}/{}", file_name(path).to_lowercase()).ends_with(clean)
        };
        files
            .iter()
            .find(|path| ends_with(path))
            .or_else(|| files.iter().find(|path| simple(path)))
            .cloned()
    }
}

/// Bundles and non-serialized dependencies hold no objects to read.
fn load_unit(path: &Path, name: &str) -> Option<Rc<Unit>> {
    let bytes = std::fs::read(path).ok()?;
    if !is_asset_head(&bytes[..bytes.len().min(48)], bytes.len() as u64)
        || bytes.starts_with(b"UnityFS\0")
    {
        return None;
    }
    read_unit(name.to_string(), bytes, None).ok().map(Rc::new)
}

fn clean_external(path: &str) -> String {
    let path = path.strip_prefix("archive:/").unwrap_or(path);
    let path = path.strip_prefix("assets/").unwrap_or(path);
    path.rsplit('/').next().unwrap_or(path).to_lowercase()
}

fn list_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

/// Bundles keep their packing.
pub fn save(path: &Path, changed: &Changed, target: &Path) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let file =
        File::create(target).with_context(|| format!("Cannot write {}", target.display()))?;
    let mut out = BufWriter::new(file);
    if bytes.starts_with(b"UnityFS\0") {
        save_bundle(bytes, changed, &mut out)?;
    } else if rawbundle::is_raw_bundle(&bytes) {
        out.write_all(&save_raw_bundle(&bytes, changed)?)?;
    } else {
        write_serialized(&bytes, &file_name(path), None, changed, &mut out)?;
    }
    out.flush()
        .with_context(|| format!("Cannot write {}", target.display()))
}

fn save_bundle(bytes: Vec<u8>, changed: &Changed, out: &mut (impl Write + Seek)) -> Result<()> {
    let header = BundleFileHeader::from_reader(&mut Cursor::new(&bytes))?;
    let bundle = BundleFileReader::from_reader(Cursor::new(bytes), &ExtractionConfig::default())?;
    let engine = header.unity_revision.clone();
    let flags = bundle.blocks().first().map_or(0, |block| block.flags);
    let compression = CompressionType::try_from(flags & COMPRESSION_MASK)?;
    let mut entries = Vec::new();
    let mut data = Vec::new();
    for entry in bundle.files() {
        let before = bundle.read_at_entry(entry)?;
        let name = base_name(&entry.path);
        let touched = changed.keys().any(|(file, _)| *file == name);
        let after = match touched {
            true => {
                let mut rewritten = Cursor::new(Vec::new());
                write_serialized(&before, &name, engine.clone(), changed, &mut rewritten)?;
                rewritten.into_inner()
            }
            false => before,
        };
        entries.push(FileEntry {
            offset: i64::try_from(data.len())?,
            size: i64::try_from(after.len())?,
            flags: entry.flags,
            path: entry.path.clone(),
        });
        data.extend(after);
    }
    write_bundle(&header, out, compression, compression, &entries, &data)?;
    Ok(())
}

fn save_raw_bundle(bytes: &[u8], changed: &Changed) -> Result<Vec<u8>> {
    let bundle = rawbundle::read_raw_bundle(bytes)?;
    let engine = bundle.header.engine_version.parse::<UnityVersion>().ok();
    let files = bundle
        .files
        .into_iter()
        .map(|(path, data)| {
            let name = base_name(&path);
            if !changed.keys().any(|(file, _)| *file == name) {
                return Ok((path, data));
            }
            let mut rewritten = Cursor::new(Vec::new());
            write_serialized(&data, &name, engine.clone(), changed, &mut rewritten)?;
            Ok((path, rewritten.into_inner()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rawbundle::write_raw_bundle(&bundle.header, &files)?)
}

fn write_serialized(
    bytes: &[u8],
    name: &str,
    engine: Option<UnityVersion>,
    changed: &Changed,
    out: &mut (impl Write + Seek),
) -> Result<()> {
    let file = SerializedFile::from_reader(&mut Cursor::new(bytes))?;
    let version = file
        .m_UnityVersion
        .clone()
        .or(engine)
        .context("no Unity version")?;
    let objects = file
        .objects()
        .map(|info| {
            let data = match changed.get(&(name.to_string(), info.m_PathID)) {
                Some(new) => Cow::Owned(new.clone()),
                None => {
                    let start = usize::try_from(info.m_Offset)?;
                    Cow::Borrowed(
                        bytes
                            .get(start..start + info.m_Size as usize)
                            .context("object data is outside the file")?,
                    )
                }
            };
            Ok((info.clone(), data))
        })
        .collect::<Result<Vec<_>>>()?;
    let blob = TpkTypeTreeBlob::embedded();
    let offsets = build_common_offset_map(&blob, &version);
    write_serialized_with_objects(out, &file, &offsets, objects.into_iter())?;
    Ok(())
}

/// Unity paths are case-insensitive; UnityPy tries the exact name first.
fn find_case_insensitive(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_file() && file_name(path).to_lowercase() == name.to_lowercase())
}

pub fn int_field(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn base_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}
