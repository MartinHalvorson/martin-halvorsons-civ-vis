//! Ruleset loaded from the shared JSON data files (embedded at compile time).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
}

#[derive(Clone, Serialize, Deserialize)]
pub struct GovSpec {
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub effects: GovEffects,
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
    pub techs: BTreeMap<String, TechSpec>,
    pub civics: BTreeMap<String, TechSpec>,
    pub governments: BTreeMap<String, GovSpec>,
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
            techs: serde_json::from_str(include_str!("../data/techs.json")).unwrap(),
            civics: serde_json::from_str(include_str!("../data/civics.json")).unwrap(),
            governments: serde_json::from_str(include_str!("../data/governments.json")).unwrap(),
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
        c
    }
}
