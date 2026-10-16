
use std::io::Cursor;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

use crate::read_ext::{ReadUrexExt, invalid_data};
use crate::write_ext::WriteExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBundleHeader {
    pub signature: String,
    pub version: u32,
    pub player_version: String,
    pub engine_version: String,
    pub hash_and_crc: Option<[u8; 20]>,
    pub levels_before_streaming: u32,
}

pub struct RawBundle {
    pub header: RawBundleHeader,
    pub files: Vec<(String, Vec<u8>)>,
}

pub fn is_raw_bundle(bytes: &[u8]) -> bool {
    bytes.starts_with(b"UnityRaw\0") || bytes.starts_with(b"UnityWeb\0")
}

fn is_web(header: &RawBundleHeader) -> bool {
    header.signature == "UnityWeb"
}

pub fn read_raw_bundle(bytes: &[u8]) -> Result<RawBundle, std::io::Error> {
    let mut reader = Cursor::new(bytes);
    let header = read_header(&mut reader)?;
    reader.read_u32::<BigEndian>()?;
    let header_size = reader.read_u32::<BigEndian>()? as usize;
    let levels_before_streaming = reader.read_u32::<BigEndian>()?;
    let level_count = reader.read_i32::<BigEndian>()?.max(1) as u64;
    reader.set_position(reader.position() + 8 * (level_count - 1));
    let compressed = reader.read_u32::<BigEndian>()? as usize;
    let uncompressed = reader.read_u32::<BigEndian>()? as usize;
    let header = RawBundleHeader {
        levels_before_streaming,
        ..header
    };
    let packed = bytes
        .get(header_size..header_size + compressed)
        .ok_or_else(|| invalid_data("bundle data is outside the file"))?;
    let content = unpack(&header, packed, uncompressed)?;
    let files = read_directory(&content)?;
    Ok(RawBundle { header, files })
}

fn read_header(reader: &mut Cursor<&[u8]>) -> Result<RawBundleHeader, std::io::Error> {
    let signature = reader.read_cstr()?;
    let version = reader.read_u32::<BigEndian>()?;
    let player_version = reader.read_cstr()?;
    let engine_version = reader.read_cstr()?;
    let hash_and_crc = match version >= 4 {
        true => Some(reader.read_bytes_array::<20>()?),
        false => None,
    };
    Ok(RawBundleHeader {
        signature,
        version,
        player_version,
        engine_version,
        hash_and_crc,
        levels_before_streaming: 0,
    })
}

fn unpack(header: &RawBundleHeader, packed: &[u8], size: usize) -> Result<Vec<u8>, std::io::Error> {
    if !is_web(header) {
        return Ok(packed.to_vec());
    }
    let mut content = Vec::with_capacity(size);
    lzma_rs::lzma_decompress_with_options(
        &mut &packed[..],
        &mut content,
        &lzma_rs::decompress::Options {
            unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(Some(size as u64)),
            ..Default::default()
        },
    )
    .map_err(invalid_data)?;
    Ok(content)
}

fn read_directory(content: &[u8]) -> Result<Vec<(String, Vec<u8>)>, std::io::Error> {
    let mut reader = Cursor::new(content);
    let count = reader.read_i32::<BigEndian>()?;
    (0..count)
        .map(|_| {
            let name = reader.read_cstr()?;
            let offset = reader.read_u32::<BigEndian>()? as usize;
            let size = reader.read_u32::<BigEndian>()? as usize;
            let data = content
                .get(offset..offset + size)
                .ok_or_else(|| invalid_data("bundle entry is outside the bundle"))?;
            Ok((name, data.to_vec()))
        })
        .collect()
}

/// One level, the directory padded to 4, the files right after it, as Unity writes these bundles.
pub fn write_raw_bundle(
    header: &RawBundleHeader,
    files: &[(String, Vec<u8>)],
) -> Result<Vec<u8>, std::io::Error> {
    let content = write_content(files)?;
    let packed = pack(header, &content)?;
    let mut out = Vec::new();
    out.write_cstr(&header.signature)?;
    out.write_u32::<BigEndian>(header.version)?;
    out.write_cstr(&header.player_version)?;
    out.write_cstr(&header.engine_version)?;
    if let Some(hash_and_crc) = &header.hash_and_crc {
        out.extend_from_slice(hash_and_crc);
    }
    let fields = 4 * 4 + 8 + 4 * usize::from(header.version >= 2) + 4 * usize::from(header.version >= 3);
    let header_size = align4(out.len() + fields);
    let complete = (header_size + packed.len()) as u32;
    out.write_u32::<BigEndian>(complete)?;
    out.write_u32::<BigEndian>(header_size as u32)?;
    out.write_u32::<BigEndian>(header.levels_before_streaming)?;
    out.write_i32::<BigEndian>(1)?;
    out.write_u32::<BigEndian>(packed.len() as u32)?;
    out.write_u32::<BigEndian>(content.len() as u32)?;
    if header.version >= 2 {
        out.write_u32::<BigEndian>(complete)?;
    }
    if header.version >= 3 {
        out.write_u32::<BigEndian>(directory_size(files) as u32)?;
    }
    out.resize(header_size, 0);
    out.extend_from_slice(&packed);
    Ok(out)
}

fn directory_size(files: &[(String, Vec<u8>)]) -> usize {
    align4(4 + files.iter().map(|(name, _)| name.len() + 1 + 8).sum::<usize>())
}

fn write_content(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, std::io::Error> {
    let start = directory_size(files);
    let mut content = Vec::new();
    content.write_i32::<BigEndian>(files.len() as i32)?;
    let mut offset = start;
    for (name, data) in files {
        content.write_cstr(name)?;
        content.write_u32::<BigEndian>(offset as u32)?;
        content.write_u32::<BigEndian>(data.len() as u32)?;
        offset += data.len();
    }
    content.resize(start, 0);
    for (_, data) in files {
        content.extend_from_slice(data);
    }
    // Unity pads the content to whole 4-byte words.
    content.resize(align4(content.len()), 0);
    Ok(content)
}

fn pack(header: &RawBundleHeader, content: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    if !is_web(header) {
        return Ok(content.to_vec());
    }
    let mut packed = Vec::new();
    lzma_rs::lzma_compress_with_options(
        &mut &content[..],
        &mut packed,
        &lzma_rs::compress::Options {
            unpacked_size: lzma_rs::compress::UnpackedSize::SkipWritingToHeader,
        },
    )?;
    Ok(packed)
}

fn align4(size: usize) -> usize {
    size.div_ceil(4) * 4
}
