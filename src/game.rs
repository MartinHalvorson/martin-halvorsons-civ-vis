//! Core turn engine (mirrors civvis/game.py — same mechanics and action protocol).
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::rng::Rng;
use crate::rules::{Rules, Yields};
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

pub fn effective_strength(base: f64, hp: i32) -> f64 {
    (base - (100 - hp) as f64 / 10.0).max(1.0)
}

pub fn damage(att: f64, def: f64, rng: &mut Rng) -> i32 {
    let d = 30.0 * ((att - def) / 25.0).exp() * rng.uniform(0.8, 1.2);
    (d.round() as i32).clamp(1, 100)
}

fn pair(a: usize, b: usize) -> (usize, usize) {
    (a.min(b), a.max(b))
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
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(untagged)]
pub enum Item {
    Unit { unit: String },
    Building { building: String },
    District { district: String, pos: Pos },
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
    pub counters: BTreeMap<String, i64>,
    #[serde(default)]
    pub boosted_techs: BTreeSet<String>,
    #[serde(default)]
    pub boosted_civics: BTreeSet<String>,
}

impl Player {
    fn new(id: usize, civ: &str, is_minor: bool) -> Player {
        let mut techs = BTreeSet::new();
        techs.insert("agriculture".to_string());
        Player {
            id,
            civ: civ.to_string(),
            techs,
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
            counters: BTreeMap::new(),
            boosted_techs: BTreeSet::new(),
            boosted_civics: BTreeSet::new(),
        }
    }
}

// ------------------------------------------------------------------- actions

