//! Mods: a folder of JSON overlays on the shipped ruleset.
//!
//! Unciv's defining feature is that a mod is not code. It is a folder of the
//! same JSON files the base game ships, dropped in next to them, and the
//! engine merges them at load. That is the whole reason its ruleset lives in
//! data at all, and it is what a Civ VI equivalent has to be able to do.
//!
//! A Civ VIS mod is a directory containing any subset of the files in
//! [`Rules::DATA_FILES`](crate::rules::DATA_FILES) — `units.json`,
//! `difficulties.json`, and so on — plus an optional `mod.json` naming it.
//! Each file is an object keyed by entry id, and merging follows three rules:
//!
//! - an id the base ruleset does not have is **added**;
//! - an id it does have is **merged field by field**, so a mod that only wants
//!   a cheaper Warrior writes `{"warrior": {"cost": 30}}` and inherits the
//!   rest;
//! - an id mapped to `null` is **removed**.
//!
//! Mods apply in the order given, so a later mod overrides an earlier one.
//! The merged ruleset is then run through [`crate::validate`] and refused if
//! it has errors — which is the point of having written the validator first.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;

use crate::rules::{Rules, DATA_FILES};
use crate::validate::{self, Severity};

#[derive(Clone, Debug)]
pub struct ModInfo {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    /// Which ruleset files this mod touched.
    pub files: Vec<String>,
}

/// Read and merge mods onto the shipped ruleset without installing them.
pub fn load(paths: &[PathBuf]) -> Result<(Rules, Vec<ModInfo>), String> {
    let mut values = Rules::shipped_values();
    let mut loaded = Vec::new();
    for path in paths {
        loaded.push(apply(&mut values, path)?);
    }
    let rules = Rules::from_values(values)?;
    let findings = validate::validate(&rules);
    let errors: Vec<String> = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
        .map(|finding| finding.to_string())
        .collect();
    if !errors.is_empty() {
        return Err(format!(
            "the modded ruleset does not validate:\n{}",
            errors.join("\n")
        ));
    }
    Ok((rules, loaded))
}

/// Names of the mods currently installed, in load order. Every new game
/// records these so a save can say what rules it was played under.
static ACTIVE_NAMES: OnceLock<Vec<String>> = OnceLock::new();

pub fn active_names() -> Vec<String> {
    ACTIVE_NAMES.get().cloned().unwrap_or_default()
}

/// Read, merge, validate and install mods as the active ruleset. Must happen
/// before any game exists.
pub fn activate(paths: &[PathBuf]) -> Result<Vec<ModInfo>, String> {
    let (rules, loaded) = load(paths)?;
    Rules::install(rules)?;
    let _ = ACTIVE_NAMES.set(loaded.iter().map(|info| info.name.clone()).collect());
    Ok(loaded)
}

/// Parse a `--mods` argument: paths separated by commas.
pub fn parse_arg(arg: &str) -> Vec<PathBuf> {
    arg.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn apply(values: &mut BTreeMap<String, Value>, path: &Path) -> Result<ModInfo, String> {
    if !path.is_dir() {
        return Err(format!("mod {} is not a directory", path.display()));
    }
    let known: Vec<&str> = DATA_FILES.iter().map(|(name, _)| *name).collect();
    let mut info = ModInfo {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string()),
        description: String::new(),
        path: path.to_path_buf(),
        files: Vec::new(),
    };

    let manifest = path.join("mod.json");
    if manifest.is_file() {
        let text = std::fs::read_to_string(&manifest)
            .map_err(|error| format!("cannot read {}: {error}", manifest.display()))?;
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|error| format!("{}: {error}", manifest.display()))?;
        if let Some(name) = parsed["name"].as_str() {
            info.name = name.to_string();
        }
        if let Some(description) = parsed["description"].as_str() {
            info.description = description.to_string();
        }
    }

    // Anything named like a ruleset file is an overlay; anything else in the
    // folder is the author's business, not ours.
    let entries = std::fs::read_dir(path)
        .map_err(|error| format!("cannot read mod {}: {error}", path.display()))?;
    let mut overlays: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read mod {}: {error}", path.display()))?;
        let file = entry.path();
        let Some(stem) = file.file_stem().map(|stem| stem.to_string_lossy().to_string()) else {
            continue;
        };
        if file.extension().is_none_or(|ext| ext != "json") || stem == "mod" {
            continue;
        }
        if !known.contains(&stem.as_str()) {
            return Err(format!(
                "mod {} has {stem}.json, which is not a ruleset file (expected one of: {})",
                info.name,
                known.join(", ")
            ));
        }
        overlays.push((stem, file));
    }
    overlays.sort();

    for (name, file) in overlays {
        let text = std::fs::read_to_string(&file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        let overlay: Value = serde_json::from_str(&text)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        let base = values
            .get_mut(&name)
            .ok_or_else(|| format!("mod {} overlays unknown file {name}.json", info.name))?;
        merge(base, overlay).map_err(|error| format!("{}: {error}", file.display()))?;
        info.files.push(name);
    }
    Ok(info)
}

