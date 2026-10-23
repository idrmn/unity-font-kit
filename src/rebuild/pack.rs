//! MaxRects packing with the best-short-side-fit rule, the rule TMP uses when it adds glyphs.

use indexmap::{IndexMap, IndexSet};

pub const MAX_ATLAS_SIDE: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spot {
    pub x: usize,
    pub y: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Rect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

pub type Spots = IndexMap<u32, Spot>;

#[derive(Debug)]
pub struct Atlas {
    pub width: usize,
    pub height: usize,
    /// The glyph rect corner of each placed glyph, padding already added.
    pub spots: Spots,
}

/// Packs into the recipe's atlas, doubling the shorter side until everything fits or both reach 8192.
#[must_use]
pub fn pack_growing(
    items: &IndexMap<u32, Size>,
    start: Size,
    padding: usize,
    spacing: usize,
) -> Atlas {
    let Size {
        mut width,
        mut height,
    } = start;
    loop {
        let placed = pack(items, width - spacing, height - spacing);
        if placed.len() == items.len() || (width >= MAX_ATLAS_SIDE && height >= MAX_ATLAS_SIDE) {
            let offset = padding + spacing;
            let spots = placed
                .into_iter()
                .map(|(index, spot)| {
                    let moved = Spot {
                        x: spot.x + offset,
                        y: spot.y + offset,
                    };
                    (index, moved)
                })
                .collect();
            return Atlas {
                width,
                height,
                spots,
            };
        }
        if height < width || width >= MAX_ATLAS_SIDE {
            height *= 2;
        } else {
            width *= 2;
        }
    }
}

#[must_use]
pub fn pack(items: &IndexMap<u32, Size>, width: usize, height: usize) -> Spots {
    let mut free = vec![Rect {
        x: 0,
        y: 0,
        width,
        height,
    }];
    let mut placed = Spots::new();
    let mut order: Vec<(u32, Size)> = items.iter().map(|(index, size)| (*index, *size)).collect();
    // Must stay a stable sort: equal sizes keep the tile order.
    order.sort_by(|(_, a), (_, b)| b.height.cmp(&a.height).then(b.width.cmp(&a.width)));
    for (index, size) in order {
        let Some(spot) = find_spot(&free, size) else {
            continue;
        };
        placed.insert(index, spot);
        let used = Rect {
            x: spot.x,
            y: spot.y,
            width: size.width,
            height: size.height,
        };
        let (untouched, pieces) = split_free(&free, used);
        free = prune(untouched, pieces);
    }
    placed
}

fn find_spot(free: &[Rect], size: Size) -> Option<Spot> {
    free.iter()
        .filter(|rect| rect.width >= size.width && rect.height >= size.height)
        .map(|rect| {
            let (spare_x, spare_y) = (rect.width - size.width, rect.height - size.height);
            (spare_x.min(spare_y), spare_x.max(spare_y), rect.x, rect.y)
        })
        .min()
        .map(|(_, _, x, y)| Spot { x, y })
}

fn split_free(free: &[Rect], used: Rect) -> (Vec<Rect>, Vec<Rect>) {
    let (mut untouched, mut pieces) = (Vec::new(), Vec::new());
    for &rect in free {
        if !overlaps(rect, used) {
            untouched.push(rect);
            continue;
        }
        let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);
        let (used_right, used_bottom) = (used.x + used.width, used.y + used.height);
        if used.x > rect.x {
            pieces.push(Rect {
                width: used.x - rect.x,
                ..rect
            });
        }
        if used_right < right {
            pieces.push(Rect {
                x: used_right,
                width: right - used_right,
                ..rect
            });
        }
        if used.y > rect.y {
            pieces.push(Rect {
                height: used.y - rect.y,
                ..rect
            });
        }
        if used_bottom < bottom {
            pieces.push(Rect {
                y: used_bottom,
                height: bottom - used_bottom,
                ..rect
            });
        }
    }
    (untouched, pieces)
}

fn overlaps(a: Rect, b: Rect) -> bool {
    b.x < a.x + a.width && b.x + b.width > a.x && b.y < a.y + a.height && b.y + b.height > a.y
}

/// Untouched rects never contain each other, and a piece lies inside a rect that contained none of them.
fn prune(untouched: Vec<Rect>, pieces: Vec<Rect>) -> Vec<Rect> {
    let unique: IndexSet<Rect> = pieces.into_iter().collect();
    let kept: Vec<Rect> = unique
        .iter()
        .copied()
        .filter(|a| {
            !untouched
                .iter()
                .chain(unique.iter())
                .any(|b| b != a && contains(*b, *a))
        })
        .collect();
    untouched.into_iter().chain(kept).collect()
}

fn contains(outer: Rect, inner: Rect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && inner.x + inner.width <= outer.x + outer.width
        && inner.y + inner.height <= outer.y + outer.height
}
