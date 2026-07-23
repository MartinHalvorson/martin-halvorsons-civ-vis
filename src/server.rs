//! Zero-dependency local HTTP server for the human-vs-AI browser GUI.
//! Endpoints: GET / (page), GET /state, GET /save, GET /rules, GET /pedia,
//! POST /action, POST /step, POST /view, POST /spectator-status, POST /new,
//! POST /supervisor-new.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::ai::{AdvancedAi, Ai, BasicAi};
use crate::game::{Action, Game, GameOptions, VictoryConditions};
use crate::rules::Rules;
use crate::obs::{observation, observation_player_view, observation_spectator};
use crate::setup::{
    GameSpeed, MapScript, MapSize, CIV6_GAME_SPEEDS, CIV6_MAP_SCRIPTS, CIV6_MAP_SIZES,
};
use crate::Pos;

const EMBEDDED_INDEX: &str = include_str!("../web/index.html");
const EMBEDDED_TERRAIN_ATLAS: &[u8] = include_bytes!("../web/assets/terrain-atlas.png");
const EMBEDDED_FEATURE_ATLAS: &[u8] = include_bytes!("../web/assets/feature-atlas.png");
const EMBEDDED_ENVIRONMENT_FEATURE_ATLAS: &[u8] =
    include_bytes!("../web/assets/environment-feature-atlas.png");
const EMBEDDED_NATURAL_WONDER_ATLAS: &[u8] =
    include_bytes!("../web/assets/natural-wonder-atlas.png");
const EMBEDDED_MOUNTAIN_ATLAS: &[u8] = include_bytes!("../web/assets/mountain-atlas.png");

#[derive(Clone)]
pub struct Params {
    pub num_players: usize,
    pub width: i32,
    pub height: i32,
    pub seed: u64,
    pub map_script: MapScript,
    pub game_speed: GameSpeed,
    pub max_turns: u32,
    pub victory_conditions: VictoryConditions,
    pub num_city_states: usize,
    /// All players AI-driven; the GUI just watches (auto-steps via /step).
    pub spectate: bool,
    pub difficulty: String,
    pub speed: String,
    /// A lifecycle supervisor, rather than the browser countdown, owns the
    /// transition after a completed spectator game.
    pub supervised: bool,
}

pub struct Session {
    pub params: Params,
    pub game: Game,
    ais: Vec<Box<dyn Ai + Send>>,
    spectator_paused: bool,
    /// `None` is the omniscient spectator; `Some(pid)` is that major
    /// civilization's fog-of-war perspective. Only meaningful in spectate
    /// mode—the AI still controls every seat either way.
    view_player: Option<usize>,
    /// Irreversible event-log history and the running totals for active wars.
    /// Session scope prevents destroyed infrastructure or a temporarily lost
    /// high-population city from being announced as a first a second time.
    chronicle: ChronicleState,
    /// Manual new-game handoff consumed by the external spectator supervisor.
    /// The current process stays available until the requested runtime is ready.
    supervisor_request: Option<Value>,
}

#[derive(Clone)]
struct ChronicleCity {
    name: String,
    owner: usize,
    pop: i32,
    occupied_from: Option<usize>,
}

#[derive(Clone)]
struct ChronicleDistrict {
    city: u32,
    district: String,
    owner: usize,
}

struct ChronicleSnapshot {
    turn: u32,
    cities: BTreeMap<u32, ChronicleCity>,
    districts: BTreeMap<Pos, ChronicleDistrict>,
    buildings: BTreeMap<(u32, String), usize>,
    wonders: BTreeMap<String, usize>,
    religions: Vec<Option<String>>,
    governments: Vec<Option<String>>,
    suzerains: BTreeMap<usize, Option<usize>>,
    tech_eras: Vec<usize>,
    civic_eras: Vec<usize>,
    majors: Vec<bool>,
    wars: BTreeSet<(usize, usize)>,
    military_units: BTreeMap<u32, usize>,
    combat_owners: BTreeMap<Pos, BTreeSet<usize>>,
}

#[derive(Clone, Default)]
struct WarLosses {
    units: u32,
    cities: u32,
}

#[derive(Clone)]
struct ChronicleWar {
    aggressor: usize,
    defender: usize,
    losses: BTreeMap<usize, WarLosses>,
}

impl ChronicleWar {
    fn new(aggressor: usize, defender: usize) -> Self {
        Self {
            aggressor,
            defender,
            losses: BTreeMap::new(),
        }
    }

    fn losses_for(&self, player: usize) -> WarLosses {
        self.losses.get(&player).cloned().unwrap_or_default()
    }
}

struct ChronicleState {
    districts: BTreeSet<String>,
    buildings: BTreeSet<String>,
    population_milestones: Vec<i32>,
    wars: BTreeMap<(usize, usize), ChronicleWar>,
}

pub struct SpectatorStep {
    pub player: usize,
    pub actions: Vec<Action>,
    pub world_events: Vec<Value>,
}

impl ChronicleSnapshot {
    fn capture(game: &Game) -> Self {
        let mut districts = BTreeMap::new();
        let mut buildings = BTreeMap::new();
        let mut wonders = BTreeMap::new();
        let mut combat_owners: BTreeMap<Pos, BTreeSet<usize>> = BTreeMap::new();
        for city in game.cities.values() {
            for (district, position) in &city.districts {
                districts.insert(
                    *position,
                    ChronicleDistrict {
                        city: city.id,
                        district: district.clone(),
                        owner: city.owner,
                    },
                );
            }
            for building in &city.buildings {
                if game
                    .rules
                    .buildings
                    .get(building)
                    .is_some_and(|spec| spec.buildable)
                {
                    buildings.insert((city.id, building.clone()), city.owner);
                }
            }
            for wonder in city.wonders.keys() {
                wonders.insert(wonder.clone(), city.owner);
            }
            combat_owners
                .entry(city.pos)
                .or_default()
                .insert(city.owner);
        }
        let military_units = game
            .units
            .values()
            .filter(|unit| game.rules.units[unit.kind.as_str()].class == "military")
            .map(|unit| {
                combat_owners
                    .entry(unit.pos)
                    .or_default()
                    .insert(unit.owner);
                (unit.id, unit.owner)
            })
            .collect();
        let tree_era = |nodes: &BTreeSet<String>, technology: bool| {
            nodes
                .iter()
                .filter_map(|node| {
                    if technology {
                        game.rules.techs.get(node).map(|spec| spec.era)
                    } else {
                        game.rules.civics.get(node).map(|spec| spec.era)
                    }
                })
                .max()
                .unwrap_or(0)
        };
        Self {
            turn: game.turn,
            cities: game
                .cities
                .values()
                .map(|city| {
                    (
                        city.id,
                        ChronicleCity {
                            name: city.name.clone(),
                            owner: city.owner,
                            pop: city.pop,
                            occupied_from: city.occupied_from,
                        },
                    )
                })
                .collect(),
            districts,
            buildings,
            wonders,
            religions: game
                .players
                .iter()
                .map(|player| player.religion.clone())
                .collect(),
            governments: game
                .players
                .iter()
                .map(|player| player.government.clone())
                .collect(),
            suzerains: game
                .players
                .iter()
                .filter(|player| player.is_minor && !player.is_barbarian)
                .map(|player| (player.id, game.suzerain_of(player.id)))
                .collect(),
            tech_eras: game
                .players
                .iter()
                .map(|player| tree_era(&player.techs, true))
                .collect(),
            civic_eras: game
                .players
                .iter()
                .map(|player| tree_era(&player.civics, false))
                .collect(),
            majors: game
                .players
                .iter()
                .map(|player| !player.is_minor && !player.is_barbarian)
                .collect(),
            wars: game.at_war.clone(),
            military_units,
            combat_owners,
        }
    }
}

fn completed_districts(game: &Game) -> BTreeSet<String> {
    game.cities
        .values()
        .flat_map(|city| city.districts.keys())
        .cloned()
        .collect()
}

fn completed_buildings(game: &Game) -> BTreeSet<String> {
    game.cities
        .values()
        .flat_map(|city| city.buildings.iter())
        .filter(|building| {
            game.rules
                .buildings
                .get(*building)
                .is_some_and(|spec| spec.buildable)
        })
        .cloned()
        .collect()
}

fn population_milestone(population: i32) -> i32 {
    if population < 4 {
        0
    } else {
        4 + ((population - 4) / 3) * 3
    }
}

impl ChronicleState {
    fn from_game(game: &Game) -> Self {
        let population_milestones = game
            .players
            .iter()
            .map(|player| {
                game.cities
                    .values()
                    .filter(|city| city.owner == player.id)
                    .map(|city| city.pop)
                    .max()
                    .map(population_milestone)
                    .unwrap_or(0)
            })
            .collect();
        let wars = game
            .at_war
            .iter()
            .map(|&(first, second)| ((first, second), ChronicleWar::new(first, second)))
            .collect();
        Self {
            districts: completed_districts(game),
            buildings: completed_buildings(game),
            population_milestones,
            wars,
        }
    }
}

