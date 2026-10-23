use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use kurbo::{Affine, BezPath, CubicBez, Point, Rect};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};
use write_fonts::FontBuilder;
use write_fonts::tables::cmap::Cmap;
use write_fonts::tables::glyf::{Bbox, GlyfLocaBuilder, Glyph, SimpleGlyph};
use write_fonts::tables::head::Head;
use write_fonts::tables::hhea::Hhea;
use write_fonts::tables::hmtx::{Hmtx, LongMetric};
use write_fonts::tables::maxp::Maxp;
use write_fonts::tables::name::{Name, NameRecord};
use write_fonts::tables::os2::Os2;
use write_fonts::tables::post::Post;
use write_fonts::types::{FWord, GlyphId, NameId, UfWord};

/// The letters drawn under each number, also the window's own font.
pub const LETTERS: &[u8] = include_bytes!("../../assets/fonts/IBMPlexSans-Regular.otf");

const UNITS: u16 = 1000;
const ASCENT: i16 = 1030;
const DESCENT: i16 = -170;
// The letter keeps its baseline and shrinks so the number fits above accented capitals;
// the number stays about a third of a capital, so it does not read as text.
const LETTER_SCALE: f64 = 0.7;
const DIGIT_WIDTH: f64 = 175.;
const DIGIT_HEIGHT: f64 = 280.;
const DIGIT_BOTTOM: f64 = 730.;
const STROKE: f64 = 50.;
const GAP: f64 = 36.;
const SIDE: f64 = 40.;
const SPACE_WIDTH: u16 = 300;
// Accuracy of turning the letters' cubic curves into the quadratic ones TrueType needs.
const CURVE_ACCURACY: f64 = 1.;

/// Segments a to g of a seven-segment display, lit for each digit.
const SEGMENTS: [[bool; 7]; 10] = [
    [true, true, true, true, true, true, false],
    [false, true, true, false, false, false, false],
    [true, true, false, true, true, false, true],
    [true, true, true, true, false, false, true],
    [false, true, true, false, false, true, true],
    [true, false, true, true, false, true, true],
    [true, false, true, true, true, true, true],
    [true, true, true, false, false, false, false],
    [true, true, true, true, true, true, true],
    [true, true, true, true, false, true, true],
];

// Latin, Greek, Cyrillic, punctuation, currency and letterlike symbols such as №.
const MARKED: [(u32, u32); 5] = [
    (0x21, 0x24F),
    (0x370, 0x52F),
    (0x2000, 0x206F),
    (0x20A0, 0x20CF),
    (0x2100, 0x214F),
];

struct Outline {
    path: BezPath,
    width: f64,
}

struct Drawn {
    glyph: SimpleGlyph,
    advance: u16,
}

/// A font that draws each letter with `number` over it, so the game shows which font a text uses.
pub fn make_number_font(number: usize) -> Result<Vec<u8>> {
    let letters = FontRef::new(LETTERS).context("Cannot read the letters font")?;
    let mark = draw_number(number);
    let nothing = Outline {
        path: BezPath::new(),
        width: 0.,
    };
    let only_number = place_glyph(&nothing, &mark)?;
    let mut glyphs = vec![only_number];
    let mut by_source: HashMap<skrifa::GlyphId, u32> = HashMap::new();
    let mut mappings = Vec::new();
    for ch in marked_characters() {
        if ch.is_whitespace() {
            mappings.push((ch, GlyphId::new(1)));
            continue;
        }
        let Some(source) = letters.charmap().map(ch) else {
            mappings.push((ch, GlyphId::new(2)));
            continue;
        };
        let id = match by_source.get(&source) {
            Some(id) => *id,
            None => {
                let letter = read_letter(&letters, source)?;
                glyphs.push(place_glyph(&letter, &mark)?);
                let id = glyphs.len() as u32 + 1;
                by_source.insert(source, id);
                id
            }
        };
        mappings.push((ch, GlyphId::new(id)));
    }
    build_font(number, &glyphs, mappings)
}

