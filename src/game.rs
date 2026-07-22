//! Core turn engine (mirrors civvis/game.py — same mechanics and action protocol).
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};

use crate::rng::Rng;
use crate::rules::{Rules, Yields};
use crate::setup::MapSize;
use crate::world::WorldMap;
use crate::{hex, mapgen, Pos};

pub const CIV_NAMES: [&str; 8] = ["Rome", "Egypt", "Greece", "China", "Sumeria",
                                  "Aztec", "Nubia", "Scythia"];
pub const CITY_STATE_NAMES: [&str; 12] = ["Kabul", "Geneva", "Carthage", "Hattusa",
                                          "Mohenjo-Daro", "Yerevan", "Zanzibar",
                                          "Auckland", "Valletta", "Vilnius",
                                          "Stockholm", "Kandy"];

fn city_names(civ: &str) -> &'static [&'static str] {
    match civ {
        "Rome" => &["Rome", "Ostia", "Antium", "Ravenna"],
        "Egypt" => &["Thebes", "Memphis", "Akhetaten", "Giza"],
        "Greece" => &["Athens", "Sparta", "Corinth", "Argos"],
        "China" => &["Xian", "Chengdu", "Luoyang", "Kaifeng"],
        "Sumeria" => &["Uruk", "Ur", "Nippur", "Lagash"],
        "Aztec" => &["Tenochtitlan", "Texcoco", "Tlatelolco", "Xochimilco"],
        "Nubia" => &["Meroe", "Kerma", "Napata", "Dongola"],
        "Scythia" => &["Pokrovka", "Gelonos", "Kamenka", "Aktau"],
        _ => &[],
    }
}

pub fn growth_threshold(pop: i32) -> f64 {
    15.0 + 8.0 * (pop - 1) as f64 + ((pop - 1) as f64).powf(1.5).trunc()
}

/// Gathering Storm victory thresholds on standard speed.
pub const DIPLOMATIC_VICTORY_POINTS: i64 = 20;
pub const EXOPLANET_DESTINATION: f64 = 50.0;
pub const TOURISM_PER_VISITOR: f64 = 200.0;

pub fn effective_strength(base: f64, hp: i32) -> f64 {
    let wounded_penalty = (10.0 - hp.clamp(0, 100) as f64 / 10.0).round();
    (base - wounded_penalty).max(0.0)
}

pub fn damage(att: f64, def: f64, rng: &mut Rng) -> i32 {
    let d = 30.0 * ((att - def) / 25.0).exp() * rng.uniform(0.8, 1.2);
    (d.round() as i32).clamp(1, 100)
}

fn pair(a: usize, b: usize) -> (usize, usize) {
    (a.min(b), a.max(b))
}

