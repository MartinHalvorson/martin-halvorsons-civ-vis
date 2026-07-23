//! The Civilopedia, generated from whatever ruleset is loaded.
//!
//! Unciv builds its in-game encyclopedia out of the ruleset rather than
//! writing it by hand, which means a mod gets documentation for free and the
//! documentation cannot drift from the rules. Now that `--mods` exists here
//! (see [MODS.md](../docs/MODS.md)), the same argument applies: a mod author
//! who halves the Warrior's cost should see the new number, not ours.
//!
//! So this reads `Rules` and nothing else. Every entry is a name, a one-line
//! summary, a list of facts, and links to the entries it depends on.
//! `civvis pedia` prints it, `GET /pedia` serves it, and the GUI browses it.

use serde::Serialize;

use crate::rules::{Rules, Yields, ERA_NAMES};

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    /// `category/id`, which is also how links are written.
    pub key: String,
    pub category: String,
    pub id: String,
    pub name: String,
    pub summary: String,
    /// Label/value pairs, in the order they should be read.
    pub facts: Vec<(String, String)>,
    /// Keys of entries this one depends on.
    pub links: Vec<String>,
}

/// Ruleset identifiers are snake_case; a reader wants words.
pub fn title(id: &str) -> String {
    id.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn number(value: f64) -> String {
    if (value - value.round()).abs() < 1e-9 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.1}")
    }
}

