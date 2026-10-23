use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, anyhow, bail};
use indexmap::IndexMap;
use rabex::files::serializedfile::ObjectInfo;
use rabex::objects::ClassId;
use rabex::serde_typetree;
use rabex::typetree::TypeTreeNode;
use serde_json::{Value, json};

use super::recipe::{self, Recipe};
use super::render::{self, Gray};
use super::{bake, legacy};
use crate::game::assets::{self, Changed, Fit, Source, Tpk, Unit};
use crate::game::folder;
use crate::game::scan::{
    self, AtlasSize, Pointers, Reader, Ref, STATIC_POPULATION, TMP_FONT, TmpFont,
};
use crate::game::texture;
use crate::game::tmp_schema::{self, Tree};
use crate::game::typetrees::Generator;
use crate::progress::{self, Report};
use crate::work::project;

#[derive(Debug, Default)]
pub struct Options {
    pub raw_note: String,
}

#[derive(Debug)]
pub struct Bake {
    pub recipe: Recipe,
    pub baked: Tree,
    pub atlas: Gray,
}

#[derive(Debug)]
pub struct Prepared {
    pub tree: Tree,
    pub atlas: Gray,
    pub characters: usize,
    pub lost: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Injected {
    pub file: String,
    pub font: String,
    pub characters: usize,
    pub resized: bool,
    pub materials: usize,
    pub lost: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swap {
    pub file: String,
    pub serialized_file: String,
    pub path_id: i64,
    pub font: String,
    pub source: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swapped {
    pub file: String,
    pub font: String,
    pub source: String,
    pub lost: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reset {
    pub file: String,
    pub serialized_file: String,
    pub path_id: i64,
    pub font: String,
    pub source: PathBuf,
    pub unity_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleared {
    pub file: String,
    pub font: String,
    pub source: String,
}

#[derive(Debug)]
pub enum Job {
    Swap(Swap),
    Bake(Box<Bake>),
    Reset(Reset),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    Injected(Injected),
    Swapped(Swapped),
    Cleared(Cleared),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontAtlas {
    pub name: String,
    pub atlas_ids: HashSet<i64>,
}

/// Keyed by the font's reference.
pub type FontAtlases = IndexMap<Ref, FontAtlas>;

struct Material<'file> {
    unit: &'file Unit,
    info: &'file ObjectInfo,
    node: TypeTreeNode,
    value: Value,
}

/// Keyed by the main texture's reference.
type Materials<'file> = HashMap<Ref, Vec<Material<'file>>>;

#[derive(Default)]
struct FileIndex<'file> {
    atlases: FontAtlases,
    materials: Materials<'file>,
}

/// A field the font asset lacks, reported as an unexpected font asset layout.
#[derive(Debug)]
struct Missing(String);

impl fmt::Display for Missing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}'", self.0)
    }
}

impl Error for Missing {}

struct Opened<'file> {
    source: &'file Source,
    reader: &'file Reader<'file>,
}

pub fn load_bake(recipe_path: &Path) -> Result<Bake> {
    let built = recipe::load(recipe_path)?;
    let baked = recipe::load_baked(recipe_path)?;
    let atlas_png = recipe::artifact_paths(recipe_path)?.png;
    if !baked.get("glyphs").is_some_and(is_truthy)
        || !baked.get("characters").is_some_and(is_truthy)
    {
        bail!("{}: the bake has no glyphs", built.font);
    }
    let atlas = texture::read_png_alpha(&atlas_png)?;
    let size = (
        baked
            .get("atlas_width")
            .map_or(0, |v| tmp_schema::number(v) as usize),
        baked
            .get("atlas_height")
            .map_or(0, |v| tmp_schema::number(v) as usize),
    );
    if (atlas.width, atlas.height) != size {
        bail!(
            "{} is ({}, {}), the bake says ({}, {})",
            file_name(&atlas_png),
            atlas.width,
            atlas.height,
            size.0,
            size.1
        );
    }
    Ok(Bake {
        recipe: built,
        baked,
        atlas,
    })
}

#[derive(Debug)]
pub struct Files<'run> {
    pub data_dir: &'run Path,
    pub originals: Option<&'run Path>,
    pub out_dir: &'run Path,
}

#[derive(Debug, Default)]
pub struct Jobs {
    pub bakes: Vec<Bake>,
    pub swaps: Vec<Swap>,
    pub resets: Vec<Reset>,
}

pub fn inject_all(
    files: &Files,
    jobs: Jobs,
    generator: Option<&Generator>,
    options: &Options,
    report: Report,
) -> Result<Vec<Done>> {
    let Files {
        data_dir,
        originals,
        out_dir,
    } = *files;
    folder::reject_output_inside(data_dir, out_dir)?;
    let groups = group_jobs(jobs);
    let mut done = Vec::new();
    for (file, jobs) in progress::track(&groups, "Writing", |(file, _)| file.clone(), report) {
        let target = data_dir.join(file);
        let home = target.parent().unwrap_or(data_dir);
        let source = project::original_file(originals, data_dir, file);
        done.extend(inject_file(
            &source,
            home,
            jobs,
            &out_dir.join(file),
            generator,
            options,
        )?);
    }
    Ok(done)
}

/// Jobs by game file; within a file, swaps come before the dynamic fonts drawn from them are cleared.
fn group_jobs(
    Jobs {
        bakes,
        swaps,
        resets,
    }: Jobs,
) -> Vec<(String, Vec<Job>)> {
    let mut jobs: Vec<Job> = swaps
        .into_iter()
        .map(Job::Swap)
        .chain(bakes.into_iter().map(|b| Job::Bake(Box::new(b))))
        .chain(resets.into_iter().map(Job::Reset))
        .collect();
    jobs.sort_by(|a, b| read_job_file(a).cmp(read_job_file(b)));
    let mut groups: Vec<(String, Vec<Job>)> = Vec::new();
    for job in jobs {
        let file = read_job_file(&job).to_string();
        match groups.last_mut() {
            Some((last, group)) if *last == file => group.push(job),
            _ => groups.push((file, vec![job])),
        }
    }
    groups
}