impl Game {
    /// Stock setup profile governing this world. Exact stock dimensions win;
    /// custom maps fall back to the profile for their major-player count.
    pub fn map_size(&self) -> &'static MapSize {
        MapSize::from_dimensions(self.map.width, self.map.height)
            .unwrap_or_else(|| {
                let majors = self.players.iter().filter(|p| !p.is_minor).count();
                MapSize::for_players(majors)
            })
    }

    pub fn max_religions(&self) -> usize {
        self.map_size().max_religions
    }

    /// Wrapped hex distance (the world is an east-west cylinder).
    pub fn wdist(&self, a: Pos, b: Pos) -> i32 {
        hex::wdistance(a, b, self.map.width)
    }

    /// Canonicalized in-map neighbors across the wrap seam.
    pub fn nbrs(&self, p: Pos) -> Vec<Pos> {
        crate::hex::neighbors(p)
            .into_iter()
            .map(|n| hex::canon(n, self.map.width))
            .filter(|n| self.map.tiles.contains_key(n))
            .collect()
    }

    /// Canonicalized in-map disk across the wrap seam.
    pub fn wdisk(&self, c: Pos, r: i32) -> Vec<Pos> {
        let mut v: Vec<Pos> = crate::hex::disk(c, r)
            .into_iter()
            .map(|p| hex::canon(p, self.map.width))
            .filter(|p| self.map.tiles.contains_key(p))
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

fn bump(p: &mut Player, key: &str) {
    *p.counters.entry(key.to_string()).or_insert(0) += 1;
}

// ------------------------------------------------------------------ entities

fn lvl1() -> i32 {
    1
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Unit {
    pub id: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub owner: usize,
    pub pos: Pos,
    pub hp: i32,
    pub moves_left: f64,
    pub charges: i32,
    #[serde(default)]
    pub xp: i64,
    #[serde(default = "lvl1")]
    pub level: i32,
    #[serde(default)]
    pub fortified: bool,
    /// Consecutive inactive/fortified turns, capped at two for +3/+6 CS.
    #[serde(default)]
    pub fortify_turns: i32,
    /// Whether the unit moved or acted since its last turn began. Healing and
    /// siege setup both depend on this rather than on remaining movement.
    #[serde(default)]
    pub acted: bool,
    #[serde(default)]
    pub zoc_stopped: bool,
}

/// The four location classes used for passive unit healing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealingLocation {
    District,
    FriendlyTerritory,
    NeutralTerritory,
    EnemyTerritory,
}

impl HealingLocation {
    pub fn rate(self) -> i32 {
        match self {
            HealingLocation::District => 20,
            HealingLocation::FriendlyTerritory => 15,
            HealingLocation::NeutralTerritory => 10,
            HealingLocation::EnemyTerritory => 5,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(untagged)]
pub enum Item {
    Unit { unit: String },
    Building { building: String },
    District { district: String, pos: Pos },
    Project { project: String },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct City {
    pub id: u32,
    pub name: String,
    pub owner: usize,
    pub pos: Pos,
    pub pop: i32,
    pub food: f64,
    pub production: f64,
    pub border_culture: f64,
    pub hp: i32,
    pub buildings: Vec<String>,
    pub districts: BTreeMap<String, Pos>,
    pub owned_tiles: Vec<Pos>,
    pub queue: Vec<Item>,
    pub original_owner: usize,
    pub is_capital: bool,
    #[serde(default)]
    pub struck: bool,
    /// Outer-defense pool from walls (Civ 6); -1 in old saves = derive on load.
    #[serde(default = "wall_unset")]
    pub wall_hp: i32,
    #[serde(default)]
    pub last_attacked: u32,
    #[serde(default)]
    pub pressure: BTreeMap<String, f64>, // religious pressure by religion
    #[serde(default = "full_loyalty")]
    pub loyalty: f64,
}

/// The priorities used by a city's automatic citizen governor.  These are
/// deliberately observable: agents and the browser can explain why a tile is
/// being worked instead of treating city yields as a hidden heuristic.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CitizenStrategy {
    pub focus: String,
    pub weights: Yields,
    /// Total food the governor tries to collect, including the city center.
    /// It always covers consumption when the owned tiles make that possible;
    /// surplus is requested only when housing and amenities support growth.
    pub food_target: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CitizenPlan {
    pub strategy: CitizenStrategy,
    /// Tiles worked by population. The free city-center tile is not included.
    pub worked_tiles: Vec<Pos>,
}

fn full_loyalty() -> f64 {
    100.0
}

fn wall_unset() -> i32 {
    -1
}

fn normal_age() -> String {
    "normal".to_string()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Player {
    pub id: usize,
    pub civ: String,
    pub techs: BTreeSet<String>,
    pub research: Option<String>,
    pub research_progress: f64,
    pub research_overflow: f64,
    pub civics: BTreeSet<String>,
    pub civic: Option<String>,
    pub civic_progress: f64,
    pub civic_overflow: f64,
    pub gold: f64,
    pub faith: f64,
    pub explored: BTreeSet<Pos>,
    pub alive: bool,
    pub is_minor: bool,
    #[serde(default)]
    pub is_barbarian: bool,
    #[serde(default)]
    pub government: Option<String>,
    #[serde(default)]
    pub policies: BTreeSet<String>,
    #[serde(default)]
    pub influence: f64,
    #[serde(default)]
    pub envoys_free: i64,
    #[serde(default)]
    pub gpp: BTreeMap<String, f64>, // great person points by type
    #[serde(default)]
    pub gp_claimed: BTreeMap<String, i64>,
    #[serde(default)]
    pub pantheon: Option<String>,
    #[serde(default)]
    pub religion: Option<String>,
    #[serde(default)]
    pub religion_beliefs: Vec<String>,
    #[serde(default)]
    pub prophet_pending: bool,
    #[serde(default)]
    pub era_score: i64,
    #[serde(default)]
    pub governors: Vec<u32>, // city ids with an established governor
    #[serde(default)]
    pub dvp: i64, // diplomatic victory points
    #[serde(default = "normal_age")]
    pub age: String,
    #[serde(default)]
    pub culture_lifetime: f64,
    #[serde(default)]
    pub tourism_lifetime: f64,
    /// Completed one-time space-race projects.
    #[serde(default)]
    pub science_projects: BTreeSet<String>,
    /// Light-years travelled after launching the Exoplanet Expedition.
    #[serde(default)]
    pub exoplanet_distance: f64,
    #[serde(default)]
    pub envoys: Vec<(usize, i64)>, // (city-state pid, envoys placed)
    #[serde(default)]
    pub counters: BTreeMap<String, i64>,
    #[serde(default)]
    pub boosted_techs: BTreeSet<String>,
    #[serde(default)]
    pub boosted_civics: BTreeSet<String>,
}

impl Player {
    fn new(id: usize, civ: &str, is_minor: bool) -> Player {
        Player {
            id,
            civ: civ.to_string(),
            techs: BTreeSet::new(),
            research: None,
            research_progress: 0.0,
            research_overflow: 0.0,
            civics: BTreeSet::new(),
            civic: None,
            civic_progress: 0.0,
            civic_overflow: 0.0,
            gold: 0.0,
            faith: 0.0,
            explored: BTreeSet::new(),
            alive: true,
            is_minor,
            is_barbarian: false,
            government: None,
            policies: BTreeSet::new(),
            influence: 0.0,
            envoys_free: 0,
            gpp: BTreeMap::new(),
            gp_claimed: BTreeMap::new(),
            pantheon: None,
            religion: None,
            religion_beliefs: Vec::new(),
            prophet_pending: false,
            era_score: 0,
            governors: Vec::new(),
            dvp: 0,
            age: "normal".to_string(),
            culture_lifetime: 0.0,
            tourism_lifetime: 0.0,
            science_projects: BTreeSet::new(),
            exoplanet_distance: 0.0,
            envoys: Vec::new(),
            counters: BTreeMap::new(),
            boosted_techs: BTreeSet::new(),
            boosted_civics: BTreeSet::new(),
        }
    }
}

// ------------------------------------------------------------------- actions

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Move { unit: u32, to: Pos },
    MoveTo { unit: u32, to: Pos },
    Attack { unit: u32, target: Pos },
    Ranged { unit: u32, target: Pos },
    FoundCity { unit: u32 },
    Improve { unit: u32, improvement: String },
    Produce { city: u32, item: Item },
    Buy { city: u32, unit: String, #[serde(default = "gold_s")] currency: String },
    Research { tech: String },
    Civic { civic: String },
    DeclareWar { player: usize },
    MakePeace { player: usize },
    Fortify { unit: u32 },
    Government { government: String },
    SlotPolicy { policy: String },
    UnslotPolicy { policy: String },
    TradeRoute { unit: u32, city: u32 },
    SendEnvoy { player: usize },
    ChoosePantheon { belief: String },
    AssignGovernor { city: u32 },
    FoundReligion { follower: String, founder: String },
    Spread { unit: u32 },
    CityStrike { city: u32, target: Pos },
    EndTurn,
}

fn gold_s() -> String {
    "gold".to_string()
}

// --------------------------------------------------------------------- game

#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "GameSer", into = "GameSer")]
pub struct Game {
    pub rules: Rules,
    pub rng: Rng,
    pub seed: u64,
    pub max_turns: u32,
    pub turn: u32,
    pub current: usize,
    pub winner: Option<usize>,
    pub victory_type: Option<String>,
    pub next_id: u32,
    pub map: WorldMap,
    pub players: Vec<Player>,
    pub units: BTreeMap<u32, Unit>,
    pub cities: BTreeMap<u32, City>,
    pub at_war: BTreeSet<(usize, usize)>,
    pub barb_pid: Option<usize>,
    pub barb_camps: BTreeMap<Pos, u32>,
    pub routes: Vec<TradeRoute>,
    pub world_era: usize,
    occ: BTreeMap<Pos, Vec<u32>>,
    city_by_pos: BTreeMap<Pos, u32>,
    /// Every successfully applied action, in order — the game is exactly
    /// f(seed+params, log), so this is the replay/desync-detection record.
    /// Runtime-only (not in saves yet).
    pub log: Vec<(usize, Action)>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct TradeRoute {
    pub origin: u32,
    pub dest: u32,
    pub owner: usize,
    pub ends: u32,
}

#[derive(Clone, Serialize, Deserialize)]
struct GameSer {
    seed: u64,
    max_turns: u32,
    turn: u32,
    current: usize,
    winner: Option<usize>,
    victory_type: Option<String>,
    next_id: u32,
    rng: Rng,
    at_war: Vec<(usize, usize)>,
    #[serde(default)]
    barb_pid: Option<usize>,
    #[serde(default)]
    barb_camps: Vec<(Pos, u32)>,
    #[serde(default)]
    routes: Vec<TradeRoute>,
    #[serde(default)]
    world_era: usize,
    map: WorldMap,
    players: Vec<Player>,
    units: Vec<Unit>,
    cities: Vec<City>,
}

impl From<GameSer> for Game {
    fn from(s: GameSer) -> Game {
        let mut g = Game {
            rules: Rules::embedded(),
            rng: s.rng,
            seed: s.seed,
            max_turns: s.max_turns,
            turn: s.turn,
            current: s.current,
            winner: s.winner,
            victory_type: s.victory_type,
            next_id: s.next_id,
            map: s.map,
            players: s.players,
            units: s.units.into_iter().map(|u| (u.id, u)).collect(),
            cities: s.cities.into_iter().map(|c| (c.id, c)).collect(),
            at_war: s.at_war.into_iter().collect(),
            barb_pid: s.barb_pid,
            barb_camps: s.barb_camps.into_iter().collect(),
            routes: s.routes,
            world_era: s.world_era,
            occ: BTreeMap::new(),
            city_by_pos: BTreeMap::new(),
            log: Vec::new(),
        };
        for u in g.units.values() {
            g.occ.entry(u.pos).or_default().push(u.id);
        }
        for c in g.cities.values() {
            g.city_by_pos.insert(c.pos, c.id);
        }
        let legacy: Vec<u32> = g.cities.values()
            .filter(|c| c.wall_hp < 0).map(|c| c.id).collect();
        for cid in legacy {
            let max = g.city_max_wall_hp(&g.cities[&cid]);
            g.cities.get_mut(&cid).unwrap().wall_hp = max;
        }
        g
    }
}

impl From<Game> for GameSer {
    fn from(g: Game) -> GameSer {
        GameSer {
            seed: g.seed,
            max_turns: g.max_turns,
            turn: g.turn,
            current: g.current,
            winner: g.winner,
            victory_type: g.victory_type,
            next_id: g.next_id,
            rng: g.rng,
            at_war: g.at_war.into_iter().collect(),
            barb_pid: g.barb_pid,
            barb_camps: g.barb_camps.into_iter().collect(),
            routes: g.routes,
            world_era: g.world_era,
            map: g.map,
            players: g.players,
            units: g.units.into_values().collect(),
            cities: g.cities.into_values().collect(),
        }
    }
}

impl Game {
    pub fn new(num_players: usize, width: i32, height: i32, seed: u64,
               max_turns: u32, num_city_states: usize) -> Game {
        Game::new_full(num_players, width, height, seed, max_turns,
                       num_city_states, true)
    }

    pub fn new_full(num_players: usize, width: i32, height: i32, seed: u64,
                    max_turns: u32, num_city_states: usize,
                    barbarians: bool) -> Game {
        let rules = Rules::embedded();
        let mut rng = Rng::new(seed);
        let total = num_players + num_city_states;
        let map_size = MapSize::from_dimensions(width, height)
            .unwrap_or_else(|| MapSize::for_players(num_players));
        let (map, spawns) = mapgen::generate(
            &rules,
            width,
            height,
            total,
            map_size.natural_wonders,
            map_size.continents,
            &mut rng,
        );
        let mut g = Game {
            rules,
            rng,
            seed,
            max_turns,
            turn: 1,
            current: 0,
            winner: None,
            victory_type: None,
            next_id: 1,
            map,
            players: Vec::new(),
            units: BTreeMap::new(),
            cities: BTreeMap::new(),
            at_war: BTreeSet::new(),
            barb_pid: None,
            barb_camps: BTreeMap::new(),
            routes: Vec::new(),
            world_era: 0,
            occ: BTreeMap::new(),
            city_by_pos: BTreeMap::new(),
            log: Vec::new(),
        };
        for i in 0..num_players {
            g.players.push(Player::new(i, CIV_NAMES[i % CIV_NAMES.len()], false));
        }
        for (i, pos) in spawns.iter().take(num_players).enumerate() {
            g.spawn_unit("settler", i, *pos);
            g.spawn_unit("warrior", i, *pos);
            g.reveal(i, *pos, 3);
        }
        let major_spawns: Vec<Pos> = spawns.iter().take(num_players).cloned().collect();
        for (i, pos) in spawns.iter().skip(num_players).enumerate() {
            let crowded = major_spawns.iter().any(|s| g.wdist(*pos, *s) < 4)
                || g.cities.values().any(|c| g.wdist(*pos, c.pos) < 4);
            if crowded {
                continue;
            }
            let pid = g.players.len();
            let name = CITY_STATE_NAMES[i % CITY_STATE_NAMES.len()];
            g.players.push(Player::new(pid, name, true));
            g.found_city_for(pid, *pos, Some(name.to_string()));
            g.place_new_unit("warrior", pid, *pos);
            g.place_new_unit("slinger", pid, *pos);
        }
        if barbarians {
            let pid = g.players.len();
            let mut barb = Player::new(pid, "Barbarians", true);
            barb.is_barbarian = true;
            g.players.push(barb);
            g.barb_pid = Some(pid);
            for _ in 0..2 {
                g.spawn_camp();
            }
        }
        g
    }

    fn spawn_camp(&mut self) {
        let mut cands: Vec<Pos> = Vec::new();
        for (pos, t) in &self.map.tiles {
            if self.rules.is_water(t) || !self.rules.is_passable(t) {
                continue;
            }
            if t.owner_city.is_some() || t.improvement.is_some()
                || self.city_by_pos.contains_key(pos) {
                continue;
            }
            if self.cities.values().any(|c| self.wdist(*pos, c.pos) < 4) {
                continue;
            }
            if self.barb_camps.keys().any(|cp| self.wdist(*pos, *cp) < 4) {
                continue;
            }
            cands.push(*pos);
        }
        if cands.is_empty() {
            return;
        }
        let pos = cands[self.rng.below(cands.len())];
        self.map.tiles.get_mut(&pos).unwrap().improvement =
            Some("barbarian_camp".to_string());
        self.barb_camps.insert(pos, self.turn + 2);
    }

    fn barbarian_phase(&mut self) {
        let bpid = match self.barb_pid {
            Some(p) => p,
            None => return,
        };
        let n_majors = self.players.iter().filter(|p| !p.is_minor).count();
        if self.turn % 10 == 0 && self.barb_camps.len() < n_majors + 1 {
            self.spawn_camp();
        }
        let cap = 2 + 2 * self.barb_camps.len();
        let mut n_barb = self.player_unit_ids(bpid).len();
        let era = self.players.iter().filter(|p| !p.is_minor)
            .map(|p| p.techs.len()).max().unwrap_or(1);
        let pool: &[&str] = if era < 8 {
            &["warrior"]
        } else if era < 14 {
            &["warrior", "spearman", "archer"]
        } else if era < 22 {
            &["swordsman", "spearman", "archer"]
        } else {
            &["swordsman", "crossbowman", "pikeman"]
        };
        let camps: Vec<(Pos, u32)> = self.barb_camps.iter()
            .map(|(p, n)| (*p, *n)).collect();
        for (pos, nxt) in camps {
            if self.turn < nxt || n_barb >= cap {
                continue;
            }
            let utype = pool[self.rng.below(pool.len())];
            if self.place_new_unit(utype, bpid, pos).is_some() {
                n_barb += 1;
                self.barb_camps.insert(pos, self.turn + 6);
            }
        }
    }

    /// Tribal village rewards on entering a goody-hut tile (Civ 6-style
    /// lean table: gold, faith, a eureka, or an inspiration).
    fn maybe_goody_hut(&mut self, uid: u32) {
        let (pos, owner) = match self.units.get(&uid) {
            Some(u) => (u.pos, u.owner),
            None => return,
        };
        if self.players[owner].is_barbarian {
            return;
        }
        let hut = self.map.get(pos)
            .map(|t| t.improvement.as_deref() == Some("goody_hut"))
            .unwrap_or(false);
        if !hut {
            return;
        }
        self.map.tiles.get_mut(&pos).unwrap().improvement = None;
        match self.rng.below(4) {
            0 => self.players[owner].gold += 60.0,
            1 => self.players[owner].faith += 20.0,
            2 => self.grant_random_boosts(owner, 1, true),
            _ => self.grant_random_boosts(owner, 1, false),
        }
    }

    fn maybe_clear_camp(&mut self, uid: u32) {
        let (pos, owner, kind) = {
            let u = &self.units[&uid];
            (u.pos, u.owner, u.kind.clone())
        };
        if self.barb_camps.contains_key(&pos) && Some(owner) != self.barb_pid
            && self.rules.units[kind.as_str()].class == "military" {
            self.barb_camps.remove(&pos);
            let t = self.map.tiles.get_mut(&pos).unwrap();
            if t.improvement.as_deref() == Some("barbarian_camp") {
                t.improvement = None;
            }
            self.players[owner].gold += 50.0;
            self.players[owner].era_score += 1;
            if self.has_ability(owner, "epic_quest") {
                match self.rng.below(4) {
                    0 => self.players[owner].gold += 60.0,
                    1 => self.players[owner].faith += 20.0,
                    2 => self.grant_random_boosts(owner, 1, true),
                    _ => self.grant_random_boosts(owner, 1, false),
                }
            }
            bump(&mut self.players[owner], "camps");
        }
    }

    // ------------------------------------------------------------- queries

    pub fn city_at(&self, pos: Pos) -> Option<u32> {
        self.city_by_pos.get(&pos).copied()
    }

    pub fn units_at(&self, pos: Pos) -> Vec<u32> {
        self.occ.get(&pos).cloned().unwrap_or_default()
    }

    pub fn player_unit_ids(&self, pid: usize) -> Vec<u32> {
        self.units.values().filter(|u| u.owner == pid).map(|u| u.id).collect()
    }

    pub fn player_city_ids(&self, pid: usize) -> Vec<u32> {
        self.cities.values().filter(|c| c.owner == pid).map(|c| c.id).collect()
    }

    pub fn is_at_war(&self, a: usize, b: usize) -> bool {
        if a == b {
            return false;
        }
        if let Some(bp) = self.barb_pid {
            if bp == a || bp == b {
                return true;
            }
        }
        self.at_war.contains(&pair(a, b))
    }

    /// Classify a tile from a unit owner's perspective for passive healing.
    /// Districts use the district rate, while any foreign civilization's
    /// territory is rival territory even when Open Borders or peace applies.
    pub fn healing_location(&self, owner: usize, pos: Pos) -> HealingLocation {
        let tile = self.map.get(pos);
        let territory_owner = tile
            .and_then(|tile| tile.owner_city)
            .and_then(|cid| self.cities.get(&cid))
            .map(|city| city.owner);
        if self.city_at(pos).is_some()
            || tile.and_then(|tile| tile.district.as_ref()).is_some()
        {
            return HealingLocation::District;
        }
        match territory_owner {
            Some(tile_owner) if tile_owner == owner => HealingLocation::FriendlyTerritory,
            Some(tile_owner) if self.suzerain_of(tile_owner) == Some(owner) => {
                HealingLocation::FriendlyTerritory
            }
            Some(_) => HealingLocation::EnemyTerritory,
            None => HealingLocation::NeutralTerritory,
        }
    }

    pub fn unit_heal_rate(&self, uid: u32) -> i32 {
        let unit = &self.units[&uid];
        let location = self.healing_location(unit.owner, unit.pos);
        let naval_or_embarked = self.rules.units[unit.kind.as_str()]
            .domain
            .as_deref()
            == Some("sea")
            || self.is_embarked(unit);
        if naval_or_embarked {
            let friendly = self
                .map
                .get(unit.pos)
                .and_then(|tile| tile.owner_city)
                .and_then(|cid| self.cities.get(&cid))
                .is_some_and(|city| {
                    city.owner == unit.owner || self.suzerain_of(city.owner) == Some(unit.owner)
                });
            if friendly {
                location.rate()
            } else {
                0
            }
        } else {
            location.rate()
        }
    }

    pub fn gov_effects(&self, pid: usize) -> crate::rules::GovEffects {
        match &self.players[pid].government {
            Some(g) => self.rules.governments.get(g)
                .map(|s| s.effects).unwrap_or_default(),
            None => Default::default(),
        }
    }

    pub fn has_policy(&self, pid: usize, name: &str) -> bool {
        self.players[pid].policies.contains(name)
    }

    /// Leader/civ ability check (data in civs.json, effects keyed by name).
    pub fn has_ability(&self, pid: usize, ability: &str) -> bool {
        self.rules.civs.get(&self.players[pid].civ)
            .map(|c| c.ability == ability)
            .unwrap_or(false)
    }

    /// Eureka/inspiration fraction (China's Dynastic Cycle: 50% vs 40%).
    fn boost_frac(&self, pid: usize) -> f64 {
        if self.has_ability(pid, "dynastic_cycle") {
            0.5
        } else {
            0.4
        }
    }

    // -------------------------------------------------- trade routes

    /// Trading capacity: 1 with Foreign Trade, +1 per city with a Commercial
    /// Hub or Harbor (not cumulative per city), +2 under Merchant Republic.
    pub fn trade_capacity(&self, pid: usize) -> i64 {
        let p = &self.players[pid];
        if !p.civics.contains("foreign_trade") {
            return 0;
        }
        let mut cap = 1;
        for c in self.cities.values().filter(|c| c.owner == pid) {
            if c.districts.contains_key("commercial_hub")
                || c.districts.contains_key("harbor") {
                cap += 1;
            }
        }
        if p.government.as_deref() == Some("merchant_republic") {
            cap += 2;
        }
        cap
    }

    pub fn active_routes(&self, pid: usize) -> i64 {
        self.routes.iter().filter(|r| r.owner == pid).count() as i64
    }

    /// Origin-city yields of a route by destination districts (Civ 6 vanilla
    /// table): domestic food/production, international gold/science/etc.
    pub fn route_yields(&self, dest: u32, domestic: bool) -> Yields {
        let city = &self.cities[&dest];
        let mut ys = Yields::default();
        if domestic {
            ys.food += 1.0;
            ys.production += 1.0; // city center
        } else {
            ys.gold += 3.0;
        }
        for d in city.districts.keys() {
            match (d.as_str(), domestic) {
                ("campus", true) | ("holy_site", true) | ("theater_square", true)
                | ("entertainment_complex", true) => ys.food += 1.0,
                ("encampment", _) | ("industrial_zone", _)
                | ("commercial_hub", true) | ("harbor", true) => ys.production += 1.0,
                ("commercial_hub", false) | ("harbor", false) => ys.gold += 3.0,
                ("campus", false) => ys.science += 1.0,
                ("holy_site", false) => ys.faith += 1.0,
                ("theater_square", false) => ys.culture += 1.0,
                ("entertainment_complex", false) => ys.food += 1.0,
                _ => {}
            }
        }
        ys
    }

    fn do_trade_route(&mut self, pid: usize, uid: u32, dest: u32) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        if u.kind != "trader" {
            return Err("only traders run routes".into());
        }
        let origin = self.city_at(u.pos)
            .filter(|cid| self.cities[cid].owner == pid)
            .ok_or_else(|| "trader must be in one of your cities".to_string())?;
        let dc = self.cities.get(&dest).ok_or_else(|| "no such city".to_string())?;
        if dest == origin {
            return Err("destination is the origin".into());
        }
        if self.is_at_war(pid, dc.owner) {
            return Err("cannot trade with an enemy".into());
        }
        if self.wdist(self.cities[&origin].pos, dc.pos) > 15 {
            return Err("destination out of range".into());
        }
        if self.routes.iter().any(|r| r.origin == origin && r.dest == dest) {
            return Err("route already active".into());
        }
        if self.active_routes(pid) >= self.trade_capacity(pid) {
            return Err("no trading capacity".into());
        }
        self.build_road(self.cities[&origin].pos, self.cities[&dest].pos);
        let ends = self.turn + 30;
        self.routes.push(TradeRoute { origin, dest, owner: pid, ends });
        self.remove_unit(uid); // the trader services the route until it ends
        Ok(())
    }

    /// Lay an ancient road along a greedy shortest walk between two cities
    /// (traders build roads as they go in Civ 6).
    fn build_road(&mut self, from: Pos, to: Pos) {
        let mut cur = from;
        for _ in 0..40 {
            if let Some(t) = self.map.tiles.get_mut(&cur) {
                if !self.rules.terrains[t.terrain.as_str()].water {
                    t.road = true;
                }
            }
            if cur == to {
                break;
            }
            let next = self.nbrs(cur).into_iter()
                .filter(|n| self.rules.is_passable(&self.map.tiles[n]))
                .min_by_key(|n| (self.wdist(*n, to), *n));
            match next {
                Some(n) if self.wdist(n, to) < self.wdist(cur, to) => cur = n,
                _ => break,
            }
        }
    }

    /// Route upkeep at the owner's turn start: expire finished routes and
    /// hand the trader back to its origin city.
    fn process_routes(&mut self, pid: usize) {
        let turn = self.turn;
        let expired: Vec<TradeRoute> = self.routes.iter()
            .filter(|r| r.owner == pid && turn >= r.ends).cloned().collect();
        self.routes.retain(|r| !(r.owner == pid && turn >= r.ends));
        for r in expired {
            if let Some(c) = self.cities.get(&r.origin) {
                if c.owner == pid {
                    let pos = c.pos;
                    self.place_new_unit("trader", pid, pos);
                }
            }
        }
    }

    fn cancel_routes_with(&mut self, a: usize, b: usize) {
        self.routes.retain(|r| {
            let downer = self.cities.get(&r.dest).map(|c| c.owner);
            let oowner = self.cities.get(&r.origin).map(|c| c.owner);
            !((r.owner == a && downer == Some(b))
                || (r.owner == b && downer == Some(a))
                || oowner.is_none() || downer.is_none())
        });
    }

    // -------------------------------------------------- religion

    const RELIGION_NAMES: [&'static str; 8] = ["Buddhism", "Christianity",
        "Confucianism", "Hinduism", "Islam", "Judaism", "Protestantism", "Shinto"];

    pub fn religions_founded(&self) -> usize {
        self.players.iter().filter(|p| p.religion.is_some()).count()
    }

    pub fn has_pantheon_belief(&self, pid: usize, belief: &str) -> bool {
        self.players[pid].pantheon.as_deref() == Some(belief)
    }

    /// The religion a city predominantly follows (highest pressure, min 50).
    pub fn city_religion<'a>(&self, city: &'a City) -> Option<&'a str> {
        city.pressure.iter()
            .filter(|(_, v)| **v >= 50.0)
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap().then(b.0.cmp(a.0)))
            .map(|(r, _)| r.as_str())
    }

    fn religion_founder(&self, religion: &str) -> Option<usize> {
        self.players.iter()
            .find(|p| p.religion.as_deref() == Some(religion))
            .map(|p| p.id)
    }

    fn founder_has(&self, religion: &str, belief: &str) -> bool {
        self.religion_founder(religion)
            .map(|pid| self.players[pid].religion_beliefs.iter().any(|b| b == belief))
            .unwrap_or(false)
    }

    fn do_choose_pantheon(&mut self, pid: usize, belief: &str) -> Result<(), String> {
        if self.players[pid].pantheon.is_some() {
            return Err("pantheon already chosen".into());
        }
        if self.players[pid].faith < 25.0 {
            return Err("needs 25 faith".into());
        }
        if !self.rules.beliefs.pantheon.contains_key(belief) {
            return Err("no such pantheon belief".into());
        }
        if self.players.iter().any(|p| p.pantheon.as_deref() == Some(belief)) {
            return Err("belief already taken".into());
        }
        self.players[pid].pantheon = Some(belief.to_string());
        self.players[pid].era_score += 1;
        Ok(())
    }

    fn do_found_religion(&mut self, pid: usize, follower: &str,
                         founder: &str) -> Result<(), String> {
        if !self.players[pid].prophet_pending {
            return Err("no great prophet available".into());
        }
        if self.religions_founded() >= self.max_religions() {
            return Err("this map has reached its religion limit".into());
        }
        if !self.rules.beliefs.follower.contains_key(follower)
            || !self.rules.beliefs.founder.contains_key(founder) {
            return Err("no such belief".into());
        }
        let taken = |b: &str| self.players.iter()
            .any(|p| p.religion_beliefs.iter().any(|x| x == b));
        if taken(follower) || taken(founder) {
            return Err("belief already taken".into());
        }
        let holy = self.cities.values()
            .find(|c| c.owner == pid && c.districts.contains_key("holy_site"))
            .map(|c| c.id)
            .ok_or_else(|| "needs a city with a holy site".to_string())?;
        let name = Self::RELIGION_NAMES[self.religions_founded() % 8].to_string();
        let p = &mut self.players[pid];
        p.prophet_pending = false;
        p.religion = Some(name.clone());
        p.era_score += 3;
        p.religion_beliefs = vec![follower.to_string(), founder.to_string()];
        self.cities.get_mut(&holy).unwrap().pressure.insert(name, 1000.0);
        Ok(())
    }

    fn do_spread(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        if u.kind != "missionary" || u.charges <= 0 {
            return Err("not a missionary with charges".into());
        }
        let religion = self.players[pid].religion.clone()
            .ok_or_else(|| "no religion to spread".to_string())?;
        let cid = self.city_at(u.pos)
            .or_else(|| self.nbrs(u.pos).into_iter()
                .find_map(|n| self.city_at(n)))
            .ok_or_else(|| "no city in range".to_string())?;
        *self.cities.get_mut(&cid).unwrap()
            .pressure.entry(religion).or_insert(0.0) += 200.0;
        let mu = self.units.get_mut(&uid).unwrap();
        mu.charges -= 1;
        mu.moves_left = 0.0;
        mu.acted = true;
        if self.units[&uid].charges <= 0 {
            self.remove_unit(uid);
        }
        self.check_religious_victory();
        Ok(())
    }

    /// Passive spread: each city following a religion exerts +1 pressure/turn
    /// on cities within 9 tiles (+2 from the founder's holy city).
    fn process_pressure(&mut self, pid: usize) {
        let sources: Vec<(Pos, String, f64)> = self.cities.values()
            .filter_map(|c| {
                self.city_religion(c).map(|r| {
                    let boost = if c.pressure.get(r).copied().unwrap_or(0.0) >= 1000.0 {
                        2.0
                    } else {
                        1.0
                    };
                    (c.pos, r.to_string(), boost)
                })
            })
            .collect();
        let targets: Vec<u32> = self.player_city_ids(pid);
        for cid in targets {
            let cpos = self.cities[&cid].pos;
            for (spos, r, amt) in &sources {
                if *spos != cpos && self.wdist(*spos, cpos) <= 9 {
                    *self.cities.get_mut(&cid).unwrap()
                        .pressure.entry(r.clone()).or_insert(0.0) += amt;
                }
            }
        }
        self.check_religious_victory();
    }

    /// Religious victory: your religion is the majority in over half the
    /// cities of every living major civilization (Civ 6 simplified).
    fn check_religious_victory(&mut self) {
        if self.winner.is_some() {
            return;
        }
        for p in 0..self.players.len() {
            if !self.victory_eligible(p) {
                continue;
            }
            let religion = match &self.players[p].religion {
                Some(r) => r.clone(),
                None => continue,
            };
            let mut all = true;
            for o in self.players.iter().filter(|o| o.alive && !o.is_minor) {
                let cities: Vec<&City> = self.cities.values()
                    .filter(|c| c.owner == o.id).collect();
                if cities.is_empty() {
                    all = false; // a civ without cities cannot be converted
                    break;
                }
                let following = cities.iter()
                    .filter(|c| self.city_religion(c) == Some(religion.as_str()))
                    .count();
                if following * 2 <= cities.len() {
                    all = false;
                    break;
                }
            }
            if all {
                self.set_winner(p, "religious");
                return;
            }
        }
    }

    // -------------------------------------------------- great people

    fn gp_district(d: &str) -> Option<&'static str> {
        match d {
            "campus" => Some("scientist"),
            "holy_site" => Some("prophet"),
            "commercial_hub" => Some("merchant"),
            "theater_square" => Some("artist"),
            "industrial_zone" => Some("engineer"),
            "encampment" => Some("general"),
            "harbor" => Some("admiral"),
            _ => None,
        }
    }

    fn gp_building(b: &str) -> Option<&'static str> {
        match b {
            "library" | "university" => Some("scientist"),
            "shrine" => Some("prophet"),
            "market" | "bank" => Some("merchant"),
            "amphitheater" => Some("artist"),
            "workshop" => Some("engineer"),
            "barracks" | "armory" => Some("general"),
            "lighthouse" => Some("admiral"),
            _ => None,
        }
    }

    /// Point cost of a player's next great person of a type (doubles per
    /// claim, Civ 6-style era scaling).
    pub fn gp_cost(&self, pid: usize, kind: &str) -> f64 {
        let n = self.players[pid].gp_claimed.get(kind).copied().unwrap_or(0);
        60.0 * (1u64 << n.min(6) as u64) as f64
    }

    /// Accrue great person points and auto-claim on reaching the threshold
    /// (simplified: generic named-less great people with instant effects).
    fn process_great_people(&mut self, pid: usize) {
        if self.players[pid].is_minor {
            return;
        }
        let mut earn: BTreeMap<String, f64> = BTreeMap::new();
        for c in self.cities.values().filter(|c| c.owner == pid) {
            for d in c.districts.keys() {
                if let Some(t) = Self::gp_district(d) {
                    *earn.entry(t.to_string()).or_insert(0.0) += 1.0;
                }
            }
            for b in &c.buildings {
                if let Some(t) = Self::gp_building(b) {
                    *earn.entry(t.to_string()).or_insert(0.0) += 1.0;
                }
            }
        }
        if self.has_policy(pid, "strategos") {
            *earn.entry("general".to_string()).or_insert(0.0) += 2.0;
        }
        if self.has_policy(pid, "inspiration") {
            *earn.entry("scientist".to_string()).or_insert(0.0) += 2.0;
        }
        if self.has_policy(pid, "revelation") {
            *earn.entry("prophet".to_string()).or_insert(0.0) += 2.0;
        }
        if self.has_pantheon_belief(pid, "divine_spark") {
            for c in self.cities.values().filter(|c| c.owner == pid) {
                for d in ["campus", "holy_site", "theater_square"] {
                    if c.districts.contains_key(d) {
                        let t = Self::gp_district(d).unwrap();
                        *earn.entry(t.to_string()).or_insert(0.0) += 1.0;
                    }
                }
            }
        }
        let mult = 1.0 + self.gov_effects(pid).great_people_pct / 100.0;
        for (t, amt) in earn {
            *self.players[pid].gpp.entry(t).or_insert(0.0) += amt * mult;
        }
        let due: Vec<String> = self.players[pid].gpp.iter()
            .filter(|(t, pts)| **pts >= self.gp_cost(pid, t))
            .map(|(t, _)| t.clone())
            .collect();
        for t in due {
            let cost = self.gp_cost(pid, &t);
            let p = &mut self.players[pid];
            *p.gpp.get_mut(&t).unwrap() -= cost;
            *p.gp_claimed.entry(t.clone()).or_insert(0) += 1;
            p.era_score += 2;
            bump(p, "great_people");
            self.great_person_effect(pid, &t);
        }
    }

    /// Simplified instant retirement effects for a claimed great person.
    fn great_person_effect(&mut self, pid: usize, kind: &str) {
        match kind {
            "scientist" => self.grant_random_boosts(pid, 2, true),
            "artist" => self.grant_random_boosts(pid, 2, false),
            "engineer" => {
                let best = self.cities.values()
                    .filter(|c| c.owner == pid)
                    .max_by(|a, b| a.production.partial_cmp(&b.production)
                        .unwrap().then(a.id.cmp(&b.id)))
                    .map(|c| c.id);
                if let Some(cid) = best {
                    self.cities.get_mut(&cid).unwrap().production += 150.0;
                }
            }
            "merchant" => {
                self.players[pid].gold += 200.0;
                self.players[pid].envoys_free += 1;
            }
            "prophet" => {
                let can_found = self.players[pid].religion.is_none()
                    && self.religions_founded() < self.max_religions()
                    && self.cities.values().any(|c| {
                        c.owner == pid && c.districts.contains_key("holy_site")
                    });
                if can_found {
                    self.players[pid].prophet_pending = true;
                } else {
                    self.players[pid].faith += 100.0;
                }
            }
            "general" | "admiral" => {
                let sea = kind == "admiral";
                for uid in self.player_unit_ids(pid) {
                    let spec = &self.rules.units[self.units[&uid].kind.as_str()];
                    if spec.class == "military"
                        && (spec.domain.as_deref() == Some("sea")) == sea {
                        let u = self.units.get_mut(&uid).unwrap();
                        u.level = (u.level + 1).min(8);
                    }
                }
                if sea {
                    self.players[pid].gold += 100.0;
                }
            }
            _ => {}
        }
    }

    /// Eureka (techs) or Inspiration (civics) boosts on `n` random
    /// not-yet-boosted entries.
    fn grant_random_boosts(&mut self, pid: usize, n: usize, techs: bool) {
        for _ in 0..n {
            let cands: Vec<(String, f64)> = if techs {
                self.rules.techs.iter()
                    .filter(|(name, _)| {
                        let p = &self.players[pid];
                        !p.techs.contains(*name) && !p.boosted_techs.contains(*name)
                    })
                    .map(|(name, s)| (name.clone(), s.cost))
                    .collect()
            } else {
                self.rules.civics.iter()
                    .filter(|(name, _)| {
                        let p = &self.players[pid];
                        !p.civics.contains(*name) && !p.boosted_civics.contains(*name)
                    })
                    .map(|(name, s)| (name.clone(), s.cost))
                    .collect()
            };
            if cands.is_empty() {
                return;
            }
            let (name, cost) = cands[self.rng.below(cands.len())].clone();
            let f = self.boost_frac(pid);
            let p = &mut self.players[pid];
            if techs {
                p.boosted_techs.insert(name.clone());
                if p.research.as_deref() == Some(name.as_str()) {
                    p.research_progress += f * cost;
                }
            } else {
                p.boosted_civics.insert(name.clone());
                if p.civic.as_deref() == Some(name.as_str()) {
                    p.civic_progress += f * cost;
                }
            }
        }
    }

    // -------------------------------------------------- city-state envoys

    pub fn cs_type(civ: &str) -> &'static str {
        match civ {
            "Geneva" | "Hattusa" | "Stockholm" => "scientific",
            "Mohenjo-Daro" | "Vilnius" => "cultural",
            "Yerevan" | "Kandy" => "religious",
            "Kabul" | "Valletta" => "militaristic",
            "Auckland" => "industrial",
            _ => "trade", // Carthage, Zanzibar, ...
        }
    }

    /// (yield kind, matching district) for a city-state type's envoy bonuses.
    fn cs_bonus(kind: &str) -> (&'static str, &'static str) {
        match kind {
            "scientific" => ("science", "campus"),
            "cultural" => ("culture", "theater_square"),
            "religious" => ("faith", "holy_site"),
            "militaristic" => ("production", "encampment"),
            "industrial" => ("production", "industrial_zone"),
            _ => ("gold", "commercial_hub"),
        }
    }

    pub fn envoys_at(&self, pid: usize, minor: usize) -> i64 {
        self.players[pid].envoys.iter()
            .find(|(m, _)| *m == minor).map(|(_, n)| *n).unwrap_or(0)
    }

    /// Suzerain: at least 6 envoys and strictly more than every other major.
    pub fn suzerain_of(&self, minor: usize) -> Option<usize> {
        let mut best: Option<(i64, usize)> = None;
        let mut tied = false;
        for p in self.players.iter().filter(|p| !p.is_minor && p.alive) {
            let n = self.envoys_at(p.id, minor);
            match best {
                Some((bn, _)) if n == bn => tied = true,
                Some((bn, _)) if n > bn => {
                    best = Some((n, p.id));
                    tied = false;
                }
                None => {
                    best = Some((n, p.id));
                    tied = false;
                }
                _ => {}
            }
        }
        match best {
            Some((n, pid)) if n >= 6 && !tied => Some(pid),
            _ => None,
        }
    }

    fn do_send_envoy(&mut self, pid: usize, minor: usize) -> Result<(), String> {
        if self.players[pid].envoys_free <= 0 {
            return Err("no envoys to send".into());
        }
        let ok = self.players.get(minor)
            .map(|m| m.is_minor && !m.is_barbarian && m.alive)
            .unwrap_or(false);
        if !ok || self.is_at_war(pid, minor) {
            return Err("invalid city-state".into());
        }
        let p = &mut self.players[pid];
        p.envoys_free -= 1;
        match p.envoys.iter_mut().find(|(m, _)| *m == minor) {
            Some(e) => e.1 += 1,
            None => p.envoys.push((minor, 1)),
        }
        Ok(())
    }

    /// Envoy yield bonuses for one of `pid`'s cities (Civ 6 vanilla: +2 of
    /// the type yield in the capital at 1 envoy, +2 in each matching district
    /// at 3 and again at 6; suzerain repeats the district bonus).
    fn envoy_yields(&self, pid: usize, city: &City) -> Yields {
        let mut ys = Yields::default();
        for m in self.players.iter().filter(|m| m.is_minor && !m.is_barbarian && m.alive) {
            let n = self.envoys_at(pid, m.id);
            if n == 0 {
                continue;
            }
            let (kind, district) = Self::cs_bonus(Self::cs_type(&m.civ));
            let mut amt = 0.0;
            if n >= 1 && city.is_capital && city.owner == city.original_owner {
                amt += 2.0;
            }
            if city.districts.contains_key(district) {
                if n >= 3 {
                    amt += 2.0;
                }
                if n >= 6 {
                    amt += 2.0;
                }
                if self.suzerain_of(m.id) == Some(pid) {
                    amt += 2.0;
                }
            }
            match kind {
                "science" => ys.science += amt,
                "culture" => ys.culture += amt,
                "faith" => ys.faith += amt,
                "production" => ys.production += amt,
                _ => ys.gold += amt,
            }
        }
        ys
    }

    /// Apply unit-specific experience modifiers, then round to the nearest
    /// integer (with .5 rounding upward) as Civ VI does.
    fn modified_xp(&self, uid: u32, amt: f64) -> i64 {
        let (owner, kind) = match self.units.get(&uid) {
            Some(u) => (u.owner, u.kind.clone()),
            None => return 0,
        };
        let spec = &self.rules.units[kind.as_str()];
        let mut multiplier = 1.0;
        if self.players[owner].government.as_deref() == Some("oligarchy") {
            multiplier += 0.2;
        }
        if spec.promotion_class == "recon" && self.has_policy(owner, "survey") {
            multiplier += 1.0;
        }
        if spec.promotion_class == "ranged" && self.has_ability(owner, "ta_seti") {
            multiplier += 0.5;
        }
        (amt * multiplier).round() as i64
    }

    fn award_xp(&mut self, uid: u32, amt: f64) {
        let gained = self.modified_xp(uid, amt);
        if let Some(unit) = self.units.get_mut(&uid) {
            unit.xp += gained;
        }
    }

    /// Unit-vs-unit XP is based on relative base Combat Strength. Kills
    /// double that component. The final, modified award is capped at 8 XP.
    fn award_unit_combat_xp(
        &mut self,
        uid: u32,
        opponent: &Unit,
        ranged: bool,
        attacking: bool,
        killed_opponent: bool,
    ) {
        let Some(unit) = self.units.get(&uid) else {
            return;
        };
        if self.players[opponent.owner].is_barbarian && unit.level >= 2 {
            // Once a unit has its first promotion, combat with Barbarians and
            // Free Cities grants exactly 1 XP regardless of other modifiers.
            self.units.get_mut(&uid).unwrap().xp += 1;
            return;
        }
        let own = self.rules.units[unit.kind.as_str()].strength.max(1.0);
        let other = self.rules.units[opponent.kind.as_str()].strength.max(1.0);
        let mut relative = other / own;
        if killed_opponent {
            relative *= 2.0;
        }
        let amount = relative
            + if ranged { 1.0 } else { 2.0 }
            + if attacking { 1.0 } else { 0.0 };
        let gained = self.modified_xp(uid, amount).min(8);
        self.units.get_mut(&uid).unwrap().xp += gained;
    }

    /// Discipline: +5 combat strength when fighting barbarians.
    fn vs_bonus(&self, owner: usize, opponent: usize) -> f64 {
        if self.players[opponent].is_barbarian && self.has_policy(owner, "discipline") {
            5.0
        } else {
            0.0
        }
    }

    /// +% production toward the item at the head of a city's queue from
    /// slotted policy cards (Agoge, Maneuver, Maritime Industries, Ilkum,
    /// Colonization, Feudal Contract, Limes).
    fn item_prod_mult(&self, pid: usize, cid: u32, item: Option<&Item>) -> f64 {
        let mut bonus: f64 = 0.0;
        match item {
            Some(Item::Unit { unit }) => {
                let spec = &self.rules.units[unit.as_str()];
                if unit == "builder" && self.has_policy(pid, "ilkum") {
                    bonus += 0.3;
                } else if unit == "settler" && self.has_policy(pid, "colonization") {
                    bonus += 0.5;
                } else if spec.domain.as_deref() == Some("sea")
                    && self.has_policy(pid, "maritime_industries") {
                    bonus += 1.0;
                } else if spec.cavalry && self.has_policy(pid, "maneuver") {
                    bonus += 0.5;
                }
                if spec.ranged_strength > 0.0 && spec.class == "military"
                    && self.has_ability(pid, "ta_seti") {
                    bonus += 0.5; // Nubia: Ta-Seti
                } else if spec.class == "military" && !spec.siege {
                    if self.has_policy(pid, "agoge")
                        || self.has_policy(pid, "feudal_contract") {
                        bonus += 0.5;
                    }
                    if self.has_pantheon_belief(pid, "god_of_the_forge") {
                        bonus += 0.25;
                    }
                }
            }
            Some(Item::Building { building }) => {
                if (building == "walls" || building == "medieval_walls")
                    && self.has_policy(pid, "limes") {
                    bonus += 1.0;
                }
                if self.rules.buildings[building.as_str()].wonder
                    && self.has_ability(pid, "iteru")
                    && self.map.tiles[&self.cities[&cid].pos].river {
                    bonus += 0.15; // Egypt: Iteru (river cities)
                }
            }
            _ => {}
        }
        1.0 + bonus
    }

    pub fn gov_slots(&self, pid: usize) -> crate::rules::PolicySlots {
        let mut slots = match &self.players[pid].government {
            Some(g) => self.rules.governments.get(g)
                .map(|s| s.slots).unwrap_or_default(),
            None => Default::default(),
        };
        let any = slots.military + slots.economic + slots.diplomatic + slots.wildcard;
        if any > 0 && self.has_ability(pid, "platos_republic") {
            slots.wildcard += 1; // Greece: Plato's Republic
        }
        slots
    }

    /// Can this set of cards be seated in the player's slots? Typed cards
    /// fill their own slot type first; overflow and wildcard cards need
    /// wildcard slots (Civ 6 rule).
    fn policies_fit(&self, pid: usize, cards: &BTreeSet<String>) -> bool {
        let slots = self.gov_slots(pid);
        let (mut m, mut e, mut d, mut w) = (0i64, 0i64, 0i64, 0i64);
        for c in cards {
            match self.rules.policies.get(c).map(|p| p.slot.as_str()) {
                Some("military") => m += 1,
                Some("economic") => e += 1,
                Some("diplomatic") => d += 1,
                _ => w += 1,
            }
        }
        let overflow = (m - slots.military).max(0) + (e - slots.economic).max(0)
            + (d - slots.diplomatic).max(0);
        overflow + w <= slots.wildcard
    }

    /// Cards the player has unlocked and may slot (not yet slotted, civic
    /// met, not obsoleted by an unlocked successor card).
    pub fn available_policies(&self, pid: usize) -> Vec<String> {
        let p = &self.players[pid];
        if p.is_minor {
            return vec![];
        }
        let obsolete: BTreeSet<&str> = self.rules.policies.values()
            .filter(|s| {
                s.civic.as_ref().map(|c| p.civics.contains(c)).unwrap_or(true)
            })
            .filter_map(|s| s.replaces.as_deref())
            .collect();
        self.rules.policies.iter()
            .filter(|(name, s)| {
                !p.policies.contains(*name)
                    && !obsolete.contains(name.as_str())
                    && s.civic.as_ref().map(|c| p.civics.contains(c)).unwrap_or(true)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub fn is_embarked(&self, u: &Unit) -> bool {
        self.rules.units[u.kind.as_str()].domain.as_deref() != Some("sea")
            && self.map.get(u.pos).map(|t| self.rules.is_water(t)).unwrap_or(false)
    }

    fn oligarchy_applies(spec: &crate::rules::UnitSpec) -> bool {
        matches!(spec.promotion_class.as_str(),
            "melee" | "anti_cavalry" | "naval_melee")
    }

    fn government_combat_bonus(&self, u: &Unit) -> f64 {
        let bonus = self.gov_effects(u.owner).combat_strength;
        if self.players[u.owner].government.as_deref() == Some("oligarchy") {
            if Self::oligarchy_applies(&self.rules.units[u.kind.as_str()]) {
                bonus
            } else {
                0.0
            }
        } else {
            bonus
        }
    }

    fn unit_unembarked_strength(&self, u: &Unit) -> f64 {
        let mut s = self.rules.units[u.kind.as_str()].strength.max(1.0)
            + self.government_combat_bonus(u);
        if self.has_ability(u.owner, "gifts_for_the_tlatoani") {
            s += self.empire_luxuries(u.owner) as f64; // Montezuma
        }
        s
    }

    pub fn unit_strength(&self, u: &Unit, defending: bool) -> f64 {
        if self.is_embarked(u) {
            return 10.0; // embarked units are nearly defenseless
        }
        let mut s = self.unit_unembarked_strength(u);
        if defending {
            s += 3.0 * u.fortify_turns.clamp(0, 2) as f64;
        }
        s
    }

    fn unit_can_fortify(&self, u: &Unit) -> bool {
        let spec = &self.rules.units[u.kind.as_str()];
        spec.class == "military"
            && spec.domain.as_deref() != Some("sea")
            && !self.is_embarked(u)
    }

    pub fn unit_ranged_strength(&self, u: &Unit) -> f64 {
        let rs = self.rules.units[u.kind.as_str()].ranged_strength;
        if rs <= 0.0 {
            return 0.0;
        }
        rs + self.government_combat_bonus(u)
            + if self.has_ability(u.owner, "gifts_for_the_tlatoani") {
                self.empire_luxuries(u.owner) as f64
            } else { 0.0 }
    }

    pub fn unit_bombard_strength(&self, u: &Unit) -> f64 {
        let bs = self.rules.units[u.kind.as_str()].bombard_strength;
        if bs <= 0.0 {
            return 0.0;
        }
        bs + self.government_combat_bonus(u)
            + if self.has_ability(u.owner, "gifts_for_the_tlatoani") {
                self.empire_luxuries(u.owner) as f64
            } else { 0.0 }
    }

    pub fn unit_ranged_attack_strength(&self, u: &Unit) -> f64 {
        self.unit_ranged_strength(u).max(self.unit_bombard_strength(u))
    }

    pub fn city_housing(&self, city: &City) -> f64 {
        // fresh water (river/oasis) = 5, coastal = 3, otherwise 2 (Civ 6)
        let center = &self.map.tiles[&city.pos];
        let fresh = center.river || self.nbrs(city.pos).iter().any(|n| {
            self.map.get(*n).map(|t| {
                t.river || t.feature.as_deref() == Some("oasis")
            }).unwrap_or(false)
        });
        let coastal = self.nbrs(city.pos).iter().any(|n| {
            self.map.get(*n).map(|t| self.rules.is_water(t)).unwrap_or(false)
        });
        let mut h = if fresh { 5.0 } else if coastal { 3.0 } else { 2.0 };
        for pos in city.owned_tiles.iter().filter(|p| self.wdist(city.pos, **p) <= 3) {
            if let Some(improvement) = self.map.tiles[pos].improvement.as_deref() {
                h += self.rules.improvements.get(improvement)
                    .map(|spec| spec.housing).unwrap_or(0.0);
            }
        }
        for b in &city.buildings {
            h += self.rules.buildings[b.as_str()].housing;
        }
        if self.has_policy(city.owner, "insulae") && city.districts.len() >= 2 {
            h += 1.0;
        }
        h + self.gov_effects(city.owner).housing
    }

    pub fn wonder_built(&self, name: &str) -> bool {
        self.cities.values().any(|c| c.buildings.iter().any(|b| b == name))
    }

    fn empire_building_sum(&self, pid: usize, f: impl Fn(&crate::rules::BuildingSpec) -> f64) -> f64 {
        self.cities.values()
            .filter(|c| c.owner == pid)
            .flat_map(|c| c.buildings.iter())
            .map(|b| f(&self.rules.buildings[b.as_str()]))
            .sum()
    }

    pub fn empire_luxuries(&self, pid: usize) -> usize {
        let mut lux: BTreeSet<&str> = BTreeSet::new();
        for c in self.cities.values().filter(|c| c.owner == pid) {
            for pos in &c.owned_tiles {
                if let Some(r) = &self.map.tiles[pos].resource {
                    if self.rules.resources[r.as_str()].class == "luxury" {
                        lux.insert(r);
                    }
                }
            }
        }
        lux.len()
    }

    pub fn city_amenity_surplus(&self, city: &City) -> i64 {
        let mut supply = self.empire_luxuries(city.owner) as f64;
        for dname in city.districts.keys() {
            supply += self.rules.districts[dname.as_str()].amenity;
        }
        for b in &city.buildings {
            supply += self.rules.buildings[b.as_str()].amenity;
        }
        supply += self.gov_effects(city.owner).amenity;
        if self.players[city.owner].governors.contains(&city.id) {
            supply += 1.0; // an established governor steadies the city
        }
        if self.has_policy(city.owner, "retainers") {
            let garrison = self.units_at(city.pos).into_iter().any(|id| {
                let o = &self.units[&id];
                o.owner == city.owner
                    && self.rules.units[o.kind.as_str()].class == "military"
            });
            if garrison {
                supply += 1.0;
            }
        }
        supply as i64 - 0.max((city.pop - 1) / 2) as i64
    }

    fn amenity_yield_mult(&self, city: &City) -> f64 {
        let s = self.city_amenity_surplus(city);
        if s >= 2 {
            1.05
        } else if s >= 0 {
            1.0
        } else if s >= -2 {
            0.93
        } else {
            0.85
        }
    }

    pub fn city_can_strike(&self, city: &City) -> bool {
        !city.struck && city.wall_hp > 0 // ranged strike needs standing walls
    }

    /// Route an attack roll into a walled city: walls absorb it (melee does
    /// 15%, ranged 50%, siege 100% of the roll to walls), while the city
    /// itself takes 1 damage behind healthy walls (>=80%), half through
    /// damaged walls, and full damage once breached (<20%) or bare (Civ 6).
    fn city_take_damage(&mut self, cid: u32, dmg: i32, wall_mult: f64,
                        bypass_walls: bool) {
        let (wall, max) = {
            let c = &self.cities[&cid];
            (c.wall_hp, self.city_max_wall_hp(c))
        };
        let c = self.cities.get_mut(&cid).unwrap();
        c.last_attacked = self.turn;
        if wall > 0 && max > 0 {
            let frac = wall as f64 / max as f64;
            let through = if bypass_walls {
                dmg // siege tower: attackers pour past the walls (Civ 6)
            } else if frac >= 0.8 {
                1
            } else if frac >= 0.2 {
                dmg / 2
            } else {
                dmg
            };
            c.wall_hp = (wall - ((dmg as f64 * wall_mult).round() as i32).max(1)).max(0);
            c.hp -= through.max(1);
        } else {
            c.hp -= dmg;
        }
    }

    pub fn available_techs(&self, pid: usize) -> Vec<String> {
        let p = &self.players[pid];
        self.rules
            .techs
            .iter()
            .filter(|(t, s)| {
                !p.techs.contains(*t) && s.requires.iter().all(|r| p.techs.contains(r))
            })
            .map(|(t, _)| t.clone())
            .collect()
    }

    pub fn available_civics(&self, pid: usize) -> Vec<String> {
        let p = &self.players[pid];
        self.rules
            .civics
            .iter()
            .filter(|(c, s)| {
                !p.civics.contains(*c) && s.requires.iter().all(|r| p.civics.contains(r))
            })
            .map(|(c, _)| c.clone())
            .collect()
    }

    pub fn score(&self, pid: usize) -> i64 {
        let p = &self.players[pid];
        let cities: Vec<&City> = self.cities.values().filter(|c| c.owner == pid).collect();
        10 * cities.len() as i64
            + 3 * cities.iter().map(|c| c.pop as i64).sum::<i64>()
            + 3 * cities.iter().map(|c| c.districts.len() as i64).sum::<i64>()
            + cities.iter().map(|c| c.buildings.len() as i64).sum::<i64>()
            + 2 * p.techs.len() as i64
            + 2 * p.civics.len() as i64
            + self.units.values().filter(|u| u.owner == pid).count() as i64
    }

    pub fn military_power(&self, pid: usize) -> f64 {
        self.units
            .values()
            .filter(|u| u.owner == pid)
            .map(|u| self.rules.units[u.kind.as_str()].strength * u.hp as f64 / 100.0)
            .sum()
    }

    fn unlocked(&self, pid: usize, tech: &Option<String>, civic: &Option<String>) -> bool {
        let p = &self.players[pid];
        if let Some(t) = tech {
            if !p.techs.contains(t) {
                return false;
            }
        }
        if let Some(c) = civic {
            if !p.civics.contains(c) {
                return false;
            }
        }
        true
    }

    fn has_resource(&self, pid: usize, res: &str) -> bool {
        for c in self.cities.values().filter(|c| c.owner == pid) {
            for pos in &c.owned_tiles {
                if self.map.tiles[pos].resource.as_deref() == Some(res) {
                    return true;
                }
            }
        }
        false
    }

    // -------------------------------------------------------- unit helpers

    fn spawn_unit(&mut self, kind: &str, owner: usize, pos: Pos) -> u32 {
        let spec = &self.rules.units[kind];
        if spec.class == "military" {
            let best = self.players[owner]
                .counters
                .entry("strongest_unit_built".to_string())
                .or_insert(0);
            *best = (*best).max(spec.strength.round() as i64);
            if spec.ranged_strength > 0.0 {
                let best_ranged = self.players[owner]
                    .counters
                    .entry("strongest_ranged_built".to_string())
                    .or_insert(0);
                *best_ranged = (*best_ranged).max(spec.ranged_strength.round() as i64);
            }
        }
        let mut charges = spec.charges;
        if kind == "builder" {
            charges += self.empire_building_sum(owner, |b| b.builder_charges as f64) as i32;
            if self.has_policy(owner, "serfdom") {
                charges += 2;
            }
            if self.has_ability(owner, "dynastic_cycle") {
                charges += 1; // China: First Emperor
            }
        }
        let u = Unit {
            id: self.next_id,
            kind: kind.to_string(),
            owner,
            pos,
            hp: 100,
            moves_left: spec.moves,
            charges,
            xp: 0,
            level: 1,
            fortified: false,
            fortify_turns: 0,
            acted: false,
            zoc_stopped: false,
        };
        self.next_id += 1;
        let id = u.id;
        let sight = spec.sight;
        self.occ.entry(pos).or_default().push(id);
        self.units.insert(id, u);
        self.reveal(owner, pos, sight);
        id
    }

    fn remove_unit(&mut self, uid: u32) {
        if let Some(u) = self.units.remove(&uid) {
            if let Some(ids) = self.occ.get_mut(&u.pos) {
                ids.retain(|i| *i != uid);
                if ids.is_empty() {
                    self.occ.remove(&u.pos);
                }
            }
        }
    }

    fn relocate(&mut self, uid: u32, pos: Pos) {
        let (old, owner) = {
            let u = &self.units[&uid];
            (u.pos, u.owner)
        };
        if let Some(ids) = self.occ.get_mut(&old) {
            ids.retain(|i| *i != uid);
            if ids.is_empty() {
                self.occ.remove(&old);
            }
        }
        self.units.get_mut(&uid).unwrap().pos = pos;
        self.occ.entry(pos).or_default().push(uid);
        let sight = self.rules.units[self.units[&uid].kind.as_str()].sight;
        self.reveal(owner, pos, sight);
    }

    fn reveal(&mut self, pid: usize, pos: Pos, radius: i32) {
        let mut wonders: Vec<Pos> = Vec::new();
        for p in self.wdisk(pos, radius) {
            if let Some(t) = self.map.tiles.get(&p) {
                let new = self.players[pid].explored.insert(p);
                if new && !self.players[pid].is_minor {
                    let nw = t.feature.as_ref()
                        .map(|f| self.rules.features[f.as_str()].natural_wonder)
                        .unwrap_or(false);
                    if nw {
                        wonders.push(p);
                    }
                }
            }
        }
        for p in wonders {
            // +1 era score on discovery, +2 more for the world's first finder
            let first = !self.players.iter().any(|o| {
                o.id != pid && !o.is_minor && o.explored.contains(&p)
            });
            self.players[pid].era_score += if first { 3 } else { 1 };
        }
    }

    /// MP to step from `from` onto adjacent `to`. Entering a land river tile
    /// from off-river adds the Civ 6 crossing surcharge (+2 MP).
    pub fn step_cost(&self, from: Pos, to: Pos) -> f64 {
        let t = &self.map.tiles[&to];
        let mut c = self.rules.move_cost(t);
        if t.river && !t.road && !self.rules.is_water(t)
            && self.map.get(from).map(|f| !f.river).unwrap_or(true) {
            c += 2.0; // roads bridge rivers (simplified: any road)
        }
        c
    }

    fn crosses_river(&self, from: Pos, to: Pos) -> bool {
        let Some(a) = self.map.get(from) else { return false };
        let Some(b) = self.map.get(to) else { return false };
        !self.rules.is_water(a) && !self.rules.is_water(b) && a.river != b.river
    }

    fn sight_height(&self, pos: Pos) -> i32 {
        let t = &self.map.tiles[&pos];
        if t.terrain == "mountain" {
            return 3;
        }
        t.hills as i32
            + matches!(t.feature.as_deref(), Some("forest" | "jungle")) as i32
    }

    /// Civ VI line of sight for the ranges represented by this ruleset. At
    /// range 2 either unobstructed hex corridor is enough; hills provide a
    /// vantage point, while wooded hills and mountains remain taller cover.
    fn has_line_of_sight(&self, from: Pos, to: Pos, unit_in_district: bool) -> bool {
        let distance = self.wdist(from, to);
        if distance <= 1 || distance >= 3 {
            return true; // adjacent fire is unconditional; range 3+ lobs shots
        }
        let mut attacker_height = self.map.tiles[&from].hills as i32;
        if unit_in_district {
            let t = &self.map.tiles[&from];
            if self.city_at(from).is_some()
                || t.district.as_deref() == Some("encampment")
            {
                attacker_height += 1;
            }
        }
        let target_height = self.sight_height(to);
        self.nbrs(from).into_iter()
            .filter(|p| self.wdist(*p, to) == 1)
            .any(|middle| {
                let blocker = self.sight_height(middle);
                blocker <= attacker_height || blocker < target_height
            })
    }

    fn unit_max_moves(&self, uid: u32) -> f64 {
        let u = &self.units[&uid];
        let spec = &self.rules.units[u.kind.as_str()];
        if matches!(u.kind.as_str(), "war_cart" | "maryannu_chariot_archer") {
            let t = &self.map.tiles[&u.pos];
            if !t.hills
                && !matches!(t.feature.as_deref(), Some("forest" | "jungle"))
            {
                return 4.0;
            }
        }
        spec.moves
    }

    fn exerts_zoc(&self, u: &Unit) -> bool {
        let spec = &self.rules.units[u.kind.as_str()];
        // Embarkation does not remove a land unit's ZOC. Its native domain
        // still limits projection to land tiles, which is handled by the
        // caller when comparing the target tile's domain.
        spec.zone_of_control
    }

    /// Is `pos` inside a military enemy zone of control for player `pid`?
    /// ZOC exists from turn one: it is not unlocked by a technology or civic.
    /// Units only project into their native domain and rivers block projection;
    /// defensible districts project into every adjacent land or water tile.
    pub fn in_enemy_zoc(&self, pid: usize, pos: Pos) -> bool {
        let t = match self.map.get(pos) {
            Some(t) => t,
            None => return false,
        };
        let water = self.rules.is_water(t);
        for n in self.nbrs(pos) {
            let nt = match self.map.get(n) {
                Some(nt) => nt,
                None => continue,
            };
            for oid in self.units_at(n) {
                let o = &self.units[&oid];
                if o.owner == pid || !self.is_at_war(pid, o.owner)
                    || !self.exerts_zoc(o) {
                    continue;
                }
                let o_water = self.rules.units[o.kind.as_str()]
                    .domain.as_deref() == Some("sea");
                if o_water != water || self.crosses_river(n, pos) {
                    continue;
                }
                return true;
            }
            let hostile_city = self.city_at(n).map(|cid| {
                let c = &self.cities[&cid];
                c.owner != pid && self.is_at_war(pid, c.owner)
            }).unwrap_or(false);
            let hostile_camp = nt.district.as_deref() == Some("encampment")
                && nt.owner_city.map(|oc| {
                    let owner = self.cities[&oc].owner;
                    owner != pid && self.is_at_war(pid, owner)
                }).unwrap_or(false);
            if hostile_city || hostile_camp {
                return true;
            }
        }
        false
    }

    fn in_enemy_zoc_for(&self, uid: u32, pos: Pos) -> bool {
        let mover = &self.units[&uid];
        let mover_spec = &self.rules.units[mover.kind.as_str()];
        let Some(t) = self.map.get(pos) else { return false };
        let water = self.rules.is_water(t);
        for n in self.nbrs(pos) {
            for oid in self.units_at(n) {
                let other = &self.units[&oid];
                if other.owner == mover.owner {
                    continue;
                }
                let other_spec = &self.rules.units[other.kind.as_str()];
                let religious_zoc = mover_spec.class == "religious"
                    && other_spec.class == "religious"
                    && self.players[mover.owner].religion
                        != self.players[other.owner].religion;
                let military_zoc = self.is_at_war(mover.owner, other.owner)
                    && self.exerts_zoc(other);
                if !religious_zoc && !military_zoc {
                    continue;
                }
                let other_water = other_spec.domain.as_deref() == Some("sea");
                if other_water == water && !self.crosses_river(n, pos) {
                    return true;
                }
            }
            let district_owner = self.city_at(n).map(|cid| self.cities[&cid].owner)
                .or_else(|| {
                    let nt = self.map.get(n)?;
                    if nt.district.as_deref() != Some("encampment") {
                        return None;
                    }
                    nt.owner_city.map(|cid| self.cities[&cid].owner)
                });
            if district_owner.map(|owner| owner != mover.owner
                && self.is_at_war(mover.owner, owner)).unwrap_or(false)
            {
                return true;
            }
        }
        false
    }

    pub fn can_move(&self, uid: u32, pos: Pos) -> bool {
        let u = &self.units[&uid];
        if u.zoc_stopped {
            return false;
        }
        // MP is paid before entering (Civ 6): need the full step cost, but a
        // unit with untouched movement may always take one step.
        let full = u.moves_left >= self.unit_max_moves(uid);
        if !full && self.map.tiles.contains_key(&pos)
            && u.moves_left < self.step_cost(u.pos, pos) {
            return false;
        }
        self.can_enter(uid, self.units[&uid].pos, pos)
    }

    fn can_enter(&self, uid: u32, from: Pos, pos: Pos) -> bool {
        let u = &self.units[&uid];
        if self.wdist(from, pos) != 1 {
            return false;
        }
        let t = match self.map.get(pos) {
            Some(t) => t,
            None => return false,
        };
        if !self.rules.is_passable(t) {
            return false;
        }
        let spec = &self.rules.units[u.kind.as_str()];
        let water = self.rules.is_water(t);
        if spec.domain.as_deref() == Some("sea") {
            if !water {
                return false;
            }
        } else if water && !self.players[u.owner].techs.contains("shipbuilding") {
            return false; // shipbuilding lets land units embark
        }
        for oid in self.units_at(pos) {
            let o = &self.units[&oid];
            let ospec = &self.rules.units[o.kind.as_str()];
            if o.owner != u.owner {
                // Religious units occupy their own layer and may share a tile
                // with any non-religious unit, regardless of diplomacy.
                if (spec.class == "religious") != (ospec.class == "religious") {
                    continue;
                }
                if ospec.class == "military" || spec.class != "military" {
                    return false;
                }
                if !self.is_at_war(u.owner, o.owner) {
                    return false;
                }
            } else if ospec.class == spec.class {
                return false;
            }
        }
        if let Some(cid) = self.city_at(pos) {
            if self.cities[&cid].owner != u.owner {
                return false;
            }
        }
        true
    }

    /// All tiles the unit can reach this turn with its remaining movement
    /// (Dijkstra maximizing leftover MP; every intermediate tile must be
    /// legally enterable, matching repeated single-step moves).
    pub fn reachable(&self, uid: u32) -> Vec<Pos> {
        let (start, moves) = match self.units.get(&uid) {
            Some(u) => (u.pos, u.moves_left),
            None => return vec![],
        };
        let best = self.flow(uid, start, moves);
        best.into_keys().filter(|p| *p != start).collect()
    }

    /// First step of a deterministic long-range route to `to`, stopping once
    /// the unit is within `stop_range`. Unlike `reachable`, this plans across
    /// future turns so AI units can detour around mountains, coastlines, and
    /// occupied choke points instead of getting stuck in a greedy local
    /// minimum. The returned step is still validated against the current
    /// turn by the caller's normal Move action.
    pub fn route_step(&self, uid: u32, to: Pos, stop_range: i32) -> Option<Pos> {
        let unit = self.units.get(&uid)?;
        let start = unit.pos;
        let range = stop_range.max(0);
        if unit.zoc_stopped || self.wdist(start, to) <= range {
            return None;
        }
        if range == 0 {
            let target = self.map.get(to)?;
            let spec = &self.rules.units[unit.kind.as_str()];
            let target_is_water = self.rules.is_water(target);
            let sea_unit = spec.domain.as_deref() == Some("sea");
            if sea_unit != target_is_water
                && (sea_unit || !self.players[unit.owner].techs.contains("shipbuilding"))
            {
                return None;
            }
        }

        // A* keeps known-target routing cheap enough for high-throughput
        // self-play. Tuple ordering gives deterministic tie-breaking.
        let mut frontier = BinaryHeap::with_capacity(128);
        let mut distance: HashMap<Pos, i32> = HashMap::with_capacity(128);
        let mut parent: HashMap<Pos, Pos> = HashMap::with_capacity(128);
        distance.insert(start, 0);
        frontier.push(Reverse((self.wdist(start, to), 0, start)));

        let mut goal = None;
        let mut expanded = 0;
        while let Some(Reverse((_, traveled, cur))) = frontier.pop() {
            if traveled != distance[&cur] {
                continue;
            }
            expanded += 1;
            if expanded > 64 {
                break; // avoid exhaustive scans for disconnected landmasses
            }
            if self.wdist(cur, to) <= range {
                goal = Some(cur);
                break;
            }
            for n in self.nbrs(cur) {
                let enterable = if cur == start {
                    self.can_enter(uid, cur, n)
                } else {
                    self.can_path_through(uid, cur, n)
                };
                if !enterable {
                    continue;
                }
                let next_distance = traveled + 1;
                if distance.get(&n).map(|d| next_distance >= *d).unwrap_or(false) {
                    continue;
                }
                distance.insert(n, next_distance);
                parent.insert(n, cur);
                let estimate = next_distance + (self.wdist(n, to) - range).max(0);
                frontier.push(Reverse((estimate, next_distance, n)));
            }
        }
        Self::unwind_route(start, goal?, &parent)
    }

    /// Terrain/domain legality for future route segments. Dynamic unit
    /// occupancy is enforced on the returned first step; ignoring it deeper
    /// in the plan avoids expensive scans and lets moving units clear before
    /// the traveler arrives. Routes are recalculated whenever the immediate
    /// step remains blocked.
    fn can_path_through(&self, uid: u32, from: Pos, pos: Pos) -> bool {
        if self.wdist(from, pos) != 1 {
            return false;
        }
        let unit = &self.units[&uid];
        let tile = match self.map.get(pos) {
            Some(tile) => tile,
            None => return false,
        };
        if !self.rules.is_passable(tile) {
            return false;
        }
        let spec = &self.rules.units[unit.kind.as_str()];
        let water = self.rules.is_water(tile);
        if spec.domain.as_deref() == Some("sea") {
            if !water {
                return false;
            }
        } else if water && !self.players[unit.owner].techs.contains("shipbuilding") {
            return false;
        }
        self.city_at(pos)
            .map(|cid| self.cities[&cid].owner == unit.owner)
            .unwrap_or(true)
    }

    /// First step toward the nearest reachable member of `goals`. This is
    /// useful for exploration, where the geometrically nearest hidden tile
    /// may be on the far side of an impassable ridge or pre-embarkation sea.
    pub fn route_step_to_any(&self, uid: u32, goals: &HashSet<Pos>) -> Option<Pos> {
        self.first_route_step(uid, |p| goals.contains(&p))
    }

    fn first_route_step<F>(&self, uid: u32, is_goal: F) -> Option<Pos>
    where
        F: Fn(Pos) -> bool,
    {
        let unit = self.units.get(&uid)?;
        let start = unit.pos;
        if unit.zoc_stopped || is_goal(start) {
            return None;
        }

        let mut parent: HashMap<Pos, Pos> = HashMap::new();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        seen.insert(start);
        queue.push_back(start);

        let mut goal = None;
        'search: while let Some(cur) = queue.pop_front() {
            for n in self.nbrs(cur) {
                let enterable = if cur == start {
                    self.can_enter(uid, cur, n)
                } else {
                    self.can_path_through(uid, cur, n)
                };
                if seen.contains(&n) || !enterable {
                    continue;
                }
                seen.insert(n);
                parent.insert(n, cur);
                if is_goal(n) {
                    goal = Some(n);
                    break 'search;
                }
                queue.push_back(n);
            }
        }

        Self::unwind_route(start, goal?, &parent)
    }

    fn unwind_route(start: Pos, goal: Pos, parent: &HashMap<Pos, Pos>) -> Option<Pos> {
        let mut step = goal;
        while parent.get(&step).copied()? != start {
            step = parent.get(&step).copied()?;
        }
        Some(step)
    }

    fn flow(&self, uid: u32, start: Pos, moves: f64) -> BTreeMap<Pos, f64> {
        let (ignores_zoc, max_moves) = {
            let u = &self.units[&uid];
            let spec = &self.rules.units[u.kind.as_str()];
            (spec.cavalry, self.unit_max_moves(uid))
        };
        if self.units[&uid].zoc_stopped {
            return BTreeMap::new();
        }
        let mut best: BTreeMap<Pos, f64> = BTreeMap::new();
        best.insert(start, moves);
        let mut queue = vec![start];
        while let Some(cur) = queue.pop() {
            let rem = best[&cur];
            if rem <= 0.0 {
                continue;
            }
            for n in self.nbrs(cur) {
                if !self.map.tiles.contains_key(&n) || !self.can_enter(uid, cur, n) {
                    continue;
                }
                let cost = self.step_cost(cur, n);
                let fresh = cur == start && rem >= max_moves;
                if rem < cost && !fresh {
                    continue; // MP paid up front (Civ 6)
                }
                let mut new_rem = (rem - cost).max(0.0);
                if !ignores_zoc && self.in_enemy_zoc_for(uid, n) {
                    new_rem = 0.0; // entering enemy ZOC ends movement
                }
                if best.get(&n).map(|b| new_rem > *b).unwrap_or(true) {
                    best.insert(n, new_rem);
                    queue.push(n);
                }
            }
        }
        best
    }

    fn path_to(&self, uid: u32, to: Pos) -> Option<Vec<Pos>> {
        let (start, moves) = {
            let u = self.units.get(&uid)?;
            (u.pos, u.moves_left)
        };
        if start == to {
            return Some(vec![]);
        }
        let (ignores_zoc, max_moves) = {
            let u = &self.units[&uid];
            let spec = &self.rules.units[u.kind.as_str()];
            (spec.cavalry, self.unit_max_moves(uid))
        };
        if self.units[&uid].zoc_stopped {
            return None;
        }
        let mut best: BTreeMap<Pos, f64> = BTreeMap::new();
        let mut parent: BTreeMap<Pos, Pos> = BTreeMap::new();
        best.insert(start, moves);
        let mut queue = vec![start];
        while let Some(cur) = queue.pop() {
            let rem = best[&cur];
            if rem <= 0.0 {
                continue;
            }
            for n in self.nbrs(cur) {
                if !self.map.tiles.contains_key(&n) || !self.can_enter(uid, cur, n) {
                    continue;
                }
                let cost = self.step_cost(cur, n);
                let fresh = cur == start && rem >= max_moves;
                if rem < cost && !fresh {
                    continue;
                }
                let mut new_rem = (rem - cost).max(0.0);
                if !ignores_zoc && self.in_enemy_zoc_for(uid, n) {
                    new_rem = 0.0;
                }
                if best.get(&n).map(|b| new_rem > *b).unwrap_or(true) {
                    best.insert(n, new_rem);
                    parent.insert(n, cur);
                    queue.push(n);
                }
            }
        }
        parent.get(&to)?;
        let mut path = vec![to];
        let mut cur = to;
        while let Some(p) = parent.get(&cur) {
            if *p == start {
                break;
            }
            path.push(*p);
            cur = *p;
        }
        path.reverse();
        Some(path)
    }

    fn do_move_to(&mut self, pid: usize, uid: u32, to: Pos) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        if u.moves_left <= 0.0 {
            return Err("no moves left".into());
        }
        let path = self.path_to(uid, to).ok_or_else(|| "unreachable".to_string())?;
        if path.is_empty() {
            return Err("already there".into());
        }
        for step in path {
            if self.units.get(&uid).map(|x| x.moves_left <= 0.0).unwrap_or(true) {
                break;
            }
            if self.do_move(pid, uid, step).is_err() {
                break; // out of MP or stopped by ZOC mid-path
            }
        }
        Ok(())
    }

    // -------------------------------------------------------- city helpers

    pub fn can_found_city(&self, uid: u32) -> bool {
        let u = &self.units[&uid];
        let t = &self.map.tiles[&u.pos];
        if self.rules.is_water(t) || !self.rules.is_passable(t) {
            return false;
        }
        for c in self.cities.values() {
            if self.wdist(c.pos, u.pos) < 4 {
                return false;
            }
        }
        if let Some(oc) = t.owner_city {
            if self.cities[&oc].owner != u.owner {
                return false;
            }
        }
        true
    }

    /// Gathering Storm grants 100 HP of Outer Defenses per wall level.
    pub fn city_max_wall_hp(&self, city: &City) -> i32 {
        100 * city.buildings.iter()
            .filter(|b| *b == "walls" || *b == "medieval_walls").count() as i32
    }

    /// City ranged strike strength: the strongest ranged unit the owner
    /// fields, or 3 if none (Civ 6 rule).
    pub fn city_ranged_strength(&self, cid: u32) -> f64 {
        let owner = self.cities[&cid].owner;
        let current = self
            .units
            .values()
            .filter(|u| u.owner == owner)
            .map(|u| self.rules.units[u.kind.as_str()].ranged_strength)
            .fold(3.0, f64::max);
        let base = self.players[owner]
            .counters
            .get("strongest_ranged_built")
            .map(|v| *v as f64)
            .unwrap_or(current)
            .max(3.0);
        base + if self.has_policy(owner, "bastions") { 5.0 } else { 0.0 }
    }

    pub fn city_strength(&self, cid: u32) -> f64 {
        let city = &self.cities[&cid];
        let current_best = self
            .units
            .values()
            .filter(|u| u.owner == city.owner)
            .map(|u| self.rules.units[u.kind.as_str()].strength)
            .fold(20.0, f64::max);
        let strongest_built = self.players[city.owner]
            .counters
            .get("strongest_unit_built")
            .map(|v| *v as f64)
            .unwrap_or(current_best);
        let garrison = self
            .units_at(city.pos)
            .into_iter()
            .filter_map(|id| {
                let u = &self.units[&id];
                (u.owner == city.owner
                    && self.rules.units[u.kind.as_str()].class == "military")
                    .then(|| self.rules.units[u.kind.as_str()].strength)
            })
            .fold(0.0, f64::max);
        let mut s = (strongest_built - 10.0).max(garrison).max(10.0);
        if city.wall_hp > 0 {
            // +3 combat strength per standing wall level (Civ 6)
            s += 3.0 * city.buildings.iter()
                .filter(|b| *b == "walls" || *b == "medieval_walls").count() as f64;
        }
        s += 2.0 * city.districts.len() as f64;
        s += self.tile_defense_bonus(city.pos);
        if city.is_capital {
            s += 3.0;
        }
        if self.has_policy(city.owner, "bastions") {
            s += 6.0;
        }
        if self.players[city.owner].is_minor {
            if let Some(suzerain) = self.suzerain_of(city.owner) {
                s += self.envoys_at(suzerain, city.owner) as f64;
            }
        }
        let damaged_penalty = (10.0 - city.hp.clamp(0, 200) as f64 / 20.0).round();
        (s - damaged_penalty).max(0.0)
    }

    /// A city cannot heal when every adjacent passable tile is occupied by a
    /// hostile combat unit or covered by hostile ZOC. Off-map and impassable
    /// neighbors count as sealed sides of the siege ring.
    fn city_under_siege(&self, cid: u32) -> bool {
        let city = &self.cities[&cid];
        crate::hex::neighbors(city.pos)
            .into_iter()
            .map(|p| hex::canon(p, self.map.width))
            .all(|pos| {
                let Some(tile) = self.map.get(pos) else {
                    return true;
                };
                if !self.rules.is_passable(tile) {
                    return true;
                }
                self.units_at(pos).into_iter().any(|id| {
                    let unit = &self.units[&id];
                    unit.owner != city.owner
                        && self.is_at_war(city.owner, unit.owner)
                        && self.rules.units[unit.kind.as_str()].class == "military"
                }) || self.in_enemy_zoc(city.owner, pos)
            })
    }

    pub fn district_yields(&self, dname: &str, dpos: Pos) -> Yields {
        let spec = &self.rules.districts[dname];
        let mut ys = spec.yields;
        if !spec.adjacency.is_empty() {
            let (mut mountain, mut forest, mut district, mut river) = (0, 0, 0, 0);
            if self.map.get(dpos).map(|t| t.river).unwrap_or(false) {
                river = 1;
            }
            for n in self.nbrs(dpos) {
                if let Some(t) = self.map.get(n) {
                    if t.terrain == "mountain" {
                        mountain += 1;
                    }
                    if t.feature.as_deref() == Some("forest") {
                        forest += 1;
                    }
                    if t.district.is_some() {
                        district += 1;
                    }
                    if t.river {
                        river = 1; // flat bonus for river adjacency, Civ 6 style
                    }
                }
            }
            let mut adj = Yields::default();
            for (key, bonus) in &spec.adjacency {
                let n = match key.as_str() {
                    "mountain" => mountain,
                    "forest" => forest,
                    "district" => district,
                    "river" => river,
                    _ => 0,
                } as f64;
                adj.food += (n * bonus.food).trunc();
                adj.production += (n * bonus.production).trunc();
                adj.gold += (n * bonus.gold).trunc();
                adj.science += (n * bonus.science).trunc();
                adj.culture += (n * bonus.culture).trunc();
                adj.faith += (n * bonus.faith).trunc();
            }
            // Town Charters / Craftsmen double the district's adjacency bonus
            let owner = self.map.get(dpos)
                .and_then(|t| t.owner_city)
                .and_then(|oc| self.cities.get(&oc))
                .map(|c| c.owner);
            if let Some(pid) = owner {
                let doubled = (dname == "commercial_hub"
                        && self.has_policy(pid, "town_charters"))
                    || (dname == "industrial_zone"
                        && self.has_policy(pid, "craftsmen"));
                if doubled {
                    adj.add(adj);
                }
            }
            ys.add(adj);
        }
        ys
    }

    /// Build the automatic citizen governor's priorities from three layers:
    /// survival/growth, this city's current role and production, and the
    /// civilization's strengths.  Re-evaluating it from current state means a
    /// city changes jobs immediately when it starts a wonder, goes to war,
    /// reaches its housing cap, or develops a specialty district.
    pub fn citizen_strategy(&self, cid: u32) -> CitizenStrategy {
        let city = &self.cities[&cid];
        let player = &self.players[city.owner];
        let mut weights = Yields {
            food: 1.25,
            production: 1.55,
            gold: 0.85,
            science: 1.30,
            culture: 1.20,
            faith: 0.90,
        };
        let mut focus = "balanced".to_string();

        // Existing districts make cities lean into their established role.
        // This is intentionally based on the district's actual ruleset yields
        // so modded specialty districts inherit sensible behavior.
        let mut specialty = Yields::default();
        for (name, pos) in &city.districts {
            specialty.add(self.district_yields(name, *pos));
        }
        for name in &city.buildings {
            specialty.add(self.rules.buildings[name.as_str()].yields);
        }
        weights.production += specialty.production * 0.12;
        weights.gold += specialty.gold * 0.12;
        weights.science += specialty.science * 0.18;
        weights.culture += specialty.culture * 0.18;
        weights.faith += specialty.faith * 0.18;
        let specialties = [
            (specialty.production, "production"),
            (specialty.gold, "commerce"),
            (specialty.science, "science"),
            (specialty.culture, "culture"),
            (specialty.faith, "faith"),
        ];
        if let Some((amount, name)) = specialties.into_iter()
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap()
                .then_with(|| a.1.cmp(b.1)))
        {
            if amount > 0.0 {
                focus = name.to_string();
            }
        }

        // Production is the immediate city-level plan. Yield-bearing items
        // also reinforce the specialization they are being built to support.
        if let Some(item) = city.queue.first() {
            match item {
                Item::Unit { unit } if unit == "settler" => {
                    focus = "expansion".to_string();
                    weights.food += 0.55;
                    weights.production += 1.15;
                }
                Item::Unit { unit } => {
                    focus = if self.rules.units[unit.as_str()].class == "military" {
                        "military".to_string()
                    } else {
                        "development".to_string()
                    };
                    weights.production += if focus == "military" { 1.35 } else { 0.85 };
                }
                Item::Building { building } => {
                    let spec = &self.rules.buildings[building.as_str()];
                    focus = if spec.wonder { "wonder" } else { "infrastructure" }.to_string();
                    weights.production += if spec.wonder { 1.75 } else { 0.80 };
                    weights.food += spec.yields.food * 0.10;
                    weights.gold += spec.yields.gold * 0.15;
                    weights.science += spec.yields.science * 0.22;
                    weights.culture += spec.yields.culture * 0.22;
                    weights.faith += spec.yields.faith * 0.22;
                }
                Item::District { district, pos } => {
                    focus = district.replace('_', " ");
                    weights.production += 1.00;
                    let dy = self.district_yields(district, *pos);
                    weights.gold += dy.gold * 0.16;
                    weights.science += dy.science * 0.22;
                    weights.culture += dy.culture * 0.22;
                    weights.faith += dy.faith * 0.22;
                }
                Item::Project { .. } => {
                    focus = "space race".to_string();
                    weights.production += 1.75;
                    weights.science += 0.60;
                }
            }
        }

        // Civilization plans use ability keys rather than seat numbers, so a
        // custom ruleset may reorder civilizations without changing behavior.
        match self.rules.civs.get(&player.civ).map(|c| c.ability.as_str()) {
            Some("trajans_column") => {
                weights.production += 0.30;
                weights.culture += 0.55;
            }
            Some("iteru") => {
                weights.production += if self.map.tiles[&city.pos].river { 1.00 } else { 0.55 };
                weights.gold += 0.35;
            }
            Some("platos_republic") => weights.culture += 1.35,
            Some("dynastic_cycle") => {
                weights.production += 0.30;
                weights.science += 0.75;
                weights.culture += 0.75;
            }
            Some("epic_quest") => {
                weights.production += 0.65;
                weights.gold += 0.45;
            }
            Some("gifts_for_the_tlatoani") => {
                weights.food += 0.20;
                weights.production += 0.85;
                weights.gold += 0.40;
            }
            Some("ta_seti") => {
                weights.food += 0.20;
                weights.production += 1.25;
            }
            Some("killer_of_cyrus") => {
                weights.food += 0.35;
                weights.production += 1.05;
            }
            _ => {}
        }

        let at_war = self.players.iter().any(|other| {
            other.id != city.owner && other.alive && !other.is_barbarian
                && self.is_at_war(city.owner, other.id)
        });
        if at_war {
            weights.production += 1.00;
            if city.queue.is_empty() {
                focus = "wartime".to_string();
            }
        }

        let housing_headroom = self.city_housing(city) - city.pop as f64;
        let amenities = self.city_amenity_surplus(city);
        let growth_surplus = if housing_headroom > 1.0 && amenities >= -2 {
            (0.75 + housing_headroom * 0.25).min(2.0)
        } else {
            // Do not sacrifice useful production/science to grow into a hard
            // housing cap; the food constraint below still prevents starvation.
            weights.food *= 0.55;
            0.0
        };
        let food_target = 2.0 * city.pop as f64 + growth_surplus;
        CitizenStrategy { focus, weights, food_target }
    }

    fn citizen_value(ys: Yields, weights: Yields) -> f64 {
        ys.food * weights.food + ys.production * weights.production
            + ys.gold * weights.gold + ys.science * weights.science
            + ys.culture * weights.culture + ys.faith * weights.faith
    }

    /// Choose the actual population-worked tiles. It starts with the highest
    /// strategic-value set, then performs the least-cost swaps needed to hit
    /// the food target. A final local improvement pass recovers strategic
    /// value without violating nutrition. This keeps the hot turn loop fast
    /// while preventing a production-focused governor from starving a city.
    pub fn city_citizen_plan(&self, cid: u32) -> CitizenPlan {
        let city = &self.cities[&cid];
        let strategy = self.citizen_strategy(cid);
        let mut center = self.rules.tile_yields(&self.map.tiles[&city.pos]);
        center.food = center.food.max(2.0);
        center.production = center.production.max(1.0);

        let mut cands: Vec<(Pos, Yields, f64)> = city.owned_tiles.iter()
            .filter(|pos| **pos != city.pos)
            .filter_map(|pos| {
                let tile = &self.map.tiles[pos];
                if tile.district.is_some() {
                    return None;
                }
                let ys = self.rules.tile_yields(tile);
                Some((*pos, ys, Self::citizen_value(ys, strategy.weights)))
            })
            .collect();
        cands.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap().then(a.0.cmp(&b.0)));
        let workers = (city.pop.max(0) as usize).min(cands.len());
        let mut selected = vec![false; cands.len()];
        for slot in selected.iter_mut().take(workers) {
            *slot = true;
        }
        // Fixed food (buildings, districts, routes, envoys, beliefs) satisfies
        // nutrition before citizens are pulled off more valuable jobs. This is
        // important for granary/harbor cities: food infrastructure should let
        // their population work production or specialty yields.
        let mut food = center.food;
        for (name, pos) in &city.districts {
            food += self.district_yields(name, *pos).food;
        }
        for name in &city.buildings {
            food += self.rules.buildings[name.as_str()].yields.food;
        }
        for route in self.routes.iter().filter(|r| r.origin == cid) {
            if let Some(dest) = self.cities.get(&route.dest) {
                food += self.route_yields(route.dest, dest.owner == city.owner).food;
            }
        }
        if !self.players[city.owner].is_minor {
            food += self.envoy_yields(city.owner, city).food;
        }
        if let Some(religion) = self.city_religion(city) {
            let has_shrine = city.buildings.iter().any(|b| b == "shrine");
            if has_shrine && self.founder_has(religion, "feed_the_world") {
                food += 2.0;
            }
        }
        food += cands.iter().enumerate()
            .filter(|(i, _)| selected[*i]).map(|(_, c)| c.1.food).sum::<f64>();

        // Meet nutrition through the smallest strategic-value sacrifice per
        // useful food. The loop is bounded by the candidate count because
        // every accepted swap strictly raises collected food.
        for _ in 0..cands.len() {
            if food + 1e-9 >= strategy.food_target {
                break;
            }
            let need = strategy.food_target - food;
            let mut best: Option<(f64, f64, Pos, Pos, usize, usize)> = None;
            for (out, a) in cands.iter().enumerate().filter(|(i, _)| selected[*i]) {
                for (inside, b) in cands.iter().enumerate().filter(|(i, _)| !selected[*i]) {
                    let food_gain = b.1.food - a.1.food;
                    if food_gain <= 1e-9 {
                        continue;
                    }
                    let value_gain = b.2 - a.2;
                    let useful_food = food_gain.min(need);
                    let efficiency = value_gain / useful_food;
                    let candidate = (efficiency, value_gain, a.0, b.0, out, inside);
                    if best.as_ref().map(|old| {
                        candidate.0 > old.0 + 1e-9
                            || ((candidate.0 - old.0).abs() < 1e-9
                                && (candidate.1 > old.1 + 1e-9
                                    || ((candidate.1 - old.1).abs() < 1e-9
                                        && (candidate.2, candidate.3) < (old.2, old.3))))
                    }).unwrap_or(true) {
                        best = Some(candidate);
                    }
                }
            }
            match best {
                Some((_, _, _, _, out, inside)) => {
                    selected[out] = false;
                    selected[inside] = true;
                    food += cands[inside].1.food - cands[out].1.food;
                }
                None => break,
            }
        }

        // One-swap local optimum under the nutrition constraint.
        for _ in 0..cands.len() {
            let mut best: Option<(f64, Pos, Pos, usize, usize)> = None;
            for (out, a) in cands.iter().enumerate().filter(|(i, _)| selected[*i]) {
                for (inside, b) in cands.iter().enumerate().filter(|(i, _)| !selected[*i]) {
                    let value_gain = b.2 - a.2;
                    let next_food = food + b.1.food - a.1.food;
                    if value_gain <= 1e-9 || next_food + 1e-9 < strategy.food_target {
                        continue;
                    }
                    let candidate = (value_gain, a.0, b.0, out, inside);
                    if best.as_ref().map(|old| {
                        candidate.0 > old.0 + 1e-9
                            || ((candidate.0 - old.0).abs() < 1e-9
                                && (candidate.1, candidate.2) < (old.1, old.2))
                    }).unwrap_or(true) {
                        best = Some(candidate);
                    }
                }
            }
            match best {
                Some((_, _, _, out, inside)) => {
                    selected[out] = false;
                    selected[inside] = true;
                    food += cands[inside].1.food - cands[out].1.food;
                }
                None => break,
            }
        }

        let mut worked_tiles: Vec<Pos> = cands.iter().enumerate()
            .filter(|(i, _)| selected[*i]).map(|(_, c)| c.0).collect();
        worked_tiles.sort();
        CitizenPlan { strategy, worked_tiles }
    }

    pub fn city_yields(&self, cid: u32) -> Yields {
        let city = &self.cities[&cid];
        let mut ys = Yields::default();
        let mut center = self.rules.tile_yields(&self.map.tiles[&city.pos]);
        center.food = center.food.max(2.0);
        center.production = center.production.max(1.0);
        ys.add(center);
        for pos in self.city_citizen_plan(cid).worked_tiles {
            ys.add(self.rules.tile_yields(&self.map.tiles[&pos]));
        }
        for (dname, dpos) in &city.districts {
            ys.add(self.district_yields(dname, *dpos));
        }
        for b in &city.buildings {
            ys.add(self.rules.buildings[b.as_str()].yields);
        }
        ys.science += 0.5 * city.pop as f64;
        ys.culture += 0.3 * city.pop as f64;
        if city.is_capital && city.owner == city.original_owner {
            ys.gold += 3.0;
            ys.science += 1.0;
            ys.culture += 1.0;
            if self.has_policy(city.owner, "god_king") {
                ys.gold += 1.0;
                ys.faith += 1.0;
            }
        }
        if self.has_policy(city.owner, "urban_planning") {
            ys.production += 1.0;
        }
        if self.has_policy(city.owner, "meritocracy") {
            ys.culture += city.districts.len() as f64;
        }
        for r in self.routes.iter().filter(|r| r.origin == cid) {
            if let Some(dc) = self.cities.get(&r.dest) {
                let mut rys = self.route_yields(r.dest, dc.owner == city.owner);
                if self.has_policy(city.owner, "caravansaries") {
                    rys.gold += 2.0;
                }
                ys.add(rys);
            }
        }
        if !self.players[city.owner].is_minor {
            ys.add(self.envoy_yields(city.owner, city));
        }
        if let Some(r) = self.city_religion(city) {
            let r = r.to_string();
            let has_shrine = city.buildings.iter().any(|b| b == "shrine");
            if has_shrine && self.founder_has(&r, "choral_music") {
                ys.culture += 2.0;
            }
            if has_shrine && self.founder_has(&r, "feed_the_world") {
                ys.food += 2.0;
            }
            if self.founder_has(&r, "work_ethic")
                && city.districts.contains_key("holy_site") {
                ys.production += 1.0;
            }
        }
        if self.has_ability(city.owner, "platos_republic") {
            let suz = self.players.iter()
                .filter(|m| m.is_minor && !m.is_barbarian && m.alive)
                .filter(|m| self.suzerain_of(m.id) == Some(city.owner))
                .count() as f64;
            ys.culture *= 1.0 + 0.05 * suz; // Surrounded by Glory
        }
        match self.players[city.owner].pantheon.as_deref() {
            Some("god_of_the_open_sky") => {
                ys.culture += city.owned_tiles.iter().filter(|p| {
                    self.map.tiles[p].improvement.as_deref() == Some("pasture")
                }).count() as f64;
            }
            Some("god_of_the_sea") => {
                ys.production += city.owned_tiles.iter().filter(|p| {
                    self.map.tiles[p].improvement.as_deref() == Some("fishing_boats")
                }).count() as f64;
            }
            _ => {}
        }
        let eff = self.gov_effects(city.owner);
        ys.production *= 1.0 + eff.production_pct / 100.0;
        ys.science *= 1.0 + eff.science_pct / 100.0;
        ys.gold *= 1.0 + eff.gold_pct / 100.0;
        let mut m = self.amenity_yield_mult(city);
        m *= match self.players[city.owner].age.as_str() {
            "golden" => 1.10,
            "dark" => 0.95,
            _ => 1.0,
        };
        ys.production *= m;
        ys.gold *= m;
        ys.science *= m;
        ys.culture *= m;
        ys.faith *= m;
        ys
    }

    pub fn valid_improvements(&self, pid: usize, pos: Pos) -> Vec<String> {
        let t = match self.map.get(pos) {
            Some(t) => t,
            None => return vec![],
        };
        if t.district.is_some() || self.city_at(pos).is_some() {
            return vec![];
        }
        let oc = match t.owner_city {
            Some(oc) => oc,
            None => return vec![],
        };
        if self.cities[&oc].owner != pid {
            return vec![];
        }
        let mut out = Vec::new();
        if let Some(res) = &t.resource {
            let imp = self.rules.resources[res.as_str()].improvement.clone();
            let spec = &self.rules.improvements[imp.as_str()];
            if self.unlocked(pid, &spec.tech, &None)
                && self.rules.is_water(t) == spec.water
                && t.improvement.as_deref() != Some(imp.as_str())
            {
                out.push(imp);
            }
        } else if !self.rules.is_water(t) {
            if t.hills {
                let spec = &self.rules.improvements["mine"];
                if self.unlocked(pid, &spec.tech, &None)
                    && t.improvement.as_deref() != Some("mine")
                {
                    out.push("mine".to_string());
                }
            } else {
                let spec = &self.rules.improvements["farm"];
                if spec.terrain.contains(&t.terrain)
                    && self.unlocked(pid, &spec.tech, &None)
                    && t.improvement.as_deref() != Some("farm")
                {
                    out.push("farm".to_string());
                }
            }
        }
        out
    }

    pub fn district_sites(&self, cid: u32, dname: &str) -> Vec<Pos> {
        let city = &self.cities[&cid];
        let want_water = self.rules.districts[dname].water;
        let mut out = Vec::new();
        for pos in &city.owned_tiles {
            if *pos == city.pos || self.wdist(*pos, city.pos) > 3 {
                continue;
            }
            let t = &self.map.tiles[pos];
            if t.district.is_some() || t.resource.is_some() || !self.rules.is_passable(t) {
                continue;
            }
            if self.rules.is_water(t) != want_water {
                continue;
            }
            out.push(*pos);
        }
        out.sort();
        out
    }

    pub fn item_cost(&self, item: &Item) -> f64 {
        match item {
            Item::Unit { unit } => self.rules.units[unit.as_str()].cost,
            Item::Building { building } => self.rules.buildings[building.as_str()].cost,
            Item::District { district, .. } => self.rules.districts[district.as_str()].cost,
            Item::Project { project } => self.rules.projects[project.as_str()].cost,
        }
    }

    pub fn can_produce(&self, pid: usize, cid: u32, item: &Item) -> bool {
        let city = &self.cities[&cid];
        match item {
            Item::Unit { unit } => {
                let spec = match self.rules.units.get(unit) {
                    Some(s) => s,
                    None => return false,
                };
                if !self.unlocked(pid, &spec.tech, &spec.civic) {
                    return false;
                }
                if spec.class == "religious" {
                    return false; // faith purchase only (Civ 6)
                }
                if let Some(civ) = &spec.unique_to {
                    if self.players[pid].civ != *civ {
                        return false; // another civ's unique unit
                    }
                }
                // a civ with a unique replacement cannot build the base unit
                let replaced = self.rules.units.values().any(|s| {
                    s.replaces.as_deref() == Some(unit.as_str())
                        && s.unique_to.as_deref()
                            == Some(self.players[pid].civ.as_str())
                });
                if replaced {
                    return false;
                }
                if let Some(res) = &spec.requires_resource {
                    if !self.has_resource(pid, res) {
                        return false;
                    }
                }
                if spec.domain.as_deref() == Some("sea") {
                    let coastal = self.nbrs(city.pos).iter().any(|n| {
                        self.map.get(*n).map(|t| self.rules.is_water(t)).unwrap_or(false)
                    });
                    if !coastal {
                        return false;
                    }
                }
                true
            }
            Item::Building { building } => {
                let spec = match self.rules.buildings.get(building) {
                    Some(s) => s,
                    None => return false,
                };
                if city.buildings.contains(building) || !self.unlocked(pid, &spec.tech, &spec.civic) {
                    return false;
                }
                if building == "medieval_walls"
                    && (!city.buildings.iter().any(|built| built == "walls")
                        || city.wall_hp < self.city_max_wall_hp(city))
                {
                    return false;
                }
                if spec.wonder && self.wonder_built(building) {
                    return false; // one per world
                }
                if spec.coastal {
                    let ok = self.nbrs(city.pos).iter().any(|n| {
                        self.map.get(*n).map(|t| self.rules.is_water(t)).unwrap_or(false)
                    });
                    if !ok {
                        return false;
                    }
                }
                match &spec.district {
                    None => true,
                    Some(d) => city.districts.contains_key(d),
                }
            }
            Item::District { district, pos } => {
                let spec = match self.rules.districts.get(district) {
                    Some(s) => s,
                    None => return false,
                };
                if city.districts.contains_key(district)
                    || !self.unlocked(pid, &spec.tech, &spec.civic)
                {
                    return false;
                }
                self.district_sites(cid, district).contains(pos)
            }
            Item::Project { project } => {
                if project == "repair_outer_defenses" {
                    let max = self.city_max_wall_hp(city);
                    return max > 0
                        && city.wall_hp < max
                        && self.turn.saturating_sub(city.last_attacked) >= 3;
                }
                let spec = match self.rules.projects.get(project) {
                    Some(s) => s,
                    None => return false,
                };
                if self.players[pid].is_minor || self.players[pid].is_barbarian {
                    return false;
                }
                if spec.tech.as_ref().is_some_and(|t| !self.players[pid].techs.contains(t)) {
                    return false;
                }
                if spec.district.as_ref().is_some_and(|d| !city.districts.contains_key(d)) {
                    return false;
                }
                if !spec
                    .requires
                    .iter()
                    .all(|required| self.players[pid].science_projects.contains(required))
                {
                    return false;
                }
                spec.repeatable || !self.players[pid].science_projects.contains(project)
            }
        }
    }

    pub fn producible_items(&self, pid: usize, cid: u32) -> Vec<Item> {
        let mut items = Vec::new();
        for name in self.rules.units.keys() {
            let it = Item::Unit { unit: name.clone() };
            if self.can_produce(pid, cid, &it) {
                items.push(it);
            }
        }
        for name in self.rules.buildings.keys() {
            let it = Item::Building { building: name.clone() };
            if self.can_produce(pid, cid, &it) {
                items.push(it);
            }
        }
        for name in self.rules.projects.keys() {
            let it = Item::Project { project: name.clone() };
            if self.can_produce(pid, cid, &it) {
                items.push(it);
            }
        }
        let city = &self.cities[&cid];
        for (name, spec) in &self.rules.districts {
            if city.districts.contains_key(name)
                || !self.unlocked(pid, &spec.tech, &spec.civic)
            {
                continue;
            }
            let mut sites = self.district_sites(cid, name);
            sites.sort_by(|a, b| {
                let ya = self.district_yields(name, *a).total();
                let yb = self.district_yields(name, *b).total();
                yb.partial_cmp(&ya).unwrap().then(a.cmp(b))
            });
            for s in sites.into_iter().take(2) {
                items.push(Item::District { district: name.clone(), pos: s });
            }
        }
        items
    }

    // ------------------------------------------------------- action layer

    pub fn legal_actions(&self, pid: usize) -> Vec<Action> {
        if self.winner.is_some() || self.current != pid {
            return vec![];
        }
        let p = &self.players[pid];
        let mut acts = Vec::new();
        for uid in self.player_unit_ids(pid) {
            let u = self.units[&uid].clone();
            let spec = self.rules.units[u.kind.as_str()].clone();
            let embarked = self.is_embarked(&u);
            if u.moves_left > 0.0 {
                for n in self.nbrs(u.pos) {
                    if self.can_move(uid, n) {
                        acts.push(Action::Move { unit: uid, to: n });
                    }
                }
                if spec.class == "military" && !embarked {
                    if spec.has_ranged_attack() {
                        if !spec.siege || !u.acted {
                            for pos in self.wdisk(u.pos, spec.range.max(1)) {
                                if pos == u.pos || !self.map.tiles.contains_key(&pos) {
                                    continue;
                                }
                                if self.enemy_combat_target_at(pid, pos)
                                    && self.has_line_of_sight(u.pos, pos, true)
                                {
                                    acts.push(Action::Ranged { unit: uid, target: pos });
                                }
                            }
                        }
                    } else {
                        for pos in self.nbrs(u.pos) {
                            if self.map.tiles.contains_key(&pos)
                                && self.enemy_combat_target_at(pid, pos)
                                && self.can_pay_melee_entry(uid, pos)
                            {
                                acts.push(Action::Attack { unit: uid, target: pos });
                            }
                        }
                    }
                }
            }
            if u.kind == "settler" && self.can_found_city(uid) {
                acts.push(Action::FoundCity { unit: uid });
            }
            if u.kind == "builder" && u.charges > 0 {
                for imp in self.valid_improvements(pid, u.pos) {
                    acts.push(Action::Improve { unit: uid, improvement: imp });
                }
            }
        }
        for cid in self.player_city_ids(pid) {
            for item in self.producible_items(pid, cid) {
                acts.push(Action::Produce { city: cid, item });
            }
            for utype in ["builder", "settler", "warrior", "archer", "spearman"] {
                let it = Item::Unit { unit: utype.to_string() };
                if self.can_produce(pid, cid, &it) {
                    let cost = self.rules.units[utype].cost;
                    if p.gold >= cost * 4.0 {
                        acts.push(Action::Buy {
                            city: cid,
                            unit: utype.to_string(),
                            currency: "gold".to_string(),
                        });
                    }
                    if p.faith >= cost * 2.0 && (utype == "builder" || utype == "settler") {
                        acts.push(Action::Buy {
                            city: cid,
                            unit: utype.to_string(),
                            currency: "faith".to_string(),
                        });
                    }
                }
            }
        }
        if p.research.is_none() {
            for t in self.available_techs(pid) {
                acts.push(Action::Research { tech: t });
            }
        }
        if p.civic.is_none() {
            for c in self.available_civics(pid) {
                acts.push(Action::Civic { civic: c });
            }
        }
        for uid in self.player_unit_ids(pid) {
            let u = self.units[&uid].clone();
            if self.unit_can_fortify(&u) && u.moves_left > 0.0 && !u.fortified {
                acts.push(Action::Fortify { unit: uid });
            }
        }
        for cid in self.player_city_ids(pid) {
            if self.city_can_strike(&self.cities[&cid]) {
                let cpos = self.cities[&cid].pos;
                for pos in self.wdisk(cpos, 2) {
                    if !self.map.tiles.contains_key(&pos) {
                        continue;
                    }
                    let hit = self.units_at(pos).into_iter().any(|oid| {
                        let o = &self.units[&oid];
                        o.owner != pid && self.is_at_war(pid, o.owner)
                    });
                    if hit
                        && self.city_at(pos).is_none()
                        && self.has_line_of_sight(cpos, pos, true)
                    {
                        acts.push(Action::CityStrike { city: cid, target: pos });
                    }
                }
            }
        }
        if !p.is_minor {
            for (g, spec) in &self.rules.governments {
                let ok = spec.civic.as_ref()
                    .map(|c| p.civics.contains(c)).unwrap_or(true);
                if ok && p.government.as_deref() != Some(g.as_str()) {
                    acts.push(Action::Government { government: g.clone() });
                }
            }
            for card in self.available_policies(pid) {
                let mut next = p.policies.clone();
                next.insert(card.clone());
                if self.policies_fit(pid, &next) {
                    acts.push(Action::SlotPolicy { policy: card });
                }
            }
            for card in &p.policies {
                acts.push(Action::UnslotPolicy { policy: card.clone() });
            }
            if self.active_routes(pid) < self.trade_capacity(pid) {
                for uid in self.player_unit_ids(pid) {
                    if self.units[&uid].kind != "trader" {
                        continue;
                    }
                    let origin = match self.city_at(self.units[&uid].pos) {
                        Some(cid) if self.cities[&cid].owner == pid => cid,
                        _ => continue,
                    };
                    for (dest, dc) in &self.cities {
                        if *dest == origin || self.is_at_war(pid, dc.owner)
                            || self.wdist(self.cities[&origin].pos, dc.pos) > 15
                            || self.routes.iter()
                                .any(|r| r.origin == origin && r.dest == *dest) {
                            continue;
                        }
                        acts.push(Action::TradeRoute { unit: uid, city: *dest });
                    }
                }
            }
            if p.envoys_free > 0 {
                for m in &self.players {
                    if m.is_minor && !m.is_barbarian && m.alive
                        && !self.is_at_war(pid, m.id) {
                        acts.push(Action::SendEnvoy { player: m.id });
                    }
                }
            }
            if p.pantheon.is_none() && p.faith >= 25.0 {
                for b in self.rules.beliefs.pantheon.keys() {
                    if !self.players.iter()
                        .any(|o| o.pantheon.as_deref() == Some(b.as_str())) {
                        acts.push(Action::ChoosePantheon { belief: b.clone() });
                    }
                }
            }
            if p.prophet_pending && self.religions_founded() < self.max_religions() {
                let taken = |b: &str| self.players.iter()
                    .any(|o| o.religion_beliefs.iter().any(|x| x == b));
                for fo in self.rules.beliefs.follower.keys().filter(|b| !taken(b)) {
                    for fu in self.rules.beliefs.founder.keys().filter(|b| !taken(b)) {
                        acts.push(Action::FoundReligion {
                            follower: fo.clone(), founder: fu.clone() });
                    }
                }
            }
            for uid in self.player_unit_ids(pid) {
                let u = &self.units[&uid];
                if u.kind == "missionary" && u.charges > 0 && u.moves_left > 0.0 {
                    let near_city = self.city_at(u.pos).is_some()
                        || self.nbrs(u.pos).iter().any(|n| self.city_at(*n).is_some());
                    if near_city {
                        acts.push(Action::Spread { unit: uid });
                    }
                }
            }
            if p.governors.len() < self.governor_titles(pid) {
                for cid in self.player_city_ids(pid) {
                    if !p.governors.contains(&cid) {
                        acts.push(Action::AssignGovernor { city: cid });
                    }
                }
            }
            if p.religion.is_some() && p.faith >= 200.0 {
                for cid in self.player_city_ids(pid) {
                    if self.cities[&cid].districts.contains_key("holy_site") {
                        acts.push(Action::Buy {
                            city: cid,
                            unit: "missionary".to_string(),
                            currency: "faith".to_string(),
                        });
                    }
                }
            }
        }
        if !p.is_barbarian {
            for o in &self.players {
                if o.id != pid && o.alive && !o.is_barbarian {
                    if self.is_at_war(pid, o.id) {
                        acts.push(Action::MakePeace { player: o.id });
                    } else {
                        acts.push(Action::DeclareWar { player: o.id });
                    }
                }
            }
        }
        acts.push(Action::EndTurn);
        acts
    }

    fn enemy_combat_target_at(&self, pid: usize, pos: Pos) -> bool {
        for oid in self.units_at(pos) {
            let unit = &self.units[&oid];
            if unit.owner != pid
                && self.is_at_war(pid, unit.owner)
                && self.rules.units[unit.kind.as_str()].class == "military"
            {
                return true;
            }
        }
        if let Some(cid) = self.city_at(pos) {
            let owner = self.cities[&cid].owner;
            return owner != pid && self.is_at_war(pid, owner);
        }
        false
    }

    pub fn apply(&mut self, pid: usize, action: &Action) -> Result<(), String> {
        if self.winner.is_some() {
            return Err("game over".into());
        }
        if self.current != pid {
            return Err("not your turn".into());
        }
        let r = match action {
            Action::Move { unit, to } => self.do_move(pid, *unit, *to),
            Action::MoveTo { unit, to } => self.do_move_to(pid, *unit, *to),
            Action::Attack { unit, target } => self.do_attack(pid, *unit, *target),
            Action::Ranged { unit, target } => self.do_ranged(pid, *unit, *target),
            Action::FoundCity { unit } => self.do_found_city(pid, *unit),
            Action::Improve { unit, improvement } => self.do_improve(pid, *unit, improvement),
            Action::Produce { city, item } => self.do_produce(pid, *city, item),
            Action::Buy { city, unit, currency } => self.do_buy(pid, *city, unit, currency),
            Action::Research { tech } => self.do_research(pid, tech),
            Action::Civic { civic } => self.do_civic(pid, civic),
            Action::DeclareWar { player } => self.do_declare_war(pid, *player),
            Action::MakePeace { player } => self.do_make_peace(pid, *player),
            Action::Fortify { unit } => self.do_fortify(pid, *unit),
            Action::Government { government } => self.do_government(pid, government),
            Action::SlotPolicy { policy } => self.do_slot_policy(pid, policy),
            Action::UnslotPolicy { policy } => self.do_unslot_policy(pid, policy),
            Action::TradeRoute { unit, city } => self.do_trade_route(pid, *unit, *city),
            Action::SendEnvoy { player } => self.do_send_envoy(pid, *player),
            Action::ChoosePantheon { belief } => self.do_choose_pantheon(pid, belief),
            Action::AssignGovernor { city } => self.do_assign_governor(pid, *city),
            Action::FoundReligion { follower, founder } =>
                self.do_found_religion(pid, follower, founder),
            Action::Spread { unit } => self.do_spread(pid, *unit),
            Action::CityStrike { city, target } => self.do_city_strike(pid, *city, *target),
            Action::EndTurn => {
                self.do_end_turn();
                Ok(())
            }
        };
        if r.is_ok() {
            self.log.push((pid, action.clone()));
        }
        r
    }

    fn own_unit(&self, pid: usize, uid: u32) -> Result<Unit, String> {
        match self.units.get(&uid) {
            Some(u) if u.owner == pid => Ok(u.clone()),
            _ => Err("not your unit".into()),
        }
    }

    fn do_move(&mut self, pid: usize, uid: u32, to: Pos) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        if u.moves_left <= 0.0 {
            return Err("no moves left".into());
        }
        if u.zoc_stopped {
            return Err("stopped by zone of control".into());
        }
        if !self.can_move(uid, to) {
            return Err("invalid move".into());
        }
        self.resolve_entered_units(uid, to);
        let cost = self.step_cost(u.pos, to);
        let spec = self.rules.units[u.kind.as_str()].clone();
        {
            let mu = self.units.get_mut(&uid).unwrap();
            mu.fortified = false;
            mu.fortify_turns = 0;
            mu.acted = true;
        }
        self.relocate(uid, to);
        let mu = self.units.get_mut(&uid).unwrap();
        mu.moves_left = (mu.moves_left - cost).max(0.0);
        if !spec.cavalry && self.in_enemy_zoc_for(uid, to) {
            let mu = self.units.get_mut(&uid).unwrap();
            if matches!(spec.class.as_str(), "military" | "religious") {
                mu.zoc_stopped = true; // may still attack or use religious actions
            } else {
                mu.moves_left = 0.0; // civilian/support units lose all movement
                mu.zoc_stopped = true;
            }
        }
        self.maybe_clear_camp(uid);
        self.maybe_goody_hut(uid);
        Ok(())
    }

    fn tile_defense_bonus(&self, pos: Pos) -> f64 {
        let t = &self.map.tiles[&pos];
        let mut bonus = 0.0;
        if t.hills {
            bonus += 3.0;
        }
        match t.feature.as_deref() {
            Some("forest" | "jungle" | "great_barrier_reef") => bonus += 3.0,
            Some("marsh") => bonus -= 2.0,
            _ => {}
        }
        bonus
    }

    fn matchup_bonus(&self, uid: u32, opponent: &Unit, attacking: bool) -> f64 {
        let u = &self.units[&uid];
        let spec = &self.rules.units[u.kind.as_str()];
        let other = &self.rules.units[opponent.kind.as_str()];
        let mut bonus = 0.0;
        if spec.promotion_class == "anti_cavalry"
            && (matches!(
                other.promotion_class.as_str(),
                "light_cavalry" | "heavy_cavalry"
            ) || (other.cavalry && other.promotion_class == "ranged"))
            && opponent.kind != "war_cart"
        {
            bonus += 10.0;
        }
        if spec.promotion_class == "melee" && other.promotion_class == "anti_cavalry" {
            bonus += 5.0;
        }
        if u.kind == "hoplite" && self.nbrs(u.pos).into_iter().any(|p| {
            self.units_at(p).into_iter().any(|id| id != uid
                && self.units[&id].owner == u.owner
                && self.units[&id].kind == "hoplite")
        }) {
            bonus += 10.0;
        }
        if attacking && self.has_ability(u.owner, "killer_of_cyrus") && opponent.hp < 100 {
            bonus += 5.0;
        }
        bonus
    }

    fn eagle_capture_chance(&self, uid: u32, opponent: &Unit) -> f64 {
        let unit = &self.units[&uid];
        if unit.kind != "eagle_warrior"
            || self.players[opponent.owner].is_barbarian
            || self.rules.units[opponent.kind.as_str()].class != "military"
        {
            return 0.0;
        }
        let attacker = self.rules.units[unit.kind.as_str()].strength;
        let defender = self.rules.units[opponent.kind.as_str()].strength;
        (50.0 + (attacker - defender) * 2.5).clamp(0.0, 100.0)
    }

    fn flanking_support_unlocked(&self, owner: usize) -> bool {
        if self.players[owner].is_barbarian {
            let majors: Vec<&Player> = self
                .players
                .iter()
                .filter(|p| !p.is_minor && !p.is_barbarian)
                .collect();
            !majors.is_empty()
                && 2 * majors
                    .iter()
                    .filter(|p| p.civics.contains("military_tradition"))
                    .count()
                    >= majors.len()
        } else {
            self.players[owner].civics.contains("military_tradition")
        }
    }

    fn flanking_bonus(&self, uid: u32, target: Pos) -> f64 {
        let owner = self.units[&uid].owner;
        if !self.flanking_support_unlocked(owner) {
            return 0.0;
        }
        let additional = self.nbrs(target).into_iter()
            .flat_map(|p| self.units_at(p))
            .filter(|id| *id != uid)
            .filter(|id| {
                let u = &self.units[id];
                u.owner == owner
                    && self.rules.units[u.kind.as_str()].class == "military"
                    && !self.is_embarked(u)
                    && !self.crosses_river(u.pos, target)
            })
            .count();
        2.0 * additional as f64
    }

    /// Melee attacks pay the movement cost of entering the defender's tile.
    /// As with ordinary movement, a unit that has all of its Movement may
    /// always perform one attack even when the terrain costs more than its
    /// maximum Movement.
    fn can_pay_melee_entry(&self, uid: u32, target: Pos) -> bool {
        let u = &self.units[&uid];
        if !self.map.tiles.contains_key(&target) {
            return false;
        }
        u.moves_left >= self.unit_max_moves(uid)
            || u.moves_left >= self.step_cost(u.pos, target)
    }

    fn support_bonus(&self, defender: &Unit) -> f64 {
        if !self.flanking_support_unlocked(defender.owner) {
            return 0.0;
        }
        let adjacent = self.nbrs(defender.pos).into_iter()
            .flat_map(|p| self.units_at(p))
            .filter(|id| {
                let u = &self.units[id];
                u.owner == defender.owner
                    && self.rules.units[u.kind.as_str()].class == "military"
            })
            .count();
        2.0 * adjacent as f64
    }

    fn do_attack(&mut self, pid: usize, uid: u32, target: Pos) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        let spec = self.rules.units[u.kind.as_str()].clone();
        if !spec.is_melee_capable() {
            return Err("unit cannot melee attack".into());
        }
        let amphibious = self.is_embarked(&u);
        if u.moves_left <= 0.0 {
            return Err("no moves left".into());
        }
        if self.wdist(u.pos, target) != 1 {
            return Err("target not adjacent".into());
        }
        if !self.can_pay_melee_entry(uid, target) {
            return Err("not enough movement to attack".into());
        }
        if amphibious && self.map.get(target)
            .map(|t| self.rules.is_water(t)).unwrap_or(true)
        {
            return Err("embarked units can only attack onto land".into());
        }
        let enemy_ids: Vec<u32> = self
            .units_at(target)
            .into_iter()
            .filter(|id| {
                let owner = self.units[id].owner;
                owner != pid && self.is_at_war(pid, owner)
            })
            .collect();
        let mut city_id = self.city_at(target);
        if let Some(cid) = city_id {
            let owner = self.cities[&cid].owner;
            if owner == pid || !self.is_at_war(pid, owner) {
                city_id = None;
            }
        }
        if enemy_ids.is_empty() && city_id.is_none() {
            return Err("nothing to attack".into());
        }
        let military: Vec<u32> = enemy_ids
            .iter()
            .cloned()
            .filter(|id| self.rules.units[self.units[id].kind.as_str()].class == "military")
            .collect();
        if military.is_empty() && city_id.is_none() {
            return Err("no combat target".into());
        }
        {
            let mu = self.units.get_mut(&uid).unwrap();
            mu.moves_left = 0.0;
            mu.fortified = false;
            mu.fortify_turns = 0;
            mu.acted = true;
        }
        // A unit garrisoned in a City Center cannot be targeted directly;
        // attacks hit the city and the garrison only affects its strength.
        if city_id.is_none() && !military.is_empty() {
            let did = *military
                .iter()
                .max_by(|a, b| {
                    let ea = effective_strength(
                        self.unit_strength(&self.units[*a], true), self.units[*a].hp);
                    let eb = effective_strength(
                        self.unit_strength(&self.units[*b], true), self.units[*b].hp);
                    ea.partial_cmp(&eb).unwrap()
                })
                .unwrap();
            let d = self.units[&did].clone();
            let attacker = self.units[&uid].clone();
            let mut att_base = self.unit_unembarked_strength(&attacker)
                + self.matchup_bonus(uid, &d, true)
                + self.flanking_bonus(uid, target)
                + self.vs_bonus(pid, d.owner);
            if amphibious {
                att_base -= 10.0;
            }
            let mut def_base = self.unit_strength(&d, true)
                + self.matchup_bonus(did, &attacker, false)
                + self.tile_defense_bonus(target)
                + self.support_bonus(&d)
                + self.vs_bonus(d.owner, pid);
            if self.crosses_river(u.pos, target) {
                def_base += 5.0;
            }
            let att = effective_strength(att_base, attacker.hp);
            let ds = effective_strength(def_base, d.hp);
            let dmg_out = damage(att, ds, &mut self.rng);
            let dmg_in = damage(ds, att, &mut self.rng);
            self.units.get_mut(&did).unwrap().hp -= dmg_out;
            self.units.get_mut(&uid).unwrap().hp -= dmg_in;
            let d_dead = self.units[&did].hp <= 0;
            let downer = self.units[&did].owner;
            if d_dead {
                if self.has_ability(pid, "killer_of_cyrus") {
                    if let Some(mu) = self.units.get_mut(&uid) {
                        mu.hp = (mu.hp + 30).min(100); // Tomyris
                    }
                }
            }
            let attacker_dead = self.units[&uid].hp <= 0;
            if !attacker_dead {
                self.award_unit_combat_xp(uid, &d, false, true, d_dead);
            }
            if !d_dead {
                self.award_unit_combat_xp(did, &attacker, false, false, attacker_dead);
            }
            let capture_chance = self.eagle_capture_chance(uid, &d);
            let captured_as_builder = d_dead
                && !attacker_dead
                && capture_chance > 0.0
                && self.rng.uniform(0.0, 100.0) < capture_chance;
            if d_dead {
                bump(&mut self.players[pid], "kills");
                self.remove_unit(did);
                self.on_unit_lost(downer);
                if captured_as_builder {
                    self.spawn_unit("builder", pid, target);
                }
            }
            if attacker_dead {
                if self.units.contains_key(&uid) {
                    bump(&mut self.players[downer], "kills");
                    self.remove_unit(uid);
                    self.on_unit_lost(pid);
                }
                return Ok(());
            }
            if d_dead {
                let enemy_military_left = self.units_at(target).into_iter().any(|id| {
                    let o = &self.units[&id];
                    o.owner != pid && self.rules.units[o.kind.as_str()].class == "military"
                });
                if !enemy_military_left {
                    let city_blocks = match city_id {
                        Some(cid) => self.cities.get(&cid).map(|c| c.hp > 0).unwrap_or(false),
                        None => false,
                    };
                    if !city_blocks {
                        self.enter_tile(uid, target);
                    }
                }
            }
        } else if let Some(cid) = city_id {
            if self.cities[&cid].hp > 0 {
                let attacker = self.units[&uid].clone();
                let mut att_base = self.unit_unembarked_strength(&attacker)
                    + self.vs_bonus(pid, self.cities[&cid].owner);
                if amphibious {
                    att_base -= 10.0;
                }
                let att = effective_strength(att_base, attacker.hp);
                let cs = self.city_strength(cid)
                    + if self.crosses_river(u.pos, target) { 5.0 } else { 0.0 };
                let dmg_out = damage(att, cs, &mut self.rng);
                let dmg_in = damage(cs, att, &mut self.rng);
                let walls = self.cities[&cid].buildings.iter()
                    .filter(|b| *b == "walls" || *b == "medieval_walls").count();
                let support = |kind: &str| self.nbrs(target).into_iter().any(|p| {
                    self.units_at(p).into_iter().any(|id| {
                        let o = &self.units[&id];
                        o.owner == pid && o.kind == kind
                    })
                });
                // battering ram: full melee damage vs ancient walls;
                // siege tower: only melee/anti-cavalry pour through the walls
                let support_eligible = matches!(spec.promotion_class.as_str(),
                    "melee" | "anti_cavalry");
                let ram = support_eligible && support("battering_ram") && walls <= 1;
                let tower = support_eligible && support("siege_tower") && walls <= 2;
                let mult = if ram { 1.0 } else { 0.15 };
                self.city_take_damage(cid, dmg_out, mult, tower);
                self.units.get_mut(&uid).unwrap().hp -= dmg_in;
                if self.units[&uid].hp <= 0 {
                    self.remove_unit(uid);
                    self.on_unit_lost(pid);
                    let c = self.cities.get_mut(&cid).unwrap();
                    c.hp = c.hp.max(1);
                    return Ok(());
                }
                if self.cities[&cid].hp <= 0 {
                    if self.players[pid].is_barbarian {
                        self.award_xp(uid, 3.0);
                        self.cities.get_mut(&cid).unwrap().hp = 1;
                    } else {
                        self.award_xp(uid, 10.0);
                        self.capture_city(cid, pid);
                        self.enter_tile(uid, target);
                    }
                } else {
                    self.award_xp(uid, 3.0);
                }
            } else if self.players[pid].is_barbarian {
                self.cities.get_mut(&cid).unwrap().hp = 1;
            } else {
                // A previous ranged attack may have depleted the garrison
                // health. The melee unit captures it but earns no XP for an
                // attack made after the city was already at 0 HP.
                self.capture_city(cid, pid);
                self.enter_tile(uid, target);
            }
        }
        Ok(())
    }

    fn enter_tile(&mut self, uid: u32, pos: Pos) {
        self.resolve_entered_units(uid, pos);
        self.relocate(uid, pos);
        self.maybe_clear_camp(uid);
        self.maybe_goody_hut(uid);
    }

    /// Resolve undefended units when a combat unit enters their tile.
    /// Settlers and Builders are captured; Traders and support units are
    /// destroyed. Religious units are neither automatically captured nor
    /// destroyed (they use theological combat/Condemn Heretic instead).
    fn resolve_entered_units(&mut self, uid: u32, pos: Pos) {
        let owner = self.units[&uid].owner;
        let mover_spec = &self.rules.units[self.units[&uid].kind.as_str()];
        let military = mover_spec.class == "military";
        if !military {
            return;
        }
        let can_capture = mover_spec.domain.as_deref() == Some("sea")
            || !self
                .map
                .get(pos)
                .map(|tile| self.rules.is_water(tile))
                .unwrap_or(false);
        let mut affected_owners = BTreeSet::new();
        for oid in self.units_at(pos) {
            if oid == uid || self.units[&oid].owner == owner {
                continue;
            }
            let kind = self.units[&oid].kind.clone();
            let class = self.rules.units[kind.as_str()].class.as_str();
            if can_capture && matches!(kind.as_str(), "builder" | "settler") {
                affected_owners.insert(self.units[&oid].owner);
                self.units.get_mut(&oid).unwrap().owner = owner;
            } else if matches!(class, "civilian" | "support") {
                affected_owners.insert(self.units[&oid].owner);
                self.remove_unit(oid);
            }
        }
        for old in affected_owners {
            self.on_unit_lost(old);
        }
    }

    fn do_ranged(&mut self, pid: usize, uid: u32, target: Pos) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        let spec = self.rules.units[u.kind.as_str()].clone();
        if !spec.has_ranged_attack() {
            return Err("unit has no ranged attack".into());
        }
        if self.is_embarked(&u) {
            return Err("cannot attack while embarked".into());
        }
        if spec.siege && u.acted {
            return Err("siege units cannot move and attack in the same turn".into());
        }
        if u.moves_left <= 0.0 {
            return Err("no moves left".into());
        }
        if self.wdist(u.pos, target) > spec.range.max(1) {
            return Err("out of range".into());
        }
        if !self.has_line_of_sight(u.pos, target, true) {
            return Err("line of sight blocked".into());
        }
        let enemy_ids: Vec<u32> = self
            .units_at(target)
            .into_iter()
            .filter(|id| {
                let owner = self.units[id].owner;
                owner != pid && self.is_at_war(pid, owner)
            })
            .collect();
        let mut city_id = self.city_at(target);
        if let Some(cid) = city_id {
            let owner = self.cities[&cid].owner;
            if owner == pid || !self.is_at_war(pid, owner) {
                city_id = None;
            }
        }
        let military: Vec<u32> = enemy_ids
            .iter()
            .cloned()
            .filter(|id| self.rules.units[self.units[id].kind.as_str()].class == "military")
            .collect();
        if military.is_empty() && city_id.is_none() {
            return Err("nothing to attack".into());
        }
        {
            let mu = self.units.get_mut(&uid).unwrap();
            mu.moves_left = 0.0;
            mu.fortified = false;
            mu.fortify_turns = 0;
            mu.acted = true;
        }
        // City Center garrisons are protected while the city stands.
        if city_id.is_none() && !military.is_empty() {
            let did = *military
                .iter()
                .max_by(|a, b| {
                    let ea = effective_strength(
                        self.unit_strength(&self.units[*a], true), self.units[*a].hp);
                    let eb = effective_strength(
                        self.unit_strength(&self.units[*b], true), self.units[*b].hp);
                    ea.partial_cmp(&eb).unwrap()
                })
                .unwrap();
            let defender = self.units[&did].clone();
            let attacker = self.units[&uid].clone();
            let downer = defender.owner;
            let mut att_base = self.unit_ranged_attack_strength(&self.units[&uid])
                + self.matchup_bonus(uid, &defender, true)
                + self.vs_bonus(pid, downer);
            let defender_spec = &self.rules.units[defender.kind.as_str()];
            if (spec.bombard_strength > 0.0
                && defender_spec.domain.as_deref() != Some("sea"))
                || (spec.ranged_strength > 0.0
                    && defender_spec.domain.as_deref() == Some("sea"))
            {
                att_base -= 17.0;
            }
            let att = effective_strength(att_base, self.units[&uid].hp);
            let ds = effective_strength(
                self.unit_strength(&defender, true)
                    + self.tile_defense_bonus(target)
                    + self.vs_bonus(downer, pid),
                defender.hp);
            let dmg = damage(att, ds, &mut self.rng);
            self.units.get_mut(&did).unwrap().hp -= dmg;
            let defender_dead = self.units[&did].hp <= 0;
            self.award_unit_combat_xp(uid, &defender, true, true, defender_dead);
            if !defender_dead {
                self.award_unit_combat_xp(did, &attacker, true, false, false);
            }
            if defender_dead {
                bump(&mut self.players[pid], "kills");
                if self.has_ability(pid, "killer_of_cyrus") {
                    if let Some(attacker) = self.units.get_mut(&uid) {
                        attacker.hp = (attacker.hp + 30).min(100);
                    }
                }
                let downer = self.units[&did].owner;
                self.remove_unit(did);
                self.on_unit_lost(downer);
            }
        } else if let Some(cid) = city_id {
            let mut att_base = self.unit_ranged_attack_strength(&self.units[&uid])
                + self.vs_bonus(pid, self.cities[&cid].owner);
            if spec.ranged_strength > 0.0
                && spec.domain.as_deref() != Some("sea")
            {
                att_base -= 17.0;
            }
            let att = effective_strength(att_base, self.units[&uid].hp);
            let cs = self.city_strength(cid);
            let dmg = damage(att, cs, &mut self.rng);
            let mult = if spec.siege { 1.0 } else { 0.5 };
            self.city_take_damage(cid, dmg, mult, false);
            // Ranged and Bombard attacks can breach defenses but cannot
            // capture a city or reduce its Garrison Health below 1.
            self.cities.get_mut(&cid).unwrap().hp = self.cities[&cid].hp.max(1);
            self.award_xp(uid, 3.0);
        }
        Ok(())
    }

    fn do_found_city(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        if u.kind != "settler" {
            return Err("only settlers found cities".into());
        }
        if self.players[pid].is_barbarian {
            return Err("barbarians do not found cities".into());
        }
        if !self.can_found_city(uid) {
            return Err("cannot found city here".into());
        }
        self.found_city_for(pid, u.pos, None);
        self.remove_unit(uid);
        Ok(())
    }

    fn found_city_for(&mut self, pid: usize, pos: Pos, name: Option<String>) -> u32 {
        let p_civ = self.players[pid].civ.clone();
        let is_minor = self.players[pid].is_minor;
        let name = name.unwrap_or_else(|| {
            let names = city_names(&p_civ);
            let n_mine = self
                .cities
                .values()
                .filter(|c| c.original_owner == pid)
                .count();
            if n_mine < names.len() {
                names[n_mine].to_string()
            } else {
                format!("{} {}", p_civ, n_mine + 1)
            }
        });
        let is_capital = !is_minor
            && !self
                .cities
                .values()
                .any(|c| c.original_owner == pid && c.is_capital);
        let cid = self.next_id;
        self.next_id += 1;
        let mut city = City {
            id: cid,
            name,
            owner: pid,
            pos,
            pop: 1,
            food: 0.0,
            production: 0.0,
            border_culture: 0.0,
            hp: 200,
            buildings: Vec::new(),
            districts: BTreeMap::new(),
            owned_tiles: Vec::new(),
            queue: Vec::new(),
            original_owner: pid,
            is_capital,
            struck: false,
            wall_hp: 0,
            last_attacked: 0,
            pressure: BTreeMap::new(),
            loyalty: 100.0,
        };
        {
            let center = self.map.tiles.get_mut(&pos).unwrap();
            center.feature = None;
            center.improvement = None;
        }
        let mut claim = vec![pos];
        claim.extend(self.nbrs(pos));
        for tpos in claim {
            if let Some(t) = self.map.tiles.get_mut(&tpos) {
                if t.owner_city.is_none() {
                    t.owner_city = Some(cid);
                    city.owned_tiles.push(tpos);
                }
            }
        }
        if self.has_ability(pid, "trajans_column") && !is_minor {
            city.buildings.push("monument".to_string()); // Trajan's Column
        }
        self.city_by_pos.insert(pos, cid);
        self.cities.insert(cid, city);
        self.reveal(pid, pos, 3);
        cid
    }

    fn do_improve(&mut self, pid: usize, uid: u32, imp: &str) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        if u.kind != "builder" || u.charges <= 0 {
            return Err("not a builder with charges".into());
        }
        if !self.valid_improvements(pid, u.pos).iter().any(|i| i == imp) {
            return Err("invalid improvement here".into());
        }
        let removes = self.rules.improvements[imp].removes_feature;
        let t = self.map.tiles.get_mut(&u.pos).unwrap();
        t.improvement = Some(imp.to_string());
        if removes {
            t.feature = None;
        }
        let mu = self.units.get_mut(&uid).unwrap();
        mu.charges -= 1;
        mu.moves_left = 0.0;
        mu.acted = true;
        bump(&mut self.players[pid], "improvements");
        if self.units[&uid].charges <= 0 {
            self.remove_unit(uid);
        }
        Ok(())
    }

    fn do_produce(&mut self, pid: usize, cid: u32, item: &Item) -> Result<(), String> {
        match self.cities.get(&cid) {
            Some(c) if c.owner == pid => {}
            _ => return Err("not your city".into()),
        }
        if !self.can_produce(pid, cid, item) {
            return Err("cannot produce that".into());
        }
        self.cities.get_mut(&cid).unwrap().queue = vec![item.clone()];
        Ok(())
    }

    fn do_buy(&mut self, pid: usize, cid: u32, unit: &str, currency: &str) -> Result<(), String> {
        match self.cities.get(&cid) {
            Some(c) if c.owner == pid => {}
            _ => return Err("not your city".into()),
        }
        let religious = self.rules.units.get(unit)
            .map(|s| s.class == "religious").unwrap_or(false);
        if religious {
            // faith purchase in a holy-site city of a religion founder
            if currency != "faith" {
                return Err("religious units are bought with faith".into());
            }
            if self.players[pid].religion.is_none() {
                return Err("no religion founded".into());
            }
            if !self.cities[&cid].districts.contains_key("holy_site") {
                return Err("needs a holy site".into());
            }
            let spec = &self.rules.units[unit];
            if !self.unlocked(pid, &spec.tech.clone(), &spec.civic.clone()) {
                return Err("not unlocked".into());
            }
        } else {
            let it = Item::Unit { unit: unit.to_string() };
            if !self.can_produce(pid, cid, &it) {
                return Err("cannot buy that".into());
            }
        }
        if unit == "settler" && self.cities[&cid].pop < 2 {
            return Err("city too small for settler".into());
        }
        let mult = if currency == "gold" { 4.0 } else { 2.0 };
        let cost = self.rules.units[unit].cost * mult;
        let bank = if currency == "gold" {
            self.players[pid].gold
        } else {
            self.players[pid].faith
        };
        if bank < cost {
            return Err("cannot afford".into());
        }
        let pos = self.cities[&cid].pos;
        if self.place_new_unit(unit, pid, pos).is_none() {
            return Err("no space to place unit".into());
        }
        if currency == "gold" {
            self.players[pid].gold -= cost;
        } else {
            self.players[pid].faith -= cost;
        }
        if unit == "settler" {
            self.cities.get_mut(&cid).unwrap().pop -= 1;
        }
        Ok(())
    }

    fn do_research(&mut self, pid: usize, tech: &str) -> Result<(), String> {
        if self.players[pid].research.is_some() {
            return Err("already researching".into());
        }
        if !self.available_techs(pid).iter().any(|t| t == tech) {
            return Err("tech unavailable".into());
        }
        let cost = self.rules.techs[tech].cost;
        let p = &mut self.players[pid];
        p.research = Some(tech.to_string());
        p.research_progress = p.research_overflow;
        p.research_overflow = 0.0;
        let f = self.boost_frac(pid);
        if self.players[pid].boosted_techs.contains(tech) {
            self.players[pid].research_progress += f * cost;
        }
        Ok(())
    }

    fn do_civic(&mut self, pid: usize, civic: &str) -> Result<(), String> {
        if self.players[pid].civic.is_some() {
            return Err("already working a civic".into());
        }
        if !self.available_civics(pid).iter().any(|c| c == civic) {
            return Err("civic unavailable".into());
        }
        let cost = self.rules.civics[civic].cost;
        let p = &mut self.players[pid];
        p.civic = Some(civic.to_string());
        p.civic_progress = p.civic_overflow;
        p.civic_overflow = 0.0;
        let f = self.boost_frac(pid);
        if self.players[pid].boosted_civics.contains(civic) {
            self.players[pid].civic_progress += f * cost;
        }
        Ok(())
    }

    fn do_fortify(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        if !self.unit_can_fortify(&u) {
            return Err("only unembarked land military units can fortify".into());
        }
        let mu = self.units.get_mut(&uid).unwrap();
        mu.fortified = true;
        if !mu.acted {
            mu.fortify_turns = mu.fortify_turns.max(1);
        }
        mu.moves_left = 0.0;
        Ok(())
    }

    fn do_government(&mut self, pid: usize, g: &str) -> Result<(), String> {
        let spec = self.rules.governments.get(g)
            .ok_or_else(|| "government unavailable".to_string())?;
        let p = &self.players[pid];
        if let Some(c) = &spec.civic {
            if !p.civics.contains(c) {
                return Err("government unavailable".into());
            }
        }
        if p.government.as_deref() == Some(g) {
            return Err("already that government".into());
        }
        self.players[pid].government = Some(g.to_string());
        // new slot layout: drop slotted cards until they fit again
        while !self.policies_fit(pid, &self.players[pid].policies)
            && !self.players[pid].policies.is_empty() {
            let drop = self.players[pid].policies.iter().next_back().unwrap().clone();
            self.players[pid].policies.remove(&drop);
        }
        Ok(())
    }

    fn do_slot_policy(&mut self, pid: usize, policy: &str) -> Result<(), String> {
        if !self.available_policies(pid).iter().any(|c| c == policy) {
            return Err("policy unavailable".into());
        }
        let mut next = self.players[pid].policies.clone();
        next.insert(policy.to_string());
        if !self.policies_fit(pid, &next) {
            return Err("no free slot for that card".into());
        }
        self.players[pid].policies = next;
        Ok(())
    }

    fn do_unslot_policy(&mut self, pid: usize, policy: &str) -> Result<(), String> {
        if !self.players[pid].policies.remove(policy) {
            return Err("policy not slotted".into());
        }
        Ok(())
    }

    fn do_city_strike(&mut self, pid: usize, cid: u32, target: Pos) -> Result<(), String> {
        match self.cities.get(&cid) {
            Some(c) if c.owner == pid => {}
            _ => return Err("not your city".into()),
        }
        if !self.city_can_strike(&self.cities[&cid]) {
            return Err("city cannot strike".into());
        }
        if self.wdist(self.cities[&cid].pos, target) > 2 {
            return Err("out of range".into());
        }
        if self.city_at(target).is_some() {
            return Err("cities cannot strike defensible districts".into());
        }
        if !self.has_line_of_sight(self.cities[&cid].pos, target, true) {
            return Err("line of sight blocked".into());
        }
        let enemies: Vec<u32> = self.units_at(target).into_iter()
            .filter(|id| {
                let o = &self.units[id];
                o.owner != pid && self.is_at_war(pid, o.owner)
            })
            .collect();
        if enemies.is_empty() {
            return Err("no enemy target".into());
        }
        let military: Vec<u32> = enemies.iter().cloned()
            .filter(|id| self.rules.units[self.units[id].kind.as_str()].class == "military")
            .collect();
        if military.is_empty() {
            return Err("no enemy military target".into());
        }
        let did = *military.iter().max_by(|a, b| {
                let ea = effective_strength(
                    self.unit_strength(&self.units[*a], true), self.units[*a].hp);
                let eb = effective_strength(
                    self.unit_strength(&self.units[*b], true), self.units[*b].hp);
                ea.partial_cmp(&eb).unwrap()
            }).unwrap();
        let d = self.units[&did].clone();
        let ds = effective_strength(self.unit_strength(&d, true), d.hp)
            + self.tile_defense_bonus(target);
        let naval = self.rules.units[d.kind.as_str()].domain.as_deref() == Some("sea");
        let att = self.city_ranged_strength(cid) - if naval { 17.0 } else { 0.0 };
        let dmg = damage(att, ds, &mut self.rng);
        self.units.get_mut(&did).unwrap().hp -= dmg;
        if self.units[&did].hp > 0 {
            self.award_xp(did, 2.0);
        } else {
            bump(&mut self.players[pid], "kills");
            let downer = self.units[&did].owner;
            self.remove_unit(did);
            self.on_unit_lost(downer);
        }
        self.cities.get_mut(&cid).unwrap().struck = true;
        Ok(())
    }

    fn do_declare_war(&mut self, pid: usize, other: usize) -> Result<(), String> {
        if other == pid || other >= self.players.len() || !self.players[other].alive {
            return Err("invalid war target".into());
        }
        if self.players[pid].is_barbarian || self.players[other].is_barbarian {
            return Err("barbarians are always at war".into());
        }
        self.at_war.insert(pair(pid, other));
        self.cancel_routes_with(pid, other);
        if self.players[other].is_minor {
            self.players[pid].envoys.retain(|(m, _)| *m != other);
        }
        Ok(())
    }

    fn do_make_peace(&mut self, pid: usize, other: usize) -> Result<(), String> {
        if !self.at_war.remove(&pair(pid, other)) {
            return Err("not at war".into());
        }
        Ok(())
    }

    // -------------------------------------------------- loyalty & governors

    /// Governor titles come from civic milestones (R&F simplified).
    pub fn governor_titles(&self, pid: usize) -> usize {
        ["political_philosophy", "civil_service", "guilds"].iter()
            .filter(|c| self.players[pid].civics.contains(**c))
            .count()
    }

    fn do_assign_governor(&mut self, pid: usize, cid: u32) -> Result<(), String> {
        match self.cities.get(&cid) {
            Some(c) if c.owner == pid => {}
            _ => return Err("not your city".into()),
        }
        if self.players[pid].governors.contains(&cid) {
            return Err("governor already there".into());
        }
        if self.players[pid].governors.len() >= self.governor_titles(pid) {
            return Err("no free governor titles".into());
        }
        self.players[pid].governors.push(cid);
        Ok(())
    }

    /// Population-based loyalty pressure (R&F simplified): nearby own pops
    /// pull up, foreign pops pull down; a governor anchors +8; the capital
    /// never flips. At 0 loyalty the city defects to the strongest neighbor.
    fn process_loyalty(&mut self, pid: usize) {
        if self.players[pid].is_minor {
            return;
        }
        let mut flips: Vec<(u32, usize)> = Vec::new();
        for cid in self.player_city_ids(pid) {
            let (cpos, is_cap) = {
                let c = &self.cities[&cid];
                (c.pos, c.is_capital)
            };
            let mut net = 0.0;
            let mut best_foreign: Option<(f64, usize)> = None;
            let mut foreign_pop: BTreeMap<usize, f64> = BTreeMap::new();
            for o in self.cities.values() {
                if o.id == cid {
                    continue;
                }
                let d = self.wdist(o.pos, cpos);
                if d > 9 {
                    continue;
                }
                let w = o.pop as f64 * (10.0 - d as f64) / 10.0;
                if o.owner == pid {
                    net += w;
                } else if !self.players[o.owner].is_barbarian
                    && !self.players[o.owner].is_minor {
                    net -= w;
                    *foreign_pop.entry(o.owner).or_insert(0.0) += w;
                }
            }
            net += self.cities[&cid].pop as f64; // a city anchors itself
            for (o, w) in foreign_pop {
                if best_foreign.map(|(bw, _)| w > bw).unwrap_or(true) {
                    best_foreign = Some((w, o));
                }
            }
            let mut delta = (net * 0.5).clamp(-6.0, 6.0);
            if self.players[pid].governors.contains(&cid) {
                delta += 8.0;
            }
            let c = self.cities.get_mut(&cid).unwrap();
            c.loyalty = (c.loyalty + delta).clamp(0.0, 100.0);
            if c.loyalty <= 0.0 && !is_cap {
                if let Some((_, new_owner)) = best_foreign {
                    flips.push((cid, new_owner));
                }
            }
        }
        for (cid, new_owner) in flips {
            self.capture_city(cid, new_owner);
            self.cities.get_mut(&cid).unwrap().loyalty = 100.0;
        }
    }

    // -------------------------------------------------- world congress

    /// From the medieval world era, a congress convenes every 30 turns: the
    /// civ with the most diplomatic standing (envoys + 2 per suzerainty)
    /// gains 2 victory points; 20 points win (GS congress simplified).
    fn process_congress(&mut self) {
        if self.world_era < 2 || self.turn % 30 != 0 || self.winner.is_some() {
            return;
        }
        let mut best: Option<(i64, usize)> = None;
        let mut tied = false;
        for p in self.players.iter().filter(|p| p.alive && !p.is_minor) {
            let envoys: i64 = p.envoys.iter().map(|(_, n)| *n).sum();
            let suz: i64 = self.players.iter()
                .filter(|m| m.is_minor && !m.is_barbarian && m.alive)
                .filter(|m| self.suzerain_of(m.id) == Some(p.id))
                .count() as i64;
            let favor = envoys + 2 * suz;
            match best {
                Some((bf, _)) if favor == bf => tied = true,
                Some((bf, _)) if favor > bf => {
                    best = Some((favor, p.id));
                    tied = false;
                }
                None => {
                    best = Some((favor, p.id));
                    tied = false;
                }
                _ => {}
            }
        }
        if let Some((favor, pid)) = best {
            if !tied && favor > 0 {
                self.players[pid].dvp += 2;
                if self.players[pid].dvp >= DIPLOMATIC_VICTORY_POINTS {
                    self.set_winner(pid, "diplomatic");
                }
            }
        }
    }

    // -------------------------------------------------- eras & tourism

    /// World era from the most advanced civ's tech+civic count.
    fn era_from_progress(&self) -> usize {
        let best = self.players.iter()
            .filter(|p| !p.is_minor)
            .map(|p| p.techs.len() + p.civics.len())
            .max().unwrap_or(0);
        match best {
            0..=11 => 0,  // ancient
            12..=21 => 1, // classical
            22..=31 => 2, // medieval
            _ => 3,       // renaissance
        }
    }

    /// On a world-era transition, era score decides each major's age
    /// (R&F-style): golden = +10% yields, dark = -5%; score then resets.
    fn process_eras(&mut self) {
        let era = self.era_from_progress();
        if era <= self.world_era {
            return;
        }
        let need = 12 + 4 * self.world_era as i64;
        self.world_era = era;
        for p in self.players.iter_mut().filter(|p| !p.is_minor) {
            p.age = if p.era_score >= need {
                "golden".to_string()
            } else if p.era_score * 2 < need {
                "dark".to_string()
            } else {
                "normal".to_string()
            };
            p.era_score = 0;
        }
    }

    pub fn exoplanet_speed(&self, pid: usize) -> f64 {
        let p = &self.players[pid];
        if !p.science_projects.contains("exoplanet_expedition") {
            return 0.0;
        }
        1.0
            + p.counters
                .get("project:lagrange_laser_station")
                .copied()
                .unwrap_or(0) as f64
            + p.counters
                .get("project:terrestrial_laser_station")
                .copied()
                .unwrap_or(0) as f64
    }

    fn advance_exoplanet(&mut self, pid: usize) {
        if !self.victory_eligible(pid)
            || !self.players[pid]
                .science_projects
                .contains("exoplanet_expedition")
        {
            return;
        }
        self.players[pid].exoplanet_distance =
            (self.players[pid].exoplanet_distance + self.exoplanet_speed(pid))
                .min(EXOPLANET_DESTINATION);
        if self.players[pid].exoplanet_distance >= EXOPLANET_DESTINATION {
            self.set_winner(pid, "science");
        }
    }

    pub fn domestic_tourists(&self, pid: usize) -> i64 {
        (self.players[pid].culture_lifetime / 100.0).floor() as i64
    }

    pub fn foreign_tourists(&self, pid: usize) -> i64 {
        let starting_majors = self
            .players
            .iter()
            .filter(|p| !p.is_minor && !p.is_barbarian)
            .count();
        if starting_majors == 0 {
            return 0;
        }
        // Tourism output is applied to each rival. Civ VI converts total
        // lifetime tourism into visitors using the starting player count;
        // using that fixed count also keeps visitors from disappearing when
        // a civilization is eliminated.
        (self.players[pid].tourism_lifetime * starting_majors.saturating_sub(1) as f64
            / (starting_majors as f64 * TOURISM_PER_VISITOR))
            .floor() as i64
    }

    /// Culture victory: visiting tourists must exceed the largest rival
    /// domestic-tourist count.
    fn check_culture_victory(&mut self) {
        if self.winner.is_some() {
            return;
        }
        let majors: Vec<usize> = self
            .players
            .iter()
            .filter(|p| self.victory_eligible(p.id))
            .map(|p| p.id)
            .collect();
        if majors.len() < 2 {
            return;
        }
        for pid in &majors {
            let foreign = self.foreign_tourists(*pid);
            let target = majors
                .iter()
                .filter(|oid| *oid != pid)
                .map(|oid| self.domestic_tourists(*oid))
                .max()
                .unwrap_or(0);
            if foreign > target {
                self.set_winner(*pid, "culture");
                return;
            }
        }
    }

    fn do_end_turn(&mut self) {
        let n = self.players.len();
        let mut nxt = None;
        for i in 1..=n {
            let cand = (self.current + i) % n;
            if self.players[cand].alive {
                nxt = Some(cand);
                break;
            }
        }
        let nxt = match nxt {
            Some(x) if x != self.current => x,
            _ => return,
        };
        let wrapped = nxt <= self.current;
        self.current = nxt;
        if wrapped {
            self.turn += 1;
            self.barbarian_phase();
            self.process_eras();
            self.process_congress();
            self.check_culture_victory();
            // A score victory is only a turn-limit tiebreak, never an
            // immediate win for crossing an arbitrary score threshold.
            if self.turn > self.max_turns && self.winner.is_none() {
                let mut best: Option<(i64, i64)> = None; // (score, -pid)
                let mut best_pid = 0;
                for pl in &self.players {
                    if pl.alive && !pl.is_minor {
                        let key = (self.score(pl.id), -(pl.id as i64));
                        if best.is_none() || key > best.unwrap() {
                            best = Some(key);
                            best_pid = pl.id;
                        }
                    }
                }
                if best.is_none() {
                    for pl in &self.players {
                        let key = (self.score(pl.id), -(pl.id as i64));
                        if best.is_none() || key > best.unwrap() {
                            best = Some(key);
                            best_pid = pl.id;
                        }
                    }
                }
                self.set_winner(best_pid, "score");
            }
        }
        if self.winner.is_none() {
            self.begin_turn(self.current);
        }
    }

    // ------------------------------------------------------- turn engine

    fn begin_turn(&mut self, pid: usize) {
        self.advance_exoplanet(pid);
        if self.winner.is_some() {
            return;
        }
        self.check_boosts(pid);
        self.process_routes(pid);
        self.process_great_people(pid);
        self.process_pressure(pid);
        self.process_loyalty(pid);
        if !self.players[pid].is_minor {
            // influence points scale with government tier; 100 points = 1 envoy
            let tier = match self.players[pid].government.as_deref() {
                Some("monarchy") | Some("merchant_republic") => 2.0,
                Some("autocracy") | Some("oligarchy")
                | Some("classical_republic") => 1.0,
                _ => 0.0,
            };
            let p = &mut self.players[pid];
            p.influence += 1.0 + tier;
            if p.influence >= 100.0 {
                p.influence -= 100.0;
                p.envoys_free += 1;
            }
        }
        for uid in self.player_unit_ids(pid) {
            let (kind, hp, acted) = {
                let u = &self.units[&uid];
                (u.kind.clone(), u.hp, u.acted)
            };
            let moves = self.unit_max_moves(uid);
            let spec = &self.rules.units[kind.as_str()];
            let embarked = self.is_embarked(&self.units[&uid]);
            let heal = if hp < 100 && !acted {
                self.unit_heal_rate(uid)
            } else {
                0
            };
            let u = self.units.get_mut(&uid).unwrap();
            u.moves_left = moves;
            u.zoc_stopped = false;
            u.hp = (u.hp + heal).min(100);
            if spec.class == "military"
                && spec.domain.as_deref() != Some("sea")
                && !embarked
                && (u.fortified || !acted)
            {
                u.fortify_turns = (u.fortify_turns + 1).min(2);
            } else if acted {
                u.fortify_turns = 0;
            }
            u.acted = false;
            while u.level < 8
                && u.xp >= (15 * u.level as i64 * (u.level as i64 + 1)) / 2 {
                u.level += 1;
            }
        }
        let mut sci = 0.0;
        let mut cul = 0.0;
        let mut gold = 0.0;
        let mut faith = 0.0;
        for cid in self.player_city_ids(pid) {
            let ys = self.process_city(pid, cid);
            sci += ys.science;
            cul += ys.culture;
            gold += ys.gold;
            faith += ys.faith;
        }
        if !self.players[pid].is_minor {
            let wonders = self.cities.values()
                .filter(|c| c.owner == pid)
                .flat_map(|c| c.buildings.iter())
                .filter(|b| self.rules.buildings[b.as_str()].wonder)
                .count() as f64;
            let p = &mut self.players[pid];
            p.culture_lifetime += cul;
            p.tourism_lifetime += 2.0 * wonders + 0.15 * cul;
        }
        if let Some(r) = self.players[pid].religion.clone() {
            let following = self.cities.values()
                .filter(|c| self.city_religion(c) == Some(r.as_str()))
                .count() as f64;
            if self.players[pid].religion_beliefs.iter().any(|b| b == "tithe") {
                gold += (following / 4.0).floor();
            }
            if self.players[pid].religion_beliefs.iter().any(|b| b == "world_church") {
                cul += (following / 5.0).floor();
            }
        }
        let n_units = self.player_unit_ids(pid).len() as f64;
        // 1 gold/unit past the first three; conscription (-1/unit) zeroes it
        if !self.has_policy(pid, "conscription") {
            gold -= (n_units - 3.0).max(0.0);
        }
        {
            let p = &mut self.players[pid];
            p.gold = (p.gold + gold).max(0.0);
            p.faith += faith;
        }
        let research = self.players[pid].research.clone();
        if let Some(tech) = research {
            let cost = self.rules.techs[tech.as_str()].cost;
            let p = &mut self.players[pid];
            p.research_progress += sci;
            if p.research_progress >= cost {
                p.techs.insert(tech);
                p.research_overflow = p.research_progress - cost;
                p.research = None;
                p.research_progress = 0.0;
            }
        } else {
            self.players[pid].research_overflow += sci;
        }
        let civic = self.players[pid].civic.clone();
        if let Some(cv) = civic {
            let cost = self.rules.civics[cv.as_str()].cost;
            let p = &mut self.players[pid];
            p.civic_progress += cul;
            if p.civic_progress >= cost {
                p.civics.insert(cv);
                p.civic_overflow = p.civic_progress - cost;
                p.civic = None;
                p.civic_progress = 0.0;
            }
        } else {
            self.players[pid].civic_overflow += cul;
        }
    }

    fn check_boosts(&mut self, pid: usize) {
        if self.players[pid].is_minor {
            return;
        }
        let techs: Vec<(String, f64, crate::rules::BoostSpec)> = self.rules.techs.iter()
            .filter_map(|(n, s)| s.boost.clone().map(|b| (n.clone(), s.cost, b)))
            .collect();
        for (name, cost, b) in techs {
            let p = &self.players[pid];
            if p.techs.contains(&name) || p.boosted_techs.contains(&name) {
                continue;
            }
            if self.boost_met(pid, &b) {
                let f = self.boost_frac(pid);
                let p = &mut self.players[pid];
                p.boosted_techs.insert(name.clone());
                if p.research.as_deref() == Some(name.as_str()) {
                    p.research_progress += f * cost;
                }
            }
        }
        let civics: Vec<(String, f64, crate::rules::BoostSpec)> = self.rules.civics.iter()
            .filter_map(|(n, s)| s.boost.clone().map(|b| (n.clone(), s.cost, b)))
            .collect();
        for (name, cost, b) in civics {
            let p = &self.players[pid];
            if p.civics.contains(&name) || p.boosted_civics.contains(&name) {
                continue;
            }
            if self.boost_met(pid, &b) {
                let f = self.boost_frac(pid);
                let p = &mut self.players[pid];
                p.boosted_civics.insert(name.clone());
                if p.civic.as_deref() == Some(name.as_str()) {
                    p.civic_progress += f * cost;
                }
            }
        }
    }

    fn boost_met(&self, pid: usize, b: &crate::rules::BoostSpec) -> bool {
        let p = &self.players[pid];
        let n = b.count;
        let cities: Vec<&City> = self.cities.values()
            .filter(|c| c.owner == pid).collect();
        let trig = b.trigger.as_str();
        match trig {
            "kills" | "improvements" | "camps" | "captures" => {
                p.counters.get(trig).copied().unwrap_or(0) >= n
            }
            "cities" => cities.len() as i64 >= n,
            "districts" => cities.iter()
                .map(|c| c.districts.len() as i64).sum::<i64>() >= n,
            "pop" => cities.iter().any(|c| c.pop as i64 >= n),
            "total_pop" => cities.iter()
                .map(|c| c.pop as i64).sum::<i64>() >= n,
            "units" => self.units.values()
                .filter(|u| u.owner == pid
                    && self.rules.units[u.kind.as_str()].class == "military")
                .count() as i64 >= n,
            "coastal_city" => cities.iter().any(|c| {
                self.nbrs(c.pos).iter().any(|nb| {
                    self.map.get(*nb).map(|t| self.rules.is_water(t)).unwrap_or(false)
                })
            }),
            "war" => self.players.iter().any(|o| {
                o.id != pid && !o.is_barbarian && self.is_at_war(pid, o.id)
            }),
            _ => {
                if let Some(t) = trig.strip_prefix("units_of:") {
                    self.units.values()
                        .filter(|u| u.owner == pid && u.kind == t)
                        .count() as i64 >= n
                } else if let Some(d) = trig.strip_prefix("district:") {
                    cities.iter().any(|c| c.districts.contains_key(d))
                } else if let Some(bn) = trig.strip_prefix("building:") {
                    cities.iter()
                        .filter(|c| c.buildings.iter().any(|x| x == bn))
                        .count() as i64 >= n
                } else if let Some(t) = trig.strip_prefix("tech:") {
                    p.techs.contains(t)
                } else {
                    false
                }
            }
        }
    }

    fn process_city(&mut self, pid: usize, cid: u32) -> Yields {
        self.cities.get_mut(&cid).unwrap().struck = false;
        let ys = self.city_yields(cid);
        let housing = self.city_housing(&self.cities[&cid]);
        let am = self.city_amenity_surplus(&self.cities[&cid]);
        let repair_project = matches!(
            self.cities[&cid].queue.first(),
            Some(Item::Project { project }) if project == "repair_outer_defenses"
        );
        let mut growth_bonus = self.empire_building_sum(pid, |b| b.growth_pct);
        if self.players[pid].pantheon.as_deref() == Some("fertility_rites") {
            growth_bonus += 10.0;
        }
        {
            let city = self.cities.get_mut(&cid).unwrap();
            let mut surplus = ys.food - 2.0 * city.pop as f64;
            if surplus > 0.0 {
                let headroom = housing - city.pop as f64;
                let hf = if headroom > 1.0 { 1.0 }
                    else if headroom >= 1.0 { 0.5 }
                    else if headroom > -2.0 { 0.25 } else { 0.0 };
                let af = if am >= 2 { 1.1 } else if am >= 0 { 1.0 }
                    else if am >= -2 { 0.75 } else { 0.5 };
                surplus *= hf * af * (1.0 + growth_bonus / 100.0);
            }
            city.food += surplus;
            let need = growth_threshold(city.pop);
            if city.food >= need {
                city.pop += 1;
                city.food -= need;
            } else if city.food < 0.0 {
                city.pop = (city.pop - 1).max(1);
                city.food = 0.0;
            }
            let mult = {
                let c = &self.cities[&cid];
                self.item_prod_mult(pid, cid, c.queue.first())
            };
            let produced = ys.production * mult;
            if repair_project {
                let can_repair = {
                    let city = &self.cities[&cid];
                    let max = self.city_max_wall_hp(city);
                    max > 0
                        && city.wall_hp < max
                        && self.turn.saturating_sub(city.last_attacked) >= 3
                };
                if can_repair {
                    let max = self.city_max_wall_hp(&self.cities[&cid]);
                    let city = self.cities.get_mut(&cid).unwrap();
                    city.production += produced;
                    let repaired = (city.production.floor() as i32).min(max - city.wall_hp);
                    city.wall_hp += repaired;
                    city.production -= repaired as f64;
                    if city.wall_hp == max {
                        city.queue.remove(0);
                    }
                }
            } else {
                self.cities.get_mut(&cid).unwrap().production += produced;
            }
        }
        let queue_head = self.cities[&cid].queue.first().cloned();
        if let Some(item) = queue_head {
            if matches!(&item, Item::Project { project } if project == "repair_outer_defenses") {
                // Repair applies Production directly to wall HP above.
            } else {
            let cost = self.item_cost(&item);
            let stalled = matches!(&item, Item::Unit { unit } if unit == "settler")
                && self.cities[&cid].pop < 2;
            if !stalled && self.cities[&cid].production >= cost {
                if self.complete_item(pid, cid, &item) {
                    let city = self.cities.get_mut(&cid).unwrap();
                    city.production -= cost;
                    city.queue.remove(0);
                }
            }
            }
        }
        {
            let owned = self.cities[&cid].owned_tiles.len() as i32;
            let border_mult =
                if self.players[pid].pantheon.as_deref() == Some("religious_settlements") {
                    1.15
                } else {
                    1.0
                };
            let city = self.cities.get_mut(&cid).unwrap();
            city.border_culture += (1.0 + ys.culture * 0.5) * border_mult;
            let need_b = (15 + 8 * (owned - 7).max(0)) as f64;
            if city.border_culture >= need_b {
                city.border_culture -= need_b;
                self.expand_borders(cid);
            }
        }
        let besieged = self.city_under_siege(cid);
        let city = self.cities.get_mut(&cid).unwrap();
        if !besieged {
            city.hp = (city.hp + 20).min(200); // Civ 6 heal rate
        }
        ys
    }

    fn complete_item(&mut self, pid: usize, cid: u32, item: &Item) -> bool {
        match item {
            Item::Unit { unit } => {
                let pos = self.cities[&cid].pos;
                if self.place_new_unit(unit, pid, pos).is_none() {
                    return false;
                }
                if unit == "settler" {
                    self.cities.get_mut(&cid).unwrap().pop -= 1;
                }
                true
            }
            Item::Building { building } => {
                let spec = self.rules.buildings[building.as_str()].clone();
                if spec.wonder && self.wonder_built(building) {
                    // wonder race lost: drop the item, keep banked production
                    let city = self.cities.get_mut(&cid).unwrap();
                    city.queue.clear();
                    return false;
                }
                self.cities.get_mut(&cid).unwrap().buildings.push(building.clone());
                if building == "walls" || building == "medieval_walls" {
                    self.cities.get_mut(&cid).unwrap().wall_hp += 100;
                }
                if spec.wonder {
                    self.players[pid].era_score += 3;
                }
                if spec.unit_levels > 0 {
                    for uid in self.player_unit_ids(pid) {
                        let mil = self.rules.units[self.units[&uid].kind.as_str()]
                            .class == "military";
                        if mil {
                            let u = self.units.get_mut(&uid).unwrap();
                            u.level = (u.level + spec.unit_levels).min(8);
                        }
                    }
                }
                true
            }
            Item::District { district, pos } => {
                if self.district_sites(cid, district).contains(pos) {
                    let t = self.map.tiles.get_mut(pos).unwrap();
                    t.district = Some(district.clone());
                    t.improvement = None;
                    t.feature = None;
                    self.cities
                        .get_mut(&cid)
                        .unwrap()
                        .districts
                        .insert(district.clone(), *pos);
                }
                true
            }
            Item::Project { project } => {
                if project == "repair_outer_defenses" {
                    return true;
                }
                let spec = self.rules.projects[project.as_str()].clone();
                if !spec.repeatable && self.players[pid].science_projects.contains(project) {
                    // Another city won this internal project race.
                    return true;
                }
                if spec.repeatable {
                    bump(
                        &mut self.players[pid],
                        &format!("project:{project}"),
                    );
                } else {
                    self.players[pid].science_projects.insert(project.clone());
                }
                if project == "launch_earth_satellite" {
                    self.players[pid]
                        .explored
                        .extend(self.map.tiles.keys().copied());
                }
                if project == "launch_moon_landing" {
                    let bonus = 10.0
                        * self
                            .player_city_ids(pid)
                            .into_iter()
                            .map(|city_id| self.city_yields(city_id).science)
                            .sum::<f64>();
                    let player = &mut self.players[pid];
                    player.culture_lifetime += bonus;
                    if player.civic.is_some() {
                        player.civic_progress += bonus;
                    } else {
                        player.civic_overflow += bonus;
                    }
                }
                if project == "exoplanet_expedition" {
                    self.players[pid].exoplanet_distance = 0.0;
                }
                true
            }
        }
    }

    fn place_new_unit(&mut self, kind: &str, owner: usize, pos: Pos) -> Option<u32> {
        let spec = self.rules.units[kind].clone();
        let want_sea = spec.domain.as_deref() == Some("sea");
        let mut cands = vec![pos];
        cands.extend(self.nbrs(pos));
        for cand in cands {
            let t = match self.map.get(cand) {
                Some(t) => t,
                None => continue,
            };
            if !self.rules.is_passable(t) || self.rules.is_water(t) != want_sea {
                continue;
            }
            let mut occupied = false;
            for oid in self.units_at(cand) {
                let o = &self.units[&oid];
                if o.owner != owner || self.rules.units[o.kind.as_str()].class == spec.class {
                    occupied = true;
                    break;
                }
            }
            if let Some(ccid) = self.city_at(cand) {
                if self.cities[&ccid].owner != owner {
                    occupied = true;
                }
            }
            if !occupied {
                return Some(self.spawn_unit(kind, owner, cand));
            }
        }
        None
    }

    fn expand_borders(&mut self, cid: u32) {
        let city_pos = self.cities[&cid].pos;
        let owned = self.cities[&cid].owned_tiles.clone();
        let mut best: Option<((f64, Pos), Pos)> = None;
        for pos in &owned {
            for n in self.nbrs(*pos) {
                let t = match self.map.get(n) {
                    Some(t) => t,
                    None => continue,
                };
                if t.owner_city.is_some() || self.wdist(n, city_pos) > 3 {
                    continue;
                }
                let tys = self.rules.tile_yields(t);
                let val = tys.total() + if t.resource.is_some() { 2.0 } else { 0.0 };
                let key = (val, n);
                let better = match &best {
                    None => true,
                    Some((bk, _)) => {
                        key.0 > bk.0 || (key.0 == bk.0 && key.1 > bk.1)
                    }
                };
                if better {
                    best = Some((key, n));
                }
            }
        }
        if let Some((_, n)) = best {
            self.map.tiles.get_mut(&n).unwrap().owner_city = Some(cid);
            self.cities.get_mut(&cid).unwrap().owned_tiles.push(n);
        }
    }

    // ----------------------------------------------------- win handling

    fn capture_city(&mut self, cid: u32, new_owner: usize) {
        let old = self.cities[&cid].owner;
        {
            let city = self.cities.get_mut(&cid).unwrap();
            city.owner = new_owner;
            city.pop = (city.pop - 1).max(1);
            city.hp = 100;
            city.queue.clear();
            // Civ 6: walls are destroyed outright when a city falls
            city.buildings.retain(|b| b != "walls" && b != "medieval_walls");
            city.wall_hp = 0;
        }
        let pos = self.cities[&cid].pos;
        for oid in self.units_at(pos) {
            if self.units[&oid].owner == old {
                if matches!(self.units[&oid].kind.as_str(), "builder" | "settler") {
                    self.units.get_mut(&oid).unwrap().owner = new_owner;
                } else {
                    self.remove_unit(oid);
                }
            }
        }
        for p in self.players.iter_mut() {
            p.governors.retain(|g| *g != cid);
        }
        bump(&mut self.players[new_owner], "captures");
        self.players[new_owner].era_score += 2;
        self.routes.retain(|r| r.origin != cid && r.dest != cid);
        self.check_elimination(old);
        self.check_domination();
    }

    fn on_unit_lost(&mut self, pid: usize) {
        self.check_elimination(pid);
        self.check_domination();
    }

    fn check_elimination(&mut self, pid: usize) {
        if !self.players[pid].alive || self.players[pid].is_barbarian {
            return;
        }
        if self.cities.values().any(|c| c.owner == pid) {
            return;
        }
        if self
            .units
            .values()
            .any(|u| u.owner == pid && u.kind == "settler")
        {
            return;
        }
        self.players[pid].alive = false;
        for uid in self.player_unit_ids(pid) {
            self.remove_unit(uid);
        }
    }

    fn check_domination(&mut self) {
        let majors: Vec<usize> = self
            .players
            .iter()
            .filter(|p| !p.is_minor && !p.is_barbarian)
            .map(|p| p.id)
            .collect();
        if majors.len() < 2 {
            return;
        }
        for candidate in majors.iter().copied().filter(|p| self.victory_eligible(*p)) {
            let controls_every_foreign_capital = majors.iter().copied().all(|original_owner| {
                if original_owner == candidate {
                    return true;
                }
                match self.cities.values().find(|c| {
                    c.is_capital && c.original_owner == original_owner
                }) {
                    Some(capital) => capital.owner == candidate,
                    // The engine begins with settlers. Defeating a civ before
                    // it founds its original capital satisfies that opponent.
                    None => !self.players[original_owner].alive,
                }
            });
            if controls_every_foreign_capital {
                self.set_winner(candidate, "domination");
                return;
            }
        }
    }

    fn victory_eligible(&self, pid: usize) -> bool {
        self.players.get(pid).is_some_and(|p| {
            p.alive && !p.is_minor && !p.is_barbarian
        })
    }

    fn set_winner(&mut self, pid: usize, vtype: &str) {
        if self.winner.is_none() && self.victory_eligible(pid) {
            self.winner = Some(pid);
            self.victory_type = Some(vtype.to_string());
        }
    }
}

