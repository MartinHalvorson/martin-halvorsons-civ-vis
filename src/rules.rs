//! Ruleset loaded from the shared JSON data files (embedded at compile time).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn default_true() -> bool {
    true
}

use crate::world::Tile;

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
    /// Specialty districts consume the 1/4/7/... population capacity.
    #[serde(default = "default_true")]
    pub specialty: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BuildingSpec {
    pub cost: f64,
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
