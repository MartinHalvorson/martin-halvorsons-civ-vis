//! Ruleset validation.
//!
//! The ruleset is data, and data can be wrong in ways `serde` will happily
//! accept: a unit gated behind a technology nobody defined, a building placed
//! in a district that was renamed, a unique unit belonging to no civilization.
//! Those used to surface as a panic mid-game, or not at all.
//!
//! Unciv checks its ruleset the way a compiler checks a program — every
//! cross-reference resolved, findings split by severity, and an escape hatch
//! for the ones an author knows about. This is that, for our data: run it with
//! `civvis validate`, and see `docs/UNCIV_LESSONS.md` for the lineage.

use std::collections::{BTreeMap, BTreeSet};

use crate::rules::{Rules, ERA_NAMES};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The ruleset refers to something that does not exist. The engine cannot
    /// honour the rule, so this fails the check.
    Error,
    /// Legal, but almost certainly not what the author meant.
    Warning,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub severity: Severity,
    /// `catalogue/entry`, e.g. `units/legion` — also the waiver key.
    pub subject: String,
    pub message: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:<7} {}: {}",
            self.severity.label(),
            self.subject,
            self.message
        )
    }
}

/// Findings an author has looked at and accepted, keyed by subject, each with
/// the reason it is allowed to stand. Shrinking this file is always progress.
fn waivers() -> BTreeMap<String, String> {
    serde_json::from_str(include_str!("../data/validation_waivers.json"))
        .expect("validation_waivers.json is not valid JSON")
}

struct Check<'a> {
    rules: &'a Rules,
    findings: Vec<Finding>,
    great_person_kinds: BTreeSet<String>,
    promotion_classes: BTreeSet<String>,
}

impl<'a> Check<'a> {
    fn error(&mut self, subject: impl Into<String>, message: impl Into<String>) {
        self.findings.push(Finding {
            severity: Severity::Error,
            subject: subject.into(),
            message: message.into(),
        });
    }

    fn warn(&mut self, subject: impl Into<String>, message: impl Into<String>) {
        self.findings.push(Finding {
            severity: Severity::Warning,
            subject: subject.into(),
            message: message.into(),
        });
    }

    /// The workhorse: `field` on `subject` names `value`, which has to exist in
    /// `catalogue`.
    fn reference<T>(
        &mut self,
        subject: &str,
        field: &str,
        value: Option<&String>,
        catalogue: &BTreeMap<String, T>,
        catalogue_name: &str,
    ) {
        let Some(value) = value else { return };
        if !catalogue.contains_key(value) {
            self.error(
                subject,
                format!("{field} names {value:?}, which is not a known {catalogue_name}"),
            );
        }
    }

    fn references<T>(
        &mut self,
        subject: &str,
        field: &str,
        values: &[String],
        catalogue: &BTreeMap<String, T>,
        catalogue_name: &str,
    ) {
        for value in values {
            self.reference(subject, field, Some(value), catalogue, catalogue_name);
        }
    }

    /// Every catalogue entry may be gated behind a technology and a civic.
    fn gates(&mut self, subject: &str, tech: Option<&String>, civic: Option<&String>) {
        let techs = &self.rules.techs;
        let civics = &self.rules.civics;
        if let Some(tech) = tech {
            if !techs.contains_key(tech) {
                self.error(subject, format!("tech names {tech:?}, which is not a known technology"));
            }
        }
        if let Some(civic) = civic {
            if !civics.contains_key(civic) {
                self.error(subject, format!("civic names {civic:?}, which is not a known civic"));
            }
        }
    }

    /// Unique content for a civilization we have not shipped yet is inert
    /// rather than broken: nothing can own it, so nothing can build it. That
    /// is worth saying out loud, but it is not a defect in the ruleset.
    fn civ(&mut self, subject: &str, field: &str, value: Option<&String>) {
        let civs = &self.rules.civs;
        if let Some(value) = value {
            if !civs.contains_key(value) {
                self.warn(
                    subject,
                    format!("{field} names {value:?}, an undefined civilization — unreachable until it ships"),
                );
            }
        }
    }
}