fn read_job_file(job: &Job) -> &str {
    match job {
        Job::Swap(swap) => &swap.file,
        Job::Bake(bake) => &bake.recipe.file,
        Job::Reset(reset) => &reset.file,
    }
}

fn inject_file(
    source_path: &Path,
    home: &Path,
    jobs: &[Job],
    target: &Path,
    generator: Option<&Generator>,
    options: &Options,
) -> Result<Vec<Done>> {
    let (done, changed) = run_jobs(source_path, home, jobs, generator, options)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    assets::save(source_path, &changed, target)?;
    Ok(done)
}

/// The opened file is dropped before saving, so a big file is not held twice.
fn run_jobs(
    source_path: &Path,
    home: &Path,
    jobs: &[Job],
    generator: Option<&Generator>,
    options: &Options,
) -> Result<(Vec<Done>, Changed)> {
    let source = assets::open(source_path, Some(home))?;
    let reader = Reader::new(generator, &options.raw_note);
    let context = Opened {
        source: &source,
        reader: &reader,
    };
    let index = index_file(&context, jobs)?;
    let mut changed = Changed::new();
    let mut done = Vec::new();
    for job in jobs {
        done.push(run_job(&context, &index, job, options, &mut changed)?);
    }
    Ok((done, changed))
}

fn index_file<'file>(context: &Opened<'file>, jobs: &[Job]) -> Result<FileIndex<'file>> {
    // Reading every TMP font and material is slow in big files, so it is done once and only when needed.
    if jobs.iter().all(|job| matches!(job, Job::Swap(_))) {
        return Ok(FileIndex::default());
    }
    Ok(FileIndex {
        atlases: list_font_atlases(context),
        materials: list_materials(context.source)?,
    })
}

fn run_job(
    context: &Opened,
    index: &FileIndex,
    job: &Job,
    options: &Options,
    changed: &mut Changed,
) -> Result<Done> {
    match job {
        Job::Swap(swap) => swap_font(context, swap, changed).map(Done::Swapped),
        Job::Reset(reset) => {
            reset_dynamic(context, &index.atlases, reset, options, changed).map(Done::Cleared)
        }
        Job::Bake(bake) => inject_font(context, index, bake, options, changed).map(Done::Injected),
    }
}

fn reset_dynamic(
    context: &Opened,
    atlases: &FontAtlases,
    job: &Reset,
    options: &Options,
    changed: &mut Changed,
) -> Result<Cleared> {
    let found = find_object(context.source, &job.serialized_file, job.path_id);
    let read = found.and_then(|(unit, info)| read_tree(context, unit, info, changed));
    let (tree, atlas_id) = reset_tree(read.as_ref().map(|(tree, _)| tree), job, atlases, options)?;
    if let (Some((unit, info)), Some((_, node))) = (found, &read) {
        write_object(unit, info, node, &Value::Object(tree), changed)?;
    }
    if let Some((unit, info)) = find_object(context.source, &job.serialized_file, atlas_id)
        && info.m_ClassID == ClassId::Texture2D
    {
        let node = object_node(unit, info, context.reader)?;
        let cleared = clear_texture(&read_object(unit, info, &node, changed)?)?;
        write_object(unit, info, &node, &cleared, changed)?;
    }
    Ok(Cleared {
        file: job.file.clone(),
        font: job.font.clone(),
        source: file_name(&job.source),
    })
}

/// The checks and the cleared tree of `reset_dynamic`, with the path id of the atlas to clear.
fn reset_tree(
    tree: Option<&Tree>,
    job: &Reset,
    atlases: &FontAtlases,
    options: &Options,
) -> Result<(Tree, i64)> {
    let Some(tree) = tree.filter(|tree| tree.get("m_Name") == Some(&json!(job.font))) else {
        bail!(
            "{}: cannot read it in {}, {}; scan the game again",
            job.font,
            job.file,
            options.raw_note
        );
    };
    let population = tree
        .get("m_AtlasPopulationMode")
        .map_or(STATIC_POPULATION as f64, tmp_schema::number);
    if population == STATIC_POPULATION as f64 {
        bail!(
            "{}: not a dynamic font any more, scan the game again",
            job.font
        );
    }
    let empty = json!({});
    let atlas = tree
        .get("m_AtlasTextures")
        .and_then(Value::as_array)
        .and_then(|atlases| atlases.first())
        .unwrap_or(&empty);
    if assets::int_field(atlas, "m_FileID") != 0 {
        bail!(
            "{}: its atlas is in another game file, ufk cannot clear it",
            job.font
        );
    }
    let atlas_id = assets::int_field(atlas, "m_PathID");
    let sharing = list_sharing(atlases, &job.serialized_file, job.path_id, atlas_id);
    if !sharing.is_empty() {
        bail!(
            "{}: its atlas is shared with {}, ufk cannot clear it",
            job.font,
            sharing.join(", ")
        );
    }
    let face_info = require(tree, "m_FaceInfo")?;
    let point_size =
        tmp_schema::number(require_value(face_info, "m_PointSize")?).round_ties_even() as i64;
    let data = std::fs::read(&job.source)
        .with_context(|| format!("Cannot read {}", job.source.display()))?;
    let face = bake::read_font_face_info(&data, point_size, &job.unity_version)?;
    Ok((clear_dynamic(tree, &face)?, atlas_id))
}

