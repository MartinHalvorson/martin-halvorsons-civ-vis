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
    pub river: bool,
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
            river: false,
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
        WorldMap { width: s.width, height: s.height, tiles }
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
        WorldMap { width, height, tiles }
    }

    pub fn get(&self, pos: Pos) -> Option<&Tile> {
        self.tiles.get(&pos)
    }
}
