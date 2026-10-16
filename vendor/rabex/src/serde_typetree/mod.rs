//! [serde] support for unity typetrees.
mod de;
mod error;
mod ser;

pub use de::{Deserializer, from_reader, from_reader_endianed, from_slice};
pub use error::{Error, Result};
pub use ser::{Serializer, to_vec, to_vec_endianed, to_writer};

// serde_json would keep each byte as a number of about 72 bytes; one base64 string keeps a megabyte atlas small.
const BYTES_PREFIX: &str = "bytes:";

pub fn encode_bytes(bytes: &[u8]) -> String {
    use base64::Engine;
    format!("{BYTES_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub fn decode_bytes(text: &str) -> Option<std::result::Result<Vec<u8>, base64::DecodeError>> {
    use base64::Engine;
    let encoded = text.strip_prefix(BYTES_PREFIX)?;
    Some(base64::engine::general_purpose::STANDARD.decode(encoded))
}

pub(crate) fn is_byte_type(name: &str) -> bool {
    matches!(name, "UInt8" | "char")
}