/// Empty the letters a dynamic font cached, as TextMeshPro's ClearFontAssetTables does.
pub fn clear_dynamic(tree: &Tree, face: &Value) -> Result<Tree> {
    let mode = tree
        .get("m_AtlasRenderMode")
        .and_then(Value::as_u64)
        .and_then(|mode| u32::try_from(mode).ok());
    let is_sdf = mode.is_some_and(render::is_distance_field);
    let spacing = i64::from(is_sdf);
    let width = tmp_schema::number(require(tree, "m_AtlasWidth")?) as i64 - spacing;
    let height = tmp_schema::number(require(tree, "m_AtlasHeight")?) as i64 - spacing;
    let first_atlas: Vec<Value> = tree
        .get("m_AtlasTextures")
        .and_then(Value::as_array)
        .map(|atlases| atlases.iter().take(1).cloned().collect())
        .unwrap_or_default();
    let mut cleared = tree.clone();
    let updates = [
        ("m_FaceInfo", conform(face, require(tree, "m_FaceInfo")?)),
        ("m_GlyphTable", json!([])),
        ("m_CharacterTable", json!([])),
        ("m_UsedGlyphRects", json!([])),
        (
            "m_FreeGlyphRects",
            json!([{"m_X": 0, "m_Y": 0, "m_Width": width, "m_Height": height}]),
        ),
        ("m_AtlasTextures", Value::Array(first_atlas)),
        ("m_AtlasTextureIndex", json!(0)),
    ];
    for (key, value) in updates {
        cleared.insert(key.to_string(), value);
    }
    if let Some(table) = tree.get("m_FontFeatureTable") {
        cleared.insert("m_FontFeatureTable".to_string(), clear_lists(table));
    }
    Ok(cleared)
}

/// TextMeshPro draws new letters only into an atlas the game can read.
pub fn clear_texture(texture: &Value) -> Result<Value> {
    let mut readable = texture.clone();
    if let Some(fields) = readable.as_object_mut() {
        fields.insert("m_IsReadable".to_string(), json!(true));
    }
    let (width, height) = (
        assets::int_field(texture, "m_Width"),
        assets::int_field(texture, "m_Height"),
    );
    if width <= 0 || height <= 0 {
        return Ok(readable);
    }
    let (width, height) = (width as usize, height as usize);
    let blank = Gray {
        width,
        height,
        data: vec![0; width * height],
    };
    texture::write_alpha(&readable, &blank)
}

fn swap_font(context: &Opened, swap: &Swap, changed: &mut Changed) -> Result<Swapped> {
    let found = find_object(context.source, &swap.serialized_file, swap.path_id)
        .filter(|(_, info)| info.m_ClassID == ClassId::Font);
    let Some((unit, info)) = found else {
        bail!(
            "{}: no Font {} in {}, scan the game again",
            swap.font,
            swap.path_id,
            swap.file
        );
    };
    let node = object_node(unit, info, context.reader)?;
    let tree = match read_object(unit, info, &node, changed)? {
        Value::Object(tree) => tree,
        _ => bail!("{}: object {} is not a Font", swap.file, swap.path_id),
    };
    let (swapped, done) = swap_tree(&tree, swap)?;
    write_object(unit, info, &node, &Value::Object(swapped), changed)?;
    Ok(done)
}

/// The Font tree with the new font data, checked to be the font the plan names.
fn swap_tree(tree: &Tree, swap: &Swap) -> Result<(Tree, Swapped)> {
    let name = tree.get("m_Name");
    if name != Some(&json!(swap.font)) {
        bail!(
            "{} object {} is {}, not {}",
            swap.file,
            swap.path_id,
            describe_value(name),
            describe_value(Some(&json!(swap.font)))
        );
    }
    let data = std::fs::read(&swap.source)
        .with_context(|| format!("Cannot read {}", swap.source.display()))?;
    let old = assets::read_bytes(tree.get("m_FontData"));
    let lost = legacy::list_lost(&old, &data)?;
    let swapped = legacy::swap_font_data(tree, &data)?;
    let done = Swapped {
        file: swap.file.clone(),
        font: swap.font.clone(),
        source: file_name(&swap.source),
        lost,
    };
    Ok((swapped, done))
}

fn inject_font(
    context: &Opened,
    index: &FileIndex,
    bake: &Bake,
    options: &Options,
    changed: &mut Changed,
) -> Result<Injected> {
    let built = &bake.recipe;
    let Some((unit, info)) = find_object(context.source, &built.serialized_file, built.path_id)
    else {
        bail!(
            "{}: no object {} in {}, the game may have changed; scan the game again",
            built.font,
            built.path_id,
            built.file
        );
    };
    let Some((tree, node)) = read_tree(context, unit, info, changed) else {
        bail!(
            "{}: cannot read it in {}, {}",
            built.font,
            built.file,
            options.raw_note
        );
    };
    let name = tree.get("m_Name");
    if name != Some(&json!(built.font)) {
        bail!(
            "{} object {} is {}, not {}",
            built.file,
            built.path_id,
            describe_value(name),
            describe_value(Some(&json!(built.font)))
        );
    }
    let atlas_id = require_own_atlas(&tree, context.source, &index.atlases, built)?;
    let (atlas_unit, atlas_info) = find_object(context.source, &built.serialized_file, atlas_id)
        .context("the atlas texture is gone")?;
    let texture_node = object_node(atlas_unit, atlas_info, context.reader)?;
    let texture = read_object(atlas_unit, atlas_info, &texture_node, changed)?;
    let (font, prepared) = read_and_prepare(&tree, unit, info, built, bake)?;
    write_object(unit, info, &node, &Value::Object(prepared.tree), changed)?;
    let written = texture::write_alpha(&texture, &prepared.atlas)?;
    write_object(atlas_unit, atlas_info, &texture_node, &written, changed)?;
    let (width, height) = (prepared.atlas.width, prepared.atlas.height);
    let materials = index
        .materials
        .get(&Ref {
            serialized_file: built.serialized_file.clone(),
            path_id: atlas_id,
        })
        .map_or(&[][..], Vec::as_slice);
    Ok(Injected {
        file: built.file.clone(),
        font: built.font.clone(),
        characters: prepared.characters,
        resized: font.atlas
            != Some(AtlasSize {
                width: width as i64,
                height: height as i64,
            }),
        materials: update_materials(materials, width, height, changed)?,
        lost: prepared.lost,
    })
}

