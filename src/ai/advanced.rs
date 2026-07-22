//! Stateful, hierarchical AI for major civilizations.
//!
//! `BasicAi` deliberately remains the small deterministic baseline.  This
//! agent adds a shared strategic model so research, production, diplomacy,
//! civilian work, and military movement pursue the same medium-term goal.
use super::{Ai, BasicAi, UnitDoctrine, Weights};
use crate::game::{Action, CongressResolution, DiplomaticDeal, Game, Item};
use crate::rules::Yields;
use crate::Pos;
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrandStrategy {
    Expansion,
    Science,
    Culture,
    Religion,
    Diplomacy,
    Conquest,
    Recovery,
}

/// A concrete game-ending objective. Unlike `GrandStrategy`, which may
/// temporarily become Expansion or Recovery, this remains fixed for the
/// lifetime of a deliberately targeted AI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VictoryTarget {
    Science,
    Culture,
    Religion,
    Diplomacy,
    Domination,
    Score,
}

impl VictoryTarget {
    pub const ALL: [VictoryTarget; 6] = [
        VictoryTarget::Science,
        VictoryTarget::Culture,
        VictoryTarget::Religion,
        VictoryTarget::Diplomacy,
        VictoryTarget::Domination,
        VictoryTarget::Score,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            VictoryTarget::Science => "science",
            VictoryTarget::Culture => "culture",
            VictoryTarget::Religion => "religious",
            VictoryTarget::Diplomacy => "diplomatic",
            VictoryTarget::Domination => "domination",
            VictoryTarget::Score => "score",
        }
    }

    fn strategy(self) -> GrandStrategy {
        match self {
            VictoryTarget::Science => GrandStrategy::Science,
            VictoryTarget::Culture => GrandStrategy::Culture,
            VictoryTarget::Religion => GrandStrategy::Religion,
            VictoryTarget::Diplomacy => GrandStrategy::Diplomacy,
            VictoryTarget::Domination => GrandStrategy::Conquest,
            VictoryTarget::Score => GrandStrategy::Expansion,
        }
    }
}

