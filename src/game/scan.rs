use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result};
use rabex::files::serializedfile::{FileIdentifier, ObjectInfo};
use rabex::objects::ClassId;
use rabex::typetree::TypeTreeNode;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::game::assets::{self, Fit, Script, Source, Tpk, Unit};
use crate::game::raw_tmp;
use crate::game::tmp_schema::{self, Tree};
use crate::game::typetrees::{self, Generator};
use crate::progress::{self, Report};

pub const TMP_FONT: &str = "TMP_FontAsset";
pub const TMP_SETTINGS: &str = "TMP_Settings";
// Text components and the field that points at their font; namespaces tell them from a game's own "Text".
const TEXT_FONT_FIELDS: [TextFontField; 3] = [
    TextFontField {
        namespace: "TMPro",
        class: "TextMeshProUGUI",
        field_path: &["m_fontAsset"],
    },
    TextFontField {
        namespace: "TMPro",
        class: "TextMeshPro",
        field_path: &["m_fontAsset"],
    },
    TextFontField {
        namespace: "UnityEngine.UI",
        class: "Text",
        field_path: &["m_FontData", "m_Font"],
    },
];
pub const STATIC_POPULATION: i64 = 0;
const DYNAMIC_CASE: i64 = -2;
const FONT_CLASS: ClassId = ClassId::Font;

struct TextFontField {
    namespace: &'static str,
    class: &'static str,
    field_path: &'static [&'static str],
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(from = "(String, i64)", into = "(String, i64)")]
pub struct Ref {
    pub serialized_file: String,
    pub path_id: i64,
}

impl From<(String, i64)> for Ref {
    fn from((serialized_file, path_id): (String, i64)) -> Self {
        Ref {
            serialized_file,
            path_id,
        }
    }
}

impl From<Ref> for (String, i64) {
    fn from(reference: Ref) -> Self {
        (reference.serialized_file, reference.path_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "(i64, i64)", into = "(i64, i64)")]
pub struct AtlasSize {
    pub width: i64,
    pub height: i64,
}

impl From<(i64, i64)> for AtlasSize {
    fn from((width, height): (i64, i64)) -> Self {
        AtlasSize { width, height }
    }
}

impl From<AtlasSize> for (i64, i64) {
    fn from(size: AtlasSize) -> Self {
        (size.width, size.height)
    }
}

pub struct Pointers<'a> {
    pub file: &'a str,
    pub externals: &'a [FileIdentifier],
}

