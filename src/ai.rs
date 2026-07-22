//! Scripted AIs (mirrors civvis/ai/). BasicAi reads full state (no fog) —
//! sparring partner, not a fair-play agent.
use crate::game::{effective_strength, Action, Game, Item};
use crate::rng::Rng;
use crate::Pos;
use std::collections::{HashMap, HashSet};

/// A bounded first-step initiative bonus breaks positional and formation ties
/// in favor of doing something useful with the turn. Four points can overcome
/// a couple of lost adjacency bonuses, but not the much larger penalty for
/// stepping into a dangerous attack envelope.
const FIRST_MOVE_SCORE_BONUS: f64 = 4.0;

mod advanced;
pub use advanced::{
    AdvancedAi, ForceDomain, ForceGroup, ForcePosture, GrandStrategy, StrategicPlan, VictoryTarget,
};

const TECH_PRIORITY: [&str; 15] = [
    "pottery",
    "animal_husbandry",
    "mining",
    "writing",
    "archery",
    "bronze_working",
    "currency",
    "masonry",
    "irrigation",
    "iron_working",
    "mathematics",
    "construction",
    "engineering",
    "education",
    "machinery",
];
const CIVIC_PRIORITY: [&str; 8] = [
    "code_of_laws",
    "craftsmanship",
    "foreign_trade",
    "early_empire",
    "state_workforce",
    "military_tradition",
    "drama_poetry",
    "political_philosophy",
];
const DISTRICT_PRIORITY: [&str; 4] = ["campus", "commercial_hub", "holy_site", "theater_square"];

pub trait Ai {
    fn take_turn(&mut self, g: &mut Game, pid: usize);
}

impl<T: Ai + ?Sized> Ai for Box<T> {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        (**self).take_turn(g, pid);
    }
}

pub fn run_game<A: Ai>(g: &mut Game, ais: &mut [A]) {
    while g.winner.is_none() {
        let pid = g.current;
        ais[pid].take_turn(g, pid);
        if g.winner.is_none() && g.current == pid {
            let _ = g.apply(pid, &Action::EndTurn);
        }
    }
}

// ----------------------------------------------------------------- RandomAi

pub struct RandomAi {
    rng: Rng,
}

impl RandomAi {
    pub fn new(seed: u64) -> RandomAi {
        RandomAi {
            rng: Rng::new(seed),
        }
    }
}

impl Ai for RandomAi {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        for _ in 0..60 {
            let acts: Vec<Action> = g
                .legal_actions(pid)
                .into_iter()
                .filter(|a| !matches!(a, Action::EndTurn))
                .collect();
            if acts.is_empty() {
                break;
            }
            let a = acts[self.rng.below(acts.len())].clone();
            let _ = g.apply(pid, &a);
            if g.winner.is_some() {
                break;
            }
        }
        if g.winner.is_none() && g.current == pid {
            let _ = g.apply(pid, &Action::EndTurn);
        }
    }
}

// ------------------------------------------------------------------ BasicAi

const GOV_PRIORITY: [&str; 12] = [
    "digital_democracy",
    "synthetic_technocracy",
    "corporate_libertarianism",
    "democracy",
    "communism",
    "fascism",
    "merchant_republic",
    "monarchy",
    "classical_republic",
    "oligarchy",
    "autocracy",
    "chiefdom",
];
const POLICY_PRIORITY: [&str; 20] = [
    "urban_planning",
    "colonization",
    "ilkum",
    "feudal_contract",
    "agoge",
    "discipline",
    "god_king",
    "insulae",
    "meritocracy",
    "serfdom",
    "conscription",
    "bastions",
    "retainers",
    "town_charters",
    "craftsmen",
    "maritime_industries",
    "maneuver",
    "limes",
    "survey",
    "strategos",
];

/// Strategy weights steering BasicAi decisions. Defaults reproduce the
/// original hand-tuned behavior; the `evolve` GA searches this space.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Weights {
    pub city_target: f64,       // stop settling at this many cities+settlers
    pub settler_min_pop: f64,   // city pop needed before making a settler
    pub settler_stop_turn: f64, // no new settlers after this turn
    pub mil_per_city: f64,      // military units to keep per city
    pub builder_per_city: f64,  // builders to keep per city
    pub war_ratio: f64,         // declare war if my power > ratio*theirs+margin
    pub war_margin: f64,
    pub peace_ratio: f64, // sue for peace if my power < ratio*theirs
    pub war_min_turn: f64,
    pub attack_floor: f64,  // minimum exchange score to attack (SEE-style)
    pub kill_bonus: f64,    // exchange bonus for a killing blow
    pub trade_caution: f64, // weight on expected counter-damage
    pub settle_food: f64,   // settle-site yield weights
    pub settle_prod: f64,
    pub settle_gold: f64,
    pub settle_dist: f64, // per-hex penalty on distant settle sites
    pub min_city_dist: f64,
    pub wonder_min_bld: f64, // buildings before a city tries wonders
    pub faith_builder: f64,  // faith reserve before buying a builder
    pub d_campus: f64,       // district build priorities (higher first)
    pub d_commercial: f64,
    pub d_holy: f64,
    pub d_theater: f64,
    // opening book: first four capital builds, indexes into OPENING_MENU
    // (floor; >= menu length = no scripted pick, evaluate normally)
    pub open0: f64,
    pub open1: f64,
    pub open2: f64,
    pub open3: f64,
    // 1-ply tactical movement: candidate tiles scored by progress toward the
    // target plus these positional terms
    pub mv_support: f64, // bonus per adjacent friendly military unit
    pub mv_threat: f64,  // penalty per point of expected incoming damage
    // Hierarchical combat doctrine. AdvancedAi turns these genes into shared
    // army/fleet orders; keeping them in Weights lets self-play evolve economy,
    // grand strategy, and battlefield execution as one genome.
    pub command_radius: f64,     // maximum separation inside one force group
    pub muster_radius: f64,      // distance from group anchor considered ready
    pub muster_readiness: f64,   // fraction assembled before a planned advance
    pub cohesion: f64,           // movement reward for staying with the force
    pub focus_fire: f64,         // attack bonus for the group's shared target
    pub screen: f64,             // penalty for ranged/siege moving ahead of melee
    pub role_spacing: f64,       // reward for each role's preferred engagement depth
    pub objective_progress: f64, // movement reward toward the shared objective
    pub local_superiority: f64,  // caution when local hostile power is greater
    pub withdraw_hp: f64,        // enter persistent recovery at or below this HP
    pub rejoin_hp: f64,          // leave recovery at or above this HP
}

pub const OPENING_MENU: [&str; 6] = [
    "scout", "warrior", "builder", "settler", "slinger", "monument",
];

impl Default for Weights {
    fn default() -> Weights {
        Weights {
            city_target: 4.0,
            settler_min_pop: 2.0,
            settler_stop_turn: 150.0,
            mil_per_city: 1.0,
            builder_per_city: 0.5,
            war_ratio: 1.8,
            war_margin: 20.0,
            peace_ratio: 0.6,
            war_min_turn: 40.0,
            attack_floor: 0.0,
            kill_bonus: 25.0,
            trade_caution: 1.0,
            settle_food: 1.2,
            settle_prod: 1.0,
            settle_gold: 0.3,
            settle_dist: 0.4,
            min_city_dist: 4.0,
            wonder_min_bld: 3.0,
            faith_builder: 120.0,
            d_campus: 4.0,
            d_commercial: 3.0,
            d_holy: 2.0,
            d_theater: 1.0,
            open0: 1.0,
            open1: 3.0,
            open2: 2.0,
            open3: 5.0, // warrior settler builder monument
            mv_support: 2.0,
            mv_threat: 0.5,
            command_radius: 6.0,
            muster_radius: 3.0,
            muster_readiness: 0.67,
            cohesion: 3.0,
            focus_fire: 2.5,
            screen: 4.0,
            role_spacing: 2.0,
            objective_progress: 2.5,
            local_superiority: 6.0,
            withdraw_hp: 45.0,
            rejoin_hp: 80.0,
        }
    }
}

impl Weights {
    pub fn to_vec(&self) -> Vec<f64> {
        vec![
            self.city_target,
            self.settler_min_pop,
            self.settler_stop_turn,
            self.mil_per_city,
            self.builder_per_city,
            self.war_ratio,
            self.war_margin,
            self.peace_ratio,
            self.war_min_turn,
            self.attack_floor,
            self.kill_bonus,
            self.trade_caution,
            self.settle_food,
            self.settle_prod,
            self.settle_gold,
            self.settle_dist,
            self.min_city_dist,
            self.wonder_min_bld,
            self.faith_builder,
            self.d_campus,
            self.d_commercial,
            self.d_holy,
            self.d_theater,
            self.open0,
            self.open1,
            self.open2,
            self.open3,
            self.mv_support,
            self.mv_threat,
            self.command_radius,
            self.muster_radius,
            self.muster_readiness,
            self.cohesion,
            self.focus_fire,
            self.screen,
            self.role_spacing,
            self.objective_progress,
            self.local_superiority,
            self.withdraw_hp,
            self.rejoin_hp,
        ]
    }

    pub fn from_vec(v: &[f64]) -> Weights {
        Weights {
            city_target: v[0],
            settler_min_pop: v[1],
            settler_stop_turn: v[2],
            mil_per_city: v[3],
            builder_per_city: v[4],
            war_ratio: v[5],
            war_margin: v[6],
            peace_ratio: v[7],
            war_min_turn: v[8],
            attack_floor: v[9],
            kill_bonus: v[10],
            trade_caution: v[11],
            settle_food: v[12],
            settle_prod: v[13],
            settle_gold: v[14],
            settle_dist: v[15],
            min_city_dist: v[16],
            wonder_min_bld: v[17],
            faith_builder: v[18],
            d_campus: v[19],
            d_commercial: v[20],
            d_holy: v[21],
            d_theater: v[22],
            open0: v[23],
            open1: v[24],
            open2: v[25],
            open3: v[26],
            mv_support: v[27],
            mv_threat: v[28],
            command_radius: v[29],
            muster_radius: v[30],
            muster_readiness: v[31],
            cohesion: v[32],
            focus_fire: v[33],
            screen: v[34],
            role_spacing: v[35],
            objective_progress: v[36],
            local_superiority: v[37],
            withdraw_hp: v[38],
            rejoin_hp: v[39],
        }
    }

    /// (lo, hi) clamp per gene, same order as to_vec.
    pub fn bounds() -> [(f64, f64); 40] {
        [
            (2.0, 12.0),
            (1.0, 5.0),
            (60.0, 400.0),
            (0.3, 4.0),
            (0.2, 2.0),
            (0.8, 5.0),
            (-20.0, 80.0),
            (0.2, 1.2),
            (10.0, 200.0),
            (-25.0, 25.0),
            (0.0, 80.0),
            (0.2, 3.0),
            (0.2, 3.0),
            (0.2, 3.0),
            (0.0, 2.0),
            (0.0, 2.0),
            (3.0, 7.0),
            (0.0, 8.0),
            (40.0, 400.0),
            (0.0, 8.0),
            (0.0, 8.0),
            (0.0, 8.0),
            (0.0, 8.0),
            (0.0, 6.99),
            (0.0, 6.99),
            (0.0, 6.99),
            (0.0, 6.99),
            (0.0, 10.0),
            (0.0, 3.0),
            (2.0, 12.0),
            (1.0, 6.0),
            (0.25, 1.0),
            (0.0, 10.0),
            (0.0, 8.0),
            (0.0, 12.0),
            (0.0, 8.0),
            (0.5, 6.0),
            (0.0, 16.0),
            (20.0, 65.0),
            (60.0, 100.0),
        ]
    }
}

/// Strategic job inferred from a unit's class and promotion line. Both AI
/// tiers use the same doctrine so independent movement and force coordination
/// do not disagree about what a unit is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnitDoctrine {
    Recon,
    Assault,
    Mobile,
    Ranged,
    Siege,
    Support,
    AirDefense,
    AirStrike,
    Carrier,
}

pub struct BasicAi {
    minor: bool,
    barb: bool,
    pursue_religion: bool,
    w: Weights,
    book_pos: usize, // opening-book progress (capital builds played so far)
    /// Units that have withdrawn from combat stay in recovery until they are
    /// healthy enough to rejoin it, instead of advancing again after one tick.
    recovering_units: HashSet<u32>,
    /// Persistent peacetime destinations keep surplus troops patrolling the
    /// empire's frontier instead of permanently stacking around the capital.
    patrol_targets: HashMap<u32, Pos>,
    /// Colonies, especially overseas ones, need a fixed destination. Re-scoring
    /// only a short local radius each step strands settlers on shorelines and
    /// can make them reverse course after embarking.
    settler_targets: HashMap<u32, Pos>,
}

impl Default for BasicAi {
    fn default() -> Self {
        Self::new()
    }
}

impl BasicAi {
    pub(crate) fn unit_doctrine(g: &Game, uid: u32) -> UnitDoctrine {
        let spec = &g.rules.units[g.units[&uid].kind.as_str()];
        if spec.class == "support" {
            return UnitDoctrine::Support;
        }
        if spec.domain.as_deref() == Some("air") {
            return if spec.siege {
                UnitDoctrine::AirStrike
            } else {
                UnitDoctrine::AirDefense
            };
        }
        if spec.siege {
            return UnitDoctrine::Siege;
        }
        match spec.promotion_class.as_str() {
            "recon" => UnitDoctrine::Recon,
            "light_cavalry" | "naval_raider" | "naval_melee" => UnitDoctrine::Mobile,
            "ranged" | "naval_ranged" => UnitDoctrine::Ranged,
            "naval_carrier" => UnitDoctrine::Carrier,
            _ => UnitDoctrine::Assault,
        }
    }

    pub(crate) fn city_is_coastal(g: &Game, cid: u32) -> bool {
        g.cities.get(&cid).is_some_and(|city| {
            g.nbrs(city.pos)
                .into_iter()
                .any(|pos| g.map.get(pos).is_some_and(|tile| g.rules.is_water(tile)))
        })
    }

    pub(crate) fn empire_is_coastal(g: &Game, pid: usize) -> bool {
        g.player_city_ids(pid)
            .into_iter()
            .any(|cid| Self::city_is_coastal(g, cid))
    }

    fn tech_leads_to(g: &Game, candidate: &str, target: &str) -> bool {
        candidate == target
            || g.rules.techs.get(target).is_some_and(|spec| {
                spec.requires
                    .iter()
                    .any(|parent| Self::tech_leads_to(g, candidate, parent))
            })
    }

