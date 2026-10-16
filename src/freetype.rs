//! Safe wrapper over the few FreeType calls ufk needs. The only module with `unsafe`.
#![allow(unsafe_code)]

use std::ffi::CStr;
use std::ptr;

use anyhow::{Result, bail};
use freetype_sys as ft;

pub use ft::{
    FT_LOAD_DEFAULT as LOAD_DEFAULT, FT_LOAD_NO_HINTING as LOAD_NO_HINTING,
    FT_LOAD_TARGET_MONO as LOAD_TARGET_MONO, FT_RENDER_MODE_MONO as RENDER_MONO,
    FT_RENDER_MODE_NORMAL as RENDER_NORMAL, FT_RENDER_MODE_SDF as RENDER_SDF,
};

pub const PIXEL_MODE_MONO: u8 = 1;

#[derive(Debug)]
pub struct Library {
    raw: ft::FT_Library,
}

/// A face borrows its library and its font bytes, so neither can be freed while it lives.
#[derive(Debug)]
pub struct Face<'a> {
    raw: ft::FT_Face,
    _library: &'a Library,
    _bytes: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    pub width: i64,
    pub height: i64,
    pub bearing_x: i64,
    pub bearing_y: i64,
    pub advance: i64,
    pub bitmap_left: i32,
    pub bitmap_top: i32,
    pub rows: u32,
    pub columns: u32,
    pub pitch: i32,
    pub pixel_mode: u8,
    pub buffer: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceFields {
    pub family_name: String,
    pub style_name: String,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    pub height: i16,
    pub underline_position: i16,
    pub underline_thickness: i16,
}

/// # Safety
/// `name` is null or points to a NUL-terminated string that outlives this call.
unsafe fn read_name(name: *const ft::FT_String) -> String {
    if name.is_null() {
        return String::new();
    }
    // SAFETY: the caller guarantees a NUL-terminated string.
    let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    String::from_utf8_lossy(bytes).into_owned()
}

fn check(error: ft::FT_Error, what: &str) -> Result<()> {
    if error != 0 {
        bail!("FreeType {what} failed with error {error:#x}");
    }
    Ok(())
}

impl Library {
    pub fn new() -> Result<Library> {
        let mut raw = ptr::null_mut();
        // SAFETY: FT_Init_FreeType writes a fresh library handle into `raw` on success.
        check(unsafe { ft::FT_Init_FreeType(&mut raw) }, "init")?;
        Ok(Library { raw })
    }

    /// TrueType hinting: 35 matches Unity before 6, 40 matches Unity 6.
    pub fn set_interpreter_version(&self, version: u32) -> Result<()> {
        self.set_property(c"truetype", c"interpreter-version", &version)
    }

    pub fn set_sdf_spread(&self, spread: i32) -> Result<()> {
        self.set_property(c"bsdf", c"spread", &spread)
    }

    fn set_property<T>(&self, module: &CStr, name: &CStr, value: &T) -> Result<()> {
        // SAFETY: the library is alive, the names are NUL-terminated, and FreeType reads `value`
        // as the type the property expects (FT_UInt for interpreter-version, FT_Int for spread).
        let error = unsafe {
            ft::FT_Property_Set(
                self.raw,
                module.as_ptr(),
                name.as_ptr(),
                ptr::from_ref(value).cast(),
            )
        };
        check(error, "property set")
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: every Face borrows the library, so none is alive here.
        unsafe { ft::FT_Done_FreeType(self.raw) };
    }
}

impl<'a> Face<'a> {
    pub fn from_bytes(library: &'a Library, bytes: &'a [u8]) -> Result<Face<'a>> {
        let mut raw = ptr::null_mut();
        let size = ft::FT_Long::try_from(bytes.len())?;
        // SAFETY: FreeType keeps a pointer to `bytes`; the lifetime ties them to the face.
        let error =
            unsafe { ft::FT_New_Memory_Face(library.raw, bytes.as_ptr(), size, 0, &mut raw) };
        check(error, "open face")?;
        Ok(Face {
            raw,
            _library: library,
            _bytes: bytes,
        })
    }

    /// Size in 1/64 points at 72 dpi, so one point is one pixel.
    pub fn set_char_size(&self, size_64: i64) -> Result<()> {
        let size = ft::FT_F26Dot6::try_from(size_64)?;
        // SAFETY: the face is alive.
        check(
            unsafe { ft::FT_Set_Char_Size(self.raw, size, 0, 72, 72) },
            "set size",
        )
    }

    pub fn fields(&self) -> FaceFields {
        // SAFETY: the face is alive; FreeType keeps the name strings NUL-terminated or null.
        unsafe {
            let face = &*self.raw;
            FaceFields {
                family_name: read_name(face.family_name),
                style_name: read_name(face.style_name),
                units_per_em: face.units_per_EM,
                ascender: face.ascender,
                descender: face.descender,
                height: face.height,
                underline_position: face.underline_position,
                underline_thickness: face.underline_thickness,
            }
        }
    }

    pub fn char_index(&self, codepoint: u32) -> u32 {
        // SAFETY: the face is alive.
        unsafe { ft::FT_Get_Char_Index(self.raw, ft::FT_ULong::from(codepoint)) }
    }

    pub fn load_glyph(&self, index: u32, flags: i32) -> Result<()> {
        // SAFETY: the face is alive; an index past the glyph count returns an error code.
        check(
            unsafe { ft::FT_Load_Glyph(self.raw, index, flags) },
            "load glyph",
        )
    }

    pub fn render(&self, mode: u32) -> Result<()> {
        // SAFETY: the face and its glyph slot are alive.
        check(
            unsafe { ft::FT_Render_Glyph((*self.raw).glyph, mode) },
            "render",
        )
    }

    // FT_Pos is a C long: 64-bit on Linux, 32-bit on Windows.
    #[allow(clippy::useless_conversion)]
    pub fn glyph(&self) -> Glyph {
        // SAFETY: the slot is owned by the live face; the bitmap buffer holds rows * |pitch| bytes.
        unsafe {
            let slot = &*(*self.raw).glyph;
            let bitmap = &slot.bitmap;
            let length = bitmap.rows.unsigned_abs() as usize * bitmap.pitch.unsigned_abs() as usize;
            let buffer = if bitmap.buffer.is_null() || length == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(bitmap.buffer, length).to_vec()
            };
            Glyph {
                width: i64::from(slot.metrics.width),
                height: i64::from(slot.metrics.height),
                bearing_x: i64::from(slot.metrics.horiBearingX),
                bearing_y: i64::from(slot.metrics.horiBearingY),
                advance: i64::from(slot.metrics.horiAdvance),
                bitmap_left: slot.bitmap_left,
                bitmap_top: slot.bitmap_top,
                rows: bitmap.rows.unsigned_abs(),
                columns: bitmap.width.unsigned_abs(),
                pitch: bitmap.pitch,
                pixel_mode: bitmap.pixel_mode as u8,
                buffer,
            }
        }
    }
}

impl Drop for Face<'_> {
    fn drop(&mut self) {
        // SAFETY: the face was created by FT_New_Memory_Face and is freed once.
        unsafe { ft::FT_Done_Face(self.raw) };
    }
}
