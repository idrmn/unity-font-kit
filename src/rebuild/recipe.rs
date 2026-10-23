use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::fonts::charsets::{self, Charset};
use crate::game::folder;
use crate::game::scan::{AtlasSize, TmpFont};
use crate::game::tmp_schema::Tree;

/// TMP before 1.4 did not store its render mode; SDFAA is a guess the recipe file can override.
pub const GUESSED_RENDER_MODE: u32 = 4165;
pub const SUFFIX: &str = ".recipe.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipe {
    pub font: String,
    pub file: String,
    pub serialized_file: String,
    pub path_id: i64,
    pub family: String,
    pub point_size: i64,
    pub padding: usize,
    pub atlas_width: usize,
    pub atlas_height: usize,
    pub render_mode: u32,
    pub codepoints: Vec<u32>,
    pub added: String,
    /// Empty in recipes from an older ufk; only bake needs it.
    #[serde(default)]
    pub unity_version: String,
}

const FIELDS: [&str; 12] = [
    "font",
    "file",
    "serialized_file",
    "path_id",
    "family",
    "point_size",
    "padding",
    "atlas_width",
    "atlas_height",
    "render_mode",
    "codepoints",
    "added",
];

pub fn load(path: &Path) -> Result<Recipe> {
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let text = charsets::read_text(path)?;
    let data: Value = serde_json::from_str(&text)?;
    let Some(object) = data.as_object() else {
        bail!("{name} is not a ufk recipe");
    };
    let missing: Vec<&str> = FIELDS
        .into_iter()
        .filter(|field| !object.contains_key(*field))
        .collect();
    if !missing.is_empty() {
        bail!(
            "{name} is not a ufk recipe, it lacks {}",
            missing.join(", ")
        );
    }
    Ok(serde_json::from_value(data)?)
}

pub fn build(
    font: &TmpFont,
    data_dir: &Path,
    charset: &Charset,
    unity_version: &str,
) -> Result<Recipe> {
    let (Some(point_size), Some(AtlasSize { width, height }), Some(padding)) =
        (&font.point_size, font.atlas, font.padding)
    else {
        bail!("{}: ufk cannot read the settings of this font", font.name);
    };
    // A dynamic font without its source caches only the letters the game showed so far.
    let wanted = if font.dynamic == Some(true) {
        charset.all() + &printable_ascii()
    } else {
        charset.all()
    };
    let have: HashSet<u32> = font.codepoints.iter().copied().collect();
    let added = charsets::find_missing(&have, &wanted);
    let codepoints: BTreeSet<u32> = font
        .codepoints
        .iter()
        .copied()
        .chain(added.chars().map(u32::from))
        .collect();
    Ok(Recipe {
        font: font.name.clone(),
        file: relative_posix(&font.file, data_dir)?,
        serialized_file: font.reference.serialized_file.clone(),
        path_id: font.reference.path_id,
        family: font.family.clone().unwrap_or_default(),
        point_size: round_half_even(point_size)?,
        padding: convert(padding, "padding", &font.name)?,
        atlas_width: convert(width, "atlas width", &font.name)?,
        atlas_height: convert(height, "atlas height", &font.name)?,
        render_mode: match font.render_mode {
            None => GUESSED_RENDER_MODE,
            Some(mode) => convert(mode, "render mode", &font.name)?,
        },
        codepoints: codepoints.into_iter().collect(),
        added,
        unity_version: unity_version.to_string(),
    })
}

fn printable_ascii() -> String {
    (0x20u8..0x7F).map(char::from).collect()
}

fn relative_posix(path: &Path, root: &Path) -> Result<String> {
    if path.strip_prefix(root).is_err() {
        bail!(
            "{:?} is not in the subpath of {:?}",
            path.to_string_lossy(),
            root.to_string_lossy()
        );
    }
    Ok(folder::format_relative(path, root))
}

fn round_half_even(number: &Number) -> Result<i64> {
    if let Some(value) = number.as_i64() {
        return Ok(value);
    }
    let value = number.as_f64().unwrap_or(f64::NAN).round_ties_even();
    if !value.is_finite() {
        bail!("cannot round point size {number}");
    }
    Ok(value as i64)
}

fn convert<T: TryFrom<i64>>(value: i64, field: &str, font: &str) -> Result<T> {
    T::try_from(value).map_err(|_| anyhow!("{font}: {field} {value} is out of range"))
}

