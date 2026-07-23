//! Tiles and the world map (mirrors civvis/world.py).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use crate::{hex, Pos};

/// A district site that has been placed but has not finished construction.
/// Placement locks both the chosen district and its production cost.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DistrictFoundation {
    pub district: String,
    pub cost: f64,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Tile {
    pub pos: Pos,
    pub terrain: String,
    pub feature: Option<String>,
    pub hills: bool,
    pub resource: Option<String>,
    pub improvement: Option<String>,
    /// Improvements and ordinary districts stop producing yields while
    /// pillaged. City/Encampment defenses keep their dedicated damage state.
    #[serde(default)]
    pub pillaged: bool,
    pub district: Option<String>,
    /// Placed districts occupy their tile and count against district limits,
    /// but do not grant completed-district yields or abilities.
    #[serde(default)]
    pub district_foundation: Option<DistrictFoundation>,
    #[serde(default)]
    pub wonder: Option<String>,
    pub owner_city: Option<u32>,
    #[serde(default)]
    /// River segments on this hex's six edges, in `hex::DIRS` order.
    /// Shared edges are mirrored on both neighboring tiles.
    pub river_edges: [bool; 6],
    /// Coastal cliff segments on this hex's six shared edges. Like rivers,
    /// cliff edges are mirrored onto the neighboring tile so saves and
    /// observations remain self-contained.
    #[serde(default)]
    pub cliff_edges: [bool; 6],
    #[serde(default)]
    // Route level, the shipped PlacementValue ladder: 0 none, 1 Ancient,
    // 2 Medieval, 3 Industrial, 4 Modern, 5 Railroad.
    pub road: u8,
    /// Stock Civ VI continent region, zero-based. Water has no continent.
    #[serde(default)]
    pub continent: Option<usize>,
    /// Permanent Faith added by Great Bath flood mitigation.
    #[serde(default)]
    pub disaster_faith: f64,
    /// Whether this tile is currently suffering a drought's -1 Food effect.
    #[serde(default)]
    pub drought: bool,
    /// Gathering Storm coastal-lowland elevation band (1–3 meters). Zero
    /// means this tile is not vulnerable to sea-level rise.
    #[serde(default)]
    pub coastal_lowland: u8,
    /// A flooded lowland is unusable until its city completes a Flood Barrier.
    #[serde(default)]
    pub flooded: bool,
    /// Submerged lowlands are permanently converted to Coast.
    #[serde(default)]
    pub submerged: bool,
    /// Turn through which a nuclear accident's fallout makes the tile yieldless.
    #[serde(default)]
    pub fallout_until: u32,
}

/// Last tile state actually observed by one player. `owner` is snapshotted
/// separately because a tile stores its owning city ID, while ownership of
/// that city can change outside the observer's current vision.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct RememberedTile {
    pub tile: Tile,
    pub owner: Option<usize>,
    #[serde(default)]
    pub seen_turn: u32,
}

/// JSON cannot directly encode tuple-keyed maps. Keep fast position lookup at
/// runtime while serializing player map memory as a stable list of snapshots.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "Vec<RememberedTile>", into = "Vec<RememberedTile>")]
pub struct TileMemory {
    tiles: std::sync::Arc<BTreeMap<Pos, RememberedTile>>,
    /// When each remembered tile was last actually looked at.
    ///
    /// This moves every turn while the tiles themselves almost never do, so
    /// it is kept out of the shared map. Restamping it in place would copy
    /// every remembered tile — a few hundred hexes, each with its own
    /// strings — which is most of what refreshing a seat's map used to cost.
    stamps: BTreeMap<Pos, u32>,
}

impl TileMemory {
    /// The turn a tile was last seen on, or zero for one never seen.
    pub fn seen_turn(&self, position: &Pos) -> u32 {
        self.stamps.get(position).copied().unwrap_or_default()
    }

    /// Note that a remembered tile was looked at again. Cheap: it does not
    /// touch the shared map.
    pub fn mark_seen(&mut self, position: Pos, turn: u32) {
        if self.tiles.contains_key(&position) {
            self.stamps.insert(position, turn);
        }
    }

    /// Record what a tile looks like now.
    pub fn remember(&mut self, position: Pos, tile: RememberedTile, turn: u32) {
        std::sync::Arc::make_mut(&mut self.tiles).insert(position, tile);
        self.stamps.insert(position, turn);
    }