fn read_and_prepare(
    tree: &Tree,
    unit: &Unit,
    info: &ObjectInfo,
    built: &Recipe,
    bake: &Bake,
) -> Result<(TmpFont, Prepared)> {
    let font = scan::read_tmp_font(
        Path::new(&built.file),
        &Pointers::of(unit),
        info.m_PathID,
        tree,
    )
    .map_err(|error| layout_error(built, &Missing(format!("{error:#}"))))?;
    let prepared =
        prepare_font(&font, tree, bake).map_err(|error| match error.downcast::<Missing>() {
            Ok(missing) => layout_error(built, &missing),
            Err(error) => error,
        })?;
    Ok((font, prepared))
}

fn layout_error(built: &Recipe, missing: &Missing) -> anyhow::Error {
    anyhow!(
        "{}: unexpected font asset layout, missing {missing}",
        built.font
    )
}

pub fn prepare_font(font: &TmpFont, tree: &Tree, bake: &Bake) -> Result<Prepared> {
    let baked_codes: HashSet<i64> = list(&bake.baked, "characters")?
        .iter()
        .map(|c| Ok(tmp_schema::number(require_value(c, "m_Unicode")?) as i64))
        .collect::<Result<_>>()?;
    let lost: HashSet<i64> = font
        .codepoints
        .iter()
        .map(|cp| i64::from(*cp))
        .filter(|cp| !baked_codes.contains(cp))
        .collect();
    Ok(Prepared {
        tree: rebuild_tree(tree, &bake.baked)?,
        atlas: bake.atlas.clone(),
        characters: list(&bake.baked, "characters")?.len(),
        lost: to_sorted_text(&lost),
    })
}

fn require_own_atlas(
    tree: &Tree,
    source: &Source,
    atlases: &FontAtlases,
    built: &Recipe,
) -> Result<i64> {
    let refs = tmp_schema::read_atlas_refs(tree);
    let [atlas_ref] = refs.as_slice() else {
        bail!(
            "{}: has {} atlas textures, only single-atlas fonts are supported",
            built.font,
            refs.len()
        );
    };
    let (file_id, atlas_id) = (
        assets::int_field(atlas_ref, "m_FileID"),
        assets::int_field(atlas_ref, "m_PathID"),
    );
    let found = find_object(source, &built.serialized_file, atlas_id);
    let Some((_, info)) = found.filter(|_| file_id == 0) else {
        bail!(
            "{}: its atlas texture lives in another file, which is not supported",
            built.font
        );
    };
    if info.m_ClassID != ClassId::Texture2D {
        bail!(
            "{}: its atlas reference does not point at a texture",
            built.font
        );
    }
    let sharing = list_sharing(atlases, &built.serialized_file, built.path_id, atlas_id);
    if !sharing.is_empty() {
        bail!(
            "{}: its atlas is shared with {}",
            built.font,
            sharing.join(", ")
        );
    }
    Ok(atlas_id)
}

fn list_font_atlases(context: &Opened) -> FontAtlases {
    let mut atlases = FontAtlases::new();
    for unit in &context.source.units {
        for info in unit.file.objects() {
            if info.m_ClassID != ClassId::MonoBehaviour
                || scan::read_script_class(context.source, unit, info, context.reader).as_deref()
                    != Some(TMP_FONT)
            {
                continue;
            }
            let tree = read_tree(context, unit, info, &Changed::new())
                .map(|(tree, _)| tree)
                .unwrap_or_default();
            let name = tree
                .get("m_Name")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            let atlas_ids: HashSet<i64> = tmp_schema::read_atlas_refs(&tree)
                .iter()
                .filter(|pptr| assets::int_field(pptr, "m_FileID") == 0)
                .map(|pptr| assets::int_field(pptr, "m_PathID"))
                .collect();
            atlases.insert(scan::make_ref(unit, info), FontAtlas { name, atlas_ids });
        }
    }
    atlases
}

fn list_sharing(
    atlases: &FontAtlases,
    serialized_file: &str,
    path_id: i64,
    atlas_id: i64,
) -> Vec<String> {
    atlases
        .iter()
        .filter(|(font, atlas)| {
            font.serialized_file == serialized_file
                && font.path_id != path_id
                && atlas.atlas_ids.contains(&atlas_id)
        })
        .map(|(_, atlas)| atlas.name.clone())
        .collect()
}

pub fn rebuild_tree(tree: &Tree, baked: &Tree) -> Result<Tree> {
    if tmp_schema::is_new_schema(tree) {
        return rebuild_new_tree(tree, baked);
    }
    rebuild_old_tree(tree, baked)
}

pub fn rebuild_old_tree(tree: &Tree, baked: &Tree) -> Result<Tree> {
    let glyphs = tmp_schema::to_glyph_infos(baked);
    if glyphs.is_empty() {
        bail!("the bake has no glyphs for its characters");
    }
    let game_glyphs = tree
        .get("m_glyphInfoList")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty());
    let template = game_glyphs.unwrap_or(&glyphs)[0].clone();
    let face = require(baked, "face_info")?;
    let mut rebuilt = tree.clone();
    rebuilt.insert(
        "m_fontInfo".to_string(),
        conform(
            &tmp_schema::to_font_info(face, baked),
            require(tree, "m_fontInfo")?,
        ),
    );
    rebuilt.insert(
        "m_glyphInfoList".to_string(),
        Value::Array(glyphs.iter().map(|g| conform(g, &template)).collect()),
    );
    if let Some(kerning) = tree.get("m_kerningInfo") {
        // Kerning pairs were measured on the old face; a rebake may use another font.
        rebuilt.insert("m_kerningInfo".to_string(), clear_lists(kerning));
    }
    Ok(rebuilt)
}

fn has_pixels(rect: &Value) -> bool {
    tmp_schema::number(&rect["m_Width"]) != 0.0 && tmp_schema::number(&rect["m_Height"]) != 0.0
}

