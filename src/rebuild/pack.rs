//! MaxRects packing with the best-short-side-fit rule, the rule TMP uses when it adds glyphs.

use indexmap::{IndexMap, IndexSet};

pub const MAX_ATLAS_SIDE: usize = 8192;

/// x, y, width, height.
type Rect = (usize, usize, usize, usize);
pub type Spots = IndexMap<u32, (usize, usize)>;

#[derive(Debug)]
pub struct Atlas {
    pub width: usize,
    pub height: usize,
    /// The glyph rect corner of each placed glyph, padding already added.
    pub spots: Spots,
}

/// Packs into the recipe's atlas, doubling the shorter side until everything fits or both reach 8192.
pub fn pack_growing(
    items: &IndexMap<u32, (usize, usize)>,
    start: (usize, usize),
    padding: usize,
    spacing: usize,
) -> Atlas {
    let (mut width, mut height) = start;
    loop {
        let placed = pack(items, width - spacing, height - spacing);
        if placed.len() == items.len() || (width >= MAX_ATLAS_SIDE && height >= MAX_ATLAS_SIDE) {
            let offset = padding + spacing;
            let spots = placed
                .into_iter()
                .map(|(index, (x, y))| (index, (x + offset, y + offset)))
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

pub fn pack(items: &IndexMap<u32, (usize, usize)>, width: usize, height: usize) -> Spots {
    let mut free = vec![(0, 0, width, height)];
    let mut placed = Spots::new();
    let mut order: Vec<(u32, (usize, usize))> =
        items.iter().map(|(index, size)| (*index, *size)).collect();
    // Must stay a stable sort: equal sizes keep the tile order.
    order.sort_by(|(_, (aw, ah)), (_, (bw, bh))| bh.cmp(ah).then(bw.cmp(aw)));
    for (index, (w, h)) in order {
        let Some(spot) = find_spot(&free, w, h) else {
            continue;
        };
        placed.insert(index, spot);
        let (untouched, pieces) = split_free(&free, (spot.0, spot.1, w, h));
        free = prune(untouched, pieces);
    }
    placed
}

fn find_spot(free: &[Rect], w: usize, h: usize) -> Option<(usize, usize)> {
    free.iter()
        .filter(|(_, _, fw, fh)| *fw >= w && *fh >= h)
        .map(|&(x, y, fw, fh)| ((fw - w).min(fh - h), (fw - w).max(fh - h), x, y))
        .min()
        .map(|(_, _, x, y)| (x, y))
}

fn split_free(free: &[Rect], used: Rect) -> (Vec<Rect>, Vec<Rect>) {
    let (ux, uy, uw, uh) = used;
    let (mut untouched, mut pieces) = (Vec::new(), Vec::new());
    for &(x, y, w, h) in free {
        if ux >= x + w || ux + uw <= x || uy >= y + h || uy + uh <= y {
            untouched.push((x, y, w, h));
            continue;
        }
        if ux > x {
            pieces.push((x, y, ux - x, h));
        }
        if ux + uw < x + w {
            pieces.push((ux + uw, y, x + w - ux - uw, h));
        }
        if uy > y {
            pieces.push((x, y, w, uy - y));
        }
        if uy + uh < y + h {
            pieces.push((x, uy + uh, w, y + h - uy - uh));
        }
    }
    (untouched, pieces)
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
    let (ox, oy, ow, oh) = outer;
    let (ix, iy, iw, ih) = inner;
    ox <= ix && oy <= iy && ix + iw <= ox + ow && iy + ih <= oy + oh
}