    pub fn forget_all(&mut self) {
        std::sync::Arc::make_mut(&mut self.tiles).clear();
        self.stamps.clear();
    }
}

impl Deref for TileMemory {
    type Target = BTreeMap<Pos, RememberedTile>;

    fn deref(&self) -> &Self::Target {
        &self.tiles
    }
}

/// Taking a mutable borrow is what copies the memory, so a player's
/// last-known map is shared until somebody writes to it.
///
/// A game is cloned to look ahead — that is what this engine exists for — and
/// a player's remembered map is the largest thing in it: a tile for every hex
/// they have ever seen, each with its own strings. Copying fifteen of those
/// per branch was about half the cost of cloning a game.
impl DerefMut for TileMemory {
    fn deref_mut(&mut self) -> &mut Self::Target {
        std::sync::Arc::make_mut(&mut self.tiles)
    }
}

impl From<Vec<RememberedTile>> for TileMemory {
    fn from(tiles: Vec<RememberedTile>) -> Self {
        let stamps = tiles
            .iter()
            .map(|remembered| (remembered.tile.pos, remembered.seen_turn))
            .collect();
        TileMemory {
            tiles: std::sync::Arc::new(
                tiles
                    .into_iter()
                    .map(|remembered| (remembered.tile.pos, remembered))
                    .collect(),
            ),
            stamps,
        }
    }
}

/// A save carries the stamp on each tile, which is where it used to live, so
/// the two are put back together on the way out.
impl From<TileMemory> for Vec<RememberedTile> {
    fn from(memory: TileMemory) -> Self {
        let stamps = memory.stamps;
        let restamp = |mut remembered: RememberedTile| {
            remembered.seen_turn = stamps
                .get(&remembered.tile.pos)
                .copied()
                .unwrap_or(remembered.seen_turn);
            remembered
        };
        match std::sync::Arc::try_unwrap(memory.tiles) {
            Ok(tiles) => tiles.into_values().map(restamp).collect(),
            Err(shared) => shared.values().cloned().map(restamp).collect(),
        }
    }
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
            pillaged: false,
            district: None,
            district_foundation: None,
            wonder: None,
            owner_city: None,
            river_edges: [false; 6],
            cliff_edges: [false; 6],
            road: 0,
            continent: None,
            disaster_faith: 0.0,
            drought: false,
            coastal_lowland: 0,
            flooded: false,
            submerged: false,
            fallout_until: 0,
        }
    }
}

/// Dense storage for the world's hexes.
///
/// Every position on the cylinder maps to exactly one offset column/row, so
/// the map is a rectangle with no holes and a tile lookup can be a pair of
/// array reads instead of a balanced-tree descent. Tile access sits under
/// essentially every rule in the engine, which made the old `BTreeMap<Pos,
/// Tile>` one of the hottest structures in a simulated turn.
///
/// `tiles` is kept sorted by `Pos` so iteration matches the ordering the map
/// has always had — saves, observations, and per-seed determinism all depend
/// on it — while `slot` indexes that vector by offset coordinates.
#[derive(Clone, Default)]
pub struct TileGrid {
    width: i32,
    height: i32,
    /// Bumped by every route that can write to a tile. Anything that caches a
    /// conclusion drawn from the map — what a unit can see, say — records the
    /// epoch it was drawn under and recomputes when the map has moved on.
    epoch: u64,
    /// Shared until written to. A game cloned to look ahead usually never
    /// touches the map at all — units move, tiles do not — so the hexes are
    /// copied only when something actually changes one.
    tiles: std::sync::Arc<Vec<Tile>>,
    /// `row * width + col` -> index into `tiles`, or `u32::MAX` when a save
    /// omitted that hex.
    slot: Vec<u32>,
}

const EMPTY_SLOT: u32 = u32::MAX;

impl TileGrid {
    pub fn new(width: i32, height: i32) -> TileGrid {
        let mut grid = TileGrid {
            width,
            height,
            epoch: 0,
            tiles: std::sync::Arc::new(Vec::new()),
            slot: Vec::new(),
        };
        let mut tiles = Vec::with_capacity((width.max(0) * height.max(0)) as usize);
        for row in 0..height {
            for col in 0..width {
                tiles.push(Tile::new(hex::offset_to_axial(col, row)));
            }
        }
        grid.rebuild(tiles);
        grid
    }

