#!/usr/bin/env python3
"""Census the shipped Civilization VI ``Modifiers`` tables.

Nearly all Civ VI *content* is data, not code. A leader ability, a belief, a
policy card and a governor promotion are all the same thing: rows in
``Modifiers`` naming a ``ModifierType``, which ``DynamicModifiers`` resolves to
an ``EffectType`` (what happens) and a ``CollectionType`` (who it happens to),
plus ``ModifierArguments`` and an optional ``RequirementSet``.

CIVVIS hardcodes those effects one at a time in Rust. That is a defensible
choice, but it leaves one question unanswered: *how much is left?* This tool
answers it by frequency. It ranks every ``EffectType`` by how many modifier
rows reference it, cross-references ``tools/modifier_coverage.json`` for what
CIVVIS does with it, and reports the unmodelled rows as a single number that
should only ever go down.

Usage::

    python tools/civ6_modifiers.py                    # markdown report
    python tools/civ6_modifiers.py --json out.json    # machine-readable
    python tools/civ6_modifiers.py --max-unmodelled N # CI ratchet
    python tools/civ6_modifiers.py --effect ADJUST_PLOT_YIELD   # drill in

It reads the game files and never writes them. Only the report is an artifact,
so the audit is reproducible without redistributing Firaxis data.
"""

from __future__ import annotations

import argparse
import collections
import json
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from civ6_fidelity import LOAD_ORDER, PACK_EXCLUDE, find_install, truthy  # noqa: E402

# Every gameplay file can carry modifiers, so unlike the rules-data audit there
# is no useful filename filter; a full parse of the three load-order
# directories is a couple of seconds.
MODIFIER_TABLES = {"Modifiers", "DynamicModifiers", "ModifierArguments"}

COVERAGE = Path(__file__).resolve().parent / "modifier_coverage.json"

# What a coverage entry can claim. Anything absent from the coverage file
# counts as unmodelled, so new game content shows up rather than hiding.
STATUSES = ("implemented", "partial", "unmodelled", "out-of-scope")


def fields(node) -> dict:
    """Columns of a row-ish node, in both spellings the XML uses."""
    out = dict(node.attrib)
    for child in node:
        out[child.tag] = (child.text or "").strip()
    return out


