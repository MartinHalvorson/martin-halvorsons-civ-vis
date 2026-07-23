//! Ruleset loaded from the shared JSON data files (embedded at compile time).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

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
fn done_usize() -> usize {
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
    /// Natural wonders whose Civilopedia entry reads "to adjacent tiles"
    /// project these yields onto each neighbouring tile instead of their own.
    #[serde(default)]
    pub adjacent_yields: Yields,
    /// Movement added on top of the terrain cost, the game database's
    /// ``MovementChange`` column: Woods on Hills costs 1 + 1 + 1 = 3 MP.
    #[serde(default)]
    pub move_cost: f64,
    #[serde(default)]
    pub natural_wonder: bool,
    #[serde(default)]
    pub impassable: bool,
    /// The shipped Feature_Removes yields a Builder collects for clearing
    /// this feature (base values; the payout scales with the era).
    #[serde(default)]
    pub chop: BTreeMap<String, f64>,
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ResourceSpec {
    pub class: String,
    /// Strategic and archaeological resources remain hidden until this node.
    #[serde(default)]
    pub tech: Option<String>,
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub yields: Yields,
    #[serde(default)]
    pub terrain: Vec<String>,
    #[serde(default)]
    pub feature: Vec<String>,
    /// Some(true) for hills-only spawns (Sheep), Some(false) for flat-only
    /// (Wheat, Rice, Maize, Bananas), None when either form works.
    #[serde(default)]
    pub hills: Option<bool>,
    /// The shipped Resource_Harvests row: only these bonus resources can be
    /// harvested by a Builder, for this yield, from this technology on.
    #[serde(default)]
    pub harvest: Option<HarvestSpec>,
    /// Empty for luxuries no tile improvement works (Toys, Jeans, Perfume,
    /// Cosmetics — manufactured, never map-placed).
    #[serde(default)]
    pub improvement: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HarvestSpec {
    #[serde(rename = "yield")]
    pub yield_type: String,
    pub amount: f64,
    #[serde(default)]
    pub tech: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ImprovementSpec {
    #[serde(default)]
    pub tech: Option<String>,
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub yields: Yields,
    #[serde(default)]
    pub housing: f64,
    #[serde(default)]
    pub terrain: Vec<String>,
    #[serde(default)]
    pub feature: Vec<String>,
    /// Features the improvement may also sit on once a civic is unlocked --
    /// Gathering Storm opens the Lumber Mill to Rainforest at Mercantilism.
    #[serde(default)]
    pub feature_after_civic: BTreeMap<String, String>,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub resource_only: bool,
    #[serde(default)]
    pub requires_hills: bool,
    #[serde(default)]
    pub hills_or_resource: bool,
    #[serde(default)]
    pub requires_flat: bool,
    #[serde(default)]
    pub unique_to: Option<String>,
    #[serde(default)]
    pub replaces: Option<String>,
    #[serde(default)]
    pub removes_feature: bool,
    #[serde(default)]
    pub water: bool,
    #[serde(default)]
    pub unbuildable: bool,
    #[serde(default = "default_true")]
    pub builder_buildable: bool,
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UnitSpec {
    pub class: String,
    pub cost: f64,
    /// Gold paid every turn; formations apply their Civ VI 150%/200% factor.
    #[serde(default)]
    pub maintenance: f64,
    pub moves: f64,
    /// False for units which only enter play through a special effect.
    #[serde(default = "default_true")]
    pub buildable: bool,
    /// Some super-units cannot combine into Corps/Armies or earn ordinary
    /// experience and promotion-tree upgrades.
    #[serde(default = "default_true")]
    pub can_formations: bool,
    #[serde(default = "default_true")]
    pub earns_xp: bool,
    /// Theocracy and the Grand Master's Chapel enable Faith purchase by unit
    /// class, and the Giant Death Robot is its own class outside that list.
    #[serde(default = "default_true")]
    pub faith_purchasable: bool,
    /// Extra Movement when the unit begins its turn on clear terrain -- flat,
    /// with no Woods, Rainforest or Hills. The Chariot line carries it.
    #[serde(default)]
    pub clear_terrain_start_movement: f64,
    #[serde(default)]
    pub strength: f64,
    #[serde(default)]
    pub ranged_strength: f64, // 0 = no ranged attack
    #[serde(default)]
    pub bombard_strength: f64, // 0 = no anti-district bombard attack
    /// Automatic defense against hostile air missions. This is distinct from
    /// an ordinary ranged attack: anti-air support units cannot attack ground
    /// targets, while several late naval units expose both capabilities.
    #[serde(default)]
    pub anti_air_strength: f64,
    #[serde(default)]
    pub anti_air_range: i32,
    /// Explicit overrides for hybrid and interception-only units. Most units
    /// infer these capabilities from their strength profile; the Giant Death
    /// Robot can use both ordinary attacks.
    #[serde(default)]
    pub can_melee: Option<bool>,
    #[serde(default)]
    pub can_ranged: Option<bool>,
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
    /// Strategic material paid once when construction or purchase starts.
    #[serde(default)]
    pub resource_cost: f64,
    /// Strategic fuel consumed at the beginning of every owner turn.
    #[serde(default)]
    pub resource_maintenance: f64,
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
    #[serde(default)]
    pub requires_district: Option<String>,
    /// Improvements this specialist can construct (builders use the whole
    /// ordinary improvement catalog; engineers/archaeologists are explicit).
    #[serde(default)]
    pub builds: Vec<String>,
    /// The unit this one becomes when upgraded for Gold, from the shipped
    /// `UnitUpgrades` table. A civilization's unique replacement stands in for
    /// the base unit whenever it owns one.
    #[serde(default, alias = "upgrades_to")]
    pub upgrade_to: Option<String>,
    /// The shipped `MandatoryObsoleteTech`. Once its owner researches this,
    /// the unit can no longer be trained or purchased; existing copies live on
    /// until they are upgraded.
    #[serde(default)]
    pub obsolete_tech: Option<String>,
    /// Data-driven auras and special unit rules. Support units currently use
    /// `adjacent_siege_range`, `adjacent_siege_bombard`, `adjacent_heal`, and
    /// `adjacent_movement`; unknown entries remain forward-compatible.
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
}

impl UnitSpec {
    pub fn ranged_attack_strength(&self) -> f64 {
        self.ranged_strength.max(self.bombard_strength)
    }

    pub fn has_ranged_attack(&self) -> bool {
        self.can_ranged
            .unwrap_or_else(|| self.ranged_attack_strength() > 0.0)
    }

    pub fn is_melee_capable(&self) -> bool {
        self.can_melee.unwrap_or_else(|| {
            self.class == "military"
                && self.domain.as_deref() != Some("air")
                && !self.has_ranged_attack()
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DistrictSpec {
    pub cost: f64,
    #[serde(default)]
    pub maintenance: f64,
    #[serde(default)]
    pub tech: Option<String>,
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub yields: Yields,
    /// Yield of one citizen assigned as a specialist in this district.
    #[serde(default)]
    pub citizen_yields: Yields,
    #[serde(default)]
    pub adjacency: BTreeMap<String, Yields>,
    /// Great Person points produced by the completed district itself.
    /// Buildings contribute their own points separately.
    #[serde(default)]
    pub great_person_points: BTreeMap<String, f64>,
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
    /// `null` means that a city may construct multiple copies (for example
    /// Neighborhoods, Canals, eligible Dams, and Spaceports); omitted entries
    /// default to the normal one-per-city rule.
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
    pub regional_group: String,
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

/// A world wonder occupies a map tile, unlike an ordinary district building.
/// Placement fields deliberately mirror the predicates used by stock Civ VI
/// so requirements remain data-driven and testable.
#[derive(Clone, Serialize, Deserialize)]
pub struct WonderSpec {
    pub cost: f64,
    #[serde(default)]
    pub tech: Option<String>,
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub yields: Yields,
    #[serde(default)]
    pub housing: f64,
    #[serde(default)]
    pub amenity: f64,
    /// Radius in which the wonder's listed yields and Amenities affect city
    /// centers. Zero means that the values belong only to the constructing
    /// city; Colosseum and other regional wonders set an explicit range.
    #[serde(default)]
    pub regional_range: i32,
    /// Loyalty per turn granted to every city inside `regional_range`.
    #[serde(default)]
    pub regional_loyalty: f64,
    #[serde(default)]
    pub great_work_slots: BTreeMap<String, i32>,
    #[serde(default)]
    pub great_person_points: BTreeMap<String, f64>,
    #[serde(default)]
    pub requires_buildings: Vec<String>,
    #[serde(default)]
    pub requires_any_buildings: Vec<String>,
    #[serde(default)]
    pub adjacent_district: Option<String>,
    #[serde(default)]
    pub adjacent_resource: Option<String>,
    #[serde(default)]
    pub adjacent_improvement: Option<String>,
    #[serde(default)]
    pub terrain: Vec<String>,
    #[serde(default)]
    pub feature: Vec<String>,
    #[serde(default)]
    pub hills: Option<bool>,
    #[serde(default)]
    pub water: bool,
    #[serde(default)]
    pub coast: bool,
    #[serde(default)]
    pub river: bool,
    #[serde(default)]
    pub adjacent_mountain: bool,
    #[serde(default)]
    pub founded_religion: bool,
    #[serde(default)]
    pub placement: String,
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
}

/// A named entry in the global Great Person market. Effects deliberately use
/// the same primitive keys as the rest of the ruleset so mods can add people
/// without engine-side ID checks.
#[derive(Clone, Serialize, Deserialize)]
pub struct GreatPersonSpec {
    pub name: String,
    pub kind: String,
    pub era: usize,
    pub cost: f64,
    #[serde(default = "done_usize")]
    pub charges: usize,
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct GovernorPromotionSpec {
    pub tier: i32,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct GovernorSpec {
    pub name: String,
    pub title: String,
    pub establish_turns: u32,
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
    #[serde(default)]
    pub promotions: BTreeMap<String, GovernorPromotionSpec>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectSpec {
    pub cost: f64,
    /// COST_PROGRESSION_GAME_PROGRESS maximum cost as a percentage of the
    /// base cost (1500 means the project grows linearly from 1x to 15x).
    #[serde(default)]
    pub cost_progression_max_pct: f64,
    #[serde(default)]
    pub tech: Option<String>,
    #[serde(default)]
    pub civic: Option<String>,
    #[serde(default)]
    pub district: Option<String>,
    #[serde(default)]
    pub alternate_districts: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub requires_buildings: Vec<String>,
    #[serde(default)]
    pub repeatable: bool,
    /// Per-turn yield conversion percentages while this project is active.
    #[serde(default)]
    pub ongoing_yields: BTreeMap<String, f64>,
    /// Base completion points. Stock district projects scale these from 1x
    /// to 8x with the same whole-percent game-progress model as their cost.
    #[serde(default)]
    pub completion_gpp: BTreeMap<String, f64>,
    #[serde(default)]
    pub full_power_while_active: bool,
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BoostSpec {
    pub trigger: String,
    #[serde(default = "done_i")]
    pub count: i64,
    /// Research granted on triggering, in percent. The database ships 40 for
    /// every boost except Near Future Governance's 90.
    #[serde(default)]
    pub percent: Option<f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TreeUnlock {
    pub kind: String,
    pub id: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TechSpec {
    pub cost: f64,
    /// Zero-based historical era: Ancient through Future.
    pub era: usize,
    pub requires: Vec<String>,
    #[serde(default)]
    pub boost: Option<BoostSpec>,
    /// Indexed from the reverse gates in the rules catalog at load time.
    #[serde(default)]
    pub unlocks: Vec<TreeUnlock>,
    /// Global abilities unlocked by the node. Every key has an engine handler.
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
    #[serde(default)]
    pub repeatable: bool,
    /// Governor titles the node awards on completion. Fourteen civics carry
    /// one each; technologies carry none.
    #[serde(default)]
    pub governor_title: usize,
}

#[derive(Deserialize)]
struct TreeEffectsData {
    techs: BTreeMap<String, BTreeMap<String, f64>>,
    civics: BTreeMap<String, BTreeMap<String, f64>>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GovEffects {
    pub production_pct: f64,
    pub science_pct: f64,
    pub gold_pct: f64,
    pub governor_gold_pct: f64,
    pub governor_faith_per_pop: f64,
    pub governor_production_per_pop: f64,
    pub gold_purchase_discount_pct: f64,
    pub district_production_pct: f64,
    pub wonder_production_pct: f64,
    pub unit_production_pct: f64,
    pub war_weariness_reduction_pct: f64,
    pub commercial_encampment_production_pct: f64,
    pub improved_strategic_resource_rate: f64,
    pub power_per_city: f64,
    pub tourism_pct: f64,
    pub combat_strength: f64,
    pub amenity: f64,
    pub housing: f64,
    pub district_city_amenity: f64,
    pub district_city_housing: f64,
    pub wall_level_housing: f64,
    pub influence_pct: f64,
    pub great_people_pct: f64,
    pub production_per_pop: f64,
    pub faith_per_pop: f64,
    pub culture_per_district: f64,
    pub trade_food: f64,
    pub trade_production: f64,
    pub allied_suzerain_trade_food: f64,
    pub allied_suzerain_trade_production: f64,
    pub project_production_pct: f64,
    pub religious_strength: f64,
    pub trade_route_capacity: f64,
    pub capital_yields: Yields,
    pub government_building_yields: Yields,
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
    pub influence_per_turn: f64,
    #[serde(default)]
    pub influence_threshold: f64,
    #[serde(default)]
    pub envoys_per_threshold: i64,
    #[serde(default)]
    pub diplomatic_favor_per_turn: f64,
    #[serde(default)]
    pub effects: GovEffects,
    #[serde(default)]
    pub slots: PolicySlots,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CivSpec {
    pub leader: String,
    /// Key into `Rules::agendas`.
    #[serde(default)]
    pub agenda: Option<String>,
    /// The leader's preference traits, as the shipped data names them —
    /// `expansionist`, `science_major_civ`, `aggressive_military` and so on.
    #[serde(default)]
    pub traits: Vec<String>,
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
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BeliefsData {
    pub pantheon: BTreeMap<String, BeliefSpec>,
    pub founder: BTreeMap<String, BeliefSpec>,
    pub follower: BTreeMap<String, BeliefSpec>,
    #[serde(default)]
    pub enhancer: BTreeMap<String, BeliefSpec>,
    #[serde(default)]
    pub worship: BTreeMap<String, BeliefSpec>,
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
    /// Numeric, data-driven policy primitives consumed by the game engine.
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
    /// Unit-Production cards apply only to units of these eras. Agoge boosts
    /// Ancient and Classical infantry and nothing later; an empty list means
    /// the card is not era-gated.
    #[serde(default)]
    pub unit_eras: Vec<usize>,
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

/// A leader's historical agenda: the standing opinion they hold about how
/// other civilizations ought to behave.
///
/// Unciv gives each leader a personality vector and weights its AI by it,
/// which is what stops every AI civ from playing the same game. Civ VI ships
/// the content for the same idea — `Leaders.xml` assigns each leader an
/// agenda and a set of preference traits — so we take Unciv's shape and the
/// game's own assignments. See `docs/UNCIV_LESSONS.md`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AgendaSpec {
    pub name: String,
    pub description: String,
    /// What the leader measures other civilizations by. Each value has an
    /// engine handler in `Game::agenda_measure`.
    pub measure: String,
    /// `more` to approve of a high measure, `less` to approve of a low one.
    pub approves_of: String,
}

/// A difficulty level, in the Civ VI sense: a bag of handicaps applied to the
/// AI seats above Prince and to the human seats below it. Prince is the
/// reference level and carries no modifiers at all.
///
/// The numbers come from the scaling modifiers the game itself ships in
/// `Leaders.xml` (`HIGH_DIFFICULTY_SCIENCE_SCALING` and its siblings, each
/// declared `LinearScaleFromDefaultHandicap` off Prince), so a level here is
/// the shipped per-step delta multiplied by that level's distance from Prince.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct DifficultySpec {
    pub name: String,
    /// Position on the ladder, Settler 0 through Deity 7. Also the sort order.
    pub order: usize,
    /// Percentage added to each AI city yield of the named kind.
    pub ai_yield_pct: Yields,
    /// Flat Combat Strength added to every AI unit.
    pub ai_combat_strength: f64,
    /// Percentage added to AI experience awards.
    pub ai_xp_pct: f64,
    /// Random Eurekas and Inspirations granted to each AI on a new world era.
    pub ai_era_boosts: usize,
    /// Extra units each AI receives on its start tile.
    pub ai_bonus_units: BTreeMap<String, usize>,
    /// Flat Combat Strength added to every human unit.
    pub human_combat_strength: f64,
    /// Percentage added to human experience awards.
    pub human_xp_pct: f64,
    /// Extra Gold a human receives for clearing a Barbarian camp.
    pub human_camp_gold: f64,
    /// Scales the size of barbarian raiding parties.
    #[serde(default = "done")]
    pub barb_force_scale: f64,
}

/// A game speed: everything a civilization buys with a stockpiled yield scales
/// by `cost_pct`, and the game runs for `turns` turns. Both are the values the
/// shipped `GameSpeeds.xml` uses (`CostMultiplier`, and the sum of that speed's
/// turn-length table).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SpeedSpec {
    pub name: String,
    pub order: usize,
    #[serde(default = "dhundred")]
    pub cost_pct: f64,
    #[serde(default = "dstandard_turns")]
    pub turns: u32,
}

fn dhundred() -> f64 {
    100.0
}

fn dstandard_turns() -> u32 {
    500
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
    pub wonders: BTreeMap<String, WonderSpec>,
    pub great_people: BTreeMap<String, GreatPersonSpec>,
    pub governors: BTreeMap<String, GovernorSpec>,
    pub projects: BTreeMap<String, ProjectSpec>,
    pub techs: BTreeMap<String, TechSpec>,
    pub civics: BTreeMap<String, TechSpec>,
    pub governments: BTreeMap<String, GovSpec>,
    pub policies: BTreeMap<String, PolicySpec>,
    pub promotions: BTreeMap<String, PromotionSpec>,
    pub beliefs: BeliefsData,
    pub civs: BTreeMap<String, CivSpec>,
    pub agendas: BTreeMap<String, AgendaSpec>,
    pub difficulties: BTreeMap<String, DifficultySpec>,
    pub speeds: BTreeMap<String, SpeedSpec>,
    /// Tribal village reward tables, the shipped seven categories.
    pub goody_huts: BTreeMap<String, BTreeMap<String, GoodyRewardSpec>>,
    /// Per-era constants from the shipped Eras table, keyed by ERA_NAMES.
    pub eras: BTreeMap<String, EraSpec>,
    /// The shipped WMDs table. Blast radius, fallout and ICBM range await a
    /// delivery mechanic; the per-turn Gold maintenance is charged today.
    pub wmds: BTreeMap<String, WmdSpec>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WmdSpec {
    pub blast_radius: i32,
    pub fallout_duration: u32,
    pub icbm_strike_range: i32,
    pub maintenance: f64,
}

/// The shipped per-era ladder: Great Person recruitment base cost, embarked
/// unit combat strength, and the warmonger weight of a declaration.
#[derive(Clone, Serialize, Deserialize)]
pub struct EraSpec {
    pub great_person_base_cost: f64,
    pub embarked_strength: f64,
    #[serde(default)]
    pub warmonger_points: f64,
}

/// One tribal village reward: its selection weight within the rolled
/// category, the earliest turn it appears, whether it needs a founded city,
/// and what it grants.
#[derive(Clone, Serialize, Deserialize)]
pub struct GoodyRewardSpec {
    pub weight: i64,
    #[serde(default)]
    pub min_turn: u32,
    #[serde(default)]
    pub requires_city: bool,
    #[serde(default)]
    pub reward: BTreeMap<String, f64>,
}

/// Every ruleset file the engine ships, by the name a mod overlay uses.
pub const DATA_FILES: [(&str, &str); 25] = [
    ("terrains", include_str!("../data/terrains.json")),
    ("features", include_str!("../data/features.json")),
    ("resources", include_str!("../data/resources.json")),
    ("improvements", include_str!("../data/improvements.json")),
    ("units", include_str!("../data/units.json")),
    ("districts", include_str!("../data/districts.json")),
    ("buildings", include_str!("../data/buildings.json")),
    ("wonders", include_str!("../data/wonders.json")),
    ("great_people", include_str!("../data/great_people.json")),
    ("governors", include_str!("../data/governors.json")),
    ("projects", include_str!("../data/projects.json")),
    ("techs", include_str!("../data/techs.json")),
    ("civics", include_str!("../data/civics.json")),
    ("governments", include_str!("../data/governments.json")),
    ("policies", include_str!("../data/policies.json")),
    ("promotions", include_str!("../data/promotions.json")),
    ("beliefs", include_str!("../data/beliefs.json")),
    ("civs", include_str!("../data/civs.json")),
    ("agendas", include_str!("../data/agendas.json")),
    ("difficulties", include_str!("../data/difficulties.json")),
    ("speeds", include_str!("../data/speeds.json")),
    ("goody_huts", include_str!("../data/goody_huts.json")),
    ("eras", include_str!("../data/eras.json")),
    ("wmds", include_str!("../data/wmds.json")),
    ("tree_effects", include_str!("../data/tree_effects.json")),
];

/// The ruleset every `Rules::embedded()` call sees. It is the shipped data
/// until a mod overlay is installed, which can only happen once, before a
/// game exists. Keeping it here rather than threading a ruleset through every
/// call site is what lets a save deserialize without knowing about mods.
static ACTIVE: OnceLock<Rules> = OnceLock::new();

impl Rules {
    /// The active ruleset — shipped data unless mods were installed.
    pub fn embedded() -> Rules {
        ACTIVE.get_or_init(Rules::shipped).clone()
    }

    /// The shipped ruleset, ignoring any installed mods.
    pub fn shipped() -> Rules {
        Rules::from_values(Rules::shipped_values()).expect("the shipped ruleset is well formed")
    }

    /// The shipped data as raw JSON, which is what a mod overlay merges into.
    pub fn shipped_values() -> BTreeMap<String, serde_json::Value> {
        DATA_FILES
            .iter()
            .map(|(name, text)| {
                let value = serde_json::from_str(text)
                    .unwrap_or_else(|error| panic!("shipped {name}.json is malformed: {error}"));
                (name.to_string(), value)
            })
            .collect()
    }

    /// Install a ruleset as the active one. Fails if a game has already read
    /// the ruleset, because half a game on one set of rules and half on
    /// another is not a state worth supporting.
    pub fn install(rules: Rules) -> Result<(), String> {
        ACTIVE
            .set(rules)
            .map_err(|_| "the ruleset is already in use and cannot be replaced".to_string())
    }

    /// Build a ruleset from raw JSON, one value per entry in [`DATA_FILES`].
    pub fn from_values(mut files: BTreeMap<String, serde_json::Value>) -> Result<Rules, String> {
        fn take<T: serde::de::DeserializeOwned>(
            files: &mut BTreeMap<String, serde_json::Value>,
            name: &str,
        ) -> Result<T, String> {
            let value = files
                .remove(name)
                .ok_or_else(|| format!("ruleset is missing {name}.json"))?;
            serde_json::from_value(value).map_err(|error| format!("{name}.json: {error}"))
        }
        let mut rules = Rules {
            terrains: take(&mut files, "terrains")?,
            features: take(&mut files, "features")?,
            resources: take(&mut files, "resources")?,
            improvements: take(&mut files, "improvements")?,
            units: take(&mut files, "units")?,
            districts: take(&mut files, "districts")?,
            buildings: take(&mut files, "buildings")?,
            wonders: take(&mut files, "wonders")?,
            great_people: take(&mut files, "great_people")?,
            governors: take(&mut files, "governors")?,
            projects: take(&mut files, "projects")?,
            techs: take(&mut files, "techs")?,
            civics: take(&mut files, "civics")?,
            governments: take(&mut files, "governments")?,
            policies: take(&mut files, "policies")?,
            promotions: take(&mut files, "promotions")?,
            beliefs: take(&mut files, "beliefs")?,
            civs: take(&mut files, "civs")?,
            agendas: take(&mut files, "agendas")?,
            difficulties: take(&mut files, "difficulties")?,
            speeds: take(&mut files, "speeds")?,
            goody_huts: take(&mut files, "goody_huts")?,
            eras: take(&mut files, "eras")?,
            wmds: take(&mut files, "wmds")?,
        };
        let effects: TreeEffectsData = take(&mut files, "tree_effects")?;
        for (node, values) in effects.techs {
            rules
                .techs
                .get_mut(&node)
                .ok_or_else(|| format!("tree_effects.json references missing technology {node}"))?
                .effects = values;
        }
        for (node, values) in effects.civics {
            rules
                .civics
                .get_mut(&node)
                .ok_or_else(|| format!("tree_effects.json references missing civic {node}"))?
                .effects = values;
        }
        rules.index_tree_unlocks();
        Ok(rules)
    }

    /// Build the one authoritative unlock list from each content object's
    /// technology/civic gate. This prevents the UI, legality checks, and tree
    /// documentation from drifting into three separate catalogs.
    fn index_tree_unlocks(&mut self) {
        let mut indexed: Vec<(bool, String, TreeUnlock)> = Vec::new();
        let mut add = |kind: &str, id: &str, tech: &Option<String>, civic: &Option<String>| {
            if let Some(node) = tech {
                indexed.push((
                    true,
                    node.clone(),
                    TreeUnlock {
                        kind: kind.to_string(),
                        id: id.to_string(),
                    },
                ));
            }
            if let Some(node) = civic {
                indexed.push((
                    false,
                    node.clone(),
                    TreeUnlock {
                        kind: kind.to_string(),
                        id: id.to_string(),
                    },
                ));
            }
        };
        for (id, spec) in &self.units {
            add("unit", id, &spec.tech, &spec.civic);
        }
        for (id, spec) in &self.buildings {
            add("building", id, &spec.tech, &spec.civic);
        }
        for (id, spec) in &self.wonders {
            add("wonder", id, &spec.tech, &spec.civic);
        }
        for (id, spec) in &self.districts {
            add("district", id, &spec.tech, &spec.civic);
        }
        for (id, spec) in &self.improvements {
            add("improvement", id, &spec.tech, &spec.civic);
        }
        for (id, spec) in &self.resources {
            add("resource", id, &spec.tech, &spec.civic);
        }
        for (id, spec) in &self.projects {
            add("project", id, &spec.tech, &spec.civic);
        }
        for (id, spec) in &self.policies {
            add("policy", id, &None, &spec.civic);
        }
        for (id, spec) in &self.governments {
            add("government", id, &None, &spec.civic);
        }

        for spec in self.techs.values_mut().chain(self.civics.values_mut()) {
            spec.unlocks.clear();
        }
        for (technology, node, unlock) in indexed {
            let tree = if technology {
                &mut self.techs
            } else {
                &mut self.civics
            };
            // A gate naming a node that does not exist is a ruleset defect,
            // and `civvis validate` reports it as one with the file and entry
            // to fix. Indexing simply skips it: panicking here would turn
            // every bad mod into a crash instead of a message.
            if let Some(spec) = tree.get_mut(&node) {
                spec.unlocks.push(unlock);
            }
        }
        for spec in self.techs.values_mut().chain(self.civics.values_mut()) {
            spec.unlocks
                .sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
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
        // Civ 6 movement is additive: terrain cost, +1 for Hills (the
        // database ships Hills as separate terrain rows costing 2), plus the
        // feature's MovementChange.
        let mut c = self.terrains[t.terrain.as_str()].move_cost;
        if t.hills {
            c += 1.0;
        }
        if let Some(f) = &t.feature {
            c += self.features[f.as_str()].move_cost;
        }
        if t.road > 0 && !self.terrains[t.terrain.as_str()].water {
            c = 1.0; // every route flattens terrain to at most 1 MP
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

    const DISTRICTS: &str = "
        city_center campus holy_site commercial_hub harbor encampment theater_square
        industrial_zone entertainment_complex water_park aqueduct neighborhood canal dam
        aerodrome spaceport government_plaza diplomatic_quarter preserve observatory seowon
        acropolis lavra ikanda thanh suguba cothon royal_navy_dockyard hansa oppidum
        street_carnival hippodrome copacabana bath mbanza";

    const BUILDINGS: &str = "
        airport alchemical_society amphitheater ancestral_hall aquarium aquatics_center
        archaeological_museum arena armory art_museum audience_chamber bank barracks
        basilikoi_paides broadcast_center cathedral chancery coal_power_plant consulate
        dar_e_mehr electronics_factory factory ferris_wheel film_studio flood_barrier
        food_market foreign_ministry gilded_vault granary grand_bazaar grand_masters_chapel
        grove gurdwara hangar hydroelectric_dam intelligence_agency library lighthouse madrasa
        marae market medieval_walls meeting_house military_academy monument mosque
        national_history_museum navigation_school nuclear_power_plant oil_power_plant
        old_god_obelisk ordu pagoda palace palgum prasat queens_bibliotheque renaissance_walls
        research_lab royal_society sanctuary seaport sewer shipyard shopping_mall shrine stable
        stadium stave_church stock_exchange stupa sukiennice synagogue temple thermal_bath
        tlachtli tsikhe university walls war_department warlords_throne wat water_mill workshop zoo";

    const WONDERS: &str = "
        alhambra amundsen_scott_research_station angkor_wat apadana big_ben biosphere
        bolshoi_theatre broadway casa_de_contratacion chichen_itza colosseum colossus
        cristo_redentor eiffel_tower estadio_do_maracana etemenanki forbidden_city
        golden_gate_bridge great_bath great_library great_lighthouse great_zimbabwe hagia_sophia
        hanging_gardens hermitage huey_teocalli jebel_barkal kilwa_kisiwani kotoku_in
        machu_picchu mahabodhi_temple mausoleum_at_halicarnassus meenakshi_temple mont_st_michel
        oracle orszaghaz oxford_university panama_canal petra potala_palace pyramids ruhr_valley
        st_basils_cathedral statue_of_liberty statue_of_zeus stonehenge sydney_opera_house
        taj_mahal temple_artemis terracotta_army torre_de_belem university_of_sankore
        venetian_arsenal";

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
    fn gathering_storm_unit_upgrade_graph_is_complete_and_acyclic() {
        let rules = Rules::embedded();
        let expected: BTreeSet<(&str, &str)> = [
            ("scout", "skirmisher"),
            ("skirmisher", "ranger"),
            ("ranger", "spec_ops"),
            ("warrior", "swordsman"),
            ("swordsman", "man_at_arms"),
            ("man_at_arms", "musketman"),
            ("musketman", "line_infantry"),
            ("line_infantry", "infantry"),
            ("infantry", "mechanized_infantry"),
            ("slinger", "archer"),
            ("archer", "crossbowman"),
            ("crossbowman", "field_cannon"),
            ("field_cannon", "machine_gun"),
            ("spearman", "pikeman"),
            ("pikeman", "pike_and_shot"),
            ("pike_and_shot", "at_crew"),
            ("at_crew", "modern_at"),
            ("horseman", "courser"),
            ("courser", "cavalry"),
            ("cavalry", "helicopter"),
            ("heavy_chariot", "knight"),
            ("knight", "cuirassier"),
            ("cuirassier", "tank"),
            ("tank", "modern_armor"),
            ("catapult", "trebuchet"),
            ("trebuchet", "bombard"),
            ("bombard", "artillery"),
            ("artillery", "rocket_artillery"),
            ("battering_ram", "siege_tower"),
            ("siege_tower", "medic"),
            ("medic", "supply_convoy"),
            ("observation_balloon", "drone"),
            ("anti_air_gun", "mobile_sam"),
            ("galley", "caravel"),
            ("caravel", "ironclad"),
            ("ironclad", "destroyer"),
            ("quadrireme", "frigate"),
            ("frigate", "battleship"),
            ("battleship", "missile_cruiser"),
            ("privateer", "submarine"),
            ("submarine", "nuclear_submarine"),
            ("biplane", "fighter"),
            ("fighter", "jet_fighter"),
            ("bomber", "jet_bomber"),
            ("legion", "man_at_arms"),
            ("hoplite", "pikeman"),
            ("eagle_warrior", "swordsman"),
            ("war_cart", "knight"),
            ("pitati_archer", "crossbowman"),
            ("maryannu_chariot_archer", "crossbowman"),
            ("saka_horse_archer", "crossbowman"),
            ("crouching_tiger", "field_cannon"),
        ]
        .into_iter()
        .collect();
        let actual: BTreeSet<(&str, &str)> = rules
            .units
            .iter()
            .filter_map(|(unit, spec)| {
                spec.upgrade_to
                    .as_deref()
                    .map(|target| (unit.as_str(), target))
            })
            .collect();
        assert_eq!(actual, expected);

        for (source, target) in &actual {
            let target_spec = rules
                .units
                .get(*target)
                .unwrap_or_else(|| panic!("{source} upgrades to missing unit {target}"));
            assert!(
                target_spec.buildable,
                "{source} upgrades to unbuildable {target}"
            );
            let mut seen = BTreeSet::new();
            let mut cursor = Some(*source);
            while let Some(unit) = cursor {
                assert!(seen.insert(unit), "unit upgrade cycle reaches {unit}");
                cursor = rules.units[unit].upgrade_to.as_deref();
            }
        }
    }

    #[test]
    fn every_tree_unlock_is_present_gated_and_runtime_indexed() {
        let rules = Rules::embedded();
        assert_eq!(rules.techs.len(), 77);
        assert_eq!(rules.civics.len(), 61);
        assert_eq!(rules.units.len(), 82);
        assert_eq!(rules.buildings.len(), 85);
        assert_eq!(rules.districts.len(), 35);
        assert_eq!(rules.wonders.len(), 53);
        assert_eq!(rules.improvements.len(), 36);
        assert_eq!(rules.resources.len(), 52);
        assert_eq!(rules.projects.len(), 25);
        assert_eq!(rules.policies.len(), 118);
        assert_eq!(rules.governments.len(), 13);

        let check_gate = |kind: &str, id: &str, tech: &Option<String>, civic: &Option<String>| {
            if let Some(node) = tech {
                let spec = rules
                    .techs
                    .get(node)
                    .unwrap_or_else(|| panic!("{kind} {id} references missing technology {node}"));
                assert!(
                    spec.unlocks
                        .iter()
                        .any(|unlock| unlock.kind == kind && unlock.id == id),
                    "technology {node} does not index {kind} {id}"
                );
            }
            if let Some(node) = civic {
                let spec = rules
                    .civics
                    .get(node)
                    .unwrap_or_else(|| panic!("{kind} {id} references missing civic {node}"));
                assert!(
                    spec.unlocks
                        .iter()
                        .any(|unlock| unlock.kind == kind && unlock.id == id),
                    "civic {node} does not index {kind} {id}"
                );
            }
        };

        for (id, spec) in &rules.units {
            check_gate("unit", id, &spec.tech, &spec.civic);
            assert!(
                spec.maintenance >= 0.0,
                "{id} has negative Gold maintenance"
            );
            if let Some(resource) = &spec.requires_resource {
                assert!(
                    rules.resources.contains_key(resource),
                    "{id} needs {resource}"
                );
                assert!(
                    spec.resource_cost > 0.0,
                    "{id} must define its Gathering Storm {resource} cost"
                );
                assert!(
                    spec.resource_maintenance >= 0.0,
                    "{id} has negative {resource} maintenance"
                );
            } else {
                assert_eq!(
                    spec.resource_cost, 0.0,
                    "{id} has a cost without a resource"
                );
                assert_eq!(
                    spec.resource_maintenance, 0.0,
                    "{id} has maintenance without a resource"
                );
            }
            if let Some(building) = &spec.requires_building {
                assert!(
                    rules.buildings.contains_key(building),
                    "{id} needs {building}"
                );
            }
            if let Some(district) = &spec.requires_district {
                assert!(
                    rules.districts.contains_key(district),
                    "{id} needs {district}"
                );
            }
            for improvement in &spec.builds {
                assert!(
                    rules.improvements.contains_key(improvement),
                    "{id} builds missing improvement {improvement}"
                );
            }
        }
        for (id, spec) in &rules.buildings {
            check_gate("building", id, &spec.tech, &spec.civic);
            assert!(
                spec.maintenance >= 0.0,
                "{id} has negative Gold maintenance"
            );
        }
        for (id, spec) in &rules.districts {
            check_gate("district", id, &spec.tech, &spec.civic);
            assert!(
                spec.maintenance >= 0.0,
                "{id} has negative Gold maintenance"
            );
        }
        for (id, spec) in &rules.wonders {
            check_gate("wonder", id, &spec.tech, &spec.civic);
        }
        for (id, spec) in &rules.improvements {
            check_gate("improvement", id, &spec.tech, &spec.civic);
            for resource in &spec.resources {
                assert!(
                    rules.resources.contains_key(resource),
                    "{id} references missing resource {resource}"
                );
            }
        }
        for (id, spec) in &rules.resources {
            check_gate("resource", id, &spec.tech, &spec.civic);
            if !spec.improvement.is_empty() {
                assert!(
                    rules.improvements.contains_key(&spec.improvement),
                    "{id} references missing improvement {}",
                    spec.improvement
                );
            }
        }
        for (id, spec) in &rules.projects {
            check_gate("project", id, &spec.tech, &spec.civic);
            if let Some(district) = &spec.district {
                assert!(
                    rules.districts.contains_key(district),
                    "{id} needs {district}"
                );
            }
            for prerequisite in &spec.requires {
                assert!(
                    rules.projects.contains_key(prerequisite),
                    "{id} requires missing project {prerequisite}"
                );
            }
            for building in &spec.requires_buildings {
                assert!(
                    rules.buildings.contains_key(building),
                    "{id} requires missing building {building}"
                );
            }
        }
        for (id, spec) in &rules.policies {
            check_gate("policy", id, &None, &spec.civic);
            assert!(
                matches!(
                    spec.slot.as_str(),
                    "military" | "economic" | "diplomatic" | "wildcard"
                ),
                "{id} has invalid slot {}",
                spec.slot
            );
            assert!(
                !spec.effects.is_empty(),
                "policy {id} has no runtime effect"
            );
            if let Some(replaced) = &spec.replaces {
                assert!(
                    rules.policies.contains_key(replaced),
                    "{id} replaces missing policy {replaced}"
                );
            }
        }
        for (id, spec) in &rules.governments {
            check_gate("government", id, &None, &spec.civic);
            let slots = spec.slots.military
                + spec.slots.economic
                + spec.slots.diplomatic
                + spec.slots.wildcard;
            assert!(slots > 0, "government {id} has no policy slots");
        }

        for (kind, tree) in [("technology", &rules.techs), ("civic", &rules.civics)] {
            for (node, spec) in tree {
                assert!(
                    !spec.unlocks.is_empty() || !spec.effects.is_empty(),
                    "{kind} {node} has neither a content unlock nor a runtime ability"
                );
                for unlock in &spec.unlocks {
                    let gate = match unlock.kind.as_str() {
                        "unit" => rules.units[&unlock.id]
                            .tech
                            .as_ref()
                            .or(rules.units[&unlock.id].civic.as_ref()),
                        "building" => rules.buildings[&unlock.id]
                            .tech
                            .as_ref()
                            .or(rules.buildings[&unlock.id].civic.as_ref()),
                        "district" => rules.districts[&unlock.id]
                            .tech
                            .as_ref()
                            .or(rules.districts[&unlock.id].civic.as_ref()),
                        "wonder" => rules.wonders[&unlock.id]
                            .tech
                            .as_ref()
                            .or(rules.wonders[&unlock.id].civic.as_ref()),
                        "improvement" => rules.improvements[&unlock.id]
                            .tech
                            .as_ref()
                            .or(rules.improvements[&unlock.id].civic.as_ref()),
                        "resource" => rules.resources[&unlock.id]
                            .tech
                            .as_ref()
                            .or(rules.resources[&unlock.id].civic.as_ref()),
                        "project" => rules.projects[&unlock.id]
                            .tech
                            .as_ref()
                            .or(rules.projects[&unlock.id].civic.as_ref()),
                        "policy" => rules.policies[&unlock.id].civic.as_ref(),
                        "government" => rules.governments[&unlock.id].civic.as_ref(),
                        other => panic!("{node} indexes unknown unlock kind {other}"),
                    };
                    assert_eq!(gate.map(String::as_str), Some(node.as_str()));
                }
            }
        }
    }

    #[test]
    fn gathering_storm_district_building_and_wonder_rosters_are_complete_and_linked() {
        let rules = Rules::embedded();
        fn expected(names: &str) -> BTreeSet<&str> {
            names.split_whitespace().collect()
        }
        assert_eq!(
            rules
                .districts
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            expected(DISTRICTS)
        );
        assert_eq!(
            rules
                .buildings
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            expected(BUILDINGS)
        );
        assert_eq!(
            rules
                .wonders
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            expected(WONDERS)
        );

        for (name, district) in &rules.districts {
            assert!(district.cost >= 0.0, "{name} has a negative cost");
            if let Some(tech) = &district.tech {
                assert!(
                    rules.techs.contains_key(tech),
                    "{name} has missing tech {tech}"
                );
            }
            if let Some(civic) = &district.civic {
                assert!(
                    rules.civics.contains_key(civic),
                    "{name} has missing civic {civic}"
                );
            }
            if let Some(base) = &district.replaces {
                assert!(
                    rules.districts.contains_key(base),
                    "{name} replaces missing {base}"
                );
                assert!(
                    district.unique_to.is_some(),
                    "{name} replacement is not unique"
                );
            }
            for excluded in &district.excludes {
                assert!(
                    rules.districts.contains_key(excluded),
                    "{name} excludes missing {excluded}"
                );
            }
        }

        for (name, building) in &rules.buildings {
            assert!(building.cost > 0.0, "{name} has no cost");
            if let Some(tech) = &building.tech {
                assert!(
                    rules.techs.contains_key(tech),
                    "{name} has missing tech {tech}"
                );
            }
            if let Some(civic) = &building.civic {
                assert!(
                    rules.civics.contains_key(civic),
                    "{name} has missing civic {civic}"
                );
            }
            if let Some(district) = &building.district {
                assert!(
                    rules.districts.contains_key(district),
                    "{name} has missing district {district}"
                );
            }
            for required in building.requires.iter().chain(&building.requires_any) {
                assert!(
                    rules.buildings.contains_key(required),
                    "{name} requires missing {required}"
                );
            }
            for excluded in &building.excludes {
                assert!(
                    rules.buildings.contains_key(excluded),
                    "{name} excludes missing {excluded}"
                );
            }
            if let Some(base) = &building.replaces {
                assert!(
                    rules.buildings.contains_key(base),
                    "{name} replaces missing {base}"
                );
            }
            assert!(
                !building.wonder,
                "{name} must be modeled as a map-placed wonder"
            );
        }

        for (name, wonder) in &rules.wonders {
            assert!(wonder.cost > 0.0, "{name} has no cost");
            if let Some(tech) = &wonder.tech {
                assert!(
                    rules.techs.contains_key(tech),
                    "{name} has missing tech {tech}"
                );
            }
            if let Some(civic) = &wonder.civic {
                assert!(
                    rules.civics.contains_key(civic),
                    "{name} has missing civic {civic}"
                );
            }
            if let Some(district) = &wonder.adjacent_district {
                assert!(
                    rules.districts.contains_key(district),
                    "{name} has missing adjacent district {district}"
                );
            }
            for required in wonder
                .requires_buildings
                .iter()
                .chain(&wonder.requires_any_buildings)
            {
                assert!(
                    rules.buildings.contains_key(required),
                    "{name} requires missing {required}"
                );
            }
            for terrain in &wonder.terrain {
                assert!(
                    rules.terrains.contains_key(terrain),
                    "{name} has missing terrain {terrain}"
                );
            }
            for feature in &wonder.feature {
                assert!(
                    rules.features.contains_key(feature),
                    "{name} has missing feature {feature}"
                );
            }
            if let Some(resource) = &wonder.adjacent_resource {
                assert!(
                    rules.resources.contains_key(resource),
                    "{name} has missing resource {resource}"
                );
            }
            if let Some(improvement) = &wonder.adjacent_improvement {
                assert!(
                    rules.improvements.contains_key(improvement),
                    "{name} has missing improvement {improvement}"
                );
            }
        }
    }

    #[test]
    fn modeled_unit_classes_have_complete_promotion_trees() {
        let rules = Rules::embedded();
        let classes: BTreeSet<_> = rules
            .units
            .values()
            // Spy promotions are resolved by the off-map espionage engine,
            // not the seven-node map-unit XP trees validated here.
            .filter(|unit| !unit.promotion_class.is_empty() && unit.promotion_class != "espionage")
            .map(|unit| unit.promotion_class.as_str())
            .collect();
        let promotion_count = |class: &str| match class {
            "religious_apostle" => 9,
            "rock_band" => 12,
            _ => 7,
        };
        let expected_promotions = classes
            .iter()
            .map(|class| promotion_count(class))
            .sum::<usize>();
        assert_eq!(
            rules.promotions.len(),
            expected_promotions,
            "modeled promotion classes: {classes:?}"
        );
        for class in classes {
            let nodes: Vec<_> = rules
                .promotions
                .iter()
                .filter(|(_, promotion)| promotion.class == class)
                .collect();
            let expected = promotion_count(class);
            assert_eq!(nodes.len(), expected, "{class} promotion tree");
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