    fn from_tiles(width: i32, height: i32, tiles: Vec<Tile>) -> TileGrid {
        let mut grid = TileGrid {
            width,
            height,
            epoch: 0,
            tiles: std::sync::Arc::new(Vec::new()),
            slot: Vec::new(),
        };
        grid.rebuild(tiles);
        grid
    }

    fn rebuild(&mut self, mut tiles: Vec<Tile>) {
        self.epoch += 1;
        tiles.sort_unstable_by_key(|tile| tile.pos);
        tiles.dedup_by_key(|tile| tile.pos);
        let cells = (self.width.max(0) as usize) * (self.height.max(0) as usize);
        self.slot = vec![EMPTY_SLOT; cells];
        for (index, tile) in tiles.iter().enumerate() {
            if let Some(cell) = self.cell(tile.pos) {
                self.slot[cell] = index as u32;
            }
        }
        self.tiles = std::sync::Arc::new(tiles);
    }

    #[inline]
    fn cell(&self, pos: Pos) -> Option<usize> {
        let (col, row) = hex::axial_to_offset(pos.0, pos.1);
        if col < 0 || col >= self.width || row < 0 || row >= self.height {
            return None;
        }
        Some((row * self.width + col) as usize)
    }

    /// Where a position sits in the tile vector. Callers that keep their own
    /// per-tile table — a visibility sweep's height cache, say — index it by
    /// this, so the table is dense and in the same order as the map itself.
    #[inline]
    pub fn index_of(&self, pos: Pos) -> Option<usize> {
        let slot = *self.slot.get(self.cell(pos)?)?;
        if slot == EMPTY_SLOT {
            None
        } else {
            Some(slot as usize)
        }
    }

    #[inline]
    pub fn get(&self, pos: &Pos) -> Option<&Tile> {
        self.index_of(*pos).map(|index| &self.tiles[index])
    }

    #[inline]
    pub fn get_mut(&mut self, pos: &Pos) -> Option<&mut Tile> {
        self.epoch += 1;
        let index = self.index_of(*pos)?;
        Some(&mut std::sync::Arc::make_mut(&mut self.tiles)[index])
    }

    /// How many times the map has been opened for writing. Two reads of the
    /// same epoch saw the same map.
    #[inline]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    #[inline]
    pub fn contains_key(&self, pos: &Pos) -> bool {
        self.index_of(*pos).is_some()
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &Pos> + ExactSizeIterator + '_ {
        self.tiles.iter().map(|tile| &tile.pos)
    }

    pub fn values(&self) -> std::slice::Iter<'_, Tile> {
        self.tiles.iter()
    }

    pub fn values_mut(&mut self) -> std::slice::IterMut<'_, Tile> {
        self.epoch += 1;
        std::sync::Arc::make_mut(&mut self.tiles).iter_mut()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Pos, &Tile)> + '_ {
        self.tiles.iter().map(|tile| (&tile.pos, tile))
    }

    pub fn into_values(self) -> std::vec::IntoIter<Tile> {
        match std::sync::Arc::try_unwrap(self.tiles) {
            Ok(tiles) => tiles.into_iter(),
            Err(shared) => shared.as_slice().to_vec().into_iter(),
        }
    }
}

impl<'a> IntoIterator for &'a TileGrid {
    type Item = (&'a Pos, &'a Tile);
    type IntoIter = std::iter::Map<std::slice::Iter<'a, Tile>, fn(&'a Tile) -> (&'a Pos, &'a Tile)>;

    fn into_iter(self) -> Self::IntoIter {
        self.tiles.iter().map(|tile| (&tile.pos, tile))
    }
}

/// Mutable iteration hands back an owned `Pos`: the position lives inside the
/// tile, so it cannot be lent out immutably while the tile itself is lent out
/// mutably.
impl<'a> IntoIterator for &'a mut TileGrid {
    type Item = (Pos, &'a mut Tile);
    type IntoIter =
        std::iter::Map<std::slice::IterMut<'a, Tile>, fn(&'a mut Tile) -> (Pos, &'a mut Tile)>;

    fn into_iter(self) -> Self::IntoIter {
        self.epoch += 1;
        std::sync::Arc::make_mut(&mut self.tiles)
            .iter_mut()
            .map(|tile| (tile.pos, tile))
    }
}

impl std::ops::Index<&Pos> for TileGrid {
    type Output = Tile;

