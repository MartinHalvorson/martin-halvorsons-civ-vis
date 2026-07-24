#!/usr/bin/env python3
"""Test CIVVIS' combat damage formula against damage the real game rolled.

``docs/FIDELITY.md`` records the combat roll as verified by reasoning rather
than by measurement: CIVVIS uses

    damage = 30 * exp((attacker - defender) / 25) * U(0.8, 1.2)

and the note argues this is "the same distribution as the shipped 24 base with
its 1.0-1.5 spread". Both have mean 30 and span 24..36, so the argument is
sound *if* the shipped base really is 24 and the spread really is 1.0-1.5. That
pair of constants came from community documentation, not from the game.

Civilization VI writes every resolved combat to ``Logs/CombatLog.csv`` with
both sides' strength, both sides' modifiers, and the damage each took. That
turns the claim into something measurable. For each logged combat this tool
computes the strength delta the game itself recorded, divides the observed
damage by CIVVIS' deterministic part, and collects the residual multiplier::

    multiplier = observed_damage / (30 * exp(delta / 25))

If CIVVIS' formula is right, those multipliers are uniform on [0.8, 1.2]:
mean 1.0, min ~0.8, max ~1.2, and flat in between. Any other shape is a real
divergence -- a wrong base, a wrong spread, or a missing modifier -- and the
shape says which.

Rows that cannot test the formula are excluded, and the exclusions are
reported rather than silently dropped, because a filter that quietly removes
disagreeing rows would manufacture agreement:

- damage of 0 (the attack was blocked, or this side does not strike back),
- damage of 100 or more (a kill, where the roll is clipped by remaining HP so
  the observed value is a lower bound on the roll, not the roll),
- combats involving a district or city, which add fortification rules this
  formula does not model.

Usage::

    python tools/civ6_combat_fit.py                    # read the live log
    python tools/civ6_combat_fit.py --log path.csv
    python tools/civ6_combat_fit.py --json out.json
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import statistics
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import civ6_env as env  # noqa: E402

# CIVVIS' constants, from src/game.rs::damage.
CIVVIS_BASE = 30.0
CIVVIS_SCALE = 25.0
CIVVIS_SPREAD = (0.8, 1.2)

# A kill clips the roll at the defender's remaining hit points, so any row at
# or above full damage carries no information about the upper tail.
MAX_HP = 100


# The header of CombatLog.csv does not describe its rows. It names fifteen
# columns, but each row carries thirteen: the game joins the two object-type
# columns into one "att:def" field and the two id columns into another. Reading
# it with DictReader on the header lines every value up by two from the fifth
# column on -- which silently turns strengths into ids and made every row look
# unparsable. Positions, and the split, are the actual format.
COLUMNS = (
    "turn",
    "attacking_civ",
    "defending_civ",
    "obj_types",  # "<attacker>:<defender>", 1 = unit, 3 = district/city
    "ids",  # "<attacker>:<defender>"
    "attacker_type",
    "defender_type",
    "attacker_str",
    "defender_str",
    "attacker_str_mod",
    "defender_str_mod",
    "attacker_dmg",
    "defender_dmg",
)
UNIT_OBJ_TYPE = "1"


def load_rows(path: Path) -> list[dict]:
    rows = []
    with path.open(newline="", errors="replace") as fh:
        for fields in csv.reader(fh, skipinitialspace=True):
            if len(fields) != len(COLUMNS):
                continue
            row = dict(zip(COLUMNS, (f.strip() for f in fields)))
            if not row["turn"].replace("-", "").isdigit():
                continue  # the header, or a torn line
            att_obj, _, def_obj = row["obj_types"].partition(":")
            row["attacker_obj"] = att_obj
            row["defender_obj"] = def_obj
            rows.append(row)
    return rows


def as_float(row: dict, key: str) -> float | None:
    try:
        return float(row[key])
    except (KeyError, TypeError, ValueError):
        return None


def samples(rows: list[dict]) -> tuple[list[dict], dict[str, int]]:
    """Turn logged combats into residual multipliers, with exclusion counts."""
    kept: list[dict] = []
    dropped = {"district_or_city": 0, "zero_damage": 0, "kill_clipped": 0, "unparsable": 0}

    for row in rows:
        att_type = row["attacker_type"]
        def_type = row["defender_type"]
        # Cities and districts defend with fortification rules of their own.
        if row["attacker_obj"] != UNIT_OBJ_TYPE or row["defender_obj"] != UNIT_OBJ_TYPE:
            dropped["district_or_city"] += 1
            continue

        att = as_float(row, "attacker_str")
        dfn = as_float(row, "defender_str")
        att_mod = as_float(row, "attacker_str_mod") or 0.0
        def_mod = as_float(row, "defender_str_mod") or 0.0
        att_dmg = as_float(row, "attacker_dmg")
        def_dmg = as_float(row, "defender_dmg")
        if None in (att, dfn, att_dmg, def_dmg):
            dropped["unparsable"] += 1
            continue

        att_eff = att + att_mod
        def_eff = dfn + def_mod

        # Each side's damage is its own roll, driven by the delta in its
        # favour. The attacker's damage output is driven by attacker-minus-
        # defender; the damage the attacker takes back is the mirror.
        for label, observed, delta in (
            ("defender_took", def_dmg, att_eff - def_eff),
            ("attacker_took", att_dmg, def_eff - att_eff),
        ):
            if observed <= 0:
                dropped["zero_damage"] += 1
                continue
            if observed >= MAX_HP:
                dropped["kill_clipped"] += 1
                continue
            predicted = CIVVIS_BASE * math.exp(delta / CIVVIS_SCALE)
            if predicted <= 0:
                dropped["unparsable"] += 1
                continue
            kept.append(
                {
                    "turn": int(float(row["turn"])),
                    "side": label,
                    "attacker": att_type,
                    "defender": def_type,
                    "att_eff": att_eff,
                    "def_eff": def_eff,
                    "delta": delta,
                    "observed": observed,
                    "predicted_mean": predicted,
                    "multiplier": observed / predicted,
                }
            )
    return kept, dropped


def summarise(kept: list[dict], dropped: dict[str, int]) -> dict:
    mults = [s["multiplier"] for s in kept]
    lo, hi = CIVVIS_SPREAD
    report: dict = {
        "samples": len(kept),
        "excluded": dropped,
        "civvis_formula": f"{CIVVIS_BASE} * exp(delta/{CIVVIS_SCALE}) * U({lo}, {hi})",
    }
    if not mults:
        report["verdict"] = "no usable combats yet"
        return report

    mults_sorted = sorted(mults)
    inside = [m for m in mults if lo <= m <= hi]
    report.update(
        {
            "multiplier_min": round(min(mults), 4),
            "multiplier_max": round(max(mults), 4),
            "multiplier_mean": round(statistics.fmean(mults), 4),
            "multiplier_median": round(statistics.median(mults), 4),
            "inside_civvis_spread": len(inside),
            "inside_fraction": round(len(inside) / len(mults), 4),
            "deciles": [round(mults_sorted[int(len(mults_sorted) * q / 10)], 3) for q in range(10)],
        }
    )
    # If the shape is uniform but shifted, the implied base is what the mean
    # says it should be -- that is the number to correct CIVVIS to.
    report["implied_base_if_uniform_centered"] = round(
        CIVVIS_BASE * statistics.fmean(mults), 3
    )
    return report


def render(report: dict, kept: list[dict], show: int) -> str:
    out = [
        "# Combat damage: CIVVIS formula vs the game's own combat log",
        "",
        f"CIVVIS rolls: {report['civvis_formula']}",
        f"usable combats: {report['samples']}",
        f"excluded: {report['excluded']}",
        "",
    ]
    if not report["samples"]:
        out.append("No usable combats in the log yet -- run a game with fighting in it.")
        return "\n".join(out)

    lo, hi = CIVVIS_SPREAD
    out += [
        "If CIVVIS' formula matches the game, observed/predicted is uniform on "
        f"[{lo}, {hi}].",
        "",
        f"  min      {report['multiplier_min']}",
        f"  median   {report['multiplier_median']}",
        f"  mean     {report['multiplier_mean']}   (uniform [{lo},{hi}] -> 1.0)",
        f"  max      {report['multiplier_max']}",
        f"  inside   {report['inside_civvis_spread']}/{report['samples']}"
        f"  ({report['inside_fraction']:.1%})",
        f"  deciles  {report['deciles']}",
        "",
        f"  implied base if the spread is right: {report['implied_base_if_uniform_centered']}"
        f"  (CIVVIS uses {CIVVIS_BASE})",
        "",
    ]
    if show:
        out.append(f"Widest {show} misses:")
        worst = sorted(kept, key=lambda s: abs(s["multiplier"] - 1.0), reverse=True)[:show]
        out.append(
            f"  {'turn':>5} {'side':<14} {'attacker':<26} {'defender':<26} "
            f"{'delta':>7} {'obs':>5} {'pred':>7} {'mult':>6}"
        )
        for s in worst:
            out.append(
                f"  {s['turn']:>5} {s['side']:<14} {s['attacker'][:25]:<26} "
                f"{s['defender'][:25]:<26} {s['delta']:>7.1f} {s['observed']:>5.0f} "
                f"{s['predicted_mean']:>7.1f} {s['multiplier']:>6.3f}"
            )
    return "\n".join(out)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--log", type=Path, help="CombatLog.csv (default: the live game's)")
    ap.add_argument("--json", type=Path, help="also write the raw report here")
    ap.add_argument("--show", type=int, default=12, help="how many outliers to list")
    args = ap.parse_args(argv)

    path = args.log or (env.logs_dir() / "CombatLog.csv")
    if not path.is_file():
        raise SystemExit(f"no combat log at {path}")

    rows = load_rows(path)
    kept, dropped = samples(rows)
    report = summarise(kept, dropped)
    report["log"] = str(path)
    report["logged_combats"] = len(rows)
    print(render(report, kept, args.show))
    if args.json:
        args.json.write_text(json.dumps({"summary": report, "samples": kept}, indent=2))
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