/// Merge `overlay` into `base`: objects deep-merge, `null` removes, and
/// anything else replaces outright.
fn merge(base: &mut Value, overlay: Value) -> Result<(), String> {
    let (Some(base_map), Value::Object(overlay_map)) = (base.as_object_mut(), overlay) else {
        return Err("a ruleset file must be a JSON object".to_string());
    };
    for (key, value) in overlay_map {
        if value.is_null() {
            base_map.remove(&key);
            continue;
        }
        match base_map.get_mut(&key) {
            Some(existing) if existing.is_object() && value.is_object() => {
                merge(existing, value)?;
            }
            _ => {
                base_map.insert(key, value);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{load, merge};
    use serde_json::json;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("civvis-mod-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &PathBuf, file: &str, value: serde_json::Value) {
        std::fs::write(dir.join(file), serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    /// The three merge rules: add, field-wise override, and removal.
    #[test]
    fn a_mod_adds_overrides_and_removes_entries() {
        let dir = scratch("basics");
        write(
            &dir,
            "mod.json",
            json!({"name": "Cheap Warriors", "description": "A test mod"}),
        );
        write(
            &dir,
            "units.json",
            json!({
                // Override one field and inherit the rest.
                "warrior": {"cost": 10},
                // Add something new.
                "test_skirmisher": {
                    "class": "military", "cost": 40, "moves": 2, "strength": 22,
                    "promotion_class": "melee", "tech": "bronze_working"
                },
            }),
        );
        // Removing an agenda means the leader who held it has to let go of
        // it too, or the ruleset no longer validates — which is the point.
        write(&dir, "agendas.json", json!({"tlatoani": null}));
        write(&dir, "civs.json", json!({"Aztec": {"agenda": null}}));

        let shipped = crate::rules::Rules::shipped();
        let (rules, loaded) = load(&[dir.clone()]).expect("the mod loads");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Cheap Warriors");
        assert_eq!(loaded[0].files, vec!["agendas", "civs", "units"]);

        assert_eq!(rules.units["warrior"].cost, 10.0);
        // The fields the mod did not mention survived the merge.
        assert_eq!(rules.units["warrior"].strength, shipped.units["warrior"].strength);
        assert_eq!(rules.units["test_skirmisher"].strength, 22.0);
        assert!(!rules.agendas.contains_key("tlatoani"));
        assert_eq!(rules.civs["Aztec"].agenda, None);
        assert_eq!(rules.civs["Aztec"].leader, shipped.civs["Aztec"].leader);
        // The shipped ruleset is untouched by any of this.
        assert!(shipped.units["warrior"].cost > 10.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A mod that breaks the ruleset is refused, with the reason.
    #[test]
    fn a_mod_that_breaks_the_ruleset_is_refused() {
        let dir = scratch("broken");
        write(&dir, "units.json", json!({"warrior": {"tech": "phlogiston"}}));
        let error = load(&[dir.clone()]).map(|_| ()).expect_err("a dangling tech is refused");
        assert!(error.contains("does not validate"), "{error}");
        assert!(error.contains("phlogiston"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file the engine does not know about is a typo, not a silent no-op.
    #[test]
    fn an_unknown_ruleset_file_is_refused() {
        let dir = scratch("typo");
        write(&dir, "unit.json", json!({}));
        let error = load(&[dir.clone()]).map(|_| ()).expect_err("units.json was misspelled");
        assert!(error.contains("not a ruleset file"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Later mods win, which is what makes an ordered list meaningful.
    #[test]
    fn later_mods_override_earlier_ones() {
        let (first, second) = (scratch("first"), scratch("second"));
        write(&first, "speeds.json", json!({"standard": {"turns": 111}}));
        write(&second, "speeds.json", json!({"standard": {"turns": 222}}));
        let (rules, loaded) = load(&[first.clone(), second.clone()]).expect("both load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(rules.speeds["standard"].turns, 222);
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    #[test]
    fn merging_replaces_scalars_and_recurses_into_objects() {
        let mut base = json!({"a": {"x": 1, "y": 2}, "b": 3});
        merge(&mut base, json!({"a": {"y": 9, "z": 10}, "b": 4, "c": 5})).unwrap();
        assert_eq!(base, json!({"a": {"x": 1, "y": 9, "z": 10}, "b": 4, "c": 5}));
    }
}
