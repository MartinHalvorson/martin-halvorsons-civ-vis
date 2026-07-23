//! Wrap-aware fractal height fields — the source of every clustered landform.
//!
//! Civ VI's map scripts do not roll terrain per tile. `TerrainGenerator.lua`,
//! `MountainsCliffs.lua` and `FeatureGenerator.lua` all sample *fractal height
//! fields* built by `Fractal.Create(iW, iH, grain, flags)` and then cut them at
//! a percentile — `deserts:GetHeight(75)` is "the top quarter of the desert
//! field", which lands as a few broad regions rather than as scattered tiles.
//! This module is that facility: a cylindrical midpoint-displacement field with
//! the same two-call shape (`at` for a tile, `percentile` for a threshold).
//!
//! `build_ridges` stands in for the shipped `Fractal:BuildRidges(numPlates)`,
//! whose implementation lives in the engine rather than in Lua. It weaves
//! tectonic plate boundaries into the field so that the high percentiles form
//! connected ranges instead of isolated peaks.

use crate::rng::Rng;

/// Side length of the square lattice, matching the shipped default exponent of
/// 7. Map coordinates are sampled into it proportionally, exactly as the stock
/// generator samples one 128-wide field for every map size.
const LATTICE_EXPONENT: u32 = 7;
const LATTICE: usize = 1 << LATTICE_EXPONENT;

pub struct Fractal {
    /// `LATTICE` columns (cylindrical) by `LATTICE + 1` rows (bounded).
    values: Vec<f64>,
    width: i32,
    height: i32,
    /// Every map cell's height, sorted, so a percentile is a lookup.
    sorted: Vec<u8>,
}

impl Fractal {
    /// Build a field whose coarsest detail is `grain` subdivisions across the
    /// map: lower grain means fewer, larger regions.
    pub fn new(rng: &mut Rng, width: i32, height: i32, grain: u32) -> Self {
        let mut fractal = Fractal {
            values: vec![0.0; LATTICE * (LATTICE + 1)],
            width,
            height,
            sorted: Vec::new(),
        };
        fractal.midpoint_displacement(rng, grain);
        fractal.normalize();
        fractal.index_heights();
        fractal
    }

    /// Weave `plates` tectonic plates into the field. Boundaries where two
    /// plates converge are lifted; boundaries that pull apart are left alone,
    /// so ranges follow a few long collision lines rather than every seam.
    pub fn build_ridges(
        &mut self,
        rng: &mut Rng,
        plates: usize,
        blend_ridge: f64,
        blend_fract: f64,
    ) {
        if plates == 0 {
            return;
        }
        struct Plate {
            col: f64,
            row: f64,
            drift_col: f64,
            drift_row: f64,
        }
        let seeds: Vec<Plate> = (0..plates)
            .map(|_| {
                let angle = rng.uniform(0.0, std::f64::consts::TAU);
                Plate {
                    col: rng.uniform(0.0, self.width as f64),
                    row: rng.uniform(0.0, self.height as f64),
                    drift_col: angle.cos(),
                    drift_row: angle.sin(),
                }
            })
            .collect();
        // Ranges should read as chains, so a boundary's influence fades within
        // a couple of tiles instead of lifting a whole plateau.
        let falloff = (self.width.min(self.height) as f64 / 22.0).max(1.5);

        for row_index in 0..=LATTICE {
            for col_index in 0..LATTICE {
                let col = col_index as f64 * self.width as f64 / LATTICE as f64;
                let row = row_index as f64 * self.height as f64 / LATTICE as f64;
                let mut nearest: Option<(f64, usize)> = None;
                let mut second: Option<(f64, usize)> = None;
                for (index, plate) in seeds.iter().enumerate() {
                    let mut delta_col = (plate.col - col).abs();
                    if delta_col > self.width as f64 / 2.0 {
                        delta_col = self.width as f64 - delta_col;
                    }
                    let delta_row = plate.row - row;
                    let distance = (delta_col * delta_col + delta_row * delta_row).sqrt();
                    if nearest.is_none_or(|(best, _)| distance < best) {
                        second = nearest;
                        nearest = Some((distance, index));
                    } else if second.is_none_or(|(best, _)| distance < best) {
                        second = Some((distance, index));
                    }
                }
                let (Some((first_distance, first)), Some((second_distance, other))) =
                    (nearest, second)
                else {
                    continue;
                };
                // Equidistant from two plates means standing on their seam.
                let closeness = (-((second_distance - first_distance) / falloff)).exp();
                let plate = &seeds[first];
                let neighbor = &seeds[other];
                let mut toward_col = neighbor.col - plate.col;
                if toward_col.abs() > self.width as f64 / 2.0 {
                    toward_col -= self.width as f64 * toward_col.signum();
                }
                let toward_row = neighbor.row - plate.row;
                let length = (toward_col * toward_col + toward_row * toward_row)
                    .sqrt()
                    .max(1e-9);
                let convergence = ((plate.drift_col - neighbor.drift_col) * toward_col
                    + (plate.drift_row - neighbor.drift_row) * toward_row)
                    / length
                    / 2.0;
                let ridge = 255.0 * closeness * convergence.clamp(0.0, 1.0);
                let index = row_index * LATTICE + col_index;
                self.values[index] = (self.values[index] * blend_fract + ridge * blend_ridge)
                    / (blend_ridge + blend_fract);
            }
        }
        self.normalize();
        self.index_heights();
    }