class Modifiers:
    def __init__(self) -> None:
        self.dynamic: dict[str, dict] = {}
        self.rows: dict[str, dict] = {}
        self.arguments: dict[str, dict[str, str]] = collections.defaultdict(dict)
        self.attachments: dict[str, list[str]] = collections.defaultdict(list)
        self.owners: dict[str, list[str]] = collections.defaultdict(list)
        self.requirements: dict[str, dict] = {}
        self.requirement_arguments: dict[str, dict[str, str]] = collections.defaultdict(dict)
        self.requirement_sets: dict[str, list[str]] = collections.defaultdict(list)
        self.set_kinds: dict[str, str] = {}

    def apply_file(self, path: Path) -> None:
        try:
            root = ET.parse(path).getroot()
        except ET.ParseError:
            return
        if root.tag != "GameInfo":
            return
        for table in root:
            if table.tag == "DynamicModifiers":
                for node in table:
                    row = fields(node)
                    if "ModifierType" in row:
                        self.dynamic[row["ModifierType"]] = row
            elif table.tag == "Modifiers":
                for node in table:
                    row = fields(node)
                    if "ModifierId" in row:
                        self.rows.setdefault(row["ModifierId"], {}).update(row)
            elif table.tag == "ModifierArguments":
                for node in table:
                    row = fields(node)
                    if "ModifierId" in row and "Name" in row:
                        self.arguments[row["ModifierId"]][row["Name"]] = row.get("Value", "")
            elif table.tag == "Requirements":
                for node in table:
                    row = fields(node)
                    if "RequirementId" in row:
                        self.requirements.setdefault(row["RequirementId"], {}).update(row)
            elif table.tag == "RequirementArguments":
                for node in table:
                    row = fields(node)
                    if "RequirementId" in row and "Name" in row:
                        self.requirement_arguments[row["RequirementId"]][row["Name"]] = row.get(
                            "Value", ""
                        )
            elif table.tag == "RequirementSets":
                for node in table:
                    row = fields(node)
                    if "RequirementSetId" in row:
                        self.set_kinds[row["RequirementSetId"]] = row.get("RequirementSetType", "")
            elif table.tag == "RequirementSetRequirements":
                for node in table:
                    row = fields(node)
                    if "RequirementSetId" in row and "RequirementId" in row:
                        # Overlays restate the same membership row, so keep
                        # the set a set rather than reporting each condition
                        # twice.
                        members = self.requirement_sets[row["RequirementSetId"]]
                        if row["RequirementId"] not in members:
                            members.append(row["RequirementId"])
            elif table.tag.endswith("Modifiers"):
                # An expansion can detach a modifier it no longer wants. Not
                # honouring that reports rules the shipped ruleset removed.
                for node in table:
                    if node.tag != "Delete":
                        continue
                    where = fields(node)
                    doomed = where.get("ModifierId")
                    if doomed and doomed in self.attachments:
                        del self.attachments[doomed]
                        self.owners.pop(doomed, None)
                # BuildingModifiers, TraitModifiers, BeliefModifiers, ... —
                # the tables that bind a modifier to the object that owns it.
                for node in table:
                    row = fields(node)
                    if "ModifierId" not in row:
                        continue
                    # The other column names the object that owns the modifier
                    # -- PolicyType, BuildingType, BeliefType and so on.
                    # Without it a drill can say "some policy does this" but
                    # cannot say which, which is the whole job.
                    owner = next(
                        (
                            value
                            for key, value in row.items()
                            if key not in ("ModifierId", "Name", "Id")
                        ),
                        "",
                    )
                    self.attachments[row["ModifierId"]].append(table.tag)
                    self.owners[row["ModifierId"]].append(owner)

    def condition(self, modifier_id: str) -> str:
        """The requirement set on a modifier, spelled out.

        A modifier row is only half a rule; the other half is the condition
        under which it fires. Reading the amount without the condition is how
        a base-game row gets mistaken for the shipped one.
        """
        row = self.rows.get(modifier_id, {})
        set_id = row.get("SubjectRequirementSetId") or row.get("OwnerRequirementSetId")
        if not set_id:
            return ""
        parts = []
        for requirement_id in self.requirement_sets.get(set_id, []):
            requirement = self.requirements.get(requirement_id, {})
            kind = requirement.get("RequirementType", requirement_id)
            arguments = self.requirement_arguments.get(requirement_id, {})
            negated = "NOT " if truthy(requirement.get("Inverse")) else ""
            rendered = kind.replace("REQUIREMENT_", "")
            if arguments:
                rendered += "(" + ", ".join(f"{k}={v}" for k, v in arguments.items()) + ")"
            parts.append(negated + rendered)
        joiner = " OR " if self.set_kinds.get(set_id) == "REQUIREMENTSET_TEST_ANY" else " AND "
        return f"{set_id}: " + (joiner.join(parts) if parts else "(no requirements)")

    def resolve(self, modifier_id: str) -> tuple[str, str]:
        """The (EffectType, CollectionType) a modifier row resolves to.

        A row names a ``ModifierType``; ``DynamicModifiers`` maps that to the
        pair. Rows whose type is not declared there use an effect the engine
        defines natively, which the census reports as ``UNDECLARED`` rather
        than dropping.
        """
        row = self.rows[modifier_id]
        dynamic = self.dynamic.get(row.get("ModifierType", ""))
        if dynamic is None:
            return ("UNDECLARED", "UNDECLARED")
        return (
            dynamic.get("EffectType", "UNDECLARED"),
            dynamic.get("CollectionType", "UNDECLARED"),
        )


def load(install: Path) -> Modifiers:
    modifiers = Modifiers()
    for relative in LOAD_ORDER:
        directory = install / relative
        if not directory.is_dir():
            print(f"warning: missing load-order directory {relative}", file=sys.stderr)
            continue
        for path in sorted(directory.rglob("*.xml")):
            # Match the rules audit's baseline: optional game modes and
            # non-rules pack files are out of scope, so their modifiers are
            # not backlog.
            if relative.startswith("DLC/") and PACK_EXCLUDE.search(path.name):
                continue
            modifiers.apply_file(path)
    return modifiers


def load_coverage() -> dict[str, dict]:
    if not COVERAGE.exists():
        return {}
    entries = json.loads(COVERAGE.read_text(encoding="utf-8"))["effects"]
    for name, entry in entries.items():
        if entry.get("status") not in STATUSES:
            raise SystemExit(f"{name}: status must be one of {STATUSES}")
    return entries


def short(effect: str) -> str:
    return effect[len("EFFECT_"):] if effect.startswith("EFFECT_") else effect


def census(modifiers: Modifiers) -> list[dict]:
    counts: collections.Counter = collections.Counter()
    owners: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    collections_by_effect: dict[str, collections.Counter] = collections.defaultdict(
        collections.Counter
    )
    for modifier_id in modifiers.rows:
        effect, collection = modifiers.resolve(modifier_id)
        counts[effect] += 1
        collections_by_effect[effect][collection] += 1
        for table in modifiers.attachments.get(modifier_id) or ["(unattached)"]:
            owners[effect][table] += 1
    coverage = load_coverage()
    out = []
    for effect, rows in counts.most_common():
        entry = coverage.get(short(effect), {})
        out.append(
            {
                "effect": short(effect),
                "rows": rows,
                "status": entry.get("status", "unmodelled"),
                "note": entry.get("note", ""),
                "verified": bool(entry.get("verified")),
                "collections": dict(collections_by_effect[effect].most_common()),
                "owners": dict(owners[effect].most_common(4)),
            }
        )
    return out