fn chronicle_war_pair(first: usize, second: usize) -> (usize, usize) {
    if first < second {
        (first, second)
    } else {
        (second, first)
    }
}

fn war_totals_event(event_type: &str, war: &ChronicleWar, turn: u32) -> Value {
    let aggressor = war.losses_for(war.aggressor);
    let defender = war.losses_for(war.defender);
    json!({
        "type": event_type,
        "aggressor": war.aggressor,
        "defender": war.defender,
        "aggressor_units_lost": aggressor.units,
        "aggressor_cities_lost": aggressor.cities,
        "defender_units_lost": defender.units,
        "defender_cities_lost": defender.cities,
        "turn": turn,
    })
}

fn chronicle_world_events(
    before: &ChronicleSnapshot,
    after: &ChronicleSnapshot,
    actor: usize,
    actions: &[Action],
    chronicle: &mut ChronicleState,
) -> Vec<Value> {
    let mut events = Vec::new();
    let turn = after.turn;

    for (wonder, owner) in &after.wonders {
        if !before.wonders.contains_key(wonder) {
            events.push(json!({
                "type": "wonder_built", "player": owner,
                "wonder": wonder, "turn": turn,
            }));
        }
    }

    for (player, religion) in after.religions.iter().enumerate() {
        if before.religions.get(player).is_some_and(Option::is_none) {
            if let Some(religion) = religion {
                events.push(json!({
                    "type": "religion_founded", "player": player,
                    "religion": religion, "turn": turn,
                }));
            }
        }
    }

    let mut new_districts: Vec<_> = after
        .districts
        .iter()
        .filter(|(position, _)| !before.districts.contains_key(position))
        .map(|(_, district)| district)
        .collect();
    new_districts.sort_by_key(|district| district.city);
    for district in new_districts {
        if chronicle.districts.insert(district.district.clone()) {
            let city = after
                .cities
                .get(&district.city)
                .map(|city| city.name.as_str());
            events.push(json!({
                "type": "district_first", "player": district.owner,
                "district": district.district, "city": city, "turn": turn,
            }));
        }
    }

    let mut new_buildings: Vec<_> = after
        .buildings
        .iter()
        .filter(|(key, _)| !before.buildings.contains_key(*key))
        .collect();
    new_buildings.sort_by_key(|((city, building), _)| (*city, building.as_str()));
    for ((city_id, building), owner) in new_buildings {
        if chronicle.buildings.insert(building.clone()) {
            let city = after.cities.get(city_id).map(|city| city.name.as_str());
            events.push(json!({
                "type": "building_first", "player": owner,
                "building": building, "city": city, "turn": turn,
            }));
        }
    }

    for (player, major) in after.majors.iter().copied().enumerate() {
        if !major {
            continue;
        }
        let Some(city) = after
            .cities
            .values()
            .filter(|city| city.owner == player)
            .max_by_key(|city| (city.pop, std::cmp::Reverse(city.name.as_str())))
        else {
            continue;
        };
        let milestone = population_milestone(city.pop);
        let seen = chronicle
            .population_milestones
            .get_mut(player)
            .expect("chronicle population ledger matches players");
        if milestone > *seen {
            // If conquest jumps over several thresholds, announce the current
            // one and retire the lower thresholds instead of flooding the log.
            *seen = milestone;
            events.push(json!({
                "type": "population_milestone", "player": player,
                "population": milestone, "city": city.name, "turn": turn,
            }));
        }
    }

    // Capture decisions are resolved before an AI can end its turn. Reading
    // those decisions catches kept, razed, and immediately liberated cities.
    let mut captured = BTreeSet::new();
    for action in actions {
        let city = match action {
            Action::KeepCity { city }
            | Action::RazeCity { city }
            | Action::LiberateCity { city } => Some(*city),
            _ => None,
        };
        let Some(city) = city else { continue };
        let Some(previous) = before.cities.get(&city) else {
            continue;
        };
        if captured.insert(city) {
            events.push(json!({
                "type": "city_captured", "player": actor,
                "former": previous.owner, "city": previous.name,
                "turn": turn,
            }));
        }
    }
    // Also cover a conquest that ended the match before its keep/raze choice
    // was logged.
    for (city, previous) in &before.cities {
        let Some(current) = after.cities.get(city) else {
            continue;
        };
        if current.owner != previous.owner
            && current.occupied_from == Some(previous.owner)
            && captured.insert(*city)
        {
            events.push(json!({
                "type": "city_captured", "player": current.owner,
                "former": previous.owner, "city": previous.name,
                "turn": turn,
            }));
        }
    }

    let active_wars: BTreeSet<_> = before.wars.union(&after.wars).copied().collect();
    for &(first, second) in after.wars.difference(&before.wars) {
        let (aggressor, defender) = if actor == first {
            (first, second)
        } else if actor == second {
            (second, first)
        } else {
            (first, second)
        };
        chronicle
            .wars
            .insert((first, second), ChronicleWar::new(aggressor, defender));
        events.push(json!({
            "type": "war_started", "aggressor": aggressor,
            "defender": defender, "turn": turn,
        }));
    }
    for &(first, second) in &active_wars {
        chronicle
            .wars
            .entry((first, second))
            .or_insert_with(|| ChronicleWar::new(first, second));
    }

    // Only vanished military units count as war losses. Corps/Army formation
    // consumes one constituent without a battle, so exclude both participants
    // and let the still-present one identify the survivor.
    let combined_units: BTreeSet<u32> = actions
        .iter()
        .flat_map(|action| match action {
            Action::CombineUnits { unit, with } => vec![*unit, *with],
            _ => Vec::new(),
        })
        .collect();
    let mut lost_units: BTreeMap<usize, u32> = BTreeMap::new();
    for (unit, owner) in &before.military_units {
        if !after.military_units.contains_key(unit) && !combined_units.contains(unit) {
            *lost_units.entry(*owner).or_default() += 1;
        }
    }

    let mut targeted_opponents = BTreeSet::new();
    for target in actions.iter().filter_map(|action| match action {
        Action::Attack { target, .. }
        | Action::Ranged { target, .. }
        | Action::AirStrike { target, .. }
        | Action::CityStrike { target, .. }
        | Action::EncampmentStrike { target, .. } => Some(*target),
        _ => None,
    }) {
        if let Some(owners) = before.combat_owners.get(&target) {
            targeted_opponents.extend(owners.iter().copied().filter(|owner| {
                *owner != actor && active_wars.contains(&chronicle_war_pair(actor, *owner))
            }));
        }
    }
    let enemy_losers: BTreeSet<_> = lost_units
        .keys()
        .copied()
        .filter(|owner| *owner != actor && active_wars.contains(&chronicle_war_pair(actor, *owner)))
        .collect();
    let actor_opponent = if targeted_opponents.len() == 1 {
        targeted_opponents.first().copied()
    } else if enemy_losers.len() == 1 {
        enemy_losers.first().copied()
    } else {
        let opponents: BTreeSet<_> = active_wars
            .iter()
            .filter_map(|&(first, second)| {
                if first == actor {
                    Some(second)
                } else if second == actor {
                    Some(first)
                } else {
                    None
                }
            })
            .collect();
        (opponents.len() == 1).then(|| *opponents.first().unwrap())
    };

    let mut changed_wars = BTreeSet::new();
    for (owner, losses) in lost_units {
        let opponent = if owner == actor {
            actor_opponent
        } else if active_wars.contains(&chronicle_war_pair(actor, owner)) {
            Some(actor)
        } else {
            None
        };
        let Some(opponent) = opponent else { continue };
        let pair = chronicle_war_pair(owner, opponent);
        let war = chronicle
            .wars
            .entry(pair)
            .or_insert_with(|| ChronicleWar::new(actor, opponent));
        war.losses.entry(owner).or_default().units += losses;
        changed_wars.insert(pair);
    }

    let mut lost_cities = BTreeSet::new();
    for (city_id, previous) in &before.cities {
        let conqueror = match after.cities.get(city_id) {
            Some(current) if current.owner != previous.owner => Some(current.owner),
            None if captured.contains(city_id) => Some(actor),
            _ => None,
        };
        let Some(conqueror) = conqueror else {
            continue;
        };
        let pair = chronicle_war_pair(previous.owner, conqueror);
        if previous.owner == conqueror
            || !active_wars.contains(&pair)
            || !lost_cities.insert(*city_id)
        {
            continue;
        }
        let war = chronicle
            .wars
            .entry(pair)
            .or_insert_with(|| ChronicleWar::new(conqueror, previous.owner));
        war.losses.entry(previous.owner).or_default().cities += 1;
        changed_wars.insert(pair);
    }

    for pair in changed_wars {
        if after.wars.contains(&pair) {
            if let Some(war) = chronicle.wars.get(&pair) {
                events.push(war_totals_event("war_progress", war, turn));
            }
        }
    }
    let ended_wars: Vec<_> = before.wars.difference(&after.wars).copied().collect();
    for pair in ended_wars {
        if let Some(war) = chronicle.wars.remove(&pair) {
            events.push(war_totals_event("war_ended", &war, turn));
        }
    }

    for (city_state, current) in &after.suzerains {
        let previous = before.suzerains.get(city_state).copied().flatten();
        if previous != *current {
            events.push(json!({
                "type": "suzerain_changed", "city_state": city_state,
                "from": previous, "to": current, "turn": turn,
            }));
        }
    }

    let first_era_events =
        |track: &str, before_eras: &[usize], after_eras: &[usize], events: &mut Vec<Value>| {
            let before_lead = before_eras
                .iter()
                .enumerate()
                .filter(|(player, _)| before.majors.get(*player) == Some(&true))
                .map(|(_, era)| *era)
                .max()
                .unwrap_or(0);
            let after_lead = after_eras
                .iter()
                .enumerate()
                .filter(|(player, _)| after.majors.get(*player) == Some(&true))
                .map(|(_, era)| *era)
                .max()
                .unwrap_or(0);
            for era in (before_lead + 1)..=after_lead {
                let Some(player) = after_eras
                    .iter()
                    .enumerate()
                    .find_map(|(player, after_era)| {
                        (after.majors.get(player) == Some(&true)
                            && *after_era >= era
                            && before_eras.get(player).copied().unwrap_or(0) < era)
                            .then_some(player)
                    })
                else {
                    continue;
                };
                events.push(json!({
                    "type": "era_first", "player": player,
                    "track": track, "era": era, "turn": turn,
                }));
            }
        };
    first_era_events(
        "technology",
        &before.tech_eras,
        &after.tech_eras,
        &mut events,
    );
    first_era_events("civics", &before.civic_eras, &after.civic_eras, &mut events);

    for (player, government) in after.governments.iter().enumerate() {
        if after.majors.get(player) != Some(&true) {
            continue;
        }
        let previous = before.governments.get(player).cloned().flatten();
        if previous != *government {
            events.push(json!({
                "type": "government_changed", "player": player,
                "from": previous, "to": government, "turn": turn,
            }));
        }
    }

    events
}

