use std::collections::HashSet;

use anyhow::{Result, bail};
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

use crate::game::assets;

pub const JSON_CATALOG: &str = "StreamingAssets/aa/catalog.json";
pub const BINARY_CATALOG: &str = "StreamingAssets/aa/catalog.bin";
pub const SETTINGS: &str = "StreamingAssets/aa/settings.json";
pub const BUNDLES: &str = "StreamingAssets/aa/";
const RUNTIME_PATH: &str = "{UnityEngine.AddressableAssets.Addressables.RuntimePath}/";
/// Install changes catalogs before bundles and restore after them, so the game never checks a changed bundle's CRC.
#[must_use]
pub fn is_catalog_file(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file).to_lowercase();
    name.starts_with("catalog") && (name.ends_with(".json") || name.ends_with(".bin"))
}

const CATALOG_PROVIDER: &str = "ContentCatalogProvider";
const NONE: u32 = 0xFFFF_FFFF;
const BUNDLE_OPTIONS: &str = "AssetBundleRequestOptions";
// catalog.bin keeps the CRC as the third uint32 of each bundle options record.
const CRC_FIELD: i64 = 8;
// Bundle options are JSON stored as UTF-16 inside the base64 field m_ExtraDataString.
const CRC_KEY: &[u8] = b"\"\0m\0_\0C\0r\0c\0\"\0:\0";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalogs {
    pub local: Vec<String>,
    pub elsewhere: Vec<String>,
}

pub fn list_catalogs(settings: &str) -> Result<Catalogs> {
    let parsed: Value = serde_json::from_str(settings)?;
    let Value::Object(fields) = parsed else {
        bail!("{SETTINGS} is not a JSON object");
    };
    let ids = read_catalog_ids(fields.get("m_CatalogLocations"))?;
    Ok(Catalogs {
        local: ids
            .iter()
            .filter_map(|id| id.strip_prefix(RUNTIME_PATH))
            .map(|id| format!("{BUNDLES}{id}"))
            .collect(),
        elsewhere: ids
            .iter()
            .filter(|id| !id.starts_with(RUNTIME_PATH))
            .cloned()
            .collect(),
    })
}

fn read_catalog_ids(locations: Option<&Value>) -> Result<Vec<String>> {
    let locations = match locations {
        None => return Ok(Vec::new()),
        Some(value) if !assets::is_truthy(value) => return Ok(Vec::new()),
        Some(Value::Array(locations)) => locations,
        Some(_) => bail!("{SETTINGS}: m_CatalogLocations is not a list"),
    };
    let mut ids = Vec::new();
    for location in locations {
        let Value::Object(fields) = location else {
            bail!("{SETTINGS}: a catalog location is not an object");
        };
        if !read_text(fields.get("m_Provider"))?.contains(CATALOG_PROVIDER) {
            continue;
        }
        ids.push(read_text(fields.get("m_InternalId"))?.to_owned());
    }
    Ok(ids)
}

fn read_text(value: Option<&Value>) -> Result<&str> {
    match value {
        None => Ok(""),
        Some(Value::String(text)) => Ok(text),
        Some(_) => bail!("{SETTINGS}: a catalog location field is not text"),
    }
}

#[must_use]
pub fn touches_bundles(files: &[String]) -> bool {
    files.iter().any(|file| file.starts_with(BUNDLES))
}

pub fn clear_crcs(catalog: &str) -> Result<String> {
    use base64::Engine;
    let extra = read_extra_data(catalog)?;
    if !catalog.contains(&extra) {
        bail!("{JSON_CATALOG} is written in a way ufk cannot update");
    }
    let raw = STANDARD.decode(&extra)?;
    // Addressables skips the check for a CRC of 0; spaces keep every length, so nothing else moves.
    let cleared = STANDARD.encode(replace_crcs(&raw));
    Ok(catalog.replace(&extra, &cleared))
}

fn read_extra_data(catalog: &str) -> Result<String> {
    let parsed: Value = serde_json::from_str(catalog)?;
    match parsed.get("m_ExtraDataString") {
        Some(Value::String(extra)) => Ok(extra.clone()),
        _ => bail!("{JSON_CATALOG} has no m_ExtraDataString text"),
    }
}

