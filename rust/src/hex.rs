//! Axial hex grid math (mirrors civ65/hexgrid.py).
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

pub fn axial_to_offset(q: i32, r: i32) -> (i32, i32) {
    (q + (r - (r & 1)) / 2, r)
}