#[cfg(test)]
mod combat_scenarios {
    use super::*;

    fn controlled_game(seed: u64) -> (Game, Pos, Vec<Pos>) {
        let mut g = Game::new_full(2, 20, 14, seed, 40, 0, false);
        let ids: Vec<u32> = g.units.keys().copied().collect();
        for id in ids {
            g.remove_unit(id);
        }
        for player in &mut g.players {
            player.civ = "Rome".to_string();
            player.government = None;
            player.policies.clear();
            player.techs.clear();
            player.civics.clear();
        }
        for tile in g.map.tiles.values_mut() {
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.owner_city = None;
            tile.hills = false;
            tile.river = false;
            tile.road = false;
        }
        let center = *g.map.tiles.keys()
            .find(|p| g.wdisk(**p, 2).len() == 19)
            .expect("controlled map has an interior tile");
        let ring = g.nbrs(center);
        assert_eq!(ring.len(), 6);
        g.current = 0;
        g.at_war.insert(pair(0, 1));
        (g, center, ring)
    }

    #[test]
    fn passive_healing_uses_city_friendly_neutral_and_enemy_rates() {
        let (mut g, center, ring) = controlled_game(300);
        let settler = g.spawn_unit("settler", 0, center);
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let home = g.player_city_ids(0)[0];
        let friendly = ring
            .iter()
            .copied()
            .find(|pos| g.map.tiles[pos].owner_city == Some(home))
            .unwrap();
        assert_ne!(friendly, center);
        assert_eq!(g.city_at(friendly), None);
        // Any district receives the 20 HP district rate.
        g.map.tiles.get_mut(&friendly).unwrap().district = Some("campus".to_string());
        let plain_friendly = ring.iter().copied()
            .find(|pos| *pos != friendly && g.map.tiles[pos].owner_city == Some(home))
            .unwrap();

        let neutral = g
            .wdisk(center, 2)
            .into_iter()
            .find(|pos| g.wdist(center, *pos) == 2 && g.map.tiles[pos].owner_city.is_none())
            .unwrap();
        let enemy_center = g
            .map
            .tiles
            .keys()
            .copied()
            .find(|pos| g.wdist(center, *pos) >= 6 && g.wdisk(*pos, 1).len() == 7)
            .unwrap();
        let enemy_city = g.found_city_for(1, enemy_center, None);
        let enemy = g
            .nbrs(enemy_center)
            .into_iter()
            .find(|pos| g.map.tiles[pos].owner_city == Some(enemy_city))
            .unwrap();
        assert_eq!(g.city_at(friendly), None);

        let cases = [
            (g.spawn_unit("warrior", 0, center), HealingLocation::District, 20),
            (
                g.spawn_unit("warrior", 0, friendly),
                HealingLocation::District,
                20,
            ),
            (
                g.spawn_unit("warrior", 0, plain_friendly),
                HealingLocation::FriendlyTerritory,
                15,
            ),
            (
                g.spawn_unit("warrior", 0, neutral),
                HealingLocation::NeutralTerritory,
                10,
            ),
            (
                g.spawn_unit("warrior", 0, enemy),
                HealingLocation::EnemyTerritory,
                5,
            ),
        ];
        for (uid, location, rate) in cases {
            let pos = {
                let unit = g.units.get_mut(&uid).unwrap();
                unit.hp = 50;
                unit.acted = false;
                unit.pos
            };
            assert_eq!(g.healing_location(0, pos), location);
            assert_eq!(g.unit_heal_rate(uid), rate);
        }

        g.begin_turn(0);
        assert_eq!(g.units[&cases[0].0].hp, 70);
        assert_eq!(g.units[&cases[1].0].hp, 70);
        assert_eq!(g.units[&cases[2].0].hp, 65);
        assert_eq!(g.units[&cases[3].0].hp, 60);
        assert_eq!(g.units[&cases[4].0].hp, 55);

        // Peace and Open Borders do not make another civilization friendly.
        g.at_war.remove(&pair(0, 1));
        assert_eq!(
            g.healing_location(0, enemy),
            HealingLocation::EnemyTerritory
        );
        assert_eq!(g.unit_heal_rate(cases[4].0), 5);
    }