    /// Navigation is treated as a capability chain rather than an incidental
    /// unit unlock. Coastal empires first launch ships, then unlock general
    /// embarkation and harbors, and finally cross ocean once expansion has had
    /// time to reach the edge of its home landmass.
    pub(crate) fn water_research_goal(g: &Game, pid: usize) -> Option<&'static str> {
        if !Self::empire_is_coastal(g, pid) {
            return None;
        }
        let player = &g.players[pid];
        if !player.techs.contains("sailing") {
            return Some("sailing");
        }
        if !player.techs.contains("shipbuilding") {
            return Some("shipbuilding");
        }
        if !player.techs.contains("celestial_navigation")
            && (g.turn >= 30 || g.player_city_ids(pid).len() >= 2)
        {
            return Some("celestial_navigation");
        }
        let has_ocean = g.map.tiles.values().any(|tile| tile.terrain == "ocean");
        let has_expansion_unit = g
            .units
            .values()
            .any(|unit| unit.owner == pid && unit.kind == "settler");
        if has_ocean
            && !player.techs.contains("cartography")
            && (g.turn >= 55 || g.player_city_ids(pid).len() >= 3 || has_expansion_unit)
        {
            return Some("cartography");
        }
        let naval_war = g.players.iter().any(|enemy| {
            enemy.id != pid
                && enemy.alive
                && g.is_at_war(pid, enemy.id)
                && (g.units.values().any(|unit| {
                    unit.owner == enemy.id
                        && g.map
                            .get(unit.pos)
                            .is_some_and(|tile| g.rules.is_water(tile))
                }) || g
                    .player_city_ids(enemy.id)
                    .into_iter()
                    .any(|cid| Self::city_is_coastal(g, cid)))
        });
        if naval_war && player.techs.contains("cartography") {
            if !player.techs.contains("square_rigging") {
                return Some("square_rigging");
            }
            // After the first dedicated naval-ranged unlock, pursue later
            // fleet upgrades only when their era's prerequisite is already in
            // hand. This keeps naval readiness current without dragging an
            // ancient empire through an entire industrial branch at once.
            for (goal, prerequisite) in [
                ("steam_power", "industrialization"),
                ("refining", "rifling"),
                ("electricity", "steam_power"),
                ("combined_arms", "combustion"),
                ("lasers", "nuclear_fission"),
                ("telecommunications", "computers"),
            ] {
                if player.techs.contains(prerequisite) && !player.techs.contains(goal) {
                    return Some(goal);
                }
            }
        }
        None
    }

    pub(crate) fn waterborne(g: &Game, uid: u32) -> bool {
        let unit = &g.units[&uid];
        g.rules.units[unit.kind.as_str()].domain.as_deref() == Some("sea")
            || g.map
                .get(unit.pos)
                .is_some_and(|tile| g.rules.is_water(tile))
    }

    fn naval_counts(g: &Game, pid: usize) -> (usize, usize, usize, usize, usize) {
        let mut counts = (0, 0, 0, 0, 0);
        let mut add = |kind: &str| {
            let spec = &g.rules.units[kind];
            if spec.class != "military" || spec.domain.as_deref() != Some("sea") {
                return;
            }
            counts.0 += 1;
            match spec.promotion_class.as_str() {
                "naval_melee" => counts.1 += 1,
                "naval_ranged" => counts.2 += 1,
                "naval_raider" => counts.3 += 1,
                "naval_carrier" => counts.4 += 1,
                _ => {}
            }
        };
        for uid in g.player_unit_ids(pid) {
            add(&g.units[&uid].kind);
        }
        for cid in g.player_city_ids(pid) {
            if let Some(Item::Unit { unit }) = g.cities[&cid].queue.first() {
                add(unit);
            }
        }
        counts
    }

    pub(crate) fn desired_navy(g: &Game, pid: usize) -> usize {
        let coastal_cities = g
            .player_city_ids(pid)
            .into_iter()
            .filter(|cid| Self::city_is_coastal(g, *cid))
            .count();
        if coastal_cities == 0 || !g.players[pid].techs.contains("sailing") {
            return 0;
        }
        let mut desired = 1;
        let settlers_at_sea = g.units.values().any(|unit| {
            unit.owner == pid
                && unit.kind == "settler"
                && g.map
                    .get(unit.pos)
                    .is_some_and(|tile| g.rules.is_water(tile))
        });
        if settlers_at_sea
            || (g.players[pid].techs.contains("shipbuilding")
                && g.units
                    .values()
                    .any(|unit| unit.owner == pid && unit.kind == "settler"))
        {
            desired = desired.max(2);
        }
        let naval_war = g.players.iter().any(|enemy| {
            enemy.id != pid
                && enemy.alive
                && g.is_at_war(pid, enemy.id)
                && (g.units.values().any(|unit| {
                    unit.owner == enemy.id
                        && g.map
                            .get(unit.pos)
                            .is_some_and(|tile| g.rules.is_water(tile))
                }) || g
                    .player_city_ids(enemy.id)
                    .into_iter()
                    .any(|cid| Self::city_is_coastal(g, cid)))
        });
        if naval_war {
            desired = desired.max(coastal_cities.saturating_add(1).max(2));
        } else if g.players[pid].techs.contains("cartography") && coastal_cities >= 2 {
            desired = desired.max(2);
        }
        desired
    }

    fn has_exploration_target(&self, g: &Game, pid: usize, uid: u32) -> bool {
        g.map.tiles.iter().any(|(pos, _)| {
            !g.players[pid].explored.contains(pos) && g.unit_can_traverse(uid, *pos)
        })
    }

    /// Recon explores even during war. Without recon, one ordinary combat
    /// unit per movement domain scouts at peace so the empire is not blind,
    /// while the rest remain available for patrol and defense.
    fn should_explore(&self, g: &Game, pid: usize, uid: u32, at_war: bool) -> bool {
        if !self.has_exploration_target(g, pid, uid) {
            return false;
        }
        let doctrine = Self::unit_doctrine(g, uid);
        if doctrine == UnitDoctrine::Recon {
            return true;
        }
        if at_war
            || matches!(
                doctrine,
                UnitDoctrine::Siege
                    | UnitDoctrine::Support
                    | UnitDoctrine::AirDefense
                    | UnitDoctrine::AirStrike
                    | UnitDoctrine::Carrier
            )
        {
            return false;
        }
        let domain = g.rules.units[g.units[&uid].kind.as_str()]
            .domain
            .as_deref()
            .unwrap_or("land");
        let candidates = g.player_unit_ids(pid).into_iter().filter(|other| {
            let spec = &g.rules.units[g.units[other].kind.as_str()];
            spec.class == "military"
                && spec.domain.as_deref().unwrap_or("land") == domain
                && !matches!(
                    Self::unit_doctrine(g, *other),
                    UnitDoctrine::Siege
                        | UnitDoctrine::AirDefense
                        | UnitDoctrine::AirStrike
                        | UnitDoctrine::Carrier
                )
                && self.has_exploration_target(g, pid, *other)
        });
        let recon_exists = candidates
            .clone()
            .any(|other| Self::unit_doctrine(g, other) == UnitDoctrine::Recon);
        !recon_exists && candidates.min() == Some(uid)
    }

    /// Required exchange value for an attack. Dedicated assault and mobile
    /// units accept thinner advantages, high-strength units press them harder,
    /// recon avoids routine combat, and siege strongly prefers districts.
    pub(crate) fn attack_threshold(&self, g: &Game, uid: u32, target: Pos) -> f64 {
        let unit = &g.units[&uid];
        let doctrine = Self::unit_doctrine(g, uid);
        let role = match doctrine {
            UnitDoctrine::Recon => 14.0,
            UnitDoctrine::Assault => -2.0,
            UnitDoctrine::Mobile => -5.0,
            UnitDoctrine::Ranged => 0.0,
            UnitDoctrine::Siege => 5.0,
            UnitDoctrine::Support | UnitDoctrine::Carrier => 1_000.0,
            UnitDoctrine::AirDefense => -1.0,
            UnitDoctrine::AirStrike => -4.0,
        };
        let attack_strength = g
            .unit_strength(unit, false)
            .max(g.unit_ranged_attack_strength(unit));
        let strength_drive = ((attack_strength - 25.0) * 0.12).clamp(0.0, 8.0);
        let target_adjustment = if g.city_at(target).is_some()
            || g.map
                .get(target)
                .is_some_and(|tile| tile.district.is_some())
        {
            match doctrine {
                UnitDoctrine::Siege => -22.0,
                UnitDoctrine::Assault => -3.0,
                UnitDoctrine::Recon => 8.0,
                _ => 0.0,
            }
        } else {
            match doctrine {
                UnitDoctrine::Siege => 14.0,
                UnitDoctrine::Mobile
                    if g.units_at(target).iter().any(|other| {
                        g.rules.units[g.units[other].kind.as_str()].class != "military"
                            || g.units[other].hp <= 40
                    }) =>
                {
                    -6.0
                }
                _ => 0.0,
            }
        };
        self.w.attack_floor + role + target_adjustment - strength_drive
    }

    /// Non-generic actions that define a unit's strategic job. Fast raiders
    /// exploit infrastructure, and aircraft use missions and rebasing instead
    /// of pretending to be land units with long range.
    pub(crate) fn doctrine_action(&self, g: &Game, pid: usize, uid: u32) -> Option<Action> {
        let doctrine = Self::unit_doctrine(g, uid);
        if !matches!(
            doctrine,
            UnitDoctrine::Mobile | UnitDoctrine::AirDefense | UnitDoctrine::AirStrike
        ) {
            return None;
        }
        let legal = g.legal_doctrine_actions(pid, uid);
        match doctrine {
            UnitDoctrine::Mobile => legal
                .iter()
                .find(|action| matches!(action, Action::CoastalRaid { unit, .. } if *unit == uid))
                .cloned()
                .or_else(|| {
                    legal
                        .iter()
                        .find(|action| matches!(action, Action::Pillage { unit } if *unit == uid))
                        .cloned()
                }),
            UnitDoctrine::AirDefense => legal
                .iter()
                .find(|action| match action {
                    Action::AirStrike { unit, target } if *unit == uid => {
                        g.units_at(*target).iter().any(|other| {
                            let other = &g.units[other];
                            other.owner != pid
                                && g.rules.units[other.kind.as_str()].domain.as_deref()
                                    == Some("air")
                        })
                    }
                    _ => false,
                })
                .cloned()
                .or_else(|| {
                    legal
                        .iter()
                        .find(|action| matches!(action, Action::AirPatrol { unit } if *unit == uid))
                        .cloned()
                })
                .or_else(|| {
                    legal.into_iter().find(
                        |action| matches!(action, Action::AirStrike { unit, .. } if *unit == uid),
                    )
                }),
            UnitDoctrine::AirStrike => {
                let strike = legal
                    .iter()
                    .filter_map(|action| match action {
                        Action::AirStrike { unit, target } if *unit == uid => {
                            let target_hp = g
                                .units_at(*target)
                                .iter()
                                .filter_map(|other| {
                                    let other = &g.units[other];
                                    (other.owner != pid).then_some(other.hp)
                                })
                                .min()
                                .unwrap_or(100);
                            let city = g.city_at(*target).is_some() as i32;
                            Some((city * 120 + 100 - target_hp, *target, action.clone()))
                        }
                        _ => None,
                    })
                    .max_by_key(|(score, target, _)| (*score, std::cmp::Reverse(*target)))
                    .map(|(_, _, action)| action);
                strike
                    .or_else(|| {
                        let enemy_positions: Vec<Pos> = g
                            .units
                            .values()
                            .filter(|other| other.owner != pid && g.is_at_war(pid, other.owner))
                            .map(|other| other.pos)
                            .chain(
                                g.cities
                                    .values()
                                    .filter(|city| {
                                        city.owner != pid && g.is_at_war(pid, city.owner)
                                    })
                                    .map(|city| city.pos),
                            )
                            .collect();
                        if enemy_positions.is_empty() {
                            None
                        } else {
                            legal
                                .iter()
                                .filter_map(|action| match action {
                                    Action::AirRebase { unit, to } if *unit == uid => {
                                        let distance = enemy_positions
                                            .iter()
                                            .map(|enemy| g.wdist(*to, *enemy))
                                            .min()
                                            .unwrap_or(i32::MAX);
                                        Some((distance, *to, action.clone()))
                                    }
                                    _ => None,
                                })
                                .min_by_key(|(distance, to, _)| (*distance, *to))
                                .map(|(_, _, action)| action)
                        }
                    })
                    .or_else(|| {
                        legal.into_iter().find(
                            |action| matches!(action, Action::AirPatrol { unit } if *unit == uid),
                        )
                    })
            }
            _ => None,
        }
    }

    pub fn new() -> BasicAi {
        BasicAi {
            minor: false,
            barb: false,
            pursue_religion: true,
            w: Weights::default(),
            book_pos: 0,
            recovering_units: HashSet::new(),
            patrol_targets: HashMap::new(),
            settler_targets: HashMap::new(),
        }
    }

    pub fn with_weights(w: Weights) -> BasicAi {
        BasicAi {
            minor: false,
            barb: false,
            pursue_religion: true,
            w,
            book_pos: 0,
            recovering_units: HashSet::new(),
            patrol_targets: HashMap::new(),
            settler_targets: HashMap::new(),
        }
    }

    pub fn fleet(g: &Game) -> Vec<BasicAi> {
        g.players.iter().map(|_| BasicAi::new()).collect()
    }

    /// Majors get `w`; minors/barbarians keep default weights.
    pub fn fleet_weighted(g: &Game, w: &Weights) -> Vec<BasicAi> {
        g.players
            .iter()
            .map(|p| {
                if p.is_minor || p.is_barbarian {
                    BasicAi::new()
                } else {
                    BasicAi::with_weights(w.clone())
                }
            })
            .collect()
    }
}

impl Ai for BasicAi {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        self.minor = g.players[pid].is_minor;
        self.barb = g.players[pid].is_barbarian;
        self.resolve_city_dispositions(g, pid, false, false);
        if !self.barb {
            self.research(g, pid);
            self.corporations(g, pid);
            self.diplomacy(g, pid);
            self.spies(g, pid);
            self.cities(g, pid);
        }
        self.units(g, pid);
        self.resolve_city_dispositions(g, pid, false, false);
        if g.winner.is_none() && g.current == pid {
            let _ = g.apply(pid, &Action::EndTurn);
        }
    }
}

impl BasicAi {
    /// Run each available agent once. The baseline establishes sources before
    /// attempting the highest expected-value operation and otherwise embeds
    /// agents in the most developed non-allied foreign city.
    pub(crate) fn spies(&self, g: &mut Game, pid: usize) {
        let ids: Vec<u32> = g
            .spies
            .values()
            .filter(|spy| spy.owner == pid)
            .map(|spy| spy.id)
            .collect();
        for spy_id in ids {
            let legal = g.legal_spy_actions(pid, spy_id);
            if legal.is_empty() {
                continue;
            }
            if let Some(action) = [
                "technologist",
                "con_artist",
                "disguise",
                "linguist",
                "quartermaster",
                "seduction",
            ]
            .into_iter()
            .find_map(|wanted| {
                legal.iter().find(|action| {
                    matches!(action, Action::PromoteSpy { promotion, .. } if promotion == wanted)
                })
            })
            .or_else(|| {
                legal
                    .iter()
                    .find(|action| matches!(action, Action::PromoteSpy { .. }))
            }) {
                let _ = g.apply(pid, action);
                continue;
            }
            let current_city = g.spies.get(&spy_id).and_then(|spy| spy.city);
            let offensive = current_city
                .and_then(|city| g.cities.get(&city))
                .is_some_and(|city| city.owner != pid);
            if offensive {
                if let Some(action) = legal.iter().find(|action| {
                    matches!(action, Action::SpyMission { mission, .. } if mission == "gain_sources")
                }) {
                    let _ = g.apply(pid, action);
                    continue;
                }
                let operation = legal
                    .iter()
                    .filter_map(|action| {
                        let Action::SpyMission {
                            spy,
                            mission,
                            target,
                        } = action
                        else {
                            return None;
                        };
                        let active = crate::game::SpyMission {
                            kind: mission.clone(),
                            city: current_city?,
                            target: *target,
                            started: g.turn,
                            ends: g.turn,
                        };
                        let value = match mission.as_str() {
                            "steal_tech_boost" => 105.0,
                            "siphon_funds" => 95.0,
                            "great_work_heist" => 90.0,
                            "neutralize_governor" => 82.0,
                            "disrupt_rocketry" => 80.0,
                            "fabricate_scandal" => 74.0,
                            "sabotage_production" => 70.0,
                            "foment_unrest" => 62.0,
                            "breach_dam" => 58.0,
                            "recruit_partisans" => 55.0,
                            "listening_post" => 42.0,
                            _ => 0.0,
                        };
                        Some((g.spy_success_chance(*spy, &active) * value, mission, action))
                    })
                    .max_by(|left, right| {
                        left.0
                            .partial_cmp(&right.0)
                            .unwrap()
                            .then_with(|| right.1.cmp(left.1))
                    })
                    .map(|(_, _, action)| action);
                if let Some(action) = operation {
                    let _ = g.apply(pid, action);
                    continue;
                }
            }
            let assignment = legal
                .iter()
                .filter_map(|action| match action {
                    Action::AssignSpy { city, .. } => {
                        let target = &g.cities[city];
                        (target.owner != pid).then_some((
                            target.pop as i64 * 8
                                + target.districts.len() as i64 * 12
                                + target.wonders.len() as i64 * 20
                                - i64::from(g.players[target.owner].is_minor) * 20,
                            std::cmp::Reverse(*city),
                            action,
                        ))
                    }
                    _ => None,
                })
                .max_by(|left, right| {
                    left.0
                        .cmp(&right.0)
                        .then_with(|| left.1.cmp(&right.1))
                })
                .map(|(_, _, action)| action)
                .or_else(|| {
                    legal
                        .iter()
                        .find(|action| matches!(action, Action::SpyMission { mission, .. } if mission == "counterspy"))
                });
            if let Some(action) = assignment {
                let _ = g.apply(pid, action);
            }
        }
    }

    pub(crate) fn corporations(&self, g: &mut Game, pid: usize) {
        if let Some(action) = g
            .legal_actions(pid)
            .into_iter()
            .find(|action| matches!(action, Action::FoundCorporation { .. }))
        {
            let _ = g.apply(pid, &action);
        }
    }

    /// Resolve mandatory conquest choices with explicit strategic tradeoffs.
    /// Capitals and developed bridgeheads are retained; diplomacy-oriented
    /// plans restore city-states, friends, and eliminated founders; only an
    /// aggressive plan razes a remote city whose long-run value is negligible.
    pub(crate) fn resolve_city_dispositions(
        &mut self,
        g: &mut Game,
        pid: usize,
        prefer_diplomacy: bool,
        prefer_conquest: bool,
    ) {
        loop {
            let legal = g.legal_city_disposition_actions(pid);
            let Some(cid) = legal.iter().find_map(|action| match action {
                Action::KeepCity { city }
                | Action::RazeCity { city }
                | Action::LiberateCity { city } => Some(*city),
                _ => None,
            }) else {
                break;
            };
            let city = g.cities[&cid].clone();
            let founder = city.original_owner;
            let can_liberate = legal
                .iter()
                .any(|action| matches!(action, Action::LiberateCity { city } if *city == cid));
            let diplomatic_liberation = can_liberate
                && (prefer_diplomacy
                    || g.active_emergencies.iter().any(|emergency| {
                        emergency.city == cid && emergency.members.contains(&pid)
                    })
                    || g.players[founder].is_minor
                    || !g.players[founder].alive
                    || g.are_friends(pid, founder)
                    || g.alliance_with(pid, founder).is_some());

            let nearest_core = g
                .cities
                .values()
                .filter(|other| other.owner == pid && other.id != cid)
                .map(|other| g.wdist(city.pos, other.pos))
                .min()
                .unwrap_or(i32::MAX);
            let durable_value = city.is_capital
                || city.pop >= 4
                || !city.districts.is_empty()
                || !city.wonders.is_empty()
                || nearest_core <= 8;
            let can_raze = legal
                .iter()
                .any(|action| matches!(action, Action::RazeCity { city } if *city == cid));
            let action = if diplomatic_liberation {
                Action::LiberateCity { city: cid }
            } else if can_raze && prefer_conquest && !durable_value {
                Action::RazeCity { city: cid }
            } else {
                Action::KeepCity { city: cid }
            };
            if g.apply(pid, &action).is_err() {
                break;
            }
        }
    }