impl<'a> Pointers<'a> {
    #[must_use]
    pub fn of(unit: &'a Unit) -> Self {
        Pointers {
            file: &unit.name,
            externals: &unit.file.m_Externals,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegacyFont {
    pub file: PathBuf,
    pub reference: Ref,
    pub name: String,
    // Saved as font files next to the plan, so the scan cache stays small.
    #[serde(skip)]
    pub data: Vec<u8>,
    pub os_names: Vec<String>,
    /// m_ConvertCase -2: the Font draws letters from its font data while the game runs.
    pub dynamic: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TmpFont {
    pub file: PathBuf,
    pub reference: Ref,
    pub name: String,
    pub codepoints: BTreeSet<u32>,
    pub fallbacks: Vec<Ref>,
    pub family: Option<String>,
    pub point_size: Option<Number>,
    pub padding: Option<i64>,
    pub atlas: Option<AtlasSize>,
    pub render_mode: Option<i64>,
    pub dynamic: Option<bool>,
    pub atlases: Option<Vec<Option<Ref>>>,
    pub source: String,
    /// The legacy Font a dynamic font draws new letters from; None when the build left it out.
    pub source_font: Option<Ref>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextUse {
    pub font: Ref,
    pub owner: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TmpSettings {
    pub file: PathBuf,
    pub default_font: Option<Ref>,
    pub fallbacks: Vec<Ref>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanResult {
    pub legacy: Vec<LegacyFont>,
    pub tmp: Vec<TmpFont>,
    pub settings: Vec<TmpSettings>,
    pub errors: Vec<String>,
    pub uses: Vec<TextUse>,
}

enum Found {
    Legacy(LegacyFont),
    Tmp(TmpFont),
    Settings(TmpSettings),
    Use(TextUse),
    Error(String),
}

pub struct Reader<'a> {
    pub tpk: Tpk,
    generator: Option<&'a Generator>,
    raw_note: &'a str,
    generated: RefCell<HashMap<ScriptKey, Option<TypeTreeNode>>>,
}

#[derive(PartialEq, Eq, Hash)]
struct ScriptKey {
    assembly: String,
    namespace: String,
    class: String,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(generator: Option<&'a Generator>, raw_note: &'a str) -> Reader<'a> {
        Reader {
            tpk: Tpk::embedded(),
            generator,
            raw_note,
            generated: RefCell::default(),
        }
    }
}

#[must_use]
pub fn scan_files(
    files: &[PathBuf],
    generator: Option<&Generator>,
    raw_note: &str,
    homes: &HashMap<PathBuf, PathBuf>,
    report: Report,
) -> ScanResult {
    let reader = Reader::new(generator, raw_note);
    let parts = progress::track(files, "Reading", |path| file_name(path), report)
        .map(|path| scan_file_safely(path, &reader, homes.get(path).map(PathBuf::as_path)));
    merge_results(parts)
}

#[must_use]
pub fn merge_results(parts: impl IntoIterator<Item = ScanResult>) -> ScanResult {
    parts
        .into_iter()
        .fold(ScanResult::default(), |mut all, part| {
            all.legacy.extend(part.legacy);
            all.tmp.extend(part.tmp);
            all.settings.extend(part.settings);
            all.errors.extend(part.errors);
            all.uses.extend(part.uses);
            all
        })
}

fn scan_file_safely(path: &Path, reader: &Reader, home: Option<&Path>) -> ScanResult {
    // The vendored parser panics on some malformed files; one such file must not stop the scan.
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        scan_file(path, reader, home)
    }));
    read.unwrap_or_else(|_| ScanResult {
        errors: vec![format!("{}: ufk could not read this file", file_name(path))],
        ..ScanResult::default()
    })
}

/// One unreadable file must not stop the scan, nor one unreadable object hide the rest of its file.
fn scan_file(path: &Path, reader: &Reader, home: Option<&Path>) -> ScanResult {
    let name = file_name(path);
    let source = match assets::open(path, home) {
        Ok(source) => source,
        Err(error) => {
            return ScanResult {
                errors: vec![format!("{name}: {error:#}")],
                ..ScanResult::default()
            };
        }
    };
    let mut result = ScanResult::default();
    for unit in &source.units {
        for info in unit.file.objects() {
            let found = read_object(path, &source, unit, info, reader).unwrap_or_else(|error| {
                Some(Found::Error(format!(
                    "{name}: object {}: {error:#}",
                    info.m_PathID
                )))
            });
            match found {
                Some(Found::Legacy(font)) => result.legacy.push(font),
                Some(Found::Tmp(font)) => result.tmp.push(font),
                Some(Found::Settings(settings)) => result.settings.push(settings),
                Some(Found::Use(text)) => result.uses.push(text),
                Some(Found::Error(error)) => result.errors.push(error),
                None => {}
            }
        }
    }
    result
}

fn read_object(
    path: &Path,
    source: &Source,
    unit: &Rc<Unit>,
    info: &ObjectInfo,
    reader: &Reader,
) -> Result<Option<Found>> {
    if info.m_ClassID == FONT_CLASS {
        return read_legacy_font(path, unit, info, reader).map(|font| Some(Found::Legacy(font)));
    }
    if info.m_ClassID != ClassId::MonoBehaviour {
        return Ok(None);
    }
    let script = read_script(source, unit, info, reader);
    let kind = match &script {
        Some(script) => Some(script.class.clone()),
        None => guess_class_from_fields(unit, info, &reader.tpk),
    };
    let Some(kind) = kind else {
        return Ok(None);
    };
    if TEXT_FONT_FIELDS.iter().any(|field| field.class == kind) {
        return read_text_use(source, unit, info, script.as_ref(), reader);
    }
    if kind != TMP_FONT && kind != TMP_SETTINGS {
        return Ok(None);
    }
    let tree = read_tree(source, unit, info, reader);
    if kind == TMP_SETTINGS {
        let Some(tree) = tree else {
            let note = format!(
                "{}: TMP Settings unreadable, {}",
                file_name(path),
                reader.raw_note
            );
            return Ok(Some(Found::Error(note)));
        };
        return Ok(Some(Found::Settings(read_settings(
            path,
            &Pointers::of(unit),
            &tree,
        ))));
    }
    let font = match tree {
        Some(tree) => read_tmp_font(path, &Pointers::of(unit), info.m_PathID, &tree)?,
        None => read_tmp_font_raw(path, unit, info, reader)?,
    };
    Ok(Some(Found::Tmp(font)))
}

fn read_script(
    source: &Source,
    unit: &Rc<Unit>,
    info: &ObjectInfo,
    reader: &Reader,
) -> Option<Script> {
    assets::read_head(unit, info, &reader.tpk)
        .ok()
        .and_then(|head| source.read_script(unit, &head, &reader.tpk))
}

#[must_use]
pub fn read_script_class(
    source: &Source,
    unit: &Rc<Unit>,
    info: &ObjectInfo,
    reader: &Reader,
) -> Option<String> {
    match read_script(source, unit, info, reader) {
        Some(script) => Some(script.class),
        None => guess_class_from_fields(unit, info, &reader.tpk),
    }
}

/// Bundles often keep the MonoScript in another bundle; then the embedded typetree tells the class.
fn guess_class_from_fields(unit: &Unit, info: &ObjectInfo, tpk: &Tpk) -> Option<String> {
    if !assets::has_embedded_node(unit, info) {
        return None;
    }
    let node = assets::node_for(unit, info, tpk)?;
    let mut names = Vec::new();
    collect_names(&node, &mut names);
    if names.contains(&"m_CharacterTable") || names.contains(&"m_glyphInfoList") {
        return Some(TMP_FONT.to_string());
    }
    names
        .contains(&"m_defaultFontAsset")
        .then(|| TMP_SETTINGS.to_string())
}

fn collect_names<'a>(node: &'a TypeTreeNode, names: &mut Vec<&'a str>) {
    names.push(&node.m_Name);
    for child in &node.children {
        collect_names(child, names);
    }
}

fn read_text_use(
    source: &Source,
    unit: &Rc<Unit>,
    info: &ObjectInfo,
    script: Option<&Script>,
    reader: &Reader,
) -> Result<Option<Found>> {
    let Some(script) = script else {
        return Ok(None);
    };
    let Some(text_field) = TEXT_FONT_FIELDS
        .iter()
        .find(|field| field.namespace == script.namespace && field.class == script.class)
    else {
        return Ok(None);
    };
    let Some(tree) = read_tree(source, unit, info, reader) else {
        return Ok(None);
    };
    let root = Value::Object(tree);
    let mut pptr = &root;
    let empty = Value::Object(Tree::new());
    for field in text_field.field_path {
        pptr = pptr
            .get(*field)
            .filter(|value| is_truthy(value))
            .unwrap_or(&empty);
    }
    let Some(font) = resolve_pptr(&Pointers::of(unit), pptr) else {
        return Ok(None);
    };
    let owner = read_owner_name(
        unit,
        root.get("m_GameObject").unwrap_or(&empty),
        &reader.tpk,
    )?;
    Ok(Some(Found::Use(TextUse { font, owner })))
}

fn read_owner_name(unit: &Unit, pptr: &Value, tpk: &Tpk) -> Result<String> {
    if assets::int_field(pptr, "m_FileID") != 0 {
        return Ok(String::new());
    }
    let Some(info) = unit
        .file
        .get_object_info(assets::int_field(pptr, "m_PathID"))
    else {
        return Ok(String::new());
    };
    let Some(node) = assets::node_for(unit, info, tpk) else {
        return Ok(String::new());
    };
    let owner = assets::read_value(unit, info, &node, Fit::Prefix)?;
    Ok(owner
        .get("m_Name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

/// Last resort is the bare MonoBehaviour fields, which then fail the exact size check.
fn read_tree(source: &Source, unit: &Rc<Unit>, info: &ObjectInfo, reader: &Reader) -> Option<Tree> {
    let node = tree_node(source, unit, info, reader)?;
    match assets::read_value(unit, info, &node, Fit::Exact).ok()? {
        Value::Object(tree) => Some(tree),
        _ => None,
    }
}

#[must_use]
pub fn tree_node(
    source: &Source,
    unit: &Rc<Unit>,
    info: &ObjectInfo,
    reader: &Reader,
) -> Option<TypeTreeNode> {
    if assets::has_embedded_node(unit, info) {
        return assets::node_for(unit, info, &reader.tpk).map(Cow::into_owned);
    }
    generate_node(source, unit, info, reader).or_else(|| {
        assets::tpk_node(unit, ClassId::MonoBehaviour, &reader.tpk).map(Cow::into_owned)
    })
}

fn generate_node(
    source: &Source,
    unit: &Rc<Unit>,
    info: &ObjectInfo,
    reader: &Reader,
) -> Option<TypeTreeNode> {
    let generator = reader.generator?;
    let head = assets::read_head(unit, info, &reader.tpk).ok()?;
    let script = source.read_script(unit, &head, &reader.tpk)?;
    let key = ScriptKey {
        assembly: script.assembly,
        namespace: script.namespace,
        class: script.class,
    };
    if let Some(node) = reader.generated.borrow().get(&key) {
        return node.clone();
    }
    let node =
        typetrees::generate_script_tree(generator, &key.assembly, &key.namespace, &key.class).ok();
    reader.generated.borrow_mut().insert(key, node.clone());
    node
}

#[must_use]
pub fn make_ref(unit: &Unit, info: &ObjectInfo) -> Ref {
    Ref {
        serialized_file: unit.name.clone(),
        path_id: info.m_PathID,
    }
}

#[must_use]
pub fn resolve_pptr(pointers: &Pointers, pptr: &Value) -> Option<Ref> {
    let (file_id, path_id) = (
        assets::int_field(pptr, "m_FileID"),
        assets::int_field(pptr, "m_PathID"),
    );
    if path_id == 0 {
        return None;
    }
    if file_id == 0 {
        return Some(Ref {
            serialized_file: pointers.file.to_string(),
            path_id,
        });
    }
    let index = usize::try_from(file_id).ok()? - 1;
    let external = pointers.externals.get(index)?.pathName.replace('\\', "/");
    Some(Ref {
        serialized_file: path_name(&external),
        path_id,
    })
}

fn path_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

fn resolve_pptrs(pointers: &Pointers, pptrs: Option<&Value>) -> Vec<Ref> {
    pptrs
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|pptr| resolve_pptr(pointers, pptr))
        .collect()
}

fn read_legacy_font(
    path: &Path,
    unit: &Unit,
    info: &ObjectInfo,
    reader: &Reader,
) -> Result<LegacyFont> {
    let node = assets::node_for(unit, info, &reader.tpk)
        .context("There are no TypeTree nodes for this object.")?;
    let font = assets::read_value(unit, info, &node, Fit::Exact)?;
    let data = assets::read_bytes(font.get("m_FontData"));
    let os_names = font
        .get("m_FontNames")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(LegacyFont {
        file: path.to_path_buf(),
        reference: make_ref(unit, info),
        name: text(&font, "m_Name")?,
        data,
        os_names,
        dynamic: font
            .get("m_ConvertCase")
            .and_then(Value::as_i64)
            .unwrap_or(DYNAMIC_CASE)
            == DYNAMIC_CASE,
    })
}

fn read_settings(path: &Path, pointers: &Pointers, tree: &Tree) -> TmpSettings {
    let default = tree
        .get("m_defaultFontAsset")
        .filter(|value| is_truthy(value));
    TmpSettings {
        file: path.to_path_buf(),
        default_font: default.and_then(|pptr| resolve_pptr(pointers, pptr)),
        fallbacks: resolve_pptrs(pointers, tree.get("m_fallbackFontAssets")),
    }
}

pub fn read_tmp_font(
    path: &Path,
    pointers: &Pointers,
    path_id: i64,
    tree: &Tree,
) -> Result<TmpFont> {
    if tmp_schema::is_new_schema(tree) {
        return read_new_schema(path, pointers, path_id, tree);
    }
    read_old_schema(path, pointers, path_id, tree)
}

fn read_new_schema(path: &Path, pointers: &Pointers, path_id: i64, tree: &Tree) -> Result<TmpFont> {
    let empty = Value::Object(Tree::new());
    let face = tree
        .get("m_FaceInfo")
        .filter(|value| is_truthy(value))
        .unwrap_or(&empty);
    let family: Vec<&str> = ["m_FamilyName", "m_StyleName"]
        .iter()
        .filter_map(|key| face.get(*key).and_then(Value::as_str))
        .filter(|part| !part.is_empty())
        .collect();
    let int = |key: &str| tree.get(key).and_then(Value::as_i64);
    let atlas = match (int("m_AtlasWidth"), int("m_AtlasHeight")) {
        (Some(width), Some(height)) if width != 0 && height != 0 => {
            Some(AtlasSize { width, height })
        }
        _ => None,
    };
    let characters = tree
        .get("m_CharacterTable")
        .and_then(Value::as_array)
        .context("m_CharacterTable")?;
    let codepoints: BTreeSet<u32> = characters
        .iter()
        .map(|character| {
            character
                .get("m_Unicode")
                .and_then(Value::as_u64)
                .map(|unicode| unicode as u32)
                .context("m_Unicode")
        })
        .collect::<Result<_>>()?;
    let source_font = tree
        .get("m_SourceFontFile")
        .filter(|value| is_truthy(value));
    Ok(TmpFont {
        file: path.to_path_buf(),
        reference: Ref {
            serialized_file: pointers.file.to_string(),
            path_id,
        },
        name: tree_text(tree, "m_Name")?,
        codepoints,
        fallbacks: resolve_pptrs(pointers, tree.get("m_FallbackFontAssetTable")),
        family: Some(family.join(" ")),
        point_size: face.get("m_PointSize").and_then(as_number),
        padding: int("m_AtlasPadding"),
        atlas,
        render_mode: int("m_AtlasRenderMode"),
        dynamic: Some(
            int("m_AtlasPopulationMode").unwrap_or(STATIC_POPULATION) != STATIC_POPULATION,
        ),
        atlases: Some(read_atlases(pointers, tree)),
        source: "typetree".to_string(),
        source_font: source_font.and_then(|pptr| resolve_pptr(pointers, pptr)),
    })
}

fn read_old_schema(path: &Path, pointers: &Pointers, path_id: i64, tree: &Tree) -> Result<TmpFont> {
    let empty = Value::Object(Tree::new());
    let info = tree
        .get("m_fontInfo")
        .filter(|value| is_truthy(value))
        .unwrap_or(&empty);
    let whole = |key: &str| {
        info.get(key)
            .and_then(Value::as_f64)
            .map_or(0, |value| value.trunc() as i64)
    };
    let (width, height) = (whole("AtlasWidth"), whole("AtlasHeight"));
    let codepoints: BTreeSet<u32> = tree
        .get("m_glyphInfoList")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|glyph| {
            glyph
                .get("id")
                .and_then(Value::as_i64)
                .map(|id| id as u32)
                .context("id")
        })
        .collect::<Result<_>>()?;
    Ok(TmpFont {
        file: path.to_path_buf(),
        reference: Ref {
            serialized_file: pointers.file.to_string(),
            path_id,
        },
        name: tree_text(tree, "m_Name")?,
        codepoints,
        fallbacks: resolve_pptrs(pointers, tree.get("fallbackFontAssets")),
        family: info.get("Name").and_then(Value::as_str).map(str::to_string),
        point_size: info.get("PointSize").and_then(as_number),
        padding: info.get("Padding").map(|_| whole("Padding")),
        atlas: (width != 0 && height != 0).then_some(AtlasSize { width, height }),
        render_mode: None,
        dynamic: Some(false),
        atlases: Some(read_atlases(pointers, tree)),
        source: "typetree (TMP before 1.4)".to_string(),
        source_font: None,
    })
}

#[must_use]
pub fn read_atlases(pointers: &Pointers, tree: &Tree) -> Vec<Option<Ref>> {
    tmp_schema::read_atlas_refs(tree)
        .iter()
        .map(|pptr| resolve_pptr(pointers, pptr))
        .collect()
}

fn read_tmp_font_raw(
    path: &Path,
    unit: &Unit,
    info: &ObjectInfo,
    reader: &Reader,
) -> Result<TmpFont> {
    let head = assets::read_head(unit, info, &reader.tpk)?;
    Ok(TmpFont {
        file: path.to_path_buf(),
        reference: make_ref(unit, info),
        name: text(&head, "m_Name")?,
        codepoints: raw_tmp::extract_codepoints(assets::raw_data(unit, info)?)
            .into_iter()
            .collect(),
        fallbacks: Vec::new(),
        family: None,
        point_size: None,
        padding: None,
        atlas: None,
        render_mode: None,
        dynamic: None,
        atlases: None,
        source: format!("raw bytes, {}", reader.raw_note),
        source_font: None,
    })
}

#[must_use]
pub fn relocate(result: ScanResult, moves: &HashMap<PathBuf, PathBuf>) -> ScanResult {
    let moved = |file: PathBuf| moves.get(&file).cloned().unwrap_or(file);
    ScanResult {
        legacy: result
            .legacy
            .into_iter()
            .map(|font| LegacyFont {
                file: moved(font.file),
                ..font
            })
            .collect(),
        tmp: result
            .tmp
            .into_iter()
            .map(|font| TmpFont {
                file: moved(font.file),
                ..font
            })
            .collect(),
        settings: result
            .settings
            .into_iter()
            .map(|settings| TmpSettings {
                file: moved(settings.file),
                ..settings
            })
            .collect(),
        errors: result.errors,
        uses: result.uses,
    }
}

fn as_number(value: &Value) -> Option<Number> {
    match value {
        Value::Number(number) => Some(number.clone()),
        _ => None,
    }
}

fn text(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("no {key}"))
}

fn tree_text(tree: &Tree, key: &str) -> Result<String> {
    tree.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("no {key}"))
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tree(value: Value) -> Tree {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn dynamic_font_with_empty_table_keeps_new_schema_settings() {
        let tree = tree(json!({
            "m_Name": "Body SDF",
            "m_CharacterTable": [],
            "m_FaceInfo": {"m_FamilyName": "Body", "m_PointSize": 90},
            "m_AtlasPadding": 9,
            "m_AtlasWidth": 1024,
            "m_AtlasHeight": 1024,
            "m_AtlasPopulationMode": 2,
        }));
        let pointers = Pointers {
            file: "CAB-1",
            externals: &[],
        };
        let font = read_tmp_font(Path::new("fonts"), &pointers, 7, &tree).unwrap();
        assert_eq!(
            (font.dynamic, font.point_size, font.atlas),
            (
                Some(true),
                Some(Number::from(90)),
                Some(AtlasSize {
                    width: 1024,
                    height: 1024
                })
            )
        );
    }

    #[test]
    fn pointer_to_a_missing_external_file_resolves_to_nothing() {
        let externals = [FileIdentifier::new(r"archive:\CAB-2\CAB-2".to_string())];
        let pointers = Pointers {
            file: "CAB-1",
            externals: &externals,
        };
        let pptr = |file_id: i64| json!({"m_FileID": file_id, "m_PathID": 5});
        assert_eq!(
            resolve_pptr(&pointers, &pptr(1)),
            Some(Ref {
                serialized_file: "CAB-2".to_string(),
                path_id: 5
            })
        );
        assert_eq!(resolve_pptr(&pointers, &pptr(2)), None);
    }

    #[test]
    fn an_empty_atlas_reference_counts_as_no_atlas() {
        let tree = tree(
            json!({"m_AtlasTextures": [{"m_FileID": 0, "m_PathID": 0}], "m_CharacterTable": []}),
        );
        let pointers = Pointers {
            file: "CAB-1",
            externals: &[],
        };
        assert_eq!(read_atlases(&pointers, &tree), vec![None]);
    }
}