fn rebuild_new_tree(tree: &Tree, baked: &Tree) -> Result<Tree> {
    let (baked_glyphs, baked_characters) = (list(baked, "glyphs")?, list(baked, "characters")?);
    let glyph_template = first_of(list(tree, "m_GlyphTable")?, baked_glyphs)?;
    let char_template = first_of(list(tree, "m_CharacterTable")?, baked_characters)?;
    let used: Vec<Value> = baked_glyphs
        .iter()
        .map(|g| require_value(g, "m_GlyphRect").cloned())
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(has_pixels)
        .collect();
    let mut rebuilt = tree.clone();
    let updates = [
        (
            "m_FaceInfo",
            conform(require(baked, "face_info")?, require(tree, "m_FaceInfo")?),
        ),
        (
            "m_GlyphTable",
            Value::Array(
                baked_glyphs
                    .iter()
                    .map(|g| conform(g, &glyph_template))
                    .collect(),
            ),
        ),
        (
            "m_CharacterTable",
            Value::Array(
                baked_characters
                    .iter()
                    .map(|c| conform(c, &char_template))
                    .collect(),
            ),
        ),
        ("m_UsedGlyphRects", Value::Array(used)),
        ("m_FreeGlyphRects", json!([])),
        ("m_AtlasWidth", require(baked, "atlas_width")?.clone()),
        ("m_AtlasHeight", require(baked, "atlas_height")?.clone()),
        ("m_AtlasPadding", require(baked, "atlas_padding")?.clone()),
        ("m_AtlasRenderMode", require(baked, "render_mode")?.clone()),
    ];
    for (key, value) in updates {
        rebuilt.insert(key.to_string(), value);
    }
    if tree.contains_key("m_AtlasPopulationMode") {
        // The rebaked atlas holds every letter the plan asked for, so the font stops adding letters itself.
        rebuilt.insert(
            "m_AtlasPopulationMode".to_string(),
            json!(STATIC_POPULATION),
        );
    }
    if let Some(table) = tree.get("m_FontFeatureTable") {
        // Kerning records point at glyph indices, and the old indices mean other glyphs after a rebake.
        let empty = clear_lists(table);
        let baked_table = baked
            .get("font_feature_table")
            .filter(|t| is_truthy(t))
            .cloned()
            .unwrap_or_else(|| json!({}));
        rebuilt.insert(
            "m_FontFeatureTable".to_string(),
            conform(&baked_table, &empty),
        );
    }
    Ok(rebuilt)
}

fn first_of(game: &[Value], baked: &[Value]) -> Result<Value> {
    let chosen = if game.is_empty() { baked } else { game };
    chosen
        .first()
        .cloned()
        .context("neither the game font nor the bake has a glyph table")
}

fn clear_lists(table: &Value) -> Value {
    match table {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    let cleared = if value.is_array() {
                        json!([])
                    } else {
                        value.clone()
                    };
                    (key.clone(), cleared)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The template's fields, filled from `value` where it has them: game fields stay, unknown ones drop.
#[must_use]
pub fn conform(value: &Value, template: &Value) -> Value {
    let Value::Object(template) = template else {
        return value.clone();
    };
    let source = value.as_object();
    Value::Object(
        template
            .iter()
            .map(|(key, sub)| {
                let filled = match source.and_then(|s| s.get(key)) {
                    Some(own) => conform(own, sub),
                    None => sub.clone(),
                };
                (key.clone(), filled)
            })
            .collect(),
    )
}

fn list_materials(source: &Source) -> Result<Materials<'_>> {
    let tpk = Tpk::embedded();
    let mut materials = Materials::new();
    for unit in &source.units {
        for info in unit.file.objects() {
            if info.m_ClassID != ClassId::Material {
                continue;
            }
            let node = assets::node_for(unit, info, &tpk)
                .context("There are no TypeTree nodes for this object.")?
                .into_owned();
            let value = assets::read_value(unit, info, &node, Fit::Exact)?;
            let Some(texture_id) = find_main_texture_id(&value) else {
                continue;
            };
            materials
                .entry(Ref {
                    serialized_file: unit.name.clone(),
                    path_id: texture_id,
                })
                .or_default()
                .push(Material {
                    unit,
                    info,
                    node,
                    value,
                });
        }
    }
    Ok(materials)
}

fn update_materials(
    matching: &[Material],
    width: usize,
    height: usize,
    changed: &mut Changed,
) -> Result<usize> {
    for material in matching {
        let mut value = material.value.clone();
        let floats = value
            .pointer_mut("/m_SavedProperties/m_Floats")
            .context("a Material without m_SavedProperties.m_Floats")?;
        let updated = set_atlas_size(
            floats.as_array().map_or(&[][..], Vec::as_slice),
            width,
            height,
        );
        *floats = Value::Array(updated);
        write_object(
            material.unit,
            material.info,
            &material.node,
            &value,
            changed,
        )?;
    }
    Ok(matching.len())
}

#[must_use]
pub fn set_atlas_size(floats: &[Value], width: usize, height: usize) -> Vec<Value> {
    let sizes = [
        ("_TextureWidth", width as f64),
        ("_TextureHeight", height as f64),
    ];
    let is_size = |pair: &Value| {
        pair.get("first")
            .and_then(Value::as_str)
            .is_some_and(|key| sizes.iter().any(|(size, _)| *size == key))
    };
    floats
        .iter()
        .filter(|pair| !is_size(pair))
        .cloned()
        .chain(
            sizes
                .iter()
                .map(|(key, value)| json!({"first": key, "second": value})),
        )
        .collect()
}

fn find_main_texture_id(material: &Value) -> Option<i64> {
    material
        .pointer("/m_SavedProperties/m_TexEnvs")?
        .as_array()?
        .iter()
        .find(|pair| {
            pair.get("first").and_then(Value::as_str) == Some("_MainTex")
                && pair
                    .pointer("/second/m_Texture")
                    .is_some_and(|pptr| assets::int_field(pptr, "m_FileID") == 0)
        })
        .and_then(|pair| pair.pointer("/second/m_Texture"))
        .map(|pptr| assets::int_field(pptr, "m_PathID"))
}

/// When several units share the name, the last one wins.
fn find_object<'a>(
    source: &'a Source,
    file: &str,
    path_id: i64,
) -> Option<(&'a Rc<Unit>, &'a ObjectInfo)> {
    source
        .units
        .iter()
        .rev()
        .filter(|unit| unit.name == file)
        .find_map(|unit| Some((unit, unit.file.get_object_info(path_id)?)))
}