/// Check a ruleset, hardest problems first. An empty result means the data is
/// internally consistent; see [`Severity`] for what the entries mean.
pub fn validate(rules: &Rules) -> Vec<Finding> {
    let great_person_kinds = rules
        .great_people
        .values()
        .map(|person| person.kind.clone())
        .collect();
    let promotion_classes = rules
        .promotions
        .values()
        .map(|promotion| promotion.class.clone())
        .collect();
    let mut check = Check {
        rules,
        findings: Vec::new(),
        great_person_kinds,
        promotion_classes,
    };

    trees(&mut check);
    units(&mut check);
    districts_and_buildings(&mut check);
    wonders(&mut check);
    terrain_and_improvements(&mut check);
    politics(&mut check);
    people(&mut check);
    agendas(&mut check);
    setup(&mut check);

    let waivers = waivers();
    let mut findings: Vec<Finding> = check
        .findings
        .into_iter()
        .filter(|finding| !waivers.contains_key(&finding.subject))
        .collect();
    findings.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.subject.cmp(&b.subject)));
    findings
}

/// Technology and civic trees: prerequisites resolve, nothing is its own
/// ancestor, and everything a node claims to unlock exists.
fn trees(check: &mut Check) {
    for (tree_name, tree) in [("techs", &check.rules.techs), ("civics", &check.rules.civics)] {
        let tree = tree.clone();
        for (id, spec) in &tree {
            let subject = format!("{tree_name}/{id}");
            for prerequisite in &spec.requires {
                if !tree.contains_key(prerequisite) {
                    check.error(
                        &subject,
                        format!("requires {prerequisite:?}, which is not in the {tree_name} tree"),
                    );
                }
            }
            if spec.era >= ERA_NAMES.len() {
                check.error(&subject, format!("era {} is past the Future Era", spec.era));
            }
            if spec.cost <= 0.0 {
                check.warn(&subject, format!("costs {}, so it is free", spec.cost));
            }
            if let Some(boost) = &spec.boost {
                if boost.trigger.is_empty() {
                    check.error(&subject, "boost has an empty trigger");
                }
                if boost.count <= 0 {
                    check.warn(&subject, "boost triggers on a count of zero or less");
                }
            }
            for unlock in &spec.unlocks {
                let known = match unlock.kind.as_str() {
                    "unit" => check.rules.units.contains_key(&unlock.id),
                    "building" => check.rules.buildings.contains_key(&unlock.id),
                    "district" => check.rules.districts.contains_key(&unlock.id),
                    "wonder" => check.rules.wonders.contains_key(&unlock.id),
                    "improvement" => check.rules.improvements.contains_key(&unlock.id),
                    "resource" => check.rules.resources.contains_key(&unlock.id),
                    "project" => check.rules.projects.contains_key(&unlock.id),
                    "policy" => check.rules.policies.contains_key(&unlock.id),
                    "government" => check.rules.governments.contains_key(&unlock.id),
                    other => {
                        check.error(&subject, format!("unlocks unknown kind {other:?}"));
                        continue;
                    }
                };
                if !known {
                    check.error(
                        &subject,
                        format!("unlocks {} {:?}, which does not exist", unlock.kind, unlock.id),
                    );
                }
            }
        }
        // A prerequisite cycle would hang any traversal of the tree.
        for id in tree.keys() {
            let mut seen = BTreeSet::new();
            let mut frontier = vec![id.clone()];
            while let Some(node) = frontier.pop() {
                let Some(spec) = tree.get(&node) else { continue };
                for prerequisite in &spec.requires {
                    if prerequisite == id {
                        check.error(
                            format!("{tree_name}/{id}"),
                            "is its own prerequisite, directly or through a cycle",
                        );
                        frontier.clear();
                        break;
                    }
                    if seen.insert(prerequisite.clone()) {
                        frontier.push(prerequisite.clone());
                    }
                }
            }
        }
    }
}

