#!/usr/bin/env python3
"""Configure a Civilization VI installation for grounding runs.

The game ships every channel this project needs, all of them off by default:
its Lua console (FireTuner) listens only when ``EnableTuner`` is set, and its
per-turn history, effect-application and game-core event logs only write when
their levels are raised. This turns them on, in the user directory the game
actually reads (see ``civ6_env.user_dir`` -- there are two, and only one
counts).

The game rewrites both options files on launch and on exit, so it has to be
closed while this runs. ``--restart`` handles that: quit, configure, relaunch.

Usage::

    python tools/civ6_setup.py                # report current settings
    python tools/civ6_setup.py --apply        # turn the channels on
    python tools/civ6_setup.py --apply --restart
    python tools/civ6_setup.py --revert       # back to shipped defaults
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import civ6_env as env  # noqa: E402

# Options that open a channel, and why each one is wanted.
APP_OPTIONS = {
    # The Lua console listener. Bidirectional: reads any gameplay value and
    # issues any order, which is the difference between observing the game and
    # driving it.
    "EnableTuner": 1,
    # Every game-core event, stamped per turn. The spine of a replay trace.
    "EnableGameCoreEventLog": 1,
    # Debug menu and WorldBuilder construct the exact states a micro-scenario
    # diff needs. Both already default on in this install, kept here so the
    # configuration is complete rather than incidental.
    "EnableDebugMenu": 1,
    "EnableWorldBuilder": 1,
    # Surfaces database problems a mod introduces instead of failing silently.
    "EnableDataErrorCollection": 1,
}

USER_OPTIONS = {
    # Per-turn, per-player history: score, yields, cities, tech pace. This is
    # the series CIVVIS' own trajectories get compared against.
    "GameHistoryLogLevel": 1,
    "GameHistorySequentialLogLevel": 1,
    # Every modifier as it applies. The finest-grained rules evidence the game
    # emits without a mod, and the one that catches a yield that is right in
    # the database but wrong in the engine.
    "GameEffectsLogLevel": 2,
    "AI_MasterLogging": 1,
    "GameEraMomentsLog": 1,
}

# Shipped defaults, for --revert.
DEFAULTS = {
    "EnableTuner": 0,
    "EnableGameCoreEventLog": 0,
    "EnableDebugMenu": 1,
    "EnableWorldBuilder": 1,
    "EnableDataErrorCollection": 0,
    "GameHistoryLogLevel": 0,
    "GameHistorySequentialLogLevel": 0,
    "GameEffectsLogLevel": 0,
    "AI_MasterLogging": 1,
    "GameEraMomentsLog": 0,
}


def report(user: Path) -> None:
    print(f"user dir : {user}")
    for name, keys in (("AppOptions.txt", APP_OPTIONS), ("UserOptions.txt", USER_OPTIONS)):
        path = user / name
        print(f"\n{name}  ({'present' if path.is_file() else 'MISSING'})")
        for key, want in keys.items():
            have = env.read_option(path, key)
            flag = "ok " if have == str(want) else "-> "
            print(f"  {flag}{key:<32} {have!r:<8} want {want!r}")


def apply(user: Path, values: dict) -> None:
    for name, keys in (("AppOptions.txt", APP_OPTIONS), ("UserOptions.txt", USER_OPTIONS)):
        path = user / name
        changes = {k: values[k] for k in keys if k in values}
        applied = env.set_options(path, changes)
        for key, (old, new) in applied.items():
            if old is None:
                print(f"  !! {name}: {key} not defined by this version, skipped")
            else:
                print(f"  {name}: {key} {old} -> {new}")
        if not applied:
            print(f"  {name}: already as wanted")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--apply", action="store_true", help="turn the channels on")
    ap.add_argument("--revert", action="store_true", help="restore shipped defaults")
    ap.add_argument("--restart", action="store_true", help="quit and relaunch around the change")
    args = ap.parse_args(argv)

    user = env.user_dir()
    if not (args.apply or args.revert):
        report(user)
        return 0

    if env.game_pids():
        if not args.restart:
            print(
                "Civilization VI is running; it rewrites its options on exit and would\n"
                "discard this change. Re-run with --restart, or quit the game first.",
                file=sys.stderr,
            )
            return 2
        print("quitting the game so the options survive...")
        if not env.quit_game():
            print("could not stop the game", file=sys.stderr)
            return 2

    wanted = DEFAULTS if args.revert else {**APP_OPTIONS, **USER_OPTIONS}
    apply(user, wanted)

    # The game only scans a Mods directory that exists.
    mods = env.mods_dir()
    if not mods.is_dir():
        mods.mkdir(parents=True, exist_ok=True)
        print(f"  created {mods}")

    print()
    report(user)

    if args.restart:
        print("\nrelaunching...")
        env.launch_game()
    return 0


if __name__ == "__main__":
    sys.exit(main())