/// Server-side exhibition state: in spectate mode a background thread steps
/// the game at `pace_ms` per major turn and restarts 10s after a victory, so
/// games keep running with no browser attached.
pub struct Shared {
    pub session: Mutex<Session>,
    pub pace_ms: AtomicU64,
    pub paused: AtomicBool,
    pub restart_in: AtomicU64, // ms until auto-restart; u64::MAX = not pending
}

const RESTART_MS: u64 = 10_000;

impl Session {
    fn ai_fleet(game: &Game) -> Vec<Box<dyn Ai + Send>> {
        game.players
            .iter()
            .map(|p| -> Box<dyn Ai + Send> {
                if p.is_minor || p.is_barbarian {
                    return Box::new(BasicAi::new());
                }
                Box::new(AdvancedAi::new())
            })
            .collect()
    }

    pub fn new(params: Params) -> Session {
        // Seat 0 is the person at the keyboard, which is what decides who the
        // difficulty hands its bonuses to. A spectated game has nobody there.
        let human_seats = if params.spectate {
            BTreeSet::new()
        } else {
            BTreeSet::from([0usize])
        };
        let mut game = Game::new_with(GameOptions {
            map_script: params.map_script,
            difficulty: params.difficulty.clone(),
            speed: params.speed.clone(),
            human_seats,
            ..GameOptions::new(
                params.num_players,
                params.width,
                params.height,
                params.seed,
                params.max_turns,
                params.num_city_states,
            )
        });
        game.victory_conditions = params.victory_conditions;
        // Paired and multiplayer evaluation make the hierarchical agent the
        // strongest built-in default. Minors/barbarians retain the cheaper
        // baseline because they do not need empire-level planning.
        let ais = Self::ai_fleet(&game);
        let chronicle = ChronicleState::from_game(&game);
        Session {
            params,
            game,
            ais,
            spectator_paused: false,
            view_player: None,
            chronicle,
            supervisor_request: None,
        }
    }

    /// Restore an interrupted match and rebuild only the AIs' transient plans.
    /// The serialized game retains the authoritative RNG and world state.
    pub fn from_game(mut params: Params, game: Game) -> Session {
        params.num_players = game
            .players
            .iter()
            .filter(|player| !player.is_minor && !player.is_barbarian)
            .count();
        params.num_city_states = game
            .players
            .iter()
            .filter(|player| player.is_minor && !player.is_barbarian)
            .count();
        params.width = game.map.width;
        params.height = game.map.height;
        params.seed = game.seed;
        params.map_script = game.map_script;
        params.game_speed = game.game_speed;
        params.max_turns = game.max_turns;
        params.difficulty = game.difficulty.clone();
        params.speed = game.speed.clone();
        params.victory_conditions = game.victory_conditions;
        let ais = Self::ai_fleet(&game);
        let chronicle = ChronicleState::from_game(&game);
        Session {
            params,
            game,
            ais,
            spectator_paused: false,
            view_player: None,
            chronicle,
            supervisor_request: None,
        }
    }

    fn set_view_player(&mut self, player: Option<usize>) -> Result<(), String> {
        if !self.params.spectate && player.is_none() {
            return Err("player views are only available in spectate mode".into());
        }
        if let Some(pid) = player {
            let Some(candidate) = self.game.players.get(pid) else {
                return Err(format!("unknown player {pid}"));
            };
            if candidate.is_minor || candidate.is_barbarian {
                return Err(format!("player {pid} is not a major civilization"));
            }
            // Selecting a civilization from the HUD is also the handoff from
            // an interactive match to AI-only observation. Keep the current
            // world intact; the already-created AI fleet can take over every
            // seat on the next spectator step.
            self.params.spectate = true;
        }
        self.view_player = player;
        Ok(())
    }

    /// Start a requested world, rejecting a delayed result-countdown request
    /// after the supervisor has already replaced the finished server.
    fn start_new_game(&mut self, request: &Value) -> Result<(), String> {
        if self.params.supervised {
            return Err("the spectator supervisor owns in-process game replacement".into());
        }
        if let Some(finished) = request.get("replace_finished") {
            let expected_seed = finished["seed"]
                .as_u64()
                .ok_or_else(|| "replace_finished.seed must be an integer".to_string())?;
            let expected_instance = finished["server_instance"]
                .as_u64()
                .ok_or_else(|| "replace_finished.server_instance must be an integer".to_string())?;
            if self.game.winner.is_none()
                || self.game.seed != expected_seed
                || expected_instance != std::process::id() as u64
            {
                return Err("finished game is no longer the active session".into());
            }
        } else if self.params.spectate
            && self.game.winner.is_none()
            && request["force"].as_bool() != Some(true)
        {
            // Old spectator pages used an unguarded result timer. If one
            // survives a process handoff, it must not reset a healthy game.
            // The visible setup button explicitly opts into a manual reset.
            return Err("active spectator game requires an explicit reset".into());
        }
        let previous_view = self.view_player;
        let params = new_game_params(&self.params, request);
        let mut next = Session::new(params);
        // Observation perspective is a display setting, not part of the
        // simulated world. Keep it when rolling into another spectator game
        // as long as that major-player seat still exists in the new setup.
        if next.params.spectate {
            next.view_player = previous_view.filter(|pid| {
                next.game
                    .players
                    .get(*pid)
                    .is_some_and(|player| !player.is_minor && !player.is_barbarian)
            });
        }
        *self = next;
        Ok(())
    }

    fn request_supervised_new_game(&mut self, request: &Value) -> Result<(), String> {
        if !self.params.supervised {
            return Err("fresh-code launches require the spectator supervisor".into());
        }
        let mode = request["mode"]
            .as_str()
            .ok_or_else(|| "mode must be restart or fresh_code".to_string())?;
        if mode != "restart" && mode != "fresh_code" {
            return Err("mode must be restart or fresh_code".into());
        }

        let mut params = new_game_params(&self.params, request);
        params.spectate = true;
        let victories = [
            (params.victory_conditions.science, "science"),
            (params.victory_conditions.culture, "culture"),
            (params.victory_conditions.religious, "religious"),
            (params.victory_conditions.diplomatic, "diplomatic"),
            (params.victory_conditions.domination, "domination"),
            (params.victory_conditions.score, "score"),
        ]
        .into_iter()
        .filter_map(|(enabled, name)| enabled.then_some(name))
        .collect::<Vec<_>>();
        self.supervisor_request = Some(json!({
            "mode": mode,
            "server_instance": std::process::id(),
            "settings": {
                "players": params.num_players,
                "width": params.width,
                "height": params.height,
                "city_states": params.num_city_states,
                "turns": params.max_turns,
                "map": params.map_script.id(),
                "speed": params.game_speed.id(),
                "victories": victories,
            }
        }));
        self.spectator_paused = true;
        Ok(())
    }