fn units(check: &mut Check) {
    let units = check.rules.units.clone();
    for (id, spec) in &units {
        let subject = format!("units/{id}");
        check.gates(&subject, spec.tech.as_ref(), spec.civic.as_ref());
        check.civ(&subject, "unique_to", spec.unique_to.as_ref());
        check.reference(&subject, "replaces", spec.replaces.as_ref(), &units, "unit");
        check.reference(&subject, "upgrade_to", spec.upgrade_to.as_ref(), &units, "unit");
        let techs = check.rules.techs.clone();
        check.reference(
            &subject,
            "obsolete_tech",
            spec.obsolete_tech.as_ref(),
            &techs,
            "technology",
        );
        if spec.upgrade_to.as_deref() == Some(id.as_str()) {
            check.error(&subject, "upgrades into itself");
        }
        let resources = check.rules.resources.clone();
        check.reference(
            &subject,
            "requires_resource",
            spec.requires_resource.as_ref(),
            &resources,
            "resource",
        );
        let buildings = check.rules.buildings.clone();
        check.reference(
            &subject,
            "requires_building",
            spec.requires_building.as_ref(),
            &buildings,
            "building",
        );
        let districts = check.rules.districts.clone();
        check.reference(
            &subject,
            "requires_district",
            spec.requires_district.as_ref(),
            &districts,
            "district",
        );
        let improvements = check.rules.improvements.clone();
        check.references(&subject, "builds", &spec.builds, &improvements, "improvement");
        if spec.replaces.is_some() && spec.unique_to.is_none() {
            check.error(&subject, "replaces another unit but belongs to no civilization");
        }
        if spec.buildable && spec.cost <= 0.0 {
            check.warn(&subject, "is buildable but free");
        }
        if !spec.promotion_class.is_empty()
            && !check.promotion_classes.contains(&spec.promotion_class)
            && spec.strength > 0.0
        {
            check.warn(
                &subject,
                format!(
                    "promotion class {:?} has no promotions to offer",
                    spec.promotion_class
                ),
            );
        }
    }
}

fn districts_and_buildings(check: &mut Check) {
    let districts = check.rules.districts.clone();
    for (id, spec) in &districts {
        let subject = format!("districts/{id}");
        check.gates(&subject, spec.tech.as_ref(), spec.civic.as_ref());
        check.civ(&subject, "unique_to", spec.unique_to.as_ref());
        check.reference(&subject, "replaces", spec.replaces.as_ref(), &districts, "district");
        check.references(&subject, "excludes", &spec.excludes, &districts, "district");
        for kind in spec.great_person_points.keys() {
            if !check.great_person_kinds.contains(kind) {
                check.error(
                    &subject,
                    format!("awards points to unknown Great Person class {kind:?}"),
                );
            }
        }
        if spec.buildable && spec.cost <= 0.0 {
            check.warn(&subject, "is buildable but free");
        }
    }

    let buildings = check.rules.buildings.clone();
    for (id, spec) in &buildings {
        let subject = format!("buildings/{id}");
        check.gates(&subject, spec.tech.as_ref(), spec.civic.as_ref());
        check.civ(&subject, "unique_to", spec.unique_to.as_ref());
        check.reference(&subject, "district", spec.district.as_ref(), &districts, "district");
        check.reference(&subject, "replaces", spec.replaces.as_ref(), &buildings, "building");
        check.references(&subject, "requires", &spec.requires, &buildings, "building");
        check.references(
            &subject,
            "requires_any",
            &spec.requires_any,
            &buildings,
            "building",
        );
        check.references(&subject, "excludes", &spec.excludes, &buildings, "building");
        if let Some(belief) = &spec.worship_belief {
            if !check.rules.beliefs.worship.contains_key(belief) {
                check.error(
                    &subject,
                    format!("worship_belief names {belief:?}, which is not a Worship belief"),
                );
            }
        }
        for kind in spec.great_person_points.keys() {
            if !check.great_person_kinds.contains(kind) {
                check.error(
                    &subject,
                    format!("awards points to unknown Great Person class {kind:?}"),
                );
            }
        }
    }
}