fn read_tree(
    context: &Opened,
    unit: &Rc<Unit>,
    info: &ObjectInfo,
    changed: &Changed,
) -> Option<(Tree, TypeTreeNode)> {
    let node = scan::tree_node(context.source, unit, info, context.reader)?;
    match read_object(unit, info, &node, changed).ok()? {
        Value::Object(tree) => Some((tree, node)),
        _ => None,
    }
}

fn object_node(unit: &Unit, info: &ObjectInfo, reader: &Reader) -> Result<TypeTreeNode> {
    assets::node_for(unit, info, &reader.tpk)
        .map(Cow::into_owned)
        .context("There are no TypeTree nodes for this object.")
}

/// Prefers the bytes written earlier in this run; they must be read to the end.
fn read_object(
    unit: &Unit,
    info: &ObjectInfo,
    node: &TypeTreeNode,
    changed: &Changed,
) -> Result<Value> {
    let Some(bytes) = changed.get(&scan::make_ref(unit, info)) else {
        return assets::read_value(unit, info, node, Fit::Exact);
    };
    let mut reader = Cursor::new(bytes.as_slice());
    let value: Value =
        serde_typetree::from_reader_endianed(&mut reader, node, unit.file.m_Header.m_Endianess)?;
    if reader.position() != bytes.len() as u64 {
        bail!(
            "Expected to read {} bytes, but only read {} bytes",
            bytes.len(),
            reader.position()
        );
    }
    Ok(value)
}

fn write_object(
    unit: &Unit,
    info: &ObjectInfo,
    node: &TypeTreeNode,
    value: &Value,
    changed: &mut Changed,
) -> Result<()> {
    let bytes = serde_typetree::to_vec_endianed(value, node, unit.file.m_Header.m_Endianess)?;
    changed.insert(scan::make_ref(unit, info), bytes);
    Ok(())
}

fn require<'a>(tree: &'a Tree, key: &str) -> Result<&'a Value> {
    tree.get(key)
        .ok_or_else(|| anyhow::Error::new(Missing(key.to_string())))
}

fn require_value<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value
        .get(key)
        .ok_or_else(|| anyhow::Error::new(Missing(key.to_string())))
}

fn list<'a>(tree: &'a Tree, key: &str) -> Result<&'a [Value]> {
    Ok(require(tree, key)?
        .as_array()
        .map_or(&[][..], Vec::as_slice))
}

fn to_sorted_text(codes: &HashSet<i64>) -> String {
    let mut sorted: Vec<i64> = codes.iter().copied().collect();
    sorted.sort_unstable();
    sorted
        .into_iter()
        .filter_map(|cp| u32::try_from(cp).ok().and_then(char::from_u32))
        .collect()
}