    #[test]
    fn unit_class_matchups_feed_the_real_melee_damage_roll() {
        let (mut g, target, ring) = controlled_game(301);
        let attacker = g.spawn_unit("spearman", 0, ring[0]);
        let defender = g.spawn_unit("horseman", 1, target);

        assert_eq!(g.matchup_bonus(attacker, &g.units[&defender], true), 10.0);
        let mut expected_rng = g.rng.clone();
        let expected_out = damage(35.0, 36.0, &mut expected_rng);
        let expected_in = damage(36.0, 35.0, &mut expected_rng);
        g.apply(0, &Action::Attack { unit: attacker, target }).unwrap();
        assert_eq!(g.units[&defender].hp, 100 - expected_out);
        assert_eq!(g.units[&attacker].hp, 100 - expected_in);

        let (mut g, target, ring) = controlled_game(302);
        let spear = g.spawn_unit("spearman", 0, ring[0]);
        let war_cart = g.spawn_unit("war_cart", 1, target);
        assert_eq!(g.matchup_bonus(spear, &g.units[&war_cart], true), 0.0,
                   "War-Carts are immune to the anti-cavalry modifier");
        let maryannu = g.spawn_unit("maryannu_chariot_archer", 1, ring[1]);
        assert_eq!(g.matchup_bonus(spear, &g.units[&maryannu], true), 10.0,
                   "ranged cavalry still receives the anti-cavalry modifier");
    }