fn wonders(check: &mut Check) {
    let wonders = check.rules.wonders.clone();
    let buildings = check.rules.buildings.clone();
    let districts = check.rules.districts.clone();
    let improvements = check.rules.improvements.clone();
    let resources = check.rules.resources.clone();
    let terrains = check.rules.terrains.clone();
    let features = check.rules.features.clone();
    for (id, spec) in &wonders {
        let subject = format!("wonders/{id}");
        check.gates(&subject, spec.tech.as_ref(), spec.civic.as_ref());
        check.references(
            &subject,
            "requires_buildings",
            &spec.requires_buildings,
            &buildings,
            "building",
        );
        check.references(
            &subject,
            "requires_any_buildings",
            &spec.requires_any_buildings,
            &buildings,
            "building",
        );
        check.reference(
            &subject,
            "adjacent_district",
            spec.adjacent_district.as_ref(),
            &districts,
            "district",
        );
        check.reference(
            &subject,
            "adjacent_resource",
            spec.adjacent_resource.as_ref(),
            &resources,
            "resource",
        );
        check.reference(
            &subject,
            "adjacent_improvement",
            spec.adjacent_improvement.as_ref(),
            &improvements,
            "improvement",
        );
        check.references(&subject, "terrain", &spec.terrain, &terrains, "terrain");
        check.references(&subject, "feature", &spec.feature, &features, "feature");
        if spec.cost <= 0.0 {
            check.warn(&subject, "is free to build");
        }
    }
}

fn terrain_and_improvements(check: &mut Check) {
    let terrains = check.rules.terrains.clone();
    let features = check.rules.features.clone();
    let improvements = check.rules.improvements.clone();
    let resources = check.rules.resources.clone();

    for (id, spec) in &improvements {
        let subject = format!("improvements/{id}");
        check.gates(&subject, spec.tech.as_ref(), spec.civic.as_ref());
        check.civ(&subject, "unique_to", spec.unique_to.as_ref());
        check.reference(
            &subject,
            "replaces",
            spec.replaces.as_ref(),
            &improvements,
            "improvement",
        );
        check.references(&subject, "terrain", &spec.terrain, &terrains, "terrain");
        check.references(&subject, "feature", &spec.feature, &features, "feature");
        check.references(&subject, "resources", &spec.resources, &resources, "resource");
        if spec.resource_only && spec.resources.is_empty() {
            check.error(&subject, "is resource_only but names no resources");
        }
    }

    for (id, spec) in &resources {
        let subject = format!("resources/{id}");
        check.gates(&subject, spec.tech.as_ref(), spec.civic.as_ref());
        check.references(&subject, "terrain", &spec.terrain, &terrains, "terrain");
        check.references(&subject, "feature", &spec.feature, &features, "feature");
        if !spec.improvement.is_empty() && !improvements.contains_key(&spec.improvement) {
            check.error(
                &subject,
                format!(
                    "improvement names {:?}, which is not a known improvement",
                    spec.improvement
                ),
            );
        }
        if !matches!(
            spec.class.as_str(),
            "bonus" | "luxury" | "strategic" | "artifact"
        ) {
            check.error(
                &subject,
                format!(
                    "class {:?} is not bonus, luxury, strategic or artifact",
                    spec.class
                ),
            );
        }
        if spec.terrain.is_empty() && spec.feature.is_empty() {
            check.warn(&subject, "can appear on no terrain or feature, so it never spawns");
        }
    }
}

fn politics(check: &mut Check) {
    const SLOTS: [&str; 4] = ["military", "economic", "diplomatic", "wildcard"];
    let policies = check.rules.policies.clone();
    for (id, spec) in &policies {
        let subject = format!("policies/{id}");
        check.gates(&subject, None, spec.civic.as_ref());
        check.reference(&subject, "replaces", spec.replaces.as_ref(), &policies, "policy");
        if !SLOTS.contains(&spec.slot.as_str()) {
            check.error(&subject, format!("slot {:?} is not a policy slot type", spec.slot));
        }
        if spec.effects.is_empty() {
            check.warn(&subject, "has no effects, so slotting it does nothing");
        }
    }

    let governments = check.rules.governments.clone();
    for (id, spec) in &governments {
        let subject = format!("governments/{id}");
        check.gates(&subject, None, spec.civic.as_ref());
        let slots = serde_json::to_value(&spec.slots).unwrap_or_default();
        if let Some(map) = slots.as_object() {
            let total: i64 = map.values().filter_map(|v| v.as_i64()).sum();
            if total == 0 {
                check.warn(&subject, "offers no policy slots at all");
            }
        }
    }

    let promotions = check.rules.promotions.clone();
    let classes: BTreeSet<String> = check
        .rules
        .units
        .values()
        .map(|unit| unit.promotion_class.clone())
        .collect();
    for (id, spec) in &promotions {
        let subject = format!("promotions/{id}");
        check.references(&subject, "requires", &spec.requires, &promotions, "promotion");
        if !classes.contains(&spec.class) {
            check.error(
                &subject,
                format!("class {:?} matches no unit's promotion class", spec.class),
            );
        }
        if spec.effects.is_empty() && spec.note.is_empty() {
            check.warn(&subject, "has neither effects nor a note explaining it");
        }
    }
}

