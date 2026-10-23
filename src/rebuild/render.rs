//! Glyph tiles for a TMP atlas, drawn the way Unity's TMP draws them with FreeType.

use std::ops::RangeInclusive;

use anyhow::Result;
use indexmap::IndexMap;
use kiddo::{ImmutableKdTree, SquaredEuclidean};

use super::distance::{self, Mask};
use super::kerning;
use super::pack::Size;
use crate::freetype::{
    Face, Glyph, LOAD_DEFAULT, LOAD_NO_HINTING, LOAD_TARGET_MONO, Library, PIXEL_MODE_MONO,
    RENDER_MONO, RENDER_NORMAL, RENDER_SDF,
};

pub const SMOOTH_HINTED: u32 = 4121;
pub const SDFAA: u32 = 4165;

// Unity's render modes are bit sets (TextCore FontEngine GlyphRasterModes), and old TMP versions combine them freely.
const HINTED: u32 = 0x8;
const BITMAP: u32 = 0x10;
const SDF: u32 = 0x20;
const SDF_AA: u32 = 0x40;
const UNSUPPORTED: u32 = 0x80 | 0x100 | 0x200 | 0x10000;
const UPSAMPLING: [(u32, i64); 4] = [(0x1000, 1), (0x2000, 8), (0x4000, 16), (0x8000, 32)];
// FreeType rejects other bsdf spreads and silently keeps the previous one.
const BSDF_SPREADS: RangeInclusive<i32> = 2..=32;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Gray {
    fn zeros(width: usize, height: usize) -> Gray {
        Gray {
            width,
            height,
            data: vec![0; width * height],
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// Glyph metrics in pixels at the atlas point size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub width: f64,
    pub height: f64,
    pub bearing_x: f64,
    pub bearing_y: f64,
    pub advance: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub index: u32,
    pub metrics: Metrics,
    pub pixels: Gray,
    /// The glyph rect size in the atlas, without padding.
    pub size: Size,
}

#[derive(Debug)]
pub struct Style {
    pub render_mode: u32,
    pub padding: usize,
    pub point_size: i64,
}

#[must_use]
pub fn is_sdfaa(mode: u32) -> bool {
    mode & SDF_AA != 0
}

#[must_use]
pub fn upsampling(mode: u32) -> Option<i64> {
    if mode & SDF == 0 || is_sdfaa(mode) {
        return None;
    }
    UPSAMPLING
        .iter()
        .find(|(bit, _)| mode & bit != 0)
        .map(|(_, factor)| *factor)
}

#[must_use]
pub fn is_distance_field(mode: u32) -> bool {
    upsampling(mode).is_some() || is_sdfaa(mode)
}

fn is_raster(mode: u32) -> bool {
    mode & (BITMAP | SDF | SDF_AA) == 0
}

#[must_use]
pub fn is_known(mode: u32) -> bool {
    mode & UNSUPPORTED == 0 && UPSAMPLING.iter().any(|(bit, _)| mode & bit != 0)
}

fn load_flags(mode: u32) -> i32 {
    let flags = if mode & HINTED != 0 {
        LOAD_DEFAULT
    } else {
        LOAD_NO_HINTING
    };
    // SDFAA modes hint for the normal (grey) target; only raster and upsampled SDF modes use mono.
    if is_raster(mode) || upsampling(mode).is_some() {
        return flags | LOAD_TARGET_MONO;
    }
    flags
}

/// Measured: Unity 2020.3 hints like FreeType's TrueType interpreter 35, Unity 6000.3 like 40.
#[must_use]
pub fn interpreter_for(unity_version: &str) -> u32 {
    if kerning::is_before_unity6(unity_version) {
        35
    } else {
        40
    }
}

pub fn render_glyphs(
    library: &Library,
    face: &Face,
    indices: &[u32],
    style: &Style,
) -> Result<IndexMap<u32, Tile>> {
    face.set_char_size(style.point_size * 64)?;
    let mut tiles = IndexMap::new();
    for &index in indices {
        // One glyph FreeType cannot draw must not fail the whole bake.
        if let Ok(tile) = render_glyph(library, face, index, style) {
            tiles.insert(index, tile);
        }
    }
    Ok(tiles)
}

fn render_glyph(library: &Library, face: &Face, index: u32, style: &Style) -> Result<Tile> {
    if let Some(factor) = upsampling(style.render_mode) {
        return render_upsampled_sdf(face, index, style, factor);
    }
    face.load_glyph(index, load_flags(style.render_mode))?;
    let metrics = read_metrics(&face.glyph(), 1);
    if is_sdfaa(style.render_mode) {
        return render_bitmap_sdf(library, face, index, metrics, style.padding);
    }
    let mode = if is_raster(style.render_mode) {
        RENDER_MONO
    } else {
        RENDER_NORMAL
    };
    face.render(mode)?;
    let bitmap = read_bitmap(&face.glyph());
    let size = Size {
        width: bitmap.width,
        height: bitmap.height,
    };
    let pixels = if bitmap.is_empty() {
        bitmap
    } else {
        pad(&bitmap, style.padding)
    };
    Ok(Tile {
        index,
        metrics,
        pixels,
        size,
    })
}

fn read_metrics(glyph: &Glyph, upsampling: i64) -> Metrics {
    let scale = (64 * upsampling) as f64;
    Metrics {
        width: glyph.width as f64 / scale,
        height: glyph.height as f64 / scale,
        bearing_x: glyph.bearing_x as f64 / scale,
        bearing_y: glyph.bearing_y as f64 / scale,
        advance: glyph.advance as f64 / scale,
    }
}

fn read_bitmap(glyph: &Glyph) -> Gray {
    let (rows, columns) = (glyph.rows as usize, glyph.columns as usize);
    if rows == 0 || columns == 0 {
        return Gray::default();
    }
    let pitch = glyph.pitch.unsigned_abs() as usize;
    let mono = glyph.pixel_mode == PIXEL_MODE_MONO;
    let mut data = Vec::with_capacity(rows * columns);
    for row in 0..rows {
        let line = &glyph.buffer[row * pitch..(row + 1) * pitch];
        for col in 0..columns {
            data.push(if mono {
                mono_pixel(line, col)
            } else {
                line[col]
            });
        }
    }
    Gray {
        width: columns,
        height: rows,
        data,
    }
}

fn mono_pixel(line: &[u8], col: usize) -> u8 {
    if line[col / 8] & (0x80 >> (col % 8)) != 0 {
        255
    } else {
        0
    }
}

fn pad(image: &Gray, padding: usize) -> Gray {
    let mut out = Gray::zeros(image.width + 2 * padding, image.height + 2 * padding);
    paste(&mut out, image, padding as i64, padding as i64);
    out
}

fn paste(canvas: &mut Gray, image: &Gray, x0: i64, y0: i64) {
    for row in 0..image.height {
        let y = row as i64 + y0;
        if y < 0 || y >= canvas.height as i64 {
            continue;
        }
        for col in 0..image.width {
            let x = col as i64 + x0;
            if x >= 0 && x < canvas.width as i64 {
                canvas.data[y as usize * canvas.width + x as usize] =
                    image.data[row * image.width + col];
            }
        }
    }
}

/// FreeType's bsdf turns the anti-aliased raster into a distance field, as TMP's SDFAA modes do.
fn render_bitmap_sdf(
    library: &Library,
    face: &Face,
    index: u32,
    metrics: Metrics,
    padding: usize,
) -> Result<Tile> {
    face.render(RENDER_NORMAL)?;
    let before = face.glyph();
    let plain = read_bitmap(&before);
    if plain.is_empty() {
        return Ok(Tile {
            index,
            metrics,
            pixels: plain,
            size: Size {
                width: 0,
                height: 0,
            },
        });
    }
    let size = Size {
        width: plain.width,
        height: plain.height,
    };
    let spread = i32::try_from(padding)? + 1;
    if !BSDF_SPREADS.contains(&spread) {
        return Ok(Tile {
            index,
            metrics,
            pixels: measure_distance(&plain, padding),
            size,
        });
    }
    library.set_sdf_spread(spread)?;
    face.render(RENDER_SDF)?;
    let after = face.glyph();
    let field = read_bitmap(&after);
    let mut tile = Gray::zeros(plain.width + 2 * padding, plain.height + 2 * padding);
    let x0 = i64::from(after.bitmap_left) - i64::from(before.bitmap_left) + padding as i64;
    let y0 = i64::from(before.bitmap_top) - i64::from(after.bitmap_top) + padding as i64;
    paste(&mut tile, &field, x0, y0);
    Ok(Tile {
        index,
        metrics,
        pixels: tile,
        size,
    })
}

/// The fallback for spreads bsdf rejects: an exact distance field from the thresholded raster.
fn measure_distance(plain: &Gray, padding: usize) -> Gray {
    let (width, height) = (plain.width + 2 * padding, plain.height + 2 * padding);
    let mut inside = Mask::new(width, height);
    for row in 0..plain.height {
        for col in 0..plain.width {
            inside.set(
                row + padding,
                col + padding,
                plain.data[row * plain.width + col] > 127,
            );
        }
    }
    let to_outside = distance::edt_squared(&inside.inverted());
    let to_inside = distance::edt_squared(&inside);
    let spread = (2 * (padding + 1)) as f64;
    let data = (0..width * height)
        .map(|cell| {
            let d = if inside.cells[cell] {
                distance::root(to_outside[cell]) - 0.5
            } else {
                0.5 - distance::root(to_inside[cell])
            };
            to_byte(127.5 + d * 255.0 / spread)
        })
        .collect();
    Gray {
        width,
        height,
        data,
    }
}

fn to_byte(value: f64) -> u8 {
    value.clamp(0.0, 255.0).round_ties_even() as u8
}

/// A monochrome raster at N times the size gives the exact edge, as TMP's SDF8/16/32 modes do.
fn render_upsampled_sdf(face: &Face, index: u32, style: &Style, upsampling: i64) -> Result<Tile> {
    face.set_char_size(style.point_size * upsampling * 64)?;
    let drawn = draw_mono(face, index, upsampling);
    face.set_char_size(style.point_size * 64)?;
    let raster = drawn?;
    let metrics = raster.metrics;
    if !raster.inside.any() {
        return Ok(Tile {
            index,
            metrics,
            pixels: Gray::default(),
            size: Size {
                width: 0,
                height: 0,
            },
        });
    }
    // TMP sizes the glyph rect by the raster, not by the outline metrics (they differ on 4 of 364 glyphs).
    let factor = upsampling as usize;
    let size = Size {
        width: raster.inside.width.div_ceil(factor),
        height: raster.inside.height.div_ceil(factor),
    };
    let pixels = sample_distance(&raster, size, style.padding, upsampling);
    Ok(Tile {
        index,
        metrics,
        pixels,
        size,
    })
}

struct MonoRaster {
    metrics: Metrics,
    inside: Mask,
    left: i64,
    top: i64,
}

fn draw_mono(face: &Face, index: u32, upsampling: i64) -> Result<MonoRaster> {
    face.load_glyph(index, LOAD_NO_HINTING | LOAD_TARGET_MONO)?;
    let metrics = read_metrics(&face.glyph(), upsampling);
    face.render(RENDER_MONO)?;
    let glyph = face.glyph();
    let bitmap = read_bitmap(&glyph);
    let inside = Mask {
        width: bitmap.width,
        height: bitmap.height,
        cells: bitmap.data.iter().map(|value| *value > 127).collect(),
    };
    Ok(MonoRaster {
        metrics,
        inside,
        left: i64::from(glyph.bitmap_left),
        top: i64::from(glyph.bitmap_top),
    })
}

fn sample_distance(raster: &MonoRaster, size: Size, padding: usize, upsampling: i64) -> Gray {
    let MonoRaster {
        metrics,
        inside,
        left,
        top,
    } = raster;
    let spread = (padding + 1) as f64;
    let (width, height) = (size.width + 2 * padding, size.height + 2 * padding);
    let pad = padding as i64;
    let factor = upsampling as f64;
    let cols: Vec<i64> = (0..width as i64)
        .map(|i| {
            ((((i - pad) as f64 + 0.5) + metrics.bearing_x) * factor - *left as f64).floor() as i64
        })
        .collect();
    let rows: Vec<i64> = (0..height as i64)
        .map(|j| {
            (*top as f64 - (metrics.bearing_y - ((j - pad) as f64 + 0.5)) * factor).floor() as i64
        })
        .collect();
    let (outer, inner) = list_edges(inside);
    let field = EdgeField::new(&outer, &inner);
    let mut data = Vec::with_capacity(width * height);
    for &row in &rows {
        for &col in &cols {
            let d = if is_inside(inside, row, col) {
                field.to_outer(row, col) - 0.5
            } else {
                0.5 - field.to_inner(row, col)
            };
            let sampled = d / factor;
            data.push(to_byte(127.5 + sampled * 255.0 / (2.0 * spread)));
        }
    }
    Gray {
        width,
        height,
        data,
    }
}

fn is_inside(mask: &Mask, row: i64, col: i64) -> bool {
    row >= 0
        && col >= 0
        && (row as usize) < mask.height
        && (col as usize) < mask.width
        && mask.get(row as usize, col as usize)
}

/// Row and column in raster coordinates; the frame around the raster is -1 and the size.
type Pixel = (i64, i64);

/// Edge pixels in raster coordinates (a one-pixel frame around the raster included):
/// outside pixels that touch the glyph, and glyph pixels that touch the outside.
/// The nearest pixel of the other kind always lies on the edge, so only edge pixels are searched.
fn list_edges(inside: &Mask) -> (Vec<Pixel>, Vec<Pixel>) {
    let (mut outer, mut inner) = (Vec::new(), Vec::new());
    for row in -1..=inside.height as i64 {
        for col in -1..=inside.width as i64 {
            let here = is_inside(inside, row, col);
            let neighbours = [
                (row - 1, col),
                (row + 1, col),
                (row, col - 1),
                (row, col + 1),
            ];
            let touches = |kind: bool| {
                neighbours
                    .iter()
                    .any(|&(r, c)| is_inside(inside, r, c) == kind && in_frame(inside, r, c))
            };
            if !here && touches(true) {
                outer.push((row, col));
            }
            if here && touches(false) {
                inner.push((row, col));
            }
        }
    }
    (outer, inner)
}

fn in_frame(mask: &Mask, row: i64, col: i64) -> bool {
    row >= -1 && col >= -1 && row <= mask.height as i64 && col <= mask.width as i64
}

/// Coordinates and squared distances are small integers, so f64 holds them exactly.
struct EdgeField {
    outer: Option<ImmutableKdTree<f64, 2>>,
    inner: Option<ImmutableKdTree<f64, 2>>,
}

impl EdgeField {
    fn new(outer: &[Pixel], inner: &[Pixel]) -> EdgeField {
        EdgeField {
            outer: build_tree(outer),
            inner: build_tree(inner),
        }
    }

    fn to_outer(&self, row: i64, col: i64) -> f64 {
        nearest(self.outer.as_ref(), row, col)
    }

    fn to_inner(&self, row: i64, col: i64) -> f64 {
        nearest(self.inner.as_ref(), row, col)
    }
}

fn build_tree(points: &[Pixel]) -> Option<ImmutableKdTree<f64, 2>> {
    let coords: Vec<[f64; 2]> = points
        .iter()
        .map(|&(row, col)| [row as f64, col as f64])
        .collect();
    (!coords.is_empty()).then(|| ImmutableKdTree::new_from_slice(&coords))
}

fn nearest(tree: Option<&ImmutableKdTree<f64, 2>>, row: i64, col: i64) -> f64 {
    tree.map_or(f64::INFINITY, |tree| {
        tree.nearest_one::<SquaredEuclidean>(&[row as f64, col as f64])
            .distance
            .sqrt()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_modes_are_read_by_their_bits() {
        let read = |mode: u32| {
            (
                upsampling(mode),
                is_sdfaa(mode),
                is_raster(mode),
                load_flags(mode) & LOAD_NO_HINTING == 0,
            )
        };
        assert_eq!(read(16422), (Some(16), false, false, false));
        assert_eq!(read(16426), (Some(16), false, false, true));
        assert_eq!(read(32810), (Some(32), false, false, true));
        assert_eq!(read(SDFAA), (None, true, false, false));
        assert_eq!(read(4105), (None, false, true, true));
        assert!(is_known(32810) && !is_known(0x10000 | SDFAA));
    }
}
