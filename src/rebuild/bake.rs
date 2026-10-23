use std::collections::HashSet;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result, bail};
use indexmap::{IndexMap, IndexSet};
use png::{ColorType, Encoder};
use serde_json::{Value, json};

use super::kerning::{self, PairRecord, Shift};
use super::pack::{self, Atlas, Size, Spot};
use super::recipe::{self, Recipe};
use super::render::{self, Gray, Metrics, Style, Tile};
use crate::freetype::{Face, LOAD_DEFAULT, LOAD_NO_HINTING, Library};
use crate::work::project;

#[derive(Debug)]
pub struct Baked {
    pub data: Value,
    /// Top row first, while TMP textures store the bottom row first.
    pub atlas: Gray,
}

pub fn write(recipe_path: &Path, font_path: &Path) -> Result<()> {
    let recipe = recipe::load(recipe_path)?;
    let font =
        fs::read(font_path).with_context(|| format!("Cannot read {}", font_path.display()))?;
    let baked = bake(&recipe, &font)?;
    let artifacts = recipe::artifact_paths(recipe_path)?;
    project::write_text(&artifacts.json, &serde_json::to_string(&baked.data)?)?;
    write_atlas_png(&artifacts.png, &baked.atlas)
}

fn write_atlas_png(path: &Path, alpha: &Gray) -> Result<()> {
    let file = File::create(path).with_context(|| format!("Cannot write {}", path.display()))?;
    let mut encoder = Encoder::new(
        BufWriter::new(file),
        alpha.width as u32,
        alpha.height as u32,
    );
    encoder.set_color(ColorType::Rgba);
    let rgba: Vec<u8> = alpha.data.iter().flat_map(|&a| [a, a, a, a]).collect();
    encoder.write_header()?.write_image_data(&rgba)?;
    Ok(())
}

pub fn bake(recipe: &Recipe, font: &[u8]) -> Result<Baked> {
    if recipe.unity_version.is_empty() {
        bail!(
            "{}: the recipe has no Unity version, scan the game again",
            recipe.font
        );
    }
    if !render::is_known(recipe.render_mode) {
        bail!(
            "{}: unknown render mode {}",
            recipe.font,
            recipe.render_mode
        );
    }
    let library = Library::try_new()?;
    library.set_interpreter_version(render::interpreter_for(&recipe.unity_version))?;
    let face = Face::try_new(&library, font)?;
    face.set_char_size(recipe.point_size * 64)?;
    let indices = map_codepoints(&face, &recipe.codepoints);
    let unique: Vec<u32> = indices
        .values()
        .copied()
        .collect::<IndexSet<u32>>()
        .into_iter()
        .collect();
    let style = Style {
        render_mode: recipe.render_mode,
        padding: recipe.padding,
        point_size: recipe.point_size,
    };
    let tiles = render::render_glyphs(&library, &face, &unique, &style)?;
    let atlas = pack_tiles(&tiles, recipe);
    let glyphs: Vec<&Tile> = tiles
        .values()
        .filter(|tile| atlas.spots.contains_key(&tile.index) || tile.pixels.is_empty())
        .collect();
    let placed: HashSet<u32> = glyphs.iter().map(|tile| tile.index).collect();
    let face_info = read_face_info(&face, recipe.point_size)?;
    let pairs = kerning::read_kerning(
        font,
        &placed,
        recipe.point_size as f64,
        &recipe.unity_version,
    )?;
    let data = json!({
        "atlas_width": atlas.width,
        "atlas_height": atlas.height,
        "atlas_padding": recipe.padding,
        "render_mode": recipe.render_mode,
        "face_info": face_info,
        "glyphs": glyphs.iter().map(|tile| describe_glyph(tile, atlas.spots.get(&tile.index))).collect::<Vec<_>>(),
        "characters": indices
            .iter()
            .filter(|(_, index)| placed.contains(index))
            .map(|(codepoint, index)| describe_character(*codepoint, *index))
            .collect::<Vec<_>>(),
        "font_feature_table": {"m_GlyphPairAdjustmentRecords": pairs.iter().map(describe_pair).collect::<Vec<_>>()},
        "not_baked": recipe
            .codepoints
            .iter()
            .filter(|cp| indices.get(*cp).is_none_or(|index| !placed.contains(index)))
            .collect::<Vec<_>>(),
    });
    Ok(Baked {
        data,
        atlas: draw_atlas(&tiles, &atlas, recipe.padding),
    })
}

fn map_codepoints(face: &Face, codepoints: &[u32]) -> IndexMap<u32, u32> {
    codepoints
        .iter()
        .map(|cp| (*cp, face.char_index(*cp)))
        .filter(|(_, index)| *index != 0)
        .collect()
}

fn pack_tiles(tiles: &IndexMap<u32, Tile>, recipe: &Recipe) -> Atlas {
    let mode = recipe.render_mode;
    let spacing = usize::from(render::is_distance_field(mode));
    let items = tiles
        .values()
        .filter(|tile| !tile.pixels.is_empty())
        .map(|tile| {
            let size = Size {
                width: tile.size.width + 2 * recipe.padding + spacing,
                height: tile.size.height + 2 * recipe.padding + spacing,
            };
            (tile.index, size)
        })
        .collect();
    let start = Size {
        width: recipe.atlas_width,
        height: recipe.atlas_height,
    };
    pack::pack_growing(&items, start, recipe.padding, spacing)
}