    pub fn state(&self) -> Value {
        if self.params.spectate {
            let g = &self.game;
            // The omniscient view still needs an empire perspective for the
            // side-panel summary. Follow the acting major, falling back when
            // a city-state or barbarian is up.
            let summary_pid = if g.players[g.current].is_minor || g.players[g.current].is_barbarian
            {
                g.players
                    .iter()
                    .find(|p| !p.is_minor && !p.is_barbarian && p.alive)
                    .map(|p| p.id)
                    .unwrap_or(0)
            } else {
                g.current
            };
            let mut o = match self.view_player {
                Some(pid) => observation_player_view(g, pid),
                None => observation_spectator(g, summary_pid),
            };
            if let Some(players) = o["players"].as_array_mut() {
                for player in players {
                    let Some(id) = player["id"].as_u64().map(|id| id as usize) else {
                        continue;
                    };
                    if let Some(strategy) = self.ais.get(id).and_then(|ai| ai.strategy_label()) {
                        player["ai_strategy"] = json!(strategy);
                    }
                }
            }
            o["spectate"] = json!(true);
            o["supervised"] = json!(self.params.supervised);
            o["spectator_paused"] = json!(self.spectator_paused);
            o["view_player"] = json!(self.view_player);
            o["victory_conditions"] = json!(self.game.victory_conditions);
            o["supervisor_request"] = json!(self.supervisor_request);
            o["legal_actions"] = json!([]);
            // Lets a long-running spectator notice that its server was
            // rebuilt/restarted between games and reload the latest UI.
            o["server_instance"] = json!(std::process::id());
            return o;
        }
        let mut o = observation(&self.game, 0);
        o["spectate"] = json!(false);
        o["supervised"] = json!(self.params.supervised);
        o["view_player"] = json!(0);
        o["victory_conditions"] = json!(self.game.victory_conditions);
        o["supervisor_request"] = json!(self.supervisor_request);
        o["legal_actions"] = serde_json::to_value(self.game.legal_actions(0)).unwrap();
        o["server_instance"] = json!(std::process::id());
        o
    }

    /// Spectator mode: play out exactly one player's turn with its AI.
    /// Returns the pid and successful actions so the observer UI can explain
    /// the AI's decisions instead of showing only their eventual outcomes.
    pub fn step(&mut self) -> (usize, Vec<Action>) {
        let g = &mut self.game;
        let pid = g.current;
        let log_start = g.log.len();
        if g.winner.is_some() {
            return (pid, vec![]);
        }
        self.ais[pid].take_turn(g, pid);
        if g.current == pid && g.winner.is_none() {
            let _ = g.apply(pid, &Action::EndTurn);
        }
        let actions = g.log[log_start..]
            .iter()
            .map(|(_, action)| action.clone())
            .collect();
        (pid, actions)
    }

    /// Advance a bounded batch while retaining each civilization's action
    /// trace. The HTTP layer can then serialize the large world observation
    /// once per browser paint instead of once per AI turn.
    fn spectator_step(&mut self) -> SpectatorStep {
        let before = ChronicleSnapshot::capture(&self.game);
        let (player, actions) = self.step();
        let after = ChronicleSnapshot::capture(&self.game);
        let world_events =
            chronicle_world_events(&before, &after, player, &actions, &mut self.chronicle);
        SpectatorStep {
            player,
            actions,
            world_events,
        }
    }

    pub fn step_many(&mut self, count: usize) -> Vec<SpectatorStep> {
        let mut steps = Vec::new();
        for _ in 0..count.clamp(1, 12) {
            steps.push(self.spectator_step());
            if self.game.winner.is_some() {
                break;
            }
        }
        steps
    }
    pub fn act(&mut self, v: &Value) -> Option<String> {
        let action: Action = match serde_json::from_value(v.clone()) {
            Ok(a) => a,
            Err(e) => return Some(format!("bad action: {e}")),
        };
        if let Err(e) = self.game.apply(0, &action) {
            return Some(e);
        }
        if matches!(action, Action::EndTurn) {
            let g = &mut self.game;
            let mut guard = 0;
            while g.winner.is_none()
                && g.current != 0
                && g.players[0].alive
                && guard < 2 * g.players.len()
            {
                let pid = g.current;
                self.ais[pid].take_turn(g, pid);
                if g.current == pid && g.winner.is_none() {
                    let _ = g.apply(pid, &Action::EndTurn);
                }
                guard += 1;
            }
        }
        None
    }
}

fn index_html() -> Vec<u8> {
    for p in ["web/index.html"] {
        if let Ok(b) = std::fs::read(p) {
            return b;
        }
    }
    EMBEDDED_INDEX.as_bytes().to_vec()
}

fn terrain_atlas() -> Vec<u8> {
    std::fs::read("web/assets/terrain-atlas.png")
        .unwrap_or_else(|_| EMBEDDED_TERRAIN_ATLAS.to_vec())
}

fn feature_atlas() -> Vec<u8> {
    std::fs::read("web/assets/feature-atlas.png")
        .unwrap_or_else(|_| EMBEDDED_FEATURE_ATLAS.to_vec())
}

fn environment_feature_atlas() -> Vec<u8> {
    std::fs::read("web/assets/environment-feature-atlas.png")
        .unwrap_or_else(|_| EMBEDDED_ENVIRONMENT_FEATURE_ATLAS.to_vec())
}

fn natural_wonder_atlas() -> Vec<u8> {
    std::fs::read("web/assets/natural-wonder-atlas.png")
        .unwrap_or_else(|_| EMBEDDED_NATURAL_WONDER_ATLAS.to_vec())
}

fn mountain_atlas() -> Vec<u8> {
    std::fs::read("web/assets/mountain-atlas.png")
        .unwrap_or_else(|_| EMBEDDED_MOUNTAIN_ATLAS.to_vec())
}