    #[test]
    fn military_tradition_flanking_and_support_follow_provider_rules() {
        let (mut g, target, ring) = controlled_game(303);
        let attacker = g.spawn_unit("warrior", 0, ring[0]);
        let defender = g.spawn_unit("warrior", 1, target);
        let flank_archer = g.spawn_unit("archer", 0, ring[1]);
        let support_archer = g.spawn_unit("archer", 1, ring[2]);

        assert_eq!(g.flanking_bonus(attacker, target), 0.0);
        assert_eq!(g.support_bonus(&g.units[&defender]), 0.0);
        g.players[0].civics.insert("military_tradition".to_string());
        g.players[1].civics.insert("military_tradition".to_string());
        assert_eq!(g.flanking_bonus(attacker, target), 2.0,
                   "a ranged military unit provides one flanking stack");
        assert_eq!(g.support_bonus(&g.units[&defender]), 2.0);

        // Rivers block flanking but not support.
        g.map.tiles.get_mut(&ring[1]).unwrap().river = true;
        assert_eq!(g.flanking_bonus(attacker, target), 0.0);
        g.map.tiles.get_mut(&ring[2]).unwrap().river = true;
        assert_eq!(g.support_bonus(&g.units[&defender]), 2.0);

        // Embarked land units provide Support but cannot provide Flanking.
        g.map.tiles.get_mut(&ring[1]).unwrap().river = false;
        g.map.tiles.get_mut(&ring[1]).unwrap().terrain = "coast".to_string();
        assert!(g.is_embarked(&g.units[&flank_archer]));
        assert_eq!(g.flanking_bonus(attacker, target), 0.0);
        g.map.tiles.get_mut(&ring[2]).unwrap().terrain = "coast".to_string();
        assert!(g.is_embarked(&g.units[&support_archer]));
        assert_eq!(g.support_bonus(&g.units[&defender]), 2.0);
    }

