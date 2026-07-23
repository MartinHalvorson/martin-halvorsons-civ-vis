//! Axial hex grid math (mirrors civvis/hexgrid.py).
use crate::Pos;

pub const DIRS: [(i32, i32); 6] = [(1, 0), (1, -1), (0, -1), (-1, 0), (-1, 1), (0, 1)];

pub fn neighbors(p: Pos) -> [Pos; 6] {
    [
        (p.0 + 1, p.1),
        (p.0 + 1, p.1 - 1),
        (p.0, p.1 - 1),
        (p.0 - 1, p.1),
        (p.0 - 1, p.1 + 1),
        (p.0, p.1 + 1),
    ]
}

/// The in-map neighbors of a hex, held inline.
///
/// A hex has at most six neighbors, so the answer never needs the heap.
/// Neighbor queries run inside adjacency bonuses, pathfinding, and every
/// line-of-sight ray, and a `Vec` per query dominated the engine's allocator
/// traffic.
#[derive(Clone, Copy, Debug, Default)]
pub struct Neighbors {
    buf: [Pos; 6],
    len: u8,
}

impl Neighbors {
    pub fn new() -> Neighbors {
        Neighbors::default()
    }

    #[inline]
    pub fn push(&mut self, pos: Pos) {
        self.buf[self.len as usize] = pos;
        self.len += 1;
    }
}

impl std::ops::Deref for Neighbors {
    type Target = [Pos];

    #[inline]
    fn deref(&self) -> &[Pos] {
        &self.buf[..self.len as usize]
    }
}

impl std::ops::DerefMut for Neighbors {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Pos] {
        &mut self.buf[..self.len as usize]
    }
}

impl IntoIterator for Neighbors {
    type Item = Pos;
    type IntoIter = std::iter::Take<std::array::IntoIter<Pos, 6>>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.buf.into_iter().take(self.len as usize)
    }
}

impl<'a> IntoIterator for &'a Neighbors {
    type Item = &'a Pos;
    type IntoIter = std::slice::Iter<'a, Pos>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl FromIterator<Pos> for Neighbors {
    fn from_iter<I: IntoIterator<Item = Pos>>(iter: I) -> Neighbors {
        let mut out = Neighbors::new();
        for pos in iter {
            out.push(pos);
        }
        out
    }
}

pub fn distance(a: Pos, b: Pos) -> i32 {
    let dq = a.0 - b.0;
    let dr = a.1 - b.1;
    dq.abs().max(dr.abs()).max((dq + dr).abs())
}

pub fn disk(c: Pos, radius: i32) -> Vec<Pos> {
    let mut out = Vec::new();
    for dq in -radius..=radius {
        let lo = (-radius).max(-dq - radius);
        let hi = radius.min(-dq + radius);
        for dr in lo..=hi {
            out.push((c.0 + dq, c.1 + dr));
        }
    }
    out
}

pub fn offset_to_axial(col: i32, row: i32) -> Pos {
    (col - (row - (row & 1)) / 2, row)
}

/// Canonical position on an east-west wrapping (cylindrical) map.
pub fn canon(p: Pos, width: i32) -> Pos {
    let col = p.0 + (p.1 - (p.1 & 1)) / 2;
    let m = col.rem_euclid(width);
    (p.0 + (m - col), p.1)
}

/// Hex distance on a cylinder of the given width.
pub fn wdistance(a: Pos, b: Pos, width: i32) -> i32 {
    let mut best = i32::MAX;
    for s in [-width, 0, width] {
        best = best.min(distance((a.0 + s, a.1), b));
    }
    best
}

pub fn axial_to_offset(q: i32, r: i32) -> (i32, i32) {
    (q + (r - (r & 1)) / 2, r)
}