fn respond(stream: &mut TcpStream, code: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn respond_json(stream: &mut TcpStream, v: &Value) {
    respond(
        stream,
        "200 OK",
        "application/json",
        v.to_string().as_bytes(),
    );
}

fn request_path(target: &str) -> &str {
    target.split_once('?').map_or(target, |(path, _)| path)
}

fn new_game_params(current: &Params, request: &Value) -> Params {
    let mut p = current.clone();
    if let Some(v) = request["num_players"].as_u64() {
        p.num_players = v as usize;
        let size = MapSize::for_players(p.num_players);
        p.width = size.width;
        p.height = size.height;
        p.num_city_states = size.default_city_states;
    }
    if let Some(v) = request["seed"].as_u64() {
        p.seed = v;
    }
    if let Some(v) = request["map_script"].as_str().and_then(MapScript::from_id) {
        p.map_script = v;
    }
    if let Some(v) = request["game_speed"].as_str().and_then(GameSpeed::from_id) {
        p.game_speed = v;
        p.speed = v.id().to_string();
        p.max_turns = v.turn_limit();
    }
    if let Some(v) = request["max_turns"].as_u64() {
        p.max_turns = v as u32;
    }
    if let Some(victories) = request["victory_conditions"].as_object() {
        for (name, enabled) in victories {
            let Some(enabled) = enabled.as_bool() else {
                continue;
            };
            match name.as_str() {
                "science" => p.victory_conditions.science = enabled,
                "culture" => p.victory_conditions.culture = enabled,
                "religious" => p.victory_conditions.religious = enabled,
                "diplomatic" => p.victory_conditions.diplomatic = enabled,
                "domination" => p.victory_conditions.domination = enabled,
                "score" => p.victory_conditions.score = enabled,
                _ => {}
            }
        }
    }
    // Advanced clients can still deliberately override individual stock
    // settings by sending them alongside num_players.
    if let Some(v) = request["width"].as_i64() {
        p.width = v as i32;
    }
    if let Some(v) = request["height"].as_i64() {
        p.height = v as i32;
    }
    if let Some(v) = request["num_city_states"].as_u64() {
        p.num_city_states = v as usize;
    }
    if let Some(v) = request["spectate"].as_bool() {
        p.spectate = v;
    }
    let rules = Rules::embedded();
    if let Some(v) = request["difficulty"].as_str() {
        if rules.difficulties.contains_key(v) {
            p.difficulty = v.to_string();
        }
    }
    if let Some(v) = request["speed"].as_str() {
        if let Some(spec) = rules.speeds.get(v) {
            p.speed = v.to_string();
            p.game_speed = GameSpeed::from_id(v).unwrap_or(GameSpeed::Standard);
            // A speed carries its own turn budget; adopt it unless the client
            // asked for a specific one in the same request.
            p.max_turns = request["max_turns"].as_u64().unwrap_or(spec.turns as u64) as u32;
        }
    }
    p
}

fn auto_step_loop(sh: Arc<Shared>) {
    let mut over_since: Option<Instant> = None;
    loop {
        let pace = sh.pace_ms.load(Ordering::Relaxed).clamp(20, 60_000);
        if sh.paused.load(Ordering::Relaxed) {
            over_since = None; // pausing resets the restart countdown
            std::thread::sleep(Duration::from_millis(150));
            continue;
        }
        let cadence_started = Instant::now();
        let mut delay = pace;
        {
            let mut s = sh.session.lock().unwrap();
            if !s.params.spectate {
                drop(s);
                std::thread::sleep(Duration::from_millis(300));
                continue;
            }
            if s.game.winner.is_some() {
                let t0 = *over_since.get_or_insert_with(Instant::now);
                let left = RESTART_MS.saturating_sub(t0.elapsed().as_millis() as u64);
                sh.restart_in.store(left, Ordering::Relaxed);
                if left == 0 {
                    let mut p = s.params.clone();
                    p.seed = p
                        .seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    *s = Session::new(p);
                    over_since = None;
                    sh.restart_in.store(u64::MAX, Ordering::Relaxed);
                }
                delay = 200;
            } else {
                over_since = None;
                sh.restart_in.store(u64::MAX, Ordering::Relaxed);
                let (pid, _) = s.step();
                let p = &s.game.players[pid];
                if p.is_minor || p.is_barbarian {
                    delay = (pace / 4).max(30); // quick beat for minors
                }
            }
        }
        // Pace is a start-to-start cadence. Sleeping the full interval after
        // AI computation made late-game "Lightning · 0.1s" visibly slower
        // as empires grew. Spend only the remaining frame budget instead.
        let elapsed_ms = cadence_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        std::thread::sleep(Duration::from_millis(
            delay.saturating_sub(elapsed_ms).max(1),
        ));
    }
}

/// Attach exhibition metadata (restart countdown, pace, paused) to a state.
fn decorate(o: &mut Value, sh: &Shared) {
    let r = sh.restart_in.load(Ordering::Relaxed);
    if r != u64::MAX {
        o["restart_in"] = json!(r.div_ceil(1000));
    }
    o["pace"] = json!(sh.pace_ms.load(Ordering::Relaxed));
    o["paused"] = json!(sh.paused.load(Ordering::Relaxed));
}

fn handle(stream: &mut TcpStream, sh: &Shared) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    // Route on the URL path, not its cache-busting/query component. The
    // supervised spectator tags each successor URL with its server instance
    // so a long-lived tab loads fresh embedded assets after a binary swap.
    let request_target = parts.next().unwrap_or("/");
    let path = request_path(request_target).to_string();
    let mut content_len = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() || h == "\r\n" || h == "\n" || h.is_empty() {
            break;
        }
        let hl = h.to_ascii_lowercase();
        if let Some(v) = hl.strip_prefix("content-length:") {
            content_len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        let _ = reader.read_exact(&mut body);
    }
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

    match (method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => {
            respond(stream, "200 OK", "text/html; charset=utf-8", &index_html());
        }
        ("GET", "/assets/terrain-atlas.png") => {
            respond(stream, "200 OK", "image/png", &terrain_atlas());
        }
        ("GET", "/assets/feature-atlas.png") => {
            respond(stream, "200 OK", "image/png", &feature_atlas());
        }
        ("GET", "/assets/environment-feature-atlas.png") => {
            respond(stream, "200 OK", "image/png", &environment_feature_atlas());
        }
        ("GET", "/assets/natural-wonder-atlas.png") => {
            respond(stream, "200 OK", "image/png", &natural_wonder_atlas());
        }
        ("GET", "/assets/mountain-atlas.png") => {
            respond(stream, "200 OK", "image/png", &mountain_atlas());
        }
        ("GET", "/state") => {
            let mut o = sh.session.lock().unwrap().state();
            decorate(&mut o, sh);
            respond_json(stream, &o);
        }
        ("POST", "/pace") => {
            if let Some(v) = parsed["ms"].as_u64() {
                sh.pace_ms.store(v.clamp(20, 60_000), Ordering::Relaxed);
            }
            if let Some(v) = parsed["paused"].as_bool() {
                sh.paused.store(v, Ordering::Relaxed);
            }
            let mut session = sh.session.lock().unwrap();
            if let Some(v) = parsed["paused"].as_bool() {
                session.spectator_paused = v;
            }
            let mut o = session.state();
            drop(session);
            decorate(&mut o, sh);
            respond_json(stream, &o);
        }
        ("GET", "/save") => {
            let session = sh.session.lock().unwrap();
            let save = serde_json::to_value(&session.game).unwrap();
            respond_json(stream, &save);
        }
        ("GET", "/rules") => {
            let session = sh.session.lock().unwrap();
            let r = &session.game.rules;
            respond_json(
                stream,
                &json!({
                    "techs": r.techs, "civics": r.civics,
                    "terrains": r.terrains, "features": r.features,
                    "resources": r.resources, "improvements": r.improvements,
                    "governments": r.governments, "units": r.units,
                    "promotions": r.promotions,
                    "buildings": r.buildings, "districts": r.districts,
                    "wonders": r.wonders,
                    "projects": r.projects,
                    "policies": r.policies, "beliefs": r.beliefs, "civs": r.civs,
                    "great_people": r.great_people, "governors": r.governors,
                    "map_sizes": CIV6_MAP_SIZES,
                    "difficulties": r.difficulties, "speeds": r.speeds,
                    "map_scripts": CIV6_MAP_SCRIPTS,
                    "game_speeds": CIV6_GAME_SPEEDS,
                }),
            );
        }
        ("GET", "/pedia") => {
            // Generated from the ruleset in play, mods included, so the GUI
            // reference never disagrees with the game it is attached to.
            let session = sh.session.lock().unwrap();
            let entries = crate::pedia::entries(&session.game.rules);
            drop(session);
            respond_json(stream, &json!({ "entries": entries }));
        }
        ("POST", "/action") => {
            let mut session = sh.session.lock().unwrap();
            let movement_path = serde_json::from_value::<Action>(parsed["action"].clone())
                .ok()
                .and_then(|action| match action {
                    Action::MoveTo { unit, to } => {
                        let start = session.game.units.get(&unit)?.pos;
                        let mut path = session.game.path_to(unit, to)?;
                        path.insert(0, start);
                        Some((unit, path))
                    }
                    _ => None,
                });
            let err = session.act(&parsed["action"]);
            let mut out = session.state();
            if err.is_none() {
                if let Some((unit, mut path)) = movement_path {
                    if let Some(actual) = session.game.units.get(&unit).map(|unit| unit.pos) {
                        if let Some(end) = path.iter().position(|position| *position == actual) {
                            path.truncate(end + 1);
                        } else if let Some(start) = path.first().copied() {
                            path = vec![start, actual];
                        }
                    }
                    if path.len() > 1 {
                        out["movement_paths"] = json!({unit.to_string(): path});
                    }
                }
            }
            out["error"] = match err {
                Some(e) => Value::String(e),
                None => Value::Null,
            };
            respond_json(stream, &out);
        }
        ("POST", "/step") => {
            let mut session = sh.session.lock().unwrap();
            let mut out;
            if session.params.spectate {
                let count = parsed["count"].as_u64().unwrap_or(1) as usize;
                let steps = session.step_many(count);
                out = session.state();
                // An omniscient observer can narrate every AI decision. A
                // civilization view only receives that civilization's own
                // traces; otherwise hidden movement and combat would bypass
                // the map fog through the event chronicle.
                let visible_steps: Vec<_> = steps
                    .iter()
                    .filter(|step| {
                        session
                            .view_player
                            .is_none_or(|viewer| step.player == viewer)
                    })
                    .collect();
                if let Some(step) = visible_steps.last() {
                    // Preserve the original single-step response fields for
                    // existing clients and supervisor recovery nudges.
                    out["stepped"] = json!(step.player);
                    out["actions_taken"] = serde_json::to_value(&step.actions).unwrap();
                }
                out["step_batches"] = Value::Array(
                    visible_steps
                        .iter()
                        .map(|step| {
                            json!({
                                "stepped": step.player,
                                "actions_taken": step.actions,
                                "world_events": if session.view_player.is_none() {
                                    step.world_events.clone()
                                } else {
                                    Vec::new()
                                },
                            })
                        })
                        .collect(),
                );
            } else {
                out = session.state();
                out["error"] = json!("not in spectate mode");
            }
            drop(session);
            decorate(&mut out, sh);
            respond_json(stream, &out);
        }
        ("POST", "/view") => {
            let mut session = sh.session.lock().unwrap();
            let result = match parsed.get("player") {
                Some(Value::Null) => session.set_view_player(None),
                Some(value) => value
                    .as_u64()
                    .ok_or_else(|| "player must be a non-negative integer or null".to_string())
                    .and_then(|pid| session.set_view_player(Some(pid as usize))),
                None => Err("missing player".to_string()),
            };
            let mut out = session.state();
            out["error"] = match result {
                Ok(()) => Value::Null,
                Err(error) => Value::String(error),
            };
            respond_json(stream, &out);
        }
        ("POST", "/spectator-status") => {
            let mut session = sh.session.lock().unwrap();
            if session.params.spectate {
                if let Some(paused) = parsed["paused"].as_bool() {
                    session.spectator_paused = paused;
                }
                respond_json(stream, &json!({"ok": true}));
            } else {
                respond_json(stream, &json!({"error": "not in spectate mode"}));
            }
        }
        ("POST", "/new") => {
            let mut session = sh.session.lock().unwrap();
            let result = session.start_new_game(&parsed);
            let mut o = session.state();
            o["error"] = match result {
                Ok(()) => Value::Null,
                Err(error) => Value::String(error),
            };
            drop(session);
            decorate(&mut o, sh);
            respond_json(stream, &o);
        }
        ("POST", "/supervisor-new") => {
            let result = session.request_supervised_new_game(&parsed);
            let mut out = session.state();
            out["error"] = match result {
                Ok(()) => Value::Null,
                Err(error) => Value::String(error),
            };
            respond_json(stream, &out);
        }
        _ => respond(
            stream,
            "404 Not Found",
            "application/json",
            b"{\"error\":\"not found\"}",
        ),
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        chronicle_world_events, new_game_params, request_path, ChronicleSnapshot, ChronicleState,
        Params, Session, EMBEDDED_INDEX,
    };
    use crate::game::{Action, VictoryConditions};
    use crate::setup::{GameSpeed, MapScript};
    use serde_json::json;

    fn current() -> Params {
        Params {
            num_players: 2,
            width: 20,
            height: 14,
            seed: 1,
            map_script: MapScript::Pangaea,
            game_speed: GameSpeed::Standard,
            max_turns: 500,
            victory_conditions: VictoryConditions::default(),
            num_city_states: 1,
            spectate: false,
            difficulty: crate::game::default_difficulty(),
            speed: crate::game::default_speed(),
            supervised: false,
        }
    }

    #[test]
    fn new_game_player_count_applies_the_whole_civ6_size_profile() {
        let expected = [
            (2, 44, 26, 3),
            (4, 60, 38, 6),
            (6, 74, 46, 9),
            (8, 84, 54, 12),
            (10, 96, 60, 15),
            (12, 106, 66, 18),
        ];
        let mut params = current();
        for (players, width, height, city_states) in expected {
            params = new_game_params(&params, &json!({"num_players": players}));
            assert_eq!(params.num_players, players);
            assert_eq!(
                (params.width, params.height, params.num_city_states),
                (width, height, city_states)
            );
        }
    }

    #[test]
    fn explicit_advanced_overrides_win_over_the_profile() {
        let p = new_game_params(
            &current(),
            &json!({
                "num_players": 6,
                "width": 80,
                "height": 50,
                "num_city_states": 2
            }),
        );
        assert_eq!((p.width, p.height, p.num_city_states), (80, 50, 2));
    }

    #[test]
    fn map_and_speed_choices_update_the_complete_setup() {
        let p = new_game_params(
            &current(),
            &json!({"map_script": "inland_sea", "game_speed": "online"}),
        );
        assert_eq!(p.map_script, MapScript::InlandSea);
        assert_eq!(p.game_speed, GameSpeed::Online);
        assert_eq!(p.max_turns, 250);

        let custom = new_game_params(
            &current(),
            &json!({"game_speed": "marathon", "max_turns": 99}),
        );
        assert_eq!(custom.game_speed, GameSpeed::Marathon);
        assert_eq!(custom.max_turns, 99);
    }

    #[test]
    fn new_game_applies_each_victory_condition_setting() {
        let disabled = json!({
            "science": false,
            "culture": false,
            "religious": false,
            "diplomatic": false,
            "domination": false,
            "score": false
        });
        let params = new_game_params(&current(), &json!({"victory_conditions": disabled.clone()}));
        assert_eq!(
            params.victory_conditions,
            VictoryConditions {
                science: false,
                culture: false,
                religious: false,
                diplomatic: false,
                domination: false,
                score: false,
            }
        );

        let session = Session::new(params.clone());
        assert_eq!(session.game.victory_conditions, params.victory_conditions);
        assert_eq!(session.state()["victory_conditions"], disabled);
    }

    #[test]
    fn omitted_victory_settings_preserve_the_current_selection() {
        let mut current = current();
        current.victory_conditions.culture = false;
        current.victory_conditions.score = false;
        let next = new_game_params(&current, &json!({"seed": 2}));
        assert!(!next.victory_conditions.culture);
        assert!(!next.victory_conditions.score);
        assert!(next.victory_conditions.science);
    }

    #[test]
    fn browser_orders_settings_event_log_and_strategy() {
        for players in [2, 4, 6, 8, 10, 12] {
            assert!(
                EMBEDDED_INDEX.contains(&format!("<option value=\"{players}\"")),
                "browser setup is missing the {players}-player map size"
            );
        }
        assert!(EMBEDDED_INDEX.contains("RULES.map_sizes.map(size =>"));
        assert!(EMBEDDED_INDEX.contains("RULES.map_scripts.map(script =>"));
        assert!(EMBEDDED_INDEX.contains("RULES.game_speeds.map(speed =>"));
        assert!(EMBEDDED_INDEX.contains("id=\"gamemode\""));
        assert!(EMBEDDED_INDEX.contains("id=\"maptype\""));
        assert!(EMBEDDED_INDEX.contains("id=\"gamespeed\""));
        for victory in [
            "science",
            "culture",
            "religious",
            "diplomatic",
            "domination",
            "score",
        ] {
            assert!(
                EMBEDDED_INDEX.contains(&format!("id=\"victory-{victory}\"")),
                "browser setup is missing the {victory} victory checkbox"
            );
        }
        assert!(EMBEDDED_INDEX.contains("victory_conditions: victoryConditions"));
        assert!(EMBEDDED_INDEX.contains("AI-only simulation"));
        assert!(EMBEDDED_INDEX.contains("Single player · later"));
        assert!(EMBEDDED_INDEX.contains("Multiplayer · later"));
        assert!(EMBEDDED_INDEX.contains("class=\"sim-actions\""));
        assert!(EMBEDDED_INDEX.contains("id=\"restart-sim\">Restart sim"));
        assert!(EMBEDDED_INDEX.contains("id=\"fresh-sim\""));
        assert!(EMBEDDED_INDEX.contains("New sim · fresh code"));
        assert!(EMBEDDED_INDEX.contains("async function startNewSimulation(mode)"));
        assert!(EMBEDDED_INDEX.contains("startNewSimulation(\"restart\")"));
        assert!(EMBEDDED_INDEX.contains("startNewSimulation(\"fresh_code\")"));
        assert!(EMBEDDED_INDEX.contains("fetchJSON(\"/supervisor-new\""));
        assert!(!EMBEDDED_INDEX.contains("id=\"head-newgame\""));
        assert!(EMBEDDED_INDEX.contains("spectate: gameMode === \"ai_sim\""));
        assert!(!EMBEDDED_INDEX.contains("id=\"specchk\""));
        assert!(!EMBEDDED_INDEX.contains("RULES.map_sizes.filter"));

        let mode_setting = EMBEDDED_INDEX
            .find("id=\"gamemode\"")
            .expect("game mode setting");
        let world_setting = EMBEDDED_INDEX
            .find("id=\"np\"")
            .expect("world size setting");
        let map_setting = EMBEDDED_INDEX.find("id=\"maptype\"").expect("map setting");
        let speed_setting = EMBEDDED_INDEX
            .find("id=\"gamespeed\"")
            .expect("game speed setting");
        assert!(
            mode_setting < world_setting
                && world_setting < map_setting
                && map_setting < speed_setting
        );

        let game_settings = EMBEDDED_INDEX
            .find("id=\"game-settings\"")
            .expect("game settings panel");
        let display_settings = EMBEDDED_INDEX
            .find("id=\"display-settings\"")
            .expect("display settings panel");
        let event_log = EMBEDDED_INDEX
            .find("<span>Game event log</span>")
            .expect("game event log");
        let strategy = EMBEDDED_INDEX
            .find("<span>Active strategy</span>")
            .expect("active strategy section");
        assert!(
            game_settings < display_settings
                && display_settings < event_log
                && event_log < strategy,
            "left panel should show game settings, display settings, and the event log first"
        );
        assert!(EMBEDDED_INDEX.contains("<span>Display settings</span>"));
        assert!(!EMBEDDED_INDEX.contains("Simulator settings"));
        assert!(EMBEDDED_INDEX.contains("Quick Deals"));
        assert!(EMBEDDED_INDEX.contains("function drawQuickDeals()"));
        assert!(EMBEDDED_INDEX.contains("type:\"trade\""));
        assert!(EMBEDDED_INDEX.contains("function spectatorIdentity(player)"));
        assert!(EMBEDDED_INDEX.contains("state.players[state.player] || actor"));
        assert!(EMBEDDED_INDEX.contains("Global lifetime carbon emissions"));
        assert!(EMBEDDED_INDEX.contains("Alliance · Level"));
        assert!(EMBEDDED_INDEX.contains("p.ai_strategy"));
        assert!(EMBEDDED_INDEX.contains("changed its grand strategy from"));
        assert!(EMBEDDED_INDEX.contains("e.important && now - e.at < 6000"));
        assert!(EMBEDDED_INDEX.contains("const cadence = active ? (SPEC ? 32 : 16) : 90"));
        assert!(EMBEDDED_INDEX.contains(".diplomacy-card.allied"));
        assert!(EMBEDDED_INDEX.contains("function cameraYBounds"));
        assert!(EMBEDDED_INDEX.contains("cam.y = clampCameraY(cam.y)"));
        assert!(EMBEDDED_INDEX.contains("View as"));
        assert!(EMBEDDED_INDEX.contains("id=\"viewplayer\""));
        assert!(EMBEDDED_INDEX.contains("fetchJSON(\"/view\""));
        assert!(EMBEDDED_INDEX.contains("onclick=\"spectatePlayer(${p.id})\""));
        assert!(EMBEDDED_INDEX.contains("async function spectatePlayer(player)"));
        assert!(EMBEDDED_INDEX.contains("player log"));
        assert!(EMBEDDED_INDEX.contains("Spectator · combined summary"));
        assert!(EMBEDDED_INDEX.contains("let eventLogs = new Map()"));
        assert!(EMBEDDED_INDEX.contains("function chronicleWorldEvents(next)"));
        assert!(EMBEDDED_INDEX.contains("built the world's first"));
        assert!(EMBEDDED_INDEX.contains("changed government from"));
        assert!(!EMBEDDED_INDEX.contains("completed its turn"));
        assert!(!EMBEDDED_INDEX
            .contains("civilization${summaries.length === 1 ? \"\" : \"s\"} completed"));
        assert!(EMBEDDED_INDEX.contains("id=\"strategysec\""));
        assert!(EMBEDDED_INDEX
            .contains("document.getElementById(\"strategysec\").style.display = fullMapSpectator"));
        assert!(EMBEDDED_INDEX.contains("if (!fullMapSpectator && (SPEC || govs.length"));
        assert!(EMBEDDED_INDEX.contains(".sort((a, b) => b.score - a.score || a.id - b.id)"));
        assert!(EMBEDDED_INDEX.contains("class=\"diplomacy-rank\">#${rank}"));
        assert!(EMBEDDED_INDEX.contains("#side {\n    order: -1;"));
        assert!(EMBEDDED_INDEX.contains("<strong>${state.turn}</strong>"));
        assert!(!EMBEDDED_INDEX.contains("${state.turn}/${maxTurns}"));
    }

    #[test]
    fn instance_tagged_spectator_url_routes_to_the_embedded_page() {
        assert_eq!(request_path("/"), "/");
        assert_eq!(request_path("/?instance=9232"), "/");
        assert_eq!(request_path("/state?instance=9232"), "/state");
    }

    #[test]
    fn next_spectator_game_preserves_settings_and_watched_player() {
        let mut params = current();
        params.spectate = true;
        let mut session = Session::new(params);
        session.set_view_player(Some(1)).unwrap();
        let previous_settings = (
            session.params.num_players,
            session.params.width,
            session.params.height,
            session.params.num_city_states,
            session.params.map_script,
            session.params.game_speed,
            session.params.spectate,
        );

        session
            .start_new_game(&json!({"seed": 2, "force": true}))
            .unwrap();

        assert_eq!(session.params.seed, 2);
        assert_eq!(
            (
                session.params.num_players,
                session.params.width,
                session.params.height,
                session.params.num_city_states,
                session.params.map_script,
                session.params.game_speed,
                session.params.spectate,
            ),
            previous_settings
        );
        assert_eq!(session.state()["view_player"].as_u64(), Some(1));
    }

    #[test]
    fn supervised_new_game_request_normalizes_settings_without_replacing_the_live_game() {
        let mut params = current();
        params.spectate = true;
        params.supervised = true;
        let mut session = Session::new(params);
        let original_seed = session.game.seed;

        session
            .request_supervised_new_game(&json!({
                "mode": "fresh_code",
                "num_players": 4,
                "map_script": "continents",
                "game_speed": "quick",
                "victory_conditions": {"culture": false, "score": false},
            }))
            .unwrap();

        let state = session.state();
        assert_eq!(session.game.seed, original_seed);
        assert!(session.spectator_paused);
        assert_eq!(state["supervisor_request"]["mode"], "fresh_code");
        assert_eq!(
            state["supervisor_request"]["server_instance"].as_u64(),
            Some(std::process::id() as u64)
        );
        assert_eq!(
            state["supervisor_request"]["settings"],
            json!({
                "players": 4,
                "width": 60,
                "height": 38,
                "city_states": 6,
                "turns": 330,
                "map": "continents",
                "speed": "quick",
                "victories": ["science", "religious", "diplomatic", "domination"],
            })
        );
    }

    #[test]
    fn unsupervised_server_rejects_supervisor_new_game_requests() {
        let mut session = Session::new(current());
        assert!(session
            .request_supervised_new_game(&json!({"mode": "fresh_code"}))
            .is_err());
        assert!(session.state()["supervisor_request"].is_null());
    }

    #[test]
    fn next_game_drops_a_watched_player_that_is_not_in_the_new_world() {
        let mut params = current();
        params.num_players = 4;
        params.width = 30;
        params.height = 20;
        params.spectate = true;
        let mut session = Session::new(params);
        session.set_view_player(Some(3)).unwrap();

        session
            .start_new_game(&json!({"num_players": 2, "seed": 2, "force": true}))
            .unwrap();

        assert!(session.state()["view_player"].is_null());
    }

    #[test]
    fn state_identifies_the_running_server_instance() {
        let state = Session::new(current()).state();
        assert_eq!(
            state["server_instance"].as_u64(),
            Some(std::process::id() as u64)
        );
        assert!(state["quick_deals"].is_array());
        assert!(state["active_trade_deals"].is_array());
        assert!(state["me"]["resources"].is_array());
    }

    #[test]
    fn spectator_state_reports_the_pause_liveness_signal() {
        let mut params = current();
        params.spectate = true;
        let mut session = Session::new(params);
        let state = session.state();
        assert_eq!(state["spectator_paused"].as_bool(), Some(false));
        assert!(state["view_player"].is_null());
        assert_eq!(
            state["visible"].as_array().unwrap().len(),
            state["map"]["tiles"].as_array().unwrap().len()
        );
        assert!(state["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|unit| unit.get("reachable").is_none()));
        assert!(state["players"][0]["ai_strategy"].is_null());
        session.step();
        assert_eq!(session.state()["players"][0]["ai_strategy"], "expansion");
    }

    #[test]
    fn spectator_can_view_any_major_through_that_players_fog() {
        let mut params = current();
        params.spectate = true;
        let mut session = Session::new(params);
        let omniscient = session.state();

        session.set_view_player(Some(1)).unwrap();
        let player_view = session.state();
        assert_eq!(player_view["player"].as_u64(), Some(1));
        assert_eq!(player_view["view_player"].as_u64(), Some(1));
        assert!(
            player_view["visible"].as_array().unwrap().len()
                < omniscient["visible"].as_array().unwrap().len()
        );
        assert!(
            player_view["map"]["tiles"].as_array().unwrap().len()
                < omniscient["map"]["tiles"].as_array().unwrap().len()
        );
        assert!(player_view["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|unit| unit.get("reachable").is_none()));

        session.set_view_player(None).unwrap();
        assert!(session.state()["view_player"].is_null());
    }

    #[test]
    fn selecting_any_ranked_player_promotes_the_live_match_to_spectator_mode() {
        for pid in 0..current().num_players {
            let mut session = Session::new(current());
            assert!(!session.params.spectate);
            let omniscient_tile_count = session.game.map.tiles.len();

            session.set_view_player(Some(pid)).unwrap();
            let player_view = session.state();

            assert!(session.params.spectate);
            assert_eq!(player_view["spectate"].as_bool(), Some(true));
            assert_eq!(player_view["player"].as_u64(), Some(pid as u64));
            assert_eq!(player_view["view_player"].as_u64(), Some(pid as u64));
            assert!(player_view["map"]["tiles"].as_array().unwrap().len() < omniscient_tile_count);
        }
    }

    #[test]
    fn spectator_view_rejects_non_major_and_unknown_players() {
        let mut params = current();
        params.spectate = true;
        let mut session = Session::new(params);
        let minor = session
            .game
            .players
            .iter()
            .find(|player| player.is_minor || player.is_barbarian)
            .unwrap()
            .id;

        assert!(session.set_view_player(Some(minor)).is_err());
        assert!(session.set_view_player(Some(usize::MAX)).is_err());
        assert!(session.state()["view_player"].is_null());
    }

    #[test]
    fn result_countdown_cannot_replace_an_active_successor() {
        let mut params = current();
        params.spectate = true;
        let mut session = Session::new(params);
        let original_seed = session.game.seed;
        let guarded = json!({
            "seed": 2,
            "spectate": true,
            "replace_finished": {
                "seed": original_seed,
                "server_instance": std::process::id()
            }
        });

        assert!(session.start_new_game(&guarded).is_err());
        assert_eq!(session.game.seed, original_seed);
        assert!(session
            .start_new_game(&json!({"seed": 4, "spectate": true}))
            .is_err());
        assert_eq!(session.game.seed, original_seed);

        assert!(session
            .start_new_game(&json!({"seed": 5, "spectate": true, "force": true}))
            .is_ok());
        assert_eq!(session.game.seed, 5);

        session.game.winner = Some(0);
        let guarded = json!({
            "seed": 2,
            "spectate": true,
            "replace_finished": {
                "seed": 5,
                "server_instance": std::process::id()
            }
        });
        session.params.supervised = true;
        assert!(session.start_new_game(&guarded).is_err());
        assert_eq!(session.game.seed, 5);
        assert!(session
            .start_new_game(&json!({"seed": 6, "spectate": true, "force": true}))
            .is_err());
        assert_eq!(session.game.seed, 5);
        session.params.supervised = false;
        assert!(session.start_new_game(&guarded).is_ok());
        assert_eq!(session.game.seed, 2);

        session.game.winner = Some(0);
        let stale = json!({
            "seed": 3,
            "spectate": true,
            "replace_finished": {
                "seed": 2,
                "server_instance": u64::from(std::process::id()) + 1
            }
        });
        assert!(session.start_new_game(&stale).is_err());
        assert_eq!(session.game.seed, 2);
    }

    #[test]
    fn spectator_state_uses_a_major_viewpoint_during_barbarian_turns() {
        let mut params = current();
        params.spectate = true;
        let mut session = Session::new(params);
        let barbarian = session
            .game
            .players
            .iter()
            .find(|player| player.is_barbarian)
            .unwrap()
            .id;
        session.game.current = barbarian;

        let state = session.state();
        let viewer = state["player"].as_u64().unwrap() as usize;
        assert!(!session.game.players[viewer].is_minor);
        assert!(!session.game.players[viewer].is_barbarian);
        assert!(session.game.players[viewer].alive);
    }

    #[test]
    fn spectator_chronicle_reports_world_milestones_once() {
        let mut params = current();
        params.spectate = true;
        let mut session = Session::new(params);
        let game = &mut session.game;
        let first_pos = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .map(|unit| game.units[&unit].pos)
            .unwrap();
        let second_pos = game
            .player_unit_ids(1)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .map(|unit| game.units[&unit].pos)
            .unwrap();
        let first_city = game.found_city_for(0, first_pos, Some("Alpha".to_string()));
        let captured_city = game.found_city_for(1, second_pos, Some("Beta".to_string()));
        let before = ChronicleSnapshot::capture(game);
        let mut chronicle = ChronicleState::from_game(game);

        let district_pos = game.cities[&first_city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != first_pos)
            .unwrap();
        game.cities
            .get_mut(&first_city)
            .unwrap()
            .districts
            .insert("campus".to_string(), district_pos);
        game.cities
            .get_mut(&first_city)
            .unwrap()
            .wonders
            .insert("pyramids".to_string(), district_pos);
        game.cities
            .get_mut(&first_city)
            .unwrap()
            .buildings
            .push("granary".to_string());
        game.cities.get_mut(&first_city).unwrap().pop = 4;
        game.players[0].religion = Some("Test Faith".to_string());
        game.players[0].government = Some("classical_republic".to_string());
        game.players[0].techs.insert("horseback_riding".to_string());
        game.players[0].civics.insert("drama_poetry".to_string());
        let city_state = game
            .players
            .iter()
            .find(|player| player.is_minor && !player.is_barbarian)
            .map(|player| player.id)
            .unwrap();
        game.players[0].envoys.push((city_state, 3));
        {
            let city = game.cities.get_mut(&captured_city).unwrap();
            city.owner = 0;
            city.occupied_from = Some(1);
        }

        let after = ChronicleSnapshot::capture(game);
        let events = chronicle_world_events(
            &before,
            &after,
            0,
            &[Action::KeepCity {
                city: captured_city,
            }],
            &mut chronicle,
        );
        let event_types: Vec<_> = events
            .iter()
            .filter_map(|event| event["type"].as_str())
            .collect();
        for expected in [
            "wonder_built",
            "religion_founded",
            "district_first",
            "building_first",
            "population_milestone",
            "city_captured",
            "suzerain_changed",
            "government_changed",
        ] {
            assert!(
                event_types.contains(&expected),
                "missing {expected}: {events:?}"
            );
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "era_first")
                .count(),
            2,
            "technology and civics should each announce their Classical leader"
        );

        let later = ChronicleSnapshot::capture(game);
        let repeat = chronicle_world_events(&after, &later, 0, &[], &mut chronicle);
        assert!(
            repeat.is_empty(),
            "unchanged milestones repeated: {repeat:?}"
        );
    }

    #[test]
    fn spectator_chronicle_tracks_war_declarations_losses_and_peace() {
        let mut game = Session::new(current()).game;
        let defeated = game
            .units
            .values()
            .find(|unit| {
                unit.owner == 1 && game.rules.units[unit.kind.as_str()].class == "military"
            })
            .map(|unit| unit.id)
            .expect("player two starts with a military unit");
        let before = ChronicleSnapshot::capture(&game);
        let mut chronicle = ChronicleState::from_game(&game);

        game.at_war.insert((0, 1));
        game.remove_unit(defeated);
        let after_battle = ChronicleSnapshot::capture(&game);
        let events = chronicle_world_events(
            &before,
            &after_battle,
            0,
            &[Action::DeclareWar { player: 1 }],
            &mut chronicle,
        );
        assert!(events.iter().any(|event| event["type"] == "war_started"));
        let progress = events
            .iter()
            .find(|event| event["type"] == "war_progress")
            .expect("a destroyed military unit advances the war chronicle");
        assert_eq!(progress["defender_units_lost"], 1);
        assert_eq!(progress["aggressor_units_lost"], 0);

        game.at_war.remove(&(0, 1));
        let after_peace = ChronicleSnapshot::capture(&game);
        let peace = chronicle_world_events(
            &after_battle,
            &after_peace,
            0,
            &[Action::MakePeace { player: 1 }],
            &mut chronicle,
        );
        let ended = peace
            .iter()
            .find(|event| event["type"] == "war_ended")
            .expect("peace concludes the running war chronicle");
        assert_eq!(ended["defender_units_lost"], 1);
    }

    #[test]
    fn restored_session_preserves_progress_and_derives_its_world_settings() {
        let mut game = Session::new(current()).game;
        game.turn = 37;
        game.current = 1;
        let mut wrong = current();
        wrong.num_players = 12;
        wrong.width = 106;
        wrong.height = 66;
        wrong.num_city_states = 18;

        let restored = Session::from_game(wrong, game);
        assert_eq!((restored.game.turn, restored.game.current), (37, 1));
        assert_eq!(restored.params.num_players, 2);
        assert_eq!((restored.params.width, restored.params.height), (20, 14));
        assert_eq!(restored.params.num_city_states, 1);
    }
}

pub fn serve_with_game(port: u16, open_browser: bool, params: Params, game: Option<Game>) {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .unwrap_or_else(|e| panic!("cannot bind port {port}: {e}"));
    let actual = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{actual}/");
    let session = match game {
        Some(game) => Session::from_game(params, game),
        None => Session::new(params),
    };
    println!("Martin Halvorson's Civilization VIS — playing at {url}");
    if session.params.spectate {
        println!(
            "Spectator mode: all {} players are AI-driven. Ctrl+C to quit.",
            session.params.num_players
        );
    } else {
        println!("You are player 0. Ctrl+C to quit.");
    }
    let shared = Arc::new(Shared {
        session: Mutex::new(session),
        pace_ms: AtomicU64::new(100), // lightning by default
        paused: AtomicBool::new(false),
        restart_in: AtomicU64::new(u64::MAX),
    });
    let stepper = shared.clone();
    std::thread::spawn(move || auto_step_loop(stepper));
    if open_browser {
        open_url(&url);
    }
    for stream in listener.incoming() {
        if let Ok(mut s) = stream {
            handle(&mut s, &shared);
        }
    }
}

pub fn serve(port: u16, open_browser: bool, params: Params) {
    serve_with_game(port, open_browser, params, None);
}

fn open_url(url: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}
