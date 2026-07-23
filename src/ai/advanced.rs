//! Stateful, hierarchical AI for major civilizations.
//!
//! `BasicAi` deliberately remains the small deterministic baseline.  This
//! agent adds a shared strategic model so research, production, diplomacy,
//! civilian work, and military movement pursue the same medium-term goal.
use super::{Ai, BasicAi, ForceReport, PlanReport, UnitDoctrine, Weights};
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

impl GrandStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            GrandStrategy::Expansion => "expansion",
            GrandStrategy::Science => "science",
            GrandStrategy::Culture => "culture",
            GrandStrategy::Religion => "religion",
            GrandStrategy::Diplomacy => "diplomacy",
            GrandStrategy::Conquest => "conquest",
            GrandStrategy::Recovery => "recovery",
        }
    }
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

impl ForceDomain {
    pub fn as_str(self) -> &'static str {
        match self {
            ForceDomain::Land => "land",
            ForceDomain::Sea => "sea",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForcePosture {
    Muster,
    Advance,
    Engage,
    Hold,
    Recover,
}

impl ForcePosture {
    pub fn as_str(self) -> &'static str {
        match self {
            ForcePosture::Muster => "muster",
            ForcePosture::Advance => "advance",
            ForcePosture::Engage => "engage",
            ForcePosture::Hold => "hold",
            ForcePosture::Recover => "recover",
        }
    }
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
    air_defense: usize,
    military_engineers: usize,
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
            "military_engineer" => {
                self.support += 1;
                self.military_engineers += 1;
            }
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
                    } else {
                        if spec.is_melee_capable() {
                            self.melee += 1;
                        }
                        if spec.has_ranged_attack() {
                            self.ranged += 1;
                        }
                    }
                    if spec.siege && spec.domain.as_deref() != Some("air") {
                        self.siege += 1;
                    }
                } else if spec.class == "support" {
                    self.support += 1;
                    if spec.anti_air_strength > 0.0 {
                        self.air_defense += 1;
                    }
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

    /// Redirect an existing agent at a new explicit victory target without
    /// discarding campaign memory; the strategic plan re-assesses on the
    /// next turn. Used by the rollout-driven `StrategicAi`.
    pub fn retarget(&mut self, target: VictoryTarget) {
        self.victory_target = Some(target);
        self.plan = None;
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
            let emergency_target = g
                .emergency_objective(pid)
                .is_some_and(|objective| objective.target == target);
            if !g.is_at_war(pid, target)
                && !emergency_target
                && !self.campaign_target_legal(g, pid, target)
            {
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
        if plan.threatened_city != self.threatened_city(g, pid) {
            return true;
        }
        if let Some((rival, counter)) = self.victory_denial(g, pid) {
            let expects_hostile_target = self.campaign_target_legal(g, pid, rival);
            if (expects_hostile_target && plan.target_player != Some(rival))
                || (!expects_hostile_target && plan.target_player == Some(rival))
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
                let hostile: f64 = g
                    .units
                    .values()
                    .filter(|unit| unit.owner != pid && g.is_at_war(pid, unit.owner))
                    .filter(|unit| g.wdist(city.pos, unit.pos) <= 6)
                    .filter(|unit| g.rules.units[unit.kind.as_str()].class == "military")
                    .map(|unit| {
                        crate::game::effective_strength(g.unit_strength(unit, false), unit.hp)
                    })
                    .sum();
                if hostile <= 0.0 {
                    return None;
                }
                let friendly = g.city_strength(cid)
                    + g.units
                        .values()
                        .filter(|unit| unit.owner == pid && g.wdist(city.pos, unit.pos) <= 6)
                        .filter(|unit| g.rules.units[unit.kind.as_str()].class == "military")
                        .map(|unit| {
                            crate::game::effective_strength(
                                g.unit_strength(unit, true),
                                unit.hp,
                            )
                        })
                        .sum::<f64>();
                let danger = hostile / friendly.max(1.0);
                let recently_hit =
                    city.last_attacked > 0 && g.turn.saturating_sub(city.last_attacked) <= 3;
                let wall_max = g.city_max_wall_hp(city);
                let damaged = city.hp < 200 || city.wall_hp < wall_max;
                let breached = city.hp < 160
                    || (wall_max > 0 && city.wall_hp.saturating_mul(2) < wall_max);
                // A scout or losing skirmisher in the outer city radius is a
                // tactical contact, not an empire-wide emergency. Recovery is
                // reserved for a locally competitive force or a damaged city
                // whose remaining defenders cannot safely absorb another hit.
                let critical = danger >= 0.90
                    || (danger >= 0.45 && (breached || (recently_hit && damaged)));
                critical.then_some((
                    danger,
                    (200 - city.hp).max(0) + (wall_max - city.wall_hp).max(0),
                    cid,
                ))
            })
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| left.1.cmp(&right.1))
                    .then_with(|| right.2.cmp(&left.2))
            })
            .map(|(_, _, cid)| cid)
    }

    fn religious_opening_rank(g: &Game, pid: usize) -> Option<(u8, f64, f64)> {
        let player = &g.players[pid];
        if !player.alive
            || player.is_minor
            || player.is_barbarian
            || player.religion.is_some()
            || player.prophet_pending
            || g.player_city_ids(pid).len() < 2
        {
            return None;
        }
        let city_ids = g.player_city_ids(pid);
        let has_holy_site = city_ids
            .iter()
            .any(|cid| g.cities[cid].districts.contains_key("holy_site"));
        let holy_site_planned = city_ids.iter().any(|cid| {
            matches!(
                g.cities[cid].queue.first(),
                Some(Item::District { district, .. }) if district == "holy_site"
            )
        });
        let best_site = city_ids
            .iter()
            .flat_map(|cid| g.district_sites(*cid, "holy_site"))
            .map(|pos| g.district_yields("holy_site", pos).faith)
            .max_by(f64::total_cmp);
        if !has_holy_site && !holy_site_planned && best_site.is_none() {
            return None;
        }
        // Once an empire has paid toward the race, keep that commitment ahead
        // of an uninvested late entrant. The remaining comparisons select the
        // best available Holy Site and faith economy instead of requiring the
        // unusually rare +3 adjacency that previously left most maps with a
        // single founder.
        let commitment = if has_holy_site {
            4
        } else if holy_site_planned {
            3
        } else if player.techs.contains("astrology") {
            2
        } else if player.research.as_deref() == Some("astrology") {
            1
        } else {
            0
        };
        Some((commitment, best_site.unwrap_or(0.0), player.faith))
    }

    fn religious_opening_viable(&self, g: &Game, pid: usize) -> bool {
        let player = &g.players[pid];
        if player.religion.is_some() {
            return false;
        }
        if player.prophet_pending {
            return true;
        }
        let founded = g.religions_founded();
        let pending = g
            .players
            .iter()
            .filter(|candidate| candidate.prophet_pending)
            .count();
        let claimed = founded + pending;
        if claimed >= g.max_religions()
            || g.turn > if founded > 0 { 180 } else { 120 }
            || Self::religious_opening_rank(g, pid).is_none()
        {
            return false;
        }

        // Prophet slots are a global race. Let exactly the best uncommitted
        // contenders pursue the slots that remain, while still allowing a
        // newly founded rival religion to trigger a genuine counter-race.
        let open_slots = g.max_religions() - claimed;
        let mut contenders: Vec<_> = g
            .players
            .iter()
            .filter_map(|candidate| {
                Self::religious_opening_rank(g, candidate.id).map(|rank| (candidate.id, rank))
            })
            .collect();
        contenders.sort_by(|(left_id, left), (right_id, right)| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.total_cmp(&left.1))
                .then_with(|| right.2.total_cmp(&left.2))
                .then_with(|| left_id.cmp(right_id))
        });
        contenders
            .into_iter()
            .take(open_slots)
            .any(|(contender, _)| contender == pid)
    }

    fn rocketry_readiness(&self, g: &Game, pid: usize) -> i32 {
        let player = &g.players[pid];
        let rocketry_path: Vec<_> = g
            .rules
            .techs
            .keys()
            .filter(|tech| self.tech_leads_to(g, tech, "rocketry"))
            .collect();
        let completed = rocketry_path
            .iter()
            .filter(|tech| player.techs.contains(tech.as_str()))
            .count();
        25 + (40 * completed / rocketry_path.len().max(1)) as i32
    }

    fn diplomatic_science_backup(&self, g: &Game, pid: usize, plan: &StrategicPlan) -> bool {
        self.victory_target.is_none()
            && g.victory_conditions.science
            && plan.strategy == GrandStrategy::Diplomacy
            && g.turn >= g.standard_duration(220)
            && self.rocketry_readiness(g, pid) >= 45
    }

    fn religious_conversion_tally(&self, g: &Game, pid: usize) -> (usize, usize) {
        let living_majors: Vec<usize> = g
            .players
            .iter()
            .filter(|player| player.alive && !player.is_minor && !player.is_barbarian)
            .map(|player| player.id)
            .collect();
        let converted = g.players[pid].religion.as_ref().map_or(0, |religion| {
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
        (converted, living_majors.len())
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

        // A science race starts long before the first launch.  Treating every
        // pre-space empire as exactly 25% complete made a strong researcher
        // abandon science as soon as even modest tourism appeared.  The tech
        // tree is public victory-screen information and gives the planner a
        // smooth signal until the discrete space-race milestones take over.
        let tech_progress =
            25 + (30 * player.techs.len() / g.rules.techs.len().max(1)).min(30) as i32;
        let project_progress = player.science_projects.len().min(4) as i32 * 18;
        let travel_progress = if player.science_projects.contains("exoplanet_expedition") {
            (player.exoplanet_distance * 100.0 / 50.0).clamp(0.0, 100.0) as i32
        } else {
            0
        };
        // A launch program cannot raise project progress until the AI has
        // already chosen Science long enough to unlock Rocketry and build a
        // Spaceport. Count progress along that prerequisite path so adaptive
        // agents can make the initial commitment instead of remaining stuck
        // at the old 25% floor forever.
        let readiness = self.rocketry_readiness(g, pid);
        let science = tech_progress
            .max(readiness)
            .max(project_progress)
            .max(travel_progress)
            .max((player.civ == "China") as i32 * 45);

        let culture_target = living_majors
            .iter()
            .filter(|other| **other != pid)
            .map(|other| g.domestic_tourists(*other))
            .max()
            .unwrap_or(1)
            .max(1);
        let culture = ((100 * g.foreign_tourists(pid) / culture_target).clamp(0, 100)) as i32;

        let (converted, living_religious_rivals) = self.religious_conversion_tally(g, pid);
        let religion = if player.religion.is_some() {
            // Founding a religion normally converts the founder's own small
            // empire first. That is table stakes, not progress against a
            // Religious Victory: counting it made every founder jump from the
            // 40-point commitment floor to 55 in a four-player game before it
            // had converted a single rival. Measure expansion into foreign
            // civilizations here; rival threat scoring below still counts all
            // living majors because the actual victory rule does too.
            let religion = player.religion.as_deref().unwrap();
            let own_cities = g.player_city_ids(pid);
            let own_following = own_cities
                .iter()
                .filter(|city| g.city_religion(&g.cities[city]) == Some(religion))
                .count();
            let own_converted = !own_cities.is_empty() && own_following * 2 > own_cities.len();
            let foreign_converted = converted.saturating_sub(usize::from(own_converted));
            let foreign_rivals = living_religious_rivals.saturating_sub(1);
            40 + (60 * foreign_converted / foreign_rivals.max(1)) as i32
        } else if self.religious_opening_viable(g, pid) {
            if g.religions_founded() > 0 {
                55
            } else {
                46
            }
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

        let (converted, living_religious_rivals) = self.religious_conversion_tally(g, pid);
        let religion = if player.religion.is_some() {
            (100 * converted / living_religious_rivals.max(1)) as i32
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
        // Religious progress advances in whole-civilization jumps, and a
        // defender needs time to produce and route religious counters. Start
        // reacting with two holdouts left when the rival also leads our own
        // race, then treat one remaining holdout as an unconditional match
        // point: a slower "close" victory must not suppress that interrupt.
        if pressure.strategy == GrandStrategy::Religion {
            let living = g
                .players
                .iter()
                .filter(|player| player.alive && !player.is_minor && !player.is_barbarian)
                .count()
                .max(1) as i32;
            let match_point = 100 * living.saturating_sub(1) / living;
            let early_warning = (100 * living.saturating_sub(2) / living)
                .max(50)
                .min(match_point);
            if pressure.progress < early_warning
                || (pressure.progress < match_point && pressure.progress < own_progress + 15)
            {
                return None;
            }
        } else if pressure.progress < 78 || pressure.progress < own_progress + 15 {
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
        // young empire of districts, buildings, and population growth. Scale
        // the cadence with game speed; the old fixed turn-175 cutoff expired
        // before the five-city target even became active on Standard speed.
        let city_cadence = g.standard_duration(90).max(1) as usize;
        let desired_cities = (3 + g.turn as usize / city_cadence)
            .min(map_capacity)
            .min(6);
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
        let emergency_objective = g.emergency_objective(pid).cloned();
        let strategy = if at_war && (threatened_city.is_some() || my_power * 1.25 < strongest_rival)
        {
            GrandStrategy::Recovery
        } else if emergency_objective.is_some() {
            GrandStrategy::Conquest
        } else if let Some(target) = self.victory_target {
            if target == VictoryTarget::Religion && g.players[pid].religion.is_none() {
                GrandStrategy::Religion
            } else if cities.len() < desired_cities && has_site && g.turn < g.standard_duration(175)
            {
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
        } else if self.religious_opening_viable(g, pid) {
            // A Prophet is a finite global race, not an economic goal that can
            // wait until the generic city target is complete. Religious
            // production only occupies one city, so the baseline governor can
            // continue settlers and development in the rest of the empire.
            // Keep that commitment independent of the generic progress race:
            // improving a contender's science readiness must not make it
            // abandon a nearly earned, globally limited Prophet.
            GrandStrategy::Religion
        } else if victory.progress >= 65 {
            victory.strategy
        } else if cities.len() < desired_cities && has_site && Self::expansion_window_open(g) {
            GrandStrategy::Expansion
        } else {
            victory.strategy
        };

        // Finish wars already in progress before selecting the next major
        // rival. In particular, this gives hostile city-states an explicit
        // city objective that the force-group planner can actually consume.
        let target_player = if let Some(emergency) = &emergency_objective {
            Some(emergency.target)
        } else if wartime_rivals.is_empty() {
            denial
                .filter(|(rival, _)| self.campaign_target_legal(g, pid, *rival))
                .map(|(rival, _)| rival)
                .or_else(|| {
                    let mut candidates: Vec<_> = major_rivals
                        .iter()
                        .copied()
                        .filter(|rival| self.campaign_target_legal(g, pid, *rival))
                        .collect();
                    if strategy == GrandStrategy::Conquest {
                        candidates.extend(
                            g.players
                                .iter()
                                .filter(|player| player.is_minor)
                                .filter(|player| self.campaign_target_legal(g, pid, player.id))
                                .map(|player| player.id),
                        );
                    }
                    candidates.into_iter().min_by(|a, b| {
                        self.campaign_target_value(g, pid, *a)
                            .partial_cmp(&self.campaign_target_value(g, pid, *b))
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
        let target_city = emergency_objective
            .map(|emergency| emergency.city)
            .or_else(|| {
                target_player.and_then(|target| {
                    g.cities
                        .values()
                        .filter(|c| c.owner == target)
                        .min_by(|left, right| {
                            self.campaign_city_value(g, pid, left, strategy)
                                .total_cmp(&self.campaign_city_value(g, pid, right, strategy))
                                .then_with(|| left.id.cmp(&right.id))
                        })
                        .map(|c| c.id)
                })
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

    fn expansion_window_open(g: &Game) -> bool {
        let payback_window = g.standard_duration(300);
        let endgame_reserve = g.standard_duration(50);
        let deadline = payback_window.min(g.max_turns.saturating_sub(endgame_reserve));
        g.turn < deadline
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

    /// Campaign value extends the major-rival heuristic to city-states.
    /// Conquering a city-state is strategically possible, but it burns every
    /// invested Envoy and permanently removes a potential Suzerain bonus, so
    /// a nearby minor should displace a major target only when it is a clearly
    /// cheaper objective. A city-state that can be secured immediately with
    /// free Envoys is treated as an ally to win, not territory to destroy.
    fn campaign_target_legal(&self, g: &Game, pid: usize, other: usize) -> bool {
        let Some(player) = g.players.get(other) else {
            return false;
        };
        if other == pid || !player.alive || player.is_barbarian {
            return false;
        }

        // Preserve an already active war even if a loaded legacy position has
        // contradictory diplomacy. Outside war, relationship commitments are
        // hard legality masks, never soft terms in the positional score.
        if g.is_at_war(pid, other) {
            return true;
        }
        if g.are_friends(pid, other) || g.alliance_with(pid, other).is_some() {
            return false;
        }
        if player.is_minor {
            let Some(suzerain) = g.suzerain_of(other) else {
                return true;
            };
            if suzerain == pid
                || g.are_friends(pid, suzerain)
                || g.alliance_with(pid, suzerain).is_some()
            {
                return false;
            }
        }
        true
    }

    fn campaign_target_value(&self, g: &Game, pid: usize, other: usize) -> f64 {
        let mut value = self.rival_value(g, pid, other);
        if !g.players[other].is_minor {
            // A leader marches on the civilizations their agenda disdains
            // before the ones it respects. Lower is a more attractive target,
            // so approval raises the bar and contempt lowers it. The weight
            // is deliberately smaller than distance, which still decides most
            // campaigns: an agenda colours the choice, it does not make it.
            return value + g.agenda_opinion(pid, other) * 2.0;
        }

        let mine = g.envoys_at(pid, other);
        value += 90.0 + mine as f64 * 45.0;
        let rival_envoys = g
            .players
            .iter()
            .filter(|player| !player.is_minor && !player.is_barbarian && player.id != pid)
            .map(|player| g.envoys_at(player.id, other))
            .max()
            .unwrap_or(0);
        let needed = (3_i64.max(rival_envoys + 1) - mine).max(1);
        if g.players[pid].envoys_free >= needed {
            value += 180.0;
        }
        if let Some(suzerain) = g.suzerain_of(other).filter(|suzerain| *suzerain != pid) {
            value += 40.0 + g.military_power(suzerain) * 0.25;
        }
        value += match Game::cs_type(&g.players[other].civ) {
            "militaristic" => 55.0,
            "industrial" => 35.0,
            "scientific" | "cultural" | "religious" => 25.0,
            _ => 15.0,
        };
        value
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

    /// Evaluate a repeatable district project as a bounded race move. The
    /// ongoing conversion is valued over the actual build horizon, while the
    /// completion award is priced against the live global Great Person race.
    /// This is deliberately analogous to an engine's passed-pawn extension:
    /// a project receives a large tempo bonus only when completing it crosses
    /// a concrete threshold, rather than from a fixed name-based preference.
    fn district_project_value(
        &self,
        g: &Game,
        pid: usize,
        cid: u32,
        project: &str,
        plan: &StrategicPlan,
    ) -> f64 {
        let spec = &g.rules.projects[project];
        let city = &g.cities[&cid];
        let production = g.city_yields(cid).production.max(1.0);
        let item = Item::Project {
            project: project.to_string(),
        };
        let turns = g.item_remaining_cost_for_city(pid, cid, &item) / production;
        let threatened = plan.threatened_city == Some(cid)
            || (city.last_attacked > 0 && g.turn.saturating_sub(city.last_attacked) <= 4);
        let denominator = 7.0 + turns.max(1.0);

        let mut ongoing = Yields::default();
        for (kind, percent) in &spec.ongoing_yields {
            let amount = production * percent / 100.0;
            match kind.as_str() {
                "food" => ongoing.food += amount,
                "production" => ongoing.production += amount,
                "gold" => ongoing.gold += amount,
                "science" => ongoing.science += amount,
                "culture" => ongoing.culture += amount,
                "faith" => ongoing.faith += amount,
                _ => {}
            }
        }
        let horizon = turns.clamp(1.0, 16.0);
        let mut value = self.yield_value(ongoing, plan.strategy) * horizon * 4.0;

        for (kind, award) in g.project_completion_gpp_awards(pid, cid, project) {
            // Patronage outcome B can set this class's completion award to
            // zero. Ongoing yield conversion may still justify the project,
            // but a disabled class has no race tempo to extend.
            if award <= f64::EPSILON {
                continue;
            }
            let mut affinity: f64 = match (plan.strategy, kind.as_str()) {
                (GrandStrategy::Science, "scientist") => 2.5,
                (GrandStrategy::Culture, "writer" | "artist" | "musician") => 2.6,
                (GrandStrategy::Religion, "prophet") if g.players[pid].religion.is_none() => 2.8,
                (GrandStrategy::Diplomacy, "merchant") => 2.0,
                (GrandStrategy::Conquest, "general" | "admiral") => 2.3,
                (GrandStrategy::Expansion | GrandStrategy::Recovery, "engineer" | "merchant") => {
                    1.8
                }
                (GrandStrategy::Science | GrandStrategy::Culture, "engineer") => 1.6,
                (_, "prophet") if g.players[pid].religion.is_some() => 0.15,
                _ => 0.85,
            };
            let work = match kind.as_str() {
                "writer" => Some("writing"),
                "artist" => Some("art"),
                "musician" => Some("music"),
                _ => None,
            };
            if work.is_some_and(|work| !g.can_house_additional_great_work(pid, work)) {
                affinity *= 0.20;
            }

            let cost = g.gp_cost(pid, &kind).max(1.0);
            let mine = g.players[pid].gpp.get(&kind).copied().unwrap_or(0.0);
            let rival = g
                .players
                .iter()
                .filter(|player| {
                    player.id != pid && player.alive && !player.is_minor && !player.is_barbarian
                })
                .map(|player| player.gpp.get(&kind).copied().unwrap_or(0.0))
                .fold(0.0_f64, f64::max);
            let useful = award.min((cost - mine).max(0.0));
            value += useful * (5.0 + 5.0 * affinity);
            value += (rival / cost).clamp(0.0, 1.0) * 150.0 * affinity;
            if mine + award + f64::EPSILON >= cost && mine < cost {
                value += 620.0 * affinity;
            }
            if rival > mine && mine + award > rival {
                value += 240.0 * affinity;
            }
        }

        if spec.full_power_while_active {
            let deficit = (g.city_power_demand(city) - g.city_power_supply(city)).max(0.0);
            value += deficit * 55.0 * denominator;
        }

        if project == "bread_and_circuses" {
            let loyalty_need = (100.0 - city.loyalty).max(0.0);
            let nearby_foreign_pressure = g
                .cities
                .values()
                .filter(|other| {
                    other.owner != pid
                        && !g.players[other.owner].is_barbarian
                        && g.alliance_with(pid, other.owner)
                            .is_none_or(|alliance| alliance.kind != "cultural")
                })
                .filter_map(|other| {
                    let distance = g.wdist(city.pos, other.pos);
                    (distance <= 9)
                        .then_some(other.pop.max(1) as f64 * (10 - distance) as f64 / 10.0)
                })
                .sum::<f64>();
            if loyalty_need < 5.0 && nearby_foreign_pressure < 2.0 {
                value -= 260.0;
            } else {
                value += loyalty_need * 8.0
                    + nearby_foreign_pressure * horizon * 7.0
                    + spec
                        .effects
                        .get("completion_loyalty")
                        .copied()
                        .unwrap_or(0.0)
                        * 7.0;
            }
        }

        // A project may exploit completed infrastructure, but should not
        // indefinitely postpone the first building in the district that
        // enables it. This is the economic equivalent of a quiet-move pruning
        // guard: search the forcing race only after basic development exists.
        if let Some(district) = spec.district.as_deref() {
            let family = g.district_family(district);
            let has_family_building = city.buildings.iter().any(|building| {
                g.rules.buildings[building]
                    .district
                    .as_deref()
                    .is_some_and(|built| g.district_family(built) == family)
            });
            let family_has_building = g.rules.buildings.values().any(|building| {
                building.buildable
                    && building
                        .district
                        .as_deref()
                        .is_some_and(|built| g.district_family(built) == family)
            });
            if family_has_building && !has_family_building {
                value -= 420.0;
            }
        }
        if threatened {
            value -= 360.0;
        }
        value
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
        // Explicit evaluator targets and the adaptive live plan must drive the
        // same prerequisite search.  Previously only `victory_target` enabled
        // milestone routing, so a normal spectator AI could correctly assess
        // Science or Culture yet wander through generic unlocks indefinitely.
        let objective = self
            .victory_target
            .map(VictoryTarget::strategy)
            .unwrap_or(plan.strategy);
        if g.players[pid].research.is_none() {
            let available = g.available_techs(pid);
            let science_commitment = objective == GrandStrategy::Science
                || self.diplomatic_science_backup(g, pid, plan);
            let forced_goal = match objective {
                _ if science_commitment => [
                    "rocketry",
                    "satellites",
                    "nanotechnology",
                    "smart_materials",
                    "offworld_mission",
                ]
                .into_iter()
                .find(|tech| !g.players[pid].techs.contains(*tech)),
                GrandStrategy::Culture => ["printing", "radio", "computers"]
                    .into_iter()
                    .find(|tech| !g.players[pid].techs.contains(*tech)),
                GrandStrategy::Diplomacy if !g.players[pid].techs.contains("seasteads") => {
                    Some("seasteads")
                }
                GrandStrategy::Religion if !g.players[pid].techs.contains("astrology") => {
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
            let forced_goal = match objective {
                GrandStrategy::Culture => [
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
                GrandStrategy::Science if !g.players[pid].civics.contains("space_race") => {
                    Some("space_race")
                }
                GrandStrategy::Diplomacy
                    if !g.players[pid].civics.contains("global_warming_mitigation") =>
                {
                    Some("global_warming_mitigation")
                }
                GrandStrategy::Religion if !g.players[pid].civics.contains("theology") => {
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

    fn strategic_government(&self, g: &mut Game, pid: usize, strategy: GrandStrategy) {
        let objective = self
            .victory_target
            .map(VictoryTarget::strategy)
            .unwrap_or(strategy);
        let unlocked = |government: &str| {
            g.rules.governments.get(government).is_some_and(|spec| {
                spec.civic
                    .as_ref()
                    .is_none_or(|civic| g.players[pid].civics.contains(civic))
            })
        };

        // Matching the leading Culture defender removes the full -40%
        // Gathering Storm penalty between distinct Tier 3/4 governments.
        // Lower-tier governments have zero intolerance and do not justify
        // giving up the stronger late-game government effects.
        let culture_match = (objective == GrandStrategy::Culture)
            .then(|| {
                g.players
                    .iter()
                    .filter(|rival| {
                        rival.id != pid && rival.alive && !rival.is_minor && !rival.is_barbarian
                    })
                    .max_by_key(|rival| (g.domestic_tourists(rival.id), rival.id))
                    .and_then(|rival| rival.government.clone())
            })
            .flatten()
            .filter(|government| {
                matches!(
                    government.as_str(),
                    "communism"
                        | "democracy"
                        | "fascism"
                        | "corporate_libertarianism"
                        | "digital_democracy"
                        | "synthetic_technocracy"
                ) && unlocked(government)
            });

        let faith_mobilization =
            matches!(strategy, GrandStrategy::Conquest | GrandStrategy::Recovery)
                && g.players[pid].faith >= 600.0
                && unlocked("theocracy");
        let priorities: &[&str] = match objective {
            GrandStrategy::Culture | GrandStrategy::Diplomacy => &[
                "digital_democracy",
                "democracy",
                "merchant_republic",
                "monarchy",
                "theocracy",
                "classical_republic",
                "chiefdom",
            ],
            GrandStrategy::Science => &[
                "synthetic_technocracy",
                "communism",
                "democracy",
                "merchant_republic",
                "monarchy",
                "theocracy",
                "classical_republic",
                "chiefdom",
            ],
            GrandStrategy::Conquest if faith_mobilization => &[
                "theocracy",
                "corporate_libertarianism",
                "fascism",
                "communism",
                "monarchy",
                "merchant_republic",
                "oligarchy",
                "chiefdom",
            ],
            GrandStrategy::Conquest => &[
                "corporate_libertarianism",
                "fascism",
                "communism",
                "monarchy",
                "merchant_republic",
                "theocracy",
                "oligarchy",
                "chiefdom",
            ],
            GrandStrategy::Religion => &[
                "theocracy",
                "monarchy",
                "merchant_republic",
                "classical_republic",
                "chiefdom",
            ],
            GrandStrategy::Expansion => &[
                "corporate_libertarianism",
                "communism",
                "merchant_republic",
                "monarchy",
                "theocracy",
                "classical_republic",
                "chiefdom",
            ],
            GrandStrategy::Recovery if faith_mobilization => &[
                "theocracy",
                "digital_democracy",
                "democracy",
                "communism",
                "merchant_republic",
                "monarchy",
                "classical_republic",
                "chiefdom",
            ],
            GrandStrategy::Recovery => &[
                "digital_democracy",
                "democracy",
                "communism",
                "merchant_republic",
                "monarchy",
                "theocracy",
                "classical_republic",
                "chiefdom",
            ],
        };
        let choice = culture_match.or_else(|| {
            priorities
                .iter()
                .copied()
                .find(|government| unlocked(government))
                .map(str::to_string)
        });
        if let Some(government) = choice
            .filter(|government| g.players[pid].government.as_deref() != Some(government.as_str()))
        {
            // Returning to any previously used government costs two complete
            // turns of Anarchy. An adaptive plan can legitimately change its
            // mind as a victory race moves, but a lateral return between (for
            // example) Democracy and Fascism must not zero every empire yield
            // on alternating turns. Only pay that recurring cost for a real
            // policy-capacity upgrade; first-time governments remain free.
            let policy_capacity = |name: &str| {
                g.rules.governments.get(name).map_or(0, |spec| {
                    spec.slots.military
                        + spec.slots.economic
                        + spec.slots.diplomatic
                        + spec.slots.wildcard
                })
            };
            let returning = g.players[pid].past_governments.contains(&government);
            let current_capacity = g.players[pid]
                .government
                .as_deref()
                .map_or(0, policy_capacity);
            let choice_capacity = policy_capacity(&government);
            // A newly tried government is free, but dropping from a mature
            // eight-slot government to a six-slot faith or military stopgap
            // invites an expensive return as soon as the adaptive plan moves
            // again. Never give up policy capacity; among equal-capacity
            // governments a first adoption remains free, while a repeat is
            // still blocked by the Anarchy guard below.
            if choice_capacity < current_capacity
                || (returning && choice_capacity == current_capacity)
            {
                return;
            }
            let _ = g.apply(pid, &Action::Government { government });
        }
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
            && (plan.strategy == GrandStrategy::Conquest
                || g.is_at_war(pid, partner)
                || self.rival_victory_pressure(g, partner).progress >= 78);

        let mut value = deal.give_gold - deal.request_gold;
        if deal.peace {
            value += if my_power < partner_power * 0.85 || fatigued {
                320.0
            } else if denied_partner {
                // Recovery is a temporary battlefield posture, not an order
                // to abandon the campaign. A locally threatened city can put
                // an overwhelmingly stronger attacker into Recovery for one
                // assessment window; keep refusing its active target's white
                // peace until the army is actually outmatched or fatigued.
                -260.0
            } else if plan.strategy == GrandStrategy::Recovery {
                320.0
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

    fn strategic_bilateral_trade(
        &self,
        g: &mut Game,
        pid: usize,
        excluded_partner: Option<usize>,
        strategy: GrandStrategy,
    ) {
        let objective = self
            .victory_target
            .map(VictoryTarget::strategy)
            .unwrap_or(strategy);
        if objective == GrandStrategy::Culture && g.turn % 6 == pid as u32 % 6 {
            let best = g
                .quick_deals(pid)
                .into_iter()
                .filter(|deal| Some(deal.partner) != excluded_partner)
                .filter(|deal| {
                    deal.item == "open_borders"
                        && deal.direction == "buy"
                        && deal.my_value >= 2.0
                        && deal.partner_value >= 2.0
                })
                .max_by(|left, right| {
                    g.domestic_tourists(left.partner)
                        .cmp(&g.domestic_tourists(right.partner))
                        .then_with(|| {
                            left.my_value
                                .min(left.partner_value)
                                .partial_cmp(&right.my_value.min(right.partner_value))
                                .unwrap()
                        })
                        .then_with(|| right.partner.cmp(&left.partner))
                });
            if let Some(deal) = best {
                if g.apply(
                    pid,
                    &Action::Trade {
                        player: deal.partner,
                        offer: Box::new(deal.offer),
                        request: Box::new(deal.request),
                    },
                )
                .is_ok()
                {
                    return;
                }
            }

            let best = g
                .quick_deals(pid)
                .into_iter()
                .filter(|deal| Some(deal.partner) != excluded_partner)
                .filter(|deal| {
                    deal.category == "great_work"
                        && deal.direction == "buy"
                        && deal.my_value >= 2.0
                        && deal.partner_value >= 2.0
                })
                .max_by(|left, right| {
                    left.my_value
                        .min(left.partner_value)
                        .partial_cmp(&right.my_value.min(right.partner_value))
                        .unwrap()
                        .then_with(|| right.partner.cmp(&left.partner))
                        .then_with(|| right.item.cmp(&left.item))
                });
            if let Some(deal) = best {
                if g.apply(
                    pid,
                    &Action::Trade {
                        player: deal.partner,
                        offer: Box::new(deal.offer),
                        request: Box::new(deal.request),
                    },
                )
                .is_ok()
                {
                    return;
                }
            }

            // A Culture objective preserves its own Great Works. If neither
            // the strategically useful Open Borders direction nor a housed
            // purchase is available, it may still take the best ordinary
            // mutually beneficial quote.
            let best = g
                .quick_deals(pid)
                .into_iter()
                .filter(|deal| Some(deal.partner) != excluded_partner)
                .filter(|deal| {
                    !(deal.category == "great_work" && deal.direction == "sell")
                        && deal.my_value >= 2.0
                        && deal.partner_value >= 2.0
                })
                .max_by(|left, right| {
                    left.my_value
                        .min(left.partner_value)
                        .partial_cmp(&right.my_value.min(right.partner_value))
                        .unwrap()
                });
            if let Some(deal) = best {
                let _ = g.apply(
                    pid,
                    &Action::Trade {
                        player: deal.partner,
                        offer: Box::new(deal.offer),
                        request: Box::new(deal.request),
                    },
                );
            }
            return;
        }
        self.base
            .bilateral_trade_excluding(g, pid, excluded_partner);
    }

    fn propose_strategic_alliance(
        &self,
        g: &mut Game,
        pid: usize,
        plan: &StrategicPlan,
        denied_partner: Option<usize>,
    ) {
        if g.turn % 12 != pid as u32 % 12 || !g.players[pid].civics.contains("civil_service") {
            return;
        }
        let kind = match plan.strategy {
            GrandStrategy::Science => "research",
            GrandStrategy::Culture => "cultural",
            GrandStrategy::Religion => "religious",
            GrandStrategy::Conquest | GrandStrategy::Recovery => "military",
            GrandStrategy::Expansion | GrandStrategy::Diplomacy => "economic",
        };
        if kind == "research" && g.tree_effect(pid, "research_agreements") <= 0.0 {
            return;
        }
        if g.players[pid]
            .alliances
            .values()
            .any(|alliance| alliance.ends > g.turn && alliance.kind == kind)
        {
            return;
        }
        let pending_with = |partner: usize| {
            g.pending_deals.iter().any(|deal| {
                deal.expires >= g.turn
                    && ((deal.from == pid && deal.to == partner)
                        || (deal.from == partner && deal.to == pid))
            })
        };
        let partner = g
            .players
            .iter()
            .filter(|other| {
                other.id != pid
                    && other.alive
                    && !other.is_minor
                    && !other.is_barbarian
                    && Some(other.id) != denied_partner
                    && !g.is_at_war(pid, other.id)
                    && other.civics.contains("civil_service")
                    && g.alliance_with(pid, other.id).is_none()
                    && !pending_with(other.id)
                    && (kind != "research" || g.tree_effect(other.id, "research_agreements") > 0.0)
                    && !other
                        .alliances
                        .values()
                        .any(|alliance| alliance.ends > g.turn && alliance.kind == kind)
                    && g.players[pid]
                        .grievances
                        .get(&other.id)
                        .copied()
                        .unwrap_or(0.0)
                        < 75.0
                    && self.rival_victory_pressure(g, other.id).progress < 82
            })
            .max_by(|left, right| {
                let score = |other: usize| {
                    let friendship = if g.are_friends(pid, other) {
                        180.0
                    } else {
                        0.0
                    };
                    let connected = if g.routes.iter().any(|route| {
                        route.ends > g.turn
                            && ((route.owner == pid
                                && g.cities
                                    .get(&route.dest)
                                    .is_some_and(|destination| destination.owner == other))
                                || (route.owner == other
                                    && g.cities
                                        .get(&route.dest)
                                        .is_some_and(|destination| destination.owner == pid)))
                    }) {
                        70.0
                    } else {
                        0.0
                    };
                    let complement = match kind {
                        "research" => {
                            g.players[other]
                                .techs
                                .difference(&g.players[pid].techs)
                                .count() as f64
                                * 4.0
                        }
                        "cultural" => g.tourism_per_turn(other).min(300.0) * 0.15,
                        "economic" => {
                            g.players
                                .iter()
                                .filter(|minor| minor.alive && minor.is_minor)
                                .filter(|minor| g.suzerain_of(minor.id) == Some(other))
                                .count() as f64
                                * 35.0
                        }
                        "military" => g.military_power(other).min(250.0) * 0.25,
                        "religious" => g.players[other].religion.is_some() as usize as f64 * 45.0,
                        _ => 0.0,
                    };
                    friendship + connected + complement
                        - g.players[pid]
                            .grievances
                            .get(&other)
                            .copied()
                            .unwrap_or(0.0)
                };
                score(left.id)
                    .partial_cmp(&score(right.id))
                    .unwrap()
                    .then_with(|| right.id.cmp(&left.id))
            })
            .map(|other| other.id);
        if let Some(partner) = partner {
            let _ = g.apply(
                pid,
                &Action::ProposeDeal {
                    player: partner,
                    give_gold: 0.0,
                    request_gold: 0.0,
                    open_borders: g.players[pid].civics.contains("early_empire"),
                    friendship: true,
                    peace: false,
                    alliance: Some(kind.to_string()),
                },
            );
        }
    }

    fn congress_choice(
        &self,
        g: &Game,
        pid: usize,
        resolution: &CongressResolution,
        strategy: GrandStrategy,
    ) -> Option<String> {
        if let Some(proposal) = g.emergency_proposal_for_resolution(&resolution.id) {
            if proposal.target == pid {
                return Some("B:oppose".to_string());
            }
            if !proposal.eligible.contains(&pid) {
                return None;
            }
            let grievance = g.players[pid]
                .grievances
                .get(&proposal.target)
                .copied()
                .unwrap_or(0.0);
            let threat = self.rival_victory_pressure(g, proposal.target).progress;
            let affordable_war =
                g.military_power(pid) * 1.75 + 20.0 >= g.military_power(proposal.target);
            let support = proposal.kind == "city_state"
                || strategy == GrandStrategy::Diplomacy
                || grievance >= 25.0
                || threat >= 55
                || affordable_war;
            return Some(if support { "A:support" } else { "B:oppose" }.to_string());
        }
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
                    "arms_control" => match target_player {
                        Some(target) => {
                            let inventory = |player: usize| {
                                [
                                    "project_effect:nuclear_devices",
                                    "project_effect:thermonuclear_devices",
                                ]
                                .into_iter()
                                .map(|key| {
                                    g.players[player]
                                        .counters
                                        .get(key)
                                        .copied()
                                        .unwrap_or(0)
                                        .max(0)
                                })
                                .sum::<i64>() as f64
                            };
                            let mine = inventory(pid);
                            let theirs = inventory(target);
                            let major_inventories: Vec<f64> = g
                                .players
                                .iter()
                                .filter(|player| {
                                    player.alive && !player.is_minor && !player.is_barbarian
                                })
                                .map(|player| inventory(player.id))
                                .collect();
                            let world_total = major_inventories.iter().sum::<f64>();
                            let equalized_total =
                                theirs * major_inventories.len().max(1) as f64;
                            let disarmament = world_total - equalized_total;
                            match outcome {
                                // Outcome A copies the target's stockpile to every
                                // major. Peaceful strategies therefore nominate the
                                // smallest arsenal, not the nuclear leader.
                                "A" if strategy != GrandStrategy::Conquest => {
                                    180.0 + 55.0 * disarmament
                                        - 75.0 * (-disarmament).max(0.0)
                                }
                                "A" => 20.0 + 35.0 * (theirs - mine)
                                    - 45.0 * (equalized_total - world_total).max(0.0),
                                "B" if target != pid => {
                                    let aggression = if matches!(
                                        strategy,
                                        GrandStrategy::Conquest | GrandStrategy::Recovery
                                    ) {
                                        100.0
                                    } else {
                                        55.0
                                    };
                                    90.0 + aggression * theirs
                                }
                                "B" => -500.0,
                                _ => 0.0,
                            }
                        }
                        None => 0.0,
                    },
                    "world_ideology" => {
                        let mine = g.players[pid].government.as_deref() == Some(target);
                        let rival_users = g
                            .players
                            .iter()
                            .filter(|player| {
                                player.id != pid
                                    && player.alive
                                    && !player.is_minor
                                    && !player.is_barbarian
                                    && player.government.as_deref() == Some(target)
                            })
                            .count() as f64;
                        match outcome {
                            "A" if mine => 320.0,
                            "A" => 30.0,
                            "B" if mine => -260.0,
                            "B" => 90.0 + 70.0 * rival_users,
                            _ => 0.0,
                        }
                    }
                    "border_control_treaty" => match (outcome, target_player) {
                        ("A", Some(target)) if target == pid => 300.0,
                        ("B", Some(target)) if target == pid => -240.0,
                        ("B", Some(target)) => {
                            let territory = g
                                .player_city_ids(target)
                                .into_iter()
                                .map(|city| g.cities[&city].owned_tiles.len())
                                .sum::<usize>() as f64;
                            80.0 + territory
                        }
                        _ => 20.0,
                    },
                    "public_works_program" => {
                        let queued = |owner: usize| {
                            g.cities
                                .values()
                                .filter(|city| city.owner == owner)
                                .filter(|city| {
                                    city.queue.iter().any(|item| {
                                        matches!(item, Item::Project { project } if project == target)
                                    })
                                })
                                .count() as f64
                        };
                        let own_queued = queued(pid);
                        let rival_queued = g
                            .players
                            .iter()
                            .filter(|player| {
                                player.id != pid
                                    && player.alive
                                    && !player.is_minor
                                    && !player.is_barbarian
                            })
                            .map(|player| queued(player.id))
                            .sum::<f64>();
                        let aligned = match strategy {
                            GrandStrategy::Science => {
                                target.contains("launch_")
                                    || target.contains("laser_station")
                                    || target == "exoplanet_expedition"
                            }
                            GrandStrategy::Conquest | GrandStrategy::Recovery => {
                                target.contains("nuclear")
                                    || matches!(target, "manhattan_project" | "operation_ivy")
                            }
                            GrandStrategy::Diplomacy => target == "carbon_recapture",
                            _ => false,
                        };
                        match (outcome, aligned) {
                            ("A", true) => 300.0 + 160.0 * own_queued,
                            ("A", false) => 65.0 + 180.0 * own_queued,
                            ("B", true) => {
                                -220.0 + 140.0 * rival_queued - 180.0 * own_queued
                            }
                            ("B", false) => {
                                25.0 + 140.0 * rival_queued - 180.0 * own_queued
                            }
                            _ => 0.0,
                        }
                    }
                    "global_energy_treaty" => {
                        let queued = |owner: usize| {
                            g.cities
                                .values()
                                .filter(|city| city.owner == owner)
                                .filter(|city| {
                                    city.queue.iter().any(|item| {
                                        matches!(item, Item::Building { building } if building == target)
                                    })
                                })
                                .count() as f64
                        };
                        let own_queued = queued(pid);
                        let rival_queued = g
                            .players
                            .iter()
                            .filter(|player| {
                                player.id != pid
                                    && player.alive
                                    && !player.is_minor
                                    && !player.is_barbarian
                            })
                            .map(|player| queued(player.id))
                            .sum::<f64>();
                        let preferred = match strategy {
                            GrandStrategy::Science | GrandStrategy::Diplomacy => {
                                "nuclear_power_plant"
                            }
                            GrandStrategy::Conquest | GrandStrategy::Recovery => "coal_power_plant",
                            _ => "oil_power_plant",
                        };
                        match (outcome, target) {
                            ("A", candidate) if candidate == preferred => {
                                270.0 + 160.0 * own_queued
                            }
                            ("A", _) => 90.0 + 160.0 * own_queued,
                            ("B", "coal_power_plant") if strategy == GrandStrategy::Diplomacy => {
                                180.0 + 120.0 * rival_queued - 180.0 * own_queued
                            }
                            ("B", candidate) if candidate == preferred => {
                                -180.0 + 120.0 * rival_queued - 180.0 * own_queued
                            }
                            ("B", _) => 35.0 + 120.0 * rival_queued - 180.0 * own_queued,
                            _ => 0.0,
                        }
                    }
                    "deforestation_treaty" => {
                        let owned_copies = |owner: usize| {
                            g.cities
                                .values()
                                .filter(|city| city.owner == owner)
                                .flat_map(|city| city.owned_tiles.iter())
                                .filter(|position| {
                                    g.map.tiles[*position].feature.as_deref() == Some(target)
                                })
                                .count() as f64
                        };
                        let own_copies = owned_copies(pid);
                        let rival_copies = g
                            .players
                            .iter()
                            .filter(|player| {
                                player.id != pid
                                    && player.alive
                                    && !player.is_minor
                                    && !player.is_barbarian
                            })
                            .map(|player| owned_copies(player.id))
                            .sum::<f64>();
                        match outcome {
                            "A" => {
                                65.0
                                    + 35.0 * own_copies
                                    + if strategy == GrandStrategy::Expansion {
                                        90.0
                                    } else {
                                        0.0
                                    }
                            }
                            "B" if strategy == GrandStrategy::Culture && target == "forest" => {
                                165.0 + 5.0 * (own_copies + rival_copies)
                            }
                            "B" => 20.0 + 5.0 * rival_copies - 20.0 * own_copies,
                            _ => 0.0,
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

    /// Prefer an available low-Grievance casus belli. If none is ready, a
    /// major rival is denounced and the campaign waits for Formal War rather
    /// than opening with a Surprise War. The sole exception is a rival already
    /// on the brink of victory, where five setup turns can lose the game.
    /// City-states cannot be denounced and therefore remain direct targets.
    fn preferred_war_opening(&self, g: &Game, pid: usize, target: usize) -> Option<Action> {
        let legal = g.legal_actions(pid);
        let casus_belli = legal
            .iter()
            .filter_map(|action| match action {
                Action::DeclareWarWithCasusBelli {
                    player,
                    casus_belli,
                } if *player == target => {
                    let grievance_cost = if casus_belli == "formal_war" { 100 } else { 50 };
                    Some((grievance_cost, casus_belli, action))
                }
                _ => None,
            })
            .min_by_key(|(cost, name, _)| (*cost, *name))
            .map(|(_, _, action)| action.clone());
        if casus_belli.is_some() {
            return casus_belli;
        }

        let surprise = legal.iter().find_map(|action| match action {
            Action::DeclareWar { player } if *player == target => Some(action.clone()),
            _ => None,
        });
        if g.players[target].is_minor {
            return surprise;
        }

        let urgent = self.rival_victory_pressure(g, target).progress >= 90;
        let denounced = g.players[pid]
            .denounced_until
            .get(&target)
            .is_some_and(|until| *until > g.turn);
        if !urgent && !denounced {
            return legal.iter().find_map(|action| match action {
                Action::Denounce { player } if *player == target => Some(action.clone()),
                _ => None,
            });
        }
        if urgent {
            surprise
        } else {
            // The denouncement is active but its five-turn preparation period
            // has not elapsed, so preserve the army and wait for Formal War.
            None
        }
    }

    /// A peacetime tile from which a ground force can begin the selected
    /// campaign without trespassing through the target's borders. Keeping the
    /// ring several tiles outside the city leaves room for different combat
    /// roles to assemble while keeping the army close enough to exploit the
    /// opening turns of the war.
    fn campaign_staging_position(
        &self,
        g: &Game,
        pid: usize,
        target: usize,
        uid: u32,
        objective: Pos,
        position: Pos,
    ) -> bool {
        let Some(tile) = g.map.get(position) else {
            return false;
        };
        let distance = g.wdist(position, objective);
        if !(3..=5).contains(&distance)
            || g.rules.is_water(tile)
            || g.city_at(position).is_some()
            || !g.unit_can_traverse(uid, position)
        {
            return false;
        }
        let territory = tile
            .owner_city
            .and_then(|city| g.cities.get(&city))
            .map(|city| city.owner);
        territory != Some(target)
            && territory.is_none_or(|owner| {
                owner == pid || g.has_open_borders(pid, owner)
            })
    }

    fn staged_campaign_units(
        &self,
        g: &Game,
        pid: usize,
        target: usize,
        objective: Pos,
    ) -> Vec<u32> {
        g.player_unit_ids(pid)
            .into_iter()
            .filter(|uid| {
                let unit = &g.units[uid];
                let spec = &g.rules.units[unit.kind.as_str()];
                spec.class == "military"
                    && !matches!(spec.domain.as_deref(), Some("sea" | "air"))
                    && (spec.is_melee_capable() || spec.has_ranged_attack())
                    && unit.hp as f64 > self.base.w.withdraw_hp
                    && self.campaign_staging_position(
                        g,
                        pid,
                        target,
                        *uid,
                        objective,
                        unit.pos,
                    )
            })
            .collect()
    }

    /// Global power answers whether a war is affordable; this answers whether
    /// the army is actually in position to prosecute it. At least one melee
    /// unit is mandatory because ranged and siege units cannot capture a city.
    fn campaign_staged_for_war(
        &self,
        g: &Game,
        pid: usize,
        target: usize,
        objective: Pos,
        committed_domination: bool,
    ) -> bool {
        let units = self.staged_campaign_units(g, pid, target, objective);
        let has_capturer = units.iter().any(|uid| {
            g.rules.units[g.units[uid].kind.as_str()].is_melee_capable()
        });
        let ratio = self.local_strength_ratio(g, &units, &[target], objective);
        let formation_ready = units.len() >= 3 || (units.len() >= 2 && ratio >= 1.60);
        let minimum_ratio = if committed_domination { 0.90 } else { 1.05 };
        formation_ready && has_capturer && ratio + 1e-9 >= minimum_ratio
    }

    /// Redirect an otherwise idle field unit to the active conquest front.
    /// Returning `Some` means the campaign owns this unit's peacetime order,
    /// including holding a completed staging position; `None` leaves ordinary
    /// patrol, exploration, and naval-escort behavior unchanged.
    fn campaign_staging_step(
        &mut self,
        g: &mut Game,
        pid: usize,
        uid: u32,
        plan: &StrategicPlan,
    ) -> Option<bool> {
        if plan.strategy != GrandStrategy::Conquest {
            return None;
        }
        let target = plan.target_player?;
        let objective = plan
            .target_city
            .and_then(|city| g.cities.get(&city))
            .filter(|city| city.owner == target)
            .map(|city| city.pos)?;
        if !self.campaign_target_legal(g, pid, target) || g.is_at_war(pid, target) {
            return None;
        }
        let unit = &g.units[&uid];
        let spec = &g.rules.units[unit.kind.as_str()];
        if !matches!(spec.class.as_str(), "military" | "support")
            || matches!(spec.domain.as_deref(), Some("sea" | "air"))
            || (BasicAi::unit_doctrine(g, uid) == UnitDoctrine::Recon
                && self.base.has_exploration_target(g, pid, uid))
        {
            return None;
        }
        if self.campaign_staging_position(g, pid, target, uid, objective, unit.pos) {
            return Some(self.base.fortify_or_stop(g, pid, uid));
        }

        let current = unit.pos;
        let goals: HashSet<Pos> = g
            .wdisk(objective, 5)
            .into_iter()
            .filter(|position| {
                self.campaign_staging_position(
                    g,
                    pid,
                    target,
                    uid,
                    objective,
                    *position,
                ) && g.units_at(*position).is_empty()
            })
            .collect();
        let Some(next) = g
            .route_step_to_any(uid, &goals)
            .filter(|position| g.can_move(uid, *position))
        else {
            return None;
        };
        // Do not use an Open Borders shortcut through the intended victim.
        // The next turn's route search will find a lawful way around it.
        let next_territory = g.map.tiles[&next]
            .owner_city
            .and_then(|city| g.cities.get(&city))
            .map(|city| city.owner);
        if next_territory == Some(target) {
            return Some(self.base.fortify_or_stop(g, pid, uid));
        }
        debug_assert_ne!(next, current);
        Some(g.apply(pid, &Action::Move { unit: uid, to: next }).is_ok())
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
            let Some((accept, peace)) = g
                .pending_deals
                .iter()
                .find(|deal| deal.id == deal_id)
                .map(|deal| {
                    (
                        self.incoming_deal_value(g, pid, deal, plan) >= 0.0,
                        deal.peace,
                    )
                })
            else {
                continue;
            };
            let action = if accept {
                Action::AcceptDeal { deal: deal_id }
            } else {
                Action::RejectDeal { deal: deal_id }
            };
            if g.apply(pid, &action).is_ok() && accept && peace {
                // An accepted peace offer is the negotiated equivalent of
                // MakePeace: remember the stand-down so this AI does not
                // redeclare as soon as the mandatory treaty expires.
                self.peace_until = g.turn.saturating_add(30);
                self.major_war_since = None;
            }
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
                    && g.emergency_proposal_for_resolution(&resolution.id)
                        .is_none()
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
            plan.strategy == GrandStrategy::Conquest
                || g.is_at_war(pid, *target)
                || self.rival_victory_pressure(g, *target).progress >= 78
        });
        self.strategic_bilateral_trade(g, pid, denied_partner, plan.strategy);
        self.propose_strategic_alliance(g, pid, plan, denied_partner);
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
            let peace_pending = g.pending_deals.iter().any(|deal| {
                deal.peace
                    && ((deal.from == pid && deal.to == *other)
                        || (deal.from == *other && deal.to == pid))
                    && deal.expires >= g.turn
            });
            if g.is_at_war(pid, *other)
                && !g.emergency_war_pair(pid, *other)
                && !g.players[*other].is_minor
                && !peace_pending
                && (my_power < g.military_power(*other) * 0.62
                    || (plan.strategy == GrandStrategy::Recovery
                        && plan.target_player != Some(*other))
                    || (fatigued && g.player_city_ids(*other).len() > 1))
            {
                // Peace between majors is bilateral. The former direct
                // MakePeace let an outmatched defender terminate a winning
                // invasion on the first legal turn, even when the conqueror
                // valued the campaign. Keep fighting until the recipient's
                // normal deal valuation accepts this offer.
                let _ = g.apply(
                    pid,
                    &Action::ProposeDeal {
                        player: *other,
                        give_gold: 0.0,
                        request_gold: 0.0,
                        open_borders: false,
                        friendship: false,
                        peace: true,
                        alliance: None,
                    },
                );
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
        let emergency_target = g
            .emergency_objective(pid)
            .is_some_and(|objective| objective.target == target);
        if plan.strategy != GrandStrategy::Conquest
            || major_wars > 0
            || g.turn < 35
            || g.turn < self.peace_until
            || g.player_city_ids(pid).len() < 2
            || g.is_at_war(pid, target)
            || (!emergency_target && !self.campaign_target_legal(g, pid, target))
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
        let staged = plan
            .target_city
            .and_then(|city| g.cities.get(&city))
            .is_some_and(|city| {
                self.campaign_staged_for_war(
                    g,
                    pid,
                    target,
                    city.pos,
                    committed_domination,
                )
            });
        if close_enough && ready && staged {
            if let Some(action) = self.preferred_war_opening(g, pid, target) {
                let _ = g.apply(pid, &action);
            }
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
                    let unique_alignment = match (strategy, minor.civ.as_str()) {
                        (GrandStrategy::Science, "Geneva") => 14,
                        (GrandStrategy::Science | GrandStrategy::Conquest, "Hattusa") => 11,
                        (GrandStrategy::Science | GrandStrategy::Culture, "Stockholm") => 10,
                        (GrandStrategy::Conquest, "Kabul") => 14,
                        (GrandStrategy::Conquest | GrandStrategy::Expansion, "Carthage") => 10,
                        (GrandStrategy::Expansion | GrandStrategy::Recovery, "Mohenjo-Daro") => 11,
                        (GrandStrategy::Religion, "Yerevan") => 15,
                        (GrandStrategy::Religion | GrandStrategy::Culture, "Kandy") => 12,
                        (GrandStrategy::Expansion | GrandStrategy::Recovery, "Zanzibar") => 11,
                        (_, "Zanzibar") if g.players[pid].civ == "Aztec" => 12,
                        (
                            GrandStrategy::Science
                            | GrandStrategy::Culture
                            | GrandStrategy::Conquest
                            | GrandStrategy::Expansion,
                            "Auckland",
                        ) => 9,
                        (
                            GrandStrategy::Religion
                            | GrandStrategy::Conquest
                            | GrandStrategy::Recovery,
                            "Valletta",
                        ) => 13,
                        (GrandStrategy::Culture, "Vilnius") => 14,
                        (_, "Stockholm" | "Zanzibar" | "Auckland" | "Valletta") => 5,
                        _ => 2,
                    };
                    let already_secure = g.suzerain_of(minor.id) == Some(pid) && mine > rival + 1;
                    let shared_from_partner = g.suzerain_of(minor.id).is_some_and(|leader| {
                        leader != pid
                            && g.alliance_with(pid, leader).is_some_and(|alliance| {
                                alliance.kind == "economic" && alliance.level >= 3
                            })
                    });
                    let type_bonus_value = g
                        .next_envoy_type_bonus(pid, minor.id)
                        .map(|(envoys, yields)| {
                            (self.yield_value(yields, strategy) * 14.0 / envoys as f64).round()
                                as i64
                        })
                        .unwrap_or(0);
                    let denial = denied_rival
                        .is_some_and(|leader| g.suzerain_of(minor.id) == Some(leader))
                        as i64
                        * 140;
                    let score = (alignment + unique_alignment) * 10 + type_bonus_value + denial
                        - needed * 7
                        - already_secure as i64 * 80
                        - shared_from_partner as i64 * 300;
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
            if !g.can_activate_current_great_person(pid, kind) {
                continue;
            }
            let points = g.players[pid].gpp.get(kind).copied().unwrap_or(0.0);
            let cost = g.gp_cost(pid, kind);
            let missing = (cost - points).max(0.0);
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
            let close_fraction = missing / cost.max(1.0);
            let limit = if affinity >= 500.0 { 0.40 } else { 0.15 };
            if affinity < 0.0 || close_fraction > limit {
                continue;
            }
            let effect_value = person.effects.values().sum::<f64>() * 12.0;
            for (currency, bank, reserve) in [
                ("gold", g.players[pid].gold, gold_reserve),
                ("faith", g.players[pid].faith, faith_reserve),
            ] {
                let Some(price) = g.great_person_patronage_price(pid, kind, currency) else {
                    continue;
                };
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

    /// Convert a deep treasury into one immediate tempo gain. Candidate
    /// units, buildings, and Governor-enabled districts reuse the strategic
    /// production evaluator, but are scored at their undiscounted positional
    /// value because a purchase completes now. A strategy-sensitive reserve
    /// protects Great Person patronage, Great Work deals, upgrades, and
    /// emergency reinforcement instead of treating all affordable actions as
    /// equally spendable.
    fn advanced_gold_spending(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) -> bool {
        let city_count = g.player_city_ids(pid).len() as f64;
        let reserve = match plan.strategy {
            GrandStrategy::Diplomacy | GrandStrategy::Culture => 300.0 + 75.0 * city_count,
            GrandStrategy::Expansion => 250.0 + 75.0 * city_count,
            GrandStrategy::Science => 250.0 + 50.0 * city_count,
            GrandStrategy::Religion => 150.0 + 50.0 * city_count,
            GrandStrategy::Conquest | GrandStrategy::Recovery => 75.0 + 25.0 * city_count,
        };
        let bank = g.players[pid].gold;
        let counts = self.counts(g, pid);
        let mut candidates = Vec::new();
        for action in g.legal_actions(pid) {
            let (city, item, currency) = match &action {
                Action::Buy {
                    city,
                    unit,
                    currency,
                } => (*city, Item::Unit { unit: unit.clone() }, currency.as_str()),
                Action::BuyBuilding {
                    city,
                    building,
                    currency,
                } => (
                    *city,
                    Item::Building {
                        building: building.clone(),
                    },
                    currency.as_str(),
                ),
                Action::BuyDistrict {
                    city,
                    district,
                    pos,
                    currency,
                } => (
                    *city,
                    Item::District {
                        district: district.clone(),
                        pos: *pos,
                    },
                    currency.as_str(),
                ),
                _ => continue,
            };
            if currency != "gold" {
                continue;
            }
            let production = g.city_yields(city).production.max(1.0);
            let turns = g.item_remaining_cost_for_city(pid, city, &item) / production;
            let production_score = self.production_value(g, pid, city, &item, plan, &counts);
            if production_score <= -1_000.0 {
                continue;
            }
            let mut after = g.clone();
            if after.apply(pid, &action).is_err() {
                continue;
            }
            let cost = (bank - after.players[pid].gold).max(0.0);
            if after.players[pid].gold + f64::EPSILON < reserve {
                continue;
            }
            let positional = production_score * (7.0 + turns.max(1.0));
            let score = positional + turns.clamp(0.0, 20.0) * 6.0 - cost * 0.30;
            if score >= 120.0 {
                candidates.push((score, std::cmp::Reverse(format!("{action:?}")), action));
            }
        }
        let best = candidates.into_iter().max_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        best.is_some_and(|(_, _, action)| g.apply(pid, &action).is_ok())
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
            if let Some((_, city, pos)) = best {
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
            return;
        }

        let religion_unfounded = g.players[pid].religion.is_none();
        let prophet_slot_open = g.religions_founded()
            + g.players
                .iter()
                .filter(|player| player.prophet_pending)
                .count()
            < g.max_religions();
        if religion_unfounded && !g.players[pid].prophet_pending && prophet_slot_open {
            let shrine_planned = city_ids.iter().any(|cid| {
                matches!(
                    g.cities[cid].queue.first(),
                    Some(Item::Building { building }) if building == "shrine"
                )
            });
            let has_shrine = city_ids.iter().any(|cid| {
                g.cities[cid]
                    .buildings
                    .iter()
                    .any(|building| building == "shrine")
            });
            if !has_shrine && g.religions_founded() == 0 {
                if shrine_planned {
                    return;
                }
                for cid in &city_ids {
                    let item = Item::Building {
                        building: "shrine".to_string(),
                    };
                    if g.cities[cid].queue.is_empty()
                        && g.cities[cid].districts.contains_key("holy_site")
                        && g.can_produce(pid, *cid, &item)
                    {
                        let _ = g.apply(pid, &Action::Produce { city: *cid, item });
                        return;
                    }
                }
                return;
            }

            let prayers = Item::Project {
                project: "holy_site_prayers".to_string(),
            };
            if let Some(city) = city_ids
                .iter()
                .filter(|cid| {
                    g.cities[cid].queue.is_empty()
                        && g.cities[cid].districts.contains_key("holy_site")
                        && g.can_produce(pid, **cid, &prayers)
                })
                .max_by(|left, right| {
                    g.city_yields(**left)
                        .production
                        .total_cmp(&g.city_yields(**right).production)
                        .then_with(|| right.cmp(left))
                })
                .copied()
            {
                let _ = g.apply(
                    pid,
                    &Action::Produce {
                        city,
                        item: prayers,
                    },
                );
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

    /// A rival founder's religion holding the majority in one of our cities.
    /// The religious victory requires every living major, so home
    /// reconversion alone denies it — this is the trigger for the
    /// cross-strategy defense below.
    fn home_conversion_threat(&self, g: &Game, pid: usize) -> Option<String> {
        let own = g.players[pid].religion.as_deref();
        let rival_faith = |religion: &str| {
            g.players.iter().any(|o| {
                o.id != pid && o.alive && !o.is_minor && o.religion.as_deref() == Some(religion)
            })
        };
        for cid in g.player_city_ids(pid) {
            let city = &g.cities[&cid];
            // React while the conversion is still in progress: waiting for a
            // flipped majority loses the pressure race outright. Any rival
            // faith at 60% of the city's strongest pressure is a live threat.
            let top = city.pressure.values().fold(0.0f64, |a, b| a.max(*b));
            if top <= 0.0 {
                continue;
            }
            for (religion, pressure) in &city.pressure {
                if Some(religion.as_str()) == own || *pressure + 1e-9 < top * 0.6 {
                    continue;
                }
                if rival_faith(religion) {
                    return Some(religion.clone());
                }
            }
        }
        None
    }

    /// Home religious defense for civilizations whose grand strategy is NOT
    /// religion. Founders reuse the emergency spending path; everyone else
    /// buys Missionaries of an adopted non-threat majority faith, which the
    /// engine now assigns from the purchase city (stock rule).
    fn religious_defense(&self, g: &mut Game, pid: usize, threat: &str) {
        if g.players[pid].religion.is_some() {
            self.religious_spending(g, pid, false);
            return;
        }
        let defenders = g
            .units
            .values()
            .filter(|unit| unit.owner == pid && unit.kind == "missionary")
            .count();
        if defenders >= 2 {
            return;
        }
        for cid in g.player_city_ids(pid) {
            let Some(majority) = g.city_religion(&g.cities[&cid]) else {
                continue;
            };
            if majority == threat {
                continue;
            }
            if g.apply(
                pid,
                &Action::Buy {
                    city: cid,
                    unit: "missionary".to_string(),
                    currency: "faith".to_string(),
                },
            )
            .is_ok()
            {
                return;
            }
        }
    }

    fn city_needs_religious_support(
        g: &Game,
        pid: usize,
        city: &crate::game::City,
        religion: &str,
    ) -> bool {
        if city.owner != pid {
            return false;
        }
        let own = city.pressure.get(religion).copied().unwrap_or(0.0);
        let rival = city
            .pressure
            .iter()
            .filter(|(faith, _)| faith.as_str() != religion)
            .map(|(_, pressure)| *pressure)
            .fold(0.0_f64, f64::max);
        g.city_religion(city) != Some(religion) || (rival > 0.0 && rival * 2.0 >= own)
    }

    /// Keep a founded religion's small field corps useful after the adaptive
    /// planner changes its primary victory strategy. Previously that switch
    /// made charged Missionaries stop at home and let thousands of Faith sit
    /// idle. A large surplus may start a secondary campaign, while an active
    /// spreader keeps it moving after the initial purchase lowers the bank.
    fn religious_offensive_posture(&self, g: &Game, pid: usize, strategy: GrandStrategy) -> bool {
        if strategy == GrandStrategy::Religion {
            return true;
        }
        let Some(religion) = g.players[pid].religion.as_deref() else {
            return false;
        };
        let foreign_target = g.cities.values().any(|city| {
            city.owner != pid
                && g.players[city.owner].alive
                && !g.players[city.owner].is_minor
                && !g.players[city.owner].is_barbarian
                && !g.is_at_war(pid, city.owner)
                && g.city_religion(city) != Some(religion)
        });
        if !foreign_target {
            return false;
        }
        let active_campaign = g.units.values().any(|unit| {
            unit.owner == pid
                && unit.religion.as_deref() == Some(religion)
                && unit.charges > 0
                && g.rules.units[unit.kind.as_str()].religious_spread > 0.0
        });
        active_campaign || g.players[pid].faith >= g.game_speed.scale(2_000.0)
    }

    fn religious_spending(&self, g: &mut Game, pid: usize, offensive: bool) {
        self.religious_spending_with_reserve(g, pid, offensive, 80.0);
    }

    fn religious_spending_with_reserve(
        &self,
        g: &mut Game,
        pid: usize,
        offensive: bool,
        ordinary_reserve: f64,
    ) {
        let Some(religion) = g.players[pid].religion.clone() else {
            return;
        };
        let match_point_defense = self
            .victory_denial(g, pid)
            .is_some_and(|(_, counter)| counter == GrandStrategy::Religion);
        let count_units = |kind: &str| {
            g.units
                .values()
                .filter(|unit| unit.owner == pid && unit.kind == kind)
                .count()
        };
        let missionaries = count_units("missionary");
        let apostles = count_units("apostle");
        let gurus = count_units("guru");
        let inquisitors = count_units("inquisitor");
        let defensive_targets = g
            .player_city_ids(pid)
            .into_iter()
            .filter(|cid| Self::city_needs_religious_support(g, pid, &g.cities[cid], &religion))
            .count();
        let home_under_pressure = defensive_targets > 0;
        let inquisition_launched = g.players[pid]
            .counters
            .get("inquisition")
            .copied()
            .unwrap_or(0)
            > 0;
        let spread_targets = defensive_targets
            + usize::from(offensive)
                * g.cities
                    .values()
                    .filter(|city| {
                        city.owner != pid
                            && !g.is_at_war(pid, city.owner)
                            && g.city_religion(city) != Some(religion.as_str())
                    })
                    .count();
        // A small circulating corps is enough: every Missionary has several
        // spreads, and replacements can be bought as charges are consumed.
        // Scaling gently with live targets preserves a religious push without
        // allowing one faith purchase every turn to fill the map.
        let missionary_cap = if spread_targets == 0 {
            0
        } else if offensive {
            (2 + spread_targets.div_ceil(4)).min(6)
        } else {
            (1 + defensive_targets.div_ceil(2)).min(2)
        };
        let apostle_cap = if offensive { 2 } else { 0 };
        let guru_cap = usize::from(offensive && apostles > 0);
        let inquisitor_cap = if home_under_pressure && inquisition_launched {
            2
        } else {
            0
        };
        let priorities: &[&str] = if home_under_pressure
            && inquisition_launched
            && inquisitors < 2
        {
            &["inquisitor", "apostle", "missionary", "guru"]
        } else if !offensive {
            &["missionary", "inquisitor"]
        } else if apostles < 2 {
            &["apostle", "missionary", "guru"]
        } else if gurus < 1 {
            &["guru", "apostle", "missionary"]
        } else {
            &["missionary", "apostle", "guru"]
        };
        for unit in priorities {
            let cap = match *unit {
                "missionary" => missionary_cap,
                "apostle" => apostle_cap,
                "guru" => guru_cap,
                "inquisitor" => inquisitor_cap,
                _ => 0,
            };
            let current = match *unit {
                "missionary" => missionaries,
                "apostle" => apostles,
                "guru" => gurus,
                "inquisitor" => inquisitors,
                _ => 0,
            };
            if current >= cap {
                continue;
            }
            let Some(spec) = g.rules.units.get(*unit) else {
                continue;
            };
            let price = spec.cost * 2.0;
            // The ordinary buffer is useful while safely building toward a
            // victory, but it must not block the last affordable defender at
            // match point or when one of our cities is already losing its
            // religious majority.
            let reserve = if match_point_defense || home_under_pressure {
                0.0
            } else {
                ordinary_reserve
            };
            if g.players[pid].faith + f64::EPSILON < price + reserve {
                continue;
            }
            let cities = g.player_city_ids(pid);
            for cid in cities {
                // Religious units inherit the purchase city's majority.  A
                // converted Holy Site must never make the defender spend its
                // Faith strengthening the runaway rival religion.
                if g.city_religion(&g.cities[&cid]) != Some(religion.as_str()) {
                    continue;
                }
                if g.apply(
                    pid,
                    &Action::Buy {
                        city: cid,
                        unit: (*unit).to_string(),
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
                Action::BuyBuilding {
                    city,
                    building,
                    currency,
                } if currency == "faith" => {
                    let spec = &g.rules.buildings[building];
                    let cost = g.building_faith_purchase_cost(pid, *city, building)?;
                    if g.players[pid].faith + f64::EPSILON < cost + reserve {
                        return None;
                    }
                    let worship = spec.worship_belief.is_some() as i32;
                    let defensive_value = match strategy {
                        GrandStrategy::Conquest | GrandStrategy::Recovery => spec.outer_defense * 2,
                        _ => spec.outer_defense,
                    };
                    let score = (self.yield_value(spec.yields, strategy) * 25.0) as i32
                        + (spec.housing * 35.0 + spec.amenity * 50.0) as i32
                        + spec.great_work_slots.values().sum::<i32>() * 60
                        + spec.trade_route_capacity * 100
                        + defensive_value
                        + worship * 220
                        - (cost * 0.05) as i32;
                    Some((score, std::cmp::Reverse((*city, building.clone())), action))
                }
                _ => None,
            })
            .max_by_key(|(score, key, _)| (*score, key.clone()));
        if let Some((_, _, action)) = best {
            let _ = g.apply(pid, &action);
        }
    }

    fn governor_priority(strategy: GrandStrategy) -> &'static [&'static str] {
        match strategy {
            GrandStrategy::Expansion => &[
                "magnus", "pingala", "liang", "reyna", "victor", "moksha", "amani",
            ],
            GrandStrategy::Science => &[
                "pingala", "magnus", "reyna", "liang", "victor", "moksha", "amani",
            ],
            GrandStrategy::Culture => &[
                "pingala", "reyna", "liang", "magnus", "moksha", "victor", "amani",
            ],
            GrandStrategy::Religion => &[
                "moksha", "pingala", "magnus", "amani", "liang", "victor", "reyna",
            ],
            GrandStrategy::Diplomacy => &[
                "amani", "pingala", "reyna", "magnus", "liang", "victor", "moksha",
            ],
            GrandStrategy::Conquest => &[
                "victor", "magnus", "pingala", "liang", "reyna", "moksha", "amani",
            ],
            GrandStrategy::Recovery => &[
                "victor", "reyna", "magnus", "pingala", "liang", "moksha", "amani",
            ],
        }
    }

    fn governor_promotion_priority(
        strategy: GrandStrategy,
        governor: &str,
    ) -> &'static [&'static str] {
        match governor {
            "pingala" if strategy == GrandStrategy::Culture => &[
                "connoisseur",
                "researcher",
                "grants",
                "curator",
                "space_initiative",
            ],
            "pingala" => &[
                "researcher",
                "connoisseur",
                "grants",
                "space_initiative",
                "curator",
            ],
            "magnus" => &[
                "provision",
                "surplus_logistics",
                "black_marketeer",
                "industrialist",
                "vertical_integration",
            ],
            "liang" => &[
                "zoning_commissioner",
                "aquaculture",
                "reinforced_materials",
                "water_works",
                "parks_and_recreation",
            ],
            "reyna" => &[
                "harbormaster",
                "forestry_management",
                "tax_collector",
                "contractor",
                "renewable_subsidizer",
            ],
            "victor" => &[
                "garrison_commander",
                "defense_logistics",
                "embrasure",
                "air_defense_initiative",
                "arms_race_proponent",
            ],
            "moksha" => &[
                "grand_inquisitor",
                "laying_on_of_hands",
                "citadel_of_god",
                "patron_saint",
                "divine_architect",
            ],
            "amani" => &[
                "emissary",
                "affluence",
                "local_informants",
                "foreign_investor",
                "puppeteer",
            ],
            _ => &[],
        }
    }

    fn best_governor_city(
        &self,
        g: &Game,
        pid: usize,
        governor: &str,
        plan: &StrategicPlan,
    ) -> Option<u32> {
        let occupied: BTreeSet<u32> = g.players[pid]
            .governor_roster
            .values()
            .filter_map(|state| state.city)
            .collect();
        let mut candidates: Vec<u32> = g
            .player_city_ids(pid)
            .into_iter()
            .filter(|city| !occupied.contains(city))
            .collect();
        if governor == "amani" {
            candidates.extend(
                g.players
                    .iter()
                    .filter(|player| {
                        player.alive
                            && player.is_minor
                            && !player.is_barbarian
                            && !g.is_at_war(pid, player.id)
                    })
                    .flat_map(|player| g.player_city_ids(player.id))
                    .filter(|city| !occupied.contains(city)),
            );
        }
        candidates.into_iter().max_by(|left, right| {
            let value = |city_id: u32| {
                let city = &g.cities[&city_id];
                let yields = g.city_yields(city_id);
                let own = city.owner == pid;
                let commercial = city.districts.keys().any(|district| {
                    matches!(g.district_family(district), "commercial_hub" | "harbor")
                }) as i32 as f64;
                let holy = city
                    .districts
                    .keys()
                    .any(|district| g.district_family(district) == "holy_site")
                    as i32 as f64;
                let base = if own {
                    (100.0 - city.loyalty).max(0.0) * 2.0
                } else {
                    0.0
                };
                base + match governor {
                    "pingala" => {
                        city.pop as f64 * 14.0 + yields.science * 9.0 + yields.culture * 9.0
                    }
                    "magnus" => {
                        city.pop as f64 * 5.0
                            + yields.food * 5.0
                            + yields.production * 11.0
                            + matches!(
                                city.queue.first(),
                                Some(Item::Unit { unit }) if unit == "settler"
                            ) as i32 as f64
                                * 180.0
                    }
                    "liang" => yields.production * 10.0 + city.owned_tiles.len() as f64 * 2.0,
                    "reyna" => city.pop as f64 * 8.0 + yields.gold * 13.0 + commercial * 150.0,
                    "victor" => {
                        plan.threatened_city.is_some_and(|target| target == city_id) as i32 as f64
                            * 600.0
                            + city.wall_hp.max(0) as f64
                            + city.pop as f64 * 5.0
                    }
                    "moksha" => {
                        yields.faith * 14.0
                            + holy * 180.0
                            + (g.players[pid].holy_city == Some(city_id)) as i32 as f64 * 220.0
                    }
                    "amani" if !own => 600.0 + g.envoys_at(pid, city.owner) as f64 * 55.0,
                    "amani" => (100.0 - city.loyalty).max(0.0) * 5.0,
                    _ => 0.0,
                }
            };
            value(*left)
                .partial_cmp(&value(*right))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.cmp(left))
        })
    }

    fn preferred_governor_promotion(
        &self,
        g: &Game,
        pid: usize,
        strategy: GrandStrategy,
        governor: &str,
    ) -> Option<String> {
        let available = g.available_governor_promotions(pid, governor);
        Self::governor_promotion_priority(strategy, governor)
            .iter()
            .find(|promotion| available.iter().any(|candidate| candidate == **promotion))
            .map(|promotion| (*promotion).to_string())
    }

    fn strategic_governors(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        let priority = Self::governor_priority(plan.strategy);
        while g.governor_titles_available(pid) > 0 {
            // Strategy can change every assessment window, but Governor
            // Titles arrive much more slowly. Finish the earliest incumbent's
            // two-promotion foundation before adapting the roster, otherwise
            // transient wars or victory races recreate the old dilution bug.
            let primary_name = g.players[pid]
                .governor_roster
                .iter()
                .filter(|(_, state)| state.promotions.len() < 2)
                .min_by_key(|(name, state)| (state.assigned_turn, name.as_str()))
                .map(|(name, _)| name.clone())
                .unwrap_or_else(|| priority[0].to_string());
            let primary = primary_name.as_str();
            if !g.players[pid].governor_roster.contains_key(primary) {
                if let Some(city) = self.best_governor_city(g, pid, primary, plan) {
                    if g.apply(
                        pid,
                        &Action::AppointGovernor {
                            governor: primary.to_string(),
                            city,
                        },
                    )
                    .is_ok()
                    {
                        continue;
                    }
                }
            }

            let primary_promotions = g.players[pid]
                .governor_roster
                .get(primary)
                .map(|state| state.promotions.len())
                .unwrap_or(0);
            if primary_promotions < 2 {
                if let Some(promotion) =
                    self.preferred_governor_promotion(g, pid, plan.strategy, primary)
                {
                    if g.apply(
                        pid,
                        &Action::PromoteGovernor {
                            governor: primary.to_string(),
                            promotion,
                        },
                    )
                    .is_ok()
                    {
                        continue;
                    }
                }
            }

            // After both tier-one promotions are online, establish one
            // complementary governor before completing the primary's tree.
            if g.players[pid].governor_roster.len() < 2 {
                if let Some((governor, city)) = priority.iter().skip(1).find_map(|governor| {
                    (!g.players[pid].governor_roster.contains_key(*governor))
                        .then(|| {
                            self.best_governor_city(g, pid, governor, plan)
                                .map(|city| ((*governor).to_string(), city))
                        })
                        .flatten()
                }) {
                    if g.apply(pid, &Action::AppointGovernor { governor, city })
                        .is_ok()
                    {
                        continue;
                    }
                }
            }

            if let Some(promotion) =
                self.preferred_governor_promotion(g, pid, plan.strategy, primary)
            {
                if g.apply(
                    pid,
                    &Action::PromoteGovernor {
                        governor: primary.to_string(),
                        promotion,
                    },
                )
                .is_ok()
                {
                    continue;
                }
            }

            // Add a third regional anchor before investing deeply in the
            // complementary governor. Further titles finish existing trees;
            // only then does the roster widen again.
            if g.players[pid].governor_roster.len() < 3 {
                if let Some((governor, city)) = priority.iter().skip(1).find_map(|governor| {
                    (!g.players[pid].governor_roster.contains_key(*governor))
                        .then(|| {
                            self.best_governor_city(g, pid, governor, plan)
                                .map(|city| ((*governor).to_string(), city))
                        })
                        .flatten()
                }) {
                    if g.apply(pid, &Action::AppointGovernor { governor, city })
                        .is_ok()
                    {
                        continue;
                    }
                }
            }

            let next_promotion = priority.iter().find_map(|governor| {
                self.preferred_governor_promotion(g, pid, plan.strategy, governor)
                    .map(|promotion| ((*governor).to_string(), promotion))
            });
            if let Some((governor, promotion)) = next_promotion {
                if g.apply(
                    pid,
                    &Action::PromoteGovernor {
                        governor,
                        promotion,
                    },
                )
                .is_ok()
                {
                    continue;
                }
            }

            let appointment = priority.iter().find_map(|governor| {
                (!g.players[pid].governor_roster.contains_key(*governor))
                    .then(|| {
                        self.best_governor_city(g, pid, governor, plan)
                            .map(|city| ((*governor).to_string(), city))
                    })
                    .flatten()
            });
            let Some((governor, city)) = appointment else {
                break;
            };
            if g.apply(pid, &Action::AppointGovernor { governor, city })
                .is_err()
            {
                break;
            }
        }

    }

    /// A faith-rich empire countering a military or religious victory threat
    /// should convert that otherwise stranded treasury into defenders once
    /// Theocracy (or another legal faith-purchase source) makes them available.
    fn military_faith_spending(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) -> bool {
        if !matches!(
            plan.strategy,
            GrandStrategy::Conquest | GrandStrategy::Recovery
        ) || g.players[pid].faith < 600.0
        {
            return false;
        }
        let bank = g.players[pid].faith;
        let reserve = 180.0;
        let counts = self.counts(g, pid);
        let mut candidates = Vec::new();
        for action in g.legal_actions(pid) {
            let Action::Buy {
                city,
                unit,
                currency,
            } = &action
            else {
                continue;
            };
            if currency != "faith" || g.rules.units[unit.as_str()].class != "military" {
                continue;
            }
            let mut after = g.clone();
            if after.apply(pid, &action).is_err() || after.players[pid].faith < reserve {
                continue;
            }
            let cost = (bank - after.players[pid].faith).max(0.0);
            let spec = &g.rules.units[unit.as_str()];
            let combat = spec
                .strength
                .max(spec.ranged_strength)
                .max(spec.bombard_strength);
            let strategic = self
                .production_value(
                    g,
                    pid,
                    *city,
                    &Item::Unit { unit: unit.clone() },
                    plan,
                    &counts,
                )
                .max(0.0);
            let score = strategic + combat * 12.0 - cost * 0.25;
            candidates.push((score, std::cmp::Reverse((*city, unit.clone())), action));
        }
        candidates
            .into_iter()
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| left.1.cmp(&right.1))
            })
            .is_some_and(|(_, _, action)| g.apply(pid, &action).is_ok())
    }

    fn science_production(&self, g: &mut Game, pid: usize) {
        let completed = g.players[pid].science_projects.clone();
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
        let parallel_project = matches!(
            project,
            "lagrange_laser_station" | "terrestrial_laser_station"
        );
        let already_queued = !parallel_project
            && g.player_city_ids(pid).iter().any(|cid| {
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
                        && !matches!(
                            g.cities[cid].queue.first(),
                            Some(Item::Project { project: queued }) if queued == project
                        )
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

        let city_ids = g.player_city_ids(pid);
        let built_spaceports = city_ids
            .iter()
            .filter(|cid| g.cities[cid].districts.contains_key("spaceport"))
            .count();
        let queued_spaceports = city_ids
            .iter()
            .filter(|cid| {
                matches!(
                    g.cities[cid].queue.first(),
                    Some(Item::District { district, .. }) if district == "spaceport"
                )
            })
            .count();
        // One launch site is enough for the sequential opening missions. A
        // second can prepare Mars while the first launches, and up to three
        // let the post-Exoplanet laser race run in parallel. Separate cities
        // matter; duplicate Spaceports in one production queue do not.
        let desired_spaceports = if self.victory_target == Some(VictoryTarget::Science) {
            if completed.contains("launch_mars_colony") {
                3
            } else if completed.contains("launch_moon_landing") {
                2
            } else {
                1
            }
        } else {
            1
        }
        .min(city_ids.len());
        if built_spaceports + queued_spaceports >= desired_spaceports {
            return;
        }
        let mut best: Option<(f64, u32, Pos)> = None;
        for cid in city_ids {
            if g.cities[&cid].districts.contains_key("spaceport")
                || matches!(
                    g.cities[&cid].queue.first(),
                    Some(Item::District { district, .. }) if district == "spaceport"
                )
            {
                continue;
            }
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

    fn advanced_spies(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        let ids: Vec<u32> = g
            .spies
            .values()
            .filter(|spy| spy.owner == pid)
            .map(|spy| spy.id)
            .collect();
        let infiltrated_cities: BTreeSet<u32> = g
            .spies
            .values()
            .filter(|spy| spy.owner != pid && spy.captured_by.is_none())
            .filter_map(|spy| {
                spy.city
                    .filter(|city| g.cities.get(city).is_some_and(|city| city.owner == pid))
            })
            .collect();
        let home_defender = ids
            .iter()
            .copied()
            .filter(|spy| {
                g.spies[spy].city.is_some_and(|city| {
                    // A spy outlives the city it was posted to: that city can
                    // be razed while the agent is still assigned there, and
                    // indexing the map for it took the whole server down with
                    // it. The infiltration scan above already reads this the
                    // safe way.
                    g.cities.get(&city).is_some_and(|home| {
                        home.owner == pid
                            && (home.districts.contains_key("spaceport")
                                || infiltrated_cities.contains(&city))
                    })
                })
            })
            .min();
        for spy_id in ids {
            let legal = g.legal_spy_actions(pid, spy_id);
            if legal.is_empty() {
                continue;
            }
            let promotion_priority: &[&str] = match plan.strategy {
                GrandStrategy::Science => &[
                    "technologist",
                    "rocket_scientist",
                    "disguise",
                    "linguist",
                    "quartermaster",
                ],
                GrandStrategy::Culture => &[
                    "cat_burglar",
                    "con_artist",
                    "disguise",
                    "linguist",
                    "surveillance",
                ],
                GrandStrategy::Diplomacy => &[
                    "smear_campaign",
                    "polygraph",
                    "quartermaster",
                    "seduction",
                    "disguise",
                ],
                GrandStrategy::Conquest => &[
                    "license_to_kill",
                    "demolitions",
                    "guerrilla_leader",
                    "covert_action",
                    "ace_driver",
                ],
                _ => &[
                    "quartermaster",
                    "seduction",
                    "con_artist",
                    "technologist",
                    "linguist",
                ],
            };
            if let Some(action) = promotion_priority
                .iter()
                .find_map(|wanted| {
                    legal.iter().find(|action| {
                        matches!(action, Action::PromoteSpy { promotion, .. } if promotion == *wanted)
                    })
                })
                .or_else(|| {
                    legal
                        .iter()
                        .find(|action| matches!(action, Action::PromoteSpy { .. }))
                })
            {
                let _ = g.apply(pid, action);
                continue;
            }
            let current_city = g.spies.get(&spy_id).and_then(|spy| spy.city);
            if Some(spy_id) == home_defender
                && matches!(
                    plan.strategy,
                    GrandStrategy::Science | GrandStrategy::Recovery
                )
                && current_city.is_some_and(|city| g.cities[&city].owner == pid)
            {
                let spaceport = current_city.and_then(|city| {
                    g.cities[&city]
                        .districts
                        .iter()
                        .find_map(|(district, position)| {
                            (g.district_family(district) == "spaceport").then_some(*position)
                        })
                });
                if let Some(action) = legal
                    .iter()
                    .filter(|action| {
                        matches!(action, Action::SpyMission { mission, .. } if mission == "counterspy")
                    })
                    .max_by_key(|action| match action {
                        Action::SpyMission { target, .. } => {
                            (Some(*target) == spaceport, std::cmp::Reverse(*target))
                        }
                        _ => unreachable!(),
                    })
                {
                    let _ = g.apply(pid, action);
                    continue;
                }
            }
            let offensive = current_city
                .and_then(|city| g.cities.get(&city))
                .is_some_and(|city| city.owner != pid);
            if offensive {
                if g.spies[&spy_id].level < 2 {
                    if let Some(action) = legal.iter().find(|action| {
                        matches!(action, Action::SpyMission { mission, .. } if mission == "gain_sources")
                    }) {
                        let _ = g.apply(pid, action);
                        continue;
                    }
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
                        if matches!(mission.as_str(), "gain_sources" | "counterspy") {
                            return None;
                        }
                        let city = current_city?;
                        let defender = g.cities[&city].owner;
                        let active = crate::game::SpyMission {
                            kind: mission.clone(),
                            city,
                            target: *target,
                            started: g.turn,
                            ends: g.turn,
                        };
                        let strategic = match (plan.strategy, mission.as_str()) {
                            (GrandStrategy::Science, "steal_tech_boost") => 320.0,
                            (GrandStrategy::Science, "disrupt_rocketry") => 290.0,
                            (GrandStrategy::Culture, "great_work_heist") => 340.0,
                            (GrandStrategy::Culture, "siphon_funds") => 135.0,
                            (GrandStrategy::Diplomacy, "fabricate_scandal") => 330.0,
                            (GrandStrategy::Diplomacy, "listening_post") => 185.0,
                            (GrandStrategy::Conquest, "neutralize_governor") => 310.0,
                            (GrandStrategy::Conquest, "sabotage_production") => 260.0,
                            (GrandStrategy::Conquest, "recruit_partisans") => 245.0,
                            (GrandStrategy::Conquest, "foment_unrest") => 230.0,
                            (GrandStrategy::Conquest, "breach_dam") => 210.0,
                            (_, "siphon_funds") => 150.0,
                            (_, "steal_tech_boost") => 145.0,
                            (_, "great_work_heist") => 135.0,
                            (_, "neutralize_governor") => 125.0,
                            (_, "fabricate_scandal") => 120.0,
                            (_, "listening_post") => 75.0,
                            _ => 100.0,
                        } + if plan.target_player == Some(defender) {
                            90.0
                        } else {
                            0.0
                        };
                        Some((
                            strategic * g.spy_success_chance(*spy, &active),
                            mission,
                            action,
                        ))
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
                    Action::AssignSpy { city, .. } if g.cities[city].owner != pid => {
                        let target = &g.cities[city];
                        let strategic = if plan.target_player == Some(target.owner) {
                            180
                        } else {
                            0
                        } + match plan.strategy {
                            GrandStrategy::Science => {
                                i32::from(target.districts.contains_key("campus")) * 90
                                    + i32::from(target.districts.contains_key("spaceport")) * 150
                            }
                            GrandStrategy::Culture => {
                                i32::from(target.districts.contains_key("theater_square")) * 140
                            }
                            GrandStrategy::Diplomacy => {
                                i32::from(g.players[target.owner].is_minor) * 180
                            }
                            GrandStrategy::Conquest => {
                                i32::from(g.city_can_strike(target)) * 35
                                    + i32::from(g.players[target.owner].governors.contains(city))
                                        * 120
                            }
                            _ => i32::from(target.districts.contains_key("commercial_hub")) * 70,
                        };
                        Some((
                            strategic
                                + target.pop * 8
                                + target.districts.len() as i32 * 14
                                + target.wonders.len() as i32 * 24,
                            std::cmp::Reverse(*city),
                            action,
                        ))
                    }
                    _ => None,
                })
                .max_by_key(|(score, city, _)| (*score, *city))
                .map(|(_, _, action)| action);
            if let Some(action) = assignment {
                let _ = g.apply(pid, action);
            }
        }
    }

    fn support_unit_value(
        &self,
        g: &Game,
        pid: usize,
        cid: u32,
        unit: &str,
        plan: &StrategicPlan,
        counts: &EmpireCounts,
    ) -> f64 {
        let spec = &g.rules.units[unit];
        if spec.class != "support" || unit == "military_engineer" {
            return -10_000.0;
        }
        if spec.anti_air_strength > 0.0 {
            let hostile_aircraft = g
                .units
                .values()
                .filter(|candidate| {
                    candidate.owner != pid
                        && g.is_at_war(pid, candidate.owner)
                        && g.rules.units[candidate.kind.as_str()].domain.as_deref() == Some("air")
                })
                .count();
            let desired = hostile_aircraft.min(g.player_city_ids(pid).len().div_ceil(2).max(1));
            if desired == 0 || counts.air_defense >= desired {
                return -10_000.0;
            }
            let best_available = g
                .rules
                .units
                .iter()
                .filter(|(name, candidate)| {
                    candidate.class == "support"
                        && candidate.anti_air_strength > 0.0
                        && g.can_produce(
                            pid,
                            cid,
                            &Item::Unit {
                                unit: (*name).clone(),
                            },
                        )
                })
                .map(|(_, candidate)| candidate.anti_air_strength)
                .fold(0.0_f64, f64::max);
            if spec.anti_air_strength + 5.0 < best_available {
                return -2_000.0;
            }
            return 340.0
                + spec.anti_air_strength * 3.0
                + hostile_aircraft.min(4) as f64 * 65.0
                + desired.saturating_sub(counts.air_defense) as f64 * 90.0;
        }
        let land_military = counts
            .military
            .saturating_sub(counts.naval + counts.aircraft);
        let field_support = counts
            .support
            .saturating_sub(counts.military_engineers + counts.air_defense);
        let desired_support = if land_military >= 8 {
            2
        } else if land_military >= 3 {
            1
        } else {
            0
        };
        if field_support >= desired_support {
            return -10_000.0;
        }

        let existing_kinds: Vec<&str> = g
            .units
            .values()
            .filter(|candidate| candidate.owner == pid)
            .map(|candidate| candidate.kind.as_str())
            .chain(
                g.cities
                    .values()
                    .filter(|city| city.owner == pid)
                    .filter_map(|city| match city.queue.first() {
                        Some(Item::Unit { unit }) => Some(unit.as_str()),
                        _ => None,
                    }),
            )
            .collect();
        let has_capability = |effect: &str| {
            existing_kinds.iter().any(|kind| {
                g.rules.units[*kind]
                    .effects
                    .get(effect)
                    .is_some_and(|amount| *amount > 0.0)
            })
        };
        let is_breach = matches!(unit, "battering_ram" | "siege_tower");
        if (spec
            .effects
            .get("adjacent_siege_range")
            .copied()
            .unwrap_or(0.0)
            > 0.0
            && has_capability("adjacent_siege_range"))
            || (spec.effects.get("adjacent_heal").copied().unwrap_or(0.0) > 0.0
                && has_capability("adjacent_heal"))
            || (is_breach
                && existing_kinds
                    .iter()
                    .any(|kind| matches!(*kind, "battering_ram" | "siege_tower")))
        {
            return -10_000.0;
        }

        let target_cities: Vec<_> = plan
            .target_city
            .and_then(|city| g.cities.get(&city))
            .into_iter()
            .chain(g.cities.values().filter(|city| {
                city.owner != pid
                    && g.is_at_war(pid, city.owner)
                    && plan.target_city != Some(city.id)
            }))
            .collect();
        let breach_value = if is_breach {
            target_cities
                .iter()
                .filter(|city| !g.players[city.owner].techs.contains("steel"))
                .map(|city| {
                    let wall_levels = city
                        .buildings
                        .iter()
                        .filter(|building| g.rules.buildings[building.as_str()].outer_defense > 0)
                        .count();
                    match unit {
                        "battering_ram" if wall_levels == 1 => 760.0,
                        "siege_tower" if (1..=2).contains(&wall_levels) => 800.0,
                        _ => 0.0,
                    }
                })
                .fold(0.0_f64, f64::max)
        } else {
            0.0
        };
        let siege_range = spec
            .effects
            .get("adjacent_siege_range")
            .copied()
            .unwrap_or(0.0);
        let siege_bombard = spec
            .effects
            .get("adjacent_siege_bombard")
            .copied()
            .unwrap_or(0.0);
        let siege_value = if counts.siege > 0 {
            siege_range * 470.0 + siege_bombard * 38.0
        } else {
            0.0
        };
        let wounded = g
            .units
            .values()
            .filter(|candidate| {
                candidate.owner == pid
                    && candidate.hp < 100
                    && g.rules.units[candidate.kind.as_str()].class == "military"
            })
            .count() as f64;
        let heal = spec.effects.get("adjacent_heal").copied().unwrap_or(0.0);
        let movement = spec
            .effects
            .get("adjacent_movement")
            .copied()
            .unwrap_or(0.0);
        let logistics_value = if heal > 0.0 {
            heal * 12.0 + wounded.min(4.0) * 85.0 + movement * 210.0
        } else {
            0.0
        };
        let value = breach_value.max(siege_value).max(logistics_value);
        if value > 0.0 {
            value
                + if plan.strategy == GrandStrategy::Conquest {
                    140.0
                } else {
                    0.0
                }
        } else {
            -10_000.0
        }
    }

    /// The adaptive agent normally delegates routine city queues to the
    /// lightweight governor. Reserve at most one empty queue per turn for a
    /// support capability that the active campaign and army can actually use.
    fn advanced_support_production(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        if self.base.book_pos < 4
            || !g
                .players
                .iter()
                .any(|other| other.id != pid && g.is_at_war(pid, other.id))
        {
            return;
        }
        let counts = self.counts(g, pid);
        let mut best: Option<(f64, u32, String)> = None;
        for city in g
            .cities
            .values()
            .filter(|city| city.owner == pid && city.queue.is_empty())
        {
            for item in g.producible_items(pid, city.id) {
                let Item::Unit { unit } = item else { continue };
                if g.rules.units[unit.as_str()].class != "support" || unit == "military_engineer" {
                    continue;
                }
                let value = self.production_value(
                    g,
                    pid,
                    city.id,
                    &Item::Unit { unit: unit.clone() },
                    plan,
                    &counts,
                );
                if best.as_ref().is_none_or(|(old, old_city, old_unit)| {
                    value > *old + 1e-9
                        || ((value - *old).abs() < 1e-9
                            && (city.id, unit.as_str()) < (*old_city, old_unit.as_str()))
                }) {
                    best = Some((value, city.id, unit));
                }
            }
        }
        let Some((value, city, unit)) = best else {
            return;
        };
        if value > 0.0 {
            let _ = g.apply(
                pid,
                &Action::Produce {
                    city,
                    item: Item::Unit { unit },
                },
            );
        }
    }

    /// A live strategic pivot must reach city queues, not only policies and
    /// unit orders. Pause repeatable economic projects when Conquest or
    /// Recovery has a real land-force gap; item progress remains banked and
    /// can resume after the emergency. One-off and victory projects are never
    /// interrupted here.
    fn redirect_repeatable_projects_for_force_gap(
        &self,
        g: &mut Game,
        pid: usize,
        plan: &StrategicPlan,
    ) {
        if !matches!(
            plan.strategy,
            GrandStrategy::Conquest | GrandStrategy::Recovery
        ) {
            return;
        }
        let city_ids = g.player_city_ids(pid);
        let desired_land = 2 * city_ids.len();
        for cid in city_ids {
            let counts = self.counts(g, pid);
            let land = counts
                .military
                .saturating_sub(counts.naval + counts.aircraft);
            if land >= desired_land {
                return;
            }
            let Some(Item::Project { project }) = g.cities[&cid].queue.first() else {
                continue;
            };
            let project = project.clone();
            let spec = &g.rules.projects[&project];
            if !spec.repeatable
                || (spec.completion_gpp.is_empty() && spec.ongoing_yields.is_empty())
            {
                continue;
            }
            let best = g
                .producible_items(pid, cid)
                .into_iter()
                .filter(|item| {
                    let Item::Unit { unit } = item else {
                        return false;
                    };
                    let unit = &g.rules.units[unit];
                    unit.class == "military"
                        && unit.domain.as_deref() != Some("sea")
                        && unit.domain.as_deref() != Some("air")
                })
                .map(|item| {
                    let score = self.production_value(g, pid, cid, &item, plan, &counts);
                    (score, std::cmp::Reverse(format!("{item:?}")), item)
                })
                .max_by(|left, right| {
                    left.0
                        .total_cmp(&right.0)
                        .then_with(|| left.1.cmp(&right.1))
                });
            if let Some((score, _, item)) = best {
                if score > 0.0 {
                    let _ = g.apply(pid, &Action::Produce { city: cid, item });
                }
            }
        }
    }

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
        let turns = g.item_remaining_cost_for_city(pid, cid, item) / production;
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
                let expansion_open = if self.victory_target.is_some() {
                    g.turn < g.standard_duration(175)
                } else {
                    Self::expansion_window_open(g)
                };
                if city_count + counts.settlers < plan.desired_cities
                    && counts.settlers == 0
                    && city.pop >= 2
                    && expansion_open
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
                let open_capacity = g
                    .trade_capacity(pid)
                    .saturating_sub(g.active_routes(pid))
                    .max(0) as usize;
                let usable_capacity = open_capacity.min(self.trade_route_opportunity_count(g, pid));
                if counts.traders < usable_capacity {
                    let opportunity = self
                        .best_trade_route_origin(g, pid, city.pos, plan.strategy)
                        .map(|(value, _)| value)
                        .unwrap_or(0.0);
                    280.0
                        + opportunity.max(0.0) * 18.0
                        + usable_capacity.saturating_sub(counts.traders) as f64 * 45.0
                } else {
                    -10_000.0
                }
            }
            Item::Unit { unit } if unit == "spy" => {
                let active = g.spies.values().filter(|spy| spy.owner == pid).count();
                let strategic = match plan.strategy {
                    GrandStrategy::Science | GrandStrategy::Culture => 850.0,
                    GrandStrategy::Diplomacy | GrandStrategy::Conquest => 1_050.0,
                    GrandStrategy::Recovery => 650.0,
                    _ => 500.0,
                };
                if active < g.spy_capacity(pid).max(0) as usize {
                    1_500.0 + strategic + active as f64 * 90.0
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
            Item::Unit { unit } if unit == "military_engineer" => {
                let engineering_districts = g
                    .cities
                    .values()
                    .filter(|candidate| candidate.owner == pid)
                    .filter(|candidate| {
                        matches!(
                            candidate.queue.first(),
                            Some(Item::District { district, .. })
                                if matches!(
                                    g.district_family(district),
                                    "aqueduct" | "canal" | "dam"
                                )
                        )
                    })
                    .count();
                if engineering_districts > counts.military_engineers {
                    390.0 + engineering_districts as f64 * 70.0
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
                    let aircraft = spec.domain.as_deref() == Some("air");
                    let desired_naval = BasicAi::desired_navy(g, pid);
                    let desired_aircraft = if plan.strategy == GrandStrategy::Conquest {
                        city_count.max(1)
                    } else {
                        city_count.div_ceil(2).max(1)
                    };
                    let land_military = counts
                        .military
                        .saturating_sub(counts.naval + counts.aircraft);
                    if naval && !BasicAi::city_is_coastal(g, cid) {
                        return -10_000.0;
                    }
                    let domain_saturated = if naval {
                        counts.naval >= desired_naval
                    } else if aircraft {
                        counts.aircraft >= desired_aircraft
                    } else {
                        land_military >= desired_military
                    };
                    if self.victory_target.is_some()
                        && self.victory_target != Some(VictoryTarget::Domination)
                        && domain_saturated
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
                    let force_gap = if naval {
                        desired_naval.saturating_sub(counts.naval) as f64
                    } else if aircraft {
                        desired_aircraft.saturating_sub(counts.aircraft) as f64
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
                    } else if aircraft {
                        0.0
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
                } else if spec.class == "support" {
                    self.support_unit_value(g, pid, cid, unit, plan, counts)
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
                let family = g.district_family(district);
                if family == "spaceport" && city.districts.contains_key("spaceport") {
                    // Multiple Spaceports are rules-legal, but one city can
                    // execute only one project at a time. Put additional
                    // launch sites in other cities for actual parallelism.
                    return -10_000.0;
                }
                let district_count = g
                    .cities
                    .values()
                    .filter(|candidate| {
                        candidate.owner == pid
                            && candidate
                                .districts
                                .keys()
                                .any(|built| g.district_family(built) == family)
                    })
                    .count();
                let balanced_core = if district_count * 2 < city_count {
                    match family {
                        "campus" | "theater_square" | "commercial_hub" => 130.0,
                        "harbor" | "industrial_zone" => 90.0,
                        _ => 0.0,
                    }
                } else {
                    0.0
                };

                // Evaluate the rules engine's actual post-construction
                // housing rather than duplicating Aqueduct water rules or
                // the appeal bands used by Neighborhoods and Preserves.
                let mut developed = city.clone();
                developed.districts.insert(district.clone(), *pos);
                let housing_gain = (g.city_housing(&developed) - g.city_housing(city)).max(0.0);
                let housing_need = (city.pop as f64 + 2.0 - g.city_housing(city)).max(0.0);
                let amenity_gain = g.district_amenity(district, *pos);
                let amenity_need = (-g.city_amenity_surplus(city)).max(0) as f64;
                let great_people = spec.great_person_points.values().sum::<f64>();
                let relevant_great_people = match plan.strategy {
                    GrandStrategy::Science => spec
                        .great_person_points
                        .get("scientist")
                        .copied()
                        .unwrap_or(0.0),
                    GrandStrategy::Culture => ["writer", "artist", "musician"]
                        .into_iter()
                        .map(|kind| spec.great_person_points.get(kind).copied().unwrap_or(0.0))
                        .sum(),
                    GrandStrategy::Religion => spec
                        .great_person_points
                        .get("prophet")
                        .copied()
                        .unwrap_or(0.0),
                    GrandStrategy::Diplomacy => spec
                        .great_person_points
                        .get("merchant")
                        .copied()
                        .unwrap_or(0.0),
                    GrandStrategy::Conquest => ["general", "admiral"]
                        .into_iter()
                        .map(|kind| spec.great_person_points.get(kind).copied().unwrap_or(0.0))
                        .sum(),
                    GrandStrategy::Expansion | GrandStrategy::Recovery => spec
                        .great_person_points
                        .get("engineer")
                        .copied()
                        .unwrap_or(0.0),
                };
                let effects = &spec.effects;
                let effect_value = effects.get("governor_titles").copied().unwrap_or(0.0) * 520.0
                    + effects.get("envoys").copied().unwrap_or(0.0)
                        * if plan.strategy == GrandStrategy::Diplomacy {
                            300.0
                        } else {
                            170.0
                        }
                    + effects
                        .get("envoy_if_adjacent_city_center")
                        .copied()
                        .unwrap_or(0.0)
                        * if g.wdist(city.pos, *pos) == 1 {
                            if plan.strategy == GrandStrategy::Diplomacy {
                                300.0
                            } else {
                                170.0
                            }
                        } else {
                            0.0
                        }
                    + effects.get("spy_defense_levels").copied().unwrap_or(0.0) * 75.0
                    + effects.get("flood_protection").copied().unwrap_or(0.0) * 160.0
                    + effects.get("drought_protection").copied().unwrap_or(0.0) * 55.0
                    + effects.get("culture_bomb").copied().unwrap_or(0.0) * 85.0
                    + effects.get("naval_passage").copied().unwrap_or(0.0)
                        * if plan.strategy == GrandStrategy::Conquest {
                            150.0
                        } else {
                            75.0
                        }
                    + effects
                        .get("gold_faith_purchase_discount_pct")
                        .copied()
                        .unwrap_or(0.0)
                        * 8.0
                    + effects
                        .get("corps_army_discount_pct")
                        .copied()
                        .unwrap_or(0.0)
                        * if plan.strategy == GrandStrategy::Conquest {
                            8.0
                        } else {
                            2.0
                        }
                    + effects.get("free_heavy_cavalry").copied().unwrap_or(0.0)
                        * if plan.strategy == GrandStrategy::Conquest {
                            380.0
                        } else {
                            180.0
                        }
                    + effects
                        .get("naval_settler_production_pct")
                        .copied()
                        .unwrap_or(0.0)
                        * if plan.strategy == GrandStrategy::Expansion {
                            4.0
                        } else {
                            1.5
                        }
                    + effects.get("naval_heal_full").copied().unwrap_or(0.0) * 90.0
                    + effects.get("naval_movement").copied().unwrap_or(0.0) * 130.0
                    + effects
                        .get("foreign_continent_loyalty")
                        .copied()
                        .unwrap_or(0.0)
                        * 22.0
                    + effects.get("tourism_after_flight").copied().unwrap_or(0.0)
                        * if plan.strategy == GrandStrategy::Culture {
                            180.0
                        } else {
                            35.0
                        }
                    + effects
                        .get("border_growth_on_great_person")
                        .copied()
                        .unwrap_or(0.0)
                        * 90.0
                    + effects.get("unlock_apprenticeship").copied().unwrap_or(0.0) * 120.0;

                let strategic_family = match (plan.strategy, family) {
                    (GrandStrategy::Science, "spaceport") if district_count == 0 => 3_000.0,
                    (GrandStrategy::Science, "spaceport") => 250.0,
                    (GrandStrategy::Science, "campus") => 170.0,
                    (GrandStrategy::Science, "industrial_zone") => 150.0,
                    (GrandStrategy::Religion, "holy_site") => 210.0,
                    (GrandStrategy::Culture, "theater_square") => 850.0,
                    (GrandStrategy::Culture, "preserve") => 210.0,
                    (GrandStrategy::Diplomacy, "diplomatic_quarter") => 360.0,
                    (GrandStrategy::Diplomacy, "commercial_hub") => 150.0,
                    (GrandStrategy::Diplomacy, "harbor") => 130.0,
                    (GrandStrategy::Diplomacy, "theater_square") => 100.0,
                    (GrandStrategy::Conquest, "encampment") => 170.0,
                    (GrandStrategy::Conquest, "aerodrome") => 280.0,
                    (GrandStrategy::Conquest, "harbor") => 150.0,
                    (GrandStrategy::Conquest, "industrial_zone") => 160.0,
                    (GrandStrategy::Conquest, "canal") => 120.0,
                    (GrandStrategy::Recovery, "industrial_zone") => 190.0,
                    (GrandStrategy::Recovery, "dam") => 180.0,
                    (GrandStrategy::Recovery, "aqueduct") => 120.0,
                    (GrandStrategy::Expansion, "commercial_hub" | "harbor") => 90.0,
                    (GrandStrategy::Expansion, "aqueduct" | "neighborhood") => 110.0,
                    _ => 0.0,
                };
                let first_copy = match family {
                    "government_plaza" if district_count == 0 => 420.0,
                    "diplomatic_quarter" if district_count == 0 => 180.0,
                    "aerodrome" if district_count == 0 && counts.aircraft > 0 => 260.0,
                    _ => 0.0,
                };
                let development_penalty = if spec.specialty
                    && !city.districts.is_empty()
                    && city.buildings.len() <= city.districts.len()
                {
                    -120.0
                } else {
                    0.0
                };
                self.yield_value(g.district_yields(district, *pos), plan.strategy) * 60.0
                    + self.yield_value(spec.citizen_yields, plan.strategy) * 24.0
                    + spec.defense * if threatened { 5.0 } else { 1.5 }
                    + housing_gain * (32.0 + housing_need * 18.0)
                    + amenity_gain * (55.0 + amenity_need * 35.0)
                    + spec.loyalty * if city.loyalty < 76.0 { 22.0 } else { 7.0 }
                    + spec.air_slots.max(0) as f64
                        * if plan.strategy == GrandStrategy::Conquest || counts.aircraft > 0 {
                            95.0
                        } else {
                            25.0
                        }
                    + spec.appeal
                        * if plan.strategy == GrandStrategy::Culture {
                            35.0
                        } else {
                            8.0
                        }
                    + great_people * 30.0
                    + relevant_great_people * 85.0
                    + balanced_core
                    + strategic_family
                    + first_copy
                    + effect_value
                    + development_penalty
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
                    let spec = &g.rules.projects[project];
                    match project.as_str() {
                        "repair_outer_defenses" => {
                            let missing = (g.city_max_wall_hp(city) - city.wall_hp).max(0);
                            900.0 + missing as f64 * 12.0 + if threatened { 1_500.0 } else { 0.0 }
                        }
                        "repair_encampment" => {
                            let missing = (100 - city.encampment_hp).max(0)
                                + (g.city_max_wall_hp(city) - city.encampment_wall_hp).max(0);
                            700.0 + missing as f64 * 10.0 + if threatened { 1_150.0 } else { 0.0 }
                        }
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
                        _ if !spec.completion_gpp.is_empty()
                            || !spec.ongoing_yields.is_empty()
                            || spec.full_power_while_active
                            || project == "bread_and_circuses" =>
                        {
                            self.district_project_value(g, pid, cid, project, plan)
                        }
                        // Scenario and future projects without an understood
                        // economic effect remain legal, but cannot crowd out
                        // infrastructure solely because they are repeatable.
                        _ => 180.0,
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

    /// Lower is a better operational objective. Unlike a nearest-city rule,
    /// this combines approach geometry, live defenses, staged forces,
    /// occupation pressure, development, and victory-denial value. It is the
    /// campaign analogue of a chess engine's move ordering: forces search the
    /// most forcing and profitable front first rather than the first legal one.
    fn campaign_city_value(
        &self,
        g: &Game,
        pid: usize,
        city: &crate::game::City,
        strategy: GrandStrategy,
    ) -> f64 {
        let core_distance = g
            .player_city_ids(pid)
            .into_iter()
            .map(|mine| g.wdist(g.cities[&mine].pos, city.pos))
            .min()
            .unwrap_or(40);
        let military_units = g
            .player_unit_ids(pid)
            .into_iter()
            .filter(|unit| g.rules.units[g.units[unit].kind.as_str()].class == "military")
            .collect::<Vec<_>>();
        let city_is_coastal = g.nbrs(city.pos).into_iter().any(|position| {
            g.map
                .get(position)
                .is_some_and(|tile| g.rules.is_water(tile))
        });
        let military_distance = military_units
            .iter()
            .filter(|unit| {
                let domain = g.rules.units[g.units[unit].kind.as_str()].domain.as_deref();
                domain != Some("sea") || city_is_coastal
            })
            .map(|unit| g.wdist(g.units[unit].pos, city.pos))
            .min()
            .unwrap_or(core_distance);
        let has_land_force = military_units.iter().any(|unit| {
            !matches!(
                g.rules.units[g.units[unit].kind.as_str()].domain.as_deref(),
                Some("sea" | "air")
            )
        });
        let has_naval_force = military_units.iter().any(|unit| {
            g.rules.units[g.units[unit].kind.as_str()].domain.as_deref() == Some("sea")
        });
        // With no army yet, value prospective land staging rather than
        // declaring every objective sealed. Once forces exist, only count
        // adjacent tiles that the relevant land or naval arm can exploit.
        let plan_land_approach = has_land_force || !has_naval_force;
        let approaches = g
            .nbrs(city.pos)
            .into_iter()
            .filter(|position| {
                g.map.get(*position).is_some_and(|tile| {
                    g.rules.is_passable(tile)
                        && if g.rules.is_water(tile) {
                            has_naval_force
                        } else {
                            plan_land_approach
                        }
                })
            })
            .count();
        let friendly_local: f64 = g
            .units
            .values()
            .filter(|unit| unit.owner == pid && g.wdist(unit.pos, city.pos) <= 7)
            .filter(|unit| g.rules.units[unit.kind.as_str()].class == "military")
            .map(|unit| crate::game::effective_strength(g.unit_strength(unit, true), unit.hp))
            .sum();
        let hostile_local: f64 = g
            .units
            .values()
            .filter(|unit| unit.owner == city.owner && g.wdist(unit.pos, city.pos) <= 7)
            .filter(|unit| g.rules.units[unit.kind.as_str()].class == "military")
            .map(|unit| crate::game::effective_strength(g.unit_strength(unit, true), unit.hp))
            .sum();

        let friendly_pressure: f64 = g
            .cities
            .values()
            .filter(|source| source.owner == pid)
            .filter_map(|source| {
                let distance = g.wdist(source.pos, city.pos);
                (distance <= 9).then_some(source.pop.max(1) as f64 * (10 - distance) as f64)
            })
            .sum();
        let hostile_pressure: f64 = g
            .cities
            .values()
            .filter(|source| source.owner == city.owner && source.id != city.id)
            .filter_map(|source| {
                let distance = g.wdist(source.pos, city.pos);
                (distance <= 9).then_some(source.pop.max(1) as f64 * (10 - distance) as f64)
            })
            .sum();
        let occupation_risk = (hostile_pressure - friendly_pressure).max(0.0)
            * if strategy == GrandStrategy::Conquest {
                0.7
            } else {
                1.2
            };

        let defenses = g.city_strength(city.id) * 1.8
            + city.hp.max(0) as f64 * 0.12
            + city.wall_hp.max(0) as f64 * 0.16;
        let local_balance = (hostile_local - friendly_local).clamp(-250.0, 250.0) * 0.45;
        let approach_cost = (6usize.saturating_sub(approaches)) as f64 * 11.0;
        let development = city.pop.max(1) as f64 * 7.0
            + city.buildings.len() as f64 * 5.0
            + city.districts.len() as f64 * 10.0
            + city.wonders.len() as f64 * 24.0;
        let capital_value = if city.is_capital {
            if strategy == GrandStrategy::Conquest {
                180.0
            } else {
                75.0
            }
        } else {
            0.0
        };
        let science_denial = if city.districts.contains_key("spaceport")
            && self.rival_victory_pressure(g, city.owner).strategy == GrandStrategy::Science
        {
            110.0
        } else {
            0.0
        };
        let recapture_value = if city.original_owner == pid {
            135.0
        } else {
            0.0
        };
        let liberation_value = if city.original_owner != city.owner
            && city.original_owner != pid
            && g.players
                .get(city.original_owner)
                .is_some_and(|founder| !founder.is_barbarian)
            && strategy == GrandStrategy::Diplomacy
        {
            120.0
        } else {
            0.0
        };

        core_distance as f64 * 7.0
            + military_distance as f64 * 5.0
            + defenses
            + local_balance
            + approach_cost
            + occupation_risk
            - development
            - capital_value
            - science_denial
            - recapture_value
            - liberation_value
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
            let cached = self.settler_targets.get(&uid).copied().filter(|target| {
                let Some(tile) = g.map.get(*target) else {
                    return false;
                };
                !g.rules.is_water(tile)
                    && g.rules.is_passable(tile)
                    && !g.cities.values().any(|city| g.wdist(city.pos, *target) < 4)
                    && tile
                        .owner_city
                        .is_none_or(|cid| g.cities[&cid].owner == pid)
                    && (*target == current || g.route_step(uid, *target, 0).is_some())
            });
            if cached.is_none() {
                self.settler_targets.remove(&uid);
            }
            let target = cached.or_else(|| {
                let current_value = self.settle_value(g, pid, current);
                let local = self.best_reachable_settle_site(g, pid, uid, 2);
                let target = if g.can_found_city(uid) {
                    Some(
                        local
                            .filter(|(_, value)| *value > current_value + 3.0)
                            .map(|(pos, _)| pos)
                            .unwrap_or(current),
                    )
                } else {
                    local
                        .or_else(|| {
                            self.best_reachable_settle_site(g, pid, uid, g.map.width + g.map.height)
                        })
                        .or_else(|| {
                            self.base.best_reachable_settle_site(
                                g,
                                pid,
                                uid,
                                g.map.width + g.map.height,
                            )
                        })
                        .map(|(pos, _)| pos)
                };
                if let Some(target) = target {
                    self.settler_targets.insert(uid, target);
                }
                target
            });
            if target == Some(current) && g.can_found_city(uid) {
                self.settler_targets.remove(&uid);
                return g.apply(pid, &Action::FoundCity { unit: uid }).is_ok();
            }
            if let Some(target) = target {
                let moved = self.base.step_toward(g, pid, uid, target);
                if !moved {
                    self.settler_targets.remove(&uid);
                }
                return moved;
            }
            return false;
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
            let global = self.best_reachable_settle_site(
                g,
                pid,
                uid,
                g.map.width + g.map.height,
            );
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

    /// Return only improvements that genuinely upgrade the tile. The game
    /// permits builders to replace an existing improvement, so comparing
    /// candidates in isolation made late-game builders oscillate between a
    /// high-value resort and a lower-value farm on successive turns.
    fn worthwhile_improvements(
        &self,
        g: &Game,
        pid: usize,
        pos: Pos,
        strategy: GrandStrategy,
    ) -> Vec<String> {
        let current_value = g.map.tiles[&pos]
            .improvement
            .as_deref()
            .map(|improvement| self.improvement_value(g, pos, improvement, strategy))
            .unwrap_or(0.0);
        let mut choices: Vec<String> = g
            .valid_improvements(pid, pos)
            .into_iter()
            .filter(|improvement| {
                g.rules.improvements[improvement].builder_buildable
                    && self.improvement_value(g, pos, improvement, strategy) > current_value + 0.5
            })
            .collect();
        choices.sort_by(|a, b| {
            self.improvement_value(g, pos, b, strategy)
                .partial_cmp(&self.improvement_value(g, pos, a, strategy))
                .unwrap()
                .then(a.cmp(b))
        });
        choices
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
        let here = self.worthwhile_improvements(g, pid, current, strategy);
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
                && !self
                    .worthwhile_improvements(g, pid, *pos, strategy)
                    .is_empty()
        });
        let target = current_target.or_else(|| {
            let mut best: Option<(f64, Pos)> = None;
            for cid in g.player_city_ids(pid) {
                for pos in &g.cities[&cid].owned_tiles {
                    if reserved.contains(pos) {
                        continue;
                    }
                    for improvement in self.worthwhile_improvements(g, pid, *pos, strategy) {
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
            if let Some((_, city)) = self.best_trade_route_destination(g, pid, origin, strategy) {
                return g
                    .apply(pid, &Action::TradeRoute { unit: uid, city })
                    .is_ok();
            }
        }
        let Some((_, origin)) = self.best_trade_route_origin(g, pid, current, strategy) else {
            return false;
        };
        self.base.step_toward(g, pid, uid, g.cities[&origin].pos)
    }

    fn best_trade_route_destination(
        &self,
        g: &Game,
        pid: usize,
        origin: u32,
        strategy: GrandStrategy,
    ) -> Option<(f64, u32)> {
        g.cities
            .values()
            .filter(|city| g.can_establish_trade_route(pid, origin, city.id))
            .map(|city| {
                (
                    self.trade_route_destination_value(g, pid, city, strategy),
                    city.id,
                )
            })
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| right.1.cmp(&left.1))
            })
    }

    /// Count destinations, not origin/destination pairs: final-patch Civ VI
    /// permits only one active route to a destination per empire.
    fn trade_route_opportunity_count(&self, g: &Game, pid: usize) -> usize {
        let origins = g.player_city_ids(pid);
        g.cities
            .values()
            .filter(|destination| {
                origins
                    .iter()
                    .any(|origin| g.can_establish_trade_route(pid, *origin, destination.id))
            })
            .count()
    }

    /// Best city in which an idle or newly completed Trader can begin a
    /// legal route. Travel time prevents a slightly richer distant route from
    /// delaying economic output indefinitely.
    fn best_trade_route_origin(
        &self,
        g: &Game,
        pid: usize,
        from: Pos,
        strategy: GrandStrategy,
    ) -> Option<(f64, u32)> {
        g.player_city_ids(pid)
            .into_iter()
            .filter_map(|origin| {
                self.best_trade_route_destination(g, pid, origin, strategy)
                    .map(|(value, _)| {
                        (
                            value - g.wdist(from, g.cities[&origin].pos) as f64 * 1.5,
                            origin,
                        )
                    })
            })
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| right.1.cmp(&left.1))
            })
    }

    fn trade_route_destination_value(
        &self,
        g: &Game,
        pid: usize,
        city: &crate::game::City,
        strategy: GrandStrategy,
    ) -> f64 {
        let mut value = self.yield_value(g.trade_route_yields(pid, city.id), strategy);
        if let Some(alliance) = g.alliance_with(pid, city.owner) {
            let mut yields = Yields::default();
            match alliance.kind.as_str() {
                "research" => yields.science = 2.0,
                "cultural" => yields.culture = 2.0,
                "economic" => yields.gold = 4.0,
                "religious" => yields.faith = 2.0,
                _ => {}
            }
            value += self.yield_value(yields, strategy);
            let already_connected = g.routes.iter().any(|route| {
                route.owner == pid
                    && route.ends > g.turn
                    && g.cities
                        .get(&route.dest)
                        .is_some_and(|destination| destination.owner == city.owner)
            });
            if !already_connected {
                // The first route in each direction accelerates alliance XP;
                // later duplicate routes should compete on their yields.
                value += 45.0;
            }
            if alliance.kind == "cultural" && alliance.level >= 2 {
                value += 18.0;
            }
        }
        let objective = self
            .victory_target
            .map(VictoryTarget::strategy)
            .unwrap_or(strategy);
        // One route unlocks the entire empire's +25% Tourism pressure against
        // that civilization (+75% with Online Communities). Duplicate routes
        // do not stack, so Culture agents connect every rival before
        // optimizing the route's ordinary yields.
        if objective == GrandStrategy::Culture
            && city.owner != pid
            && !g.has_tourism_trade_route(pid, city.owner)
        {
            let modifier = 25.0 + g.policy_effect(pid, "trade_partner_tourism_pct");
            value += 12.0 + g.tourism_per_turn(pid).min(400.0) * modifier / 100.0;
        }
        value
    }

    fn advanced_missionary_step(
        &self,
        g: &mut Game,
        pid: usize,
        uid: u32,
        offensive: bool,
    ) -> bool {
        let Some(religion) = g.players[pid].religion.clone() else {
            return false;
        };
        let current = g.units[&uid].pos;
        let mut targets: Vec<(i32, std::cmp::Reverse<u32>, Pos)> = g
            .cities
            .values()
            .filter(|city| {
                Self::city_needs_religious_support(g, pid, city, &religion)
                    || (offensive
                        && city.owner != pid
                        && !g.is_at_war(pid, city.owner)
                        && g.city_religion(city) != Some(religion.as_str()))
            })
            .map(|city| {
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
                (score, std::cmp::Reverse(city.id), city.pos)
            })
            .collect();
        targets.sort_by(|left, right| right.cmp(left));
        for (_, _, target) in targets {
            if g.wdist(current, target) <= 1 {
                return g.apply(pid, &Action::Spread { unit: uid }).is_ok();
            }
            if self.base.step_toward_range(g, pid, uid, target, 1) {
                return true;
            }
        }
        false
    }

    fn advanced_religious_step(&self, g: &mut Game, pid: usize, uid: u32, offensive: bool) -> bool {
        let unit = g.units[&uid].clone();
        let religion = unit
            .religion
            .clone()
            .or_else(|| g.players[pid].religion.clone());
        let legal = g.legal_actions(pid);

        // A lost core city is more urgent than another enhancer or worship
        // belief. Preserve this Apostle until it reaches the Holy City, then
        // launch the inquisition before the rival can close out the match.
        let needs_inquisition = unit.kind == "apostle"
            && unit.religion == g.players[pid].religion
            && g.players[pid]
                .counters
                .get("inquisition")
                .copied()
                .unwrap_or(0)
                == 0
            && religion.as_ref().is_some_and(|faith| {
                g.player_city_ids(pid)
                    .iter()
                    .any(|city| g.city_religion(&g.cities[city]) != Some(faith.as_str()))
            });
        if needs_inquisition {
            if let Some(action) = legal
                .iter()
                .find(|action| matches!(action, Action::LaunchInquisition { unit } if *unit == uid))
                .cloned()
            {
                return g.apply(pid, &action).is_ok();
            }
            if let Some(target) = g.players[pid]
                .holy_city
                .and_then(|city| g.cities.get(&city).map(|city| city.pos))
            {
                return self.base.step_toward(g, pid, uid, target);
            }
        }

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

        if g.rules.units[unit.kind.as_str()].religious_spread > 0.0 && unit.charges > 0 {
            return self.advanced_missionary_step(g, pid, uid, offensive);
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

    /// Reserve one reachable land combat unit for each ungarrisoned occupied
    /// city, weakest-loyalty first. The assignment is recomputed from the
    /// current position before every unit acts, so a completed garrison is
    /// immediately removed from the demand set and cannot attract a second
    /// unit. This is the strategic counterpart to Gathering Storm's -5
    /// occupation Loyalty penalty.
    fn occupation_garrison_target(&self, g: &Game, pid: usize, uid: u32) -> Option<Pos> {
        let mut cities: Vec<_> = g
            .cities
            .values()
            .filter(|city| city.owner == pid)
            .filter(|city| {
                city.occupied_from
                    .is_some_and(|former| g.players.get(former).is_some_and(|p| p.alive))
            })
            .filter(|city| {
                !g.units_at(city.pos).into_iter().any(|unit| {
                    g.units[&unit].owner == pid
                        && g.rules.units[g.units[&unit].kind.as_str()].class == "military"
                })
            })
            .collect();
        cities.sort_by(|left, right| {
            left.loyalty
                .total_cmp(&right.loyalty)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut available: BTreeSet<u32> = g
            .player_unit_ids(pid)
            .into_iter()
            .filter(|unit| {
                let spec = &g.rules.units[g.units[unit].kind.as_str()];
                spec.class == "military"
                    && !matches!(spec.domain.as_deref(), Some("sea" | "air"))
                    && g.units[unit].linked_to.is_none()
            })
            .collect();
        for city in cities {
            let selected = available
                .iter()
                .filter(|unit| {
                    g.units[unit].pos == city.pos || g.route_step(**unit, city.pos, 0).is_some()
                })
                .min_by(|left, right| {
                    let rank = |unit: u32| {
                        (
                            g.wdist(g.units[&unit].pos, city.pos),
                            g.unit_strength(&g.units[&unit], true) as i32,
                            unit,
                        )
                    };
                    rank(**left).cmp(&rank(**right))
                })
                .copied();
            if let Some(selected) = selected {
                available.remove(&selected);
                if selected == uid {
                    return Some(city.pos);
                }
            }
        }
        None
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
                .or_else(|| self.base.nearest_enemy_from(g, pid, anchor, enemies))
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
            if spec.class != "military" || (!spec.is_melee_capable() && !spec.has_ranged_attack()) {
                continue;
            }
            let radius = if spec.has_ranged_attack() {
                g.unit_attack_range(*uid).max(1)
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
                    if spec.class != "military"
                        || (!spec.is_melee_capable() && !spec.has_ranged_attack())
                    {
                        continue;
                    }
                    let distance = g.wdist(unit.pos, target);
                    let mut exchange = f64::NEG_INFINITY;
                    if spec.has_ranged_attack() && distance <= g.unit_attack_range(*uid) {
                        exchange = exchange.max(self.base.exchange_score(g, *uid, target, true));
                    }
                    if spec.is_melee_capable() && distance == 1 {
                        exchange = exchange.max(self.base.exchange_score(g, *uid, target, false));
                    }
                    if exchange.is_finite() {
                        score += exchange.max(-20.0);
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
            .sum::<f64>()
            + g.city_at(objective)
                .filter(|city| enemies.contains(&g.cities[city].owner))
                .map(|city| g.city_strength(city))
                .unwrap_or(0.0)
            + g.encampment_at(objective)
                .filter(|city| enemies.contains(&g.cities[city].owner))
                .map(|city| g.encampment_strength(city))
                .unwrap_or(0.0);
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
                let spec = &g.rules.units[g.units[uid].kind.as_str()];
                // Aircraft receive missions from the air-operations evaluator.
                // Counting them as land units makes a thin ground army appear
                // assembled and locally superior even though aircraft cannot
                // occupy its front, screen siege, or capture the objective.
                let field_unit = matches!(spec.class.as_str(), "military" | "support")
                    && spec.domain.as_deref() != Some("air");
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
            let forcing_focus = focus_target.is_some_and(|target| {
                let low_hp_unit = g
                    .units_at(target)
                    .into_iter()
                    .any(|unit| enemies.contains(&g.units[&unit].owner) && g.units[&unit].hp <= 35);
                let capturable_city = g.city_at(target).is_some_and(|city| {
                    enemies.contains(&g.cities[&city].owner)
                        && g.cities[&city].hp <= 40
                        && g.cities[&city].wall_hp <= 0
                        && units.iter().any(|unit| {
                            g.rules.units[g.units[unit].kind.as_str()].is_melee_capable()
                                && g.wdist(g.units[unit].pos, target) <= 1
                        })
                });
                low_hp_unit || capturable_city
            });
            let posture = if average_hp <= self.base.w.withdraw_hp + 10.0 {
                ForcePosture::Recover
            } else if (focus_target.is_some()
                && (local_strength_ratio >= 0.72
                    || plan.threatened_city.is_some()
                    || forcing_focus))
                || (units.iter().any(|uid| {
                    g.units.values().any(|enemy| {
                        enemies.contains(&enemy.owner)
                            && g.wdist(g.units[uid].pos, enemy.pos) <= 2
                            && (local_strength_ratio >= 0.72
                                || plan.threatened_city.is_some()
                                || enemy.hp <= 35)
                    })
                }))
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
        decline_settlers: bool,
    ) -> bool {
        let unit = &g.units[&uid];
        let upos = unit.pos;
        let role = Self::force_role(g, uid);
        let spec = &g.rules.units[unit.kind.as_str()];
        let target = match group.posture {
            ForcePosture::Muster | ForcePosture::Hold | ForcePosture::Recover => group.anchor,
            ForcePosture::Engage => group.focus_target.unwrap_or(group.objective),
            ForcePosture::Advance => group.objective,
        };
        let preferred_depth = match role {
            ForceRole::Recon => spec.range.max(2),
            ForceRole::Vanguard | ForceRole::Mobile => 1,
            ForceRole::Ranged | ForceRole::Siege => g.unit_attack_range(uid),
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
                if enemy_spec.class != "military"
                    || (!enemy_spec.is_melee_capable() && !enemy_spec.has_ranged_attack())
                {
                    continue;
                }
                let radius = if enemy_spec.has_ranged_attack() {
                    g.unit_attack_range(enemy.id).max(1)
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
        for pos in g.nbrs(upos).into_iter().filter(|pos| {
            g.can_move(uid, *pos)
                && !(decline_settlers
                    && g.units_at(*pos).iter().any(|other| {
                        let other = &g.units[other];
                        other.owner != pid
                            && g.is_at_war(pid, other.owner)
                            && other.kind == "settler"
                    }))
        }) {
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
                .filter(|pos| {
                    !(decline_settlers
                        && g.units_at(*pos).iter().any(|other| {
                            let other = &g.units[other];
                            other.owner != pid
                                && g.is_at_war(pid, other.owner)
                                && other.kind == "settler"
                        }))
                })
            {
                if self.base.move_beats_holding(g, uid, score(g, pos), stay) {
                    return g.apply(pid, &Action::Move { unit: uid, to: pos }).is_ok();
                }
            }
        }
        self.base.fortify_or_stop(g, pid, uid)
    }

    /// Candidate attacks on one tactical victim. `Game::legal_actions` also
    /// enumerates production, diplomacy, spies, purchases, every movement,
    /// and every other unit target; calling it at each reply-search node made
    /// late-game turns spend most of their time proving irrelevant actions
    /// irrelevant. Build the small target-specific superset here and let
    /// `Game::apply` remain the authoritative legality check.
    fn forcing_attacks_to(
        position: &Game,
        enemy: usize,
        victim_pos: Pos,
        only_unit: Option<u32>,
    ) -> Vec<Action> {
        let mut replies = Vec::new();
        for unit in position.units.values().filter(|unit| {
            unit.owner == enemy && only_unit.is_none_or(|candidate| candidate == unit.id)
        }) {
            if unit.moves_left <= 0.0 || unit.attacks_left <= 0 {
                continue;
            }
            let spec = &position.rules.units[unit.kind.as_str()];
            let distance = position.wdist(unit.pos, victim_pos);
            if spec.domain.as_deref() == Some("air") {
                if distance <= position.unit_attack_range(unit.id) {
                    replies.push(Action::AirStrike {
                        unit: unit.id,
                        target: victim_pos,
                    });
                }
                continue;
            }
            if spec.class != "military" || position.is_embarked(unit) {
                continue;
            }
            if spec.has_ranged_attack() && distance <= position.unit_attack_range(unit.id) {
                replies.push(Action::Ranged {
                    unit: unit.id,
                    target: victim_pos,
                });
            }
            if spec.is_melee_capable() && distance == 1 {
                replies.push(Action::Attack {
                    unit: unit.id,
                    target: victim_pos,
                });
            }
        }
        if only_unit.is_none() {
            for city in position.cities.values().filter(|city| city.owner == enemy) {
                replies.push(Action::CityStrike {
                    city: city.id,
                    target: victim_pos,
                });
                replies.push(Action::EncampmentStrike {
                    city: city.id,
                    target: victim_pos,
                });
            }
        }
        replies
    }

    fn forcing_reply_line(&self, position: &Game, enemy: usize, victim: u32, depth: usize) -> f64 {
        if depth == 0 || !position.units.contains_key(&victim) {
            return 0.0;
        }
        let victim_hp = position.units[&victim].hp;
        let victim_pos = position.units[&victim].pos;
        let replies = Self::forcing_attacks_to(position, enemy, victim_pos, None);

        let mut reply_branches = Vec::new();
        let mut direct_attackers = BTreeSet::new();
        for reply in replies {
            let reply_unit = match &reply {
                Action::Attack { unit, .. }
                | Action::Ranged { unit, .. }
                | Action::AirStrike { unit, .. } => Some(*unit),
                _ => None,
            };
            direct_attackers.extend(reply_unit);
            let reply_hp =
                reply_unit.and_then(|unit| position.units.get(&unit).map(|candidate| candidate.hp));
            let mut branch = position.clone();
            if branch.apply(enemy, &reply).is_err() {
                continue;
            }
            reply_branches.push((format!("{reply:?}"), branch, reply_unit, reply_hp));
        }

        // A Civ unit can normally move and attack in the same turn. Search
        // only one-step forcing approaches whose resulting position already
        // has a legal attack on the victim. This is the tactical analogue of
        // a check extension: it closes the horizon gap around an exposed
        // capture without admitting every quiet movement into quiescence.
        let mobile_attackers: Vec<u32> = position
            .units
            .values()
            .filter(|unit| unit.owner == enemy && !direct_attackers.contains(&unit.id))
            .filter(|unit| {
                let spec = &position.rules.units[unit.kind.as_str()];
                spec.class == "military"
                    && spec.domain.as_deref() != Some("air")
                    && position.wdist(unit.pos, victim_pos)
                        <= position.unit_attack_range(unit.id) + 2
            })
            .map(|unit| unit.id)
            .collect();
        for attacker in mobile_attackers {
            let reply_hp = position.units[&attacker].hp;
            for to in position
                .nbrs(position.units[&attacker].pos)
                .into_iter()
                .filter(|to| position.can_move(attacker, *to))
            {
                let movement = Action::Move { unit: attacker, to };
                let mut moved = position.clone();
                if moved.apply(enemy, &movement).is_err() {
                    continue;
                }
                let followups =
                    Self::forcing_attacks_to(&moved, enemy, victim_pos, Some(attacker));
                for followup in followups {
                    let mut branch = moved.clone();
                    if branch.apply(enemy, &followup).is_err() {
                        continue;
                    }
                    reply_branches.push((
                        format!("{movement:?} -> {followup:?}"),
                        branch,
                        Some(attacker),
                        Some(reply_hp),
                    ));
                }
            }
        }

        let mut ordered = Vec::new();
        for (label, branch, reply_unit, reply_hp) in reply_branches {
            let loss = branch
                .units
                .get(&victim)
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
            ordered.push(((loss - 0.35 * counter_loss).max(0.0), label, branch));
        }

        // Chess-style move ordering keeps the extension bounded: examine all
        // forcing replies at the frontier, but only extend the four strongest
        // captures/checks into another focus-fire action.
        ordered.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        ordered
            .into_iter()
            .take(4)
            .map(|(immediate, _, branch)| {
                immediate + self.forcing_reply_line(&branch, enemy, victim, depth - 1)
            })
            .fold(0.0_f64, f64::max)
    }

    /// Make a candidate battlefield action on a clone and value the exact
    /// result before extending opponent replies. This is the principal-search
    /// half of the tactical evaluator: static exchange remains useful for
    /// cheap move ordering, while the final decision sees the seeded damage
    /// roll, kills, attacker survival, wall damage, district pillage, and an
    /// actual city transfer.
    fn tactical_attack_value(
        &self,
        g: &Game,
        pid: usize,
        uid: u32,
        action: &Action,
        plan: &StrategicPlan,
    ) -> f64 {
        let target = match action {
            Action::Attack { unit, target }
            | Action::Ranged { unit, target }
            | Action::PriorityTarget { unit, target }
                if *unit == uid =>
            {
                *target
            }
            _ => return f64::NEG_INFINITY,
        };
        let priority_target = matches!(action, Action::PriorityTarget { .. });
        let attacker = &g.units[&uid];
        let attacker_spec = &g.rules.units[attacker.kind.as_str()];
        let defenders: Vec<(u32, i32, f64, f64, bool, bool)> = g
            .units_at(target)
            .into_iter()
            .filter_map(|unit| {
                let defender = &g.units[&unit];
                let spec = &g.rules.units[defender.kind.as_str()];
                (defender.owner != pid
                    && g.is_at_war(pid, defender.owner)
                    && if priority_target {
                        spec.class == "support"
                    } else {
                        spec.class == "military"
                    })
                .then_some((
                    unit,
                    defender.hp,
                    g.unit_strength(defender, true),
                    spec.cost,
                    spec.siege,
                    spec.is_melee_capable(),
                ))
            })
            .collect();
        let target_city = (!priority_target)
            .then(|| g.city_at(target))
            .flatten()
            .filter(|city| g.cities[city].owner != pid && g.is_at_war(pid, g.cities[city].owner));
        let target_encampment = target_city
            .is_none()
            .then(|| g.encampment_at(target))
            .flatten();
        let mut after = g.clone();
        if after.apply(pid, action).is_err() {
            return f64::NEG_INFINITY;
        }

        let attacker_loss = match after.units.get(&uid) {
            Some(survivor) => {
                (attacker.hp - survivor.hp).max(0) as f64 * (1.25 + attacker_spec.cost / 800.0)
            }
            None => 230.0 + attacker_spec.cost * 0.65,
        };
        let mut value = -attacker_loss;
        for (unit, hp, strength, cost, siege, captures) in defenders {
            value += match after.units.get(&unit) {
                None => {
                    190.0
                        + cost * 0.45
                        + strength * 1.8
                        + if siege { 65.0 } else { 0.0 }
                        + if captures { 30.0 } else { 0.0 }
                }
                Some(survivor) => {
                    (hp - survivor.hp).max(0) as f64 * (1.0 + strength / 100.0)
                        + if siege { 18.0 } else { 0.0 }
                        + if captures { 6.0 } else { 0.0 }
                }
            };
        }
        if let Some(city) = target_city {
            let before = &g.cities[&city];
            let captured = after
                .cities
                .get(&city)
                .is_some_and(|city| city.owner == pid);
            if captured {
                value += 520.0
                    + before.pop.max(1) as f64 * 14.0
                    + before.districts.len() as f64 * 24.0
                    + before.wonders.len() as f64 * 45.0
                    + if before.is_capital { 180.0 } else { 0.0 }
                    + if plan.target_city == Some(city) {
                        100.0
                    } else {
                        0.0
                    };
            } else if let Some(after_city) = after.cities.get(&city) {
                let wall_damage = (before.wall_hp - after_city.wall_hp).max(0) as f64;
                let city_damage = (before.hp - after_city.hp).max(0) as f64;
                let progress = wall_damage * 1.35 + city_damage;
                value += progress
                    + if progress > 0.0 && plan.target_city == Some(city) {
                        35.0
                    } else {
                        0.0
                    };
            }
        } else if let Some(city) = target_encampment {
            let before = &g.cities[&city];
            let after_city = &after.cities[&city];
            value += (before.encampment_wall_hp - after_city.encampment_wall_hp).max(0) as f64
                * 1.35
                + (before.encampment_hp - after_city.encampment_hp).max(0) as f64;
            if !before.encampment_pillaged && after_city.encampment_pillaged {
                value += 180.0;
            }
        }
        value
    }

    /// Bounded quiescence-style reply search for a proposed attack. The
    /// ordinary exchange evaluator accounts for the target's counter-damage;
    /// this extension makes the move on a cloned position, refreshes only the
    /// enemy's forcing combat actions, and prices a two-action focus-fire
    /// sequence. It catches poisoned captures and coordinated ranged kills
    /// without turning every unit decision into an unbounded turn search.
    fn forcing_reply_penalty(&self, g: &Game, pid: usize, uid: u32, action: &Action) -> f64 {
        let mut after = g.clone();
        if after.apply(pid, action).is_err() {
            return 1_000.0;
        }
        if !after.units.contains_key(&uid) {
            return 135.0;
        }
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

            worst_reply = worst_reply.max(self.forcing_reply_line(&reply_position, enemy, uid, 2));
        }
        worst_reply
    }

    /// Evaluate an air strike by making it on a cloned position. This captures
    /// the seeded combat roll, interception damage, wall-vs-city damage, and
    /// kills in one bounded result score instead of ordering targets by their
    /// pre-combat HP alone.
    fn air_strike_value(
        &self,
        g: &Game,
        pid: usize,
        uid: u32,
        target: Pos,
        plan: &StrategicPlan,
    ) -> f64 {
        let attacker = &g.units[&uid];
        let attacker_spec = &g.rules.units[attacker.kind.as_str()];
        let target_city = g
            .city_at(target)
            .filter(|city| g.cities[city].owner != pid && g.is_at_war(pid, g.cities[city].owner));
        let target_encampment = target_city
            .is_none()
            .then(|| g.encampment_at(target))
            .flatten();
        let target_unit = (target_city.is_none() && target_encampment.is_none())
            .then(|| {
                g.units_at(target).into_iter().find(|other| {
                    let defender = &g.units[other];
                    defender.owner != pid
                        && g.is_at_war(pid, defender.owner)
                        && g.rules.units[defender.kind.as_str()].class == "military"
                })
            })
            .flatten();
        let action = Action::AirStrike { unit: uid, target };
        let mut after = g.clone();
        if after.apply(pid, &action).is_err() {
            return f64::NEG_INFINITY;
        }

        let attacker_loss = match after.units.get(&uid) {
            Some(survivor) => {
                (attacker.hp - survivor.hp).max(0) as f64 * (1.4 + attacker_spec.cost / 700.0)
            }
            None => 260.0 + attacker_spec.cost * 0.7,
        };
        let mut value = -attacker_loss;
        if let Some(unit) = target_unit {
            let defender = &g.units[&unit];
            let spec = &g.rules.units[defender.kind.as_str()];
            let role_value = if spec.siege { 70.0 } else { 0.0 }
                + if spec.is_melee_capable() { 30.0 } else { 0.0 }
                + if spec.domain.as_deref() == Some("air") {
                    85.0
                } else {
                    0.0
                };
            value += match after.units.get(&unit) {
                None => {
                    190.0 + spec.cost * 0.45 + g.unit_strength(defender, true) * 1.8 + role_value
                }
                Some(survivor) => {
                    (defender.hp - survivor.hp).max(0) as f64
                        * (1.0 + g.unit_strength(defender, true) / 100.0)
                        + role_value * 0.25
                }
            };
        } else if let Some(city) = target_city {
            let before = &g.cities[&city];
            let after_city = &after.cities[&city];
            let wall_damage = (before.wall_hp - after_city.wall_hp).max(0) as f64;
            let city_damage = (before.hp - after_city.hp).max(0) as f64;
            let progress = wall_damage * 1.35 + city_damage;
            value += progress;
            if progress > 0.0 && plan.target_city == Some(city) {
                value += 45.0;
            }
            if before.is_capital && progress > 0.0 && plan.strategy == GrandStrategy::Conquest {
                value += 25.0;
            }
        } else if let Some(city) = target_encampment {
            let before = &g.cities[&city];
            let after_city = &after.cities[&city];
            value += (before.encampment_wall_hp - after_city.encampment_wall_hp).max(0) as f64
                * 1.35
                + (before.encampment_hp - after_city.encampment_hp).max(0) as f64;
        }
        value
    }

    /// Evaluate infrastructure bombing on an exact cloned position. Besides
    /// the pillaged layer, this prices interception losses and the operational
    /// disruption from scattering aircraft out of a disabled air base.
    fn air_pillage_value(&self, g: &Game, pid: usize, uid: u32, target: Pos) -> f64 {
        let attacker = &g.units[&uid];
        let attacker_spec = &g.rules.units[attacker.kind.as_str()];
        let before_tile = &g.map.tiles[&target];
        let before_aircraft: Vec<(u32, f64)> = g
            .units_at(target)
            .into_iter()
            .filter_map(|unit| {
                let candidate = &g.units[&unit];
                (candidate.owner != pid
                    && g.rules.units[candidate.kind.as_str()].domain.as_deref() == Some("air"))
                .then_some((unit, g.rules.units[candidate.kind.as_str()].cost))
            })
            .collect();
        let city_id = before_tile.owner_city;
        let before_pillaged_buildings = city_id
            .and_then(|city| g.cities.get(&city))
            .map(|city| city.pillaged_buildings.clone())
            .unwrap_or_default();
        let action = Action::AirPillage { unit: uid, target };
        let mut after = g.clone();
        if after.apply(pid, &action).is_err() {
            return f64::NEG_INFINITY;
        }

        let attacker_loss = match after.units.get(&uid) {
            Some(survivor) => {
                (attacker.hp - survivor.hp).max(0) as f64 * (1.4 + attacker_spec.cost / 700.0)
            }
            None => 260.0 + attacker_spec.cost * 0.7,
        };
        let mut value = -attacker_loss;
        let after_tile = &after.map.tiles[&target];
        if let Some(improvement) = before_tile.improvement.as_deref() {
            if !before_tile.pillaged && after_tile.pillaged {
                value += match improvement {
                    "airstrip" => 185.0,
                    "oil_well" | "offshore_oil_rig" | "mine" | "quarry" => 115.0,
                    "farm" | "fishing_boats" => 65.0,
                    _ => 85.0,
                };
            }
        } else if let Some(district) = before_tile.district.as_deref() {
            if !before_tile.pillaged && after_tile.pillaged {
                value += match g.district_family(district) {
                    "aerodrome" | "industrial_zone" | "campus" | "spaceport" => 175.0,
                    "commercial_hub" | "harbor" | "holy_site" | "theater_square" => 145.0,
                    _ => 115.0,
                };
            } else if let Some(city) = city_id.and_then(|city| after.cities.get(&city)) {
                value += city
                    .pillaged_buildings
                    .difference(&before_pillaged_buildings)
                    .map(|building| 80.0 + after.rules.buildings[building.as_str()].cost * 0.32)
                    .sum::<f64>();
            }
        }
        for (aircraft, cost) in before_aircraft {
            value += match after.units.get(&aircraft) {
                None => 150.0 + cost * 0.55,
                Some(unit) if unit.pos != target => 55.0 + cost * 0.08,
                _ => 0.0,
            };
        }
        value
    }

    fn priority_target_value(&self, g: &Game, pid: usize, uid: u32, target: Pos) -> f64 {
        let Some(defender_id) = g.priority_support_target_at(pid, target) else {
            return f64::NEG_INFINITY;
        };
        let attacker = &g.units[&uid];
        let attacker_spec = &g.rules.units[attacker.kind.as_str()];
        let defender = &g.units[&defender_id];
        let defender_spec = &g.rules.units[defender.kind.as_str()];
        let mut after = g.clone();
        if after
            .apply(pid, &Action::PriorityTarget { unit: uid, target })
            .is_err()
        {
            return f64::NEG_INFINITY;
        }
        let attacker_loss = match after.units.get(&uid) {
            Some(survivor) => {
                (attacker.hp - survivor.hp).max(0) as f64 * (1.4 + attacker_spec.cost / 700.0)
            }
            None => 260.0 + attacker_spec.cost * 0.7,
        };
        let target_value = match after.units.get(&defender_id) {
            None => 175.0 + defender_spec.cost * 0.55,
            Some(survivor) => {
                (defender.hp - survivor.hp).max(0) as f64 * (1.0 + defender_spec.cost / 500.0)
            }
        };
        target_value
            + if defender_spec.anti_air_strength > 0.0 {
                120.0
            } else if matches!(defender.kind.as_str(), "drone" | "observation_balloon") {
                55.0
            } else if matches!(defender.kind.as_str(), "medic" | "supply_convoy") {
                40.0
            } else {
                0.0
            }
            - attacker_loss
    }

    /// Choose among exact air-strike or air-pillage results, a useful patrol,
    /// and a rebase
    /// that materially improves reach to the active front. Fighters preserve
    /// interception coverage when hostile aircraft threaten the theater;
    /// bombers avoid suicidal missions and reposition when no profitable
    /// strike is available.
    fn advanced_air_action(
        &self,
        g: &Game,
        pid: usize,
        uid: u32,
        plan: &StrategicPlan,
    ) -> Option<Action> {
        let unit = &g.units[&uid];
        let doctrine = BasicAi::unit_doctrine(g, uid);
        let legal = g.legal_doctrine_actions(pid, uid);
        let best_strike = legal
            .iter()
            .filter_map(|action| match action {
                Action::AirStrike { unit, target } if *unit == uid => Some((
                    self.air_strike_value(g, pid, uid, *target, plan),
                    *target,
                    action.clone(),
                )),
                _ => None,
            })
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| right.1.cmp(&left.1))
            });
        let best_pillage = legal
            .iter()
            .filter_map(|action| match action {
                Action::AirPillage { unit, target } if *unit == uid => Some((
                    self.air_pillage_value(g, pid, uid, *target),
                    *target,
                    action.clone(),
                )),
                _ => None,
            })
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| right.1.cmp(&left.1))
            });
        let best_priority = legal
            .iter()
            .filter_map(|action| match action {
                Action::PriorityTarget { unit, target } if *unit == uid => Some((
                    self.priority_target_value(g, pid, uid, *target),
                    *target,
                    action.clone(),
                )),
                _ => None,
            })
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| right.1.cmp(&left.1))
            });
        let best_attack = best_strike
            .clone()
            .into_iter()
            .chain(best_priority.clone())
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| right.1.cmp(&left.1))
            });
        let best_mission =
            best_attack
                .clone()
                .into_iter()
                .chain(best_pillage)
                .max_by(|left, right| {
                    left.0
                        .total_cmp(&right.0)
                        .then_with(|| right.1.cmp(&left.1))
                });

        let objective = match doctrine {
            UnitDoctrine::AirDefense => plan.threatened_city.or(plan.target_city),
            _ => plan.target_city.or(plan.threatened_city),
        }
        .and_then(|city| g.cities.get(&city).map(|city| city.pos))
        .or_else(|| {
            g.units
                .values()
                .filter(|other| other.owner != pid && g.is_at_war(pid, other.owner))
                .min_by_key(|other| (g.wdist(unit.pos, other.pos), other.id))
                .map(|other| other.pos)
        });
        let best_rebase = objective.and_then(|objective| {
            let current_distance = g.wdist(unit.pos, objective);
            legal
                .iter()
                .filter_map(|action| match action {
                    Action::AirRebase { unit, to } if *unit == uid => {
                        let distance = g.wdist(*to, objective);
                        let improvement = current_distance - distance;
                        let reaches = (distance <= g.unit_attack_range(uid)) as i32;
                        Some((
                            improvement as f64 * 18.0 + reaches as f64 * 35.0,
                            *to,
                            action.clone(),
                        ))
                    }
                    _ => None,
                })
                .filter(|(value, _, _)| *value > 0.0)
                .max_by(|left, right| {
                    left.0
                        .total_cmp(&right.0)
                        .then_with(|| right.1.cmp(&left.1))
                })
        });

        if doctrine == UnitDoctrine::AirStrike {
            return best_mission
                .filter(|(value, _, _)| *value > 0.0)
                .map(|(_, _, action)| action)
                .or_else(|| best_rebase.map(|(_, _, action)| action));
        }

        let patrol = legal
            .iter()
            .filter_map(|action| match action {
                Action::AirPatrol {
                    unit: action_unit,
                    to,
                } if *action_unit == uid => {
                    let city_cover = g
                        .cities
                        .values()
                        .filter(|city| city.owner == pid && g.wdist(*to, city.pos) <= 1)
                        .map(|city| {
                            70.0 + city.pop as f64 * 4.0
                                + if Some(city.id) == plan.threatened_city {
                                    90.0
                                } else {
                                    0.0
                                }
                        })
                        .sum::<f64>();
                    let force_cover = g
                        .units
                        .values()
                        .filter(|other| {
                            other.owner == pid
                                && other.id != uid
                                && g.wdist(*to, other.pos) <= 1
                                && g.rules.units[other.kind.as_str()].class == "military"
                        })
                        .map(|other| g.rules.units[other.kind.as_str()].cost * 0.035)
                        .sum::<f64>();
                    let objective_distance = objective.map_or(0, |pos| g.wdist(*to, pos));
                    let existing = (unit.air_patrol_pos == Some(*to)) as i32 as f64 * 8.0;
                    Some((
                        city_cover + force_cover + existing - objective_distance as f64 * 2.0,
                        *to,
                        action.clone(),
                    ))
                }
                _ => None,
            })
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| right.1.cmp(&left.1))
            })
            .map(|(_, _, action)| action);
        let hostile_air_threat = g
            .units
            .values()
            .filter(|other| {
                other.owner != pid
                    && g.is_at_war(pid, other.owner)
                    && g.rules.units[other.kind.as_str()].domain.as_deref() == Some("air")
            })
            .map(|other| {
                let distance = g.wdist(unit.pos, other.pos);
                if distance <= g.air_rebase_range(uid) {
                    80.0 + g.rules.units[other.kind.as_str()].cost * 0.08
                } else {
                    0.0
                }
            })
            .fold(0.0_f64, f64::max);
        let defended_city = plan.threatened_city.is_some_and(|city| {
            g.cities.get(&city).is_some_and(|city| {
                legal.iter().any(|action| {
                    matches!(action, Action::AirPatrol { unit, to }
                        if *unit == uid && g.wdist(*to, city.pos) <= 1)
                })
            })
        });
        let patrol_value = hostile_air_threat
            + if defended_city { 55.0 } else { 0.0 }
            + if g
                .players
                .iter()
                .any(|other| other.id != pid && g.is_at_war(pid, other.id))
            {
                16.0
            } else {
                5.0
            };
        if let Some((value, _, action)) = best_attack {
            if value > patrol_value {
                return Some(action);
            }
        }
        if let Some((value, _, action)) = best_rebase {
            if value > patrol_value && hostile_air_threat <= 0.0 {
                return Some(action);
            }
        }
        patrol
    }

    /// Condemning a foreign Missionary or Apostle destroys it and pushes our
    /// own Pressure back — the standing military answer to a religious
    /// offensive. Previously this only fired when an enemy religious unit
    /// happened to already share our tile, which almost never happens, so
    /// the counter was effectively dead. Now a military unit will step onto
    /// an adjacent one and condemn it.
    fn condemn_step(&mut self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let condemnable = |game: &Game, at: Pos| -> Option<u32> {
            game.units_at(at).into_iter().find(|target| {
                let target = &game.units[target];
                target.owner != pid
                    && game.is_at_war(pid, target.owner)
                    && game.rules.units[target.kind.as_str()].class == "religious"
            })
        };
        let here = g.units[&uid].pos;
        if let Some(target_unit) = condemnable(g, here) {
            if g.apply(pid, &Action::CondemnHeretic { unit: uid, target_unit }).is_ok() {
                return true;
            }
        }
        // Only chase intruders around our own territory: a lone unit running
        // down missionaries across the map abandons the campaign.
        let near_home = g
            .cities
            .values()
            .any(|city| city.owner == pid && g.wdist(here, city.pos) <= 6);
        if !near_home {
            return false;
        }
        let mut targets: Vec<Pos> = g
            .nbrs(here)
            .into_iter()
            .filter(|n| condemnable(g, *n).is_some() && g.can_move(uid, *n))
            .collect();
        targets.sort();
        let Some(to) = targets.first().copied() else {
            return false;
        };
        if g.apply(pid, &Action::Move { unit: uid, to }).is_err() {
            return false;
        }
        if let Some(target_unit) = condemnable(g, to) {
            let _ = g.apply(pid, &Action::CondemnHeretic { unit: uid, target_unit });
        }
        true
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
        let decline_settlers = self.counts(g, pid).settlers > 0
            || g.player_city_ids(pid).len() >= plan.desired_cities
            || !self.base.has_practical_settle_site(g, pid);
        let unwanted_settler_adjacent = decline_settlers
            && g.nbrs(unit.pos).into_iter().any(|position| {
                g.units_at(position).iter().any(|other| {
                    let other = &g.units[other];
                    other.owner != pid
                        && g.is_at_war(pid, other.owner)
                        && other.kind == "settler"
                })
            });
        if !unwanted_settler_adjacent
            && self.victory_planning
            && spec.class == "military"
            && self.condemn_step(g, pid, uid)
        {
            return true;
        }
        let holding_threatened_city = plan.threatened_city.is_some_and(|cid| {
            g.cities
                .get(&cid)
                .is_some_and(|city| g.wdist(unit.pos, city.pos) <= 3)
        });
        if !unwanted_settler_adjacent && !holding_threatened_city {
            if let Some(acted) = self.base.healing_step(g, pid, uid) {
                return acted;
            }
        }
        if matches!(doctrine, UnitDoctrine::AirDefense | UnitDoctrine::AirStrike) {
            return self
                .advanced_air_action(g, pid, uid, plan)
                .is_some_and(|action| g.apply(pid, &action).is_ok());
        }
        if let Some(action) = self.base.doctrine_action(g, pid, uid) {
            return g.apply(pid, &action).is_ok();
        }
        if !unwanted_settler_adjacent {
            if let Some(city) = self.occupation_garrison_target(g, pid, uid) {
                if unit.pos != city {
                    return self.base.step_toward(g, pid, uid, city);
                }
                return self.base.fortify_or_stop(g, pid, uid);
            }
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
            if let Some(acted) = self.campaign_staging_step(g, pid, uid, plan) {
                return acted;
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

        let radius = if spec.has_ranged_attack() {
            g.unit_attack_range(uid).max(1)
        } else {
            1
        };
        let mut best: Option<(f64, Pos, Action)> = None;
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
            let distance = g.wdist(unit.pos, pos);
            let mut actions = Vec::with_capacity(2);
            if spec.has_ranged_attack() && distance <= g.unit_attack_range(uid) {
                actions.push(Action::Ranged {
                    unit: uid,
                    target: pos,
                });
            }
            if unit.kind == "spec_ops"
                && distance <= g.unit_attack_range(uid)
                && g.priority_support_target_at(pid, pos).is_some()
            {
                actions.push(Action::PriorityTarget {
                    unit: uid,
                    target: pos,
                });
            }
            if spec.is_melee_capable() && distance == 1 {
                actions.push(Action::Attack {
                    unit: uid,
                    target: pos,
                });
            }
            for action in actions {
                let mut score = self.tactical_attack_value(g, pid, uid, &action, plan)
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
                    score -= self.base.w.local_superiority
                        * (1.0 - orders.local_strength_ratio).max(0.0);
                }
                score -=
                    self.base.w.trade_caution * self.forcing_reply_penalty(g, pid, uid, &action);
                if best
                    .as_ref()
                    .map(|(old, bp, _)| score > *old || (score == *old && pos < *bp))
                    .unwrap_or(true)
                {
                    best = Some((score, pos, action));
                }
            }
        }
        if let Some((score, _, action)) = best {
            let required_margin = if unit.hp < 55 { 12.0 } else { 0.0 };
            if score > required_margin && g.apply(pid, &action).is_ok() {
                return true;
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

        if unwanted_settler_adjacent {
            return self.base.fortify_or_stop(g, pid, uid);
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
                .or_else(|| self.base.nearest_enemy(g, pid, uid, &enemies))
        };
        if let Some(orders) = &group {
            return self.coordinated_tactical_step(
                g,
                pid,
                uid,
                orders,
                &enemies,
                decline_settlers,
            );
        }
        match campaign {
            Some(target) => self
                .base
                .tactical_step(g, pid, uid, target, &enemies, radius),
            // Nothing this unit is willing to fight: explore or garrison
            // rather than shadowing a raider it will never strike.
            None => self.base.peacetime_step(g, pid, uid),
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
                    && g.units[uid].kind != "military_engineer"
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

    fn defensive_strike_value(&self, g: &Game, pid: usize, action: &Action) -> f64 {
        let target = match action {
            Action::CityStrike { target, .. } | Action::EncampmentStrike { target, .. } => *target,
            _ => return f64::NEG_INFINITY,
        };
        let defenders: Vec<(u32, i32, f64, f64, bool, bool)> = g
            .units_at(target)
            .into_iter()
            .filter_map(|unit| {
                let defender = &g.units[&unit];
                let spec = &g.rules.units[defender.kind.as_str()];
                (defender.owner != pid
                    && g.is_at_war(pid, defender.owner)
                    && spec.class == "military")
                    .then_some((
                        unit,
                        defender.hp,
                        g.unit_strength(defender, true),
                        spec.cost,
                        spec.siege,
                        !spec.has_ranged_attack(),
                    ))
            })
            .collect();
        let mut after = g.clone();
        if after.apply(pid, action).is_err() {
            return f64::NEG_INFINITY;
        }
        defenders
            .into_iter()
            .map(
                |(unit, hp, strength, cost, siege, captures)| match after.units.get(&unit) {
                    None => {
                        180.0
                            + cost * 0.45
                            + strength * 2.0
                            + if siege { 70.0 } else { 0.0 }
                            + if captures { 30.0 } else { 0.0 }
                    }
                    Some(defender) => {
                        (hp - defender.hp).max(0) as f64 * (1.0 + strength / 100.0)
                            + if siege { 25.0 } else { 0.0 }
                            + if captures { 8.0 } else { 0.0 }
                    }
                },
            )
            .sum()
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
        let mut best: BTreeMap<u32, (f64, Pos)> = BTreeMap::new();
        for action in g.legal_actions(pid) {
            let Action::EncampmentStrike { city, target } = action else {
                continue;
            };
            let strike = Action::EncampmentStrike { city, target };
            let target_value = self.defensive_strike_value(g, pid, &strike);
            let candidate = (target_value, target);
            if best.get(&city).is_none_or(|old| {
                target_value.total_cmp(&old.0).is_gt()
                    || (target_value.total_cmp(&old.0).is_eq() && target < old.1)
            }) {
                best.insert(city, candidate);
            }
        }
        for (city, (_, target)) in best {
            let _ = g.apply(pid, &Action::EncampmentStrike { city, target });
        }
    }

    /// Fire every available city-center strike, choosing each target from an
    /// exact cloned result. Explicit-victory agents do not run the Basic city
    /// governor after the opening, so this command phase is the authoritative
    /// path for walls (including Victor's extra strike).
    fn advanced_city_strikes(&self, g: &mut Game, pid: usize) {
        loop {
            let candidates: Vec<Action> = g
                .legal_actions(pid)
                .into_iter()
                .filter(|action| matches!(action, Action::CityStrike { .. }))
                .collect();
            let best = candidates.into_iter().max_by(|left, right| {
                self.defensive_strike_value(g, pid, left)
                    .total_cmp(&self.defensive_strike_value(g, pid, right))
                    .then_with(|| format!("{right:?}").cmp(&format!("{left:?}")))
            });
            let Some(action) = best else { break };
            if g.apply(pid, &action).is_err() {
                break;
            }
        }
    }

    fn advanced_command_actions(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        self.advanced_city_strikes(g, pid);
        self.advanced_encampment_strikes(g, pid);
        self.advanced_wmd_strikes(g, pid, plan);
        self.advanced_promotions(g, pid, plan.strategy);
        self.advanced_formations(g, pid);
    }

    /// Nuclear doctrine: a Conquest empire at war spends a stockpiled device
    /// on the hardest enemy city in range — the one whose walls and garrison
    /// would cost the most to break conventionally — and never on a blast
    /// that would touch its own cities or units.
    fn advanced_wmd_strikes(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        if plan.strategy != GrandStrategy::Conquest {
            return;
        }
        let candidates: Vec<(Action, Pos, bool)> = g
            .legal_actions(pid)
            .into_iter()
            .filter_map(|action| match action {
                Action::WmdStrike {
                    target,
                    thermonuclear,
                    ..
                } => Some((action.clone(), target, thermonuclear)),
                _ => None,
            })
            .collect();
        let mut best: Option<(f64, Action)> = None;
        for (action, target, thermonuclear) in candidates {
            let radius = g.rules.wmds[if thermonuclear {
                "thermonuclear_device"
            } else {
                "nuclear_device"
            }]
            .blast_radius;
            let blast = g.wdisk(target, radius);
            let friendly_exposure = blast.iter().any(|position| {
                g.city_at(*position)
                    .is_some_and(|city| g.cities[&city].owner == pid)
                    || g.units_at(*position)
                        .into_iter()
                        .any(|uid| g.units[&uid].owner == pid)
            });
            if friendly_exposure {
                continue;
            }
            let Some(city) = g.city_at(target) else {
                continue;
            };
            let garrison = blast
                .iter()
                .flat_map(|position| g.units_at(*position))
                .filter(|uid| g.is_at_war(pid, g.units[uid].owner))
                .count();
            let city_ref = &g.cities[&city];
            let hardness = g.city_strength(city) + city_ref.wall_hp as f64 / 10.0;
            // A device is worth spending only on a genuinely hard target.
            if hardness < 50.0 && garrison < 3 {
                continue;
            }
            let value = hardness + garrison as f64 * 12.0;
            if best.as_ref().is_none_or(|(current, _)| value > *current) {
                best = Some((value, action));
            }
        }
        if let Some((_, action)) = best {
            let _ = g.apply(pid, &action);
        }
    }

    fn advanced_units(&mut self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        self.base.begin_movement_turn();
        if self.victory_planning {
            self.rebuild_force_groups(g, pid, plan);
        } else {
            self.force_groups.clear();
        }
        let religious_offensive = self.religious_offensive_posture(g, pid, plan.strategy);
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
                    "military_engineer" => self.base.military_engineer_step(g, pid, uid),
                    "naturalist" => self.base.naturalist_step(g, pid, uid),
                    "archaeologist" => self.base.archaeologist_step(g, pid, uid),
                    "trader" => self.advanced_trader_step(g, pid, uid, plan.strategy),
                    "missionary" if self.victory_planning => self.advanced_missionary_step(
                        g,
                        pid,
                        uid,
                        religious_offensive,
                    ),
                    "missionary" => self.base.missionary_step(g, pid, uid),
                    "rock_band" => self.base.rock_band_step(g, pid, uid),
                    _ if self.victory_planning && class == "religious" => self
                        .advanced_religious_step(
                            g,
                            pid,
                            uid,
                            religious_offensive,
                        ),
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

    /// Immediate population-pressure component of a captured city's Loyalty
    /// change. Governors, policies, and projects can improve this later, but a
    /// city that will revolt in two or three turns cannot wait for them to be
    /// established. Mirroring the rules engine's pressure equation here lets
    /// the mandatory keep/raze decision price that short horizon explicitly.
    fn population_loyalty_delta(g: &Game, pid: usize, city_id: u32) -> f64 {
        let city = &g.cities[&city_id];
        let age_factor = |owner: usize| match g.players[owner].age.as_str() {
            "golden" | "heroic" => 1.5,
            "dark" => 0.5,
            _ => 1.0,
        };
        let mut domestic = 0.0;
        let mut foreign = 0.0;
        for source in g.cities.values() {
            if g.players[source.owner].is_minor || g.players[source.owner].is_barbarian {
                continue;
            }
            let distance = g.wdist(source.pos, city.pos);
            if distance > 9 {
                continue;
            }
            let mut pressure = source.pop as f64
                * (10 - distance) as f64
                * age_factor(source.owner);
            if source.is_capital && source.original_owner == source.owner {
                pressure += source.pop as f64;
            }
            if source.owner == pid {
                domestic += pressure;
            } else if !g.same_team(pid, source.owner)
                && !g
                    .alliance_with(pid, source.owner)
                    .is_some_and(|alliance| alliance.kind == "cultural")
            {
                foreign += pressure;
            }
        }
        (10.0 * (domestic - foreign) / (domestic.min(foreign) + 0.5)).clamp(-20.0, 20.0)
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
            let emergency_objective = before
                .active_emergencies
                .iter()
                .any(|emergency| emergency.city == city_id && emergency.members.contains(&pid));
            if emergency_objective {
                value += if matches!(action, Action::LiberateCity { .. }) {
                    100_000.0
                } else {
                    -100_000.0
                };
            }
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
            let loyalty_delta = Self::population_loyalty_delta(before, pid, city_id);
            let turns_to_flip = if loyalty_delta < 0.0 {
                city.loyalty.max(0.0) / -loyalty_delta
            } else {
                f64::INFINITY
            };
            let disposable = !city.is_capital
                && !before.players[city.original_owner].is_minor
                && city.original_owner != pid
                && !before.are_allied(pid, city.original_owner);
            let imminent_low_value_revolt = development < 35.0 && turns_to_flip <= 4.0;
            let unsupported_revolt = nearest_core > 9 && turns_to_flip <= 8.0;
            let hopeless_occupation = disposable
                && matches!(strategy, GrandStrategy::Conquest | GrandStrategy::Recovery)
                && loyalty_delta <= -8.0
                && (imminent_low_value_revolt || unsupported_revolt);
            match action {
                Action::KeepCity { .. } => {
                    value += development;
                    if nearest_core > 9 && city.loyalty <= 50.0 {
                        value -= (nearest_core - 9) as f64 * 5.0;
                    }
                    if strategy == GrandStrategy::Conquest {
                        value += 35.0;
                    }
                    if hopeless_occupation {
                        value -= 240.0
                            + -loyalty_delta * 18.0
                            + (8.0 - turns_to_flip).max(0.0) * 30.0;
                    }
                }
                Action::RazeCity { .. } => {
                    value -= development * 0.4;
                    if strategy == GrandStrategy::Conquest && nearest_core > 9 && development < 35.0
                    {
                        value += 65.0;
                    }
                    if hopeless_occupation {
                        value += 120.0 + -loyalty_delta * 8.0;
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
    fn strategy_label(&self) -> Option<&'static str> {
        self.plan.as_ref().map(|plan| plan.strategy.as_str())
    }

    fn plan_report(&self) -> Option<PlanReport> {
        let plan = self.plan.as_ref()?;
        Some(PlanReport {
            strategy: plan.strategy.as_str(),
            victory_target: self.victory_target.map(VictoryTarget::as_str),
            target_player: plan.target_player,
            target_city: plan.target_city,
            threatened_city: plan.threatened_city,
            desired_cities: plan.desired_cities,
            assessed_turn: plan.assessed_turn,
            forces: self
                .force_groups
                .iter()
                .map(|group| ForceReport {
                    domain: group.domain.as_str(),
                    posture: group.posture.as_str(),
                    units: group.units.len(),
                    objective: group.objective,
                    readiness: group.readiness,
                    strength_ratio: group.local_strength_ratio,
                })
                .collect(),
        })
    }

    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        self.base.minor = g.players[pid].is_minor;
        self.base.barb = g.players[pid].is_barbarian;
        self.base.pursue_religion =
            self.victory_target.is_none() || self.victory_target == Some(VictoryTarget::Religion);
        if self.base.minor || self.base.barb {
            self.base.take_turn(g, pid);
            return;
        }
        let disposition_strategy = self
            .victory_target
            .map(VictoryTarget::strategy)
            .unwrap_or_else(|| self.victory_focus(g, pid).strategy);
        self.resolve_city_dispositions(g, pid, disposition_strategy);
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
        // Spend Governor Titles against the same strategic plan before the
        // baseline ancillary pass can dilute them across empty cities.
        self.strategic_governors(g, pid, &plan);
        // Keep the mature ancillary systems: governments, policies, beliefs,
        // religions, and envoys. Research is already selected.
        self.base.research_without_government(g, pid);
        self.strategic_government(g, pid, plan.strategy);
        self.base.corporations(g, pid);
        self.advanced_products(g, pid, plan.strategy);
        self.advanced_great_people(g, pid, plan.strategy);
        if self.victory_planning {
            let committed = plan.strategy == GrandStrategy::Religion;
            let offensive = self.religious_offensive_posture(g, pid, plan.strategy);
            // A secondary campaign spends only the bank above a substantial
            // reserve, leaving Culture agents able to buy Naturalists or Rock
            // Bands and every other plan able to react to an emergency.
            let reserve = if committed {
                80.0
            } else {
                g.game_speed.scale(1_200.0)
            };
            self.religious_spending_with_reserve(g, pid, offensive, reserve);
        }
        self.faith_building_spending(g, pid, plan.strategy);
        self.military_faith_spending(g, pid, &plan);
        // Live spectator majors choose an adaptive plan instead of carrying
        // an explicit `victory_target`. Give both modes the same strategic
        // purchase pass; otherwise the adaptive agents are limited to the
        // baseline building/unit buyer and can carry thousands of Gold past
        // an immediately affordable plan-critical district.
        if self.victory_planning {
            self.advanced_gold_spending(g, pid, &plan);
        }
        self.strategic_policies(g, pid, plan.strategy);
        self.advanced_diplomacy(g, pid, &plan);
        self.advanced_spies(g, pid, &plan);

        // Preserve the proven four-build opening before switching every city
        // to utility planning. This also keeps the frozen baseline comparable.
        if self.base.book_pos < 4 {
            self.base.cities(g, pid);
        } else {
            if self.victory_planning {
                self.redirect_repeatable_projects_for_force_gap(g, pid, &plan);
            }
            // Explicit victory-target runs use strategic production directly;
            // otherwise the baseline governor remains the stronger general
            // policy in paired evaluation.
            if self.victory_planning && plan.strategy == GrandStrategy::Religion {
                self.religious_production(g, pid);
            } else if self.victory_planning && g.players[pid].religion.is_none() {
                // Every other strategy still defends its homeland: a rival's
                // religious victory needs a majority in every living major,
                // and before this pass non-religion civilizations never spent
                // a point of Faith resisting conversion.
                if let Some(threat) = self.home_conversion_threat(g, pid) {
                    self.religious_defense(g, pid, &threat);
                }
            }
            if self.victory_planning
                && (plan.strategy == GrandStrategy::Science
                    || self.diplomatic_science_backup(g, pid, &plan))
            {
                self.science_production(g, pid);
            }
            if self.victory_planning && plan.strategy == GrandStrategy::Culture {
                self.culture_spending(g, pid);
            }
            if plan.strategy == GrandStrategy::Recovery || self.victory_target.is_some() {
                self.advanced_production(g, pid, &plan);
            }
            if self.victory_target.is_none() {
                self.advanced_support_production(g, pid, &plan);
                self.base.cities(g, pid);
            }
        }
        if self.victory_target.is_some() {
            let counts = self.counts(g, pid);
            let cities = g.player_city_ids(pid);
            self.base.spend_gold(
                g,
                pid,
                &cities,
                counts.settlers,
                counts.builders,
                counts.traders,
                counts.military,
                counts.melee,
                counts.ranged,
            );
        }
        if self.victory_planning {
            self.advanced_command_actions(g, pid, &plan);
        }
        BasicAi::upgrade_units(g, pid);
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
    use crate::game::GovernorState;

    fn found_test_city(game: &mut Game, pid: usize) -> u32 {
        let position = game
            .map
            .tiles
            .values()
            .filter(|tile| {
                game.rules.is_passable(tile)
                    && !game.rules.is_water(tile)
                    && tile.district.is_none()
                    && tile.wonder.is_none()
                    && tile.owner_city.is_none()
                    && game.city_at(tile.pos).is_none()
                    && game.units_at(tile.pos).is_empty()
                    && game
                        .cities
                        .values()
                        .all(|city| game.wdist(city.pos, tile.pos) >= 4)
            })
            .map(|tile| tile.pos)
            .next()
            .expect("test map needs another legal city site");
        let settler = game.spawn_test_unit("settler", pid, position);
        game.current = pid;
        game.apply(pid, &Action::FoundCity { unit: settler })
            .unwrap();
        game.city_at(position).unwrap()
    }

    fn install_ai_test_district(game: &mut Game, city: u32, district: &str) -> Pos {
        let center = game.cities[&city].pos;
        let position = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| {
                *position != center
                    && game.map.tiles[position].district.is_none()
                    && game.map.tiles[position].wonder.is_none()
                    && game.map.tiles[position].improvement.is_none()
            })
            .expect("test city has an unused district tile");
        let tile = game.map.tiles.get_mut(&position).unwrap();
        tile.district = Some(district.to_string());
        tile.pillaged = false;
        game.cities
            .get_mut(&city)
            .unwrap()
            .districts
            .insert(district.to_string(), position);
        position
    }

    fn install_test_holy_site(game: &mut Game, city: u32) {
        install_ai_test_district(game, city, "holy_site");
        game.cities.get_mut(&city).unwrap().buildings =
            vec!["shrine".to_string(), "temple".to_string()];
    }

    fn found_nearby_test_city(game: &mut Game, owner: usize, anchor: Pos) -> u32 {
        let position = game
            .map
            .tiles
            .iter()
            .filter(|(_, tile)| {
                tile.owner_city.is_none()
                    && game.rules.is_passable(tile)
                    && !game.rules.is_water(tile)
            })
            .map(|(position, _)| *position)
            .find(|position| {
                (4..=10).contains(&game.wdist(anchor, *position))
                    && game
                        .cities
                        .values()
                        .all(|city| game.wdist(city.pos, *position) >= 4)
                    // Units staged here must be able to walk out: skip sites
                    // ringed by water and mountains.
                    && game
                        .nbrs(*position)
                        .iter()
                        .filter(|neighbour| {
                            game.map.get(**neighbour).is_some_and(|tile| {
                                game.rules.is_passable(tile) && !game.rules.is_water(tile)
                            })
                        })
                        .count()
                        >= 3
            })
            .expect("test map has a nearby city site");
        game.found_city_for(owner, position, None)
    }

    #[test]
    fn conquest_ai_spends_a_device_on_the_hard_city_but_spares_its_own() {
        let mut game = Game::new_full(2, 24, 16, 91_802, 200, 0, false);
        for pid in 0..2 {
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.found_city_for(pid, game.units[&settler].pos, None);
            game.remove_unit(settler);
        }
        let target_city = game.player_city_ids(1)[0];
        let target = game.cities[&target_city].pos;
        game.at_war.insert((0, 1));
        game.players[0]
            .counters
            .insert("project_effect:thermonuclear_devices".to_string(), 1);
        game.players[0].explored.insert(target);
        game.cities.get_mut(&target_city).unwrap().wall_hp = 300;
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: Some(target_city),
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::targeting(VictoryTarget::Domination);

        // A friendly scout in the blast must hold the launch.
        let radius = game.rules.wmds["thermonuclear_device"].blast_radius;
        let picket_pos = game
            .wdisk(target, radius)
            .into_iter()
            .find(|position| {
                *position != target
                    && game.map.get(*position).is_some_and(|tile| {
                        game.rules.is_passable(tile) && !game.rules.is_water(tile)
                    })
                    && game.units_at(*position).is_empty()
            })
            .expect("blast ring has an open land tile");
        let picket = game.spawn_test_unit("scout", 0, picket_pos);
        ai.advanced_wmd_strikes(&mut game, 0, &plan);
        assert_eq!(
            game.players[0].counters["project_effect:thermonuclear_devices"],
            1,
            "no launch while a friendly unit stands in the blast"
        );

        game.remove_unit(picket);
        ai.advanced_wmd_strikes(&mut game, 0, &plan);
        assert_eq!(
            game.players[0].counters["project_effect:thermonuclear_devices"],
            0,
            "the hard city draws the device once the blast is clean"
        );
        assert!(game.map.tiles[&target].fallout_until > game.turn);
    }

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
        install_ai_test_district(&mut game, origin, "commercial_hub");
        install_ai_test_district(&mut game, target, "commercial_hub");
        install_ai_test_district(&mut game, target, "theater_square");
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

        // Keep both cities in the same amenity band, so the comparison
        // exercises the culture multipliers rather than the happiness gap.
        game.cities.get_mut(&origin).unwrap().pop = 6;
        game.cities.get_mut(&target).unwrap().pop = 2;
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
    fn capital_settler_retargets_when_its_cached_site_becomes_illegal() {
        let mut game = Game::new_full(2, 30, 18, 9_204, 120, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| game.units[uid].kind == "settler")
            .unwrap();
        let start = game.units[&settler].pos;
        let blocker = game
            .map
            .tiles
            .iter()
            .filter(|(_, tile)| game.rules.is_passable(tile) && !game.rules.is_water(tile))
            .map(|(position, _)| *position)
            .find(|position| game.wdist(start, *position) == 3)
            .expect("test start has a land tile three hexes away");
        game.found_city_for(1, blocker, None);
        game.current = 0;
        assert!(!game.can_found_city(settler));

        let mut ai = AdvancedAi::new();
        ai.settler_targets.insert(settler, start);
        for _ in 0..100 {
            if !game.units.contains_key(&settler) {
                break;
            }
            let unit = game.units.get_mut(&settler).unwrap();
            unit.moves_left = 4.0;
            unit.acted = false;
            assert!(
                ai.advanced_settler_step(&mut game, 0, settler),
                "the capital settler should keep routing to a replacement site"
            );
            assert_ne!(ai.settler_targets.get(&settler), Some(&start));
        }

        assert!(!game.player_city_ids(0).is_empty());
        assert!(!game.units.contains_key(&settler));
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

        game.players[1]
            .counters
            .insert("project_effect:nuclear_devices".to_string(), 3);
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution("arms_control", &["0", "1", "2"]),
                GrandStrategy::Conquest,
            ),
            Some("B:1".to_string())
        );
        game.players[0]
            .counters
            .insert("project_effect:nuclear_devices".to_string(), 1);
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution("arms_control", &["0", "1", "2"]),
                GrandStrategy::Diplomacy,
            ),
            Some("A:2".to_string())
        );

        game.players[0].government = Some("autocracy".to_string());
        game.players[1].government = Some("democracy".to_string());
        game.players[2].government = Some("democracy".to_string());
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution("world_ideology", &["autocracy", "democracy"]),
                GrandStrategy::Science,
            ),
            Some("A:autocracy".to_string())
        );
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution("border_control_treaty", &["0", "1", "2"]),
                GrandStrategy::Expansion,
            ),
            Some("A:0".to_string())
        );
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution(
                    "public_works_program",
                    &["launch_earth_satellite", "manhattan_project"],
                ),
                GrandStrategy::Science,
            ),
            Some("A:launch_earth_satellite".to_string())
        );
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution(
                    "global_energy_treaty",
                    &["coal_power_plant", "oil_power_plant", "nuclear_power_plant"],
                ),
                GrandStrategy::Science,
            ),
            Some("A:nuclear_power_plant".to_string())
        );
        assert_eq!(
            ai.congress_choice(
                &game,
                0,
                &outcome_resolution("deforestation_treaty", &["forest"]),
                GrandStrategy::Expansion,
            ),
            Some("A:forest".to_string())
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

        plan.strategy = GrandStrategy::Conquest;
        assert!(
            ai.incoming_deal_value(&game, 0, &deal(10.0, 0.0, true, false), &plan) < 0.0,
            "a campaign target must not be protected by a new friendship"
        );
        plan.strategy = GrandStrategy::Expansion;

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
        assert!(
            ai.incoming_deal_value(&game, 0, &deal(0.0, 100.0, false, true), &plan) < 0.0,
            "a strong Recovery posture must not abandon its active campaign target"
        );
        plan.target_player = None;
        assert!(ai.incoming_deal_value(&game, 0, &deal(0.0, 100.0, false, true), &plan) > 0.0);
    }

    #[test]
    fn outmatched_major_must_negotiate_peace_with_the_winning_campaign() {
        let mut game = Game::new_full(2, 24, 16, 7_922, 300, 0, false);
        for pid in 0..2 {
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.found_city_for(pid, game.units[&settler].pos, None);
            game.remove_unit(settler);
        }
        let staging = game.cities[&game.player_city_ids(0)[0]].pos;
        for _ in 0..3 {
            game.spawn_test_unit("modern_armor", 0, staging);
        }
        game.current = 0;
        game.turn = 60;
        game.apply(0, &Action::DeclareWar { player: 1 })
            .unwrap();
        game.turn = game
            .peace_available_at(0, 1)
            .expect("the new war has a mandatory minimum");
        assert!(game.military_power(1) < game.military_power(0) * 0.62);

        let recovery = StrategicPlan {
            strategy: GrandStrategy::Recovery,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 2,
            assessed_turn: game.turn,
        };
        let mut defender = AdvancedAi::new();
        defender.major_war_since = Some(60);
        game.current = 1;
        defender.advanced_diplomacy(&mut game, 1, &recovery);

        assert!(
            game.is_at_war(0, 1),
            "an outmatched defender cannot impose white peace unilaterally"
        );
        assert!(game.pending_deals.iter().any(|deal| {
            deal.from == 1 && deal.to == 0 && deal.peace
        }));

        let winning_campaign = StrategicPlan {
            // A threatened home city can temporarily classify even a much
            // stronger attacker as Recovery. Its active campaign target must
            // still be able to refuse the defender's immediate white peace.
            strategy: GrandStrategy::Recovery,
            target_player: Some(1),
            target_city: game.player_city_ids(1).into_iter().next(),
            threatened_city: None,
            desired_cities: 2,
            assessed_turn: game.turn,
        };
        let mut refused = game.clone();
        let mut conqueror = AdvancedAi::new();
        conqueror.major_war_since = Some(60);
        refused.current = 0;
        conqueror.advanced_diplomacy(&mut refused, 0, &winning_campaign);
        assert!(refused.is_at_war(0, 1));
        assert!(
            refused.pending_deals.iter().all(|deal| !deal.peace),
            "the stronger conquest plan should reject an immediate white peace"
        );

        let mut accepting = AdvancedAi::new();
        accepting.major_war_since = Some(60);
        game.current = 0;
        accepting.advanced_diplomacy(&mut game, 0, &recovery);
        assert!(!game.is_at_war(0, 1));
        assert_eq!(accepting.peace_until, game.turn + 30);
        assert!(accepting.major_war_since.is_none());
    }

    #[test]
    fn advanced_ai_proposes_the_alliance_for_its_victory_plan() {
        let mut game = Game::new_full(3, 24, 16, 782, 300, 0, false);
        game.turn = 12;
        for player in &mut game.players {
            player.civics.insert("civil_service".to_string());
            player.techs.insert("scientific_theory".to_string());
        }
        game.players[1].techs.insert("radio".to_string());
        let plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::targeting(VictoryTarget::Science);
        assert!(game.legal_actions(0).iter().any(|action| {
            matches!(
                action,
                Action::ProposeDeal {
                    alliance: Some(kind),
                    ..
                } if kind == "research"
            )
        }));
        assert!(ai.rival_victory_pressure(&game, 1).progress < 82);
        ai.propose_strategic_alliance(&mut game, 0, &plan, None);
        let proposal = game
            .pending_deals
            .iter()
            .find(|deal| deal.from == 0)
            .unwrap();
        assert_eq!(proposal.alliance.as_deref(), Some("research"));
        assert!(proposal.friendship);
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
    fn governor_titles_promote_the_primary_before_widening_the_roster() {
        let mut game = Game::new_full(1, 24, 16, 7_111, 200, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        // Three civics that each award a Governor title: one pays for the
        // appointment, two for promotions.
        game.players[0].civics.extend([
            "state_workforce".to_string(),
            "early_empire".to_string(),
            "guilds".to_string(),
        ]);
        let plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::new();
        ai.strategic_governors(&mut game, 0, &plan);

        assert_eq!(game.players[0].governor_roster.len(), 1);
        let pingala = &game.players[0].governor_roster["pingala"];
        // Which two tier-one promotions it picks is a weighting detail; the
        // rule under test is that both titles went to the primary governor.
        assert_eq!(pingala.promotions.len(), 2);
        assert_eq!(game.governor_titles_available(0), 0);

        found_test_city(&mut game, 0);
        game.players[0]
            .counters
            .insert("district_governor_titles".to_string(), 1);
        ai.strategic_governors(&mut game, 0, &plan);
        assert_eq!(game.players[0].governor_roster.len(), 2);
        assert!(game.players[0].governor_roster.contains_key("magnus"));
        assert_eq!(
            game.players[0].governor_roster["pingala"].promotions.len(),
            2
        );
    }

    #[test]
    fn governor_path_stays_focused_when_strategy_changes_between_titles() {
        let mut game = Game::new_full(1, 24, 16, 7_112, 200, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        game.players[0]
            .civics
            .insert("state_workforce".to_string());
        let assessed_turn = game.turn;
        let plan = |strategy| StrategicPlan {
            strategy,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn,
        };
        let ai = AdvancedAi::new();
        ai.strategic_governors(&mut game, 0, &plan(GrandStrategy::Expansion));
        assert!(game.players[0].governor_roster.contains_key("magnus"));

        game.players[0]
            .counters
            .insert("district_governor_titles".to_string(), 1);
        ai.strategic_governors(&mut game, 0, &plan(GrandStrategy::Science));
        game.players[0]
            .counters
            .insert("district_governor_titles".to_string(), 2);
        ai.strategic_governors(&mut game, 0, &plan(GrandStrategy::Conquest));

        assert_eq!(game.players[0].governor_roster.len(), 1);
        assert_eq!(
            game.players[0].governor_roster["magnus"].promotions.len(),
            2
        );

        found_test_city(&mut game, 0);
        game.players[0]
            .counters
            .insert("district_governor_titles".to_string(), 3);
        ai.strategic_governors(&mut game, 0, &plan(GrandStrategy::Science));
        assert!(game.players[0].governor_roster.contains_key("pingala"));
    }

    #[test]
    fn first_governor_matches_the_empire_strategy() {
        for (index, (strategy, expected)) in [
            (GrandStrategy::Expansion, "magnus"),
            (GrandStrategy::Science, "pingala"),
            (GrandStrategy::Culture, "pingala"),
            (GrandStrategy::Religion, "moksha"),
            (GrandStrategy::Diplomacy, "amani"),
            (GrandStrategy::Conquest, "victor"),
            (GrandStrategy::Recovery, "victor"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut game = Game::new_full(1, 18, 10, 7_120 + index as u64, 120, 0, false);
            let settler = game
                .player_unit_ids(0)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
            game.players[0]
                .civics
                .insert("state_workforce".to_string());
            let plan = StrategicPlan {
                strategy,
                target_player: None,
                target_city: None,
                threatened_city: None,
                desired_cities: 3,
                assessed_turn: game.turn,
            };
            AdvancedAi::new().strategic_governors(&mut game, 0, &plan);
            assert!(
                game.players[0].governor_roster.contains_key(expected),
                "{strategy:?} appointed {:?}",
                game.players[0].governor_roster.keys().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn expansion_window_reaches_its_six_city_target_before_endgame() {
        let mut game = Game::new_full(1, 30, 18, 7_113, 500, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        for tile in game.map.tiles.values_mut() {
            tile.terrain = "grassland".to_string();
            tile.feature = None;
        }
        let city = game.player_city_ids(0)[0];
        game.cities.get_mut(&city).unwrap().pop = 6;
        game.turn = 270;

        let ai = AdvancedAi::new();
        let plan = ai.assess(&game, 0);
        assert_eq!(plan.desired_cities, 6);
        assert_eq!(plan.strategy, GrandStrategy::Expansion);
        let item = Item::Unit {
            unit: "settler".to_string(),
        };
        let counts = ai.counts(&game, 0);
        assert!(ai.production_value(&game, 0, city, &item, &plan, &counts) > -9_000.0);

        game.turn = 300;
        assert!(!AdvancedAi::expansion_window_open(&game));
        assert!(ai.production_value(&game, 0, city, &item, &plan, &counts) < -9_000.0);
    }

    #[test]
    fn conquest_can_target_an_exposed_city_state_but_preserves_its_suzerain() {
        let mut game = Game::new_full(2, 30, 18, 711, 300, 1, false);
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
        game.turn = 200;
        let minor = game
            .players
            .iter()
            .find(|player| player.is_minor && !player.is_barbarian)
            .unwrap()
            .id;
        let rival_capital = game.cities[&game.player_city_ids(1)[0]].pos;
        for _ in 0..6 {
            game.spawn_test_unit("giant_death_robot", 1, rival_capital);
        }

        let ai = AdvancedAi::targeting(VictoryTarget::Domination);
        let exposed = ai.assess(&game, 0);
        assert_eq!(exposed.strategy, GrandStrategy::Conquest);
        assert_eq!(exposed.target_player, Some(minor));

        game.players[0].envoys = vec![(minor, 3)];
        assert_eq!(game.suzerain_of(minor), Some(0));
        let allied = ai.assess(&game, 0);
        assert_eq!(allied.target_player, Some(1));
    }

    #[test]
    fn campaign_masks_allied_rivals_and_their_suzerained_city_states() {
        let mut game = Game::new_full(2, 30, 18, 7_112, 300, 1, false);
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
        game.turn = 200;
        let minor = game
            .players
            .iter()
            .find(|player| player.is_minor && !player.is_barbarian)
            .unwrap()
            .id;
        game.players[1].envoys = vec![(minor, 3)];
        assert_eq!(game.suzerain_of(minor), Some(1));

        let alliance = crate::game::AllianceState {
            kind: "military".to_string(),
            points: 0.0,
            level: 1,
            ends: game.turn + 30,
        };
        game.players[0].alliances.insert(1, alliance.clone());
        game.players[1].alliances.insert(0, alliance);

        let ai = AdvancedAi::targeting(VictoryTarget::Domination);
        assert!(!ai.campaign_target_legal(&game, 0, 1));
        assert!(!ai.campaign_target_legal(&game, 0, minor));
        assert_eq!(ai.assess(&game, 0).target_player, None);

        let mut stale_ai = AdvancedAi::targeting(VictoryTarget::Domination);
        stale_ai.plan = Some(StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: game.player_city_ids(1).first().copied(),
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        });
        assert!(
            stale_ai.plan_stale(&game, 0),
            "a new alliance must interrupt a cached hostile plan immediately"
        );

        // A loaded legacy position can contain contradictory relationship
        // state. An actual war remains a forcing objective until peace.
        game.at_war.insert((0, 1));
        assert!(ai.campaign_target_legal(&game, 0, 1));
        assert!(
            ai.assess(&game, 0)
                .target_player
                .is_some_and(|target| target == 1 || target == minor),
            "the suzerain or the city-state that joined its war must remain actionable"
        );
    }

    #[test]
    fn campaign_city_ordering_prefers_a_breach_then_the_domination_capital() {
        let mut game = Game::new_full(2, 30, 18, 7_111, 300, 0, false);
        for pid in 0..2 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        let own_capital = game.player_city_ids(0)[0];
        let enemy_capital = game.player_city_ids(1)[0];
        let enemy_position = game.cities[&enemy_capital].pos;
        let capital_distance = game.wdist(game.cities[&own_capital].pos, enemy_position);
        let outpost_position = game
            .map
            .tiles
            .iter()
            .filter(|(position, tile)| {
                game.rules.is_passable(tile)
                    && !game.rules.is_water(tile)
                    && tile.owner_city.is_none()
                    && game.wdist(enemy_position, **position) >= 9
                    && game
                        .cities
                        .values()
                        .all(|city| game.wdist(city.pos, **position) >= 4)
            })
            .min_by_key(|(position, _)| {
                (
                    (game.wdist(game.cities[&own_capital].pos, **position) - capital_distance)
                        .abs(),
                    **position,
                )
            })
            .map(|(position, _)| *position)
            .expect("test map has a comparable second-city site");
        game.current = 1;
        let settler = game.spawn_test_unit("settler", 1, outpost_position);
        game.apply(1, &Action::FoundCity { unit: settler }).unwrap();
        let enemy_outpost = game
            .player_city_ids(1)
            .into_iter()
            .find(|city| *city != enemy_capital)
            .unwrap();
        for unit in game.units.keys().copied().collect::<Vec<_>>() {
            game.remove_unit(unit);
        }

        let fortify = |game: &mut Game, city: u32| {
            let position = {
                let target = game.cities.get_mut(&city).unwrap();
                target.hp = 200;
                target.wall_hp = 400;
                target.buildings.extend([
                    "walls".to_string(),
                    "medieval_walls".to_string(),
                    "renaissance_walls".to_string(),
                ]);
                target.pos
            };
            for _ in 0..3 {
                game.spawn_test_unit("giant_death_robot", 1, position);
            }
        };
        let breach = |game: &mut Game, city: u32| {
            let target = game.cities.get_mut(&city).unwrap();
            target.hp = 25;
            target.wall_hp = 0;
            target.buildings.retain(|building| {
                !matches!(
                    building.as_str(),
                    "walls" | "medieval_walls" | "renaissance_walls"
                )
            });
        };
        let ai = AdvancedAi::targeting(VictoryTarget::Domination);

        fortify(&mut game, enemy_capital);
        breach(&mut game, enemy_outpost);
        let exposed_outpost = ai.campaign_city_value(
            &game,
            0,
            &game.cities[&enemy_outpost],
            GrandStrategy::Conquest,
        );
        let fortified_capital = ai.campaign_city_value(
            &game,
            0,
            &game.cities[&enemy_capital],
            GrandStrategy::Conquest,
        );
        assert!(
            exposed_outpost < fortified_capital,
            "an exposed breach ({exposed_outpost}) should be searched before a fully defended capital ({fortified_capital})"
        );

        for unit in game.units.keys().copied().collect::<Vec<_>>() {
            game.remove_unit(unit);
        }
        breach(&mut game, enemy_capital);
        fortify(&mut game, enemy_outpost);
        assert!(
            ai.campaign_city_value(
                &game,
                0,
                &game.cities[&enemy_capital],
                GrandStrategy::Conquest,
            ) < ai.campaign_city_value(
                &game,
                0,
                &game.cities[&enemy_outpost],
                GrandStrategy::Conquest,
            ),
            "once both geometry and defenses favor it, Domination must order the original capital first"
        );
    }

    #[test]
    fn conquest_army_stages_before_diplomacy_opens_the_war() {
        let mut game = Game::new_full(2, 30, 18, 7_114, 300, 0, false);
        for pid in 0..2 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
        game.turn = 60;
        let target_city = game.player_city_ids(1)[0];
        let objective = game.cities[&target_city].pos;

        // The declaration rule also requires a two-city operating base. Put
        // the second city close enough that this test isolates army staging.
        let second_site = game
            .map
            .tiles
            .iter()
            .filter(|(position, tile)| {
                game.rules.is_passable(tile)
                    && !game.rules.is_water(tile)
                    && tile.owner_city.is_none()
                    && game.wdist(**position, objective) <= 18
                    && game
                        .cities
                        .values()
                        .all(|city| game.wdist(city.pos, **position) >= 4)
            })
            .map(|(position, _)| *position)
            .next()
            .expect("test map has a legal second-city site");
        let settler = game.spawn_test_unit("settler", 0, second_site);
        game.apply(0, &Action::FoundCity { unit: settler })
            .unwrap();

        for unit in game.units.keys().copied().collect::<Vec<_>>() {
            game.remove_unit(unit);
        }
        for tile in game.map.tiles.values_mut() {
            tile.terrain = "grassland".to_string();
            tile.feature = None;
            tile.hills = false;
        }
        let remote = game
            .map
            .tiles
            .keys()
            .copied()
            .find(|position| {
                game.wdist(*position, objective) >= 10
                    && game.city_at(*position).is_none()
                    && game.map.tiles[position]
                        .owner_city
                        .and_then(|city| game.cities.get(&city))
                        .is_none_or(|city| city.owner != 1)
            })
            .expect("test map has a remote muster position");
        let army: Vec<u32> = (0..4)
            .map(|_| game.spawn_test_unit("swordsman", 0, remote))
            .collect();
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: Some(target_city),
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let mut ai = AdvancedAi::targeting(VictoryTarget::Domination);

        assert!(!ai.campaign_staged_for_war(&game, 0, 1, objective, true));
        let before = game.wdist(game.units[&army[0]].pos, objective);
        assert_eq!(
            ai.campaign_staging_step(&mut game, 0, army[0], &plan),
            Some(true)
        );
        assert!(game.wdist(game.units[&army[0]].pos, objective) < before);

        ai.advanced_diplomacy(&mut game, 0, &plan);
        assert!(
            !game.players[0]
                .denounced_until
                .get(&1)
                .is_some_and(|until| *until > game.turn),
            "remote global power must not begin the diplomatic war countdown"
        );

        let staging: Vec<Pos> = game
            .wdisk(objective, 7)
            .into_iter()
            .filter(|position| {
                (3..=5).contains(&game.wdist(*position, objective))
                    && game.city_at(*position).is_none()
            })
            .take(army.len())
            .collect();
        assert_eq!(staging.len(), army.len());
        for (unit, position) in army.iter().zip(staging) {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "grassland".to_string();
            tile.feature = None;
            tile.hills = false;
            tile.owner_city = None;
            game.units.get_mut(unit).unwrap().pos = position;
        }
        assert!(ai.campaign_staged_for_war(&game, 0, 1, objective, true));

        ai.advanced_diplomacy(&mut game, 0, &plan);
        assert!(
            game.players[0]
                .denounced_until
                .get(&1)
                .is_some_and(|until| *until > game.turn),
            "the staged capture force should begin the formal-war countdown"
        );
    }

    #[test]
    fn war_opening_waits_for_formal_war_but_interrupts_for_imminent_victory() {
        let mut game = Game::new_full(2, 24, 16, 712, 300, 0, false);
        game.current = 0;
        game.turn = 60;
        let ai = AdvancedAi::new();

        assert_eq!(
            ai.preferred_war_opening(&game, 0, 1),
            Some(Action::Denounce { player: 1 })
        );
        game.apply(0, &Action::Denounce { player: 1 }).unwrap();
        game.turn = 64;
        assert_eq!(ai.preferred_war_opening(&game, 0, 1), None);
        game.turn = 65;
        assert_eq!(
            ai.preferred_war_opening(&game, 0, 1),
            Some(Action::DeclareWarWithCasusBelli {
                player: 1,
                casus_belli: "formal_war".to_string(),
            })
        );

        let mut emergency = Game::new_full(2, 24, 16, 713, 300, 0, false);
        emergency.current = 0;
        emergency.turn = 60;
        emergency.players[1].science_projects.extend([
            "launch_earth_satellite".to_string(),
            "launch_moon_landing".to_string(),
            "launch_mars_colony".to_string(),
            "exoplanet_expedition".to_string(),
        ]);
        emergency.players[1].exoplanet_distance = 49.0;
        assert_eq!(
            ai.preferred_war_opening(&emergency, 0, 1),
            Some(Action::DeclareWar { player: 1 })
        );
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
    fn recovery_requires_material_local_danger_and_ends_when_it_clears() {
        let mut game = Game::new_full(2, 30, 18, 7_218, 300, 0, false);
        for pid in 0..2 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        for unit in game.units.keys().copied().collect::<Vec<_>>() {
            game.remove_unit(unit);
        }
        game.current = 0;
        game.turn = 90;
        game.at_war.insert((0, 1));
        let home = game.player_city_ids(0)[0];
        let home_pos = game.cities[&home].pos;
        let intruder_pos = game
            .wdisk(home_pos, 6)
            .into_iter()
            .find(|position| {
                game.wdist(*position, home_pos) == 3 && game.city_at(*position).is_none()
            })
            .unwrap();
        let far_pos = game
            .map
            .tiles
            .keys()
            .copied()
            .find(|position| {
                game.wdist(*position, home_pos) >= 9 && game.city_at(*position).is_none()
            })
            .unwrap();
        for position in [intruder_pos, far_pos] {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "grassland".to_string();
            tile.feature = None;
            tile.hills = false;
        }
        for _ in 0..4 {
            game.spawn_test_unit("modern_armor", 0, far_pos);
        }
        let mut intruders = vec![game.spawn_test_unit("warrior", 1, intruder_pos)];
        let mut ai = AdvancedAi::new();

        assert_eq!(
            ai.threatened_city(&game, 0),
            None,
            "one losing contact in the outer radius must not recall a dominant army"
        );
        assert_eq!(ai.assess(&game, 0).strategy, GrandStrategy::Conquest);

        for _ in 0..4 {
            intruders.push(game.spawn_test_unit("modern_armor", 1, intruder_pos));
        }
        assert_eq!(ai.threatened_city(&game, 0), Some(home));
        let recovery = ai.assess(&game, 0);
        assert_eq!(recovery.strategy, GrandStrategy::Recovery);
        assert_eq!(recovery.threatened_city, Some(home));

        ai.plan = Some(recovery);
        for unit in intruders {
            game.remove_unit(unit);
        }
        assert!(
            ai.plan_stale(&game, 0),
            "clearing the emergency must resume the campaign immediately"
        );
        assert_eq!(ai.assess(&game, 0).strategy, GrandStrategy::Conquest);
    }

    #[test]
    fn religious_denial_triggers_with_one_unconverted_civilization() {
        let mut game = Game::new_full(4, 30, 18, 7_215, 300, 0, false);
        for pid in 0..4 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
        game.players[1].religion = Some("Rival Faith".to_string());
        for owner in [1, 2, 3] {
            let city = game.player_city_ids(owner)[0];
            game.cities
                .get_mut(&city)
                .unwrap()
                .pressure
                .insert("Rival Faith".to_string(), 1_000.0);
        }

        let ai = AdvancedAi::new();
        let pressure = ai.rival_victory_pressure(&game, 1);
        assert_eq!(pressure.strategy, GrandStrategy::Religion);
        assert_eq!(pressure.progress, 75);
        assert_eq!(
            ai.victory_denial(&game, 0),
            Some((1, GrandStrategy::Conquest))
        );
    }

    #[test]
    fn religious_denial_warns_early_but_never_ignores_match_point() {
        let mut game = Game::new_full(4, 30, 18, 7_216, 300, 0, false);
        for pid in 0..4 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
        game.players[1].religion = Some("Rival Faith".to_string());
        for owner in [1, 2] {
            let city = game.player_city_ids(owner)[0];
            game.cities
                .get_mut(&city)
                .unwrap()
                .pressure
                .insert("Rival Faith".to_string(), 1_000.0);
        }

        let ai = AdvancedAi::new();
        assert_eq!(ai.rival_victory_pressure(&game, 1).progress, 50);
        assert_eq!(
            ai.victory_denial(&game, 0),
            Some((1, GrandStrategy::Conquest)),
            "two remaining holdouts leave time to build and route a defense"
        );

        game.players[0].dvp = 13;
        assert_eq!(ai.victory_focus(&game, 0).progress, 65);
        assert_eq!(
            ai.victory_denial(&game, 0),
            None,
            "an early warning need not derail a meaningfully closer race"
        );

        let last_converted = game.player_city_ids(3)[0];
        game.cities
            .get_mut(&last_converted)
            .unwrap()
            .pressure
            .insert("Rival Faith".to_string(), 1_000.0);
        assert_eq!(ai.rival_victory_pressure(&game, 1).progress, 75);
        assert_eq!(
            ai.victory_denial(&game, 0),
            Some((1, GrandStrategy::Conquest)),
            "a one-conversion match point must interrupt even a close own race"
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
    fn religious_focus_counts_foreign_conversions_not_the_founder() {
        let mut game = Game::new_full(4, 30, 18, 7_600, 300, 0, false);
        for pid in 0..4 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
        game.players[0].religion = Some("Test Faith".to_string());
        let convert = |game: &mut Game, owner: usize| {
            let city = game.player_city_ids(owner)[0];
            game.cities
                .get_mut(&city)
                .unwrap()
                .pressure
                .insert("Test Faith".to_string(), 1_000.0);
        };
        convert(&mut game, 0);

        let ai = AdvancedAi::new();
        let founded = ai.victory_focus(&game, 0);
        assert_eq!(founded.strategy, GrandStrategy::Religion);
        assert_eq!(
            founded.progress, 40,
            "the founder's own majority is not a foreign victory gain"
        );

        convert(&mut game, 1);
        assert_eq!(ai.victory_focus(&game, 0).progress, 60);
        convert(&mut game, 2);
        assert_eq!(ai.victory_focus(&game, 0).progress, 80);
        convert(&mut game, 3);
        assert_eq!(ai.victory_focus(&game, 0).progress, 100);
    }

    #[test]
    fn victory_focus_tracks_technology_before_the_first_space_project() {
        let ai = AdvancedAi::new();
        let mut game = Game::new(2, 24, 16, 761, 300, 0);
        game.turn = 111;
        let opening = ai.victory_focus(&game, 0);
        assert_eq!(opening.strategy, GrandStrategy::Science);
        assert_eq!(opening.progress, 25);

        let researched: Vec<String> = game
            .rules
            .techs
            .keys()
            .take(game.rules.techs.len() * 2 / 3)
            .cloned()
            .collect();
        game.players[0].techs.extend(researched);
        let developed = ai.victory_focus(&game, 0);
        assert_eq!(developed.strategy, GrandStrategy::Science);
        assert!(developed.progress >= 44, "progress={}", developed.progress);

        game.players[0].civ = "China".to_string();
        game.players[0].techs.clear();
        assert_eq!(ai.victory_focus(&game, 0).progress, 45);
    }

    #[test]
    fn adaptive_science_readiness_commits_to_the_rocketry_path() {
        let mut game = Game::new(2, 24, 16, 76_001, 300, 0);
        let ai = AdvancedAi::new();
        let rocketry_path: Vec<String> = game
            .rules
            .techs
            .keys()
            .filter(|tech| ai.tech_leads_to(&game, tech, "rocketry"))
            .cloned()
            .collect();
        for tech in rocketry_path
            .iter()
            .filter(|tech| tech.as_str() != "rocketry")
        {
            game.players[0].techs.insert(tech.clone());
        }
        game.players[0].dvp = 10;

        let focus = ai.victory_focus(&game, 0);
        assert_eq!(focus.strategy, GrandStrategy::Science);
        assert!(focus.progress > 50);

        let plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        ai.advanced_research(&mut game, 0, &plan);
        assert_eq!(game.players[0].research.as_deref(), Some("rocketry"));
    }

    #[test]
    fn mature_diplomatic_plan_prepares_one_science_backup() {
        let mut game = Game::new(2, 24, 16, 76_002, 500, 0);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let site = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != game.cities[&city].pos)
            .unwrap();
        {
            let tile = game.map.tiles.get_mut(&site).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.hills = false;
        }
        let ai = AdvancedAi::new();
        let rocketry_path: Vec<String> = game
            .rules
            .techs
            .keys()
            .filter(|tech| ai.tech_leads_to(&game, tech, "rocketry"))
            .cloned()
            .collect();
        for tech in rocketry_path
            .iter()
            .filter(|tech| tech.as_str() != "rocketry")
        {
            game.players[0].techs.insert(tech.clone());
        }
        game.turn = game.standard_duration(220);
        let plan = StrategicPlan {
            strategy: GrandStrategy::Diplomacy,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };

        assert!(ai.diplomatic_science_backup(&game, 0, &plan));
        ai.advanced_research(&mut game, 0, &plan);
        assert_eq!(game.players[0].research.as_deref(), Some("rocketry"));

        game.players[0].techs.insert("rocketry".to_string());
        game.players[0].research = None;
        ai.science_production(&mut game, 0);
        assert!(matches!(
            game.cities[&city].queue.first(),
            Some(Item::District { district, .. }) if district == "spaceport"
        ));

        game.victory_conditions.science = false;
        assert!(!ai.diplomatic_science_backup(&game, 0, &plan));
    }

    #[test]
    fn adaptive_research_routes_to_the_live_victory_plan() {
        let plan = |strategy| StrategicPlan {
            strategy,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: 1,
        };
        let ai = AdvancedAi::new();

        let mut science = Game::new_full(1, 20, 14, 762, 300, 0, false);
        ai.advanced_research(&mut science, 0, &plan(GrandStrategy::Science));
        assert_eq!(
            science.players[0].research.as_deref(),
            Some("animal_husbandry"),
            "the cheapest available prerequisite toward Rocketry wins"
        );

        let mut culture = Game::new_full(1, 20, 14, 763, 300, 0, false);
        ai.advanced_research(&mut culture, 0, &plan(GrandStrategy::Culture));
        assert_eq!(
            culture.players[0].research.as_deref(),
            Some("mining"),
            "the available prerequisite toward Printing wins"
        );

        let mut diplomacy = Game::new_full(1, 20, 14, 764, 300, 0, false);
        ai.advanced_research(&mut diplomacy, 0, &plan(GrandStrategy::Diplomacy));
        let tech = diplomacy.players[0].research.as_deref().unwrap();
        assert!(
            ai.tech_leads_to(&diplomacy, tech, "seasteads"),
            "diplomatic research must advance toward Seasteads' victory point"
        );
        let civic = diplomacy.players[0].civic.as_deref().unwrap();
        assert!(
            ai.civic_leads_to(&diplomacy, civic, "global_warming_mitigation"),
            "diplomatic culture must advance toward Global Warming Mitigation's victory point"
        );
    }

    #[test]
    fn religious_openings_fill_available_prophet_slots_with_stable_contenders() {
        let mut game = Game::new_full(4, 34, 20, 76_101, 300, 0, false);
        let mut capitals = Vec::new();
        for pid in 0..4 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
            capitals.push(game.player_city_ids(pid)[0]);
        }
        for (pid, capital) in capitals.into_iter().enumerate() {
            let anchor = game.cities[&capital].pos;
            found_nearby_test_city(&mut game, pid, anchor);
        }
        game.current = 0;
        game.turn = 60;

        let ai = AdvancedAi::new();
        let contenders: Vec<_> = (0..4)
            .filter(|pid| ai.religious_opening_viable(&game, *pid))
            .collect();
        assert_eq!(contenders.len(), game.max_religions().min(4));

        let founder = contenders[0];
        game.players[founder].religion = Some("Rival Faith".to_string());
        let counters = (0..4)
            .filter(|pid| ai.religious_opening_viable(&game, *pid))
            .count();
        assert_eq!(counters, (game.max_religions() - 1).min(3));
    }

    #[test]
    fn ordinary_religious_plan_routes_research_to_astrology() {
        let mut game = Game::new_full(1, 20, 14, 76_102, 120, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let plan = StrategicPlan {
            strategy: GrandStrategy::Religion,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };

        AdvancedAi::new().advanced_research(&mut game, 0, &plan);
        assert_eq!(game.players[0].research.as_deref(), Some("astrology"));
    }

    #[test]
    fn religious_production_builds_prophet_infrastructure_then_runs_prayers() {
        let mut game = Game::new_full(1, 20, 14, 76_103, 120, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        game.players[0].techs.insert("astrology".to_string());
        install_ai_test_district(&mut game, city, "holy_site");
        game.cities.get_mut(&city).unwrap().queue.clear();

        let ai = AdvancedAi::new();
        ai.religious_production(&mut game, 0);
        assert!(matches!(
            game.cities[&city].queue.first(),
            Some(Item::Building { building }) if building == "shrine"
        ));

        game.cities.get_mut(&city).unwrap().queue.clear();
        game.cities
            .get_mut(&city)
            .unwrap()
            .buildings
            .push("shrine".to_string());
        ai.religious_production(&mut game, 0);
        assert!(matches!(
            game.cities[&city].queue.first(),
            Some(Item::Project { project }) if project == "holy_site_prayers"
        ));
    }

    #[test]
    fn competitive_religious_opening_produces_multiple_founders() {
        let mut game = Game::new_full(4, 24, 16, 76_105, 110, 0, false);
        let mut ais = AdvancedAi::fleet(&game);
        run_game(&mut game, &mut ais);
        assert!(
            game.religions_founded() >= 2,
            "a stock Prophet race should not end with one uncontested founder: turn {}, {:?}",
            game.turn,
            game.players
                .iter()
                .take(4)
                .map(|player| (
                    &player.civ,
                    &player.religion,
                    player.prophet_pending,
                    player.gpp.get("prophet"),
                    player.techs.contains("astrology"),
                    game.player_city_ids(player.id)
                        .iter()
                        .filter(|city| game.cities[city].districts.contains_key("holy_site"))
                        .count(),
                ))
                .collect::<Vec<_>>()
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
    fn religious_match_point_interrupts_before_the_winning_conversion() {
        let mut game = Game::new_full(4, 42, 24, 7_621, 300, 0, false);
        for pid in 0..4 {
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
        game.turn = 150;
        game.players[3].religion = Some("Runaway Faith".to_string());
        for owner in 1..4 {
            let city = game.player_city_ids(owner)[0];
            game.cities
                .get_mut(&city)
                .unwrap()
                .pressure
                .insert("Runaway Faith".to_string(), 1_000.0);
        }

        let ai = AdvancedAi::new();
        let pressure = ai.rival_victory_pressure(&game, 3);
        assert_eq!(pressure.strategy, GrandStrategy::Religion);
        assert_eq!(pressure.progress, 75);
        assert_eq!(
            ai.victory_denial(&game, 0),
            Some((3, GrandStrategy::Conquest)),
            "a non-founder must attack before the fourth conversion ends the game"
        );
        let plan = ai.assess(&game, 0);
        assert_eq!(plan.strategy, GrandStrategy::Conquest);
        assert_eq!(plan.target_player, Some(3));

        game.players[0].religion = Some("Home Faith".to_string());
        let home = game.player_city_ids(0)[0];
        game.cities
            .get_mut(&home)
            .unwrap()
            .pressure
            .insert("Home Faith".to_string(), 1_000.0);
        assert_eq!(
            ai.victory_denial(&game, 0),
            Some((3, GrandStrategy::Religion)),
            "a founder should defend its cities with its own religion"
        );
    }

    #[test]
    fn religious_match_point_spends_the_reserve_only_in_own_faith_cities() {
        let mut game = Game::new_full(4, 42, 24, 7_622, 300, 0, false);
        for pid in 0..4 {
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.current = pid;
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        let converted_capital = game.player_city_ids(0)[0];
        let faithful_city = found_test_city(&mut game, 0);
        install_test_holy_site(&mut game, converted_capital);
        install_test_holy_site(&mut game, faithful_city);
        game.current = 0;
        game.turn = 150;
        game.players[0].religion = Some("Home Faith".to_string());
        game.players[0].holy_city = Some(converted_capital);
        game.players[0].techs.insert("astrology".to_string());
        game.players[0].faith = 200.0;
        game.players[3].religion = Some("Runaway Faith".to_string());
        game.cities
            .get_mut(&converted_capital)
            .unwrap()
            .pressure
            .insert("Runaway Faith".to_string(), 1_000.0);
        game.cities
            .get_mut(&faithful_city)
            .unwrap()
            .pressure
            .insert("Home Faith".to_string(), 1_000.0);
        for owner in 1..4 {
            let city = game.player_city_ids(owner)[0];
            game.cities
                .get_mut(&city)
                .unwrap()
                .pressure
                .insert("Runaway Faith".to_string(), 1_000.0);
        }

        let ai = AdvancedAi::new();
        let emergency = ai
            .victory_denial(&game, 0)
            .is_some_and(|(_, counter)| counter == GrandStrategy::Religion);
        assert!(emergency);
        ai.religious_spending(&mut game, 0, emergency);

        let missionary = game
            .units
            .values()
            .find(|unit| unit.owner == 0 && unit.kind == "missionary")
            .expect("match-point defense should spend the ordinary Faith reserve");
        assert_eq!(missionary.religion.as_deref(), Some("Home Faith"));
        assert_eq!(missionary.pos, game.cities[&faithful_city].pos);
        assert!(game.players[0].faith < 1.0);
    }

    /// The military answer to a religious offensive: step onto an adjacent
    /// enemy Missionary inside our own territory and condemn it.
    #[test]
    fn military_units_step_onto_and_condemn_enemy_missionaries() {
        let mut game = Game::new_full(2, 30, 18, 7_631, 200, 0, false);
        for pid in 0..2 {
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.current = pid;
            game.apply(pid, &Action::FoundCity { unit: settler }).unwrap();
        }
        game.current = 0;
        game.at_war.insert((0, 1));
        let home = game.cities[&game.player_city_ids(0)[0]].pos;
        // Both staged tiles must be free: the capital's own starting units
        // stand in this ring, and a friendly unit already there blocks the
        // step onto the intruder.
        let soldier_tile = game
            .nbrs(home)
            .into_iter()
            .find(|p| {
                game.units_at(*p).is_empty()
                    && game.map.get(*p).is_some_and(|t| {
                        game.rules.is_passable(t) && !game.rules.is_water(t)
                    })
            })
            .unwrap();
        // Condemning is a defense of our own territory, so the intruder has
        // to stand on a tile the capital actually owns - one that borders
        // both the soldier and the city centre.
        let intruder_tile = game
            .nbrs(soldier_tile)
            .into_iter()
            .find(|p| {
                *p != home
                    && game.nbrs(home).contains(p)
                    && game.units_at(*p).is_empty()
                    && game.map.get(*p).is_some_and(|t| {
                        game.rules.is_passable(t) && !game.rules.is_water(t)
                    })
            })
            .unwrap();
        for position in [soldier_tile, intruder_tile] {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.improvement = None;
            tile.hills = false;
        }
        game.map.set_river_edge(soldier_tile, intruder_tile, false);
        let soldier = game.spawn_test_unit("warrior", 0, soldier_tile);
        let missionary = game.spawn_test_unit("missionary", 1, intruder_tile);
        game.units.get_mut(&missionary).unwrap().religion = Some("Rival Faith".to_string());

        let mut ai = AdvancedAi::new();
        assert!(ai.condemn_step(&mut game, 0, soldier), "should engage");
        assert!(
            !game.units.contains_key(&missionary),
            "the adjacent missionary should be condemned, not ignored"
        );
    }

    #[test]
    fn non_founder_buys_adopted_faith_missionaries_to_defend_home() {
        let mut game = Game::new_full(3, 42, 24, 7_624, 300, 0, false);
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
        let converted_capital = game.player_city_ids(0)[0];
        let adopted_city = found_test_city(&mut game, 0);
        install_test_holy_site(&mut game, adopted_city);
        game.current = 0;
        game.turn = 150;
        // Player 0 founded no religion; a living rival's faith holds their
        // capital while a founderless neighbor faith holds the second city.
        game.players[0].techs.insert("astrology".to_string());
        game.players[0].faith = 600.0;
        game.players[1].religion = Some("Runaway Faith".to_string());
        game.cities
            .get_mut(&converted_capital)
            .unwrap()
            .pressure
            .insert("Runaway Faith".to_string(), 1_000.0);
        game.cities
            .get_mut(&adopted_city)
            .unwrap()
            .pressure
            .insert("Neighbor Faith".to_string(), 1_000.0);

        let ai = AdvancedAi::new();
        let threat = ai
            .home_conversion_threat(&game, 0)
            .expect("a rival majority in the capital is a home threat");
        assert_eq!(threat, "Runaway Faith");
        ai.religious_defense(&mut game, 0, &threat);

        let missionary = game
            .units
            .values()
            .find(|unit| unit.owner == 0 && unit.kind == "missionary")
            .expect("defense should buy a missionary of the adopted faith");
        assert_eq!(missionary.religion.as_deref(), Some("Neighbor Faith"));
        assert_eq!(missionary.pos, game.cities[&adopted_city].pos);
    }

    #[test]
    fn apostle_launches_inquisition_before_evangelizing_when_core_is_lost() {
        let mut game = Game::new_full(2, 30, 18, 7_623, 200, 0, false);
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
        let holy_city = game.player_city_ids(0)[0];
        let converted_city = found_test_city(&mut game, 0);
        game.current = 0;
        game.players[0].religion = Some("Home Faith".to_string());
        game.players[0].holy_city = Some(holy_city);
        game.players[0].religion_beliefs = vec!["work_ethic".to_string(), "tithe".to_string()];
        game.cities
            .get_mut(&holy_city)
            .unwrap()
            .pressure
            .insert("Home Faith".to_string(), 1_000.0);
        game.cities
            .get_mut(&converted_city)
            .unwrap()
            .pressure
            .insert("Rival Faith".to_string(), 1_000.0);
        let apostle = game.spawn_test_unit("apostle", 0, game.cities[&holy_city].pos);
        game.units.get_mut(&apostle).unwrap().religion = Some("Home Faith".to_string());

        assert!(AdvancedAi::new().advanced_religious_step(&mut game, 0, apostle, false));
        assert!(!game.units.contains_key(&apostle));
        assert_eq!(
            game.players[0]
                .counters
                .get("inquisition")
                .copied()
                .unwrap_or(0),
            1
        );
        assert_eq!(game.players[0].religion_beliefs.len(), 2);
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

        assert!(AdvancedAi::new().advanced_missionary_step(&mut game, 0, missionary, true));
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
    fn nonreligious_strategy_reinforces_its_founded_faith_before_conversion() {
        let mut game = Game::new_full(2, 30, 18, 7_634, 200, 0, false);
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
        game.cities.get_mut(&home).unwrap().pressure.extend([
            ("Our Faith".to_string(), 1_000.0),
            ("Rival Faith".to_string(), 600.0),
        ]);
        assert_eq!(game.city_religion(&game.cities[&home]), Some("Our Faith"));
        let missionary = game.spawn_test_unit("missionary", 0, game.cities[&home].pos);
        game.units.get_mut(&missionary).unwrap().religion = Some("Our Faith".to_string());
        let before = game.cities[&home].pressure["Our Faith"];

        assert!(AdvancedAi::targeting(VictoryTarget::Science)
            .advanced_missionary_step(&mut game, 0, missionary, false));
        assert!(game.cities[&home].pressure["Our Faith"] > before);
        assert_eq!(game.units[&missionary].charges, 2);
        game.units.get_mut(&missionary).unwrap().moves_left = 4.0;
        assert!(
            !AdvancedAi::targeting(VictoryTarget::Science)
                .advanced_missionary_step(&mut game, 0, missionary, false),
            "a defensive unit must hold once its home is safe instead of starting a foreign crusade"
        );
    }

    #[test]
    fn missionary_routes_to_spread_range_around_a_mountain_detour() {
        let mut game = Game::new_full(2, 30, 18, 7_633, 200, 0, false);
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
        let target_city = game.player_city_ids(1)[0];
        let target = game.cities[&target_city].pos;
        game.cities
            .get_mut(&target_city)
            .unwrap()
            .pressure
            .insert("Rival Faith".to_string(), 1_000.0);

        let start = (target.0 - 3, target.1);
        let direct = (target.0 - 2, target.1);
        let detour = (target.0 - 2, target.1 - 1);
        let onward = (target.0 - 1, target.1 - 1);
        for position in [start, direct, detour, onward] {
            assert!(game.map.tiles.contains_key(&position));
        }
        game.map.tiles.get_mut(&direct).unwrap().terrain = "mountain".to_string();
        for position in [start, detour, onward] {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "grassland".to_string();
            tile.feature = None;
        }

        let missionary = game.spawn_test_unit("missionary", 0, start);
        game.units.get_mut(&missionary).unwrap().religion = Some("Our Faith".to_string());
        let ai = AdvancedAi::new();
        assert!(ai.advanced_missionary_step(&mut game, 0, missionary, true));
        assert_eq!(
            game.wdist(game.units[&missionary].pos, target),
            3,
            "the first legal route step must accept a sideways mountain detour"
        );

        for _ in 0..8 {
            if !game.units.contains_key(&missionary) || game.units[&missionary].charges < 3 {
                break;
            }
            game.units.get_mut(&missionary).unwrap().moves_left = 4.0;
            assert!(ai.advanced_missionary_step(&mut game, 0, missionary, true));
        }
        assert!(
            !game.units.contains_key(&missionary) || game.units[&missionary].charges < 3,
            "a reachable foreign city must receive a spread instead of trapping the unit"
        );
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
        assert!(ai.advanced_religious_step(&mut game, 0, first, false));
        assert!(game.players[0]
            .religion_beliefs
            .contains(&"wat".to_string()));

        let second = game.spawn_test_unit("apostle", 0, game.cities[&city].pos);
        game.units.get_mut(&second).unwrap().religion = Some("Planned Faith".to_string());
        assert!(ai.advanced_religious_step(&mut game, 0, second, false));
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
    fn science_target_parallelizes_lasers_across_cities_without_local_spaceport_spam() {
        let mut game = Game::new_full(1, 34, 20, 71_002, 320, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| game.units[uid].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let mut cities = game.player_city_ids(0);
        while cities.len() < 3 {
            let center = game
                .map
                .tiles
                .iter()
                .find(|(position, tile)| {
                    tile.owner_city.is_none()
                        && game.rules.is_passable(tile)
                        && !game.rules.is_water(tile)
                        && cities
                            .iter()
                            .all(|city| game.wdist(**position, game.cities[city].pos) >= 7)
                })
                .map(|(position, _)| *position)
                .unwrap();
            game.found_city_for(0, center, None);
            cities = game.player_city_ids(0);
        }
        game.players[0].techs = game.rules.techs.keys().cloned().collect();
        game.players[0].civics = game.rules.civics.keys().cloned().collect();
        game.players[0].science_projects.extend([
            "launch_earth_satellite".to_string(),
            "launch_moon_landing".to_string(),
            "launch_mars_colony".to_string(),
            "exoplanet_expedition".to_string(),
        ]);
        for city in &cities {
            game.cities.get_mut(city).unwrap().pop = 12;
            for position in game.cities[city].owned_tiles.clone() {
                if position == game.cities[city].pos {
                    continue;
                }
                let tile = game.map.tiles.get_mut(&position).unwrap();
                tile.terrain = "plains".to_string();
                tile.feature = None;
                tile.hills = false;
                tile.resource = None;
                tile.improvement = None;
                tile.district = None;
                tile.district_foundation = None;
                tile.wonder = None;
            }
        }
        for city in cities.iter().take(2) {
            let position = game.cities[city]
                .owned_tiles
                .iter()
                .copied()
                .find(|position| *position != game.cities[city].pos)
                .unwrap();
            game.map.tiles.get_mut(&position).unwrap().district = Some("spaceport".to_string());
            game.cities
                .get_mut(city)
                .unwrap()
                .districts
                .insert("spaceport".to_string(), position);
        }
        game.cities.get_mut(&cities[0]).unwrap().queue = vec![Item::Project {
            project: "lagrange_laser_station".to_string(),
        }];

        let ai = AdvancedAi::targeting(VictoryTarget::Science);
        ai.science_production(&mut game, 0);
        assert_eq!(
            cities
                .iter()
                .filter(|city| matches!(
                    game.cities[city].queue.first(),
                    Some(Item::Project { project }) if project == "lagrange_laser_station"
                ))
                .count(),
            2
        );

        ai.science_production(&mut game, 0);
        assert!(matches!(
            game.cities[&cities[2]].queue.first(),
            Some(Item::District { district, .. }) if district == "spaceport"
        ));

        let duplicate = Item::District {
            district: "spaceport".to_string(),
            pos: game.district_sites(cities[0], "spaceport")[0],
        };
        let plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        assert!(
            ai.production_value(&game, 0, cities[0], &duplicate, &plan, &ai.counts(&game, 0))
                <= -10_000.0
        );
    }

    #[test]
    fn district_search_values_unique_families_and_real_housing_need() {
        let mut game = Game::new_full(1, 20, 14, 71_001, 200, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let site = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != game.cities[&city].pos)
            .unwrap();
        {
            let tile = game.map.tiles.get_mut(&site).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.hills = true;
        }

        for (unique, family) in [
            ("seowon", "campus"),
            ("lavra", "holy_site"),
            ("hansa", "industrial_zone"),
            ("bath", "aqueduct"),
            ("mbanza", "neighborhood"),
        ] {
            assert_eq!(game.district_family(unique), family);
        }

        let ai = AdvancedAi::targeting(VictoryTarget::Science);
        let counts = ai.counts(&game, 0);
        let mut plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let seowon = Item::District {
            district: "seowon".to_string(),
            pos: site,
        };
        let science_value = ai.production_value(&game, 0, city, &seowon, &plan, &counts);
        plan.strategy = GrandStrategy::Expansion;
        let expansion_value = ai.production_value(&game, 0, city, &seowon, &plan, &counts);
        assert!(
            science_value > expansion_value,
            "a Seowon must inherit the Campus science strategy bonus"
        );

        // The rules engine, not a second AI-only district cap, decides
        // eligibility. A high-population city with an undeveloped specialty
        // core should still recognize the urgent housing value of a
        // Neighborhood.
        for district in ["campus", "holy_site", "commercial_hub"] {
            game.cities
                .get_mut(&city)
                .unwrap()
                .districts
                .insert(district.to_string(), site);
        }
        game.cities.get_mut(&city).unwrap().pop = 12;
        let crowded = Item::District {
            district: "neighborhood".to_string(),
            pos: site,
        };
        let crowded_value = ai.production_value(&game, 0, city, &crowded, &plan, &counts);
        game.cities.get_mut(&city).unwrap().pop = 2;
        let roomy_value = ai.production_value(&game, 0, city, &crowded, &plan, &counts);
        assert!(crowded_value > -1_000.0);
        assert!(
            crowded_value > roomy_value,
            "appeal housing must be worth more when growth is constrained"
        );
    }

    #[test]
    fn production_search_uses_incremental_remaining_cost_for_paused_builds() {
        let mut game = Game::new_full(1, 20, 14, 71_002, 200, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.players[0].civ = "Egypt".to_string();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let monument = Item::Building {
            building: "monument".to_string(),
        };
        let builder = Item::Unit {
            unit: "builder".to_string(),
        };
        let plan = StrategicPlan {
            strategy: GrandStrategy::Expansion,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::new();
        let counts = ai.counts(&game, 0);
        let fresh = ai.production_value(&game, 0, city, &monument, &plan, &counts);

        game.apply(
            0,
            &Action::Produce {
                city,
                item: monument.clone(),
            },
        )
        .unwrap();
        game.cities.get_mut(&city).unwrap().production = 20.0;
        game.apply(
            0,
            &Action::Produce {
                city,
                item: builder,
            },
        )
        .unwrap();
        let resumed = ai.production_value(&game, 0, city, &monument, &plan, &counts);

        assert_eq!(
            game.item_remaining_cost_for_city(0, city, &monument),
            game.item_cost_for_city(0, city, &monument) - 20.0
        );
        assert!(
            resumed > fresh,
            "incremental evaluation should prefer finishing invested infrastructure"
        );
    }

    #[test]
    fn military_production_keeps_land_sea_and_air_force_gaps_separate() {
        let mut game = Game::new_full(1, 20, 14, 71_003, 120, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        for unit in game.player_unit_ids(0) {
            if game.rules.units[game.units[&unit].kind.as_str()].class == "military" {
                game.remove_unit(unit);
            }
        }
        let water = game
            .map
            .tiles
            .iter()
            .find(|(_, tile)| game.rules.is_water(tile))
            .map(|(position, _)| *position)
            .unwrap();
        game.spawn_test_unit("galley", 0, water);
        let city = game.player_city_ids(0)[0];
        let plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 1,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::targeting(VictoryTarget::Science);
        let counts = ai.counts(&game, 0);
        assert_eq!(counts.naval, 1);
        assert_eq!(counts.military - counts.naval - counts.aircraft, 0);

        let defender = Item::Unit {
            unit: "warrior".to_string(),
        };
        assert!(
            ai.production_value(&game, 0, city, &defender, &plan, &counts) > 0.0,
            "a Galley cannot satisfy the empire's missing land-defense quota"
        );
    }

    #[test]
    fn adaptive_conquest_turn_uses_the_live_plan_for_city_production() {
        let mut game = Game::new_full(1, 20, 14, 71_006, 120, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        for unit in game.player_unit_ids(0) {
            if game.rules.units[game.units[&unit].kind.as_str()].class == "military" {
                game.remove_unit(unit);
            }
        }
        let city = game.player_city_ids(0)[0];
        game.cities.get_mut(&city).unwrap().queue.clear();
        install_ai_test_district(&mut game, city, "campus");
        game.players[0].techs.insert("writing".to_string());
        game.apply(
            0,
            &Action::Produce {
                city,
                item: Item::Project {
                    project: "campus_research_grants".to_string(),
                },
            },
        )
        .unwrap();
        let mut ai = AdvancedAi::new();
        ai.base.book_pos = 4;
        ai.plan = Some(StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 1,
            assessed_turn: game.turn,
        });

        ai.take_turn(&mut game, 0);

        assert!(matches!(
            game.cities[&city].queue.first(),
            Some(Item::Unit { unit }) if game.rules.units[unit].class == "military"
        ));
    }

    #[test]
    fn adaptive_production_reserves_a_real_siege_support_capability() {
        let mut game = Game::new_full(2, 24, 16, 71_007, 160, 0, false);
        for pid in 0..2 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
        let home = game.player_city_ids(0)[0];
        let target = game.player_city_ids(1)[0];
        game.cities.get_mut(&home).unwrap().queue.clear();
        game.players[0].techs.insert("flight".to_string());
        let position = game.cities[&home].pos;
        game.spawn_test_unit("catapult", 0, position);
        game.spawn_test_unit("warrior", 0, position);
        game.spawn_test_unit("warrior", 0, position);
        game.at_war.insert((0, 1));
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: Some(target),
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let mut ai = AdvancedAi::new();
        ai.base.book_pos = 4;

        ai.advanced_support_production(&mut game, 0, &plan);

        assert!(matches!(
            game.cities[&home].queue.first(),
            Some(Item::Unit { unit }) if unit == "observation_balloon"
        ));
    }

    #[test]
    fn support_search_respects_ram_and_tower_wall_eras() {
        let mut game = Game::new_full(2, 24, 16, 71_008, 160, 0, false);
        for pid in 0..2 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
        let home = game.player_city_ids(0)[0];
        let target = game.player_city_ids(1)[0];
        let position = game.cities[&home].pos;
        game.spawn_test_unit("warrior", 0, position);
        game.spawn_test_unit("warrior", 0, position);
        game.spawn_test_unit("warrior", 0, position);
        game.cities.get_mut(&target).unwrap().buildings =
            vec!["walls".to_string(), "medieval_walls".to_string()];
        game.cities.get_mut(&target).unwrap().wall_hp = 200;
        game.at_war.insert((0, 1));
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: Some(target),
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::targeting(VictoryTarget::Domination);
        let counts = ai.counts(&game, 0);

        assert!(ai.support_unit_value(&game, 0, home, "battering_ram", &plan, &counts) < -9_000.0);
        assert!(ai.support_unit_value(&game, 0, home, "siege_tower", &plan, &counts) > 0.0);
    }

    #[test]
    fn support_search_builds_air_defense_only_for_a_real_hostile_air_threat() {
        let mut game = Game::new_full(2, 24, 16, 71_011, 160, 0, false);
        for pid in 0..2 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
        game.players[0]
            .techs
            .insert("advanced_ballistics".to_string());
        game.at_war.insert((0, 1));
        let city = game.player_city_ids(0)[0];
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: game.player_city_ids(1).first().copied(),
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let mut ai = AdvancedAi::targeting(VictoryTarget::Domination);
        ai.base.book_pos = 4;
        let item = Item::Unit {
            unit: "anti_air_gun".to_string(),
        };
        let counts = ai.counts(&game, 0);
        assert_eq!(counts.air_defense, 0);
        assert!(ai.production_value(&game, 0, city, &item, &plan, &counts) < -9_000.0);

        let hostile_base = game.cities[&game.player_city_ids(1)[0]].pos;
        game.spawn_test_unit("bomber", 1, hostile_base);
        let counts = ai.counts(&game, 0);
        assert!(ai.production_value(&game, 0, city, &item, &plan, &counts) > 0.0);
        ai.advanced_support_production(&mut game, 0, &plan);
        assert!(matches!(
            game.cities[&city].queue.first(),
            Some(Item::Unit { unit }) if unit == "anti_air_gun"
        ));

        game.cities.get_mut(&city).unwrap().queue.clear();
        let city_pos = game.cities[&city].pos;
        game.spawn_test_unit("anti_air_gun", 0, city_pos);
        let counts = ai.counts(&game, 0);
        assert_eq!(counts.air_defense, 1);
        assert!(ai.production_value(&game, 0, city, &item, &plan, &counts) < -9_000.0);
    }

    #[test]
    fn force_readiness_excludes_aircraft_from_ground_armies() {
        let mut game = Game::new_full(2, 24, 16, 71_004, 120, 0, false);
        game.at_war.insert((0, 1));
        let staging = game
            .map
            .tiles
            .iter()
            .find(|(position, tile)| {
                game.rules.is_passable(tile)
                    && !game.rules.is_water(tile)
                    && game.units_at(**position).is_empty()
            })
            .map(|(position, _)| *position)
            .unwrap();
        let warrior = game.spawn_test_unit("warrior", 0, staging);
        let bomber = game.spawn_test_unit("bomber", 0, staging);
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let mut ai = AdvancedAi::targeting(VictoryTarget::Domination);

        ai.rebuild_force_groups(&game, 0, &plan);

        let army = ai
            .force_groups
            .iter()
            .find(|group| group.units.contains(&warrior))
            .expect("the ground unit forms an army order");
        assert!(!army.units.contains(&bomber));
        assert!(ai.force_groups.iter().all(|group| {
            group.units.iter().all(|unit| {
                game.rules.units[game.units[unit].kind.as_str()]
                    .domain
                    .as_deref()
                    != Some("air")
            })
        }));
    }

    #[test]
    fn local_superiority_prices_the_objective_city_defense() {
        let mut game = Game::new_full(2, 24, 16, 71_006, 120, 0, false);
        game.current = 1;
        let settler = game
            .player_unit_ids(1)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(1, &Action::FoundCity { unit: settler }).unwrap();
        let target_city = game.player_city_ids(1)[0];
        for unit in game.player_unit_ids(1) {
            game.remove_unit(unit);
        }
        game.players[1]
            .counters
            .insert("strongest_unit_built".to_string(), 80);
        let city_pos = {
            let city = game.cities.get_mut(&target_city).unwrap();
            city.buildings.push("walls".to_string());
            city.wall_hp = 100;
            city.pos
        };
        let staging =
            game.nbrs(city_pos)
                .into_iter()
                .find(|position| {
                    game.map.get(*position).is_some_and(|tile| {
                        game.rules.is_passable(tile) && !game.rules.is_water(tile)
                    }) && game.units_at(*position).is_empty()
                })
                .unwrap();
        let warrior = game.spawn_test_unit("warrior", 0, staging);
        game.at_war.insert((0, 1));
        let mut ai = AdvancedAi::targeting(VictoryTarget::Domination);

        let ratio = ai.local_strength_ratio(&game, &[warrior], &[1], game.cities[&target_city].pos);

        assert!(
            ratio < 0.72,
            "one Warrior must not claim superiority over an intact defended city: {ratio}"
        );
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: Some(target_city),
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        ai.rebuild_force_groups(&game, 0, &plan);
        let order = ai
            .force_groups
            .iter()
            .find(|group| group.units.contains(&warrior))
            .unwrap();
        assert_eq!(order.posture, ForcePosture::Hold);
        assert_eq!(
            match order.posture {
                ForcePosture::Muster | ForcePosture::Hold | ForcePosture::Recover => order.anchor,
                ForcePosture::Engage => order.focus_target.unwrap_or(order.objective),
                ForcePosture::Advance => order.objective,
            },
            order.anchor,
            "an inferior force must hold its formation rather than target the city"
        );

        game.cities.get_mut(&target_city).unwrap().wall_hp = 0;
        game.cities.get_mut(&target_city).unwrap().hp = 1;
        ai.rebuild_force_groups(&game, 0, &plan);
        assert_eq!(
            ai.force_groups
                .iter()
                .find(|group| group.units.contains(&warrior))
                .unwrap()
                .posture,
            ForcePosture::Engage,
            "a forcing city capture must override the otherwise inferior local ratio"
        );
    }

    #[test]
    fn bomber_exact_result_search_prefers_a_kill_over_static_strength() {
        let mut game = Game::new_full(2, 24, 16, 71_005, 120, 0, false);
        game.at_war.insert((0, 1));
        for unit in game.player_unit_ids(0) {
            game.remove_unit(unit);
        }
        for unit in game.player_unit_ids(1) {
            game.remove_unit(unit);
        }
        let base = game
            .map
            .tiles
            .iter()
            .find(|(position, tile)| {
                game.rules.is_passable(tile)
                    && !game.rules.is_water(tile)
                    && game.city_at(**position).is_none()
            })
            .map(|(position, _)| *position)
            .unwrap();
        let mut targets: Vec<Pos> = game
            .wdisk(base, game.rules.units["bomber"].range)
            .into_iter()
            .filter(|position| {
                *position != base
                    && game.map.get(*position).is_some_and(|tile| {
                        game.rules.is_passable(tile) && !game.rules.is_water(tile)
                    })
                    && game.city_at(*position).is_none()
            })
            .take(2)
            .collect();
        assert_eq!(targets.len(), 2);
        targets.sort_unstable();
        let bomber = game.spawn_test_unit("bomber", 0, base);
        game.spawn_test_unit("modern_armor", 1, targets[0]);
        let warrior = game.spawn_test_unit("warrior", 1, targets[1]);
        game.units.get_mut(&warrior).unwrap().hp = 1;
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::targeting(VictoryTarget::Domination);

        assert_eq!(
            ai.advanced_air_action(&game, 0, bomber, &plan),
            Some(Action::AirStrike {
                unit: bomber,
                target: targets[1],
            })
        );
    }

    #[test]
    fn bomber_planners_choose_high_value_air_pillage_over_low_value_strikes() {
        let mut game = Game::new_full(2, 24, 16, 71_006, 120, 0, false);
        game.at_war.insert((0, 1));
        for unit in game.units.keys().copied().collect::<Vec<_>>() {
            game.remove_unit(unit);
        }
        let enemy_center = game
            .map
            .tiles
            .iter()
            .find(|(_, tile)| game.rules.is_passable(tile) && !game.rules.is_water(tile))
            .map(|(position, _)| *position)
            .unwrap();
        let enemy_city = game.found_city_for(1, enemy_center, None);
        let target = game
            .nbrs(enemy_center)
            .into_iter()
            .find(|position| {
                game.map
                    .get(*position)
                    .is_some_and(|tile| game.rules.is_passable(tile) && !game.rules.is_water(tile))
            })
            .unwrap();
        {
            let tile = game.map.tiles.get_mut(&target).unwrap();
            tile.owner_city = Some(enemy_city);
            tile.improvement = Some("airstrip".to_string());
            tile.pillaged = false;
        }
        let base = game
            .wdisk(target, game.rules.units["bomber"].range)
            .into_iter()
            .find(|position| {
                *position != target
                    && *position != enemy_center
                    && game.wdist(*position, target) >= 3
                    && game.map.get(*position).is_some_and(|tile| {
                        game.rules.is_passable(tile) && !game.rules.is_water(tile)
                    })
            })
            .unwrap();
        game.found_city_for(0, base, None);
        let bomber = game.spawn_test_unit("bomber", 0, base);
        let expected = Action::AirPillage {
            unit: bomber,
            target,
        };
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: Some(enemy_city),
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };

        assert_eq!(
            BasicAi::new().doctrine_action(&game, 0, bomber),
            Some(expected.clone())
        );
        assert_eq!(
            AdvancedAi::targeting(VictoryTarget::Domination)
                .advanced_air_action(&game, 0, bomber, &plan),
            Some(expected)
        );
    }

    #[test]
    fn jet_planners_priority_target_escorted_air_defenses() {
        let mut game = Game::new_full(2, 24, 16, 71_007, 120, 0, false);
        game.at_war.insert((0, 1));
        for unit in game.units.keys().copied().collect::<Vec<_>>() {
            game.remove_unit(unit);
        }
        let base = game
            .map
            .tiles
            .iter()
            .find(|(_, tile)| game.rules.is_passable(tile) && !game.rules.is_water(tile))
            .map(|(position, _)| *position)
            .unwrap();
        let target = game
            .wdisk(base, game.rules.units["jet_bomber"].range)
            .into_iter()
            .find(|position| {
                *position != base
                    && game.map.get(*position).is_some_and(|tile| {
                        game.rules.is_passable(tile) && !game.rules.is_water(tile)
                    })
            })
            .unwrap();
        game.found_city_for(0, base, None);
        game.spawn_test_unit("modern_armor", 1, target);
        game.spawn_test_unit("mobile_sam", 1, target);
        let jet = game.spawn_test_unit("jet_bomber", 0, base);
        let expected = Action::PriorityTarget { unit: jet, target };
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };

        assert_eq!(
            BasicAi::new().doctrine_action(&game, 0, jet),
            Some(expected.clone())
        );
        assert_eq!(
            AdvancedAi::targeting(VictoryTarget::Domination)
                .advanced_air_action(&game, 0, jet, &plan),
            Some(expected)
        );
    }

    #[test]
    fn exact_ground_search_prefers_the_high_value_kill_over_a_static_tie() {
        let mut game = Game::new_full(2, 24, 16, 71_009, 120, 0, false);
        game.at_war.insert((0, 1));
        let rival_origin = game
            .player_unit_ids(1)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .map(|unit| game.units[&unit].pos)
            .unwrap();
        game.found_city_for(1, rival_origin, None);
        for unit in game.units.keys().copied().collect::<Vec<_>>() {
            game.remove_unit(unit);
        }
        let (base, mut targets) = game
            .map
            .tiles
            .iter()
            .filter(|(position, tile)| {
                game.rules.is_passable(tile)
                    && !game.rules.is_water(tile)
                    && game.city_at(**position).is_none()
                    && game
                        .cities
                        .values()
                        .all(|city| game.wdist(**position, city.pos) > 5)
            })
            .find_map(|(base, _)| {
                let targets: Vec<Pos> = game
                    .nbrs(*base)
                    .into_iter()
                    .filter(|position| {
                        game.map.get(*position).is_some_and(|tile| {
                            game.rules.is_passable(tile) && !game.rules.is_water(tile)
                        }) && game.city_at(*position).is_none()
                    })
                    .collect();
                (targets.len() >= 2).then_some((*base, targets))
            })
            .expect("test map has an isolated two-target engagement");
        targets.sort_unstable();
        let robot = game.spawn_test_unit("giant_death_robot", 0, base);
        let warrior = game.spawn_test_unit("warrior", 1, targets[0]);
        let armor = game.spawn_test_unit("modern_armor", 1, targets[1]);
        game.units.get_mut(&warrior).unwrap().hp = 1;
        game.units.get_mut(&armor).unwrap().hp = 1;
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let mut ai = AdvancedAi::targeting(VictoryTarget::Domination);

        assert_eq!(
            ai.base.exchange_score(&game, robot, targets[0], true),
            ai.base.exchange_score(&game, robot, targets[1], true),
            "the static evaluator intentionally sees two one-hit kills"
        );
        let warrior_value = ai.tactical_attack_value(
            &game,
            0,
            robot,
            &Action::Ranged {
                unit: robot,
                target: targets[0],
            },
            &plan,
        );
        let armor_value = ai.tactical_attack_value(
            &game,
            0,
            robot,
            &Action::Ranged {
                unit: robot,
                target: targets[1],
            },
            &plan,
        );
        assert!(armor_value > warrior_value + 100.0);
        assert!(ai.advanced_military_step(&mut game, 0, robot, &plan));
        assert!(!game.units.contains_key(&armor));
        assert!(game.units.contains_key(&warrior));
        assert!(matches!(
            game.log.last(),
            Some((0, Action::Attack { target, .. } | Action::Ranged { target, .. }))
                if *target == targets[1]
        ));
    }

    #[test]
    fn army_declines_a_captured_settler_when_no_city_site_remains() {
        let mut game = Game::new_full(2, 20, 14, 71_019, 120, 0, false);
        for pid in 0..2 {
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.found_city_for(pid, game.units[&settler].pos, None);
            game.remove_unit(settler);
        }
        for unit in game.units.keys().copied().collect::<Vec<_>>() {
            game.remove_unit(unit);
        }
        let home = game.cities[&game.player_city_ids(0)[0]].pos;
        let origin = game.nbrs(home)[0];
        let target = game
            .nbrs(origin)
            .into_iter()
            .find(|position| *position != home && game.wdist(home, *position) <= 2)
            .unwrap();
        let city_centers: BTreeSet<Pos> =
            game.cities.values().map(|city| city.pos).collect();
        for (position, tile) in &mut game.map.tiles {
            if ![home, origin, target].contains(&position)
                && !city_centers.contains(&position)
            {
                tile.terrain = "ocean".to_string();
                tile.feature = None;
            }
        }
        for position in [origin, target] {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
        }
        game.at_war.insert((0, 1));
        let warrior = game.spawn_test_unit("warrior", 0, origin);
        let settler = game.spawn_test_unit("settler", 1, target);
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: game.player_city_ids(1).first().copied(),
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: game.turn,
        };
        let mut ai = AdvancedAi::new();
        assert!(!ai.base.has_practical_settle_site(&game, 0));
        let mut direct_capture = game.clone();
        let capture_result = direct_capture.apply(0, &Action::Move { unit: warrior, to: target });
        assert!(capture_result.is_ok(), "staged capture was not legal: {capture_result:?}");
        assert_eq!(direct_capture.units[&settler].owner, 0);

        let _ = ai.advanced_military_step(&mut game, 0, warrior, &plan);

        assert_eq!(
            game.units.get(&settler).map(|unit| unit.owner),
            Some(1),
            "capturing the civilian would create a settler with no legal city site"
        );

        game.remove_unit(warrior);
        let scout = game.spawn_test_unit("scout", 0, origin);
        let _ = ai.advanced_military_step(&mut game, 0, scout, &plan);
        assert_eq!(
            game.units.get(&settler).map(|unit| unit.owner),
            Some(1),
            "a recon fallback must not bypass the unwanted-settler guard"
        );
    }

    #[test]
    fn exact_hybrid_search_uses_melee_to_finish_a_city() {
        let mut game = Game::new_full(2, 24, 16, 71_010, 120, 0, false);
        let rival_origin = game
            .player_unit_ids(1)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .map(|unit| game.units[&unit].pos)
            .unwrap();
        let city = game.found_city_for(1, rival_origin, None);
        for unit in game.units.keys().copied().collect::<Vec<_>>() {
            game.remove_unit(unit);
        }
        let target = game.cities[&city].pos;
        let staging =
            game.nbrs(target)
                .into_iter()
                .find(|position| {
                    game.map.get(*position).is_some_and(|tile| {
                        game.rules.is_passable(tile) && !game.rules.is_water(tile)
                    }) && game.units_at(*position).is_empty()
                })
                .unwrap();
        game.cities.get_mut(&city).unwrap().hp = 0;
        game.cities.get_mut(&city).unwrap().wall_hp = 0;
        let robot = game.spawn_test_unit("giant_death_robot", 0, staging);
        game.at_war.insert((0, 1));
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: Some(city),
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let mut ai = AdvancedAi::targeting(VictoryTarget::Domination);

        assert!(ai.advanced_military_step(&mut game, 0, robot, &plan));
        assert_eq!(game.cities[&city].owner, 0);
        assert!(matches!(
            game.log.last(),
            Some((0, Action::Attack { unit, target: action_target }))
                if *unit == robot && *action_target == target
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
        install_ai_test_district(&mut game, city, "industrial_zone");
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
    fn district_project_search_extends_only_concrete_great_person_races() {
        let mut game = Game::new(2, 24, 16, 7_103, 200, 0);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let campus = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != game.cities[&city].pos)
            .unwrap();
        game.map.tiles.get_mut(&campus).unwrap().district = Some("campus".to_string());
        game.cities
            .get_mut(&city)
            .unwrap()
            .districts
            .insert("campus".to_string(), campus);
        game.players[0].techs.insert("writing".to_string());

        let project = Item::Project {
            project: "campus_research_grants".to_string(),
        };
        let library = Item::Building {
            building: "library".to_string(),
        };
        assert!(game.can_produce(0, city, &project));
        assert!(game.can_produce(0, city, &library));
        let plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::targeting(VictoryTarget::Science);
        let counts = ai.counts(&game, 0);
        let undeveloped = ai.production_value(&game, 0, city, &project, &plan, &counts);
        let first_building = ai.production_value(&game, 0, city, &library, &plan, &counts);
        assert!(
            first_building > undeveloped,
            "the first Campus building must precede a quiet project: {first_building} <= {undeveloped}"
        );

        game.cities
            .get_mut(&city)
            .unwrap()
            .buildings
            .push("library".to_string());
        let far = ai.production_value(&game, 0, city, &project, &plan, &counts);
        let award =
            game.project_completion_gpp_awards(0, city, "campus_research_grants")["scientist"];
        let cost = game.gp_cost(0, "scientist");
        game.players[0]
            .gpp
            .insert("scientist".to_string(), cost - award);
        game.players[1]
            .gpp
            .insert("scientist".to_string(), cost - award * 0.5);
        let forcing = ai.production_value(&game, 0, city, &project, &plan, &counts);
        assert!(
            forcing > far + 100.0,
            "a project that claims and overtakes in the live race must receive an extension: {forcing} <= {far}"
        );

        game.active_congress_effects
            .push(crate::game::CongressEffect {
                resolution: "patronage".to_string(),
                outcome: "B".to_string(),
                target: "scientist".to_string(),
                expires: game.turn + 30,
            });
        let disabled = ai.production_value(&game, 0, city, &project, &plan, &counts);
        assert!(
            disabled < far,
            "a Congress-disabled class must not retain Great Person race value: {disabled} >= {far}"
        );
    }

    #[test]
    fn bread_and_circuses_value_tracks_real_loyalty_need() {
        let mut game = Game::new(1, 20, 14, 7_105, 160, 0);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let district = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != game.cities[&city].pos)
            .unwrap();
        game.map.tiles.get_mut(&district).unwrap().district =
            Some("entertainment_complex".to_string());
        game.cities
            .get_mut(&city)
            .unwrap()
            .districts
            .insert("entertainment_complex".to_string(), district);
        game.cities
            .get_mut(&city)
            .unwrap()
            .buildings
            .push("arena".to_string());

        let plan = StrategicPlan {
            strategy: GrandStrategy::Expansion,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::new();
        let safe = ai.district_project_value(&game, 0, city, "bread_and_circuses", &plan);
        game.cities.get_mut(&city).unwrap().loyalty = 50.0;
        let pressured = ai.district_project_value(&game, 0, city, "bread_and_circuses", &plan);
        assert!(
            pressured > safe + 700.0,
            "loyalty recovery must transform Bread and Circuses from quiet to forcing"
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
        let city = game.player_city_ids(0)[0];
        let campus = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != game.cities[&city].pos)
            .unwrap();
        game.map.tiles.get_mut(&campus).unwrap().district = Some("campus".to_string());
        game.cities
            .get_mut(&city)
            .unwrap()
            .districts
            .insert("campus".to_string(), campus);
        let cost = game.gp_cost(0, "scientist");
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
        assert_eq!(game.players[0].gold, 225.0);
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
        install_ai_test_district(&mut game, city, "theater_square");
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
    fn religious_spending_uses_own_faith_inquisitors_without_funding_a_rival() {
        let mut game = Game::new(2, 24, 16, 7_104, 200, 0);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        install_ai_test_district(&mut game, city, "holy_site");
        game.cities.get_mut(&city).unwrap().buildings =
            vec!["shrine".to_string(), "temple".to_string()];
        game.players[0].civics.insert("theology".to_string());
        game.players[0].religion = Some("Our Faith".to_string());
        game.players[0]
            .counters
            .insert("inquisition".to_string(), 1);
        game.players[0].faith = 1_000.0;
        game.cities.get_mut(&city).unwrap().pressure.extend([
            ("Our Faith".to_string(), 1_000.0),
            ("Rival Faith".to_string(), 600.0),
        ]);

        let ai = AdvancedAi::new();
        let mut converted = game.clone();
        converted
            .cities
            .get_mut(&city)
            .unwrap()
            .pressure
            .insert("Rival Faith".to_string(), 2_000.0);
        let converted_units = converted.player_unit_ids(0).len();
        ai.religious_spending(&mut converted, 0, true);
        assert_eq!(converted.player_unit_ids(0).len(), converted_units);
        assert_eq!(converted.players[0].faith, 1_000.0);

        let before_units = game.player_unit_ids(0).len();
        ai.religious_spending(&mut game, 0, true);
        assert_eq!(game.player_unit_ids(0).len(), before_units + 1);
        let inquisitor = game
            .units
            .values()
            .find(|unit| unit.owner == 0 && unit.kind == "inquisitor")
            .unwrap();
        assert_eq!(inquisitor.religion.as_deref(), Some("Our Faith"));
        assert!(game.players[0].faith < 1_000.0);
    }

    #[test]
    fn religious_spending_stops_at_a_target_scaled_unit_ceiling() {
        let mut game = Game::new_full(2, 30, 18, 7_105, 200, 0, false);
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
        let home = game.player_city_ids(0)[0];
        let target = game.player_city_ids(1)[0];
        install_ai_test_district(&mut game, home, "holy_site");
        game.cities.get_mut(&home).unwrap().buildings =
            vec!["shrine".to_string(), "temple".to_string()];
        game.players[0].techs.insert("astrology".to_string());
        game.players[0].civics.insert("theology".to_string());
        game.players[0].religion = Some("Our Faith".to_string());
        game.players[0].faith = 10_000.0;
        game.cities
            .get_mut(&home)
            .unwrap()
            .pressure
            .insert("Our Faith".to_string(), 1_000.0);
        game.cities
            .get_mut(&target)
            .unwrap()
            .pressure
            .insert("Rival Faith".to_string(), 1_000.0);

        for kind in [
            "apostle",
            "apostle",
            "guru",
            "missionary",
            "missionary",
            "missionary",
        ] {
            let unit = game.spawn_test_unit(kind, 0, game.cities[&home].pos);
            game.units.get_mut(&unit).unwrap().religion = Some("Our Faith".to_string());
        }
        let before_units = game.player_unit_ids(0).len();
        let before_faith = game.players[0].faith;
        AdvancedAi::new().religious_spending(&mut game, 0, true);
        assert_eq!(game.player_unit_ids(0).len(), before_units);
        assert_eq!(game.players[0].faith, before_faith);
    }

    #[test]
    fn surplus_faith_keeps_a_founded_secondary_campaign_in_motion() {
        let mut game = Game::new_full(2, 30, 18, 7_115, 200, 0, false);
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
        let home = game.player_city_ids(0)[0];
        let target = game.player_city_ids(1)[0];
        install_ai_test_district(&mut game, home, "holy_site");
        game.cities.get_mut(&home).unwrap().buildings =
            vec!["shrine".to_string(), "temple".to_string()];
        game.players[0].techs.insert("astrology".to_string());
        game.players[0].civics.insert("theology".to_string());
        game.players[0].religion = Some("Our Faith".to_string());
        game.cities
            .get_mut(&home)
            .unwrap()
            .pressure
            .insert("Our Faith".to_string(), 1_000.0);
        game.cities
            .get_mut(&target)
            .unwrap()
            .pressure
            .insert("Rival Faith".to_string(), 1_000.0);

        let ai = AdvancedAi::new();
        let reserve = game.game_speed.scale(1_200.0);
        game.players[0].faith = reserve;
        assert!(!ai.religious_offensive_posture(&game, 0, GrandStrategy::Science));

        game.players[0].faith = game.game_speed.scale(2_000.0);
        assert!(ai.religious_offensive_posture(&game, 0, GrandStrategy::Science));
        for _ in 0..3 {
            ai.religious_spending_with_reserve(&mut game, 0, true, reserve);
        }
        assert_eq!(
            game.units
                .values()
                .filter(|unit| unit.owner == 0 && unit.kind == "apostle")
                .count(),
            2
        );
        assert!(game.players[0].faith + f64::EPSILON >= reserve);

        let missionary = game.spawn_test_unit("missionary", 0, game.cities[&home].pos);
        game.units.get_mut(&missionary).unwrap().religion = Some("Our Faith".to_string());
        game.players[0].faith = 0.0;
        let before = game.wdist(game.units[&missionary].pos, game.cities[&target].pos);
        let offensive = ai.religious_offensive_posture(&game, 0, GrandStrategy::Science);
        assert!(
            offensive,
            "a charged field unit should sustain the campaign"
        );
        assert!(ai.advanced_missionary_step(&mut game, 0, missionary, offensive));
        let after = game.wdist(game.units[&missionary].pos, game.cities[&target].pos);
        assert!(after < before, "the secondary Missionary should leave home");
    }

    #[test]
    fn nonreligious_strategy_buys_defense_only_when_its_home_is_pressured() {
        let mut game = Game::new_full(2, 30, 18, 7_106, 200, 0, false);
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
        let home = game.player_city_ids(0)[0];
        let foreign = game.player_city_ids(1)[0];
        install_ai_test_district(&mut game, home, "holy_site");
        game.cities.get_mut(&home).unwrap().buildings =
            vec!["shrine".to_string(), "temple".to_string()];
        game.players[0].techs.insert("astrology".to_string());
        game.players[0].civics.insert("theology".to_string());
        game.players[0].religion = Some("Our Faith".to_string());
        game.players[0].faith = 1_000.0;
        game.cities.get_mut(&home).unwrap().pressure.extend([
            ("Our Faith".to_string(), 1_000.0),
            ("Rival Faith".to_string(), 600.0),
        ]);
        game.cities
            .get_mut(&foreign)
            .unwrap()
            .pressure
            .insert("Rival Faith".to_string(), 1_000.0);

        let ai = AdvancedAi::targeting(VictoryTarget::Science);
        let mut safe = game.clone();
        safe.cities
            .get_mut(&home)
            .unwrap()
            .pressure
            .insert("Rival Faith".to_string(), 100.0);
        let safe_units = safe.player_unit_ids(0).len();
        ai.religious_spending(&mut safe, 0, false);
        assert_eq!(safe.player_unit_ids(0).len(), safe_units);

        let before_units = game.player_unit_ids(0).len();
        ai.religious_spending(&mut game, 0, false);
        assert_eq!(game.player_unit_ids(0).len(), before_units + 1);
        assert!(game
            .units
            .values()
            .any(|unit| unit.owner == 0 && unit.kind == "missionary"));
    }

    #[test]
    fn faith_spending_uses_valletta_wall_price_and_ignores_gold_actions() {
        let mut game = Game::new_full(1, 30, 18, 7_107, 160, 1, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let valletta = game
            .players
            .iter()
            .find(|player| player.is_minor && !player.is_barbarian)
            .unwrap()
            .id;
        game.players[valletta].civ = "Valletta".to_string();
        game.players[0].envoys = vec![(valletta, 3)];
        game.players[0].techs.insert("masonry".to_string());
        game.players[0].faith = 200.0;
        game.players[0].gold = 10_000.0;

        AdvancedAi::targeting(VictoryTarget::Domination).faith_building_spending(
            &mut game,
            0,
            GrandStrategy::Conquest,
        );

        assert!(game.cities[&city].buildings.contains(&"walls".to_string()));
        assert_eq!(game.players[0].faith, 120.0);
        assert_eq!(game.players[0].gold, 10_000.0);
    }

    #[test]
    fn strategic_gold_purchase_buys_science_tempo_but_preserves_the_reserve() {
        let mut game = Game::new_full(1, 20, 14, 7_106, 160, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let campus = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != game.cities[&city].pos)
            .unwrap();
        game.map.tiles.get_mut(&campus).unwrap().district = Some("campus".to_string());
        game.cities
            .get_mut(&city)
            .unwrap()
            .districts
            .insert("campus".to_string(), campus);
        game.cities
            .get_mut(&city)
            .unwrap()
            .buildings
            .extend(["monument".to_string(), "granary".to_string()]);
        game.players[0].techs.insert("writing".to_string());
        game.spawn_test_unit("builder", 0, game.cities[&city].pos);
        let plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 1,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::targeting(VictoryTarget::Science);

        game.players[0].gold = 500.0;
        assert!(!ai.advanced_gold_spending(&mut game, 0, &plan));
        assert!(!game.cities[&city]
            .buildings
            .contains(&"library".to_string()));

        game.players[0].gold = 1_000.0;
        assert!(ai.advanced_gold_spending(&mut game, 0, &plan));
        assert!(game.cities[&city]
            .buildings
            .contains(&"library".to_string()));
        assert!(game.players[0].gold >= 300.0);
    }

    #[test]
    fn adaptive_turn_uses_its_live_plan_for_gold_purchases() {
        let mut game = Game::new_full(1, 20, 14, 7_107, 160, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        game.turn = 10;
        game.players[0].techs.insert("writing".to_string());
        game.players[0].gold = 10_000.0;
        game.players[0].governor_roster.insert(
            "reyna".to_string(),
            GovernorState {
                city: Some(city),
                assigned_turn: 0,
                disabled_until: 0,
                promotions: BTreeSet::from(["contractor".to_string()]),
            },
        );
        assert!(game.legal_actions(0).iter().any(|action| matches!(
            action,
            Action::BuyDistrict { district, currency, .. }
                if district == "campus" && currency == "gold"
        )));

        let mut ai = AdvancedAi::new();
        ai.base.book_pos = 4;
        ai.plan = Some(StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 1,
            assessed_turn: game.turn,
        });

        ai.take_turn(&mut game, 0);

        assert!(
            game.cities[&city].districts.contains_key("campus"),
            "an adaptive Science plan should convert surplus Gold into its Campus immediately"
        );
        assert!(game.players[0].gold >= 300.0);
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
    fn culture_trade_routes_connect_unpressured_rivals_before_duplicating_links() {
        let mut game = Game::new_full(3, 18, 10, 79_001, 200, 0, false);
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
        let origin = game.player_city_ids(0)[0];
        let connected = game.player_city_ids(1)[0];
        let unconnected = game.player_city_ids(2)[0];
        game.routes.push(crate::game::TradeRoute {
            origin,
            dest: connected,
            owner: 0,
            ends: 30,
        });

        let ai = AdvancedAi::targeting(VictoryTarget::Culture);
        let connected_value = ai.trade_route_destination_value(
            &game,
            0,
            &game.cities[&connected],
            GrandStrategy::Expansion,
        );
        let unconnected_value = ai.trade_route_destination_value(
            &game,
            0,
            &game.cities[&unconnected],
            GrandStrategy::Expansion,
        );
        assert!(unconnected_value > connected_value);

        let science_ai = AdvancedAi::targeting(VictoryTarget::Science);
        let science_connected = science_ai.trade_route_destination_value(
            &game,
            0,
            &game.cities[&connected],
            GrandStrategy::Expansion,
        );
        let science_unconnected = science_ai.trade_route_destination_value(
            &game,
            0,
            &game.cities[&unconnected],
            GrandStrategy::Expansion,
        );
        assert!(
            unconnected_value - science_unconnected > connected_value - science_connected,
            "only the Culture objective should add the missing-rival pressure bonus"
        );
    }

    #[test]
    fn advanced_trade_routes_value_named_great_person_destination_gold() {
        let mut game = Game::new_full(2, 20, 12, 79_004, 200, 0, false);
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
        let destination = game.player_city_ids(1)[0];
        let ai = AdvancedAi::targeting(VictoryTarget::Science);
        let value = |game: &Game| {
            ai.trade_route_destination_value(
                game,
                0,
                &game.cities[&destination],
                GrandStrategy::Expansion,
            )
        };
        let baseline = value(&game);

        game.cities
            .get_mut(&destination)
            .unwrap()
            .great_person_foreign_route_gold = 2.0;
        let city_bonus = value(&game);
        assert!(city_bonus > baseline);

        let resource_tile = game.cities[&destination]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != game.cities[&destination].pos)
            .unwrap();
        let tile = game.map.tiles.get_mut(&resource_tile).unwrap();
        tile.resource = Some("iron".to_string());
        tile.improvement = Some("mine".to_string());
        tile.pillaged = false;
        game.players[0].counters.insert(
            "great_person:strategic_destination_trade_gold".to_string(),
            2,
        );
        assert!(value(&game) > city_bonus);
    }

    #[test]
    fn advanced_trader_uses_an_unreserved_destination_empire_wide() {
        let mut game = Game::new_full(1, 30, 18, 79_003, 200, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let first = game.player_city_ids(0)[0];
        let first_pos = game.cities[&first].pos;
        let second = found_nearby_test_city(&mut game, 0, first_pos);
        let third = found_nearby_test_city(&mut game, 0, first_pos);
        game.players[0].civics.insert("foreign_trade".to_string());
        game.players[0]
            .counters
            .insert("great_person_trade_capacity".to_string(), 1);
        game.routes.push(crate::game::TradeRoute {
            origin: second,
            dest: first,
            owner: 0,
            ends: game.turn + 30,
        });
        let trader = game.spawn_test_unit("trader", 0, game.cities[&third].pos);

        assert!(AdvancedAi::new().advanced_trader_step(
            &mut game,
            0,
            trader,
            GrandStrategy::Expansion,
        ));
        assert!(!game.units.contains_key(&trader));
        assert!(game
            .routes
            .iter()
            .any(|route| route.origin == third && route.dest == second));
    }

    #[test]
    fn advanced_trader_relocates_to_a_city_with_a_legal_route() {
        let mut game = Game::new_full(1, 30, 18, 79_003, 200, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let destination = game.player_city_ids(0)[0];
        let destination_pos = game.cities[&destination].pos;
        let current_city = found_nearby_test_city(&mut game, 0, destination_pos);
        game.players[0].civics.insert("foreign_trade".to_string());
        game.players[0]
            .counters
            .insert("great_person_trade_capacity".to_string(), 1);
        game.routes.push(crate::game::TradeRoute {
            origin: current_city,
            dest: destination,
            owner: 0,
            ends: game.turn + 30,
        });
        let start = game.cities[&current_city].pos;
        let target = game.cities[&destination].pos;
        let trader = game.spawn_test_unit("trader", 0, start);
        let before = game.wdist(start, target);

        assert!(AdvancedAi::new().advanced_trader_step(
            &mut game,
            0,
            trader,
            GrandStrategy::Expansion,
        ));
        assert!(game.units.contains_key(&trader));
        assert_ne!(game.units[&trader].pos, start);
        for _ in 0..20 {
            if game.units[&trader].pos == target {
                break;
            }
            game.units.get_mut(&trader).unwrap().moves_left = 4.0;
            assert!(AdvancedAi::new().advanced_trader_step(
                &mut game,
                0,
                trader,
                GrandStrategy::Expansion,
            ));
        }
        assert_eq!(game.units[&trader].pos, target);
        assert!(game.wdist(game.units[&trader].pos, target) < before);
    }

    #[test]
    fn trader_production_requires_an_open_route_and_respects_idle_supply() {
        let mut game = Game::new_full(1, 30, 18, 79_004, 200, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        game.players[0].civics.insert("foreign_trade".to_string());
        let city = game.player_city_ids(0)[0];
        let item = Item::Unit {
            unit: "trader".to_string(),
        };
        let plan = StrategicPlan {
            strategy: GrandStrategy::Expansion,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 2,
            assessed_turn: game.turn,
        };
        let ai = AdvancedAi::new();

        let counts = ai.counts(&game, 0);
        assert!(ai.production_value(&game, 0, city, &item, &plan, &counts) < -9_000.0);

        let city_pos = game.cities[&city].pos;
        found_nearby_test_city(&mut game, 0, city_pos);
        let counts = ai.counts(&game, 0);
        assert!(ai.production_value(&game, 0, city, &item, &plan, &counts) > 0.0);

        game.spawn_test_unit("trader", 0, game.cities[&city].pos);
        let counts = ai.counts(&game, 0);
        assert!(ai.production_value(&game, 0, city, &item, &plan, &counts) < -9_000.0);
    }

    #[test]
    fn strategic_governments_use_late_tiers_and_match_the_culture_holdout() {
        let mut culture = Game::new_full(3, 18, 10, 79_002, 200, 0, false);
        culture.players[0]
            .civics
            .extend(["class_struggle".to_string(), "suffrage".to_string()]);
        culture.players[1].government = Some("communism".to_string());
        culture.players[1].culture_lifetime = 20_000.0;
        culture.players[2].government = Some("democracy".to_string());
        culture.players[2].culture_lifetime = 10_000.0;
        AdvancedAi::targeting(VictoryTarget::Culture).strategic_government(
            &mut culture,
            0,
            GrandStrategy::Culture,
        );
        assert_eq!(culture.players[0].government.as_deref(), Some("communism"));

        let mut science = Game::new_full(2, 18, 10, 79_003, 200, 0, false);
        science.players[0]
            .civics
            .insert("synthetic_technocracy".to_string());
        AdvancedAi::targeting(VictoryTarget::Science).strategic_government(
            &mut science,
            0,
            GrandStrategy::Science,
        );
        assert_eq!(
            science.players[0].government.as_deref(),
            Some("synthetic_technocracy")
        );

        // An adaptive plan must not fall from its one ideal Tier-2 civic all
        // the way back to Tier 1. The live archive had a Science Rome with
        // Divine Right stuck in Classical Republic and a Religion Egypt with
        // Exploration doing the same, despite their unlocked six-slot choices.
        let mut science_fallback = Game::new_full(1, 18, 10, 79_013, 200, 0, false);
        science_fallback.players[0].government = Some("classical_republic".to_string());
        science_fallback.players[0]
            .civics
            .insert("divine_right".to_string());
        AdvancedAi::new().strategic_government(
            &mut science_fallback,
            0,
            GrandStrategy::Science,
        );
        assert_eq!(
            science_fallback.players[0].government.as_deref(),
            Some("monarchy")
        );

        let mut religion_fallback = Game::new_full(1, 18, 10, 79_014, 200, 0, false);
        religion_fallback.players[0].government = Some("classical_republic".to_string());
        religion_fallback.players[0]
            .civics
            .insert("exploration".to_string());
        AdvancedAi::new().strategic_government(
            &mut religion_fallback,
            0,
            GrandStrategy::Religion,
        );
        assert_eq!(
            religion_fallback.players[0].government.as_deref(),
            Some("merchant_republic")
        );
    }

    #[test]
    fn adaptive_government_does_not_repeat_lateral_anarchy() {
        let mut game = Game::new_full(2, 18, 10, 79_015, 200, 0, false);
        game.players[0]
            .civics
            .extend(["suffrage".to_string(), "totalitarianism".to_string()]);
        game.players[0].government = Some("fascism".to_string());
        game.players[0]
            .past_governments
            .extend(["fascism".to_string(), "democracy".to_string()]);
        game.players[1].government = Some("democracy".to_string());
        game.players[1].culture_lifetime = 20_000.0;

        AdvancedAi::new().strategic_government(&mut game, 0, GrandStrategy::Culture);

        assert_eq!(game.players[0].government.as_deref(), Some("fascism"));
        assert_eq!(game.players[0].anarchy_turns, 0);
        assert!(game.players[0].pending_government.is_none());

        // Returning to a genuinely larger government remains worthwhile:
        // the two dead turns buy a persistent jump from six to eight slots.
        game.players[0].government = Some("merchant_republic".to_string());
        game.players[0]
            .past_governments
            .insert("merchant_republic".to_string());
        AdvancedAi::new().strategic_government(&mut game, 0, GrandStrategy::Conquest);
        assert!(game.players[0].government.is_none());
        assert_eq!(game.players[0].pending_government.as_deref(), Some("fascism"));
        assert!(game.players[0].anarchy_turns > 0);
    }

    #[test]
    fn advanced_turn_does_not_run_the_baseline_government_selector_first() {
        let mut game = Game::new_full(2, 18, 10, 79_016, 200, 0, false);
        game.players[0].civics.extend([
            "code_of_laws".to_string(),
            "political_philosophy".to_string(),
        ]);
        game.players[0].government = Some("chiefdom".to_string());
        game.players[0]
            .past_governments
            .insert("chiefdom".to_string());

        let mut ai = AdvancedAi::targeting(VictoryTarget::Domination);
        ai.take_turn(&mut game, 0);

        assert_eq!(game.players[0].government.as_deref(), Some("oligarchy"));
        assert_eq!(game.players[0].anarchy_turns, 0);
        assert!(game.players[0].pending_government.is_none());
        assert!(game.players[0].past_governments.contains("oligarchy"));
        assert!(
            !game.players[0]
                .past_governments
                .contains("classical_republic"),
            "the baseline selector must not install a throwaway government before the strategic one"
        );
    }

    #[test]
    fn adaptive_government_does_not_create_anarchy_by_downgrading_first() {
        let mut game = Game::new_full(2, 18, 10, 79_017, 200, 0, false);
        game.players[0].civics.extend([
            "class_struggle".to_string(),
            "reformed_church".to_string(),
        ]);
        game.players[0].government = Some("communism".to_string());
        game.players[0]
            .past_governments
            .insert("communism".to_string());
        game.players[0].faith = 1_000.0;

        AdvancedAi::new().strategic_government(&mut game, 0, GrandStrategy::Conquest);

        assert_eq!(game.players[0].government.as_deref(), Some("communism"));
        assert_eq!(game.players[0].anarchy_turns, 0);
        assert!(game.players[0].pending_government.is_none());
        assert!(
            !game.players[0].past_governments.contains("theocracy"),
            "a free first adoption is still a downgrade when it discards two policy slots"
        );
    }

    #[test]
    fn faith_stockpile_mobilizes_for_an_imminent_threat() {
        let mut game = Game::new_full(2, 24, 16, 79_012, 200, 0, false);
        for pid in 0..2 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        game.current = 0;
        game.players[0].civics.insert("reformed_church".to_string());
        game.players[0].faith = 1_500.0;
        let target = game.player_city_ids(1)[0];
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: Some(target),
            threatened_city: None,
            desired_cities: 1,
            assessed_turn: game.turn,
        };
        let before_units = game.player_unit_ids(0).len();
        let ai = AdvancedAi::new();

        ai.strategic_government(&mut game, 0, plan.strategy);
        assert_eq!(game.players[0].government.as_deref(), Some("theocracy"));
        assert!(ai.military_faith_spending(&mut game, 0, &plan));
        assert_eq!(game.player_unit_ids(0).len(), before_units + 1);
        assert!(game.players[0].faith < 1_500.0);
    }

    #[test]
    fn culture_quick_deals_buy_the_direction_that_increases_our_tourism() {
        let mut game = Game::new_full(2, 18, 10, 79_004, 200, 0, false);
        game.turn = 6;
        game.players[0].gold = 1_000.0;
        game.players[1].gold = 1_000.0;
        game.players[0].civics.insert("early_empire".to_string());
        game.players[1].civics.insert("early_empire".to_string());

        AdvancedAi::targeting(VictoryTarget::Culture).strategic_bilateral_trade(
            &mut game,
            0,
            None,
            GrandStrategy::Expansion,
        );

        assert!(game.has_open_borders(0, 1));
        assert!(!game.has_open_borders(1, 0));
        assert_eq!(game.international_tourism_multiplier(0, 1, false), 1.25);
    }

    #[test]
    fn culture_quick_deals_buy_housed_great_works_and_preserve_our_own() {
        let mut game = Game::new_full(2, 20, 12, 79_005, 200, 0, false);
        for pid in 0..2 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
            let city = game.player_city_ids(pid)[0];
            install_ai_test_district(&mut game, city, "theater_square");
            game.cities
                .get_mut(&city)
                .unwrap()
                .buildings
                .push("amphitheater".to_string());
            game.players[pid].gold = 1_000.0;
        }
        game.current = 0;
        game.players[1]
            .counters
            .insert("great_work:writing".to_string(), 2);
        game.turn = 6;

        AdvancedAi::targeting(VictoryTarget::Culture).strategic_bilateral_trade(
            &mut game,
            0,
            None,
            GrandStrategy::Expansion,
        );
        assert_eq!(game.players[0].counters["great_work:writing"], 1);
        assert_eq!(game.players[1].counters["great_work:writing"], 1);

        let mut preserve = Game::new_full(2, 20, 12, 79_006, 200, 0, false);
        for pid in 0..2 {
            preserve.current = pid;
            let settler = preserve
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| preserve.units[unit].kind == "settler")
                .unwrap();
            preserve
                .apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
            let city = preserve.player_city_ids(pid)[0];
            install_ai_test_district(&mut preserve, city, "theater_square");
            preserve
                .cities
                .get_mut(&city)
                .unwrap()
                .buildings
                .push("amphitheater".to_string());
            preserve.players[pid].gold = 1_000.0;
        }
        preserve.current = 0;
        preserve.players[0]
            .counters
            .insert("great_work:writing".to_string(), 2);
        preserve.turn = 6;
        AdvancedAi::targeting(VictoryTarget::Culture).strategic_bilateral_trade(
            &mut preserve,
            0,
            None,
            GrandStrategy::Expansion,
        );
        assert_eq!(preserve.players[0].counters["great_work:writing"], 2);
        assert_eq!(
            preserve.players[1]
                .counters
                .get("great_work:writing")
                .copied()
                .unwrap_or(0),
            0
        );
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
    fn culture_builders_upgrade_farms_to_resorts_without_reverting_them() {
        let mut g = Game::new(2, 24, 16, 74, 80, 0);
        let settler = g
            .player_unit_ids(0)
            .into_iter()
            .find(|uid| g.units[uid].kind == "settler")
            .unwrap();
        g.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        g.players[0].techs.insert("radio".to_string());
        let city = g.player_city_ids(0)[0];
        let pos = g.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|pos| *pos != g.cities[&city].pos)
            .unwrap();
        let tile = g.map.tiles.get_mut(&pos).unwrap();
        tile.terrain = "plains".to_string();
        tile.feature = None;
        tile.resource = None;
        tile.hills = false;
        tile.improvement = Some("farm".to_string());
        for neighbor in g.nbrs(pos) {
            let tile = g.map.tiles.get_mut(&neighbor).unwrap();
            tile.terrain = "coast".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.improvement = None;
            tile.district = None;
            tile.wonder = None;
            tile.pillaged = false;
        }
        assert!(g.tile_appeal(pos) >= 4);

        let ai = AdvancedAi::targeting(VictoryTarget::Culture);
        let upgrades = ai.worthwhile_improvements(&g, 0, pos, GrandStrategy::Culture);
        assert_eq!(upgrades.first().map(String::as_str), Some("seaside_resort"));

        g.map.tiles.get_mut(&pos).unwrap().improvement = Some("seaside_resort".to_string());
        assert!(ai
            .worthwhile_improvements(&g, 0, pos, GrandStrategy::Culture)
            .is_empty());
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
    fn envoy_strategy_prices_the_next_active_building_threshold_per_envoy() {
        let mut game = Game::new_full(1, 28, 18, 7_710, 120, 2, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        let city = game.found_city_for(0, game.units[&settler].pos, None);
        game.remove_unit(settler);
        install_ai_test_district(&mut game, city, "commercial_hub");
        install_ai_test_district(&mut game, city, "harbor");
        install_ai_test_district(&mut game, city, "diplomatic_quarter");
        game.cities.get_mut(&city).unwrap().buildings.extend(
            ["stock_exchange", "seaport", "chancery"]
                .into_iter()
                .map(str::to_string),
        );

        let states: Vec<usize> = game
            .players
            .iter()
            .filter(|player| player.is_minor && !player.is_barbarian)
            .map(|player| player.id)
            .collect();
        let hattusa = states[0];
        let zanzibar = states[1];
        game.players[hattusa].civ = "Hattusa".to_string();
        game.players[zanzibar].civ = "Zanzibar".to_string();
        game.players[0].envoys = vec![(hattusa, 4), (zanzibar, 5)];
        game.players[0].envoys_free = 1;

        let (science_steps, science_gain) = game.next_envoy_type_bonus(0, hattusa).unwrap();
        let (gold_steps, gold_gain) = game.next_envoy_type_bonus(0, zanzibar).unwrap();
        assert_eq!((science_steps, science_gain.science), (2, 3.0));
        assert_eq!((gold_steps, gold_gain.gold), (1, 18.0));

        AdvancedAi::new().advanced_envoys(&mut game, 0, GrandStrategy::Science, None);
        assert_eq!(game.envoys_at(0, hattusa), 4);
        assert_eq!(game.envoys_at(0, zanzibar), 6);
    }

    #[test]
    fn religious_envoys_prefer_yerevan_but_skip_a_bonus_shared_by_economic_alliance() {
        let mut game = Game::new_full(2, 32, 20, 7_711, 120, 2, false);
        let minors: Vec<usize> = game
            .players
            .iter()
            .filter(|player| player.is_minor && !player.is_barbarian)
            .map(|player| player.id)
            .collect();
        assert_eq!(minors.len(), 2);
        game.players[minors[0]].civ = "Kandy".to_string();
        game.players[minors[1]].civ = "Yerevan".to_string();
        game.players[0].envoys_free = 1;

        AdvancedAi::new().advanced_envoys(&mut game, 0, GrandStrategy::Religion, None);
        assert_eq!(game.envoys_at(0, minors[0]), 0);
        assert_eq!(game.envoys_at(0, minors[1]), 1);

        game.players[0].envoys.clear();
        game.players[0].envoys_free = 1;
        game.players[1].envoys = vec![(minors[1], 3)];
        let alliance = crate::game::AllianceState {
            kind: "economic".to_string(),
            points: 240.0,
            level: 3,
            ends: game.turn + 30,
        };
        game.players[0].alliances.insert(1, alliance.clone());
        game.players[1].alliances.insert(0, alliance);

        AdvancedAi::new().advanced_envoys(&mut game, 0, GrandStrategy::Religion, None);
        assert_eq!(game.envoys_at(0, minors[0]), 1);
        assert_eq!(game.envoys_at(0, minors[1]), 0);
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


    /// A staged battle only tests the code under test if the organic map is
    /// not also fighting one nearby: units and cities the generator placed
    /// give the planner closer targets and quietly rewrite the expected order.
    /// Arenas are therefore ranked by how far they sit from everything the
    /// generator put down, so the quietest corner of any map wins.
    fn organic_clearance(g: &Game, pos: Pos) -> i32 {
        g.units
            .values()
            .map(|unit| g.wdist(unit.pos, pos))
            .chain(g.cities.values().map(|city| g.wdist(city.pos, pos)))
            .min()
            .unwrap_or(i32::MAX)
    }

    fn quietest_first(g: &Game, mut candidates: Vec<Pos>) -> Vec<Pos> {
        candidates.sort_by_key(|pos| (std::cmp::Reverse(organic_clearance(g, *pos)), *pos));
        candidates
    }

    #[test]
    fn armies_and_fleets_receive_domain_specific_shared_orders() {
        let mut g = Game::new_full(2, 24, 16, 78, 80, 0, false);
        g.at_war.insert((0, 1));

        let land_candidates = quietest_first(
            &g,
            g.map
                .tiles
                .iter()
                .filter(|(pos, tile)| {
                    g.rules.is_passable(tile)
                        && !g.rules.is_water(tile)
                        && g.units_at(**pos).is_empty()
                })
                .map(|(pos, _)| *pos)
                .collect(),
        );
        let land_target = land_candidates
            .into_iter()
            .find_map(|pos| {
                let ring: Vec<Pos> = g
                    .nbrs(pos)
                    .into_iter()
                    .filter(|neighbor| {
                        g.map.get(*neighbor).is_some_and(|tile| {
                            g.rules.is_passable(tile)
                                && !g.rules.is_water(tile)
                                && g.units_at(*neighbor).is_empty()
                        })
                    })
                    .collect();
                (ring.len() >= 3).then_some((pos, ring))
            })
            .expect("test map has an open land engagement");
        for position in [land_target.0, land_target.1[0], land_target.1[1], land_target.1[2]] {
            let tile = g.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.improvement = None;
            tile.hills = false;
        }
        for ring_tile in land_target.1.iter().copied() {
            g.map.set_river_edge(land_target.0, ring_tile, false);
        }
        let army = [
            g.spawn_test_unit("warrior", 0, land_target.1[0]),
            g.spawn_test_unit("archer", 0, land_target.1[1]),
            g.spawn_test_unit("catapult", 0, land_target.1[2]),
        ];
        // A mirror matchup on level ground is an even trade, which the
        // planner is right to decline; this test is about which unit
        // receives the order, so leave the defender plainly worth striking.
        let defender = g.spawn_test_unit("warrior", 1, land_target.0);
        g.units.get_mut(&defender).unwrap().hp = 20;

        let sea_candidates = quietest_first(
            &g,
            g.map
                .tiles
                .iter()
                .filter(|(pos, tile)| g.rules.is_water(tile) && g.units_at(**pos).is_empty())
                .map(|(pos, _)| *pos)
                .collect(),
        );
        let sea_target = sea_candidates
            .into_iter()
            .find_map(|pos| {
                let ring: Vec<Pos> = g
                    .nbrs(pos)
                    .into_iter()
                    .filter(|neighbor| {
                        g.map.get(*neighbor).is_some_and(|tile| {
                            g.rules.is_water(tile) && g.units_at(*neighbor).is_empty()
                        })
                    })
                    .collect();
                (ring.len() >= 2).then_some((pos, ring))
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

        let acted = ai.advanced_military_step(&mut g, 0, army[0], &plan);
        let last = g.log.last().cloned();
        assert!(
            matches!(
                last,
                Some((0, Action::Attack { unit, target }))
                    if unit == army[0] && target == land_target.0
            ),
            "the army's lead unit should strike the focus target: acted={acted}, log={last:?}"
        );
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
            ai.coordinated_tactical_step(&mut g, 0, *uid, &orders, &[1], false);
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
        let (anchor, risky, safe, reply_squares) = g
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
                        .filter(|unit| unit.owner != 0)
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
                        let replies: Vec<Pos> = g
                            .wdisk(*risky, 2)
                            .into_iter()
                            .filter(|reply| {
                                g.wdist(*risky, *reply) == 2
                                    // A ranged unit may move one tile before
                                    // firing. Keep the safe capture outside
                                    // both its current and move-then-fire reach
                                    // so this fixture is independent of the
                                    // generated terrain's movement costs.
                                    && g.wdist(*safe, *reply) > 3
                                    && *reply != *anchor
                                    && g.map.get(*reply).is_some_and(|tile| {
                                        g.rules.is_passable(tile) && !g.rules.is_water(tile)
                                    })
                                    && g.units_at(*reply).is_empty()
                                    && g.city_at(*reply).is_none()
                            })
                            .collect();
                        if replies.len() >= 2 {
                            return Some((*anchor, *risky, *safe, replies));
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
        // The reply search extends one approach step, so an archer three or
        // four tiles from the safe square could still move and shoot it.
        // Moat the approach ring — water stops land approaches without
        // blocking the archers' sight lines — so the safe capture stays
        // unpunishable by construction, not by map luck.
        for position in g.wdisk(safe, 2) {
            if position == safe
                || position == anchor
                || position == risky
                || !g.units_at(position).is_empty()
                || g.city_at(position).is_some()
            {
                continue;
            }
            let tile = g.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "coast".to_string();
            tile.feature = None;
            tile.resource = None;
            tile.improvement = None;
            tile.hills = false;
        }
        let attacker = g.spawn_test_unit("swordsman", 0, anchor);
        let risky_defender = g.spawn_test_unit("warrior", 1, risky);
        let safe_defender = g.spawn_test_unit("warrior", 1, safe);
        g.units.get_mut(&risky_defender).unwrap().hp = 1;
        g.units.get_mut(&safe_defender).unwrap().hp = 1;
        g.spawn_test_unit("archer", 1, reply_squares[0]);

        let risky_action = Action::Attack {
            unit: attacker,
            target: risky,
        };
        let safe_action = Action::Attack {
            unit: attacker,
            target: safe,
        };
        let mut ai = AdvancedAi::legacy();
        let single_reply = ai.forcing_reply_penalty(&g, 0, attacker, &risky_action);
        g.spawn_test_unit("archer", 1, reply_squares[1]);
        let risky_reply = ai.forcing_reply_penalty(&g, 0, attacker, &risky_action);
        let safe_reply = ai.forcing_reply_penalty(&g, 0, attacker, &safe_action);
        assert!(
            risky_reply > single_reply + 5.0,
            "the reply extension must price coordinated focus fire: single={single_reply}, risky={risky_reply}, safe={safe_reply}"
        );
        assert!(
            risky_reply > safe_reply + 5.0,
            "the ranged recapture must make the exposed kill materially worse: single={single_reply}, risky={risky_reply}, safe={safe_reply}"
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
    fn forcing_reply_search_prices_a_move_then_attack_counter() {
        let mut game = Game::new_full(2, 24, 16, 8_118, 80, 0, false);
        game.at_war.insert((0, 1));
        game.current = 0;
        let (anchor, prize, counter) = game
            .map
            .tiles
            .iter()
            .filter(|(position, tile)| {
                game.rules.is_passable(tile)
                    && !game.rules.is_water(tile)
                    && game.units_at(**position).is_empty()
                    && game.city_at(**position).is_none()
                    && game
                        .cities
                        .values()
                        .all(|city| game.wdist(**position, city.pos) > 5)
                    && game
                        .units
                        .values()
                        .all(|unit| game.wdist(**position, unit.pos) > 5)
            })
            .find_map(|(anchor, _)| {
                game.nbrs(*anchor).into_iter().find_map(|prize| {
                    let prize_tile = game.map.get(prize)?;
                    if !game.rules.is_passable(prize_tile)
                        || game.rules.is_water(prize_tile)
                        || !game.units_at(prize).is_empty()
                        || game.city_at(prize).is_some()
                    {
                        return None;
                    }
                    game.wdisk(prize, 3).into_iter().find_map(|counter| {
                        let tile = game.map.get(counter)?;
                        (game.wdist(prize, counter) == 3
                            && game.rules.is_passable(tile)
                            && !game.rules.is_water(tile)
                            && game.units_at(counter).is_empty()
                            && game.city_at(counter).is_none()
                            && game.nbrs(counter).into_iter().any(|step| {
                                game.wdist(step, prize) == 2
                                    && game.map.get(step).is_some_and(|tile| {
                                        game.rules.is_passable(tile) && !game.rules.is_water(tile)
                                    })
                                    && game.units_at(step).is_empty()
                                    && game.city_at(step).is_none()
                            }))
                        .then_some((*anchor, prize, counter))
                    })
                })
            })
            .expect("test map has a one-step ranged-counter geometry");

        for position in game.wdisk(prize, 3) {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
        }
        let attacker = game.spawn_test_unit("swordsman", 0, anchor);
        let defender = game.spawn_test_unit("warrior", 1, prize);
        game.units.get_mut(&defender).unwrap().hp = 1;
        let capture = Action::Attack {
            unit: attacker,
            target: prize,
        };
        let ai = AdvancedAi::new();
        let quiet = ai.forcing_reply_penalty(&game, 0, attacker, &capture);
        game.spawn_test_unit("archer", 1, counter);
        let mobile_counter = ai.forcing_reply_penalty(&game, 0, attacker, &capture);
        assert!(
            mobile_counter > quiet + 5.0,
            "a ranged unit one step outside range must still count as a forcing reply: {mobile_counter} <= {quiet}"
        );
    }

    #[test]
    fn explicit_victory_command_phase_fires_city_center_strikes() {
        let mut game = Game::new_full(2, 20, 14, 8_119, 80, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let center = game.cities[&city].pos;
        for position in game.wdisk(center, 2) {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
        }
        let target = game
            .nbrs(center)
            .into_iter()
            .find(|position| {
                game.units_at(*position).is_empty()
                    && game.city_at(*position).is_none()
                    && game.encampment_at(*position).is_none()
            })
            .unwrap();
        game.at_war.insert((0, 1));
        game.cities.get_mut(&city).unwrap().wall_hp = 100;
        let enemy = game.spawn_test_unit("warrior", 1, target);
        let before = game.units[&enemy].hp;

        AdvancedAi::targeting(VictoryTarget::Domination).advanced_city_strikes(&mut game, 0);

        assert!(game.cities[&city].struck);
        assert!(
            game.units
                .get(&enemy)
                .is_none_or(|defender| defender.hp < before),
            "the explicit victory command phase must spend an available wall strike"
        );
    }

    #[test]
    fn encampment_strikes_choose_the_exact_kill_over_static_unit_strength() {
        let mut game = Game::new_full(2, 20, 14, 8_120, 80, 0, false);
        let settler = game
            .player_unit_ids(0)
            .into_iter()
            .find(|unit| game.units[unit].kind == "settler")
            .unwrap();
        game.apply(0, &Action::FoundCity { unit: settler }).unwrap();
        let city = game.player_city_ids(0)[0];
        let center = game.cities[&city].pos;
        let encampment = game.cities[&city]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != center)
            .unwrap();
        for position in game.wdisk(encampment, 2) {
            let tile = game.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
        }
        game.map.tiles.get_mut(&encampment).unwrap().district = Some("encampment".to_string());
        game.cities
            .get_mut(&city)
            .unwrap()
            .districts
            .insert("encampment".to_string(), encampment);
        {
            let city = game.cities.get_mut(&city).unwrap();
            city.encampment_hp = 100;
            city.encampment_wall_hp = 100;
        }
        game.at_war.insert((0, 1));
        let targets: Vec<Pos> = game
            .nbrs(encampment)
            .into_iter()
            .filter(|position| {
                *position != encampment
                    && game.city_at(*position).is_none()
                    && game.encampment_at(*position).is_none()
                    && game.units_at(*position).is_empty()
            })
            .take(2)
            .collect();
        assert_eq!(targets.len(), 2);
        let armor = game.spawn_test_unit("modern_armor", 1, targets[0]);
        let weak = game.spawn_test_unit("warrior", 1, targets[1]);
        game.units.get_mut(&weak).unwrap().hp = 1;
        let armor_static = game.unit_strength(&game.units[&armor], true);
        let weak_static = game.unit_strength(&game.units[&weak], true) + 99.0 * 0.6;
        assert!(
            armor_static > weak_static,
            "the legacy heuristic chose the armor"
        );

        AdvancedAi::targeting(VictoryTarget::Domination).advanced_encampment_strikes(&mut game, 0);

        assert!(game.cities[&city].encampment_struck);
        assert!(game.units.contains_key(&armor));
        assert!(!game.units.contains_key(&weak));
    }

    #[test]
    fn force_replans_focus_after_each_battlefield_action() {
        let mut g = Game::new_full(2, 24, 16, 79, 80, 0, false);
        g.at_war.insert((0, 1));
        let front_candidates = quietest_first(
            &g,
            g.map
                .tiles
                .iter()
                .filter(|(pos, tile)| {
                    g.rules.is_passable(tile)
                        && !g.rules.is_water(tile)
                        && g.units_at(**pos).is_empty()
                })
                .map(|(pos, _)| *pos)
                .collect(),
        );
        let (first_target, second_target, firing_line) = front_candidates
            .into_iter()
            .find_map(|first| {
                g.nbrs(first).into_iter().find_map(|second| {
                    let second_tile = g.map.get(second)?;
                    if !g.rules.is_passable(second_tile)
                        || g.rules.is_water(second_tile)
                        || !g.units_at(second).is_empty()
                    {
                        return None;
                    }
                    let second_neighbors = g.nbrs(second);
                    let common: Vec<Pos> = g
                        .nbrs(first)
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
                    (common.len() >= 2).then_some((first, second, common))
                })
            })
            .expect("test map has a two-target engagement with a shared front");

        // Level the arena: the test exercises replanning after a kill, and
        // must not hinge on whichever defense modifiers the organic map put
        // under the four staged tiles.
        for position in [first_target, second_target, firing_line[0], firing_line[1]] {
            let tile = g.map.tiles.get_mut(&position).unwrap();
            tile.terrain = "plains".to_string();
            tile.feature = None;
            tile.hills = false;
        }

        let attackers = [
            g.spawn_test_unit("warrior", 0, firing_line[0]),
            g.spawn_test_unit("warrior", 0, firing_line[1]),
        ];
        let first_enemy = g.spawn_test_unit("warrior", 1, first_target);
        g.units.get_mut(&first_enemy).unwrap().hp = 1;
        let second_enemy = g.spawn_test_unit("warrior", 1, second_target);
        g.units.get_mut(&second_enemy).unwrap().hp = 1;
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
        assert!(
            matches!(
                g.log.last(),
                Some((0, Action::Attack { unit, target }))
                    if *unit == attackers[1] && *target == second_target
            ),
            "unexpected replanned action log: {:?}",
            g.log
        );
    }

    #[test]
    fn advanced_ai_votes_in_special_sessions_and_liberates_emergency_objectives() {
        let mut vote_game = Game::new_full(3, 26, 16, 73_001, 120, 0, false);
        for player in 0..3 {
            let settler = vote_game
                .player_unit_ids(player)
                .into_iter()
                .find(|unit| vote_game.units[unit].kind == "settler")
                .unwrap();
            vote_game.found_city_for(player, vote_game.units[&settler].pos, None);
        }
        let objective = vote_game.player_city_ids(0)[0];
        vote_game.pending_emergencies = vec![crate::game::EmergencyProposal {
            id: 77,
            kind: "city_state".to_string(),
            target: 0,
            city: objective,
            original_owner: 1,
            eligible: [2].into_iter().collect(),
            requested: vote_game.turn,
        }];
        vote_game.congress = Some(crate::game::CongressSession {
            convened: vote_game.turn,
            closes: vote_game.turn + 5,
            resolutions: vec![CongressResolution {
                id: "emergency:77".to_string(),
                title: "City-State Emergency".to_string(),
                choices: vec!["A:support".to_string(), "B:oppose".to_string()],
                ballots: BTreeMap::new(),
            }],
        });
        vote_game.current = 2;
        let plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: None,
            target_city: None,
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: vote_game.turn,
        };
        let mut ai = AdvancedAi::targeting(VictoryTarget::Science);
        ai.advanced_diplomacy(&mut vote_game, 2, &plan);
        assert_eq!(
            vote_game.congress.as_ref().unwrap().resolutions[0].ballots[&2].0,
            "A:support"
        );
        assert_eq!(
            ai.congress_choice(
                &vote_game,
                0,
                &vote_game.congress.as_ref().unwrap().resolutions[0],
                GrandStrategy::Conquest,
            ),
            Some("B:oppose".to_string())
        );

        let mut conquest = Game::new_full(3, 26, 16, 73_002, 120, 0, false);
        for player in 0..3 {
            let settler = conquest
                .player_unit_ids(player)
                .into_iter()
                .find(|unit| conquest.units[unit].kind == "settler")
                .unwrap();
            conquest.found_city_for(player, conquest.units[&settler].pos, None);
        }
        let objective = conquest.player_city_ids(1)[0];
        {
            let city = conquest.cities.get_mut(&objective).unwrap();
            city.owner = 0;
            city.captured_from = None;
            city.occupied_from = Some(1);
        }
        conquest.active_emergencies = vec![crate::game::Emergency {
            id: 78,
            kind: "military".to_string(),
            target: 0,
            city: objective,
            original_owner: 1,
            members: [2].into_iter().collect(),
            contributions: BTreeMap::new(),
            started: conquest.turn,
            ends: conquest.turn + 30,
        }];
        conquest.current = 2;
        let emergency_plan = ai.assess(&conquest, 2);
        assert_eq!(emergency_plan.target_player, Some(0));
        assert_eq!(emergency_plan.target_city, Some(objective));
        {
            let city = conquest.cities.get_mut(&objective).unwrap();
            city.owner = 2;
            city.captured_from = Some(0);
            city.occupied_from = Some(0);
        }
        ai.resolve_city_dispositions(&mut conquest, 2, GrandStrategy::Science);
        assert_eq!(conquest.cities[&objective].owner, 1);
        assert!(conquest.active_emergencies.is_empty());
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
        // Settlers lost to Barbarians or captured legitimately break any
        // lifetime produced-vs-founded accounting. Guard the behavior this
        // test actually cares about instead: the production gate only queues a
        // Settler while the player holds none, so a player must never end the
        // game sitting on a backlog of idle Settlers.
        for player in g.players.iter().filter(|p| !p.is_minor && p.alive) {
            let idle = g
                .units
                .values()
                .filter(|u| u.owner == player.id && u.kind == "settler")
                .count();
            assert!(
                idle <= 1,
                "advanced AI accumulated idle Settlers: player {} holds {idle}",
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

    #[test]
    fn conquest_razes_a_hopeless_isolated_city_instead_of_recapturing_it() {
        let mut game = Game::new_full(2, 30, 18, 107_002, 120, 0, false);
        for pid in 0..2 {
            game.current = pid;
            let settler = game
                .player_unit_ids(pid)
                .into_iter()
                .find(|unit| game.units[unit].kind == "settler")
                .unwrap();
            game.apply(pid, &Action::FoundCity { unit: settler })
                .unwrap();
        }
        let home = game.player_city_ids(0)[0];
        let rival_capital = game.player_city_ids(1)[0];
        let rival_pos = game.cities[&rival_capital].pos;
        let outpost_pos = game
            .wdisk(rival_pos, 4)
            .into_iter()
            .find(|position| {
                game.wdist(*position, rival_pos) == 4
                    && game.wdist(*position, game.cities[&home].pos) > 9
                    && game.city_at(*position).is_none()
                    && game.map.tiles[position].owner_city.is_none()
            })
            .expect("test map has an isolated rival outpost site");
        {
            let tile = game.map.tiles.get_mut(&outpost_pos).unwrap();
            tile.terrain = "grassland".to_string();
            tile.feature = None;
            tile.hills = false;
        }
        game.cities.get_mut(&rival_capital).unwrap().pop = 15;
        game.cities.get_mut(&home).unwrap().pop = 3;
        let outpost = game.found_city_for(1, outpost_pos, Some("Revolt Loop".to_string()));
        {
            let captured = game.cities.get_mut(&outpost).unwrap();
            captured.owner = 0;
            captured.pop = 3;
            captured.loyalty = 50.0;
            captured.captured_from = Some(1);
            captured.occupied_from = Some(1);
        }
        game.current = 0;
        assert!(game
            .legal_city_disposition_actions(0)
            .iter()
            .any(|action| matches!(action, Action::RazeCity { city } if *city == outpost)));

        let mut ai = AdvancedAi::targeting(VictoryTarget::Domination);
        assert!(AdvancedAi::population_loyalty_delta(&game, 0, outpost) <= -8.0);

        // A nearby core is not enough to save a tiny conquest that is already
        // forecast to revolt in three or four turns. The live Alexandria loop
        // was one population, six tiles from China, and changed hands five
        // times in seventeen turns because the older rule treated distance as
        // an absolute exemption.
        let mut nearby = game.clone();
        let near_home_pos = nearby
            .wdisk(outpost_pos, 6)
            .into_iter()
            .find(|position| {
                nearby.wdist(*position, outpost_pos) == 6
                    && nearby.city_at(*position).is_none()
                    && nearby.map.tiles[position].owner_city.is_none()
                    && nearby.rules.is_passable(&nearby.map.tiles[position])
                    && !nearby.rules.is_water(&nearby.map.tiles[position])
            })
            .expect("test map has a core site six tiles from the captured outpost");
        nearby.cities.get_mut(&home).unwrap().pos = near_home_pos;
        assert_eq!(
            nearby
                .cities
                .values()
                .filter(|city| city.owner == 0 && city.id != outpost)
                .map(|city| nearby.wdist(city.pos, outpost_pos))
                .min(),
            Some(6)
        );
        assert!(AdvancedAi::population_loyalty_delta(&nearby, 0, outpost) <= -8.0);
        let mut nearby_ai = AdvancedAi::targeting(VictoryTarget::Domination);
        nearby_ai.resolve_city_dispositions(&mut nearby, 0, GrandStrategy::Conquest);
        assert!(
            !nearby.cities.contains_key(&outpost),
            "an imminent low-value revolt should be razed even near a core city"
        );

        ai.resolve_city_dispositions(&mut game, 0, GrandStrategy::Conquest);

        assert!(
            !game.cities.contains_key(&outpost),
            "a city forecast to revolt before support can arrive should be razed once"
        );
    }

    #[test]
    fn adaptive_turn_uses_live_victory_focus_for_mandatory_city_disposition() {
        let mut game = Game::new_full(2, 24, 16, 107_001, 80, 1, false);
        let minor = game
            .players
            .iter()
            .find(|player| player.is_minor && !player.is_barbarian)
            .unwrap()
            .id;
        let city = game.player_city_ids(minor)[0];
        {
            let captured = game.cities.get_mut(&city).unwrap();
            captured.owner = 0;
            captured.captured_from = Some(1);
            captured.occupied_from = Some(1);
            captured.loyalty = 50.0;
        }
        game.players[0].dvp = 19;
        let mut ai = AdvancedAi::new();
        assert_eq!(
            ai.victory_focus(&game, 0).strategy,
            GrandStrategy::Diplomacy
        );

        ai.take_turn(&mut game, 0);

        assert_eq!(game.cities[&city].owner, minor);
        assert_eq!(game.players[0].diplomatic_favor, 100.0);
    }

    #[test]
    fn occupation_reserves_a_reachable_garrison_during_war() {
        let mut game = Game::new_full(3, 26, 16, 108, 80, 1, false);
        let city = game
            .players
            .iter()
            .find(|player| player.is_minor && !player.is_barbarian)
            .and_then(|minor| game.player_city_ids(minor.id).first().copied())
            .unwrap();
        {
            let occupied = game.cities.get_mut(&city).unwrap();
            occupied.owner = 0;
            occupied.captured_from = None;
            occupied.occupied_from = Some(1);
            occupied.loyalty = 35.0;
        }
        for unit in game.player_unit_ids(0) {
            game.remove_unit(unit);
        }
        let city_pos = game.cities[&city].pos;
        for unit in game.units_at(city_pos) {
            game.remove_unit(unit);
        }
        let start =
            game.nbrs(city_pos)
                .into_iter()
                .find(|position| {
                    game.map.get(*position).is_some_and(|tile| {
                        game.rules.is_passable(tile) && !game.rules.is_water(tile)
                    }) && game.units_at(*position).is_empty()
                })
                .unwrap();
        let warrior = game.spawn_test_unit("warrior", 0, start);
        game.at_war.insert((0, 1));
        let plan = StrategicPlan {
            strategy: GrandStrategy::Conquest,
            target_player: Some(1),
            target_city: game.player_city_ids(1).first().copied(),
            threatened_city: None,
            desired_cities: 4,
            assessed_turn: game.turn,
        };
        let mut ai = AdvancedAi::new();

        assert_eq!(
            ai.occupation_garrison_target(&game, 0, warrior),
            Some(city_pos)
        );
        assert!(ai.advanced_military_step(&mut game, 0, warrior, &plan));
        assert_eq!(game.units[&warrior].pos, city_pos);
    }

    #[test]
    fn a_spy_posted_to_a_razed_city_does_not_bring_the_server_down() {
        let mut game = Game::new_full(2, 24, 16, 109, 120, 0, false);
        let cities: Vec<u32> = (0..2)
            .map(|pid| {
                let settler = game
                    .player_unit_ids(pid)
                    .into_iter()
                    .find(|unit| game.units[unit].kind == "settler")
                    .unwrap();
                game.found_city_for(pid, game.units[&settler].pos, None)
            })
            .collect();
        let spy = game.next_id;
        game.next_id += 1;
        game.spies.insert(
            spy,
            crate::game::Spy {
                id: spy,
                owner: 0,
                level: 1,
                promotions: Default::default(),
                city: Some(cities[0]),
                ready_turn: game.turn,
                mission: None,
                sources_city: None,
                sources_until: 0,
                captured_by: None,
            },
        );
        // The city the agent is posted to is razed out from under it, which is
        // an ordinary wartime event. Indexing the city map for it panicked the
        // AI thread, and that poisoned the game mutex so every later HTTP
        // request died too: one razed city took the whole exhibition offline.
        game.cities.remove(&cities[0]);

        let plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: Some(1),
            target_city: Some(cities[1]),
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };
        AdvancedAi::targeting(VictoryTarget::Science).advanced_spies(&mut game, 0, &plan);
        assert!(game.spies.contains_key(&spy), "the agent survives the raze");
    }

    #[test]
    fn science_strategy_uses_an_established_spy_to_steal_a_rival_technology() {
        let mut game = Game::new_full(2, 24, 16, 109, 120, 0, false);
        let cities: Vec<u32> = (0..2)
            .map(|pid| {
                let settler = game
                    .player_unit_ids(pid)
                    .into_iter()
                    .find(|unit| game.units[unit].kind == "settler")
                    .unwrap();
                game.found_city_for(pid, game.units[&settler].pos, None)
            })
            .collect();
        let target = cities[1];
        let campus = game.cities[&target]
            .owned_tiles
            .iter()
            .copied()
            .find(|position| *position != game.cities[&target].pos)
            .unwrap();
        game.map.tiles.get_mut(&campus).unwrap().district = Some("campus".to_string());
        game.cities
            .get_mut(&target)
            .unwrap()
            .districts
            .insert("campus".to_string(), campus);
        game.players[1].techs.insert("writing".to_string());
        let spy = game.next_id;
        game.next_id += 1;
        game.spies.insert(
            spy,
            crate::game::Spy {
                id: spy,
                owner: 0,
                level: 2,
                promotions: ["technologist".to_string(), "disguise".to_string()]
                    .into_iter()
                    .collect(),
                city: Some(target),
                ready_turn: game.turn,
                mission: None,
                sources_city: Some(target),
                sources_until: game.turn + 24,
                captured_by: None,
            },
        );
        let plan = StrategicPlan {
            strategy: GrandStrategy::Science,
            target_player: Some(1),
            target_city: Some(target),
            threatened_city: None,
            desired_cities: 3,
            assessed_turn: game.turn,
        };

        AdvancedAi::targeting(VictoryTarget::Science).advanced_spies(&mut game, 0, &plan);
        assert_eq!(
            game.spies[&spy]
                .mission
                .as_ref()
                .map(|mission| mission.kind.as_str()),
            Some("steal_tech_boost")
        );
    }
}