#[derive(Clone, Serialize, Deserialize, Debug)]
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
    occ: BTreeMap<Pos, Vec<u32>>,
    city_by_pos: BTreeMap<Pos, u32>,
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
            occ: BTreeMap::new(),
            city_by_pos: BTreeMap::new(),
        };
        for u in g.units.values() {
            g.occ.entry(u.pos).or_default().push(u.id);
        }
        for c in g.cities.values() {
            g.city_by_pos.insert(c.pos, c.id);
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
        let (map, spawns) = mapgen::generate(&rules, width, height, total, &mut rng);
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
            occ: BTreeMap::new(),
            city_by_pos: BTreeMap::new(),
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
            let crowded = major_spawns.iter().any(|s| hex::distance(*pos, *s) < 4)
                || g.cities.values().any(|c| hex::distance(*pos, c.pos) < 4);
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
            if self.cities.values().any(|c| hex::distance(*pos, c.pos) < 4) {
                continue;
            }
            if self.barb_camps.keys().any(|cp| hex::distance(*pos, *cp) < 4) {
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

    pub fn gov_effects(&self, pid: usize) -> crate::rules::GovEffects {
        match &self.players[pid].government {
            Some(g) => self.rules.governments.get(g)
                .map(|s| s.effects).unwrap_or_default(),
            None => Default::default(),
        }
    }

    pub fn unit_strength(&self, u: &Unit, defending: bool) -> f64 {
        let mut s = self.rules.units[u.kind.as_str()].strength.max(1.0)
            + 5.0 * (u.level - 1) as f64
            + self.gov_effects(u.owner).combat_strength;
        if defending && u.fortified {
            s += 6.0;
        }
        s
    }

    pub fn unit_ranged_strength(&self, u: &Unit) -> f64 {
        let rs = self.rules.units[u.kind.as_str()].ranged_strength;
        if rs <= 0.0 {
            return 0.0;
        }
        rs + 5.0 * (u.level - 1) as f64 + self.gov_effects(u.owner).combat_strength
    }

    pub fn city_housing(&self, city: &City) -> f64 {
        let mut h = 2.0;
        if hex::neighbors(city.pos).iter().any(|n| {
            self.map.get(*n).map(|t| self.rules.is_water(t)).unwrap_or(false)
        }) {
            h += 2.0;
        }
        for b in &city.buildings {
            h += self.rules.buildings[b.as_str()].housing;
        }
        h + self.gov_effects(city.owner).housing
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
        !city.struck && city.buildings.iter()
            .any(|b| b == "walls" || b == "medieval_walls")
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
        let u = Unit {
            id: self.next_id,
            kind: kind.to_string(),
            owner,
            pos,
            hp: 100,
            moves_left: spec.moves,
            charges: spec.charges,
            xp: 0,
            level: 1,
            fortified: false,
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
        for p in hex::disk(pos, radius) {
            if self.map.tiles.contains_key(&p) {
                self.players[pid].explored.insert(p);
            }
        }
    }

    pub fn can_move(&self, uid: u32, pos: Pos) -> bool {
        self.can_enter(uid, self.units[&uid].pos, pos)
    }

    fn can_enter(&self, uid: u32, from: Pos, pos: Pos) -> bool {
        let u = &self.units[&uid];
        if hex::distance(from, pos) != 1 {
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
        } else if water {
            return false;
        }
        for oid in self.units_at(pos) {
            let o = &self.units[&oid];
            let ospec = &self.rules.units[o.kind.as_str()];
            if o.owner != u.owner {
                if ospec.class == "military" || spec.class == "civilian" {
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

    fn flow(&self, uid: u32, start: Pos, moves: f64) -> BTreeMap<Pos, f64> {
        let mut best: BTreeMap<Pos, f64> = BTreeMap::new();
        best.insert(start, moves);
        let mut queue = vec![start];
        while let Some(cur) = queue.pop() {
            let rem = best[&cur];
            if rem <= 0.0 {
                continue;
            }
            for n in hex::neighbors(cur) {
                if !self.map.tiles.contains_key(&n) || !self.can_enter(uid, cur, n) {
                    continue;
                }
                let cost = self.rules.move_cost(&self.map.tiles[&n]);
                let new_rem = (rem - cost).max(0.0);
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
        let mut best: BTreeMap<Pos, f64> = BTreeMap::new();
        let mut parent: BTreeMap<Pos, Pos> = BTreeMap::new();
        best.insert(start, moves);
        let mut queue = vec![start];
        while let Some(cur) = queue.pop() {
            let rem = best[&cur];
            if rem <= 0.0 {
                continue;
            }
            for n in hex::neighbors(cur) {
                if !self.map.tiles.contains_key(&n) || !self.can_enter(uid, cur, n) {
                    continue;
                }
                let cost = self.rules.move_cost(&self.map.tiles[&n]);
                let new_rem = (rem - cost).max(0.0);
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
            self.do_move(pid, uid, step)?;
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
            if hex::distance(c.pos, u.pos) < 4 {
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

    pub fn city_strength(&self, cid: u32) -> f64 {
        let city = &self.cities[&cid];
        let mut s = 10.0 + 2.0 * city.pop as f64;
        if city.buildings.iter().any(|b| b == "walls") {
            s += 10.0;
        }
        if city.buildings.iter().any(|b| b == "medieval_walls") {
            s += 15.0;
        }
        if city.districts.contains_key("encampment") {
            let d = self.rules.districts["encampment"].defense;
            s += if d > 0.0 { d } else { 10.0 };
        }
        let garrison = self.units_at(city.pos).into_iter().any(|id| {
            let o = &self.units[&id];
            o.owner == city.owner && self.rules.units[o.kind.as_str()].class == "military"
        });
        if garrison {
            s += 5.0;
        }
        s
    }

    pub fn district_yields(&self, dname: &str, dpos: Pos) -> Yields {
        let spec = &self.rules.districts[dname];
        let mut ys = spec.yields;
        if !spec.adjacency.is_empty() {
            let (mut mountain, mut forest, mut district) = (0, 0, 0);
            for n in hex::neighbors(dpos) {
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
                }
            }
            for (key, bonus) in &spec.adjacency {
                let n = match key.as_str() {
                    "mountain" => mountain,
                    "forest" => forest,
                    "district" => district,
                    _ => 0,
                } as f64;
                ys.food += (n * bonus.food).trunc();
                ys.production += (n * bonus.production).trunc();
                ys.gold += (n * bonus.gold).trunc();
                ys.science += (n * bonus.science).trunc();
                ys.culture += (n * bonus.culture).trunc();
                ys.faith += (n * bonus.faith).trunc();
            }
        }
        ys
    }

    pub fn city_yields(&self, cid: u32) -> Yields {
        let city = &self.cities[&cid];
        let mut ys = Yields::default();
        let mut center = self.rules.tile_yields(&self.map.tiles[&city.pos]);
        center.food = center.food.max(2.0);
        center.production = center.production.max(1.0);
        ys.add(center);
        let mut cands: Vec<(f64, Pos, Yields)> = Vec::new();
        for pos in &city.owned_tiles {
            if *pos == city.pos {
                continue;
            }
            let t = &self.map.tiles[pos];
            if t.district.is_some() {
                continue;
            }
            let tys = self.rules.tile_yields(t);
            let val = tys.food * 1.5 + tys.production * 1.5 + tys.gold * 0.7
                + tys.science + tys.culture + tys.faith;
            cands.push((val, *pos, tys));
        }
        cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        for (_, _, tys) in cands.iter().take(city.pop as usize) {
            ys.add(*tys);
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
        }
        let eff = self.gov_effects(city.owner);
        ys.production *= 1.0 + eff.production_pct / 100.0;
        ys.science *= 1.0 + eff.science_pct / 100.0;
        ys.gold *= 1.0 + eff.gold_pct / 100.0;
        let m = self.amenity_yield_mult(city);
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
            if *pos == city.pos || hex::distance(*pos, city.pos) > 3 {
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
                if !self.unlocked(pid, &spec.tech, &None) {
                    return false;
                }
                if let Some(res) = &spec.requires_resource {
                    if !self.has_resource(pid, res) {
                        return false;
                    }
                }
                if spec.domain.as_deref() == Some("sea") {
                    let coastal = hex::neighbors(city.pos).iter().any(|n| {
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
            if u.moves_left > 0.0 {
                for n in hex::neighbors(u.pos) {
                    if self.can_move(uid, n) {
                        acts.push(Action::Move { unit: uid, to: n });
                    }
                }
                if spec.class == "military" {
                    if spec.ranged_strength > 0.0 {
                        for pos in hex::disk(u.pos, spec.range.max(1)) {
                            if pos == u.pos || !self.map.tiles.contains_key(&pos) {
                                continue;
                            }
                            if self.enemy_target_at(pid, pos) {
                                acts.push(Action::Ranged { unit: uid, target: pos });
                            }
                        }
                    } else {
                        for pos in hex::neighbors(u.pos) {
                            if self.map.tiles.contains_key(&pos)
                                && self.enemy_target_at(pid, pos)
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
            let u = &self.units[&uid];
            let spec = &self.rules.units[u.kind.as_str()];
            if spec.class == "military" && u.moves_left > 0.0 && !u.fortified {
                acts.push(Action::Fortify { unit: uid });
            }
        }
        for cid in self.player_city_ids(pid) {
            if self.city_can_strike(&self.cities[&cid]) {
                let cpos = self.cities[&cid].pos;
                for pos in hex::disk(cpos, 2) {
                    if !self.map.tiles.contains_key(&pos) {
                        continue;
                    }
                    let hit = self.units_at(pos).into_iter().any(|oid| {
                        let o = &self.units[&oid];
                        o.owner != pid && self.is_at_war(pid, o.owner)
                    });
                    if hit {
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

    fn enemy_target_at(&self, pid: usize, pos: Pos) -> bool {
        for oid in self.units_at(pos) {
            if self.units[&oid].owner != pid {
                return true;
            }
        }
        if let Some(cid) = self.city_at(pos) {
            return self.cities[&cid].owner != pid;
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
        match action {
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
            Action::CityStrike { city, target } => self.do_city_strike(pid, *city, *target),
            Action::EndTurn => {
                self.do_end_turn();
                Ok(())
            }
        }
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
        if !self.can_move(uid, to) {
            return Err("invalid move".into());
        }
        for oid in self.units_at(to) {
            if self.units[&oid].owner != pid {
                self.units.get_mut(&oid).unwrap().owner = pid; // capture civilian
            }
        }
        let cost = self.rules.move_cost(&self.map.tiles[&to]);
        self.units.get_mut(&uid).unwrap().fortified = false;
        self.relocate(uid, to);
        let mu = self.units.get_mut(&uid).unwrap();
        mu.moves_left = (mu.moves_left - cost).max(0.0);
        self.maybe_clear_camp(uid);
        Ok(())
    }

    fn auto_declare_war(&mut self, a: usize, b: usize) {
        if a != b && !self.is_at_war(a, b) {
            self.at_war.insert(pair(a, b));
        }
    }

    fn tile_defense_bonus(&self, pos: Pos) -> f64 {
        let t = &self.map.tiles[&pos];
        if t.hills
            || t.feature.as_deref() == Some("forest")
            || t.feature.as_deref() == Some("jungle")
        {
            3.0
        } else {
            0.0
        }
    }

    fn do_attack(&mut self, pid: usize, uid: u32, target: Pos) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        let spec = self.rules.units[u.kind.as_str()].clone();
        if spec.class != "military" || spec.ranged_strength > 0.0 {
            return Err("unit cannot melee attack".into());
        }
        if u.moves_left <= 0.0 {
            return Err("no moves left".into());
        }
        if hex::distance(u.pos, target) != 1 {
            return Err("target not adjacent".into());
        }
        let enemy_ids: Vec<u32> = self
            .units_at(target)
            .into_iter()
            .filter(|id| self.units[id].owner != pid)
            .collect();
        let mut city_id = self.city_at(target);
        if let Some(cid) = city_id {
            if self.cities[&cid].owner == pid {
                city_id = None;
            }
        }
        if enemy_ids.is_empty() && city_id.is_none() {
            return Err("nothing to attack".into());
        }
        for eid in &enemy_ids {
            let o = self.units[eid].owner;
            self.auto_declare_war(pid, o);
        }
        if let Some(cid) = city_id {
            let o = self.cities[&cid].owner;
            self.auto_declare_war(pid, o);
        }
        let military: Vec<u32> = enemy_ids
            .iter()
            .cloned()
            .filter(|id| self.rules.units[self.units[id].kind.as_str()].class == "military")
            .collect();
        let att = {
            let u2 = &self.units[&uid];
            effective_strength(self.unit_strength(u2, false), u2.hp)
        };
        {
            let mu = self.units.get_mut(&uid).unwrap();
            mu.moves_left = 0.0;
            mu.fortified = false;
        }
        if !military.is_empty() {
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
            let ds = effective_strength(self.unit_strength(&d, true), d.hp)
                + self.tile_defense_bonus(target);
            let dmg_out = damage(att, ds, &mut self.rng);
            let dmg_in = damage(ds, att, &mut self.rng);
            self.units.get_mut(&did).unwrap().hp -= dmg_out;
            {
                let mu = self.units.get_mut(&uid).unwrap();
                mu.hp -= dmg_in;
                mu.xp += 5;
            }
            self.units.get_mut(&did).unwrap().xp += 4;
            let d_dead = self.units[&did].hp <= 0;
            let downer = self.units[&did].owner;
            if d_dead {
                self.units.get_mut(&uid).unwrap().xp += 3;
                bump(&mut self.players[pid], "kills");
                self.remove_unit(did);
                self.on_unit_lost(downer);
            }
            if self.units.get(&uid).map(|x| x.hp <= 0).unwrap_or(true) {
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
                let cs = self.city_strength(cid);
                let dmg_out = damage(att, cs, &mut self.rng);
                let dmg_in = damage(cs, att, &mut self.rng);
                self.cities.get_mut(&cid).unwrap().hp -= dmg_out;
                {
                    let mu = self.units.get_mut(&uid).unwrap();
                    mu.hp -= dmg_in;
                    mu.xp += 3;
                }
                if self.units[&uid].hp <= 0 {
                    self.remove_unit(uid);
                    self.on_unit_lost(pid);
                    let c = self.cities.get_mut(&cid).unwrap();
                    c.hp = c.hp.max(1);
                    return Ok(());
                }
                if self.cities[&cid].hp <= 0 {
                    if self.players[pid].is_barbarian {
                        self.cities.get_mut(&cid).unwrap().hp = 1;
                    } else {
                        self.capture_city(cid, pid);
                        self.enter_tile(uid, target);
                    }
                }
            }
        } else {
            self.enter_tile(uid, target); // undefended civilians: capture
        }
        Ok(())
    }

    fn enter_tile(&mut self, uid: u32, pos: Pos) {
        let owner = self.units[&uid].owner;
        for oid in self.units_at(pos) {
            if self.units[&oid].owner != owner {
                self.units.get_mut(&oid).unwrap().owner = owner;
            }
        }
        self.relocate(uid, pos);
        self.maybe_clear_camp(uid);
    }

    fn do_ranged(&mut self, pid: usize, uid: u32, target: Pos) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        let spec = self.rules.units[u.kind.as_str()].clone();
        if spec.ranged_strength <= 0.0 {
            return Err("unit has no ranged attack".into());
        }
        if u.moves_left <= 0.0 {
            return Err("no moves left".into());
        }
        if hex::distance(u.pos, target) > spec.range.max(1) {
            return Err("out of range".into());
        }
        let enemy_ids: Vec<u32> = self
            .units_at(target)
            .into_iter()
            .filter(|id| self.units[id].owner != pid)
            .collect();
        let mut city_id = self.city_at(target);
        if let Some(cid) = city_id {
            if self.cities[&cid].owner == pid {
                city_id = None;
            }
        }
        if enemy_ids.is_empty() && city_id.is_none() {
            return Err("nothing to attack".into());
        }
        for eid in &enemy_ids {
            let o = self.units[eid].owner;
            self.auto_declare_war(pid, o);
        }
        if let Some(cid) = city_id {
            let o = self.cities[&cid].owner;
            self.auto_declare_war(pid, o);
        }
        let att = {
            let u2 = &self.units[&uid];
            effective_strength(self.unit_ranged_strength(u2), u2.hp)
        };
        {
            let mu = self.units.get_mut(&uid).unwrap();
            mu.moves_left = 0.0;
            mu.fortified = false;
            mu.xp += 3;
        }
        let military: Vec<u32> = enemy_ids
            .iter()
            .cloned()
            .filter(|id| self.rules.units[self.units[id].kind.as_str()].class == "military")
            .collect();
        if !military.is_empty() {
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
            let ds = effective_strength(
                self.unit_strength(&self.units[&did], true), self.units[&did].hp)
                + self.tile_defense_bonus(target);
            let dmg = damage(att, ds, &mut self.rng);
            self.units.get_mut(&did).unwrap().hp -= dmg;
            if self.units[&did].hp <= 0 {
                bump(&mut self.players[pid], "kills");
                let downer = self.units[&did].owner;
                self.remove_unit(did);
                self.on_unit_lost(downer);
            }
        } else if !enemy_ids.is_empty() {
            let did = enemy_ids[0];
            let dmg = damage(att, 1.0, &mut self.rng);
            self.units.get_mut(&did).unwrap().hp -= dmg;
            if self.units[&did].hp <= 0 {
                bump(&mut self.players[pid], "kills");
                let downer = self.units[&did].owner;
                self.remove_unit(did);
                self.on_unit_lost(downer);
            }
        } else if let Some(cid) = city_id {
            let cs = self.city_strength(cid);
            let dmg = damage(att, cs, &mut self.rng);
            let c = self.cities.get_mut(&cid).unwrap();
            c.hp = (c.hp - dmg).max(1);
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
        };
        {
            let center = self.map.tiles.get_mut(&pos).unwrap();
            center.feature = None;
            center.improvement = None;
        }
        let mut claim = vec![pos];
        claim.extend(hex::neighbors(pos));
        for tpos in claim {
            if let Some(t) = self.map.tiles.get_mut(&tpos) {
                if t.owner_city.is_none() {
                    t.owner_city = Some(cid);
                    city.owned_tiles.push(tpos);
                }
            }
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
        let it = Item::Unit { unit: unit.to_string() };
        if !self.can_produce(pid, cid, &it) {
            return Err("cannot buy that".into());
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
        if p.boosted_techs.contains(tech) {
            p.research_progress += 0.4 * cost;
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
        if p.boosted_civics.contains(civic) {
            p.civic_progress += 0.4 * cost;
        }
        Ok(())
    }

    fn do_fortify(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        if self.rules.units[u.kind.as_str()].class != "military" {
            return Err("only military units fortify".into());
        }
        let mu = self.units.get_mut(&uid).unwrap();
        mu.fortified = true;
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
        if hex::distance(self.cities[&cid].pos, target) > 2 {
            return Err("out of range".into());
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
        let did = if military.is_empty() {
            enemies[0]
        } else {
            *military.iter().max_by(|a, b| {
                let ea = effective_strength(
                    self.unit_strength(&self.units[*a], true), self.units[*a].hp);
                let eb = effective_strength(
                    self.unit_strength(&self.units[*b], true), self.units[*b].hp);
                ea.partial_cmp(&eb).unwrap()
            }).unwrap()
        };
        let d = self.units[&did].clone();
        let ds = effective_strength(self.unit_strength(&d, true), d.hp)
            + self.tile_defense_bonus(target);
        let att = self.city_strength(cid);
        let dmg = damage(att, ds, &mut self.rng);
        self.units.get_mut(&did).unwrap().hp -= dmg;
        if self.units[&did].hp <= 0 {
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
        Ok(())
    }

    fn do_make_peace(&mut self, pid: usize, other: usize) -> Result<(), String> {
        if !self.at_war.remove(&pair(pid, other)) {
            return Err("not at war".into());
        }
        Ok(())
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
        self.check_boosts(pid);
        for uid in self.player_unit_ids(pid) {
            let (kind, hp, pos) = {
                let u = &self.units[&uid];
                (u.kind.clone(), u.hp, u.pos)
            };
            let moves = self.rules.units[kind.as_str()].moves;
            let mut heal = 0;
            if hp < 100 {
                let own = self.map.tiles[&pos]
                    .owner_city
                    .map(|oc| self.cities[&oc].owner == pid)
                    .unwrap_or(false);
                heal = if own { 15 } else { 10 };
            }
            let u = self.units.get_mut(&uid).unwrap();
            u.moves_left = moves;
            u.hp = (u.hp + heal).min(100);
            while u.level < 4
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
        let n_units = self.player_unit_ids(pid).len() as f64;
        gold -= (n_units - 3.0).max(0.0);
        {
            let p = &mut self.players[pid];
            p.gold = (p.gold + gold).max(0.0);
            p.faith += faith;
        }
        let total_techs = self.rules.techs.len();
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
                let done_all = p.techs.len() >= total_techs;
                let minor = p.is_minor;
                if !minor && done_all {
                    self.set_winner(pid, "science");
                }
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
                let p = &mut self.players[pid];
                p.boosted_techs.insert(name.clone());
                if p.research.as_deref() == Some(name.as_str()) {
                    p.research_progress += 0.4 * cost;
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
                let p = &mut self.players[pid];
                p.boosted_civics.insert(name.clone());
                if p.civic.as_deref() == Some(name.as_str()) {
                    p.civic_progress += 0.4 * cost;
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
                hex::neighbors(c.pos).iter().any(|nb| {
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
                surplus *= hf * af;
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
            city.production += ys.production;
        }
        let queue_head = self.cities[&cid].queue.first().cloned();
        if let Some(item) = queue_head {
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
        {
            let owned = self.cities[&cid].owned_tiles.len() as i32;
            let city = self.cities.get_mut(&cid).unwrap();
            city.border_culture += 1.0 + ys.culture * 0.5;
            let need_b = (15 + 8 * (owned - 7).max(0)) as f64;
            if city.border_culture >= need_b {
                city.border_culture -= need_b;
                drop(city);
                self.expand_borders(cid);
            }
        }
        let city = self.cities.get_mut(&cid).unwrap();
        city.hp = (city.hp + 10).min(200);
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
                self.cities.get_mut(&cid).unwrap().buildings.push(building.clone());
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
        }
    }

    fn place_new_unit(&mut self, kind: &str, owner: usize, pos: Pos) -> Option<u32> {
        let spec = self.rules.units[kind].clone();
        let want_sea = spec.domain.as_deref() == Some("sea");
        let mut cands = vec![pos];
        cands.extend(hex::neighbors(pos));
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
            for n in hex::neighbors(*pos) {
                let t = match self.map.get(n) {
                    Some(t) => t,
                    None => continue,
                };
                if t.owner_city.is_some() || hex::distance(n, city_pos) > 3 {
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
            city.buildings.retain(|b| b != "walls");
        }
        let pos = self.cities[&cid].pos;
        for oid in self.units_at(pos) {
            if self.units[&oid].owner == old {
                self.units.get_mut(&oid).unwrap().owner = new_owner;
            }
        }
        bump(&mut self.players[new_owner], "captures");
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
        let alive: Vec<usize> = self
            .players
            .iter()
            .filter(|p| p.alive && !p.is_minor)
            .map(|p| p.id)
            .collect();
        if alive.len() == 1 {
            self.set_winner(alive[0], "domination");
            return;
        }
        let capitals: Vec<&City> = self.cities.values().filter(|c| c.is_capital).collect();
        if capitals.len() >= 2 {
            let owners: BTreeSet<usize> = capitals.iter().map(|c| c.owner).collect();
            if owners.len() == 1 {
                let w = *owners.iter().next().unwrap();
                self.set_winner(w, "domination");
            }
        }
    }

    fn set_winner(&mut self, pid: usize, vtype: &str) {
        if self.winner.is_none() {
            self.winner = Some(pid);
            self.victory_type = Some(vtype.to_string());
        }
    }
}