pub fn to_json(recipe: &Recipe) -> Result<String> {
    Ok(serde_json::to_string_pretty(recipe)?)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Artifacts {
    pub json: PathBuf,
    pub png: PathBuf,
}

pub fn artifact_paths(recipe_path: &Path) -> Result<Artifacts> {
    let name = recipe_path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let Some(stem) = name.strip_suffix(SUFFIX) else {
        bail!("A recipe file name ends with {SUFFIX}, got {name}");
    };
    let parent = recipe_path.parent().unwrap_or(Path::new(""));
    Ok(Artifacts {
        json: parent.join(format!("{stem}.json")),
        png: parent.join(format!("{stem}.png")),
    })
}

pub fn load_baked(recipe_path: &Path) -> Result<Tree> {
    let baked_json = artifact_paths(recipe_path)?.json;
    if !baked_json.exists() {
        let name = baked_json.file_name().unwrap_or_default().to_string_lossy();
        bail!("no {name}, apply the changes first");
    }
    let text = charsets::read_text(&baked_json)?;
    match serde_json::from_str(&text)
        .with_context(|| format!("cannot read {}", baked_json.display()))?
    {
        Value::Object(baked) => Ok(baked),
        _ => bail!("{} is not a baked font", baked_json.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rebuild::bake;

    const DATA: &str = "/game/Demo_Data";

    fn font(file: &str, chars: &str, path_id: i64) -> TmpFont {
        TmpFont {
            file: Path::new(DATA).join(file),
            reference: ("CAB-1".to_string(), path_id).into(),
            name: "Body SDF".to_string(),
            codepoints: chars.chars().map(u32::from).collect(),
            fallbacks: Vec::new(),
            family: None,
            point_size: None,
            padding: None,
            atlas: None,
            render_mode: None,
            dynamic: None,
            atlases: None,
            source: String::new(),
            source_font: None,
        }
    }

    fn with_settings(base: TmpFont, render_mode: i64, dynamic: Option<bool>) -> TmpFont {
        TmpFont {
            point_size: Number::from_f64(30.0),
            padding: Some(3),
            atlas: Some((512, 512).into()),
            render_mode: Some(render_mode),
            dynamic,
            ..base
        }
    }

    fn uk() -> Charset {
        charsets::get("uk").unwrap()
    }

    #[test]
    fn recipe_keeps_bake_settings_and_adds_missing_characters() {
        let body = TmpFont {
            family: Some("Body Regular".to_string()),
            ..with_settings(font("resources.assets", "ABCабв", 7), 16422, Some(false))
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Body SDF.recipe.json");
        let made = build(&body, Path::new(DATA), &uk(), "2020.3.12f1").unwrap();
        std::fs::write(&path, format!("\u{feff}{}", to_json(&made).unwrap())).unwrap();
        let built = load(&path).unwrap();
        assert_eq!(
            (
                built.point_size,
                built.padding,
                built.atlas_width,
                built.render_mode
            ),
            (30, 3, 512, 16422)
        );
        assert!(built.added.contains('Ґ') && !built.added.contains('а'));
    }

    #[test]
    fn artifacts_take_the_recipe_file_name() {
        assert_eq!(
            artifact_paths(Path::new("out/Body.v2.recipe.json")).unwrap(),
            Artifacts {
                json: PathBuf::from("out/Body.v2.json"),
                png: PathBuf::from("out/Body.v2.png"),
            }
        );
    }

    #[test]
    fn recipe_from_an_older_ufk_loads_and_bake_asks_to_scan_again() {
        let body = with_settings(font("resources.assets", "AB", 7), 4169, None);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Body SDF.recipe.json");
        let made = build(&body, Path::new(DATA), &uk(), "2020.3.12f1").unwrap();
        let mut data: Value = serde_json::from_str(&to_json(&made).unwrap()).unwrap();
        data.as_object_mut().unwrap().remove("unity_version");
        std::fs::write(&path, data.to_string()).unwrap();
        let built = load(&path).unwrap();
        let Err(error) = bake::bake(&built, b"") else {
            panic!("bake took a recipe without unity_version");
        };
        assert!(error.to_string().contains("scan the game again"));
    }

    #[test]
    fn a_dynamic_font_without_its_source_is_rebaked_with_printable_ascii() {
        let body = with_settings(font("resources.assets", "AB", 7), 4169, Some(true));
        let made = build(&body, Path::new(DATA), &uk(), "2019.4.26f1").unwrap();
        assert!(
            ['z', '~', ' ', 'Ґ']
                .iter()
                .all(|ch| made.codepoints.contains(&u32::from(*ch)))
        );
    }
}