    fn research(&self, g: &mut Game, pid: usize) {
        if g.players[pid].research.is_none() {
            let avail = g.available_techs(pid);
            if !avail.is_empty() {
                let water_pick = Self::water_research_goal(g, pid).and_then(|goal| {
                    avail
                        .iter()
                        .find(|tech| tech.as_str() == goal)
                        .cloned()
                        .or_else(|| {
                            avail
                                .iter()
                                .filter(|tech| Self::tech_leads_to(g, tech, goal))
                                .min_by(|a, b| {
                                    g.rules.techs[*a]
                                        .cost
                                        .partial_cmp(&g.rules.techs[*b].cost)
                                        .unwrap()
                                        .then(a.cmp(b))
                                })
                                .cloned()
                        })
                });
                let pick = water_pick
                    .or_else(|| {
                        TECH_PRIORITY
                            .iter()
                            .find(|t| avail.iter().any(|a| a == *t))
                            .map(|t| t.to_string())
                    })
                    .unwrap_or_else(|| avail[0].clone());
                let _ = g.apply(pid, &Action::Research { tech: pick });
            }
        }
        if g.players[pid].civic.is_none() {
            let avail = g.available_civics(pid);
            if !avail.is_empty() {
                let pick = CIVIC_PRIORITY
                    .iter()
                    .find(|c| avail.iter().any(|a| a == *c))
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| avail[0].clone());
                let _ = g.apply(pid, &Action::Civic { civic: pick });
            }
        }
        for gname in GOV_PRIORITY {
            if let Some(spec) = g.rules.governments.get(gname) {
                let ok = spec
                    .civic
                    .as_ref()
                    .map(|c| g.players[pid].civics.contains(c))
                    .unwrap_or(true);
                if ok {
                    if g.players[pid].government.as_deref() != Some(gname) {
                        let _ = g.apply(
                            pid,
                            &Action::Government {
                                government: gname.to_string(),
                            },
                        );
                    }
                    break;
                }
            }
        }
        let slots = g.gov_slots(pid);
        let total = slots.military + slots.economic + slots.diplomatic + slots.wildcard;
        if (g.players[pid].policies.len() as i64) < total {
            for card in POLICY_PRIORITY {
                let _ = g.apply(
                    pid,
                    &Action::SlotPolicy {
                        policy: card.to_string(),
                    },
                );
            }
        }
        if g.players[pid].secret_society.is_none() {
            let society = if self.pursue_religion {
                "voidsingers"
            } else {
                "owls_of_minerva"
            };
            let _ = g.apply(
                pid,
                &Action::ChooseSecretSociety {
                    society: society.to_string(),
                },
            );
        }
        if g.players[pid].pantheon.is_none() && g.players[pid].faith >= 25.0 {
            for b in [
                "divine_spark",
                "fertility_rites",
                "god_of_the_forge",
                "religious_settlements",
                "god_of_the_open_sky",
                "god_of_the_sea",
            ] {
                if g.apply(
                    pid,
                    &Action::ChoosePantheon {
                        belief: b.to_string(),
                    },
                )
                .is_ok()
                {
                    break;
                }
            }
        }
        if self.pursue_religion && g.players[pid].prophet_pending {
            'found: for fo in ["work_ethic", "choral_music", "feed_the_world"] {
                for fu in ["tithe", "world_church"] {
                    if g.apply(
                        pid,
                        &Action::FoundReligion {
                            follower: fo.to_string(),
                            founder: fu.to_string(),
                        },
                    )
                    .is_ok()
                    {
                        break 'found;
                    }
                }
            }
        }
        while g.governor_titles_available(pid) > 0 {
            // anchor the shakiest city
            let target = g
                .player_city_ids(pid)
                .into_iter()
                .filter(|c| !g.players[pid].governors.contains(c))
                .min_by(|a, b| {
                    g.cities[a]
                        .loyalty
                        .partial_cmp(&g.cities[b].loyalty)
                        .unwrap()
                        .then(a.cmp(b))
                });
            if let Some(c) = target {
                let governor = [
                    "pingala", "magnus", "liang", "reyna", "victor", "moksha", "amani",
                ]
                .into_iter()
                .find(|governor| !g.players[pid].governor_roster.contains_key(*governor));
                if let Some(governor) = governor {
                    if g.apply(
                        pid,
                        &Action::AppointGovernor {
                            governor: governor.to_string(),
                            city: c,
                        },
                    )
                    .is_err()
                    {
                        break;
                    }
                    continue;
                }
            }
            let promotion = [
                "pingala", "magnus", "liang", "reyna", "victor", "moksha", "amani",
            ]
            .into_iter()
            .find_map(|governor| {
                g.available_governor_promotions(pid, governor)
                    .into_iter()
                    .next()
                    .map(|promotion| (governor.to_string(), promotion))
            });
            let Some((governor, promotion)) = promotion else {
                break;
            };
            if g.apply(
                pid,
                &Action::PromoteGovernor {
                    governor,
                    promotion,
                },
            )
            .is_err()
            {
                break;
            }
        }
        while g.players[pid].envoys_free > 0 {
            // consolidate on the city-state we already lead in (suzerain push)
            let target = g
                .players
                .iter()
                .filter(|m| m.is_minor && !m.is_barbarian && m.alive && !g.is_at_war(pid, m.id))
                .max_by_key(|m| (g.envoys_at(pid, m.id), std::cmp::Reverse(m.id)))
                .map(|m| m.id);
            match target {
                Some(t) => {
                    if g.apply(pid, &Action::SendEnvoy { player: t }).is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
    }

    fn diplomacy(&self, g: &mut Game, pid: usize) {
        while let Some(dedication) = g.available_dedications(pid).into_iter().next() {
            if g.apply(pid, &Action::ChooseDedication { dedication })
                .is_err()
            {
                break;
            }
        }
        let incoming: Vec<u32> = g
            .pending_deals
            .iter()
            .filter(|deal| deal.to == pid && deal.expires >= g.turn)
            .map(|deal| deal.id)
            .collect();
        for deal_id in incoming {
            let accept = g
                .pending_deals
                .iter()
                .find(|deal| deal.id == deal_id)
                .is_some_and(|deal| {
                    let partner_power = g.military_power(deal.from);
                    let grievance = g.players[pid]
                        .grievances
                        .get(&deal.from)
                        .copied()
                        .unwrap_or(0.0);
                    deal.peace
                        || deal.give_gold >= deal.request_gold
                        || ((deal.friendship || deal.alliance.is_some() || deal.open_borders)
                            && grievance < 75.0
                            && partner_power < g.military_power(pid) * 1.8 + 20.0)
                });
            let action = if accept {
                Action::AcceptDeal { deal: deal_id }
            } else {
                Action::RejectDeal { deal: deal_id }
            };
            let _ = g.apply(pid, &action);
        }
        if let Some(session) = g.congress.clone() {
            for resolution in session.resolutions {
                if resolution.ballots.contains_key(&pid) {
                    continue;
                }
                let own_a = format!("A:{pid}");
                let emergency_choice = g
                    .emergency_proposal_for_resolution(&resolution.id)
                    .and_then(|proposal| {
                        if proposal.target == pid {
                            Some("B:oppose".to_string())
                        } else if proposal.eligible.contains(&pid) {
                            Some("A:support".to_string())
                        } else {
                            None
                        }
                    });
                if g.emergency_proposal_for_resolution(&resolution.id)
                    .is_some()
                    && emergency_choice.is_none()
                {
                    continue;
                }
                let choice = emergency_choice
                    .or_else(|| {
                        resolution
                            .ballots
                            .values()
                            .max_by_key(|(choice, votes)| {
                                (*votes, std::cmp::Reverse(choice.clone()))
                            })
                            .map(|(choice, _)| choice.clone())
                            .or_else(|| {
                                (resolution.id == "world_leader"
                                    || resolution.id == "trade_policy"
                                    || resolution.id == "migration_treaty"
                                    || resolution.id == "border_control_treaty"
                                    || resolution.id == "public_relations")
                                    .then(|| {
                                        resolution
                                            .choices
                                            .iter()
                                            .find(|choice| **choice == own_a)
                                            .cloned()
                                    })
                                    .flatten()
                            })
                            .or_else(|| {
                                (resolution.id == "world_ideology")
                                    .then(|| {
                                        let own_government =
                                            g.players[pid].government.as_deref()?;
                                        let wanted = format!("A:{own_government}");
                                        resolution
                                            .choices
                                            .iter()
                                            .find(|choice| **choice == wanted)
                                            .cloned()
                                    })
                                    .flatten()
                            })
                            .or_else(|| {
                                (resolution.id == "mercenary_companies")
                                    .then(|| {
                                        resolution
                                            .choices
                                            .iter()
                                            .find(|choice| choice.as_str() == "B:production")
                                            .cloned()
                                    })
                                    .flatten()
                            })
                            .or_else(|| {
                                let preferred = match resolution.id.as_str() {
                                    "global_energy_treaty" => Some("A:coal_power_plant"),
                                    "public_works_program" => resolution
                                        .choices
                                        .iter()
                                        .find(|choice| choice.starts_with("A:"))
                                        .map(String::as_str),
                                    "deforestation_treaty" => resolution
                                        .choices
                                        .iter()
                                        .find(|choice| choice.starts_with("A:"))
                                        .map(String::as_str),
                                    _ => None,
                                }?;
                                resolution
                                    .choices
                                    .iter()
                                    .find(|choice| choice.as_str() == preferred)
                                    .cloned()
                            })
                    })
                    .unwrap_or_else(|| resolution.choices[pid % resolution.choices.len()].clone());
                let votes = if g.players[pid].diplomatic_favor >= 30.0 {
                    3
                } else if g.players[pid].diplomatic_favor >= 10.0 {
                    2
                } else {
                    1
                };
                let _ = g.apply(
                    pid,
                    &Action::CongressVote {
                        resolution: resolution.id,
                        choice,
                        votes,
                    },
                );
            }
        }
        self.bilateral_trade(g, pid);
        let my_power = g.military_power(pid);
        let others: Vec<usize> = g
            .players
            .iter()
            .filter(|o| o.id != pid && o.alive && !o.is_barbarian)
            .map(|o| o.id)
            .collect();
        for o in &others {
            if g.is_at_war(pid, *o)
                && !g.emergency_war_pair(pid, *o)
                && my_power < self.w.peace_ratio * g.military_power(*o)
            {
                let _ = g.apply(pid, &Action::MakePeace { player: *o });
            }
        }
        if self.minor {
            return;
        }
        if g.turn % 20 == pid as u32 % 20 {
            if let Some(partner) = others.iter().copied().find(|other| {
                !g.players[*other].is_minor
                    && !g.is_at_war(pid, *other)
                    && g.players[pid].grievances.get(other).copied().unwrap_or(0.0) < 50.0
            }) {
                let alliance = if g.are_friends(pid, partner)
                    && g.players[pid].civics.contains("civil_service")
                    && g.players[partner].civics.contains("civil_service")
                    && g.alliance_with(pid, partner).is_none()
                {
                    Some(["economic", "cultural", "military", "religious"][pid % 4].to_string())
                } else {
                    None
                };
                let _ = g.apply(
                    pid,
                    &Action::ProposeDeal {
                        player: partner,
                        give_gold: 0.0,
                        request_gold: 0.0,
                        open_borders: g.players[pid].civics.contains("early_empire"),
                        friendship: true,
                        peace: false,
                        alliance,
                    },
                );
            }
        }
        let at_war = others.iter().any(|o| g.is_at_war(pid, *o));
        if at_war {
            self.levy_city_state_military(g, pid, false);
        }
        if !at_war
            && (g.turn as f64) > self.w.war_min_turn
            && g.player_city_ids(pid).len() >= 2
            && !others.is_empty()
        {
            let weakest = *others
                .iter()
                .min_by(|a, b| {
                    g.military_power(**a)
                        .partial_cmp(&g.military_power(**b))
                        .unwrap()
                })
                .unwrap();
            if my_power > self.w.war_ratio * g.military_power(weakest) + self.w.war_margin {
                let formal = g.players[pid]
                    .denounced_until
                    .get(&weakest)
                    .is_some_and(|until| *until > g.turn && *until <= g.turn + 25);
                let action = if formal {
                    Action::DeclareWarWithCasusBelli {
                        player: weakest,
                        casus_belli: "formal_war".to_string(),
                    }
                } else if !g.players[pid]
                    .denounced_until
                    .get(&weakest)
                    .is_some_and(|until| *until > g.turn)
                {
                    Action::Denounce { player: weakest }
                } else {
                    return;
                };
                let _ = g.apply(pid, &action);
            }
        }
    }

    /// Turn spare wartime Gold into immediately usable troops when this AI is
    /// a city-state's Suzerain. `urgent` lets the strategic AI spend deeper
    /// into its treasury for Conquest/Recovery plans; the general AI retains
    /// a larger economic reserve.
    pub(crate) fn levy_city_state_military(&self, g: &mut Game, pid: usize, urgent: bool) {
        if self.minor || self.barb {
            return;
        }
        let reserve_share = if urgent { 0.20 } else { 0.40 };
        let spendable = (g.players[pid].gold * (1.0 - reserve_share) - 20.0).max(0.0);
        let best = g
            .players
            .iter()
            .filter(|minor| minor.is_minor && !minor.is_barbarian && minor.alive)
            .filter_map(|minor| {
                let cost = g.levy_cost(pid, minor.id)?;
                if cost > spendable + f64::EPSILON {
                    return None;
                }
                let strength = g
                    .units
                    .values()
                    .filter(|unit| unit.owner == minor.id && unit.levied_from.is_none())
                    .filter(|unit| g.rules.units[unit.kind.as_str()].class == "military")
                    .map(|unit| g.unit_strength(unit, true))
                    .sum::<f64>();
                Some((
                    strength / cost.max(1.0),
                    strength,
                    std::cmp::Reverse(minor.id),
                    minor.id,
                ))
            })
            .max_by(|left, right| {
                left.0
                    .partial_cmp(&right.0)
                    .unwrap()
                    .then_with(|| left.1.partial_cmp(&right.1).unwrap())
                    .then(left.2.cmp(&right.2))
            })
            .map(|(_, _, _, minor)| minor);
        if let Some(player) = best {
            let _ = g.apply(pid, &Action::LevyMilitary { player });
        }
    }

    /// Execute at most one pre-negotiated exchange on a staggered cadence.
    /// `Game::quick_deals` has already valued both sides, and `Action::Trade`
    /// revalidates the contract atomically, so the AI never relies on gifts,
    /// exploits stale quotes, or trades when either empire would lose value.
    pub(crate) fn bilateral_trade(&self, g: &mut Game, pid: usize) {
        self.bilateral_trade_excluding(g, pid, None);
    }

    pub(crate) fn bilateral_trade_excluding(
        &self,
        g: &mut Game,
        pid: usize,
        excluded_partner: Option<usize>,
    ) {
        if self.minor || self.barb || g.turn % 6 != (pid as u32 % 6) {
            return;
        }
        let best = g
            .quick_deals(pid)
            .into_iter()
            .filter(|deal| Some(deal.partner) != excluded_partner)
            .max_by(|left, right| {
                left.my_value
                    .min(left.partner_value)
                    .partial_cmp(&right.my_value.min(right.partner_value))
                    .unwrap()
            });
        let Some(deal) = best.filter(|deal| deal.my_value >= 2.0 && deal.partner_value >= 2.0)
        else {
            return;
        };
        let _ = g.apply(
            pid,
            &Action::Trade {
                player: deal.partner,
                offer: deal.offer,
                request: deal.request,
            },
        );
    }

    fn cities(&mut self, g: &mut Game, pid: usize) {
        let mut settlers = 0;
        let mut builders = 0;
        let mut traders = 0;
        let mut military = 0;
        let mut melee = 0;
        let mut ranged = 0;
        for uid in g.player_unit_ids(pid) {
            let kind = g.units[&uid].kind.clone();
            match kind.as_str() {
                "settler" => settlers += 1,
                "builder" => builders += 1,
                "trader" => traders += 1,
                _ => {
                    let spec = &g.rules.units[kind.as_str()];
                    if spec.class == "military" {
                        military += 1;
                        if spec.has_ranged_attack() {
                            ranged += 1;
                        } else {
                            melee += 1;
                        }
                    }
                }
            }
        }
        let city_ids = g.player_city_ids(pid);
        let n_cities = city_ids.len();
        // Treat queued units as part of the force plan. Without this, every
        // occupied city forgets what it is already building and the next
        // empty city can queue a duplicate settler, builder, or trader.
        for cid in &city_ids {
            if let Some(Item::Unit { unit }) = g.cities[cid].queue.first() {
                match unit.as_str() {
                    "settler" => settlers += 1,
                    "builder" => builders += 1,
                    "trader" => traders += 1,
                    _ => {
                        let spec = &g.rules.units[unit.as_str()];
                        if spec.class == "military" {
                            military += 1;
                            if spec.has_ranged_attack() {
                                ranged += 1;
                            } else {
                                melee += 1;
                            }
                        }
                    }
                }
            }
        }
        // Walls fire at raiders in range. Encampment strikes are collected
        // once after the city loop: rebuilding the complete action list for
        // every city also enumerates production, deals, Congress votes, and
        // every unit move, which becomes quadratic in a developed empire.
        for cid in &city_ids {
            if g.city_can_strike(&g.cities[cid]) {
                let cpos = g.cities[cid].pos;
                for pos in g.wdisk(cpos, 2) {
                    let hit = g.units_at(pos).into_iter().any(|oid| {
                        let o = &g.units[&oid];
                        o.owner != pid && g.is_at_war(pid, o.owner)
                    });
                    if hit {
                        let _ = g.apply(
                            pid,
                            &Action::CityStrike {
                                city: *cid,
                                target: pos,
                            },
                        );
                        break;
                    }
                }
            }
        }
        let has_ready_encampment = city_ids.iter().any(|cid| {
            let city = &g.cities[cid];
            city.encampment_hp > 0
                && city.encampment_wall_hp > 0
                && !city.encampment_pillaged
                && !city.encampment_struck
        });
        if has_ready_encampment {
            let strikes: Vec<Action> = g
                .legal_actions(pid)
                .into_iter()
                .filter(|action| matches!(action, Action::EncampmentStrike { .. }))
                .collect();
            let mut used = HashSet::new();
            for action in strikes {
                let Action::EncampmentStrike { city, .. } = &action else {
                    unreachable!()
                };
                if used.insert(*city) {
                    let _ = g.apply(pid, &action);
                }
            }
        }
        for cid in &city_ids {
            if !g.cities[cid].queue.is_empty() {
                continue;
            }
            // chess-style opening book: scripted first capital builds
            if !self.minor && !self.barb && g.cities[cid].is_capital && self.book_pos < 4 {
                let mut played = false;
                while self.book_pos < 4 && !played {
                    let gene =
                        [self.w.open0, self.w.open1, self.w.open2, self.w.open3][self.book_pos];
                    self.book_pos += 1;
                    let i = gene.max(0.0) as usize;
                    if i >= OPENING_MENU.len() {
                        continue; // "pass" gene: fall back to evaluation
                    }
                    let name = OPENING_MENU[i];
                    let item = if name == "monument" {
                        Item::Building {
                            building: name.to_string(),
                        }
                    } else {
                        Item::Unit {
                            unit: name.to_string(),
                        }
                    };
                    if g.apply(
                        pid,
                        &Action::Produce {
                            city: *cid,
                            item: item.clone(),
                        },
                    )
                    .is_ok()
                    {
                        match &item {
                            Item::Unit { unit } if unit == "settler" => settlers += 1,
                            Item::Unit { unit } if unit == "builder" => builders += 1,
                            Item::Unit { unit } if unit == "trader" => traders += 1,
                            Item::Unit { unit } => {
                                let spec = &g.rules.units[unit.as_str()];
                                if spec.class == "military" {
                                    military += 1;
                                    if spec.has_ranged_attack() {
                                        ranged += 1;
                                    } else {
                                        melee += 1;
                                    }
                                }
                            }
                            _ => {}
                        }
                        played = true;
                    }
                }
                if played {
                    continue;
                }
            }
            if let Some(item) = self.pick_item(
                g, pid, *cid, n_cities, settlers, builders, traders, military, melee, ranged,
            ) {
                if g.apply(
                    pid,
                    &Action::Produce {
                        city: *cid,
                        item: item.clone(),
                    },
                )
                .is_ok()
                {
                    match &item {
                        Item::Unit { unit } if unit == "settler" => settlers += 1,
                        Item::Unit { unit } if unit == "builder" => builders += 1,
                        Item::Unit { unit } if unit == "trader" => traders += 1,
                        Item::Unit { unit } => {
                            let spec = &g.rules.units[unit.as_str()];
                            if spec.class == "military" {
                                military += 1;
                                if spec.has_ranged_attack() {
                                    ranged += 1;
                                } else {
                                    melee += 1;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        self.spend_gold(
            g, pid, &city_ids, settlers, builders, traders, military, melee, ranged,
        );
        if g.players[pid].faith >= self.w.faith_builder
            && builders < n_cities
            && !city_ids.is_empty()
        {
            let _ = g.apply(
                pid,
                &Action::Buy {
                    city: city_ids[0],
                    unit: "builder".to_string(),
                    currency: "faith".to_string(),
                },
            );
        }
        if self.pursue_religion
            && g.players[pid].religion.is_some()
            && g.players[pid].faith >= 250.0
        {
            let missionaries = g
                .units
                .values()
                .filter(|u| u.owner == pid && u.kind == "missionary")
                .count();
            if missionaries < 2 {
                for cid in &city_ids {
                    if g.cities[cid].districts.contains_key("holy_site") {
                        let _ = g.apply(
                            pid,
                            &Action::Buy {
                                city: *cid,
                                unit: "missionary".to_string(),
                                currency: "faith".to_string(),
                            },
                        );
                        break;
                    }
                }
            }
        }
    }

    fn best_military(
        &self,
        g: &Game,
        pid: usize,
        cid: u32,
        want_ranged: Option<bool>,
    ) -> Option<String> {
        let mut best: Option<(f64, String)> = None;
        for (name, spec) in &g.rules.units {
            if spec.class != "military" || spec.domain.as_deref() == Some("sea") {
                continue;
            }
            if want_ranged
                .map(|want| want != spec.has_ranged_attack())
                .unwrap_or(false)
            {
                continue;
            }
            if !g.can_produce(pid, cid, &Item::Unit { unit: name.clone() }) {
                continue;
            }
            let power = spec.strength.max(spec.ranged_attack_strength());
            if best.as_ref().map(|(b, _)| power > *b).unwrap_or(true) {
                best = Some((power, name.clone()));
            }
        }
        best.map(|(_, n)| n)
    }

    fn best_naval_unit(&self, g: &Game, pid: usize, cid: u32) -> Option<String> {
        if !Self::city_is_coastal(g, cid) {
            return None;
        }
        let (total, melee, ranged, raiders, carriers) = Self::naval_counts(g, pid);
        let has_aircraft = g.units.values().any(|unit| {
            unit.owner == pid && g.rules.units[unit.kind.as_str()].domain.as_deref() == Some("air")
        });
        g.rules
            .units
            .iter()
            .filter(|(name, spec)| {
                spec.class == "military"
                    && spec.domain.as_deref() == Some("sea")
                    && g.can_produce(
                        pid,
                        cid,
                        &Item::Unit {
                            unit: (*name).clone(),
                        },
                    )
            })
            .map(|(name, spec)| {
                let power = spec.strength.max(spec.ranged_attack_strength());
                let role = match spec.promotion_class.as_str() {
                    // A navy without melee ships can bombard but never take a
                    // coastal city; preserve at least half the fleet for that
                    // capturing/screening role.
                    "naval_melee" => 42.0 * (melee <= ranged + raiders) as i32 as f64,
                    "naval_ranged" => 34.0 * (ranged < melee.max(1)) as i32 as f64,
                    "naval_raider" => 22.0 * (total >= 2 && raiders == 0) as i32 as f64,
                    "naval_carrier" => {
                        if has_aircraft && carriers == 0 {
                            30.0
                        } else {
                            -120.0
                        }
                    }
                    _ => 0.0,
                };
                (power * 3.0 + role - spec.cost * 0.04, name.clone())
            })
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then_with(|| b.1.cmp(&a.1)))
            .map(|(_, name)| name)
    }

    fn combined_arms_unit(
        &self,
        g: &Game,
        pid: usize,
        cid: u32,
        melee: usize,
        ranged: usize,
    ) -> Option<String> {
        // Ranged units trade efficiently, but only melee units can take a
        // city. Alternate the strongest available unit in each role so an
        // advanced army never degenerates into an uncapturing firing line.
        let want_ranged = melee > ranged;
        self.best_military(g, pid, cid, Some(want_ranged))
            .or_else(|| self.best_military(g, pid, cid, None))
    }

    fn buy_gold_unit(
        &self,
        g: &mut Game,
        pid: usize,
        city_ids: &[u32],
        unit: &str,
        reserve: f64,
    ) -> bool {
        let price = match g.rules.units.get(unit) {
            Some(spec) => spec.cost * 4.0,
            None => return false,
        };
        if g.players[pid].gold + 1e-9 < price + reserve {
            return false;
        }
        for cid in city_ids {
            if !g.can_produce(
                pid,
                *cid,
                &Item::Unit {
                    unit: unit.to_string(),
                },
            ) {
                continue;
            }
            if g.apply(
                pid,
                &Action::Buy {
                    city: *cid,
                    unit: unit.to_string(),
                    currency: "gold".to_string(),
                },
            )
            .is_ok()
            {
                return true;
            }
        }
        false
    }

    fn buy_gold_military(
        &self,
        g: &mut Game,
        pid: usize,
        city_ids: &[u32],
        reserve: f64,
        want_ranged: bool,
    ) -> bool {
        let budget = g.players[pid].gold - reserve;
        if budget <= 0.0 {
            return false;
        }
        let choose = |role: Option<bool>| -> Option<(u32, String)> {
            let mut best: Option<(f64, f64, String, u32)> = None;
            for cid in city_ids {
                for (name, spec) in &g.rules.units {
                    if spec.class != "military"
                        || spec.domain.as_deref() == Some("sea")
                        || role.map(|r| r != spec.has_ranged_attack()).unwrap_or(false)
                    {
                        continue;
                    }
                    let price = spec.cost * 4.0;
                    if price > budget + 1e-9
                        || !g.can_produce(pid, *cid, &Item::Unit { unit: name.clone() })
                    {
                        continue;
                    }
                    let power = spec.strength.max(spec.ranged_attack_strength());
                    let replace = match &best {
                        None => true,
                        Some((bp, bc, bn, bid)) => {
                            power > *bp + 1e-9
                                || ((power - *bp).abs() < 1e-9
                                    && (price < *bc - 1e-9
                                        || ((price - *bc).abs() < 1e-9
                                            && (name.as_str(), *cid) < (bn.as_str(), *bid))))
                        }
                    };
                    if replace {
                        best = Some((power, price, name.clone(), *cid));
                    }
                }
            }
            best.map(|(_, _, unit, city)| (city, unit))
        };
        let (city, unit) = match choose(Some(want_ranged)).or_else(|| choose(None)) {
            Some(choice) => choice,
            None => return false,
        };
        g.apply(
            pid,
            &Action::Buy {
                city,
                unit,
                currency: "gold".to_string(),
            },
        )
        .is_ok()
    }

    #[allow(clippy::too_many_arguments)]
    fn spend_gold(
        &self,
        g: &mut Game,
        pid: usize,
        city_ids: &[u32],
        settlers: usize,
        builders: usize,
        traders: usize,
        military: usize,
        melee: usize,
        ranged: usize,
    ) -> bool {
        if city_ids.is_empty() {
            return false;
        }
        let n_cities = city_ids.len();
        let at_major_war = g
            .players
            .iter()
            .any(|p| p.id != pid && p.alive && !p.is_barbarian && g.is_at_war(pid, p.id));
        let reserve = if at_major_war {
            40.0 + 10.0 * n_cities as f64
        } else {
            100.0 + 25.0 * n_cities as f64
        };
        let want_ranged = melee > ranged;

        // A threatened empire converts cash into defenders before pursuing
        // infrastructure. Two units per city is enough to react without
        // draining the treasury into an endless standing army.
        let normal_military = (self.w.mil_per_city * n_cities as f64).ceil() as usize;
        let wartime_military = normal_military.max(2 * n_cities);
        if at_major_war
            && military < wartime_military
            && self.buy_gold_military(g, pid, city_ids, reserve, want_ranged)
        {
            return true;
        }

        let desired_builders = (self.w.builder_per_city * n_cities as f64).ceil() as usize;
        if builders < desired_builders && self.buy_gold_unit(g, pid, city_ids, "builder", reserve) {
            return true;
        }

        if !self.minor
            && g.active_routes(pid) + (traders as i64) < g.trade_capacity(pid)
            && self.buy_gold_unit(g, pid, city_ids, "trader", reserve)
        {
            return true;
        }

        if !self.minor
            && settlers == 0
            && (n_cities as f64) < self.w.city_target
            && (g.turn as f64) < self.w.settler_stop_turn
            && self.buy_gold_unit(g, pid, city_ids, "settler", reserve)
        {
            return true;
        }

        // At peace, retain a larger reserve but turn a deep surplus into a
        // modest deterrent instead of hoarding gold indefinitely.
        g.players[pid].gold >= reserve + 600.0
            && military < 2 * n_cities
            && self.buy_gold_military(g, pid, city_ids, reserve, want_ranged)
    }

    #[allow(clippy::too_many_arguments)]
    fn pick_item(
        &self,
        g: &Game,
        pid: usize,
        cid: u32,
        n_cities: usize,
        settlers: usize,
        builders: usize,
        traders: usize,
        military: usize,
        melee: usize,
        ranged: usize,
    ) -> Option<Item> {
        let city_pop = g.cities[&cid].pop;
        if (military as f64) < self.w.mil_per_city * n_cities as f64 {
            if let Some(m) = self.combined_arms_unit(g, pid, cid, melee, ranged) {
                return Some(Item::Unit { unit: m });
            }
        }
        if !self.minor && !self.barb {
            let has_spaceport = g
                .cities
                .values()
                .any(|c| c.owner == pid && c.districts.contains_key("spaceport"));
            if !has_spaceport && g.players[pid].techs.contains("rocketry") {
                if let Some(pos) = g.district_sites(cid, "spaceport").into_iter().next() {
                    let item = Item::District {
                        district: "spaceport".to_string(),
                        pos,
                    };
                    if g.can_produce(pid, cid, &item) {
                        return Some(item);
                    }
                }
            }
            let spy = Item::Unit {
                unit: "spy".to_string(),
            };
            if g.can_produce(pid, cid, &spy) {
                return Some(spy);
            }
            if let Some(product) = g
                .producible_items(pid, cid)
                .into_iter()
                .find(|item| matches!(item, Item::Product { .. }))
            {
                return Some(product);
            }
            let mut projects: Vec<Item> = g
                .rules
                .projects
                .iter()
                .filter(|(project, spec)| {
                    !spec.repeatable
                        || matches!(
                            project.as_str(),
                            "lagrange_laser_station" | "terrestrial_laser_station"
                        )
                })
                .map(|(project, _)| Item::Project {
                    project: project.clone(),
                })
                .filter(|item| g.can_produce(pid, cid, item))
                .collect();
            projects.sort_by(|a, b| {
                g.item_cost_for(pid, a)
                    .partial_cmp(&g.item_cost_for(pid, b))
                    .unwrap()
                    .then_with(|| format!("{a:?}").cmp(&format!("{b:?}")))
            });
            if let Some(project) = projects.into_iter().next() {
                return Some(project);
            }
        }
        let naval = Self::naval_counts(g, pid).0;
        if naval < Self::desired_navy(g, pid) {
            if let Some(unit) = self.best_naval_unit(g, pid, cid) {
                return Some(Item::Unit { unit });
            }
        }
        if !self.minor
            && !self.barb
            && ((n_cities + settlers) as f64) < self.w.city_target
            && settlers == 0
            && (city_pop as f64) >= self.w.settler_min_pop
            && (g.turn as f64) < self.w.settler_stop_turn
        {
            return Some(Item::Unit {
                unit: "settler".to_string(),
            });
        }
        if (builders as f64) < self.w.builder_per_city * n_cities as f64 {
            return Some(Item::Unit {
                unit: "builder".to_string(),
            });
        }
        if !self.minor
            && g.active_routes(pid) + (traders as i64) < g.trade_capacity(pid)
            && g.can_produce(
                pid,
                cid,
                &Item::Unit {
                    unit: "trader".to_string(),
                },
            )
        {
            return Some(Item::Unit {
                unit: "trader".to_string(),
            });
        }
        if !g.cities[&cid].buildings.iter().any(|b| b == "monument") {
            return Some(Item::Building {
                building: "monument".to_string(),
            });
        }
        // Coastal infrastructure is part of the water strategy, not an
        // accidental fallback after every land district. A harbor also gives
        // later naval production somewhere sensible to concentrate.
        if Self::city_is_coastal(g, cid) && !g.cities[&cid].districts.contains_key("harbor") {
            let sites = g.district_sites(cid, "harbor");
            if let Some(pos) = sites.into_iter().max_by(|a, b| {
                g.district_yields("harbor", *a)
                    .total()
                    .partial_cmp(&g.district_yields("harbor", *b).total())
                    .unwrap()
                    .then(a.cmp(b))
            }) {
                let item = Item::District {
                    district: "harbor".to_string(),
                    pos,
                };
                if g.can_produce(pid, cid, &item) {
                    return Some(item);
                }
            }
        }
        let mut dpri: Vec<(&str, f64)> = DISTRICT_PRIORITY
            .iter()
            .cloned()
            .zip([
                self.w.d_campus,
                self.w.d_commercial,
                self.w.d_holy,
                self.w.d_theater,
            ])
            .collect();
        dpri.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        for (dname, _) in dpri {
            if g.cities[&cid].districts.contains_key(dname) {
                continue;
            }
            let spec = &g.rules.districts[dname];
            let unlocked = spec
                .tech
                .as_ref()
                .map(|t| g.players[pid].techs.contains(t))
                .unwrap_or(true)
                && spec
                    .civic
                    .as_ref()
                    .map(|c| g.players[pid].civics.contains(c))
                    .unwrap_or(true);
            if !unlocked {
                continue;
            }
            let sites = g.district_sites(cid, dname);
            if !sites.is_empty() {
                let best = *sites
                    .iter()
                    .max_by(|a, b| {
                        let ya = g.district_yields(dname, **a).total();
                        let yb = g.district_yields(dname, **b).total();
                        ya.partial_cmp(&yb).unwrap().then(a.cmp(b))
                    })
                    .unwrap();
                return Some(Item::District {
                    district: dname.to_string(),
                    pos: best,
                });
            }
        }
        let mut buildable: Vec<(i64, String)> = g
            .rules
            .buildings
            .iter()
            .filter(|(b, s)| {
                !s.wonder
                    && g.can_produce(
                        pid,
                        cid,
                        &Item::Building {
                            building: (*b).clone(),
                        },
                    )
            })
            .map(|(b, s)| (s.cost as i64, b.clone()))
            .collect();
        if !buildable.is_empty() {
            buildable.sort();
            return Some(Item::Building {
                building: buildable[0].1.clone(),
            });
        }
        // developed cities turn to wonders
        if g.cities[&cid].buildings.len() as f64 >= self.w.wonder_min_bld {
            let mut wonders: Vec<(i64, String)> = g
                .rules
                .buildings
                .iter()
                .filter(|(b, s)| {
                    s.wonder
                        && g.can_produce(
                            pid,
                            cid,
                            &Item::Building {
                                building: (*b).clone(),
                            },
                        )
                })
                .map(|(b, s)| (s.cost as i64, b.clone()))
                .collect();
            if !wonders.is_empty() {
                wonders.sort();
                return Some(Item::Building {
                    building: wonders[0].1.clone(),
                });
            }
        }
        // Repeatable district projects are a developed-city fallback. If
        // considered with mandatory projects above, their low early base cost
        // makes a basic AI loop them forever before building Monuments,
        // districts, or district buildings.
        if !self.minor && !self.barb {
            let mut projects: Vec<Item> = g
                .rules
                .projects
                .iter()
                .filter(|(project, spec)| {
                    spec.repeatable
                        && !matches!(
                            project.as_str(),
                            "lagrange_laser_station" | "terrestrial_laser_station"
                        )
                })
                .map(|(project, _)| Item::Project {
                    project: project.clone(),
                })
                .filter(|item| g.can_produce(pid, cid, item))
                .collect();
            projects.sort_by(|a, b| {
                g.item_cost_for(pid, a)
                    .partial_cmp(&g.item_cost_for(pid, b))
                    .unwrap()
                    .then_with(|| format!("{a:?}").cmp(&format!("{b:?}")))
            });
            if let Some(project) = projects.into_iter().next() {
                return Some(project);
            }
        }
        self.combined_arms_unit(g, pid, cid, melee, ranged)
            .map(|m| Item::Unit { unit: m })
    }

    fn units(&mut self, g: &mut Game, pid: usize) {
        self.prepare_unit_formations(g, pid);
        self.recovering_units
            .retain(|uid| g.units.get(uid).is_some_and(|unit| unit.owner == pid));
        self.patrol_targets
            .retain(|uid, _| g.units.get(uid).is_some_and(|unit| unit.owner == pid));
        self.settler_targets
            .retain(|uid, _| g.units.get(uid).is_some_and(|unit| unit.owner == pid));
        for uid in g.player_unit_ids(pid) {
            for _ in 0..8 {
                if !g.units.contains_key(&uid) {
                    break;
                }
                if g.units[&uid].moves_left <= 0.0 {
                    break;
                }
                let kind = g.units[&uid].kind.clone();
                let acted = match kind.as_str() {
                    "settler" => self.settler_step(g, pid, uid),
                    "builder" => self.builder_step(g, pid, uid),
                    "military_engineer" => self.military_engineer_step(g, pid, uid),
                    "naturalist" => self.naturalist_step(g, pid, uid),
                    "archaeologist" => self.archaeologist_step(g, pid, uid),
                    "trader" => self.trader_step(g, pid, uid),
                    "missionary" => self.missionary_step(g, pid, uid),
                    "rock_band" => self.rock_band_step(g, pid, uid),
                    _ => self.military_step(g, pid, uid),
                };
                if !acted {
                    break;
                }
            }
        }
    }

    /// Spend earned promotions before moving, then consolidate eligible
    /// military units into Corps/Armies and attach colocated support units.
    /// These actions otherwise never occur in headless self-play because they
    /// are neither movement nor attacks.
    pub(crate) fn prepare_unit_formations(&self, g: &mut Game, pid: usize) {
        for uid in g.player_unit_ids(pid) {
            let Some(promotion) = g.available_promotions(uid).into_iter().max_by(|a, b| {
                let value = |name: &str| {
                    g.rules.promotions[name]
                        .effects
                        .values()
                        .map(|effect| effect.abs())
                        .sum::<f64>()
                };
                value(a)
                    .partial_cmp(&value(b))
                    .unwrap()
                    .then_with(|| b.cmp(a))
            }) else {
                continue;
            };
            let _ = g.apply(
                pid,
                &Action::Promote {
                    unit: uid,
                    promotion,
                },
            );
        }

        if g.players[pid].civics.contains("nationalism") {
            let reserve = (g.player_city_ids(pid).len() + 3).max(5);
            loop {
                let military = g
                    .player_unit_ids(pid)
                    .into_iter()
                    .filter(|uid| g.rules.units[g.units[uid].kind.as_str()].class == "military")
                    .count();
                if military <= reserve {
                    break;
                }
                let action = g
                    .legal_actions(pid)
                    .into_iter()
                    .find(|action| matches!(action, Action::CombineUnits { .. }));
                let Some(action) = action else { break };
                if g.apply(pid, &action).is_err() {
                    break;
                }
            }
        }

        let has_link_candidate = |game: &Game| {
            let units = game.player_unit_ids(pid);
            units.iter().enumerate().any(|(index, unit)| {
                units[index + 1..].iter().any(|with| {
                    let a = &game.units[unit];
                    let b = &game.units[with];
                    if a.pos != b.pos || a.linked_to.is_some() || b.linked_to.is_some() {
                        return false;
                    }
                    let a_spec = &game.rules.units[a.kind.as_str()];
                    let b_spec = &game.rules.units[b.kind.as_str()];
                    let support = (a_spec.class == "support"
                        && a.kind != "military_engineer"
                        && b_spec.class == "military")
                        || (b_spec.class == "support"
                            && b.kind != "military_engineer"
                            && a_spec.class == "military");
                    let naval_settler = (a_spec.domain.as_deref() == Some("sea")
                        && b.kind == "settler")
                        || (b_spec.domain.as_deref() == Some("sea") && a.kind == "settler");
                    support || naval_settler
                })
            })
        };
        while has_link_candidate(g) {
            let action = g
                .legal_actions(pid)
                .into_iter()
                .find(|action| match action {
                    Action::LinkUnits { unit, with } => {
                        let a = &g.rules.units[g.units[unit].kind.as_str()];
                        let b = &g.rules.units[g.units[with].kind.as_str()];
                        let support = (a.class == "support"
                            && g.units[unit].kind != "military_engineer")
                            || (b.class == "support" && g.units[with].kind != "military_engineer");
                        let naval_settler = (a.domain.as_deref() == Some("sea")
                            && g.units[with].kind == "settler")
                            || (b.domain.as_deref() == Some("sea")
                                && g.units[unit].kind == "settler");
                        support || naval_settler
                    }
                    _ => false,
                });
            let Some(action) = action else { break };
            if g.apply(pid, &action).is_err() {
                break;
            }
        }
    }

    /// 1-ply positional search for wartime marching: score each candidate
    /// tile (stay put or any legal neighbor) by progress toward the target,
    /// adjacent friendly support, and expected incoming damage; take the best.
    fn tactical_step(
        &self,
        g: &mut Game,
        pid: usize,
        uid: u32,
        target: Pos,
        enemy_ids: &[usize],
        attack_range: i32,
    ) -> bool {
        let upos = g.units[&uid].pos;
        let u = &g.units[&uid];
        let my_def = effective_strength(g.unit_strength(u, true), u.hp);
        let doctrine = Self::unit_doctrine(g, uid);
        let (preferred_range, progress, threat_caution) = match doctrine {
            UnitDoctrine::Recon => (2, 0.60, 1.35),
            UnitDoctrine::Assault => (1, 1.15, 1.00),
            UnitDoctrine::Mobile => (1, 1.40, 0.80),
            UnitDoctrine::Ranged => (attack_range.max(1), 0.90, 1.15),
            UnitDoctrine::Siege => (attack_range.max(1), 0.80, 1.25),
            UnitDoctrine::Support | UnitDoctrine::Carrier => (2, 0.65, 1.40),
            UnitDoctrine::AirDefense | UnitDoctrine::AirStrike => (attack_range.max(1), 1.0, 1.0),
        };
        let score = |g: &Game, tile: Pos| -> f64 {
            let depth_error = (g.wdist(tile, target) - preferred_range).abs();
            let mut s = -3.0 * progress * depth_error as f64;
            let mut adjacent_support = 0;
            for n in g.nbrs(tile) {
                for oid in g.units_at(n) {
                    let o = &g.units[&oid];
                    if g.rules.units[o.kind.as_str()].class != "military" {
                        continue;
                    }
                    if o.owner == pid && oid != uid {
                        adjacent_support += 1;
                    } else if enemy_ids.contains(&o.owner) {
                        let att = effective_strength(g.unit_strength(o, false), o.hp);
                        s -= self.w.mv_threat
                            * threat_caution
                            * 30.0
                            * ((att - my_def) / 25.0).exp();
                    }
                }
            }
            // A pair of neighbors is enough to hold a coherent line. Giving
            // every extra adjacent unit the full bonus makes dense armies
            // refuse to leave their initial cluster even when a safe campaign
            // route is open.
            s += self.w.mv_support * adjacent_support.min(2) as f64;
            s
        };
        let stay = score(g, upos);
        let holding_role_position = g.wdist(upos, target) == preferred_range;
        let mut best: Option<(f64, Pos)> = None;
        for n in g.nbrs(upos) {
            if !g.can_move(uid, n) {
                continue;
            }
            let sc = score(g, n);
            if best.map(|(b, bp)| (sc, n) > (b, bp)).unwrap_or(true) {
                best = Some((sc, n));
            }
        }
        match best {
            Some((sc, n))
                if if holding_role_position {
                    sc > stay + 1e-9
                } else {
                    self.move_beats_holding(g, uid, sc, stay)
                } =>
            {
                g.apply(pid, &Action::Move { unit: uid, to: n }).is_ok()
            }
            _ => {
                // Long-range search is the fallback, not the hot path: most
                // turns keep the original cheap local tactic, while a unit at
                // a genuine obstacle can take the first safe detour step.
                let n = match g.route_step(uid, target, preferred_range) {
                    Some(n) if g.can_move(uid, n) => n,
                    _ => return false,
                };
                let routed = score(g, n) + 2.5;
                self.move_beats_holding(g, uid, routed, stay)
                    && g.apply(pid, &Action::Move { unit: uid, to: n }).is_ok()
            }
        }
    }

    pub(crate) fn move_beats_holding(
        &self,
        g: &Game,
        uid: u32,
        candidate: f64,
        holding: f64,
    ) -> bool {
        let initiative = if g.units[&uid].moved {
            0.0
        } else {
            FIRST_MOVE_SCORE_BONUS
        };
        candidate + initiative > holding + 1e-9
    }

    fn step_toward(&self, g: &mut Game, pid: usize, uid: u32, target: Pos) -> bool {
        let cur = g.units[&uid].pos;
        let mut local: Vec<Pos> = g
            .nbrs(cur)
            .into_iter()
            .filter(|p| g.can_move(uid, *p))
            .collect();
        local.sort_by_key(|p| (g.wdist(*p, target), *p));
        if let Some(next) = local.first().copied() {
            if g.wdist(next, target) < g.wdist(cur, target) {
                return g
                    .apply(
                        pid,
                        &Action::Move {
                            unit: uid,
                            to: next,
                        },
                    )
                    .is_ok();
            }
        }

        // The common case above stays as cheap as the original greedy AI;
        // invoke A* only when no legal neighbor makes geometric progress.
        let next = match g.route_step(uid, target, 0) {
            Some(p) if g.can_move(uid, p) => p,
            _ => return false,
        };
        g.apply(
            pid,
            &Action::Move {
                unit: uid,
                to: next,
            },
        )
        .is_ok()
    }

    fn settle_value(&self, g: &Game, pos: Pos) -> f64 {
        let mut total = 0.0;
        for p in g.wdisk(pos, 1) {
            if let Some(t) = g.map.get(p) {
                if t.owner_city.is_some() {
                    continue;
                }
                let ys = g.rules.tile_yields(t);
                total += ys.food * self.w.settle_food
                    + ys.production * self.w.settle_prod
                    + ys.gold * self.w.settle_gold;
            }
        }
        total
    }

    fn valid_settle_site(&self, g: &Game, pid: usize, pos: Pos) -> bool {
        let Some(tile) = g.map.get(pos) else {
            return false;
        };
        !g.rules.is_water(tile)
            && g.rules.is_passable(tile)
            && !g
                .cities
                .values()
                .any(|city| (g.wdist(city.pos, pos) as f64) < self.w.min_city_dist)
            && tile
                .owner_city
                .is_none_or(|cid| g.cities[&cid].owner == pid)
    }

    fn best_reachable_settle_site(
        &self,
        g: &Game,
        pid: usize,
        uid: u32,
        radius: i32,
    ) -> Option<(Pos, f64)> {
        let from = g.units[&uid].pos;
        let mut candidates: Vec<(f64, Pos)> = g
            .wdisk(from, radius)
            .into_iter()
            .filter(|pos| self.valid_settle_site(g, pid, *pos))
            .map(|pos| {
                let score =
                    self.settle_value(g, pos) - self.w.settle_dist * g.wdist(from, pos) as f64;
                (score, pos)
            })
            .collect();
        candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        candidates
            .into_iter()
            .take(40)
            .find(|(_, pos)| *pos == from || g.route_step(uid, *pos, 0).is_some())
            .map(|(score, pos)| (pos, score))
    }

    fn settler_step(&mut self, g: &mut Game, pid: usize, uid: u32) -> bool {
        if self.minor {
            return false; // city-states and barbarians never settle
        }
        let upos = g.units[&uid].pos;
        let current_target = self.settler_targets.get(&uid).copied().filter(|target| {
            self.valid_settle_site(g, pid, *target)
                && (*target == upos || g.route_step(uid, *target, 0).is_some())
        });
        let target = current_target.or_else(|| {
            let local_radius = if g.player_city_ids(pid).is_empty() {
                2
            } else {
                6
            };
            let local = self.best_reachable_settle_site(g, pid, uid, local_radius);
            let global = g.players[pid]
                .techs
                .contains("shipbuilding")
                .then(|| g.map.width + g.map.height)
                .and_then(|radius| self.best_reachable_settle_site(g, pid, uid, radius));
            match (local, global) {
                (Some(local), Some(global)) if global.1 > local.1 + 4.0 => Some(global),
                (Some(local), _) => Some(local),
                (None, global) => global,
            }
            .map(|(target, _)| {
                self.settler_targets.insert(uid, target);
                target
            })
        });
        let Some(target) = target else {
            self.settler_targets.remove(&uid);
            return false;
        };
        if target == upos {
            self.settler_targets.remove(&uid);
            return g.apply(pid, &Action::FoundCity { unit: uid }).is_ok();
        }
        // A linked settler is the follower: the naval military unit is the
        // formation leader and must execute movement for both. Keep the
        // destination for that leader instead of treating the follower's
        // intentionally unavailable Move action as a failed route.
        if let Some(escort) = g.units[&uid].linked_to.filter(|peer| {
            g.units.get(peer).is_some_and(|escort| {
                g.rules.units[escort.kind.as_str()].domain.as_deref() == Some("sea")
            })
        }) {
            if g.wdist(upos, target) == 1 {
                return g.apply(pid, &Action::UnlinkUnits { unit: escort }).is_ok();
            }
            return false;
        }
        let moved = self.step_toward(g, pid, uid, target);
        if !moved {
            self.settler_targets.remove(&uid);
        }
        moved
    }

    fn trader_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let upos = g.units[&uid].pos;
        if let Some(origin) = g.city_at(upos).filter(|c| g.cities[c].owner == pid) {
            // best destination: most districts in range (domestic or foreign)
            let mut best: Option<(usize, u32)> = None;
            for (cid, c) in &g.cities {
                if *cid == origin
                    || g.is_at_war(pid, c.owner)
                    || g.wdist(g.cities[&origin].pos, c.pos) > 15
                    || g.routes
                        .iter()
                        .any(|r| r.origin == origin && r.dest == *cid)
                {
                    continue;
                }
                let key = (c.districts.len() + 1, *cid);
                if best.map(|b| (key.0, key.1) > b).unwrap_or(true) {
                    best = Some(key);
                }
            }
            if let Some((_, dest)) = best {
                return g
                    .apply(
                        pid,
                        &Action::TradeRoute {
                            unit: uid,
                            city: dest,
                        },
                    )
                    .is_ok();
            }
            return false;
        }
        let target = g
            .cities
            .values()
            .filter(|c| c.owner == pid)
            .min_by_key(|c| (g.wdist(upos, c.pos), c.id))
            .map(|c| c.pos);
        match target {
            Some(t) => self.step_toward(g, pid, uid, t),
            None => false,
        }
    }

    fn missionary_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let religion = match g.players[pid].religion.clone() {
            Some(r) => r,
            None => return false,
        };
        let upos = g.units[&uid].pos;
        let target = g
            .cities
            .values()
            .filter(|c| g.city_religion(c) != Some(religion.as_str()) && !g.is_at_war(pid, c.owner))
            .min_by_key(|c| (g.wdist(upos, c.pos), c.id))
            .map(|c| c.pos);
        let target = match target {
            Some(t) => t,
            None => return false,
        };
        if g.wdist(upos, target) <= 1 {
            return g.apply(pid, &Action::Spread { unit: uid }).is_ok();
        }
        self.step_toward(g, pid, uid, target)
    }

    pub(crate) fn rock_band_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        if g.rock_concert_tourism(pid, uid).is_some() {
            return g.apply(pid, &Action::PerformConcert { unit: uid }).is_ok();
        }
        let current = g.units[&uid].pos;
        let mut venues: Vec<(f64, i32, Pos)> = g
            .map
            .tiles
            .keys()
            .copied()
            .filter_map(|position| {
                let tourism = g.rock_concert_ai_value(pid, uid, position)?;
                g.route_step(uid, position, 0)?;
                Some((tourism, g.wdist(current, position), position))
            })
            .collect();
        venues.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap()
                .then(left.1.cmp(&right.1))
                .then(left.2.cmp(&right.2))
        });
        let Some((_, _, target)) = venues.first().copied() else {
            return false;
        };
        let Some(next) = g.route_step(uid, target, 0) else {
            return false;
        };
        g.apply(
            pid,
            &Action::Move {
                unit: uid,
                to: next,
            },
        )
        .is_ok()
    }

    fn builder_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let upos = g.units[&uid].pos;
        let project = g
            .player_city_ids(pid)
            .into_iter()
            .filter_map(|city| {
                g.project_contribution_target(pid, city)
                    .map(|position| (g.wdist(upos, position), position, city))
            })
            .min();
        if let Some((_, position, city)) = project {
            if upos == position && g.can_contribute_project(pid, uid, city) {
                return g
                    .apply(pid, &Action::ContributeProject { unit: uid, city })
                    .is_ok();
            }
            if self.step_toward(g, pid, uid, position) {
                return true;
            }
        }
        let repairable = g.map.get(upos).is_some_and(|tile| {
            tile.pillaged
                && tile.improvement.is_some()
                && tile
                    .owner_city
                    .and_then(|city| g.cities.get(&city))
                    .is_some_and(|city| city.owner == pid)
        });
        if repairable {
            return g
                .apply(pid, &Action::RepairImprovement { unit: uid })
                .is_ok();
        }
        let imps: Vec<String> = g
            .valid_improvements(pid, upos)
            .into_iter()
            .filter(|improvement| g.rules.improvements[improvement].builder_buildable)
            .collect();
        if !imps.is_empty() {
            return g
                .apply(
                    pid,
                    &Action::Improve {
                        unit: uid,
                        improvement: imps[0].clone(),
                    },
                )
                .is_ok();
        }
        let mut best: Option<(i32, Pos)> = None;
        for cid in g.player_city_ids(pid) {
            for pos in g.cities[&cid].owned_tiles.clone() {
                if g.valid_improvements(pid, pos)
                    .iter()
                    .any(|improvement| g.rules.improvements[improvement].builder_buildable)
                {
                    let d = g.wdist(upos, pos);
                    if best.map(|b| (d, pos) < b).unwrap_or(true) {
                        best = Some((d, pos));
                    }
                }
            }
        }
        match best {
            Some((_, pos)) => self.step_toward(g, pid, uid, pos),
            None => false,
        }
    }

    pub(crate) fn military_engineer_step(&mut self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let current = g.units[&uid].pos;
        let target = g
            .player_city_ids(pid)
            .into_iter()
            .filter_map(|city| {
                let position = g.district_contribution_target(pid, city)?;
                if position != current {
                    g.route_step(uid, position, 0)?;
                }
                Some((g.wdist(current, position), position, city))
            })
            .min();
        let Some((_, position, city)) = target else {
            return self.military_step(g, pid, uid);
        };
        if current == position && g.can_contribute_district(pid, uid, city) {
            return g
                .apply(pid, &Action::ContributeDistrict { unit: uid, city })
                .is_ok();
        }
        self.step_toward(g, pid, uid, position)
    }

    fn naturalist_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let current = g.units[&uid].pos;
        if g.valid_improvements(pid, current)
            .iter()
            .any(|improvement| improvement == "national_park")
        {
            return g
                .apply(
                    pid,
                    &Action::Improve {
                        unit: uid,
                        improvement: "national_park".to_string(),
                    },
                )
                .is_ok();
        }
        let target = g
            .national_park_sites(pid)
            .into_iter()
            .filter_map(|site| {
                let appeal = site
                    .iter()
                    .map(|position| g.tile_appeal(*position).max(0))
                    .sum::<i32>();
                site.into_iter()
                    .filter(|position| g.rules.is_passable(&g.map.tiles[position]))
                    .filter(|position| g.route_step(uid, *position, 0).is_some())
                    .min_by_key(|position| (g.wdist(current, *position), *position))
                    .map(|position| {
                        (
                            appeal,
                            std::cmp::Reverse(g.wdist(current, position)),
                            position,
                        )
                    })
            })
            .max();
        target.is_some_and(|(_, _, position)| self.step_toward(g, pid, uid, position))
    }

    fn archaeologist_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let current = g.units[&uid].pos;
        if let Some(improvement) = g
            .valid_improvements(pid, current)
            .into_iter()
            .find(|name| matches!(name.as_str(), "archaeological_dig" | "shipwreck_excavation"))
        {
            return g
                .apply(
                    pid,
                    &Action::Improve {
                        unit: uid,
                        improvement,
                    },
                )
                .is_ok();
        }
        let target = g
            .excavation_sites(pid)
            .into_iter()
            .filter(|(position, _)| g.route_step(uid, *position, 0).is_some())
            .min_by_key(|(position, improvement)| {
                (
                    g.wdist(current, *position),
                    improvement == "shipwreck_excavation",
                    *position,
                )
            })
            .map(|(position, _)| position);
        target.is_some_and(|position| self.step_toward(g, pid, uid, position))
    }

    fn is_enemy_tile(&self, g: &Game, pos: Pos, enemy_ids: &[usize]) -> bool {
        for oid in g.units_at(pos) {
            if enemy_ids.contains(&g.units[&oid].owner) {
                return true;
            }
        }
        if let Some(cid) = g.city_at(pos) {
            return enemy_ids.contains(&g.cities[&cid].owner);
        }
        false
    }

    /// Chess-style static exchange evaluation: expected damage traded if we
    /// attack `pos` (combat model: 30·e^((att−def)/25), sans rng).
    fn exchange_score(&self, g: &Game, uid: u32, pos: Pos, ranged: bool) -> f64 {
        let u = &g.units[&uid];
        let att = effective_strength(g.unit_strength(u, false), u.hp);
        if let Some(cid) = g.city_at(pos) {
            let c = &g.cities[&cid];
            if c.owner != u.owner {
                // cities: press wounded ones, big bonus on a capturable one
                let mut s = 20.0 + 0.5 * (100 - c.hp) as f64;
                if !ranged && c.hp <= 40 && c.wall_hp <= 0 {
                    s += self.w.kill_bonus;
                }
                return s;
            }
        }
        let defender = g
            .units_at(pos)
            .into_iter()
            .map(|oid| &g.units[&oid])
            .filter(|o| g.rules.units[o.kind.as_str()].class == "military")
            .max_by(|a, b| {
                effective_strength(g.unit_strength(a, true), a.hp)
                    .partial_cmp(&effective_strength(g.unit_strength(b, true), b.hp))
                    .unwrap()
            });
        let o = match defender {
            None => return 15.0 + self.w.kill_bonus * 0.5, // undefended civilians
            Some(o) => o,
        };
        let def = effective_strength(g.unit_strength(o, true), o.hp);
        let deal = 30.0 * ((att - def) / 25.0).exp();
        let mut s = deal.min(o.hp as f64);
        if deal >= o.hp as f64 {
            s += self.w.kill_bonus;
        } else if !ranged {
            let their_att = effective_strength(g.unit_strength(o, false), o.hp);
            let my_def = effective_strength(g.unit_strength(u, true), u.hp);
            let recv = 30.0 * ((their_att - my_def) / 25.0).exp();
            s -= self.w.trade_caution * recv.min(u.hp as f64);
            if recv >= u.hp as f64 {
                s -= 35.0; // don't suicide into a counter
            }
        }
        s
    }

    fn nearest_enemy(&self, g: &Game, pid: usize, pos: Pos, enemy_ids: &[usize]) -> Option<Pos> {
        // Majors chase barbarians (and their camps) only near their own
        // territory; wars against civs have no leash.
        let my_cities: Vec<Pos> = g
            .cities
            .values()
            .filter(|c| c.owner == pid)
            .map(|c| c.pos)
            .collect();
        let near_home = |tpos: Pos| -> bool {
            if self.barb || my_cities.is_empty() {
                return true;
            }
            my_cities.iter().map(|c| g.wdist(tpos, *c)).min().unwrap() <= 6
        };
        let mut best: Option<(i32, Pos)> = None;
        for c in g.cities.values() {
            if enemy_ids.contains(&c.owner) {
                let d = g.wdist(pos, c.pos);
                if best.map(|b| (d, c.pos) < b).unwrap_or(true) {
                    best = Some((d, c.pos));
                }
            }
        }
        for u in g.units.values() {
            if enemy_ids.contains(&u.owner) {
                if Some(u.owner) == g.barb_pid && !near_home(u.pos) {
                    continue;
                }
                let d = g.wdist(pos, u.pos);
                if best.map(|b| (d, u.pos) < b).unwrap_or(true) {
                    best = Some((d, u.pos));
                }
            }
        }
        if !self.barb {
            if let Some(bp) = g.barb_pid {
                if enemy_ids.contains(&bp) {
                    for cpos in g.barb_camps.keys() {
                        if near_home(*cpos) {
                            let d = g.wdist(pos, *cpos);
                            if best.map(|b| (d, *cpos) < b).unwrap_or(true) {
                                best = Some((d, *cpos));
                            }
                        }
                    }
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// Naval forces should not select an attractive but unreachable inland
    /// target. Waterborne enemies (including embarked land units) come first,
    /// followed by coastal cities that melee ships can actually capture.
    pub(crate) fn nearest_enemy_for_unit(
        &self,
        g: &Game,
        pid: usize,
        uid: u32,
        enemy_ids: &[usize],
    ) -> Option<Pos> {
        let unit = &g.units[&uid];
        if g.rules.units[unit.kind.as_str()].domain.as_deref() != Some("sea") {
            return self.nearest_enemy(g, pid, unit.pos, enemy_ids);
        }
        g.units
            .values()
            .filter(|enemy| enemy_ids.contains(&enemy.owner) && Self::waterborne(g, enemy.id))
            .map(|enemy| (g.wdist(unit.pos, enemy.pos), 0, enemy.pos))
            .chain(
                g.cities
                    .values()
                    .filter(|city| {
                        enemy_ids.contains(&city.owner) && Self::city_is_coastal(g, city.id)
                    })
                    .map(|city| (g.wdist(unit.pos, city.pos), 1, city.pos)),
            )
            .min()
            .map(|(_, _, pos)| pos)
    }

    /// Objective for a ship assigned to colony protection. A linked ship
    /// leads the formation toward the settler's persistent colony site; an
    /// unlinked ship first closes on the embarked settler so they can link on
    /// a later command phase.
    pub(crate) fn naval_approach(g: &Game, uid: u32, target: Pos) -> Option<Pos> {
        let current = g.units.get(&uid)?.pos;
        let mut approaches: Vec<Pos> = g
            .nbrs(target)
            .into_iter()
            .filter(|pos| g.unit_can_traverse(uid, *pos))
            .collect();
        approaches.sort_by_key(|pos| (g.wdist(current, *pos), *pos));
        approaches
            .into_iter()
            .find(|pos| *pos == current || g.route_step(uid, *pos, 0).is_some())
    }

    fn naval_escort_objective(&self, g: &Game, pid: usize, uid: u32) -> Option<Pos> {
        let unit = &g.units[&uid];
        if g.rules.units[unit.kind.as_str()].domain.as_deref() != Some("sea") {
            return None;
        }
        if let Some(settler) = unit.linked_to.filter(|peer| {
            g.units
                .get(peer)
                .is_some_and(|peer| peer.owner == pid && peer.kind == "settler")
        }) {
            return self
                .settler_targets
                .get(&settler)
                .copied()
                .and_then(|target| Self::naval_approach(g, uid, target))
                .or_else(|| Some(g.units[&settler].pos));
        }
        g.units
            .values()
            .filter(|settler| {
                settler.owner == pid
                    && settler.kind == "settler"
                    && settler.linked_to.is_none()
                    && g.map
                        .get(settler.pos)
                        .is_some_and(|tile| g.rules.is_water(tile))
            })
            .min_by_key(|settler| (g.wdist(unit.pos, settler.pos), settler.id))
            .map(|settler| settler.pos)
    }

    fn explore_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let upos = g.units[&uid].pos;
        let goals: HashSet<Pos> = g
            .map
            .tiles
            .iter()
            .filter(|(pos, _)| {
                !g.players[pid].explored.contains(pos) && g.unit_can_traverse(uid, **pos)
            })
            .map(|(pos, _)| *pos)
            .collect();
        let nearest = goals
            .iter()
            .min_by_key(|pos| (g.wdist(upos, **pos), **pos))
            .copied();
        if let Some(target) = nearest {
            if self.step_toward(g, pid, uid, target) {
                return true;
            }
        }

        // If the geometrically nearest hidden tile was unreachable, search
        // for the nearest hidden tile by actual traversable route instead.
        let next = match g.route_step_to_any(uid, &goals) {
            Some(p) if g.can_move(uid, p) => p,
            _ => return false,
        };
        g.apply(
            pid,
            &Action::Move {
                unit: uid,
                to: next,
            },
        )
        .is_ok()
    }

    fn patrol_tile(&self, g: &Game, pid: usize, uid: u32, pos: Pos) -> bool {
        let Some(tile) = g.map.get(pos) else {
            return false;
        };
        if !g.unit_can_traverse(uid, pos) {
            return false;
        }
        let sea_unit = g.rules.units[g.units[&uid].kind.as_str()].domain.as_deref() == Some("sea");
        let water = g.rules.is_water(tile);
        if sea_unit != water {
            return false;
        }
        tile.owner_city
            .and_then(|cid| g.cities.get(&cid))
            .is_some_and(|city| city.owner == pid)
    }

    /// Move an otherwise idle military unit between useful frontier posts.
    /// Targets persist across turns, avoiding random-looking oscillation; a
    /// new post is selected only after the old one is reached or invalidated.
    fn patrol_step(&mut self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let current = g.units[&uid].pos;
        let previous = self.patrol_targets.get(&uid).copied();
        if let Some(target) = previous {
            if target != current && self.patrol_tile(g, pid, uid, target) {
                if let Some(next) = g
                    .route_step(uid, target, 0)
                    .filter(|pos| g.can_move(uid, *pos))
                {
                    return g
                        .apply(
                            pid,
                            &Action::Move {
                                unit: uid,
                                to: next,
                            },
                        )
                        .is_ok();
                }
            }
            self.patrol_targets.remove(&uid);
        }

        let mut posts: Vec<Pos> = g
            .map
            .tiles
            .keys()
            .copied()
            .filter(|pos| self.patrol_tile(g, pid, uid, *pos))
            .filter(|pos| {
                // A frontier post borders land or water outside this empire.
                // Interior city centers remain fallback destinations below.
                g.nbrs(*pos).into_iter().any(|neighbor| {
                    g.map.get(neighbor).is_some_and(|tile| {
                        tile.owner_city
                            .and_then(|cid| g.cities.get(&cid))
                            .is_none_or(|city| city.owner != pid)
                    })
                })
            })
            .collect();
        if posts.is_empty() {
            posts = g
                .player_city_ids(pid)
                .into_iter()
                .map(|cid| g.cities[&cid].pos)
                .filter(|pos| self.patrol_tile(g, pid, uid, *pos))
                .collect();
        }
        posts.sort_unstable();
        posts.dedup();
        if posts.is_empty() {
            return false;
        }

        let start = previous
            .and_then(|target| posts.binary_search(&target).ok().map(|index| index + 1))
            .unwrap_or(uid as usize % posts.len());
        // Trying a bounded number of distributed posts avoids an expensive
        // all-map path search when a unit is isolated on another landmass.
        for offset in 0..posts.len().min(24) {
            let target = posts[(start + offset) % posts.len()];
            if target == current {
                continue;
            }
            let Some(next) = g
                .route_step(uid, target, 0)
                .filter(|pos| g.can_move(uid, *pos))
            else {
                continue;
            };
            self.patrol_targets.insert(uid, target);
            return g
                .apply(
                    pid,
                    &Action::Move {
                        unit: uid,
                        to: next,
                    },
                )
                .is_ok();
        }
        false
    }

    fn healing_step(&mut self, g: &mut Game, pid: usize, uid: u32) -> Option<bool> {
        let withdraw_at_hp = self.w.withdraw_hp.round() as i32;
        let return_at_hp = self.w.rejoin_hp.max(self.w.withdraw_hp + 5.0).round() as i32;

        let hp = g.units[&uid].hp;
        if hp >= return_at_hp {
            self.recovering_units.remove(&uid);
            return None;
        }
        if hp <= withdraw_at_hp {
            self.recovering_units.insert(uid);
        }
        if !self.recovering_units.contains(&uid) {
            return None;
        }

        // Once safely inside friendly borders, spending the turn stationary
        // is faster than sacrificing another healing tick to chase a city.
        if g.unit_heal_rate(uid) >= 15 {
            return Some(self.fortify_or_stop(g, pid, uid));
        }

        let friendly_tiles: HashSet<Pos> = g
            .map
            .tiles
            .keys()
            .filter(|pos| g.healing_location(pid, **pos).rate() >= 15)
            .copied()
            .collect();
        if let Some(next) = g
            .route_step_to_any(uid, &friendly_tiles)
            .filter(|pos| g.can_move(uid, *pos))
        {
            return Some(
                g.apply(
                    pid,
                    &Action::Move {
                        unit: uid,
                        to: next,
                    },
                )
                .is_ok(),
            );
        }

        // If home is unreachable (for example, an isolated naval unit), wait
        // and use the neutral/enemy rate instead of continuing a bad attack.
        Some(self.fortify_or_stop(g, pid, uid))
    }

    fn military_step(&mut self, g: &mut Game, pid: usize, uid: u32) -> bool {
        if let Some(acted) = self.healing_step(g, pid, uid) {
            return acted;
        }
        let upos = g.units[&uid].pos;
        let spec = g.rules.units[g.units[&uid].kind.as_str()].clone();
        let doctrine = Self::unit_doctrine(g, uid);
        if let Some(action) = self.doctrine_action(g, pid, uid) {
            return g.apply(pid, &action).is_ok();
        }
        if matches!(doctrine, UnitDoctrine::AirDefense | UnitDoctrine::AirStrike) {
            return false;
        }
        let enemy_ids: Vec<usize> = g
            .players
            .iter()
            .filter(|o| o.id != pid && o.alive && g.is_at_war(pid, o.id))
            .map(|o| o.id)
            .collect();
        if !enemy_ids.is_empty() {
            self.patrol_targets.remove(&uid);
            // Pick the best role-adjusted exchange among all attackable tiles.
            // A scout needs a clear opportunity; an assault unit presses a
            // thinner edge, and siege spends its attacks on districts.
            let ranged = spec.has_ranged_attack();
            let radius = if ranged { spec.range.max(1) } else { 1 };
            let mut best: Option<(f64, Pos)> = None;
            for pos in g.wdisk(upos, radius) {
                if pos == upos
                    || g.map.get(pos).is_none()
                    || !self.is_enemy_tile(g, pos, &enemy_ids)
                {
                    continue;
                }
                let utility =
                    self.exchange_score(g, uid, pos, ranged) - self.attack_threshold(g, uid, pos);
                if best
                    .map(|(old, old_pos)| (utility, pos) > (old, old_pos))
                    .unwrap_or(true)
                {
                    best = Some((utility, pos));
                }
            }
            if let Some((utility, pos)) = best {
                if utility > 0.0 {
                    let act = if ranged {
                        Action::Ranged {
                            unit: uid,
                            target: pos,
                        }
                    } else {
                        Action::Attack {
                            unit: uid,
                            target: pos,
                        }
                    };
                    if g.apply(pid, &act).is_ok() {
                        return true;
                    }
                }
            }
            let hostile_water_unit = g
                .units
                .values()
                .any(|enemy| enemy_ids.contains(&enemy.owner) && Self::waterborne(g, enemy.id));
            if !hostile_water_unit {
                if let Some(target) = self.naval_escort_objective(g, pid, uid) {
                    if target != upos && self.step_toward(g, pid, uid, target) {
                        return true;
                    }
                    if g.units[&uid].linked_to.is_some_and(|peer| {
                        g.units
                            .get(&peer)
                            .is_some_and(|unit| unit.kind == "settler")
                    }) {
                        return self.fortify_or_stop(g, pid, uid);
                    }
                }
            }
            if doctrine == UnitDoctrine::Recon
                && self.should_explore(g, pid, uid, true)
                && self.explore_step(g, pid, uid)
            {
                return true;
            }
            return match self.nearest_enemy_for_unit(g, pid, uid, &enemy_ids) {
                Some(t) => self.tactical_step(g, pid, uid, t, &enemy_ids, radius),
                None => self.fortify_or_stop(g, pid, uid),
            };
        }
        // peace: minors guard home; majors explore, then garrison
        if self.minor {
            let cities = g.player_city_ids(pid);
            if cities.is_empty() {
                return false;
            }
            let cap = g.cities[&cities[0]].pos;
            if g.wdist(upos, cap) > 2 {
                return self.step_toward(g, pid, uid, cap);
            }
            return self.fortify_or_stop(g, pid, uid);
        }
        if let Some(target) = self.naval_escort_objective(g, pid, uid) {
            if target != upos && self.step_toward(g, pid, uid, target) {
                return true;
            }
            if g.units[&uid].linked_to.is_some_and(|peer| {
                g.units
                    .get(&peer)
                    .is_some_and(|unit| unit.kind == "settler")
            }) {
                return self.fortify_or_stop(g, pid, uid);
            }
        }
        if self.should_explore(g, pid, uid, false) && self.explore_step(g, pid, uid) {
            return true;
        }
        if self.patrol_step(g, pid, uid) {
            return true;
        }
        self.fortify_or_stop(g, pid, uid)
    }

    fn fortify_or_stop(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        if !g.units[&uid].fortified {
            let _ = g.apply(pid, &Action::Fortify { unit: uid });
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn island_colony_game(players: usize) -> (Game, Pos, Pos) {
        let mut g = Game::new_full(players, 18, 10, 91, 120, 0, false);
        let founding_settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| g.units[uid].kind == "settler")
            .unwrap();
        let source = g.units[&founding_settler].pos;
        let target = g
            .map
            .tiles
            .keys()
            .copied()
            .max_by_key(|pos| (g.wdist(source, *pos), *pos))
            .expect("map has a tile");
        assert!(g.wdist(source, target) > 6);
        for tile in g.map.tiles.values_mut() {
            tile.terrain = "coast".to_string();
            tile.feature = None;
            tile.hills = false;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.wonder = None;
            tile.owner_city = None;
            tile.cliff_edges = [false; 6];
        }
        g.map.tiles.get_mut(&source).unwrap().terrain = "plains".to_string();
        g.map.tiles.get_mut(&target).unwrap().terrain = "grassland".to_string();
        g.apply(
            0,
            &Action::FoundCity {
                unit: founding_settler,
            },
        )
        .unwrap();
        (g, source, target)
    }

    fn grant_tech_with_prerequisites(g: &mut Game, pid: usize, tech: &str) {
        let prerequisites = g.rules.techs[tech].requires.clone();
        for prerequisite in prerequisites {
            grant_tech_with_prerequisites(g, pid, &prerequisite);
        }
        g.players[pid].techs.insert(tech.to_string());
    }

    #[test]
    fn coastal_empires_research_navigation_before_generic_land_unlocks() {
        let (mut g, _, _) = island_colony_game(1);
        g.players[0].research = None;
        let ai = BasicAi::new();
        ai.research(&mut g, 0);
        assert_eq!(g.players[0].research.as_deref(), Some("sailing"));
    }

    #[test]
    fn naval_wars_prioritize_the_next_fleet_upgrade() {
        let (mut g, source, _) = island_colony_game(2);
        grant_tech_with_prerequisites(&mut g, 0, "cartography");
        grant_tech_with_prerequisites(&mut g, 0, "celestial_navigation");
        g.at_war.insert((0, 1));
        let contact = g
            .nbrs(source)
            .into_iter()
            .find(|pos| g.map.get(*pos).is_some_and(|tile| g.rules.is_water(tile)))
            .unwrap();
        g.spawn_test_unit("galley", 1, contact);
        g.players[0].research = None;
        assert_eq!(
            BasicAi::water_research_goal(&g, 0),
            Some("square_rigging"),
            "available={:?}, war={}, enemy_alive={}",
            g.available_techs(0),
            g.is_at_war(0, 1),
            g.players[1].alive
        );
        assert!(
            g.available_techs(0)
                .iter()
                .any(|tech| tech == "square_rigging"),
            "available={:?}",
            g.available_techs(0)
        );
        let available = g.available_techs(0);
        BasicAi::new().research(&mut g, 0);
        assert_eq!(
            g.players[0].research.as_deref(),
            Some("square_rigging"),
            "available before selection: {available:?}"
        );
    }

    #[test]
    fn coastal_cities_build_a_melee_ship_for_exploration_and_capture() {
        let (mut g, _, _) = island_colony_game(1);
        g.players[0].techs.insert("sailing".to_string());
        let cid = g.player_city_ids(0)[0];
        let ai = BasicAi::new();
        let item = ai
            .pick_item(&g, 0, cid, 1, 0, 2, 1, 4, 2, 2)
            .expect("coastal city has a production choice");
        assert!(matches!(item, Item::Unit { unit } if unit == "galley"));
    }

    #[test]
    fn coastal_cities_add_ranged_firepower_after_the_melee_screen() {
        let (mut g, source, _) = island_colony_game(2);
        g.players[0]
            .techs
            .extend(["sailing".to_string(), "shipbuilding".to_string()]);
        g.at_war.insert((0, 1));
        let water = g
            .nbrs(source)
            .into_iter()
            .find(|pos| g.map.get(*pos).is_some_and(|tile| g.rules.is_water(tile)))
            .unwrap();
        g.spawn_test_unit("galley", 0, water);
        let enemy_water = g
            .nbrs(water)
            .into_iter()
            .find(|pos| g.map.get(*pos).is_some_and(|tile| g.rules.is_water(tile)))
            .unwrap();
        g.spawn_test_unit("galley", 1, enemy_water);
        let cid = g.player_city_ids(0)[0];
        let item = BasicAi::new()
            .pick_item(&g, 0, cid, 1, 0, 2, 1, 5, 3, 2)
            .expect("coastal city has a production choice");
        assert!(matches!(item, Item::Unit { unit } if unit == "quadrireme"));
    }

    #[test]
    fn repeatable_district_projects_do_not_preempt_basic_infrastructure() {
        let mut g = Game::new_full(1, 24, 16, 91_771, 120, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| g.units[unit].kind == "settler")
            .unwrap();
        g.found_city_for(0, g.units[&settler].pos, None);
        let city = g.player_city_ids(0)[0];
        g.cities
            .get_mut(&city)
            .unwrap()
            .buildings
            .retain(|building| building != "monument");
        for position in g.nbrs(g.cities[&city].pos) {
            g.map.tiles.get_mut(&position).unwrap().terrain = "plains".to_string();
        }
        let campus = g.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != g.cities[&city].pos)
            .unwrap();
        g.map.tiles.get_mut(&campus).unwrap().district = Some("campus".to_string());
        g.cities
            .get_mut(&city)
            .unwrap()
            .districts
            .insert("campus".to_string(), campus);
        let grants = Item::Project {
            project: "campus_research_grants".to_string(),
        };
        assert!(g.can_produce(0, city, &grants));

        let item = BasicAi::new()
            .pick_item(&g, 0, city, 8, 2, 20, 10, 20, 10, 10)
            .expect("developing city has a production choice");
        assert!(
            matches!(item, Item::Building { ref building } if building == "monument"),
            "repeatable project displaced {item:?}"
        );
    }

    #[test]
    fn settler_keeps_a_reachable_colony_target_across_water() {
        let (mut g, source, target) = island_colony_game(1);
        g.players[0]
            .techs
            .extend(["sailing".to_string(), "shipbuilding".to_string()]);
        let settler = g.spawn_test_unit("settler", 0, source);
        let mut ai = BasicAi::new();

        assert!(ai.settler_step(&mut g, 0, settler));
        assert_eq!(ai.settler_targets.get(&settler), Some(&target));
        assert!(g
            .map
            .get(g.units[&settler].pos)
            .is_some_and(|tile| g.rules.is_water(tile)));
    }

    #[test]
    fn naval_escorts_link_to_embarked_settlers() {
        let (mut g, source, _) = island_colony_game(1);
        g.players[0]
            .techs
            .extend(["sailing".to_string(), "shipbuilding".to_string()]);
        let water = g
            .nbrs(source)
            .into_iter()
            .find(|pos| g.map.get(*pos).is_some_and(|tile| g.rules.is_water(tile)))
            .unwrap();
        let settler = g.spawn_test_unit("settler", 0, water);
        let galley = g.spawn_test_unit("galley", 0, water);
        BasicAi::new().prepare_unit_formations(&mut g, 0);
        assert_eq!(g.units[&galley].linked_to, Some(settler));
        assert_eq!(g.units[&settler].linked_to, Some(galley));
    }

    #[test]
    fn linked_ship_leads_settler_toward_the_persistent_colony_target() {
        let (mut g, source, target) = island_colony_game(1);
        g.players[0]
            .techs
            .extend(["sailing".to_string(), "shipbuilding".to_string()]);
        let settler = g.spawn_test_unit("settler", 0, source);
        let galley = g.spawn_test_unit("galley", 0, source);
        let mut ai = BasicAi::new();
        ai.prepare_unit_formations(&mut g, 0);

        assert!(!ai.settler_step(&mut g, 0, settler));
        assert_eq!(ai.settler_targets.get(&settler), Some(&target));
        assert!(ai.military_step(&mut g, 0, galley));
        assert_eq!(g.units[&galley].pos, g.units[&settler].pos);
        assert!(g
            .map
            .get(g.units[&galley].pos)
            .is_some_and(|tile| g.rules.is_water(tile)));
    }

    #[test]
    fn escorted_settler_unlinks_at_the_destination_coast_and_founds_the_colony() {
        let (mut g, source, target) = island_colony_game(1);
        g.players[0]
            .techs
            .extend(["sailing".to_string(), "shipbuilding".to_string()]);
        let settler = g.spawn_test_unit("settler", 0, source);
        let galley = g.spawn_test_unit("galley", 0, source);
        let mut ai = BasicAi::new();
        ai.prepare_unit_formations(&mut g, 0);

        for _ in 0..12 {
            for uid in [settler, galley] {
                if let Some(unit) = g.units.get_mut(&uid) {
                    unit.moves_left = 4.0;
                    unit.attacks_left = 1;
                    unit.acted = false;
                    unit.moved = false;
                    unit.fortified = false;
                }
            }
            for _ in 0..8 {
                if !g.units.contains_key(&settler)
                    || g.units[&settler].moves_left <= 0.0
                    || !ai.settler_step(&mut g, 0, settler)
                {
                    break;
                }
            }
            for _ in 0..8 {
                if !g.units.contains_key(&galley)
                    || g.units[&galley].moves_left <= 0.0
                    || !ai.military_step(&mut g, 0, galley)
                {
                    break;
                }
            }
            if !g.units.contains_key(&settler) {
                break;
            }
        }

        assert!(!g.units.contains_key(&settler));
        assert!(g
            .city_at(target)
            .is_some_and(|cid| g.cities[&cid].owner == 0));
    }

    #[test]
    fn ships_intercept_embarked_enemies_instead_of_chasing_inland_targets() {
        let (mut g, source, target) = island_colony_game(2);
        g.at_war.insert((0, 1));
        let water = g
            .nbrs(source)
            .into_iter()
            .find(|pos| g.map.get(*pos).is_some_and(|tile| g.rules.is_water(tile)))
            .unwrap();
        let galley = g.spawn_test_unit("galley", 0, water);
        let enemy_water = g
            .nbrs(water)
            .into_iter()
            .find(|pos| g.map.get(*pos).is_some_and(|tile| g.rules.is_water(tile)))
            .unwrap();
        let embarked = g.spawn_test_unit("settler", 1, enemy_water);
        g.spawn_test_unit("warrior", 1, target);
        let ai = BasicAi::new();
        assert_eq!(
            ai.nearest_enemy_for_unit(&g, 0, galley, &[1]),
            Some(g.units[&embarked].pos)
        );
    }

    #[test]
    fn wounded_units_withdraw_and_finish_recovering_before_rejoining() {
        let mut g = Game::new_full(2, 20, 14, 30, 30, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| g.units[uid].kind == "settler")
            .unwrap();
        let warrior = g
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| g.units[uid].kind == "warrior")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();

        let neutral = g
            .map
            .tiles
            .iter()
            .filter(|(_, tile)| {
                tile.owner_city.is_none() && g.rules.is_passable(tile) && !g.rules.is_water(tile)
            })
            .map(|(pos, _)| *pos)
            .find(|pos| {
                g.nbrs(*pos).into_iter().any(|neighbor| {
                    let tile = &g.map.tiles[&neighbor];
                    tile.owner_city.is_some()
                        && g.rules.is_passable(tile)
                        && !g.rules.is_water(tile)
                })
            })
            .expect("map has neutral land adjacent to the capital's territory");
        {
            let unit = g.units.get_mut(&warrior).unwrap();
            unit.pos = neutral;
            unit.hp = 45;
            unit.moves_left = 2.0;
            unit.acted = false;
            unit.fortified = false;
        }
        // Rebuild occupancy after placing the unit in this controlled setup.
        let snapshot = serde_json::to_value(&g).unwrap();
        let mut g: Game = serde_json::from_value(snapshot).unwrap();
        let mut ai = BasicAi::new();

        assert_eq!(ai.healing_step(&mut g, 0, warrior), Some(true));
        assert!(
            g.unit_heal_rate(warrior) >= 15,
            "unit should seek friendly borders"
        );
        assert!(ai.recovering_units.contains(&warrior));

        // Once safe, it waits instead of immediately marching back out.
        assert_eq!(ai.healing_step(&mut g, 0, warrior), Some(false));
        assert!(g.units[&warrior].fortified);
        g.units.get_mut(&warrior).unwrap().hp = 79;
        assert_eq!(ai.healing_step(&mut g, 0, warrior), Some(false));

        // Recovery mode has hysteresis and releases the unit at 80 HP.
        g.units.get_mut(&warrior).unwrap().hp = 80;
        assert_eq!(ai.healing_step(&mut g, 0, warrior), None);
        assert!(!ai.recovering_units.contains(&warrior));
    }

    #[test]
    fn first_step_ties_favor_movement_but_real_positional_losses_do_not() {
        let g = Game::new_full(1, 20, 14, 34, 30, 0, false);
        let warrior = g
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| g.units[uid].kind == "warrior")
            .unwrap();
        let ai = BasicAi::new();

        assert!(ai.move_beats_holding(&g, warrior, 10.0, 10.0));
        assert!(!ai.move_beats_holding(&g, warrior, 5.5, 10.0));

        let mut already_moved = g;
        already_moved.units.get_mut(&warrior).unwrap().moved = true;
        assert!(!ai.move_beats_holding(&already_moved, warrior, 10.0, 10.0));
    }

    #[test]
    fn most_idle_peacetime_troops_patrol_instead_of_fortifying() {
        let mut g = Game::new_full(1, 24, 16, 35, 30, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| g.units[uid].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let capital = g.cities[&g.player_city_ids(0)[0]].pos;
        let staging: Vec<Pos> = g
            .wdisk(capital, 5)
            .into_iter()
            .filter(|pos| {
                g.map.get(*pos).is_some_and(|tile| {
                    g.rules.is_passable(tile)
                        && !g.rules.is_water(tile)
                        && g.units_at(*pos).is_empty()
                })
            })
            .take(5)
            .collect();
        assert_eq!(
            staging.len(),
            5,
            "test map needs open land near the capital"
        );
        for pos in staging {
            g.spawn_test_unit("warrior", 0, pos);
        }
        g.players[0].explored.extend(g.map.tiles.keys().copied());

        let military: Vec<u32> = g
            .player_unit_ids(0)
            .into_iter()
            .filter(|uid| g.rules.units[g.units[uid].kind.as_str()].class == "military")
            .collect();
        let mut ai = BasicAi::new();
        ai.units(&mut g, 0);
        let moved = military.iter().filter(|uid| g.units[uid].moved).count();

        assert!(
            moved * 2 > military.len(),
            "expected most idle troops to patrol; moved {moved}/{}",
            military.len()
        );
    }

    #[test]
    fn most_wartime_troops_advance_when_a_campaign_route_exists() {
        let mut g = Game::new_full(2, 24, 16, 36, 30, 0, false);
        g.at_war.insert((0, 1));
        let (target, staging) = g
            .map
            .tiles
            .iter()
            .filter(|(pos, tile)| {
                g.rules.is_passable(tile) && !g.rules.is_water(tile) && g.units_at(**pos).is_empty()
            })
            .find_map(|(target, _)| {
                let staging: Vec<Pos> = g
                    .wdisk(*target, 6)
                    .into_iter()
                    .filter(|pos| {
                        (3..=6).contains(&g.wdist(*target, *pos))
                            && g.map.get(*pos).is_some_and(|tile| {
                                g.rules.is_passable(tile)
                                    && !g.rules.is_water(tile)
                                    && g.units_at(*pos).is_empty()
                            })
                    })
                    .take(6)
                    .collect();
                (staging.len() == 6).then_some((*target, staging))
            })
            .expect("test map needs an open land campaign");
        g.spawn_test_unit("warrior", 1, target);
        let army: Vec<u32> = staging
            .into_iter()
            .map(|pos| g.spawn_test_unit("warrior", 0, pos))
            .collect();

        let mut ai = BasicAi::new();
        for uid in &army {
            for _ in 0..8 {
                if !g.units.contains_key(uid)
                    || g.units[uid].moves_left <= 0.0
                    || !ai.military_step(&mut g, 0, *uid)
                {
                    break;
                }
            }
        }
        let moved = army
            .iter()
            .filter(|uid| g.units.get(uid).is_some_and(|unit| unit.moved))
            .count();
        assert!(
            moved * 2 > army.len(),
            "expected most campaign troops to advance; moved {moved}/{}",
            army.len()
        );
    }

    #[test]
    fn military_roster_maps_to_distinct_strategic_doctrines() {
        let mut g = Game::new_full(1, 24, 16, 37, 30, 0, false);
        let positions: Vec<Pos> = g
            .map
            .tiles
            .keys()
            .copied()
            .filter(|pos| g.units_at(*pos).is_empty())
            .take(9)
            .collect();
        let cases = [
            ("scout", UnitDoctrine::Recon),
            ("swordsman", UnitDoctrine::Assault),
            ("horseman", UnitDoctrine::Mobile),
            ("archer", UnitDoctrine::Ranged),
            ("catapult", UnitDoctrine::Siege),
            ("battering_ram", UnitDoctrine::Support),
            ("biplane", UnitDoctrine::AirDefense),
            ("bomber", UnitDoctrine::AirStrike),
            ("aircraft_carrier", UnitDoctrine::Carrier),
        ];
        for ((kind, expected), pos) in cases.into_iter().zip(positions) {
            let uid = g.spawn_test_unit(kind, 0, pos);
            assert_eq!(BasicAi::unit_doctrine(&g, uid), expected, "{kind}");
        }
    }

    #[test]
    fn scout_explores_while_strong_assault_unit_attacks() {
        let mut g = Game::new_full(2, 24, 16, 38, 30, 0, false);
        g.at_war.insert((0, 1));
        let (enemy_pos, scout_pos, assault_pos, hidden) = g
            .map
            .tiles
            .iter()
            .filter(|(pos, tile)| {
                g.rules.is_passable(tile) && !g.rules.is_water(tile) && g.units_at(**pos).is_empty()
            })
            .find_map(|(center, _)| {
                let ring: Vec<Pos> = g
                    .nbrs(*center)
                    .into_iter()
                    .filter(|pos| {
                        g.map.get(*pos).is_some_and(|tile| {
                            g.rules.is_passable(tile)
                                && !g.rules.is_water(tile)
                                && g.units_at(*pos).is_empty()
                        })
                    })
                    .collect();
                if ring.len() < 3 {
                    return None;
                }
                let scout = ring[0];
                let hidden = ring
                    .iter()
                    .copied()
                    .skip(1)
                    .find(|pos| g.wdist(scout, *pos) == 1)?;
                let assault = ring
                    .iter()
                    .copied()
                    .find(|pos| *pos != scout && *pos != hidden)?;
                Some((*center, scout, assault, hidden))
            })
            .expect("test map needs an open tactical ring");
        let enemy = g.spawn_test_unit("modern_armor", 1, enemy_pos);
        let scout = g.spawn_test_unit("scout", 0, scout_pos);
        let assault = g.spawn_test_unit("giant_death_robot", 0, assault_pos);
        g.players[0].explored.extend(g.map.tiles.keys().copied());
        g.players[0].explored.remove(&hidden);

        let mut ai = BasicAi::new();
        assert!(ai.military_step(&mut g, 0, scout));
        assert!(matches!(
            g.log.last(),
            Some((0, Action::Move { unit, to })) if *unit == scout && *to == hidden
        ));
        assert!(g.units.contains_key(&enemy));

        assert!(
            ai.attack_threshold(&g, assault, enemy_pos) < ai.attack_threshold(&g, scout, enemy_pos),
            "strong assault units should have a more aggressive attack threshold"
        );
        assert!(ai.military_step(&mut g, 0, assault));
        assert!(
            matches!(
                g.log.last(),
                Some((0, Action::Attack { unit, target } | Action::Ranged { unit, target }))
                    if *unit == assault && *target == enemy_pos
            ),
            "unexpected assault decision: {:?}",
            g.log.last()
        );
    }

    #[test]
    fn raiders_and_aircraft_use_their_specialized_actions() {
        let mut g = Game::new_full(2, 24, 16, 39, 30, 0, false);
        g.at_war.insert((0, 1));
        for unit in g.player_unit_ids(1) {
            g.units.get_mut(&unit).unwrap().owner = 0;
        }
        let positions: Vec<Pos> = g
            .map
            .tiles
            .iter()
            .filter(|(pos, tile)| {
                g.rules.is_passable(tile) && !g.rules.is_water(tile) && g.units_at(**pos).is_empty()
            })
            .map(|(pos, _)| *pos)
            .take(3)
            .collect();
        let air_target = g
            .nbrs(positions[2])
            .into_iter()
            .find(|pos| {
                !positions.contains(pos)
                    && g.map.get(*pos).is_some_and(|tile| {
                        g.rules.is_passable(tile)
                            && !g.rules.is_water(tile)
                            && g.units_at(*pos).is_empty()
                    })
            })
            .expect("test map needs a land target beside the air base");
        let raider = g.spawn_test_unit("horseman", 0, positions[0]);
        let assault = g.spawn_test_unit("swordsman", 0, positions[1]);
        g.map.tiles.get_mut(&positions[0]).unwrap().improvement =
            Some("barbarian_camp".to_string());
        g.map.tiles.get_mut(&positions[1]).unwrap().improvement =
            Some("barbarian_camp".to_string());
        let fighter = g.spawn_test_unit("biplane", 0, positions[2]);
        let bomber = g.spawn_test_unit("bomber", 0, positions[2]);
        g.spawn_test_unit("modern_armor", 1, air_target);
        let ai = BasicAi::new();

        let full_legal = g.legal_actions(0);
        for uid in [raider, assault, fighter, bomber] {
            let expected: Vec<Action> = full_legal
                .iter()
                .filter(|action| match action {
                    Action::Pillage { unit }
                    | Action::AirRebase { unit, .. }
                    | Action::AirStrike { unit, .. }
                    | Action::AirPatrol { unit }
                    | Action::CoastalRaid { unit, .. } => *unit == uid,
                    _ => false,
                })
                .cloned()
                .collect();
            assert_eq!(g.legal_doctrine_actions(0, uid), expected);
        }

        assert!(matches!(
            ai.doctrine_action(&g, 0, raider),
            Some(Action::Pillage { unit }) if unit == raider
        ));
        assert_eq!(ai.doctrine_action(&g, 0, assault), None);
        assert!(matches!(
            ai.doctrine_action(&g, 0, fighter),
            Some(Action::AirPatrol { unit }) if unit == fighter
        ));
        let bomber_action = ai.doctrine_action(&g, 0, bomber);
        assert!(
            matches!(
                bomber_action,
                Some(Action::AirStrike { unit, target })
                    if unit == bomber && target == air_target
            ),
            "unexpected bomber action: {bomber_action:?}"
        );
    }

    #[test]
    fn ranged_holds_firing_depth_while_mobile_unit_closes() {
        let mut g = Game::new_full(1, 24, 16, 40, 30, 0, false);
        let (target, ranged_pos, mobile_pos) = g
            .map
            .tiles
            .iter()
            .filter(|(pos, tile)| {
                g.rules.is_passable(tile) && !g.rules.is_water(tile) && g.units_at(**pos).is_empty()
            })
            .find_map(|(target, _)| {
                let ranged = g.wdisk(*target, 2).into_iter().find(|pos| {
                    g.wdist(*target, *pos) == 2
                        && g.map.get(*pos).is_some_and(|tile| {
                            g.rules.is_passable(tile)
                                && !g.rules.is_water(tile)
                                && g.units_at(*pos).is_empty()
                        })
                })?;
                let mobile = g.wdisk(*target, 4).into_iter().find(|pos| {
                    g.wdist(*target, *pos) == 4
                        && *pos != ranged
                        && g.map.get(*pos).is_some_and(|tile| {
                            g.rules.is_passable(tile)
                                && !g.rules.is_water(tile)
                                && g.units_at(*pos).is_empty()
                        })
                })?;
                Some((*target, ranged, mobile))
            })
            .expect("test map needs open role-spacing positions");
        let archer = g.spawn_test_unit("archer", 0, ranged_pos);
        let ai = BasicAi::new();
        ai.tactical_step(&mut g, 0, archer, target, &[], 2);
        assert_eq!(g.wdist(g.units[&archer].pos, target), 2);

        let horseman = g.spawn_test_unit("horseman", 0, mobile_pos);
        assert!(ai.tactical_step(&mut g, 0, horseman, target, &[], 1));
        assert!(g.wdist(g.units[&horseman].pos, target) < g.wdist(mobile_pos, target));
    }

    #[test]
    fn military_picker_preserves_city_capturing_melee() {
        let mut g = Game::new_full(1, 20, 14, 31, 30, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        g.players[0].techs.extend([
            "archery".to_string(),
            "iron_working".to_string(),
            "machinery".to_string(),
        ]);
        let cid = g.player_city_ids(0)[0];
        let ai = BasicAi::new();

        let ranged = ai.combined_arms_unit(&g, 0, cid, 2, 0).unwrap();
        assert!(g.rules.units[ranged.as_str()].has_ranged_attack());

        let melee = ai.combined_arms_unit(&g, 0, cid, 2, 2).unwrap();
        assert!(!g.rules.units[melee.as_str()].has_ranged_attack());
    }

    #[test]
    fn gold_spending_fills_worker_gap_but_keeps_reserve() {
        let mut g = Game::new_full(1, 20, 14, 32, 30, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let cid = g.player_city_ids(0)[0];
        let ai = BasicAi::new();

        // One city keeps 125 gold and spends 200 on its missing builder.
        g.players[0].gold = 325.0;
        assert!(ai.spend_gold(&mut g, 0, &[cid], 0, 0, 0, 1, 1, 0));
        assert_eq!(g.players[0].gold, 125.0);
        assert!(g
            .units
            .values()
            .any(|u| u.owner == 0 && u.kind == "builder"));

        let builders = g
            .units
            .values()
            .filter(|u| u.owner == 0 && u.kind == "builder")
            .count();
        g.players[0].gold = 324.0;
        assert!(!ai.spend_gold(&mut g, 0, &[cid], 0, 0, 0, 1, 1, 0));
        assert_eq!(g.players[0].gold, 324.0);
        assert_eq!(
            g.units
                .values()
                .filter(|u| u.owner == 0 && u.kind == "builder")
                .count(),
            builders
        );
    }

    #[test]
    fn headless_ai_spends_promotions_and_forms_unlocked_corps() {
        let mut g = Game::new_full(1, 20, 14, 33, 30, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        let veteran = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "warrior")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        g.units.get_mut(&veteran).unwrap().xp = 15;
        let ai = BasicAi::new();
        ai.prepare_unit_formations(&mut g, 0);
        assert_eq!(g.units[&veteran].level, 2);
        assert_eq!(g.units[&veteran].promotions.len(), 1);

        let pos = g
            .map
            .tiles
            .iter()
            .find(|(pos, tile)| {
                g.rules.is_passable(tile) && !g.rules.is_water(tile) && g.units_at(**pos).is_empty()
            })
            .map(|(pos, _)| *pos)
            .unwrap();
        for _ in 0..6 {
            g.spawn_test_unit("warrior", 0, pos);
        }
        g.players[0].civics.insert("nationalism".to_string());
        ai.prepare_unit_formations(&mut g, 0);
        let warriors: Vec<_> = g
            .units
            .values()
            .filter(|unit| unit.owner == 0 && unit.kind == "warrior")
            .collect();
        assert_eq!(warriors.len(), 5, "the AI keeps a five-unit reserve");
        assert!(warriors.iter().any(|unit| unit.formation == 1));
        assert!(
            warriors.iter().any(|unit| unit.promotions.len() == 1),
            "the veteran remains in the force"
        );
    }

    #[test]
    fn headless_ai_resolves_mandatory_capture_choices() {
        let mut g = Game::new_full(2, 20, 14, 34, 30, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = g.player_city_ids(0)[0];
        g.cities.get_mut(&city).unwrap().captured_from = Some(1);
        assert!(matches!(
            g.legal_actions(0).as_slice(),
            [Action::KeepCity { city: pending }] if *pending == city
        ));

        let mut ai = BasicAi::new();
        ai.resolve_city_dispositions(&mut g, 0, false, false);

        assert_eq!(g.cities[&city].captured_from, None);
        assert_eq!(g.players[1].grievances.get(&0), Some(&50.0));
    }

    #[test]
    fn builder_routes_to_a_royal_society_project_and_contributes() {
        let mut g = Game::new_full(1, 20, 14, 35, 40, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = g.player_city_ids(0)[0];
        let spaceport = g.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != g.cities[&city].pos)
            .unwrap();
        g.map.tiles.get_mut(&spaceport).unwrap().district = Some("spaceport".to_string());
        g.cities
            .get_mut(&city)
            .unwrap()
            .districts
            .insert("spaceport".to_string(), spaceport);
        g.cities
            .get_mut(&city)
            .unwrap()
            .buildings
            .push("royal_society".to_string());
        g.cities.get_mut(&city).unwrap().queue = vec![Item::Project {
            project: "launch_earth_satellite".to_string(),
        }];
        let builder = g.spawn_test_unit("builder", 0, g.cities[&city].pos);
        g.units.get_mut(&builder).unwrap().charges = 3;

        let ai = BasicAi::new();
        assert!(ai.builder_step(&mut g, 0, builder));
        assert_eq!(g.units[&builder].pos, spaceport);
        if !g.can_contribute_project(0, builder, city) {
            // Entering a wooded/hill district can spend the Builder's whole
            // movement allowance. The project action then belongs to its next
            // turn, just as it does during an ordinary AI game loop.
            let movement = g.rules.units["builder"].moves;
            let builder = g.units.get_mut(&builder).unwrap();
            builder.moves_left = movement;
            builder.moved = false;
            builder.acted = false;
        }
        assert!(ai.builder_step(&mut g, 0, builder));
        assert!(!g.units.contains_key(&builder));
        assert_eq!(g.cities[&city].production, 54.0);
    }

    #[test]
    fn headless_naturalist_routes_to_and_establishes_a_complete_park() {
        let mut g = Game::new_full(1, 20, 14, 36, 40, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = g.player_city_ids(0)[0];
        let center = g.cities[&city].pos;
        let positions = g
            .map
            .tiles
            .keys()
            .copied()
            .filter(|top| g.wdist(center, *top) > 4)
            .find_map(|top| {
                let positions = [
                    top,
                    crate::hex::canon((top.0 - 1, top.1 + 1), g.map.width),
                    crate::hex::canon((top.0, top.1 + 1), g.map.width),
                    crate::hex::canon((top.0 - 1, top.1 + 2), g.map.width),
                ];
                positions
                    .iter()
                    .all(|position| g.map.tiles.contains_key(position))
                    .then_some(positions)
            })
            .unwrap();

        let old_owned = g.cities[&city].owned_tiles.clone();
        for position in old_owned {
            g.map.tiles.get_mut(&position).unwrap().owner_city = None;
        }
        g.cities.get_mut(&city).unwrap().owned_tiles = positions.to_vec();
        for position in positions
            .iter()
            .flat_map(|position| g.nbrs(*position))
            .chain(positions)
            .collect::<std::collections::BTreeSet<_>>()
        {
            let tile = g.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
            tile.resource = None;
            tile.improvement = None;
            tile.pillaged = false;
            tile.district = None;
            tile.wonder = None;
            tile.flooded = false;
            tile.submerged = false;
        }
        for position in positions {
            let tile = g.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "mountain".to_string();
            tile.owner_city = Some(city);
        }
        g.map.tiles.get_mut(&positions[0]).unwrap().terrain = "grassland".to_string();
        g.players[0].civics.insert("conservation".to_string());
        assert_eq!(g.national_park_sites(0), vec![positions]);

        let start = g
            .nbrs(positions[0])
            .into_iter()
            .find(|position| !positions.contains(position))
            .unwrap();
        let naturalist = g.spawn_test_unit("naturalist", 0, start);
        let ai = BasicAi::new();
        assert!(ai.naturalist_step(&mut g, 0, naturalist));
        assert_eq!(g.units[&naturalist].pos, positions[0]);
        assert!(ai.naturalist_step(&mut g, 0, naturalist));
        assert!(!g.units.contains_key(&naturalist));
        assert!(positions.iter().all(|position| {
            g.map.tiles[position].improvement.as_deref() == Some("national_park")
        }));
    }

    #[test]
    fn headless_military_engineer_routes_to_and_accelerates_an_aqueduct() {
        let mut game = Game::new_full(1, 20, 14, 36_001, 80, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let center = game.cities[&city].pos;
        let site = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != center && game.units_at(*position).is_empty())
            .unwrap();
        let center_edge = game.map.direction_to(site, center).unwrap();
        {
            let tile = game.map.tiles.get_mut(&site).unwrap();
            tile.terrain = "plains".to_string();
            tile.hills = false;
            tile.feature = None;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.river_edges[(center_edge + 1) % 6] = true;
        }
        game.players[0].techs.insert("engineering".to_string());
        let aqueduct = Item::District {
            district: "aqueduct".to_string(),
            pos: site,
        };
        game.cities.get_mut(&city).unwrap().queue = vec![aqueduct.clone()];
        let district_cost = game.item_cost_for_city(0, city, &aqueduct);
        let engineer = game.spawn_test_unit("military_engineer", 0, center);
        let mut ai = BasicAi::new();

        assert!(ai.military_engineer_step(&mut game, 0, engineer));
        assert_eq!(game.units[&engineer].pos, site);
        // The river-adjacent construction tile can consume the Engineer's
        // full movement; the contribution is made after movement refreshes.
        game.units.get_mut(&engineer).unwrap().moves_left = 2.0;
        assert!(game.can_contribute_district(0, engineer, city));
        assert!(ai.military_engineer_step(&mut game, 0, engineer));
        assert!(
            (game.cities[&city].production - district_cost * 0.2).abs() < 1e-9,
            "production was {}",
            game.cities[&city].production
        );
        assert_eq!(game.units[&engineer].charges, 1);
        assert_eq!(game.units[&engineer].moves_left, 0.0);
    }

    #[test]
    fn headless_archaeologist_routes_to_and_extracts_an_artifact() {
        let mut g = Game::new_full(1, 20, 14, 37, 40, 0, false);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|id| g.units[id].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = g.player_city_ids(0)[0];
        g.cities
            .get_mut(&city)
            .unwrap()
            .buildings
            .push("archaeological_museum".to_string());
        g.players[0].civics.insert("natural_history".to_string());
        let site = g.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != g.cities[&city].pos && g.units_at(*position).is_empty())
            .unwrap();
        let tile = g.map.tiles.get_mut(&site).unwrap();
        tile.terrain = "plains".to_string();
        tile.feature = None;
        tile.resource = Some("antiquity_site".to_string());
        tile.improvement = None;
        tile.district = None;
        tile.wonder = None;
        let archaeologist = g.spawn_test_unit("archaeologist", 0, g.cities[&city].pos);

        let ai = BasicAi::new();
        assert!(ai.archaeologist_step(&mut g, 0, archaeologist));
        assert_eq!(g.units[&archaeologist].pos, site);
        assert!(ai.archaeologist_step(&mut g, 0, archaeologist));
        assert!(g.map.tiles[&site].resource.is_none());
        assert_eq!(g.players[0].counters["great_work:artifact"], 1);
        assert_eq!(g.units[&archaeologist].charges, 2);
    }
}
