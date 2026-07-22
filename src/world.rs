//! Tiles and the world map (mirrors civvis/world.py).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{hex, Pos};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Tile {
    pub pos: Pos,
    pub terrain: String,
    pub feature: Option<String>,
    pub hills: bool,
    pub resource: Option<String>,
    pub improvement: Option<String>,
    pub district: Option<String>,
    pub owner_city: Option<u32>,
    #[serde(default)]
    /// River segments on this hex's six edges, in `hex::DIRS` order.
    /// Shared edges are mirrored on both neighboring tiles.
    pub river_edges: [bool; 6],
    #[serde(default)]
    pub road: bool,
    /// Stock Civ VI continent region, zero-based. Water has no continent.
    #[serde(default)]
    pub continent: Option<usize>,
}

impl Tile {
    pub fn new(pos: Pos) -> Tile {
        Tile {
            pos,
            terrain: "ocean".to_string(),
            feature: None,
            hills: false,
            resource: None,
            improvement: None,
            district: None,
            owner_city: None,
            river_edges: [false; 6],
            road: false,
            continent: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "WorldMapSer", into = "WorldMapSer")]
pub struct WorldMap {
    pub width: i32,
    pub height: i32,
    pub tiles: BTreeMap<Pos, Tile>,
}

#[derive(Clone, Serialize, Deserialize)]
struct WorldMapSer {
    width: i32,
    height: i32,
    tiles: Vec<Tile>,
}

impl From<WorldMapSer> for WorldMap {
    fn from(s: WorldMapSer) -> WorldMap {
        let tiles = s.tiles.into_iter().map(|t| (t.pos, t)).collect();
        WorldMap {
            width: s.width,
            height: s.height,
            tiles,
        }
    }
}

impl From<WorldMap> for WorldMapSer {
    fn from(m: WorldMap) -> WorldMapSer {
        WorldMapSer {
            width: m.width,
            height: m.height,
            tiles: m.tiles.into_values().collect(),
        }
    }
}

impl WorldMap {
    pub fn new(width: i32, height: i32) -> WorldMap {
        let mut tiles = BTreeMap::new();
        for row in 0..height {
            for col in 0..width {
                let pos = hex::offset_to_axial(col, row);
                tiles.insert(pos, Tile::new(pos));
            }
        }
        WorldMap {
            width,
            height,
            tiles,
        }
    }

    pub fn get(&self, pos: Pos) -> Option<&Tile> {
        self.tiles.get(&pos)
    }

    /// Direction index from one adjacent tile to another, accounting for the
    /// east-west cylindrical seam.
    pub fn direction_to(&self, from: Pos, to: Pos) -> Option<usize> {
        hex::neighbors(from)
            .into_iter()
            .map(|p| hex::canon(p, self.width))
            .position(|p| p == to)
    }

    /// Add or remove the river segment shared by two adjacent tiles.
    /// Returns false when either tile is absent or the positions are not
    /// adjacent. Keeping both edge masks in sync makes saves and observations
    /// self-contained tile by tile.
    pub fn set_river_edge(&mut self, a: Pos, b: Pos, present: bool) -> bool {
        let Some(direction) = self.direction_to(a, b) else {
            return false;
        };
        if !self.tiles.contains_key(&a) || !self.tiles.contains_key(&b) {
            return false;
        }
        self.tiles.get_mut(&a).unwrap().river_edges[direction] = present;
        self.tiles.get_mut(&b).unwrap().river_edges[(direction + 3) % 6] = present;
        true
    }

    /// Whether the shared boundary between two adjacent tiles carries a river.
    pub fn has_river_edge(&self, a: Pos, b: Pos) -> bool {
        self.direction_to(a, b)
            .and_then(|direction| self.tiles.get(&a).map(|t| t.river_edges[direction]))
            .unwrap_or(false)
    }

    pub fn clear_rivers(&mut self) {
        for tile in self.tiles.values_mut() {
            tile.river_edges = [false; 6];
        }
    }
}

impl Tile {
    pub fn has_river(&self) -> bool {
        self.river_edges.iter().any(|edge| *edge)
    }
}