impl std::str::FromStr for VictoryTarget {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "science" => Ok(VictoryTarget::Science),
            "culture" => Ok(VictoryTarget::Culture),
            "religion" | "religious" => Ok(VictoryTarget::Religion),
            "diplomacy" | "diplomatic" => Ok(VictoryTarget::Diplomacy),
            "domination" | "conquest" => Ok(VictoryTarget::Domination),
            "score" => Ok(VictoryTarget::Score),
            _ => Err(format!("unknown victory target {value:?}")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategicPlan {
    pub strategy: GrandStrategy,
    pub target_player: Option<usize>,
    pub target_city: Option<u32>,
    pub threatened_city: Option<u32>,
    pub desired_cities: usize,
    pub assessed_turn: u32,
}

/// Movement domain for a coordinated force. The same planner operates on
/// armies, fleets, and future domains without baking land-unit assumptions
/// into the campaign layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForceDomain {
    Land,
    Sea,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForcePosture {
    Muster,
    Advance,
    Engage,
    Hold,
    Recover,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForceRole {
    Recon,
    Vanguard,
    Mobile,
    Ranged,
    Siege,
    Support,
    AirStrike,
}

/// A deterministic, inspectable order shared by a group of nearby units.
/// `focus_target` is recomputed every turn from attacks available to the
/// entire force, preventing units from selecting unrelated victims.
#[derive(Clone, Debug, PartialEq)]
pub struct ForceGroup {
    pub id: u32,
    pub domain: ForceDomain,
    pub units: Vec<u32>,
    pub anchor: Pos,
    pub objective: Pos,
    pub focus_target: Option<Pos>,
    pub posture: ForcePosture,
    pub readiness: f64,
    pub local_strength_ratio: f64,
}

#[derive(Default)]
struct EmpireCounts {
    settlers: usize,
    builders: usize,
    traders: usize,
    scouts: usize,
    military: usize,
    melee: usize,
    ranged: usize,
    naval: usize,
    naval_melee: usize,
    naval_ranged: usize,
    naval_raider: usize,
    carriers: usize,
    aircraft: usize,
    siege: usize,
    support: usize,
    missionaries: usize,
}

#[derive(Clone, Copy)]
struct VictoryFocus {
    strategy: GrandStrategy,
    progress: i32,
}

impl EmpireCounts {
    fn add_unit(&mut self, g: &Game, name: &str) {
        match name {
            "settler" => self.settlers += 1,
            "builder" => self.builders += 1,
            "trader" => self.traders += 1,
            "missionary" => self.missionaries += 1,
            "scout" => {
                self.scouts += 1;
                self.military += 1;
                self.melee += 1;
            }
            _ => {
                let spec = &g.rules.units[name];
                if spec.class == "military" {
                    self.military += 1;
                    if spec.domain.as_deref() == Some("sea") {
                        self.naval += 1;
                        match spec.promotion_class.as_str() {
                            "naval_melee" => self.naval_melee += 1,
                            "naval_ranged" => self.naval_ranged += 1,
                            "naval_raider" => self.naval_raider += 1,
                            "naval_carrier" => self.carriers += 1,
                            _ => {}
                        }
                    } else if spec.domain.as_deref() == Some("air") {
                        self.aircraft += 1;
                    }
                    if spec.has_ranged_attack() {
                        self.ranged += 1;
                    } else {
                        self.melee += 1;
                    }
                    if spec.siege {
                        self.siege += 1;
                    }
                } else if spec.class == "support" {
                    self.support += 1;
                }
            }
        }
    }

    fn add_item(&mut self, g: &Game, item: &Item) {
        match item {
            Item::Unit { unit } | Item::Formation { unit, .. } => self.add_unit(g, unit),
            _ => {}
        }
    }
}

pub struct AdvancedAi {
    base: BasicAi,
    plan: Option<StrategicPlan>,
    settler_targets: BTreeMap<u32, Pos>,
    builder_targets: BTreeMap<u32, Pos>,
    major_war_since: Option<u32>,
    last_campaign_progress: u32,
    last_city_count: usize,
    peace_until: u32,
    victory_planning: bool,
    victory_target: Option<VictoryTarget>,
    force_groups: Vec<ForceGroup>,
}

impl Default for AdvancedAi {
    fn default() -> Self {
        Self::new()
    }
}

impl AdvancedAi {
    pub fn new() -> AdvancedAi {
        Self::configured(BasicAi::new(), true, None)
    }

    pub fn targeting(target: VictoryTarget) -> AdvancedAi {
        Self::configured(BasicAi::new(), true, Some(target))
    }

    /// Frozen control for measuring future strategic changes against the
    /// first promoted hierarchical agent rather than only against BasicAi.
    pub fn legacy() -> AdvancedAi {
        Self::configured(BasicAi::new(), false, None)
    }

    fn configured(
        base: BasicAi,
        victory_planning: bool,
        victory_target: Option<VictoryTarget>,
    ) -> AdvancedAi {
        AdvancedAi {
            base,
            plan: None,
            settler_targets: BTreeMap::new(),
            builder_targets: BTreeMap::new(),
            major_war_since: None,
            last_campaign_progress: 0,
            last_city_count: 0,
            peace_until: 0,
            victory_planning,
            victory_target,
            force_groups: Vec::new(),
        }
    }

    pub fn with_weights(weights: Weights) -> AdvancedAi {
        Self::configured(BasicAi::with_weights(weights), true, None)
    }

    pub fn with_weights_and_target(weights: Weights, target: VictoryTarget) -> AdvancedAi {
        Self::configured(BasicAi::with_weights(weights), true, Some(target))
    }

    pub fn fleet(g: &Game) -> Vec<AdvancedAi> {
        g.players.iter().map(|_| AdvancedAi::new()).collect()
    }

    pub fn fleet_targeting(g: &Game, target: VictoryTarget) -> Vec<AdvancedAi> {
        g.players
            .iter()
            .map(|_| AdvancedAi::targeting(target))
            .collect()
    }

    pub fn fleet_weighted(g: &Game, weights: &Weights) -> Vec<AdvancedAi> {
        g.players
            .iter()
            .map(|p| {
                if p.is_minor || p.is_barbarian {
                    AdvancedAi::new()
                } else {
                    AdvancedAi::with_weights(weights.clone())
                }
            })
            .collect()
    }

    pub fn current_plan(&self) -> Option<&StrategicPlan> {
        self.plan.as_ref()
    }

    pub fn victory_target(&self) -> Option<VictoryTarget> {
        self.victory_target
    }

    /// Last set of force orders produced for this agent. This is useful to
    /// observers, evaluators, and tests; orders are rebuilt at every war turn.
    pub fn force_groups(&self) -> &[ForceGroup] {
        &self.force_groups
    }

    pub fn strategy_weights(&self) -> &Weights {
        &self.base.w
    }

    pub fn coordinates_forces(&self) -> bool {
        self.victory_planning
    }

    fn observe_campaign(&mut self, g: &Game, pid: usize) {
        let cities = g.player_city_ids(pid).len();
        if cities > self.last_city_count {
            self.last_campaign_progress = g.turn;
        }
        self.last_city_count = cities;
        let major_war = g.players.iter().any(|p| {
            p.id != pid && p.alive && !p.is_minor && !p.is_barbarian && g.is_at_war(pid, p.id)
        });
        if major_war {
            self.major_war_since.get_or_insert(g.turn);
        } else {
            self.major_war_since = None;
        }
    }

    fn plan_stale(&self, g: &Game, pid: usize) -> bool {
        let Some(plan) = &self.plan else { return true };
        if g.turn.saturating_sub(plan.assessed_turn) >= 5 {
            return true;
        }
        if let Some(target) = plan.target_player {
            if !g.players.get(target).map(|p| p.alive).unwrap_or(false) {
                return true;
            }
        }
        if let Some(cid) = plan.target_city {
            if !g.cities.get(&cid).map(|c| c.owner != pid).unwrap_or(false) {
                return true;
            }
        }
        // The five-turn planning horizon keeps economic choices stable, but
        // wars and victory races are interrupts rather than ordinary inputs.
        // Waiting four more turns after a surprise attack or a rival's final
        // launch can make the eventual plan irrelevant.
        let major_wars: Vec<usize> = g
            .players
            .iter()
            .filter(|player| {
                player.id != pid
                    && player.alive
                    && !player.is_minor
                    && !player.is_barbarian
                    && g.is_at_war(pid, player.id)
            })
            .map(|player| player.id)
            .collect();
        if !major_wars.is_empty()
            && !plan
                .target_player
                .is_some_and(|target| major_wars.contains(&target))
        {
            return true;
        }
        if let Some(threatened) = self.threatened_city(g, pid) {
            if plan.threatened_city != Some(threatened) {
                return true;
            }
        }
        if let Some((rival, counter)) = self.victory_denial(g, pid) {
            if plan.target_player != Some(rival)
                || (major_wars.is_empty() && plan.strategy != counter)
            {
                return true;
            }
        }
        false
    }

    fn threatened_city(&self, g: &Game, pid: usize) -> Option<u32> {
        g.player_city_ids(pid)
            .into_iter()
            .filter_map(|cid| {
                let city = &g.cities[&cid];
                let nearby = g
                    .units
                    .values()
                    .filter(|unit| unit.owner != pid && g.is_at_war(pid, unit.owner))
                    .map(|unit| g.wdist(city.pos, unit.pos))
                    .min()
                    .unwrap_or(i32::MAX);
                let recently_hit =
                    city.last_attacked > 0 && g.turn.saturating_sub(city.last_attacked) <= 3;
                (nearby <= 6 || recently_hit).then_some((nearby, city.hp, cid))
            })
            .min()
            .map(|(_, _, cid)| cid)
    }

    fn religious_opening_viable(&self, g: &Game, pid: usize) -> bool {
        if g.players[pid].religion.is_some()
            || g.religions_founded() >= g.max_religions()
            || g.turn > 110
            || g.player_city_ids(pid).len() < 2
        {
            return false;
        }
        g.player_city_ids(pid).into_iter().any(|cid| {
            g.district_sites(cid, "holy_site")
                .into_iter()
                .any(|pos| g.district_yields("holy_site", pos).faith >= 3.0)
        })
    }

    fn victory_focus(&self, g: &Game, pid: usize) -> VictoryFocus {
        if let Some(target) = self.victory_target {
            return VictoryFocus {
                strategy: target.strategy(),
                progress: 100,
            };
        }
        if !self.victory_planning {
            return VictoryFocus {
                strategy: if g.players[pid].civ == "Greece" {
                    GrandStrategy::Culture
                } else {
                    GrandStrategy::Science
                },
                progress: 25,
            };
        }
        let player = &g.players[pid];
        let living_majors: Vec<usize> = g
            .players
            .iter()
            .filter(|p| p.alive && !p.is_minor && !p.is_barbarian)
            .map(|p| p.id)
            .collect();

        let project_progress = player.science_projects.len().min(4) as i32 * 18;
        let travel_progress = if player.science_projects.contains("exoplanet_expedition") {
            (player.exoplanet_distance * 100.0 / 50.0).clamp(0.0, 100.0) as i32
        } else {
            0
        };
        let science = project_progress.max(travel_progress).max(25);

        let culture_target = living_majors
            .iter()
            .filter(|other| **other != pid)
            .map(|other| g.domestic_tourists(*other))
            .max()
            .unwrap_or(1)
            .max(1);
        let culture = ((100 * g.foreign_tourists(pid) / culture_target).clamp(0, 100)) as i32;

        let converted = player.religion.as_ref().map_or(0, |religion| {
            living_majors
                .iter()
                .filter(|other| {
                    let cities = g.player_city_ids(**other);
                    let following = cities
                        .iter()
                        .filter(|cid| g.city_religion(&g.cities[cid]) == Some(religion.as_str()))
                        .count();
                    !cities.is_empty() && 2 * following > cities.len()
                })
                .count()
        });
        let religion = if player.religion.is_some() {
            40 + (60 * converted / living_majors.len().max(1)) as i32
        } else if self.religious_opening_viable(g, pid) {
            42
        } else {
            0
        };

        let suzerain = g
            .players
            .iter()
            .filter(|minor| {
                minor.alive
                    && minor.is_minor
                    && !minor.is_barbarian
                    && g.suzerain_of(minor.id) == Some(pid)
            })
            .count() as i64;
        let diplomacy = (player.dvp * 5 + suzerain * 6).clamp(0, 100) as i32;

        let mut best = VictoryFocus {
            strategy: GrandStrategy::Science,
            progress: science,
        };
        for candidate in [
            VictoryFocus {
                strategy: GrandStrategy::Culture,
                progress: culture.max((player.civ == "Greece") as i32 * 45),
            },
            VictoryFocus {
                strategy: GrandStrategy::Religion,
                progress: religion,
            },
            VictoryFocus {
                strategy: GrandStrategy::Diplomacy,
                progress: diplomacy,
            },
        ] {
            if candidate.progress > best.progress {
                best = candidate;
            }
        }
        best
    }

    /// Public victory-screen information distilled into a single urgency
    /// signal. Strong opponents must be judged by how close they are to ending
    /// the game, not only by how cheap their nearest city looks to capture.
    fn rival_victory_pressure(&self, g: &Game, pid: usize) -> VictoryFocus {
        let player = &g.players[pid];
        let starting_majors: Vec<usize> = g
            .players
            .iter()
            .filter(|candidate| !candidate.is_minor && !candidate.is_barbarian)
            .map(|candidate| candidate.id)
            .collect();
        let living_majors: Vec<usize> = starting_majors
            .iter()
            .copied()
            .filter(|candidate| g.players[*candidate].alive)
            .collect();

        let science = if player.science_projects.contains("exoplanet_expedition") {
            75 + (25.0 * player.exoplanet_distance / 50.0).clamp(0.0, 25.0) as i32
        } else if player.science_projects.contains("launch_mars_colony") {
            65
        } else if player.science_projects.contains("launch_moon_landing") {
            45
        } else if player.science_projects.contains("launch_earth_satellite") {
            25
        } else {
            0
        };

        let culture_target = living_majors
            .iter()
            .filter(|other| **other != pid)
            .map(|other| g.domestic_tourists(*other))
            .max()
            .unwrap_or(1)
            .max(1);
        let culture = (100 * g.foreign_tourists(pid) / culture_target).clamp(0, 100) as i32;

        let converted = player.religion.as_ref().map_or(0, |religion| {
            living_majors
                .iter()
                .filter(|other| {
                    let cities = g.player_city_ids(**other);
                    let following = cities
                        .iter()
                        .filter(|city| g.city_religion(&g.cities[city]) == Some(religion.as_str()))
                        .count();
                    !cities.is_empty() && following * 2 > cities.len()
                })
                .count()
        });
        let religion = if player.religion.is_some() {
            (100 * converted / living_majors.len().max(1)) as i32
        } else {
            0
        };
        let diplomacy = (player.dvp * 5).clamp(0, 100) as i32;

        let foreign_capitals = starting_majors
            .iter()
            .filter(|owner| **owner != pid)
            .count();
        let controlled_capitals = g
            .cities
            .values()
            .filter(|city| city.is_capital && city.original_owner != pid && city.owner == pid)
            .count();
        let domination = (100 * controlled_capitals)
            .checked_div(foreign_capitals)
            .unwrap_or(0) as i32;

        let score = if g.max_turns > 0
            && g.turn.saturating_mul(4) >= g.max_turns.saturating_mul(3)
            && living_majors
                .iter()
                .map(|candidate| g.score(*candidate))
                .max()
                == Some(g.score(pid))
        {
            (40 + 60 * g.turn.min(g.max_turns) / g.max_turns) as i32
        } else {
            0
        };

        [
            VictoryFocus {
                strategy: GrandStrategy::Science,
                progress: science,
            },
            VictoryFocus {
                strategy: GrandStrategy::Culture,
                progress: culture,
            },
            VictoryFocus {
                strategy: GrandStrategy::Religion,
                progress: religion,
            },
            VictoryFocus {
                strategy: GrandStrategy::Diplomacy,
                progress: diplomacy,
            },
            VictoryFocus {
                strategy: GrandStrategy::Conquest,
                progress: domination,
            },
            VictoryFocus {
                strategy: GrandStrategy::Expansion,
                progress: score,
            },
        ]
        .into_iter()
        .max_by_key(|focus| focus.progress)
        .unwrap()
    }

    fn victory_denial(&self, g: &Game, pid: usize) -> Option<(usize, GrandStrategy)> {
        if self.victory_target.is_some() {
            return None;
        }
        let own_progress = self.victory_focus(g, pid).progress;
        let (rival, pressure) = g
            .players
            .iter()
            .filter(|player| {
                player.id != pid && player.alive && !player.is_minor && !player.is_barbarian
            })
            .map(|player| (player.id, self.rival_victory_pressure(g, player.id)))
            .max_by(|left, right| {
                left.1
                    .progress
                    .cmp(&right.1.progress)
                    .then_with(|| right.0.cmp(&left.0))
            })?;
        if pressure.progress < 78 || pressure.progress < own_progress + 15 {
            return None;
        }
        let counter = match pressure.strategy {
            GrandStrategy::Science => GrandStrategy::Conquest,
            GrandStrategy::Culture => GrandStrategy::Culture,
            GrandStrategy::Religion if g.players[pid].religion.is_some() => GrandStrategy::Religion,
            GrandStrategy::Religion => GrandStrategy::Conquest,
            GrandStrategy::Diplomacy => GrandStrategy::Diplomacy,
            GrandStrategy::Conquest => GrandStrategy::Recovery,
            GrandStrategy::Expansion => GrandStrategy::Conquest,
            GrandStrategy::Recovery => GrandStrategy::Recovery,
        };
        Some((rival, counter))
    }

    fn assess(&self, g: &Game, pid: usize) -> StrategicPlan {
        let cities = g.player_city_ids(pid);
        let my_power = g.military_power(pid);
        let major_rivals: Vec<usize> = g
            .players
            .iter()
            .filter(|p| p.id != pid && p.alive && !p.is_minor && !p.is_barbarian)
            .map(|p| p.id)
            .collect();
        // City-states follow their Suzerain into wars and can also be attacked
        // directly. Once hostilities exist they are real campaign actors, not
        // an uncoordinated side task for whichever unit happens to be nearby.
        let wartime_rivals: Vec<usize> = g
            .players
            .iter()
            .filter(|p| p.id != pid && p.alive && !p.is_barbarian && g.is_at_war(pid, p.id))
            .map(|p| p.id)
            .collect();
        let at_war = !wartime_rivals.is_empty();
        let strongest_rival = major_rivals
            .iter()
            .map(|o| g.military_power(*o))
            .fold(0.0_f64, f64::max);
        let weakest_rival = major_rivals
            .iter()
            .map(|o| g.military_power(*o))
            .fold(f64::INFINITY, f64::min);

        let threatened_city = self.threatened_city(g, pid);

        let land = g
            .map
            .tiles
            .values()
            .filter(|t| g.rules.is_passable(t) && !g.rules.is_water(t))
            .count();
        let map_capacity = (2 + land / 55).clamp(3, 9);
        // Expansion must compound before it pays back. Add roughly one city
        // per era instead of continuously raising the target and starving a
        // young empire of districts, buildings, and population growth.
        let desired_cities = (3 + g.turn as usize / 90).min(map_capacity).min(5);
        let mut expansion_origins: Vec<Pos> = cities.iter().map(|cid| g.cities[cid].pos).collect();
        if expansion_origins.is_empty() {
            expansion_origins.extend(
                g.player_unit_ids(pid)
                    .into_iter()
                    .filter(|uid| g.units[uid].kind == "settler")
                    .map(|uid| g.units[&uid].pos),
            );
        }
        let has_site = expansion_origins.iter().any(|pos| {
            self.best_settle_site(g, pid, *pos, 10).is_some()
                || (g.players[pid].techs.contains("shipbuilding")
                    && self
                        .best_settle_site(g, pid, *pos, g.map.width + g.map.height)
                        .is_some())
        });

        let military_civ = matches!(
            g.players[pid].civ.as_str(),
            "Sumeria" | "Aztec" | "Nubia" | "Scythia"
        );
        let victory = self.victory_focus(g, pid);
        let denial = self.victory_denial(g, pid);
        let strategy = if at_war && (threatened_city.is_some() || my_power * 1.25 < strongest_rival)
        {
            GrandStrategy::Recovery
        } else if let Some(target) = self.victory_target {
            if target == VictoryTarget::Religion && g.players[pid].religion.is_none() {
                GrandStrategy::Religion
            } else if cities.len() < desired_cities && has_site && g.turn < 175 {
                GrandStrategy::Expansion
            } else {
                target.strategy()
            }
        } else if let Some((_, counter)) = denial {
            counter
        } else if at_war
            || (g.turn >= 55 && cities.len() >= 2 && my_power > weakest_rival * 1.80 + 20.0)
            || (military_civ
                && g.turn >= 35
                && cities.len() >= 2
                && my_power >= strongest_rival * 1.10)
        {
            GrandStrategy::Conquest
        } else if victory.progress >= 65 {
            victory.strategy
        } else if cities.len() < desired_cities && has_site && g.turn < 175 {
            GrandStrategy::Expansion
        } else {
            victory.strategy
        };

        // Finish wars already in progress before selecting the next major
        // rival. In particular, this gives hostile city-states an explicit
        // city objective that the force-group planner can actually consume.
        let target_player = if wartime_rivals.is_empty() {
            denial.map(|(rival, _)| rival).or_else(|| {
                major_rivals.iter().copied().min_by(|a, b| {
                    self.rival_value(g, pid, *a)
                        .partial_cmp(&self.rival_value(g, pid, *b))
                        .unwrap()
                        .then(a.cmp(b))
                })
            })
        } else {
            wartime_rivals.iter().copied().min_by(|a, b| {
                self.rival_value(g, pid, *a)
                    .partial_cmp(&self.rival_value(g, pid, *b))
                    .unwrap()
                    .then(a.cmp(b))
            })
        };
        let target_city = target_player.and_then(|target| {
            let from = cities
                .iter()
                .map(|cid| g.cities[cid].pos)
                .collect::<Vec<_>>();
            g.cities
                .values()
                .filter(|c| c.owner == target)
                .min_by_key(|c| {
                    let distance = from
                        .iter()
                        .map(|p| g.wdist(*p, c.pos))
                        .min()
                        .unwrap_or(i32::MAX);
                    (distance, c.hp + c.wall_hp.max(0), c.id)
                })
                .map(|c| c.id)
        });

        StrategicPlan {
            strategy,
            target_player,
            target_city,
            threatened_city,
            desired_cities,
            assessed_turn: g.turn,
        }
    }

    /// Lower is a more attractive rival: nearby, weak empires with valuable
    /// cities are preferable to distant low-power distractions.
    fn rival_value(&self, g: &Game, pid: usize, other: usize) -> f64 {
        let mine = g.player_city_ids(pid);
        let theirs = g.player_city_ids(other);
        let distance = mine
            .iter()
            .flat_map(|a| {
                theirs
                    .iter()
                    .map(move |b| g.wdist(g.cities[a].pos, g.cities[b].pos))
            })
            .min()
            .unwrap_or(40) as f64;
        let victory_pressure = self.rival_victory_pressure(g, other).progress as f64;
        distance * 7.0 + g.military_power(other) * 1.5
            - g.score(other) as f64 * 0.35
            - victory_pressure * 2.4
    }

    fn yield_value(&self, yields: Yields, strategy: GrandStrategy) -> f64 {
        let (food, prod, gold, science, culture, faith) = match strategy {
            GrandStrategy::Expansion => (2.0, 2.2, 0.9, 1.2, 1.2, 0.5),
            GrandStrategy::Science => (1.4, 2.0, 1.0, 4.2, 1.2, 0.4),
            GrandStrategy::Culture => (1.4, 1.8, 1.0, 1.3, 4.2, 0.8),
            GrandStrategy::Religion => (1.4, 1.8, 0.9, 1.1, 1.5, 4.5),
            GrandStrategy::Diplomacy => (1.4, 1.7, 2.2, 1.2, 2.8, 0.7),
            GrandStrategy::Conquest => (1.2, 2.8, 1.4, 1.7, 0.8, 0.3),
            GrandStrategy::Recovery => (1.6, 3.2, 1.5, 1.0, 0.8, 0.3),
        };
        yields.food * food
            + yields.production * prod
            + yields.gold * gold
            + yields.science * science
            + yields.culture * culture
            + yields.faith * faith
    }

    fn product_layout_value(&self, g: &Game, pid: usize, strategy: GrandStrategy) -> f64 {
        g.player_city_ids(pid)
            .into_iter()
            .map(|city_id| {
                let city = &g.cities[&city_id];
                let mut value = self.yield_value(g.city_yields(city_id), strategy);
                // Housing beyond +3 no longer changes the immediate growth
                // rate. Valuing only the useful band sends Salt Products to
                // constrained cities instead of accumulating them in a city
                // that already has abundant headroom.
                let headroom = (g.city_housing(city) - city.pop as f64).clamp(-2.0, 3.0);
                value += headroom * 18.0;
                let active_salt = city
                    .products
                    .iter()
                    .take(g.product_capacity(city))
                    .filter(|product| product.as_str() == "salt")
                    .count() as f64;
                value += active_salt * city.pop.max(1) as f64 * 2.5;
                value
            })
            .sum()
    }

    /// Products are movable economic Great Works. Search every legal move on
    /// a cloned position and make one only when it strictly improves the
    /// strategy-sensitive empire evaluation; the strict threshold prevents a
    /// free relocation from oscillating between equivalent slots.
    fn advanced_products(&self, g: &mut Game, pid: usize, strategy: GrandStrategy) {
        let candidates: BTreeSet<(u32, u32, String)> = g
            .legal_actions(pid)
            .into_iter()
            .filter_map(|action| match action {
                Action::MoveProduct { from, to, product } => Some((from, to, product)),
                _ => None,
            })
            .collect();
        let baseline = self.product_layout_value(g, pid, strategy);
        let mut best: Option<(f64, u32, u32, String)> = None;
        for (from, to, product) in candidates {
            let action = Action::MoveProduct {
                from,
                to,
                product: product.clone(),
            };
            let mut next = g.clone();
            if next.apply(pid, &action).is_err() {
                continue;
            }
            let value = self.product_layout_value(&next, pid, strategy);
            let replace = best.as_ref().is_none_or(|current| {
                value > current.0 + 1e-9
                    || ((value - current.0).abs() <= 1e-9
                        && (to, from, product.as_str())
                            < (current.2, current.1, current.3.as_str()))
            });
            if replace {
                best = Some((value, from, to, product));
            }
        }
        let Some((value, from, to, product)) = best else {
            return;
        };
        if value <= baseline + 0.01 {
            return;
        }
        let _ = g.apply(pid, &Action::MoveProduct { from, to, product });
    }

    fn advanced_research(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        if g.players[pid].research.is_none() {
            let available = g.available_techs(pid);
            let forced_goal = match self.victory_target {
                Some(VictoryTarget::Science) => [
                    "rocketry",
                    "satellites",
                    "nanotechnology",
                    "smart_materials",
                    "offworld_mission",
                ]
                .into_iter()
                .find(|tech| !g.players[pid].techs.contains(*tech)),
                Some(VictoryTarget::Culture) => ["printing", "radio", "computers"]
                    .into_iter()
                    .find(|tech| !g.players[pid].techs.contains(*tech)),
                Some(VictoryTarget::Religion) if !g.players[pid].techs.contains("astrology") => {
                    Some("astrology")
                }
                _ => None,
            };
            let goal_pick = forced_goal.and_then(|goal| {
                available
                    .iter()
                    .filter(|tech| self.tech_leads_to(g, tech, goal))
                    .min_by(|a, b| {
                        g.rules.techs[*a]
                            .cost
                            .partial_cmp(&g.rules.techs[*b].cost)
                            .unwrap()
                            .then(a.cmp(b))
                    })
                    .cloned()
            });
            let pick = goal_pick.or_else(|| {
                available.into_iter().max_by(|a, b| {
                    self.tech_value(g, pid, a, plan.strategy)
                        .partial_cmp(&self.tech_value(g, pid, b, plan.strategy))
                        .unwrap()
                        .then_with(|| b.cmp(a))
                })
            });
            if let Some(tech) = pick {
                let _ = g.apply(pid, &Action::Research { tech });
            }
        }
        if g.players[pid].civic.is_none() {
            let available = g.available_civics(pid);
            let forced_goal = match self.victory_target {
                Some(VictoryTarget::Culture) => [
                    "humanism",
                    "conservation",
                    "professional_sports",
                    "cultural_heritage",
                    "space_race",
                    "environmentalism",
                    "social_media",
                ]
                .into_iter()
                .find(|civic| !g.players[pid].civics.contains(*civic)),
                Some(VictoryTarget::Science) if !g.players[pid].civics.contains("space_race") => {
                    Some("space_race")
                }
                Some(VictoryTarget::Religion) if !g.players[pid].civics.contains("theology") => {
                    Some("theology")
                }
                _ => None,
            };
            let goal_pick = forced_goal.and_then(|goal| {
                available
                    .iter()
                    .filter(|civic| self.civic_leads_to(g, civic, goal))
                    .min_by(|a, b| {
                        g.rules.civics[*a]
                            .cost
                            .partial_cmp(&g.rules.civics[*b].cost)
                            .unwrap()
                            .then(a.cmp(b))
                    })
                    .cloned()
            });
            let pick = goal_pick.or_else(|| {
                available.into_iter().max_by(|a, b| {
                    self.civic_value(g, pid, a, plan.strategy)
                        .partial_cmp(&self.civic_value(g, pid, b, plan.strategy))
                        .unwrap()
                        .then_with(|| b.cmp(a))
                })
            });
            if let Some(civic) = pick {
                let _ = g.apply(pid, &Action::Civic { civic });
            }
        }
    }

    fn tech_leads_to(&self, g: &Game, candidate: &str, target: &str) -> bool {
        candidate == target
            || g.rules.techs.get(target).is_some_and(|spec| {
                spec.requires
                    .iter()
                    .any(|parent| self.tech_leads_to(g, candidate, parent))
            })
    }

    fn civic_leads_to(&self, g: &Game, candidate: &str, target: &str) -> bool {
        candidate == target
            || g.rules.civics.get(target).is_some_and(|spec| {
                spec.requires
                    .iter()
                    .any(|parent| self.civic_leads_to(g, candidate, parent))
            })
    }

    fn advanced_secret_society(&self, g: &mut Game, pid: usize, strategy: GrandStrategy) {
        if g.players[pid].secret_society.is_some()
            || !g.players[pid].civics.contains("code_of_laws")
        {
            return;
        }
        let long_term = self
            .victory_target
            .map(VictoryTarget::strategy)
            .unwrap_or(strategy);
        let society = match long_term {
            GrandStrategy::Science => "hermetic_order",
            GrandStrategy::Culture | GrandStrategy::Religion => "voidsingers",
            GrandStrategy::Diplomacy
            | GrandStrategy::Conquest
            | GrandStrategy::Expansion
            | GrandStrategy::Recovery => "owls_of_minerva",
        };
        let _ = g.apply(
            pid,
            &Action::ChooseSecretSociety {
                society: society.to_string(),
            },
        );
    }

    /// Replace generic early cards with the late-game cards that directly
    /// advance an explicitly selected victory. Typed cards preferentially
    /// replace cards of their own type so wildcard capacity remains useful.
    fn strategic_policies(&self, g: &mut Game, pid: usize, strategy: GrandStrategy) {
        let objective = self
            .victory_target
            .map(VictoryTarget::strategy)
            .unwrap_or(strategy);
        let desired: &[&str] = match objective {
            GrandStrategy::Culture => &[
                "heritage_tourism",
                "satellite_broadcasts",
                "online_communities",
            ],
            GrandStrategy::Science => &["integrated_space_cell"],
            _ => return,
        };
        for card in desired {
            if g.players[pid].policies.contains(*card)
                || !g
                    .available_policies(pid)
                    .iter()
                    .any(|available| available == card)
            {
                continue;
            }
            if g.apply(
                pid,
                &Action::SlotPolicy {
                    policy: (*card).to_string(),
                },
            )
            .is_ok()
            {
                continue;
            }

            let slot = g.rules.policies[*card].slot.clone();
            let mut replaceable: Vec<String> = g.players[pid]
                .policies
                .iter()
                .filter(|current| !desired.contains(&current.as_str()))
                .cloned()
                .collect();
            replaceable.sort_by_key(|current| {
                usize::from(g.rules.policies[current.as_str()].slot != slot)
            });
            for current in replaceable {
                let _ = g.apply(pid, &Action::UnslotPolicy { policy: current });
                if g.apply(
                    pid,
                    &Action::SlotPolicy {
                        policy: (*card).to_string(),
                    },
                )
                .is_ok()
                {
                    break;
                }
            }
        }
    }

    fn tech_value(&self, g: &Game, pid: usize, tech: &str, strategy: GrandStrategy) -> f64 {
        let spec = &g.rules.techs[tech];
        let mut value = if g.players[pid].boosted_techs.contains(tech) {
            28.0
        } else {
            0.0
        };
        for (name, unit) in &g.rules.units {
            if unit.tech.as_deref() == Some(tech)
                && unit
                    .unique_to
                    .as_ref()
                    .is_none_or(|c| c == &g.players[pid].civ)
            {
                let power = unit.strength.max(unit.ranged_attack_strength());
                value += if strategy == GrandStrategy::Conquest {
                    power * 3.2
                } else {
                    power * 1.1
                };
                if g.rules.civs[&g.players[pid].civ].unique_unit.as_deref() == Some(name) {
                    value += 55.0;
                }
            }
        }
        for building in g
            .rules
            .buildings
            .values()
            .filter(|b| b.tech.as_deref() == Some(tech))
        {
            value += self.yield_value(building.yields, strategy) * 14.0
                + building.housing * 12.0
                + building.amenity * 18.0;
        }
        for district in g
            .rules
            .districts
            .values()
            .filter(|d| d.tech.as_deref() == Some(tech))
        {
            value += self.yield_value(district.yields, strategy) * 18.0
                + district.defense * 1.5
                + district.amenity * 18.0;
        }
        for project in g
            .rules
            .projects
            .values()
            .filter(|p| p.tech.as_deref() == Some(tech))
        {
            value += if strategy == GrandStrategy::Science {
                if project.repeatable {
                    120.0
                } else {
                    260.0
                }
            } else if project.repeatable {
                25.0
            } else {
                65.0
            };
        }
        for improvement in g
            .rules
            .improvements
            .values()
            .filter(|i| i.tech.as_deref() == Some(tech))
        {
            value += self.yield_value(improvement.yields, strategy) * 10.0 + 18.0;
        }
        if strategy == GrandStrategy::Religion && tech == "astrology" {
            value += 95.0;
        }
        if let Some(goal) = BasicAi::water_research_goal(g, pid) {
            if self.tech_leads_to(g, tech, goal) {
                // Embarkation and ocean access change which parts of the map
                // are strategically reachable, so their prerequisites must
                // compete with ordinary yield unlocks rather than wait for a
                // naval unit to happen to win a generic score comparison.
                value += match goal {
                    "sailing" => 190.0,
                    "shipbuilding" => 230.0,
                    "celestial_navigation" => 150.0,
                    "cartography" => 210.0,
                    "square_rigging" | "steam_power" | "refining" | "electricity"
                    | "combined_arms" | "lasers" | "telecommunications" => 185.0,
                    _ => 0.0,
                };
            }
        }
        if strategy == GrandStrategy::Science {
            let milestone = if !g.players[pid]
                .science_projects
                .contains("launch_earth_satellite")
            {
                "rocketry"
            } else if !g.players[pid]
                .science_projects
                .contains("launch_moon_landing")
            {
                "satellites"
            } else if !g.players[pid]
                .science_projects
                .contains("launch_mars_colony")
            {
                "nanotechnology"
            } else if !g.players[pid]
                .science_projects
                .contains("exoplanet_expedition")
            {
                "smart_materials"
            } else {
                "offworld_mission"
            };
            if self.tech_leads_to(g, tech, milestone) {
                value += if self.victory_target == Some(VictoryTarget::Science) {
                    900.0
                } else {
                    260.0
                };
            }
        }
        // One-step lookahead prevents cheap prerequisites from being ignored.
        for (future, child) in &g.rules.techs {
            if child.requires.iter().any(|r| r == tech) {
                let unlocks = g
                    .rules
                    .units
                    .values()
                    .filter(|u| u.tech.as_deref() == Some(future))
                    .count()
                    + g.rules
                        .buildings
                        .values()
                        .filter(|b| b.tech.as_deref() == Some(future))
                        .count()
                    + g.rules
                        .districts
                        .values()
                        .filter(|d| d.tech.as_deref() == Some(future))
                        .count()
                    + g.rules
                        .projects
                        .values()
                        .filter(|p| p.tech.as_deref() == Some(future))
                        .count();
                value += unlocks as f64 * 8.0;
            }
        }
        // Discount by opportunity cost so a flashy late-era unlock does not
        // stall several cheaper advances. Square root still lets a genuinely
        // transformative breakthrough win the comparison.
        (value + 35.0) / spec.cost.max(10.0).sqrt()
    }

    fn civic_value(&self, g: &Game, pid: usize, civic: &str, strategy: GrandStrategy) -> f64 {
        let spec = &g.rules.civics[civic];
        let mut value = if g.players[pid].boosted_civics.contains(civic) {
            28.0
        } else {
            0.0
        };
        for building in g
            .rules
            .buildings
            .values()
            .filter(|b| b.civic.as_deref() == Some(civic))
        {
            value += self.yield_value(building.yields, strategy) * 15.0
                + building.housing * 12.0
                + building.amenity * 18.0;
        }
        for district in g
            .rules
            .districts
            .values()
            .filter(|d| d.civic.as_deref() == Some(civic))
        {
            value += self.yield_value(district.yields, strategy) * 18.0 + district.amenity * 18.0;
        }
        value += g
            .rules
            .governments
            .values()
            .filter(|gov| gov.civic.as_deref() == Some(civic))
            .map(|gov| {
                let slots = gov.slots.military
                    + gov.slots.economic
                    + gov.slots.diplomatic
                    + gov.slots.wildcard;
                45.0 + slots as f64 * 18.0
            })
            .sum::<f64>();
        value += g
            .rules
            .policies
            .values()
            .filter(|p| p.civic.as_deref() == Some(civic))
            .count() as f64
            * 13.0;
        if strategy == GrandStrategy::Expansion && matches!(civic, "early_empire" | "foreign_trade")
        {
            value += 45.0;
        }
        if strategy == GrandStrategy::Culture && civic == "drama_poetry" {
            value += 60.0;
        }
        if strategy == GrandStrategy::Diplomacy
            && matches!(civic, "political_philosophy" | "civil_service" | "guilds")
        {
            value += 60.0;
        }
        if strategy == GrandStrategy::Religion && civic == "theology" {
            value += 120.0;
        }
        value += match civic {
            "foreign_trade" | "craftsmanship" => 25.0,
            "early_empire" | "state_workforce" => 38.0,
            "political_philosophy" => 70.0,
            // Culture infrastructure is a prerequisite for every strategy,
            // not only a culture-victory plan.
            "drama_poetry" => 55.0,
            _ => 0.0,
        };
        (value + 32.0) / spec.cost.max(10.0).sqrt()
    }

    fn incoming_deal_value(
        &self,
        g: &Game,
        pid: usize,
        deal: &DiplomaticDeal,
        plan: &StrategicPlan,
    ) -> f64 {
        let partner = deal.from;
        let my_power = g.military_power(pid);
        let partner_power = g.military_power(partner);
        let grievance = g.players[pid]
            .grievances
            .get(&partner)
            .copied()
            .unwrap_or(0.0);
        let fatigued = self.major_war_since.is_some_and(|started| {
            g.turn.saturating_sub(started) >= 24
                && g.turn.saturating_sub(self.last_campaign_progress) >= 12
        });
        let denied_partner = plan.target_player == Some(partner)
            && (g.is_at_war(pid, partner)
                || self.rival_victory_pressure(g, partner).progress >= 78);

        let mut value = deal.give_gold - deal.request_gold;
        if deal.peace {
            value += if plan.strategy == GrandStrategy::Recovery
                || my_power < partner_power * 0.85
                || fatigued
            {
                320.0
            } else if plan.strategy == GrandStrategy::Conquest
                && plan.target_player == Some(partner)
            {
                -260.0
            } else {
                35.0
            };
        } else if denied_partner {
            return -1_000.0;
        }
        if deal.open_borders {
            value += match plan.strategy {
                GrandStrategy::Culture => 70.0,
                GrandStrategy::Conquest => 45.0,
                _ => 25.0,
            };
        }
        if deal.friendship {
            value += if plan.strategy == GrandStrategy::Diplomacy {
                80.0
            } else {
                40.0
            };
        }
        if let Some(alliance) = deal.alliance.as_deref() {
            value += match (plan.strategy, alliance) {
                (GrandStrategy::Science, "research")
                | (GrandStrategy::Culture, "cultural")
                | (GrandStrategy::Religion, "religious")
                | (GrandStrategy::Conquest | GrandStrategy::Recovery, "military")
                | (GrandStrategy::Expansion | GrandStrategy::Diplomacy, "economic") => 150.0,
                (GrandStrategy::Diplomacy, _) => 110.0,
                _ => 55.0,
            };
        }
        value - grievance * 0.8
    }

    fn congress_choice(
        &self,
        g: &Game,
        pid: usize,
        resolution: &CongressResolution,
        strategy: GrandStrategy,
    ) -> Option<String> {
        // Legacy saves encoded only a target. Preserve their old strategic
        // behavior while new sessions use explicit `A:target`/`B:target`
        // ballots.
        if resolution
            .choices
            .iter()
            .all(|choice| !choice.contains(':'))
        {
            let own = pid.to_string();
            return match resolution.id.as_str() {
                "world_leader" | "international_aid" if strategy == GrandStrategy::Diplomacy => {
                    resolution
                        .choices
                        .iter()
                        .find(|choice| **choice == own)
                        .cloned()
                }
                "world_leader" | "international_aid" => resolution
                    .choices
                    .iter()
                    .filter_map(|choice| {
                        choice.parse::<usize>().ok().map(|target| (choice, target))
                    })
                    .min_by_key(|(_, target)| (g.players[*target].dvp, *target))
                    .map(|(choice, _)| choice.clone()),
                "world_fair" if strategy == GrandStrategy::Culture => resolution
                    .choices
                    .iter()
                    .find(|choice| **choice == own)
                    .cloned(),
                "world_fair" => resolution
                    .choices
                    .iter()
                    .filter_map(|choice| {
                        choice.parse::<usize>().ok().map(|target| (choice, target))
                    })
                    .max_by(|left, right| {
                        g.players[left.1]
                            .culture_lifetime
                            .partial_cmp(&g.players[right.1].culture_lifetime)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| right.1.cmp(&left.1))
                    })
                    .map(|(choice, _)| choice.clone()),
                _ => resolution.choices.first().cloned(),
            };
        }

        let diplomatic_leader = g
            .players
            .iter()
            .filter(|player| player.alive && !player.is_minor && !player.is_barbarian)
            .max_by_key(|player| (player.dvp, std::cmp::Reverse(player.id)))
            .map(|player| player.id);
        let preferred_district = match strategy {
            GrandStrategy::Science => "campus",
            GrandStrategy::Culture => "theater_square",
            GrandStrategy::Religion => "holy_site",
            GrandStrategy::Conquest | GrandStrategy::Recovery => "encampment",
            GrandStrategy::Diplomacy => "diplomatic_quarter",
            GrandStrategy::Expansion => "commercial_hub",
        };
        let preferred_person = match strategy {
            GrandStrategy::Science => "scientist",
            GrandStrategy::Culture => "artist",
            GrandStrategy::Religion => "prophet",
            GrandStrategy::Conquest | GrandStrategy::Recovery => "general",
            GrandStrategy::Diplomacy | GrandStrategy::Expansion => "merchant",
        };
        let preferred_work = match strategy {
            GrandStrategy::Culture => "art",
            GrandStrategy::Religion => "relic",
            _ => "writing",
        };
        let observed = |choice: &str| {
            resolution
                .ballots
                .values()
                .filter(|(cast, _)| cast == choice)
                .map(|(_, votes)| *votes as f64)
                .sum::<f64>()
        };

        resolution.choices.iter().cloned().max_by(|left, right| {
            let score = |choice: &str| {
                let (outcome, target) = Game::congress_choice_parts(choice);
                let target_player = target.parse::<usize>().ok();
                let base = match resolution.id.as_str() {
                    "world_leader" => match (outcome, target_player) {
                        ("A", Some(target))
                            if target == pid && strategy == GrandStrategy::Diplomacy =>
                        {
                            1_000.0
                        }
                        ("B", Some(target))
                            if Some(target) == diplomatic_leader && target != pid =>
                        {
                            900.0
                        }
                        ("A", Some(target)) => 100.0 - 12.0 * g.players[target].dvp as f64,
                        ("B", Some(target)) => 20.0 + 18.0 * g.players[target].dvp as f64,
                        _ => 0.0,
                    },
                    "mercenary_companies" => match (outcome, target) {
                        ("B", "production") => 340.0,
                        ("B", "gold") => 180.0,
                        ("B", "faith") if strategy == GrandStrategy::Religion => 230.0,
                        ("B", "faith") => 90.0,
                        ("A", _) => -120.0,
                        _ => 0.0,
                    },
                    "luxury_policy" => {
                        let own = g.resource_access_count(pid, target) as f64;
                        let rival = g
                            .players
                            .iter()
                            .filter(|player| player.id != pid)
                            .map(|player| g.resource_access_count(player.id, target))
                            .max()
                            .unwrap_or(0) as f64;
                        if outcome == "A" {
                            own * 75.0
                        } else {
                            rival * 55.0 - own * 85.0
                        }
                    }
                    "trade_policy" => match target_player {
                        Some(target) if outcome == "A" && target == pid => 260.0,
                        Some(target)
                            if outcome == "B"
                                && Some(target) == diplomatic_leader
                                && target != pid =>
                        {
                            150.0
                        }
                        Some(target)
                            if outcome == "A" && g.alliance_with(pid, target).is_some() =>
                        {
                            120.0
                        }
                        _ => 10.0,
                    },
                    "world_religion" => {
                        let mine = g.players[pid].religion.as_deref() == Some(target);
                        if outcome == "A" && mine {
                            320.0
                        } else if outcome == "B" && !mine {
                            150.0
                        } else {
                            0.0
                        }
                    }
                    "urban_development_treaty" => {
                        if outcome == "A" && target == preferred_district {
                            280.0
                        } else if outcome == "A" {
                            80.0
                        } else {
                            -80.0
                        }
                    }
                    "patronage" => {
                        if outcome == "A" && target == preferred_person {
                            280.0
                        } else if outcome == "A" {
                            70.0
                        } else {
                            -100.0
                        }
                    }
                    "military_advisory" => {
                        let own = g
                            .units
                            .values()
                            .filter(|unit| {
                                unit.owner == pid
                                    && g.rules.units[unit.kind.as_str()].promotion_class == target
                            })
                            .count() as f64;
                        let rival = g
                            .units
                            .values()
                            .filter(|unit| {
                                unit.owner != pid
                                    && g.rules.units[unit.kind.as_str()].promotion_class == target
                            })
                            .count() as f64;
                        if outcome == "A" {
                            own * 45.0 - rival * 10.0
                        } else {
                            rival * 35.0 - own * 50.0
                        }
                    }
                    "migration_treaty" => match (outcome, target_player) {
                        ("A", Some(target))
                            if target == pid && strategy == GrandStrategy::Expansion =>
                        {
                            220.0
                        }
                        ("B", Some(target)) if target == pid => 140.0,
                        ("A", Some(target)) if target != pid => 35.0,
                        _ => 0.0,
                    },
                    "public_relations" => match (outcome, target_player) {
                        ("B", Some(target)) if target == pid => 230.0,
                        ("A", Some(target))
                            if Some(target) == diplomatic_leader && target != pid =>
                        {
                            150.0
                        }
                        _ => 0.0,
                    },
                    "heritage_organization" => {
                        if outcome == "A" && target == preferred_work {
                            300.0
                        } else if outcome == "A" {
                            90.0
                        } else {
                            -120.0
                        }
                    }
                    _ => 0.0,
                };
                base + observed(choice) * 35.0
            };
            score(left)
                .partial_cmp(&score(right))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.cmp(left))
        })
    }

    fn advanced_diplomacy(&mut self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
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
                .is_some_and(|deal| self.incoming_deal_value(g, pid, deal, plan) >= 0.0);
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
                // In an explicit victory evaluation every major shares the
                // same objective. Civ VI awards a Diplomatic Victory Point for
                // predicting any winning resolution, including International
                // Aid, so repeated participation can end a healthy science or
                // culture race with the wrong victory. Explicit non-diplomatic
                // targets abstain; adaptive agents still participate normally.
                if self.victory_target.is_some()
                    && self.victory_target != Some(VictoryTarget::Diplomacy)
                {
                    continue;
                }
                if let Some(choice) = self.congress_choice(g, pid, &resolution, plan.strategy) {
                    let votes = if plan.strategy == GrandStrategy::Diplomacy
                        && g.players[pid].diplomatic_favor >= 30.0
                    {
                        3
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
        }
        let denied_partner = plan.target_player.filter(|target| {
            g.is_at_war(pid, *target) || self.rival_victory_pressure(g, *target).progress >= 78
        });
        self.base.bilateral_trade_excluding(g, pid, denied_partner);
        let my_power = g.military_power(pid);
        let rivals: Vec<usize> = g
            .players
            .iter()
            .filter(|p| p.id != pid && p.alive && !p.is_barbarian)
            .map(|p| p.id)
            .collect();
        for other in &rivals {
            let fatigued = self.major_war_since.is_some_and(|started| {
                g.turn.saturating_sub(started) >= 24
                    && g.turn.saturating_sub(self.last_campaign_progress) >= 12
            });
            if g.is_at_war(pid, *other)
                && !g.players[*other].is_minor
                && (my_power < g.military_power(*other) * 0.62
                    || (plan.strategy == GrandStrategy::Recovery
                        && plan.target_player != Some(*other))
                    || (fatigued && g.player_city_ids(*other).len() > 1))
                && g.apply(pid, &Action::MakePeace { player: *other }).is_ok()
            {
                self.peace_until = g.turn.saturating_add(30);
                self.major_war_since = None;
            }
        }
        let major_wars = rivals
            .iter()
            .filter(|o| !g.players[**o].is_minor && g.is_at_war(pid, **o))
            .count();
        if major_wars > 0
            && matches!(
                plan.strategy,
                GrandStrategy::Conquest | GrandStrategy::Recovery
            )
        {
            self.base.levy_city_state_military(g, pid, true);
        }
        let Some(target) = plan.target_player else {
            return;
        };
        if plan.strategy != GrandStrategy::Conquest
            || major_wars > 0
            || g.turn < 35
            || g.turn < self.peace_until
            || g.player_city_ids(pid).len() < 2
            || g.is_at_war(pid, target)
        {
            return;
        }
        let target_power = g.military_power(target);
        let close_enough = plan
            .target_city
            .and_then(|cid| g.cities.get(&cid))
            .is_some_and(|target_city| {
                g.player_city_ids(pid)
                    .iter()
                    .any(|cid| g.wdist(g.cities[cid].pos, target_city.pos) <= 18)
            });
        let committed_domination = self.victory_target == Some(VictoryTarget::Domination);
        let ready = if committed_domination {
            my_power >= target_power * 0.85 && my_power >= 30.0
        } else {
            my_power > target_power * 1.32 + 12.0
        };
        if close_enough && ready {
            let _ = g.apply(pid, &Action::DeclareWar { player: target });
        }
    }

    fn advanced_envoys(
        &self,
        g: &mut Game,
        pid: usize,
        strategy: GrandStrategy,
        denied_rival: Option<usize>,
    ) {
        while g.players[pid].envoys_free > 0 {
            let target = g
                .players
                .iter()
                .filter(|minor| {
                    minor.alive
                        && minor.is_minor
                        && !minor.is_barbarian
                        && !g.is_at_war(pid, minor.id)
                })
                .map(|minor| {
                    let mine = g.envoys_at(pid, minor.id);
                    let rival = g
                        .players
                        .iter()
                        .filter(|p| !p.is_minor && !p.is_barbarian && p.id != pid)
                        .map(|p| g.envoys_at(p.id, minor.id))
                        .max()
                        .unwrap_or(0);
                    let needed = (3_i64.max(rival + 1) - mine).max(1);
                    let kind = Game::cs_type(&minor.civ);
                    let alignment = match (strategy, kind) {
                        (GrandStrategy::Science, "scientific") => 10,
                        (GrandStrategy::Culture, "cultural") => 10,
                        (GrandStrategy::Religion, "religious") => 12,
                        (GrandStrategy::Diplomacy, _) => 10,
                        (GrandStrategy::Conquest, "militaristic") => 10,
                        (GrandStrategy::Expansion, "trade") => 8,
                        (_, "trade") => 4,
                        _ => 2,
                    };
                    let already_secure = g.suzerain_of(minor.id) == Some(pid) && mine > rival + 1;
                    let denial = denied_rival
                        .is_some_and(|leader| g.suzerain_of(minor.id) == Some(leader))
                        as i64
                        * 140;
                    let score = alignment * 10 + denial - needed * 7 - already_secure as i64 * 80;
                    (
                        score,
                        std::cmp::Reverse(needed),
                        std::cmp::Reverse(minor.id),
                        minor.id,
                    )
                })
                .max()
                .map(|(_, _, _, id)| id);
            let Some(target) = target else { break };
            if g.apply(pid, &Action::SendEnvoy { player: target }).is_err() {
                break;
            }
        }
    }

    /// Buy out a close Great Person race only when the person advances the
    /// active plan and the purchase leaves a useful operating reserve. Normal
    /// GPP recruitment is automatic at turn start; this phase is deliberately
    /// limited to one tempo purchase per turn.
    fn advanced_great_people(&self, g: &mut Game, pid: usize, strategy: GrandStrategy) {
        let city_count = g.player_city_ids(pid).len() as f64;
        let gold_reserve = 150.0 + 50.0 * city_count;
        let faith_reserve = match strategy {
            GrandStrategy::Religion => 250.0,
            GrandStrategy::Culture if g.players[pid].civics.contains("cold_war") => 700.0,
            _ => 100.0,
        };
        let mut candidates = Vec::new();
        for kind in [
            "scientist",
            "engineer",
            "writer",
            "artist",
            "musician",
            "merchant",
            "general",
            "admiral",
            "prophet",
        ] {
            let Some((_, person)) = g.current_great_person(kind) else {
                continue;
            };
            let work_kind = match kind {
                "writer" => Some("writing"),
                "artist" => Some("art"),
                "musician" => Some("music"),
                _ => None,
            };
            if work_kind.is_some_and(|work| !g.can_house_additional_great_work(pid, work)) {
                continue;
            }
            let points = g.players[pid].gpp.get(kind).copied().unwrap_or(0.0);
            let missing = (person.cost - points).max(0.0);
            if missing <= f64::EPSILON {
                continue;
            }
            let affinity = match (strategy, kind) {
                (GrandStrategy::Science, "scientist")
                | (GrandStrategy::Culture, "writer" | "artist" | "musician")
                | (GrandStrategy::Diplomacy, "merchant")
                | (GrandStrategy::Conquest, "general" | "admiral") => 500.0,
                (GrandStrategy::Religion, "prophet") if g.players[pid].religion.is_none() => 650.0,
                (GrandStrategy::Expansion | GrandStrategy::Recovery, "engineer" | "merchant")
                | (GrandStrategy::Science | GrandStrategy::Culture, "engineer") => 300.0,
                (_, "prophet") if g.players[pid].religion.is_some() => -1_000.0,
                _ => 100.0,
            };
            let close_fraction = missing / person.cost.max(1.0);
            let limit = if affinity >= 500.0 { 0.40 } else { 0.15 };
            if affinity < 0.0 || close_fraction > limit {
                continue;
            }
            let effect_value = person.effects.values().sum::<f64>() * 12.0;
            for (currency, price, bank, reserve) in [
                ("gold", missing * 15.0, g.players[pid].gold, gold_reserve),
                ("faith", missing * 10.0, g.players[pid].faith, faith_reserve),
            ] {
                if bank + f64::EPSILON < price + reserve {
                    continue;
                }
                let opportunity = price / (bank - reserve).max(1.0);
                let score = (affinity + effect_value) * (1.0 - opportunity.min(0.95));
                candidates.push((
                    score,
                    std::cmp::Reverse((kind.to_string(), currency.to_string())),
                    Action::PatronizeGreatPerson {
                        kind: kind.to_string(),
                        currency: currency.to_string(),
                    },
                ));
            }
        }
        if let Some((_, _, action)) = candidates.into_iter().max_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.1.cmp(&right.1))
        }) {
            let _ = g.apply(pid, &action);
        }
    }

    fn counts(&self, g: &Game, pid: usize) -> EmpireCounts {
        let mut counts = EmpireCounts::default();
        for uid in g.player_unit_ids(pid) {
            counts.add_unit(g, &g.units[&uid].kind);
        }
        for cid in g.player_city_ids(pid) {
            if let Some(item) = g.cities[&cid].queue.first() {
                counts.add_item(g, item);
            }
        }
        counts
    }

    fn religious_production(&self, g: &mut Game, pid: usize) {
        let city_ids = g.player_city_ids(pid);
        let has_holy_site = city_ids
            .iter()
            .any(|cid| g.cities[cid].districts.contains_key("holy_site"));
        if !has_holy_site {
            let holy_site_planned = city_ids.iter().any(|cid| {
                matches!(
                    g.cities[cid].queue.first(),
                    Some(Item::District { district, .. }) if district == "holy_site"
                )
            });
            if holy_site_planned {
                return;
            }
            let mut best: Option<(f64, u32, Pos)> = None;
            for cid in &city_ids {
                if !g.cities[cid].queue.is_empty() {
                    continue;
                }
                for item in g.producible_items(pid, *cid) {
                    let Item::District { district, pos } = item else {
                        continue;
                    };
                    if district != "holy_site" {
                        continue;
                    }
                    let faith = g.district_yields("holy_site", pos).faith;
                    if best
                        .map(|old| {
                            faith > old.0 || (faith == old.0 && (*cid, pos) > (old.1, old.2))
                        })
                        .unwrap_or(true)
                    {
                        best = Some((faith, *cid, pos));
                    }
                }
            }
            if let Some((faith, city, pos)) = best {
                if faith >= 3.0 {
                    let _ = g.apply(
                        pid,
                        &Action::Produce {
                            city,
                            item: Item::District {
                                district: "holy_site".to_string(),
                                pos,
                            },
                        },
                    );
                }
            }
            return;
        }
        for building in ["shrine", "temple"] {
            for cid in &city_ids {
                let item = Item::Building {
                    building: building.to_string(),
                };
                if g.cities[cid].queue.is_empty()
                    && g.cities[cid].districts.contains_key("holy_site")
                    && g.can_produce(pid, *cid, &item)
                {
                    let _ = g.apply(pid, &Action::Produce { city: *cid, item });
                    return;
                }
            }
        }
    }

    fn religious_spending(&self, g: &mut Game, pid: usize) {
        if g.players[pid].religion.is_none() {
            return;
        }
        let count = |kind: &str| {
            g.units
                .values()
                .filter(|unit| unit.owner == pid && unit.kind == kind)
                .count()
        };
        let priorities = if count("apostle") < 2 {
            ["apostle", "missionary", "guru"]
        } else if count("guru") < 1 {
            ["guru", "apostle", "missionary"]
        } else {
            ["missionary", "apostle", "guru"]
        };
        for unit in priorities {
            let Some(spec) = g.rules.units.get(unit) else {
                continue;
            };
            let price = spec.cost * 2.0;
            if g.players[pid].faith < price + 80.0 {
                continue;
            }
            let cities = g.player_city_ids(pid);
            for cid in cities {
                if g.apply(
                    pid,
                    &Action::Buy {
                        city: cid,
                        unit: unit.to_string(),
                        currency: "faith".to_string(),
                    },
                )
                .is_ok()
                {
                    return;
                }
            }
        }
    }

    fn culture_spending(&self, g: &mut Game, pid: usize) {
        let active_naturalists = g
            .units
            .values()
            .filter(|unit| unit.owner == pid && unit.kind == "naturalist")
            .count();
        if active_naturalists == 0
            && !g.national_park_sites(pid).is_empty()
            && g.players[pid].faith + f64::EPSILON >= g.naturalist_purchase_cost(pid)
        {
            for city in g.player_city_ids(pid) {
                if g.apply(
                    pid,
                    &Action::Buy {
                        city,
                        unit: "naturalist".to_string(),
                        currency: "faith".to_string(),
                    },
                )
                .is_ok()
                {
                    return;
                }
            }
        }
        let active_bands = g
            .units
            .values()
            .filter(|unit| unit.owner == pid && unit.kind == "rock_band")
            .count();
        if active_bands >= 2
            || !g.players[pid].civics.contains("cold_war")
            || g.players[pid].faith + f64::EPSILON < g.rules.units["rock_band"].cost
        {
            return;
        }
        for city in g.player_city_ids(pid) {
            if g.apply(
                pid,
                &Action::Buy {
                    city,
                    unit: "rock_band".to_string(),
                    currency: "faith".to_string(),
                },
            )
            .is_ok()
            {
                return;
            }
        }
    }

    fn faith_building_spending(&self, g: &mut Game, pid: usize, strategy: GrandStrategy) {
        let reserve = match strategy {
            GrandStrategy::Religion => 180.0,
            GrandStrategy::Culture if !g.national_park_sites(pid).is_empty() => {
                g.naturalist_purchase_cost(pid)
            }
            GrandStrategy::Culture if g.players[pid].civics.contains("cold_war") => 700.0,
            _ => 80.0,
        };
        let best = g
            .legal_actions(pid)
            .into_iter()
            .filter_map(|action| match &action {
                Action::BuyBuilding { city, building, .. } => {
                    let spec = &g.rules.buildings[building];
                    let estimated_cost = spec.cost * 2.0;
                    if g.players[pid].faith + f64::EPSILON < estimated_cost + reserve {
                        return None;
                    }
                    let worship = spec.worship_belief.is_some() as i32;
                    let score = (self.yield_value(spec.yields, strategy) * 25.0) as i32
                        + (spec.housing * 35.0 + spec.amenity * 50.0) as i32
                        + spec.great_work_slots.values().sum::<i32>() * 60
                        + worship * 220;
                    Some((score, std::cmp::Reverse((*city, building.clone())), action))
                }
                _ => None,
            })
            .max_by_key(|(score, key, _)| (*score, key.clone()));
        if let Some((_, _, action)) = best {
            let _ = g.apply(pid, &action);
        }
    }

    fn science_production(&self, g: &mut Game, pid: usize) {
        let completed = &g.players[pid].science_projects;
        let project = if !completed.contains("launch_earth_satellite") {
            "launch_earth_satellite"
        } else if !completed.contains("launch_moon_landing") {
            "launch_moon_landing"
        } else if !completed.contains("launch_mars_colony") {
            "launch_mars_colony"
        } else if !completed.contains("exoplanet_expedition") {
            "exoplanet_expedition"
        } else {
            "lagrange_laser_station"
        };
        let project_item = Item::Project {
            project: project.to_string(),
        };
        let already_queued = g.player_city_ids(pid).iter().any(|cid| {
            matches!(
                g.cities[cid].queue.first(),
                Some(Item::Project { project: queued }) if queued == project
            )
        });
        if !already_queued {
            let project_city = g
                .player_city_ids(pid)
                .into_iter()
                .filter(|cid| {
                    g.cities[cid].districts.contains_key("spaceport")
                        && g.can_produce(pid, *cid, &project_item)
                        && (self.victory_target == Some(VictoryTarget::Science)
                            || g.cities[cid].queue.is_empty())
                })
                .max_by(|a, b| {
                    g.city_yields(*a)
                        .production
                        .partial_cmp(&g.city_yields(*b).production)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| b.cmp(a))
                });
            if let Some(city) = project_city {
                let _ = g.apply(
                    pid,
                    &Action::Produce {
                        city,
                        item: project_item,
                    },
                );
                return;
            }
        }

        let has_spaceport = g
            .player_city_ids(pid)
            .iter()
            .any(|cid| g.cities[cid].districts.contains_key("spaceport"));
        let spaceport_queued = g.player_city_ids(pid).iter().any(|cid| {
            matches!(
                g.cities[cid].queue.first(),
                Some(Item::District { district, .. }) if district == "spaceport"
            )
        });
        if has_spaceport || spaceport_queued {
            return;
        }
        let mut best: Option<(f64, u32, Pos)> = None;
        for cid in g.player_city_ids(pid) {
            if self.victory_target != Some(VictoryTarget::Science)
                && !g.cities[&cid].queue.is_empty()
            {
                continue;
            }
            for item in g.producible_items(pid, cid) {
                let Item::District { district, pos } = item else {
                    continue;
                };
                if district != "spaceport" {
                    continue;
                }
                let production = g.city_yields(cid).production;
                if best
                    .map(|old| {
                        production > old.0 || (production == old.0 && (cid, pos) < (old.1, old.2))
                    })
                    .unwrap_or(true)
                {
                    best = Some((production, cid, pos));
                }
            }
        }
        if let Some((_, city, pos)) = best {
            let _ = g.apply(
                pid,
                &Action::Produce {
                    city,
                    item: Item::District {
                        district: "spaceport".to_string(),
                        pos,
                    },
                },
            );
        }
    }

    #[allow(dead_code)]
    fn advanced_production(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        let mut counts = self.counts(g, pid);
        let city_ids = g.player_city_ids(pid);
        for cid in city_ids {
            if !g.cities[&cid].queue.is_empty() {
                continue;
            }
            let mut best: Option<(f64, String, Item)> = None;
            for item in g.producible_items(pid, cid) {
                if let Item::Project { project } = &item {
                    let spec = &g.rules.projects[project];
                    let already_queued_elsewhere = !spec.repeatable
                        && g.cities.values().any(|city| {
                            city.owner == pid
                                && city.id != cid
                                && matches!(
                                    city.queue.first(),
                                    Some(Item::Project { project: queued }) if queued == project
                                )
                        });
                    if already_queued_elsewhere {
                        continue;
                    }
                }
                let score = self.production_value(g, pid, cid, &item, plan, &counts);
                let key = format!("{item:?}");
                let replace = best
                    .as_ref()
                    .map(|(old, old_key, _)| {
                        score > *old + 1e-9 || ((score - *old).abs() < 1e-9 && key < *old_key)
                    })
                    .unwrap_or(true);
                if replace {
                    best = Some((score, key, item));
                }
            }
            if let Some((score, _, item)) = best {
                if score > -1_000.0
                    && g.apply(
                        pid,
                        &Action::Produce {
                            city: cid,
                            item: item.clone(),
                        },
                    )
                    .is_ok()
                {
                    counts.add_item(g, &item);
                }
            }
        }
    }

    fn production_value(
        &self,
        g: &Game,
        pid: usize,
        cid: u32,
        item: &Item,
        plan: &StrategicPlan,
        counts: &EmpireCounts,
    ) -> f64 {
        let city = &g.cities[&cid];
        let city_count = g.player_city_ids(pid).len();
        let production = g.city_yields(cid).production.max(1.0);
        let turns = g.item_cost_for_city(pid, cid, item) / production;
        let remaining_turns = g.max_turns.saturating_sub(g.turn).max(1) as f64;
        let threatened = plan.threatened_city == Some(cid)
            || (city.last_attacked > 0 && g.turn.saturating_sub(city.last_attacked) <= 4);
        let desired_military = match plan.strategy {
            GrandStrategy::Conquest => 2 * city_count,
            GrandStrategy::Recovery => 2 * city_count,
            _ => city_count,
        };
        let raw = match item {
            Item::Unit { unit } if unit == "settler" => {
                let site = self.best_settle_site(g, pid, city.pos, 11).or_else(|| {
                    g.players[pid]
                        .techs
                        .contains("shipbuilding")
                        .then(|| {
                            self.best_settle_site(g, pid, city.pos, g.map.width + g.map.height)
                        })
                        .flatten()
                });
                if city_count + counts.settlers < plan.desired_cities
                    && counts.settlers == 0
                    && city.pop >= 2
                    && g.turn < 175
                    && site.is_some()
                {
                    920.0 + site.map(|(_, v)| v * 4.0).unwrap_or(0.0)
                } else {
                    -10_000.0
                }
            }
            Item::Unit { unit } if unit == "builder" => {
                let desired = city_count.div_ceil(2).max(1);
                if counts.builders < desired {
                    260.0 + 35.0 * (desired - counts.builders) as f64
                } else {
                    25.0
                }
            }
            Item::Unit { unit } if unit == "trader" => {
                if g.active_routes(pid) + (counts.traders as i64) < g.trade_capacity(pid) {
                    255.0
                } else {
                    -10_000.0
                }
            }
            Item::Unit { unit } if unit == "missionary" => {
                if self.victory_target.is_some()
                    && self.victory_target != Some(VictoryTarget::Religion)
                {
                    -10_000.0
                } else if g.players[pid].religion.is_some() && counts.missionaries < 2 {
                    150.0
                } else {
                    -10_000.0
                }
            }
            Item::Unit { unit } if unit == "archaeologist" => {
                let active = g
                    .units
                    .values()
                    .any(|unit| unit.owner == pid && unit.kind == "archaeologist");
                let sites = g.excavation_sites(pid).len();
                if plan.strategy == GrandStrategy::Culture && !active && sites > 0 {
                    2_700.0 + sites.min(3) as f64 * 180.0
                } else {
                    -10_000.0
                }
            }
            Item::Formation { unit, formation } => {
                let spec = &g.rules.units[unit];
                let naval = spec.domain.as_deref() == Some("sea");
                let desired = if naval {
                    BasicAi::desired_navy(g, pid)
                } else {
                    desired_military
                };
                let current = if naval {
                    counts.naval
                } else {
                    counts
                        .military
                        .saturating_sub(counts.naval + counts.aircraft)
                };
                let effective_power = spec.strength.max(spec.ranged_attack_strength())
                    + if *formation >= 2 { 17.0 } else { 10.0 };
                effective_power
                    * if current < desired || threatened {
                        4.25
                    } else {
                        0.75
                    }
                    + if threatened { 240.0 } else { 0.0 }
                    + if plan.strategy == GrandStrategy::Conquest {
                        160.0
                    } else {
                        0.0
                    }
            }
            Item::Unit { unit } => {
                let spec = &g.rules.units[unit];
                if spec.class == "military" {
                    let naval = spec.domain.as_deref() == Some("sea");
                    let desired_naval = BasicAi::desired_navy(g, pid);
                    if naval && !BasicAi::city_is_coastal(g, cid) {
                        return -10_000.0;
                    }
                    if self.victory_target.is_some()
                        && self.victory_target != Some(VictoryTarget::Domination)
                        && counts.military >= desired_military
                        && (!naval || counts.naval >= desired_naval)
                        && !threatened
                    {
                        return -2_000.0;
                    }
                    if unit == "scout" && counts.scouts >= 1 {
                        return -2_000.0;
                    }
                    let power = spec.strength.max(spec.ranged_attack_strength());
                    let best_role_power = g
                        .rules
                        .units
                        .iter()
                        .filter(|(name, candidate)| {
                            candidate.class == "military"
                                && candidate.domain == spec.domain
                                && candidate.has_ranged_attack() == spec.has_ranged_attack()
                                && g.can_produce(
                                    pid,
                                    cid,
                                    &Item::Unit {
                                        unit: (*name).clone(),
                                    },
                                )
                        })
                        .map(|(_, candidate)| {
                            candidate.strength.max(candidate.ranged_attack_strength())
                        })
                        .fold(0.0_f64, f64::max);
                    if unit != "scout" && power + 5.0 < best_role_power {
                        return -2_000.0;
                    }
                    let land_military = counts
                        .military
                        .saturating_sub(counts.naval + counts.aircraft);
                    let force_gap = if naval {
                        desired_naval.saturating_sub(counts.naval) as f64
                    } else {
                        desired_military.saturating_sub(land_military) as f64
                    };
                    let role_gap = if force_gap <= 0.0 {
                        0.0
                    } else if naval {
                        match spec.promotion_class.as_str() {
                            "naval_melee" => {
                                (counts.naval_melee <= counts.naval_ranged + counts.naval_raider)
                                    as i32 as f64
                                    * 80.0
                            }
                            "naval_ranged" => {
                                (counts.naval_ranged < counts.naval_melee.max(1)) as i32 as f64
                                    * 65.0
                            }
                            "naval_raider" => {
                                (counts.naval >= 2 && counts.naval_raider == 0) as i32 as f64 * 45.0
                            }
                            "naval_carrier" => {
                                if counts.aircraft > 0 && counts.carriers == 0 {
                                    55.0
                                } else {
                                    -180.0
                                }
                            }
                            _ => 0.0,
                        }
                    } else if spec.has_ranged_attack() {
                        (counts.melee > counts.ranged) as i32 as f64 * 55.0
                    } else {
                        (counts.ranged >= counts.melee) as i32 as f64 * 55.0
                    };
                    power * if force_gap > 0.0 { 4.0 } else { 0.65 }
                        + role_gap
                        + force_gap * 58.0
                        + if threatened { 210.0 } else { 0.0 }
                        + if plan.strategy == GrandStrategy::Conquest
                            && counts.military < desired_military + 2
                        {
                            120.0
                        } else {
                            0.0
                        }
                        + if spec.siege && counts.siege == 0 && plan.target_city.is_some() {
                            95.0
                        } else {
                            0.0
                        }
                } else if spec.class == "support"
                    && plan.strategy == GrandStrategy::Conquest
                    && counts.support == 0
                {
                    180.0
                } else if spec.class == "support" {
                    -10_000.0
                } else {
                    20.0
                }
            }
            Item::Building { building } => {
                let spec = &g.rules.buildings[building];
                if self.victory_target.is_some()
                    && self.victory_target != Some(VictoryTarget::Culture)
                    && !spec.great_work_slots.is_empty()
                {
                    return -10_000.0;
                }
                if spec.wonder {
                    let wonder_civ = matches!(g.players[pid].civ.as_str(), "Egypt" | "China");
                    if threatened
                        || city.buildings.len() < 3
                        || turns > remaining_turns * 0.65
                        || (plan.strategy != GrandStrategy::Culture && !wonder_civ)
                    {
                        -10_000.0
                    } else {
                        self.yield_value(spec.yields, plan.strategy) * 35.0
                            + spec.housing * 30.0
                            + spec.amenity * 45.0
                            + if plan.strategy == GrandStrategy::Culture {
                                150.0
                            } else {
                                0.0
                            }
                            + if wonder_civ { 120.0 } else { 0.0 }
                    }
                } else {
                    let housing_need = (city.pop as f64 + 1.0 - g.city_housing(city)).max(0.0);
                    let amenity_need = (-g.city_amenity_surplus(city)).max(0) as f64;
                    let great_work_slots =
                        spec.great_work_slots.values().sum::<i32>().max(0) as f64;
                    let cultural_gpp = ["writer", "artist", "musician"]
                        .into_iter()
                        .map(|kind| spec.great_person_points.get(kind).copied().unwrap_or(0.0))
                        .sum::<f64>();
                    self.yield_value(spec.yields, plan.strategy) * 42.0
                        + spec.housing * (22.0 + housing_need * 18.0)
                        + spec.amenity * (30.0 + amenity_need * 22.0)
                        + great_work_slots
                            * if plan.strategy == GrandStrategy::Culture {
                                180.0
                            } else {
                                25.0
                            }
                        + cultural_gpp
                            * if plan.strategy == GrandStrategy::Culture {
                                140.0
                            } else {
                                10.0
                            }
                        + spec.effects.get("tourism").copied().unwrap_or(0.0) * 80.0
                        + if building == "monument" && g.turn < 120 {
                            240.0
                        } else {
                            0.0
                        }
                        + if building == "granary" && city.pop as f64 + 1.0 >= g.city_housing(city)
                        {
                            180.0
                        } else {
                            0.0
                        }
                        + if building.contains("walls") && threatened {
                            320.0
                        } else {
                            0.0
                        }
                }
            }
            Item::District { district, pos } => {
                let spec = &g.rules.districts[district];
                let developed_capacity = ((city.pop + 1) / 2).max(2) as usize;
                if city.districts.len() >= developed_capacity
                    && city.buildings.len() <= city.districts.len()
                {
                    return -1_200.0;
                }
                let district_count = g
                    .cities
                    .values()
                    .filter(|c| c.owner == pid && c.districts.contains_key(district))
                    .count();
                let balanced_core = if district_count * 2 < city_count {
                    match district.as_str() {
                        "campus" | "theater_square" | "commercial_hub" => 130.0,
                        _ => 0.0,
                    }
                } else {
                    0.0
                };
                let culture_district = district == "theater_square"
                    || spec.replaces.as_deref() == Some("theater_square");
                self.yield_value(g.district_yields(district, *pos), plan.strategy) * 60.0
                    + spec.defense * if threatened { 5.0 } else { 1.5 }
                    + spec.amenity * 50.0
                    + balanced_core
                    + if plan.strategy == GrandStrategy::Culture && culture_district {
                        // A Theater Square starts earning Great People long
                        // before its building chain is complete, and every
                        // city supplies another set of work slots. Establish
                        // the network early instead of stopping at a merely
                        // balanced half-empire coverage.
                        850.0
                    } else {
                        0.0
                    }
                    + match (plan.strategy, district.as_str()) {
                        (GrandStrategy::Science, "spaceport") if district_count == 0 => 3_000.0,
                        (GrandStrategy::Science, "spaceport") => 250.0,
                        (GrandStrategy::Science, "campus") => 170.0,
                        (GrandStrategy::Religion, "holy_site") => 210.0,
                        (GrandStrategy::Diplomacy, "commercial_hub") => 150.0,
                        (GrandStrategy::Diplomacy, "theater_square") => 100.0,
                        (GrandStrategy::Conquest, "encampment") => 130.0,
                        (GrandStrategy::Recovery, "industrial_zone") => 130.0,
                        (GrandStrategy::Expansion, "commercial_hub") => 90.0,
                        _ => 0.0,
                    }
            }
            Item::Repair { repair, .. } => {
                if repair == "district" {
                    1_500.0 + if threatened { 300.0 } else { 0.0 }
                } else {
                    1_050.0 + if threatened { 180.0 } else { 0.0 }
                }
            }
            Item::Wonder { wonder, .. } => {
                let spec = &g.rules.wonders[wonder];
                let wonder_civ = matches!(g.players[pid].civ.as_str(), "Egypt" | "China");
                let already_queued = g.cities.values().any(|other| {
                    matches!(
                        other.queue.first(),
                        Some(Item::Wonder { wonder: queued, .. }) if queued == wonder
                    )
                });
                if already_queued
                    || threatened
                    || city.buildings.len() < 2
                    || turns > remaining_turns * 0.65
                    || (plan.strategy != GrandStrategy::Culture
                        && self.victory_target != Some(VictoryTarget::Score)
                        && (!wonder_civ || self.victory_target.is_some()))
                {
                    -10_000.0
                } else {
                    self.yield_value(spec.yields, plan.strategy) * 45.0
                        + spec.housing * 30.0
                        + spec.amenity * 50.0
                        + spec.great_work_slots.values().sum::<i32>() as f64 * 40.0
                        + spec.great_person_points.values().sum::<f64>() * 18.0
                        + if plan.strategy == GrandStrategy::Culture {
                            320.0
                        } else if self.victory_target == Some(VictoryTarget::Score) {
                            180.0
                        } else {
                            0.0
                        }
                        + if wonder_civ { 120.0 } else { 0.0 }
                }
            }
            Item::Project { project } => {
                let space_race = matches!(
                    project.as_str(),
                    "launch_earth_satellite"
                        | "launch_moon_landing"
                        | "launch_mars_colony"
                        | "exoplanet_expedition"
                        | "lagrange_laser_station"
                        | "terrestrial_laser_station"
                );
                if (space_race
                    && self.victory_target.is_some()
                    && self.victory_target != Some(VictoryTarget::Science))
                    || turns > remaining_turns * 0.8
                {
                    -10_000.0
                } else {
                    let completed = g.players[pid].science_projects.len() as f64;
                    match project.as_str() {
                        "recommission_reactor" => {
                            if city.reactor_age <= 12 {
                                -10_000.0
                            } else {
                                // Maintenance becomes urgent as the reactor's
                                // per-turn accident risk compounds. A fresh
                                // plant must never monopolize production just
                                // because this is a repeatable project.
                                500.0 + (city.reactor_age - 10) as f64 * 75.0
                            }
                        }
                        "convert_reactor_to_coal"
                        | "convert_reactor_to_oil"
                        | "convert_reactor_to_uranium" => {
                            let (resource, stock_value, clean_value) = match project.as_str() {
                                "convert_reactor_to_coal" => ("coal", 18.0, -110.0),
                                "convert_reactor_to_oil" => ("oil", 20.0, -55.0),
                                _ => ("uranium", 55.0, 130.0),
                            };
                            450.0
                                + g.strategic_stockpile(pid, resource).min(50.0) * stock_value
                                + g.climate_phase as f64 * clean_value
                        }
                        "carbon_recapture" => {
                            if g.global_co2_emissions() <= f64::EPSILON
                                && plan.strategy != GrandStrategy::Diplomacy
                            {
                                -10_000.0
                            } else {
                                450.0
                                    + g.climate_phase as f64 * 260.0
                                    + (g.players[pid].co2_emissions.max(0.0) / 500.0).min(800.0)
                                    + if plan.strategy == GrandStrategy::Diplomacy {
                                        900.0
                                    } else {
                                        0.0
                                    }
                            }
                        }
                        "manhattan_project" | "operation_ivy" => {
                            if plan.strategy == GrandStrategy::Conquest {
                                2_200.0
                            } else {
                                350.0
                            }
                        }
                        "build_nuclear_device" | "build_thermonuclear_device" => {
                            if plan.strategy == GrandStrategy::Conquest {
                                2_600.0
                            } else if plan.target_player.is_some() {
                                850.0
                            } else {
                                250.0
                            }
                        }
                        _ if space_race => {
                            3_300.0
                                + completed * 220.0
                                + if plan.strategy == GrandStrategy::Science {
                                    650.0
                                } else {
                                    0.0
                                }
                        }
                        _ => 700.0,
                    }
                }
            }
            Item::Product { product } => {
                let existing = g
                    .cities
                    .values()
                    .filter(|other| other.owner == pid)
                    .flat_map(|other| other.products.iter())
                    .filter(|existing| *existing == product)
                    .count() as f64;
                let strategic = match (plan.strategy, product.as_str()) {
                    (GrandStrategy::Culture, "silk" | "wine") => 2_000.0,
                    (GrandStrategy::Expansion | GrandStrategy::Recovery, "salt") => 1_650.0,
                    (GrandStrategy::Diplomacy, _) => 900.0,
                    _ => 600.0,
                };
                1_600.0 + strategic - existing * 280.0
            }
        };
        if raw <= -9_999.0 {
            return raw;
        }
        if turns > remaining_turns + 1.0 {
            return -1_500.0;
        }
        let completion_discount = if turns > remaining_turns * 0.6 {
            0.25
        } else {
            1.0
        };
        completion_discount * raw / (7.0 + turns.max(1.0))
    }

    fn settle_value(&self, g: &Game, pid: usize, pos: Pos) -> f64 {
        let tile = &g.map.tiles[&pos];
        let mut value = 0.0;
        for p in g.wdisk(pos, 2) {
            let Some(t) = g.map.get(p) else { continue };
            if t.owner_city.is_some() && p != pos {
                continue;
            }
            let y = g.rules.tile_yields(t);
            let ring_discount = if g.wdist(pos, p) <= 1 { 1.0 } else { 0.45 };
            value += ring_discount
                * (y.food * 2.0
                    + y.production * 2.2
                    + y.gold * 0.7
                    + y.science * 1.2
                    + y.culture * 1.2
                    + y.faith * 0.4);
            if let Some(resource) = &t.resource {
                value += match g.rules.resources[resource].class.as_str() {
                    "luxury" => 5.0,
                    "strategic" => 4.0,
                    _ => 1.5,
                } * ring_discount;
            }
        }
        let fresh = tile.has_river()
            || g.nbrs(pos).iter().any(|p| {
                g.map
                    .get(*p)
                    .is_some_and(|t| t.feature.as_deref() == Some("oasis"))
            });
        let coastal = g
            .nbrs(pos)
            .iter()
            .any(|p| g.map.get(*p).is_some_and(|t| g.rules.is_water(t)));
        value += if fresh {
            14.0
        } else if coastal {
            6.0
        } else {
            -5.0
        };
        let enemy_distance = g
            .cities
            .values()
            .filter(|c| c.owner != pid && !g.players[c.owner].is_barbarian)
            .map(|c| g.wdist(pos, c.pos))
            .min()
            .unwrap_or(20);
        if enemy_distance < 6 {
            value -= (6 - enemy_distance) as f64 * 6.0;
        }
        value
    }

    fn settle_sites(&self, g: &Game, pid: usize, from: Pos, radius: i32) -> Vec<(Pos, f64)> {
        let mut sites = Vec::new();
        let distance_penalty = if radius > 12 { 0.45 } else { 0.9 };
        for pos in g.wdisk(from, radius) {
            let Some(tile) = g.map.get(pos) else { continue };
            if g.rules.is_water(tile)
                || !g.rules.is_passable(tile)
                || g.cities.values().any(|c| g.wdist(c.pos, pos) < 4)
                || tile
                    .owner_city
                    .is_some_and(|cid| g.cities[&cid].owner != pid)
            {
                continue;
            }
            let value =
                self.settle_value(g, pid, pos) - g.wdist(from, pos) as f64 * distance_penalty;
            if value >= 12.0 {
                sites.push((pos, value));
            }
        }
        sites.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
        sites
    }

    fn best_settle_site(&self, g: &Game, pid: usize, from: Pos, radius: i32) -> Option<(Pos, f64)> {
        self.settle_sites(g, pid, from, radius).into_iter().next()
    }

    fn best_reachable_settle_site(
        &self,
        g: &Game,
        pid: usize,
        uid: u32,
        radius: i32,
    ) -> Option<(Pos, f64)> {
        let from = g.units[&uid].pos;
        self.settle_sites(g, pid, from, radius)
            .into_iter()
            .take(40)
            .find(|(pos, _)| *pos == from || g.route_step(uid, *pos, 0).is_some())
    }

    fn advanced_settler_step(&mut self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let current = g.units[&uid].pos;
        // Search only the immediate neighborhood for the capital. The target
        // is fixed after the first assessment, preventing a rolling optimum
        // from leading the settler across the map for many compounding turns.
        if g.player_city_ids(pid).is_empty() {
            let target = self.settler_targets.get(&uid).copied().or_else(|| {
                let current_value = self.settle_value(g, pid, current);
                let best = self.best_reachable_settle_site(g, pid, uid, 2);
                let target = best
                    .filter(|(_, value)| *value > current_value + 3.0)
                    .map(|(pos, _)| pos)
                    .unwrap_or(current);
                self.settler_targets.insert(uid, target);
                Some(target)
            });
            if target == Some(current) && g.can_found_city(uid) {
                self.settler_targets.remove(&uid);
                return g.apply(pid, &Action::FoundCity { unit: uid }).is_ok();
            }
            if let Some(target) = target {
                return self.base.step_toward(g, pid, uid, target);
            }
        }
        let valid_target = self.settler_targets.get(&uid).copied().filter(|target| {
            let Some(tile) = g.map.get(*target) else {
                return false;
            };
            !g.rules.is_water(tile)
                && g.rules.is_passable(tile)
                && !g.cities.values().any(|c| g.wdist(c.pos, *target) < 4)
                && tile
                    .owner_city
                    .is_none_or(|cid| g.cities[&cid].owner == pid)
                && (*target == current || g.route_step(uid, *target, 0).is_some())
        });
        let target = valid_target.or_else(|| {
            let local = self.best_reachable_settle_site(g, pid, uid, 8);
            let global = g.players[pid]
                .techs
                .contains("shipbuilding")
                .then(|| g.map.width + g.map.height)
                .and_then(|radius| self.best_reachable_settle_site(g, pid, uid, radius));
            match (local, global) {
                (Some(local), Some(global)) if global.1 > local.1 + 5.0 => Some(global),
                (Some(local), _) => Some(local),
                (None, global) => global,
            }
            .map(|(pos, _)| {
                self.settler_targets.insert(uid, pos);
                pos
            })
        });
        let Some(target) = target else {
            return self.base.settler_step(g, pid, uid);
        };
        if current == target && g.can_found_city(uid) {
            self.settler_targets.remove(&uid);
            return g.apply(pid, &Action::FoundCity { unit: uid }).is_ok();
        }
        if let Some(escort) = g.units[&uid].linked_to.filter(|peer| {
            g.units.get(peer).is_some_and(|escort| {
                g.rules.units[escort.kind.as_str()].domain.as_deref() == Some("sea")
            })
        }) {
            if g.wdist(current, target) == 1 {
                return g.apply(pid, &Action::UnlinkUnits { unit: escort }).is_ok();
            }
            return false;
        }
        let moved = self.base.step_toward(g, pid, uid, target);
        if !moved {
            self.settler_targets.remove(&uid);
        }
        moved
    }

    fn improvement_value(
        &self,
        g: &Game,
        pos: Pos,
        improvement: &str,
        strategy: GrandStrategy,
    ) -> f64 {
        let tile = &g.map.tiles[&pos];
        let spec = &g.rules.improvements[improvement];
        let appeal = g.tile_appeal(pos).max(0) as f64;
        let mut yields = spec.yields;
        yields.gold += spec.effects.get("appeal_gold").copied().unwrap_or(0.0) * appeal;
        let mut value = self.yield_value(yields, strategy);
        if strategy == GrandStrategy::Culture {
            // Tourism is cumulative: delaying a resort or national park by
            // dozens of turns loses visitors that cannot be recovered by an
            // equivalent late-game yield. Treat it as a durable strategic
            // yield so builders seek tourist sites as soon as they unlock.
            let tourism = spec.effects.get("tourism").copied().unwrap_or(0.0)
                + spec.effects.get("appeal_tourism").copied().unwrap_or(0.0) * appeal;
            value += tourism * 35.0;
        }
        if let Some(resource) = &tile.resource {
            value += match g.rules.resources[resource].class.as_str() {
                "luxury" => 14.0,
                "strategic" => 11.0,
                _ => 4.0,
            };
        }
        value
    }

    fn advanced_builder_step(
        &mut self,
        g: &mut Game,
        pid: usize,
        uid: u32,
        strategy: GrandStrategy,
    ) -> bool {
        let current = g.units[&uid].pos;
        let project = g
            .player_city_ids(pid)
            .into_iter()
            .filter_map(|city| {
                g.project_contribution_target(pid, city)
                    .map(|position| (g.wdist(current, position), position, city))
            })
            .min();
        if let Some((_, position, city)) = project {
            self.builder_targets.remove(&uid);
            if current == position && g.can_contribute_project(pid, uid, city) {
                return g
                    .apply(pid, &Action::ContributeProject { unit: uid, city })
                    .is_ok();
            }
            if self.base.step_toward(g, pid, uid, position) {
                return true;
            }
        }
        let repairable = g.map.get(current).is_some_and(|tile| {
            tile.pillaged
                && tile.improvement.is_some()
                && tile
                    .owner_city
                    .and_then(|city| g.cities.get(&city))
                    .is_some_and(|city| city.owner == pid)
        });
        if repairable {
            self.builder_targets.remove(&uid);
            return g
                .apply(pid, &Action::RepairImprovement { unit: uid })
                .is_ok();
        }
        let mut here = g.valid_improvements(pid, current);
        here.retain(|improvement| g.rules.improvements[improvement].builder_buildable);
        here.sort_by(|a, b| {
            self.improvement_value(g, current, b, strategy)
                .partial_cmp(&self.improvement_value(g, current, a, strategy))
                .unwrap()
                .then(a.cmp(b))
        });
        if let Some(improvement) = here.first() {
            self.builder_targets.remove(&uid);
            return g
                .apply(
                    pid,
                    &Action::Improve {
                        unit: uid,
                        improvement: improvement.clone(),
                    },
                )
                .is_ok();
        }
        let reserved: HashSet<Pos> = self
            .builder_targets
            .iter()
            .filter(|(other, _)| **other != uid && g.units.contains_key(other))
            .map(|(_, pos)| *pos)
            .collect();
        let current_target = self.builder_targets.get(&uid).copied().filter(|pos| {
            !reserved.contains(pos)
                && g.valid_improvements(pid, *pos)
                    .iter()
                    .any(|improvement| g.rules.improvements[improvement].builder_buildable)
        });
        let target = current_target.or_else(|| {
            let mut best: Option<(f64, Pos)> = None;
            for cid in g.player_city_ids(pid) {
                for pos in &g.cities[&cid].owned_tiles {
                    if reserved.contains(pos) {
                        continue;
                    }
                    for improvement in g.valid_improvements(pid, *pos) {
                        if !g.rules.improvements[&improvement].builder_buildable {
                            continue;
                        }
                        let score = self.improvement_value(g, *pos, &improvement, strategy)
                            - g.wdist(current, *pos) as f64 * 0.7;
                        if best
                            .map(|(old, bp)| score > old || (score == old && *pos < bp))
                            .unwrap_or(true)
                        {
                            best = Some((score, *pos));
                        }
                    }
                }
            }
            best.map(|(_, pos)| {
                self.builder_targets.insert(uid, pos);
                pos
            })
        });
        target.is_some_and(|pos| self.base.step_toward(g, pid, uid, pos))
    }

    fn advanced_trader_step(
        &self,
        g: &mut Game,
        pid: usize,
        uid: u32,
        strategy: GrandStrategy,
    ) -> bool {
        let current = g.units[&uid].pos;
        if let Some(origin) = g.city_at(current).filter(|cid| g.cities[cid].owner == pid) {
            let target = g
                .cities
                .values()
                .filter(|c| {
                    c.id != origin
                        && !g.is_at_war(pid, c.owner)
                        && g.wdist(g.cities[&origin].pos, c.pos) <= 15
                        && !g
                            .routes
                            .iter()
                            .any(|r| r.origin == origin && r.dest == c.id)
                })
                .max_by(|a, b| {
                    let av = self.yield_value(g.route_yields(a.id, a.owner == pid), strategy);
                    let bv = self.yield_value(g.route_yields(b.id, b.owner == pid), strategy);
                    av.partial_cmp(&bv).unwrap().then_with(|| b.id.cmp(&a.id))
                })
                .map(|c| c.id);
            if let Some(city) = target {
                return g
                    .apply(pid, &Action::TradeRoute { unit: uid, city })
                    .is_ok();
            }
        }
        self.base.trader_step(g, pid, uid)
    }

    fn advanced_missionary_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let Some(religion) = g.players[pid].religion.clone() else {
            return false;
        };
        let current = g.units[&uid].pos;
        let target = g
            .cities
            .values()
            .filter(|city| {
                !g.is_at_war(pid, city.owner) && g.city_religion(city) != Some(religion.as_str())
            })
            .max_by_key(|city| {
                let own_pressure = city.pressure.get(&religion).copied().unwrap_or(0.0);
                let rival_pressure = city
                    .pressure
                    .iter()
                    .filter(|(belief, _)| belief.as_str() != religion)
                    .map(|(_, pressure)| *pressure)
                    .fold(0.0_f64, f64::max);
                let swing = (rival_pressure - own_pressure).clamp(0.0, 500.0) as i32;
                let foreign = (city.owner != pid) as i32;
                let defensive_conversion = (city.owner == pid) as i32 * 170;
                let score = defensive_conversion
                    + foreign * 90
                    + city.pop * 12
                    + city.is_capital as i32 * 18
                    + swing / 10
                    - g.wdist(current, city.pos) * 4;
                (score, std::cmp::Reverse(city.id))
            })
            .map(|city| city.pos);
        let Some(target) = target else { return false };
        if g.wdist(current, target) <= 1 {
            return g.apply(pid, &Action::Spread { unit: uid }).is_ok();
        }
        self.base.step_toward(g, pid, uid, target)
    }

    fn advanced_religious_step(&self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let unit = g.units[&uid].clone();
        let religion = unit
            .religion
            .clone()
            .or_else(|| g.players[pid].religion.clone());
        let legal = g.legal_actions(pid);

        if unit.kind == "apostle" && g.players[pid].religion_beliefs.len() < 4 {
            let objective = self
                .victory_target
                .map(VictoryTarget::strategy)
                .unwrap_or(GrandStrategy::Religion);
            let evangelize = legal
                .iter()
                .filter_map(|action| match action {
                    Action::EvangelizeBelief { unit, belief } if *unit == uid => {
                        let score = match (objective, belief.as_str()) {
                            (GrandStrategy::Science, "wat")
                            | (GrandStrategy::Culture, "cathedral")
                            | (GrandStrategy::Diplomacy, "pagoda")
                            | (GrandStrategy::Conquest, "crusade")
                            | (GrandStrategy::Expansion, "religious_colonization")
                            | (GrandStrategy::Religion, "holy_order") => 300,
                            (GrandStrategy::Conquest, "meeting_house")
                            | (GrandStrategy::Expansion, "gurdwara")
                            | (GrandStrategy::Religion, "mosque") => 240,
                            (_, "holy_order" | "mosque" | "wat" | "pagoda") => 180,
                            _ => 100,
                        };
                        Some((score, std::cmp::Reverse(belief.clone()), action.clone()))
                    }
                    _ => None,
                })
                .max_by_key(|(score, belief, _)| (*score, belief.clone()));
            if let Some((_, _, action)) = evangelize {
                return g.apply(pid, &action).is_ok();
            }
        }

        if unit.kind == "guru" {
            if let Some(action) = legal
                .iter()
                .find(|action| matches!(action, Action::HealReligious { unit } if *unit == uid))
                .cloned()
            {
                return g.apply(pid, &action).is_ok();
            }
        }
        if unit.kind == "inquisitor" {
            if let Some(action) = legal
                .iter()
                .find(|action| matches!(action, Action::RemoveHeresy { unit } if *unit == uid))
                .cloned()
            {
                return g.apply(pid, &action).is_ok();
            }
        }

        let theological = legal
            .iter()
            .filter_map(|action| match action {
                Action::TheologicalAttack { unit, target } if *unit == uid => {
                    let defender_hp = g
                        .units_at(*target)
                        .into_iter()
                        .filter(|other| {
                            let other = &g.units[other];
                            g.rules.units[other.kind.as_str()].class == "religious"
                                && other.religion != religion
                        })
                        .map(|other| g.units[&other].hp)
                        .min()
                        .unwrap_or(100);
                    Some((100 - defender_hp, *target, action.clone()))
                }
                _ => None,
            })
            .max_by_key(|(score, target, _)| (*score, std::cmp::Reverse(*target)));
        if let Some((score, _, action)) = theological {
            if unit.hp >= 55 || score >= 45 {
                return g.apply(pid, &action).is_ok();
            }
        }

        if unit.kind == "apostle"
            && religion.as_ref().is_some_and(|faith| {
                g.player_city_ids(pid)
                    .iter()
                    .any(|cid| g.city_religion(&g.cities[cid]) != Some(faith.as_str()))
            })
        {
            if let Some(action) = legal
                .iter()
                .find(|action| matches!(action, Action::LaunchInquisition { unit } if *unit == uid))
                .cloned()
            {
                return g.apply(pid, &action).is_ok();
            }
        }

        if g.rules.units[unit.kind.as_str()].religious_spread > 0.0 && unit.charges > 0 {
            return self.advanced_missionary_step(g, pid, uid);
        }

        let target = g
            .units
            .values()
            .filter(|other| {
                other.owner != pid
                    && g.rules.units[other.kind.as_str()].class == "religious"
                    && other.religion != religion
            })
            .min_by_key(|other| (g.wdist(unit.pos, other.pos), other.id))
            .map(|other| other.pos)
            .or_else(|| {
                g.players[pid]
                    .holy_city
                    .and_then(|cid| g.cities.get(&cid).map(|city| city.pos))
            });
        target.is_some_and(|target| self.base.step_toward(g, pid, uid, target))
    }

    fn force_domain(g: &Game, uid: u32) -> ForceDomain {
        if g.rules.units[g.units[&uid].kind.as_str()].domain.as_deref() == Some("sea") {
            ForceDomain::Sea
        } else {
            ForceDomain::Land
        }
    }

    fn force_role(g: &Game, uid: u32) -> ForceRole {
        match BasicAi::unit_doctrine(g, uid) {
            UnitDoctrine::Recon => ForceRole::Recon,
            UnitDoctrine::Assault => ForceRole::Vanguard,
            UnitDoctrine::Mobile => ForceRole::Mobile,
            UnitDoctrine::Ranged => ForceRole::Ranged,
            UnitDoctrine::Siege => ForceRole::Siege,
            UnitDoctrine::Support | UnitDoctrine::Carrier => ForceRole::Support,
            UnitDoctrine::AirDefense | UnitDoctrine::AirStrike => ForceRole::AirStrike,
        }
    }

    fn force_anchor(g: &Game, units: &[u32]) -> Pos {
        units
            .iter()
            .map(|uid| {
                let pos = g.units[uid].pos;
                let total: i32 = units
                    .iter()
                    .map(|other| g.wdist(pos, g.units[other].pos))
                    .sum();
                (total, *uid, pos)
            })
            .min()
            .map(|(_, _, pos)| pos)
            .unwrap_or((0, 0))
    }

    fn domain_objective(
        &self,
        g: &Game,
        pid: usize,
        plan: &StrategicPlan,
        domain: ForceDomain,
        anchor: Pos,
        enemies: &[usize],
    ) -> Pos {
        let threatened_enemy = plan.threatened_city.and_then(|cid| {
            let city = g.cities.get(&cid)?;
            g.units
                .values()
                .filter(|unit| {
                    enemies.contains(&unit.owner)
                        && match domain {
                            ForceDomain::Sea => BasicAi::waterborne(g, unit.id),
                            ForceDomain::Land => !BasicAi::waterborne(g, unit.id),
                        }
                        && g.wdist(city.pos, unit.pos) <= 8
                })
                .min_by_key(|unit| (g.wdist(anchor, unit.pos), unit.id))
                .map(|unit| unit.pos)
        });
        if let Some(pos) = threatened_enemy {
            return pos;
        }

        let planned = plan
            .threatened_city
            .or(plan.target_city)
            .and_then(|cid| g.cities.get(&cid).map(|city| city.pos));
        if domain == ForceDomain::Land {
            return planned
                .or_else(|| self.base.nearest_enemy(g, pid, anchor, enemies))
                .unwrap_or(anchor);
        }

        // Fleets interdict hostile ships first. Against a land objective they
        // share the campaign but receive a reachable coastal approach tile.
        if let Some(pos) = g
            .units
            .values()
            .filter(|unit| enemies.contains(&unit.owner) && BasicAi::waterborne(g, unit.id))
            .min_by_key(|unit| (g.wdist(anchor, unit.pos), unit.id))
            .map(|unit| unit.pos)
        {
            return pos;
        }
        // During colonization, a fleet without an immediate contact screens
        // the embarked settler. Once the civilian is linked, its naval leader
        // will carry the pair all the way to the persistent colony objective.
        if let Some(pos) = g
            .units
            .values()
            .filter(|unit| {
                unit.owner == pid
                    && unit.kind == "settler"
                    && g.map
                        .get(unit.pos)
                        .is_some_and(|tile| g.rules.is_water(tile))
            })
            .min_by_key(|unit| (g.wdist(anchor, unit.pos), unit.id))
            .map(|unit| unit.pos)
        {
            return pos;
        }

        let coastal_campaign_city = planned
            .filter(|pos| {
                g.city_at(*pos)
                    .is_some_and(|cid| BasicAi::city_is_coastal(g, cid))
            })
            .or_else(|| {
                g.cities
                    .values()
                    .filter(|city| {
                        enemies.contains(&city.owner) && BasicAi::city_is_coastal(g, city.id)
                    })
                    .min_by_key(|city| (g.wdist(anchor, city.pos), city.id))
                    .map(|city| city.pos)
            });
        coastal_campaign_city
            .and_then(|city_pos| {
                let approach = |radius| {
                    g.wdisk(city_pos, radius)
                        .into_iter()
                        .filter(|pos| {
                            g.map.get(*pos).is_some_and(|tile| {
                                g.rules.is_water(tile)
                                    && g.rules.is_passable(tile)
                                    && (tile.terrain != "ocean"
                                        || g.players[pid].techs.contains("cartography"))
                            })
                        })
                        .min_by_key(|pos| (g.wdist(anchor, *pos), *pos))
                };
                // Adjacent water lets melee ships capture after ranged ships
                // remove defenses. Radius three is only a fallback for cities
                // behind a narrow land/coast configuration.
                approach(1).or_else(|| approach(3))
            })
            .unwrap_or(anchor)
    }

    fn force_focus_target(
        &self,
        g: &Game,
        units: &[u32],
        enemies: &[usize],
        plan: &StrategicPlan,
    ) -> Option<Pos> {
        let mut targets = BTreeSet::new();
        for uid in units {
            let unit = &g.units[uid];
            let spec = &g.rules.units[unit.kind.as_str()];
            if spec.class != "military" {
                continue;
            }
            let radius = if spec.has_ranged_attack() {
                spec.range.max(1)
            } else {
                1
            };
            for pos in g.wdisk(unit.pos, radius) {
                if pos != unit.pos && self.base.is_enemy_tile(g, pos, enemies) {
                    targets.insert(pos);
                }
            }
        }
        targets.into_iter().max_by(|a, b| {
            let value = |target: Pos| -> f64 {
                let mut score = 0.0;
                let mut attackers = 0;
                for uid in units {
                    let unit = &g.units[uid];
                    let spec = &g.rules.units[unit.kind.as_str()];
                    if spec.class != "military" {
                        continue;
                    }
                    let ranged = spec.has_ranged_attack();
                    let radius = if ranged { spec.range.max(1) } else { 1 };
                    if g.wdist(unit.pos, target) <= radius {
                        score += self.base.exchange_score(g, *uid, target, ranged).max(-20.0);
                        attackers += 1;
                    }
                }
                score += attackers as f64 * 8.0;
                if plan
                    .target_city
                    .is_some_and(|cid| g.cities.get(&cid).is_some_and(|city| city.pos == target))
                {
                    score += 35.0;
                }
                if let Some(hp) = g
                    .units_at(target)
                    .iter()
                    .filter_map(|uid| {
                        enemies
                            .contains(&g.units[uid].owner)
                            .then_some(g.units[uid].hp)
                    })
                    .min()
                {
                    score += (100 - hp) as f64 * 0.4;
                }
                score
            };
            value(*a)
                .partial_cmp(&value(*b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.cmp(a))
        })
    }

    fn local_strength_ratio(
        &self,
        g: &Game,
        units: &[u32],
        enemies: &[usize],
        objective: Pos,
    ) -> f64 {
        let friendly: f64 = units
            .iter()
            .filter_map(|uid| {
                let unit = &g.units[uid];
                (g.rules.units[unit.kind.as_str()].class == "military").then_some(
                    crate::game::effective_strength(g.unit_strength(unit, true), unit.hp),
                )
            })
            .sum();
        let hostile: f64 = g
            .units
            .values()
            .filter(|unit| enemies.contains(&unit.owner) && g.wdist(unit.pos, objective) <= 6)
            .filter(|unit| g.rules.units[unit.kind.as_str()].class == "military")
            .map(|unit| crate::game::effective_strength(g.unit_strength(unit, true), unit.hp))
            .sum();
        if hostile <= 0.0 {
            3.0
        } else {
            (friendly / hostile).clamp(0.0, 3.0)
        }
    }

    fn rebuild_force_groups(&mut self, g: &Game, pid: usize, plan: &StrategicPlan) {
        self.force_groups.clear();
        let enemies: Vec<usize> = g
            .players
            .iter()
            .filter(|player| {
                player.id != pid
                    && player.alive
                    && !player.is_barbarian
                    && g.is_at_war(pid, player.id)
            })
            .map(|player| player.id)
            .collect();
        if enemies.is_empty() {
            return;
        }

        let mut remaining: BTreeSet<u32> = g
            .player_unit_ids(pid)
            .into_iter()
            .filter(|uid| {
                let field_unit = matches!(
                    g.rules.units[g.units[uid].kind.as_str()].class.as_str(),
                    "military" | "support"
                );
                field_unit
                    && !(BasicAi::unit_doctrine(g, *uid) == UnitDoctrine::Recon
                        && self.base.has_exploration_target(g, pid, *uid))
            })
            .collect();
        let command_radius = self.base.w.command_radius.round().max(1.0) as i32;
        while let Some(seed) = remaining.iter().next().copied() {
            remaining.remove(&seed);
            let domain = Self::force_domain(g, seed);
            let mut units = vec![seed];
            loop {
                let additions: Vec<u32> = remaining
                    .iter()
                    .copied()
                    .filter(|candidate| {
                        Self::force_domain(g, *candidate) == domain
                            && units.iter().any(|member| {
                                g.wdist(g.units[member].pos, g.units[candidate].pos)
                                    <= command_radius
                            })
                    })
                    .collect();
                if additions.is_empty() {
                    break;
                }
                for uid in additions {
                    remaining.remove(&uid);
                    units.push(uid);
                }
            }
            units.sort_unstable();
            let anchor = Self::force_anchor(g, &units);
            let objective = self.domain_objective(g, pid, plan, domain, anchor, &enemies);
            let focus_target = self.force_focus_target(g, &units, &enemies, plan);
            let muster_radius = self.base.w.muster_radius.round().max(1.0) as i32;
            let readiness = units
                .iter()
                .filter(|uid| {
                    g.wdist(g.units[uid].pos, anchor) <= muster_radius
                        && g.units[uid].hp as f64 > self.base.w.withdraw_hp
                })
                .count() as f64
                / units.len().max(1) as f64;
            let local_strength_ratio = self.local_strength_ratio(g, &units, &enemies, objective);
            let average_hp = units.iter().map(|uid| g.units[uid].hp).sum::<i32>() as f64
                / units.len().max(1) as f64;
            let posture = if average_hp <= self.base.w.withdraw_hp + 10.0 {
                ForcePosture::Recover
            } else if focus_target.is_some()
                || units.iter().any(|uid| {
                    g.units.values().any(|enemy| {
                        enemies.contains(&enemy.owner) && g.wdist(g.units[uid].pos, enemy.pos) <= 2
                    })
                })
            {
                ForcePosture::Engage
            } else if plan.threatened_city.is_some() || local_strength_ratio < 0.72 {
                ForcePosture::Hold
            } else if units.len() > 1 && readiness + 1e-9 < self.base.w.muster_readiness {
                ForcePosture::Muster
            } else {
                ForcePosture::Advance
            };
            self.force_groups.push(ForceGroup {
                id: units[0],
                domain,
                units,
                anchor,
                objective,
                focus_target,
                posture,
                readiness,
                local_strength_ratio,
            });
        }
        self.force_groups.sort_by_key(|group| group.id);
    }

    fn coordinated_tactical_step(
        &self,
        g: &mut Game,
        pid: usize,
        uid: u32,
        group: &ForceGroup,
        enemies: &[usize],
    ) -> bool {
        let unit = &g.units[&uid];
        let upos = unit.pos;
        let role = Self::force_role(g, uid);
        let spec = &g.rules.units[unit.kind.as_str()];
        let target = match group.posture {
            ForcePosture::Muster | ForcePosture::Recover => group.anchor,
            ForcePosture::Engage => group.focus_target.unwrap_or(group.objective),
            _ => group.objective,
        };
        let preferred_depth = match role {
            ForceRole::Recon => spec.range.max(2),
            ForceRole::Vanguard | ForceRole::Mobile => 1,
            ForceRole::Ranged | ForceRole::Siege => spec.range.max(1),
            ForceRole::Support => 2,
            ForceRole::AirStrike => spec.range.max(3),
        };
        let vanguard_depth = group
            .units
            .iter()
            .filter(|other| {
                **other != uid
                    && g.units.contains_key(other)
                    && matches!(
                        Self::force_role(g, **other),
                        ForceRole::Vanguard | ForceRole::Mobile
                    )
            })
            .map(|other| g.wdist(g.units[other].pos, target))
            .min();
        let score = |g: &Game, tile: Pos| -> f64 {
            let objective_distance = g.wdist(tile, target);
            let (progress, cohesion, threat_caution, spacing) = match role {
                ForceRole::Recon => (0.55, 0.40, 1.35, 1.25),
                ForceRole::Vanguard => (1.15, 1.00, 1.00, 1.00),
                ForceRole::Mobile => (1.40, 0.65, 0.80, 1.00),
                ForceRole::Ranged => (0.90, 1.10, 1.15, 1.50),
                ForceRole::Siege => (0.80, 1.30, 1.25, 1.70),
                ForceRole::Support => (0.65, 1.50, 1.40, 1.20),
                ForceRole::AirStrike => (1.20, 0.20, 0.75, 0.50),
            };
            let mut value = -self.base.w.objective_progress * progress * objective_distance as f64;
            let nearest_friend = group
                .units
                .iter()
                .filter(|other| **other != uid && g.units.contains_key(other))
                .map(|other| g.wdist(tile, g.units[other].pos))
                .min();
            if let Some(distance) = nearest_friend {
                value -= self.base.w.cohesion * cohesion * (distance - 2).max(0) as f64;
                if distance == 1 {
                    value += self.base.w.mv_support;
                }
            }
            for enemy in g
                .units
                .values()
                .filter(|other| enemies.contains(&other.owner))
            {
                let enemy_spec = &g.rules.units[enemy.kind.as_str()];
                if enemy_spec.class != "military" {
                    continue;
                }
                let radius = if enemy_spec.has_ranged_attack() {
                    enemy_spec.range.max(1)
                } else {
                    1
                };
                if g.wdist(tile, enemy.pos) <= radius {
                    let attack =
                        crate::game::effective_strength(g.unit_strength(enemy, false), enemy.hp);
                    let defense =
                        crate::game::effective_strength(g.unit_strength(unit, true), unit.hp);
                    value -= self.base.w.mv_threat
                        * threat_caution
                        * 30.0
                        * ((attack - defense) / 25.0).exp();
                }
            }
            if g.wdist(tile, target) <= 5 {
                value -= self.base.w.role_spacing
                    * spacing
                    * (g.wdist(tile, target) - preferred_depth).abs() as f64;
                if matches!(
                    role,
                    ForceRole::Recon | ForceRole::Ranged | ForceRole::Siege | ForceRole::AirStrike
                ) {
                    if let Some(front_depth) = vanguard_depth {
                        value -= self.base.w.screen
                            * (front_depth - g.wdist(tile, target)).max(0) as f64;
                    }
                }
            }
            if group.local_strength_ratio < 1.0 {
                let advance = g.wdist(upos, target) - objective_distance;
                value -= self.base.w.local_superiority
                    * (1.0 - group.local_strength_ratio)
                    * advance.max(0) as f64;
            }
            value
        };

        let stay = score(g, upos);
        let holding_role_position = g.wdist(upos, target) == preferred_depth;
        let mut best: Option<(f64, Pos)> = None;
        for pos in g.nbrs(upos).into_iter().filter(|pos| g.can_move(uid, *pos)) {
            let candidate = score(g, pos);
            if best
                .map(|(old, old_pos)| candidate > old || (candidate == old && pos < old_pos))
                .unwrap_or(true)
            {
                best = Some((candidate, pos));
            }
        }
        if let Some((candidate, pos)) = best {
            let should_move = if holding_role_position {
                candidate > stay + 1e-9
            } else {
                self.base.move_beats_holding(g, uid, candidate, stay)
            };
            if should_move {
                return g.apply(pid, &Action::Move { unit: uid, to: pos }).is_ok();
            }
        }

        let stop_range = if matches!(
            role,
            ForceRole::Recon | ForceRole::Ranged | ForceRole::Siege | ForceRole::AirStrike
        ) {
            preferred_depth
        } else {
            1
        };
        if g.wdist(upos, target) > stop_range {
            if let Some(pos) = g
                .route_step(uid, target, stop_range)
                .filter(|pos| g.can_move(uid, *pos))
            {
                if self.base.move_beats_holding(g, uid, score(g, pos), stay) {
                    return g.apply(pid, &Action::Move { unit: uid, to: pos }).is_ok();
                }
            }
        }
        self.base.fortify_or_stop(g, pid, uid)
    }

    /// Bounded quiescence-style reply search for a proposed attack. The
    /// ordinary exchange evaluator accounts for the target's counter-damage;
    /// this extension makes the move on a cloned position, refreshes only the
    /// enemy's forcing combat actions, and prices its single best reply to the
    /// attacking unit. It therefore catches poisoned captures without turning
    /// every unit decision into an unbounded turn search.
    fn forcing_reply_penalty(&self, g: &Game, pid: usize, uid: u32, action: &Action) -> f64 {
        let mut after = g.clone();
        if after.apply(pid, action).is_err() {
            return 1_000.0;
        }
        let Some(victim) = after.units.get(&uid).cloned() else {
            return 135.0;
        };
        let victim_hp = victim.hp;
        let victim_pos = victim.pos;
        let enemies: Vec<usize> = after
            .players
            .iter()
            .filter(|player| player.id != pid && player.alive && after.is_at_war(pid, player.id))
            .map(|player| player.id)
            .collect();
        let mut worst_reply = 0.0_f64;

        for enemy in enemies {
            let mut reply_position = after.clone();
            reply_position.current = enemy;
            for unit in reply_position
                .units
                .values_mut()
                .filter(|unit| unit.owner == enemy)
            {
                // Only attacks are searched, so a generous movement budget
                // merely restores next-turn attack availability; it cannot
                // manufacture a move-and-attack line in this one-ply search.
                unit.moves_left = 100.0;
                unit.attacks_left = 1;
                unit.acted = false;
                unit.zoc_stopped = false;
            }
            for city in reply_position
                .cities
                .values_mut()
                .filter(|city| city.owner == enemy)
            {
                city.struck = false;
                city.encampment_struck = false;
            }

            let replies: Vec<Action> = reply_position
                .legal_actions(enemy)
                .into_iter()
                .filter(|reply| match reply {
                    Action::Attack { target, .. }
                    | Action::Ranged { target, .. }
                    | Action::AirStrike { target, .. }
                    | Action::CityStrike { target, .. }
                    | Action::EncampmentStrike { target, .. } => *target == victim_pos,
                    _ => false,
                })
                .collect();
            for reply in replies {
                let reply_unit = match &reply {
                    Action::Attack { unit, .. }
                    | Action::Ranged { unit, .. }
                    | Action::AirStrike { unit, .. } => Some(*unit),
                    _ => None,
                };
                let reply_hp =
                    reply_unit.and_then(|unit| reply_position.units.get(&unit).map(|unit| unit.hp));
                let mut branch = reply_position.clone();
                if branch.apply(enemy, &reply).is_err() {
                    continue;
                }
                let loss = branch
                    .units
                    .get(&uid)
                    .map(|unit| (victim_hp - unit.hp).max(0) as f64)
                    .unwrap_or(victim_hp as f64 + 35.0);
                let counter_loss = match (reply_unit, reply_hp) {
                    (Some(unit), Some(hp)) => branch
                        .units
                        .get(&unit)
                        .map(|unit| (hp - unit.hp).max(0) as f64)
                        .unwrap_or(hp as f64 + 20.0),
                    _ => 0.0,
                };
                worst_reply = worst_reply.max((loss - 0.35 * counter_loss).max(0.0));
            }
        }
        worst_reply
    }

    fn advanced_military_step(
        &mut self,
        g: &mut Game,
        pid: usize,
        uid: u32,
        plan: &StrategicPlan,
    ) -> bool {
        let unit = g.units[&uid].clone();
        let spec = g.rules.units[unit.kind.as_str()].clone();
        let doctrine = BasicAi::unit_doctrine(g, uid);
        if self.victory_planning && spec.class == "military" {
            if let Some(target_unit) = g.units_at(unit.pos).into_iter().find(|target| {
                let target = &g.units[target];
                target.owner != pid
                    && g.is_at_war(pid, target.owner)
                    && g.rules.units[target.kind.as_str()].class == "religious"
            }) {
                return g
                    .apply(
                        pid,
                        &Action::CondemnHeretic {
                            unit: uid,
                            target_unit,
                        },
                    )
                    .is_ok();
            }
        }
        let holding_threatened_city = plan.threatened_city.is_some_and(|cid| {
            g.cities
                .get(&cid)
                .is_some_and(|city| g.wdist(unit.pos, city.pos) <= 3)
        });
        if !holding_threatened_city {
            if let Some(acted) = self.base.healing_step(g, pid, uid) {
                return acted;
            }
        }
        if let Some(action) = self.base.doctrine_action(g, pid, uid) {
            return g.apply(pid, &action).is_ok();
        }
        if matches!(doctrine, UnitDoctrine::AirDefense | UnitDoctrine::AirStrike) {
            return false;
        }
        let enemies: Vec<usize> = g
            .players
            .iter()
            .filter(|p| p.id != pid && p.alive && !p.is_barbarian && g.is_at_war(pid, p.id))
            .map(|p| p.id)
            .collect();
        if enemies.is_empty() {
            if spec.domain.as_deref() == Some("sea") {
                if let Some(settler) = unit
                    .linked_to
                    .filter(|peer| g.units.get(peer).is_some_and(|peer| peer.kind == "settler"))
                {
                    if let Some(target) = self.settler_targets.get(&settler).copied() {
                        let approach = BasicAi::naval_approach(g, uid, target).unwrap_or(target);
                        if approach != unit.pos && self.base.step_toward(g, pid, uid, approach) {
                            return true;
                        }
                    }
                    return self.base.fortify_or_stop(g, pid, uid);
                }
            }
            return self.base.military_step(g, pid, uid);
        }
        // Combat can change occupancy, local power, line of sight, and the
        // best focus target after every action. Replan before each unit step
        // so later units exploit the new position instead of following the
        // turn-start snapshot.
        if self.victory_planning {
            self.rebuild_force_groups(g, pid, plan);
        }
        let group = self
            .force_groups
            .iter()
            .find(|group| group.units.contains(&uid))
            .cloned();

        let ranged = spec.has_ranged_attack();
        let radius = if ranged { spec.range.max(1) } else { 1 };
        let decline_settlers =
            self.counts(g, pid).settlers > 0 || g.player_city_ids(pid).len() >= plan.desired_cities;
        let mut best: Option<(f64, Pos)> = None;
        for pos in g.wdisk(unit.pos, radius) {
            if spec.class != "military" {
                break;
            }
            if pos == unit.pos || !self.base.is_enemy_tile(g, pos, &enemies) {
                continue;
            }
            let unusable_settler = g
                .units_at(pos)
                .iter()
                .any(|oid| g.units[oid].kind == "settler" && decline_settlers);
            if unusable_settler && g.city_at(pos).is_none() {
                continue;
            }
            let mut score = self.base.exchange_score(g, uid, pos, ranged)
                - self.base.attack_threshold(g, uid, pos);
            if plan
                .target_city
                .is_some_and(|cid| g.cities.get(&cid).is_some_and(|c| c.pos == pos))
            {
                score += 28.0;
            }
            if g.units_at(pos).iter().any(|oid| g.units[oid].hp <= 35) {
                score += 16.0;
            }
            if group.as_ref().and_then(|orders| orders.focus_target) == Some(pos) {
                score += self.base.w.focus_fire * 10.0;
            }
            if let Some(orders) = &group {
                score -=
                    self.base.w.local_superiority * (1.0 - orders.local_strength_ratio).max(0.0);
            }
            let action = if ranged {
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
            score -= self.base.w.trade_caution * self.forcing_reply_penalty(g, pid, uid, &action);
            if best
                .map(|(old, bp)| score > old || (score == old && pos < bp))
                .unwrap_or(true)
            {
                best = Some((score, pos));
            }
        }
        if let Some((score, pos)) = best {
            let required_margin = if unit.hp < 55 { 12.0 } else { 0.0 };
            if score > required_margin {
                let action = if ranged {
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
                if g.apply(pid, &action).is_ok() {
                    return true;
                }
            }
        }

        let linked_settler = (spec.domain.as_deref() == Some("sea"))
            .then_some(unit.linked_to)
            .flatten()
            .filter(|peer| g.units.get(peer).is_some_and(|peer| peer.kind == "settler"));
        let hostile_water_unit = g
            .units
            .values()
            .any(|enemy| enemies.contains(&enemy.owner) && BasicAi::waterborne(g, enemy.id));
        if !hostile_water_unit {
            if let Some(settler) = linked_settler {
                if let Some(target) = self.settler_targets.get(&settler).copied() {
                    let approach = BasicAi::naval_approach(g, uid, target).unwrap_or(target);
                    if approach != unit.pos && self.base.step_toward(g, pid, uid, approach) {
                        return true;
                    }
                }
                return self.base.fortify_or_stop(g, pid, uid);
            }
        }

        if doctrine == UnitDoctrine::Recon
            && self.base.should_explore(g, pid, uid, true)
            && self.base.explore_step(g, pid, uid)
        {
            return true;
        }

        let defend_target = plan.threatened_city.and_then(|cid| {
            let city = g.cities.get(&cid)?;
            g.units
                .values()
                .filter(|u| enemies.contains(&u.owner) && g.wdist(city.pos, u.pos) <= 7)
                .min_by_key(|u| (g.wdist(unit.pos, u.pos), u.id))
                .map(|u| u.pos)
        });
        let campaign = if spec.domain.as_deref() == Some("sea") {
            defend_target
                .filter(|pos| g.map.get(*pos).is_some_and(|tile| g.rules.is_water(tile)))
                .or_else(|| self.base.nearest_enemy_for_unit(g, pid, uid, &enemies))
        } else {
            defend_target
                .or_else(|| {
                    plan.target_city
                        .and_then(|cid| g.cities.get(&cid).map(|c| c.pos))
                })
                .or_else(|| self.base.nearest_enemy(g, pid, unit.pos, &enemies))
        };
        if let Some(orders) = &group {
            return self.coordinated_tactical_step(g, pid, uid, orders, &enemies);
        }
        match campaign {
            Some(target) => self
                .base
                .tactical_step(g, pid, uid, target, &enemies, radius),
            None => self.base.fortify_or_stop(g, pid, uid),
        }
    }

    fn promotion_value(&self, g: &Game, name: &str, strategy: GrandStrategy) -> f64 {
        let promotion = &g.rules.promotions[name];
        let mut value = promotion.tier as f64 * 4.0;
        for (effect, amount) in &promotion.effects {
            let weight = match effect.as_str() {
                "extra_attacks" => 70.0,
                "range" => 55.0,
                "attack_after_move" => 48.0,
                "move_after_attack" => 42.0,
                "heal_anywhere" => 38.0,
                "escort_mobility" => 32.0,
                "zone_of_control" | "camouflage" => 28.0,
                "movement" => 20.0,
                "support_multiplier" | "flanking_multiplier" => 18.0,
                "sight" | "see_through_woods" => 15.0,
                "pillage_cost" | "scale_cliffs" | "amphibious" => 14.0,
                "woods_move_cost" | "hills_move_cost" => 12.0,
                name if name.starts_with("rock_") && name.ends_with("_levels") => 110.0,
                "rock_nature_venue" | "rock_space_venue" | "rock_surf_venue" => 150.0,
                "rock_nearby_tourism_pct" => 6.0,
                "rock_gold_pct" => 2.5,
                "rock_loyalty_loss" => 1.5,
                "rock_convert_city" if strategy == GrandStrategy::Religion => 350.0,
                "rock_convert_city" => 50.0,
                "combat_all" => 4.0,
                name if name.starts_with("attack_")
                    || name.starts_with("ranged_")
                    || name.starts_with("siege_")
                    || name.starts_with("vs_") =>
                {
                    3.5
                }
                name if name.starts_with("defend_") || name.ends_with("_defense") => 3.0,
                _ => 2.0,
            };
            value += weight * amount;
        }
        match strategy {
            GrandStrategy::Conquest => value * 1.18,
            GrandStrategy::Recovery => {
                value
                    + promotion
                        .effects
                        .iter()
                        .filter(|(effect, _)| {
                            effect.starts_with("defend_") || effect.ends_with("_defense")
                        })
                        .map(|(_, amount)| 2.0 * amount)
                        .sum::<f64>()
            }
            _ => value,
        }
    }

    fn advanced_promotions(&self, g: &mut Game, pid: usize, strategy: GrandStrategy) {
        for uid in g.player_unit_ids(pid) {
            let promotions = g.available_promotions(uid);
            let choice = promotions.into_iter().max_by(|a, b| {
                self.promotion_value(g, a, strategy)
                    .partial_cmp(&self.promotion_value(g, b, strategy))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.cmp(a))
            });
            if let Some(promotion) = choice {
                let _ = g.apply(
                    pid,
                    &Action::Promote {
                        unit: uid,
                        promotion,
                    },
                );
            }
        }
    }

    fn advanced_formations(&self, g: &mut Game, pid: usize) {
        let reserve = (g.player_city_ids(pid).len() + 3).max(5);
        let military: Vec<u32> = g
            .player_unit_ids(pid)
            .into_iter()
            .filter(|uid| g.rules.units[g.units[uid].kind.as_str()].class == "military")
            .collect();
        let max_combinations = military.len().saturating_sub(reserve);
        let mut pairs = Vec::new();
        for (index, unit) in military.iter().enumerate() {
            for with in &military[index + 1..] {
                let a = &g.units[unit];
                let b = &g.units[with];
                let valid_formation = match (a.formation, b.formation) {
                    (0, 0) => g.players[pid].civics.contains("nationalism"),
                    (0, 1) | (1, 0) => g.players[pid].civics.contains("mobilization"),
                    _ => false,
                };
                if a.kind != b.kind
                    || a.linked_to.is_some()
                    || b.linked_to.is_some()
                    || a.moves_left <= 0.0
                    || b.moves_left <= 0.0
                    || g.wdist(a.pos, b.pos) > 1
                    || !valid_formation
                {
                    continue;
                }
                let army = (a.formation.max(b.formation) == 1) as i64;
                let score = army * 100 + a.xp.max(b.xp) + a.hp.max(b.hp) as i64 / 10;
                pairs.push((
                    std::cmp::Reverse(score),
                    (*unit).min(*with),
                    (*unit).max(*with),
                ));
            }
        }
        pairs.sort_unstable();
        let mut used = HashSet::new();
        let mut combined = 0;
        for (_, unit, with) in pairs {
            if combined >= max_combinations || !used.insert(unit) || !used.insert(with) {
                continue;
            }
            if g.apply(pid, &Action::CombineUnits { unit, with }).is_ok() {
                combined += 1;
            }
        }

        let support: Vec<u32> = g
            .player_unit_ids(pid)
            .into_iter()
            .filter(|uid| {
                g.rules.units[g.units[uid].kind.as_str()].class == "support"
                    && g.units[uid].linked_to.is_none()
            })
            .collect();
        for with in support {
            let pos = g.units[&with].pos;
            let escort = g
                .units_at(pos)
                .into_iter()
                .filter(|unit| {
                    let unit = &g.units[unit];
                    unit.owner == pid
                        && unit.linked_to.is_none()
                        && g.rules.units[unit.kind.as_str()].class == "military"
                })
                .max_by_key(|unit| {
                    let unit = &g.units[unit];
                    (
                        !g.rules.units[unit.kind.as_str()].has_ranged_attack(),
                        g.unit_strength(unit, true) as i64,
                        std::cmp::Reverse(unit.id),
                    )
                });
            if let Some(unit) = escort {
                let _ = g.apply(pid, &Action::LinkUnits { unit, with });
            }
        }

        let embarked_settlers: Vec<u32> = g
            .player_unit_ids(pid)
            .into_iter()
            .filter(|uid| {
                let unit = &g.units[uid];
                unit.kind == "settler"
                    && unit.linked_to.is_none()
                    && g.map
                        .get(unit.pos)
                        .is_some_and(|tile| g.rules.is_water(tile))
            })
            .collect();
        for with in embarked_settlers {
            let escort = g.units_at(g.units[&with].pos).into_iter().find(|uid| {
                let unit = &g.units[uid];
                unit.owner == pid
                    && unit.linked_to.is_none()
                    && g.rules.units[unit.kind.as_str()].domain.as_deref() == Some("sea")
            });
            if let Some(unit) = escort {
                let _ = g.apply(pid, &Action::LinkUnits { unit, with });
            }
        }
    }

    fn advanced_encampment_strikes(&self, g: &mut Game, pid: usize) {
        let has_ready_encampment = g.player_city_ids(pid).into_iter().any(|cid| {
            let city = &g.cities[&cid];
            city.encampment_hp > 0
                && city.encampment_wall_hp > 0
                && !city.encampment_pillaged
                && !city.encampment_struck
        });
        if !has_ready_encampment {
            return;
        }
        let mut best: BTreeMap<u32, (i64, Pos)> = BTreeMap::new();
        for action in g.legal_actions(pid) {
            let Action::EncampmentStrike { city, target } = action else {
                continue;
            };
            let target_value = g
                .units_at(target)
                .into_iter()
                .filter(|uid| {
                    let unit = &g.units[uid];
                    unit.owner != pid && g.rules.units[unit.kind.as_str()].class == "military"
                })
                .map(|uid| {
                    let unit = &g.units[&uid];
                    g.unit_strength(unit, true) + (100 - unit.hp) as f64 * 0.6
                })
                .fold(0.0_f64, f64::max) as i64;
            let candidate = (target_value, target);
            if best.get(&city).is_none_or(|old| candidate > *old) {
                best.insert(city, candidate);
            }
        }
        for (city, (_, target)) in best {
            let _ = g.apply(pid, &Action::EncampmentStrike { city, target });
        }
    }

    fn advanced_command_actions(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        self.advanced_encampment_strikes(g, pid);
        self.advanced_promotions(g, pid, plan.strategy);
        self.advanced_formations(g, pid);
    }

    fn advanced_units(&mut self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        if self.victory_planning {
            self.rebuild_force_groups(g, pid, plan);
        } else {
            self.force_groups.clear();
        }
        let mut ids = g.player_unit_ids(pid);
        ids.sort_by_key(|uid| {
            let u = &g.units[uid];
            let spec = &g.rules.units[u.kind.as_str()];
            let order = match u.kind.as_str() {
                "settler" => 0,
                "builder" => 1,
                "naturalist" => 1,
                "archaeologist" => 1,
                "trader" => 2,
                "missionary" => 3,
                "rock_band" => 3,
                _ if spec.has_ranged_attack() && !spec.siege => 4,
                _ if spec.siege => 5,
                _ => 6,
            };
            (order, *uid)
        });
        for uid in ids {
            for _ in 0..8 {
                if !g.units.contains_key(&uid) || g.units[&uid].moves_left <= 0.0 {
                    break;
                }
                let kind = g.units[&uid].kind.clone();
                let class = g.rules.units[kind.as_str()].class.clone();
                let acted = match kind.as_str() {
                    "settler" => self.advanced_settler_step(g, pid, uid),
                    "builder" => self.advanced_builder_step(g, pid, uid, plan.strategy),
                    "naturalist" => self.base.naturalist_step(g, pid, uid),
                    "archaeologist" => self.base.archaeologist_step(g, pid, uid),
                    "trader" => self.advanced_trader_step(g, pid, uid, plan.strategy),
                    "missionary" if self.victory_planning => {
                        self.advanced_missionary_step(g, pid, uid)
                    }
                    "missionary" => self.base.missionary_step(g, pid, uid),
                    "rock_band" => self.base.rock_band_step(g, pid, uid),
                    _ if self.victory_planning && class == "religious" => {
                        self.advanced_religious_step(g, pid, uid)
                    }
                    _ => self.advanced_military_step(g, pid, uid, plan),
                };
                if !acted {
                    break;
                }
            }
        }
        self.settler_targets
            .retain(|uid, _| g.units.contains_key(uid));
        self.builder_targets
            .retain(|uid, _| g.units.contains_key(uid));
    }

    /// Evaluate each legal city disposition on a cloned position, then play
    /// the best resulting state. This is the same separation used by strong
    /// chess engines: generate a very small set of forcing candidates, make
    /// each move, and compare the resulting position with strategy-sensitive
    /// terms instead of relying on a single local rule.
    fn resolve_city_dispositions(&mut self, g: &mut Game, pid: usize, strategy: GrandStrategy) {
        loop {
            let candidates: Vec<Action> = g
                .legal_city_disposition_actions(pid)
                .into_iter()
                .filter(|action| {
                    matches!(
                        action,
                        Action::KeepCity { .. }
                            | Action::RazeCity { .. }
                            | Action::LiberateCity { .. }
                    )
                })
                .collect();
            if candidates.is_empty() {
                break;
            }
            let mut best: Option<(f64, Action)> = None;
            for action in candidates {
                let mut next = g.clone();
                if next.apply(pid, &action).is_err() {
                    continue;
                }
                let value = self.city_disposition_value(g, &next, pid, strategy, &action);
                if best.as_ref().is_none_or(|(old, _)| value > *old + 1e-9) {
                    best = Some((value, action));
                }
            }
            let Some((_, action)) = best else { break };
            if g.apply(pid, &action).is_err() {
                break;
            }
        }
    }

    fn city_disposition_value(
        &self,
        before: &Game,
        after: &Game,
        pid: usize,
        strategy: GrandStrategy,
        action: &Action,
    ) -> f64 {
        if after.winner == Some(pid) {
            return 1_000_000_000.0;
        }
        if after.winner.is_some() {
            return -1_000_000_000.0;
        }
        let player = &after.players[pid];
        let yield_weights = match strategy {
            GrandStrategy::Science => Yields {
                food: 1.0,
                production: 1.5,
                gold: 0.7,
                science: 2.8,
                culture: 0.8,
                faith: 0.3,
            },
            GrandStrategy::Culture => Yields {
                food: 1.0,
                production: 1.2,
                gold: 0.8,
                science: 0.7,
                culture: 2.8,
                faith: 0.8,
            },
            GrandStrategy::Religion => Yields {
                food: 1.0,
                production: 1.1,
                gold: 0.6,
                science: 0.5,
                culture: 0.8,
                faith: 3.0,
            },
            GrandStrategy::Diplomacy => Yields {
                food: 1.0,
                production: 1.0,
                gold: 1.5,
                science: 0.8,
                culture: 1.0,
                faith: 0.5,
            },
            GrandStrategy::Conquest | GrandStrategy::Recovery => Yields {
                food: 1.1,
                production: 2.3,
                gold: 1.0,
                science: 0.8,
                culture: 0.7,
                faith: 0.3,
            },
            GrandStrategy::Expansion => Yields {
                food: 1.7,
                production: 1.8,
                gold: 0.9,
                science: 0.8,
                culture: 0.8,
                faith: 0.4,
            },
        };
        let weighted = |yields: Yields| {
            yields.food * yield_weights.food
                + yields.production * yield_weights.production
                + yields.gold * yield_weights.gold
                + yields.science * yield_weights.science
                + yields.culture * yield_weights.culture
                + yields.faith * yield_weights.faith
        };
        let economy = after
            .player_city_ids(pid)
            .into_iter()
            .map(|city| weighted(after.city_yields(city)))
            .sum::<f64>();
        let grievances = after
            .players
            .iter()
            .filter(|observer| observer.id != pid)
            .map(|observer| observer.grievances.get(&pid).copied().unwrap_or(0.0))
            .sum::<f64>();
        let grievance_weight = if strategy == GrandStrategy::Diplomacy {
            0.75
        } else {
            0.12
        };
        let favor_weight = if strategy == GrandStrategy::Diplomacy {
            2.5
        } else {
            0.15
        };
        let mut value = after.score(pid) as f64 * 6.0
            + economy * 3.0
            + after.military_power(pid) * 0.3
            + player.gold * 0.02
            + player.faith * 0.02
            + player.diplomatic_favor * favor_weight
            + player.dvp as f64 * 140.0
            - grievances * grievance_weight;

        value += match strategy {
            GrandStrategy::Science => {
                player.techs.len() as f64 * 8.0 + player.science_projects.len() as f64 * 80.0
            }
            GrandStrategy::Culture => {
                player.culture_lifetime * 0.02 + player.tourism_lifetime * 0.06
            }
            GrandStrategy::Religion => player.faith * 0.08,
            GrandStrategy::Diplomacy => {
                after
                    .players
                    .iter()
                    .filter(|minor| {
                        minor.is_minor
                            && !minor.is_barbarian
                            && minor.alive
                            && after.suzerain_of(minor.id) == Some(pid)
                    })
                    .count() as f64
                    * 35.0
            }
            GrandStrategy::Conquest => {
                let capitals = after
                    .cities
                    .values()
                    .filter(|city| {
                        city.owner == pid && city.is_capital && city.original_owner != pid
                    })
                    .count() as f64;
                capitals * 180.0 + after.military_power(pid) * 0.25
            }
            GrandStrategy::Expansion => after.player_city_ids(pid).len() as f64 * 20.0,
            GrandStrategy::Recovery => after.player_city_ids(pid).len() as f64 * 12.0,
        };

        let city_id = match action {
            Action::KeepCity { city }
            | Action::RazeCity { city }
            | Action::LiberateCity { city } => *city,
            _ => return value,
        };
        if let Some(city) = before.cities.get(&city_id) {
            let nearest_core = before
                .cities
                .values()
                .filter(|other| other.owner == pid && other.id != city_id)
                .map(|other| before.wdist(city.pos, other.pos))
                .min()
                .unwrap_or(20);
            let development = city.pop.max(1) as f64 * 6.0
                + city.districts.len() as f64 * 12.0
                + city.wonders.len() as f64 * 35.0;
            match action {
                Action::KeepCity { .. } => {
                    value += development;
                    if nearest_core > 9 && city.loyalty <= 50.0 {
                        value -= (nearest_core - 9) as f64 * 5.0;
                    }
                    if strategy == GrandStrategy::Conquest {
                        value += 35.0;
                    }
                }
                Action::RazeCity { .. } => {
                    value -= development * 0.4;
                    if strategy == GrandStrategy::Conquest && nearest_core > 9 && development < 35.0
                    {
                        value += 65.0;
                    }
                }
                Action::LiberateCity { .. } => {
                    if strategy == GrandStrategy::Diplomacy {
                        value += 100.0;
                    }
                    if before.players[city.original_owner].is_minor {
                        value += if strategy == GrandStrategy::Diplomacy {
                            70.0
                        } else {
                            15.0
                        };
                    }
                }
                _ => {}
            }
        }
        value
    }
}

impl Ai for AdvancedAi {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        self.base.minor = g.players[pid].is_minor;
        self.base.barb = g.players[pid].is_barbarian;
        self.base.pursue_religion =
            self.victory_target.is_none() || self.victory_target == Some(VictoryTarget::Religion);
        if self.base.minor || self.base.barb {
            self.base.take_turn(g, pid);
            return;
        }
        self.resolve_city_dispositions(
            g,
            pid,
            self.victory_target
                .map(VictoryTarget::strategy)
                .unwrap_or(GrandStrategy::Expansion),
        );
        self.observe_campaign(g, pid);
        if self.plan_stale(g, pid) {
            self.plan = Some(self.assess(g, pid));
        }
        let plan = self.plan.clone().unwrap();
        self.advanced_research(g, pid, &plan);
        if self.victory_planning {
            let denied_rival = plan
                .target_player
                .filter(|target| self.rival_victory_pressure(g, *target).progress >= 78);
            self.advanced_envoys(g, pid, plan.strategy, denied_rival);
            self.advanced_secret_society(g, pid, plan.strategy);
        }
        // Keep the mature ancillary systems: governments, policies, beliefs,
        // governors, religions, and envoys. Research is already selected.
        self.base.research(g, pid);
        self.base.corporations(g, pid);
        self.advanced_products(g, pid, plan.strategy);
        self.advanced_great_people(g, pid, plan.strategy);
        self.faith_building_spending(g, pid, plan.strategy);
        self.strategic_policies(g, pid, plan.strategy);
        self.advanced_diplomacy(g, pid, &plan);

        // Preserve the proven four-build opening before switching every city
        // to utility planning. This also keeps the frozen baseline comparable.
        if self.base.book_pos < 4 {
            self.base.cities(g, pid);
        } else {
            // Explicit victory-target runs use strategic production directly;
            // otherwise the baseline governor remains the stronger general
            // policy in paired evaluation.
            if self.victory_planning && plan.strategy == GrandStrategy::Religion {
                self.religious_production(g, pid);
                self.religious_spending(g, pid);
            }
            if self.victory_planning && plan.strategy == GrandStrategy::Science {
                self.science_production(g, pid);
            }
            if self.victory_planning && plan.strategy == GrandStrategy::Culture {
                self.culture_spending(g, pid);
            }
            if plan.strategy == GrandStrategy::Recovery || self.victory_target.is_some() {
                self.advanced_production(g, pid, &plan);
            }
            if self.victory_target.is_none() {
                self.base.cities(g, pid);
            }
        }
        if self.victory_planning {
            self.advanced_command_actions(g, pid, &plan);
        }
        self.advanced_units(g, pid, &plan);
        self.resolve_city_dispositions(g, pid, plan.strategy);
        if g.winner.is_none() && g.current == pid {
            let _ = g.apply(pid, &Action::EndTurn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::run_game;

    fn island_colony_game() -> (Game, Pos, Pos) {
        let mut g = Game::new_full(1, 18, 10, 92, 120, 0, false);
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
            .unwrap();
        assert!(g.wdist(source, target) > 8);
        for tile in g.map.tiles.values_mut() {
            tile.terrain = "coast".to_string();
            tile.feature = None;
            tile.hills = false;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.wonder = None;
            tile.owner_city = None;
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

    #[test]
    fn product_search_concentrates_culture_multipliers_without_cycling() {
        let mut game = Game::new_full(1, 20, 14, 92_101, 120, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let origin = game.player_city_ids(0)[0];
        let target_position = game
            .map
            .tiles
            .keys()
            .copied()
            .filter(|position| {
                game.rules.is_passable(&game.map.tiles[position])
                    && !game.rules.is_water(&game.map.tiles[position])
                    && game.map.tiles[position].owner_city.is_none()
                    && game.wdist(game.cities[&origin].pos, *position) >= 4
            })
            .max_by_key(|position| game.wdist(game.cities[&origin].pos, *position))
            .unwrap();
        let second_settler = game.spawn_test_unit("settler", 0, target_position);
        game.apply(
            0,
            &Action::FoundCity {
                unit: second_settler,
            },
        )
        .unwrap();
        let target = game.city_at(target_position).unwrap();
        game.cities
            .get_mut(&origin)
            .unwrap()
            .buildings
            .push("stock_exchange".to_string());
        game.cities
            .get_mut(&origin)
            .unwrap()
            .products
            .push("silk".to_string());
        game.cities.get_mut(&target).unwrap().buildings.extend([
            "stock_exchange".to_string(),
            "monument".to_string(),
            "amphitheater".to_string(),
            "broadcast_center".to_string(),
        ]);

        let ai = AdvancedAi::targeting(VictoryTarget::Culture);
        ai.advanced_products(&mut game, 0, GrandStrategy::Culture);
        assert!(game.cities[&origin].products.is_empty());
        assert_eq!(game.cities[&target].products, vec!["silk"]);

        ai.advanced_products(&mut game, 0, GrandStrategy::Culture);
        assert!(game.cities[&origin].products.is_empty());
        assert_eq!(game.cities[&target].products, vec!["silk"]);
    }

    #[test]
    fn strategic_settler_routes_to_an_island_beyond_the_local_search_radius() {
        let (mut g, source, target) = island_colony_game();
        g.players[0]
            .techs
            .extend(["sailing".to_string(), "shipbuilding".to_string()]);
        let settler = g.spawn_test_unit("settler", 0, source);
        let mut ai = AdvancedAi::new();
        assert!(ai.advanced_settler_step(&mut g, 0, settler));
        assert_eq!(ai.settler_targets.get(&settler), Some(&target));
        assert!(g
            .map
            .get(g.units[&settler].pos)
            .is_some_and(|tile| g.rules.is_water(tile)));
    }

    #[test]
    fn fleet_objective_treats_embarked_enemies_as_naval_contacts() {
        let mut g = Game::new_full(2, 24, 16, 93, 80, 0, false);
        let (anchor, contact) = g
            .map
            .tiles
            .iter()
            .filter(|(_, tile)| g.rules.is_water(tile))
            .find_map(|(pos, _)| {
                g.nbrs(*pos)
                    .into_iter()
                    .find(|neighbor| {
                        g.map
                            .get(*neighbor)
                            .is_some_and(|tile| g.rules.is_water(tile))
                    })
                    .map(|neighbor| (*pos, neighbor))
            })
            .expect("map has adjacent water");
        let embarked = g.spawn_test_unit("settler", 1, contact);
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: g.turn,
        };
        let objective =
            AdvancedAi::new().domain_objective(&g, 0, &plan, ForceDomain::Sea, anchor, &[1]);
        assert_eq!(objective, g.units[&embarked].pos);
    }

    #[test]
    fn fleet_uses_an_adjacent_water_approach_for_coastal_city_capture() {
        let mut g = Game::new_full(2, 24, 16, 94, 80, 0, false);
        for pid in 0..2 {
            g.current = pid;
            let settler = g
                .player_unit_ids(pid)
                .into_iter()
                .find(|uid| g.units[uid].kind == "settler")
                .unwrap();
            g.apply(pid, &Action::FoundCity { unit: settler }).unwrap();
        }
        g.current = 0;
        let target_city = g.player_city_ids(1)[0];
        let target = g.cities[&target_city].pos;
        let approach = g.nbrs(target)[0];
        {
            let tile = g.map.tiles.get_mut(&approach).unwrap();
            tile.terrain = "coast".to_string();
            tile.feature = None;
            tile.hills = false;
        }
        g.players[0].techs.insert("sailing".to_string());
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: Some(target_city),
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: g.turn,
        };
        let objective =
            AdvancedAi::new().domain_objective(&g, 0, &plan, ForceDomain::Sea, approach, &[1]);
        assert_eq!(g.wdist(objective, target), 1);
        assert!(g
            .map
            .get(objective)
            .is_some_and(|tile| g.rules.is_water(tile)));
    }

    #[test]
    fn every_victory_condition_can_be_forced_for_every_major() {
        let g = Game::new(4, 24, 16, 70, 80, 0);
        for target in VictoryTarget::ALL {
            let mut ais = AdvancedAi::fleet_targeting(&g, target);
            assert_eq!(ais.len(), g.players.len());
            for pid in g
                .players
                .iter()
                .filter(|player| !player.is_minor && !player.is_barbarian)
                .map(|player| player.id)
            {
                let ai = &mut ais[pid];
                assert_eq!(ai.victory_target(), Some(target));
                ai.base.minor = false;
                ai.base.barb = false;
                let plan = ai.assess(&g, pid);
                let expected = if target == VictoryTarget::Religion {
                    GrandStrategy::Religion
                } else {
                    GrandStrategy::Expansion
                };
                assert_eq!(plan.strategy, expected, "player {pid} targeting {target:?}");
            }
        }

        // The public parser accepts both victory nouns and result labels.
        assert_eq!("religious".parse(), Ok(VictoryTarget::Religion));
        assert_eq!("diplomatic".parse(), Ok(VictoryTarget::Diplomacy));
        assert_eq!("conquest".parse(), Ok(VictoryTarget::Domination));
    }

    #[test]
    fn explicit_non_diplomatic_targets_do_not_score_congress_points() {
        use crate::game::{CongressResolution, CongressSession};

        let session = || CongressSession {
            convened: 0,
            closes: 5,
            resolutions: vec![
                CongressResolution {
                    id: "world_leader".to_string(),
                    title: "Diplomatic Victory".to_string(),
                    choices: vec!["0".to_string(), "1".to_string()],
                    ballots: BTreeMap::new(),
                },
                CongressResolution {
                    id: "international_aid".to_string(),
                    title: "International Aid".to_string(),
                    choices: vec!["0".to_string(), "1".to_string()],
                    ballots: BTreeMap::new(),
                },
            ],
        };
        let plan = |strategy| StrategicPlan {
            strategy,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: 0,
        };

        let mut science_game = Game::new(2, 24, 16, 77, 80, 0);
        science_game.congress = Some(session());
        AdvancedAi::targeting(VictoryTarget::Science).advanced_diplomacy(
            &mut science_game,
            0,
            &plan(GrandStrategy::Science),
        );
        let science_resolutions = &science_game.congress.as_ref().unwrap().resolutions;
        assert!(!science_resolutions[0].ballots.contains_key(&0));
        assert!(!science_resolutions[1].ballots.contains_key(&0));

        let mut diplomacy_game = Game::new(2, 24, 16, 78, 80, 0);
        diplomacy_game.congress = Some(session());
        AdvancedAi::targeting(VictoryTarget::Diplomacy).advanced_diplomacy(
            &mut diplomacy_game,
            0,
            &plan(GrandStrategy::Diplomacy),
        );
        assert!(diplomacy_game.congress.as_ref().unwrap().resolutions[0]
            .ballots
            .contains_key(&0));
    }

    #[test]
    fn congress_strategy_contests_leaders_and_predicts_competitions() {
        let mut game = Game::new_full(3, 24, 16, 780, 200, 0, false);
        game.players[0].dvp = 10;
        game.players[1].dvp = 18;
        game.players[2].dvp = 3;
        game.players[1].culture_lifetime = 2_000.0;
        let ai = AdvancedAi::new();
        let resolution = |id: &str| CongressResolution {
            id: id.to_string(),
            title: id.to_string(),
            choices: vec!["0".to_string(), "1".to_string(), "2".to_string()],
            ballots: BTreeMap::new(),
        };

        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &resolution("world_leader"),
                GrandStrategy::Expansion,
            ),
            Some("2".to_string())
        );
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &resolution("world_leader"),
                GrandStrategy::Diplomacy,
            ),
            Some("0".to_string())
        );
        assert_eq!(
            ai.congress_choice(&game, 0, &resolution("world_fair"), GrandStrategy::Science,),
            Some("1".to_string())
        );
        assert_eq!(
            ai.congress_choice(&game, 0, &resolution("world_fair"), GrandStrategy::Culture,),
            Some("0".to_string())
        );

        let outcome_resolution = |id: &str, targets: &[&str]| CongressResolution {
            id: id.to_string(),
            title: id.to_string(),
            choices: ["A", "B"]
                .into_iter()
                .flat_map(|outcome| {
                    targets
                        .iter()
                        .map(move |target| format!("{outcome}:{target}"))
                })
                .collect(),
            ballots: BTreeMap::new(),
        };
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution("world_leader", &["0", "1", "2"]),
                GrandStrategy::Expansion,
            ),
            Some("B:1".to_string())
        );
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution("world_leader", &["0", "1", "2"]),
                GrandStrategy::Diplomacy,
            ),
            Some("A:0".to_string())
        );
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution("mercenary_companies", &["production", "gold", "faith"]),
                GrandStrategy::Conquest,
            ),
            Some("B:production".to_string())
        );
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution(
                    "urban_development_treaty",
                    &["campus", "theater_square", "holy_site"],
                ),
                GrandStrategy::Science,
            ),
            Some("A:campus".to_string())
        );
    }

    #[test]
    fn strategic_diplomacy_prices_incoming_deals_and_rejects_victory_leaders() {
        let mut game = Game::new_full(2, 24, 16, 781, 300, 0, false);
        let ai = AdvancedAi::new();
        let mut plan = StrategicPlan {
            strategy: GrandStrategy::Expansion,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: game.turn,
        };
        let expires = game.turn + 10;
        let deal = |give_gold, request_gold, friendship, peace| DiplomaticDeal {
            id: 1,
            from: 1,
            to: 0,
            give_gold,
            request_gold,
            open_borders: false,
            friendship,
            peace,
            alliance: None,
            expires,
        };

        assert!(ai.incoming_deal_value(&game, 0, &deal(0.0, 100.0, true, false), &plan) < 0.0);
        assert!(ai.incoming_deal_value(&game, 0, &deal(10.0, 0.0, true, false), &plan) > 0.0);

        game.players[1].science_projects.extend([
            "launch_earth_satellite".to_string(),
            "launch_moon_landing".to_string(),
            "launch_mars_colony".to_string(),
            "exoplanet_expedition".to_string(),
        ]);
        game.players[1].exoplanet_distance = 42.0;
        assert!(ai.incoming_deal_value(&game, 0, &deal(10.0, 0.0, true, false), &plan) < 0.0);

        game.at_war.insert((0, 1));
        plan.strategy = GrandStrategy::Recovery;
        assert!(ai.incoming_deal_value(&game, 0, &deal(0.0, 100.0, false, true), &plan) > 0.0);
    }

    #[test]
    fn initial_plan_coordinates_expansion() {
        let g = Game::new(2, 24, 16, 71, 80, 0);
        let ai = AdvancedAi::new();
        let plan = ai.assess(&g, 0);
        assert_eq!(plan.strategy, GrandStrategy::Expansion);
        assert!(plan.desired_cities >= 3);
        assert!(plan.target_player.is_some());
    }

    #[test]
    fn strategic_plan_is_stable_inside_assessment_window() {
        let mut g = Game::new(2, 24, 16, 72, 30, 0);
        let mut ai = AdvancedAi::new();
        ai.take_turn(&mut g, 0);
        let first = ai.current_plan().unwrap().clone();
        assert!(!ai.plan_stale(&g, 0));
        assert_eq!(ai.current_plan(), Some(&first));
    }

    #[test]
    fn surprise_wars_and_imminent_victories_interrupt_the_planning_window() {
        let mut game = Game::new_full(3, 30, 18, 721, 300, 0, false);
        for pid in 0..3 {
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.current = pid;
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
        game.turn = 190;
        let mut ai = AdvancedAi::new();
        ai.plan = Some(StrategicPlan {
            strategy: GrandStrategy::Expansion,
            target_player: Some(2),
            target_city: None,
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: game.turn,
        });
        assert!(!ai.plan_stale(&game, 0));

        game.at_war.insert((0, 1));
        assert!(ai.plan_stale(&game, 0), "a surprise war must replan now");

        game.at_war.clear();
        ai.plan.as_mut().unwrap().target_player = Some(1);
        game.players[2].science_projects.extend([
            "launch_earth_satellite".to_string(),
            "launch_moon_landing".to_string(),
            "launch_mars_colony".to_string(),
            "exoplanet_expedition".to_string(),
        ]);
        game.players[2].exoplanet_distance = 42.0;
        assert!(
            ai.plan_stale(&game, 0),
            "an imminent rival victory must replan now"
        );
    }

    #[test]
    fn victory_focus_tracks_religious_diplomatic_and_culture_races() {
        let ai = AdvancedAi::new();

        let mut religion = Game::new(2, 24, 16, 74, 80, 0);
        religion.players[0].religion = Some("Test Faith".to_string());
        assert_eq!(
            ai.victory_focus(&religion, 0).strategy,
            GrandStrategy::Religion
        );
        assert_eq!(
            AdvancedAi::legacy().victory_focus(&religion, 0).strategy,
            GrandStrategy::Science
        );

        let mut diplomacy = Game::new(2, 24, 16, 75, 80, 0);
        diplomacy.players[0].dvp = 14;
        assert_eq!(
            ai.victory_focus(&diplomacy, 0).strategy,
            GrandStrategy::Diplomacy
        );

        let mut culture = Game::new(2, 24, 16, 76, 80, 0);
        culture.players[0].tourism_lifetime = 100_000.0;
        culture.players[1].culture_lifetime = 100.0;
        assert_eq!(
            ai.victory_focus(&culture, 0).strategy,
            GrandStrategy::Culture
        );
    }

    #[test]
    fn rival_pressure_uses_living_civilizations_for_active_victory_races() {
        let mut game = Game::new_full(3, 24, 16, 760, 300, 0, false);
        game.players[1].tourism_lifetime = 300_000.0;
        game.players[0].culture_lifetime = 100.0;
        game.players[2].culture_lifetime = 1_000_000.0;
        game.players[2].alive = false;

        let pressure = AdvancedAi::new().rival_victory_pressure(&game, 1);
        assert_eq!(pressure.strategy, GrandStrategy::Culture);
        assert_eq!(pressure.progress, 100);
    }

    #[test]
    fn strategic_plan_denies_an_imminent_victory_instead_of_farming_a_weak_rival() {
        let establish_capitals = |game: &mut Game| {
            for pid in 0..3 {
                let settler = game
                    .player_unit_ids(pid)
                    .into_iter()
                    .find(|unit| game.units[unit].kind == "settler")
                    .unwrap();
                game.current = pid;
                game.apply(pid, &Action::FoundCity { unit: settler })
                    .unwrap();
            }
            game.current = 0;
            game.turn = 190;
        };

        let mut science = Game::new_full(3, 36, 22, 761, 300, 0, false);
        establish_capitals(&mut science);
        science.players[2].science_projects.extend([
            "launch_earth_satellite".to_string(),
            "launch_moon_landing".to_string(),
            "launch_mars_colony".to_string(),
            "exoplanet_expedition".to_string(),
        ]);
        science.players[2].exoplanet_distance = 42.0;
        let ai = AdvancedAi::new();
        let pressure = ai.rival_victory_pressure(&science, 2);
        assert_eq!(pressure.strategy, GrandStrategy::Science);
        assert!(pressure.progress >= 95);
        let plan = ai.assess(&science, 0);
        assert_eq!(plan.strategy, GrandStrategy::Conquest);
        assert_eq!(plan.target_player, Some(2));

        let mut culture = Game::new_full(3, 36, 22, 762, 300, 0, false);
        establish_capitals(&mut culture);
        culture.players[1].tourism_lifetime = 300_000.0;
        culture.players[0].culture_lifetime = 100.0;
        culture.players[2].culture_lifetime = 100.0;
        let pressure = ai.rival_victory_pressure(&culture, 1);
        assert_eq!(pressure.strategy, GrandStrategy::Culture);
        assert_eq!(pressure.progress, 100);
        let plan = ai.assess(&culture, 0);
        assert_eq!(plan.strategy, GrandStrategy::Culture);
        assert_eq!(plan.target_player, Some(1));
    }

    #[test]
    fn religious_strategy_reconverts_its_core_before_chasing_foreign_cities() {
        let mut game = Game::new_full(2, 30, 18, 763, 200, 0, false);
        for pid in 0..2 {
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.current = pid;
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
        game.players[0].religion = Some("Our Faith".to_string());
        let home = game.player_city_ids(0)[0];
        let foreign = game.player_city_ids(1)[0];
        game.cities
            .get_mut(&home)
            .unwrap()
            .pressure
            .insert("Rival Faith".to_string(), 1_000.0);
        game.cities
            .get_mut(&foreign)
            .unwrap()
            .pressure
            .insert("Rival Faith".to_string(), 1_000.0);
        let missionary = game.spawn_test_unit("missionary", 0, game.cities[&home].pos);
        game.units.get_mut(&missionary).unwrap().religion = Some("Our Faith".to_string());

        assert!(AdvancedAi::new().advanced_missionary_step(&mut game, 0, missionary));
        assert!(
            game.cities[&home]
                .pressure
                .get("Our Faith")
                .copied()
                .unwrap_or(0.0)
                > 0.0
        );
        assert_eq!(game.units[&missionary].pos, game.cities[&home].pos);
    }

    #[test]
    fn apostles_complete_one_worship_and_one_enhancer_belief_for_the_plan() {
        let mut game = Game::new(2, 24, 16, 7_632, 200, 0);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        game.players[0].religion = Some("Planned Faith".to_string());
        game.players[0].religion_beliefs = vec!["work_ethic".to_string(), "tithe".to_string()];
        let ai = AdvancedAi::targeting(VictoryTarget::Science);

        let first = game.spawn_test_unit("apostle", 0, game.cities[&city].pos);
        game.units.get_mut(&first).unwrap().religion = Some("Planned Faith".to_string());
        assert!(ai.advanced_religious_step(&mut game, 0, first));
        assert!(game.players[0]
            .religion_beliefs
            .contains(&"wat".to_string()));

        let second = game.spawn_test_unit("apostle", 0, game.cities[&city].pos);
        game.units.get_mut(&second).unwrap().religion = Some("Planned Faith".to_string());
        assert!(ai.advanced_religious_step(&mut game, 0, second));
        assert_eq!(game.players[0].religion_beliefs.len(), 4);
        assert_eq!(
            game.players[0]
                .religion_beliefs
                .iter()
                .filter(|belief| game.rules.beliefs.enhancer.contains_key(*belief))
                .count(),
            1
        );
        assert_eq!(
            game.players[0]
                .religion_beliefs
                .iter()
                .filter(|belief| game.rules.beliefs.worship.contains_key(*belief))
                .count(),
            1
        );
    }

    #[test]
    fn science_target_reserves_a_spaceport_then_queues_the_project_chain() {
        let mut g = Game::new(2, 24, 16, 71, 200, 0);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| g.units[uid].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = g.player_city_ids(0)[0];
        let site = g.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != g.cities[&city].pos)
            .unwrap();
        {
            let tile = g.map.tiles.get_mut(&site).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.hills = false;
        }
        g.players[0].techs.insert("rocketry".to_string());
        let ai = AdvancedAi::targeting(VictoryTarget::Science);
        ai.science_production(&mut g, 0);
        let spaceport = match g.cities[&city].queue.first() {
            Some(Item::District { district, pos }) if district == "spaceport" => *pos,
            queued => panic!("expected a queued spaceport, got {queued:?}"),
        };

        g.cities.get_mut(&city).unwrap().queue.clear();
        g.cities
            .get_mut(&city)
            .unwrap()
            .districts
            .insert("spaceport".to_string(), spaceport);
        ai.science_production(&mut g, 0);
        assert!(matches!(
            g.cities[&city].queue.first(),
            Some(Item::Project { project }) if project == "launch_earth_satellite"
        ));
    }

    #[test]
    fn culture_production_trains_one_archaeologist_for_available_artifact_slots() {
        let mut game = Game::new(2, 24, 16, 7_100, 1_500, 0);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        game.cities
            .get_mut(&city)
            .unwrap()
            .buildings
            .push("archaeological_museum".to_string());
        game.players[0].civics.insert("natural_history".to_string());
        let site = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != game.cities[&city].pos)
            .unwrap();
        let tile = game.map.tiles.get_mut(&site).unwrap();
        tile.terrain = "plains".to_string();
        tile.feature = None;
        tile.resource = Some("antiquity_site".to_string());
        tile.improvement = None;
        tile.district = None;
        tile.wonder = None;
        let archaeologist_item = Item::Unit {
            unit: "archaeologist".to_string(),
        };
        assert!(game.can_produce(0, city, &archaeologist_item));

        let plan = StrategicPlan {
            strategy: GrandStrategy::Culture,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::targeting(VictoryTarget::Culture);
        ai.advanced_production(&mut game, 0, &plan);
        assert!(
            matches!(
                game.cities[&city].queue.first(),
                Some(Item::Unit { unit }) if unit == "archaeologist"
            ),
            "queued {:?}",
            game.cities[&city].queue.first()
        );

        game.cities.get_mut(&city).unwrap().queue.clear();
        game.spawn_test_unit("archaeologist", 0, game.cities[&city].pos);
        ai.advanced_production(&mut game, 0, &plan);
        assert!(!matches!(
            game.cities[&city].queue.first(),
            Some(Item::Unit { unit }) if unit == "archaeologist"
        ));
    }

    #[test]
    fn project_search_maintains_aged_reactors_and_avoids_dirty_conversion_churn() {
        let mut game = Game::new(2, 24, 16, 7_101, 200, 0);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        game.cities.get_mut(&city).unwrap().buildings =
            vec!["factory".to_string(), "nuclear_power_plant".to_string()];
        let plan = StrategicPlan {
            strategy: GrandStrategy::Expansion,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let counts = EmpireCounts::default();
        let ai = AdvancedAi::new();
        let recommission = Item::Project {
            project: "recommission_reactor".to_string(),
        };

        assert!(ai.production_value(&game, 0, city, &recommission, &plan, &counts) < -9_000.0);
        game.cities.get_mut(&city).unwrap().reactor_age = 30;
        assert!(ai.production_value(&game, 0, city, &recommission, &plan, &counts) > 0.0);

        game.cities.get_mut(&city).unwrap().buildings =
            vec!["factory".to_string(), "oil_power_plant".to_string()];
        game.climate_phase = 6;
        game.players[0]
            .strategic_resources
            .insert("coal".to_string(), 10.0);
        game.players[0]
            .strategic_resources
            .insert("uranium".to_string(), 10.0);
        let coal = Item::Project {
            project: "convert_reactor_to_coal".to_string(),
        };
        let nuclear = Item::Project {
            project: "convert_reactor_to_uranium".to_string(),
        };
        assert!(
            ai.production_value(&game, 0, city, &nuclear, &plan, &counts)
                > ai.production_value(&game, 0, city, &coal, &plan, &counts)
        );
    }

    #[test]
    fn great_person_patronage_buys_close_strategy_races_without_spending_the_reserve() {
        let mut game = Game::new(2, 24, 16, 7_102, 200, 0);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let cost = game.current_great_person("scientist").unwrap().1.cost;
        game.players[0]
            .gpp
            .insert("scientist".to_string(), cost - 5.0);
        let ai = AdvancedAi::targeting(VictoryTarget::Science);

        game.players[0].gold = 250.0;
        ai.advanced_great_people(&mut game, 0, GrandStrategy::Science);
        assert_eq!(
            game.players[0]
                .gp_claimed
                .get("scientist")
                .copied()
                .unwrap_or(0),
            0
        );

        game.players[0].gold = 500.0;
        ai.advanced_great_people(&mut game, 0, GrandStrategy::Science);
        assert_eq!(game.players[0].gp_claimed["scientist"], 1);
        assert_eq!(game.players[0].gold, 425.0);
    }

    #[test]
    fn culture_patronage_waits_for_compatible_great_work_slots() {
        let mut game = Game::new(1, 20, 14, 7_104, 200, 0);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        game.players[0]
            .counters
            .insert("great_work:writing".to_string(), 1);
        let cost = game.current_great_person("writer").unwrap().1.cost;
        game.players[0].gpp.insert("writer".to_string(), cost - 5.0);
        game.players[0].gold = 500.0;
        let ai = AdvancedAi::targeting(VictoryTarget::Culture);

        ai.advanced_great_people(&mut game, 0, GrandStrategy::Culture);
        assert_eq!(
            game.players[0]
                .gp_claimed
                .get("writer")
                .copied()
                .unwrap_or(0),
            0,
            "the occupied Palace slot cannot host another work"
        );

        game.cities
            .get_mut(&city)
            .unwrap()
            .buildings
            .push("amphitheater".to_string());
        ai.advanced_great_people(&mut game, 0, GrandStrategy::Culture);
        assert_eq!(game.players[0].gp_claimed["writer"], 1);
    }

    #[test]
    fn faith_spending_buys_the_victory_aligned_worship_building() {
        let mut game = Game::new(2, 24, 16, 7_103, 200, 0);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let holy_site = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != game.cities[&city].pos)
            .unwrap();
        game.map.tiles.get_mut(&holy_site).unwrap().district = Some("holy_site".to_string());
        game.cities
            .get_mut(&city)
            .unwrap()
            .districts
            .insert("holy_site".to_string(), holy_site);
        game.cities.get_mut(&city).unwrap().buildings =
            vec!["shrine".to_string(), "temple".to_string()];
        game.players[0].religion = Some("Scholastic Faith".to_string());
        game.players[0].religion_beliefs = vec![
            "work_ethic".to_string(),
            "tithe".to_string(),
            "wat".to_string(),
        ];
        game.cities
            .get_mut(&city)
            .unwrap()
            .pressure
            .insert("Scholastic Faith".to_string(), 1_000.0);
        game.players[0].faith = 1_000.0;

        AdvancedAi::targeting(VictoryTarget::Science).faith_building_spending(
            &mut game,
            0,
            GrandStrategy::Science,
        );
        assert!(game.cities[&city].buildings.contains(&"wat".to_string()));
        assert!(game.players[0].faith < 1_000.0);
    }

    #[test]
    fn explicit_targets_replace_early_cards_with_victory_policies() {
        let mut culture = Game::new(2, 24, 16, 78, 200, 0);
        culture.players[0].government = Some("chiefdom".to_string());
        culture.players[0]
            .civics
            .insert("cultural_heritage".to_string());
        culture.players[0]
            .policies
            .extend(["discipline".to_string(), "urban_planning".to_string()]);
        AdvancedAi::targeting(VictoryTarget::Culture).strategic_policies(
            &mut culture,
            0,
            GrandStrategy::Expansion,
        );
        assert!(culture.players[0].policies.contains("heritage_tourism"));
        assert!(culture.players[0].policies.contains("discipline"));
        assert!(!culture.players[0].policies.contains("urban_planning"));

        let mut science = Game::new(2, 24, 16, 79, 200, 0);
        science.players[0].government = Some("chiefdom".to_string());
        science.players[0].civics.insert("space_race".to_string());
        science.players[0]
            .policies
            .extend(["discipline".to_string(), "urban_planning".to_string()]);
        AdvancedAi::targeting(VictoryTarget::Science).strategic_policies(
            &mut science,
            0,
            GrandStrategy::Expansion,
        );
        assert!(science.players[0]
            .policies
            .contains("integrated_space_cell"));
        assert!(science.players[0].policies.contains("urban_planning"));
        assert!(!science.players[0].policies.contains("discipline"));

        let mut reactive = culture.clone();
        reactive.players[0].policies.clear();
        reactive.players[0]
            .policies
            .extend(["discipline".to_string(), "urban_planning".to_string()]);
        AdvancedAi::new().strategic_policies(&mut reactive, 0, GrandStrategy::Culture);
        assert!(reactive.players[0].policies.contains("heritage_tourism"));
    }

    #[test]
    fn explicit_targets_choose_synergistic_secret_societies() {
        for (index, (target, expected)) in [
            (VictoryTarget::Science, "hermetic_order"),
            (VictoryTarget::Culture, "voidsingers"),
            (VictoryTarget::Religion, "voidsingers"),
            (VictoryTarget::Diplomacy, "owls_of_minerva"),
            (VictoryTarget::Domination, "owls_of_minerva"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut game = Game::new(2, 24, 16, 110 + index as u64, 80, 0);
            game.players[0].civics.insert("code_of_laws".to_string());
            let ai = AdvancedAi::targeting(target);
            ai.advanced_secret_society(&mut game, 0, target.strategy());
            assert_eq!(game.players[0].secret_society.as_deref(), Some(expected));
        }
    }

    #[test]
    fn culture_strategy_treats_tourism_as_a_builder_yield() {
        let mut g = Game::new(2, 24, 16, 73, 80, 0);
        let pos = *g.map.tiles.keys().next().unwrap();
        for neighbor in g.nbrs(pos) {
            let tile = g.map.tiles.get_mut(&neighbor).unwrap();
            tile.terrain = "coast".to_string();
            tile.feature = None;
            tile.district = None;
            tile.wonder = None;
            tile.improvement = None;
            tile.pillaged = false;
        }
        assert!(g.tile_appeal(pos) >= 4);
        let ai = AdvancedAi::targeting(VictoryTarget::Culture);

        let resort = ai.improvement_value(&g, pos, "seaside_resort", GrandStrategy::Culture);
        let farm = ai.improvement_value(&g, pos, "farm", GrandStrategy::Culture);

        assert!(resort > farm + 100.0, "resort={resort}, farm={farm}");
    }

    #[test]
    fn diplomatic_strategy_concentrates_envoys_into_a_suzerainty() {
        let mut g = Game::new(2, 24, 16, 77, 80, 2);
        g.players[0].envoys_free = 3;
        AdvancedAi::new().advanced_envoys(&mut g, 0, GrandStrategy::Diplomacy, None);
        assert_eq!(g.players[0].envoys_free, 0);
        assert!(g.players[0].envoys.iter().any(|(_, count)| *count >= 3));
    }

    #[test]
    fn command_phase_spends_promotions_and_links_support() {
        let mut g = Game::new_full(2, 24, 16, 79, 80, 0, false);
        let veteran = g
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| !g.available_promotions(*uid).is_empty())
            .or_else(|| {
                let uid = g.player_unit_ids(0).into_iter().find(|uid| {
                    !g.rules.units[g.units[uid].kind.as_str()]
                        .promotion_class
                        .is_empty()
                })?;
                g.units.get_mut(&uid).unwrap().xp = 15;
                Some(uid)
            })
            .expect("major starts with a promotable military class");
        g.units.get_mut(&veteran).unwrap().xp = 15;
        g.units.get_mut(&veteran).unwrap().hp = 45;
        AdvancedAi::new().advanced_promotions(&mut g, 0, GrandStrategy::Conquest);
        assert_eq!(g.units[&veteran].promotions.len(), 1);
        assert_eq!(g.units[&veteran].hp, 95);
        assert_eq!(g.units[&veteran].moves_left, 0.0);

        let pos = g
            .map
            .tiles
            .iter()
            .find(|(pos, tile)| {
                g.rules.is_passable(tile) && !g.rules.is_water(tile) && g.units_at(**pos).is_empty()
            })
            .map(|(pos, _)| *pos)
            .unwrap();
        let escort = g.spawn_test_unit("warrior", 0, pos);
        let support = g.spawn_test_unit("battering_ram", 0, pos);
        AdvancedAi::new().advanced_formations(&mut g, 0);
        assert_eq!(g.units[&escort].linked_to, Some(support));
        assert_eq!(g.units[&support].linked_to, Some(escort));
    }

    #[test]
    fn command_phase_forms_corps_without_hollowing_out_the_army() {
        let mut g = Game::new_full(2, 24, 16, 80, 80, 0, false);
        g.players[0].civics.insert("nationalism".to_string());
        let pos = g
            .map
            .tiles
            .iter()
            .find(|(_, tile)| g.rules.is_passable(tile) && !g.rules.is_water(tile))
            .map(|(pos, _)| *pos)
            .unwrap();
        for _ in 0..6 {
            g.spawn_test_unit("warrior", 0, pos);
        }
        let before = g
            .player_unit_ids(0)
            .into_iter()
            .filter(|uid| g.rules.units[g.units[uid].kind.as_str()].class == "military")
            .count();
        AdvancedAi::new().advanced_formations(&mut g, 0);
        let military: Vec<u32> = g
            .player_unit_ids(0)
            .into_iter()
            .filter(|uid| g.rules.units[g.units[uid].kind.as_str()].class == "military")
            .collect();
        assert!(military.len() < before);
        assert!(military.len() >= 5);
        assert!(military.iter().any(|uid| g.units[uid].formation == 1));
    }

    #[test]
    fn armies_and_fleets_receive_domain_specific_shared_orders() {
        let mut g = Game::new_full(2, 24, 16, 78, 80, 0, false);
        g.at_war.insert((0, 1));

        let land_target = g
            .map
            .tiles
            .iter()
            .filter(|(pos, tile)| {
                g.rules.is_passable(tile) && !g.rules.is_water(tile) && g.units_at(**pos).is_empty()
            })
            .find_map(|(pos, _)| {
                let ring: Vec<Pos> = g
                    .nbrs(*pos)
                    .into_iter()
                    .filter(|neighbor| {
                        g.map.get(*neighbor).is_some_and(|tile| {
                            g.rules.is_passable(tile)
                                && !g.rules.is_water(tile)
                                && g.units_at(*neighbor).is_empty()
                        })
                    })
                    .collect();
                (ring.len() >= 3).then_some((*pos, ring))
            })
            .expect("test map has an open land engagement");
        let army = [
            g.spawn_test_unit("warrior", 0, land_target.1[0]),
            g.spawn_test_unit("archer", 0, land_target.1[1]),
            g.spawn_test_unit("catapult", 0, land_target.1[2]),
        ];
        g.spawn_test_unit("warrior", 1, land_target.0);

        let sea_target = g
            .map
            .tiles
            .iter()
            .filter(|(pos, tile)| g.rules.is_water(tile) && g.units_at(**pos).is_empty())
            .find_map(|(pos, _)| {
                let ring: Vec<Pos> = g
                    .nbrs(*pos)
                    .into_iter()
                    .filter(|neighbor| {
                        g.map.get(*neighbor).is_some_and(|tile| {
                            g.rules.is_water(tile) && g.units_at(*neighbor).is_empty()
                        })
                    })
                    .collect();
                (ring.len() >= 2).then_some((*pos, ring))
            })
            .expect("test map has an open naval engagement");
        let fleet = [
            g.spawn_test_unit("galley", 0, sea_target.1[0]),
            g.spawn_test_unit("galley", 0, sea_target.1[1]),
        ];
        g.spawn_test_unit("galley", 1, sea_target.0);

        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: g.turn,
        };
        let mut ai = AdvancedAi::new();
        ai.rebuild_force_groups(&g, 0, &plan);

        let army_orders = ai
            .force_groups()
            .iter()
            .find(|group| army.iter().all(|uid| group.units.contains(uid)))
            .expect("combined-arms units should share one army order");
        assert_eq!(army_orders.domain, ForceDomain::Land);
        assert_eq!(army_orders.focus_target, Some(land_target.0));
        assert_eq!(army_orders.posture, ForcePosture::Engage);

        let fleet_orders = ai
            .force_groups()
            .iter()
            .find(|group| fleet.iter().all(|uid| group.units.contains(uid)))
            .expect("nearby ships should share one fleet order");
        assert_eq!(fleet_orders.domain, ForceDomain::Sea);
        assert_eq!(fleet_orders.objective, sea_target.0);
        assert_eq!(fleet_orders.focus_target, Some(sea_target.0));
        assert_eq!(fleet_orders.posture, ForcePosture::Engage);

        ai.advanced_military_step(&mut g, 0, army[0], &plan);
        assert!(matches!(
            g.log.last(),
            Some((0, Action::Attack { unit, target }))
                if *unit == army[0] && *target == land_target.0
        ));
    }

    #[test]
    fn city_state_wars_receive_a_campaign_target_and_combined_arms_orders() {
        let mut g = Game::new_full(2, 24, 16, 96, 80, 1, false);
        let minor = g
            .players
            .iter()
            .find(|player| player.is_minor && !player.is_barbarian)
            .map(|player| player.id)
            .expect("test map has a city-state");
        let target_city = g.player_city_ids(minor)[0];
        let target = g.cities[&target_city].pos;
        let staging = g
            .nbrs(target)
            .into_iter()
            .find(|position| {
                g.map.get(*position).is_some_and(|tile| {
                    g.rules.is_passable(tile)
                        && !g.rules.is_water(tile)
                        && g.units_at(*position).is_empty()
                })
            })
            .expect("city-state needs an open attack front");
        let attackers = [
            g.spawn_test_unit("warrior", 0, staging),
            g.spawn_test_unit("archer", 0, staging),
        ];
        g.at_war.insert((0, minor));

        let mut ai = AdvancedAi::new();
        let plan = ai.assess(&g, 0);
        assert_eq!(plan.target_player, Some(minor));
        assert_eq!(plan.target_city, Some(target_city));

        ai.rebuild_force_groups(&g, 0, &plan);
        let orders = ai
            .force_groups()
            .iter()
            .find(|group| attackers.iter().all(|unit| group.units.contains(unit)))
            .expect("the city-state front should form a shared army order");
        assert_eq!(orders.domain, ForceDomain::Land);
        assert_eq!(orders.objective, target);
        let focus = orders
            .focus_target
            .expect("the army should focus a city-state defender or its city");
        assert!(
            g.city_at(focus)
                .is_some_and(|city| g.cities[&city].owner == minor)
                || g.units_at(focus)
                    .iter()
                    .any(|unit| g.units[unit].owner == minor)
        );
        assert_eq!(orders.posture, ForcePosture::Engage);
    }

    #[test]
    fn coordinated_force_moves_most_routed_units_on_advance() {
        let mut g = Game::new_full(2, 24, 16, 80, 80, 0, false);
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
        let orders = ForceGroup {
            id: army[0],
            domain: ForceDomain::Land,
            units: army.clone(),
            anchor: g.units[&army[0]].pos,
            objective: target,
            focus_target: None,
            posture: ForcePosture::Advance,
            readiness: 1.0,
            local_strength_ratio: 2.0,
        };
        let ai = AdvancedAi::new();
        for uid in &army {
            ai.coordinated_tactical_step(&mut g, 0, *uid, &orders, &[1]);
        }
        let moved = army.iter().filter(|uid| g.units[uid].moved).count();
        assert!(
            moved * 2 > army.len(),
            "expected most coordinated troops to advance; moved {moved}/{}",
            army.len()
        );
    }

    #[test]
    fn recon_explores_independently_while_combat_roles_form_the_army() {
        let mut g = Game::new_full(2, 24, 16, 81, 80, 0, false);
        g.at_war.insert((0, 1));
        let positions: Vec<Pos> = g
            .map
            .tiles
            .iter()
            .filter(|(pos, tile)| {
                g.rules.is_passable(tile) && !g.rules.is_water(tile) && g.units_at(**pos).is_empty()
            })
            .map(|(pos, _)| *pos)
            .take(6)
            .collect();
        let scout = g.spawn_test_unit("scout", 0, positions[0]);
        let vanguard = g.spawn_test_unit("swordsman", 0, positions[1]);
        let mobile = g.spawn_test_unit("horseman", 0, positions[2]);
        let ranged = g.spawn_test_unit("archer", 0, positions[3]);
        let siege = g.spawn_test_unit("catapult", 0, positions[4]);
        let support = g.spawn_test_unit("battering_ram", 0, positions[5]);
        assert_eq!(AdvancedAi::force_role(&g, scout), ForceRole::Recon);
        assert_eq!(AdvancedAi::force_role(&g, vanguard), ForceRole::Vanguard);
        assert_eq!(AdvancedAi::force_role(&g, mobile), ForceRole::Mobile);
        assert_eq!(AdvancedAi::force_role(&g, ranged), ForceRole::Ranged);
        assert_eq!(AdvancedAi::force_role(&g, siege), ForceRole::Siege);
        assert_eq!(AdvancedAi::force_role(&g, support), ForceRole::Support);

        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: g.turn,
        };
        let mut ai = AdvancedAi::new();
        ai.rebuild_force_groups(&g, 0, &plan);
        assert!(
            ai.force_groups()
                .iter()
                .all(|group| !group.units.contains(&scout)),
            "recon with unexplored terrain should not make the army wait to muster"
        );
        assert!(ai
            .force_groups()
            .iter()
            .any(|group| group.units.contains(&vanguard)));
    }

    #[test]
    fn forcing_reply_search_avoids_a_poisoned_capture() {
        let mut g = Game::new_full(2, 24, 16, 8_117, 80, 0, false);
        g.at_war.insert((0, 1));
        g.current = 0;
        let (anchor, risky, safe, reply_square) = g
            .map
            .tiles
            .iter()
            .filter(|(position, tile)| {
                g.rules.is_passable(tile)
                    && !g.rules.is_water(tile)
                    && g.units_at(**position).is_empty()
                    && g.city_at(**position).is_none()
                    && g.cities
                        .values()
                        .all(|city| g.wdist(**position, city.pos) > 5)
                    && g.units
                        .values()
                        .filter(|unit| unit.owner == 1)
                        .all(|unit| g.wdist(**position, unit.pos) > 5)
            })
            .find_map(|(anchor, _)| {
                let targets: Vec<Pos> = g
                    .nbrs(*anchor)
                    .into_iter()
                    .filter(|position| {
                        g.map.get(*position).is_some_and(|tile| {
                            g.rules.is_passable(tile) && !g.rules.is_water(tile)
                        }) && g.units_at(*position).is_empty()
                            && g.city_at(*position).is_none()
                    })
                    .collect();
                for risky in &targets {
                    for safe in &targets {
                        if risky == safe || g.wdist(*risky, *safe) < 2 {
                            continue;
                        }
                        if let Some(reply) = g.wdisk(*risky, 2).into_iter().find(|reply| {
                            g.wdist(*risky, *reply) == 2
                                && g.wdist(*safe, *reply) > 2
                                && *reply != *anchor
                                && g.map.get(*reply).is_some_and(|tile| {
                                    g.rules.is_passable(tile) && !g.rules.is_water(tile)
                                })
                                && g.units_at(*reply).is_empty()
                                && g.city_at(*reply).is_none()
                        }) {
                            return Some((*anchor, *risky, *safe, reply));
                        }
                    }
                }
                None
            })
            .expect("test map has an isolated poisoned-capture geometry");

        for position in g.wdisk(risky, 2) {
            let tile = g.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
        }
        let attacker = g.spawn_test_unit("swordsman", 0, anchor);
        let risky_defender = g.spawn_test_unit("warrior", 1, risky);
        let safe_defender = g.spawn_test_unit("warrior", 1, safe);
        g.units.get_mut(&risky_defender).unwrap().hp = 1;
        g.units.get_mut(&safe_defender).unwrap().hp = 1;
        g.spawn_test_unit("archer", 1, reply_square);

        let risky_action = Action::Attack {
            unit: attacker,
            target: risky,
        };
        let safe_action = Action::Attack {
            unit: attacker,
            target: safe,
        };
        let mut ai = AdvancedAi::legacy();
        let risky_reply = ai.forcing_reply_penalty(&g, 0, attacker, &risky_action);
        let safe_reply = ai.forcing_reply_penalty(&g, 0, attacker, &safe_action);
        assert!(
            risky_reply > safe_reply + 5.0,
            "the ranged recapture must make the exposed kill materially worse"
        );

        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: g.turn,
        };
        assert!(ai.advanced_military_step(&mut g, 0, attacker, &plan));
        assert!(!g.units.contains_key(&safe_defender));
        assert!(g.units.contains_key(&risky_defender));
        assert_eq!(g.units[&attacker].pos, safe);
    }

    #[test]
    fn force_replans_focus_after_each_battlefield_action() {
        let mut g = Game::new_full(2, 24, 16, 79, 80, 0, false);
        g.at_war.insert((0, 1));
        let (first_target, second_target, firing_line) = g
            .map
            .tiles
            .iter()
            .filter(|(pos, tile)| {
                g.rules.is_passable(tile) && !g.rules.is_water(tile) && g.units_at(**pos).is_empty()
            })
            .find_map(|(first, _)| {
                g.nbrs(*first).into_iter().find_map(|second| {
                    let second_tile = g.map.get(second)?;
                    if !g.rules.is_passable(second_tile)
                        || g.rules.is_water(second_tile)
                        || !g.units_at(second).is_empty()
                    {
                        return None;
                    }
                    let second_neighbors = g.nbrs(second);
                    let common: Vec<Pos> = g
                        .nbrs(*first)
                        .into_iter()
                        .filter(|pos| second_neighbors.contains(pos))
                        .filter(|pos| {
                            g.map.get(*pos).is_some_and(|tile| {
                                g.rules.is_passable(tile)
                                    && !g.rules.is_water(tile)
                                    && g.units_at(*pos).is_empty()
                            })
                        })
                        .collect();
                    (common.len() >= 2).then_some((*first, second, common))
                })
            })
            .expect("test map has a two-target engagement with a shared front");

        let attackers = [
            g.spawn_test_unit("warrior", 0, firing_line[0]),
            g.spawn_test_unit("warrior", 0, firing_line[1]),
        ];
        let first_enemy = g.spawn_test_unit("warrior", 1, first_target);
        g.units.get_mut(&first_enemy).unwrap().hp = 1;
        g.spawn_test_unit("warrior", 1, second_target);
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: g.turn,
        };
        let mut ai = AdvancedAi::new();
        ai.rebuild_force_groups(&g, 0, &plan);
        let initial = ai
            .force_groups()
            .iter()
            .find(|group| attackers.iter().all(|uid| group.units.contains(uid)))
            .unwrap();
        assert_eq!(initial.focus_target, Some(first_target));

        assert!(ai.advanced_military_step(&mut g, 0, attackers[0], &plan));
        assert!(!g.units.contains_key(&first_enemy));
        assert!(ai.advanced_military_step(&mut g, 0, attackers[1], &plan));
        let replanned = ai
            .force_groups()
            .iter()
            .find(|group| group.units.contains(&attackers[1]))
            .unwrap();
        assert_eq!(replanned.focus_target, Some(second_target));
        assert!(matches!(
            g.log.last(),
            Some((0, Action::Attack { unit, target }))
                if *unit == attackers[1] && *target == second_target
        ));
    }

    #[test]
    fn advanced_selfplay_completes() {
        let mut g = Game::new(2, 20, 14, 73, 65, 1);
        let mut ais = AdvancedAi::fleet(&g);
        run_game(&mut g, &mut ais);
        assert!(g.winner.is_some());
        assert!(g
            .players
            .iter()
            .filter(|p| !p.is_minor && p.alive)
            .all(|p| p.techs.len() > 1));
        // Captured enemy Settlers legitimately make the on-map total exceed
        // one. Guard the behavior this test actually cares about: the AI must
        // never manufacture an accumulating backlog of its own Settlers.
        for player in g.players.iter().filter(|p| !p.is_minor && p.alive) {
            let produced = g
                .log
                .iter()
                .filter(|(pid, action)| {
                    *pid == player.id
                        && matches!(
                            action,
                            Action::Produce {
                                item: Item::Unit { unit },
                                ..
                            } if unit == "settler"
                        )
                })
                .count();
            let founded = g
                .log
                .iter()
                .filter(|(pid, action)| {
                    *pid == player.id && matches!(action, Action::FoundCity { .. })
                })
                .count();
            assert!(
                produced <= founded + 1,
                "advanced AI accumulated self-produced Settlers: player {}, {produced} produced and {founded} used",
                player.id
            );
        }
    }

    #[test]
    fn disposition_search_liberates_for_diplomacy_but_keeps_a_developed_conquest_prize() {
        let captured_city_state = |seed| {
            let mut game = Game::new_full(3, 26, 16, seed, 80, 1, false);
            let minor = game
                .players
                .iter()
                .find(|player| player.is_minor && !player.is_barbarian)
                .unwrap()
                .id;
            let city = game.player_city_ids(minor)[0];
            let captured = game.cities.get_mut(&city).unwrap();
            captured.owner = 0;
            captured.captured_from = Some(1);
            captured.loyalty = 50.0;
            (game, minor, city)
        };

        let (mut diplomatic, minor, city) = captured_city_state(106);
        let mut ai = AdvancedAi::targeting(VictoryTarget::Diplomacy);
        ai.resolve_city_dispositions(&mut diplomatic, 0, GrandStrategy::Diplomacy);
        assert_eq!(diplomatic.cities[&city].owner, minor);
        assert_eq!(diplomatic.players[0].diplomatic_favor, 100.0);

        let (mut conquest, _, city) = captured_city_state(107);
        conquest.cities.get_mut(&city).unwrap().pop = 10;
        let mut ai = AdvancedAi::targeting(VictoryTarget::Domination);
        ai.resolve_city_dispositions(&mut conquest, 0, GrandStrategy::Conquest);
        assert_eq!(conquest.cities[&city].owner, 0);
        assert_eq!(conquest.cities[&city].captured_from, None);
    }
}