fn marked_characters() -> impl Iterator<Item = char> {
    MARKED
        .into_iter()
        .flat_map(|(first, last)| first..=last)
        .filter_map(char::from_u32)
        .chain([' '])
}

fn read_letter(font: &FontRef, glyph: skrifa::GlyphId) -> Result<Outline> {
    use skrifa::raw::TableProvider;

    let units = f64::from(font.head()?.units_per_em());
    let scale = LETTER_SCALE * f64::from(UNITS) / units;
    let width = font
        .glyph_metrics(Size::unscaled(), LocationRef::default())
        .advance_width(glyph)
        .map_or(0., f64::from);
    let mut pen = PathPen::default();
    if let Some(outline) = font.outline_glyphs().get(glyph) {
        let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
        outline
            .draw(settings, &mut pen)
            .map_err(|error| anyhow!("Cannot draw a letter: {error}"))?;
    }
    Ok(Outline {
        path: Affine::scale(scale) * pen.path,
        width: width * scale,
    })
}

/// The number sits over the letter, both centered in an advance wide enough for either.
fn place_glyph(letter: &Outline, mark: &Outline) -> Result<Drawn> {
    let advance = letter.width.max(mark.width + 2. * SIDE);
    let mut path = Affine::translate(((advance - letter.width) / 2., 0.)) * letter.path.clone();
    path.extend(Affine::translate(((advance - mark.width) / 2., 0.)) * mark.path.clone());
    let glyph = SimpleGlyph::from_bezpath(&path)
        .map_err(|error| anyhow!("Cannot draw the number: {error:?}"))?;
    Ok(Drawn {
        glyph,
        advance: advance.round() as u16,
    })
}

fn build_font(number: usize, drawn: &[Drawn], mappings: Vec<(char, GlyphId)>) -> Result<Vec<u8>> {
    let mut glyphs = GlyfLocaBuilder::new();
    glyphs.add_glyph(&Glyph::Empty)?;
    glyphs.add_glyph(&Glyph::Empty)?;
    for item in drawn {
        glyphs.add_glyph(&item.glyph)?;
    }
    let (glyf, loca, loca_format) = glyphs.build();
    let bbox = drawn
        .iter()
        .map(|item| item.glyph.bbox)
        .reduce(|a, b| a.union(b))
        .unwrap_or(Bbox::default());
    let widest = drawn
        .iter()
        .map(|item| item.advance)
        .max()
        .unwrap_or(SPACE_WIDTH);
    let metrics: Vec<LongMetric> = [SPACE_WIDTH, SPACE_WIDTH]
        .into_iter()
        .map(|advance| LongMetric::new(advance, 0))
        .chain(
            drawn
                .iter()
                .map(|item| LongMetric::new(item.advance, item.glyph.bbox.x_min)),
        )
        .collect();
    let head = Head {
        units_per_em: UNITS,
        x_min: bbox.x_min,
        y_min: bbox.y_min,
        x_max: bbox.x_max,
        y_max: bbox.y_max,
        lowest_rec_ppem: 8,
        index_to_loc_format: loca_format as i16,
        ..Default::default()
    };
    let hhea = Hhea {
        ascender: FWord::new(ASCENT),
        descender: FWord::new(DESCENT),
        advance_width_max: UfWord::new(widest),
        x_max_extent: FWord::new(bbox.x_max),
        caret_slope_rise: 1,
        number_of_h_metrics: metrics.len() as u16,
        ..Default::default()
    };
    let os2 = Os2 {
        x_avg_char_width: widest as i16,
        us_weight_class: 400,
        us_width_class: 5,
        s_typo_ascender: ASCENT,
        s_typo_descender: DESCENT,
        us_win_ascent: ASCENT as u16,
        us_win_descent: (-DESCENT) as u16,
        sx_height: Some(300),
        s_cap_height: Some(700),
        ul_code_page_range_1: Some(1),
        ul_code_page_range_2: Some(0),
        us_default_char: Some(0),
        us_break_char: Some(32),
        us_max_context: Some(0),
        ..Default::default()
    };
    let family = format!("ufk number {number}");
    let name = Name::new(
        [
            (NameId::FAMILY_NAME, family.clone()),
            (NameId::SUBFAMILY_NAME, "Regular".to_string()),
            (NameId::FULL_NAME, family.clone()),
            (NameId::POSTSCRIPT_NAME, format!("ufk-number-{number}")),
        ]
        .into_iter()
        .map(|(id, text)| NameRecord::new(3, 1, 0x409, id, text.into()))
        .collect(),
    );
    let cmap = Cmap::from_mappings(mappings)?;
    let mut font = FontBuilder::new();
    font.add_table(&head)?
        .add_table(&hhea)?
        .add_table(&Maxp::new(metrics.len() as u16))?
        .add_table(&os2)?
        .add_table(&Hmtx::new(metrics, Vec::new()))?
        .add_table(&cmap)?
        .add_table(&glyf)?
        .add_table(&loca)?
        .add_table(&name)?
        .add_table(&Post::default())?;
    Ok(font.build())
}