    #[inline]
    fn index(&self, pos: &Pos) -> &Tile {
        self.get(pos).expect("tile position outside the world map")
    }
}

impl PartialEq for TileGrid {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width && self.height == other.height && self.tiles == other.tiles
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "WorldMapSer", into = "WorldMapSer")]
pub struct WorldMap {
    pub width: i32,
    pub height: i32,
    pub tiles: TileGrid,
}

#[derive(Clone, Serialize, Deserialize)]
struct WorldMapSer {
    width: i32,
    height: i32,
    tiles: Vec<Tile>,
}

impl From<WorldMapSer> for WorldMap {
    fn from(s: WorldMapSer) -> WorldMap {
        WorldMap {
            width: s.width,
            height: s.height,
            tiles: TileGrid::from_tiles(s.width, s.height, s.tiles),
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
        WorldMap {
            width,
            height,
            tiles: TileGrid::new(width, height),
        }
    }

    #[inline]
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

    /// Add or remove a coastal cliff on the shared edge between two tiles.
    pub fn set_cliff_edge(&mut self, a: Pos, b: Pos, present: bool) -> bool {
        let Some(direction) = self.direction_to(a, b) else {
            return false;
        };
        if !self.tiles.contains_key(&a) || !self.tiles.contains_key(&b) {
            return false;
        }
        self.tiles.get_mut(&a).unwrap().cliff_edges[direction] = present;
        self.tiles.get_mut(&b).unwrap().cliff_edges[(direction + 3) % 6] = present;
        true
    }

    pub fn has_cliff_edge(&self, a: Pos, b: Pos) -> bool {
        self.direction_to(a, b)
            .and_then(|direction| self.tiles.get(&a).map(|t| t.cliff_edges[direction]))
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

/// A set of tiles held as one bit each.
///
/// Visibility is unioned constantly — every unit's view into its owner's,
/// every ally's into the alliance's — and doing that through a `BTreeSet` of
/// positions meant an allocation and a tree descent per tile. Bits are
/// indexed by [`TileGrid::index_of`], which runs in position order, so
/// reading a `TileBits` back out yields tiles already sorted.
#[derive(Clone, Default, PartialEq)]
pub struct TileBits {
    words: Vec<u64>,
}

impl TileBits {
    pub fn with_capacity(tiles: usize) -> TileBits {
        TileBits {
            words: vec![0; tiles.div_ceil(64)],
        }
    }

    #[inline]
    pub fn insert(&mut self, index: usize) {
        let word = index / 64;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= 1 << (index % 64);
    }

    #[inline]
    pub fn contains(&self, index: usize) -> bool {
        self.words
            .get(index / 64)
            .is_some_and(|word| word & (1 << (index % 64)) != 0)
    }

    pub fn union_with(&mut self, other: &TileBits) {
        if self.words.len() < other.words.len() {
            self.words.resize(other.words.len(), 0);
        }
        for (into, from) in self.words.iter_mut().zip(&other.words) {
            *into |= *from;
        }
    }

    pub fn clear(&mut self) {
        self.words.iter_mut().for_each(|word| *word = 0);
    }

    /// Whether every tile in this set is also in `other`.
    pub fn is_subset_of(&self, other: &TileBits) -> bool {
        self.words
            .iter()
            .enumerate()
            .all(|(slot, word)| word & !other.words.get(slot).copied().unwrap_or(0) == 0)
    }

    /// The set members in ascending index order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(slot, word)| {
            let mut bits = *word;
            std::iter::from_fn(move || {
                if bits == 0 {
                    return None;
                }
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                Some(slot * 64 + bit)
            })
        })
    }
}

impl TileGrid {
    /// The position at a tile index, as handed out by [`TileGrid::index_of`].
    #[inline]
    pub fn pos_at(&self, index: usize) -> Option<Pos> {
        self.tiles.get(index).map(|tile| tile.pos)
    }

    /// The positions in a bit set, in map order.
    pub fn positions(&self, bits: &TileBits) -> impl Iterator<Item = Pos> + '_ {
        bits.iter()
            .filter_map(|index| self.tiles.get(index).map(|tile| tile.pos))
            .collect::<Vec<_>>()
            .into_iter()
    }
}