    #[test]
    fn ranged_attacks_require_an_open_range_two_sight_corridor() {
        let (mut g, target, _) = controlled_game(304);
        let from = g.wdisk(target, 2).into_iter()
            .find(|p| g.wdist(*p, target) == 2).unwrap();
        let attacker = g.spawn_unit("archer", 0, from);
        let defender = g.spawn_unit("warrior", 1, target);
        let middles: Vec<Pos> = g.nbrs(from).into_iter()
            .filter(|p| g.wdist(*p, target) == 1).collect();
        assert!(!middles.is_empty());
        for middle in &middles {
            g.map.tiles.get_mut(middle).unwrap().terrain = "mountain".to_string();
        }
        let shot = Action::Ranged { unit: attacker, target };
        let legal_shot = |g: &Game| g.legal_actions(0).into_iter().any(|action| {
            matches!(action, Action::Ranged { unit, target: to }
                if unit == attacker && to == target)
        });
        assert!(!legal_shot(&g));
        assert_eq!(g.apply(0, &shot).unwrap_err(), "line of sight blocked");
        assert_eq!(g.units[&defender].hp, 100);

        g.map.tiles.get_mut(&middles[0]).unwrap().terrain = "plains".to_string();
        assert!(legal_shot(&g));
        g.apply(0, &shot).unwrap();
        assert!(g.units[&defender].hp < 100);
    }