def report(entries: list[dict], modifiers: Modifiers, install: Path, limit: int) -> str:
    total = sum(entry["rows"] for entry in entries)
    by_status: collections.Counter = collections.Counter()
    for entry in entries:
        by_status[entry["status"]] += entry["rows"]
    lines = [
        "# Modifier census",
        "",
        f"Reference: `{install}` (Gathering Storm load order).",
        "",
        f"{total} modifier rows across {len(entries)} distinct effects, bound by "
        f"{len(modifiers.attachments)} attachments.",
        "",
        "| Status | Effects | Rows | Share |",
        "|---|---:|---:|---:|",
    ]
    for status in STATUSES:
        effects = sum(1 for entry in entries if entry["status"] == status)
        rows = by_status[status]
        lines.append(f"| {status} | {effects} | {rows} | {rows * 100 // max(total, 1)}% |")
    # How concentrated the work is decides the strategy. If a handful of
    # effects covered most rows, hardcoding them would finish the job; if the
    # tail is long, only an interpreter reaches the end of it.
    ranked = sorted((entry["rows"] for entry in entries), reverse=True)
    verified = sum(entry["rows"] for entry in entries if entry["verified"])
    claimed = by_status["implemented"] + by_status["partial"]
    lines += [
        "",
        f"Of the {claimed} covered rows, {verified} are verified row by row "
        "against the shipped modifiers; the rest are inspection judgements.",
        "",
        "| Share of rows | Effects needed |",
        "|---|---:|",
    ]
    for share in (50, 80, 95, 100):
        running = 0
        needed = 0
        for rows in ranked:
            if running * 100 >= share * total:
                break
            running += rows
            needed += 1
        lines.append(f"| {share}% | {needed} |")
    lines += [
        "",
        "## Largest unmodelled effects",
        "",
        "| Rows | Effect | Mostly attached to |",
        "|---:|---|---|",
    ]
    shown = 0
    for entry in entries:
        if entry["status"] not in ("unmodelled", "partial"):
            continue
        owners = ", ".join(f"{table} x{count}" for table, count in entry["owners"].items())
        lines.append(f"| {entry['rows']} | {entry['effect']} | {owners} |")
        shown += 1
        if shown >= limit:
            break
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--civ6", help="path to the Civilization VI install")
    parser.add_argument("--json", help="write the full census here")
    parser.add_argument("--out", help="write the markdown report here instead of stdout")
    parser.add_argument("--limit", type=int, default=40, help="rows in the backlog table")
    parser.add_argument("--effect", help="print every modifier using this effect and stop")
    parser.add_argument(
        "--max-unmodelled",
        type=int,
        default=None,
        help="exit 1 when unmodelled+partial rows exceed this ratchet",
    )
    args = parser.parse_args()

    install = find_install(args.civ6)
    modifiers = load(install)

    if args.effect:
        wanted = args.effect if args.effect.startswith("EFFECT_") else f"EFFECT_{args.effect}"
        for modifier_id in sorted(modifiers.rows):
            effect, collection = modifiers.resolve(modifier_id)
            if effect != wanted:
                continue
            arguments = modifiers.arguments.get(modifier_id, {})
            attached = modifiers.attachments.get(modifier_id) or ["(unattached)"]
            objects = list(modifiers.owners.get(modifier_id) or [])
            objects += [""] * (len(attached) - len(objects))
            owners = ", ".join(
                f"{table}:{obj}" if obj else table
                for table, obj in dict.fromkeys(zip(attached, objects))
            )
            print(f"{modifier_id}\n    {collection}  {owners}\n    {arguments}")
            if condition := modifiers.condition(modifier_id):
                print(f"    when {condition}")
        return 0

    entries = census(modifiers)
    text = report(entries, modifiers, install, args.limit)
    if args.out:
        Path(args.out).write_text(text + "\n", encoding="utf-8")
    else:
        print(text)
    if args.json:
        Path(args.json).write_text(json.dumps(entries, indent=2), encoding="utf-8")

    open_rows = sum(
        entry["rows"] for entry in entries if entry["status"] in ("unmodelled", "partial")
    )
    print(f"\n{open_rows} modifier rows unmodelled or partial", file=sys.stderr)
    if args.max_unmodelled is not None and open_rows > args.max_unmodelled:
        print(
            f"FAIL: {open_rows} exceeds the ratchet of {args.max_unmodelled}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