fn people(check: &mut Check) {
    for (id, spec) in &check.rules.great_people.clone() {
        let subject = format!("great_people/{id}");
        if spec.era >= ERA_NAMES.len() {
            check.error(&subject, format!("era {} is past the Future Era", spec.era));
        }
        if spec.name.is_empty() {
            check.error(&subject, "has no name");
        }
        if spec.charges == 0 && spec.effects.is_empty() {
            check.warn(&subject, "has no charges and no effects");
        }
    }

    for (id, spec) in &check.rules.governors.clone() {
        let subject = format!("governors/{id}");
        for (promotion_id, promotion) in &spec.promotions {
            for prerequisite in &promotion.requires {
                if !spec.promotions.contains_key(prerequisite) {
                    check.error(
                        format!("{subject}/{promotion_id}"),
                        format!("requires {prerequisite:?}, which this governor does not offer"),
                    );
                }
            }
        }
    }

    let units = check.rules.units.clone();
    for (id, spec) in &check.rules.civs.clone() {
        let subject = format!("civs/{id}");
        if spec.leader.is_empty() {
            check.error(&subject, "has no leader");
        }
        if spec.ability.is_empty() {
            check.warn(&subject, "has no signature ability");
        }
        match &spec.agenda {
            None => check.warn(&subject, "has no historical agenda"),
            Some(agenda) if !check.rules.agendas.contains_key(agenda) => check.error(
                &subject,
                format!("agenda names {agenda:?}, which is not a known agenda"),
            ),
            Some(_) => {}
        }
        if let Some(unit) = &spec.unique_unit {
            match units.get(unit) {
                None => check.error(
                    &subject,
                    format!("unique_unit names {unit:?}, which is not a known unit"),
                ),
                Some(unique) if unique.unique_to.as_deref() != Some(id.as_str()) => check.error(
                    &subject,
                    format!("unique_unit {unit:?} is not marked unique_to this civilization"),
                ),
                Some(_) => {}
            }
        }
    }
}

/// Agendas: every measure needs an engine handler, or the leader silently
/// holds no opinion at all.
fn agendas(check: &mut Check) {
    const MEASURES: [&str; 8] = [
        "territory",
        "military",
        "wonders",
        "districts_per_city",
        "city_state_rivalry",
        "loyalty_to_friends",
        "shared_luxuries",
        "trustworthiness",
    ];
    for (id, spec) in &check.rules.agendas.clone() {
        let subject = format!("agendas/{id}");
        if spec.name.is_empty() {
            check.error(&subject, "has no display name");
        }
        if !MEASURES.contains(&spec.measure.as_str()) {
            check.error(
                &subject,
                format!("measure {:?} has no engine handler", spec.measure),
            );
        }
        if !matches!(spec.approves_of.as_str(), "more" | "less") {
            check.error(
                &subject,
                format!("approves_of {:?} is not more or less", spec.approves_of),
            );
        }
        if spec.description.is_empty() {
            check.warn(&subject, "has no description for the player to read");
        }
    }
}

