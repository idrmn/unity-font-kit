//! Images are grey (one channel), top row first; Unity stores rows bottom-up.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::game::assets::{self, Source, int_field};
use crate::rebuild::render::Gray;

pub const ALPHA8: i64 = 1;
pub const RGBA32: i64 = 4;
pub const ARGB32: i64 = 5;
// Other formats (block-compressed) are written back as Alpha8, which TMP reads.
const WRITABLE_FORMATS: [i64; 3] = [ALPHA8, RGBA32, ARGB32];

pub fn read_alpha(texture: &Value, source: &Source) -> Result<Gray> {
    let (width, height) = (
        int_field(texture, "m_Width"),
        int_field(texture, "m_Height"),
    );
    if width == 0 || height == 0 {
        return Ok(Gray {
            width: 0,
            height: 0,
            data: Vec::new(),
        });
    }
    let data = read_image_data(texture, source)?;
    if data.is_empty() {
        bail!("Texture2D has no image data");
    }
    let format = int_field(texture, "m_TextureFormat");
    let (stride, channel) = match format {
        ALPHA8 => (1, 0),
        RGBA32 => (4, 3),
        ARGB32 => (4, 0),
        _ => bail!("Not implemented texture format: {format}"),
    };
    let (width, height) = (usize::try_from(width)?, usize::try_from(height)?);
    if data.len() < width * height * stride {
        bail!("not enough image data");
    }
    let rows: Vec<&[u8]> = data[..width * height * stride]
        .chunks(width * stride)
        .collect();
    let flipped = rows
        .iter()
        .rev()
        .flat_map(|row| row.iter().skip(channel).step_by(stride).copied())
        .collect();
    Ok(Gray {
        width,
        height,
        data: flipped,
    })
}

fn read_image_data(texture: &Value, source: &Source) -> Result<Vec<u8>> {
    let inline = assets::read_bytes(texture.get("image data"));
    if !inline.is_empty() {
        return Ok(inline);
    }
    let stream = texture.get("m_StreamData").context("No image data found")?;
    let path = stream.get("path").and_then(Value::as_str).unwrap_or("");
    let offset = stream.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let size = stream.get("size").and_then(Value::as_u64).unwrap_or(0);
    source.read_resource(path, offset, size)
}

pub fn write_alpha(texture: &Value, alpha: &Gray) -> Result<Value> {
    let format = int_field(texture, "m_TextureFormat");
    let target = if WRITABLE_FORMATS.contains(&format) {
        format
    } else {
        ALPHA8
    };
    // ARGB32 is stored as RGBA32; with all channels equal the bytes are the same.
    let (stored, channels) = match target {
        ALPHA8 => (ALPHA8, 1),
        _ => (RGBA32, 4),
    };
    let data: Vec<u8> = alpha
        .data
        .chunks(alpha.width.max(1))
        .rev()
        .flatten()
        .flat_map(|value| std::iter::repeat_n(*value, channels))
        .collect();
    let mut written = texture.clone();
    let fields = written
        .as_object_mut()
        .context("a Texture2D is not an object")?;
    fields.insert("m_Width".into(), json!(alpha.width));
    fields.insert("m_Height".into(), json!(alpha.height));
    if fields.contains_key("m_MipMap") {
        fields.insert("m_MipMap".into(), json!(false));
    }
    if fields.contains_key("m_MipCount") {
        fields.insert("m_MipCount".into(), json!(1));
    }
    fields.insert("m_CompleteImageSize".into(), json!(data.len()));
    fields.insert("image data".into(), assets::bytes_value(&data));
    fields.insert("m_TextureFormat".into(), json!(stored));
    if let Some(stream) = fields
        .get_mut("m_StreamData")
        .and_then(Value::as_object_mut)
    {
        stream.insert("path".into(), json!(""));
        stream.insert("offset".into(), json!(0));
        stream.insert("size".into(), json!(0));
    }
    Ok(written)
}

/// A PNG without alpha gives its grey value instead.
pub fn read_png_alpha(path: &Path) -> Result<Gray> {
    let file =
        std::fs::File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().context("PNG too large")?];
    let frame = reader.next_frame(&mut buffer)?;
    if frame.bit_depth != png::BitDepth::Eight {
        bail!("{}: only 8-bit PNG files are supported", path.display());
    }
    let pixels = &buffer[..frame.buffer_size()];
    let (width, height) = (frame.width as usize, frame.height as usize);
    let data = match frame.color_type {
        png::ColorType::Rgba => pixels.chunks(4).map(|p| p[3]).collect(),
        png::ColorType::GrayscaleAlpha => pixels.chunks(2).map(|p| p[1]).collect(),
        png::ColorType::Grayscale => pixels.to_vec(),
        png::ColorType::Rgb => pixels.chunks(3).map(to_luma).collect(),
        png::ColorType::Indexed => {
            let palette = reader.info().palette.as_deref().unwrap_or_default();
            pixels
                .iter()
                .map(|&i| palette.get(usize::from(i) * 3..usize::from(i) * 3 + 3))
                .map(|rgb| rgb.map_or(0, to_luma))
                .collect()
        }
    };
    Ok(Gray {
        width,
        height,
        data,
    })
}

/// ITU-R 601-2 luma in 16-bit fixed point.
fn to_luma(rgb: &[u8]) -> u8 {
    let [r, g, b] = [rgb[0], rgb[1], rgb[2]].map(u32::from);
    ((r * 19595 + g * 38470 + b * 7471 + 0x8000) >> 16) as u8
}
