//! Stateful, hierarchical AI for major civilizations.
//!
//! `BasicAi` deliberately remains the small deterministic baseline.  This
//! agent adds a shared strategic model so research, production, diplomacy,
//! civilian work, and military movement pursue the same medium-term goal.
use super::{Ai, BasicAi, Weights};
use crate::game::{Action, Game, Item};
use crate::rules::Yields;
use crate::Pos;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrandStrategy {
    Expansion,
    Science,
    Culture,
    Conquest,
    Recovery,
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

#[derive(Default)]
struct EmpireCounts {
    settlers: usize,
    builders: usize,
    traders: usize,
    scouts: usize,
    military: usize,
    melee: usize,
    ranged: usize,
    siege: usize,
}

impl EmpireCounts {
    fn add_unit(&mut self, g: &Game, name: &str) {
        match name {
            "settler" => self.settlers += 1,
            "builder" => self.builders += 1,
            "trader" => self.traders += 1,
            "scout" => {
                self.scouts += 1;
                self.military += 1;
                self.melee += 1;
            }
            _ => {
                let spec = &g.rules.units[name];
                if spec.class == "military" {
                    self.military += 1;
                    if spec.has_ranged_attack() {
                        self.ranged += 1;
                    } else {
                        self.melee += 1;
                    }
                    if spec.siege {
                        self.siege += 1;
                    }
                }
            }
        }
    }

    fn add_item(&mut self, g: &Game, item: &Item) {
        if let Item::Unit { unit } = item {
            self.add_unit(g, unit);
        }
    }
}

pub struct AdvancedAi {
    base: BasicAi,
    plan: Option<StrategicPlan>,
    settler_targets: BTreeMap<u32, Pos>,
    builder_targets: BTreeMap<u32, Pos>,
}

impl Default for AdvancedAi {
    fn default() -> Self {
        Self::new()
    }
}

impl AdvancedAi {
    pub fn new() -> AdvancedAi {
        AdvancedAi {
            base: BasicAi::new(),
            plan: None,
            settler_targets: BTreeMap::new(),
            builder_targets: BTreeMap::new(),
        }
    }

    pub fn with_weights(weights: Weights) -> AdvancedAi {
        AdvancedAi {
            base: BasicAi::with_weights(weights),
            plan: None,
            settler_targets: BTreeMap::new(),
            builder_targets: BTreeMap::new(),
        }
    }

    pub fn fleet(g: &Game) -> Vec<AdvancedAi> {
        g.players.iter().map(|_| AdvancedAi::new()).collect()
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
        false
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
        let at_war = major_rivals.iter().any(|o| g.is_at_war(pid, *o));
        let strongest_rival = major_rivals
            .iter()
            .map(|o| g.military_power(*o))
            .fold(0.0_f64, f64::max);

        let threatened_city = cities
            .iter()
            .filter_map(|cid| {
                let c = &g.cities[cid];
                let nearby = g
                    .units
                    .values()
                    .filter(|u| u.owner != pid && g.is_at_war(pid, u.owner))
                    .map(|u| g.wdist(c.pos, u.pos))
                    .min()
                    .unwrap_or(i32::MAX);
                let recently_hit =
                    c.last_attacked > 0 && g.turn.saturating_sub(c.last_attacked) <= 3;
                (nearby <= 6 || recently_hit).then_some((nearby, c.hp, *cid))
            })
            .min()
            .map(|(_, _, cid)| cid);

        let land = g
            .map
            .tiles
            .values()
            .filter(|t| g.rules.is_passable(t) && !g.rules.is_water(t))
            .count();
        let map_capacity = (2 + land / 55).clamp(3, 9);
        let desired_cities = (3 + g.turn as usize / 38).min(map_capacity);
        let mut expansion_origins: Vec<Pos> = cities.iter().map(|cid| g.cities[cid].pos).collect();
        if expansion_origins.is_empty() {
            expansion_origins.extend(
                g.player_unit_ids(pid)
                    .into_iter()
                    .filter(|uid| g.units[uid].kind == "settler")
                    .map(|uid| g.units[&uid].pos),
            );
        }
        let has_site = expansion_origins
            .iter()
            .any(|pos| self.best_settle_site(g, pid, *pos, 10).is_some());

        let military_civ = matches!(
            g.players[pid].civ.as_str(),
            "Sumeria" | "Aztec" | "Nubia" | "Scythia"
        );
        let strategy = if at_war && (threatened_city.is_some() || my_power * 1.25 < strongest_rival)
        {
            GrandStrategy::Recovery
        } else if at_war
            || (military_civ
                && g.turn >= 35
                && cities.len() >= 2
                && my_power >= strongest_rival * 1.10)
        {
            GrandStrategy::Conquest
        } else if cities.len() < desired_cities && has_site && g.turn < 175 {
            GrandStrategy::Expansion
        } else if g.players[pid].civ == "Greece" {
            GrandStrategy::Culture
        } else {
            GrandStrategy::Science
        };

        let target_player = major_rivals
            .iter()
            .min_by(|a, b| {
                self.rival_value(g, pid, **a)
                    .partial_cmp(&self.rival_value(g, pid, **b))
                    .unwrap()
                    .then(a.cmp(b))
            })
            .copied();
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
        distance * 7.0 + g.military_power(other) * 1.5 - g.score(other) as f64 * 0.35
    }

    fn yield_value(&self, yields: Yields, strategy: GrandStrategy) -> f64 {
        let (food, prod, gold, science, culture, faith) = match strategy {
            GrandStrategy::Expansion => (2.0, 2.2, 0.9, 1.2, 1.2, 0.5),
            GrandStrategy::Science => (1.4, 2.0, 1.0, 4.2, 1.2, 0.4),
            GrandStrategy::Culture => (1.4, 1.8, 1.0, 1.3, 4.2, 0.8),
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

    fn advanced_research(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        if g.players[pid].research.is_none() {
            let pick = g.available_techs(pid).into_iter().max_by(|a, b| {
                self.tech_value(g, pid, a, plan.strategy)
                    .partial_cmp(&self.tech_value(g, pid, b, plan.strategy))
                    .unwrap()
                    .then_with(|| b.cmp(a))
            });
            if let Some(tech) = pick {
                let _ = g.apply(pid, &Action::Research { tech });
            }
        }
        if g.players[pid].civic.is_none() {
            let pick = g.available_civics(pid).into_iter().max_by(|a, b| {
                self.civic_value(g, pid, a, plan.strategy)
                    .partial_cmp(&self.civic_value(g, pid, b, plan.strategy))
                    .unwrap()
                    .then_with(|| b.cmp(a))
            });
            if let Some(civic) = pick {
                let _ = g.apply(pid, &Action::Civic { civic });
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
        for improvement in g
            .rules
            .improvements
            .values()
            .filter(|i| i.tech.as_deref() == Some(tech))
        {
            value += self.yield_value(improvement.yields, strategy) * 10.0 + 18.0;
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
        (value + 32.0) / spec.cost.max(10.0).sqrt()
    }

    fn advanced_diplomacy(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        let my_power = g.military_power(pid);
        let rivals: Vec<usize> = g
            .players
            .iter()
            .filter(|p| p.id != pid && p.alive && !p.is_barbarian)
            .map(|p| p.id)
            .collect();
        for other in &rivals {
            if g.is_at_war(pid, *other)
                && !g.players[*other].is_minor
                && (my_power < g.military_power(*other) * 0.62
                    || (plan.strategy == GrandStrategy::Recovery
                        && plan.target_player != Some(*other)))
            {
                let _ = g.apply(pid, &Action::MakePeace { player: *other });
            }
        }
        let major_wars = rivals
            .iter()
            .filter(|o| !g.players[**o].is_minor && g.is_at_war(pid, **o))
            .count();
        let Some(target) = plan.target_player else {
            return;
        };
        if plan.strategy != GrandStrategy::Conquest
            || major_wars > 0
            || g.turn < 35
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
        if close_enough && my_power > target_power * 1.32 + 12.0 {
            let _ = g.apply(pid, &Action::DeclareWar { player: target });
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

    fn advanced_production(&self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        let mut counts = self.counts(g, pid);
        let city_ids = g.player_city_ids(pid);
        for cid in city_ids {
            if !g.cities[&cid].queue.is_empty() {
                continue;
            }
            let mut best: Option<(f64, String, Item)> = None;
            for item in g.producible_items(pid, cid) {
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
        let turns = g.item_cost(item) / production;
        let threatened = plan.threatened_city == Some(cid)
            || (city.last_attacked > 0 && g.turn.saturating_sub(city.last_attacked) <= 4);
        let desired_military = match plan.strategy {
            GrandStrategy::Conquest => 2 * city_count + 2,
            GrandStrategy::Recovery => 2 * city_count + 1,
            _ => city_count + 1,
        };
        let raw = match item {
            Item::Unit { unit } if unit == "settler" => {
                let site = self.best_settle_site(g, pid, city.pos, 11);
                if city_count + counts.settlers < plan.desired_cities
                    && counts.settlers == 0
                    && city.pop >= 2
                    && g.turn < 175
                    && site.is_some()
                {
                    660.0 + site.map(|(_, v)| v * 4.0).unwrap_or(0.0)
                } else {
                    -10_000.0
                }
            }
            Item::Unit { unit } if unit == "builder" => {
                let desired = (3 * city_count).div_ceil(4).max(1);
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
                if g.players[pid].religion.is_some() {
                    150.0
                } else {
                    20.0
                }
            }
            Item::Unit { unit } => {
                let spec = &g.rules.units[unit];
                if spec.class == "military" {
                    if unit == "scout" && counts.scouts >= 1 {
                        return -2_000.0;
                    }
                    let power = spec.strength.max(spec.ranged_attack_strength());
                    let force_gap = desired_military.saturating_sub(counts.military) as f64;
                    let role_gap = if force_gap <= 0.0 {
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
                } else if spec.class == "support" && plan.strategy == GrandStrategy::Conquest {
                    180.0
                } else {
                    20.0
                }
            }
            Item::Building { building } => {
                let spec = &g.rules.buildings[building];
                if spec.wonder {
                    let wonder_civ = matches!(g.players[pid].civ.as_str(), "Egypt" | "China");
                    if threatened || (city.buildings.len() < 3 && !wonder_civ) {
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
                    self.yield_value(spec.yields, plan.strategy) * 42.0
                        + spec.housing * (22.0 + housing_need * 18.0)
                        + spec.amenity * (30.0 + amenity_need * 22.0)
                        + if building == "monument" && g.turn < 90 {
                            105.0
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
                self.yield_value(g.district_yields(district, *pos), plan.strategy) * 60.0
                    + spec.defense * if threatened { 5.0 } else { 1.5 }
                    + spec.amenity * 50.0
                    + match (plan.strategy, district.as_str()) {
                        (GrandStrategy::Science, "campus") => 170.0,
                        (GrandStrategy::Culture, "theater_square") => 170.0,
                        (GrandStrategy::Conquest, "encampment") => 130.0,
                        (GrandStrategy::Recovery, "industrial_zone") => 130.0,
                        (GrandStrategy::Expansion, "commercial_hub") => 90.0,
                        _ => 0.0,
                    }
            }
        };
        raw / (7.0 + turns.max(1.0))
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
        let fresh = tile.river
            || g.nbrs(pos).iter().any(|p| {
                g.map
                    .get(*p)
                    .is_some_and(|t| t.river || t.feature.as_deref() == Some("oasis"))
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

    fn best_settle_site(&self, g: &Game, pid: usize, from: Pos, radius: i32) -> Option<(Pos, f64)> {
        let mut best: Option<(Pos, f64)> = None;
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
            let value = self.settle_value(g, pid, pos) - g.wdist(from, pos) as f64 * 0.9;
            if best
                .map(|(bp, bv)| value > bv || (value == bv && pos < bp))
                .unwrap_or(true)
            {
                best = Some((pos, value));
            }
        }
        best.filter(|(_, value)| *value >= 18.0)
    }

    fn advanced_settler_step(&mut self, g: &mut Game, pid: usize, uid: u32) -> bool {
        let current = g.units[&uid].pos;
        // Map generation already places each starting party in a viable
        // region. Delaying the capital for a theoretical optimum sacrifices
        // many turns of compounding yields and can strand an entire empire.
        if g.player_city_ids(pid).is_empty() && g.can_found_city(uid) {
            return g.apply(pid, &Action::FoundCity { unit: uid }).is_ok();
        }
        let valid_target = self.settler_targets.get(&uid).copied().filter(|target| {
            g.map.get(*target).is_some() && !g.cities.values().any(|c| g.wdist(c.pos, *target) < 4)
        });
        let target = valid_target.or_else(|| {
            self.best_settle_site(g, pid, current, 8).map(|(pos, _)| {
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
        self.base.step_toward(g, pid, uid, target)
    }

    fn improvement_value(
        &self,
        g: &Game,
        pos: Pos,
        improvement: &str,
        strategy: GrandStrategy,
    ) -> f64 {
        let tile = &g.map.tiles[&pos];
        let mut value = self.yield_value(g.rules.improvements[improvement].yields, strategy);
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
        let mut here = g.valid_improvements(pid, current);
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
        let current_target =
            self.builder_targets.get(&uid).copied().filter(|pos| {
                !reserved.contains(pos) && !g.valid_improvements(pid, *pos).is_empty()
            });
        let target = current_target.or_else(|| {
            let mut best: Option<(f64, Pos)> = None;
            for cid in g.player_city_ids(pid) {
                for pos in &g.cities[&cid].owned_tiles {
                    if reserved.contains(pos) {
                        continue;
                    }
                    for improvement in g.valid_improvements(pid, *pos) {
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

    fn advanced_military_step(
        &self,
        g: &mut Game,
        pid: usize,
        uid: u32,
        plan: &StrategicPlan,
    ) -> bool {
        let unit = g.units[&uid].clone();
        let spec = g.rules.units[unit.kind.as_str()].clone();
        let enemies: Vec<usize> = g
            .players
            .iter()
            .filter(|p| p.id != pid && p.alive && g.is_at_war(pid, p.id))
            .map(|p| p.id)
            .collect();
        if enemies.is_empty() {
            return self.base.military_step(g, pid, uid);
        }
        if unit.hp <= 32 {
            let refuge = g
                .player_city_ids(pid)
                .iter()
                .min_by_key(|cid| (g.wdist(unit.pos, g.cities[cid].pos), **cid))
                .map(|cid| g.cities[cid].pos);
            if let Some(pos) = refuge {
                if g.wdist(unit.pos, pos) > 1 && self.base.step_toward(g, pid, uid, pos) {
                    return true;
                }
                return self.base.fortify_or_stop(g, pid, uid);
            }
        }

        let ranged = spec.has_ranged_attack();
        let radius = if ranged { spec.range.max(1) } else { 1 };
        let mut best: Option<(f64, Pos)> = None;
        for pos in g.wdisk(unit.pos, radius) {
            if pos == unit.pos || !self.base.is_enemy_tile(g, pos, &enemies) {
                continue;
            }
            let mut score = self.base.exchange_score(g, uid, pos, ranged);
            if plan
                .target_city
                .is_some_and(|cid| g.cities.get(&cid).is_some_and(|c| c.pos == pos))
            {
                score += 28.0;
            }
            if g.units_at(pos).iter().any(|oid| g.units[oid].hp <= 35) {
                score += 16.0;
            }
            if best
                .map(|(old, bp)| score > old || (score == old && pos < bp))
                .unwrap_or(true)
            {
                best = Some((score, pos));
            }
        }
        if let Some((score, pos)) = best {
            let threshold = if unit.hp < 55 { 12.0 } else { -2.0 };
            if score > threshold {
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

        let defend_target = plan.threatened_city.and_then(|cid| {
            let city = g.cities.get(&cid)?;
            g.units
                .values()
                .filter(|u| enemies.contains(&u.owner) && g.wdist(city.pos, u.pos) <= 7)
                .min_by_key(|u| (g.wdist(unit.pos, u.pos), u.id))
                .map(|u| u.pos)
        });
        let campaign = defend_target
            .or_else(|| {
                plan.target_city
                    .and_then(|cid| g.cities.get(&cid).map(|c| c.pos))
            })
            .or_else(|| self.base.nearest_enemy(g, pid, unit.pos, &enemies));
        match campaign {
            Some(target) => self
                .base
                .tactical_step(g, pid, uid, target, &enemies, radius),
            None => self.base.fortify_or_stop(g, pid, uid),
        }
    }

    fn advanced_units(&mut self, g: &mut Game, pid: usize, plan: &StrategicPlan) {
        let mut ids = g.player_unit_ids(pid);
        ids.sort_by_key(|uid| {
            let u = &g.units[uid];
            let spec = &g.rules.units[u.kind.as_str()];
            let order = match u.kind.as_str() {
                "settler" => 0,
                "builder" => 1,
                "trader" => 2,
                "missionary" => 3,
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
                let acted = match kind.as_str() {
                    "settler" => self.advanced_settler_step(g, pid, uid),
                    "builder" => self.advanced_builder_step(g, pid, uid, plan.strategy),
                    "trader" => self.advanced_trader_step(g, pid, uid, plan.strategy),
                    "missionary" => self.base.missionary_step(g, pid, uid),
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
}

impl Ai for AdvancedAi {
    fn take_turn(&mut self, g: &mut Game, pid: usize) {
        self.base.minor = g.players[pid].is_minor;
        self.base.barb = g.players[pid].is_barbarian;
        if self.base.minor || self.base.barb {
            self.base.take_turn(g, pid);
            return;
        }
        if self.plan_stale(g, pid) {
            self.plan = Some(self.assess(g, pid));
        }
        let plan = self.plan.clone().unwrap();
        self.advanced_research(g, pid, &plan);
        // Keep the mature ancillary systems: governments, policies, beliefs,
        // governors, religions, and envoys. Research is already selected.
        self.base.research(g, pid);
        self.advanced_diplomacy(g, pid, &plan);

        // Preserve the proven four-build opening before switching every city
        // to utility planning. This also keeps the frozen baseline comparable.
        if self.base.book_pos < 4 {
            self.base.cities(g, pid);
        } else {
            self.advanced_production(g, pid, &plan);
            // Handles city strikes and strategic currency spending; filled
            // queues prevent its fallback production policy from taking over.
            self.base.cities(g, pid);
        }
        self.advanced_units(g, pid, &plan);
        if g.winner.is_none() && g.current == pid {
            let _ = g.apply(pid, &Action::EndTurn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::run_game;

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
    }
}
