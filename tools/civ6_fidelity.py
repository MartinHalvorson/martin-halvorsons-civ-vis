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
# in the order the game applies them, then the content packs a standard
# all-content game enables (civilization, leader and landmark packs).
# Scenario and optional-game-mode packs stay out: their rules only apply
# inside those modes.
CONTENT_PACKS = [
    "Aztec_Montezuma",
    "Poland_Jadwiga",
    "Australia",
    "Macedonia_Persia",
    "Nubia_Amanitore",
    "Indonesia_Khmer",
    "Maya_GranColombia",
    "GranColombia_Maya",
    "Ethiopia",
    "Byzantium_Gaul",
    "Babylon",
    "KublaiKhan_Vietnam",
    "Portugal",
    "VikingsLandmarks",
    "CatherineDeMedici",
    "TeddyRoosevelt",
    "GreatBuilders",
    "GreatNegotiators",
    "GreatWarlords",
    "RulersOfChina",
    "RulersOfEngland",
    "RulersOfTheSahara",
    "JuliusCaesar",
]
LOAD_ORDER = [
    "Base/Assets/Gameplay/Data",
    "DLC/Expansion1/Data",
    "DLC/Expansion2/Data",
] + [f"DLC/{pack}/Data" for pack in CONTENT_PACKS]

# Pack files that never carry rules tables, or that belong to optional modes.
PACK_EXCLUDE = re.compile(
    r"_MODE|Icons|Colors|Config|Civilopedia|RemoveData|Loc_|Text|Audio|ARX",
    re.IGNORECASE,
)

INSTALL_CANDIDATES = [
    r"C:\Program Files (x86)\Steam\steamapps\common\Sid Meier's Civilization VI",
    r"C:\Program Files\Steam\steamapps\common\Sid Meier's Civilization VI",
    r"D:\SteamLibrary\steamapps\common\Sid Meier's Civilization VI",
    r"E:\SteamLibrary\steamapps\common\Sid Meier's Civilization VI",
]

# Tables we project onto CIVVIS' schema, and the primary key of each.
TABLE_KEYS = {
    "Units": "UnitType",
    "UnitUpgrades": ("Unit", "UpgradeUnit"),
    "Technologies": "TechnologyType",
    "Civics": "CivicType",
    "Buildings": "BuildingType",
    "Districts": "DistrictType",
    "TechnologyPrereqs": ("Technology", "PrereqTech"),
    "CivicPrereqs": ("Civic", "PrereqCivic"),
    "Terrains": "TerrainType",
    "Terrain_YieldChanges": ("TerrainType", "YieldType"),
    "Features": "FeatureType",
    "Feature_YieldChanges": ("FeatureType", "YieldType"),
    "Feature_AdjacentYields": ("FeatureType", "YieldType"),
    "Resources": "ResourceType",
    "Resource_YieldChanges": ("ResourceType", "YieldType"),
    "Resource_ValidTerrains": ("ResourceType", "TerrainType"),
    "Resource_ValidFeatures": ("ResourceType", "FeatureType"),
    "Improvements": "ImprovementType",
    "Improvement_YieldChanges": ("ImprovementType", "YieldType"),
    "Improvement_ValidTerrains": ("ImprovementType", "TerrainType"),
    "Improvement_ValidFeatures": ("ImprovementType", "FeatureType"),
    "Improvement_ValidResources": ("ImprovementType", "ResourceType"),
    "Improvement_ValidBuildUnits": ("ImprovementType", "UnitType"),
    "Improvement_BonusYieldChanges": ("Id",),
    "Resource_Harvests": ("ResourceType", "YieldType"),
    "Feature_Removes": ("FeatureType", "YieldType"),
    "District_Adjacencies": ("DistrictType", "YieldChangeId"),
    "Adjacency_YieldChanges": "ID",
    "Boosts": ("TechnologyType", "CivicType"),
    "GoodyHuts": "GoodyHutType",
    "GoodyHutSubTypes": "SubTypeGoodyHut",
    "ModifierArguments": ("ModifierId", "Name"),
    "Eras": "EraType",
    "GreatPersonIndividuals": "GreatPersonIndividualType",
    "GlobalParameters": "Name",
    "GreatWorks": "GreatWorkType",
    "GreatWork_YieldChanges": ("GreatWorkType", "YieldType"),
    "WMDs": "WeaponType",
    "Maps": "MapSizeType",
    "Happinesses": "HappinessType",
    "Building_YieldChanges": ("BuildingType", "YieldType"),
    "Building_GreatPersonPoints": ("BuildingType", "GreatPersonClassType"),
    "Building_GreatWorks": ("BuildingType", "GreatWorkSlotType"),
    "Building_ValidTerrains": ("BuildingType", "TerrainType"),
    "Building_ValidFeatures": ("BuildingType", "FeatureType"),
    "Building_RequiredFeatures": ("BuildingType", "FeatureType"),
    "Policies": "PolicyType",
    "ObsoletePolicies": ("PolicyType", "ObsoletePolicy"),
    "Governments": "GovernmentType",
    "Government_SlotCounts": ("GovernmentType", "GovernmentSlotType"),
    "Beliefs": "BeliefType",
    "UnitPromotions": "UnitPromotionType",
    "UnitPromotionPrereqs": ("UnitPromotion", "PrereqUnitPromotion"),
    "Projects": "ProjectType",
    "Project_GreatPersonPoints": ("ProjectType", "GreatPersonClassType"),
    "Project_YieldConversions": ("ProjectType", "YieldType"),
}