fn describe_value(value: Option<&Value>) -> String {
    value.map_or_else(|| "nothing".to_string(), Value::to_string)
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
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
    use serde_json::Map;

    use super::*;

    fn tree(value: Value) -> Tree {
        value.as_object().unwrap().clone()
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR")))
    }

    fn reset_job(source: PathBuf) -> Reset {
        Reset {
            file: "resources.assets".into(),
            serialized_file: "CAB-1".into(),
            path_id: 7,
            font: "Body SDF".into(),
            source,
            unity_version: "2020.3.12f1".into(),
        }
    }

    #[test]
    fn conform_keeps_game_fields_and_drops_unknown_ones() {
        let game_glyph = json!({"m_Index": 0, "m_Metrics": {"m_Width": 0.0, "m_Height": 0.0}, "m_ClassDefinitionType": 0});
        let baked_glyph = json!({"m_Index": 36, "m_Metrics": {"m_Width": 12.5, "m_Height": 20.0}, "m_AtlasIndex": 0});
        assert_eq!(
            conform(&baked_glyph, &game_glyph),
            json!({"m_Index": 36, "m_Metrics": {"m_Width": 12.5, "m_Height": 20.0}, "m_ClassDefinitionType": 0})
        );
    }

    #[test]
    fn rebuild_leaves_the_game_tree_alone_and_drops_old_kerning() {
        let rect = json!({"m_X": 0, "m_Y": 0, "m_Width": 10, "m_Height": 12});
        let game = tree(json!({
            "m_FaceInfo": {"m_PointSize": 30},
            "m_GlyphTable": [{"m_Index": 1, "m_GlyphRect": rect}],
            "m_CharacterTable": [{"m_Unicode": 65, "m_GlyphIndex": 1}],
            "m_FontFeatureTable": {"m_GlyphPairAdjustmentRecords": [{"m_FirstGlyph": 1}]},
        }));
        let before = game.clone();
        let baked = tree(json!({
            "face_info": {"m_PointSize": 30},
            "glyphs": [{"m_Index": 5, "m_GlyphRect": rect}],
            "characters": [{"m_Unicode": 1031, "m_GlyphIndex": 5}],
            "atlas_width": 1024,
            "atlas_height": 512,
            "atlas_padding": 3,
            "render_mode": 16422,
        }));
        let rebuilt = rebuild_tree(&game, &baked).unwrap();
        assert_eq!(game, before);
        assert_eq!(
            rebuilt["m_CharacterTable"],
            json!([{"m_Unicode": 1031, "m_GlyphIndex": 5}])
        );
        assert_eq!(
            rebuilt["m_FontFeatureTable"],
            json!({"m_GlyphPairAdjustmentRecords": []})
        );
    }

    #[test]
    fn atlas_size_sets_missing_and_existing_material_fields() {
        let floats = [
            json!({"first": "_GradientScale", "second": 4.0}),
            json!({"first": "_TextureWidth", "second": 512.0}),
        ];
        let sizes: Map<String, Value> = set_atlas_size(&floats, 1024, 512)
            .iter()
            .map(|pair| {
                (
                    pair["first"].as_str().unwrap().to_string(),
                    pair["second"].clone(),
                )
            })
            .collect();
        assert_eq!(
            Value::Object(sizes),
            json!({"_GradientScale": 4.0, "_TextureWidth": 1024.0, "_TextureHeight": 512.0})
        );
    }

    fn font_tree(padding: i64) -> Tree {
        let rect = json!({"m_X": 2, "m_Y": 2, "m_Width": 4, "m_Height": 4});
        tree(json!({
            "m_Name": "Body SDF",
            "m_FaceInfo": {"m_PointSize": 30},
            "m_AtlasWidth": 16,
            "m_AtlasHeight": 16,
            "m_AtlasPadding": padding,
            "m_GlyphTable": [{"m_Index": 1, "m_Metrics": {"m_HorizontalAdvance": 5.0}, "m_GlyphRect": rect}],
            "m_CharacterTable": [{"m_Unicode": 0xF1BE, "m_GlyphIndex": 1, "m_Scale": 1.0}],
        }))
    }

    fn baked_body() -> Tree {
        tree(json!({
            "face_info": {"m_PointSize": 30},
            "glyphs": [{"m_Index": 3, "m_GlyphRect": {"m_X": 0, "m_Y": 0, "m_Width": 0, "m_Height": 0}}],
            "characters": [{"m_Unicode": 65, "m_GlyphIndex": 3}],
            "atlas_width": 16,
            "atlas_height": 16,
            "atlas_padding": 1,
            "render_mode": 16422,
        }))
    }

    fn blank(size: usize) -> Gray {
        Gray {
            width: size,
            height: size,
            data: vec![0; size * size],
        }
    }

    fn prepare() -> Result<Prepared> {
        let game = font_tree(1);
        let pointers = Pointers {
            file: "CAB-1",
            externals: &[],
        };
        let font = scan::read_tmp_font(Path::new("resources.assets"), &pointers, 7, &game)?;
        let recipe: Recipe = serde_json::from_value(json!({
            "font": "Body SDF", "file": "resources.assets", "serialized_file": "CAB-1", "path_id": 7,
            "family": "Body", "point_size": 30, "padding": 1, "atlas_width": 16, "atlas_height": 16,
            "render_mode": 16422, "codepoints": [65], "added": "",
        }))?;
        let bake = Bake {
            recipe,
            baked: baked_body(),
            atlas: blank(16),
        };
        prepare_font(&font, &game, &bake)
    }

    #[test]
    fn prepare_reports_game_glyphs_the_new_font_lacks_and_leaves_them_out() {
        let prepared = prepare().unwrap();
        assert_eq!(prepared.lost, "\u{f1be}");
        let codes: HashSet<i64> = prepared.tree["m_CharacterTable"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["m_Unicode"].as_i64().unwrap())
            .collect();
        assert_eq!(codes, HashSet::from([65]));
    }

    #[test]
    fn old_schema_rebuild_writes_font_info_and_glyph_list() {
        let game = tree(json!({
            "m_fontInfo": {"Name": "", "PointSize": 0.0, "Padding": 0.0, "AtlasWidth": 0.0, "AtlasHeight": 0.0},
            "m_glyphInfoList": [],
            "m_kerningInfo": {"kerningPairs": [{"AscII_Left": 65}]},
        }));
        let baked = tree(json!({
            "face_info": {"m_FamilyName": "Body", "m_PointSize": 30},
            "glyphs": [{
                "m_Index": 3,
                "m_Metrics": {
                    "m_Width": 4.0, "m_Height": 4.0, "m_HorizontalBearingX": 0.0,
                    "m_HorizontalBearingY": 4.0, "m_HorizontalAdvance": 5.0,
                },
                "m_GlyphRect": {"m_X": 2, "m_Y": 10, "m_Width": 4, "m_Height": 4},
            }],
            "characters": [{"m_Unicode": 1031, "m_GlyphIndex": 3}],
            "atlas_width": 32,
            "atlas_height": 16,
            "atlas_padding": 2,
        }));
        let rebuilt = rebuild_old_tree(&game, &baked).unwrap();
        assert_eq!(
            rebuilt["m_fontInfo"],
            json!({"Name": "Body", "PointSize": 30, "Padding": 2.0, "AtlasWidth": 32.0, "AtlasHeight": 16.0})
        );
        let glyphs: Vec<(i64, f64)> = rebuilt["m_glyphInfoList"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| (g["id"].as_i64().unwrap(), g["y"].as_f64().unwrap()))
            .collect();
        assert_eq!(glyphs, [(1031, 2.0)]);
        assert_eq!(rebuilt["m_kerningInfo"], json!({"kerningPairs": []}));
    }

    #[test]
    fn font_swap_writes_the_new_data_and_refuses_another_font() {
        let old = std::fs::read(fixture("make-font-ABC.ttf")).unwrap();
        let game =
            tree(json!({"m_Name": "Body", "m_FontSize": 16.0, "m_FontData": old, "m_Ascent": 0.0}));
        let source = fixture("make-font-AB.ttf");
        let swap = |font: &str| Swap {
            file: "resources.assets".into(),
            serialized_file: "CAB-1".into(),
            path_id: 7,
            font: font.into(),
            source: source.clone(),
        };
        let (swapped, done) = swap_tree(&game, &swap("Body")).unwrap();
        assert_eq!(
            (
                assets::read_bytes(swapped.get("m_FontData")),
                done.lost.as_str()
            ),
            (std::fs::read(&source).unwrap(), "C")
        );
        let error = swap_tree(&game, &swap("Other")).err().unwrap();
        assert!(error.to_string().contains("not \"Other\""));
    }

    #[test]
    fn clearing_a_dynamic_font_matches_textmeshpro_and_takes_the_new_face() {
        let game = tree(json!({
            "m_FaceInfo": {"m_PointSize": 90, "m_LineHeight": 1.0},
            "m_GlyphTable": [{"m_Index": 3}],
            "m_CharacterTable": [{"m_Unicode": 65}],
            "m_UsedGlyphRects": [{"m_X": 0}],
            "m_FreeGlyphRects": [],
            "m_FontFeatureTable": {"m_GlyphPairAdjustmentRecords": [{"x": 1}]},
            "m_AtlasTextures": [{"m_PathID": 1}, {"m_PathID": 2}],
            "m_AtlasTextureIndex": 1,
            "m_AtlasWidth": 512,
            "m_AtlasHeight": 256,
            "m_AtlasRenderMode": 4165,
        }));
        let face = json!({"m_PointSize": 90, "m_LineHeight": 99.0, "m_UnitsPerEM": 1000});
        let cleared = clear_dynamic(&game, &face).unwrap();
        for key in ["m_GlyphTable", "m_CharacterTable", "m_UsedGlyphRects"] {
            assert_eq!(cleared[key], json!([]));
        }
        assert_eq!(
            cleared["m_FreeGlyphRects"],
            json!([{"m_X": 0, "m_Y": 0, "m_Width": 511, "m_Height": 255}])
        );
        assert_eq!(
            cleared["m_FontFeatureTable"],
            json!({"m_GlyphPairAdjustmentRecords": []})
        );
        assert_eq!(
            (&cleared["m_AtlasTextures"], &cleared["m_AtlasTextureIndex"]),
            (&json!([{"m_PathID": 1}]), &json!(0))
        );
        assert_eq!(
            cleared["m_FaceInfo"],
            json!({"m_PointSize": 90, "m_LineHeight": 99.0})
        );
    }

    #[test]
    fn dynamic_font_whose_atlas_another_font_uses_is_not_cleared() {
        let game = tree(json!({
            "m_Name": "Body SDF",
            "m_AtlasPopulationMode": 1,
            "m_AtlasTextures": [{"m_FileID": 0, "m_PathID": 82}],
        }));
        let atlases = FontAtlases::from([(
            Ref {
                serialized_file: "CAB-1".to_string(),
                path_id: 9,
            },
            FontAtlas {
                name: "Title SDF".to_string(),
                atlas_ids: HashSet::from([82]),
            },
        )]);
        let job = reset_job(PathBuf::from("new.ttf"));
        let error = reset_tree(Some(&game), &job, &atlases, &Options::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("shared with Title SDF"));
    }

    #[test]
    fn cleared_dynamic_atlas_is_readable_so_textmeshpro_can_draw_into_it() {
        let atlas = json!({"m_Width": 4, "m_Height": 4, "m_IsReadable": false, "m_TextureFormat": 1, "image data": []});
        let cleared = clear_texture(&atlas).unwrap();
        assert_eq!(cleared["m_IsReadable"], json!(true));
        assert_eq!(assets::read_bytes(cleared.get("image data")), vec![0u8; 16]);
    }

    #[test]
    fn dynamic_font_with_its_atlas_in_another_file_is_not_cleared() {
        let game = tree(json!({
            "m_Name": "Body SDF",
            "m_AtlasPopulationMode": 1,
            "m_AtlasTextures": [{"m_FileID": 1, "m_PathID": 82}],
        }));
        let job = reset_job(PathBuf::from("new.ttf"));
        let error = reset_tree(Some(&game), &job, &FontAtlases::new(), &Options::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("another game file"));
    }

    #[test]
    fn dynamic_font_reset_clears_its_tables_and_atlas_and_takes_the_new_face() {
        let game = tree(json!({
            "m_Name": "Body SDF",
            "m_AtlasPopulationMode": 1,
            "m_FaceInfo": {"m_PointSize": 40, "m_FamilyName": "Old"},
            "m_GlyphTable": [{"m_Index": 3}],
            "m_CharacterTable": [{"m_Unicode": 65}],
            "m_UsedGlyphRects": [],
            "m_FreeGlyphRects": [],
            "m_AtlasTextures": [{"m_FileID": 0, "m_PathID": 82}],
            "m_AtlasWidth": 64,
            "m_AtlasHeight": 64,
            "m_AtlasRenderMode": 4165,
        }));
        let job = reset_job(fixture("make-font-AB.ttf"));
        let (cleared, atlas_id) =
            reset_tree(Some(&game), &job, &FontAtlases::new(), &Options::default()).unwrap();
        assert_eq!(
            (
                &cleared["m_CharacterTable"],
                &cleared["m_FaceInfo"]["m_FamilyName"],
                atlas_id
            ),
            (&json!([]), &json!("Test"), 82)
        );
    }

    #[test]
    fn jobs_in_one_file_swap_fonts_before_clearing_the_dynamic_fonts_drawn_from_them() {
        let swap = Swap {
            file: "a.assets".into(),
            serialized_file: "CAB-1".into(),
            path_id: 1,
            font: "Body".into(),
            source: PathBuf::from("f.ttf"),
        };
        let reset = Reset {
            file: "a.assets".into(),
            path_id: 2,
            ..reset_job(PathBuf::from("f.ttf"))
        };
        let groups = group_jobs(Jobs {
            bakes: Vec::new(),
            swaps: vec![swap],
            resets: vec![reset],
        });
        let kinds: Vec<&str> = groups[0]
            .1
            .iter()
            .map(|job| match job {
                Job::Swap(_) => "swap",
                Job::Bake(_) => "bake",
                Job::Reset(_) => "reset",
            })
            .collect();
        assert_eq!((groups.len(), kinds), (1, vec!["swap", "reset"]));
    }
}