fn draw_number(number: usize) -> Outline {
    use kurbo::Shape;

    let digits: Vec<usize> = number
        .to_string()
        .bytes()
        .map(|byte| usize::from(byte - b'0'))
        .collect();
    let mut path = BezPath::new();
    for (place, digit) in digits.iter().enumerate() {
        let left = place as f64 * (DIGIT_WIDTH + GAP);
        for segment in lit_segments(*digit, left) {
            path.extend(segment.path_elements(0.1));
        }
    }
    let width = digits.len() as f64 * (DIGIT_WIDTH + GAP) - GAP;
    Outline { path, width }
}

fn lit_segments(digit: usize, left: f64) -> Vec<Rect> {
    let (right, bottom) = (left + DIGIT_WIDTH, DIGIT_BOTTOM);
    let (top, middle) = (bottom + DIGIT_HEIGHT, bottom + DIGIT_HEIGHT / 2.);
    let half = STROKE / 2.;
    let all = [
        Rect::new(left, top - STROKE, right, top),
        Rect::new(right - STROKE, middle, right, top),
        Rect::new(right - STROKE, bottom, right, middle),
        Rect::new(left, bottom, right, bottom + STROKE),
        Rect::new(left, bottom, left + STROKE, middle),
        Rect::new(left, middle, left + STROKE, top),
        Rect::new(left, middle - half, right, middle + half),
    ];
    all.into_iter()
        .zip(SEGMENTS[digit])
        .filter_map(|(rect, lit)| lit.then_some(rect))
        .collect()
}

#[derive(Default)]
struct PathPen {
    path: BezPath,
    last: Point,
}

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.last = Point::new(x.into(), y.into());
        self.path.move_to(self.last);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.last = Point::new(x.into(), y.into());
        self.path.line_to(self.last);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.last = Point::new(x.into(), y.into());
        self.path
            .quad_to(Point::new(cx0.into(), cy0.into()), self.last);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let end = Point::new(x.into(), y.into());
        let cubic = CubicBez::new(
            self.last,
            Point::new(cx0.into(), cy0.into()),
            Point::new(cx1.into(), cy1.into()),
            end,
        );
        for (_, _, quad) in cubic.to_quads(CURVE_ACCURACY) {
            self.path.quad_to(quad.p1, quad.p2);
        }
        self.last = end;
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_font_keeps_letters_apart_and_spaces_blank() {
        let data = make_number_font(12).unwrap();
        let font = FontRef::new(&data).unwrap();
        let charmap = font.charmap();
        assert_ne!(charmap.map('Ї'), charmap.map('Є'));
        assert_eq!(charmap.map(' '), Some(skrifa::GlyphId::new(1)));
    }
}
