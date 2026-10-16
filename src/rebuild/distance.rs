#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<bool>,
}

impl Mask {
    pub fn new(width: usize, height: usize) -> Mask {
        Mask {
            width,
            height,
            cells: vec![false; width * height],
        }
    }

    pub fn get(&self, row: usize, col: usize) -> bool {
        self.cells[row * self.width + col]
    }

    pub fn set(&mut self, row: usize, col: usize, value: bool) {
        self.cells[row * self.width + col] = value;
    }

    pub fn any(&self) -> bool {
        self.cells.iter().any(|cell| *cell)
    }

    pub fn inverted(&self) -> Mask {
        Mask {
            cells: self.cells.iter().map(|cell| !cell).collect(),
            ..*self
        }
    }
}

const FAR: i64 = i64::MAX / 4;

/// For every cell, the squared distance to the nearest `true` cell (`FAR` if there is none).
pub fn squared_distance_to(targets: &Mask) -> Vec<i64> {
    let (width, height) = (targets.width, targets.height);
    let mut grid: Vec<i64> = targets
        .cells
        .iter()
        .map(|cell| if *cell { 0 } else { FAR })
        .collect();
    let mut column = vec![0; height];
    for col in 0..width {
        for row in 0..height {
            column[row] = grid[row * width + col];
        }
        let lowered = lower_envelope(&column);
        for row in 0..height {
            grid[row * width + col] = lowered[row];
        }
    }
    for row in 0..height {
        let lowered = lower_envelope(&grid[row * width..(row + 1) * width]);
        grid[row * width..(row + 1) * width].copy_from_slice(&lowered);
    }
    grid
}

/// With no target at all it measures to the point (-1, 0); the bsdf fallback for a solid glyph
/// without padding relies on it.
pub fn edt_squared(targets: &Mask) -> Vec<i64> {
    if targets.any() {
        return squared_distance_to(targets);
    }
    (0..targets.height as i64)
        .flat_map(|row| (0..targets.width as i64).map(move |col| (row + 1).pow(2) + col.pow(2)))
        .collect()
}

/// One pass of Felzenszwalb and Huttenlocher's distance transform: min over q of f(q) + (p - q)^2.
fn lower_envelope(values: &[i64]) -> Vec<i64> {
    let n = values.len();
    let mut out = vec![FAR; n];
    let sites: Vec<usize> = (0..n).filter(|&q| values[q] < FAR).collect();
    if sites.is_empty() {
        return out;
    }
    let mut hull: Vec<usize> = Vec::with_capacity(sites.len());
    let mut starts: Vec<f64> = Vec::with_capacity(sites.len());
    for &q in &sites {
        loop {
            let Some(&v) = hull.last() else {
                hull.push(q);
                starts.push(f64::NEG_INFINITY);
                break;
            };
            let s = intersection(values, v, q);
            if s <= *starts.last().unwrap_or(&f64::NEG_INFINITY) {
                hull.pop();
                starts.pop();
                continue;
            }
            hull.push(q);
            starts.push(s);
            break;
        }
    }
    let mut k = 0;
    for (p, slot) in out.iter_mut().enumerate() {
        while k + 1 < hull.len() && starts[k + 1] <= p as f64 {
            k += 1;
        }
        let q = hull[k];
        let d = p as i64 - q as i64;
        *slot = values[q] + d * d;
    }
    out
}

fn intersection(values: &[i64], v: usize, q: usize) -> f64 {
    let (v_i, q_i) = (v as i64, q as i64);
    let numerator = (values[q] + q_i * q_i) - (values[v] + v_i * v_i);
    numerator as f64 / (2 * (q_i - v_i)) as f64
}

pub fn root(squared: i64) -> f64 {
    if squared >= FAR {
        return f64::INFINITY;
    }
    (squared as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_targets_measures_to_the_point_above_the_first_cell() {
        let squared = edt_squared(&Mask::new(2, 2));
        assert_eq!(squared, [1, 2, 4, 5]);
    }

    #[test]
    fn matches_brute_force_on_a_ragged_mask() {
        let mut mask = Mask::new(7, 5);
        for (row, col) in [(0, 0), (2, 3), (4, 6), (1, 5)] {
            mask.set(row, col, true);
        }
        let fast = squared_distance_to(&mask);
        for row in 0..5i64 {
            for col in 0..7i64 {
                let brute = [(0, 0), (2, 3), (4, 6), (1, 5)]
                    .iter()
                    .map(|(r, c)| (row - r).pow(2) + (col - c).pow(2))
                    .min()
                    .unwrap();
                assert_eq!(fast[(row * 7 + col) as usize], brute);
            }
        }
    }
}