    #[test]
    fn melee_attack_requires_enough_movement_to_enter_the_target_tile() {
        let (mut g, target, ring) = controlled_game(305);
        let attacker = g.spawn_unit("warrior", 0, ring[0]);
        g.spawn_unit("warrior", 1, target);
        let tile = g.map.tiles.get_mut(&target).unwrap();
        tile.feature = Some("forest".to_string());
        tile.river = true;
        let attack = Action::Attack { unit: attacker, target };
        let legal_attack = |g: &Game| g.legal_actions(0).into_iter().any(|action| {
            matches!(action, Action::Attack { unit, target: to }
                if unit == attacker && to == target)
        });

        g.units.get_mut(&attacker).unwrap().moves_left = 1.0;
        assert!(!legal_attack(&g));
        assert_eq!(g.apply(0, &attack).unwrap_err(), "not enough movement to attack");

        // The minimum-one-tile rule allows the costly forest/river entry
        // when the unit still has all of its normal Movement.
        g.units.get_mut(&attacker).unwrap().moves_left = 2.0;
        assert!(legal_attack(&g));
        g.apply(0, &attack).unwrap();
        assert_eq!(g.units[&attacker].moves_left, 0.0);
    }

    #[test]
    fn zoc_is_innate_and_the_unit_roster_uses_explicit_civ6_classes() {
        let (g, _, _) = controlled_game(306);
        let actual: BTreeSet<&str> = g
            .rules
            .units
            .iter()
            .filter(|(_, spec)| spec.zone_of_control)
            .map(|(name, _)| name.as_str())
            .collect();
        let expected: BTreeSet<&str> = [
            "scout",
            "warrior",
            "spearman",
            "swordsman",
            "horseman",
            "pikeman",
            "knight",
            "musketman",
            "galley",
            "legion",
            "hoplite",
            "eagle_warrior",
            "war_cart",
        ]
        .into_iter()
        .collect();
        assert_eq!(actual, expected);

        for name in ["horseman", "knight", "war_cart"] {
            let spec = &g.rules.units[name];
            assert!(spec.zone_of_control, "{name} must exert ZOC");
            assert!(spec.cavalry, "{name} must ignore incoming ZOC");
        }
        for name in [
            "slinger",
            "archer",
            "catapult",
            "crossbowman",
            "pitati_archer",
            "maryannu_chariot_archer",
            "saka_horse_archer",
            "crouching_tiger",
        ] {
            assert!(!g.rules.units[name].zone_of_control, "{name} must not exert ZOC");
        }
        assert!(g.players.iter().all(|p| !p.civics.contains("military_tradition")));
    }