fn replace_crcs(raw: &[u8]) -> Vec<u8> {
    let mut cleared = Vec::with_capacity(raw.len());
    let mut at = 0;
    while at < raw.len() {
        let digits = count_crc_digits(&raw[at..]);
        if digits == 0 {
            cleared.push(raw[at]);
            at += 1;
            continue;
        }
        cleared.extend_from_slice(CRC_KEY);
        cleared.extend(pad_zero(digits));
        at += CRC_KEY.len() + 2 * digits;
    }
    cleared
}

fn count_crc_digits(raw: &[u8]) -> usize {
    let Some(rest) = raw.strip_prefix(CRC_KEY) else {
        return 0;
    };
    rest.as_chunks::<2>()
        .0
        .iter()
        .take_while(|pair| pair[0].is_ascii_digit() && pair[1] == 0)
        .count()
}

fn pad_zero(digits: usize) -> Vec<u8> {
    std::iter::once(b'0')
        .chain(std::iter::repeat_n(b' ', digits - 1))
        .flat_map(|letter| [letter, 0])
        .collect()
}

pub fn clear_binary_crcs(data: &[u8]) -> Result<Vec<u8>> {
    // The binary layout follows nesrak1/AddressablesTools.
    let version = read_u32(data, 4)?;
    if !(1..=3).contains(&version) {
        bail!("{BINARY_CATALOG} has version {version}, ufk reads versions 1 to 3");
    }
    let mut cleared = data.to_vec();
    for offset in find_crc_offsets(data, read_u32(data, 8)?, version)? {
        cleared[offset..offset + 4].fill(0);
    }
    Ok(cleared)
}

fn find_crc_offsets(data: &[u8], keys: u32, version: u32) -> Result<HashSet<usize>> {
    let pairs = read_array(data, keys)?;
    let mut stack = Vec::new();
    for &listing in pairs.iter().skip(1).step_by(2) {
        stack.extend(read_array(data, listing)?);
    }
    let mut seen = HashSet::new();
    let mut found = HashSet::new();
    while let Some(location) = stack.pop() {
        if !seen.insert(location) {
            continue;
        }
        let location = i64::from(location);
        stack.extend(read_array(data, read_u32(data, location + 12)?)?);
        if let Some(crc) = find_crc(data, read_u32(data, location + 20)?, version)? {
            found.insert(crc);
        }
    }
    Ok(found)
}

fn find_crc(data: &[u8], data_offset: u32, version: u32) -> Result<Option<usize>> {
    if data_offset == NONE {
        return Ok(None);
    }
    let data_offset = i64::from(data_offset);
    let type_offset = read_u32(data, data_offset)?;
    let object_offset = read_u32(data, data_offset + 4)?;
    if object_offset == NONE {
        return Ok(None);
    }
    let class_name = read_string(
        data,
        read_u32(data, i64::from(type_offset) + 4)?,
        ".",
        version,
    )?;
    if !class_name.ends_with(BUNDLE_OPTIONS) {
        return Ok(None);
    }
    let crc = i64::from(object_offset) + CRC_FIELD;
    read_u32(data, crc)?;
    Ok(usize::try_from(crc).ok())
}

fn read_u32(data: &[u8], offset: i64) -> Result<u32> {
    let bytes = usize::try_from(offset)
        .ok()
        .and_then(|start| data.get(start..start.checked_add(4)?));
    match bytes.and_then(|bytes| <[u8; 4]>::try_from(bytes).ok()) {
        Some(bytes) => Ok(u32::from_le_bytes(bytes)),
        None => bail!("{BINARY_CATALOG} is damaged or written in a way ufk cannot read"),
    }
}

fn read_array(data: &[u8], encoded: u32) -> Result<Vec<u32>> {
    if encoded == NONE {
        return Ok(Vec::new());
    }
    let start = i64::from(encoded);
    let size = read_u32(data, start - 4)?;
    if size % 4 != 0 {
        bail!("{BINARY_CATALOG} is damaged or written in a way ufk cannot read");
    }
    (0..i64::from(size / 4))
        .map(|index| read_u32(data, start + 4 * index))
        .collect()
}