# Only parse files that can carry those tables. Parsing every gameplay XML
# costs about a minute; this keeps a full audit under a couple of seconds.
# The expansions ship a plain overlay plus a ``_Major`` overlay per table; the
# latter carries the rebalance passes and is applied after it, which sorted
# filename order already gives us ('.' sorts before '_').
FILE_PATTERN = re.compile(
    r"^(Expansion[12]_)?"
    r"(Units|Technologies|Civics|Buildings|Districts|Terrains|Features|Resources"
    r"|Improvements|Policies|Governments|Beliefs|UnitPromotions|Projects|GreatWorks"
    r"|GoodyHuts|Eras|GreatPeople(?:_[A-Za-z]+)?|GlobalParameters|WMDs|Maps"
    r"|Happinesses|Alliances)"
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
        if table == "Boosts":
            # A boost row names the technology OR the civic it boosts,
            # never both.
            node = row.get("TechnologyType") or row.get("CivicType")
            return (node,) if node else None
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
            if not relative.startswith("DLC/"):
                print(f"warning: missing load-order directory {relative}", file=sys.stderr)
            continue
        core = relative in LOAD_ORDER[:3]
        for path in sorted(directory.rglob("*.xml")):
            if core:
                if FILE_PATTERN.match(path.name):
                    database.apply_file(path)
            elif not PACK_EXCLUDE.search(path.name):
                # Content packs interleave tables freely (and ship
                # ``<Pack>_Expansion2.xml`` compat overlays), so parse
                # everything that is not clearly cosmetic; apply_file skips
                # tables the audit does not track.
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
    upgrades = {
        slug(row["Unit"], "UNIT_"): slug(row["UpgradeUnit"], "UNIT_")
        for row in database.rows("UnitUpgrades")
    }
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
            # UnitUpgrades is a separate table; MandatoryObsoleteTech is the
            # column that closes a unit's production menu for good.
            "upgrade_to": upgrades.get(slug(row["UnitType"], "UNIT_")),
            "obsolete_tech": (
                slug(row["MandatoryObsoleteTech"], "TECH_")
                if row.get("MandatoryObsoleteTech")
                else None
            ),
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


YIELDS = {
    "YIELD_FOOD": "food",
    "YIELD_PRODUCTION": "production",
    "YIELD_GOLD": "gold",
    "YIELD_SCIENCE": "science",
    "YIELD_CULTURE": "culture",
    "YIELD_FAITH": "faith",
}

# CIVVIS names a handful of terrain-layer entries differently from the game's
# type constants. Naming, not rules.
TERRAIN_ALIASES = {"grass": "grassland"}
DISTRICT_ALIASES = {
    "theater": "theater_square",
    "government": "government_plaza",
    "water_entertainment_complex": "water_park",
}
FEATURE_ALIASES = {
    "floodplains_grassland": "grassland_floodplains",
    "floodplains_plains": "plains_floodplains",
    "barrier_reef": "great_barrier_reef",
    "everest": "mount_everest",
    "forest": "forest",
}


def yield_map(database: Database, table: str, key_column: str, prefix: str, aliases=None) -> dict[str, dict]:
    """``*_YieldChanges`` rows folded into {entry: {yield: amount}}."""
    folded: dict[str, dict] = {}
    for row in database.rows(table):
        yield_type = YIELDS.get(row.get("YieldType"))
        if yield_type is None:
            continue
        name = slug(row[key_column], prefix)
        if aliases:
            name = aliases.get(name, name)
        amount = number(row.get("YieldChange"))
        if amount:
            folded.setdefault(name, {})[yield_type] = amount
    return folded


def terrain_base(game_terrain: str) -> tuple[str, str]:
    """Split TERRAIN_GRASS_HILLS into (grassland, hills)."""
    name = slug(game_terrain, "TERRAIN_")
    for form in ("hills", "mountain"):
        if name.endswith("_" + form):
            base = name[: -len(form) - 1]
            return TERRAIN_ALIASES.get(base, base), form
    return TERRAIN_ALIASES.get(name, name), "flat"


def project_terrains(database: Database) -> dict[str, dict]:
    yields = yield_map(database, "Terrain_YieldChanges", "TerrainType", "TERRAIN_")
    projected: dict[str, dict] = {}
    hills: dict[str, dict] = {}
    for row in database.rows("Terrains"):
        base, form = terrain_base(row["TerrainType"])
        entry = {
            "yields": yields.get(slug(row["TerrainType"], "TERRAIN_"), {}),
            "water": truthy(row.get("Water")),
            "passable": not truthy(row.get("Impassable")),
            "move_cost": number(row.get("MovementCost"), 1),
            "defense": number(row.get("DefenseModifier")),
        }
        if form == "flat":
            projected[base] = entry
        elif form == "hills":
            hills[base] = entry
        elif form == "mountain":
            # CIVVIS models one impassable mountain terrain; the game ships a
            # variant per base terrain. All of them must agree for the single
            # entry to be faithful.
            merged = projected.setdefault(
                "mountain", {"yields": {}, "water": False, "passable": True, "defense": 0}
            )
            merged["passable"] = merged["passable"] and entry["passable"]
            merged["yields"] = merged["yields"] or entry["yields"]
    # CIVVIS stores hills as a tile flag with one engine-wide rule set. The
    # synthetic ``hills`` entry checks that rule against every game variant:
    # each must add the same yields on top of its base terrain.
    if hills:
        deltas = set()
        for base, entry in hills.items():
            flat = projected.get(base, {"yields": {}})
            delta = {
                yield_type: amount - flat["yields"].get(yield_type, 0)
                for yield_type, amount in entry["yields"].items()
                if amount != flat["yields"].get(yield_type, 0)
            }
            deltas.add(
                (
                    tuple(sorted(delta.items())),
                    entry["move_cost"],
                    entry["defense"],
                )
            )
        if len(deltas) == 1:
            ((delta, move_cost, defense),) = deltas
            projected["hills"] = {
                "yield_delta": dict(delta),
                "move_cost": move_cost,
                "defense": defense,
            }
    # Lakes are coast-terrain plots flagged as lakes; CIVVIS spells the flag
    # as its own terrain. Same rules row either way.
    if "coast" in projected:
        projected["lake"] = dict(projected["coast"])
    return projected


def project_features(database: Database) -> dict[str, dict]:
    yields = yield_map(
        database, "Feature_YieldChanges", "FeatureType", "FEATURE_", FEATURE_ALIASES
    )
    adjacent = yield_map(
        database, "Feature_AdjacentYields", "FeatureType", "FEATURE_", FEATURE_ALIASES
    )
    chops: dict[str, dict] = {}
    for row in database.rows("Feature_Removes"):
        yield_type = slug(row["YieldType"], "YIELD_")
        chops.setdefault(slug(row["FeatureType"], "FEATURE_"), {})[yield_type] = number(
            row.get("Yield")
        )
    projected = {}
    for row in database.rows("Features"):
        name = slug(row["FeatureType"], "FEATURE_")
        name = FEATURE_ALIASES.get(name, name)
        entry = {
            "yields": yields.get(name, {}),
            "move_cost": number(row.get("MovementChange")),
            "impassable": truthy(row.get("Impassable")),
            "natural_wonder": truthy(row.get("NaturalWonder")),
            "defense": number(row.get("DefenseModifier")),
            "chop": chops.get(name, {}),
        }
        if adjacent.get(name):
            entry["adjacent_yields"] = adjacent[name]
        projected[name] = entry
    return projected


RESOURCE_CLASSES = {
    "RESOURCECLASS_BONUS": "bonus",
    "RESOURCECLASS_LUXURY": "luxury",
    "RESOURCECLASS_STRATEGIC": "strategic",
    "RESOURCECLASS_ARTIFACT": "artifact",
}


LAND_BASES = {"desert", "grassland", "plains", "snow", "tundra"}


def collapse_terrains(rows) -> tuple[set, bool, bool, bool]:
    """Valid-terrain rows folded to CIVVIS base names.

    Returns (bases, any_flat_land, any_hills, any_mountain): the game encodes
    "hills only" and "flat only" by which variant rows exist, CIVVIS by
    boolean flags next to a base-terrain list.
    """
    bases, flat_land, hills, mountain = set(), False, False, False
    for game_terrain in rows:
        base, form = terrain_base(game_terrain)
        if form == "mountain":
            # CIVVIS spells every mountain variant as the one impassable
            # ``mountain`` terrain (Mountain Tunnels, Ski Resorts).
            bases.add("mountain")
            mountain = True
            continue
        bases.add(base)
        if form == "hills":
            hills = True
        elif base not in ("coast", "ocean", "lake"):
            flat_land = True
    return bases, flat_land, hills, mountain


def project_resources(database: Database) -> dict[str, dict]:
    yields = yield_map(database, "Resource_YieldChanges", "ResourceType", "RESOURCE_")
    terrains: dict[str, list] = {}
    for row in database.rows("Resource_ValidTerrains"):
        terrains.setdefault(slug(row["ResourceType"], "RESOURCE_"), []).append(
            row["TerrainType"]
        )
    features: dict[str, set] = {}
    for row in database.rows("Resource_ValidFeatures"):
        name = slug(row["FeatureType"], "FEATURE_")
        features.setdefault(slug(row["ResourceType"], "RESOURCE_"), set()).add(
            FEATURE_ALIASES.get(name, name)
        )
    # A resource's improvement, preferring the land improvement when a sea
    # counterpart also accepts it (Oil: Oil Wells on land, Oil Rigs at sea —
    # CIVVIS keys the resource on the land build and lets the improvement's
    # own resource list cover the water case).
    sea_improvements = {
        slug(row["ImprovementType"], "IMPROVEMENT_")
        for row in database.rows("Improvements")
        if truthy(row.get("Coast")) or row.get("Domain") == "DOMAIN_SEA"
    }
    improvements: dict[str, str] = {}
    for row in database.rows("Improvement_ValidResources"):
        name = slug(row["ResourceType"], "RESOURCE_")
        improvement = slug(row["ImprovementType"], "IMPROVEMENT_")
        current = improvements.get(name)
        if current is None or (current in sea_improvements and improvement not in sea_improvements):
            improvements[name] = improvement
    harvests: dict[str, dict] = {}
    for row in database.rows("Resource_Harvests"):
        entry = {
            "yield": slug(row["YieldType"], "YIELD_"),
            "amount": number(row.get("Amount")),
        }
        if row.get("PrereqTech"):
            entry["tech"] = slug(row["PrereqTech"], "TECH_")
        harvests[slug(row["ResourceType"], "RESOURCE_")] = entry
    civvis_features = set(load_ours("features"))
    projected = {}
    for row in database.rows("Resources"):
        name = slug(row["ResourceType"], "RESOURCE_")
        bases, flat_land, hills, _ = collapse_terrains(terrains.get(name, []))
        entry = {
            "class": RESOURCE_CLASSES.get(row.get("ResourceClassType"), "?"),
            "yields": yields.get(name, {}),
            "terrain": bases,
            # Some(true): hills-only spawns (Sheep); Some(false): flat-only
            # (grains); None: either form, or a sea/feature-placed resource.
            "hills": True if hills and not flat_land else (False if flat_land and not hills else None),
            # Placement on features CIVVIS does not model at all (volcanic
            # soil) is tracked by the Features table's missing-entry count,
            # not as noise on every resource.
            "feature": features.get(name, set()) & civvis_features,
        }
        if entry["class"] == "artifact":
            entry["hills"] = None  # dig sites spawn where history happened
        entry["harvest"] = harvests.get(name)
        if row.get("PrereqTech"):
            entry["tech"] = slug(row["PrereqTech"], "TECH_")
        if row.get("PrereqCivic"):
            entry["civic"] = slug(row["PrereqCivic"], "CIVIC_")
        if name in improvements:
            entry["improvement"] = improvements[name]
        projected[name] = entry
    return projected


def project_improvements(database: Database) -> dict[str, dict]:
    yields = yield_map(database, "Improvement_YieldChanges", "ImprovementType", "IMPROVEMENT_")
    terrains: dict[str, list] = {}
    gated: dict[str, set] = {}
    for row in database.rows("Improvement_ValidTerrains"):
        name = slug(row["ImprovementType"], "IMPROVEMENT_")
        if row.get("PrereqCivic") or row.get("PrereqTech"):
            # Conditional rows (farms on Hills with Civil Engineering) are a
            # separate rule; the base terrain set is what is always legal.
            gated.setdefault(name, set()).add(terrain_base(row["TerrainType"])[0])
            continue
        terrains.setdefault(name, []).append(row["TerrainType"])
    features: dict[str, set] = {}
    for row in database.rows("Improvement_ValidFeatures"):
        feature = slug(row["FeatureType"], "FEATURE_")
        features.setdefault(slug(row["ImprovementType"], "IMPROVEMENT_"), set()).add(
            FEATURE_ALIASES.get(feature, feature)
        )
    resources: dict[str, set] = {}
    for row in database.rows("Improvement_ValidResources"):
        resources.setdefault(slug(row["ImprovementType"], "IMPROVEMENT_"), set()).add(
            slug(row["ResourceType"], "RESOURCE_")
        )
    builder_built: set[str] = set()
    for row in database.rows("Improvement_ValidBuildUnits"):
        if row.get("UnitType") == "UNIT_BUILDER":
            builder_built.add(slug(row["ImprovementType"], "IMPROVEMENT_"))
    civvis_features = set(load_ours("features"))
    projected = {}
    for row in database.rows("Improvements"):
        name = slug(row["ImprovementType"], "IMPROVEMENT_")
        bases, flat_land, hills, mountain = collapse_terrains(terrains.get(name, []))
        entry = {
            "yields": yields.get(name, {}),
            # The game column counts half-Housing steps: Farms carry 1 for
            # their +0.5, Seasteads 4 for their +2.
            "housing": number(row.get("Housing")),
            "terrain": bases,
            "feature": features.get(name, set()) & civvis_features,
            "resources": resources.get(name, set()),
            "builder_buildable": name in builder_built,
            "requires_flat": flat_land and not hills and not mountain,
            "requires_hills": hills and not flat_land and not resources.get(name),
            "hills_or_resource": hills and not flat_land and bool(resources.get(name)),
        }
        if row.get("PrereqTech"):
            entry["tech"] = slug(row["PrereqTech"], "TECH_")
        if row.get("PrereqCivic"):
            entry["civic"] = slug(row["PrereqCivic"], "CIVIC_")
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


BUILDING_ALIASES = {
    "museum_art": "art_museum",
    "museum_artifact": "archaeological_museum",
    # Gathering Storm added Coal and Oil ("fossil fuel") plants and reused
    # the vanilla type for the Nuclear Power Plant.
    "fossil_fuel_power_plant": "oil_power_plant",
    "power_plant": "nuclear_power_plant",
}


# Eureka/Inspiration triggers: the game's BoostClass rows spelled in the
# trigger vocabulary CIVVIS' engine evaluates (game.rs boost_met).
def boost_spec(row) -> dict | None:
    def named(game_type, prefix, aliases=None):
        entry = slug(game_type, prefix)
        return (aliases or {}).get(entry, entry)

    cls = (row.get("BoostClass") or "").replace("BOOST_TRIGGER_", "")
    n = number(row.get("NumItems"), 1)
    simple = {
        "DISCOVER_CONTINENT": "discover_continent",
        "CLEAR_CAMP": "camps",
        "CREATE_PANTHEON": "pantheon",
        "RECEIVE_DOW": "received_dow",
        "FOUND_RELIGION": "religion",
        "HAVE_AN_ALLIANCE": "alliances",
        "DOW_CASUS_BELLI": "casus_belli",
        "AIRBASE_FOREIGN_CONTINENT": "airbase_foreign_continent",
        "SETTLE_COAST": "coastal_city",
        "FIND_NATURAL_WONDER": "natural_wonder",
        "MEET_CIV": "met_civ",
        "CREATED_NATIONAL_PARK": "national_park",
        "ARTIFACT_EXTRACTED": "artifacts",
    }
    counted = {
        "HAVE_X_UNIQUE_SPECIALTY_DISTRICTS": "specialty_districts",
        "CITY_POPULATION": "pop",
        "EMPIRE_POPULATION": "total_pop",
        "MAINTAIN_X_TRADE_ROUTES": "trade_routes",
        "HAVE_WONDER_PAST_X_ERA": "wonder_era",
        "NUM_IMPROVED_TILES": "improvements",
        "MEET_X_CITY_STATES": "met_city_states",
        "HAVE_X_WONDERS": "wonders",
        "HAVE_X_LAND_UNITS": "land_units",
        "HAVE_ALLIANCE_LEVEL_X": "alliance_level",
        "HAVE_X_CITIES_FOLLOWING_YOUR_RELIGION": "religion_cities",
        "HAVE_X_GREAT_PEOPLE": "great_people",
        "HAVE_X_CORPS": "corps",
        "HAVE_X_ARMIES": "armies",
        "HAVE_X_THEMED_BUILDINGS": "themed_buildings",
        "NUM_BARBS_KILLED": "barbs_killed",
    }
    if cls == "NONE_LATE_GAME_CRITICAL_TECH":
        return None  # boostable only through espionage, never a trigger
    if cls in simple:
        return {"trigger": simple[cls]}
    if cls in counted:
        return {"trigger": counted[cls], "count": n}
    if cls == "HAVE_X_BUILDINGS":
        return {
            "trigger": f"building:{named(row['BuildingType'], 'BUILDING_', BUILDING_ALIASES)}",
            "count": n,
        }
    if cls == "CONSTRUCT_BUILDING":
        return {"trigger": f"building:{named(row['BuildingType'], 'BUILDING_', BUILDING_ALIASES)}"}
    if cls == "OWN_X_UNITS_OF_TYPE":
        return {"trigger": f"units_of:{named(row['Unit1Type'], 'UNIT_')}", "count": n}
    if cls == "HAVE_X_IMPROVEMENTS":
        key = "improvement_on_resource" if truthy(row.get("RequiresResource")) else "improvement"
        return {"trigger": f"{key}:{named(row['ImprovementType'], 'IMPROVEMENT_')}", "count": n}
    if cls == "HAVE_X_DISTRICTS":
        return {
            "trigger": f"district:{named(row['DistrictType'], 'DISTRICT_', DISTRICT_ALIASES)}",
            "count": n,
        }
    if cls == "RESEARCH_TECH":
        return {"trigger": f"tech:{named(row['BoostingTechType'], 'TECH_')}"}
    if cls == "CULTURVATE_CIVIC":
        return {"trigger": f"civic:{named(row['BoostingCivicType'], 'CIVIC_')}"}
    if cls == "KILL_WITH":
        return {"trigger": f"kill_with:{named(row['Unit1Type'], 'UNIT_')}"}
    if cls == "KILL_SPECIFIC_UNIT":
        return {"trigger": f"kill_kind:{named(row['Unit1Type'], 'UNIT_')}"}
    if cls == "IMPROVE_SPECIFIC_RESOURCE":
        return {"trigger": f"improve_resource:{named(row['ResourceType'], 'RESOURCE_')}"}
    if cls == "TRAIN_UNIT":
        unit = slug(row["Unit1Type"], "UNIT_")
        if unit.startswith("great_"):
            return {"trigger": f"great_person_of:{unit[len('great_'):]}"}
        return {"trigger": f"trained:{unit}"}
    if cls == "HAVE_GOVERNMENT_TIER":
        # NumItems is total policy slots (Tier 2 = 6, Tier 3 = 8); the
        # tierless Near Future Governance row means Tier 4 = 10.
        return {"trigger": "government_slots", "count": n if row.get("NumItems") else 10}
    if cls == "DISTRICT_APPEAL_LEVEL_MINIMUM_X":
        return {
            "trigger": f"district_appeal:{named(row['DistrictType'], 'DISTRICT_', DISTRICT_ALIASES)}",
            "count": n,
        }
    if cls == "HAVE_BUILDING_MOUNTAIN":
        return {
            "trigger": "building_near_mountain:"
            + named(row["BuildingType"], "BUILDING_", BUILDING_ALIASES)
        }
    if cls == "HAVE_UNIT_AND_IMPROVEMENT":
        return {
            "trigger": f"unit_and_improve:{named(row['Unit1Type'], 'UNIT_')}"
            f":{named(row['ResourceType'], 'RESOURCE_')}"
        }
    return {"trigger": f"?{cls.lower()}"}


def project_boosts(database: Database) -> dict[str, dict]:
    projected = {}
    for row in database.rows("Boosts"):
        node = slug(
            row.get("TechnologyType") or row.get("CivicType"),
            "TECH_" if row.get("TechnologyType") else "CIVIC_",
        )
        spec = boost_spec(row)
        if spec is None:
            continue
        entry = {"trigger": spec["trigger"], "count": spec.get("count", 1)}
        percent = number(row.get("Boost"), 40)
        if percent != 40:
            entry["percent"] = percent
        projected[node] = entry
    return projected


def ours_boosts() -> dict[str, dict]:
    out = {}
    for name in ("techs", "civics"):
        for node, entry in load_ours(name).items():
            boost = entry.get("boost")
            if boost:
                row = {"trigger": boost.get("trigger", "?"), "count": boost.get("count", 1)}
                if boost.get("percent"):
                    row["percent"] = boost["percent"]
                out[node] = row
    return out


def project_goody_huts(database: Database) -> dict[str, dict]:
    """Tribal village rewards: weights, turn gates, city gates and amounts.

    The reward amounts live in each subtype's modifier arguments; ``Amount``
    (or the governor title's ``Delta``) is the number CIVVIS stores in its
    reward map. Weight-0 rows are rewards the shipped game has disabled.
    """
    arguments: dict[tuple, str] = {}
    for row in database.rows("ModifierArguments"):
        arguments[(row.get("ModifierId"), row.get("Name"))] = row.get("Value")
    projected = {}
    for row in database.rows("GoodyHutSubTypes"):
        weight = number(row.get("Weight"))
        if weight <= 0:
            continue
        category = slug(row.get("GoodyHut", ""), "GOODYHUT_")
        subtype = slug(row["SubTypeGoodyHut"], "GOODYHUT_")
        modifier = row.get("ModifierID")
        amount = number(
            arguments.get((modifier, "Amount")) or arguments.get((modifier, "Delta")), 1
        )
        projected[f"{category}/{subtype}"] = {
            "weight": weight,
            "min_turn": number(row.get("Turn")),
            "requires_city": truthy(row.get("MinOneCity")),
            "amount": amount,
        }
    return projected


def ours_goody_huts() -> dict[str, dict]:
    out = {}
    for category, rewards in load_ours("goody_huts").items():
        for subtype, spec in rewards.items():
            out[f"{category}/{subtype}"] = {
                "weight": spec.get("weight", 0),
                "min_turn": spec.get("min_turn", 0),
                "requires_city": spec.get("requires_city", False),
                "amount": max(spec.get("reward", {}).values(), default=0),
            }
    return out


# Per-object-type great work values (game.rs city yields and
# great_work_tourism) against the modal shipped GreatWorks rows. CIVVIS
# counts works per kind, which is exact while each type's works share
# values - the audit's job is to notice if a patch ever splits them.
ENGINE_GREAT_WORKS = {
    "writing": {"culture": 2, "tourism": 2},
    "sculpture": {"culture": 3, "tourism": 2},
    "portrait": {"culture": 3, "tourism": 2},
    "landscape": {"culture": 3, "tourism": 2},
    "religious": {"culture": 3, "tourism": 2},
    "artifact": {"culture": 3, "tourism": 3},
    "music": {"culture": 4, "tourism": 4},
    "relic": {"faith": 4, "tourism": 8},
}


def project_great_works(database: Database) -> dict[str, dict]:
    """Modal per-type values across all shipped works of that type."""
    from collections import Counter, defaultdict

    kinds = {
        row["GreatWorkType"]: slug(row.get("GreatWorkObjectType", ""), "GREATWORKOBJECT_")
        for row in database.rows("GreatWorks")
    }
    yields: dict[str, Counter] = defaultdict(Counter)
    tourisms: dict[str, Counter] = defaultdict(Counter)
    for row in database.rows("GreatWork_YieldChanges"):
        kind = kinds.get(row["GreatWorkType"])
        if kind:
            yields[kind][(slug(row["YieldType"], "YIELD_"), number(row.get("YieldChange")))] += 1
    for work, kind in kinds.items():
        tourisms[kind][
            number(
                next(
                    (r.get("Tourism") for r in database.rows("GreatWorks") if r["GreatWorkType"] == work),
                    0,
                )
            )
        ] += 1
    projected = {}
    for kind in yields:
        (yield_type, amount), _ = yields[kind].most_common(1)[0]
        tourism, _ = tourisms[kind].most_common(1)[0]
        projected[kind] = {yield_type: amount, "tourism": tourism}
    return projected


# Global parameters the engine implements, next to the value its code uses.
# Each mirrors a specific site in src/ — change one side, change the other.
ENGINE_PARAMETERS = {
    "CITY_FOOD_CONSUMPTION_PER_POPULATION": 2,  # game.rs process_city
    "CITY_GROWTH_THRESHOLD": 15,  # game.rs growth_threshold
    "CITY_GROWTH_MULTIPLIER": 8,
    "CITY_GROWTH_EXPONENT": 1.5,
    "CITY_HOUSING_LEFT_25PCT_GROWTH": 0,  # game.rs housing_growth_mult
    "CITY_HOUSING_LEFT_50PCT_GROWTH": 1,
    "CITY_HOUSING_LEFT_ZERO_GROWTH": -4,
    "CITY_POPULATION_RIVER_LAKE": 5,  # fresh-water Housing
    "CITY_POPULATION_COAST": 3,
    "CITY_POPULATION_NO_WATER": 2,
    "CITY_POP_PER_AMENITY": 2,  # game.rs city_amenities_required
    "CITY_AMENITIES_FOR_FREE": 0,  # ceil(pop/2), no free Amenity in GS
    "CITY_MIN_RANGE": 3,  # game.rs can_found_city (wdist < 4)
    "COMBAT_BASE_DAMAGE": 24,  # game.rs damage: 30 * U(0.8,1.2) == 24 * U(1.0,1.5)
    "COMBAT_CORPS_STRENGTH_MODIFIER": 10,  # game.rs unit_formation_bonus
    "COMBAT_ARMY_STRENGTH_MODIFIER": 17,
    "COMBAT_AMPHIBIOUS_ATTACK_PENALTY": -10,  # attacks while embarked
    "COMBAT_RIVER_DEFENSE": 5,  # melee and encampment river crossings
    "EXPERIENCE_MAX_BARB_LEVEL": 2,  # award_unit_combat_xp
    "EXPERIENCE_BARB_SOFT_CAP": 1,
    "EXPERIENCE_MAXIMUM_ONE_COMBAT": 8,
    "CULTURE_COST_FIRST_PLOT": 10,  # border growth: 10 + 6 * plots^1.3
    "CULTURE_COST_LATER_PLOT_MULTIPLIER": 6,
    "CULTURE_COST_LATER_PLOT_EXPONENT": 1.3,
    "BARBARIAN_CAMP_MINIMUM_DISTANCE_CITY": 4,  # game.rs spawn_camp
    "BARBARIAN_CAMP_MINIMUM_DISTANCE_ANOTHER_CAMP": 7,
    "BARBARIAN_TECH_PERCENT": 50,  # game.rs barbarian_phase unit pool
    "BARBARIAN_NUM_RANDOM_UNIT_CHOICES": 3,
}


def project_parameters(database: Database) -> dict[str, dict]:
    rows = {row["Name"]: row.get("Value") for row in database.rows("GlobalParameters")}
    return {
        name: {"value": number(rows[name])}
        for name in ENGINE_PARAMETERS
        if name in rows
    }


def ours_parameters() -> dict[str, dict]:
    return {name: {"value": value} for name, value in ENGINE_PARAMETERS.items()}


# The engine's six stock map profiles (src/setup.rs) next to the shipped
# Maps rows. City-state and religion counts live in front-end config, not
# the gameplay database, so only these five columns are comparable.
ENGINE_MAP_SIZES = {
    "duel": {"width": 44, "height": 26, "players": 2, "natural_wonders": 2, "continents": 1},
    "tiny": {"width": 60, "height": 38, "players": 4, "natural_wonders": 3, "continents": 2},
    "small": {"width": 74, "height": 46, "players": 6, "natural_wonders": 4, "continents": 3},
    "standard": {"width": 84, "height": 54, "players": 8, "natural_wonders": 5, "continents": 4},
    "large": {"width": 96, "height": 60, "players": 10, "natural_wonders": 6, "continents": 5},
    "huge": {"width": 106, "height": 66, "players": 12, "natural_wonders": 7, "continents": 6},
}

# The engine's amenity bands (game.rs amenity_yield_mult_for /
# amenity_growth_mult), spelled the way the Happinesses table spells them.
ENGINE_HAPPINESS = {
    "ecstatic": {"growth": 20, "yields": 20},
    "happy": {"growth": 10, "yields": 10},
    "content": {"growth": 0, "yields": 0},
    "displeased": {"growth": -15, "yields": -10},
    "unhappy": {"growth": -30, "yields": -20},
    "unrest": {"growth": -100, "yields": -30},
    "revolt": {"growth": -100, "yields": -40},
}


def project_maps(database: Database) -> dict[str, dict]:
    projected = {}
    for row in database.rows("Maps"):
        projected[slug(row["MapSizeType"], "MAPSIZE_")] = {
            "width": number(row.get("GridWidth")),
            "height": number(row.get("GridHeight")),
            "players": number(row.get("DefaultPlayers")),
            "natural_wonders": number(row.get("NumNaturalWonders")),
            "continents": number(row.get("Continents")),
        }
    return projected


def project_happiness(database: Database) -> dict[str, dict]:
    projected = {}
    for row in database.rows("Happinesses"):
        projected[slug(row["HappinessType"], "HAPPINESS_")] = {
            "growth": number(row.get("GrowthModifier")),
            "yields": number(row.get("NonFoodYieldModifier")),
        }
    return projected


def project_wmds(database: Database) -> dict[str, dict]:
    projected = {}
    for row in database.rows("WMDs"):
        projected[slug(row["WeaponType"], "WMD_")] = {
            "blast_radius": number(row.get("BlastRadius")),
            "fallout_duration": number(row.get("FalloutDuration")),
            "icbm_strike_range": number(row.get("ICBMStrikeRange")),
            "maintenance": number(row.get("Maintenance")),
        }
    return projected


def ours_wmds() -> dict[str, dict]:
    return {
        name: {
            "blast_radius": entry.get("blast_radius", 0),
            "fallout_duration": entry.get("fallout_duration", 0),
            "icbm_strike_range": entry.get("icbm_strike_range", 0),
            "maintenance": entry.get("maintenance", 0),
        }
        for name, entry in load_ours("wmds").items()
    }


def project_eras(database: Database) -> dict[str, dict]:
    projected = {}
    for row in database.rows("Eras"):
        projected[slug(row["EraType"], "ERA_")] = {
            "great_person_base_cost": number(row.get("GreatPersonBaseCost")),
            "embarked_strength": number(row.get("EmbarkedUnitStrength")),
            "warmonger_points": number(row.get("WarmongerPoints")),
        }
    return projected


def ours_eras() -> dict[str, dict]:
    return {
        name: {
            "great_person_base_cost": entry.get("great_person_base_cost", 0),
            "embarked_strength": entry.get("embarked_strength", 0),
            "warmonger_points": entry.get("warmonger_points", 0),
        }
        for name, entry in load_ours("eras").items()
        # The future era has no shipped row; CIVVIS carries the Information
        # values forward for its extended tree.
        if name != "future"
    }


def project_great_people(database: Database) -> dict[str, dict]:
    era_costs = {
        slug(row["EraType"], "ERA_"): number(row.get("GreatPersonBaseCost"))
        for row in database.rows("Eras")
    }
    projected = {}
    for row in database.rows("GreatPersonIndividuals"):
        era = slug(row.get("EraType", ""), "ERA_")
        projected[slug(row["GreatPersonIndividualType"], "GREAT_PERSON_INDIVIDUAL_")] = {
            "kind": slug(row.get("GreatPersonClassType", ""), "GREAT_PERSON_CLASS_"),
            "era": ERAS.index("ERA_" + era.upper()) if "ERA_" + era.upper() in ERAS else -1,
            "cost": era_costs.get(era, 0),
            # Writers, artists, musicians and prophets carry ActionCharges 0:
            # their one activation creates works rather than a map action.
            # CIVVIS spells every activation as a charge.
            "charges": max(number(row.get("ActionCharges"), 1), 1),
        }
    return projected


def ours_great_people() -> dict[str, dict]:
    return {
        name: {
            "kind": entry.get("kind", "?"),
            "era": entry.get("era", -1),
            "cost": entry.get("cost", 0),
            "charges": entry.get("charges", 1),
        }
        for name, entry in load_ours("great_people").items()
    }


# CIVVIS names the adjacency source; the game names the column that matched.
ADJACENCY_FEATURES = {
    "jungle": "rainforest",
    "barrier_reef": "great_barrier_reef",
    "everest": "mount_everest",
}
ADJACENCY_DISTRICTS = {
    "government": "government_plaza",
    "theater": "theater_square",
    "water_entertainment_complex": "water_park",
    "water_street_carnival": "street_carnival",
}


def adjacency_source(rule: dict) -> str | None:
    """The CIVVIS adjacency key a game adjacency rule corresponds to."""
    if truthy(rule.get("Self")):
        return "self"
    if truthy(rule.get("OtherDistrictAdjacent")):
        return "district"
    if truthy(rule.get("AdjacentRiver")):
        return "river"
    if truthy(rule.get("AdjacentWonder")):
        return "wonder"
    if truthy(rule.get("AdjacentNaturalWonder")):
        return "natural_wonder"
    if truthy(rule.get("AdjacentSeaResource")):
        return "coast_resource"
    if truthy(rule.get("AdjacentResource")):
        return "resource"
    if terrain := rule.get("AdjacentTerrain"):
        # The game spells one rule per terrain family; CIVVIS keys the family.
        return "mountain" if terrain.endswith("_MOUNTAIN") else slug(terrain, "TERRAIN_")
    if feature := rule.get("AdjacentFeature"):
        name = slug(feature, "FEATURE_")
        return ADJACENCY_FEATURES.get(name, name)
    if district := rule.get("AdjacentDistrict"):
        name = slug(district, "DISTRICT_")
        return ADJACENCY_DISTRICTS.get(name, name)
    if improvement := rule.get("AdjacentImprovement"):
        return slug(improvement, "IMPROVEMENT_")
    if klass := rule.get("AdjacentResourceClass"):
        return f"{slug(klass, 'RESOURCECLASS_')}_resource"
    return None


def project_adjacency(database: Database) -> dict[str, dict]:
    rules = {row["ID"]: row for row in database.rows("Adjacency_YieldChanges")}
    projected: dict[str, dict] = {
        slug(row["DistrictType"], "DISTRICT_"): {} for row in database.rows("Districts")
    }
    for row in database.rows("District_Adjacencies"):
        rule = rules.get(row["YieldChangeId"])
        if rule is None:
            continue
        source = adjacency_source(rule)
        if source is None:
            continue
        district = slug(row["DistrictType"], "DISTRICT_")
        amount = number(rule.get("YieldChange")) / max(number(rule.get("TilesRequired"), 1), 1)
        field = f"{source}_{slug(rule['YieldType'], 'YIELD_')}"
        entry = projected.setdefault(district, {})
        # Several game rules can collapse onto one CIVVIS key (five mountain
        # terrains, two Mine rules). Same-value rules are one rule restated per
        # terrain; genuinely different values sum the way adjacency does.
        entry[field] = amount if entry.get(field) in (None, amount) else entry[field] + amount
    return projected


def ours_adjacency() -> dict[str, dict]:
    return {
        name: {
            f"{source}_{field}": amount
            for source, yields in (entry.get("adjacency") or {}).items()
            for field, amount in yields.items()
        }
        for name, entry in load_ours("districts").items()
    }


GREAT_WORK_SLOTS = {
    "GREATWORKSLOT_WRITING": "writing",
    "GREATWORKSLOT_ART": "art",
    "GREATWORKSLOT_MUSIC": "music",
    "GREATWORKSLOT_RELIC": "relic",
    "GREATWORKSLOT_ARTIFACT": "artifact",
    "GREATWORKSLOT_PALACE": "any",
    "GREATWORKSLOT_CATHEDRAL": "religious_art",
}


def building_extras(database: Database):
    yields = yield_map(database, "Building_YieldChanges", "BuildingType", "BUILDING_")
    gpp: dict[str, dict] = {}
    for row in database.rows("Building_GreatPersonPoints"):
        gpp.setdefault(slug(row["BuildingType"], "BUILDING_"), {})[
            slug(row["GreatPersonClassType"], "GREAT_PERSON_CLASS_")
        ] = number(row.get("PointsPerTurn"))
    works: dict[str, dict] = {}
    for row in database.rows("Building_GreatWorks"):
        slot = GREAT_WORK_SLOTS.get(row.get("GreatWorkSlotType"))
        if slot:
            entry = works.setdefault(slug(row["BuildingType"], "BUILDING_"), {})
            entry[slot] = entry.get(slot, 0) + number(row.get("NumSlots"), 1)
    return yields, gpp, works


def project_buildings(database: Database) -> dict[str, dict]:
    yields, gpp, works = building_extras(database)
    projected = {}
    for row in database.rows("Buildings"):
        if truthy(row.get("IsWonder")):
            continue  # audited as Wonders, CIVVIS' spelling
        name = slug(row["BuildingType"], "BUILDING_")
        entry = {
            "cost": number(row.get("Cost")),
            "maintenance": number(row.get("Maintenance")),
            "housing": number(row.get("Housing")),
            "amenity": number(row.get("Entertainment")),
            "citizen_slots": number(row.get("CitizenSlots")),
            "yields": yields.get(name, {}),
            "regional_range": number(row.get("RegionalRange")),
        }
        if row.get("PrereqTech"):
            entry["tech"] = slug(row["PrereqTech"], "TECH_")
        if row.get("PrereqCivic"):
            entry["civic"] = slug(row["PrereqCivic"], "CIVIC_")
        if row.get("PrereqDistrict"):
            district = slug(row["PrereqDistrict"], "DISTRICT_")
            entry["district"] = DISTRICT_ALIASES.get(district, district)
        if gpp.get(name):
            entry["great_person_points"] = gpp[name]
        if works.get(name):
            entry["great_work_slots"] = works[name]
        projected[BUILDING_ALIASES.get(name, name)] = entry
    return projected


def project_wonders(database: Database) -> dict[str, dict]:
    yields, gpp, works = building_extras(database)
    terrains: dict[str, list] = {}
    for row in database.rows("Building_ValidTerrains"):
        terrains.setdefault(slug(row["BuildingType"], "BUILDING_"), []).append(
            row["TerrainType"]
        )
    # CIVVIS wonder feature lists are placement requirements — the game's
    # ``Building_RequiredFeatures`` (Chichen Itza on Rainforest). The softer
    # ``Building_ValidFeatures`` merely widens placement (Petra also on its
    # Floodplains) and stays unaudited until CIVVIS models the distinction.
    features: dict[str, set] = {}
    for row in database.rows("Building_RequiredFeatures"):
        feature = slug(row["FeatureType"], "FEATURE_")
        features.setdefault(slug(row["BuildingType"], "BUILDING_"), set()).add(
            FEATURE_ALIASES.get(feature, feature)
        )
    civvis_features = set(load_ours("features"))
    projected = {}
    for row in database.rows("Buildings"):
        if not truthy(row.get("IsWonder")):
            continue
        name = slug(row["BuildingType"], "BUILDING_")
        bases, _, hills, _ = collapse_terrains(terrains.get(name, []))
        if bases == LAND_BASES:
            bases = set()
        coast = truthy(row.get("Coast"))
        if bases and bases <= {"coast", "ocean"}:
            # Valid-terrain rows on water terrain are the game's spelling of
            # "stands on a coastal water tile"; CIVVIS spells that coast.
            bases = set()
            coast = True
        if truthy(row.get("MustBeLake")):
            # Huey Teocalli: the lake tile is the placement, the redundant
            # Coast column is not the rule.
            bases = {"lake"}
            coast = False
        entry = {
            "cost": number(row.get("Cost")),
            "yields": yields.get(name, {}),
            "housing": number(row.get("Housing")),
            "amenity": number(row.get("Entertainment")),
            "regional_range": number(row.get("RegionalRange")),
            "coast": coast,
            "river": truthy(row.get("RequiresRiver"))
            or truthy(row.get("RequiresAdjacentRiver")),
            "adjacent_mountain": truthy(row.get("AdjacentToMountain")),
            "terrain": bases,
            "feature": features.get(name, set()) & civvis_features,
        }
        if hills is False and bases and "mountain" not in bases:
            # Wonders whose valid-terrain rows are flat variants only must be
            # placed on flat ground; CIVVIS spells that hills: false.
            entry["flat_only"] = True
        if row.get("PrereqTech"):
            entry["tech"] = slug(row["PrereqTech"], "TECH_")
        if row.get("PrereqCivic"):
            entry["civic"] = slug(row["PrereqCivic"], "CIVIC_")
        if row.get("AdjacentDistrict"):
            district = slug(row["AdjacentDistrict"], "DISTRICT_")
            entry["adjacent_district"] = DISTRICT_ALIASES.get(district, district)
        if row.get("AdjacentResource"):
            entry["adjacent_resource"] = slug(row["AdjacentResource"], "RESOURCE_")
        if row.get("AdjacentImprovement"):
            entry["adjacent_improvement"] = slug(row["AdjacentImprovement"], "IMPROVEMENT_")
        if gpp.get(name):
            entry["great_person_points"] = gpp[name]
        if works.get(name):
            entry["great_work_slots"] = works[name]
        projected[name] = entry
    return projected


POLICY_SLOTS = {
    "SLOT_MILITARY": "military",
    "SLOT_ECONOMIC": "economic",
    "SLOT_DIPLOMATIC": "diplomatic",
    "SLOT_WILDCARD": "wildcard",
    "SLOT_GREAT_PERSON": "wildcard",
    "SLOT_DARKAGE": "dark_age",
}


def project_policies(database: Database) -> dict[str, dict]:
    projected = {}
    for row in database.rows("Policies"):
        entry = {"slot": POLICY_SLOTS.get(row.get("GovernmentSlotType"), "?")}
        if row.get("PrereqCivic"):
            entry["civic"] = slug(row["PrereqCivic"], "CIVIC_")
        projected[slug(row["PolicyType"], "POLICY_")] = entry
    return projected


def project_governments(database: Database) -> dict[str, dict]:
    slots: dict[str, dict] = {}
    for row in database.rows("Government_SlotCounts"):
        slot = POLICY_SLOTS.get(row.get("GovernmentSlotType"))
        if slot:
            entry = slots.setdefault(slug(row["GovernmentType"], "GOVERNMENT_"), {})
            entry[slot] = entry.get(slot, 0) + number(row.get("NumSlots"))
    projected = {}
    for row in database.rows("Governments"):
        name = slug(row["GovernmentType"], "GOVERNMENT_")
        entry = {
            "slots": slots.get(name, {}),
            "influence_per_turn": number(row.get("InfluencePointsPerTurn")),
            "influence_threshold": number(row.get("InfluencePointsThreshold")),
            "envoys_per_threshold": number(row.get("InfluenceTokensPerThreshold")),
        }
        if row.get("PrereqCivic"):
            entry["civic"] = slug(row["PrereqCivic"], "CIVIC_")
        projected[name] = entry
    return projected


def project_beliefs(database: Database) -> dict[str, dict]:
    projected = {}
    for row in database.rows("Beliefs"):
        projected[slug(row["BeliefType"], "BELIEF_")] = {
            "class": slug(row.get("BeliefClassType", ""), "BELIEF_CLASS_"),
        }
    return projected


PROMOTION_CLASSES = {
    "PROMOTION_CLASS_RECON": "recon",
    "PROMOTION_CLASS_MELEE": "melee",
    "PROMOTION_CLASS_RANGED": "ranged",
    "PROMOTION_CLASS_SIEGE": "siege",
    "PROMOTION_CLASS_LIGHT_CAVALRY": "light_cavalry",
    "PROMOTION_CLASS_HEAVY_CAVALRY": "heavy_cavalry",
    "PROMOTION_CLASS_ANTI_CAVALRY": "anti_cavalry",
    "PROMOTION_CLASS_NAVAL_MELEE": "naval_melee",
    "PROMOTION_CLASS_NAVAL_RANGED": "naval_ranged",
    "PROMOTION_CLASS_NAVAL_RAIDER": "naval_raider",
    "PROMOTION_CLASS_NAVAL_CARRIER": "naval_carrier",
    "PROMOTION_CLASS_AIR_FIGHTER": "air_fighter",
    "PROMOTION_CLASS_AIR_BOMBER": "air_bomber",
    "PROMOTION_CLASS_MONK": "warrior_monk",
    "PROMOTION_CLASS_APOSTLE": "religious_apostle",
    "PROMOTION_CLASS_ROCK_BAND": "rock_band",
    "PROMOTION_CLASS_GIANT_DEATH_ROBOT": "giant_death_robot",
    "PROMOTION_CLASS_SUPPORT": "support",
}


def project_promotions(database: Database) -> dict[str, dict]:
    prereqs: dict[str, set] = {}
    for row in database.rows("UnitPromotionPrereqs"):
        prereqs.setdefault(slug(row["UnitPromotion"], "PROMOTION_"), set()).add(
            slug(row["PrereqUnitPromotion"], "PROMOTION_")
        )
    projected = {}
    for row in database.rows("UnitPromotions"):
        name = slug(row["UnitPromotionType"], "PROMOTION_")
        projected[name] = {
            "class": PROMOTION_CLASSES.get(row.get("PromotionClass"), "?"),
            "tier": number(row.get("Level")),
            "requires": prereqs.get(name, set()),
        }
    return projected


def project_projects(database: Database) -> dict[str, dict]:
    gpp: dict[str, dict] = {}
    for row in database.rows("Project_GreatPersonPoints"):
        gpp.setdefault(slug(row["ProjectType"], "PROJECT_"), {})[
            slug(row["GreatPersonClassType"], "GREAT_PERSON_CLASS_")
        ] = number(row.get("Points"))
    conversions: dict[str, dict] = {}
    for row in database.rows("Project_YieldConversions"):
        yield_type = YIELDS.get(row.get("YieldType"))
        if yield_type:
            conversions.setdefault(slug(row["ProjectType"], "PROJECT_"), {})[
                yield_type
            ] = number(row.get("PercentOfProductionRate"))
    projected = {}
    for row in database.rows("Projects"):
        name = slug(row["ProjectType"], "PROJECT_")
        entry = {
            "cost": number(row.get("Cost")),
            "repeatable": number(row.get("MaxPlayerInstances"), 0) != 1,
        }
        if row.get("CostProgressionModel") == "COST_PROGRESSION_GAME_PROGRESS":
            entry["cost_progression_max_pct"] = number(row.get("CostProgressionParam1"))
        if row.get("PrereqDistrict"):
            entry["district"] = slug(row["PrereqDistrict"], "DISTRICT_")
        if row.get("PrereqTech"):
            entry["tech"] = slug(row["PrereqTech"], "TECH_")
        if gpp.get(name):
            entry["completion_gpp"] = gpp[name]
        if conversions.get(name):
            entry["ongoing_yields"] = conversions[name]
        projected[name] = entry
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
    "upgrade_to": None,
    "obsolete_tech": None,
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


# Rules CIVVIS hardcodes in the engine rather than in data. Each mirrors a
# specific site in src/: change one side, change the other.
ENGINE_HILLS = {"yield_delta": {"production": 1}, "move_cost": 2, "defense": 3}  # rules.rs tile_yields/move_cost, game.rs tile_defense_bonus
ENGINE_FEATURE_DEFENSE = {  # game.rs tile_defense_bonus
    "forest": 3,
    "jungle": 3,
    "reef": 3,
    "burning_forest": 3,
    "burning_jungle": 3,
    "burnt_forest": 3,
    "burnt_jungle": 3,
    "marsh": -2,
    "floodplains": -2,
    "grassland_floodplains": -2,
    "plains_floodplains": -2,
    "pantanal": -2,
}


def ours_terrains() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("terrains").items():
        out[name] = {
            "yields": {k: v for k, v in entry.get("yields", {}).items() if v},
            "water": entry.get("water", False),
            "passable": entry.get("passable", True),
            "move_cost": entry.get("move_cost", 1),
        }
    out["hills"] = dict(ENGINE_HILLS)
    return out


def ours_features() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("features").items():
        row = {
            "yields": {k: v for k, v in entry.get("yields", {}).items() if v},
            "move_cost": entry.get("move_cost", 0),
            "impassable": entry.get("impassable", False),
            "natural_wonder": entry.get("natural_wonder", False),
            "defense": ENGINE_FEATURE_DEFENSE.get(name, 0),
            "chop": entry.get("chop", {}),
        }
        if entry.get("adjacent_yields"):
            row["adjacent_yields"] = entry["adjacent_yields"]
        out[name] = row
    return out


def lakes_are_coast(terrains: set) -> set:
    """CIVVIS spells lakes as their own terrain; the game's lake plots are
    coast-terrain rows, so a valid-terrain list treats the two as one."""
    return {"coast" if terrain == "lake" else terrain for terrain in terrains}


def ours_resources() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("resources").items():
        row = {
            "class": entry.get("class", "bonus"),
            "yields": {k: v for k, v in entry.get("yields", {}).items() if v},
            "terrain": lakes_are_coast(set(entry.get("terrain", []))),
            "feature": set(entry.get("feature", [])),
            "hills": entry.get("hills"),
        }
        for key in ("tech", "civic", "improvement"):
            if entry.get(key):
                row[key] = entry[key]
        row["harvest"] = entry.get("harvest")
        out[name] = row
    return out


def ours_improvements() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("improvements").items():
        row = {
            "yields": {k: v for k, v in entry.get("yields", {}).items() if v},
            "housing": entry.get("housing", 0) * 2,
            "terrain": lakes_are_coast(set(entry.get("terrain", []))),
            "feature": set(entry.get("feature", [])),
            "resources": set(entry.get("resources", [])),
            "builder_buildable": entry.get("builder_buildable", True)
            and not entry.get("unbuildable", False),
            "requires_flat": entry.get("requires_flat", False),
            "requires_hills": entry.get("requires_hills", False),
            "hills_or_resource": entry.get("hills_or_resource", False),
        }
        for key in ("tech", "civic"):
            if entry.get(key):
                row[key] = entry[key]
        out[name] = row
    return out


def ours_buildings() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("buildings").items():
        row = {
            "cost": entry.get("cost", 0),
            "maintenance": entry.get("maintenance", 0),
            "housing": entry.get("housing", 0),
            "amenity": entry.get("amenity", 0),
            "citizen_slots": entry.get("citizen_slots", 0),
            "yields": {k: v for k, v in entry.get("yields", {}).items() if v},
            "regional_range": entry.get("regional_range", 0),
        }
        for key in ("tech", "civic", "district", "great_person_points", "great_work_slots"):
            if entry.get(key):
                row[key] = entry[key]
        # The Palace is placed, never produced, and CIVVIS prices it
        # symbolically; the game's row carries a production cost no player
        # pays either.
        if name == "palace":
            del row["cost"]
        out[name] = row
    return out


# Effects that are not improvement yield grants live in the same file; the
# audit only claims the ones the game database can speak to.
def ours_improvement_upgrades() -> dict[str, dict]:
    known = set(load_ours("improvements"))
    tree = load_ours("tree_effects")
    out: dict[str, dict] = {}
    for node, effects in list(tree["techs"].items()) + list(tree["civics"].items()):
        kept = {
            effect: amount
            for effect, amount in effects.items()
            if any(
                effect == f"{improvement}_{yield_name}"
                for improvement in known
                for yield_name in YIELDS.values()
            )
        }
        if kept:
            out[node] = kept
    return out


def ours_wonders() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("wonders").items():
        effects = entry.get("effects", {})
        yields = {k: v for k, v in entry.get("yields", {}).items() if v}
        regional_range = entry.get("regional_range", 0)
        # Jebel Barkal's Faith is spelled as a regional effect pair; the game
        # spells the same rule as a building yield with a regional range.
        if effects.get("regional_faith"):
            yields["faith"] = yields.get("faith", 0) + effects["regional_faith"]
            regional_range = regional_range or effects.get("regional_faith_range", 0)
        row = {
            "cost": entry.get("cost", 0),
            "yields": yields,
            "housing": entry.get("housing", 0),
            "amenity": entry.get("amenity", 0),
            "regional_range": regional_range,
            "coast": entry.get("coast", False),
            "river": entry.get("river", False),
            "adjacent_mountain": entry.get("adjacent_mountain", False),
            "terrain": set(entry.get("terrain", [])),
            "feature": set(entry.get("feature", [])),
        }
        if entry.get("hills") is False:
            row["flat_only"] = True
        for key in (
            "tech",
            "civic",
            "adjacent_district",
            "adjacent_resource",
            "adjacent_improvement",
            "great_person_points",
            "great_work_slots",
        ):
            if entry.get(key):
                row[key] = entry[key]
        out[name] = row
    return out


def ours_policies() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("policies").items():
        row = {"slot": entry.get("slot", "?")}
        if entry.get("civic"):
            row["civic"] = entry["civic"]
        out[name] = row
    return out


def ours_governments() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("governments").items():
        row = {
            "slots": entry.get("slots", {}),
            "influence_per_turn": entry.get("influence_per_turn", 0),
            "influence_threshold": entry.get("influence_threshold", 0),
            "envoys_per_threshold": entry.get("envoys_per_threshold", 0),
        }
        if entry.get("civic"):
            row["civic"] = entry["civic"]
        out[name] = row
    return out


def ours_beliefs() -> dict[str, dict]:
    out = {}
    for belief_class, entries in load_ours("beliefs").items():
        for name in entries:
            out[name] = {"class": belief_class}
    return out


def ours_promotions() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("promotions").items():
        out[name] = {
            "class": entry.get("class", "?"),
            "tier": entry.get("tier", 0),
            "requires": set(entry.get("requires", [])),
        }
    return out


def ours_projects() -> dict[str, dict]:
    out = {}
    for name, entry in load_ours("projects").items():
        row = {
            "cost": entry.get("cost", 0),
            "repeatable": entry.get("repeatable", False),
        }
        if entry.get("cost_progression_max_pct"):
            row["cost_progression_max_pct"] = entry["cost_progression_max_pct"]
        for key in ("district", "tech", "completion_gpp", "ongoing_yields"):
            if entry.get(key):
                row[key] = entry[key]
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
        ("Adjacency", ours_adjacency(), project_adjacency(database)),
        ("Boosts", ours_boosts(), project_boosts(database)),
        ("GoodyHuts", ours_goody_huts(), project_goody_huts(database)),
        ("Eras", ours_eras(), project_eras(database)),
        ("GreatPeople", ours_great_people(), project_great_people(database)),
        ("GlobalParameters", ours_parameters(), project_parameters(database)),
        ("Maps", dict(ENGINE_MAP_SIZES), project_maps(database)),
        ("GreatWorkValues", dict(ENGINE_GREAT_WORKS), project_great_works(database)),
        ("Happinesses", dict(ENGINE_HAPPINESS), project_happiness(database)),
        ("WMDs", ours_wmds(), project_wmds(database)),
        ("Terrains", ours_terrains(), project_terrains(database)),
        ("Features", ours_features(), project_features(database)),
        ("Resources", ours_resources(), project_resources(database)),
        ("Improvements", ours_improvements(), project_improvements(database)),
        (
            "ImprovementUpgrades",
            ours_improvement_upgrades(),
            project_improvement_upgrades(database),
        ),
        ("Wonders", ours_wonders(), project_wonders(database)),
        ("Policies", ours_policies(), project_policies(database)),
        ("Governments", ours_governments(), project_governments(database)),
        ("Beliefs", ours_beliefs(), project_beliefs(database)),
        ("Promotions", ours_promotions(), project_promotions(database)),
        ("Projects", ours_projects(), project_projects(database)),
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
