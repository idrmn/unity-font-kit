// FreeType 2.13.2 with default options (include/freetype/config/ftoption.h):
// the atlas bakes were measured against Unity with this build.
fn main() {
    let root = std::path::Path::new("freetype2/src");
    let modules = [
        "autofit/autofit",
        "base/ftbase",
        "base/ftbbox",
        "base/ftbitmap",
        "base/ftdebug",
        "base/ftfstype",
        "base/ftgasp",
        "base/ftglyph",
        "base/ftinit",
        "base/ftmm",
        "base/ftsynth",
        "base/ftsystem",
        "cff/cff",
        "cid/type1cid",
        "gzip/ftgzip",
        "psaux/psaux",
        "pshinter/pshinter",
        "psnames/psnames",
        "raster/raster",
        "sdf/sdf",
        "svg/svg",
        "sfnt/sfnt",
        "smooth/smooth",
        "truetype/truetype",
        "type1/type1",
        "type42/type42",
        "bdf/bdf",
        "pcf/pcf",
        "pfr/pfr",
        "winfonts/winfnt",
        "lzw/ftlzw",
        "bzip2/ftbzip2",
        "cache/ftcache",
    ];
    cc::Build::new()
        .warnings(false)
        .include("freetype2/include")
        .define("FT2_BUILD_LIBRARY", None)
        .files(modules.iter().map(|module| root.join(format!("{module}.c"))))
        .compile("freetype2");
    println!("cargo:rerun-if-changed=build.rs");
}