fn read_string(data: &[u8], encoded: u32, separator: &str, version: u32) -> Result<String> {
    if encoded == NONE {
        return Ok(String::new());
    }
    let mut offset = encoded & 0x3FFF_FFFF;
    if encoded & 0x4000_0000 == 0 {
        let length = read_u32(data, i64::from(offset) - 4)?;
        let text = slice_clamped(data, offset, length);
        return Ok(if encoded & 0x8000_0000 != 0 {
            decode_utf16_lossy(text)
        } else {
            decode_ascii_lossy(text)
        });
    }
    let mut parts = Vec::new();
    let mut seen = HashSet::new();
    while offset != NONE && seen.insert(offset) {
        let part = read_u32(data, i64::from(offset))? & !0x4000_0000;
        parts.push(read_string(data, part, separator, version)?);
        offset = read_u32(data, i64::from(offset) + 4)?;
    }
    // Versions after 1 store the parts of a dotted name from the last one.
    if version > 1 {
        parts.reverse();
    }
    Ok(parts.join(separator))
}

fn slice_clamped(data: &[u8], offset: u32, length: u32) -> &[u8] {
    let start = (offset as usize).min(data.len());
    let end = (offset as usize)
        .saturating_add(length as usize)
        .min(data.len());
    &data[start..end.max(start)]
}

fn decode_ascii_lossy(text: &[u8]) -> String {
    text.iter()
        .map(|&byte| {
            if byte.is_ascii() {
                char::from(byte)
            } else {
                '\u{FFFD}'
            }
        })
        .collect()
}

