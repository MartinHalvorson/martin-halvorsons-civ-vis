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

// ------------------------------------------------------------------ entities

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
        }
    }
}

// ------------------------------------------------------------------- actions

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Move { unit: u32, to: Pos },
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
        g
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
        self.at_war.contains(&pair(a, b))
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
        };
        self.next_id += 1;
        let id = u.id;
        self.occ.entry(pos).or_default().push(id);
        self.units.insert(id, u);
        self.reveal(owner, pos, 2);
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
        self.reveal(owner, pos, 2);
    }

    fn reveal(&mut self, pid: usize, pos: Pos, radius: i32) {
        for p in hex::disk(pos, radius) {
            if self.map.tiles.contains_key(&p) {
                self.players[pid].explored.insert(p);
            }
        }
    }

    pub fn can_move(&self, uid: u32, pos: Pos) -> bool {
        let u = &self.units[&uid];
        if hex::distance(u.pos, pos) != 1 {
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
        for o in &self.players {
            if o.id != pid && o.alive {
                if self.is_at_war(pid, o.id) {
                    acts.push(Action::MakePeace { player: o.id });
                } else {
                    acts.push(Action::DeclareWar { player: o.id });
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
        self.relocate(uid, to);
        let mu = self.units.get_mut(&uid).unwrap();
        mu.moves_left = (mu.moves_left - cost).max(0.0);
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
        let att = effective_strength(spec.strength, u.hp);
        self.units.get_mut(&uid).unwrap().moves_left = 0.0;
        if !military.is_empty() {
            let did = *military
                .iter()
                .max_by(|a, b| {
                    let ea = effective_strength(
                        self.rules.units[self.units[*a].kind.as_str()].strength.max(1.0),
                        self.units[*a].hp,
                    );
                    let eb = effective_strength(
                        self.rules.units[self.units[*b].kind.as_str()].strength.max(1.0),
                        self.units[*b].hp,
                    );
                    ea.partial_cmp(&eb).unwrap()
                })
                .unwrap();
            let d = self.units[&did].clone();
            let ds = effective_strength(
                self.rules.units[d.kind.as_str()].strength.max(1.0),
                d.hp,
            ) + self.tile_defense_bonus(target);
            let dmg_out = damage(att, ds, &mut self.rng);
            let dmg_in = damage(ds, att, &mut self.rng);
            self.units.get_mut(&did).unwrap().hp -= dmg_out;
            self.units.get_mut(&uid).unwrap().hp -= dmg_in;
            let d_dead = self.units[&did].hp <= 0;
            if d_dead {
                let downer = self.units[&did].owner;
                self.remove_unit(did);
                self.on_unit_lost(downer);
            }
            if self.units.get(&uid).map(|x| x.hp <= 0).unwrap_or(true) {
                if self.units.contains_key(&uid) {
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
                self.units.get_mut(&uid).unwrap().hp -= dmg_in;
                if self.units[&uid].hp <= 0 {
                    self.remove_unit(uid);
                    self.on_unit_lost(pid);
                    let c = self.cities.get_mut(&cid).unwrap();
                    c.hp = c.hp.max(1);
                    return Ok(());
                }
                if self.cities[&cid].hp <= 0 {
                    self.capture_city(cid, pid);
                    self.enter_tile(uid, target);
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
        let att = effective_strength(spec.ranged_strength, u.hp);
        self.units.get_mut(&uid).unwrap().moves_left = 0.0;
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
                        self.rules.units[self.units[*a].kind.as_str()].strength.max(1.0),
                        self.units[*a].hp,
                    );
                    let eb = effective_strength(
                        self.rules.units[self.units[*b].kind.as_str()].strength.max(1.0),
                        self.units[*b].hp,
                    );
                    ea.partial_cmp(&eb).unwrap()
                })
                .unwrap();
            let ds = effective_strength(
                self.rules.units[self.units[&did].kind.as_str()].strength.max(1.0),
                self.units[&did].hp,
            ) + self.tile_defense_bonus(target);
            let dmg = damage(att, ds, &mut self.rng);
            self.units.get_mut(&did).unwrap().hp -= dmg;
            if self.units[&did].hp <= 0 {
                let downer = self.units[&did].owner;
                self.remove_unit(did);
                self.on_unit_lost(downer);
            }
        } else if !enemy_ids.is_empty() {
            let did = enemy_ids[0];
            let dmg = damage(att, 1.0, &mut self.rng);
            self.units.get_mut(&did).unwrap().hp -= dmg;
            if self.units[&did].hp <= 0 {
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
        let p = &mut self.players[pid];
        p.research = Some(tech.to_string());
        p.research_progress = p.research_overflow;
        p.research_overflow = 0.0;
        Ok(())
    }

    fn do_civic(&mut self, pid: usize, civic: &str) -> Result<(), String> {
        if self.players[pid].civic.is_some() {
            return Err("already working a civic".into());
        }
        if !self.available_civics(pid).iter().any(|c| c == civic) {
            return Err("civic unavailable".into());
        }
        let p = &mut self.players[pid];
        p.civic = Some(civic.to_string());
        p.civic_progress = p.civic_overflow;
        p.civic_overflow = 0.0;
        Ok(())
    }

    fn do_declare_war(&mut self, pid: usize, other: usize) -> Result<(), String> {
        if other == pid || other >= self.players.len() || !self.players[other].alive {
            return Err("invalid war target".into());
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

    fn process_city(&mut self, pid: usize, cid: u32) -> Yields {
        let ys = self.city_yields(cid);
        {
            let city = self.cities.get_mut(&cid).unwrap();
            city.food += ys.food - 2.0 * city.pop as f64;
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
        self.check_elimination(old);
        self.check_domination();
    }

    fn on_unit_lost(&mut self, pid: usize) {
        self.check_elimination(pid);
        self.check_domination();
    }

    fn check_elimination(&mut self, pid: usize) {
        if !self.players[pid].alive {
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