    /// The field's height under a map tile, bilinearly interpolated.
    pub fn at(&self, col: i32, row: i32) -> u8 {
        let x = col as f64 * LATTICE as f64 / self.width.max(1) as f64;
        let y = row as f64 * LATTICE as f64 / self.height.max(1) as f64;
        let x0 = x.floor();
        let y0 = y.floor().clamp(0.0, LATTICE as f64);
        let (fx, fy) = (x - x0, y - y0);
        let col0 = x0 as usize % LATTICE;
        let col1 = (col0 + 1) % LATTICE;
        let row0 = (y0 as usize).min(LATTICE);
        let row1 = (row0 + 1).min(LATTICE);
        let top = self.values[row0 * LATTICE + col0] * (1.0 - fx)
            + self.values[row0 * LATTICE + col1] * fx;
        let bottom = self.values[row1 * LATTICE + col0] * (1.0 - fx)
            + self.values[row1 * LATTICE + col1] * fx;
        (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8
    }

    /// The height that `percent` of the map's tiles fall below — the stock
    /// `GetHeight(percent)`, which is how a band like "the driest quarter of
    /// the world" is expressed independently of the field's own distribution.
    pub fn percentile(&self, percent: u32) -> u8 {
        if self.sorted.is_empty() {
            return 0;
        }
        let index = (percent.min(100) as usize * (self.sorted.len() - 1)) / 100;
        self.sorted[index]
    }

    /// The same percentile, measured over a chosen subset of the map. The
    /// stock generator reads elevation percentiles off every plot including
    /// open ocean, which only works because its continents cover a predictable
    /// share of the world; sampling the land itself keeps a band like "the
    /// highest six percent" worth the same fraction of a continent whatever
    /// shape the map script gave it.
    pub fn percentile_within(
        &self,
        cells: impl IntoIterator<Item = (i32, i32)>,
        percent: u32,
    ) -> u8 {
        let mut heights: Vec<u8> = cells
            .into_iter()
            .map(|(col, row)| self.at(col, row))
            .collect();
        if heights.is_empty() {
            return self.percentile(percent);
        }
        heights.sort_unstable();
        heights[(percent.min(100) as usize * (heights.len() - 1)) / 100]
    }

    fn get(&self, col: usize, row: usize) -> f64 {
        self.values[row.min(LATTICE) * LATTICE + col % LATTICE]
    }

    fn set(&mut self, col: usize, row: usize, value: f64) {
        let index = row.min(LATTICE) * LATTICE + col % LATTICE;
        self.values[index] = value;
    }

    fn midpoint_displacement(&mut self, rng: &mut Rng, grain: u32) {
        let mut step = LATTICE >> grain.clamp(1, LATTICE_EXPONENT - 1);
        for row in (0..=LATTICE).step_by(step) {
            for col in (0..LATTICE).step_by(step) {
                self.set(col, row, rng.uniform(0.0, 255.0));
            }
        }
        let initial = step as f64;
        while step > 1 {
            let half = step / 2;
            let amplitude = 255.0 * (half as f64 / initial) * 0.7;
            // Diamond: each cell's center from its four corners.
            for row in (0..LATTICE).step_by(step) {
                for col in (0..LATTICE).step_by(step) {
                    let average = (self.get(col, row)
                        + self.get(col + step, row)
                        + self.get(col, row + step)
                        + self.get(col + step, row + step))
                        / 4.0;
                    let jitter = rng.uniform(-amplitude, amplitude);
                    self.set(col + half, row + half, average + jitter);
                }
            }
            // Square: the edge midpoints, wrapping in x and folding at the poles.
            for row in (0..=LATTICE).step_by(half) {
                let offset = if (row / half).is_multiple_of(2) { half } else { 0 };
                for col in (offset..LATTICE).step_by(step) {
                    let mut total = self.get(col + half, row) + self.get(col + LATTICE - half, row);
                    let mut count = 2.0;
                    if row >= half {
                        total += self.get(col, row - half);
                        count += 1.0;
                    }
                    if row + half <= LATTICE {
                        total += self.get(col, row + half);
                        count += 1.0;
                    }
                    let jitter = rng.uniform(-amplitude, amplitude);
                    self.set(col, row, total / count + jitter);
                }
            }
            step = half;
        }
    }

    fn normalize(&mut self) {
        let (mut low, mut high) = (f64::MAX, f64::MIN);
        for value in &self.values {
            low = low.min(*value);
            high = high.max(*value);
        }
        let span = (high - low).max(1e-9);
        for value in &mut self.values {
            *value = (*value - low) / span * 255.0;
        }
    }

    fn index_heights(&mut self) {
        let mut heights = Vec::with_capacity((self.width * self.height).max(0) as usize);
        for row in 0..self.height {
            for col in 0..self.width {
                heights.push(self.at(col, row));
            }
        }
        heights.sort_unstable();
        self.sorted = heights;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field is useful to the generator only if cutting it at a percentile
    /// yields regions: contiguous runs of tiles, not a per-tile sprinkle.
    #[test]
    fn percentile_bands_are_clustered_rather_than_scattered() {
        let mut rng = Rng::new(4);
        let field = Fractal::new(&mut rng, 60, 38, 3);
        let threshold = field.percentile(75);
        let mut selected = 0;
        let mut with_selected_neighbor = 0;
        for row in 0..38 {
            for col in 0..60 {
                if field.at(col, row) < threshold {
                    continue;
                }
                selected += 1;
                let neighbors = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                    .into_iter()
                    .filter(|(dcol, drow)| {
                        let (ncol, nrow) = (col + dcol, row + drow);
                        (0..38).contains(&nrow)
                            && field.at((ncol + 60) % 60, nrow) >= threshold
                    })
                    .count();
                if neighbors >= 2 {
                    with_selected_neighbor += 1;
                }
            }
        }
        assert!(
            selected > 400 && selected < 800,
            "the top quarter of the field should cover about a quarter of {} tiles, got {selected}",
            60 * 38
        );
        assert!(
            with_selected_neighbor * 100 / selected >= 70,
            "a fractal band must be regional: only {with_selected_neighbor}/{selected} tiles \
             had two same-band orthogonal neighbors"
        );
    }

    #[test]
    fn the_field_wraps_around_the_cylinder() {
        let mut rng = Rng::new(11);
        let field = Fractal::new(&mut rng, 64, 40, 3);
        for row in 0..40 {
            let left = field.at(0, row) as i32;
            let right = field.at(63, row) as i32;
            assert!(
                (left - right).abs() <= 48,
                "row {row} jumps {left} -> {right} across the seam",
            );
        }
    }

    /// Ridged fields are what turn "the top 6%" into mountain ranges: a cut of
    /// a ridged field should contain long connected chains.
    #[test]
    fn ridges_produce_connected_chains() {
        let mut rng = Rng::new(21);
        let mut field = Fractal::new(&mut rng, 60, 38, 3);
        field.build_ridges(&mut rng, 9, 5.0, 5.0);
        let threshold = field.percentile(94);
        let mut peaks = std::collections::BTreeSet::new();
        for row in 0..38 {
            for col in 0..60 {
                if field.at(col, row) >= threshold {
                    peaks.insert((col, row));
                }
            }
        }
        assert!(!peaks.is_empty());
        let mut seen = std::collections::BTreeSet::new();
        let mut largest = 0;
        for start in &peaks {
            if seen.contains(start) {
                continue;
            }
            let mut stack = vec![*start];
            let mut size = 0;
            while let Some((col, row)) = stack.pop() {
                if !seen.insert((col, row)) {
                    continue;
                }
                size += 1;
                for (dcol, drow) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, 1), (1, -1)] {
                    let next = ((col + dcol + 60) % 60, row + drow);
                    if peaks.contains(&next) && !seen.contains(&next) {
                        stack.push(next);
                    }
                }
            }
            largest = largest.max(size);
        }
        assert!(
            largest >= 8,
            "tectonic ridges should form a chain of at least eight tiles, longest was {largest}",
        );
    }
}