fn decode_utf16_lossy(text: &[u8]) -> String {
    let units: Vec<u16> = text
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let mut decoded = String::from_utf16_lossy(&units);
    if text.len() % 2 == 1 {
        decoded.push('\u{FFFD}');
    }
    decoded
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use serde_json::json;

    use super::*;

    fn pack(values: &[u32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn bundle_crcs_become_zero_and_nothing_else_moves() {
        let header = b"\x07\x00head".to_vec();
        let options = r#"{"m_Hash":"ab","m_Crc":2108122294,"m_Timeout":0}"#;
        let raw = [header.clone(), utf16(options)].concat();
        let catalog = json!({"m_LocatorId": "Main", "m_ExtraDataString": STANDARD.encode(&raw)});
        let cleared = clear_crcs(&catalog.to_string()).unwrap();
        let extra = serde_json::from_str::<Value>(&cleared).unwrap()["m_ExtraDataString"]
            .as_str()
            .unwrap()
            .to_owned();
        let cleared = STANDARD.decode(&extra).unwrap();
        assert_eq!(cleared[..header.len()], header);
        assert_eq!(
            cleared[header.len()..],
            utf16(&options.replace("2108122294", "0         "))
        );
    }

    #[test]
    fn binary_catalog_from_unity_6_gets_its_bundle_crc_zeroed_in_place() {
        let data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/catalog.bin"
        ))
        .unwrap();
        let cleared = clear_binary_crcs(&data).unwrap();
        let changed: Vec<usize> = (0..data.len()).filter(|&i| data[i] != cleared[i]).collect();
        assert_eq!(cleared.len(), data.len());
        assert!(!changed.is_empty() && changed[0] >= 1103 && changed[changed.len() - 1] < 1107);
        assert_eq!(cleared[1103..1107], [0; 4]);
    }

    #[test]
    fn catalogs_are_found_where_the_game_settings_point() {
        let provider = "UnityEngine.AddressableAssets.ResourceProviders.ContentCatalogProvider";
        let runtime = "{UnityEngine.AddressableAssets.Addressables.RuntimePath}";
        let location = |id: String| json!({"m_InternalId": id, "m_Provider": provider});
        let settings = json!({"m_CatalogLocations": [
            location(format!("{runtime}/catalog.bin")),
            location(format!("{runtime}/catalog_2024.01.01.json")),
            location("https://cdn.example.com/catalog_1.json".to_owned()),
        ]});
        let found = list_catalogs(&settings.to_string()).unwrap();
        assert_eq!(
            found.local,
            [
                "StreamingAssets/aa/catalog.bin",
                "StreamingAssets/aa/catalog_2024.01.01.json"
            ]
        );
        assert_eq!(found.elsewhere, ["https://cdn.example.com/catalog_1.json"]);
    }

    #[test]
    fn a_record_pointing_past_the_end_is_an_error_not_a_longer_file() {
        let name = b"AssetBundleRequestOptions";
        let data = [
            pack(&[8, 0xFFF0, NONE, 20, name.len() as u32]),
            name.to_vec(),
        ]
        .concat();
        let error = find_crc(&data, 0, 2).unwrap_err();
        assert!(error.to_string().contains("damaged"));
    }

    #[test]
    fn an_array_whose_size_is_not_whole_numbers_is_an_error() {
        let error = read_array(&pack(&[3, 0]), 4).unwrap_err();
        assert!(error.to_string().contains("damaged"));
    }

    struct CatalogWriter {
        data: Vec<u8>,
    }

    impl CatalogWriter {
        fn new() -> Self {
            Self {
                data: pack(&[0x0DE3_8942, 0, 0, 0, 0, 0, 0, 0]),
            }
        }

        fn put(&mut self, raw: &[u8]) -> u32 {
            let offset = self.data.len() as u32;
            self.data.extend_from_slice(raw);
            offset
        }

        fn set(&mut self, at: u32, values: &[u32]) {
            let at = at as usize;
            let raw = pack(values);
            self.data[at..at + raw.len()].copy_from_slice(&raw);
        }

        fn array(&mut self, items: &[u32]) -> u32 {
            let raw = pack(&[&[4 * items.len() as u32], items].concat());
            self.put(&raw) + 4
        }

        fn text(&mut self, value: &str) -> u32 {
            let raw = [pack(&[value.len() as u32]), value.as_bytes().to_vec()].concat();
            self.put(&raw) + 4
        }

        fn dotted(&mut self, parts: &[&str]) -> u32 {
            let nodes: Vec<u32> = parts
                .iter()
                .map(|part| {
                    let text = self.text(part);
                    self.put(&pack(&[text, NONE]))
                })
                .collect();
            for pair in nodes.windows(2) {
                self.set(pair[0] + 4, &[pair[1]]);
            }
            nodes[0] | 0x4000_0000
        }

        fn location(&mut self, data: u32, dependencies: &[u32]) -> u32 {
            let deps = if dependencies.is_empty() {
                NONE
            } else {
                self.array(dependencies)
            };
            self.put(&pack(&[NONE, NONE, NONE, deps, 0, data, NONE]))
        }

        fn data_object(&mut self, class_name: u32, record: u32) -> u32 {
            let class = self.put(&pack(&[NONE, class_name]));
            self.put(&pack(&[class, record]))
        }
    }

    #[test]
    fn binary_catalog_versions_reach_bundles_behind_dependencies() {
        for version in 1..=3 {
            let mut writer = CatalogWriter::new();
            let parts = ["Ns", "AssetBundleRequestOptions"];
            let ordered: Vec<&str> = if version == 1 {
                parts.to_vec()
            } else {
                parts.iter().rev().copied().collect()
            };
            let class_name = writer.dotted(&ordered);
            let record = writer.put(&pack(&[0, 0, 1234, 99, 0]));
            let bundle_data = writer.data_object(class_name, record);
            let bundle = writer.location(bundle_data, &[]);
            let empty_data = writer.data_object(class_name, NONE);
            let empty = writer.location(empty_data, &[]);
            let asset = writer.location(NONE, &[bundle, empty]);
            let key = writer.text("key");
            let listing = writer.array(&[asset]);
            let keys = writer.array(&[key, listing]);
            writer.set(4, &[version, keys]);
            let cleared = clear_binary_crcs(&writer.data).unwrap();
            let at = record as usize + 8;
            assert_eq!(cleared[at..at + 4], [0; 4]);
            assert_eq!(cleared.len(), writer.data.len());
            assert_eq!(
                cleared
                    .iter()
                    .zip(&writer.data)
                    .filter(|(a, b)| a != b)
                    .count(),
                2
            );
        }
    }
}