    #[test]
    fn zoc_stops_combatants_but_cavalry_ignores_and_rivers_block_it() {
        let (mut g, enemy_pos, ring) = controlled_game(307);
        g.spawn_unit("warrior", 1, enemy_pos);
        let entry = ring[0];
        let start = g
            .nbrs(entry)
            .into_iter()
            .find(|p| g.wdist(*p, enemy_pos) == 2)
            .unwrap();
        let warrior = g.spawn_unit("warrior", 0, start);
        g.apply(0, &Action::Move { unit: warrior, to: entry }).unwrap();
        assert!(g.units[&warrior].zoc_stopped);
        assert_eq!(g.units[&warrior].moves_left, 1.0);
        assert!(g.legal_actions(0).into_iter().any(|action| {
            matches!(action, Action::Attack { unit, target }
                if unit == warrior && target == enemy_pos)
        }));

        let (mut g, enemy_pos, ring) = controlled_game(308);
        g.spawn_unit("warrior", 1, enemy_pos);
        let entry = ring[0];
        let start = g
            .nbrs(entry)
            .into_iter()
            .find(|p| g.wdist(*p, enemy_pos) == 2)
            .unwrap();
        let horse = g.spawn_unit("horseman", 0, start);
        g.apply(0, &Action::Move { unit: horse, to: entry }).unwrap();
        assert!(!g.units[&horse].zoc_stopped);
        assert!(g.units[&horse].moves_left > 0.0);

        let (mut g, enemy_pos, ring) = controlled_game(309);
        g.spawn_unit("warrior", 1, enemy_pos);
        g.map.tiles.get_mut(&ring[0]).unwrap().river = true;
        assert!(!g.in_enemy_zoc(0, ring[0]));
    }

    #[test]
    fn civilian_support_religious_and_district_zoc_follow_civ6_behavior() {
        for (seed, kind) in [(310, "builder"), (311, "battering_ram")] {
            let (mut g, enemy_pos, ring) = controlled_game(seed);
            g.spawn_unit("warrior", 1, enemy_pos);
            let entry = ring[0];
            let start = g
                .nbrs(entry)
                .into_iter()
                .find(|p| g.wdist(*p, enemy_pos) == 2)
                .unwrap();
            let mover = g.spawn_unit(kind, 0, start);
            g.apply(0, &Action::Move { unit: mover, to: entry }).unwrap();
            assert_eq!(g.units[&mover].moves_left, 0.0, "{kind}");
            assert!(g.units[&mover].zoc_stopped, "{kind}");
        }

        let (mut g, enemy_pos, ring) = controlled_game(312);
        g.at_war.clear();
        g.players[0].religion = Some("A".to_string());
        g.players[1].religion = Some("B".to_string());
        g.spawn_unit("missionary", 1, enemy_pos);
        let entry = ring[0];
        let start = g
            .nbrs(entry)
            .into_iter()
            .find(|p| g.wdist(*p, enemy_pos) == 2)
            .unwrap();
        let missionary = g.spawn_unit("missionary", 0, start);
        g.apply(0, &Action::Move { unit: missionary, to: entry }).unwrap();
        assert!(g.units[&missionary].zoc_stopped);
        assert!(g.units[&missionary].moves_left > 0.0);

        let (mut g, city_pos, ring) = controlled_game(313);
        g.found_city_for(1, city_pos, Some("Test".to_string()));
        assert!(g.in_enemy_zoc(0, ring[0]));
    }

    #[test]
    fn city_garrisons_are_protected_and_a_siege_ring_prevents_healing() {
        let (mut g, city_pos, ring) = controlled_game(314);
        let cid = g.found_city_for(1, city_pos, Some("Test".to_string()));
        let garrison = g.spawn_unit("warrior", 1, city_pos);
        let archer = g.spawn_unit("archer", 0, ring[0]);
        let before = g.cities[&cid].hp;
        g.apply(0, &Action::Ranged { unit: archer, target: city_pos })
            .unwrap();
        assert!(g.cities[&cid].hp < before);
        assert_eq!(g.units[&garrison].hp, 100);

        g.cities.get_mut(&cid).unwrap().hp = 100;
        for pos in ring {
            g.spawn_unit("warrior", 0, pos);
        }
        assert!(g.city_under_siege(cid));
        g.process_city(1, cid);
        assert_eq!(g.cities[&cid].hp, 100);
    }

    #[test]
    fn combat_xp_and_fortification_use_civ6_timing_and_modifiers() {
        let (mut g, target, ring) = controlled_game(315);
        g.players[0].civ = "Nubia".to_string();
        let archer = g.spawn_unit("archer", 0, ring[0]);
        let defender = g.spawn_unit("warrior", 1, target);
        g.apply(0, &Action::Ranged { unit: archer, target }).unwrap();
        assert_eq!(g.units[&archer].xp, 5);
        assert_eq!(g.units[&defender].xp, 2);
        assert_eq!(g.modified_xp(defender, 2.49), 2);
        assert_eq!(g.modified_xp(defender, 2.50), 3,
                   "half an XP rounds upward, while smaller fractions do not");

        g.players[0].government = Some("oligarchy".to_string());
        assert_eq!(g.modified_xp(archer, 3.0), 5,
                   "Nubia's 50% and Oligarchy's 20% XP modifiers stack");

        let scout = g.spawn_unit("scout", 0, ring[2]);
        g.players[0].policies.insert("survey".to_string());
        let strong_enemy = g.spawn_unit("swordsman", 1, ring[3]);
        let enemy = g.units[&strong_enemy].clone();
        g.award_unit_combat_xp(scout, &enemy, false, true, true);
        assert_eq!(g.units[&scout].xp, 8,
                   "the unit-combat XP cap applies after percentage modifiers");

        g.players[1].is_barbarian = true;
        let barb = g.units[&strong_enemy].clone();
        g.units.get_mut(&scout).unwrap().level = 2;
        g.award_unit_combat_xp(scout, &barb, false, true, true);
        assert_eq!(g.units[&scout].xp, 9,
                   "post-promotion barbarian combat grants exactly 1 XP");

        let veteran = g.spawn_unit("warrior", 0, ring[4]);
        g.units.get_mut(&veteran).unwrap().xp = 420;
        g.begin_turn(0);
        assert_eq!(g.units[&veteran].level, 8);

        let (mut g, _, ring) = controlled_game(316);
        let unit = g.spawn_unit("warrior", 0, ring[0]);
        let destination = ring[1];
        g.apply(0, &Action::Move { unit, to: destination }).unwrap();
        g.apply(0, &Action::Fortify { unit }).unwrap();
        assert_eq!(g.units[&unit].fortify_turns, 0);
        g.begin_turn(0);
        assert_eq!(g.units[&unit].fortify_turns, 1);
        g.begin_turn(0);
        assert_eq!(g.units[&unit].fortify_turns, 2);
    }
}

#[cfg(test)]
mod victory_conditions {
    use super::*;

    fn game_with_capitals(players: usize, seed: u64, max_turns: u32) -> Game {
        let mut g = Game::new_full(players, 26, 16, seed, max_turns, 0, false);
        for pid in 0..players {
            let pos = g
                .player_unit_ids(pid)
                .into_iter()
                .find_map(|uid| {
                    let u = &g.units[&uid];
                    (u.kind == "settler").then_some(u.pos)
                })
                .unwrap();
            g.found_city_for(pid, pos, None);
        }
        g
    }

    #[test]
    fn science_requires_the_space_race_and_exoplanet_arrival() {
        let protocol_item: Item = serde_json::from_str(
            r#"{"project":"launch_earth_satellite"}"#,
        )
        .unwrap();
        assert_eq!(
            protocol_item,
            Item::Project {
                project: "launch_earth_satellite".to_string()
            }
        );

        let mut g = game_with_capitals(2, 401, 300);
        let all_techs: Vec<String> = g.rules.techs.keys().cloned().collect();
        for tech in all_techs.iter().filter(|t| t.as_str() != "offworld_mission") {
            g.players[0].techs.insert(tech.clone());
        }
        g.players[0].research = Some("offworld_mission".to_string());
        g.players[0].research_progress = g.rules.techs["offworld_mission"].cost;
        g.begin_turn(0);
        assert_eq!(g.players[0].techs.len(), g.rules.techs.len());
        assert_eq!(g.winner, None, "finishing the technology tree is not a science victory");

        let cid = g.player_city_ids(0)[0];
        let spaceport = g.cities[&cid].owned_tiles[1];
        g.cities
            .get_mut(&cid)
            .unwrap()
            .districts
            .insert("spaceport".to_string(), spaceport);
        assert_eq!(g.rules.districts["spaceport"].cost, 1800.0);
        assert_eq!(g.rules.projects["launch_earth_satellite"].cost, 900.0);
        assert_eq!(g.rules.projects["launch_moon_landing"].cost, 1500.0);
        assert_eq!(g.rules.projects["launch_mars_colony"].cost, 1800.0);
        assert_eq!(g.rules.projects["exoplanet_expedition"].cost, 2100.0);

        let earth = Item::Project { project: "launch_earth_satellite".to_string() };
        let moon = Item::Project { project: "launch_moon_landing".to_string() };
        let mars = Item::Project { project: "launch_mars_colony".to_string() };
        let exoplanet = Item::Project { project: "exoplanet_expedition".to_string() };
        assert!(g.can_produce(0, cid, &earth));
        assert!(!g.can_produce(0, cid, &moon));
        g.players[0].explored.clear();
        assert!(g.complete_item(0, cid, &earth));
        assert_eq!(g.players[0].explored.len(), g.map.tiles.len(),
                   "Earth Satellite reveals the whole map");
        assert!(g.can_produce(0, cid, &moon));
        let science = g.player_city_ids(0).into_iter()
            .map(|city_id| g.city_yields(city_id).science).sum::<f64>();
        let culture_before = g.players[0].culture_lifetime;
        assert!(g.complete_item(0, cid, &moon));
        assert!((g.players[0].culture_lifetime - culture_before - 10.0 * science).abs() < 1e-9,
                "Moon Landing grants Culture equal to ten turns of Science");
        assert!(!g.can_produce(0, cid, &exoplanet));
        assert!(g.complete_item(0, cid, &mars));
        assert!(g.can_produce(0, cid, &exoplanet));
        assert!(g.complete_item(0, cid, &exoplanet));
        assert_eq!(g.winner, None, "launching is not the same as arriving");

        let laser = Item::Project { project: "lagrange_laser_station".to_string() };
        assert!(g.complete_item(0, cid, &laser));
        assert_eq!(g.exoplanet_speed(0), 2.0);
        for _ in 0..24 {
            g.advance_exoplanet(0);
        }
        assert_eq!(g.players[0].exoplanet_distance, 48.0);
        assert_eq!(g.winner, None);
        g.advance_exoplanet(0);
        assert_eq!(g.players[0].exoplanet_distance, EXOPLANET_DESTINATION);
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("science"));
    }

    #[test]
    fn domination_requires_every_foreign_original_capital() {
        let mut g = game_with_capitals(3, 402, 300);
        let capital = |g: &Game, original_owner: usize| {
            g.cities
                .values()
                .find(|c| c.is_capital && c.original_owner == original_owner)
                .unwrap()
                .id
        };
        let second = capital(&g, 1);
        let third = capital(&g, 2);
        g.capture_city(second, 0);
        assert_eq!(g.winner, None);
        g.capture_city(third, 0);
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("domination"));
    }

    #[test]
    fn religion_must_be_a_strict_majority_in_every_living_civ() {
        let mut g = game_with_capitals(2, 403, 300);
        let extra_pos = g
            .map
            .tiles
            .keys()
            .copied()
            .find(|pos| g.city_at(*pos).is_none())
            .unwrap();
        let extra = g.found_city_for(1, extra_pos, None);
        let religion = "Test Religion".to_string();
        g.players[0].religion = Some(religion.clone());
        let own = g.player_city_ids(0)[0];
        let rival: Vec<u32> = g.player_city_ids(1);
        g.cities.get_mut(&own).unwrap().pressure.insert(religion.clone(), 100.0);
        g.cities.get_mut(&rival[0]).unwrap().pressure.insert(religion.clone(), 100.0);
        g.check_religious_victory();
        assert_eq!(g.winner, None, "one of two rival cities is not a majority");
        g.cities.get_mut(&extra).unwrap().pressure.insert(religion, 100.0);
        g.check_religious_victory();
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("religious"));
    }

    #[test]
    fn culture_requires_more_visiting_tourists_than_the_best_rival_domestic_total() {
        let mut g = game_with_capitals(2, 404, 300);
        g.players[1].culture_lifetime = 1_000.0;
        g.players[0].tourism_lifetime = 4_000.0;
        assert_eq!(g.domestic_tourists(1), 10);
        assert_eq!(g.foreign_tourists(0), 10);
        g.check_culture_victory();
        assert_eq!(g.winner, None, "a tie in tourist counts is not a victory");
        g.players[0].tourism_lifetime = 4_400.0;
        assert_eq!(g.foreign_tourists(0), 11);
        g.check_culture_victory();
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("culture"));
    }

    #[test]
    fn diplomacy_requires_twenty_victory_points() {
        let mut g = Game::new_full(2, 26, 16, 405, 300, 1, false);
        let minor = g.players.iter().find(|p| p.is_minor).unwrap().id;
        g.players[0].envoys = vec![(minor, 6)];
        g.players[0].dvp = 16;
        g.world_era = 2;
        g.turn = 30;
        g.process_congress();
        assert_eq!(g.players[0].dvp, 18);
        assert_eq!(g.winner, None);
        g.turn = 60;
        g.process_congress();
        assert_eq!(g.players[0].dvp, DIPLOMATIC_VICTORY_POINTS);
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("diplomatic"));
    }

    #[test]
    fn score_only_decides_the_game_after_the_turn_limit() {
        let mut g = game_with_capitals(2, 406, 3);
        let capital = g.player_city_ids(0)[0];
        g.cities.get_mut(&capital).unwrap().pop = 200;
        assert!(g.score(0) > 500);
        g.current = 1;
        g.turn = 2;
        g.do_end_turn();
        assert_eq!(g.turn, 3);
        assert_eq!(g.winner, None);
        g.current = 1;
        g.do_end_turn();
        assert_eq!(g.turn, 4);
        assert_eq!(g.winner, Some(0));
        assert_eq!(g.victory_type.as_deref(), Some("score"));
    }
}
