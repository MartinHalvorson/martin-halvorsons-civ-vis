#!/usr/bin/env python3
"""Compare Civilization VI runs driven by different CIVVIS league genomes.

A league rating is a claim about relative strength inside CIVVIS. Running the
same genomes in the real game tests whether that claim survives contact with
the engine it was meant to model. This reads the per-turn records two (or more)
runs wrote and reports, for each, how the genome-driven player did against the
field it played in.

Measuring against the field rather than against the other run's raw numbers is
deliberate: unless the runs shared a map seed they had different starts,
different neighbours and different land. The genome-driven player's margin over
its *own* opponents is the comparable quantity; the absolute score is not.

Even so, one game per genome is an anecdote. This prints the sample size it
used and does not pretend otherwise.

Usage::

    python tools/civ6_compare_runs.py run_a.log run_b.log
    python tools/civ6_compare_runs.py --label Maverick2 a.log --label WildCard10 b.log
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from pathlib import Path

MARKER = "CIVVISJSON "


def parse(path: Path) -> dict:
    """Pull the turn records, decisions and genome name out of one run's log."""
    turns, decisions, genome, tag = [], [], None, None
    for line in path.read_text(errors="replace").splitlines():
        index = line.find(MARKER)
        if index < 0:
            continue
        try:
            record = json.loads(line[index + len(MARKER):])
        except json.JSONDecodeError:
            continue
        kind = record.get("kind")
        if kind == "turn":
            turns.append(record)
        elif kind == "decision":
            decisions.append(record)
        elif kind == "loaded":
            genome = record.get("genome") or genome
        if record.get("run"):
            tag = record["run"]
    turns.sort(key=lambda r: r.get("turn", 0))
    return {"path": path, "turns": turns, "decisions": decisions,
            "genome": genome, "tag": tag}


def summarise(run: dict, player_id: int = 0) -> dict | None:
    turns = run["turns"]
    if not turns:
        return None
    last = turns[-1]
    driven = next((p for p in last["players"] if p.get("id") == player_id), None)
    if driven is None:
        return None
    field = [p for p in last["players"] if p.get("id") != player_id and p.get("alive")]
    if not field:
        return None

    def field_mean(key, of=lambda p, k: p.get(k, 0)):
        return statistics.fmean(of(p, key) for p in field)

    cities = lambda p, _: len(p.get("cities", []))  # noqa: E731
    applied = sum(1 for d in run["decisions"] if d.get("applied"))
    return {
        "genome": run["genome"] or "(none)",
        "tag": run["tag"],
        "last_turn": last.get("turn"),
        "opponents": len(field),
        "orders": len(run["decisions"]),
        "orders_applied": applied,
        "score": driven.get("score", 0),
        "field_score": field_mean("score"),
        "cities": len(driven.get("cities", [])),
        "field_cities": statistics.fmean(cities(p, None) for p in field),
        "techs": driven.get("techs", -1),
        "field_techs": field_mean("techs"),
        "units": driven.get("units", 0),
        "field_units": field_mean("units"),
    }


def render(rows: list[dict]) -> str:
    out = ["# Genome runs in Civilization VI", ""]
    if not rows:
        return "no usable runs"
    out.append(
        f"{'genome':<14}{'turns':>6}{'orders':>8}{'score':>7}{'field':>7}{'margin':>8}"
        f"{'cities':>8}{'field':>7}{'techs':>7}{'field':>7}"
    )
    for row in rows:
        margin = row["score"] - row["field_score"]
        out.append(
            f"{row['genome'][:13]:<14}{row['last_turn']:>6}"
            f"{row['orders_applied']:>8}{row['score']:>7}{row['field_score']:>7.1f}"
            f"{margin:>+8.1f}{row['cities']:>8}{row['field_cities']:>7.1f}"
            f"{row['techs']:>7}{row['field_techs']:>7.1f}"
        )
    out += [
        "",
        "`margin` is the driven player's score minus its own field's mean, which is",
        "the only quantity comparable across runs that did not share a map seed.",
        "",
        f"Sample: {len(rows)} game(s), one per genome. That is an anecdote, not a",
        "result -- Civilization's variance across starts is larger than the gaps",
        "below. Repeat on a fixed seed before drawing a conclusion.",
    ]
    return "\n".join(out)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("logs", nargs="+", type=Path, help="Automation.log from each run")
    ap.add_argument("--player", type=int, default=0, help="the genome-driven player id")
    ap.add_argument("--json", type=Path)
    args = ap.parse_args(argv)

    rows = []
    for path in args.logs:
        if not path.is_file():
            print(f"skipping missing {path}", file=sys.stderr)
            continue
        summary = summarise(parse(path), args.player)
        if summary is None:
            print(f"skipping {path}: no usable turn records", file=sys.stderr)
            continue
        rows.append(summary)

    print(render(rows))
    if args.json:
        args.json.write_text(json.dumps(rows, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
