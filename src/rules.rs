//! Ruleset loaded from the shared JSON data files (embedded at compile time).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn default_true() -> bool {
    true
}

fn default_one_limit() -> Option<usize> {
    Some(1)
}

use crate::world::Tile;

pub const ERA_NAMES: [&str; 9] = [
    "ancient",
    "classical",
    "medieval",
    "renaissance",
    "industrial",
    "modern",
    "atomic",
    "information",
    "future",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Yields {
    pub food: f64,
    pub production: f64,
    pub gold: f64,
    pub science: f64,
    pub culture: f64,
    pub faith: f64,
}

impl Yields {
    pub fn add(&mut self, o: Yields) {
        self.food += o.food;
        self.production += o.production;
        self.gold += o.gold;
        self.science += o.science;
        self.culture += o.culture;
        self.faith += o.faith;
    }
    pub fn total(&self) -> f64 {
        self.food + self.production + self.gold + self.science + self.culture + self.faith
    }
}

fn dtrue() -> bool {
    true
}
fn done() -> f64 {
    1.0
}
fn dsight() -> i32 {
    2
}
fn done_i() -> i64 {
    1
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TerrainSpec {
    #[serde(default)]
    pub yields: Yields,
    #[serde(default)]
    pub water: bool,
    #[serde(default = "dtrue")]
    pub passable: bool,
    #[serde(default = "done")]
    pub move_cost: f64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FeatureSpec {
    #[serde(default)]
    pub yields: Yields,
    #[serde(default = "done")]
    pub move_cost: f64,
    #[serde(default)]
    pub natural_wonder: bool,
    #[serde(default)]
    pub impassable: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ResourceSpec {
    pub class: String,
    #[serde(default)]
    pub yields: Yields,
    #[serde(default)]
    pub terrain: Vec<String>,
    #[serde(default)]
    pub feature: Vec<String>,
    pub improvement: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ImprovementSpec {
    #[serde(default)]
    pub tech: Option<String>,
    #[serde(default)]
    pub yields: Yields,
    #[serde(default)]
    pub housing: f64,
    #[serde(default)]
    pub terrain: Vec<String>,
    #[serde(default)]
    pub removes_feature: bool,
    #[serde(default)]
    pub water: bool,
    #[serde(default)]
    pub unbuildable: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UnitSpec {
    pub class: String,
    pub cost: f64,
    pub moves: f64,
    #[serde(default)]
    pub strength: f64,
    #[serde(default)]
    pub ranged_strength: f64, // 0 = no ranged attack
    #[serde(default)]
    pub bombard_strength: f64, // 0 = no anti-district bombard attack
    #[serde(default)]
    pub range: i32,
    #[serde(default)]
    pub charges: i32,
    #[serde(default = "dsight")]
    pub sight: i32,
    #[serde(default)]
    pub tech: Option<String>,
    #[serde(default)]
    pub requires_resource: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub unique_to: Option<String>, // civ that alone may build this unit
    #[serde(default)]
    pub replaces: Option<String>, // base unit this unique replaces
    #[serde(default)]
    pub promotion_class: String,
    #[serde(default)]
    pub zone_of_control: bool,
    #[serde(default)]
    pub cavalry: bool, // light, heavy, and ranged cavalry ignore enemy ZOC
    #[serde(default)]
    pub siege: bool, // full damage vs city walls
    #[serde(default)]
    pub religious_strength: f64,
    /// Base pressure from one Spread Religion charge.
    #[serde(default)]
    pub religious_spread: f64,
    /// Religious units are faith-purchased in a city containing this building.
    #[serde(default)]
    pub requires_building: Option<String>,
}

impl UnitSpec {
    pub fn ranged_attack_strength(&self) -> f64 {
        self.ranged_strength.max(self.bombard_strength)
    }

    pub fn has_ranged_attack(&self) -> bool {
        self.ranged_attack_strength() > 0.0
    }

    pub fn is_melee_capable(&self) -> bool {
        self.class == "military" && !self.has_ranged_attack()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DistrictSpec {
    pub cost: f64,
    #[serde(default)]
    pub tech: Option<String>,
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub yields: Yields,
    #[serde(default)]
    pub adjacency: BTreeMap<String, Yields>,
    #[serde(default)]
    pub water: bool,
    #[serde(default)]
    pub defense: f64,
    #[serde(default)]
    pub amenity: f64,
    #[serde(default)]
    pub housing: f64,
    /// Specialty districts consume the 1/4/7/... population capacity.
    #[serde(default = "default_true")]
    pub specialty: bool,
    #[serde(default = "default_true")]
    pub buildable: bool,
    /// `null` means that a city may construct multiple copies (Neighborhood
    /// and Canal); omitted entries default to the normal one-per-city rule.
    #[serde(default = "default_one_limit")]
    pub max_per_city: Option<usize>,
    /// `null` means no empire-wide cap. Government Plaza and Diplomatic
    /// Quarter use one; ordinary districts omit it.
    #[serde(default)]
    pub max_per_empire: Option<usize>,
    #[serde(default)]
    pub unique_to: Option<String>,
    #[serde(default)]
    pub replaces: Option<String>,
    /// IDs of district families that cannot coexist in the same city (for
    /// example Entertainment Complex and Water Park).
    #[serde(default)]
    pub excludes: Vec<String>,
    /// Placement rule interpreted by `Game::district_sites`.
    #[serde(default)]
    pub placement: String,
    #[serde(default)]
    pub trade_route_capacity: i32,
    #[serde(default)]
    pub air_slots: i32,
    #[serde(default)]
    pub appeal: f64,
    #[serde(default)]
    pub loyalty: f64,
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BuildingSpec {
    pub cost: f64,
    #[serde(default = "default_true")]
    pub buildable: bool,
    #[serde(default)]
    pub tech: Option<String>,
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub district: Option<String>,
    #[serde(default)]
    pub yields: Yields,
    #[serde(default)]
    pub housing: f64,
    #[serde(default)]
    pub amenity: f64,
    #[serde(default)]
    pub wonder: bool,
    #[serde(default)]
    pub coastal: bool,
    #[serde(default)]
    pub growth_pct: f64,
    #[serde(default)]
    pub builder_charges: i32,
    #[serde(default)]
    pub unit_levels: i32,
    #[serde(default)]
    pub unique_to: Option<String>,
    #[serde(default)]
    pub replaces: Option<String>,
    /// Buildings that must already exist in this city.
    #[serde(default)]
    pub requires: Vec<String>,
    /// At least one member of this list must exist. Replacement-family
    /// matching applies, so a unique replacement satisfies a base entry.
    #[serde(default)]
    pub requires_any: Vec<String>,
    /// Mutually exclusive buildings in the same tier or Government Plaza
    /// choice.
    #[serde(default)]
    pub excludes: Vec<String>,
    #[serde(default)]
    pub power: f64,
    #[serde(default)]
    pub maintenance: f64,
    #[serde(default)]
    pub outer_defense: i32,
    #[serde(default)]
    pub citizen_slots: i32,
    #[serde(default)]
    pub great_work_slots: BTreeMap<String, i32>,
    #[serde(default)]
    pub great_person_points: BTreeMap<String, f64>,
    #[serde(default)]
    pub regional_range: i32,
    #[serde(default)]
    pub trade_route_capacity: i32,
    /// Free-form numeric rule primitives used by named effects that are not
    /// plain yields (production modifiers, combat strength, tourism, etc.).
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
    /// Worship buildings are selected by this religion belief and purchased
    /// with Faith rather than constructed with Production.
    #[serde(default)]
    pub worship_belief: Option<String>,
    #[serde(default)]
    pub purchase_only: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectSpec {
    pub cost: f64,
    #[serde(default)]
    pub tech: Option<String>,
    #[serde(default)]
    pub district: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub repeatable: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BoostSpec {
    pub trigger: String,
    #[serde(default = "done_i")]
    pub count: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TechSpec {
    pub cost: f64,
    /// Zero-based historical era: Ancient through Future.
    pub era: usize,
    pub requires: Vec<String>,
    #[serde(default)]
    pub boost: Option<BoostSpec>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GovEffects {
    pub production_pct: f64,
    pub science_pct: f64,
    pub gold_pct: f64,
    pub combat_strength: f64,
    pub amenity: f64,
    pub housing: f64,
    pub great_people_pct: f64,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicySlots {
    pub military: i64,
    pub economic: i64,
    pub diplomatic: i64,
    pub wildcard: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct GovSpec {
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub effects: GovEffects,
    #[serde(default)]
    pub slots: PolicySlots,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CivSpec {
    pub leader: String,
    pub ability: String,
    #[serde(default)]
    pub unique_unit: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BeliefSpec {
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BeliefsData {
    pub pantheon: BTreeMap<String, BeliefSpec>,
    pub founder: BTreeMap<String, BeliefSpec>,
    pub follower: BTreeMap<String, BeliefSpec>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PolicySpec {
    pub slot: String, // military | economic | diplomatic | wildcard
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub replaces: Option<String>, // unlocking this obsoletes the named card
    #[serde(default)]
    pub note: String,
}

/// A stock unit-promotion node. Effects are numeric flags so rules data can
/// add promotions without changing the action/state protocol.
#[derive(Clone, Serialize, Deserialize)]
pub struct PromotionSpec {
    pub class: String,
    pub tier: i32,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone)]
pub struct Rules {
    pub terrains: BTreeMap<String, TerrainSpec>,
    pub features: BTreeMap<String, FeatureSpec>,
    pub resources: BTreeMap<String, ResourceSpec>,
    pub improvements: BTreeMap<String, ImprovementSpec>,
    pub units: BTreeMap<String, UnitSpec>,
    pub districts: BTreeMap<String, DistrictSpec>,
    pub buildings: BTreeMap<String, BuildingSpec>,
    pub projects: BTreeMap<String, ProjectSpec>,
    pub techs: BTreeMap<String, TechSpec>,
    pub civics: BTreeMap<String, TechSpec>,
    pub governments: BTreeMap<String, GovSpec>,
    pub policies: BTreeMap<String, PolicySpec>,
    pub promotions: BTreeMap<String, PromotionSpec>,
    pub beliefs: BeliefsData,
    pub civs: BTreeMap<String, CivSpec>,
}

impl Rules {
    pub fn embedded() -> Rules {
        Rules {
            terrains: serde_json::from_str(include_str!("../data/terrains.json")).unwrap(),
            features: serde_json::from_str(include_str!("../data/features.json")).unwrap(),
            resources: serde_json::from_str(include_str!("../data/resources.json")).unwrap(),
            improvements: serde_json::from_str(include_str!("../data/improvements.json")).unwrap(),
            units: serde_json::from_str(include_str!("../data/units.json")).unwrap(),
            districts: serde_json::from_str(include_str!("../data/districts.json")).unwrap(),
            buildings: serde_json::from_str(include_str!("../data/buildings.json")).unwrap(),
            projects: serde_json::from_str(include_str!("../data/projects.json")).unwrap(),
            techs: serde_json::from_str(include_str!("../data/techs.json")).unwrap(),
            civics: serde_json::from_str(include_str!("../data/civics.json")).unwrap(),
            governments: serde_json::from_str(include_str!("../data/governments.json")).unwrap(),
            policies: serde_json::from_str(include_str!("../data/policies.json")).unwrap(),
            promotions: serde_json::from_str(include_str!("../data/promotions.json")).unwrap(),
            beliefs: serde_json::from_str(include_str!("../data/beliefs.json")).unwrap(),
            civs: serde_json::from_str(include_str!("../data/civs.json")).unwrap(),
        }
    }

    pub fn tile_yields(&self, t: &Tile) -> Yields {
        let mut ys = self.terrains[t.terrain.as_str()].yields;
        if t.hills {
            ys.production += 1.0;
        }
        if let Some(f) = &t.feature {
            ys.add(self.features[f.as_str()].yields);
        }
        if let Some(r) = &t.resource {
            ys.add(self.resources[r.as_str()].yields);
        }
        if let Some(i) = &t.improvement {
            ys.add(self.improvements[i.as_str()].yields);
        }
        ys
    }

    pub fn is_water(&self, t: &Tile) -> bool {
        self.terrains[t.terrain.as_str()].water
    }

    pub fn is_passable(&self, t: &Tile) -> bool {
        if let Some(f) = &t.feature {
            if self.features[f.as_str()].impassable {
                return false;
            }
        }
        self.terrains[t.terrain.as_str()].passable
    }

    pub fn move_cost(&self, t: &Tile) -> f64 {
        let mut c = self.terrains[t.terrain.as_str()].move_cost;
        if let Some(f) = &t.feature {
            c = c.max(self.features[f.as_str()].move_cost);
        }
        if t.hills {
            c = c.max(2.0);
        }
        if t.road && !self.terrains[t.terrain.as_str()].water {
            c = 1.0; // roads flatten terrain (Civ 6 ancient roads)
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const TECHS: &str = "
        pottery animal_husbandry mining sailing astrology irrigation archery writing masonry
        bronze_working wheel horseback_riding currency celestial_navigation iron_working
        shipbuilding mathematics construction engineering apprenticeship buttress machinery
        military_tactics stirrups castles education military_engineering banking cartography
        gunpowder mass_production printing astronomy metal_casting siege_tactics square_rigging
        ballistics industrialization military_science scientific_theory economics rifling
        sanitation steam_power flight refining replaceable_parts steel chemistry combustion
        electricity radio advanced_ballistics advanced_flight combined_arms plastics rocketry
        computers nuclear_fission synthetic_materials composites guidance_systems lasers
        satellites stealth_technology telecommunications nanotechnology nuclear_fusion robotics
        seasteads advanced_ai advanced_power_cells cybernetics smart_materials predictive_systems
        offworld_mission future_tech";

    const CIVICS: &str = "
        code_of_laws craftsmanship foreign_trade military_tradition mysticism early_empire
        state_workforce drama_poetry games_recreation political_philosophy military_training
        theology defensive_tactics recorded_history naval_tradition civil_service feudalism
        divine_right mercenaries guilds medieval_faires exploration reformed_church
        diplomatic_service humanism mercantilism the_enlightenment colonialism opera_ballet
        civil_engineering nationalism natural_history scorched_earth urbanization conservation
        mass_media mobilization capitalism class_struggle ideology suffrage totalitarianism
        nuclear_program cultural_heritage cold_war professional_sports rapid_deployment
        space_race environmentalism globalization social_media digital_democracy
        synthetic_technocracy corporate_libertarianism near_future_governance
        information_warfare global_warming_mitigation cultural_hegemony smart_power_doctrine
        exodus_imperative future_civic";

    fn assert_complete_tree(
        tree: &BTreeMap<String, TechSpec>,
        expected: &str,
        era_counts: [usize; 9],
    ) {
        let actual: BTreeSet<&str> = tree.keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = expected.split_whitespace().collect();
        assert_eq!(actual, expected);

        let mut counts = [0; 9];
        for (name, spec) in tree {
            assert!(spec.cost > 0.0, "{name} has no research cost");
            assert!(
                spec.era < ERA_NAMES.len(),
                "{name} has invalid era {}",
                spec.era
            );
            counts[spec.era] += 1;
            for prerequisite in &spec.requires {
                let parent = tree
                    .get(prerequisite)
                    .unwrap_or_else(|| panic!("{name} requires missing node {prerequisite}"));
                assert!(
                    parent.era <= spec.era,
                    "{name} requires later-era node {prerequisite}"
                );
            }
        }
        assert_eq!(counts, era_counts);

        // Repeatedly remove nodes whose prerequisites have been removed. If
        // anything remains, the graph contains a cycle or an unreachable root.
        let mut reached = BTreeSet::new();
        while reached.len() < tree.len() {
            let before = reached.len();
            for (name, spec) in tree {
                if spec.requires.iter().all(|node| reached.contains(node)) {
                    reached.insert(name.clone());
                }
            }
            assert!(reached.len() > before, "tree contains a dependency cycle");
        }
    }

    #[test]
    fn gathering_storm_technology_and_civics_trees_are_complete() {
        let rules = Rules::embedded();
        assert_complete_tree(&rules.techs, TECHS, [11, 8, 8, 9, 8, 8, 8, 9, 8]);
        assert_complete_tree(&rules.civics, CIVICS, [7, 7, 7, 6, 7, 9, 5, 7, 6]);
    }

    #[test]
    fn modeled_unit_classes_have_complete_promotion_trees() {
        let rules = Rules::embedded();
        let classes: BTreeSet<_> = rules
            .units
            .values()
            .filter(|unit| !unit.promotion_class.is_empty())
            .map(|unit| unit.promotion_class.as_str())
            .collect();
        assert_eq!(rules.promotions.len(), classes.len() * 7);
        for class in classes {
            let nodes: Vec<_> = rules
                .promotions
                .iter()
                .filter(|(_, promotion)| promotion.class == class)
                .collect();
            assert_eq!(nodes.len(), 7, "{class} promotion tree");
            for (name, promotion) in nodes {
                assert!((1..=4).contains(&promotion.tier), "{name} tier");
                for prerequisite in &promotion.requires {
                    let required = rules
                        .promotions
                        .get(prerequisite)
                        .unwrap_or_else(|| panic!("{name} requires missing {prerequisite}"));
                    assert_eq!(required.class, class, "{name} crosses unit classes");
                    assert!(required.tier <= promotion.tier, "{name} prerequisite tier");
                }
                assert!(
                    promotion.requires.is_empty()
                        || promotion.requires.iter().any(|prerequisite| {
                            rules.promotions[prerequisite].tier < promotion.tier
                        }),
                    "{name} has no prerequisite from an earlier tier"
                );
            }
        }
    }
}
