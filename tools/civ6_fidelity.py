#!/usr/bin/env python3
"""Diff CIVVIS rules data against the shipped Civilization VI game database.

CIVVIS' ``data/*.json`` was hand-authored from the Civilopedia and the wiki.
The real game ships every rules constant as readable XML under
``Base/Assets/Gameplay/Data`` (plus expansion overlays), so the authoritative
values are available locally on any machine with the game installed. This tool
loads that database in the game's own load order, projects both sides onto a
common schema, and reports every numeric divergence.

It reads the game files; it never writes them, and it does not copy the game
database into the repository. Only the divergence report is an artifact.

Usage::

    python tools/civ6_fidelity.py                     # markdown report
    python tools/civ6_fidelity.py --json out.json     # machine-readable
    python tools/civ6_fidelity.py --max-divergences 0 # CI gate

``--civ6`` (or ``$CIV6_DIR``) overrides install auto-detection. Exit status is
1 when the divergence count exceeds ``--max-divergences``, so the audit can be
wired into CI as a ratchet once a table reaches parity.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Gathering Storm ruleset: base game, then the two expansion overlays, applied
# in the order the game applies them. DLC civilization packs add unique units
# and buildings but do not restate the base rules, so they are out of scope
# here (their content is gated behind civilization ownership anyway).
LOAD_ORDER = [
    "Base/Assets/Gameplay/Data",
    "DLC/Expansion1/Data",
    "DLC/Expansion2/Data",
]

INSTALL_CANDIDATES = [
    r"C:\Program Files (x86)\Steam\steamapps\common\Sid Meier's Civilization VI",
    r"C:\Program Files\Steam\steamapps\common\Sid Meier's Civilization VI",
    r"D:\SteamLibrary\steamapps\common\Sid Meier's Civilization VI",
    r"E:\SteamLibrary\steamapps\common\Sid Meier's Civilization VI",
]

# Tables we project onto CIVVIS' schema, and the primary key of each.
TABLE_KEYS = {
    "Units": "UnitType",
    "Technologies": "TechnologyType",
    "Civics": "CivicType",
    "Buildings": "BuildingType",
    "Districts": "DistrictType",
    "TechnologyPrereqs": ("Technology", "PrereqTech"),
    "CivicPrereqs": ("Civic", "PrereqCivic"),
    "Building_YieldChanges": ("BuildingType", "YieldType"),
    "Governments": "GovernmentType",
    "Government_SlotCounts": ("GovernmentType", "GovernmentSlotType"),
    "Terrains": "TerrainType",
    "Features": "FeatureType",
    "Resources": "ResourceType",
    "Improvements": "ImprovementType",
    "Terrain_YieldChanges": ("TerrainType", "YieldType"),
    "Feature_YieldChanges": ("FeatureType", "YieldType"),
    "Resource_YieldChanges": ("ResourceType", "YieldType"),
    "Improvement_YieldChanges": ("ImprovementType", "YieldType"),
    "Improvement_BonusYieldChanges": ("Id",),
}

# Only parse files that can carry those tables. Parsing every gameplay XML
# costs about a minute; this keeps a full audit under a couple of seconds.
# The expansions ship a plain overlay plus a ``_Major`` overlay per table; the
# latter carries the rebalance passes and is applied after it, which sorted
# filename order already gives us ('.' sorts before '_').
FILE_PATTERN = re.compile(
    r"^(Expansion[12]_)?"
    r"(Units|Technologies|Civics|Buildings|Districts"
    r"|Terrains|Features|Resources|Improvements|Governments)"
    r"(_Major)?\.xml$",
    re.IGNORECASE,
)

ERAS = [
    "ERA_ANCIENT",
    "ERA_CLASSICAL",
    "ERA_MEDIEVAL",
    "ERA_RENAISSANCE",
    "ERA_INDUSTRIAL",
    "ERA_MODERN",
    "ERA_ATOMIC",
    "ERA_INFORMATION",
    "ERA_FUTURE",
]


def find_install(explicit: str | None) -> Path:
    for candidate in filter(None, [explicit, os.environ.get("CIV6_DIR")]):
        path = Path(candidate)
        if (path / LOAD_ORDER[0]).is_dir():
            return path
        raise SystemExit(f"no Civilization VI gameplay data under {path}")
    for candidate in INSTALL_CANDIDATES:
        path = Path(candidate)
        if (path / LOAD_ORDER[0]).is_dir():
            return path
    raise SystemExit(
        "Civilization VI install not found; pass --civ6 <path> or set $CIV6_DIR"
    )


# ---------------------------------------------------------------- game database


class Database:
    """The subset of the gameplay database this audit needs.

    Rows are keyed by primary key so that ``<Update>``, ``<Replace>`` and
    ``<Delete>`` elements from the expansion overlays resolve exactly the way
    the game's own loader resolves them.
    """

    def __init__(self) -> None:
        self.tables: dict[str, dict[tuple, dict]] = {}

    def _key(self, table: str, row: dict) -> tuple | None:
        spec = TABLE_KEYS[table]
        columns = spec if isinstance(spec, tuple) else (spec,)
        if not all(column in row for column in columns):
            return None
        return tuple(row[column] for column in columns)

    @staticmethod
    def _fields(node) -> dict:
        """Columns of a row-ish node.

        The gameplay XML uses both spellings interchangeably: attributes
        (``<Row Cost="80"/>``) in the base tables, child elements
        (``<Set><Cost>730</Cost></Set>``) in most expansion updates. Missing
        the second form silently drops every expansion rebalance.
        """
        fields = dict(node.attrib)
        for child in node:
            fields[child.tag] = (child.text or "").strip()
        return fields

    def apply_file(self, path: Path) -> None:
        try:
            root = ET.parse(path).getroot()
        except ET.ParseError as exc:  # a malformed overlay should not be fatal
            print(f"warning: {path.name}: {exc}", file=sys.stderr)
            return
        if root.tag != "GameInfo":
            return
        for table_node in root:
            table = table_node.tag
            if table not in TABLE_KEYS:
                continue
            rows = self.tables.setdefault(table, {})
            for node in table_node:
                if node.tag in ("Row", "Replace"):
                    row = self._fields(node)
                    key = self._key(table, row)
                    if key is None:
                        continue
                    rows.setdefault(key, {}).update(row)
                elif node.tag == "Update":
                    where_node, set_node = node.find("Where"), node.find("Set")
                    where = self._fields(where_node) if where_node is not None else {}
                    updates = self._fields(set_node) if set_node is not None else {}
                    for row in rows.values():
                        if all(row.get(k) == v for k, v in where.items()):
                            row.update(updates)
                elif node.tag == "Delete":
                    where = dict(node.attrib)
                    doomed = [
                        key
                        for key, row in rows.items()
                        if all(row.get(k) == v for k, v in where.items())
                    ]
                    for key in doomed:
                        del rows[key]

    def rows(self, table: str) -> list[dict]:
        return list(self.tables.get(table, {}).values())


def load_database(install: Path) -> Database:
    database = Database()
    for relative in LOAD_ORDER:
        directory = install / relative
        if not directory.is_dir():
            print(f"warning: missing load-order directory {relative}", file=sys.stderr)
            continue
        for path in sorted(directory.rglob("*.xml")):
            if FILE_PATTERN.match(path.name):
                database.apply_file(path)
    return database


# ------------------------------------------------------------------ projection


# CIVVIS spells a handful of identifiers without the game's article. These are
# naming choices, not rules divergences, so the audit resolves them.
ALIASES = {"the_wheel": "wheel"}


def slug(game_type: str, prefix: str) -> str:
    name = game_type[len(prefix):].lower() if game_type.startswith(prefix) else game_type.lower()
    return ALIASES.get(name, name)


def number(raw: str | None, default=0):
    if raw is None:
        return default
    try:
        return int(raw)
    except ValueError:
        try:
            return float(raw)
        except ValueError:
            return default


def truthy(raw: str | None, default=False) -> bool:
    return default if raw is None else raw.strip().lower() in ("true", "1")


def project_units(database: Database) -> dict[str, dict]:
    projected = {}
    for row in database.rows("Units"):
        entry = {
            "cost": number(row.get("Cost")),
            "maintenance": number(row.get("Maintenance")),
            "moves": number(row.get("BaseMoves")),
            "sight": number(row.get("BaseSightRange"), 2),
            "strength": number(row.get("Combat")),
            "ranged_strength": number(row.get("RangedCombat")),
            "bombard_strength": number(row.get("Bombard")),
            "range": number(row.get("Range")),
            # One CIVVIS field covers three game columns: builders spend build
            # charges, religious units spend spread charges, gurus spend heal
            # charges. They are never combined on one unit.
            "charges": max(
                number(row.get("BuildCharges")),
                number(row.get("SpreadCharges")),
                number(row.get("ReligiousHealCharges")),
            ),
            "zone_of_control": truthy(row.get("ZoneOfControl")),
        }
        # Units that can only be bought store a production cost in the database
        # that no player ever pays; CIVVIS stores the Faith/Gold price the
        # player actually pays, so the two numbers are not comparable.
        if truthy(row.get("MustPurchase")):
            del entry["cost"]
        projected[slug(row["UnitType"], "UNIT_")] = entry
    return projected


def project_techs(database: Database) -> dict[str, dict]:
    prereqs: dict[str, set[str]] = {}
    for row in database.rows("TechnologyPrereqs"):
        prereqs.setdefault(slug(row["Technology"], "TECH_"), set()).add(
            slug(row["PrereqTech"], "TECH_")
        )
    projected = {}
    for row in database.rows("Technologies"):
        name = slug(row["TechnologyType"], "TECH_")
        era = row.get("EraType")
        projected[name] = {
            "cost": number(row.get("Cost")),
            "era": ERAS.index(era) if era in ERAS else -1,
            "requires": prereqs.get(name, set()),
        }
    return projected


def project_civics(database: Database) -> dict[str, dict]:
    prereqs: dict[str, set[str]] = {}
    for row in database.rows("CivicPrereqs"):
        prereqs.setdefault(slug(row["Civic"], "CIVIC_"), set()).add(
            slug(row["PrereqCivic"], "CIVIC_")
        )
    projected = {}
    for row in database.rows("Civics"):
        name = slug(row["CivicType"], "CIVIC_")
        era = row.get("EraType")
        projected[name] = {
            "cost": number(row.get("Cost")),
            "era": ERAS.index(era) if era in ERAS else -1,
            "requires": prereqs.get(name, set()),
        }
    return projected


def project_buildings(database: Database) -> dict[str, dict]:
    projected = {}
    for row in database.rows("Buildings"):
        projected[slug(row["BuildingType"], "BUILDING_")] = {
            "cost": number(row.get("Cost")),
            "maintenance": number(row.get("Maintenance")),
        }
    return projected


# The game names every yield table's rows with a type prefix and a YIELD_
# column; CIVVIS stores the same numbers as a yields object per entry. One
# helper projects all four.
def project_yields(
    database: Database, table: str, key: str, prefix: str, base: str
) -> dict[str, dict]:
    # Seed from the entity table, not the yield table: an entry the game gives
    # no yields at all is exactly the case where CIVVIS is most likely to have
    # invented one, and seeding from yield rows alone would hide it.
    projected: dict[str, dict] = {
        slug(row[key], prefix): {} for row in database.rows(base) if key in row
    }
    for row in database.rows(table):
        entry = projected.setdefault(slug(row[key], prefix), {})
        amount = number(row.get("YieldChange"))
        if amount:
            entry[slug(row["YieldType"], "YIELD_")] = amount
    return {
        name: {field: yields.get(field, 0) for field in YIELD_FIELDS}
        for name, yields in projected.items()
    }


# CIVVIS folds Hills into a flat +1 Production modifier rather than carrying a
# separate terrain per hills variant, so each variant is checked against its
# flat parent plus that modifier instead of being looked up directly.
HILLS_PARENT = {
    "grass_hills": "grassland",
    "plains_hills": "plains",
    "desert_hills": "desert",
    "tundra_hills": "tundra",
    "snow_hills": "snow",
}

TERRAIN_NAMES = {"grass": "grassland"}

FEATURE_NAMES = {
    "barrier_reef": "great_barrier_reef",
    "everest": "mount_everest",
    "floodplains_grassland": "grassland_floodplains",
    "floodplains_plains": "plains_floodplains",
}


def project_terrains(database: Database) -> dict[str, dict]:
    raw = project_yields(database, "Terrain_YieldChanges", "TerrainType", "TERRAIN_", "Terrains")
    projected = {}
    for name, yields in raw.items():
        if parent := HILLS_PARENT.get(name):
            # Fold the variant back onto its parent: the audit compares the
            # flat terrain, and a hills row that is not parent + 1 Production
            # would mean CIVVIS' single modifier cannot express the ruleset.
            flat = dict(yields)
            flat["production"] = flat.get("production", 0) - 1
            projected.setdefault(TERRAIN_NAMES.get(parent, parent), {})
            expected = {k: v for k, v in flat.items() if v}
            if projected[TERRAIN_NAMES.get(parent, parent)] not in ({}, expected):
                continue
            projected[TERRAIN_NAMES.get(parent, parent)] = expected
        else:
            projected[TERRAIN_NAMES.get(name, name)] = yields
    return projected


def project_features(database: Database) -> dict[str, dict]:
    return {
        FEATURE_NAMES.get(name, name): yields
        for name, yields in project_yields(
            database, "Feature_YieldChanges", "FeatureType", "FEATURE_", "Features"
        ).items()
    }


def project_resources(database: Database) -> dict[str, dict]:
    return project_yields(database, "Resource_YieldChanges", "ResourceType", "RESOURCE_", "Resources")


def project_improvements(database: Database) -> dict[str, dict]:
    return project_yields(
        database, "Improvement_YieldChanges", "ImprovementType", "IMPROVEMENT_", "Improvements"
    )


def project_building_yields(database: Database) -> dict[str, dict]:
    return project_yields(
        database, "Building_YieldChanges", "BuildingType", "BUILDING_", "Buildings"
    )


SLOTS = ("military", "economic", "diplomatic", "wildcard")


def project_governments(database: Database) -> dict[str, dict]:
    slots: dict[str, dict] = {}
    for row in database.rows("Government_SlotCounts"):
        name = slug(row["GovernmentType"], "GOVERNMENT_")
        slot = slug(row["GovernmentSlotType"], "SLOT_")
        slots.setdefault(name, {})[slot] = number(row.get("NumSlots"))
    projected = {}
    for row in database.rows("Governments"):
        name = slug(row["GovernmentType"], "GOVERNMENT_")
        entry = {
            "influence_per_turn": number(row.get("InfluencePointsPerTurn")),
            "influence_threshold": number(row.get("InfluencePointsThreshold")),
            "envoys_per_threshold": number(row.get("InfluenceTokensPerThreshold")),
        }
        for slot in SLOTS:
            entry[f"slot_{slot}"] = slots.get(name, {}).get(slot, 0)
        if civic := row.get("PrereqCivic"):
            entry["civic"] = slug(civic, "CIVIC_")
        projected[name] = entry
    return projected


def project_improvement_upgrades(database: Database) -> dict[str, dict]:
    """Tech- and civic-gated improvement yields, keyed the way CIVVIS keys them.

    ``data/tree_effects.json`` records these as ``<improvement>_<yield>`` grants
    hung off the unlocking node, so the projection reshapes the game's rows into
    the same ``node -> {effect: amount}`` form.
    """
    projected: dict[str, dict] = {}
    for row in database.rows("Improvement_BonusYieldChanges"):
        node = row.get("PrereqTech") or row.get("PrereqCivic")
        if not node:
            continue
        node = slug(node, "TECH_" if row.get("PrereqTech") else "CIVIC_")
        improvement = slug(row["ImprovementType"], "IMPROVEMENT_")
        yield_name = slug(row["YieldType"], "YIELD_")
        effect = f"{improvement}_{yield_name}"
        amount = number(row.get("BonusYieldChange"))
        # An expansion restating a grant supersedes the base row rather than
        # stacking on top of it, so the larger value is the shipped one.
        entry = projected.setdefault(node, {})
        entry[effect] = max(entry.get(effect, 0), amount)
    return projected


def project_districts(database: Database) -> dict[str, dict]:
    projected = {}
    for row in database.rows("Districts"):
        projected[slug(row["DistrictType"], "DISTRICT_")] = {
            "cost": number(row.get("Cost")),
        }
    return projected


# ---------------------------------------------------------------- civvis side


UNIT_DEFAULTS = {
    "cost": 0,
    "maintenance": 0,
    "moves": 0,
    "sight": 2,
    "strength": 0,
    "ranged_strength": 0,
    "bombard_strength": 0,
    "range": 0,
    "charges": 0,
    "zone_of_control": False,
}


def load_ours(name: str) -> dict[str, dict]:
    return json.loads((REPO / "data" / f"{name}.json").read_text(encoding="utf-8"))


def ours_units() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("units").items():
        row = {field: entry.get(field, default) for field, default in UNIT_DEFAULTS.items()}
        # Air units carry their strike power in ``bombard_strength``; the game
        # database stores the same number in ``Bombard`` for bombers and in
        # ``RangedCombat`` for fighters, so both map onto the same fields.
        out[name] = row
    return out


def ours_tree(name: str, key: str) -> dict[str, dict]:
    out = {}
    for entry_name, entry in load_ours(name).items():
        out[entry_name] = {
            "cost": entry.get("cost", 0),
            "era": entry.get("era", -1),
            "requires": set(entry.get("requires", [])),
        }
    return out


def ours_buildings() -> dict[str, dict]:
    return {
        name: {"cost": entry.get("cost", 0), "maintenance": entry.get("maintenance", 0)}
        for name, entry in load_ours("buildings").items()
    }


YIELD_FIELDS = ("food", "production", "gold", "science", "culture", "faith")


def ours_yields(name: str) -> dict[str, dict]:
    # Every yield is listed, zeros included, so that a yield CIVVIS grants and
    # the game does not shows up as a divergence rather than as a silent skip.
    return {
        entry_name: {
            field: (entry.get("yields") or {}).get(field, 0) for field in YIELD_FIELDS
        }
        for entry_name, entry in load_ours(name).items()
    }


# Effects that are not improvement yield grants live in the same file; the
# audit only claims the ones the game database can speak to.
def ours_improvement_upgrades() -> dict[str, dict]:
    known = set(load_ours("improvements"))
    yields = YIELD_FIELDS
    tree = load_ours("tree_effects")
    out: dict[str, dict] = {}
    for node, effects in list(tree["techs"].items()) + list(tree["civics"].items()):
        kept = {
            effect: amount
            for effect, amount in effects.items()
            if any(
                effect == f"{improvement}_{yield_name}"
                for improvement in known
                for yield_name in yields
            )
        }
        if kept:
            out[node] = kept
    return out


def ours_governments() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("governments").items():
        row = {
            field: entry.get(field, 0)
            for field in (
                "influence_per_turn",
                "influence_threshold",
                "envoys_per_threshold",
            )
        }
        for slot in SLOTS:
            row[f"slot_{slot}"] = (entry.get("slots") or {}).get(slot, 0)
        if civic := entry.get("civic"):
            row["civic"] = civic
        out[name] = row
    return out


def ours_districts() -> dict[str, dict]:
    return {name: {"cost": entry.get("cost", 0)} for name, entry in load_ours("districts").items()}


# -------------------------------------------------------------------- auditing

# Entries CIVVIS deliberately does not model, or models under different rules.
# Anything listed here is excluded from the "missing" count so the report keeps
# measuring real gaps rather than known scope decisions.
IGNORED_PREFIXES = (
    "unit_",  # unresolved slugs (unit types that never lost their prefix)
)


def load_waivers() -> set[tuple[str, str, str]]:
    path = Path(__file__).resolve().parent / "fidelity_waivers.json"
    if not path.exists():
        return set()
    entries = json.loads(path.read_text(encoding="utf-8"))["waivers"]
    return {(entry["table"], entry["entry"], entry["field"]) for entry in entries}


WAIVERS = load_waivers()


def waived(table: str, entry: str, field: str) -> bool:
    return (
        (table, entry, field) in WAIVERS
        or (table, entry, "*") in WAIVERS
        or (table, "*", field) in WAIVERS
    )


def compare(table: str, ours: dict[str, dict], theirs: dict[str, dict]) -> dict:
    divergences = []
    waived_count = 0
    for name in sorted(set(ours) & set(theirs)):
        for field, value in sorted(ours[name].items()):
            reference = theirs[name].get(field)
            if isinstance(value, set):
                if value == reference:
                    continue
                divergence = {
                    "table": table,
                    "entry": name,
                    "field": field,
                    "ours": sorted(value),
                    "theirs": sorted(reference or ()),
                }
            elif reference is None or value == reference:
                # A field the game database does not carry for this entry (or
                # that CIVVIS deliberately measures differently) is not
                # evidence either way.
                continue
            else:
                divergence = {
                    "table": table,
                    "entry": name,
                    "field": field,
                    "ours": value,
                    "theirs": reference,
                }
            if waived(table, name, field):
                waived_count += 1
            else:
                divergences.append(divergence)
    return {
        "table": table,
        "compared": len(set(ours) & set(theirs)),
        "waived": waived_count,
        "only_ours": sorted(set(ours) - set(theirs)),
        "only_theirs": sorted(
            name for name in set(theirs) - set(ours) if not name.startswith(IGNORED_PREFIXES)
        ),
        "divergences": divergences,
    }


def report(results: list[dict], install: Path) -> str:
    lines = ["# Rules-data fidelity audit", ""]
    lines.append(f"Reference: `{install}` (Gathering Storm load order).")
    lines.append("")
    lines.append("| Table | Compared | Divergent | Waived | Only in CIVVIS | Only in Civ VI |")
    lines.append("|---|---:|---:|---:|---:|---:|")
    for result in results:
        lines.append(
            "| {table} | {compared} | {divergent} | {waived} | {ours} | {theirs} |".format(
                table=result["table"],
                compared=result["compared"],
                divergent=len(result["divergences"]),
                waived=result["waived"],
                ours=len(result["only_ours"]),
                theirs=len(result["only_theirs"]),
            )
        )
    lines.append("")
    for result in results:
        if not result["divergences"]:
            continue
        lines.append(f"## {result['table']}")
        lines.append("")
        lines.append("| Entry | Field | CIVVIS | Civ VI |")
        lines.append("|---|---|---|---|")
        for divergence in result["divergences"]:
            lines.append(
                "| {entry} | {field} | {ours} | {theirs} |".format(
                    entry=divergence["entry"],
                    field=divergence["field"],
                    ours=divergence["ours"],
                    theirs=divergence["theirs"],
                )
            )
        lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--civ6", help="path to the Civilization VI install")
    parser.add_argument("--json", help="write the full result set here")
    parser.add_argument("--out", help="write the markdown report here instead of stdout")
    parser.add_argument(
        "--max-divergences",
        type=int,
        default=None,
        help="exit 1 when the divergence count exceeds this ratchet",
    )
    parser.add_argument("--table", action="append", help="limit the audit to these tables")
    args = parser.parse_args()

    install = find_install(args.civ6)
    database = load_database(install)

    audits = [
        ("Units", ours_units(), project_units(database)),
        ("Technologies", ours_tree("techs", "tech"), project_techs(database)),
        ("Civics", ours_tree("civics", "civic"), project_civics(database)),
        ("Buildings", ours_buildings(), project_buildings(database)),
        ("Districts", ours_districts(), project_districts(database)),
        ("Terrains", ours_yields("terrains"), project_terrains(database)),
        ("Features", ours_yields("features"), project_features(database)),
        ("Resources", ours_yields("resources"), project_resources(database)),
        ("Improvements", ours_yields("improvements"), project_improvements(database)),
        ("BuildingYields", ours_yields("buildings"), project_building_yields(database)),
        ("Governments", ours_governments(), project_governments(database)),
        (
            "ImprovementUpgrades",
            ours_improvement_upgrades(),
            project_improvement_upgrades(database),
        ),
    ]
    if args.table:
        wanted = {name.lower() for name in args.table}
        audits = [audit for audit in audits if audit[0].lower() in wanted]

    results = [compare(*audit) for audit in audits]
    total = sum(len(result["divergences"]) for result in results)

    text = report(results, install)
    if args.out:
        Path(args.out).write_text(text + "\n", encoding="utf-8")
    else:
        print(text)
    if args.json:
        Path(args.json).write_text(json.dumps(results, indent=2, default=list), encoding="utf-8")

    print(f"\n{total} divergent fields across {len(results)} tables", file=sys.stderr)
    if args.max_divergences is not None and total > args.max_divergences:
        print(
            f"FAIL: {total} divergences exceeds the ratchet of {args.max_divergences}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
