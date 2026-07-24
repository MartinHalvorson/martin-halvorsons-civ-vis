#!/usr/bin/env python3
"""Export a CIVVIS league strategy as a genome the Civ 6 grounding mod can play.

The league rates strategies that are, concretely, ~40 scalar weights driving
`AdvancedAi`. Most of that genome is tactical -- how a force groups, when it
withdraws -- and does not transfer, because in the real game the shipped AI
moves the units. What *does* transfer is the economic policy, which is also
what most distinguishes the top strategies from one another:

- ``open0``..``open3``  the first four capital builds, as indices into
  ``OPENING_MENU`` (``ai.rs``); an index past the end means "no scripted pick"
- ``city_target``, ``settler_stop_turn``, ``settler_min_pop``  how wide to go
  and for how long
- ``builder_per_city``, ``mil_per_city``  the standing builder and army targets
- ``d_campus``, ``d_commercial``, ``d_holy``, ``d_theater``  district priority

Those are exactly the levers a mod can pull through ``CityManager``, so this
exports them, verbatim, into a Lua table the mod reads. Nothing is rescaled or
reinterpreted on the way out: a divergence between the two engines should be
the engines' fault, not the exporter's.

Usage::

    python tools/civ6_strategy.py --list
    python tools/civ6_strategy.py Maverick2
    python tools/civ6_strategy.py Maverick2 --lua      # the mod's table
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LEAGUE = REPO / "data/league/league.json"

# src/ai.rs::OPENING_MENU. Order is load-bearing: the genome stores indices.
OPENING_MENU = ["scout", "warrior", "builder", "settler", "slinger", "monument"]

# The genome fields that describe economic policy, i.e. the ones a mod can act
# on. Everything else in Weights is tactical and stays in CIVVIS.
ECONOMIC_FIELDS = (
    "city_target",
    "settler_min_pop",
    "settler_stop_turn",
    "mil_per_city",
    "builder_per_city",
    "settle_food",
    "settle_prod",
    "settle_gold",
    "settle_dist",
    "min_city_dist",
    "wonder_min_bld",
    "d_campus",
    "d_commercial",
    "d_holy",
    "d_theater",
    "open0",
    "open1",
    "open2",
    "open3",
)


def load_league(path: Path = LEAGUE) -> list[dict]:
    if not path.is_file():
        raise SystemExit(f"no league snapshot at {path}")
    return json.loads(path.read_text())["strategies"]


def opening(weights: dict) -> list[str]:
    """The scripted first four capital builds this genome plays."""
    out = []
    for key in ("open0", "open1", "open2", "open3"):
        index = int(math.floor(max(0.0, float(weights.get(key, 99)))))
        out.append(OPENING_MENU[index] if index < len(OPENING_MENU) else "(evaluate)")
    return out


def find(strategies: list[dict], name: str) -> dict:
    for entry in strategies:
        if entry.get("username") == name or entry.get("name") == name:
            return entry
    raise SystemExit(f"no strategy named {name!r}; try --list")


def weights_of(entry: dict) -> dict:
    kind = entry.get("kind", {})
    if "Advanced" in kind:
        return kind["Advanced"]["weights"]
    raise SystemExit(
        f"{entry.get('username')} is a builtin AI ({kind}), not a genome; "
        "it has no weights to export"
    )


def as_lua(entry: dict) -> str:
    weights = weights_of(entry)
    kind = entry.get("kind", {})
    target = kind.get("Advanced", {}).get("target")
    lines = [
        "-- Exported by tools/civ6_strategy.py from data/league/league.json.",
        f"-- {entry['username']}: elo {entry['rating']:.0f} +/- {entry['rd']:.0f} "
        f"over {entry['games']} league games, {entry['wins']} wins.",
        f"-- Scripted opening: {', '.join(opening(weights))}",
        "CivvisGenome = {",
        f'\tName = "{entry["username"]}",',
        f'\tVictoryTarget = "{target or ""}",',
        f"\tElo = {entry['rating']:.1f},",
    ]
    for field in ECONOMIC_FIELDS:
        if field in weights:
            lines.append(f"\t{field} = {float(weights[field]):.6g},")
    lines.append("\tOpeningMenu = {")
    for name in OPENING_MENU:
        lines.append(f'\t\t"{name}",')
    lines.append("\t},")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("strategy", nargs="?", help="username from the league")
    ap.add_argument("--list", action="store_true", help="rank the league's genomes")
    ap.add_argument("--lua", action="store_true", help="emit the mod's Lua table")
    ap.add_argument("--top", type=int, default=12)
    args = ap.parse_args(argv)

    strategies = load_league()
    if args.list or not args.strategy:
        ranked = sorted(strategies, key=lambda s: -s.get("rating", 0))
        print(f"{'player':<16}{'elo':>6}{'rd':>6}{'games':>7}{'wins':>6}  opening / policy")
        for entry in ranked[: args.top]:
            kind = entry.get("kind", {})
            if "Advanced" not in kind:
                note = f"builtin {kind.get('Builtin', {}).get('ai', '?')}"
            else:
                weights = kind["Advanced"]["weights"]
                note = (
                    f"{', '.join(opening(weights))}"
                    f"  | cities {weights.get('city_target', 0):.1f}"
                    f" mil/city {weights.get('mil_per_city', 0):.2f}"
                )
            print(
                f"{entry['username'][:15]:<16}{entry['rating']:>6.0f}{entry['rd']:>6.0f}"
                f"{entry['games']:>7}{entry['wins']:>6}  {note}"
            )
        return 0

    entry = find(strategies, args.strategy)
    if args.lua:
        print(as_lua(entry), end="")
        return 0

    weights = weights_of(entry)
    print(f"{entry['username']}  elo {entry['rating']:.0f} +/- {entry['rd']:.0f}"
          f"  ({entry['games']} games, {entry['wins']} wins)")
    print(f"  scripted opening : {', '.join(opening(weights))}")
    for field in ECONOMIC_FIELDS:
        if field in weights:
            print(f"  {field:<20} {float(weights[field]):.4g}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