fn describe_glyph(tile: &Tile, spot: Option<&Spot>) -> Value {
    let Spot { x, y } = spot.copied().unwrap_or(Spot { x: 0, y: 0 });
    json!({
        "m_Index": tile.index,
        "m_Metrics": describe_metrics(tile.metrics),
        "m_GlyphRect": {"m_X": x, "m_Y": y, "m_Width": tile.size.width, "m_Height": tile.size.height},
        "m_Scale": 1.0,
        "m_AtlasIndex": 0,
        "m_ClassDefinitionType": 0,
    })
}

fn describe_metrics(metrics: Metrics) -> Value {
    json!({
        "m_Width": metrics.width,
        "m_Height": metrics.height,
        "m_HorizontalBearingX": metrics.bearing_x,
        "m_HorizontalBearingY": metrics.bearing_y,
        "m_HorizontalAdvance": metrics.advance,
    })
}

fn describe_character(codepoint: u32, index: u32) -> Value {
    json!({"m_ElementType": 1, "m_Unicode": codepoint, "m_GlyphIndex": index, "m_Scale": 1.0})
}

fn describe_pair(pair: &PairRecord) -> Value {
    let record = |index: u32, shift: Shift<f64>| {
        json!({
            "m_GlyphIndex": index,
            "m_GlyphValueRecord": {
                "m_XPlacement": shift.x_placement,
                "m_YPlacement": shift.y_placement,
                "m_XAdvance": shift.x_advance,
                "m_YAdvance": shift.y_advance,
            },
        })
    };
    json!({
        "m_FirstAdjustmentRecord": record(pair.first, pair.first_shift),
        "m_SecondAdjustmentRecord": record(pair.second, pair.second_shift),
        "m_FeatureLookupFlags": 0,
    })
}

/// Tiles are drawn bottom-up as TMP stores them; overlapping padding keeps the brighter pixel.
fn draw_atlas(tiles: &IndexMap<u32, Tile>, atlas: &Atlas, padding: usize) -> Gray {
    let (width, height) = (atlas.width, atlas.height);
    let mut flipped = vec![0u8; width * height];
    for (index, &Spot { x, y }) in &atlas.spots {
        let pixels = &tiles[index].pixels;
        for row in 0..pixels.height {
            let source = &pixels.data[(pixels.height - 1 - row) * pixels.width..][..pixels.width];
            let target_row = y - padding + row;
            if target_row >= height {
                continue;
            }
            for (col, value) in source.iter().enumerate() {
                let target_col = x - padding + col;
                if target_col < width {
                    let cell = &mut flipped[target_row * width + target_col];
                    *cell = (*cell).max(*value);
                }
            }
        }
    }
    let data = flipped
        .chunks(width.max(1))
        .rev()
        .flatten()
        .copied()
        .collect();
    Gray {
        width,
        height,
        data,
    }
}

pub fn read_font_face_info(font: &[u8], point_size: i64, unity_version: &str) -> Result<Value> {
    let library = Library::try_new()?;
    library.set_interpreter_version(render::interpreter_for(unity_version))?;
    let face = Face::try_new(&library, font)?;
    face.set_char_size(point_size * 64)?;
    read_face_info(&face, point_size)
}

fn read_face_info(face: &Face, point: i64) -> Result<Value> {
    let fields = face.fields();
    let scale = point as f64 / f64::from(fields.units_per_em);
    let (ascent, descent) = (
        f64::from(fields.ascender) * scale,
        f64::from(fields.descender) * scale,
    );
    let mean_line = read_bearing_y(face, 'x')?;
    let underline = f64::from(fields.underline_thickness) * scale;
    Ok(json!({
        "m_FaceIndex": 0,
        "m_FamilyName": fields.family_name,
        "m_StyleName": fields.style_name,
        "m_PointSize": point,
        "m_Scale": 1.0,
        "m_UnitsPerEM": fields.units_per_em,
        "m_LineHeight": f64::from(fields.height) * scale,
        "m_AscentLine": ascent,
        "m_CapLine": read_bearing_y(face, 'H')?,
        "m_MeanLine": mean_line,
        "m_Baseline": 0.0,
        "m_DescentLine": descent,
        "m_SuperscriptOffset": ascent,
        "m_SuperscriptSize": 0.5,
        "m_SubscriptOffset": descent,
        "m_SubscriptSize": 0.5,
        "m_UnderlineOffset": f64::from(fields.underline_position) * scale,
        "m_UnderlineThickness": underline,
        "m_StrikethroughOffset": mean_line * 0.4,
        "m_StrikethroughThickness": underline,
        "m_TabWidth": (read_advance(face, ' ')? + 0.5).floor(),
    }))
}

fn read_bearing_y(face: &Face, ch: char) -> Result<f64> {
    let index = face.char_index(u32::from(ch));
    if index == 0 {
        return Ok(0.0);
    }
    face.load_glyph(index, LOAD_DEFAULT)?;
    Ok((face.glyph().bearing_y as f64 / 64.0).round_ties_even())
}

fn read_advance(face: &Face, ch: char) -> Result<f64> {
    let index = face.char_index(u32::from(ch));
    if index == 0 {
        return Ok(0.0);
    }
    face.load_glyph(index, LOAD_NO_HINTING)?;
    Ok(face.glyph().advance as f64 / 64.0)
}
