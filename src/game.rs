//! Core turn engine (mirrors civvis/game.py — same mechanics and action protocol).
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};

use crate::rng::Rng;
use crate::rules::{Rules, Yields};
use crate::setup::MapSize;
use crate::world::WorldMap;
use crate::{hex, mapgen, Pos};

pub const CIV_NAMES: [&str; 8] = [
    "Rome", "Egypt", "Greece", "China", "Sumeria", "Aztec", "Nubia", "Scythia",
];
pub const CITY_STATE_NAMES: [&str; 12] = [
    "Kabul",
    "Geneva",
    "Carthage",
    "Hattusa",
    "Mohenjo-Daro",
    "Yerevan",
    "Zanzibar",
    "Auckland",
    "Valletta",
    "Vilnius",
    "Stockholm",
    "Kandy",
];

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
        MapSize::from_dimensions(self.map.width, self.map.height).unwrap_or_else(|| {
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

fn one_attack() -> i32 {
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
    /// Chosen nodes from this unit class's promotion tree.
    #[serde(default)]
    pub promotions: BTreeSet<String>,
    /// 0 = unit, 1 = Corps/Fleet, 2 = Army/Armada.
    #[serde(default)]
    pub formation: u8,
    /// Symmetric escort/support link. Linked units occupy and move on one tile.
    #[serde(default)]
    pub linked_to: Option<u32>,
    /// Religious units retain the majority religion of their purchase city.
    #[serde(default)]
    pub religion: Option<String>,
    #[serde(default = "one_attack")]
    pub attacks_left: i32,
    #[serde(default)]
    pub moved: bool,
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
    /// Snapshot taken when this unit's turn begins. A unit may leave a ZOC
    /// as its first action, but attacking before leaving forfeits that move.
    #[serde(default)]
    pub started_turn_in_zoc: bool,
    /// Fighters on patrol intercept the first hostile air mission in range.
    #[serde(default)]
    pub air_patrol: bool,
    /// Permanent movement granted when the unit is trained (for example by
    /// the Royal Navy Dockyard).
    #[serde(default)]
    pub bonus_moves: f64,
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
    Unit {
        unit: String,
    },
    Building {
        building: String,
    },
    District {
        district: String,
        pos: Pos,
    },
    Wonder {
        wonder: String,
        pos: Pos,
    },
    /// Restores either a pillaged district base (`repair = "district"`) or
    /// one pillaged building on that district tile.
    Repair {
        repair: String,
        pos: Pos,
    },
    Project {
        project: String,
    },
}

/// District placements owned by a city.
///
/// Older saves encoded this as `{ "campus": [q, r] }`, which made it
/// impossible to represent Civ VI's repeatable Neighborhood and Canal
/// districts.  The in-memory representation stores every placement while the
/// custom deserializer accepts both that legacy shape and the new
/// `{ "neighborhood": [[q1, r1], [q2, r2]] }` shape.
#[derive(Clone, Default)]
pub struct Districts(BTreeMap<String, Vec<Pos>>);

impl Districts {
    pub fn contains_key(&self, name: &str) -> bool {
        self.0
            .get(name)
            .is_some_and(|positions| !positions.is_empty())
    }

    /// Total district instances, rather than the number of distinct types.
    pub fn len(&self) -> usize {
        self.0.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn get(&self, name: &str) -> Option<&Pos> {
        self.0.get(name).and_then(|positions| positions.first())
    }

    pub fn positions(&self, name: &str) -> &[Pos] {
        self.0.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn insert(&mut self, name: String, pos: Pos) {
        self.0.entry(name).or_default().push(pos);
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.0
            .iter()
            .flat_map(|(name, positions)| std::iter::repeat_n(name, positions.len()))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Pos)> {
        self.0
            .iter()
            .flat_map(|(name, positions)| positions.iter().map(move |pos| (name, pos)))
    }
}

impl<'a> IntoIterator for &'a Districts {
    type Item = (&'a String, &'a Pos);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter().collect::<Vec<_>>().into_iter()
    }
}

impl Serialize for Districts {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, positions) in &self.0 {
            if positions.len() == 1 {
                map.serialize_entry(name, &positions[0])?;
            } else {
                map.serialize_entry(name, positions)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Districts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(Pos),
            Many(Vec<Pos>),
        }

        let encoded = BTreeMap::<String, OneOrMany>::deserialize(deserializer)?;
        Ok(Self(
            encoded
                .into_iter()
                .map(|(name, positions)| {
                    let positions = match positions {
                        OneOrMany::One(pos) => vec![pos],
                        OneOrMany::Many(positions) => positions,
                    };
                    (name, positions)
                })
                .collect(),
        ))
    }
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
    /// Banked progress for paused builds. `production` is the active build's
    /// progress (or unassigned overflow when the queue is empty).
    #[serde(default)]
    pub production_progress: BTreeMap<String, f64>,
    pub border_culture: f64,
    pub hp: i32,
    pub buildings: Vec<String>,
    #[serde(default)]
    pub pillaged_buildings: BTreeSet<String>,
    #[serde(default)]
    pub districts: Districts,
    #[serde(default)]
    pub wonders: BTreeMap<String, Pos>,
    pub owned_tiles: Vec<Pos>,
    pub queue: Vec<Item>,
    pub original_owner: usize,
    pub is_capital: bool,
    #[serde(default)]
    pub struck: bool,
    /// Outer-defense pool from walls (Civ 6); -1 in old saves = derive on load.
    #[serde(default = "wall_unset")]
    pub wall_hp: i32,
    /// Encampments are independent defensible districts with their own 100 HP,
    /// wall pool, ranged strike, pillage state, and attack timer.
    #[serde(default)]
    pub encampment_hp: i32,
    #[serde(default = "wall_unset")]
    pub encampment_wall_hp: i32,
    #[serde(default)]
    pub encampment_struck: bool,
    #[serde(default)]
    pub encampment_last_attacked: u32,
    #[serde(default)]
    pub encampment_pillaged: bool,
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
    /// IDs of named Great People recruited from the global market.
    #[serde(default)]
    pub great_people: Vec<String>,
    #[serde(default)]
    pub pantheon: Option<String>,
    #[serde(default)]
    pub religion: Option<String>,
    #[serde(default)]
    pub holy_city: Option<u32>,
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
            great_people: Vec::new(),
            pantheon: None,
            religion: None,
            holy_city: None,
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
    Move {
        unit: u32,
        to: Pos,
    },
    MoveTo {
        unit: u32,
        to: Pos,
    },
    Attack {
        unit: u32,
        target: Pos,
    },
    Ranged {
        unit: u32,
        target: Pos,
    },
    FoundCity {
        unit: u32,
    },
    Improve {
        unit: u32,
        improvement: String,
    },
    Pillage {
        unit: u32,
    },
    RepairImprovement {
        unit: u32,
    },
    CoastalRaid {
        unit: u32,
        target: Pos,
    },
    AirRebase {
        unit: u32,
        to: Pos,
    },
    AirStrike {
        unit: u32,
        target: Pos,
    },
    AirPatrol {
        unit: u32,
    },
    Produce {
        city: u32,
        item: Item,
    },
    Buy {
        city: u32,
        unit: String,
        #[serde(default = "gold_s")]
        currency: String,
    },
    Research {
        tech: String,
    },
    Civic {
        civic: String,
    },
    DeclareWar {
        player: usize,
    },
    MakePeace {
        player: usize,
    },
    Fortify {
        unit: u32,
    },
    Promote {
        unit: u32,
        promotion: String,
    },
    CombineUnits {
        unit: u32,
        with: u32,
    },
    LinkUnits {
        unit: u32,
        with: u32,
    },
    UnlinkUnits {
        unit: u32,
    },
    Government {
        government: String,
    },
    SlotPolicy {
        policy: String,
    },
    UnslotPolicy {
        policy: String,
    },
    TradeRoute {
        unit: u32,
        city: u32,
    },
    SendEnvoy {
        player: usize,
    },
    RecruitGreatPerson {
        kind: String,
    },
    PatronizeGreatPerson {
        kind: String,
        #[serde(default = "gold_s")]
        currency: String,
    },
    ChoosePantheon {
        belief: String,
    },
    AssignGovernor {
        city: u32,
    },
    FoundReligion {
        follower: String,
        founder: String,
    },
    Spread {
        unit: u32,
    },
    TheologicalAttack {
        unit: u32,
        target: Pos,
    },
    CondemnHeretic {
        unit: u32,
        target_unit: u32,
    },
    HealReligious {
        unit: u32,
    },
    RemoveHeresy {
        unit: u32,
    },
    LaunchInquisition {
        unit: u32,
    },
    CityStrike {
        city: u32,
        target: Pos,
    },
    EncampmentStrike {
        city: u32,
        target: Pos,
    },
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
    /// Retired named Great People leave the global market permanently.
    pub retired_great_people: BTreeSet<String>,
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
    #[serde(default)]
    retired_great_people: BTreeSet<String>,
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
            retired_great_people: s.retired_great_people,
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
        let legacy: Vec<u32> = g
            .cities
            .values()
            .filter(|c| c.wall_hp < 0)
            .map(|c| c.id)
            .collect();
        for cid in legacy {
            let max = g.city_max_wall_hp(&g.cities[&cid]);
            g.cities.get_mut(&cid).unwrap().wall_hp = max;
        }
        let legacy_encampments: Vec<u32> = g
            .cities
            .values()
            .filter(|c| g.city_has_district_family(c, "encampment"))
            .map(|c| c.id)
            .collect();
        for cid in legacy_encampments {
            let max = g.city_max_wall_hp(&g.cities[&cid]);
            let city = g.cities.get_mut(&cid).unwrap();
            if city.encampment_hp <= 0 && !city.encampment_pillaged {
                city.encampment_hp = 100;
            }
            if city.encampment_wall_hp < 0 {
                city.encampment_wall_hp = max;
            }
        }
        let unit_ids: Vec<u32> = g.units.keys().copied().collect();
        for uid in unit_ids {
            let max_attacks = g.unit_max_attacks(uid);
            let unit = g.units.get_mut(&uid).unwrap();
            // Missing legacy fields already deserialize through `one_attack`.
            // Preserve a real mid-turn zero so save/load cannot restore an
            // attack that the unit has already spent.
            unit.attacks_left = unit.attacks_left.clamp(0, max_attacks);
            if unit.religion.is_none() && g.rules.units[unit.kind.as_str()].class == "religious" {
                unit.religion = g.players[unit.owner].religion.clone();
            }
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
            retired_great_people: g.retired_great_people,
            map: g.map,
            players: g.players,
            units: g.units.into_values().collect(),
            cities: g.cities.into_values().collect(),
        }
    }
}

impl Game {
    pub fn new(
        num_players: usize,
        width: i32,
        height: i32,
        seed: u64,
        max_turns: u32,
        num_city_states: usize,
    ) -> Game {
        Game::new_full(
            num_players,
            width,
            height,
            seed,
            max_turns,
            num_city_states,
            true,
        )
    }

    pub fn new_full(
        num_players: usize,
        width: i32,
        height: i32,
        seed: u64,
        max_turns: u32,
        num_city_states: usize,
        barbarians: bool,
    ) -> Game {
        let rules = Rules::embedded();
        let mut rng = Rng::new(seed);
        let map_size = MapSize::from_dimensions(width, height)
            .unwrap_or_else(|| MapSize::for_players(num_players));
        let (map, spawns) = mapgen::generate(
            &rules,
            width,
            height,
            num_players,
            num_city_states,
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
            retired_great_people: BTreeSet::new(),
            occ: BTreeMap::new(),
            city_by_pos: BTreeMap::new(),
            log: Vec::new(),
        };
        for i in 0..num_players {
            g.players
                .push(Player::new(i, CIV_NAMES[i % CIV_NAMES.len()], false));
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
            if t.owner_city.is_some()
                || t.improvement.is_some()
                || self.city_by_pos.contains_key(pos)
            {
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
        self.map.tiles.get_mut(&pos).unwrap().improvement = Some("barbarian_camp".to_string());
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
        let era = self
            .players
            .iter()
            .filter(|p| !p.is_minor)
            .map(|p| p.techs.len())
            .max()
            .unwrap_or(1);
        let pool: &[&str] = if era < 8 {
            &["warrior"]
        } else if era < 14 {
            &["warrior", "spearman", "archer"]
        } else if era < 22 {
            &["swordsman", "spearman", "archer"]
        } else {
            &["swordsman", "crossbowman", "pikeman"]
        };
        let camps: Vec<(Pos, u32)> = self.barb_camps.iter().map(|(p, n)| (*p, *n)).collect();
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
        let hut = self
            .map
            .get(pos)
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
        if self.barb_camps.contains_key(&pos)
            && Some(owner) != self.barb_pid
            && self.rules.units[kind.as_str()].class == "military"
        {
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
        self.units
            .values()
            .filter(|u| u.owner == pid)
            .map(|u| u.id)
            .collect()
    }

    pub fn player_city_ids(&self, pid: usize) -> Vec<u32> {
        self.cities
            .values()
            .filter(|c| c.owner == pid)
            .map(|c| c.id)
            .collect()
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
        if self.city_at(pos).is_some() || tile.and_then(|tile| tile.district.as_ref()).is_some() {
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
        let spec = &self.rules.units[unit.kind.as_str()];
        if spec.class == "religious" {
            let mut best: f64 = 0.0;
            for position in self.wdisk(unit.pos, 1) {
                let Some(tile) = self.map.get(position) else {
                    continue;
                };
                let Some(district) = tile.district.as_deref() else {
                    continue;
                };
                if !self.district_is_family(district, "holy_site") {
                    continue;
                }
                let Some(city) = tile.owner_city.and_then(|cid| self.cities.get(&cid)) else {
                    continue;
                };
                if city.owner != unit.owner {
                    continue;
                }
                let mut faith = self.district_yields(district, position).faith;
                faith += city
                    .buildings
                    .iter()
                    .filter_map(|name| {
                        let building = &self.rules.buildings[name.as_str()];
                        building
                            .district
                            .as_deref()
                            .is_some_and(|district| self.district_is_family(district, "holy_site"))
                            .then_some(building.yields.faith)
                    })
                    .sum::<f64>();
                best = best.max(3.0 * faith);
            }
            return best.round() as i32;
        }
        if spec.domain.as_deref() == Some("sea")
            && self
                .map
                .get(unit.pos)
                .and_then(|tile| tile.owner_city)
                .and_then(|city_id| self.cities.get(&city_id))
                .is_some_and(|city| {
                    city.owner == unit.owner
                        && self.city_district_effect(city, "naval_heal_full") > 0.0
                })
        {
            return 100;
        }
        let location = self.healing_location(unit.owner, unit.pos);
        let naval_or_embarked = spec.domain.as_deref() == Some("sea") || self.is_embarked(unit);
        if naval_or_embarked {
            let friendly = self
                .map
                .get(unit.pos)
                .and_then(|tile| tile.owner_city)
                .and_then(|cid| self.cities.get(&cid))
                .is_some_and(|city| {
                    city.owner == unit.owner || self.suzerain_of(city.owner) == Some(unit.owner)
                });
            if friendly || self.promotion_effect(unit, "heal_anywhere") > 0.0 {
                20
            } else {
                0
            }
        } else {
            location.rate()
        }
    }

    pub fn gov_effects(&self, pid: usize) -> crate::rules::GovEffects {
        match &self.players[pid].government {
            Some(g) => self
                .rules
                .governments
                .get(g)
                .map(|s| s.effects)
                .unwrap_or_default(),
            None => Default::default(),
        }
    }

    pub fn has_policy(&self, pid: usize, name: &str) -> bool {
        self.players[pid].policies.contains(name)
    }

    /// Sum a numeric primitive across all currently slotted policy cards.
    /// Policy rules remain data-driven while callers provide the game context
    /// (unit class, district family, city state, and so on).
    pub fn policy_effect(&self, pid: usize, effect: &str) -> f64 {
        self.players[pid]
            .policies
            .iter()
            .filter_map(|name| self.rules.policies.get(name)?.effects.get(effect))
            .sum()
    }

    /// Sum global abilities from every researched technology and civic.
    pub fn tree_effect(&self, pid: usize, effect: &str) -> f64 {
        let player = &self.players[pid];
        player
            .techs
            .iter()
            .filter_map(|node| self.rules.techs.get(node)?.effects.get(effect))
            .chain(
                player
                    .civics
                    .iter()
                    .filter_map(|node| self.rules.civics.get(node)?.effects.get(effect)),
            )
            .sum()
    }

    /// Leader/civ ability check (data in civs.json, effects keyed by name).
    pub fn has_ability(&self, pid: usize, ability: &str) -> bool {
        self.rules
            .civs
            .get(&self.players[pid].civ)
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

    /// Trading capacity: 1 with Foreign Trade, plus capacity granted by
    /// buildings/districts and +2 under Merchant Republic.
    pub fn trade_capacity(&self, pid: usize) -> i64 {
        let p = &self.players[pid];
        if !p.civics.contains("foreign_trade") {
            return 0;
        }
        let mut cap = 1i64;
        for city in self.cities.values().filter(|city| city.owner == pid) {
            // Market and Lighthouse are alternative ways to earn this city's
            // one infrastructure route; owning both never grants two.
            if self.city_has_building_family(city, "market")
                || self.city_has_building_family(city, "lighthouse")
            {
                cap += 1;
            }
            // The Owls of Minerva Gilded Vault is the explicit exception: a
            // Harbor in the same city grants one additional route.
            if city
                .buildings
                .iter()
                .any(|building| building == "gilded_vault")
                && self.city_has_district_family(city, "harbor")
            {
                cap += self.rules.buildings["gilded_vault"]
                    .effects
                    .get("harbor_trade_route_capacity")
                    .copied()
                    .unwrap_or(0.0) as i64;
            }
        }
        cap += self.empire_wonder_effect(pid, "trade_route_capacity") as i64;
        cap += p
            .counters
            .get("great_person_trade_capacity")
            .copied()
            .unwrap_or(0);
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
            match (self.district_family(d), domestic) {
                ("campus", true)
                | ("holy_site", true)
                | ("theater_square", true)
                | ("entertainment_complex", true) => ys.food += 1.0,
                ("encampment", _)
                | ("industrial_zone", _)
                | ("commercial_hub", true)
                | ("harbor", true) => ys.production += 1.0,
                ("commercial_hub", false) | ("harbor", false) => ys.gold += 3.0,
                ("campus", false) => ys.science += 1.0,
                ("holy_site", false) => ys.faith += 1.0,
                ("theater_square", false) => ys.culture += 1.0,
                ("entertainment_complex", false) => ys.food += 1.0,
                ("government_plaza", true) | ("diplomatic_quarter", true) => {
                    ys.food += 1.0;
                    ys.production += 1.0;
                }
                ("government_plaza", false) => ys.gold += 2.0,
                ("diplomatic_quarter", false) => ys.culture += 1.0,
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
        let origin = self
            .city_at(u.pos)
            .filter(|cid| self.cities[cid].owner == pid)
            .ok_or_else(|| "trader must be in one of your cities".to_string())?;
        let dc = self
            .cities
            .get(&dest)
            .ok_or_else(|| "no such city".to_string())?;
        if dest == origin {
            return Err("destination is the origin".into());
        }
        if self.is_at_war(pid, dc.owner) {
            return Err("cannot trade with an enemy".into());
        }
        if self.wdist(self.cities[&origin].pos, dc.pos) > 15 {
            return Err("destination out of range".into());
        }
        if self
            .routes
            .iter()
            .any(|r| r.origin == origin && r.dest == dest)
        {
            return Err("route already active".into());
        }
        if self.active_routes(pid) >= self.trade_capacity(pid) {
            return Err("no trading capacity".into());
        }
        self.build_road(self.cities[&origin].pos, self.cities[&dest].pos);
        let ends = self.turn + 30;
        self.routes.push(TradeRoute {
            origin,
            dest,
            owner: pid,
            ends,
        });
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
            let next = self
                .nbrs(cur)
                .into_iter()
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
        let expired: Vec<TradeRoute> = self
            .routes
            .iter()
            .filter(|r| r.owner == pid && turn >= r.ends)
            .cloned()
            .collect();
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
                || oowner.is_none()
                || downer.is_none())
        });
    }

    // -------------------------------------------------- religion

    const RELIGION_NAMES: [&'static str; 8] = [
        "Buddhism",
        "Christianity",
        "Confucianism",
        "Hinduism",
        "Islam",
        "Judaism",
        "Protestantism",
        "Shinto",
    ];

    pub fn religions_founded(&self) -> usize {
        self.players.iter().filter(|p| p.religion.is_some()).count()
    }

    pub fn has_pantheon_belief(&self, pid: usize, belief: &str) -> bool {
        self.players[pid].pantheon.as_deref() == Some(belief)
    }

    /// The religion a city predominantly follows (highest pressure, min 50).
    pub fn city_religion<'a>(&self, city: &'a City) -> Option<&'a str> {
        city.pressure
            .iter()
            .filter(|(_, v)| **v >= 50.0)
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap().then(b.0.cmp(a.0)))
            .map(|(r, _)| r.as_str())
    }

    fn religion_founder(&self, religion: &str) -> Option<usize> {
        self.players
            .iter()
            .find(|p| p.religion.as_deref() == Some(religion))
            .map(|p| p.id)
    }

    fn founder_has(&self, religion: &str, belief: &str) -> bool {
        self.religion_founder(religion)
            .map(|pid| {
                self.players[pid]
                    .religion_beliefs
                    .iter()
                    .any(|b| b == belief)
            })
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
        if self
            .players
            .iter()
            .any(|p| p.pantheon.as_deref() == Some(belief))
        {
            return Err("belief already taken".into());
        }
        self.players[pid].pantheon = Some(belief.to_string());
        self.players[pid].era_score += 1;
        Ok(())
    }

    fn do_found_religion(
        &mut self,
        pid: usize,
        follower: &str,
        founder: &str,
    ) -> Result<(), String> {
        if !self.players[pid].prophet_pending {
            return Err("no great prophet available".into());
        }
        if self.religions_founded() >= self.max_religions() {
            return Err("this map has reached its religion limit".into());
        }
        if !self.rules.beliefs.follower.contains_key(follower)
            || !self.rules.beliefs.founder.contains_key(founder)
        {
            return Err("no such belief".into());
        }
        let taken = |b: &str| {
            self.players
                .iter()
                .any(|p| p.religion_beliefs.iter().any(|x| x == b))
        };
        if taken(follower) || taken(founder) {
            return Err("belief already taken".into());
        }
        let holy = self
            .cities
            .values()
            .find(|c| c.owner == pid && self.city_has_district_family(c, "holy_site"))
            .map(|c| c.id)
            .ok_or_else(|| "needs a city with a holy site".to_string())?;
        let name = Self::RELIGION_NAMES[self.religions_founded() % 8].to_string();
        let p = &mut self.players[pid];
        p.prophet_pending = false;
        p.religion = Some(name.clone());
        p.holy_city = Some(holy);
        p.era_score += 3;
        p.religion_beliefs = vec![follower.to_string(), founder.to_string()];
        let holy_site_cities: Vec<u32> = self
            .cities
            .values()
            .filter(|city| city.owner == pid && self.city_has_district_family(city, "holy_site"))
            .map(|city| city.id)
            .collect();
        for cid in holy_site_cities {
            self.cities
                .get_mut(&cid)
                .unwrap()
                .pressure
                .insert(name.clone(), 1000.0);
        }
        Ok(())
    }

    fn do_spread(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        let spread = self.rules.units[u.kind.as_str()].religious_spread;
        if spread <= 0.0 || u.charges <= 0 || u.moves_left <= 0.0 {
            return Err("religious unit has no spread charges".into());
        }
        let kind = u.kind.clone();
        let religion = u
            .religion
            .clone()
            .ok_or_else(|| "unit has no religion to spread".to_string())?;
        let cid = self
            .city_at(u.pos)
            .or_else(|| self.nbrs(u.pos).into_iter().find_map(|n| self.city_at(n)))
            .ok_or_else(|| "no city in range".to_string())?;
        let city = self.cities.get_mut(&cid).unwrap();
        let eviction = match kind.as_str() {
            "apostle" => 0.25,
            "missionary" => 0.10,
            _ => 0.0,
        };
        for (faith, pressure) in city.pressure.iter_mut() {
            if *faith != religion {
                *pressure *= 1.0 - eviction;
            }
        }
        *city.pressure.entry(religion).or_insert(0.0) += spread * u.hp.clamp(0, 100) as f64 / 100.0;
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

    fn religious_combat_pressure(
        &mut self,
        winner: Option<&str>,
        loser: &str,
        pos: Pos,
        radius: i32,
        amount: f64,
    ) {
        let targets: Vec<u32> = self
            .cities
            .values()
            .filter(|city| self.wdist(city.pos, pos) <= radius)
            .map(|city| city.id)
            .collect();
        for cid in targets {
            let city = self.cities.get_mut(&cid).unwrap();
            let losing = city.pressure.entry(loser.to_string()).or_insert(0.0);
            *losing = (*losing - amount).max(0.0);
            if let Some(religion) = winner {
                *city.pressure.entry(religion.to_string()).or_insert(0.0) += amount;
            }
        }
    }

    fn theological_strength(&self, unit: &Unit) -> f64 {
        let mut strength = self.rules.units[unit.kind.as_str()].religious_strength;
        strength += self.gov_effects(unit.owner).religious_strength;
        strength += self.policy_effect(unit.owner, "religious_strength");
        let Some(religion) = unit.religion.as_deref() else {
            return strength;
        };
        if self
            .map
            .get(unit.pos)
            .and_then(|tile| tile.owner_city)
            .and_then(|cid| self.cities.get(&cid))
            .is_some_and(|city| self.city_religion(city) == Some(religion))
        {
            strength += 5.0;
        }
        if self.wdisk(unit.pos, 1).into_iter().any(|position| {
            let Some(tile) = self.map.get(position) else {
                return false;
            };
            tile.district
                .as_deref()
                .is_some_and(|district| self.district_is_family(district, "holy_site"))
                && tile
                    .owner_city
                    .and_then(|cid| self.cities.get(&cid))
                    .is_some_and(|city| self.city_religion(city) == Some(religion))
        }) {
            strength += 5.0;
        }
        if self
            .religion_founder(religion)
            .and_then(|founder| self.players[founder].holy_city)
            .and_then(|cid| self.cities.get(&cid))
            .is_some_and(|city| city.owned_tiles.contains(&unit.pos))
        {
            strength += 15.0;
        }
        strength
            + 2.0
                * self
                    .nbrs(unit.pos)
                    .into_iter()
                    .flat_map(|pos| self.units_at(pos))
                    .filter(|id| self.units[id].religion.as_deref() == Some(religion))
                    .count() as f64
    }

    fn do_theological_attack(&mut self, pid: usize, uid: u32, target: Pos) -> Result<(), String> {
        let attacker = self.own_unit(pid, uid)?;
        if !matches!(attacker.kind.as_str(), "apostle" | "inquisitor") || attacker.moves_left <= 0.0
        {
            return Err("unit cannot initiate theological combat".into());
        }
        if self.wdist(attacker.pos, target) != 1 {
            return Err("target not adjacent".into());
        }
        let attacker_religion = attacker
            .religion
            .clone()
            .ok_or_else(|| "attacker has no religion".to_string())?;
        let defender_id = self
            .units_at(target)
            .into_iter()
            .find(|other_id| {
                let defender = &self.units[other_id];
                let spec = &self.rules.units[defender.kind.as_str()];
                defender.owner != pid
                    && spec.class == "religious"
                    && defender
                        .religion
                        .as_deref()
                        .is_some_and(|r| r != attacker_religion)
            })
            .ok_or_else(|| "no opposing religious unit".to_string())?;
        let defender = self.units[&defender_id].clone();
        let defender_religion = defender.religion.clone().unwrap();
        let att = effective_strength(self.theological_strength(&attacker), attacker.hp);
        let def = effective_strength(self.theological_strength(&defender), defender.hp);
        let dealt = damage(att, def, &mut self.rng);
        let received = damage(def, att, &mut self.rng);
        self.units.get_mut(&defender_id).unwrap().hp -= dealt;
        self.units.get_mut(&uid).unwrap().hp -= received;
        {
            let unit = self.units.get_mut(&uid).unwrap();
            unit.moves_left = 0.0;
            unit.attacks_left = 0;
            unit.acted = true;
        }
        let attacker_dead = self.units[&uid].hp <= 0;
        let defender_dead = self.units[&defender_id].hp <= 0;
        if defender_dead {
            self.remove_unit(defender_id);
            self.religious_combat_pressure(
                Some(&attacker_religion),
                &defender_religion,
                target,
                10,
                250.0,
            );
        }
        if attacker_dead {
            self.remove_unit(uid);
            self.religious_combat_pressure(
                Some(&defender_religion),
                &attacker_religion,
                attacker.pos,
                10,
                250.0,
            );
        }
        self.check_religious_victory();
        Ok(())
    }

    fn do_condemn_heretic(&mut self, pid: usize, uid: u32, target_id: u32) -> Result<(), String> {
        let unit = self.own_unit(pid, uid)?;
        let target = self
            .units
            .get(&target_id)
            .cloned()
            .ok_or_else(|| "no such religious unit".to_string())?;
        if self.rules.units[unit.kind.as_str()].class != "military"
            || self.rules.units[target.kind.as_str()].class != "religious"
            || !self.is_at_war(pid, target.owner)
            || unit.pos != target.pos
            || unit.moves_left <= 0.0
        {
            return Err("cannot condemn that unit".into());
        }
        let religion = target
            .religion
            .clone()
            .ok_or_else(|| "target has no religion".to_string())?;
        self.remove_unit(target_id);
        self.religious_combat_pressure(None, &religion, target.pos, 6, 125.0);
        let unit = self.units.get_mut(&uid).unwrap();
        unit.moves_left = 0.0;
        unit.acted = true;
        self.check_religious_victory();
        Ok(())
    }

    fn do_heal_religious(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let guru = self.own_unit(pid, uid)?;
        if guru.kind != "guru" || guru.charges <= 0 || guru.moves_left <= 0.0 {
            return Err("not a Guru with a healing charge".into());
        }
        let religion = guru.religion.clone();
        let targets: Vec<u32> = self
            .player_unit_ids(pid)
            .into_iter()
            .filter(|other_id| self.wdist(guru.pos, self.units[other_id].pos) <= 1)
            .filter(|other_id| {
                self.rules.units[self.units[other_id].kind.as_str()].class == "religious"
                    && self.units[other_id].religion == religion
                    && self.units[other_id].hp < 100
            })
            .collect();
        if targets.is_empty() {
            return Err("no damaged adjacent religious units".into());
        }
        for target in targets {
            self.units.get_mut(&target).unwrap().hp = (self.units[&target].hp + 40).min(100);
        }
        let guru = self.units.get_mut(&uid).unwrap();
        guru.charges -= 1;
        guru.moves_left = 0.0;
        guru.acted = true;
        if guru.charges <= 0 {
            self.remove_unit(uid);
        }
        Ok(())
    }

    fn do_remove_heresy(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let inquisitor = self.own_unit(pid, uid)?;
        if inquisitor.kind != "inquisitor"
            || inquisitor.charges <= 0
            || inquisitor.moves_left <= 0.0
        {
            return Err("not an Inquisitor with charges".into());
        }
        let religion = inquisitor
            .religion
            .clone()
            .ok_or_else(|| "Inquisitor has no religion".to_string())?;
        let cid = self
            .city_at(inquisitor.pos)
            .filter(|cid| self.cities[cid].owner == pid)
            .ok_or_else(|| "Remove Heresy requires a friendly City Center".to_string())?;
        for (name, pressure) in self.cities.get_mut(&cid).unwrap().pressure.iter_mut() {
            if *name != religion {
                *pressure *= 0.25;
            }
        }
        let unit = self.units.get_mut(&uid).unwrap();
        unit.charges -= 1;
        unit.moves_left = 0.0;
        unit.acted = true;
        if unit.charges <= 0 {
            self.remove_unit(uid);
        }
        self.check_religious_victory();
        Ok(())
    }

    fn do_launch_inquisition(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let apostle = self.own_unit(pid, uid)?;
        if apostle.kind != "apostle"
            || apostle.moves_left <= 0.0
            || apostle.religion != self.players[pid].religion
            || self.players[pid]
                .counters
                .get("inquisition")
                .copied()
                .unwrap_or(0)
                > 0
        {
            return Err("cannot launch an inquisition".into());
        }
        let holy = self.players[pid]
            .holy_city
            .ok_or_else(|| "religion has no holy city".to_string())?;
        let holy_pos = self
            .cities
            .get(&holy)
            .map(|city| city.pos)
            .ok_or_else(|| "holy city is missing".to_string())?;
        if self.wdist(apostle.pos, holy_pos) > 1 {
            return Err("Apostle must be at its Holy Site".into());
        }
        self.remove_unit(uid);
        bump(&mut self.players[pid], "inquisition");
        Ok(())
    }

    /// Passive spread: each city following a religion exerts +1 pressure/turn
    /// on cities within 9 tiles (+2 from the founder's holy city).
    fn process_pressure(&mut self, pid: usize) {
        let sources: Vec<(Pos, String, f64)> = self
            .cities
            .values()
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
                    *self
                        .cities
                        .get_mut(&cid)
                        .unwrap()
                        .pressure
                        .entry(r.clone())
                        .or_insert(0.0) += amt;
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
                let cities: Vec<&City> = self.cities.values().filter(|c| c.owner == o.id).collect();
                if cities.is_empty() {
                    all = false; // a civ without cities cannot be converted
                    break;
                }
                let following = cities
                    .iter()
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

    fn gp_district(&self, district: &str) -> Option<&'static str> {
        // The Thành deliberately provides no Great General points despite
        // replacing the Encampment.
        if district == "thanh" {
            return None;
        }
        match self.district_family(district) {
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

    pub fn current_great_person(
        &self,
        kind: &str,
    ) -> Option<(&str, &crate::rules::GreatPersonSpec)> {
        self.rules
            .great_people
            .iter()
            .filter(|(id, spec)| {
                spec.kind == kind
                    && spec.era <= self.world_era + 1
                    && !self.retired_great_people.contains(*id)
            })
            .min_by_key(|(id, spec)| (spec.era, *id))
            .map(|(id, spec)| (id.as_str(), spec))
    }

    /// Point cost of the named person currently offered in this global
    /// market. The legacy fallback keeps old/modded saves playable when a
    /// ruleset has no named entry for a point type.
    pub fn gp_cost(&self, pid: usize, kind: &str) -> f64 {
        if let Some((_, person)) = self.current_great_person(kind) {
            return person.cost;
        }
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
            for (d, position) in &c.districts {
                if self.map.tiles[position].pillaged {
                    continue;
                }
                if let Some(t) = self.gp_district(d) {
                    *earn.entry(t.to_string()).or_insert(0.0) += 1.0;
                }
                if d == "lavra" {
                    for kind in ["writer", "artist", "musician"] {
                        *earn.entry(kind.to_string()).or_insert(0.0) += 1.0;
                    }
                }
            }
            for b in &c.buildings {
                if c.pillaged_buildings.contains(b) {
                    continue;
                }
                for (kind, points) in &self.rules.buildings[b.as_str()].great_person_points {
                    *earn.entry(kind.clone()).or_insert(0.0) += *points;
                }
            }
            for wonder in c.wonders.keys() {
                for (kind, points) in &self.rules.wonders[wonder.as_str()].great_person_points {
                    *earn.entry(kind.clone()).or_insert(0.0) += *points;
                }
            }
        }
        for kind in [
            "general",
            "admiral",
            "scientist",
            "prophet",
            "writer",
            "artist",
            "engineer",
            "merchant",
            "musician",
        ] {
            let amount = self.policy_effect(pid, &format!("gpp_{kind}"));
            if amount != 0.0 {
                *earn.entry(kind.to_string()).or_insert(0.0) += amount;
            }
        }
        if self.has_pantheon_belief(pid, "divine_spark") {
            for c in self.cities.values().filter(|c| c.owner == pid) {
                for d in ["campus", "holy_site", "theater_square"] {
                    if self.city_has_district_family(c, d) {
                        let t = self.gp_district(d).unwrap();
                        *earn.entry(t.to_string()).or_insert(0.0) += 1.0;
                    }
                }
            }
        }
        let mult = 1.0 + self.gov_effects(pid).great_people_pct / 100.0;
        for (t, amt) in earn {
            *self.players[pid].gpp.entry(t).or_insert(0.0) += amt * mult;
        }
        let due: Vec<String> = self.players[pid]
            .gpp
            .iter()
            .filter(|(t, pts)| **pts >= self.gp_cost(pid, t))
            .map(|(t, _)| t.clone())
            .collect();
        for t in due {
            let _ = self.claim_great_person(pid, &t, None);
        }
    }

    fn claim_great_person(
        &mut self,
        pid: usize,
        kind: &str,
        patronage: Option<&str>,
    ) -> Result<(), String> {
        let (id, spec) = self
            .current_great_person(kind)
            .map(|(id, spec)| (id.to_string(), spec.clone()))
            .ok_or_else(|| "no Great Person of that type is currently available".to_string())?;
        let points = self.players[pid].gpp.get(kind).copied().unwrap_or(0.0);
        let missing = (spec.cost - points).max(0.0);
        match patronage {
            None if missing > 0.0 => return Err("not enough Great Person points".into()),
            Some("gold") => {
                let price = missing * 15.0;
                if missing <= 0.0 || self.players[pid].gold < price {
                    return Err("cannot patronize with Gold".into());
                }
                self.players[pid].gold -= price;
            }
            Some("faith") => {
                let discount =
                    self.empire_wonder_effect(pid, "great_person_faith_patronage_discount_pct");
                let price = missing * 10.0 * (1.0 - discount / 100.0);
                if missing <= 0.0 || self.players[pid].faith < price {
                    return Err("cannot patronize with Faith".into());
                }
                self.players[pid].faith -= price;
            }
            Some(_) => return Err("patronage currency must be gold or faith".into()),
            None => {}
        }
        self.players[pid].gpp.insert(kind.to_string(), 0.0);
        self.retired_great_people.insert(id.clone());
        let player = &mut self.players[pid];
        player.great_people.push(id);
        *player.gp_claimed.entry(kind.to_string()).or_insert(0) += 1;
        player.era_score += 2;
        bump(player, "great_people");
        self.named_great_person_effect(pid, &spec);
        Ok(())
    }

    fn named_great_person_effect(&mut self, pid: usize, spec: &crate::rules::GreatPersonSpec) {
        if let Some(amount) = spec.effects.get("gold") {
            self.players[pid].gold += *amount;
        }
        if let Some(amount) = spec.effects.get("envoys") {
            self.players[pid].envoys_free += *amount as i64;
        }
        if let Some(amount) = spec.effects.get("trade_capacity") {
            *self.players[pid]
                .counters
                .entry("great_person_trade_capacity".to_string())
                .or_insert(0) += *amount as i64;
        }
        if let Some(amount) = spec.effects.get("tech_boosts") {
            self.grant_random_boosts(pid, *amount as usize, true);
        }
        if let Some(amount) = spec.effects.get("city_production") {
            if let Some(cid) = self
                .player_city_ids(pid)
                .into_iter()
                .max_by_key(|cid| self.cities[cid].pop)
            {
                self.cities.get_mut(&cid).unwrap().production += *amount;
            }
        }
        for (effect, counter) in [
            ("great_work_writing", "great_work:writing"),
            ("great_work_art", "great_work:art"),
            ("great_work_music", "great_work:music"),
        ] {
            if let Some(amount) = spec.effects.get(effect) {
                *self.players[pid]
                    .counters
                    .entry(counter.to_string())
                    .or_insert(0) += *amount as i64;
            }
        }
        // The existing class effects cover Prophets and military people and
        // remain the fallback primitive for modded named entries.
        if spec.effects.contains_key("found_religion")
            || spec.effects.contains_key("military_promotion")
            || spec.effects.contains_key("naval_promotion")
        {
            self.great_person_effect(pid, &spec.kind);
        }
    }

    fn do_recruit_great_person(&mut self, pid: usize, kind: &str) -> Result<(), String> {
        self.claim_great_person(pid, kind, None)
    }

    fn do_patronize_great_person(
        &mut self,
        pid: usize,
        kind: &str,
        currency: &str,
    ) -> Result<(), String> {
        self.claim_great_person(pid, kind, Some(currency))
    }

    /// Simplified instant retirement effects for a claimed great person.
    fn great_person_effect(&mut self, pid: usize, kind: &str) {
        match kind {
            "scientist" => self.grant_random_boosts(pid, 2, true),
            "artist" => self.grant_random_boosts(pid, 2, false),
            "engineer" => {
                let best = self
                    .cities
                    .values()
                    .filter(|c| c.owner == pid)
                    .max_by(|a, b| {
                        a.production
                            .partial_cmp(&b.production)
                            .unwrap()
                            .then(a.id.cmp(&b.id))
                    })
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
                    && self
                        .cities
                        .values()
                        .any(|c| c.owner == pid && self.city_has_district_family(c, "holy_site"));
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
                    if spec.class == "military" && (spec.domain.as_deref() == Some("sea")) == sea {
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
                self.rules
                    .techs
                    .iter()
                    .filter(|(name, _)| {
                        let p = &self.players[pid];
                        !p.techs.contains(*name) && !p.boosted_techs.contains(*name)
                    })
                    .map(|(name, s)| (name.clone(), s.cost))
                    .collect()
            } else {
                self.rules
                    .civics
                    .iter()
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
        self.players[pid]
            .envoys
            .iter()
            .find(|(m, _)| *m == minor)
            .map(|(_, n)| *n)
            .unwrap_or(0)
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
        let ok = self
            .players
            .get(minor)
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
        for m in self
            .players
            .iter()
            .filter(|m| m.is_minor && !m.is_barbarian && m.alive)
        {
            let n = self.envoys_at(pid, m.id);
            if n == 0 {
                continue;
            }
            let (kind, district) = Self::cs_bonus(Self::cs_type(&m.civ));
            let mut amt = 0.0;
            if n >= 1 && self.city_has_palace(city) {
                amt += 2.0;
            }
            if self.city_has_district_family(city, district) {
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
        if spec.promotion_class == "recon" {
            multiplier += self.policy_effect(owner, "recon_xp_pct") / 100.0;
        }
        multiplier += self.policy_effect(owner, "unit_xp_pct") / 100.0;
        if spec.promotion_class == "ranged" && self.has_ability(owner, "ta_seti") {
            multiplier += 0.5;
        }
        (amt * multiplier).round() as i64
    }

    fn promotion_threshold(level: i32) -> i64 {
        (15 * level as i64 * (level as i64 + 1)) / 2
    }

    pub fn promotion_pending(&self, uid: u32) -> bool {
        self.units.get(&uid).is_some_and(|unit| {
            let class = &self.rules.units[unit.kind.as_str()].promotion_class;
            !class.is_empty() && unit.level < 8 && unit.xp >= Self::promotion_threshold(unit.level)
        })
    }

    fn promotion_effect(&self, unit: &Unit, effect: &str) -> f64 {
        unit.promotions
            .iter()
            .filter_map(|name| {
                self.rules
                    .promotions
                    .get(name)?
                    .effects
                    .get(effect)
                    .copied()
            })
            .sum()
    }

    pub fn available_promotions(&self, uid: u32) -> Vec<String> {
        let Some(unit) = self.units.get(&uid) else {
            return vec![];
        };
        if unit.moves_left <= 0.0 || !self.promotion_pending(uid) {
            return vec![];
        }
        let class = self.rules.units[unit.kind.as_str()]
            .promotion_class
            .as_str();
        self.rules
            .promotions
            .iter()
            .filter(|(name, spec)| {
                spec.class == class
                    && !unit.promotions.contains(*name)
                    && (spec.requires.is_empty()
                        || spec
                            .requires
                            .iter()
                            .any(|req| unit.promotions.contains(req)))
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn unit_formation_bonus(&self, unit: &Unit) -> f64 {
        match unit.formation {
            1 => 10.0,
            2.. => 17.0,
            _ => 0.0,
        }
    }

    fn unit_max_attacks(&self, uid: u32) -> i32 {
        self.units
            .get(&uid)
            .map(|unit| 1 + self.promotion_effect(unit, "extra_attacks") as i32)
            .unwrap_or(1)
    }

    fn award_xp(&mut self, uid: u32, amt: f64) {
        if self.promotion_pending(uid) {
            return; // XP gain pauses until the pending promotion is selected.
        }
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
        if self.promotion_pending(uid) {
            return;
        }
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
        let amount = relative + if ranged { 1.0 } else { 2.0 } + if attacking { 1.0 } else { 0.0 };
        let gained = self.modified_xp(uid, amount).min(8);
        self.units.get_mut(&uid).unwrap().xp += gained;
    }

    /// Discipline: +5 combat strength when fighting barbarians.
    fn vs_bonus(&self, owner: usize, opponent: usize) -> f64 {
        if self.players[opponent].is_barbarian {
            self.policy_effect(owner, "barbarian_combat")
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
                if unit == "builder" {
                    bonus += self.policy_effect(pid, "builder_production_pct") / 100.0;
                }
                if unit == "settler" {
                    bonus += self.policy_effect(pid, "settler_production_pct") / 100.0;
                }
                if spec.domain.as_deref() == Some("sea") {
                    bonus += self.policy_effect(pid, "naval_production_pct") / 100.0;
                }
                if spec.domain.as_deref() == Some("sea") || unit == "settler" {
                    bonus += self
                        .city_district_effect(&self.cities[&cid], "naval_settler_production_pct")
                        / 100.0;
                }
                if spec.domain.as_deref() == Some("air") {
                    bonus += self.policy_effect(pid, "air_production_pct") / 100.0;
                }
                if spec.cavalry {
                    bonus += self.policy_effect(pid, "cavalry_production_pct") / 100.0;
                }
                if spec.class == "support" {
                    bonus += self.policy_effect(pid, "support_production_pct") / 100.0;
                }
                if matches!(
                    spec.promotion_class.as_str(),
                    "melee" | "anti_cavalry" | "ranged"
                ) {
                    bonus += self.policy_effect(pid, "infantry_production_pct") / 100.0;
                }
                if unit == "giant_death_robot" {
                    bonus += self.policy_effect(pid, "gdr_production_pct") / 100.0;
                }
                if unit == "aircraft_carrier" {
                    bonus += self.policy_effect(pid, "carrier_production_pct") / 100.0;
                }
                if spec.ranged_strength > 0.0
                    && spec.class == "military"
                    && self.has_ability(pid, "ta_seti")
                {
                    bonus += 0.5; // Nubia: Ta-Seti
                } else if spec.class == "military" && !spec.siege {
                    if self.has_pantheon_belief(pid, "god_of_the_forge") {
                        bonus += 0.25;
                    }
                }
            }
            Some(Item::Building { building }) => {
                if self.rules.buildings[building.as_str()].outer_defense > 0 {
                    bonus += self.policy_effect(pid, "wall_production_pct") / 100.0;
                }
                if self.rules.buildings[building.as_str()].wonder
                    && self.has_ability(pid, "iteru")
                    && self.map.tiles[&self.cities[&cid].pos].has_river()
                {
                    bonus += 0.15; // Egypt: Iteru (river cities)
                }
            }
            Some(Item::Wonder { pos, .. }) => {
                bonus += self.policy_effect(pid, "wonder_production_pct") / 100.0;
                if self.has_ability(pid, "iteru") && self.map.tiles[pos].has_river() {
                    bonus += 0.15; // Egypt: Iteru (river cities)
                }
            }
            Some(Item::Project { .. }) => {
                bonus += self.gov_effects(pid).project_production_pct / 100.0;
                bonus += self.policy_effect(pid, "space_project_production_pct") / 100.0;
                let completions = self.players[pid]
                    .counters
                    .get("tree_completions:future_tech")
                    .copied()
                    .unwrap_or(0) as f64;
                bonus += completions
                    * self.rules.techs["future_tech"]
                        .effects
                        .get("project_production_pct_per_completion")
                        .copied()
                        .unwrap_or(0.0)
                    / 100.0;
            }
            _ => {}
        }
        1.0 + bonus
    }

    pub fn gov_slots(&self, pid: usize) -> crate::rules::PolicySlots {
        let mut slots = match &self.players[pid].government {
            Some(g) => self
                .rules
                .governments
                .get(g)
                .map(|s| s.slots)
                .unwrap_or_default(),
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
        let overflow = (m - slots.military).max(0)
            + (e - slots.economic).max(0)
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
        let obsolete: BTreeSet<&str> = self
            .rules
            .policies
            .values()
            .filter(|s| {
                s.civic
                    .as_ref()
                    .map(|c| p.civics.contains(c))
                    .unwrap_or(true)
            })
            .filter_map(|s| s.replaces.as_deref())
            .collect();
        self.rules
            .policies
            .iter()
            .filter(|(name, s)| {
                !p.policies.contains(*name)
                    && !obsolete.contains(name.as_str())
                    && s.civic
                        .as_ref()
                        .map(|c| p.civics.contains(c))
                        .unwrap_or(true)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub fn is_embarked(&self, u: &Unit) -> bool {
        self.rules.units[u.kind.as_str()].domain.as_deref() != Some("sea")
            && self
                .map
                .get(u.pos)
                .map(|t| self.rules.is_water(t))
                .unwrap_or(false)
    }

    /// Whether this unit type has learned how to embark onto Coast tiles.
    /// Sailing unlocks Builders, Celestial Navigation unlocks Traders, and
    /// Shipbuilding unlocks every remaining land unit (Gathering Storm).
    fn has_embarkation(&self, owner: usize, kind: &str) -> bool {
        let techs = &self.players[owner].techs;
        match kind {
            "builder" => techs.contains("sailing"),
            "trader" => techs.contains("celestial_navigation"),
            _ => techs.contains("shipbuilding"),
        }
    }

    /// Static terrain/domain portion of unit movement. Occupancy and
    /// diplomacy are checked separately by `can_enter`.
    pub fn unit_can_traverse(&self, uid: u32, pos: Pos) -> bool {
        let Some(unit) = self.units.get(&uid) else {
            return false;
        };
        let Some(tile) = self.map.get(pos) else {
            return false;
        };
        if !self.rules.is_passable(tile) {
            return false;
        }
        let spec = &self.rules.units[unit.kind.as_str()];
        if spec.domain.as_deref() == Some("air") {
            return false; // aircraft use rebase/strike missions, never tile steps
        }
        let water = self.rules.is_water(tile);
        if water
            && tile.terrain == "ocean"
            && !self.players[unit.owner].techs.contains("cartography")
        {
            return false;
        }
        if spec.domain.as_deref() == Some("sea") {
            // Naval units may use water, City Centers, and Canals. This also
            // lets naval melee units attack and capture coastal cities.
            water || self.city_at(pos).is_some() || tile.district.as_deref() == Some("canal")
        } else {
            !water || self.has_embarkation(unit.owner, &unit.kind)
        }
    }

    fn unit_can_melee_target_domain(&self, uid: u32, target: Pos) -> bool {
        let Some(unit) = self.units.get(&uid) else {
            return false;
        };
        let Some(tile) = self.map.get(target) else {
            return false;
        };
        let sea_unit = self.rules.units[unit.kind.as_str()].domain.as_deref() == Some("sea");
        if sea_unit {
            self.unit_can_traverse(uid, target)
        } else {
            // Land units may disembark into a melee attack, but may never
            // initiate one into a water tile.
            !self.rules.is_water(tile)
        }
    }

    fn oligarchy_applies(spec: &crate::rules::UnitSpec) -> bool {
        matches!(
            spec.promotion_class.as_str(),
            "melee" | "anti_cavalry" | "naval_melee"
        )
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
            + self.government_combat_bonus(u)
            + self.unit_formation_bonus(u)
            + self.promotion_effect(u, "combat_all");
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
            s += self.promotion_effect(u, "defend_all");
            let tile = &self.map.tiles[&u.pos];
            if self.city_at(u.pos).is_some() || tile.district.is_some() {
                s += self.promotion_effect(u, "district_defense");
            }
            if tile.hills || matches!(tile.feature.as_deref(), Some("forest" | "jungle" | "marsh"))
            {
                s += self.promotion_effect(u, "rough_defense");
            }
            if u.linked_to.is_some() {
                s += self.promotion_effect(u, "formation_combat");
            }
        }
        s
    }

    fn ranged_defense_bonus(&self, unit: &Unit, city_attack: bool) -> f64 {
        self.promotion_effect(unit, "defend_ranged")
            + if city_attack {
                self.promotion_effect(unit, "defend_city_attack")
            } else {
                0.0
            }
    }

    fn unit_can_fortify(&self, u: &Unit) -> bool {
        let spec = &self.rules.units[u.kind.as_str()];
        spec.class == "military" && spec.domain.as_deref() != Some("sea") && !self.is_embarked(u)
    }

    pub fn unit_ranged_strength(&self, u: &Unit) -> f64 {
        let rs = self.rules.units[u.kind.as_str()].ranged_strength;
        if rs <= 0.0 {
            return 0.0;
        }
        rs + self.government_combat_bonus(u)
            + self.unit_formation_bonus(u)
            + if self.has_ability(u.owner, "gifts_for_the_tlatoani") {
                self.empire_luxuries(u.owner) as f64
            } else {
                0.0
            }
    }

    pub fn unit_bombard_strength(&self, u: &Unit) -> f64 {
        let bs = self.rules.units[u.kind.as_str()].bombard_strength;
        if bs <= 0.0 {
            return 0.0;
        }
        bs + self.government_combat_bonus(u)
            + self.unit_formation_bonus(u)
            + if self.has_ability(u.owner, "gifts_for_the_tlatoani") {
                self.empire_luxuries(u.owner) as f64
            } else {
                0.0
            }
    }

    pub fn unit_ranged_attack_strength(&self, u: &Unit) -> f64 {
        self.unit_ranged_strength(u)
            .max(self.unit_bombard_strength(u))
    }

    pub fn city_housing(&self, city: &City) -> f64 {
        // fresh water (river/oasis) = 5, coastal = 3, otherwise 2 (Civ 6)
        let center = &self.map.tiles[&city.pos];
        let fresh = center.has_river()
            || self.nbrs(city.pos).iter().any(|n| {
                self.map
                    .get(*n)
                    .is_some_and(|t| t.terrain == "lake" || t.feature.as_deref() == Some("oasis"))
            });
        let coastal = self.nbrs(city.pos).iter().any(|n| {
            self.map
                .get(*n)
                .map(|t| matches!(t.terrain.as_str(), "coast" | "ocean"))
                .unwrap_or(false)
        });
        let has_aqueduct = self.city_has_district_family(city, "aqueduct");
        let mut h = if has_aqueduct {
            if fresh {
                7.0
            } else {
                6.0
            }
        } else if fresh {
            5.0
        } else if coastal {
            3.0
        } else {
            2.0
        };
        if self.city_has_palace(city) {
            h += self.rules.buildings["palace"].housing;
        }
        for pos in city
            .owned_tiles
            .iter()
            .filter(|p| self.wdist(city.pos, **p) <= 3)
        {
            if let Some(improvement) = self.map.tiles[pos].improvement.as_deref() {
                h += self
                    .rules
                    .improvements
                    .get(improvement)
                    .map(|spec| spec.housing)
                    .unwrap_or(0.0);
            }
        }
        for b in &city.buildings {
            h += self.rules.buildings[b.as_str()].housing;
            if b == "lighthouse" && coastal {
                h += 1.0;
            }
        }
        for (district, position) in &city.districts {
            h += self.district_housing(district, *position);
        }
        for wonder in city.wonders.keys() {
            h += self.rules.wonders[wonder.as_str()].housing;
        }
        if city.districts.len() >= 2 {
            h += self.policy_effect(city.owner, "housing_at_2_districts");
        }
        if city.districts.len() >= 3 {
            h += self.policy_effect(city.owner, "housing_at_3_districts");
        }
        if self.players[city.owner].governors.contains(&city.id) {
            h += self.policy_effect(city.owner, "governor_housing");
        }
        h + self.gov_effects(city.owner).housing
    }

    pub fn wonder_built(&self, name: &str) -> bool {
        self.cities
            .values()
            .any(|city| city.wonders.contains_key(name))
    }

    fn empire_building_sum(
        &self,
        pid: usize,
        f: impl Fn(&crate::rules::BuildingSpec) -> f64,
    ) -> f64 {
        self.cities
            .values()
            .filter(|c| c.owner == pid)
            .flat_map(|c| c.buildings.iter())
            .map(|b| f(&self.rules.buildings[b.as_str()]))
            .sum()
    }

    fn empire_wonder_effect(&self, pid: usize, effect: &str) -> f64 {
        self.cities
            .values()
            .filter(|city| city.owner == pid)
            .flat_map(|city| city.wonders.keys())
            .map(|wonder| {
                self.rules.wonders[wonder.as_str()]
                    .effects
                    .get(effect)
                    .copied()
                    .unwrap_or(0.0)
            })
            .sum()
    }

    pub fn city_power_demand(&self, city: &City) -> f64 {
        city.buildings
            .iter()
            .filter(|building| !city.pillaged_buildings.contains(*building))
            .map(|building| self.rules.buildings[building.as_str()].power)
            .sum()
    }

    pub fn city_power_supply(&self, city: &City) -> f64 {
        let mut renewable = city
            .buildings
            .iter()
            .filter(|building| !city.pillaged_buildings.contains(*building))
            .map(|building| {
                self.rules.buildings[building.as_str()]
                    .effects
                    .get("renewable_power_generated")
                    .copied()
                    .unwrap_or(0.0)
            })
            .sum::<f64>();
        for position in &city.owned_tiles {
            let tile = &self.map.tiles[position];
            if tile.pillaged {
                continue;
            }
            if let Some(improvement) = &tile.improvement {
                renewable += self.rules.improvements[improvement.as_str()]
                    .effects
                    .get("power")
                    .copied()
                    .unwrap_or(0.0);
            }
        }

        let fueled = self.cities.values().any(|source| {
            if source.owner != city.owner {
                return false;
            }
            let Some(industrial_zone) =
                self.city_district_family_position(source, "industrial_zone")
            else {
                return false;
            };
            if self.wdist(industrial_zone, city.pos) > 6 {
                return false;
            }
            [
                ("nuclear_power_plant", "uranium"),
                ("oil_power_plant", "oil"),
                ("coal_power_plant", "coal"),
            ]
            .into_iter()
            .any(|(plant, resource)| {
                source.buildings.iter().any(|building| building == plant)
                    && !source
                        .pillaged_buildings
                        .iter()
                        .any(|building| building == plant)
                    && self.has_resource(city.owner, resource)
            })
        });
        if fueled {
            renewable.max(self.city_power_demand(city))
        } else {
            renewable
        }
    }

    pub fn city_is_powered(&self, city: &City) -> bool {
        let demand = self.city_power_demand(city);
        demand <= 0.0 || self.city_power_supply(city) + 1e-9 >= demand
    }

    fn add_powered_building_yields(spec: &crate::rules::BuildingSpec, yields: &mut Yields) {
        yields.food += spec.effects.get("powered_food").copied().unwrap_or(0.0);
        yields.production += spec
            .effects
            .get("powered_production")
            .copied()
            .unwrap_or(0.0);
        yields.gold += spec.effects.get("powered_gold").copied().unwrap_or(0.0);
        yields.science += spec.effects.get("powered_science").copied().unwrap_or(0.0);
        yields.culture += spec.effects.get("powered_culture").copied().unwrap_or(0.0);
        yields.faith += spec.effects.get("powered_faith").copied().unwrap_or(0.0);
    }

    /// Non-stacking regional building yields and Amenities reaching a city.
    fn regional_building_effects(&self, city: &City) -> (Yields, f64) {
        let mut groups: BTreeMap<String, (Yields, f64)> = BTreeMap::new();
        for source in self
            .cities
            .values()
            .filter(|source| source.owner == city.owner)
        {
            for building in &source.buildings {
                if source.pillaged_buildings.contains(building) {
                    continue;
                }
                let spec = &self.rules.buildings[building.as_str()];
                let origin = spec
                    .district
                    .as_deref()
                    .and_then(|district| self.city_district_family_position(source, district))
                    .unwrap_or(source.pos);
                if spec.regional_range <= 0 || self.wdist(origin, city.pos) > spec.regional_range {
                    continue;
                }
                let group = if !spec.regional_group.is_empty() {
                    spec.regional_group.clone()
                } else {
                    spec.replaces.clone().unwrap_or_else(|| building.clone())
                };
                let entry = groups.entry(group).or_default();
                let mut source_yields = spec.yields;
                let mut source_amenity = spec.amenity;
                if self.city_is_powered(source) {
                    Self::add_powered_building_yields(spec, &mut source_yields);
                    source_amenity += spec.effects.get("powered_amenity").copied().unwrap_or(0.0);
                }
                entry.0.food = entry.0.food.max(source_yields.food);
                entry.0.production = entry.0.production.max(source_yields.production);
                entry.0.gold = entry.0.gold.max(source_yields.gold);
                entry.0.science = entry.0.science.max(source_yields.science);
                entry.0.culture = entry.0.culture.max(source_yields.culture);
                entry.0.faith = entry.0.faith.max(source_yields.faith);
                entry.1 = entry.1.max(source_amenity);
            }
        }
        groups.values().fold(
            (Yields::default(), 0.0),
            |(mut yields, amenities), (group_yields, group_amenities)| {
                yields.add(*group_yields);
                (yields, amenities + group_amenities)
            },
        )
    }

    pub fn empire_luxuries(&self, pid: usize) -> usize {
        self.empire_luxury_names(pid).len()
    }

    /// Distinct luxury resources that actually supply the empire. A resource
    /// must be improved (or lie under a City Center); merely owning an
    /// unimproved copy does not unlock its Amenities or Aztec combat bonus.
    fn empire_luxury_names(&self, pid: usize) -> BTreeSet<&str> {
        let mut lux = BTreeSet::new();
        for c in self.cities.values().filter(|c| c.owner == pid) {
            for pos in &c.owned_tiles {
                let tile = &self.map.tiles[pos];
                if let Some(r) = tile.resource.as_deref() {
                    let spec = &self.rules.resources[r];
                    let connected = *pos == c.pos
                        || tile.improvement.as_deref() == Some(spec.improvement.as_str());
                    if spec.class == "luxury" && connected {
                        lux.insert(r);
                    }
                }
            }
        }
        lux
    }

    /// Gathering Storm removed the free population Amenity: cities require
    /// ceil(population / 2), while the Palace supplies the capital with one.
    fn city_amenities_required(city: &City) -> i64 {
        (city.pop.max(1) as i64 + 1) / 2
    }

    fn city_local_amenities(&self, city: &City) -> i64 {
        let mut supply = if self.city_has_palace(city) {
            self.rules.buildings["palace"].amenity
        } else {
            0.0
        };
        for (district, position) in &city.districts {
            supply += self.district_amenity(district, *position);
        }
        for b in &city.buildings {
            let spec = &self.rules.buildings[b.as_str()];
            if !city.pillaged_buildings.contains(b) && spec.regional_range <= 0 {
                supply += spec.amenity;
                if self.city_is_powered(city) {
                    supply += spec.effects.get("powered_amenity").copied().unwrap_or(0.0);
                }
            }
        }
        supply += self.regional_building_effects(city).1;
        for wonder in city.wonders.keys() {
            supply += self.rules.wonders[wonder.as_str()].amenity;
        }
        supply += self.gov_effects(city.owner).amenity;
        let garrison = self.units_at(city.pos).into_iter().any(|id| {
            let o = &self.units[&id];
            o.owner == city.owner && self.rules.units[o.kind.as_str()].class == "military"
        });
        if garrison {
            supply += self.policy_effect(city.owner, "garrison_amenity");
        }
        if city.districts.len() >= 2 {
            supply += self.policy_effect(city.owner, "amenity_at_2_districts");
        }
        if city.districts.len() >= 3 {
            supply += self.policy_effect(city.owner, "amenity_at_3_districts");
        }
        if self.players[city.owner].governors.contains(&city.id) {
            supply += self.policy_effect(city.owner, "governor_amenity");
        }
        supply.round() as i64
    }

    /// Each distinct luxury gives +1 Amenity to the four cities that need it
    /// most. Gifts for the Tlatoani raises that reach to six for the Aztecs.
    fn luxury_amenity_allocations(&self, pid: usize) -> BTreeMap<u32, i64> {
        let cities: Vec<&City> = self
            .cities
            .values()
            .filter(|city| city.owner == pid)
            .collect();
        let mut allocations: BTreeMap<u32, i64> = cities.iter().map(|city| (city.id, 0)).collect();
        let mut surplus: BTreeMap<u32, i64> = cities
            .iter()
            .map(|city| {
                (
                    city.id,
                    self.city_local_amenities(city) - Self::city_amenities_required(city),
                )
            })
            .collect();
        let reach = if self.has_ability(pid, "gifts_for_the_tlatoani") {
            6
        } else {
            4
        };
        for _ in self.empire_luxury_names(pid) {
            let mut neediest: Vec<u32> = cities.iter().map(|city| city.id).collect();
            neediest.sort_by_key(|cid| (surplus[cid], *cid));
            for cid in neediest.into_iter().take(reach) {
                *allocations.get_mut(&cid).unwrap() += 1;
                *surplus.get_mut(&cid).unwrap() += 1;
            }
        }
        allocations
    }

    pub fn city_amenity_surplus(&self, city: &City) -> i64 {
        let luxury = self
            .luxury_amenity_allocations(city.owner)
            .get(&city.id)
            .copied()
            .unwrap_or(0);
        self.city_local_amenities(city) + luxury - Self::city_amenities_required(city)
    }

    fn amenity_yield_mult(&self, city: &City) -> f64 {
        Self::amenity_yield_mult_for(self.city_amenity_surplus(city))
    }

    fn amenity_yield_mult_for(surplus: i64) -> f64 {
        if surplus >= 5 {
            1.20
        } else if surplus >= 3 {
            1.10
        } else if surplus >= 0 {
            1.0
        } else if surplus >= -2 {
            0.90
        } else if surplus >= -4 {
            0.80
        } else if surplus >= -6 {
            0.70
        } else {
            0.60
        }
    }

    fn amenity_growth_mult(surplus: i64) -> f64 {
        if surplus >= 5 {
            1.20
        } else if surplus >= 3 {
            1.10
        } else if surplus >= 0 {
            1.0
        } else if surplus >= -2 {
            0.85
        } else if surplus >= -4 {
            0.70
        } else {
            0.0
        }
    }

    fn housing_growth_mult(headroom: f64) -> f64 {
        if headroom >= 2.0 {
            1.0
        } else if headroom >= 1.0 {
            0.5
        } else if headroom > -5.0 {
            0.25
        } else {
            0.0
        }
    }

    pub fn city_can_strike(&self, city: &City) -> bool {
        !city.struck && city.wall_hp > 0 // ranged strike needs standing walls
    }

    /// Route an attack roll into a walled city: walls absorb it (melee does
    /// 15%, ranged 50%, siege 100% of the roll to walls), while the city
    /// itself takes 1 damage behind healthy walls (>=80%), half through
    /// damaged walls, and full damage once breached (<20%) or bare (Civ 6).
    fn city_take_damage(&mut self, cid: u32, dmg: i32, wall_mult: f64, bypass_walls: bool) {
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
                (!p.techs.contains(*t) || s.repeatable)
                    && s.requires.iter().all(|r| p.techs.contains(r))
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
                (!p.civics.contains(*c) || s.repeatable)
                    && s.requires.iter().all(|r| p.civics.contains(r))
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

    /// Whether a resource is revealed to a player by its technology/civic.
    pub fn resource_visible_to(&self, pid: usize, res: &str) -> bool {
        self.rules
            .resources
            .get(res)
            .is_some_and(|spec| self.unlocked(pid, &spec.tech, &spec.civic))
    }

    fn has_resource(&self, pid: usize, res: &str) -> bool {
        if !self.resource_visible_to(pid, res) {
            return false;
        }
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
            charges += self.empire_wonder_effect(owner, "builder_charges") as i32;
            if self.has_policy(owner, "serfdom") {
                charges += 2;
            }
            if self.has_ability(owner, "dynastic_cycle") {
                charges += 1; // China: First Emperor
            }
        }
        let mut u = Unit {
            id: self.next_id,
            kind: kind.to_string(),
            owner,
            pos,
            hp: 100,
            moves_left: spec.moves,
            charges,
            xp: 0,
            level: 1,
            promotions: BTreeSet::new(),
            formation: 0,
            linked_to: None,
            religion: if spec.class == "religious" {
                self.players[owner].religion.clone()
            } else {
                None
            },
            attacks_left: 1,
            moved: false,
            fortified: false,
            fortify_turns: 0,
            acted: false,
            zoc_stopped: false,
            started_turn_in_zoc: false,
            air_patrol: false,
            bonus_moves: 0.0,
        };
        if kind == "apostle" {
            // Apostles choose one promotion immediately after purchase.
            u.xp = Self::promotion_threshold(1);
            if self.empire_wonder_effect(owner, "apostles_gain_martyr") > 0.0 {
                u.promotions.insert("martyr".to_string());
            }
        }
        self.next_id += 1;
        let id = u.id;
        let sight = spec.sight;
        self.occ.entry(pos).or_default().push(id);
        self.units.insert(id, u);
        self.reveal(owner, pos, sight);
        id
    }

    #[cfg(test)]
    pub(crate) fn spawn_test_unit(&mut self, kind: &str, owner: usize, pos: Pos) -> u32 {
        self.spawn_unit(kind, owner, pos)
    }

    fn remove_unit(&mut self, uid: u32) {
        let carried_aircraft: Vec<u32> = self
            .units
            .get(&uid)
            .filter(|unit| unit.kind == "aircraft_carrier")
            .map(|carrier| {
                self.units_at(carrier.pos)
                    .into_iter()
                    .filter(|other| {
                        *other != uid
                            && self.units[other].owner == carrier.owner
                            && self.rules.units[self.units[other].kind.as_str()]
                                .domain
                                .as_deref()
                                == Some("air")
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(u) = self.units.remove(&uid) {
            if let Some(other) = u.linked_to {
                if let Some(peer) = self.units.get_mut(&other) {
                    peer.linked_to = None;
                }
            }
            if let Some(ids) = self.occ.get_mut(&u.pos) {
                ids.retain(|i| *i != uid);
                if ids.is_empty() {
                    self.occ.remove(&u.pos);
                }
            }
        }
        for aircraft in carried_aircraft {
            self.remove_unit(aircraft);
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
        let sight = self.unit_sight(uid);
        self.reveal(owner, pos, sight);
    }

    fn reveal(&mut self, pid: usize, pos: Pos, radius: i32) {
        let mut wonders: Vec<Pos> = Vec::new();
        for p in self.wdisk(pos, radius) {
            if let Some(t) = self.map.tiles.get(&p) {
                let new = self.players[pid].explored.insert(p);
                if new && !self.players[pid].is_minor {
                    let nw = t
                        .feature
                        .as_ref()
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
            let first = !self
                .players
                .iter()
                .any(|o| o.id != pid && !o.is_minor && o.explored.contains(&p));
            self.players[pid].era_score += if first { 3 } else { 1 };
        }
    }

    /// Terrain MP to step between adjacent tiles. The unit-aware wrapper
    /// applies bridge technology and promotion exceptions.
    pub fn step_cost(&self, from: Pos, to: Pos) -> f64 {
        let t = &self.map.tiles[&to];
        let mut c = self.rules.move_cost(t);
        if self.crosses_river(from, to) {
            c += 2.0;
        }
        c
    }

    fn unit_step_cost(&self, uid: u32, from: Pos, to: Pos) -> f64 {
        let unit = &self.units[&uid];
        let tile = &self.map.tiles[&to];
        let mut cost = self.step_cost(from, to);
        if self.crosses_river(from, to)
            && self.map.tiles[&from].road
            && tile.road
            && self.players[unit.owner]
                .techs
                .contains("military_engineering")
        {
            cost -= 2.0;
        }
        if self.promotion_effect(unit, "woods_move_cost") > 0.0
            && matches!(tile.feature.as_deref(), Some("forest" | "jungle"))
        {
            cost = 1.0;
        }
        if self.promotion_effect(unit, "hills_move_cost") > 0.0 && tile.hills {
            cost = 1.0;
        }
        if self.promotion_effect(unit, "amphibious") > 0.0 && self.crosses_river(from, to) {
            cost = self.rules.move_cost(tile);
        }
        cost
    }

    fn crosses_river(&self, from: Pos, to: Pos) -> bool {
        let Some(a) = self.map.get(from) else {
            return false;
        };
        let Some(b) = self.map.get(to) else {
            return false;
        };
        !self.rules.is_water(a) && !self.rules.is_water(b) && self.map.has_river_edge(from, to)
    }

    fn crosses_cliff(&self, from: Pos, to: Pos) -> bool {
        let Some(a) = self.map.get(from) else {
            return false;
        };
        let Some(b) = self.map.get(to) else {
            return false;
        };
        self.rules.is_water(a) != self.rules.is_water(b) && self.map.has_cliff_edge(from, to)
    }

    fn sight_height(&self, pos: Pos) -> i32 {
        let t = &self.map.tiles[&pos];
        if t.terrain == "mountain" {
            return 3;
        }
        t.hills as i32 + matches!(t.feature.as_deref(), Some("forest" | "jungle")) as i32
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
                || t.district
                    .as_deref()
                    .is_some_and(|district| self.district_is_family(district, "encampment"))
            {
                attacker_height += 1;
            }
        }
        let target_height = self.sight_height(to);
        self.nbrs(from)
            .into_iter()
            .filter(|p| self.wdist(*p, to) == 1)
            .any(|middle| {
                let blocker = self.sight_height(middle);
                blocker <= attacker_height || blocker < target_height
            })
    }

    fn unit_has_line_of_sight(&self, uid: u32, to: Pos) -> bool {
        let unit = &self.units[&uid];
        if self.promotion_effect(unit, "see_through_woods") > 0.0 && self.wdist(unit.pos, to) == 2 {
            let attacker_height = self.map.tiles[&unit.pos].hills as i32;
            let target_height = self.sight_height(to);
            return self
                .nbrs(unit.pos)
                .into_iter()
                .filter(|middle| self.wdist(*middle, to) == 1)
                .any(|middle| {
                    let tile = &self.map.tiles[&middle];
                    let blocker = if tile.terrain == "mountain" {
                        3
                    } else {
                        tile.hills as i32
                    };
                    blocker <= attacker_height || blocker < target_height
                });
        }
        self.has_line_of_sight(unit.pos, to, true)
    }

    fn unit_base_max_moves(&self, uid: u32) -> f64 {
        let u = &self.units[&uid];
        let spec = &self.rules.units[u.kind.as_str()];
        let tile = &self.map.tiles[&u.pos];
        let mut moves = if matches!(u.kind.as_str(), "war_cart" | "maryannu_chariot_archer")
            && !tile.hills
            && !matches!(tile.feature.as_deref(), Some("forest" | "jungle"))
        {
            4.0
        } else {
            spec.moves
        } + self.promotion_effect(u, "movement")
            + u.bonus_moves;
        if self.rules.is_water(tile) {
            moves += self.tree_effect(u.owner, "naval_movement");
            if spec.domain.as_deref() != Some("sea") {
                moves += self.tree_effect(u.owner, "embarked_movement");
            }
        }
        if u.kind == "giant_death_robot" {
            moves += self.tree_effect(u.owner, "gdr_movement");
        }
        moves
    }

    fn unit_max_moves(&self, uid: u32) -> f64 {
        let base = self.unit_base_max_moves(uid);
        let unit = &self.units[&uid];
        let Some(linked) = unit.linked_to.and_then(|id| self.units.get(&id)) else {
            return base;
        };
        let spec = &self.rules.units[unit.kind.as_str()];
        if spec.class != "military" {
            return base;
        }
        if self.promotion_effect(unit, "escort_mobility") > 0.0 {
            base
        } else {
            base.min(self.unit_base_max_moves(linked.id))
        }
    }

    pub fn unit_sight(&self, uid: u32) -> i32 {
        let unit = &self.units[&uid];
        self.rules.units[unit.kind.as_str()].sight + self.promotion_effect(unit, "sight") as i32
    }

    /// Camouflaged recon and Naval Raider units are hidden unless an
    /// adjacent unit, Naval Raider, or Destroyer detects them.
    pub fn unit_visible_to(&self, uid: u32, viewer: usize) -> bool {
        let Some(unit) = self.units.get(&uid) else {
            return false;
        };
        if unit.owner == viewer {
            return true;
        }
        let raider = self.rules.units[unit.kind.as_str()].promotion_class == "naval_raider";
        let camouflaged = self.promotion_effect(unit, "camouflage") > 0.0;
        if !raider && !camouflaged {
            return true;
        }
        self.player_unit_ids(viewer).into_iter().any(|other_id| {
            let other = &self.units[&other_id];
            let distance = self.wdist(other.pos, unit.pos);
            distance <= 1
                || (raider
                    && distance <= self.unit_sight(other_id)
                    && (other.kind == "destroyer"
                        || self.rules.units[other.kind.as_str()].promotion_class == "naval_raider"))
        })
    }

    fn exerts_zoc(&self, u: &Unit) -> bool {
        let spec = &self.rules.units[u.kind.as_str()];
        // Embarkation does not remove a land unit's ZOC. Its native domain
        // still limits projection to land tiles, which is handled by the
        // caller when comparing the target tile's domain.
        spec.zone_of_control || self.promotion_effect(u, "zone_of_control") > 0.0
    }

    /// Cavalry, Naval Raiders, the Viking Longship, and air units ignore
    /// incoming ZOC. Civilian/support passengers inherit that ability from
    /// an escort, matching linked-formation behavior in Civ VI.
    fn unit_ignores_zoc(&self, uid: u32) -> bool {
        let Some(unit) = self.units.get(&uid) else {
            return false;
        };
        let spec = &self.rules.units[unit.kind.as_str()];
        let innate = spec.cavalry
            || spec.promotion_class == "naval_raider"
            || spec.domain.as_deref() == Some("air")
            || unit.kind == "viking_longship";
        if innate {
            return true;
        }
        if !matches!(spec.class.as_str(), "civilian" | "support") {
            return false;
        }
        unit.linked_to
            .and_then(|peer| self.units.get(&peer))
            .is_some_and(|peer| {
                let peer_spec = &self.rules.units[peer.kind.as_str()];
                peer_spec.cavalry
                    || peer_spec.promotion_class == "naval_raider"
                    || peer_spec.domain.as_deref() == Some("air")
                    || peer.kind == "viking_longship"
            })
    }

    fn formation_zoc_stopped(&self, uid: u32) -> bool {
        let Some(unit) = self.units.get(&uid) else {
            return true;
        };
        unit.zoc_stopped
            || (self.is_linked_leader(uid)
                && unit
                    .linked_to
                    .and_then(|peer| self.units.get(&peer))
                    .is_some_and(|peer| peer.zoc_stopped))
    }

    fn formation_movement_locked_by_zoc(&self, uid: u32) -> bool {
        if self.formation_zoc_stopped(uid) {
            return true;
        }
        let action_locked = |id: u32| {
            self.units.get(&id).is_some_and(|unit| {
                unit.started_turn_in_zoc
                    && unit.acted
                    && !unit.moved
                    && !self.unit_ignores_zoc(id)
                    && self.in_enemy_zoc_for(id, unit.pos)
            })
        };
        action_locked(uid)
            || (self.is_linked_leader(uid) && self.units[&uid].linked_to.is_some_and(action_locked))
    }

    fn formation_enters_enemy_zoc(&self, uid: u32, pos: Pos) -> bool {
        if !self.unit_ignores_zoc(uid) && self.in_enemy_zoc_for(uid, pos) {
            return true;
        }
        let Some(peer) = self
            .units
            .get(&uid)
            .filter(|_| self.is_linked_leader(uid))
            .and_then(|unit| unit.linked_to)
        else {
            return false;
        };
        !self.unit_ignores_zoc(peer) && self.in_enemy_zoc_for(peer, pos)
    }

    fn stop_unit_by_zoc(&mut self, uid: u32) {
        let Some(unit) = self.units.get(&uid) else {
            return;
        };
        let loses_all_movement = matches!(
            self.rules.units[unit.kind.as_str()].class.as_str(),
            "civilian" | "support"
        );
        let unit = self.units.get_mut(&uid).unwrap();
        unit.zoc_stopped = true;
        if loses_all_movement {
            unit.moves_left = 0.0;
        }
    }

    fn noncombat_action_blocked_by_zoc(&self, uid: u32) -> bool {
        self.units.get(&uid).is_some_and(|unit| {
            unit.zoc_stopped
                && matches!(
                    self.rules.units[unit.kind.as_str()].class.as_str(),
                    "civilian" | "support"
                )
        })
    }

    /// A standing Encampment remains a combat target at zero HP until a
    /// melee unit enters and pillages it, like a depleted City Center.
    pub(crate) fn encampment_at(&self, pos: Pos) -> Option<u32> {
        let tile = self.map.get(pos)?;
        if !tile
            .district
            .as_deref()
            .is_some_and(|district| self.district_is_family(district, "encampment"))
        {
            return None;
        }
        let cid = tile.owner_city?;
        let city = self.cities.get(&cid)?;
        (!city.encampment_pillaged).then_some(cid)
    }

    /// City Centers and every district whose rules data grants ZOC (the
    /// Encampment family and Oppidum in stock Civ VI) project across both
    /// domains and across rivers until pillaged.
    fn defensible_district_owner_at(&self, pos: Pos) -> Option<usize> {
        if let Some(cid) = self.city_at(pos) {
            return self.cities.get(&cid).map(|city| city.owner);
        }
        let tile = self.map.get(pos)?;
        let district = tile.district.as_deref()?;
        let has_zoc =
            self.rules.districts.get(district).is_some_and(|spec| {
                spec.effects.get("zone_of_control").copied().unwrap_or(0.0) > 0.0
            });
        if !has_zoc {
            return None;
        }
        let cid = tile.owner_city?;
        let city = self.cities.get(&cid)?;
        let pillaged = if self.district_is_family(district, "encampment") {
            city.encampment_pillaged
        } else {
            tile.pillaged
        };
        (!pillaged).then_some(city.owner)
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
            if self.map.get(n).is_none() {
                continue;
            }
            for oid in self.units_at(n) {
                let o = &self.units[&oid];
                if o.owner == pid || !self.is_at_war(pid, o.owner) || !self.exerts_zoc(o) {
                    continue;
                }
                let o_water = self.rules.units[o.kind.as_str()].domain.as_deref() == Some("sea");
                if o_water != water || self.crosses_river(n, pos) {
                    continue;
                }
                return true;
            }
            if self
                .defensible_district_owner_at(n)
                .is_some_and(|owner| owner != pid && self.is_at_war(pid, owner))
            {
                return true;
            }
        }
        false
    }

    fn in_enemy_zoc_for(&self, uid: u32, pos: Pos) -> bool {
        let mover = &self.units[&uid];
        let mover_spec = &self.rules.units[mover.kind.as_str()];
        let Some(t) = self.map.get(pos) else {
            return false;
        };
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
                    && mover.religion.is_some()
                    && other.religion.is_some()
                    && mover.religion != other.religion;
                let military_zoc =
                    self.is_at_war(mover.owner, other.owner) && self.exerts_zoc(other);
                if !religious_zoc && !military_zoc {
                    continue;
                }
                let other_water = other_spec.domain.as_deref() == Some("sea");
                if other_water == water && !self.crosses_river(n, pos) {
                    return true;
                }
            }
            if self
                .defensible_district_owner_at(n)
                .map(|owner| owner != mover.owner && self.is_at_war(mover.owner, owner))
                .unwrap_or(false)
            {
                return true;
            }
        }
        false
    }

    pub fn can_move(&self, uid: u32, pos: Pos) -> bool {
        let u = &self.units[&uid];
        if u.linked_to.is_some() && !self.is_linked_leader(uid) {
            return false;
        }
        if self.formation_movement_locked_by_zoc(uid) {
            return false;
        }
        if u.attacks_left < self.unit_max_attacks(uid)
            && self.promotion_effect(u, "move_after_attack") == 0.0
        {
            return false;
        }
        // MP is paid before entering (Civ 6): need the full step cost, but a
        // unit with untouched movement may always take one step.
        let full = u.moves_left >= self.unit_max_moves(uid);
        if !full
            && self.map.tiles.contains_key(&pos)
            && u.moves_left < self.unit_step_cost(uid, u.pos, pos)
        {
            return false;
        }
        if !self.can_enter(uid, self.units[&uid].pos, pos) {
            return false;
        }
        if self.is_linked_leader(uid) {
            let peer = self.units[&uid].linked_to.unwrap();
            return self.can_enter(peer, self.units[&peer].pos, pos);
        }
        true
    }

    fn can_enter(&self, uid: u32, from: Pos, pos: Pos) -> bool {
        let u = &self.units[&uid];
        if self.wdist(from, pos) != 1 {
            return false;
        }
        if !self.unit_can_traverse(uid, pos) {
            return false;
        }
        if self.crosses_cliff(from, pos) && self.promotion_effect(u, "scale_cliffs") <= 0.0 {
            return false;
        }
        let spec = &self.rules.units[u.kind.as_str()];
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
        if let Some(cid) = self.encampment_at(pos) {
            if self.cities[&cid].owner != u.owner {
                return false;
            }
        }
        true
    }

    fn is_linked_leader(&self, uid: u32) -> bool {
        let Some(unit) = self.units.get(&uid) else {
            return false;
        };
        let Some(peer) = unit.linked_to.and_then(|id| self.units.get(&id)) else {
            return false;
        };
        let spec = &self.rules.units[unit.kind.as_str()];
        let peer_spec = &self.rules.units[peer.kind.as_str()];
        spec.class == "military"
            && (peer_spec.class != "military"
                || (spec.domain.as_deref() == Some("sea")
                    && peer_spec.domain.as_deref() != Some("sea")))
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
        if self.formation_movement_locked_by_zoc(uid) || self.wdist(start, to) <= range {
            return None;
        }
        if range == 0 {
            self.map.get(to)?;
            if !self.unit_can_traverse(uid, to) {
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
                if distance
                    .get(&n)
                    .map(|d| next_distance >= *d)
                    .unwrap_or(false)
                {
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
        if !self.unit_can_traverse(uid, pos) {
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
        if self.formation_movement_locked_by_zoc(uid) || is_goal(start) {
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
        let max_moves = self.unit_max_moves(uid);
        if self.formation_movement_locked_by_zoc(uid) {
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
                let cost = self.unit_step_cost(uid, cur, n);
                let fresh = cur == start && rem >= max_moves;
                if rem < cost && !fresh {
                    continue; // MP paid up front (Civ 6)
                }
                let mut new_rem = (rem - cost).max(0.0);
                if self.formation_enters_enemy_zoc(uid, n) {
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
        let max_moves = self.unit_max_moves(uid);
        if self.formation_movement_locked_by_zoc(uid) {
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
                let cost = self.unit_step_cost(uid, cur, n);
                let fresh = cur == start && rem >= max_moves;
                if rem < cost && !fresh {
                    continue;
                }
                let mut new_rem = (rem - cost).max(0.0);
                if self.formation_enters_enemy_zoc(uid, n) {
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
        let path = self
            .path_to(uid, to)
            .ok_or_else(|| "unreachable".to_string())?;
        if path.is_empty() {
            return Err("already there".into());
        }
        for step in path {
            if self
                .units
                .get(&uid)
                .map(|x| x.moves_left <= 0.0)
                .unwrap_or(true)
            {
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
        city.buildings
            .iter()
            .map(|building| self.rules.buildings[building.as_str()].outer_defense)
            .sum()
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
        base + self.policy_effect(owner, "city_ranged")
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
                (u.owner == city.owner && self.rules.units[u.kind.as_str()].class == "military")
                    .then(|| self.unit_unembarked_strength(u))
            })
            .fold(0.0, f64::max);
        let mut s = (strongest_built - 10.0).max(garrison).max(10.0);
        if city.wall_hp > 0 {
            // +3 combat strength per standing wall level (Civ 6)
            s += 3.0
                * city
                    .buildings
                    .iter()
                    .filter(|building| self.rules.buildings[building.as_str()].outer_defense > 0)
                    .count() as f64;
        }
        s += 2.0 * city.districts.len() as f64;
        s += self.tile_defense_bonus(city.pos);
        if self.city_has_palace(city) {
            s += 3.0;
        }
        s += self.policy_effect(city.owner, "city_defense");
        if self.players[city.owner].is_minor {
            if let Some(suzerain) = self.suzerain_of(city.owner) {
                s += self.envoys_at(suzerain, city.owner) as f64;
            }
        }
        let damaged_penalty = (10.0 - city.hp.clamp(0, 200) as f64 / 20.0).round();
        (s - damaged_penalty).max(0.0)
    }

    pub fn encampment_strength(&self, cid: u32) -> f64 {
        let city = &self.cities[&cid];
        let current_best = self
            .units
            .values()
            .filter(|unit| unit.owner == city.owner)
            .map(|unit| self.rules.units[unit.kind.as_str()].strength)
            .fold(20.0, f64::max);
        let strongest_built = self.players[city.owner]
            .counters
            .get("strongest_unit_built")
            .map(|value| *value as f64)
            .unwrap_or(current_best);
        let mut strength = (strongest_built - 10.0).max(10.0);
        if city.encampment_wall_hp > 0 {
            strength += 3.0
                * city
                    .buildings
                    .iter()
                    .filter(|building| self.rules.buildings[building.as_str()].outer_defense > 0)
                    .count() as f64;
        }
        strength += 2.0 * city.districts.len() as f64;
        if let Some(position) = self.city_district_family_position(city, "encampment") {
            strength += self.tile_defense_bonus(position);
        }
        if self.city_has_palace(city) {
            strength += 3.0;
        }
        strength += self.policy_effect(city.owner, "city_defense");
        let damaged = (10.0 - city.encampment_hp.clamp(0, 100) as f64 / 10.0).round();
        (strength - damaged).max(0.0)
    }

    fn encampment_take_damage(
        &mut self,
        cid: u32,
        damage: i32,
        wall_mult: f64,
        bypass_walls: bool,
    ) {
        let (wall, max) = {
            let city = &self.cities[&cid];
            (city.encampment_wall_hp, self.city_max_wall_hp(city))
        };
        let city = self.cities.get_mut(&cid).unwrap();
        city.encampment_last_attacked = self.turn;
        if wall > 0 && max > 0 {
            let fraction = wall as f64 / max as f64;
            let through = if bypass_walls {
                damage
            } else if fraction >= 0.8 {
                1
            } else if fraction >= 0.2 {
                damage / 2
            } else {
                damage
            };
            city.encampment_wall_hp =
                (wall - ((damage as f64 * wall_mult).round() as i32).max(1)).max(0);
            city.encampment_hp -= through.max(1);
        } else {
            city.encampment_hp -= damage;
        }
    }

    fn district_under_siege(&self, owner: usize, position: Pos) -> bool {
        crate::hex::neighbors(position)
            .into_iter()
            .map(|pos| hex::canon(pos, self.map.width))
            .all(|pos| {
                let Some(tile) = self.map.get(pos) else {
                    return true;
                };
                if !self.rules.is_passable(tile) {
                    return true;
                }
                self.units_at(pos).into_iter().any(|id| {
                    let unit = &self.units[&id];
                    unit.owner != owner
                        && self.is_at_war(owner, unit.owner)
                        && self.rules.units[unit.kind.as_str()].class == "military"
                }) || self.in_enemy_zoc(owner, pos)
            })
    }

    /// A city cannot heal when every adjacent passable tile is occupied by a
    /// hostile combat unit or covered by hostile ZOC. Off-map and impassable
    /// neighbors count as sealed sides of the siege ring.
    fn city_under_siege(&self, cid: u32) -> bool {
        let city = &self.cities[&cid];
        self.district_under_siege(city.owner, city.pos)
    }

    fn district_family<'a>(&'a self, district: &'a str) -> &'a str {
        let mut current = district;
        // Stock replacements are one level deep. Following the full chain
        // makes modded replacements compose without scattered name lists.
        for _ in 0..self.rules.districts.len() {
            let Some(parent) = self
                .rules
                .districts
                .get(current)
                .and_then(|spec| spec.replaces.as_deref())
            else {
                break;
            };
            current = parent;
        }
        current
    }

    fn district_is_family(&self, district: &str, family: &str) -> bool {
        self.district_family(district) == self.district_family(family)
    }

    fn city_has_district_family(&self, city: &City, family: &str) -> bool {
        if family == "city_center" {
            return true;
        }
        city.districts
            .keys()
            .any(|district| self.district_is_family(district, family))
    }

    fn city_district_effect(&self, city: &City, effect: &str) -> f64 {
        city.districts
            .keys()
            .map(|district| {
                self.rules.districts[district.as_str()]
                    .effects
                    .get(effect)
                    .copied()
                    .unwrap_or(0.0)
            })
            .sum()
    }

    fn city_district_family_position(&self, city: &City, family: &str) -> Option<Pos> {
        city.districts.iter().find_map(|(district, position)| {
            self.district_is_family(district, family)
                .then_some(*position)
        })
    }

    fn home_continent(&self, pid: usize) -> Option<usize> {
        self.cities
            .values()
            .find(|city| city.is_capital && city.original_owner == pid)
            .and_then(|city| self.map.get(city.pos))
            .and_then(|tile| tile.continent)
    }

    fn on_foreign_continent(&self, pid: usize, position: Pos) -> bool {
        self.home_continent(pid).is_some_and(|home| {
            self.map
                .get(position)
                .and_then(|tile| tile.continent)
                .is_some_and(|continent| continent != home)
        })
    }

    fn city_has_building_family(&self, city: &City, family: &str) -> bool {
        city.buildings.iter().any(|building| {
            building == family
                || self
                    .rules
                    .buildings
                    .get(building)
                    .is_some_and(|spec| spec.replaces.as_deref() == Some(family))
        })
    }

    /// Gathering Storm appeal of a tile from adjacent terrain, features,
    /// improvements, wonders, and districts. River appeal belongs to the
    /// evaluated tile itself; the other modifiers come from its neighbors.
    pub fn tile_appeal(&self, position: Pos) -> i32 {
        let Some(tile) = self.map.get(position) else {
            return 0;
        };
        let mut appeal = i32::from(tile.has_river());
        for neighbor in self.nbrs(position) {
            let Some(adjacent) = self.map.get(neighbor) else {
                continue;
            };
            if matches!(adjacent.terrain.as_str(), "mountain" | "coast" | "lake") {
                appeal += 1;
            }
            match adjacent.feature.as_deref() {
                Some("forest" | "oasis") => appeal += 1,
                Some(
                    "jungle"
                    | "marsh"
                    | "floodplains"
                    | "grassland_floodplains"
                    | "plains_floodplains",
                ) => appeal -= 1,
                _ => {}
            }
            if adjacent.feature.as_ref().is_some_and(|feature| {
                self.rules
                    .features
                    .get(feature.as_str())
                    .is_some_and(|spec| spec.natural_wonder)
            }) {
                appeal += 2;
            }
            if adjacent.wonder.is_some() {
                appeal += 1;
            }
            if let Some(district) = adjacent.district.as_deref() {
                appeal += self
                    .rules
                    .districts
                    .get(district)
                    .map(|spec| spec.appeal.round() as i32)
                    .unwrap_or(0);
            }
            if matches!(
                adjacent.improvement.as_deref(),
                Some("mine" | "quarry" | "oil_well" | "airstrip" | "missile_silo")
            ) {
                appeal -= 1;
            }
        }
        if let Some(owner) = tile
            .owner_city
            .and_then(|city_id| self.cities.get(&city_id))
            .map(|city| city.owner)
        {
            appeal += self.empire_wonder_effect(owner, "empire_tile_appeal") as i32;
        }
        appeal
    }

    fn district_housing(&self, district: &str, position: Pos) -> f64 {
        let spec = &self.rules.districts[district];
        let Some(maximum) = spec.effects.get("appeal_housing_max").copied() else {
            return spec.housing;
        };
        let appeal = self.tile_appeal(position);
        let dynamic: f64 = if maximum >= 6.0 {
            match appeal {
                4.. => 6.0,
                2..=3 => 5.0,
                0..=1 => 4.0,
                -2..=-1 => 3.0,
                _ => 2.0,
            }
        } else {
            match appeal {
                4.. => 3.0,
                2..=3 => 2.0,
                0..=1 => 1.0,
                _ => 0.0,
            }
        };
        spec.housing + dynamic.min(maximum)
    }

    fn district_amenity(&self, district: &str, position: Pos) -> f64 {
        let spec = &self.rules.districts[district];
        let geothermal = spec
            .effects
            .get("geothermal_amenity")
            .copied()
            .unwrap_or(0.0);
        spec.amenity
            + if geothermal > 0.0
                && self.nbrs(position).into_iter().any(|neighbor| {
                    self.map
                        .get(neighbor)
                        .is_some_and(|tile| tile.feature.as_deref() == Some("geothermal_fissure"))
                })
            {
                geothermal
            } else {
                0.0
            }
    }

    pub fn district_yields(&self, dname: &str, dpos: Pos) -> Yields {
        let spec = &self.rules.districts[dname];
        let mut ys = spec.yields;
        if !spec.adjacency.is_empty() {
            let tile = &self.map.tiles[&dpos];
            let neighbors: Vec<&crate::world::Tile> = self
                .nbrs(dpos)
                .into_iter()
                .filter_map(|pos| self.map.get(pos))
                .collect();
            let owner = self
                .map
                .get(dpos)
                .and_then(|tile| tile.owner_city)
                .and_then(|city_id| self.cities.get(&city_id))
                .map(|city| city.owner);
            let gaul = owner.is_some_and(|pid| self.players[pid].civ == "Gaul");
            let count = |key: &str| -> usize {
                match key {
                    "self" => 1,
                    "river" => usize::from(tile.has_river()),
                    "mountain" => neighbors.iter().filter(|t| t.terrain == "mountain").count(),
                    "forest" | "woods" => neighbors
                        .iter()
                        .filter(|t| t.feature.as_deref() == Some("forest"))
                        .count(),
                    "rainforest" | "jungle" => neighbors
                        .iter()
                        .filter(|t| t.feature.as_deref() == Some("jungle"))
                        .count(),
                    "natural_wonder" => neighbors
                        .iter()
                        .filter(|t| {
                            t.feature.as_ref().is_some_and(|feature| {
                                self.rules
                                    .features
                                    .get(feature.as_str())
                                    .is_some_and(|spec| spec.natural_wonder)
                            })
                        })
                        .count(),
                    "reef" => neighbors
                        .iter()
                        .filter(|t| {
                            matches!(t.feature.as_deref(), Some("reef" | "great_barrier_reef"))
                        })
                        .count(),
                    "geothermal_fissure" => neighbors
                        .iter()
                        .filter(|t| t.feature.as_deref() == Some("geothermal_fissure"))
                        .count(),
                    "pamukkale" => neighbors
                        .iter()
                        .filter(|t| t.feature.as_deref() == Some("pamukkale"))
                        .count(),
                    // City Centers are districts but are represented by the
                    // city index instead of `Tile::district`.
                    "district" => neighbors
                        .iter()
                        .filter(|t| {
                            !gaul && (t.district.is_some() || self.city_at(t.pos).is_some())
                        })
                        .count(),
                    "city_center" => neighbors
                        .iter()
                        .filter(|t| {
                            t.owner_city
                                .and_then(|cid| self.cities.get(&cid))
                                .is_some_and(|city| city.pos == t.pos)
                        })
                        .count(),
                    "wonder" => neighbors.iter().filter(|t| t.wonder.is_some()).count(),
                    "coast_resource" | "sea_resource" => neighbors
                        .iter()
                        .filter(|t| self.rules.is_water(t) && t.resource.is_some())
                        .count(),
                    "strategic_resource" => neighbors
                        .iter()
                        .filter(|t| {
                            t.resource.as_ref().is_some_and(|resource| {
                                self.rules
                                    .resources
                                    .get(resource.as_str())
                                    .is_some_and(|spec| spec.class == "strategic")
                            })
                        })
                        .count(),
                    "luxury_resource" => neighbors
                        .iter()
                        .filter(|t| {
                            t.resource.as_ref().is_some_and(|resource| {
                                self.rules
                                    .resources
                                    .get(resource.as_str())
                                    .is_some_and(|spec| spec.class == "luxury")
                            })
                        })
                        .count(),
                    "resource" => neighbors.iter().filter(|t| t.resource.is_some()).count(),
                    "mine" | "quarry" | "lumber_mill" | "plantation" | "farm" => neighbors
                        .iter()
                        .filter(|t| t.improvement.as_deref() == Some(key))
                        .count(),
                    district_family if self.rules.districts.contains_key(district_family) => {
                        neighbors
                            .iter()
                            .filter(|t| {
                                t.district.as_deref().is_some_and(|district| {
                                    self.district_is_family(district, district_family)
                                })
                            })
                            .count()
                    }
                    _ => 0,
                }
            };
            let mut adj = Yields::default();
            for (key, bonus) in &spec.adjacency {
                let n = count(key) as f64;
                // Every source has its own TilesRequired bucket in Civ VI.
                // Fractions from different sources therefore never combine.
                adj.food += (n * bonus.food).trunc();
                adj.production += (n * bonus.production).trunc();
                adj.gold += (n * bonus.gold).trunc();
                adj.science += (n * bonus.science).trunc();
                adj.culture += (n * bonus.culture).trunc();
                adj.faith += (n * bonus.faith).trunc();
            }
            if gaul && spec.specialty {
                let mines = count("mine") as f64;
                let minor = (mines * 0.5).trunc();
                match self.district_family(dname) {
                    "campus" => adj.science += minor,
                    "holy_site" => adj.faith += minor,
                    "commercial_hub" | "harbor" => adj.gold += minor,
                    "theater_square" => adj.culture += minor,
                    "industrial_zone" => adj.production += minor,
                    _ => {}
                }
            }
            // The six adjacency-card families include unique replacements.
            if let Some(pid) = owner {
                let percent = if self.district_is_family(dname, "campus") {
                    self.policy_effect(pid, "campus_adjacency_pct")
                } else if self.district_is_family(dname, "holy_site") {
                    self.policy_effect(pid, "holy_site_adjacency_pct")
                } else if self.district_is_family(dname, "commercial_hub") {
                    self.policy_effect(pid, "commercial_hub_adjacency_pct")
                } else if self.district_is_family(dname, "harbor") {
                    self.policy_effect(pid, "harbor_adjacency_pct")
                } else if self.district_is_family(dname, "theater_square") {
                    self.policy_effect(pid, "theater_square_adjacency_pct")
                } else if self.district_is_family(dname, "industrial_zone") {
                    self.policy_effect(pid, "industrial_zone_adjacency_pct")
                } else {
                    0.0
                };
                let scale = percent / 100.0;
                adj.add(Yields {
                    food: adj.food * scale,
                    production: adj.production * scale,
                    gold: adj.gold * scale,
                    science: adj.science * scale,
                    culture: adj.culture * scale,
                    faith: adj.faith * scale,
                });
            }
            ys.add(adj);
        }
        if let Some(owner) = self
            .map
            .get(dpos)
            .and_then(|tile| tile.owner_city)
            .and_then(|city_id| self.cities.get(&city_id))
            .map(|city| city.owner)
        {
            if self.on_foreign_continent(owner, dpos) {
                ys.gold += spec
                    .effects
                    .get("foreign_continent_gold")
                    .copied()
                    .unwrap_or(0.0);
            }
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
        if let Some((amount, name)) = specialties
            .into_iter()
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then_with(|| a.1.cmp(b.1)))
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
                    focus = if spec.wonder {
                        "wonder"
                    } else {
                        "infrastructure"
                    }
                    .to_string();
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
                Item::Wonder { wonder, .. } => {
                    let spec = &self.rules.wonders[wonder.as_str()];
                    focus = "wonder".to_string();
                    weights.production += 1.75;
                    weights.gold += spec.yields.gold * 0.15;
                    weights.science += spec.yields.science * 0.22;
                    weights.culture += spec.yields.culture * 0.22;
                    weights.faith += spec.yields.faith * 0.22;
                }
                Item::Repair { .. } => {
                    focus = "repair".to_string();
                    weights.production += 1.25;
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
                weights.production += if self.map.tiles[&city.pos].has_river() {
                    1.00
                } else {
                    0.55
                };
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
            other.id != city.owner
                && other.alive
                && !other.is_barbarian
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
        CitizenStrategy {
            focus,
            weights,
            food_target,
        }
    }

    fn citizen_value(ys: Yields, weights: Yields) -> f64 {
        ys.food * weights.food
            + ys.production * weights.production
            + ys.gold * weights.gold
            + ys.science * weights.science
            + ys.culture * weights.culture
            + ys.faith * weights.faith
    }

    /// Choose the actual population-worked tiles. It starts with the highest
    /// strategic-value set, then performs the least-cost swaps needed to hit
    /// the food target. A final local improvement pass recovers strategic
    /// value without violating nutrition. This keeps the hot turn loop fast
    /// while preventing a production-focused governor from starving a city.
    pub fn city_citizen_plan(&self, cid: u32) -> CitizenPlan {
        let city = &self.cities[&cid];
        let strategy = self.citizen_strategy(cid);
        let mut center = self.workable_tile_yields(city.pos);
        center.food = center.food.max(2.0);
        center.production = center.production.max(1.0);

        let mut cands: Vec<(Pos, Yields, f64)> = city
            .owned_tiles
            .iter()
            .filter(|pos| **pos != city.pos)
            .filter_map(|pos| {
                let tile = &self.map.tiles[pos];
                if tile.district.is_some() || tile.wonder.is_some() {
                    return None;
                }
                let ys = self.workable_tile_yields(*pos);
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
        food += cands
            .iter()
            .enumerate()
            .filter(|(i, _)| selected[*i])
            .map(|(_, c)| c.1.food)
            .sum::<f64>();

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
                    if best
                        .as_ref()
                        .map(|old| {
                            candidate.0 > old.0 + 1e-9
                                || ((candidate.0 - old.0).abs() < 1e-9
                                    && (candidate.1 > old.1 + 1e-9
                                        || ((candidate.1 - old.1).abs() < 1e-9
                                            && (candidate.2, candidate.3) < (old.2, old.3))))
                        })
                        .unwrap_or(true)
                    {
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
                    if best
                        .as_ref()
                        .map(|old| {
                            candidate.0 > old.0 + 1e-9
                                || ((candidate.0 - old.0).abs() < 1e-9
                                    && (candidate.1, candidate.2) < (old.1, old.2))
                        })
                        .unwrap_or(true)
                    {
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

        let mut worked_tiles: Vec<Pos> = cands
            .iter()
            .enumerate()
            .filter(|(i, _)| selected[*i])
            .map(|(_, c)| c.0)
            .collect();
        worked_tiles.sort();
        CitizenPlan {
            strategy,
            worked_tiles,
        }
    }

    fn player_tile_yields(&self, pid: usize, pos: Pos, tile: &crate::world::Tile) -> Yields {
        let mut yields = self.rules.tile_yields(tile);
        match tile.improvement.as_deref() {
            Some("mine") => yields.production += self.tree_effect(pid, "mine_production"),
            Some("pasture") => {
                yields.food += self.tree_effect(pid, "pasture_food");
                yields.production += self.tree_effect(pid, "pasture_production");
            }
            Some("quarry") => yields.production += self.tree_effect(pid, "quarry_production"),
            Some("plantation") => {
                yields.food += self.tree_effect(pid, "plantation_food");
                yields.gold += self.tree_effect(pid, "plantation_gold");
            }
            Some("camp") => {
                yields.food += self.tree_effect(pid, "camp_food");
                yields.production += self.tree_effect(pid, "camp_production");
                yields.gold += self.tree_effect(pid, "camp_gold");
            }
            Some("fishing_boats") => {
                yields.food += self.tree_effect(pid, "fishing_boats_food");
                yields.gold += self.tree_effect(pid, "fishing_boats_gold");
            }
            Some("lumber_mill") => {
                yields.production += self.tree_effect(pid, "lumber_mill_production");
            }
            Some("oil_well" | "offshore_oil_rig") => {
                yields.production += self.tree_effect(pid, "oil_improvement_production");
            }
            Some("farm") => {
                let adjacent_farms = self
                    .nbrs(pos)
                    .iter()
                    .filter(|neighbor| {
                        self.map.tiles[neighbor].improvement.as_deref() == Some("farm")
                            && !self.map.tiles[neighbor].pillaged
                    })
                    .count() as f64;
                yields.food += (adjacent_farms / 2.0).floor()
                    * self.tree_effect(pid, "farm_pair_adjacency_food");
                yields.food += adjacent_farms * self.tree_effect(pid, "farm_adjacency_food");
            }
            _ => {}
        }
        yields
    }

    fn workable_tile_yields(&self, pos: Pos) -> Yields {
        let tile = &self.map.tiles[&pos];
        let owner = tile
            .owner_city
            .and_then(|city| self.cities.get(&city))
            .map(|city| city.owner);
        if !tile.pillaged || tile.improvement.is_none() {
            return owner
                .map(|pid| self.player_tile_yields(pid, pos, tile))
                .unwrap_or_else(|| self.rules.tile_yields(tile));
        }
        let mut unworked = tile.clone();
        unworked.improvement = None;
        owner
            .map(|pid| self.player_tile_yields(pid, pos, &unworked))
            .unwrap_or_else(|| self.rules.tile_yields(&unworked))
    }

    pub fn city_yields(&self, cid: u32) -> Yields {
        let city = &self.cities[&cid];
        let mut ys = Yields::default();
        let mut center = self.workable_tile_yields(city.pos);
        center.food = center.food.max(2.0);
        center.production = center.production.max(1.0);
        ys.add(center);
        for pos in self.city_citizen_plan(cid).worked_tiles {
            ys.add(self.workable_tile_yields(pos));
        }
        for (dname, dpos) in &city.districts {
            if self.map.tiles[dpos].pillaged
                || (self.district_is_family(dname, "encampment") && city.encampment_pillaged)
            {
                continue;
            }
            ys.add(self.district_yields(dname, *dpos));
        }
        for b in &city.buildings {
            if city.pillaged_buildings.contains(b)
                || (city.encampment_pillaged
                    && self.rules.buildings[b.as_str()].district.as_deref() == Some("encampment"))
            {
                continue;
            }
            let building = &self.rules.buildings[b.as_str()];
            let mut yields = if building.regional_range > 0 {
                Yields::default()
            } else {
                building.yields
            };
            if building.regional_range <= 0 && self.city_is_powered(city) {
                Self::add_powered_building_yields(building, &mut yields);
            }
            match building.district.as_deref() {
                Some("campus") => {
                    yields.science *=
                        1.0 + self.policy_effect(city.owner, "campus_building_science_pct") / 100.0;
                }
                Some("commercial_hub") => {
                    yields.gold *= 1.0
                        + self.policy_effect(city.owner, "commercial_building_gold_pct") / 100.0;
                }
                Some("theater_square") => {
                    yields.culture *= 1.0
                        + self.policy_effect(city.owner, "theater_building_culture_pct") / 100.0;
                }
                Some("holy_site") => {
                    yields.faith *= 1.0
                        + self.policy_effect(city.owner, "holy_site_building_faith_pct") / 100.0;
                }
                _ => {}
            }
            if let Some(district) = building.district.as_deref() {
                if let Some(position) = self.city_district_family_position(city, district) {
                    let placed = self.map.tiles[&position]
                        .district
                        .as_deref()
                        .unwrap_or(district);
                    let mut adjacency = self.district_yields(placed, position);
                    let base = self.rules.districts[placed].yields;
                    adjacency.food -= base.food;
                    adjacency.production -= base.production;
                    adjacency.gold -= base.gold;
                    adjacency.science -= base.science;
                    adjacency.culture -= base.culture;
                    adjacency.faith -= base.faith;
                    yields.production += building
                        .effects
                        .get("production_equal_harbor_adjacency")
                        .copied()
                        .unwrap_or(0.0)
                        * adjacency.gold;
                    yields.production += building
                        .effects
                        .get("production_equal_industrial_adjacency")
                        .copied()
                        .unwrap_or(0.0)
                        * adjacency.production;
                    yields.faith += building
                        .effects
                        .get("faith_equal_campus_adjacency")
                        .copied()
                        .unwrap_or(0.0)
                        * adjacency.science;
                    yields.science += building
                        .effects
                        .get("science_equal_harbor_adjacency")
                        .copied()
                        .unwrap_or(0.0)
                        * adjacency.gold;
                    yields.gold += building
                        .effects
                        .get("gold_equal_campus_adjacency")
                        .copied()
                        .unwrap_or(0.0)
                        * adjacency.science;
                    yields.culture += building
                        .effects
                        .get("culture_equal_commercial_adjacency")
                        .copied()
                        .unwrap_or(0.0)
                        * adjacency.gold;
                }
            }
            if self.players[city.owner].civ == "Vietnam" {
                if let Some(family) = building.district.as_deref() {
                    let family = self.district_family(family);
                    if self
                        .rules
                        .districts
                        .get(family)
                        .is_some_and(|district| district.specialty)
                    {
                        if let Some(position) = self.city_district_family_position(city, family) {
                            let amount = if self.world_era >= 4 {
                                3.0
                            } else if self.world_era >= 2 {
                                2.0
                            } else {
                                1.0
                            };
                            match self.map.tiles[&position].feature.as_deref() {
                                Some("forest") => yields.culture += amount,
                                Some("jungle") => yields.science += amount,
                                Some("marsh") => yields.production += amount,
                                _ => {}
                            }
                        }
                    }
                }
            }
            ys.add(yields);
        }
        ys.add(self.regional_building_effects(city).0);
        for wonder in city.wonders.keys() {
            ys.add(self.rules.wonders[wonder.as_str()].yields);
        }
        ys.science += 0.5 * city.pop as f64;
        ys.culture += 0.3 * city.pop as f64;
        if self.city_has_palace(city) {
            ys.add(self.rules.buildings["palace"].yields);
            ys.gold += self.policy_effect(city.owner, "capital_gold");
            ys.faith += self.policy_effect(city.owner, "capital_faith");
        }
        ys.production += self.policy_effect(city.owner, "city_production");
        for r in self.routes.iter().filter(|r| r.origin == cid) {
            if let Some(dc) = self.cities.get(&r.dest) {
                let mut rys = self.route_yields(r.dest, dc.owner == city.owner);
                rys.gold += self.policy_effect(city.owner, "trade_gold");
                rys.food += self.policy_effect(city.owner, "trade_food");
                rys.production += self.policy_effect(city.owner, "trade_production");
                rys.science += self.policy_effect(city.owner, "trade_science");
                rys.culture += self.policy_effect(city.owner, "trade_culture");
                rys.faith += self.policy_effect(city.owner, "trade_faith");
                let government = self.gov_effects(city.owner);
                rys.food += government.trade_food;
                rys.production += government.trade_production;
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
                && self.city_has_district_family(city, "holy_site")
            {
                ys.production += 1.0;
            }
        }
        if self.has_ability(city.owner, "platos_republic") {
            let suz = self
                .players
                .iter()
                .filter(|m| m.is_minor && !m.is_barbarian && m.alive)
                .filter(|m| self.suzerain_of(m.id) == Some(city.owner))
                .count() as f64;
            ys.culture *= 1.0 + 0.05 * suz; // Surrounded by Glory
        }
        match self.players[city.owner].pantheon.as_deref() {
            Some("god_of_the_open_sky") => {
                ys.culture += city
                    .owned_tiles
                    .iter()
                    .filter(|p| self.map.tiles[p].improvement.as_deref() == Some("pasture"))
                    .count() as f64;
            }
            Some("god_of_the_sea") => {
                ys.production += city
                    .owned_tiles
                    .iter()
                    .filter(|p| self.map.tiles[p].improvement.as_deref() == Some("fishing_boats"))
                    .count() as f64;
            }
            _ => {}
        }
        let eff = self.gov_effects(city.owner);
        ys.production += eff.production_per_pop * city.pop as f64;
        ys.faith += eff.faith_per_pop * city.pop as f64;
        ys.culture += eff.culture_per_district * city.districts.len() as f64;
        if self.city_has_palace(city) {
            ys.add(eff.capital_yields);
        }
        ys.production *= 1.0 + eff.production_pct / 100.0;
        ys.science *= 1.0 + eff.science_pct / 100.0;
        ys.gold *= 1.0 + eff.gold_pct / 100.0;
        let suzerains = self
            .players
            .iter()
            .filter(|minor| {
                minor.is_minor
                    && !minor.is_barbarian
                    && self.suzerain_of(minor.id) == Some(city.owner)
            })
            .count() as f64;
        ys.science *=
            1.0 + suzerains * self.policy_effect(city.owner, "science_pct_per_suzerain") / 100.0;
        ys.culture *=
            1.0 + suzerains * self.policy_effect(city.owner, "culture_pct_per_suzerain") / 100.0;
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

    /// The Palace occupies the original capital while it is controlled;
    /// after that city is captured it moves to another owned city. City-states
    /// likewise have a Palace even though their city is not an original
    /// capital for Domination Victory purposes.
    fn city_has_palace(&self, city: &City) -> bool {
        let owns_original_capital = self.cities.values().any(|candidate| {
            candidate.owner == city.owner
                && candidate.original_owner == city.owner
                && candidate.is_capital
        });
        if owns_original_capital {
            city.is_capital && city.original_owner == city.owner
        } else {
            self.player_city_ids(city.owner).into_iter().min() == Some(city.id)
        }
    }

    pub fn valid_improvements(&self, pid: usize, pos: Pos) -> Vec<String> {
        let t = match self.map.get(pos) {
            Some(t) => t,
            None => return vec![],
        };
        if t.district.is_some() || t.wonder.is_some() || self.city_at(pos).is_some() {
            return vec![];
        }
        let oc = match t.owner_city {
            Some(oc) => oc,
            None => return vec![],
        };
        if self.cities[&oc].owner != pid {
            return vec![];
        }
        let visible_resource = t
            .resource
            .as_deref()
            .filter(|resource| self.resource_visible_to(pid, resource));
        let water = self.rules.is_water(t);
        let civ = self.players[pid].civ.as_str();
        let mut out = Vec::new();
        for (name, spec) in &self.rules.improvements {
            if spec.unbuildable
                || !self.unlocked(pid, &spec.tech, &spec.civic)
                || spec.unique_to.as_deref().is_some_and(|owner| owner != civ)
                || water != spec.water
                || t.improvement.as_deref() == Some(name)
                || (spec.requires_hills && !t.hills)
                || (spec.hills_or_resource && !t.hills && visible_resource.is_none())
                || (spec.requires_flat && t.hills)
                || (!spec.terrain.is_empty() && !spec.terrain.contains(&t.terrain))
                || (!spec.feature.is_empty()
                    && !t
                        .feature
                        .as_ref()
                        .is_some_and(|feature| spec.feature.contains(feature)))
            {
                continue;
            }
            match visible_resource {
                Some(resource) => {
                    let stock_improvement = &self.rules.resources[resource].improvement;
                    if !spec.resources.iter().any(|candidate| candidate == resource)
                        && stock_improvement != name
                    {
                        continue;
                    }
                }
                None if spec.resource_only => continue,
                None if t.resource.is_some() => continue, // unrevealed resource
                None => {}
            }
            // Unique replacements suppress their base improvement for that civ.
            if self.rules.improvements.values().any(|candidate| {
                candidate.replaces.as_deref() == Some(name)
                    && candidate.unique_to.as_deref() == Some(civ)
            }) {
                continue;
            }
            out.push(name.clone());
        }
        out.sort();
        out
    }

    pub fn district_sites(&self, cid: u32, dname: &str) -> Vec<Pos> {
        let city = &self.cities[&cid];
        let spec = &self.rules.districts[dname];
        if !spec.buildable
            || spec.max_per_city.is_some_and(|limit| {
                city.districts
                    .keys()
                    .filter(|built| self.district_is_family(built, dname))
                    .count()
                    >= limit
            })
            || spec.max_per_empire.is_some_and(|limit| {
                self.cities
                    .values()
                    .filter(|candidate| candidate.owner == city.owner)
                    .map(|candidate| {
                        candidate
                            .districts
                            .keys()
                            .filter(|built| self.district_is_family(built, dname))
                            .count()
                    })
                    .sum::<usize>()
                    >= limit
            })
            || spec
                .excludes
                .iter()
                .any(|excluded| self.city_has_district_family(city, excluded))
        {
            return Vec::new();
        }
        if spec.specialty {
            let capacity = 1 + (city.pop.max(1) - 1) as usize / 3;
            let built = city
                .districts
                .keys()
                .filter(|name| self.rules.districts[name.as_str()].specialty)
                .count();
            if built >= capacity {
                return Vec::new();
            }
        }
        let mut out = Vec::new();
        for pos in &city.owned_tiles {
            if *pos == city.pos || self.wdist(*pos, city.pos) > 3 {
                continue;
            }
            let t = &self.map.tiles[pos];
            if t.district.is_some() || t.wonder.is_some() || !self.rules.is_passable(t) {
                continue;
            }
            if t.feature.as_ref().is_some_and(|feature| {
                self.rules
                    .features
                    .get(feature.as_str())
                    .is_some_and(|feature| feature.natural_wonder)
            }) {
                continue;
            }
            if t.resource
                .as_ref()
                .is_some_and(|resource| self.rules.resources[resource.as_str()].class != "bonus")
            {
                continue;
            }
            let removal_tech = match t.feature.as_deref() {
                Some("forest") => Some("mining"),
                Some("jungle") => Some("bronze_working"),
                Some("marsh") => Some("irrigation"),
                _ => None,
            };
            let vietnam_specialty = self.players[city.owner].civ == "Vietnam" && spec.specialty;
            if vietnam_specialty
                && !matches!(t.feature.as_deref(), Some("forest" | "jungle" | "marsh"))
            {
                continue;
            }
            if !vietnam_specialty
                && removal_tech.is_some_and(|tech| !self.players[city.owner].techs.contains(tech))
            {
                continue;
            }
            if let Some(resource) = &t.resource {
                let improvement = &self.rules.resources[resource.as_str()].improvement;
                if self.rules.improvements[improvement.as_str()]
                    .tech
                    .as_ref()
                    .is_some_and(|tech| !self.players[city.owner].techs.contains(tech))
                {
                    continue;
                }
            }
            let is_water = self.rules.is_water(t);
            let neighbors = self.nbrs(*pos);
            let adjacent_city = neighbors.contains(&city.pos);
            let adjacent_any_city = neighbors
                .iter()
                .any(|neighbor| self.city_at(*neighbor).is_some());
            let adjacent_land = neighbors.iter().any(|neighbor| {
                self.map
                    .get(*neighbor)
                    .is_some_and(|tile| !self.rules.is_water(tile))
            });
            let adjacent_water = |neighbor: Pos| {
                self.map
                    .get(neighbor)
                    .is_some_and(|tile| self.rules.is_water(tile))
            };
            let valid = match spec.placement.as_str() {
                "coast" => {
                    is_water
                        && matches!(t.terrain.as_str(), "coast" | "lake")
                        && t.feature.as_deref() != Some("reef")
                        && adjacent_land
                }
                "water_park" => {
                    is_water
                        && t.terrain == "coast"
                        && t.feature.as_deref() != Some("reef")
                        && adjacent_land
                }
                "flat_land" => !is_water && !t.hills,
                "hills" => !is_water && t.hills,
                "hills_adjacent_city" => !is_water && t.hills && adjacent_city,
                "not_adjacent_city" => !is_water && !adjacent_any_city,
                "forest" => !is_water && matches!(t.feature.as_deref(), Some("forest" | "jungle")),
                "vietnam_feature" => {
                    !is_water && matches!(t.feature.as_deref(), Some("forest" | "jungle" | "marsh"))
                }
                "aqueduct" => {
                    let center_edge = self.map.direction_to(*pos, city.pos);
                    let river_source = t
                        .river_edges
                        .iter()
                        .enumerate()
                        .any(|(edge, present)| *present && Some(edge) != center_edge);
                    let water_source = river_source
                        || neighbors.iter().any(|neighbor| {
                            self.map.get(*neighbor).is_some_and(|tile| {
                                tile.terrain == "mountain"
                                    || tile.feature.as_deref() == Some("oasis")
                                    || tile.terrain == "lake"
                            })
                        });
                    !is_water && adjacent_city && water_source
                }
                "dam" => {
                    !is_water
                        && matches!(
                            t.feature.as_deref(),
                            Some("floodplains" | "grassland_floodplains" | "plains_floodplains")
                        )
                        && t.river_edges.iter().filter(|edge| **edge).count() >= 2
                }
                "canal" => {
                    let connections: Vec<usize> = neighbors
                        .iter()
                        .enumerate()
                        .filter(|(_, neighbor)| {
                            **neighbor == city.pos || adjacent_water(**neighbor)
                        })
                        .map(|(index, _)| index)
                        .collect();
                    !is_water
                        && !t.hills
                        && connections.iter().any(|a| {
                            connections.iter().any(|b| {
                                let difference = (*a as i32 - *b as i32).abs();
                                difference.min(6 - difference) >= 2
                            })
                        })
                }
                "land" | "" => !is_water && !spec.water,
                _ => self.rules.is_water(t) == spec.water,
            };
            if !valid {
                continue;
            }
            if self.players[city.owner].civ == "Gaul" && spec.specialty && adjacent_city {
                continue;
            }
            out.push(*pos);
        }
        out.sort();
        out
    }

    pub fn wonder_sites(&self, cid: u32, wname: &str) -> Vec<Pos> {
        let city = &self.cities[&cid];
        let spec = &self.rules.wonders[wname];
        if self.wonder_built(wname)
            || !self.unlocked(city.owner, &spec.tech, &spec.civic)
            || spec
                .requires_buildings
                .iter()
                .any(|required| !self.city_has_building_family(city, required))
            || (!spec.requires_any_buildings.is_empty()
                && !spec
                    .requires_any_buildings
                    .iter()
                    .any(|required| self.city_has_building_family(city, required)))
            || (spec.founded_religion && self.players[city.owner].religion.is_none())
        {
            return Vec::new();
        }
        let mut out = Vec::new();
        for pos in &city.owned_tiles {
            if *pos == city.pos || self.wdist(*pos, city.pos) > 3 {
                continue;
            }
            let tile = &self.map.tiles[pos];
            if tile.district.is_some()
                || tile.wonder.is_some()
                || (!self.rules.is_passable(tile) && spec.placement != "mountain")
                || tile
                    .feature
                    .as_ref()
                    .is_some_and(|feature| self.rules.features[feature.as_str()].natural_wonder)
                || tile.resource.as_ref().is_some_and(|resource| {
                    self.rules.resources[resource.as_str()].class != "bonus"
                })
            {
                continue;
            }
            let is_water = self.rules.is_water(tile);
            if is_water != spec.water
                || (!spec.terrain.is_empty() && !spec.terrain.contains(&tile.terrain))
                || (!spec.feature.is_empty()
                    && !tile
                        .feature
                        .as_ref()
                        .is_some_and(|feature| spec.feature.contains(feature)))
                || spec.hills.is_some_and(|hills| tile.hills != hills)
                || (spec.river && !tile.has_river())
            {
                continue;
            }
            let neighbors = self.nbrs(*pos);
            if spec.coast {
                let valid_coast = if spec.water {
                    tile.terrain == "coast"
                        && neighbors.iter().any(|neighbor| {
                            self.map
                                .get(*neighbor)
                                .is_some_and(|candidate| !self.rules.is_water(candidate))
                        })
                } else {
                    neighbors.iter().any(|neighbor| {
                        self.map
                            .get(*neighbor)
                            .is_some_and(|candidate| candidate.terrain == "coast")
                    })
                };
                if !valid_coast {
                    continue;
                }
            }
            if spec.adjacent_mountain
                && !neighbors.iter().any(|neighbor| {
                    self.map
                        .get(*neighbor)
                        .is_some_and(|candidate| candidate.terrain == "mountain")
                })
            {
                continue;
            }
            if spec.adjacent_district.as_ref().is_some_and(|required| {
                !neighbors.iter().any(|neighbor| {
                    if required == "city_center" {
                        return self.city_at(*neighbor).is_some();
                    }
                    self.map.get(*neighbor).is_some_and(|candidate| {
                        candidate
                            .district
                            .as_deref()
                            .is_some_and(|district| self.district_is_family(district, required))
                    })
                })
            }) {
                continue;
            }
            if spec.adjacent_resource.as_ref().is_some_and(|required| {
                !neighbors.iter().any(|neighbor| {
                    self.map
                        .get(*neighbor)
                        .is_some_and(|candidate| candidate.resource.as_ref() == Some(required))
                })
            }) {
                continue;
            }
            if spec.adjacent_improvement.as_ref().is_some_and(|required| {
                !neighbors.iter().any(|neighbor| {
                    self.map
                        .get(*neighbor)
                        .is_some_and(|candidate| candidate.improvement.as_ref() == Some(required))
                })
            }) {
                continue;
            }
            let special_valid = match spec.placement.as_str() {
                "adjacent_capital" => neighbors.iter().any(|neighbor| {
                    self.city_at(*neighbor).is_some_and(|candidate| {
                        self.cities[&candidate].owner == city.owner
                            && self.cities[&candidate].is_capital
                    })
                }),
                "panama_canal" => {
                    let connections: Vec<usize> = neighbors
                        .iter()
                        .enumerate()
                        .filter(|(_, neighbor)| {
                            **neighbor == city.pos
                                || self.map.get(**neighbor).is_some_and(|candidate| {
                                    self.rules.is_water(candidate)
                                        || self.district_is_family(
                                            candidate.district.as_deref().unwrap_or(""),
                                            "canal",
                                        )
                                })
                        })
                        .map(|(index, _)| index)
                        .collect();
                    !tile.hills
                        && connections.iter().any(|a| {
                            connections.iter().any(|b| {
                                let difference = (*a as i32 - *b as i32).abs();
                                difference.min(6 - difference) >= 2
                            })
                        })
                }
                "golden_gate_bridge" => {
                    let land_sides: Vec<usize> = neighbors
                        .iter()
                        .enumerate()
                        .filter(|(_, neighbor)| {
                            self.map
                                .get(**neighbor)
                                .is_some_and(|candidate| !self.rules.is_water(candidate))
                        })
                        .map(|(index, _)| index)
                        .collect();
                    land_sides.iter().any(|a| {
                        land_sides
                            .iter()
                            .any(|b| (*a as i32 - *b as i32).abs() == 3)
                    })
                }
                "lake_adjacent_land" => neighbors.iter().any(|neighbor| {
                    self.map
                        .get(*neighbor)
                        .is_some_and(|candidate| !self.rules.is_water(candidate))
                }),
                _ => true,
            };
            if special_valid {
                out.push(*pos);
            }
        }
        out.sort();
        out
    }

    pub fn item_cost(&self, item: &Item) -> f64 {
        match item {
            Item::Unit { unit } => self.rules.units[unit.as_str()].cost,
            Item::Building { building } => self.rules.buildings[building.as_str()].cost,
            Item::District { district, .. } => self.rules.districts[district.as_str()].cost,
            Item::Wonder { wonder, .. } => self.rules.wonders[wonder.as_str()].cost,
            Item::Repair { repair, pos } => {
                if repair == "district" {
                    self.map
                        .get(*pos)
                        .and_then(|tile| tile.district.as_ref())
                        .and_then(|district| self.rules.districts.get(district))
                        .map(|spec| spec.cost * 0.25)
                        .unwrap_or(1.0)
                } else {
                    self.rules
                        .buildings
                        .get(repair)
                        .map(|spec| spec.cost * 0.25)
                        .unwrap_or(1.0)
                }
            }
            Item::Project { project } => self.rules.projects[project.as_str()].cost,
        }
    }

    pub fn item_cost_for(&self, pid: usize, item: &Item) -> f64 {
        let base = self.item_cost(item);
        match item {
            Item::Unit { unit } if unit == "settler" => {
                base + 30.0
                    * self.players[pid]
                        .counters
                        .get("trained:settler")
                        .copied()
                        .unwrap_or(0) as f64
            }
            Item::Unit { unit } if unit == "builder" => {
                base + 4.0
                    * self.players[pid]
                        .counters
                        .get("trained:builder")
                        .copied()
                        .unwrap_or(0) as f64
            }
            _ => base,
        }
    }

    fn item_progress_key(item: &Item) -> String {
        match item {
            Item::Unit { unit } => format!("unit:{unit}"),
            Item::Building { building } => format!("building:{building}"),
            Item::District { district, pos } => {
                format!("district:{district}:{},{}", pos.0, pos.1)
            }
            Item::Wonder { wonder, pos } => format!("wonder:{wonder}:{},{}", pos.0, pos.1),
            Item::Repair { repair, pos } => {
                format!("repair:{repair}:{},{}", pos.0, pos.1)
            }
            Item::Project { project } => format!("project:{project}"),
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
                if unit == "settler" && city.pop < 2 {
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
                        && s.unique_to.as_deref() == Some(self.players[pid].civ.as_str())
                });
                if replaced {
                    return false;
                }
                if let Some(res) = &spec.requires_resource {
                    if !self.has_resource(pid, res) {
                        return false;
                    }
                }
                if spec
                    .requires_building
                    .as_ref()
                    .is_some_and(|building| !self.city_has_building_family(city, building))
                    || spec
                        .requires_district
                        .as_ref()
                        .is_some_and(|district| !self.city_has_district_family(city, district))
                {
                    return false;
                }
                if spec.domain.as_deref() == Some("sea") {
                    let coastal = self.nbrs(city.pos).iter().any(|n| {
                        self.map
                            .get(*n)
                            .map(|t| self.rules.is_water(t))
                            .unwrap_or(false)
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
                if city.buildings.contains(building)
                    || !self.unlocked(pid, &spec.tech, &spec.civic)
                    || !spec.buildable
                    || spec.purchase_only
                {
                    return false;
                }
                if spec
                    .unique_to
                    .as_ref()
                    .is_some_and(|civ| self.players[pid].civ != *civ)
                    || self.rules.buildings.values().any(|candidate| {
                        candidate.replaces.as_deref() == Some(building.as_str())
                            && candidate.unique_to.as_deref()
                                == Some(self.players[pid].civ.as_str())
                    })
                    || !spec
                        .requires
                        .iter()
                        .all(|required| self.city_has_building_family(city, required))
                    || (!spec.requires_any.is_empty()
                        && !spec
                            .requires_any
                            .iter()
                            .any(|required| self.city_has_building_family(city, required)))
                    || spec
                        .excludes
                        .iter()
                        .any(|excluded| self.city_has_building_family(city, excluded))
                {
                    return false;
                }
                if spec.outer_defense > 0
                    && (!spec
                        .requires
                        .iter()
                        .all(|required| self.city_has_building_family(city, required))
                        || city.wall_hp < self.city_max_wall_hp(city))
                {
                    return false;
                }
                if spec.wonder && self.wonder_built(building) {
                    return false; // one per world
                }
                if spec.coastal {
                    let ok = self.nbrs(city.pos).iter().any(|n| {
                        self.map
                            .get(*n)
                            .map(|t| self.rules.is_water(t))
                            .unwrap_or(false)
                    });
                    if !ok {
                        return false;
                    }
                }
                if spec
                    .effects
                    .get("requires_river_city")
                    .copied()
                    .unwrap_or(0.0)
                    > 0.0
                    && !self.map.tiles[&city.pos].has_river()
                {
                    return false;
                }
                if spec
                    .effects
                    .get("requires_fresh_water_city")
                    .copied()
                    .unwrap_or(0.0)
                    > 0.0
                {
                    let fresh = self.map.tiles[&city.pos].has_river()
                        || self.nbrs(city.pos).iter().any(|neighbor| {
                            self.map.get(*neighbor).is_some_and(|tile| {
                                tile.terrain == "lake" || tile.feature.as_deref() == Some("oasis")
                            })
                        });
                    if !fresh {
                        return false;
                    }
                }
                match &spec.district {
                    None => true,
                    Some(d) => self.city_has_district_family(city, d),
                }
            }
            Item::District { district, pos } => {
                let spec = match self.rules.districts.get(district) {
                    Some(s) => s,
                    None => return false,
                };
                if !self.unlocked(pid, &spec.tech, &spec.civic)
                    || !spec.buildable
                    || spec
                        .unique_to
                        .as_ref()
                        .is_some_and(|civ| self.players[pid].civ != *civ)
                    || self.rules.districts.values().any(|candidate| {
                        candidate.replaces.as_deref() == Some(district.as_str())
                            && candidate.unique_to.as_deref()
                                == Some(self.players[pid].civ.as_str())
                    })
                {
                    return false;
                }
                self.district_sites(cid, district).contains(pos)
            }
            Item::Wonder { wonder, pos } => {
                let Some(spec) = self.rules.wonders.get(wonder) else {
                    return false;
                };
                !self.wonder_built(wonder)
                    && self.unlocked(pid, &spec.tech, &spec.civic)
                    && self.wonder_sites(cid, wonder).contains(pos)
            }
            Item::Repair { repair, pos } => {
                let Some(tile) = self.map.get(*pos) else {
                    return false;
                };
                if tile.owner_city != Some(cid) || tile.district.is_none() {
                    return false;
                }
                if repair == "district" {
                    tile.pillaged
                } else {
                    city.pillaged_buildings.contains(repair)
                        && self.rules.buildings.get(repair).is_some_and(|building| {
                            building.district.as_ref().is_some_and(|family| {
                                tile.district.as_deref().is_some_and(|district| {
                                    self.district_is_family(district, family)
                                })
                            })
                        })
                }
            }
            Item::Project { project } => {
                if project == "repair_outer_defenses" {
                    let max = self.city_max_wall_hp(city);
                    return max > 0
                        && city.wall_hp < max
                        && self.turn.saturating_sub(city.last_attacked) >= 3;
                }
                if project == "repair_encampment" {
                    let max_wall = self.city_max_wall_hp(city);
                    return self.city_has_district_family(city, "encampment")
                        && (city.encampment_pillaged
                            || city.encampment_hp < 100
                            || city.encampment_wall_hp < max_wall)
                        && self.turn.saturating_sub(city.encampment_last_attacked) >= 3;
                }
                let spec = match self.rules.projects.get(project) {
                    Some(s) => s,
                    None => return false,
                };
                if self.players[pid].is_minor || self.players[pid].is_barbarian {
                    return false;
                }
                if spec
                    .tech
                    .as_ref()
                    .is_some_and(|t| !self.players[pid].techs.contains(t))
                {
                    return false;
                }
                if spec
                    .civic
                    .as_ref()
                    .is_some_and(|c| !self.players[pid].civics.contains(c))
                {
                    return false;
                }
                if spec
                    .district
                    .as_ref()
                    .is_some_and(|d| !self.city_has_district_family(city, d))
                {
                    return false;
                }
                if !spec
                    .requires
                    .iter()
                    .all(|required| self.players[pid].science_projects.contains(required))
                {
                    return false;
                }
                if !spec
                    .requires_buildings
                    .iter()
                    .all(|building| self.city_has_building_family(city, building))
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
            let it = Item::Building {
                building: name.clone(),
            };
            if self.can_produce(pid, cid, &it) {
                items.push(it);
            }
        }
        for name in self.rules.wonders.keys() {
            let mut sites = self.wonder_sites(cid, name);
            sites.sort();
            for pos in sites.into_iter().take(2) {
                let item = Item::Wonder {
                    wonder: name.clone(),
                    pos,
                };
                if self.can_produce(pid, cid, &item) {
                    items.push(item);
                }
            }
        }
        let city = &self.cities[&cid];
        for (_, pos) in &city.districts {
            let district = self.map.tiles[pos].district.as_deref().unwrap_or("");
            let district_repair = Item::Repair {
                repair: "district".to_string(),
                pos: *pos,
            };
            if self.can_produce(pid, cid, &district_repair) {
                items.push(district_repair);
            }
            for building in &city.pillaged_buildings {
                let matches_district = self.rules.buildings.get(building).is_some_and(|spec| {
                    spec.district
                        .as_ref()
                        .is_some_and(|family| self.district_is_family(district, family))
                });
                if matches_district {
                    let repair = Item::Repair {
                        repair: building.clone(),
                        pos: *pos,
                    };
                    if self.can_produce(pid, cid, &repair) {
                        items.push(repair);
                    }
                }
            }
        }
        for name in self.rules.projects.keys() {
            let it = Item::Project {
                project: name.clone(),
            };
            if self.can_produce(pid, cid, &it) {
                items.push(it);
            }
        }
        for (name, spec) in &self.rules.districts {
            if !spec.buildable || !self.unlocked(pid, &spec.tech, &spec.civic) {
                continue;
            }
            let mut sites = self.district_sites(cid, name);
            sites.sort_by(|a, b| {
                let ya = self.district_yields(name, *a).total();
                let yb = self.district_yields(name, *b).total();
                yb.partial_cmp(&ya).unwrap().then(a.cmp(b))
            });
            for s in sites.into_iter().take(2) {
                let item = Item::District {
                    district: name.clone(),
                    pos: s,
                };
                if self.can_produce(pid, cid, &item) {
                    items.push(item);
                }
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
            if self.noncombat_action_blocked_by_zoc(uid) {
                continue;
            }
            for promotion in self.available_promotions(uid) {
                acts.push(Action::Promote {
                    unit: uid,
                    promotion,
                });
            }
            if spec.domain.as_deref() == Some("air") {
                if u.moves_left > 0.0 {
                    let range = spec.range.max(1);
                    for target in self.wdisk(u.pos, range) {
                        if target != u.pos && self.enemy_combat_target_at(pid, target) {
                            acts.push(Action::AirStrike { unit: uid, target });
                        }
                    }
                    for base in self.wdisk(u.pos, range * 2) {
                        if base != u.pos && self.can_air_base_at(pid, base, Some(uid)) {
                            acts.push(Action::AirRebase {
                                unit: uid,
                                to: base,
                            });
                        }
                    }
                    if !spec.siege && u.attacks_left > 0 {
                        acts.push(Action::AirPatrol { unit: uid });
                    }
                }
                continue;
            }
            if u.moves_left > 0.0 {
                for n in self.nbrs(u.pos) {
                    if self.can_move(uid, n) {
                        acts.push(Action::Move { unit: uid, to: n });
                    }
                }
                if spec.class == "military" && !embarked {
                    if spec.has_ranged_attack() {
                        if u.attacks_left > 0
                            && (!spec.siege
                                || !u.moved
                                || self.promotion_effect(&u, "attack_after_move") > 0.0)
                        {
                            let range =
                                spec.range.max(1) + self.promotion_effect(&u, "range") as i32;
                            for pos in self.wdisk(u.pos, range) {
                                if pos == u.pos || !self.map.tiles.contains_key(&pos) {
                                    continue;
                                }
                                if self.enemy_combat_target_at(pid, pos)
                                    && self.unit_has_line_of_sight(uid, pos)
                                {
                                    acts.push(Action::Ranged {
                                        unit: uid,
                                        target: pos,
                                    });
                                }
                            }
                        }
                    } else if u.attacks_left > 0 {
                        for pos in self.nbrs(u.pos) {
                            if self.map.tiles.contains_key(&pos)
                                && self.enemy_combat_target_at(pid, pos)
                                && self.unit_can_melee_target_domain(uid, pos)
                                && self.can_pay_melee_entry(uid, pos)
                            {
                                acts.push(Action::Attack {
                                    unit: uid,
                                    target: pos,
                                });
                            }
                        }
                    }
                }
            }
            if u.kind == "settler" && self.can_found_city(uid) {
                acts.push(Action::FoundCity { unit: uid });
            }
            if (u.kind == "builder" || !spec.builds.is_empty()) && u.charges > 0 {
                for imp in self.valid_improvements(pid, u.pos) {
                    if (u.kind == "builder"
                        && !self.rules.improvements[imp.as_str()].builder_buildable)
                        || (u.kind != "builder" && !spec.builds.contains(&imp))
                    {
                        continue;
                    }
                    acts.push(Action::Improve {
                        unit: uid,
                        improvement: imp,
                    });
                }
            }
            if u.kind == "builder"
                && u.moves_left > 0.0
                && self.map.tiles[&u.pos].pillaged
                && self.map.tiles[&u.pos].improvement.is_some()
                && self.map.tiles[&u.pos]
                    .owner_city
                    .and_then(|cid| self.cities.get(&cid))
                    .is_some_and(|city| city.owner == pid)
            {
                acts.push(Action::RepairImprovement { unit: uid });
            }
            if spec.class == "military"
                && u.moves_left > 0.0
                && !embarked
                && self.pillageable_at(pid, u.pos)
            {
                acts.push(Action::Pillage { unit: uid });
            }
            if spec.promotion_class == "naval_raider" && u.moves_left > 0.0 && u.attacks_left > 0 {
                for target in self.nbrs(u.pos) {
                    if self.pillageable_at(pid, target) {
                        acts.push(Action::CoastalRaid { unit: uid, target });
                    }
                }
            }
            if u.linked_to.is_some() {
                acts.push(Action::UnlinkUnits { unit: uid });
            }
            if spec.religious_spread > 0.0 && u.charges > 0 && u.moves_left > 0.0 {
                let near_city = self.city_at(u.pos).is_some()
                    || self
                        .nbrs(u.pos)
                        .iter()
                        .any(|position| self.city_at(*position).is_some());
                if near_city {
                    acts.push(Action::Spread { unit: uid });
                }
            }
            if matches!(u.kind.as_str(), "apostle" | "inquisitor")
                && u.moves_left > 0.0
                && u.attacks_left > 0
            {
                for target in self.nbrs(u.pos) {
                    let rival = self.units_at(target).into_iter().any(|id| {
                        let other = &self.units[&id];
                        self.rules.units[other.kind.as_str()].class == "religious"
                            && other.owner != pid
                            && other.religion.is_some()
                            && u.religion.is_some()
                            && other.religion != u.religion
                    });
                    if rival {
                        acts.push(Action::TheologicalAttack { unit: uid, target });
                    }
                }
            }
            if spec.class == "military" && u.moves_left > 0.0 {
                for target_unit in self.units_at(u.pos) {
                    let target = &self.units[&target_unit];
                    if target.owner != pid
                        && self.is_at_war(pid, target.owner)
                        && self.rules.units[target.kind.as_str()].class == "religious"
                    {
                        acts.push(Action::CondemnHeretic {
                            unit: uid,
                            target_unit,
                        });
                    }
                }
            }
            if u.kind == "guru" && u.charges > 0 && u.moves_left > 0.0 {
                let damaged = self
                    .wdisk(u.pos, 1)
                    .into_iter()
                    .flat_map(|pos| self.units_at(pos))
                    .any(|id| self.units[&id].religion == u.religion && self.units[&id].hp < 100);
                if damaged {
                    acts.push(Action::HealReligious { unit: uid });
                }
            }
            if u.kind == "inquisitor"
                && u.charges > 0
                && u.moves_left > 0.0
                && self
                    .city_at(u.pos)
                    .is_some_and(|cid| self.cities[&cid].owner == pid)
            {
                acts.push(Action::RemoveHeresy { unit: uid });
            }
            if u.kind == "apostle"
                && u.moves_left > 0.0
                && p.counters.get("inquisition").copied().unwrap_or(0) == 0
                && p.holy_city
                    .and_then(|cid| self.cities.get(&cid))
                    .is_some_and(|city| self.wdist(u.pos, city.pos) <= 1)
            {
                acts.push(Action::LaunchInquisition { unit: uid });
            }
        }
        let owned_units = self.player_unit_ids(pid);
        for (index, &uid) in owned_units.iter().enumerate() {
            for &other in &owned_units[index + 1..] {
                if self.can_combine_units(pid, uid, other).is_some() {
                    acts.push(Action::CombineUnits {
                        unit: uid,
                        with: other,
                    });
                }
                if self.can_link_units(pid, uid, other) {
                    let uid_military =
                        self.rules.units[self.units[&uid].kind.as_str()].class == "military";
                    let (unit, with) = if uid_military {
                        (uid, other)
                    } else {
                        (other, uid)
                    };
                    acts.push(Action::LinkUnits { unit, with });
                }
            }
        }
        for cid in self.player_city_ids(pid) {
            for item in self.producible_items(pid, cid) {
                acts.push(Action::Produce { city: cid, item });
            }
            for utype in ["builder", "settler", "warrior", "archer", "spearman"] {
                let it = Item::Unit {
                    unit: utype.to_string(),
                };
                if self.can_produce(pid, cid, &it) {
                    let cost = self.item_cost_for(pid, &it);
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
                        o.owner != pid
                            && self.is_at_war(pid, o.owner)
                            && self.rules.units[o.kind.as_str()].class == "military"
                    });
                    if hit
                        && self.city_at(pos).is_none()
                        && self.encampment_at(pos).is_none()
                        && self.has_line_of_sight(cpos, pos, true)
                    {
                        acts.push(Action::CityStrike {
                            city: cid,
                            target: pos,
                        });
                    }
                }
            }
            let city = &self.cities[&cid];
            if city.encampment_hp > 0
                && city.encampment_wall_hp > 0
                && !city.encampment_pillaged
                && !city.encampment_struck
            {
                let Some(source) = self.city_district_family_position(city, "encampment") else {
                    continue;
                };
                for pos in self.wdisk(source, 2) {
                    let hit = self.units_at(pos).into_iter().any(|id| {
                        let other = &self.units[&id];
                        other.owner != pid
                            && self.is_at_war(pid, other.owner)
                            && self.rules.units[other.kind.as_str()].class == "military"
                    });
                    if hit
                        && self.city_at(pos).is_none()
                        && self.encampment_at(pos).is_none()
                        && self.has_line_of_sight(source, pos, true)
                    {
                        acts.push(Action::EncampmentStrike {
                            city: cid,
                            target: pos,
                        });
                    }
                }
            }
        }
        if !p.is_minor {
            for (g, spec) in &self.rules.governments {
                let ok = spec
                    .civic
                    .as_ref()
                    .map(|c| p.civics.contains(c))
                    .unwrap_or(true);
                if ok && p.government.as_deref() != Some(g.as_str()) {
                    acts.push(Action::Government {
                        government: g.clone(),
                    });
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
                acts.push(Action::UnslotPolicy {
                    policy: card.clone(),
                });
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
                        if *dest == origin
                            || self.is_at_war(pid, dc.owner)
                            || self.wdist(self.cities[&origin].pos, dc.pos) > 15
                            || self
                                .routes
                                .iter()
                                .any(|r| r.origin == origin && r.dest == *dest)
                        {
                            continue;
                        }
                        acts.push(Action::TradeRoute {
                            unit: uid,
                            city: *dest,
                        });
                    }
                }
            }
            if p.envoys_free > 0 {
                for m in &self.players {
                    if m.is_minor && !m.is_barbarian && m.alive && !self.is_at_war(pid, m.id) {
                        acts.push(Action::SendEnvoy { player: m.id });
                    }
                }
            }
            let gp_kinds: BTreeSet<String> = self
                .rules
                .great_people
                .values()
                .map(|person| person.kind.clone())
                .collect();
            for kind in gp_kinds {
                let Some((_, person)) = self.current_great_person(&kind) else {
                    continue;
                };
                let points = p.gpp.get(&kind).copied().unwrap_or(0.0);
                let missing = (person.cost - points).max(0.0);
                if missing <= 0.0 {
                    acts.push(Action::RecruitGreatPerson { kind });
                } else {
                    if p.gold >= missing * 15.0 {
                        acts.push(Action::PatronizeGreatPerson {
                            kind: kind.clone(),
                            currency: "gold".to_string(),
                        });
                    }
                    let discount =
                        self.empire_wonder_effect(pid, "great_person_faith_patronage_discount_pct");
                    if p.faith >= missing * 10.0 * (1.0 - discount / 100.0) {
                        acts.push(Action::PatronizeGreatPerson {
                            kind,
                            currency: "faith".to_string(),
                        });
                    }
                }
            }
            if p.pantheon.is_none() && p.faith >= 25.0 {
                for b in self.rules.beliefs.pantheon.keys() {
                    if !self
                        .players
                        .iter()
                        .any(|o| o.pantheon.as_deref() == Some(b.as_str()))
                    {
                        acts.push(Action::ChoosePantheon { belief: b.clone() });
                    }
                }
            }
            if p.prophet_pending && self.religions_founded() < self.max_religions() {
                let taken = |b: &str| {
                    self.players
                        .iter()
                        .any(|o| o.religion_beliefs.iter().any(|x| x == b))
                };
                for fo in self.rules.beliefs.follower.keys().filter(|b| !taken(b)) {
                    for fu in self.rules.beliefs.founder.keys().filter(|b| !taken(b)) {
                        acts.push(Action::FoundReligion {
                            follower: fo.clone(),
                            founder: fu.clone(),
                        });
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
            for cid in self.player_city_ids(pid) {
                for unit in ["missionary", "apostle", "guru", "inquisitor"] {
                    let spec = &self.rules.units[unit];
                    let building = spec
                        .requires_building
                        .as_ref()
                        .is_none_or(|name| self.city_has_building_family(&self.cities[&cid], name));
                    let inquisition = unit != "inquisitor"
                        || p.counters.get("inquisition").copied().unwrap_or(0) > 0;
                    let cost = self.item_cost_for(
                        pid,
                        &Item::Unit {
                            unit: unit.to_string(),
                        },
                    ) * 2.0;
                    if building
                        && inquisition
                        && p.faith >= cost
                        && self.unlocked(pid, &spec.tech, &spec.civic)
                        && self.city_has_district_family(&self.cities[&cid], "holy_site")
                        && self.city_religion(&self.cities[&cid]).is_some()
                    {
                        acts.push(Action::Buy {
                            city: cid,
                            unit: unit.to_string(),
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
        if let Some(cid) = self.encampment_at(pos) {
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
        let blocked_unit = match action {
            Action::Move { unit, .. }
            | Action::MoveTo { unit, .. }
            | Action::Attack { unit, .. }
            | Action::Ranged { unit, .. }
            | Action::FoundCity { unit }
            | Action::Improve { unit, .. }
            | Action::Pillage { unit }
            | Action::RepairImprovement { unit }
            | Action::CoastalRaid { unit, .. }
            | Action::AirRebase { unit, .. }
            | Action::AirStrike { unit, .. }
            | Action::AirPatrol { unit }
            | Action::Fortify { unit }
            | Action::Promote { unit, .. }
            | Action::UnlinkUnits { unit }
            | Action::TradeRoute { unit, .. }
            | Action::Spread { unit }
            | Action::TheologicalAttack { unit, .. }
            | Action::CondemnHeretic { unit, .. }
            | Action::HealReligious { unit }
            | Action::RemoveHeresy { unit }
            | Action::LaunchInquisition { unit } => self.noncombat_action_blocked_by_zoc(*unit),
            Action::CombineUnits { unit, with } | Action::LinkUnits { unit, with } => {
                self.noncombat_action_blocked_by_zoc(*unit)
                    || self.noncombat_action_blocked_by_zoc(*with)
            }
            _ => false,
        };
        if blocked_unit {
            return Err("non-combat unit cannot act after entering zone of control".into());
        }
        let r = match action {
            Action::Move { unit, to } => self.do_move(pid, *unit, *to),
            Action::MoveTo { unit, to } => self.do_move_to(pid, *unit, *to),
            Action::Attack { unit, target } => self.do_attack(pid, *unit, *target),
            Action::Ranged { unit, target } => self.do_ranged(pid, *unit, *target),
            Action::FoundCity { unit } => self.do_found_city(pid, *unit),
            Action::Improve { unit, improvement } => self.do_improve(pid, *unit, improvement),
            Action::Pillage { unit } => self.do_pillage(pid, *unit),
            Action::RepairImprovement { unit } => self.do_repair_improvement(pid, *unit),
            Action::CoastalRaid { unit, target } => self.do_coastal_raid(pid, *unit, *target),
            Action::AirRebase { unit, to } => self.do_air_rebase(pid, *unit, *to),
            Action::AirStrike { unit, target } => self.do_air_strike(pid, *unit, *target),
            Action::AirPatrol { unit } => self.do_air_patrol(pid, *unit),
            Action::Produce { city, item } => self.do_produce(pid, *city, item),
            Action::Buy {
                city,
                unit,
                currency,
            } => self.do_buy(pid, *city, unit, currency),
            Action::Research { tech } => self.do_research(pid, tech),
            Action::Civic { civic } => self.do_civic(pid, civic),
            Action::DeclareWar { player } => self.do_declare_war(pid, *player),
            Action::MakePeace { player } => self.do_make_peace(pid, *player),
            Action::Fortify { unit } => self.do_fortify(pid, *unit),
            Action::Promote { unit, promotion } => self.do_promote(pid, *unit, promotion),
            Action::CombineUnits { unit, with } => self.do_combine_units(pid, *unit, *with),
            Action::LinkUnits { unit, with } => self.do_link_units(pid, *unit, *with),
            Action::UnlinkUnits { unit } => self.do_unlink_units(pid, *unit),
            Action::Government { government } => self.do_government(pid, government),
            Action::SlotPolicy { policy } => self.do_slot_policy(pid, policy),
            Action::UnslotPolicy { policy } => self.do_unslot_policy(pid, policy),
            Action::TradeRoute { unit, city } => self.do_trade_route(pid, *unit, *city),
            Action::SendEnvoy { player } => self.do_send_envoy(pid, *player),
            Action::RecruitGreatPerson { kind } => self.do_recruit_great_person(pid, kind),
            Action::PatronizeGreatPerson { kind, currency } => {
                self.do_patronize_great_person(pid, kind, currency)
            }
            Action::ChoosePantheon { belief } => self.do_choose_pantheon(pid, belief),
            Action::AssignGovernor { city } => self.do_assign_governor(pid, *city),
            Action::FoundReligion { follower, founder } => {
                self.do_found_religion(pid, follower, founder)
            }
            Action::Spread { unit } => self.do_spread(pid, *unit),
            Action::TheologicalAttack { unit, target } => {
                self.do_theological_attack(pid, *unit, *target)
            }
            Action::CondemnHeretic { unit, target_unit } => {
                self.do_condemn_heretic(pid, *unit, *target_unit)
            }
            Action::HealReligious { unit } => self.do_heal_religious(pid, *unit),
            Action::RemoveHeresy { unit } => self.do_remove_heresy(pid, *unit),
            Action::LaunchInquisition { unit } => self.do_launch_inquisition(pid, *unit),
            Action::CityStrike { city, target } => self.do_city_strike(pid, *city, *target),
            Action::EncampmentStrike { city, target } => {
                self.do_encampment_strike(pid, *city, *target)
            }
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
        if self.formation_movement_locked_by_zoc(uid) {
            return Err("stopped by zone of control".into());
        }
        if !self.can_move(uid, to) {
            return Err("invalid move".into());
        }
        self.resolve_entered_units(uid, to);
        let cost = self.unit_step_cost(uid, u.pos, to);
        let linked = if self.is_linked_leader(uid) {
            u.linked_to
        } else {
            None
        };
        let carrier_aircraft: Vec<u32> = if u.kind == "aircraft_carrier" {
            self.units_at(u.pos)
                .into_iter()
                .filter(|other| {
                    *other != uid
                        && self.units[other].owner == pid
                        && self.rules.units[self.units[other].kind.as_str()]
                            .domain
                            .as_deref()
                            == Some("air")
                })
                .collect()
        } else {
            Vec::new()
        };
        {
            let mu = self.units.get_mut(&uid).unwrap();
            mu.fortified = false;
            mu.fortify_turns = 0;
            mu.acted = true;
            mu.moved = true;
        }
        self.relocate(uid, to);
        for aircraft in carrier_aircraft {
            self.relocate(aircraft, to);
        }
        if let Some(peer) = linked {
            self.relocate(peer, to);
            let escort_speed = self.promotion_effect(&self.units[&uid], "escort_mobility") > 0.0;
            let peer_cost = if escort_speed {
                cost
            } else {
                self.unit_step_cost(peer, u.pos, to)
            };
            let passenger = self.units.get_mut(&peer).unwrap();
            passenger.moves_left = (passenger.moves_left - peer_cost).max(0.0);
            passenger.acted = true;
            passenger.moved = true;
        }
        let remaining = (self.units[&uid].moves_left - cost).max(0.0);
        self.units.get_mut(&uid).unwrap().moves_left = remaining;
        if self.formation_enters_enemy_zoc(uid, to) {
            // A linked formation stops when either member is affected. Apply
            // class-specific movement loss to both occupants so a passenger
            // cannot unlink and act after being dragged into ZOC.
            self.stop_unit_by_zoc(uid);
            if let Some(peer) = linked {
                self.stop_unit_by_zoc(peer);
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
        if u.kind == "hoplite"
            && self.nbrs(u.pos).into_iter().any(|p| {
                self.units_at(p).into_iter().any(|id| {
                    id != uid
                        && self.units[&id].owner == u.owner
                        && self.units[&id].kind == "hoplite"
                })
            })
        {
            bonus += 10.0;
        }
        if attacking && self.has_ability(u.owner, "killer_of_cyrus") && opponent.hp < 100 {
            bonus += 5.0;
        }
        if attacking {
            if matches!(other.promotion_class.as_str(), "melee" | "ranged") {
                bonus += self.promotion_effect(u, "attack_vs_melee_ranged");
            }
            if other.promotion_class == "melee" {
                bonus += self.promotion_effect(u, "vs_melee");
            }
            if other.promotion_class == "anti_cavalry" {
                bonus += self.promotion_effect(u, "vs_anti_cavalry");
            }
            if matches!(
                other.promotion_class.as_str(),
                "light_cavalry" | "heavy_cavalry"
            ) {
                bonus += self.promotion_effect(u, "vs_cavalry");
            }
            if matches!(other.promotion_class.as_str(), "ranged" | "siege") {
                bonus += self.promotion_effect(u, "attack_vs_ranged_siege");
            }
            if other.promotion_class == "siege" {
                bonus += self.promotion_effect(u, "vs_siege");
            }
            if other.promotion_class == "heavy_cavalry" {
                bonus += self.promotion_effect(u, "vs_heavy_cavalry");
            }
            if opponent.hp < 100 {
                bonus += self.promotion_effect(u, "vs_damaged");
            }
            if opponent.fortify_turns > 0 {
                bonus += self.promotion_effect(u, "vs_fortified");
            }
            let tile = &self.map.tiles[&opponent.pos];
            if self.city_at(opponent.pos).is_some() || tile.district.is_some() {
                bonus += self.promotion_effect(u, "vs_unit_in_district");
                bonus += self.promotion_effect(u, "district_melee");
            }
            if other.domain.as_deref() == Some("sea") {
                bonus += self.promotion_effect(u, "vs_naval");
            }
            if other.promotion_class == "naval_raider" {
                bonus += self.promotion_effect(u, "vs_naval_raider");
            }
        } else {
            if other.promotion_class == "melee" {
                bonus += self.promotion_effect(u, "defend_melee");
            }
            if matches!(
                other.promotion_class.as_str(),
                "heavy_cavalry" | "anti_cavalry"
            ) {
                bonus += self.promotion_effect(u, "defend_heavy_anti");
            }
            let tile = &self.map.tiles[&u.pos];
            if self.city_at(u.pos).is_some() || tile.district.is_some() {
                bonus += self.promotion_effect(u, "district_melee");
            }
        }
        if matches!(
            other.promotion_class.as_str(),
            "light_cavalry" | "heavy_cavalry"
        ) {
            bonus += self
                .nbrs(u.pos)
                .into_iter()
                .flat_map(|pos| self.units_at(pos))
                .filter(|id| {
                    let ally = &self.units[id];
                    ally.owner == u.owner
                        && self.rules.units[ally.kind.as_str()].promotion_class
                            != spec.promotion_class
                })
                .map(|id| self.promotion_effect(&self.units[&id], "adjacent_vs_cavalry"))
                .sum::<f64>();
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

    fn promotion_kill_rewards(&mut self, attacker: &Unit, defeated: &Unit) {
        if self.rules.units[defeated.kind.as_str()].domain.as_deref() != Some("sea") {
            return;
        }
        let pct = self.promotion_effect(attacker, "gold_from_naval_kill_pct");
        if pct > 0.0 {
            let strength = self.rules.units[defeated.kind.as_str()].strength;
            self.players[attacker.owner].gold += strength * pct / 100.0;
        }
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
        let additional = self
            .nbrs(target)
            .into_iter()
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
        let multiplier = self
            .promotion_effect(&self.units[&uid], "flanking_multiplier")
            .max(1.0);
        2.0 * additional as f64 * multiplier
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
        if self.crosses_cliff(u.pos, target) && self.promotion_effect(u, "scale_cliffs") <= 0.0 {
            return false;
        }
        u.moves_left >= self.unit_max_moves(uid)
            || u.moves_left >= self.unit_step_cost(uid, u.pos, target)
    }

    fn support_bonus(&self, defender: &Unit) -> f64 {
        if !self.flanking_support_unlocked(defender.owner) {
            return 0.0;
        }
        let adjacent = self
            .nbrs(defender.pos)
            .into_iter()
            .flat_map(|p| self.units_at(p))
            .filter(|id| {
                let u = &self.units[id];
                u.owner == defender.owner && self.rules.units[u.kind.as_str()].class == "military"
            })
            .count();
        let multiplier = self
            .promotion_effect(defender, "support_multiplier")
            .max(1.0);
        2.0 * adjacent as f64 * multiplier
    }

    fn consume_unit_attack(&mut self, uid: u32) {
        let move_after = self.promotion_effect(&self.units[&uid], "move_after_attack") > 0.0;
        let unit = self.units.get_mut(&uid).unwrap();
        unit.attacks_left = (unit.attacks_left - 1).max(0);
        if unit.attacks_left == 0 && !move_after {
            unit.moves_left = 0.0;
        }
        unit.fortified = false;
        unit.fortify_turns = 0;
        unit.acted = true;
    }

    fn consume_melee_attack(&mut self, uid: u32, target: Pos) {
        let cost = self.unit_step_cost(uid, self.units[&uid].pos, target);
        let remaining = (self.units[&uid].moves_left - cost).max(0.0);
        self.units.get_mut(&uid).unwrap().moves_left = remaining;
        self.consume_unit_attack(uid);
    }

    fn pillage_encampment(&mut self, uid: u32, cid: u32, target: Pos) {
        let defender = self.cities[&cid].owner;
        let garrison: Vec<u32> = self
            .units_at(target)
            .into_iter()
            .filter(|id| {
                self.units[id].owner == defender
                    && self.rules.units[self.units[id].kind.as_str()].class == "military"
            })
            .collect();
        for id in garrison {
            self.remove_unit(id);
            self.on_unit_lost(defender);
        }
        let city = self.cities.get_mut(&cid).unwrap();
        city.encampment_hp = 0;
        city.encampment_wall_hp = 0;
        city.encampment_pillaged = true;
        self.enter_tile(uid, target);
    }

    fn do_encampment_melee(
        &mut self,
        pid: usize,
        uid: u32,
        cid: u32,
        target: Pos,
        embarked: bool,
    ) -> Result<(), String> {
        if self.cities[&cid].encampment_hp <= 0 {
            self.consume_melee_attack(uid, target);
            self.pillage_encampment(uid, cid, target);
            return Ok(());
        }
        let attacker = self.units[&uid].clone();
        let spec = self.rules.units[attacker.kind.as_str()].clone();
        let mut attack_base =
            self.unit_unembarked_strength(&attacker) + self.vs_bonus(pid, self.cities[&cid].owner);
        if embarked && self.promotion_effect(&attacker, "amphibious") == 0.0 {
            attack_base -= 10.0;
        }
        let mut defense = self.encampment_strength(cid);
        if self.crosses_river(attacker.pos, target)
            && self.promotion_effect(&attacker, "amphibious") == 0.0
        {
            defense += 5.0;
        }
        let attack = effective_strength(attack_base, attacker.hp);
        let dealt = damage(attack, defense, &mut self.rng);
        let received = damage(defense, attack, &mut self.rng);
        let walls = self.cities[&cid]
            .buildings
            .iter()
            .filter(|building| self.rules.buildings[building.as_str()].outer_defense > 0)
            .count();
        let support = |kind: &str| {
            self.nbrs(target).into_iter().any(|position| {
                self.units_at(position)
                    .into_iter()
                    .any(|id| self.units[&id].owner == pid && self.units[&id].kind == kind)
            })
        };
        let eligible = matches!(spec.promotion_class.as_str(), "melee" | "anti_cavalry");
        let ram = eligible && support("battering_ram") && walls <= 1;
        let tower = eligible && support("siege_tower") && walls <= 2;
        self.encampment_take_damage(cid, dealt, if ram { 1.0 } else { 0.15 }, tower);
        self.units.get_mut(&uid).unwrap().hp -= received;
        self.consume_melee_attack(uid, target);
        if self.units[&uid].hp <= 0 {
            self.remove_unit(uid);
            self.on_unit_lost(pid);
            self.cities.get_mut(&cid).unwrap().encampment_hp =
                self.cities[&cid].encampment_hp.max(1);
            return Ok(());
        }
        if self.cities[&cid].encampment_hp <= 0 {
            self.award_xp(uid, 10.0);
            self.pillage_encampment(uid, cid, target);
        } else {
            self.award_xp(uid, 3.0);
        }
        Ok(())
    }

    fn do_encampment_ranged(
        &mut self,
        pid: usize,
        uid: u32,
        cid: u32,
        _target: Pos,
    ) -> Result<(), String> {
        if self.cities[&cid].encampment_hp <= 0 {
            self.consume_unit_attack(uid);
            self.cities.get_mut(&cid).unwrap().encampment_hp = 0;
            return Ok(());
        }
        let attacker = self.units[&uid].clone();
        let spec = self.rules.units[attacker.kind.as_str()].clone();
        let mut attack_base = self.unit_ranged_attack_strength(&attacker)
            + self.vs_bonus(pid, self.cities[&cid].owner)
            + self.promotion_effect(&attacker, "ranged_vs_district");
        if spec.ranged_strength > 0.0 && spec.domain.as_deref() != Some("sea") {
            attack_base -= 17.0;
        }
        let attack = effective_strength(attack_base, attacker.hp);
        let dealt = damage(attack, self.encampment_strength(cid), &mut self.rng);
        self.encampment_take_damage(cid, dealt, if spec.siege { 1.0 } else { 0.5 }, false);
        self.consume_unit_attack(uid);
        if self.cities[&cid].encampment_hp <= 0 {
            if spec.siege {
                self.cities.get_mut(&cid).unwrap().encampment_hp = 0;
                self.award_xp(uid, 10.0);
            } else {
                self.cities.get_mut(&cid).unwrap().encampment_hp = 1;
                self.award_xp(uid, 3.0);
            }
        } else {
            self.award_xp(uid, 3.0);
        }
        Ok(())
    }

    fn do_attack(&mut self, pid: usize, uid: u32, target: Pos) -> Result<(), String> {
        let u = self.own_unit(pid, uid)?;
        let spec = self.rules.units[u.kind.as_str()].clone();
        if !spec.is_melee_capable() {
            return Err("unit cannot melee attack".into());
        }
        let amphibious = self.is_embarked(&u);
        if u.moves_left <= 0.0 || u.attacks_left <= 0 {
            return Err("no moves left".into());
        }
        if self.wdist(u.pos, target) != 1 {
            return Err("target not adjacent".into());
        }
        if !self.unit_can_melee_target_domain(uid, target) {
            return Err("unit cannot attack into that domain".into());
        }
        if !self.can_pay_melee_entry(uid, target) {
            return Err("not enough movement to attack".into());
        }
        if amphibious
            && self
                .map
                .get(target)
                .map(|t| self.rules.is_water(t))
                .unwrap_or(true)
        {
            return Err("embarked units can only attack onto land".into());
        }
        if let Some(cid) = self.encampment_at(target) {
            let owner = self.cities[&cid].owner;
            if owner != pid && self.is_at_war(pid, owner) {
                return self.do_encampment_melee(pid, uid, cid, target, amphibious);
            }
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
        self.consume_melee_attack(uid, target);
        // A unit garrisoned in a City Center cannot be targeted directly;
        // attacks hit the city and the garrison only affects its strength.
        if city_id.is_none() && !military.is_empty() {
            let did = *military
                .iter()
                .max_by(|a, b| {
                    let ea = effective_strength(
                        self.unit_strength(&self.units[*a], true),
                        self.units[*a].hp,
                    );
                    let eb = effective_strength(
                        self.unit_strength(&self.units[*b], true),
                        self.units[*b].hp,
                    );
                    ea.partial_cmp(&eb).unwrap()
                })
                .unwrap();
            let d = self.units[&did].clone();
            let attacker = self.units[&uid].clone();
            let mut att_base = self.unit_unembarked_strength(&attacker)
                + self.matchup_bonus(uid, &d, true)
                + self.flanking_bonus(uid, target)
                + self.vs_bonus(pid, d.owner);
            if amphibious && self.promotion_effect(&attacker, "amphibious") == 0.0 {
                att_base -= 10.0;
            }
            let mut def_base = self.unit_strength(&d, true)
                + self.matchup_bonus(did, &attacker, false)
                + self.tile_defense_bonus(target)
                + self.support_bonus(&d)
                + self.vs_bonus(d.owner, pid);
            if self.crosses_river(u.pos, target)
                && self.promotion_effect(&attacker, "amphibious") == 0.0
            {
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
            if attacker_dead && !d_dead && self.has_ability(downer, "killer_of_cyrus") {
                if let Some(defender) = self.units.get_mut(&did) {
                    defender.hp = (defender.hp + 30).min(100);
                }
            }
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
                self.promotion_kill_rewards(&attacker, &d);
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
                if amphibious && self.promotion_effect(&attacker, "amphibious") == 0.0 {
                    att_base -= 10.0;
                }
                let att = effective_strength(att_base, attacker.hp);
                let cs = self.city_strength(cid)
                    + if self.crosses_river(u.pos, target)
                        && self.promotion_effect(&attacker, "amphibious") == 0.0
                    {
                        5.0
                    } else {
                        0.0
                    };
                let dmg_out = damage(att, cs, &mut self.rng);
                let dmg_in = damage(cs, att, &mut self.rng);
                let walls = self.cities[&cid]
                    .buildings
                    .iter()
                    .filter(|building| self.rules.buildings[building.as_str()].outer_defense > 0)
                    .count();
                let support = |kind: &str| {
                    self.nbrs(target).into_iter().any(|p| {
                        self.units_at(p).into_iter().any(|id| {
                            let o = &self.units[&id];
                            o.owner == pid && o.kind == kind
                        })
                    })
                };
                // battering ram: full melee damage vs ancient walls;
                // siege tower: only melee/anti-cavalry pour through the walls
                let support_eligible =
                    matches!(spec.promotion_class.as_str(), "melee" | "anti_cavalry");
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
        let linked = self
            .units
            .get(&uid)
            .and_then(|unit| unit.linked_to)
            .filter(|_| self.is_linked_leader(uid));
        self.resolve_entered_units(uid, pos);
        self.relocate(uid, pos);
        if let Some(peer) = linked {
            self.relocate(peer, pos);
        }
        if self.formation_enters_enemy_zoc(uid, pos) {
            self.stop_unit_by_zoc(uid);
            if let Some(peer) = linked {
                self.stop_unit_by_zoc(peer);
            }
        }
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
        if spec.siege && u.moved && self.promotion_effect(&u, "attack_after_move") == 0.0 {
            return Err("siege units cannot move and attack in the same turn".into());
        }
        if u.moves_left <= 0.0 || u.attacks_left <= 0 {
            return Err("no moves left".into());
        }
        let range = spec.range.max(1) + self.promotion_effect(&u, "range") as i32;
        if self.wdist(u.pos, target) > range {
            return Err("out of range".into());
        }
        if !self.unit_has_line_of_sight(uid, target) {
            return Err("line of sight blocked".into());
        }
        if let Some(cid) = self.encampment_at(target) {
            let owner = self.cities[&cid].owner;
            if owner != pid && self.is_at_war(pid, owner) {
                return self.do_encampment_ranged(pid, uid, cid, target);
            }
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
        self.consume_unit_attack(uid);
        // City Center garrisons are protected while the city stands.
        if city_id.is_none() && !military.is_empty() {
            let did = *military
                .iter()
                .max_by(|a, b| {
                    let ea = effective_strength(
                        self.unit_strength(&self.units[*a], true),
                        self.units[*a].hp,
                    );
                    let eb = effective_strength(
                        self.unit_strength(&self.units[*b], true),
                        self.units[*b].hp,
                    );
                    ea.partial_cmp(&eb).unwrap()
                })
                .unwrap();
            let defender = self.units[&did].clone();
            let attacker = self.units[&uid].clone();
            let downer = defender.owner;
            let defender_spec = &self.rules.units[defender.kind.as_str()];
            let mut att_base = self.unit_ranged_attack_strength(&self.units[&uid])
                + self.matchup_bonus(uid, &defender, true)
                + if defender_spec.domain.as_deref() == Some("sea") {
                    self.promotion_effect(&attacker, "ranged_vs_units")
                        + self.promotion_effect(&attacker, "ranged_vs_naval")
                        + self.promotion_effect(&attacker, "siege_vs_naval")
                } else {
                    self.promotion_effect(&attacker, "ranged_vs_land")
                        + self.promotion_effect(&attacker, "ranged_vs_units")
                        + self.promotion_effect(&attacker, "siege_vs_land")
                }
                + self.vs_bonus(pid, downer);
            if (spec.bombard_strength > 0.0 && defender_spec.domain.as_deref() != Some("sea"))
                || (spec.ranged_strength > 0.0
                    && spec.domain.as_deref() != Some("sea")
                    && defender_spec.domain.as_deref() == Some("sea"))
            {
                att_base -= 17.0;
            }
            let att = effective_strength(att_base, self.units[&uid].hp);
            let ds = effective_strength(
                self.unit_strength(&defender, true)
                    + self.ranged_defense_bonus(&defender, false)
                    + self.tile_defense_bonus(target)
                    + self.vs_bonus(downer, pid),
                defender.hp,
            );
            let dmg = damage(att, ds, &mut self.rng);
            self.units.get_mut(&did).unwrap().hp -= dmg;
            let defender_dead = self.units[&did].hp <= 0;
            self.award_unit_combat_xp(uid, &defender, true, true, defender_dead);
            if !defender_dead {
                self.award_unit_combat_xp(did, &attacker, true, false, false);
            }
            if defender_dead {
                bump(&mut self.players[pid], "kills");
                self.promotion_kill_rewards(&attacker, &defender);
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
            let starting_hp = self.cities[&cid].hp;
            let mut att_base = self.unit_ranged_attack_strength(&self.units[&uid])
                + self.promotion_effect(&self.units[&uid], "ranged_vs_district")
                + self.vs_bonus(pid, self.cities[&cid].owner);
            if spec.ranged_strength > 0.0 && spec.domain.as_deref() != Some("sea") {
                att_base -= 17.0;
            }
            let att = effective_strength(att_base, self.units[&uid].hp);
            let cs = self.city_strength(cid);
            let dmg = damage(att, cs, &mut self.rng);
            let mult = if spec.siege { 1.0 } else { 0.5 };
            self.city_take_damage(cid, dmg, mult, false);
            if starting_hp <= 0 {
                // Shots after a Bombard attack has depleted the garrison
                // grant no XP and must not revive the city.
                self.cities.get_mut(&cid).unwrap().hp = 0;
            } else if self.cities[&cid].hp <= 0 && spec.siege {
                // Bombard-class shots may deplete a city, but still cannot
                // capture it. The depleting shot earns the city final-blow XP.
                self.cities.get_mut(&cid).unwrap().hp = 0;
                self.award_xp(uid, 10.0);
            } else {
                // Ordinary ranged attacks cannot reduce Garrison Health
                // below 1 and earn the normal city-attack XP.
                self.cities.get_mut(&cid).unwrap().hp = self.cities[&cid].hp.max(1);
                self.award_xp(uid, 3.0);
            }
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
            production_progress: BTreeMap::new(),
            border_culture: 0.0,
            hp: 200,
            buildings: Vec::new(),
            pillaged_buildings: BTreeSet::new(),
            districts: Districts::default(),
            wonders: BTreeMap::new(),
            owned_tiles: Vec::new(),
            queue: Vec::new(),
            original_owner: pid,
            is_capital,
            struck: false,
            wall_hp: 0,
            encampment_hp: 0,
            encampment_wall_hp: 0,
            encampment_struck: false,
            encampment_last_attacked: 0,
            encampment_pillaged: false,
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
        let can_build = (u.kind == "builder" && self.rules.improvements[imp].builder_buildable)
            || self.rules.units[u.kind.as_str()]
                .builds
                .iter()
                .any(|built| built == imp);
        if !can_build || u.charges <= 0 {
            return Err("unit cannot build that improvement".into());
        }
        if !self.valid_improvements(pid, u.pos).iter().any(|i| i == imp) {
            return Err("invalid improvement here".into());
        }
        let removes = self.rules.improvements[imp].removes_feature;
        let t = self.map.tiles.get_mut(&u.pos).unwrap();
        t.improvement = Some(imp.to_string());
        t.pillaged = false;
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

    fn pillageable_at(&self, pid: usize, pos: Pos) -> bool {
        let Some(tile) = self.map.get(pos) else {
            return false;
        };
        if tile.improvement.as_deref() == Some("barbarian_camp") {
            return Some(pid) != self.barb_pid;
        }
        let Some(cid) = tile.owner_city else {
            return false;
        };
        let Some(city) = self.cities.get(&cid) else {
            return false;
        };
        if city.owner == pid || !self.is_at_war(pid, city.owner) || self.city_at(pos).is_some() {
            return false;
        }
        if tile.improvement.is_some() && !tile.pillaged {
            return true;
        }
        let Some(district) = tile.district.as_deref() else {
            return false;
        };
        if self.district_is_family(district, "encampment")
            || self.units_at(pos).iter().any(|id| {
                self.units[id].owner == city.owner
                    && self.rules.units[self.units[id].kind.as_str()].class == "military"
            })
        {
            return false;
        }
        if !tile.pillaged {
            return true;
        }
        city.buildings.iter().any(|building| {
            !city.pillaged_buildings.contains(building)
                && self.rules.buildings.get(building).is_some_and(|spec| {
                    spec.district
                        .as_ref()
                        .is_some_and(|family| self.district_is_family(district, family))
                })
        })
    }

    fn grant_pillage_reward(&mut self, pid: usize, uid: u32, source: &str, coastal: bool) {
        let amount = 25.0 * (self.world_era as f64 + 1.0);
        let district_family = self
            .rules
            .districts
            .get(source)
            .map(|_| self.district_family(source))
            .unwrap_or(source);
        match district_family {
            "farm" | "fishing_boats" => {
                if let Some(unit) = self.units.get_mut(&uid) {
                    unit.hp = (unit.hp + 50).min(100);
                }
            }
            "campus" | "mine" | "quarry" | "oil_well" | "offshore_oil_rig" | "geothermal_plant"
            | "solar_farm" | "wind_farm" | "offshore_wind_farm" => {
                self.players[pid].research_overflow += amount;
            }
            "holy_site" | "sphinx" | "kurgan" | "nubian_pyramid" => {
                self.players[pid].faith += amount;
            }
            "theater_square"
            | "great_wall"
            | "seaside_resort"
            | "ski_resort"
            | "national_park"
            | "archaeological_dig"
            | "shipwreck_excavation" => {
                self.players[pid].civic_overflow += amount;
            }
            "industrial_zone" | "aerodrome" => {
                if let Some(cid) = self
                    .player_city_ids(pid)
                    .into_iter()
                    .min_by_key(|cid| self.wdist(self.cities[cid].pos, self.units[&uid].pos))
                {
                    self.cities.get_mut(&cid).unwrap().production += amount;
                }
            }
            _ => {
                let bonus = if coastal {
                    self.promotion_effect(&self.units[&uid], "coastal_raid_gold_pct")
                } else {
                    0.0
                };
                self.players[pid].gold += amount * (1.0 + bonus / 100.0);
            }
        }
    }

    fn pillage_tile(
        &mut self,
        pid: usize,
        uid: u32,
        pos: Pos,
        coastal: bool,
    ) -> Result<(), String> {
        if !self.pillageable_at(pid, pos) {
            return Err("nothing pillageable there".into());
        }
        if self.map.tiles[&pos].improvement.as_deref() == Some("barbarian_camp") {
            self.barb_camps.remove(&pos);
            self.map.tiles.get_mut(&pos).unwrap().improvement = None;
            self.players[pid].gold += 50.0;
            self.players[pid].era_score += 1;
            bump(&mut self.players[pid], "camps");
            return Ok(());
        }
        let source = if let Some(improvement) = self.map.tiles[&pos].improvement.clone() {
            self.map.tiles.get_mut(&pos).unwrap().pillaged = true;
            improvement
        } else {
            let district = self.map.tiles[&pos].district.clone().unwrap();
            if !self.map.tiles[&pos].pillaged {
                self.map.tiles.get_mut(&pos).unwrap().pillaged = true;
                district
            } else {
                let cid = self.map.tiles[&pos].owner_city.unwrap();
                let building = self.cities[&cid]
                    .buildings
                    .iter()
                    .filter(|building| !self.cities[&cid].pillaged_buildings.contains(*building))
                    .filter(|building| {
                        self.rules.buildings[building.as_str()]
                            .district
                            .as_ref()
                            .is_some_and(|family| self.district_is_family(&district, family))
                    })
                    .max_by(|a, b| {
                        self.rules.buildings[a.as_str()]
                            .cost
                            .partial_cmp(&self.rules.buildings[b.as_str()].cost)
                            .unwrap()
                            .then(a.cmp(b))
                    })
                    .cloned()
                    .ok_or_else(|| "district is already fully pillaged".to_string())?;
                self.cities
                    .get_mut(&cid)
                    .unwrap()
                    .pillaged_buildings
                    .insert(building.clone());
                building
            }
        };
        self.grant_pillage_reward(pid, uid, &source, coastal);
        Ok(())
    }

    fn do_pillage(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let unit = self.own_unit(pid, uid)?;
        let spec = &self.rules.units[unit.kind.as_str()];
        if spec.class != "military"
            || spec.domain.as_deref() == Some("air")
            || self.is_embarked(&unit)
            || unit.moves_left <= 0.0
        {
            return Err("unit cannot pillage".into());
        }
        self.pillage_tile(pid, uid, unit.pos, false)?;
        let cost = if self.promotion_effect(&unit, "pillage_cost") > 0.0 {
            1.0
        } else {
            3.0
        };
        let unit = self.units.get_mut(&uid).unwrap();
        unit.moves_left = (unit.moves_left - cost).max(0.0);
        unit.acted = true;
        Ok(())
    }

    fn do_repair_improvement(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let builder = self.own_unit(pid, uid)?;
        let tile = self
            .map
            .get(builder.pos)
            .ok_or_else(|| "unit is off map".to_string())?;
        if builder.kind != "builder"
            || builder.moves_left <= 0.0
            || !tile.pillaged
            || tile.improvement.is_none()
            || tile
                .owner_city
                .and_then(|cid| self.cities.get(&cid))
                .is_none_or(|city| city.owner != pid)
        {
            return Err("builder cannot repair this improvement".into());
        }
        self.map.tiles.get_mut(&builder.pos).unwrap().pillaged = false;
        let builder = self.units.get_mut(&uid).unwrap();
        builder.moves_left = 0.0;
        builder.acted = true;
        Ok(())
    }

    fn do_coastal_raid(&mut self, pid: usize, uid: u32, target: Pos) -> Result<(), String> {
        let unit = self.own_unit(pid, uid)?;
        let spec = &self.rules.units[unit.kind.as_str()];
        if spec.promotion_class != "naval_raider"
            || unit.moves_left <= 0.0
            || unit.attacks_left <= 0
            || self.wdist(unit.pos, target) != 1
            || self
                .map
                .get(target)
                .is_none_or(|tile| self.rules.is_water(tile))
        {
            return Err("unit cannot coastal raid that tile".into());
        }
        self.pillage_tile(pid, uid, target, true)?;
        self.consume_unit_attack(uid);
        Ok(())
    }

    fn air_capacity_at(&self, pid: usize, pos: Pos) -> i32 {
        let mut capacity = 0;
        if let Some(cid) = self.city_at(pos) {
            let city = &self.cities[&cid];
            if city.owner == pid {
                capacity += city
                    .districts
                    .iter()
                    .filter(|(district, _)| self.district_is_family(district, "aerodrome"))
                    .map(|(district, _)| self.rules.districts[district.as_str()].air_slots)
                    .sum::<i32>();
                capacity += city
                    .buildings
                    .iter()
                    .filter(|building| !city.pillaged_buildings.contains(*building))
                    .map(|building| {
                        self.rules.buildings[building.as_str()]
                            .effects
                            .get("air_slots")
                            .copied()
                            .unwrap_or(0.0) as i32
                    })
                    .sum::<i32>();
            }
        }
        if let Some(tile) = self.map.get(pos) {
            if tile
                .owner_city
                .and_then(|cid| self.cities.get(&cid))
                .is_some_and(|city| city.owner == pid)
            {
                if let Some(district) = tile.district.as_deref() {
                    if self.district_is_family(district, "aerodrome") && !tile.pillaged {
                        capacity += self.rules.districts[district].air_slots;
                    }
                }
                if tile.improvement.as_deref() == Some("airstrip") && !tile.pillaged {
                    capacity += self.rules.improvements["airstrip"]
                        .effects
                        .get("air_slots")
                        .copied()
                        .unwrap_or(3.0) as i32;
                }
            }
        }
        capacity += self
            .units_at(pos)
            .into_iter()
            .filter(|id| self.units[id].owner == pid && self.units[id].kind == "aircraft_carrier")
            .map(|id| 2 + self.promotion_effect(&self.units[&id], "aircraft_slots") as i32)
            .sum::<i32>();
        capacity
    }

    fn air_units_at(&self, pid: usize, pos: Pos) -> i32 {
        self.units_at(pos)
            .into_iter()
            .filter(|id| {
                self.units[id].owner == pid
                    && self.rules.units[self.units[id].kind.as_str()]
                        .domain
                        .as_deref()
                        == Some("air")
            })
            .count() as i32
    }

    fn can_air_base_at(&self, pid: usize, pos: Pos, moving: Option<u32>) -> bool {
        let occupied = self.air_units_at(pid, pos)
            - moving
                .and_then(|uid| self.units.get(&uid))
                .is_some_and(|unit| unit.pos == pos) as i32;
        self.air_capacity_at(pid, pos) > occupied
    }

    fn do_air_rebase(&mut self, pid: usize, uid: u32, to: Pos) -> Result<(), String> {
        let unit = self.own_unit(pid, uid)?;
        let spec = &self.rules.units[unit.kind.as_str()];
        if spec.domain.as_deref() != Some("air")
            || unit.moves_left <= 0.0
            || unit.pos == to
            || self.wdist(unit.pos, to) > spec.range.max(1) * 2
            || !self.can_air_base_at(pid, to, Some(uid))
        {
            return Err("aircraft cannot rebase there".into());
        }
        self.relocate(uid, to);
        let aircraft = self.units.get_mut(&uid).unwrap();
        aircraft.moves_left = 0.0;
        aircraft.attacks_left = 0;
        aircraft.acted = true;
        aircraft.air_patrol = false;
        Ok(())
    }

    fn air_interception_strength(&mut self, attacker: &Unit, target: Pos) -> f64 {
        let mut candidates: Vec<(f64, u32)> = self
            .units
            .values()
            .filter(|unit| {
                unit.owner != attacker.owner && self.is_at_war(attacker.owner, unit.owner)
            })
            .filter_map(|unit| {
                let spec = &self.rules.units[unit.kind.as_str()];
                let fighter = spec.domain.as_deref() == Some("air")
                    && unit.air_patrol
                    && self.wdist(unit.pos, target) <= spec.range.max(1);
                let ground = matches!(unit.kind.as_str(), "anti_air_gun" | "mobile_sam")
                    && self.wdist(unit.pos, target) <= 1;
                (fighter || ground).then_some((spec.ranged_attack_strength(), unit.id))
            })
            .collect();
        candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        let Some((strength, interceptor)) = candidates.first().copied() else {
            return 0.0;
        };
        if self.rules.units[self.units[&interceptor].kind.as_str()]
            .domain
            .as_deref()
            == Some("air")
        {
            self.units.get_mut(&interceptor).unwrap().air_patrol = false;
        }
        strength
    }

    fn do_air_strike(&mut self, pid: usize, uid: u32, target: Pos) -> Result<(), String> {
        let attacker = self.own_unit(pid, uid)?;
        let spec = self.rules.units[attacker.kind.as_str()].clone();
        if spec.domain.as_deref() != Some("air")
            || attacker.moves_left <= 0.0
            || attacker.attacks_left <= 0
            || self.wdist(attacker.pos, target) > spec.range.max(1)
            || !self.enemy_combat_target_at(pid, target)
        {
            return Err("invalid air strike".into());
        }
        let interception = self.air_interception_strength(&attacker, target);
        if interception > 0.0 {
            let attack_defense = effective_strength(spec.strength.max(1.0), attacker.hp);
            let incoming = damage(interception, attack_defense, &mut self.rng);
            self.units.get_mut(&uid).unwrap().hp -= incoming;
            if self.units[&uid].hp <= 0 {
                self.remove_unit(uid);
                self.on_unit_lost(pid);
                return Ok(());
            }
        }
        let attack = effective_strength(spec.ranged_attack_strength(), self.units[&uid].hp);
        if let Some(cid) = self.city_at(target) {
            if self.cities[&cid].owner != pid && self.is_at_war(pid, self.cities[&cid].owner) {
                let dealt = damage(attack, self.city_strength(cid), &mut self.rng);
                self.city_take_damage(cid, dealt, if spec.siege { 1.0 } else { 0.5 }, false);
                if self.cities[&cid].hp <= 0 {
                    self.cities.get_mut(&cid).unwrap().hp = 1;
                }
            }
        } else if let Some(cid) = self.encampment_at(target) {
            if self.cities[&cid].owner != pid && self.is_at_war(pid, self.cities[&cid].owner) {
                let dealt = damage(attack, self.encampment_strength(cid), &mut self.rng);
                self.encampment_take_damage(cid, dealt, if spec.siege { 1.0 } else { 0.5 }, false);
                if self.cities[&cid].encampment_hp <= 0 {
                    self.cities.get_mut(&cid).unwrap().encampment_hp = 1;
                }
            }
        } else if let Some(defender_id) = self.units_at(target).into_iter().find(|id| {
            self.units[id].owner != pid
                && self.is_at_war(pid, self.units[id].owner)
                && self.rules.units[self.units[id].kind.as_str()].class == "military"
        }) {
            let defender = self.units[&defender_id].clone();
            let anti_air = self.promotion_effect(&defender, "defend_air");
            let defense =
                effective_strength(self.unit_strength(&defender, true) + anti_air, defender.hp);
            let dealt = damage(attack, defense, &mut self.rng);
            self.units.get_mut(&defender_id).unwrap().hp -= dealt;
            let killed = self.units[&defender_id].hp <= 0;
            self.award_unit_combat_xp(uid, &defender, true, true, killed);
            if killed {
                bump(&mut self.players[pid], "kills");
                self.remove_unit(defender_id);
                self.on_unit_lost(defender.owner);
            }
        }
        self.consume_unit_attack(uid);
        Ok(())
    }

    fn do_air_patrol(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let unit = self.own_unit(pid, uid)?;
        let spec = &self.rules.units[unit.kind.as_str()];
        if spec.domain.as_deref() != Some("air")
            || spec.siege
            || unit.moves_left <= 0.0
            || unit.attacks_left <= 0
        {
            return Err("aircraft cannot patrol".into());
        }
        let unit = self.units.get_mut(&uid).unwrap();
        unit.air_patrol = true;
        unit.moves_left = 0.0;
        unit.attacks_left = 0;
        unit.acted = true;
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
        let old = self.cities[&cid].queue.first().cloned();
        if old.as_ref() == Some(item) {
            return Ok(());
        }
        let new_key = Self::item_progress_key(item);
        let city = self.cities.get_mut(&cid).unwrap();
        if let Some(old_item) = old {
            let old_key = Self::item_progress_key(&old_item);
            city.production_progress.insert(old_key, city.production);
            city.production = city.production_progress.remove(&new_key).unwrap_or(0.0);
        } else {
            let overflow = city.production;
            city.production = city.production_progress.remove(&new_key).unwrap_or(0.0) + overflow;
        }
        city.queue = vec![item.clone()];
        Ok(())
    }

    fn do_buy(&mut self, pid: usize, cid: u32, unit: &str, currency: &str) -> Result<(), String> {
        match self.cities.get(&cid) {
            Some(c) if c.owner == pid => {}
            _ => return Err("not your city".into()),
        }
        let religious = self
            .rules
            .units
            .get(unit)
            .map(|s| s.class == "religious")
            .unwrap_or(false);
        if religious {
            // Religious units adopt the majority religion of their purchase city.
            if currency != "faith" {
                return Err("religious units are bought with faith".into());
            }
            if !self.city_has_district_family(&self.cities[&cid], "holy_site") {
                return Err("needs a holy site".into());
            }
            if self.city_religion(&self.cities[&cid]).is_none() {
                return Err("city has no majority religion".into());
            }
            let spec = &self.rules.units[unit];
            if !self.unlocked(pid, &spec.tech.clone(), &spec.civic.clone()) {
                return Err("not unlocked".into());
            }
            if spec.requires_building.as_ref().is_some_and(|building| {
                !self.city_has_building_family(&self.cities[&cid], building)
            }) {
                return Err("required religious building is missing".into());
            }
            if unit == "inquisitor"
                && self.players[pid]
                    .counters
                    .get("inquisition")
                    .copied()
                    .unwrap_or(0)
                    == 0
            {
                return Err("inquisition has not been launched".into());
            }
        } else {
            let it = Item::Unit {
                unit: unit.to_string(),
            };
            if !self.can_produce(pid, cid, &it) {
                return Err("cannot buy that".into());
            }
        }
        if unit == "settler" && self.cities[&cid].pop < 2 {
            return Err("city too small for settler".into());
        }
        let mult = if currency == "gold" { 4.0 } else { 2.0 };
        let item = Item::Unit {
            unit: unit.to_string(),
        };
        let purchase_discount =
            self.city_district_effect(&self.cities[&cid], "gold_faith_purchase_discount_pct");
        let cost =
            self.item_cost_for(pid, &item) * mult * (1.0 - purchase_discount / 100.0).max(0.0);
        let bank = if currency == "gold" {
            self.players[pid].gold
        } else {
            self.players[pid].faith
        };
        if bank < cost {
            return Err("cannot afford".into());
        }
        let pos = self.cities[&cid].pos;
        let placed = self
            .place_new_unit(unit, pid, pos)
            .ok_or_else(|| "no space to place unit".to_string())?;
        self.apply_training_district_effects(cid, placed);
        if religious {
            self.units.get_mut(&placed).unwrap().religion =
                self.city_religion(&self.cities[&cid]).map(str::to_string);
        }
        if currency == "gold" {
            self.players[pid].gold -= cost;
        } else {
            self.players[pid].faith -= cost;
        }
        if unit == "settler" {
            self.cities.get_mut(&cid).unwrap().pop -= 1;
        }
        if unit == "settler" || unit == "builder" {
            bump(&mut self.players[pid], &format!("trained:{unit}"));
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

    fn do_promote(&mut self, pid: usize, uid: u32, promotion: &str) -> Result<(), String> {
        let unit = self.own_unit(pid, uid)?;
        if unit.moves_left <= 0.0 {
            return Err("unit has no movement left".into());
        }
        if !self
            .available_promotions(uid)
            .iter()
            .any(|name| name == promotion)
        {
            return Err("promotion unavailable".into());
        }
        let extra_charges = self.rules.promotions[promotion]
            .effects
            .get("religious_charges")
            .copied()
            .unwrap_or(0.0)
            + self.rules.promotions[promotion]
                .effects
                .get("natural_wonder_charges")
                .copied()
                .unwrap_or(0.0);
        let unit = self.units.get_mut(&uid).unwrap();
        unit.promotions.insert(promotion.to_string());
        unit.charges += extra_charges as i32;
        unit.level = (unit.level + 1).min(8);
        unit.hp = (unit.hp + 50).min(100);
        unit.moves_left = 0.0;
        unit.attacks_left = 0;
        unit.acted = true;
        unit.fortified = false;
        unit.fortify_turns = 0;
        Ok(())
    }

    /// Return the resulting formation size if these two units may combine.
    fn can_combine_units(&self, pid: usize, a: u32, b: u32) -> Option<u8> {
        let (a, b) = (self.units.get(&a)?, self.units.get(&b)?);
        if a.owner != pid
            || b.owner != pid
            || a.id == b.id
            || a.kind != b.kind
            || a.linked_to.is_some()
            || b.linked_to.is_some()
            || a.moves_left <= 0.0
            || b.moves_left <= 0.0
            || self.wdist(a.pos, b.pos) > 1
            || self.rules.units[a.kind.as_str()].class != "military"
        {
            return None;
        }
        match (a.formation, b.formation) {
            (0, 0) if self.players[pid].civics.contains("nationalism") => Some(1),
            (0, 1) | (1, 0) if self.players[pid].civics.contains("mobilization") => Some(2),
            _ => None,
        }
    }

    fn do_combine_units(&mut self, pid: usize, a: u32, b: u32) -> Result<(), String> {
        let formation = self
            .can_combine_units(pid, a, b)
            .ok_or_else(|| "units cannot form a Corps or Army".to_string())?;
        let ua = self.units[&a].clone();
        let ub = self.units[&b].clone();
        // Civilopedia rule: preserve the XP and promotions of the most
        // experienced constituent. Stable ID resolves an exact tie.
        let a_key = (ua.xp, ua.promotions.len(), Reverse(ua.id));
        let b_key = (ub.xp, ub.promotions.len(), Reverse(ub.id));
        let (survivor, consumed) = if a_key >= b_key { (a, b) } else { (b, a) };
        let destination = ub.pos;
        let hp = ua.hp.max(ub.hp);
        self.remove_unit(consumed);
        if self.units[&survivor].pos != destination {
            self.relocate(survivor, destination);
        }
        let unit = self.units.get_mut(&survivor).unwrap();
        unit.formation = formation;
        unit.hp = hp;
        unit.moves_left = 0.0;
        unit.attacks_left = 0;
        unit.acted = true;
        if formation == 1 {
            bump(&mut self.players[pid], "corps");
        }
        Ok(())
    }

    fn can_link_units(&self, pid: usize, a: u32, b: u32) -> bool {
        let (Some(a), Some(b)) = (self.units.get(&a), self.units.get(&b)) else {
            return false;
        };
        if a.owner != pid
            || b.owner != pid
            || a.id == b.id
            || a.pos != b.pos
            || a.linked_to.is_some()
            || b.linked_to.is_some()
            || self.noncombat_action_blocked_by_zoc(a.id)
            || self.noncombat_action_blocked_by_zoc(b.id)
        {
            return false;
        }
        let (aspec, bspec) = (
            &self.rules.units[a.kind.as_str()],
            &self.rules.units[b.kind.as_str()],
        );
        let ordinary = (aspec.class == "military"
            && matches!(bspec.class.as_str(), "civilian" | "support" | "religious"))
            || (bspec.class == "military"
                && matches!(aspec.class.as_str(), "civilian" | "support" | "religious"));
        let naval_escort = (aspec.domain.as_deref() == Some("sea")
            && bspec.class == "military"
            && self.is_embarked(b))
            || (bspec.domain.as_deref() == Some("sea")
                && aspec.class == "military"
                && self.is_embarked(a));
        ordinary || naval_escort
    }

    fn do_link_units(&mut self, pid: usize, a: u32, b: u32) -> Result<(), String> {
        if !self.can_link_units(pid, a, b) {
            return Err("units cannot form a linked formation".into());
        }
        self.units.get_mut(&a).unwrap().linked_to = Some(b);
        self.units.get_mut(&b).unwrap().linked_to = Some(a);
        Ok(())
    }

    fn do_unlink_units(&mut self, pid: usize, uid: u32) -> Result<(), String> {
        let unit = self.own_unit(pid, uid)?;
        let peer = unit
            .linked_to
            .ok_or_else(|| "unit is not linked".to_string())?;
        self.units.get_mut(&uid).unwrap().linked_to = None;
        if let Some(other) = self.units.get_mut(&peer) {
            other.linked_to = None;
        }
        Ok(())
    }

    fn do_government(&mut self, pid: usize, g: &str) -> Result<(), String> {
        let spec = self
            .rules
            .governments
            .get(g)
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
            && !self.players[pid].policies.is_empty()
        {
            let drop = self.players[pid]
                .policies
                .iter()
                .next_back()
                .unwrap()
                .clone();
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
        if self.city_at(target).is_some() || self.encampment_at(target).is_some() {
            return Err("cities cannot strike defensible districts".into());
        }
        if !self.has_line_of_sight(self.cities[&cid].pos, target, true) {
            return Err("line of sight blocked".into());
        }
        let enemies: Vec<u32> = self
            .units_at(target)
            .into_iter()
            .filter(|id| {
                let o = &self.units[id];
                o.owner != pid && self.is_at_war(pid, o.owner)
            })
            .collect();
        if enemies.is_empty() {
            return Err("no enemy target".into());
        }
        let military: Vec<u32> = enemies
            .iter()
            .cloned()
            .filter(|id| self.rules.units[self.units[id].kind.as_str()].class == "military")
            .collect();
        if military.is_empty() {
            return Err("no enemy military target".into());
        }
        let did = *military
            .iter()
            .max_by(|a, b| {
                let ea = effective_strength(
                    self.unit_strength(&self.units[*a], true),
                    self.units[*a].hp,
                );
                let eb = effective_strength(
                    self.unit_strength(&self.units[*b], true),
                    self.units[*b].hp,
                );
                ea.partial_cmp(&eb).unwrap()
            })
            .unwrap();
        let d = self.units[&did].clone();
        let ds = effective_strength(
            self.unit_strength(&d, true) + self.ranged_defense_bonus(&d, true),
            d.hp,
        ) + self.tile_defense_bonus(target);
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

    fn do_encampment_strike(&mut self, pid: usize, cid: u32, target: Pos) -> Result<(), String> {
        let city = self
            .cities
            .get(&cid)
            .filter(|city| city.owner == pid)
            .ok_or_else(|| "not your city".to_string())?;
        let position = city
            .districts
            .iter()
            .find_map(|(district, position)| {
                self.district_is_family(district, "encampment")
                    .then_some(*position)
            })
            .ok_or_else(|| "city has no Encampment".to_string())?;
        if city.encampment_pillaged
            || city.encampment_hp <= 0
            || city.encampment_wall_hp <= 0
            || city.encampment_struck
        {
            return Err("Encampment cannot strike".into());
        }
        if self.wdist(position, target) > 2 || !self.has_line_of_sight(position, target, true) {
            return Err("target out of range or sight".into());
        }
        if self.city_at(target).is_some() || self.encampment_at(target).is_some() {
            return Err("defensible districts cannot target each other".into());
        }
        let defender_id = self
            .units_at(target)
            .into_iter()
            .filter(|id| {
                let unit = &self.units[id];
                unit.owner != pid
                    && self.is_at_war(pid, unit.owner)
                    && self.rules.units[unit.kind.as_str()].class == "military"
            })
            .max_by(|a, b| {
                let a_strength =
                    effective_strength(self.unit_strength(&self.units[a], true), self.units[a].hp);
                let b_strength =
                    effective_strength(self.unit_strength(&self.units[b], true), self.units[b].hp);
                a_strength.partial_cmp(&b_strength).unwrap()
            })
            .ok_or_else(|| "no enemy military target".to_string())?;
        let defender = self.units[&defender_id].clone();
        let defense = effective_strength(
            self.unit_strength(&defender, true) + self.ranged_defense_bonus(&defender, false),
            defender.hp,
        ) + self.tile_defense_bonus(target);
        let naval = self.rules.units[defender.kind.as_str()].domain.as_deref() == Some("sea");
        let attack = self.city_ranged_strength(cid) - if naval { 17.0 } else { 0.0 };
        let dealt = damage(attack, defense, &mut self.rng);
        self.units.get_mut(&defender_id).unwrap().hp -= dealt;
        if self.units[&defender_id].hp > 0 {
            self.award_xp(defender_id, 2.0);
        } else {
            bump(&mut self.players[pid], "kills");
            let owner = self.units[&defender_id].owner;
            self.remove_unit(defender_id);
            self.on_unit_lost(owner);
        }
        self.cities.get_mut(&cid).unwrap().encampment_struck = true;
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

    /// Governor titles come from civic milestones and completed districts
    /// such as the Government Plaza.
    pub fn governor_titles(&self, pid: usize) -> usize {
        let civics = ["political_philosophy", "civil_service", "guilds"]
            .iter()
            .filter(|c| self.players[pid].civics.contains(**c))
            .count();
        civics
            + self.players[pid]
                .counters
                .get("district_governor_titles")
                .copied()
                .unwrap_or(0)
                .max(0) as usize
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
                } else if !self.players[o.owner].is_barbarian && !self.players[o.owner].is_minor {
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
            delta += self.cities[&cid]
                .districts
                .iter()
                .map(|(district, position)| {
                    let spec = &self.rules.districts[district.as_str()];
                    spec.loyalty
                        + if self.on_foreign_continent(pid, *position) {
                            spec.effects
                                .get("foreign_continent_loyalty")
                                .copied()
                                .unwrap_or(0.0)
                        } else {
                            0.0
                        }
                })
                .sum::<f64>();
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
            let suz: i64 = self
                .players
                .iter()
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

    /// World era from the most advanced researched technology or civic.
    ///
    /// Era metadata lives on the tree nodes, so late-game progress cannot be
    /// capped by a hard-coded count table when the ruleset grows.
    fn era_from_progress(&self) -> usize {
        self.players
            .iter()
            .filter(|p| !p.is_minor)
            .flat_map(|p| {
                p.techs
                    .iter()
                    .filter_map(|name| self.rules.techs.get(name).map(|spec| spec.era))
                    .chain(
                        p.civics
                            .iter()
                            .filter_map(|name| self.rules.civics.get(name).map(|spec| spec.era)),
                    )
            })
            .max()
            .unwrap_or(0)
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
        1.0 + p
            .counters
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
        self.players[pid].exoplanet_distance = (self.players[pid].exoplanet_distance
            + self.exoplanet_speed(pid))
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

    /// Current tourism generated by this civilization each turn.
    ///
    /// Great people are intentionally generic in this simulation. Each
    /// claimed Artist therefore represents a writer/artist/musician who
    /// automatically fills up to three available Great Work slots. Art and
    /// artifact slots use their themed value, and Printing doubles Writing.
    pub fn tourism_per_turn(&self, pid: usize) -> f64 {
        let mut tourism = 0.0;
        let mut work_slots: Vec<f64> = Vec::new();
        for city in self.cities.values().filter(|city| city.owner == pid) {
            tourism += 2.0 * city.wonders.len() as f64;
            if self.players[pid].techs.contains("flight") {
                for (district, position) in &city.districts {
                    let multiplier = self.rules.districts[district.as_str()]
                        .effects
                        .get("tourism_after_flight")
                        .copied()
                        .unwrap_or(0.0);
                    if multiplier > 0.0 {
                        tourism += self.district_yields(district, *position).culture * multiplier;
                    }
                }
            }
            for wonder in city.wonders.keys() {
                let spec = &self.rules.wonders[wonder.as_str()];
                tourism += spec.effects.get("tourism").copied().unwrap_or(0.0);
                for (kind, count) in &spec.great_work_slots {
                    let value = self.great_work_tourism(pid, kind);
                    work_slots.extend(std::iter::repeat(value).take((*count).max(0) as usize));
                }
            }
            for building in &city.buildings {
                let spec = &self.rules.buildings[building.as_str()];
                tourism += spec.effects.get("tourism").copied().unwrap_or(0.0);
                for (kind, count) in &spec.great_work_slots {
                    let value = self.great_work_tourism(pid, kind);
                    work_slots.extend(std::iter::repeat(value).take((*count).max(0) as usize));
                }
            }
            for pos in &city.owned_tiles {
                let Some(improvement) = self.map.tiles[pos].improvement.as_deref() else {
                    continue;
                };
                tourism += self.rules.improvements[improvement]
                    .effects
                    .get("tourism")
                    .copied()
                    .unwrap_or(0.0);
            }
        }

        work_slots.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let named_works = ["great_work:writing", "great_work:art", "great_work:music"]
            .into_iter()
            .map(|key| self.players[pid].counters.get(key).copied().unwrap_or(0))
            .sum::<i64>()
            .max(0) as usize;
        let great_works = if named_works > 0 {
            named_works
        } else {
            // Legacy saves used one generic Artist claim for three works.
            self.players[pid]
                .gp_claimed
                .get("artist")
                .copied()
                .unwrap_or(0)
                .max(0) as usize
                * 3
        };
        tourism += work_slots.into_iter().take(great_works).sum::<f64>();

        let culture = self
            .player_city_ids(pid)
            .into_iter()
            .map(|cid| self.city_yields(cid).culture)
            .sum::<f64>();
        let base = tourism + 0.15 * culture;
        base * (1.0 + self.tree_effect(pid, "tourism_pct") / 100.0)
    }

    fn great_work_tourism(&self, pid: usize, kind: &str) -> f64 {
        match kind {
            "writing" => 2.0 * (1.0 + self.tree_effect(pid, "writing_tourism_pct") / 100.0),
            "art" | "artifact" => {
                6.0 * (1.0 + self.policy_effect(pid, "art_artifact_tourism_pct") / 100.0)
            }
            "music" => 4.0 * (1.0 + self.policy_effect(pid, "music_tourism_pct") / 100.0),
            "any" => 4.0,
            "relic" => 8.0,
            "religious_art" => 3.0,
            _ => 0.0,
        }
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
                Some("monarchy") | Some("merchant_republic") | Some("theocracy") => 2.0,
                Some("communism") | Some("democracy") | Some("fascism") => 3.0,
                Some("corporate_libertarianism")
                | Some("digital_democracy")
                | Some("synthetic_technocracy") => 4.0,
                Some("autocracy") | Some("oligarchy") | Some("classical_republic") => 1.0,
                _ => 0.0,
            };
            let influence = 1.0 + tier + self.policy_effect(pid, "influence_per_turn");
            let p = &mut self.players[pid];
            p.influence += influence;
            if p.influence >= 100.0 {
                p.influence -= 100.0;
                p.envoys_free += 1;
            }
        }
        for uid in self.player_unit_ids(pid) {
            let (kind, hp, acted, attacks_left) = {
                let u = &self.units[&uid];
                (u.kind.clone(), u.hp, u.acted, u.attacks_left)
            };
            let start_owner = self.map.tiles[&self.units[&uid].pos]
                .owner_city
                .and_then(|city| self.cities.get(&city))
                .map(|city| city.owner);
            let territory_bonus = if start_owner == Some(pid) {
                self.policy_effect(pid, "friendly_start_movement")
            } else if start_owner.is_some_and(|owner| owner != pid) {
                self.policy_effect(pid, "enemy_start_movement")
            } else {
                0.0
            };
            let moves = self.unit_max_moves(uid) + territory_bonus;
            let attacks = self.unit_max_attacks(uid);
            let spec = &self.rules.units[kind.as_str()];
            let embarked = self.is_embarked(&self.units[&uid]);
            let heal = if hp < 100
                && (!acted
                    || (attacks_left < attacks
                        && self.promotion_effect(&self.units[&uid], "heal_after_attack") > 0.0))
            {
                self.unit_heal_rate(uid)
            } else {
                0
            };
            let u = self.units.get_mut(&uid).unwrap();
            u.moves_left = moves;
            u.attacks_left = attacks;
            u.moved = false;
            u.zoc_stopped = false;
            u.air_patrol = false;
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
        }
        // Snapshot ZOC after every per-turn stop flag has been cleared. This
        // distinguishes beginning a turn in ZOC from entering one mid-turn.
        for uid in self.player_unit_ids(pid) {
            let started_in_zoc =
                !self.unit_ignores_zoc(uid) && self.in_enemy_zoc_for(uid, self.units[&uid].pos);
            self.units.get_mut(&uid).unwrap().started_turn_in_zoc = started_in_zoc;
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
            let tourism = self.tourism_per_turn(pid);
            let p = &mut self.players[pid];
            p.culture_lifetime += cul;
            p.tourism_lifetime += tourism;
        }
        if let Some(r) = self.players[pid].religion.clone() {
            let following = self
                .cities
                .values()
                .filter(|c| self.city_religion(c) == Some(r.as_str()))
                .count() as f64;
            if self.players[pid]
                .religion_beliefs
                .iter()
                .any(|b| b == "tithe")
            {
                gold += (following / 4.0).floor();
            }
            if self.players[pid]
                .religion_beliefs
                .iter()
                .any(|b| b == "world_church")
            {
                cul += (following / 5.0).floor();
            }
        }
        let n_units = self.player_unit_ids(pid).len() as f64;
        // Simplified base maintenance is 1 Gold per unit past the first three.
        let maintenance = (1.0 - self.policy_effect(pid, "unit_maintenance_discount")).max(0.0);
        gold -= (n_units - 3.0).max(0.0) * maintenance;
        {
            let p = &mut self.players[pid];
            p.gold = (p.gold + gold).max(0.0);
            p.faith += faith;
        }
        let research = self.players[pid].research.clone();
        let mut completed_tech = None;
        if let Some(tech) = research {
            let cost = self.rules.techs[tech.as_str()].cost;
            let p = &mut self.players[pid];
            p.research_progress += sci;
            if p.research_progress >= cost {
                let first = p.techs.insert(tech.clone());
                p.research_overflow = p.research_progress - cost;
                p.research = None;
                p.research_progress = 0.0;
                completed_tech = Some((tech, first));
            }
        } else {
            self.players[pid].research_overflow += sci;
        }
        if let Some((node, first)) = completed_tech {
            self.apply_tree_completion(pid, true, &node, first);
        }
        let civic = self.players[pid].civic.clone();
        let mut completed_civic = None;
        if let Some(cv) = civic {
            let cost = self.rules.civics[cv.as_str()].cost;
            let p = &mut self.players[pid];
            p.civic_progress += cul;
            if p.civic_progress >= cost {
                let first = p.civics.insert(cv.clone());
                p.civic_overflow = p.civic_progress - cost;
                p.civic = None;
                p.civic_progress = 0.0;
                completed_civic = Some((cv, first));
            }
        } else {
            self.players[pid].civic_overflow += cul;
        }
        if let Some((node, first)) = completed_civic {
            self.apply_tree_completion(pid, false, &node, first);
        }
    }

    fn apply_tree_completion(&mut self, pid: usize, technology: bool, node: &str, first: bool) {
        let effects = if technology {
            self.rules.techs[node].effects.clone()
        } else {
            self.rules.civics[node].effects.clone()
        };
        *self.players[pid]
            .counters
            .entry(format!("tree_completions:{node}"))
            .or_insert(0) += 1;
        if first {
            self.players[pid].envoys_free +=
                effects.get("free_envoys").copied().unwrap_or(0.0) as i64;
            *self.players[pid]
                .counters
                .entry("district_governor_titles".to_string())
                .or_insert(0) += effects.get("governor_titles").copied().unwrap_or(0.0) as i64;
            self.players[pid].dvp += effects
                .get("diplomatic_victory_points")
                .copied()
                .unwrap_or(0.0) as i64;
        }
        *self.players[pid]
            .counters
            .entry("district_governor_titles".to_string())
            .or_insert(0) += effects
            .get("governor_titles_per_completion")
            .copied()
            .unwrap_or(0.0) as i64;
        *self.players[pid]
            .counters
            .entry("diplomatic_favor".to_string())
            .or_insert(0) += effects
            .get("diplomatic_favor_per_completion")
            .copied()
            .unwrap_or(0.0) as i64;
        if self.players[pid].dvp >= DIPLOMATIC_VICTORY_POINTS {
            self.set_winner(pid, "diplomatic");
        }
    }

    fn check_boosts(&mut self, pid: usize) {
        if self.players[pid].is_minor {
            return;
        }
        let techs: Vec<(String, f64, crate::rules::BoostSpec)> = self
            .rules
            .techs
            .iter()
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
        let civics: Vec<(String, f64, crate::rules::BoostSpec)> = self
            .rules
            .civics
            .iter()
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
        let cities: Vec<&City> = self.cities.values().filter(|c| c.owner == pid).collect();
        let trig = b.trigger.as_str();
        match trig {
            "kills" | "improvements" | "camps" | "captures" => {
                p.counters.get(trig).copied().unwrap_or(0) >= n
            }
            "cities" => cities.len() as i64 >= n,
            "districts" => cities.iter().map(|c| c.districts.len() as i64).sum::<i64>() >= n,
            "pop" => cities.iter().any(|c| c.pop as i64 >= n),
            "total_pop" => cities.iter().map(|c| c.pop as i64).sum::<i64>() >= n,
            "units" => {
                self.units
                    .values()
                    .filter(|u| {
                        u.owner == pid && self.rules.units[u.kind.as_str()].class == "military"
                    })
                    .count() as i64
                    >= n
            }
            "coastal_city" => cities.iter().any(|c| {
                self.nbrs(c.pos).iter().any(|nb| {
                    self.map
                        .get(*nb)
                        .map(|t| self.rules.is_water(t))
                        .unwrap_or(false)
                })
            }),
            "war" => self
                .players
                .iter()
                .any(|o| o.id != pid && !o.is_barbarian && self.is_at_war(pid, o.id)),
            _ => {
                if let Some(t) = trig.strip_prefix("units_of:") {
                    self.units
                        .values()
                        .filter(|u| u.owner == pid && u.kind == t)
                        .count() as i64
                        >= n
                } else if let Some(d) = trig.strip_prefix("district:") {
                    cities.iter().any(|c| c.districts.contains_key(d))
                } else if let Some(bn) = trig.strip_prefix("building:") {
                    cities
                        .iter()
                        .filter(|c| c.buildings.iter().any(|x| x == bn))
                        .count() as i64
                        >= n
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
        self.cities.get_mut(&cid).unwrap().encampment_struck = false;
        let ys = self.city_yields(cid);
        let housing = self.city_housing(&self.cities[&cid]);
        let am = self.city_amenity_surplus(&self.cities[&cid]);
        let repair_project = matches!(
            self.cities[&cid].queue.first(),
            Some(Item::Project { project }) if project == "repair_outer_defenses"
        );
        let production_before = self.cities[&cid].production;
        let base_produced = ys.production;
        let production_multiplier = {
            let city = &self.cities[&cid];
            self.item_prod_mult(pid, cid, city.queue.first())
        };
        let mut growth_bonus = self.empire_building_sum(pid, |b| b.growth_pct);
        growth_bonus += self.empire_wonder_effect(pid, "empire_growth_pct");
        if self.players[pid].pantheon.as_deref() == Some("fertility_rites") {
            growth_bonus += 10.0;
        }
        {
            let city = self.cities.get_mut(&cid).unwrap();
            let mut surplus = ys.food - 2.0 * city.pop as f64;
            if surplus > 0.0 {
                let headroom = housing - city.pop as f64;
                let hf = Self::housing_growth_mult(headroom);
                let af = Self::amenity_growth_mult(am);
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
            let produced = base_produced * production_multiplier;
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
            } else if !self.cities[&cid].queue.is_empty() {
                self.cities.get_mut(&cid).unwrap().production += produced;
            }
        }
        let queue_head = self.cities[&cid].queue.first().cloned();
        if let Some(item) = queue_head {
            if matches!(&item, Item::Project { project } if project == "repair_outer_defenses") {
                // Repair applies Production directly to wall HP above.
            } else {
                let cost = self.item_cost_for(pid, &item);
                let stalled = matches!(&item, Item::Unit { unit } if unit == "settler")
                    && self.cities[&cid].pop < 2;
                if !stalled && self.cities[&cid].production >= cost {
                    if self.complete_item(pid, cid, &item) {
                        // Gathering Storm strips item-specific Production
                        // bonuses from overflow. Only unspent base Production
                        // and previously banked base progress carry forward.
                        let overflow = if production_before >= cost {
                            production_before - cost + base_produced
                        } else {
                            let remaining = cost - production_before;
                            base_produced - remaining / production_multiplier.max(f64::EPSILON)
                        }
                        .max(0.0);
                        let city = self.cities.get_mut(&cid).unwrap();
                        city.queue.remove(0);
                        city.production = overflow;
                        if let Some(next) = city.queue.first() {
                            let key = Self::item_progress_key(next);
                            city.production += city.production_progress.remove(&key).unwrap_or(0.0);
                        }
                    } else if self.cities[&cid].queue.first() == Some(&item) {
                        // A completed unit waiting for an open placement tile
                        // does not bank more Production every turn.
                        self.cities.get_mut(&cid).unwrap().production = cost;
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
        let encampment = self.city_district_family_position(&self.cities[&cid], "encampment");
        let encampment_besieged =
            encampment.is_some_and(|position| self.district_under_siege(pid, position));
        let city = self.cities.get_mut(&cid).unwrap();
        if !besieged {
            city.hp = (city.hp + 20).min(200); // Civ 6 heal rate
        }
        if encampment.is_some() && !city.encampment_pillaged && !encampment_besieged {
            city.encampment_hp = (city.encampment_hp + 20).min(100);
        }
        ys
    }

    fn complete_item(&mut self, pid: usize, cid: u32, item: &Item) -> bool {
        match item {
            Item::Unit { unit } => {
                let pos = self.cities[&cid].pos;
                let Some(placed) = self.place_new_unit(unit, pid, pos) else {
                    return false;
                };
                self.apply_training_district_effects(cid, placed);
                if unit == "settler" {
                    self.cities.get_mut(&cid).unwrap().pop -= 1;
                }
                if unit == "settler" || unit == "builder" {
                    bump(&mut self.players[pid], &format!("trained:{unit}"));
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
                self.cities
                    .get_mut(&cid)
                    .unwrap()
                    .buildings
                    .push(building.clone());
                if spec.outer_defense > 0 {
                    let has_encampment =
                        self.city_has_district_family(&self.cities[&cid], "encampment");
                    let city = self.cities.get_mut(&cid).unwrap();
                    city.wall_hp += spec.outer_defense;
                    if has_encampment && !city.encampment_pillaged {
                        city.encampment_wall_hp += spec.outer_defense;
                    }
                }
                if spec.wonder {
                    self.players[pid].era_score += 3;
                }
                if spec.unit_levels > 0 {
                    for uid in self.player_unit_ids(pid) {
                        let mil =
                            self.rules.units[self.units[&uid].kind.as_str()].class == "military";
                        if mil {
                            let u = self.units.get_mut(&uid).unwrap();
                            u.xp = u.xp.max(Self::promotion_threshold(u.level));
                        }
                    }
                }
                if spec.district.as_deref().is_some_and(|district| {
                    self.district_is_family(district, "entertainment_complex")
                }) && self.city_district_effect(&self.cities[&cid], "free_heavy_cavalry") > 0.0
                {
                    self.grant_heavy_cavalry(pid, cid);
                }
                true
            }
            Item::District { district, pos } => {
                if !self.district_sites(cid, district).contains(pos) {
                    return false;
                }
                let spec = self.rules.districts[district.as_str()].clone();
                let preserve_feature = self.players[pid].civ == "Vietnam" && spec.specialty;
                let t = self.map.tiles.get_mut(pos).unwrap();
                t.district = Some(district.clone());
                t.improvement = None;
                t.pillaged = false;
                if !preserve_feature {
                    t.feature = None;
                }
                self.cities
                    .get_mut(&cid)
                    .unwrap()
                    .districts
                    .insert(district.clone(), *pos);
                if self.district_is_family(district, "encampment") {
                    let max_wall = self.city_max_wall_hp(&self.cities[&cid]);
                    let city = self.cities.get_mut(&cid).unwrap();
                    city.encampment_hp = 100;
                    city.encampment_wall_hp = max_wall;
                    city.encampment_pillaged = false;
                }
                if let Some(amount) = spec.effects.get("governor_titles") {
                    *self.players[pid]
                        .counters
                        .entry("district_governor_titles".to_string())
                        .or_insert(0) += *amount as i64;
                }
                if let Some(amount) = spec.effects.get("envoys") {
                    self.players[pid].envoys_free += *amount as i64;
                }
                if let Some(amount) = spec.effects.get("envoy_if_adjacent_city_center") {
                    if self.nbrs(*pos).contains(&self.cities[&cid].pos) {
                        self.players[pid].envoys_free += *amount as i64;
                    }
                }
                if spec
                    .effects
                    .get("unlock_apprenticeship")
                    .copied()
                    .unwrap_or(0.0)
                    > 0.0
                {
                    let player = &mut self.players[pid];
                    player.techs.insert("apprenticeship".to_string());
                    if player.research.as_deref() == Some("apprenticeship") {
                        player.research = None;
                        player.research_progress = 0.0;
                    }
                }
                if spec.effects.get("culture_bomb").copied().unwrap_or(0.0) > 0.0 {
                    self.culture_bomb(cid, *pos);
                }
                if spec
                    .effects
                    .get("free_heavy_cavalry")
                    .copied()
                    .unwrap_or(0.0)
                    > 0.0
                {
                    self.grant_heavy_cavalry(pid, cid);
                }
                true
            }
            Item::Wonder { wonder, pos } => {
                if self.wonder_built(wonder) {
                    self.cities.get_mut(&cid).unwrap().queue.clear();
                    return false;
                }
                if !self.wonder_sites(cid, wonder).contains(pos) {
                    return false;
                }
                let spec = self.rules.wonders[wonder.as_str()].clone();
                let tile = self.map.tiles.get_mut(pos).unwrap();
                tile.wonder = Some(wonder.clone());
                tile.improvement = None;
                tile.pillaged = false;
                tile.feature = None;
                self.cities
                    .get_mut(&cid)
                    .unwrap()
                    .wonders
                    .insert(wonder.clone(), *pos);
                self.players[pid].era_score += 3;
                if spec.effects.get("free_builder").copied().unwrap_or(0.0) > 0.0 {
                    let city_pos = self.cities[&cid].pos;
                    self.place_new_unit("builder", pid, city_pos);
                }
                if spec
                    .effects
                    .get("promote_all_current_units")
                    .copied()
                    .unwrap_or(0.0)
                    > 0.0
                {
                    for uid in self.player_unit_ids(pid) {
                        if self.rules.units[self.units[&uid].kind.as_str()].class == "military" {
                            let unit = self.units.get_mut(&uid).unwrap();
                            unit.xp = unit.xp.max(Self::promotion_threshold(unit.level));
                        }
                    }
                }
                true
            }
            Item::Repair { repair, pos } => {
                if repair == "district" {
                    self.map.tiles.get_mut(pos).unwrap().pillaged = false;
                } else {
                    self.cities
                        .get_mut(&cid)
                        .unwrap()
                        .pillaged_buildings
                        .remove(repair);
                }
                true
            }
            Item::Project { project } => {
                if project == "repair_outer_defenses" {
                    return true;
                }
                if project == "repair_encampment" {
                    let max_wall = self.city_max_wall_hp(&self.cities[&cid]);
                    let city = self.cities.get_mut(&cid).unwrap();
                    city.encampment_hp = 100;
                    city.encampment_wall_hp = max_wall;
                    city.encampment_pillaged = false;
                    return true;
                }
                let spec = self.rules.projects[project.as_str()].clone();
                if !spec.repeatable && self.players[pid].science_projects.contains(project) {
                    // Another city won this internal project race.
                    return true;
                }
                if spec.repeatable {
                    bump(&mut self.players[pid], &format!("project:{project}"));
                } else {
                    self.players[pid].science_projects.insert(project.clone());
                }
                for (effect, amount) in &spec.effects {
                    *self.players[pid]
                        .counters
                        .entry(format!("project_effect:{effect}"))
                        .or_insert(0) += *amount as i64;
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

    fn apply_training_district_effects(&mut self, cid: u32, uid: u32) {
        if self.rules.units[self.units[&uid].kind.as_str()]
            .domain
            .as_deref()
            == Some("sea")
        {
            let movement = self.city_district_effect(&self.cities[&cid], "naval_movement");
            self.units.get_mut(&uid).unwrap().bonus_moves += movement;
        }
    }

    fn grant_heavy_cavalry(&mut self, pid: usize, cid: u32) {
        let mut candidates: Vec<String> = self
            .rules
            .units
            .iter()
            .filter(|(_, spec)| spec.promotion_class == "heavy_cavalry")
            .filter(|(_, spec)| self.unlocked(pid, &spec.tech, &spec.civic))
            .filter(|(_, spec)| {
                spec.unique_to
                    .as_deref()
                    .is_none_or(|civilization| civilization == self.players[pid].civ)
            })
            .map(|(name, _)| name.clone())
            .collect();
        candidates.sort_by(|a, b| {
            self.rules.units[b.as_str()]
                .cost
                .partial_cmp(&self.rules.units[a.as_str()].cost)
                .unwrap()
                .then(a.cmp(b))
        });
        if let Some(kind) = candidates.first() {
            if let Some(unit) = self.place_new_unit(kind, pid, self.cities[&cid].pos) {
                self.apply_training_district_effects(cid, unit);
            }
        }
    }

    fn place_new_unit(&mut self, kind: &str, owner: usize, pos: Pos) -> Option<u32> {
        let spec = self.rules.units[kind].clone();
        if spec.domain.as_deref() == Some("air") {
            let mut bases = vec![pos];
            bases.extend(self.nbrs(pos));
            if let Some(base) = bases
                .into_iter()
                .find(|base| self.can_air_base_at(owner, *base, None))
            {
                return Some(self.spawn_unit(kind, owner, base));
            }
            return None;
        }
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

    fn culture_bomb(&mut self, cid: u32, center: Pos) {
        let claims: Vec<Pos> = self
            .nbrs(center)
            .into_iter()
            .filter(|position| {
                self.map
                    .get(*position)
                    .is_some_and(|tile| tile.owner_city.is_none())
            })
            .collect();
        for position in claims {
            self.map.tiles.get_mut(&position).unwrap().owner_city = Some(cid);
            if !self.cities[&cid].owned_tiles.contains(&position) {
                self.cities
                    .get_mut(&cid)
                    .unwrap()
                    .owned_tiles
                    .push(position);
            }
        }
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
                    Some((bk, _)) => key.0 > bk.0 || (key.0 == bk.0 && key.1 > bk.1),
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
        let defensive_buildings: BTreeSet<String> = self
            .rules
            .buildings
            .iter()
            .filter(|(_, spec)| spec.outer_defense > 0)
            .map(|(name, _)| name.clone())
            .collect();
        {
            let city = self.cities.get_mut(&cid).unwrap();
            city.owner = new_owner;
            city.pop = (city.pop - 1).max(1);
            city.hp = 100;
            city.queue.clear();
            // Civ 6: walls are destroyed outright when a city falls
            city.buildings
                .retain(|building| !defensive_buildings.contains(building));
            city.wall_hp = 0;
            city.encampment_wall_hp = 0;
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
                match self
                    .cities
                    .values()
                    .find(|c| c.is_capital && c.original_owner == original_owner)
                {
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
        self.players
            .get(pid)
            .is_some_and(|p| p.alive && !p.is_minor && !p.is_barbarian)
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
        g.map.clear_rivers();
        for tile in g.map.tiles.values_mut() {
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.owner_city = None;
            tile.hills = false;
            tile.road = false;
        }
        let center = *g
            .map
            .tiles
            .keys()
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
        let plain_friendly = ring
            .iter()
            .copied()
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
            (
                g.spawn_unit("warrior", 0, center),
                HealingLocation::District,
                20,
            ),
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
    fn naval_and_embarked_units_only_heal_in_friendly_territory() {
        let (mut g, center, ring) = controlled_game(3001);
        let cid = g.found_city_for(0, center, None);
        let friendly = ring[0];
        g.map.tiles.get_mut(&friendly).unwrap().owner_city = Some(cid);
        g.map.tiles.get_mut(&friendly).unwrap().terrain = "coast".to_string();
        let neutral = g
            .wdisk(center, 2)
            .into_iter()
            .find(|pos| g.wdist(center, *pos) == 2 && g.map.tiles[pos].owner_city.is_none())
            .unwrap();
        g.map.tiles.get_mut(&neutral).unwrap().terrain = "coast".to_string();

        let galley_home = g.spawn_unit("galley", 0, friendly);
        let galley_away = g.spawn_unit("galley", 0, neutral);
        let embarked = g.spawn_unit("warrior", 0, friendly);
        assert_eq!(g.unit_heal_rate(galley_home), 20);
        assert_eq!(g.unit_heal_rate(embarked), 20);
        assert_eq!(g.unit_heal_rate(galley_away), 0);
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
        g.apply(
            0,
            &Action::Attack {
                unit: attacker,
                target,
            },
        )
        .unwrap();
        assert_eq!(g.units[&defender].hp, 100 - expected_out);
        assert_eq!(g.units[&attacker].hp, 100 - expected_in);

        let (mut g, target, ring) = controlled_game(302);
        let spear = g.spawn_unit("spearman", 0, ring[0]);
        let war_cart = g.spawn_unit("war_cart", 1, target);
        assert_eq!(
            g.matchup_bonus(spear, &g.units[&war_cart], true),
            0.0,
            "War-Carts are immune to the anti-cavalry modifier"
        );
        let maryannu = g.spawn_unit("maryannu_chariot_archer", 1, ring[1]);
        assert_eq!(
            g.matchup_bonus(spear, &g.units[&maryannu], true),
            10.0,
            "ranged cavalry still receives the anti-cavalry modifier"
        );
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
        assert_eq!(
            g.flanking_bonus(attacker, target),
            2.0,
            "a ranged military unit provides one flanking stack"
        );
        assert_eq!(g.support_bonus(&g.units[&defender]), 2.0);

        // Rivers block flanking but not support.
        assert!(g.map.set_river_edge(ring[1], target, true));
        assert_eq!(g.flanking_bonus(attacker, target), 0.0);
        assert!(g.map.set_river_edge(ring[2], target, true));
        assert_eq!(g.support_bonus(&g.units[&defender]), 2.0);

        // Embarked land units provide Support but cannot provide Flanking.
        assert!(g.map.set_river_edge(ring[1], target, false));
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
        let from = g
            .wdisk(target, 2)
            .into_iter()
            .find(|p| g.wdist(*p, target) == 2)
            .unwrap();
        let attacker = g.spawn_unit("archer", 0, from);
        let defender = g.spawn_unit("warrior", 1, target);
        let middles: Vec<Pos> = g
            .nbrs(from)
            .into_iter()
            .filter(|p| g.wdist(*p, target) == 1)
            .collect();
        assert!(!middles.is_empty());
        for middle in &middles {
            g.map.tiles.get_mut(middle).unwrap().terrain = "mountain".to_string();
        }
        let shot = Action::Ranged {
            unit: attacker,
            target,
        };
        let legal_shot = |g: &Game| {
            g.legal_actions(0).into_iter().any(|action| {
                matches!(action, Action::Ranged { unit, target: to }
                if unit == attacker && to == target)
            })
        };
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
        g.map.tiles.get_mut(&target).unwrap().feature = Some("forest".to_string());
        assert!(g.map.set_river_edge(ring[0], target, true));
        let attack = Action::Attack {
            unit: attacker,
            target,
        };
        let legal_attack = |g: &Game| {
            g.legal_actions(0).into_iter().any(|action| {
                matches!(action, Action::Attack { unit, target: to }
                if unit == attacker && to == target)
            })
        };

        g.units.get_mut(&attacker).unwrap().moves_left = 1.0;
        assert!(!legal_attack(&g));
        assert_eq!(
            g.apply(0, &attack).unwrap_err(),
            "not enough movement to attack"
        );

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
        for name in [
            "scout",
            "warrior",
            "spearman",
            "horseman",
            "infantry",
            "tank",
            "helicopter",
            "galley",
            "quadrireme",
            "frigate",
            "privateer",
            "battleship",
            "destroyer",
            "aircraft_carrier",
            "missile_cruiser",
            "giant_death_robot",
        ] {
            assert!(g.rules.units[name].zone_of_control, "{name} must exert ZOC");
        }

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
            "artillery",
            "machine_gun",
            "anti_air_gun",
            "mobile_sam",
            "observation_balloon",
            "submarine",
            "nuclear_submarine",
        ] {
            assert!(
                !g.rules.units[name].zone_of_control,
                "{name} must not exert ZOC"
            );
        }
        assert!(g
            .players
            .iter()
            .all(|p| !p.civics.contains("military_tradition")));
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
        g.apply(
            0,
            &Action::Move {
                unit: warrior,
                to: entry,
            },
        )
        .unwrap();
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
        g.apply(
            0,
            &Action::Move {
                unit: horse,
                to: entry,
            },
        )
        .unwrap();
        assert!(!g.units[&horse].zoc_stopped);
        assert!(g.units[&horse].moves_left > 0.0);

        let (mut g, enemy_pos, ring) = controlled_game(309);
        g.spawn_unit("warrior", 1, enemy_pos);
        assert!(g.map.set_river_edge(enemy_pos, ring[0], true));
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
            g.apply(
                0,
                &Action::Move {
                    unit: mover,
                    to: entry,
                },
            )
            .unwrap();
            assert_eq!(g.units[&mover].moves_left, 0.0, "{kind}");
            assert!(g.units[&mover].zoc_stopped, "{kind}");
            assert!(
                !g.legal_actions(0).iter().any(|action| {
                    matches!(action, Action::Improve { unit, .. } if *unit == mover)
                        || matches!(action, Action::UnlinkUnits { unit } if *unit == mover)
                }),
                "{kind} must not receive follow-up actions after entering ZOC"
            );
            if kind == "builder" {
                assert_eq!(
                    g.apply(
                        0,
                        &Action::Improve {
                            unit: mover,
                            improvement: "farm".to_string(),
                        },
                    )
                    .unwrap_err(),
                    "non-combat unit cannot act after entering zone of control"
                );
            }
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
        g.apply(
            0,
            &Action::Move {
                unit: missionary,
                to: entry,
            },
        )
        .unwrap();
        assert!(g.units[&missionary].zoc_stopped);
        assert!(g.units[&missionary].moves_left > 0.0);

        let (mut g, enemy_pos, ring) = controlled_game(3121);
        g.at_war.clear();
        g.players[0].religion = Some("A".to_string());
        g.players[1].religion = Some("A".to_string());
        g.spawn_unit("missionary", 1, enemy_pos);
        let entry = ring[0];
        let start = g
            .nbrs(entry)
            .into_iter()
            .find(|p| g.wdist(*p, enemy_pos) == 2)
            .unwrap();
        let missionary = g.spawn_unit("missionary", 0, start);
        g.apply(
            0,
            &Action::Move {
                unit: missionary,
                to: entry,
            },
        )
        .unwrap();
        assert!(!g.units[&missionary].zoc_stopped);

        let (mut g, city_pos, ring) = controlled_game(313);
        g.found_city_for(1, city_pos, Some("Test".to_string()));
        assert!(g.in_enemy_zoc(0, ring[0]));
    }

    #[test]
    fn naval_surface_units_exert_zoc_and_naval_raiders_ignore_it() {
        let (mut g, enemy_pos, ring) = controlled_game(314);
        let entry = ring[0];
        let start = g
            .nbrs(entry)
            .into_iter()
            .find(|p| g.wdist(*p, enemy_pos) == 2)
            .unwrap();
        for pos in [enemy_pos, entry, start] {
            g.map.tiles.get_mut(&pos).unwrap().terrain = "coast".to_string();
        }
        g.spawn_unit("quadrireme", 1, enemy_pos);
        assert!(g.in_enemy_zoc(0, entry), "naval ranged units exert ZOC");
        let privateer = g.spawn_unit("privateer", 0, start);
        g.apply(
            0,
            &Action::Move {
                unit: privateer,
                to: entry,
            },
        )
        .unwrap();
        assert!(!g.units[&privateer].zoc_stopped);
        assert!(g.units[&privateer].moves_left > 0.0);

        let (mut g, enemy_pos, ring) = controlled_game(315);
        for pos in std::iter::once(enemy_pos).chain(ring.iter().copied()) {
            g.map.tiles.get_mut(&pos).unwrap().terrain = "coast".to_string();
        }
        g.spawn_unit("privateer", 1, enemy_pos);
        assert!(g.in_enemy_zoc(0, ring[0]), "Privateers also project ZOC");

        let (mut g, enemy_pos, ring) = controlled_game(316);
        for pos in std::iter::once(enemy_pos).chain(ring.iter().copied()) {
            g.map.tiles.get_mut(&pos).unwrap().terrain = "coast".to_string();
        }
        g.spawn_unit("submarine", 1, enemy_pos);
        assert!(
            !g.in_enemy_zoc(0, ring[0]),
            "Submarines are the naval projection exception"
        );
    }

    #[test]
    fn linked_noncombat_units_inherit_their_escorts_zoc_behavior() {
        let (mut g, enemy_pos, ring) = controlled_game(317);
        g.spawn_unit("warrior", 1, enemy_pos);
        let entry = ring[0];
        let start = g
            .nbrs(entry)
            .into_iter()
            .find(|p| g.wdist(*p, enemy_pos) == 2)
            .unwrap();
        let horse = g.spawn_unit("horseman", 0, start);
        let ram = g.spawn_unit("battering_ram", 0, start);
        g.apply(
            0,
            &Action::LinkUnits {
                unit: horse,
                with: ram,
            },
        )
        .unwrap();
        g.apply(
            0,
            &Action::Move {
                unit: horse,
                to: entry,
            },
        )
        .unwrap();
        assert!(!g.units[&horse].zoc_stopped);
        assert!(!g.units[&ram].zoc_stopped);
        assert!(g.units[&ram].moves_left > 0.0);

        let (mut g, enemy_pos, ring) = controlled_game(318);
        g.spawn_unit("warrior", 1, enemy_pos);
        let entry = ring[0];
        let start = g
            .nbrs(entry)
            .into_iter()
            .find(|p| g.wdist(*p, enemy_pos) == 2)
            .unwrap();
        let escort = g.spawn_unit("warrior", 0, start);
        let ram = g.spawn_unit("battering_ram", 0, start);
        g.apply(
            0,
            &Action::LinkUnits {
                unit: escort,
                with: ram,
            },
        )
        .unwrap();
        g.apply(
            0,
            &Action::Move {
                unit: escort,
                to: entry,
            },
        )
        .unwrap();
        assert!(g.units[&escort].zoc_stopped);
        assert!(g.units[&ram].zoc_stopped);
        assert_eq!(g.units[&ram].moves_left, 0.0);
        assert!(g.apply(0, &Action::UnlinkUnits { unit: ram }).is_err());
    }

    #[test]
    fn starting_in_zoc_allows_leaving_first_but_not_after_attacking() {
        let (mut g, enemy_pos, ring) = controlled_game(319);
        let entry = ring[0];
        let escape = g
            .nbrs(entry)
            .into_iter()
            .find(|p| g.wdist(*p, enemy_pos) == 2)
            .unwrap();
        g.spawn_unit("warrior", 1, enemy_pos);
        let warrior = g.spawn_unit("warrior", 0, entry);
        g.units
            .get_mut(&warrior)
            .unwrap()
            .promotions
            .insert("elite_guard".to_string());
        g.begin_turn(0);
        assert!(g.units[&warrior].started_turn_in_zoc);
        g.apply(
            0,
            &Action::Attack {
                unit: warrior,
                target: enemy_pos,
            },
        )
        .unwrap();
        assert!(g.units.contains_key(&warrior));
        assert!(g.units[&warrior].moves_left > 0.0);
        assert!(!g.can_move(warrior, escape));

        let (mut g, enemy_pos, ring) = controlled_game(320);
        let entry = ring[0];
        let escape = g
            .nbrs(entry)
            .into_iter()
            .find(|p| g.wdist(*p, enemy_pos) == 2)
            .unwrap();
        g.spawn_unit("warrior", 1, enemy_pos);
        let warrior = g.spawn_unit("warrior", 0, entry);
        g.begin_turn(0);
        assert!(g.units[&warrior].started_turn_in_zoc);
        assert!(g.can_move(warrior, escape));
        g.apply(
            0,
            &Action::Move {
                unit: warrior,
                to: escape,
            },
        )
        .unwrap();
        assert_eq!(g.units[&warrior].pos, escape);
        assert!(!g.units[&warrior].zoc_stopped);
    }

    #[test]
    fn melee_advance_into_a_second_zoc_stops_move_after_attack_units() {
        let (mut g, provider_pos, ring) = controlled_game(3201);
        let target = ring[0];
        let start = g
            .nbrs(target)
            .into_iter()
            .find(|p| g.wdist(*p, provider_pos) == 2)
            .unwrap();
        let reserve = g
            .map
            .tiles
            .keys()
            .copied()
            .find(|p| g.wdist(*p, provider_pos) >= 4)
            .unwrap();
        g.spawn_unit("settler", 1, reserve);
        g.spawn_unit("warrior", 1, provider_pos);
        let victim = g.spawn_unit("scout", 1, target);
        g.units.get_mut(&victim).unwrap().hp = 1;
        let scout = g.spawn_unit("scout", 0, start);
        g.units
            .get_mut(&scout)
            .unwrap()
            .promotions
            .insert("guerrilla".to_string());
        g.apply(
            0,
            &Action::Attack {
                unit: scout,
                target,
            },
        )
        .unwrap();
        assert_eq!(g.units[&scout].pos, target);
        assert!(g.units[&scout].moves_left > 0.0);
        assert!(g.in_enemy_zoc_for(scout, target));
        assert!(g.units[&scout].zoc_stopped);
        assert!(g.reachable(scout).is_empty());
    }

    #[test]
    fn stopped_combatants_can_promote_and_suppression_projects_zoc() {
        let (mut g, enemy_pos, ring) = controlled_game(321);
        g.spawn_unit("warrior", 1, enemy_pos);
        let entry = ring[0];
        let start = g
            .nbrs(entry)
            .into_iter()
            .find(|p| g.wdist(*p, enemy_pos) == 2)
            .unwrap();
        let archer = g.spawn_unit("archer", 0, start);
        g.units.get_mut(&archer).unwrap().xp = 15;
        g.apply(
            0,
            &Action::Move {
                unit: archer,
                to: entry,
            },
        )
        .unwrap();
        assert!(g.units[&archer].zoc_stopped);
        assert!(g.units[&archer].acted);
        assert!(g.units[&archer].moves_left > 0.0);
        let promotion = g.available_promotions(archer).into_iter().next().unwrap();
        g.apply(
            0,
            &Action::Promote {
                unit: archer,
                promotion,
            },
        )
        .unwrap();

        let (mut g, enemy_pos, ring) = controlled_game(322);
        let archer = g.spawn_unit("archer", 1, enemy_pos);
        assert!(!g.in_enemy_zoc(0, ring[0]));
        g.units
            .get_mut(&archer)
            .unwrap()
            .promotions
            .insert("suppression".to_string());
        assert!(g.in_enemy_zoc(0, ring[0]));
    }

    #[test]
    fn zoc_respects_native_domains_and_unpillaged_districts() {
        let (mut g, enemy_pos, ring) = controlled_game(323);
        g.map.tiles.get_mut(&enemy_pos).unwrap().terrain = "coast".to_string();
        g.map.tiles.get_mut(&ring[0]).unwrap().terrain = "coast".to_string();
        g.spawn_unit("warrior", 1, enemy_pos);
        assert!(
            !g.in_enemy_zoc(0, ring[0]),
            "embarked land units do not project onto adjacent water"
        );

        let (mut g, enemy_pos, ring) = controlled_game(324);
        g.map.tiles.get_mut(&enemy_pos).unwrap().terrain = "coast".to_string();
        g.spawn_unit("galley", 1, enemy_pos);
        assert!(
            !g.in_enemy_zoc(0, ring[0]),
            "naval units do not project onto adjacent land"
        );

        let (mut g, city_pos, ring) = controlled_game(325);
        g.found_city_for(1, city_pos, None);
        g.map.tiles.get_mut(&ring[0]).unwrap().terrain = "coast".to_string();
        assert!(g.map.set_river_edge(city_pos, ring[0], true));
        assert!(
            g.in_enemy_zoc(0, ring[0]),
            "City Centers project across rivers and into water"
        );

        let (mut g, city_pos, ring) = controlled_game(326);
        let cid = g.found_city_for(1, city_pos, None);
        let camp = ring[0];
        let target = g
            .nbrs(camp)
            .into_iter()
            .find(|p| g.wdist(*p, city_pos) == 2)
            .unwrap();
        {
            let tile = g.map.tiles.get_mut(&camp).unwrap();
            tile.district = Some("encampment".to_string());
            tile.owner_city = Some(cid);
        }
        g.cities.get_mut(&cid).unwrap().encampment_hp = 0;
        g.cities.get_mut(&cid).unwrap().encampment_pillaged = false;
        assert!(g.in_enemy_zoc(0, target));
        g.cities.get_mut(&cid).unwrap().encampment_pillaged = true;
        assert!(!g.in_enemy_zoc(0, target));

        let (mut g, city_pos, ring) = controlled_game(327);
        let cid = g.found_city_for(1, city_pos, None);
        let oppidum = ring[0];
        let target = g
            .nbrs(oppidum)
            .into_iter()
            .find(|p| g.wdist(*p, city_pos) == 2)
            .unwrap();
        {
            let tile = g.map.tiles.get_mut(&oppidum).unwrap();
            tile.district = Some("oppidum".to_string());
            tile.owner_city = Some(cid);
            tile.pillaged = false;
        }
        assert!(g.in_enemy_zoc(0, target));
        g.map.tiles.get_mut(&oppidum).unwrap().pillaged = true;
        assert!(!g.in_enemy_zoc(0, target));
    }

    #[test]
    fn religious_layer_and_undefended_unit_capture_follow_civ6_targeting() {
        let (mut g, target, ring) = controlled_game(3131);
        g.at_war.clear();
        let missionary = g.spawn_unit("missionary", 1, target);
        let warrior = g.spawn_unit("warrior", 0, ring[0]);
        g.apply(
            0,
            &Action::Move {
                unit: warrior,
                to: target,
            },
        )
        .unwrap();
        assert_eq!(g.units[&missionary].pos, target);
        assert_eq!(
            g.units[&warrior].pos, target,
            "military and religious units use separate map layers"
        );

        let (mut g, target, ring) = controlled_game(3132);
        let builder = g.spawn_unit("builder", 1, target);
        let warrior = g.spawn_unit("warrior", 0, ring[0]);
        let archer = g.spawn_unit("archer", 0, ring[1]);
        assert!(
            g.apply(
                0,
                &Action::Ranged {
                    unit: archer,
                    target
                }
            )
            .is_err(),
            "ranged attacks cannot target undefended civilians"
        );
        assert!(!g.legal_actions(0).into_iter().any(|action| {
            matches!(action, Action::Attack { unit, target: to }
                if unit == warrior && to == target)
                || matches!(action, Action::Ranged { unit, target: to }
                    if unit == archer && to == target)
        }));
        g.apply(
            0,
            &Action::Move {
                unit: warrior,
                to: target,
            },
        )
        .unwrap();
        assert_eq!(g.units[&builder].owner, 0);

        let (mut g, city_pos, ring) = controlled_game(3133);
        let cid = g.found_city_for(0, city_pos, None);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .buildings
            .push("walls".to_string());
        g.cities.get_mut(&cid).unwrap().wall_hp = 100;
        let civilian_pos = ring[0];
        g.spawn_unit("builder", 1, civilian_pos);
        assert!(!g.legal_actions(0).into_iter().any(|action| {
            matches!(action, Action::CityStrike { city, target }
                if city == cid && target == civilian_pos)
        }));
        assert!(g
            .apply(
                0,
                &Action::CityStrike {
                    city: cid,
                    target: civilian_pos,
                }
            )
            .is_err());
    }

    #[test]
    fn city_garrisons_are_protected_and_a_siege_ring_prevents_healing() {
        let (mut g, city_pos, ring) = controlled_game(314);
        let cid = g.found_city_for(1, city_pos, Some("Test".to_string()));
        let garrison = g.spawn_unit("warrior", 1, city_pos);
        let archer = g.spawn_unit("archer", 0, ring[0]);
        let before = g.cities[&cid].hp;
        g.apply(
            0,
            &Action::Ranged {
                unit: archer,
                target: city_pos,
            },
        )
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
    fn ranged_and_bombard_city_final_blows_and_scythia_healing_follow_civ6() {
        let (mut g, city_pos, ring) = controlled_game(3141);
        let cid = g.found_city_for(1, city_pos, None);
        g.cities.get_mut(&cid).unwrap().hp = 1;
        let archer = g.spawn_unit("archer", 0, ring[0]);
        g.apply(
            0,
            &Action::Ranged {
                unit: archer,
                target: city_pos,
            },
        )
        .unwrap();
        assert_eq!(g.cities[&cid].hp, 1);
        assert_eq!(g.units[&archer].xp, 3);

        let catapult = g.spawn_unit("catapult", 0, ring[1]);
        g.apply(
            0,
            &Action::Ranged {
                unit: catapult,
                target: city_pos,
            },
        )
        .unwrap();
        assert_eq!(
            g.cities[&cid].hp, 0,
            "Bombard attacks may deplete but cannot capture cities"
        );
        assert_eq!(g.cities[&cid].owner, 1);
        assert_eq!(g.units[&catapult].xp, 10);

        let second_catapult = g.spawn_unit("catapult", 0, ring[2]);
        g.apply(
            0,
            &Action::Ranged {
                unit: second_catapult,
                target: city_pos,
            },
        )
        .unwrap();
        assert_eq!(g.cities[&cid].hp, 0);
        assert_eq!(
            g.units[&second_catapult].xp, 0,
            "attacks after city depletion grant no XP"
        );

        let (mut g, target, ring) = controlled_game(3142);
        g.players[0].civ = "Scythia".to_string();
        let archer = g.spawn_unit("archer", 0, ring[0]);
        g.units.get_mut(&archer).unwrap().hp = 50;
        let defender = g.spawn_unit("warrior", 1, target);
        g.units.get_mut(&defender).unwrap().hp = 1;
        g.apply(
            0,
            &Action::Ranged {
                unit: archer,
                target,
            },
        )
        .unwrap();
        assert!(!g.units.contains_key(&defender));
        assert_eq!(g.units[&archer].hp, 80);

        let (mut g, target, ring) = controlled_game(3145);
        g.players[1].civ = "Scythia".to_string();
        let attacker = g.spawn_unit("warrior", 0, ring[0]);
        g.units.get_mut(&attacker).unwrap().hp = 1;
        let defender = g.spawn_unit("warrior", 1, target);
        g.units.get_mut(&defender).unwrap().hp = 50;
        let mut expected_rng = g.rng.clone();
        let expected_damage = damage(10.0, 15.0, &mut expected_rng);
        g.apply(
            0,
            &Action::Attack {
                unit: attacker,
                target,
            },
        )
        .unwrap();
        assert!(!g.units.contains_key(&attacker));
        assert_eq!(
            g.units[&defender].hp,
            (50 - expected_damage + 30).min(100),
            "Scythian defenders also heal after eliminating an attacker"
        );
    }

    #[test]
    fn gathering_storm_walls_require_explicit_repair_and_land_units_to_fortify() {
        let (mut g, city_pos, _) = controlled_game(3143);
        let cid = g.found_city_for(0, city_pos, None);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .buildings
            .push("walls".to_string());
        g.cities.get_mut(&cid).unwrap().wall_hp = 50;
        assert_eq!(g.city_max_wall_hp(&g.cities[&cid]), 100);

        let medieval = Item::Building {
            building: "medieval_walls".to_string(),
        };
        g.players[0].techs.insert("castles".to_string());
        assert!(
            !g.can_produce(0, cid, &medieval),
            "damaged Ancient Walls must be repaired before upgrading"
        );

        g.turn = 3;
        g.process_city(0, cid);
        assert_eq!(
            g.cities[&cid].wall_hp, 50,
            "Outer Defenses never regenerate passively"
        );
        let repair = Item::Project {
            project: "repair_outer_defenses".to_string(),
        };
        assert!(g.can_produce(0, cid, &repair));
        g.cities.get_mut(&cid).unwrap().queue.push(repair);
        g.process_city(0, cid);
        assert!(g.cities[&cid].wall_hp > 50);

        let galley = g.spawn_unit("galley", 0, city_pos);
        assert!(!g.unit_can_fortify(&g.units[&galley]));
        assert!(g.apply(0, &Action::Fortify { unit: galley }).is_err());
    }

    #[test]
    fn eagle_warrior_conversion_uses_base_strength_probability() {
        let (mut g, target, ring) = controlled_game(3144);
        let eagle = g.spawn_unit("eagle_warrior", 0, ring[0]);
        let warrior = g.spawn_unit("warrior", 1, target);
        let scout = g.spawn_unit("scout", 1, ring[1]);
        let horse = g.spawn_unit("horseman", 1, ring[2]);
        assert_eq!(g.eagle_capture_chance(eagle, &g.units[&warrior]), 70.0);
        assert_eq!(g.eagle_capture_chance(eagle, &g.units[&scout]), 95.0);
        assert_eq!(g.eagle_capture_chance(eagle, &g.units[&horse]), 30.0);
        g.players[1].is_barbarian = true;
        assert_eq!(g.eagle_capture_chance(eagle, &g.units[&warrior]), 0.0);
    }

    #[test]
    fn combat_xp_and_fortification_use_civ6_timing_and_modifiers() {
        let (mut g, target, ring) = controlled_game(315);
        g.players[0].civ = "Nubia".to_string();
        let archer = g.spawn_unit("archer", 0, ring[0]);
        let defender = g.spawn_unit("warrior", 1, target);
        g.apply(
            0,
            &Action::Ranged {
                unit: archer,
                target,
            },
        )
        .unwrap();
        assert_eq!(g.units[&archer].xp, 5);
        assert_eq!(g.units[&defender].xp, 2);
        assert_eq!(g.modified_xp(defender, 2.49), 2);
        assert_eq!(
            g.modified_xp(defender, 2.50),
            3,
            "half an XP rounds upward, while smaller fractions do not"
        );

        g.players[0].government = Some("oligarchy".to_string());
        assert_eq!(
            g.modified_xp(archer, 3.0),
            5,
            "Nubia's 50% and Oligarchy's 20% XP modifiers stack"
        );

        let scout = g.spawn_unit("scout", 0, ring[2]);
        g.players[0].policies.insert("survey".to_string());
        let strong_enemy = g.spawn_unit("swordsman", 1, ring[3]);
        let enemy = g.units[&strong_enemy].clone();
        g.award_unit_combat_xp(scout, &enemy, false, true, true);
        assert_eq!(
            g.units[&scout].xp, 8,
            "the unit-combat XP cap applies after percentage modifiers"
        );

        g.players[1].is_barbarian = true;
        let barb = g.units[&strong_enemy].clone();
        g.units.get_mut(&scout).unwrap().level = 2;
        g.award_unit_combat_xp(scout, &barb, false, true, true);
        assert_eq!(
            g.units[&scout].xp, 9,
            "post-promotion barbarian combat grants exactly 1 XP"
        );

        let veteran = g.spawn_unit("warrior", 0, ring[4]);
        g.units.get_mut(&veteran).unwrap().xp = 420;
        g.begin_turn(0);
        assert_eq!(
            g.units[&veteran].level, 1,
            "earned promotions remain explicit choices"
        );
        for expected_level in 2..=8 {
            let promotion = g.available_promotions(veteran)[0].clone();
            g.apply(
                0,
                &Action::Promote {
                    unit: veteran,
                    promotion,
                },
            )
            .unwrap();
            assert_eq!(g.units[&veteran].level, expected_level);
            assert!(
                g.available_promotions(veteran).is_empty(),
                "a promotion consumes the unit's turn"
            );
            if expected_level < 8 {
                g.begin_turn(0);
            }
        }

        let (mut g, _, ring) = controlled_game(316);
        let unit = g.spawn_unit("warrior", 0, ring[0]);
        let destination = ring[1];
        g.apply(
            0,
            &Action::Move {
                unit,
                to: destination,
            },
        )
        .unwrap();
        g.apply(0, &Action::Fortify { unit }).unwrap();
        assert_eq!(g.units[&unit].fortify_turns, 0);
        g.begin_turn(0);
        assert_eq!(g.units[&unit].fortify_turns, 1);
        g.begin_turn(0);
        assert_eq!(g.units[&unit].fortify_turns, 2);
    }

    #[test]
    fn corps_armies_and_linked_escorts_preserve_their_rules() {
        let (mut g, center, ring) = controlled_game(3161);
        g.players[0].civics.insert("nationalism".to_string());
        let veteran = g.spawn_unit("warrior", 0, center);
        let recruit = g.spawn_unit("warrior", 0, ring[0]);
        g.units.get_mut(&veteran).unwrap().xp = 20;
        g.units
            .get_mut(&veteran)
            .unwrap()
            .promotions
            .insert("battlecry".to_string());
        g.apply(
            0,
            &Action::CombineUnits {
                unit: veteran,
                with: recruit,
            },
        )
        .unwrap();
        assert!(!g.units.contains_key(&recruit));
        assert_eq!(g.units[&veteran].formation, 1);
        assert_eq!(g.units[&veteran].xp, 20);
        assert!(g.units[&veteran].promotions.contains("battlecry"));
        assert_eq!(g.unit_unembarked_strength(&g.units[&veteran]), 30.0);

        g.begin_turn(0);
        g.players[0].civics.insert("mobilization".to_string());
        let third = g.spawn_unit("warrior", 0, ring[1]);
        g.apply(
            0,
            &Action::CombineUnits {
                unit: veteran,
                with: third,
            },
        )
        .unwrap();
        assert_eq!(g.units[&veteran].formation, 2);
        assert_eq!(g.unit_unembarked_strength(&g.units[&veteran]), 37.0);

        let (mut g, center, ring) = controlled_game(3162);
        let escort = g.spawn_unit("warrior", 0, center);
        let builder = g.spawn_unit("builder", 0, center);
        g.apply(
            0,
            &Action::LinkUnits {
                unit: escort,
                with: builder,
            },
        )
        .unwrap();
        assert_eq!(g.units[&escort].linked_to, Some(builder));
        assert_eq!(g.units[&builder].linked_to, Some(escort));
        g.apply(
            0,
            &Action::Move {
                unit: escort,
                to: ring[0],
            },
        )
        .unwrap();
        assert_eq!(g.units[&escort].pos, ring[0]);
        assert_eq!(g.units[&builder].pos, ring[0]);
        g.apply(0, &Action::UnlinkUnits { unit: escort }).unwrap();
        assert_eq!(g.units[&escort].linked_to, None);
        assert_eq!(g.units[&builder].linked_to, None);
    }

    #[test]
    fn naval_raider_promotions_apply_strength_and_victory_gold() {
        let (mut g, target, ring) = controlled_game(3165);
        g.map.tiles.get_mut(&target).unwrap().terrain = "coast".to_string();
        g.map.tiles.get_mut(&ring[0]).unwrap().terrain = "coast".to_string();
        let raider = g.spawn_unit("privateer", 0, ring[0]);
        g.units
            .get_mut(&raider)
            .unwrap()
            .promotions
            .extend(["boarding".to_string(), "homing_torpedoes".to_string()]);
        let victim = g.spawn_unit("galley", 1, target);
        g.units.get_mut(&victim).unwrap().hp = 1;
        assert_eq!(
            g.promotion_effect(&g.units[&raider], "ranged_vs_naval"),
            10.0
        );
        let gold = g.players[0].gold;
        g.apply(
            0,
            &Action::Ranged {
                unit: raider,
                target,
            },
        )
        .unwrap();
        assert!(!g.units.contains_key(&victim));
        assert_eq!(g.players[0].gold, gold + 15.0);
    }

    #[test]
    fn theological_combat_and_condemnation_change_nearby_pressure() {
        let (mut g, center, ring) = controlled_game(3163);
        g.at_war.clear();
        g.players[0].religion = Some("A".to_string());
        g.players[1].religion = Some("B".to_string());
        let cid = g.found_city_for(0, ring[2], None);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .pressure
            .insert("A".to_string(), 500.0);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .pressure
            .insert("B".to_string(), 500.0);
        let apostle = g.spawn_unit("apostle", 0, ring[0]);
        let rival = g.spawn_unit("apostle", 1, center);
        g.units.get_mut(&rival).unwrap().hp = 1;
        assert!(g.legal_actions(0).into_iter().any(|action| {
            matches!(action, Action::TheologicalAttack { unit, target }
                if unit == apostle && target == center)
        }));
        g.apply(
            0,
            &Action::TheologicalAttack {
                unit: apostle,
                target: center,
            },
        )
        .unwrap();
        assert!(!g.units.contains_key(&rival));
        assert_eq!(g.cities[&cid].pressure["A"], 750.0);
        assert_eq!(g.cities[&cid].pressure["B"], 250.0);
        assert!(!g.is_at_war(0, 1), "theological combat needs no war");

        let (mut g, center, _) = controlled_game(3164);
        g.players[1].religion = Some("B".to_string());
        let cid = g.found_city_for(0, center, None);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .pressure
            .insert("B".to_string(), 500.0);
        let soldier = g.spawn_unit("warrior", 0, center);
        let missionary = g.spawn_unit("missionary", 1, center);
        g.apply(
            0,
            &Action::CondemnHeretic {
                unit: soldier,
                target_unit: missionary,
            },
        )
        .unwrap();
        assert!(!g.units.contains_key(&missionary));
        assert_eq!(g.cities[&cid].pressure["B"], 375.0);
    }

    #[test]
    fn encampments_initialize_strike_pillage_and_repair_independently() {
        let (mut g, city_pos, _ring) = controlled_game(317);
        let cid = g.found_city_for(0, city_pos, None);
        let encampment_pos = g
            .wdisk(city_pos, 2)
            .into_iter()
            .find(|position| g.wdist(city_pos, *position) == 2)
            .unwrap();
        g.map.tiles.get_mut(&encampment_pos).unwrap().owner_city = Some(cid);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .owned_tiles
            .push(encampment_pos);
        let district = Item::District {
            district: "encampment".to_string(),
            pos: encampment_pos,
        };
        g.players[0].techs.insert("bronze_working".to_string());
        assert!(g.complete_item(0, cid, &district));
        assert_eq!(g.cities[&cid].encampment_hp, 100);
        assert_eq!(g.cities[&cid].encampment_wall_hp, 0);

        assert!(g.complete_item(
            0,
            cid,
            &Item::Building {
                building: "walls".to_string(),
            },
        ));
        assert_eq!(g.cities[&cid].wall_hp, 100);
        assert_eq!(g.cities[&cid].encampment_wall_hp, 100);

        let target = g
            .wdisk(encampment_pos, 2)
            .into_iter()
            .find(|pos| {
                *pos != city_pos
                    && *pos != encampment_pos
                    && g.map.tiles.contains_key(pos)
                    && g.wdist(*pos, city_pos) > 1
            })
            .unwrap();
        g.spawn_unit("warrior", 1, target);
        assert!(g.legal_actions(0).into_iter().any(|action| {
            matches!(action, Action::EncampmentStrike { city, target: to }
                if city == cid && to == target)
        }));
        g.apply(0, &Action::EncampmentStrike { city: cid, target })
            .unwrap();
        assert!(g.cities[&cid].encampment_struck);
        assert!(g
            .apply(0, &Action::EncampmentStrike { city: cid, target },)
            .is_err());
        g.begin_turn(0);
        assert!(!g.cities[&cid].encampment_struck);

        let attacker_pos = g
            .nbrs(encampment_pos)
            .into_iter()
            .find(|pos| *pos != city_pos && *pos != target)
            .unwrap();
        let attacker = g.spawn_unit("warrior", 1, attacker_pos);
        // A Bombard-depleted Encampment remains targetable until melee enters.
        g.cities.get_mut(&cid).unwrap().encampment_hp = 0;
        g.cities.get_mut(&cid).unwrap().encampment_wall_hp = 0;
        g.current = 1;
        assert!(g.legal_actions(1).into_iter().any(|action| {
            matches!(action, Action::Attack { unit, target }
                if unit == attacker && target == encampment_pos)
        }));
        g.apply(
            1,
            &Action::Attack {
                unit: attacker,
                target: encampment_pos,
            },
        )
        .unwrap();
        assert!(g.cities[&cid].encampment_pillaged);
        let repair = Item::Project {
            project: "repair_encampment".to_string(),
        };
        assert!(!g.can_produce(0, cid, &repair));
        g.turn = g.cities[&cid].encampment_last_attacked + 3;
        assert!(g.can_produce(0, cid, &repair));
        assert!(g.complete_item(0, cid, &repair));
        assert_eq!(g.cities[&cid].encampment_hp, 100);
        assert_eq!(g.cities[&cid].encampment_wall_hp, 100);
        assert!(!g.cities[&cid].encampment_pillaged);
        assert!(!g.can_produce(0, cid, &repair));
    }

    #[test]
    fn naval_roster_uses_gathering_storm_technology_and_civic_unlocks() {
        let (mut g, city_pos, ring) = controlled_game(319);
        let cid = g.found_city_for(0, city_pos, None);
        g.map.tiles.get_mut(&ring[0]).unwrap().terrain = "coast".to_string();

        let unlocks = [
            ("galley", Some("sailing"), None),
            ("quadrireme", Some("shipbuilding"), None),
            ("caravel", Some("cartography"), None),
            ("frigate", Some("square_rigging"), None),
            ("privateer", None, Some("mercantilism")),
            ("ironclad", Some("steam_power"), None),
            ("battleship", Some("refining"), None),
            ("submarine", Some("electricity"), None),
            ("destroyer", Some("combined_arms"), None),
            ("aircraft_carrier", Some("combined_arms"), None),
            ("missile_cruiser", Some("lasers"), None),
            ("nuclear_submarine", Some("telecommunications"), None),
        ];
        for (kind, tech, civic) in unlocks {
            let spec = &g.rules.units[kind];
            assert_eq!(spec.domain.as_deref(), Some("sea"), "{kind} domain");
            assert_eq!(spec.tech.as_deref(), tech, "{kind} technology");
            assert_eq!(spec.civic.as_deref(), civic, "{kind} civic");
            let item = Item::Unit {
                unit: kind.to_string(),
            };
            assert!(!g.can_produce(0, cid, &item), "{kind} starts locked");
        }
        for (kind, tech, civic) in unlocks {
            let item = Item::Unit {
                unit: kind.to_string(),
            };
            if let Some(technology) = tech {
                g.players[0].techs.insert(technology.to_string());
            }
            if let Some(required_civic) = civic {
                g.players[0].civics.insert(required_civic.to_string());
            }
            assert!(g.can_produce(0, cid, &item), "{kind} unlocks on schedule");
        }
    }

    #[test]
    fn embarkation_and_ocean_access_unlock_in_distinct_stages() {
        let (mut g, land, ring) = controlled_game(320);
        let coast = ring[0];
        let ocean = g
            .nbrs(coast)
            .into_iter()
            .find(|pos| *pos != land)
            .expect("coast has another adjacent tile");
        g.map.tiles.get_mut(&coast).unwrap().terrain = "coast".to_string();
        g.map.tiles.get_mut(&ocean).unwrap().terrain = "ocean".to_string();

        let builder = g.spawn_unit("builder", 0, land);
        assert!(!g.can_move(builder, coast));
        g.players[0].techs.insert("sailing".to_string());
        assert!(g.can_move(builder, coast), "Sailing embarks Builders");
        g.relocate(builder, coast);
        assert!(!g.can_move(builder, ocean));
        g.players[0].techs.insert("cartography".to_string());
        assert!(g.can_move(builder, ocean), "Cartography opens Ocean");
        g.remove_unit(builder);

        g.players[0].techs.clear();
        let trader = g.spawn_unit("trader", 0, land);
        g.players[0].techs.insert("sailing".to_string());
        assert!(!g.can_move(trader, coast));
        g.players[0]
            .techs
            .insert("celestial_navigation".to_string());
        assert!(
            g.can_move(trader, coast),
            "Celestial Navigation embarks Traders"
        );
        g.remove_unit(trader);

        g.players[0].techs.clear();
        let warrior = g.spawn_unit("warrior", 0, land);
        g.players[0].techs.insert("sailing".to_string());
        assert!(!g.can_move(warrior, coast));
        g.players[0].techs.insert("shipbuilding".to_string());
        assert!(
            g.can_move(warrior, coast),
            "Shipbuilding embarks other land units"
        );
        g.remove_unit(warrior);

        g.players[0].techs.clear();
        let galley = g.spawn_unit("galley", 0, coast);
        assert!(!g.can_move(galley, ocean), "early ships are Coast-bound");
        assert!(g.route_step(galley, ocean, 0).is_none());
        assert_eq!(g.unit_max_moves(galley), 3.0);
        g.players[0].techs.insert("mathematics".to_string());
        assert_eq!(
            g.unit_max_moves(galley),
            4.0,
            "Mathematics adds sea Movement"
        );
        g.players[0].techs.insert("cartography".to_string());
        assert!(g.can_move(galley, ocean));
        assert_eq!(g.route_step(galley, ocean, 0), Some(ocean));
    }

    #[test]
    fn naval_roles_fight_at_sea_and_melee_ships_capture_only_coastal_cities() {
        let (mut g, city_pos, ring) = controlled_game(321);
        let coast = ring[0];
        g.map.tiles.get_mut(&coast).unwrap().terrain = "coast".to_string();
        let city = g.found_city_for(1, city_pos, None);
        g.cities.get_mut(&city).unwrap().hp = 1;
        let galley = g.spawn_unit("galley", 0, coast);
        assert!(g.legal_actions(0).into_iter().any(|action| {
            matches!(action, Action::Attack { unit, target }
                if unit == galley && target == city_pos)
        }));
        g.apply(
            0,
            &Action::Attack {
                unit: galley,
                target: city_pos,
            },
        )
        .unwrap();
        assert_eq!(g.cities[&city].owner, 0);
        assert_eq!(g.units[&galley].pos, city_pos);

        let (mut g, land, ring) = controlled_game(322);
        let coast = ring[0];
        let inland = g.nbrs(coast).into_iter().find(|pos| *pos != land).unwrap();
        g.map.tiles.get_mut(&coast).unwrap().terrain = "coast".to_string();
        let galley = g.spawn_unit("galley", 0, coast);
        g.spawn_unit("warrior", 1, inland);
        assert!(g
            .apply(
                0,
                &Action::Attack {
                    unit: galley,
                    target: inland,
                },
            )
            .is_err());

        let enemy_coast = g
            .nbrs(coast)
            .into_iter()
            .find(|pos| *pos != inland && *pos != land)
            .unwrap();
        g.map.tiles.get_mut(&enemy_coast).unwrap().terrain = "coast".to_string();
        let enemy_ship = g.spawn_unit("galley", 1, enemy_coast);
        let quadrireme = g.spawn_unit("quadrireme", 0, coast);
        g.apply(
            0,
            &Action::Ranged {
                unit: quadrireme,
                target: enemy_coast,
            },
        )
        .unwrap();
        assert!(g.units.get(&enemy_ship).is_none_or(|unit| unit.hp < 100));
    }

    #[test]
    fn naval_ranged_units_do_not_take_the_land_ranged_anti_ship_penalty() {
        let (base, center, ring) = controlled_game(324);
        let attacker_pos = ring[0];

        let mut naval = base.clone();
        naval.map.tiles.get_mut(&center).unwrap().terrain = "coast".to_string();
        naval.map.tiles.get_mut(&attacker_pos).unwrap().terrain = "coast".to_string();
        let naval_attacker = naval.spawn_unit("quadrireme", 0, attacker_pos);
        let naval_target = naval.spawn_unit("galley", 1, center);
        naval
            .apply(
                0,
                &Action::Ranged {
                    unit: naval_attacker,
                    target: center,
                },
            )
            .unwrap();
        let naval_damage = 100 - naval.units[&naval_target].hp;

        let mut land = base;
        land.map.tiles.get_mut(&center).unwrap().terrain = "coast".to_string();
        land.rules.units.get_mut("archer").unwrap().ranged_strength = 25.0;
        let land_attacker = land.spawn_unit("archer", 0, attacker_pos);
        let land_target = land.spawn_unit("galley", 1, center);
        land.apply(
            0,
            &Action::Ranged {
                unit: land_attacker,
                target: center,
            },
        )
        .unwrap();
        let land_damage = 100 - land.units[&land_target].hp;

        assert!(
            naval_damage > land_damage,
            "naval ranged {naval_damage} should outperform equal-strength land ranged {land_damage} against a ship"
        );
    }

    #[test]
    fn naval_raiders_require_adjacent_or_specialized_detection() {
        let (mut g, center, ring) = controlled_game(323);
        for pos in std::iter::once(center).chain(ring.iter().copied()) {
            g.map.tiles.get_mut(&pos).unwrap().terrain = "coast".to_string();
        }
        let submarine = g.spawn_unit("submarine", 0, center);
        let distant = g
            .wdisk(center, 2)
            .into_iter()
            .find(|pos| g.wdist(*pos, center) == 2)
            .unwrap();
        g.map.tiles.get_mut(&distant).unwrap().terrain = "plains".to_string();
        let observer = g.spawn_unit("warrior", 1, distant);
        assert!(!g.unit_visible_to(submarine, 1));
        g.relocate(observer, ring[0]);
        assert!(g.unit_visible_to(submarine, 1));
        g.relocate(observer, distant);
        g.map.tiles.get_mut(&distant).unwrap().terrain = "coast".to_string();
        g.remove_unit(observer);
        g.spawn_unit("destroyer", 1, distant);
        assert!(g.unit_visible_to(submarine, 1));
    }

    #[test]
    fn pyramids_use_a_legal_tile_remain_world_unique_and_improve_builders() {
        let (mut g, center, ring) = controlled_game(324);
        let cid = g.found_city_for(0, center, None);
        let site = ring[0];
        {
            let tile = g.map.tiles.get_mut(&site).unwrap();
            tile.terrain = "desert".to_string();
            tile.hills = false;
            tile.owner_city = Some(cid);
        }
        g.players[0].techs.insert("masonry".to_string());
        let item = Item::Wonder {
            wonder: "pyramids".to_string(),
            pos: site,
        };
        assert!(g.can_produce(0, cid, &item));

        let builders_before = g
            .units
            .values()
            .filter(|unit| unit.owner == 0 && unit.kind == "builder")
            .count();
        assert!(g.complete_item(0, cid, &item));
        assert_eq!(g.map.tiles[&site].wonder.as_deref(), Some("pyramids"));
        assert_eq!(g.cities[&cid].wonders.get("pyramids"), Some(&site));
        assert!(g.wonder_built("pyramids"));
        assert!(!g.can_produce(0, cid, &item));

        let builders: Vec<&Unit> = g
            .units
            .values()
            .filter(|unit| unit.owner == 0 && unit.kind == "builder")
            .collect();
        assert_eq!(builders.len(), builders_before + 1);
        assert!(builders
            .iter()
            .any(|builder| builder.charges >= g.rules.units["builder"].charges + 1));
    }

    #[test]
    fn religious_spreads_combat_and_guru_healing_follow_gathering_storm() {
        let (mut g, city_pos, ring) = controlled_game(318);
        let cid = g.found_city_for(1, city_pos, None);
        g.cities
            .get_mut(&cid)
            .unwrap()
            .pressure
            .insert("Rival".to_string(), 300.0);

        let missionary = g.spawn_unit("missionary", 0, ring[0]);
        g.units.get_mut(&missionary).unwrap().religion = Some("Our".to_string());
        g.apply(0, &Action::Spread { unit: missionary }).unwrap();
        assert_eq!(g.cities[&cid].pressure["Rival"], 270.0);
        assert_eq!(g.cities[&cid].pressure["Our"], 200.0);
        assert!(g.apply(0, &Action::Spread { unit: missionary }).is_err());

        let apostle = g.spawn_unit("apostle", 0, ring[1]);
        g.units.get_mut(&apostle).unwrap().religion = Some("Our".to_string());
        g.apply(0, &Action::Spread { unit: apostle }).unwrap();
        assert_eq!(g.cities[&cid].pressure["Rival"], 202.5);
        assert_eq!(g.cities[&cid].pressure["Our"], 420.0);

        let victim = g.spawn_unit("missionary", 1, city_pos);
        {
            let victim = g.units.get_mut(&victim).unwrap();
            victim.religion = Some("Rival".to_string());
            victim.hp = 1;
        }
        {
            let apostle = g.units.get_mut(&apostle).unwrap();
            apostle.moves_left = 4.0;
            apostle.acted = false;
        }
        g.apply(
            0,
            &Action::TheologicalAttack {
                unit: apostle,
                target: city_pos,
            },
        )
        .unwrap();
        assert!(!g.units.contains_key(&victim));
        assert_eq!(g.cities[&cid].pressure["Rival"], 0.0);
        assert_eq!(g.cities[&cid].pressure["Our"], 670.0);

        let guru_pos = ring[3];
        let guru = g.spawn_unit("guru", 0, guru_pos);
        let faithful = g.spawn_unit("missionary", 0, guru_pos);
        let other_faith = g.spawn_unit("missionary", 0, guru_pos);
        for uid in [guru, faithful] {
            let unit = g.units.get_mut(&uid).unwrap();
            unit.religion = Some("Our".to_string());
            unit.hp = 50;
        }
        {
            let unit = g.units.get_mut(&other_faith).unwrap();
            unit.religion = Some("Other".to_string());
            unit.hp = 50;
        }
        g.apply(0, &Action::HealReligious { unit: guru }).unwrap();
        assert_eq!(g.units[&guru].hp, 90, "a Guru heals itself");
        assert_eq!(g.units[&faithful].hp, 90);
        assert_eq!(g.units[&other_faith].hp, 50);
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
    fn world_era_uses_all_nine_tree_eras_including_future() {
        let mut g = game_with_capitals(2, 400, 500);
        for player in &mut g.players {
            player.techs.clear();
            player.civics.clear();
        }
        assert_eq!(g.era_from_progress(), 0);

        for era in 0..crate::rules::ERA_NAMES.len() {
            let tech = g
                .rules
                .techs
                .iter()
                .find(|(_, spec)| spec.era == era)
                .map(|(name, _)| name.clone())
                .unwrap();
            g.players[0].techs.clear();
            g.players[0].techs.insert(tech);
            assert_eq!(g.era_from_progress(), era, "technology era {era}");

            let civic = g
                .rules
                .civics
                .iter()
                .find(|(_, spec)| spec.era == era)
                .map(|(name, _)| name.clone())
                .unwrap();
            g.players[0].techs.clear();
            g.players[0].civics.clear();
            g.players[0].civics.insert(civic);
            assert_eq!(g.era_from_progress(), era, "civic era {era}");
        }

        g.world_era = 3;
        g.players[0].civics.clear();
        g.players[0].techs.insert("smart_materials".to_string());
        g.process_eras();
        assert_eq!(
            g.world_era, 8,
            "late research must not stay capped at Renaissance"
        );
    }

    #[test]
    fn science_requires_the_space_race_and_exoplanet_arrival() {
        let protocol_item: Item =
            serde_json::from_str(r#"{"project":"launch_earth_satellite"}"#).unwrap();
        assert_eq!(
            protocol_item,
            Item::Project {
                project: "launch_earth_satellite".to_string()
            }
        );

        let mut g = game_with_capitals(2, 401, 300);
        let all_techs: Vec<String> = g.rules.techs.keys().cloned().collect();
        for tech in all_techs
            .iter()
            .filter(|t| t.as_str() != "offworld_mission")
        {
            g.players[0].techs.insert(tech.clone());
        }
        g.players[0].research = Some("offworld_mission".to_string());
        g.players[0].research_progress = g.rules.techs["offworld_mission"].cost;
        g.begin_turn(0);
        assert_eq!(g.players[0].techs.len(), g.rules.techs.len());
        assert_eq!(
            g.winner, None,
            "finishing the technology tree is not a science victory"
        );

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

        let earth = Item::Project {
            project: "launch_earth_satellite".to_string(),
        };
        let moon = Item::Project {
            project: "launch_moon_landing".to_string(),
        };
        let mars = Item::Project {
            project: "launch_mars_colony".to_string(),
        };
        let exoplanet = Item::Project {
            project: "exoplanet_expedition".to_string(),
        };
        assert!(g.can_produce(0, cid, &earth));
        assert!(!g.can_produce(0, cid, &moon));
        g.players[0].explored.clear();
        assert!(g.complete_item(0, cid, &earth));
        assert_eq!(
            g.players[0].explored.len(),
            g.map.tiles.len(),
            "Earth Satellite reveals the whole map"
        );
        assert!(g.can_produce(0, cid, &moon));
        let science = g
            .player_city_ids(0)
            .into_iter()
            .map(|city_id| g.city_yields(city_id).science)
            .sum::<f64>();
        let culture_before = g.players[0].culture_lifetime;
        assert!(g.complete_item(0, cid, &moon));
        assert!(
            (g.players[0].culture_lifetime - culture_before - 10.0 * science).abs() < 1e-9,
            "Moon Landing grants Culture equal to ten turns of Science"
        );
        assert!(!g.can_produce(0, cid, &exoplanet));
        assert!(g.complete_item(0, cid, &mars));
        assert!(g.can_produce(0, cid, &exoplanet));
        assert!(g.complete_item(0, cid, &exoplanet));
        assert_eq!(g.winner, None, "launching is not the same as arriving");

        let laser = Item::Project {
            project: "lagrange_laser_station".to_string(),
        };
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
        g.cities
            .get_mut(&own)
            .unwrap()
            .pressure
            .insert(religion.clone(), 100.0);
        g.cities
            .get_mut(&rival[0])
            .unwrap()
            .pressure
            .insert(religion.clone(), 100.0);
        g.check_religious_victory();
        assert_eq!(g.winner, None, "one of two rival cities is not a majority");
        g.cities
            .get_mut(&extra)
            .unwrap()
            .pressure
            .insert(religion, 100.0);
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
    fn great_work_tourism_uses_tree_and_slotted_policy_modifiers() {
        let mut g = game_with_capitals(2, 412, 300);
        let city = g.player_city_ids(0)[0];
        g.cities.get_mut(&city).unwrap().buildings = vec![
            "amphitheater".to_string(),
            "art_museum".to_string(),
            "broadcast_center".to_string(),
        ];
        g.players[0].gp_claimed.insert("artist".to_string(), 2);

        let base = g.tourism_per_turn(0);
        g.players[0].techs.insert("printing".to_string());
        let printing = g.tourism_per_turn(0);
        assert!((printing - base - 4.0).abs() < 1e-9);

        g.players[0].policies.insert("heritage_tourism".to_string());
        let heritage = g.tourism_per_turn(0);
        assert!((heritage - printing - 18.0).abs() < 1e-9);

        g.players[0]
            .policies
            .insert("satellite_broadcasts".to_string());
        let broadcasts = g.tourism_per_turn(0);
        assert!((broadcasts - heritage - 8.0).abs() < 1e-9);
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

    #[test]
    fn specialty_district_capacity_unlocks_at_population_one_four_and_seven() {
        let mut g = game_with_capitals(2, 407, 300);
        let cid = g.player_city_ids(0)[0];
        let center = g.cities[&cid].pos;
        let owned = g.cities[&cid].owned_tiles.clone();
        for pos in owned.iter().copied().filter(|pos| *pos != center) {
            let tile = g.map.tiles.get_mut(&pos).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.district = None;
            tile.hills = false;
        }
        g.players[0].techs.extend([
            "writing".to_string(),
            "astrology".to_string(),
            "rocketry".to_string(),
        ]);
        g.players[0].civics.insert("drama_poetry".to_string());

        let campus_site = g.district_sites(cid, "campus")[0];
        assert!(g.complete_item(
            0,
            cid,
            &Item::District {
                district: "campus".to_string(),
                pos: campus_site,
            }
        ));
        assert!(
            g.district_sites(cid, "holy_site").is_empty(),
            "population 1 supports only one specialty district"
        );
        assert!(
            !g.district_sites(cid, "spaceport").is_empty(),
            "Spaceports ignore the specialty-district population cap"
        );

        g.cities.get_mut(&cid).unwrap().pop = 4;
        let holy_site = g.district_sites(cid, "holy_site")[0];
        assert!(g.complete_item(
            0,
            cid,
            &Item::District {
                district: "holy_site".to_string(),
                pos: holy_site,
            }
        ));
        assert!(
            g.district_sites(cid, "theater_square").is_empty(),
            "population 4 supports exactly two specialty districts"
        );

        g.cities.get_mut(&cid).unwrap().pop = 7;
        assert!(!g.district_sites(cid, "theater_square").is_empty());
    }

    #[test]
    fn gathering_storm_amenities_require_connected_luxuries_and_ration_them() {
        let mut g = game_with_capitals(2, 408, 300);
        let capital = g.player_city_ids(0)[0];
        let occupied: BTreeSet<Pos> = g.cities.values().map(|city| city.pos).collect();
        let sites: Vec<Pos> = g
            .map
            .tiles
            .keys()
            .copied()
            .filter(|pos| !occupied.contains(pos))
            .take(5)
            .collect();
        for (n, pos) in sites.into_iter().enumerate() {
            g.found_city_for(0, pos, Some(format!("Amenity {n}")));
        }
        for cid in g.player_city_ids(0) {
            let city = g.cities.get_mut(&cid).unwrap();
            city.pop = 1;
            city.buildings.clear();
            city.districts.clear();
        }
        g.players[0].government = None;
        g.players[0].policies.clear();
        g.players[0].governors.clear();

        let luxury_pos = g.cities[&capital]
            .owned_tiles
            .iter()
            .copied()
            .find(|pos| *pos != g.cities[&capital].pos)
            .unwrap();
        let tile = g.map.tiles.get_mut(&luxury_pos).unwrap();
        tile.resource = Some("silk".to_string());
        tile.improvement = None;
        assert_eq!(
            g.empire_luxuries(0),
            0,
            "an unimproved luxury supplies no Amenities"
        );

        g.map.tiles.get_mut(&luxury_pos).unwrap().improvement = Some("plantation".to_string());
        assert_eq!(g.empire_luxuries(0), 1);
        let mut surpluses: Vec<i64> = g
            .player_city_ids(0)
            .into_iter()
            .map(|cid| g.city_amenity_surplus(&g.cities[&cid]))
            .collect();
        surpluses.sort();
        assert_eq!(
            surpluses,
            vec![-1, 0, 0, 0, 0, 0],
            "one luxury serves the four neediest cities; the Palace serves the capital"
        );

        let duplicate_pos = g.cities[&capital]
            .owned_tiles
            .iter()
            .copied()
            .find(|pos| *pos != g.cities[&capital].pos && *pos != luxury_pos)
            .unwrap();
        let tile = g.map.tiles.get_mut(&duplicate_pos).unwrap();
        tile.resource = Some("silk".to_string());
        tile.improvement = Some("plantation".to_string());
        assert_eq!(
            g.empire_luxuries(0),
            1,
            "duplicate copies of a luxury do not supply more cities"
        );

        g.players[0].civ = "Aztec".to_string();
        let mut aztec_surpluses: Vec<i64> = g
            .player_city_ids(0)
            .into_iter()
            .map(|cid| g.city_amenity_surplus(&g.cities[&cid]))
            .collect();
        aztec_surpluses.sort();
        assert_eq!(
            aztec_surpluses,
            vec![0, 0, 0, 0, 0, 1],
            "Gifts for the Tlatoani extends each luxury from four to six cities"
        );
    }

    #[test]
    fn gathering_storm_happiness_bands_apply_exact_growth_and_yield_modifiers() {
        let cases = [
            (5, 1.20, 1.20),
            (3, 1.10, 1.10),
            (0, 1.00, 1.00),
            (-2, 0.90, 0.85),
            (-4, 0.80, 0.70),
            (-6, 0.70, 0.00),
            (-7, 0.60, 0.00),
        ];
        for (surplus, yields, growth) in cases {
            assert_eq!(Game::amenity_yield_mult_for(surplus), yields);
            assert_eq!(Game::amenity_growth_mult(surplus), growth);
        }
    }

    #[test]
    fn housing_uses_palace_aqueduct_lighthouse_and_exact_growth_bands() {
        let mut g = game_with_capitals(2, 409, 300);
        let cid = g.player_city_ids(0)[0];
        let center = g.cities[&cid].pos;
        g.map.clear_rivers();
        for pos in std::iter::once(center).chain(g.nbrs(center)) {
            let tile = g.map.tiles.get_mut(&pos).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
        }
        assert_eq!(
            g.city_housing(&g.cities[&cid]),
            3.0,
            "a dry capital has 2 water Housing plus 1 from its Palace"
        );

        let coast = g.nbrs(center)[0];
        g.map.tiles.get_mut(&coast).unwrap().terrain = "coast".to_string();
        assert_eq!(
            g.city_housing(&g.cities[&cid]),
            4.0,
            "a coastal capital starts with 3 + Palace"
        );
        g.cities
            .get_mut(&cid)
            .unwrap()
            .buildings
            .push("lighthouse".to_string());
        assert_eq!(
            g.city_housing(&g.cities[&cid]),
            6.0,
            "a coastal Lighthouse supplies 2 total Housing, plus the Palace"
        );

        g.cities
            .get_mut(&cid)
            .unwrap()
            .buildings
            .retain(|b| b != "lighthouse");
        g.map.tiles.get_mut(&coast).unwrap().terrain = "plains".to_string();
        g.cities
            .get_mut(&cid)
            .unwrap()
            .districts
            .insert("aqueduct".to_string(), coast);
        assert_eq!(
            g.city_housing(&g.cities[&cid]),
            7.0,
            "an Aqueduct raises dry water Housing to 6, plus the Palace"
        );
        assert!(g.map.set_river_edge(center, g.nbrs(center)[1], true));
        assert_eq!(
            g.city_housing(&g.cities[&cid]),
            8.0,
            "a fresh-water Aqueduct adds 2 to 5, plus the Palace"
        );

        let cases = [
            (2.0, 1.0),
            (1.5, 0.5),
            (1.0, 0.5),
            (0.0, 0.25),
            (-4.5, 0.25),
            (-5.0, 0.0),
        ];
        for (headroom, growth) in cases {
            assert_eq!(Game::housing_growth_mult(headroom), growth);
        }
    }

    #[test]
    fn palace_supplies_the_stock_capital_yield_package_and_moves_when_captured() {
        let mut g = game_with_capitals(2, 410, 300);
        let original = g.player_city_ids(0)[0];
        let city = &g.cities[&original];
        assert!(g.city_has_palace(city));
        let yields = g.city_yields(original);
        assert!(yields.production >= 3.0);
        assert!(yields.gold >= 5.0);
        assert!(yields.science >= 2.5);
        assert!(yields.culture >= 1.3);

        let second_pos = g
            .map
            .tiles
            .keys()
            .copied()
            .find(|pos| {
                g.rules.is_passable(&g.map.tiles[pos])
                    && !g.rules.is_water(&g.map.tiles[pos])
                    && g.cities.values().all(|city| g.wdist(city.pos, *pos) >= 4)
            })
            .unwrap();
        let second = g.found_city_for(0, second_pos, Some("Fallback".to_string()));
        g.capture_city(original, 1);
        assert!(!g.city_has_palace(&g.cities[&original]));
        assert!(g.city_has_palace(&g.cities[&second]));
    }

    #[test]
    fn production_switching_preserves_item_progress_without_banking_idle_turns() {
        let mut g = game_with_capitals(2, 411, 300);
        let pid = 1;
        let cid = g.player_city_ids(pid)[0];
        let monument = Item::Building {
            building: "monument".to_string(),
        };
        let builder = Item::Unit {
            unit: "builder".to_string(),
        };

        g.cities.get_mut(&cid).unwrap().queue.clear();
        g.cities.get_mut(&cid).unwrap().production = 0.0;
        g.process_city(pid, cid);
        assert_eq!(
            g.cities[&cid].production, 0.0,
            "idle turns do not bank Production"
        );

        g.do_produce(pid, cid, &monument).unwrap();
        g.cities.get_mut(&cid).unwrap().production = 20.0;
        g.do_produce(pid, cid, &builder).unwrap();
        assert_eq!(g.cities[&cid].production, 0.0);
        g.cities.get_mut(&cid).unwrap().production = 10.0;
        g.do_produce(pid, cid, &monument).unwrap();
        assert_eq!(g.cities[&cid].production, 20.0);
        g.do_produce(pid, cid, &builder).unwrap();
        assert_eq!(g.cities[&cid].production, 10.0);
    }

    #[test]
    fn gathering_storm_overflow_removes_item_specific_production_bonus() {
        let mut g = game_with_capitals(2, 412, 300);
        let cid = g.player_city_ids(0)[0];
        g.players[0].techs.insert("masonry".to_string());
        g.players[0].policies.insert("limes".to_string());
        let walls = Item::Building {
            building: "walls".to_string(),
        };
        g.do_produce(0, cid, &walls).unwrap();
        let base = g.city_yields(cid).production;
        let cost = g.item_cost_for(0, &walls);
        g.cities.get_mut(&cid).unwrap().production = cost - base;
        g.process_city(0, cid);
        assert!(g.cities[&cid].buildings.contains(&"walls".to_string()));
        assert!(
            (g.cities[&cid].production - base / 2.0).abs() < 1e-9,
            "only the unused base Production survives a +100% Limes completion"
        );
    }

    #[test]
    fn settlers_and_builders_scale_and_settlers_consume_population() {
        let mut g = game_with_capitals(2, 413, 300);
        let cid = g.player_city_ids(0)[0];
        g.cities.get_mut(&cid).unwrap().pop = 2;
        let settler = Item::Unit {
            unit: "settler".to_string(),
        };
        let builder = Item::Unit {
            unit: "builder".to_string(),
        };
        assert_eq!(g.item_cost_for(0, &settler), 80.0);
        assert!(g.complete_item(0, cid, &settler));
        assert_eq!(g.cities[&cid].pop, 1);
        assert_eq!(g.item_cost_for(0, &settler), 110.0);
        assert!(!g.can_produce(0, cid, &settler));

        g.cities.get_mut(&cid).unwrap().pop = 2;
        g.players[0].gold = 1_000.0;
        g.do_buy(0, cid, "settler", "gold").unwrap();
        assert_eq!(g.cities[&cid].pop, 1);
        assert_eq!(g.players[0].gold, 560.0);
        assert_eq!(g.item_cost_for(0, &settler), 140.0);

        assert_eq!(g.item_cost_for(0, &builder), 50.0);
        assert!(g.complete_item(0, cid, &builder));
        assert_eq!(g.item_cost_for(0, &builder), 54.0);
    }

    #[test]
    fn religions_convert_all_owned_holy_sites_and_temples_require_shrines() {
        let mut g = game_with_capitals(2, 414, 300);
        let first = g.player_city_ids(0)[0];
        let second_pos = g
            .map
            .tiles
            .keys()
            .copied()
            .find(|pos| {
                g.rules.is_passable(&g.map.tiles[pos])
                    && !g.rules.is_water(&g.map.tiles[pos])
                    && g.cities.values().all(|city| g.wdist(city.pos, *pos) >= 4)
            })
            .unwrap();
        let second = g.found_city_for(0, second_pos, Some("Second Holy Site".to_string()));
        let first_site = g.cities[&first]
            .owned_tiles
            .iter()
            .copied()
            .find(|pos| *pos != g.cities[&first].pos)
            .unwrap();
        let second_site = g.cities[&second]
            .owned_tiles
            .iter()
            .copied()
            .find(|pos| *pos != g.cities[&second].pos)
            .unwrap();
        g.cities
            .get_mut(&first)
            .unwrap()
            .districts
            .insert("holy_site".to_string(), first_site);
        g.cities
            .get_mut(&second)
            .unwrap()
            .districts
            .insert("holy_site".to_string(), second_site);
        g.players[0].civics.insert("theology".to_string());
        let temple = Item::Building {
            building: "temple".to_string(),
        };
        assert!(!g.can_produce(0, first, &temple));
        g.cities
            .get_mut(&first)
            .unwrap()
            .buildings
            .push("shrine".to_string());
        assert!(g.can_produce(0, first, &temple));

        g.players[0].prophet_pending = true;
        g.do_found_religion(0, "choral_music", "tithe").unwrap();
        let religion = g.players[0].religion.clone().unwrap();
        assert_eq!(g.cities[&first].pressure[&religion], 1_000.0);
        assert_eq!(g.cities[&second].pressure[&religion], 1_000.0);
    }
}

#[cfg(test)]
mod district_mechanics {
    use super::*;

    fn controlled_game() -> (Game, u32, Pos, Vec<Pos>) {
        let mut game = Game::new_full(1, 20, 14, 5150, 300, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| game.units[uid].kind == "settler")
            .unwrap();
        let city = game.found_city_for(0, game.units[&settler].pos, None);
        let city_position = game.cities[&city].pos;
        let district_position = game
            .map
            .tiles
            .keys()
            .copied()
            .find(|position| {
                game.wdisk(*position, 1).len() == 7 && game.wdist(*position, city_position) > 4
            })
            .unwrap();
        let mut ring = game.nbrs(district_position);
        ring.sort();
        for position in std::iter::once(district_position).chain(ring.iter().copied()) {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.wonder = None;
            tile.owner_city = None;
            tile.river_edges = [false; 6];
        }
        game.map
            .tiles
            .get_mut(&district_position)
            .unwrap()
            .owner_city = Some(city);
        (game, city, district_position, ring)
    }

    fn adjacency_value(game: &Game, district: &str, source: &str) -> f64 {
        game.rules.districts[district].adjacency[source].total()
    }

    #[test]
    fn gathering_storm_catalog_contains_every_generic_and_unique_district() {
        let game = Game::new_full(1, 20, 14, 5151, 30, 0, false);
        let expected: BTreeSet<&str> = [
            "acropolis",
            "aerodrome",
            "aqueduct",
            "bath",
            "campus",
            "canal",
            "city_center",
            "commercial_hub",
            "copacabana",
            "cothon",
            "dam",
            "diplomatic_quarter",
            "encampment",
            "entertainment_complex",
            "government_plaza",
            "hansa",
            "harbor",
            "hippodrome",
            "holy_site",
            "ikanda",
            "industrial_zone",
            "lavra",
            "mbanza",
            "neighborhood",
            "observatory",
            "oppidum",
            "preserve",
            "royal_navy_dockyard",
            "seowon",
            "spaceport",
            "street_carnival",
            "suguba",
            "thanh",
            "theater_square",
            "water_park",
        ]
        .into_iter()
        .collect();
        let actual: BTreeSet<&str> = game.rules.districts.keys().map(String::as_str).collect();
        assert_eq!(actual, expected);

        for (name, spec) in &game.rules.districts {
            if let Some(base) = spec.replaces.as_deref() {
                assert!(
                    game.rules.districts.contains_key(base),
                    "{name} replaces {base}"
                );
                assert!(
                    spec.unique_to.is_some(),
                    "{name} replacement has no civilization"
                );
            }
        }
        assert_eq!(
            game.rules.districts["ikanda"].placement,
            "not_adjacent_city"
        );
        assert_eq!(game.rules.districts["thanh"].placement, "not_adjacent_city");
        assert_eq!(game.rules.districts["acropolis"].placement, "hills");
        assert!(!game.rules.districts["thanh"].specialty);
        assert!(!game.rules.districts["spaceport"].specialty);
        assert!(!game.rules.districts["city_center"].buildable);
        assert_eq!(game.rules.districts["water_park"].cost, 54.0);
        assert_eq!(game.rules.districts["copacabana"].cost, 27.0);
        assert_eq!(game.rules.districts["dam"].max_per_city, None);
    }

    #[test]
    fn every_stock_adjacency_source_has_its_exact_value() {
        let game = Game::new_full(1, 20, 14, 5152, 30, 0, false);
        let expected = [
            ("campus", "mountain", 1.0),
            ("campus", "rainforest", 0.5),
            ("campus", "district", 0.5),
            ("campus", "reef", 2.0),
            ("campus", "geothermal_fissure", 2.0),
            ("campus", "pamukkale", 2.0),
            ("campus", "government_plaza", 1.0),
            ("holy_site", "natural_wonder", 2.0),
            ("holy_site", "mountain", 1.0),
            ("holy_site", "forest", 0.5),
            ("holy_site", "district", 0.5),
            ("holy_site", "pamukkale", 1.0),
            ("holy_site", "government_plaza", 1.0),
            ("commercial_hub", "river", 2.0),
            ("commercial_hub", "harbor", 2.0),
            ("commercial_hub", "district", 0.5),
            ("commercial_hub", "pamukkale", 2.0),
            ("commercial_hub", "government_plaza", 1.0),
            ("harbor", "coast_resource", 1.0),
            ("harbor", "district", 0.5),
            ("harbor", "city_center", 2.0),
            ("harbor", "government_plaza", 1.0),
            ("theater_square", "wonder", 2.0),
            ("theater_square", "district", 0.5),
            ("theater_square", "entertainment_complex", 2.0),
            ("theater_square", "water_park", 2.0),
            ("theater_square", "pamukkale", 2.0),
            ("theater_square", "government_plaza", 1.0),
            ("industrial_zone", "quarry", 1.0),
            ("industrial_zone", "strategic_resource", 1.0),
            ("industrial_zone", "mine", 0.5),
            ("industrial_zone", "lumber_mill", 0.5),
            ("industrial_zone", "district", 0.5),
            ("industrial_zone", "aqueduct", 2.0),
            ("industrial_zone", "canal", 2.0),
            ("industrial_zone", "dam", 2.0),
            ("industrial_zone", "government_plaza", 1.0),
            ("acropolis", "wonder", 2.0),
            ("acropolis", "district", 1.0),
            ("acropolis", "city_center", 1.0),
            ("acropolis", "entertainment_complex", 2.0),
            ("acropolis", "water_park", 2.0),
            ("acropolis", "pamukkale", 2.0),
            ("acropolis", "government_plaza", 1.0),
            ("observatory", "plantation", 2.0),
            ("observatory", "farm", 0.5),
            ("observatory", "district", 0.5),
            ("observatory", "government_plaza", 1.0),
            ("seowon", "self", 4.0),
            ("seowon", "district", -1.0),
            ("seowon", "government_plaza", 1.0),
            ("thanh", "district", 2.0),
            ("suguba", "river", 2.0),
            ("suguba", "holy_site", 2.0),
            ("suguba", "district", 0.5),
            ("suguba", "pamukkale", 2.0),
            ("suguba", "government_plaza", 1.0),
            ("hansa", "commercial_hub", 2.0),
            ("hansa", "resource", 1.0),
            ("hansa", "district", 0.5),
            ("hansa", "aqueduct", 2.0),
            ("hansa", "canal", 2.0),
            ("hansa", "dam", 2.0),
            ("hansa", "government_plaza", 1.0),
            ("oppidum", "quarry", 2.0),
            ("oppidum", "strategic_resource", 2.0),
            ("oppidum", "government_plaza", 1.0),
        ];
        for (district, source, value) in expected {
            assert_eq!(
                adjacency_value(&game, district, source),
                value,
                "{district}/{source}"
            );
        }

        for (replacement, base) in [
            ("lavra", "holy_site"),
            ("cothon", "harbor"),
            ("royal_navy_dockyard", "harbor"),
        ] {
            assert_eq!(
                game.rules.districts[replacement].adjacency, game.rules.districts[base].adjacency,
                "{replacement} must inherit {base} adjacency"
            );
        }
    }

    #[test]
    fn adjacency_rounding_policies_wonders_and_unique_families_are_runtime_correct() {
        let (mut game, city, position, ring) = controlled_game();
        game.map.tiles.get_mut(&ring[0]).unwrap().feature = Some("jungle".to_string());
        game.map.tiles.get_mut(&ring[1]).unwrap().district = Some("encampment".to_string());
        assert_eq!(
            game.district_yields("campus", position).science,
            0.0,
            "one Rainforest and one district are separate half-point buckets"
        );
        game.map.tiles.get_mut(&ring[2]).unwrap().feature = Some("jungle".to_string());
        game.map.tiles.get_mut(&ring[3]).unwrap().terrain = "mountain".to_string();
        game.map.tiles.get_mut(&ring[4]).unwrap().feature = Some("reef".to_string());
        game.map.tiles.get_mut(&ring[5]).unwrap().feature = Some("geothermal_fissure".to_string());
        assert_eq!(game.district_yields("campus", position).science, 6.0);
        game.players[0]
            .policies
            .insert("natural_philosophy".to_string());
        assert_eq!(game.district_yields("campus", position).science, 12.0);

        game.players[0].policies.clear();
        for neighbor in &ring {
            let tile = game.map.tiles.get_mut(neighbor).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.district = None;
            tile.wonder = None;
        }
        game.map.tiles.get_mut(&ring[0]).unwrap().wonder = Some("oracle".to_string());
        assert_eq!(
            game.district_yields("theater_square", position).culture,
            2.0
        );

        game.map.tiles.get_mut(&ring[0]).unwrap().wonder = None;
        game.map.tiles.get_mut(&ring[0]).unwrap().district = Some("suguba".to_string());
        assert_eq!(
            game.district_yields("hansa", position).production,
            2.0,
            "a unique Commercial Hub satisfies Hansa's family adjacency"
        );

        let center = game.cities[&city].pos;
        let harbor = game.nbrs(center)[0];
        for neighbor in game.nbrs(harbor) {
            game.map.tiles.get_mut(&neighbor).unwrap().district = None;
        }
        game.map.tiles.get_mut(&harbor).unwrap().owner_city = Some(city);
        let other = game
            .nbrs(harbor)
            .into_iter()
            .find(|neighbor| *neighbor != center)
            .unwrap();
        game.map.tiles.get_mut(&other).unwrap().district = Some("campus".to_string());
        assert_eq!(
            game.district_yields("harbor", harbor).gold,
            3.0,
            "the City Center counts as both its major source and a district"
        );
    }

    #[test]
    fn neighborhood_and_preserve_housing_follow_tile_appeal() {
        let (mut game, _, position, ring) = controlled_game();
        assert_eq!(game.tile_appeal(position), 0);
        assert_eq!(game.district_housing("neighborhood", position), 4.0);
        assert_eq!(game.district_housing("preserve", position), 1.0);
        assert_eq!(game.district_housing("mbanza", position), 5.0);

        for neighbor in ring.iter().take(4) {
            game.map.tiles.get_mut(neighbor).unwrap().feature = Some("forest".to_string());
        }
        assert_eq!(game.tile_appeal(position), 4);
        assert_eq!(game.district_housing("neighborhood", position), 6.0);
        assert_eq!(game.district_housing("preserve", position), 3.0);
    }

    #[test]
    fn district_completion_applies_government_envoy_and_culture_bomb_effects() {
        let mut game = Game::new_full(1, 20, 14, 5153, 300, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| game.units[uid].kind == "settler")
            .unwrap();
        let city = game.found_city_for(0, game.units[&settler].pos, None);
        let center = game.cities[&city].pos;
        for position in game.cities[&city].owned_tiles.clone() {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.wonder = None;
        }
        game.players[0].civics.extend([
            "state_workforce".to_string(),
            "diplomatic_service".to_string(),
            "mysticism".to_string(),
        ]);

        let plaza = game.district_sites(city, "government_plaza")[0];
        assert!(game.complete_item(
            0,
            city,
            &Item::District {
                district: "government_plaza".to_string(),
                pos: plaza,
            },
        ));
        assert_eq!(game.governor_titles(0), 1);

        game.cities.get_mut(&city).unwrap().pop = 4;
        let quarter = game
            .district_sites(city, "diplomatic_quarter")
            .into_iter()
            .find(|position| game.wdist(*position, center) == 1)
            .unwrap();
        assert!(game.complete_item(
            0,
            city,
            &Item::District {
                district: "diplomatic_quarter".to_string(),
                pos: quarter,
            },
        ));
        assert_eq!(game.players[0].envoys_free, 1);

        game.cities.get_mut(&city).unwrap().pop = 7;
        let preserve = game
            .wdisk(center, 2)
            .into_iter()
            .find(|position| game.wdist(*position, center) == 2)
            .unwrap();
        {
            let tile = game.map.tiles.get_mut(&preserve).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.wonder = None;
            tile.owner_city = Some(city);
        }
        if !game.cities[&city].owned_tiles.contains(&preserve) {
            game.cities
                .get_mut(&city)
                .unwrap()
                .owned_tiles
                .push(preserve);
        }
        let claim = game
            .nbrs(preserve)
            .into_iter()
            .find(|position| game.map.tiles[position].owner_city.is_none())
            .unwrap();
        assert!(game.complete_item(
            0,
            city,
            &Item::District {
                district: "preserve".to_string(),
                pos: preserve,
            },
        ));
        assert_eq!(game.map.tiles[&claim].owner_city, Some(city));
        assert!(game.cities[&city].owned_tiles.contains(&claim));
    }

    #[test]
    fn special_placement_rules_cover_land_water_features_and_city_distance() {
        let mut game = Game::new_full(1, 20, 14, 5154, 300, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| game.units[uid].kind == "settler")
            .unwrap();
        let city = game.found_city_for(0, game.units[&settler].pos, None);
        let center = game.cities[&city].pos;
        game.cities.get_mut(&city).unwrap().pop = 100;
        game.players[0].techs = game.rules.techs.keys().cloned().collect();
        game.players[0].civics = game.rules.civics.keys().cloned().collect();
        let owned = game.wdisk(center, 3);
        game.cities.get_mut(&city).unwrap().owned_tiles = owned.clone();
        for position in owned {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.wonder = None;
            tile.owner_city = Some(city);
            tile.river_edges = [false; 6];
        }
        let near = game.nbrs(center)[0];
        let far = game
            .wdisk(center, 2)
            .into_iter()
            .find(|position| game.wdist(*position, center) == 2)
            .unwrap();
        let is_site = |game: &Game, district: &str, position: Pos| {
            game.district_sites(city, district).contains(&position)
        };

        assert!(!is_site(&game, "encampment", near));
        assert!(is_site(&game, "encampment", far));
        assert!(!is_site(&game, "preserve", near));
        assert!(is_site(&game, "preserve", far));

        game.map.tiles.get_mut(&far).unwrap().hills = true;
        assert!(is_site(&game, "acropolis", far));
        assert!(!is_site(&game, "aerodrome", far));
        assert!(!is_site(&game, "spaceport", far));
        game.map.tiles.get_mut(&far).unwrap().hills = false;
        assert!(is_site(&game, "aerodrome", far));
        assert!(is_site(&game, "spaceport", far));

        game.map.tiles.get_mut(&far).unwrap().terrain = "lake".to_string();
        assert!(is_site(&game, "harbor", far));
        assert!(!is_site(&game, "water_park", far));
        game.map.tiles.get_mut(&far).unwrap().terrain = "coast".to_string();
        assert!(is_site(&game, "water_park", far));
        game.map.tiles.get_mut(&far).unwrap().feature = Some("reef".to_string());
        assert!(!is_site(&game, "harbor", far));
        assert!(!is_site(&game, "water_park", far));

        {
            let tile = game.map.tiles.get_mut(&far).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = Some("grassland_floodplains".to_string());
        }
        assert!(
            is_site(&game, "campus", far),
            "Gathering Storm allows ordinary districts on Floodplains"
        );
        game.map.tiles.get_mut(&far).unwrap().river_edges[0] = true;
        game.map.tiles.get_mut(&far).unwrap().river_edges[1] = true;
        assert!(is_site(&game, "dam", far));

        let center_edge = game.map.direction_to(near, center).unwrap();
        {
            let tile = game.map.tiles.get_mut(&near).unwrap();
            tile.feature = None;
            tile.river_edges[(center_edge + 1) % 6] = true;
        }
        assert!(is_site(&game, "aqueduct", near));
        assert!(!is_site(&game, "aqueduct", far));
    }
}