fn yields_summary(yields: Yields) -> Option<String> {
    let parts: Vec<String> = [
        ("Food", yields.food),
        ("Production", yields.production),
        ("Gold", yields.gold),
        ("Science", yields.science),
        ("Culture", yields.culture),
        ("Faith", yields.faith),
    ]
    .into_iter()
    .filter(|(_, value)| *value != 0.0)
    .map(|(name, value)| format!("{}{} {name}", if value > 0.0 { "+" } else { "" }, number(value)))
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn era_name(era: usize) -> String {
    title(ERA_NAMES.get(era).copied().unwrap_or("unknown"))
}

/// A builder that keeps each catalogue's construction to the interesting bits.
struct Builder {
    entries: Vec<Entry>,
}

impl Builder {
    fn push(
        &mut self,
        category: &str,
        id: &str,
        summary: impl Into<String>,
        facts: Vec<(String, String)>,
        links: Vec<String>,
    ) {
        self.entries.push(Entry {
            key: format!("{category}/{id}"),
            category: category.to_string(),
            id: id.to_string(),
            name: title(id),
            summary: summary.into(),
            facts,
            links,
        });
    }
}

/// The pedia category an unlock belongs to. Kinds the pedia does not write
/// pages for get no link rather than a broken one.
fn unlock_key(kind: &str, id: &str) -> Option<String> {
    let category = match kind {
        "unit" => "units",
        "building" => "buildings",
        "district" => "districts",
        "wonder" => "wonders",
        "improvement" => "improvements",
        "policy" => "policies",
        "government" => "governments",
        "resource" => "resources",
        _ => return None,
    };
    Some(format!("{category}/{id}"))
}

/// Facts and links shared by everything that can be gated behind the trees.
fn gates(tech: &Option<String>, civic: &Option<String>) -> (Vec<(String, String)>, Vec<String>) {
    let mut facts = Vec::new();
    let mut links = Vec::new();
    if let Some(tech) = tech {
        facts.push(("Unlocked by".to_string(), title(tech)));
        links.push(format!("techs/{tech}"));
    }
    if let Some(civic) = civic {
        facts.push(("Unlocked by".to_string(), title(civic)));
        links.push(format!("civics/{civic}"));
    }
    (facts, links)
}

/// Everything the loaded ruleset can tell a reader, sorted by category then
/// name so the order is stable across runs and across mods.
pub fn entries(rules: &Rules) -> Vec<Entry> {
    let mut b = Builder {
        entries: Vec::new(),
    };

    for (id, spec) in &rules.units {
        let (mut facts, mut links) = gates(&spec.tech, &spec.civic);
        facts.insert(0, ("Cost".to_string(), format!("{} Production", number(spec.cost))));
        facts.push(("Movement".to_string(), number(spec.moves)));
        if spec.strength > 0.0 {
            facts.push(("Combat Strength".to_string(), number(spec.strength)));
        }
        if spec.ranged_strength > 0.0 {
            facts.push(("Ranged Strength".to_string(), number(spec.ranged_strength)));
            facts.push(("Range".to_string(), spec.range.to_string()));
        }
        if spec.bombard_strength > 0.0 {
            facts.push(("Bombard Strength".to_string(), number(spec.bombard_strength)));
        }
        if spec.maintenance > 0.0 {
            facts.push(("Maintenance".to_string(), format!("{} Gold", number(spec.maintenance))));
        }
        facts.push(("Sight".to_string(), spec.sight.to_string()));
        if let Some(resource) = &spec.requires_resource {
            facts.push(("Requires".to_string(), title(resource)));
            links.push(format!("resources/{resource}"));
        }
        if let Some(civ) = &spec.unique_to {
            facts.push(("Unique to".to_string(), civ.clone()));
            // Content can name a civilization the ruleset has not defined yet
            // — validate reports those. Say who owns it, but do not offer a
            // link to a page that does not exist.
            if rules.civs.contains_key(civ) {
                links.push(format!("civs/{civ}"));
            }
        }
        if let Some(replaced) = &spec.replaces {
            facts.push(("Replaces".to_string(), title(replaced)));
            links.push(format!("units/{replaced}"));
        }
        let summary = match (spec.unique_to.as_ref(), spec.strength > 0.0) {
            (Some(civ), _) => format!("{} unique {} unit", civ, title(&spec.promotion_class)),
            (None, true) => format!("{} unit", title(&spec.promotion_class)),
            (None, false) => format!("{} unit", title(&spec.class)),
        };
        b.push("units", id, summary, facts, links);
    }

    for (id, spec) in &rules.buildings {
        let (mut facts, mut links) = gates(&spec.tech, &spec.civic);
        facts.insert(0, ("Cost".to_string(), format!("{} Production", number(spec.cost))));
        if let Some(district) = &spec.district {
            facts.push(("District".to_string(), title(district)));
            links.push(format!("districts/{district}"));
        }
        if let Some(yields) = yields_summary(spec.yields) {
            facts.push(("Yields".to_string(), yields));
        }
        for (label, value) in [
            ("Housing", spec.housing),
            ("Amenities", spec.amenity),
            ("Maintenance", spec.maintenance),
        ] {
            if value != 0.0 {
                facts.push((label.to_string(), number(value)));
            }
        }
        for required in spec.requires.iter().chain(spec.requires_any.iter()) {
            facts.push(("Requires".to_string(), title(required)));
            links.push(format!("buildings/{required}"));
        }
        if let Some(civ) = &spec.unique_to {
            facts.push(("Unique to".to_string(), civ.clone()));
            // Content can name a civilization the ruleset has not defined yet
            // — validate reports those. Say who owns it, but do not offer a
            // link to a page that does not exist.
            if rules.civs.contains_key(civ) {
                links.push(format!("civs/{civ}"));
            }
        }
        b.push("buildings", id, "Building", facts, links);
    }

    for (id, spec) in &rules.districts {
        let (mut facts, mut links) = gates(&spec.tech, &spec.civic);
        facts.insert(0, ("Cost".to_string(), format!("{} Production", number(spec.cost))));
        if let Some(yields) = yields_summary(spec.yields) {
            facts.push(("Yields".to_string(), yields));
        }
        if !spec.adjacency.is_empty() {
            let mut sources: Vec<String> = spec
                .adjacency
                .iter()
                .map(|(source, yields)| {
                    match yields_summary(*yields) {
                        Some(summary) => format!("{} → {summary}", title(source)),
                        None => title(source),
                    }
                })
                .collect();
            sources.sort();
            facts.push(("Adjacency".to_string(), sources.join(", ")));
        }
        for (label, value) in [
            ("Housing", spec.housing),
            ("Amenities", spec.amenity),
            ("City Defense", spec.defense),
        ] {
            if value != 0.0 {
                facts.push((label.to_string(), number(value)));
            }
        }
        if let Some(civ) = &spec.unique_to {
            facts.push(("Unique to".to_string(), civ.clone()));
            // Content can name a civilization the ruleset has not defined yet
            // — validate reports those. Say who owns it, but do not offer a
            // link to a page that does not exist.
            if rules.civs.contains_key(civ) {
                links.push(format!("civs/{civ}"));
            }
        }
        let summary = if spec.specialty {
            "Specialty district"
        } else {
            "District"
        };
        b.push("districts", id, summary, facts, links);
    }

    for (id, spec) in &rules.wonders {
        let (mut facts, links) = gates(&spec.tech, &spec.civic);
        facts.insert(0, ("Cost".to_string(), format!("{} Production", number(spec.cost))));
        if let Some(yields) = yields_summary(spec.yields) {
            facts.push(("Yields".to_string(), yields));
        }
        let mut placement = Vec::new();
        if spec.river {
            placement.push("beside a river".to_string());
        }
        if spec.coast {
            placement.push("on the coast".to_string());
        }
        if spec.adjacent_mountain {
            placement.push("beside a mountain".to_string());
        }
        if let Some(district) = &spec.adjacent_district {
            placement.push(format!("beside a {}", title(district)));
        }
        if !spec.terrain.is_empty() {
            placement.push(format!(
                "on {}",
                spec.terrain.iter().map(|t| title(t)).collect::<Vec<_>>().join(" or ")
            ));
        }
        if !placement.is_empty() {
            facts.push(("Placement".to_string(), placement.join(", ")));
        }
        b.push("wonders", id, "World wonder — only one may exist", facts, links);
    }

    for (tree, catalogue) in [("techs", &rules.techs), ("civics", &rules.civics)] {
        for (id, spec) in catalogue {
            let mut facts = vec![
                (
                    "Cost".to_string(),
                    format!(
                        "{} {}",
                        number(spec.cost),
                        if tree == "techs" { "Science" } else { "Culture" }
                    ),
                ),
                ("Era".to_string(), era_name(spec.era)),
            ];
            let mut links = Vec::new();
            if !spec.requires.is_empty() {
                facts.push((
                    "Requires".to_string(),
                    spec.requires.iter().map(|r| title(r)).collect::<Vec<_>>().join(", "),
                ));
                links.extend(spec.requires.iter().map(|r| format!("{tree}/{r}")));
            }
            if let Some(boost) = &spec.boost {
                facts.push((
                    if tree == "techs" { "Eureka" } else { "Inspiration" }.to_string(),
                    format!("{} × {}", title(&boost.trigger), boost.count),
                ));
            }
            if !spec.unlocks.is_empty() {
                facts.push((
                    "Unlocks".to_string(),
                    spec.unlocks
                        .iter()
                        .map(|unlock| title(&unlock.id))
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
                links.extend(
                    spec.unlocks
                        .iter()
                        .filter_map(|unlock| unlock_key(&unlock.kind, &unlock.id)),
                );
            }
            let summary = if tree == "techs" {
                format!("{} technology", era_name(spec.era))
            } else {
                format!("{} civic", era_name(spec.era))
            };
            b.push(tree, id, summary, facts, links);
        }
    }

    for (id, spec) in &rules.civs {
        let mut facts = vec![("Leader".to_string(), spec.leader.clone())];
        let mut links = Vec::new();
        if let Some(agenda) = &spec.agenda {
            let name = rules
                .agendas
                .get(agenda)
                .map(|spec| spec.name.clone())
                .unwrap_or_else(|| title(agenda));
            facts.push(("Agenda".to_string(), name));
            links.push(format!("agendas/{agenda}"));
        }
        if let Some(unit) = &spec.unique_unit {
            facts.push(("Unique unit".to_string(), title(unit)));
            links.push(format!("units/{unit}"));
        }
        for district in rules
            .districts
            .iter()
            .filter(|(_, d)| d.unique_to.as_deref() == Some(id.as_str()))
            .map(|(name, _)| name)
        {
            facts.push(("Unique district".to_string(), title(district)));
            links.push(format!("districts/{district}"));
        }
        for building in rules
            .buildings
            .iter()
            .filter(|(_, spec)| spec.unique_to.as_deref() == Some(id.as_str()))
            .map(|(name, _)| name)
        {
            facts.push(("Unique building".to_string(), title(building)));
            links.push(format!("buildings/{building}"));
        }
        b.push("civs", id, spec.note.clone(), facts, links);
    }

    for (id, spec) in &rules.agendas {
        b.push(
            "agendas",
            id,
            spec.description.clone(),
            vec![(
                "Approves of".to_string(),
                format!("{} {}", title(&spec.approves_of), title(&spec.measure)),
            )],
            Vec::new(),
        );
    }

    for (id, spec) in &rules.promotions {
        let mut facts = vec![
            ("Class".to_string(), title(&spec.class)),
            ("Tier".to_string(), spec.tier.to_string()),
        ];
        let mut links = Vec::new();
        if !spec.requires.is_empty() {
            facts.push((
                "Requires".to_string(),
                spec.requires.iter().map(|r| title(r)).collect::<Vec<_>>().join(", "),
            ));
            links.extend(spec.requires.iter().map(|r| format!("promotions/{r}")));
        }
        b.push("promotions", id, spec.note.clone(), facts, links);
    }

    for (id, spec) in &rules.policies {
        let (mut facts, mut links) = gates(&None, &spec.civic);
        facts.insert(0, ("Slot".to_string(), title(&spec.slot)));
        if let Some(replaced) = &spec.replaces {
            facts.push(("Replaces".to_string(), title(replaced)));
            links.push(format!("policies/{replaced}"));
        }
        b.push("policies", id, spec.note.clone(), facts, links);
    }

    for (id, spec) in &rules.governments {
        let (mut facts, links) = gates(&None, &spec.civic);
        let slots = serde_json::to_value(&spec.slots).unwrap_or_default();
        if let Some(map) = slots.as_object() {
            let mut listed: Vec<String> = map
                .iter()
                .filter(|(_, value)| value.as_i64().unwrap_or(0) > 0)
                .map(|(kind, value)| format!("{} {}", value, title(kind)))
                .collect();
            listed.sort();
            if !listed.is_empty() {
                facts.push(("Policy slots".to_string(), listed.join(", ")));
            }
        }
        b.push("governments", id, "Government", facts, links);
    }

    for (id, spec) in &rules.improvements {
        let (mut facts, mut links) = gates(&spec.tech, &spec.civic);
        if let Some(yields) = yields_summary(spec.yields) {
            facts.insert(0, ("Yields".to_string(), yields));
        }
        if spec.housing != 0.0 {
            facts.push(("Housing".to_string(), number(spec.housing)));
        }
        if !spec.terrain.is_empty() {
            facts.push((
                "Terrain".to_string(),
                spec.terrain.iter().map(|t| title(t)).collect::<Vec<_>>().join(", "),
            ));
        }
        if !spec.resources.is_empty() {
            facts.push((
                "Resources".to_string(),
                spec.resources.iter().map(|r| title(r)).collect::<Vec<_>>().join(", "),
            ));
            links.extend(spec.resources.iter().map(|r| format!("resources/{r}")));
        }
        b.push("improvements", id, "Tile improvement", facts, links);
    }

    for (id, spec) in &rules.resources {
        let (mut facts, mut links) = gates(&spec.tech, &spec.civic);
        facts.insert(0, ("Class".to_string(), title(&spec.class)));
        if let Some(yields) = yields_summary(spec.yields) {
            facts.push(("Yields".to_string(), yields));
        }
        if !spec.improvement.is_empty() {
            facts.push(("Improved by".to_string(), title(&spec.improvement)));
            links.push(format!("improvements/{}", spec.improvement));
        }
        b.push("resources", id, format!("{} resource", title(&spec.class)), facts, links);
    }

    for (id, spec) in &rules.terrains {
        let mut facts = vec![("Movement cost".to_string(), number(spec.move_cost))];
        if let Some(yields) = yields_summary(spec.yields) {
            facts.insert(0, ("Yields".to_string(), yields));
        }
        let summary = if spec.water {
            "Water terrain"
        } else if !spec.passable {
            "Impassable terrain"
        } else {
            "Land terrain"
        };
        b.push("terrains", id, summary, facts, Vec::new());
    }

    for (id, spec) in &rules.features {
        let mut facts = vec![("Movement cost".to_string(), number(spec.move_cost))];
        if let Some(yields) = yields_summary(spec.yields) {
            facts.insert(0, ("Yields".to_string(), yields));
        }
        if let Some(adjacent) = yields_summary(spec.adjacent_yields) {
            facts.push(("To adjacent tiles".to_string(), adjacent));
        }
        let summary = if spec.natural_wonder {
            "Natural wonder"
        } else {
            "Terrain feature"
        };
        b.push("features", id, summary, facts, Vec::new());
    }

    for (id, spec) in &rules.great_people {
        b.push(
            "great_people",
            id,
            format!("{} of the {} era", title(&spec.kind), era_name(spec.era)),
            vec![
                ("Class".to_string(), title(&spec.kind)),
                ("Era".to_string(), era_name(spec.era)),
                ("Cost".to_string(), format!("{} points", number(spec.cost))),
                ("Charges".to_string(), spec.charges.to_string()),
            ],
            Vec::new(),
        );
    }

    for (id, spec) in &rules.difficulties {
        let mut facts = vec![("Ladder position".to_string(), (spec.order + 1).to_string())];
        if let Some(yields) = yields_summary(spec.ai_yield_pct) {
            facts.push(("AI yield bonus".to_string(), format!("{yields} (%)")));
        }
        if spec.ai_combat_strength != 0.0 {
            facts.push((
                "AI Combat Strength".to_string(),
                format!("+{}", number(spec.ai_combat_strength)),
            ));
        }
        if spec.human_combat_strength != 0.0 {
            facts.push((
                "Your Combat Strength".to_string(),
                format!("+{}", number(spec.human_combat_strength)),
            ));
        }
        if !spec.ai_bonus_units.is_empty() {
            let mut listed: Vec<String> = spec
                .ai_bonus_units
                .iter()
                .map(|(unit, count)| format!("{count} × {}", title(unit)))
                .collect();
            listed.sort();
            facts.push(("AI opening units".to_string(), listed.join(", ")));
        }
        b.push("difficulties", id, "Difficulty level", facts, Vec::new());
    }

    for (id, spec) in &rules.speeds {
        b.push(
            "speeds",
            id,
            "Game speed",
            vec![
                ("Costs".to_string(), format!("{}% of standard", number(spec.cost_pct))),
                ("Length".to_string(), format!("{} turns", spec.turns)),
            ],
            Vec::new(),
        );
    }

    b.entries
        .sort_by(|left, right| (&left.category, &left.name).cmp(&(&right.category, &right.name)));
    b.entries
}

/// Entries whose name, id, category or summary contains `query`, case
/// insensitively. An empty query returns everything.
pub fn search(rules: &Rules, query: &str) -> Vec<Entry> {
    let needle = query.trim().to_ascii_lowercase();
    entries(rules)
        .into_iter()
        .filter(|entry| {
            needle.is_empty()
                || entry.name.to_ascii_lowercase().contains(&needle)
                || entry.id.contains(&needle)
                || entry.category.contains(&needle)
                || entry.summary.to_ascii_lowercase().contains(&needle)
        })
        .collect()
}

/// Render entries for a terminal.
pub fn render(entries: &[Entry]) -> String {
    let mut out = String::new();
    let mut category = String::new();
    for entry in entries {
        if entry.category != category {
            category = entry.category.clone();
            out.push_str(&format!("\n{}\n", title(&category).to_uppercase()));
        }
        out.push_str(&format!("  {} — {}\n", entry.name, entry.summary));
        for (label, value) in &entry.facts {
            out.push_str(&format!("      {label}: {value}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{entries, search, title};
    use crate::rules::Rules;

    #[test]
    fn the_pedia_covers_the_ruleset_and_reads_its_numbers_from_it() {
        let rules = Rules::shipped();
        let all = entries(&rules);
        // Every catalogue that has entries is represented.
        for category in ["units", "buildings", "districts", "wonders", "techs", "civics", "civs"] {
            assert!(
                all.iter().any(|entry| entry.category == category),
                "{category} is missing from the pedia"
            );
        }
        assert_eq!(
            all.iter().filter(|entry| entry.category == "units").count(),
            rules.units.len()
        );
        let warrior = all
            .iter()
            .find(|entry| entry.key == "units/warrior")
            .expect("the Warrior is documented");
        assert_eq!(warrior.name, "Warrior");
        let cost = format!("{} Production", rules.units["warrior"].cost.round() as i64);
        assert!(warrior.facts.iter().any(|(label, value)| label == "Cost" && *value == cost));
        // Links point at entries that exist.
        let keys: std::collections::BTreeSet<&str> =
            all.iter().map(|entry| entry.key.as_str()).collect();
        for entry in &all {
            for link in &entry.links {
                assert!(keys.contains(link.as_str()), "{} links to missing {link}", entry.key);
            }
        }
    }

    /// The pedia describes the ruleset in play, not the one we shipped —
    /// which is the whole reason to generate it.
    #[test]
    fn the_pedia_follows_a_mod() {
        let mut rules = Rules::shipped();
        rules.units.get_mut("warrior").unwrap().cost = 7.0;
        let warrior = entries(&rules)
            .into_iter()
            .find(|entry| entry.key == "units/warrior")
            .unwrap();
        assert!(warrior
            .facts
            .iter()
            .any(|(label, value)| label == "Cost" && value == "7 Production"));
    }

    #[test]
    fn search_matches_names_categories_and_summaries() {
        let rules = Rules::shipped();
        let hits = search(&rules, "warrior");
        assert!(hits.iter().any(|entry| entry.key == "units/warrior"));
        assert!(hits.iter().all(|entry| {
            entry.name.to_ascii_lowercase().contains("warrior")
                || entry.id.contains("warrior")
                || entry.summary.to_ascii_lowercase().contains("warrior")
        }));
        assert!(search(&rules, "").len() > 500);
        assert!(search(&rules, "no such thing at all").is_empty());
    }

    #[test]
    fn titles_are_readable() {
        assert_eq!(title("giant_death_robot"), "Giant Death Robot");
        assert_eq!(title("warrior"), "Warrior");
    }
}