/// The setup catalogues: the difficulty ladder and the game speeds.
fn setup(check: &mut Check) {
    let difficulties = check.rules.difficulties.clone();
    let units = check.rules.units.clone();
    let mut orders: Vec<usize> = difficulties.values().map(|spec| spec.order).collect();
    orders.sort_unstable();
    if orders != (0..difficulties.len()).collect::<Vec<_>>() {
        check.error(
            "difficulties",
            format!("orders {orders:?} are not a contiguous ladder from zero"),
        );
    }
    let neutral = difficulties
        .values()
        .filter(|spec| {
            spec.ai_combat_strength == 0.0
                && spec.human_combat_strength == 0.0
                && spec.ai_era_boosts == 0
                && spec.ai_bonus_units.is_empty()
        })
        .count();
    if neutral != 1 {
        check.error(
            "difficulties",
            format!("{neutral} levels hand out no handicap at all; exactly one should"),
        );
    }
    for (id, spec) in &difficulties {
        let subject = format!("difficulties/{id}");
        if spec.name.is_empty() {
            check.error(&subject, "has no display name");
        }
        for unit in spec.ai_bonus_units.keys() {
            if !units.contains_key(unit) {
                check.error(
                    &subject,
                    format!("grants bonus unit {unit:?}, which is not a known unit"),
                );
            }
        }
        if spec.barb_force_scale <= 0.0 {
            check.error(&subject, "scales barbarian forces to nothing");
        }
    }

    let speeds = check.rules.speeds.clone();
    let mut orders: Vec<usize> = speeds.values().map(|spec| spec.order).collect();
    orders.sort_unstable();
    if orders != (0..speeds.len()).collect::<Vec<_>>() {
        check.error(
            "speeds",
            format!("orders {orders:?} are not a contiguous ladder from zero"),
        );
    }
    for (id, spec) in &speeds {
        let subject = format!("speeds/{id}");
        if spec.cost_pct <= 0.0 {
            check.error(&subject, "makes everything free");
        }
        if spec.turns == 0 {
            check.error(&subject, "runs for no turns");
        }
    }
}

/// Render a report for the command line. Returns the text and whether it is
/// clean enough to pass.
pub fn report(findings: &[Finding]) -> (String, bool) {
    let errors = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let warnings = findings.len() - errors;
    let mut out = String::new();
    for finding in findings {
        out.push_str(&format!("{finding}\n"));
    }
    if !findings.is_empty() {
        out.push('\n');
    }
    out.push_str(&format!(
        "{errors} error{}, {warnings} warning{}\n",
        if errors == 1 { "" } else { "s" },
        if warnings == 1 { "" } else { "s" }
    ));
    (out, errors == 0)
}

#[cfg(test)]
mod tests {
    use super::{validate, Severity};
    use crate::rules::Rules;

    /// The shipped ruleset is internally consistent. A failure here names the
    /// data file and entry to fix.
    #[test]
    fn the_shipped_ruleset_validates() {
        let findings = validate(&Rules::embedded());
        let errors: Vec<String> = findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
            .map(|finding| finding.to_string())
            .collect();
        assert!(errors.is_empty(), "ruleset errors:\n{}", errors.join("\n"));
    }

    /// A broken reference is caught rather than reaching the engine.
    #[test]
    fn a_dangling_reference_is_an_error() {
        let mut rules = Rules::embedded();
        rules
            .units
            .get_mut("warrior")
            .unwrap()
            .tech = Some("nonexistent_tech".to_string());
        let findings = validate(&rules);
        assert!(findings.iter().any(|finding| finding.severity == Severity::Error
            && finding.subject == "units/warrior"
            && finding.message.contains("nonexistent_tech")));
    }

    /// Difficulty and speed are validated like any other catalogue.
    #[test]
    fn a_broken_difficulty_ladder_is_an_error() {
        let mut rules = Rules::embedded();
        rules.difficulties.get_mut("deity").unwrap().order = 99;
        rules
            .difficulties
            .get_mut("king")
            .unwrap()
            .ai_bonus_units
            .insert("trebuchet_of_theseus".to_string(), 1);
        let findings = validate(&rules);
        assert!(findings
            .iter()
            .any(|finding| finding.subject == "difficulties" && finding.severity == Severity::Error));
        assert!(findings.iter().any(|finding| finding.subject == "difficulties/king"
            && finding.message.contains("trebuchet_of_theseus")));
    }
}
