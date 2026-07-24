#!/usr/bin/env python3
"""Install the CIVVIS grounding mod and configure a Civilization VI run.

The mod in ``tools/civ6_mod`` drives the game's autoplay manager and writes a
per-turn record of every major player to the game's own logs. This installs it
where this build will actually find it, stamps the run's settings into it, and
optionally restarts the game so the change takes effect.

Two things about this build shape the installer, and both were found the
expensive way -- by a mod that loaded and did nothing:

- The mod goes in the *install's* ``DLC`` tree. No user Mods directory is
  scanned, so a mod placed in one is never discovered and nothing says why.
  The install is only added to; ``--uninstall`` fully reverts it.
- Settings are prepended into the script rather than ``include``d, because a
  file listed under ``<Files>`` is not on the include path.

Output lands in ``Logs/Automation.log``: this build writes no ``Lua.log``, so
``print`` from a mod goes nowhere and ``Automation.Log`` is the channel that
survives.

Usage::

    python tools/civ6_run.py --install                      # install only
    python tools/civ6_run.py --install --turns 250 --tag t1 --launch
    python tools/civ6_run.py --status                       # what is installed
"""

from __future__ import annotations

import argparse
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import civ6_env as env  # noqa: E402

MOD_SOURCE = Path(__file__).resolve().parent / "civ6_mod"
MOD_NAME = "CivvisGrounding"
CONFIG_FILE = "CivvisGroundingConfig.lua"
SCRIPT_FILE = "CivvisGrounding.lua"

CONFIG_PRELUDE = """-- Prepended by tools/civ6_run.py. Do not edit the installed copy.
--
-- These are the run's settings. They are prepended rather than `include`d
-- because a file listed under <Files> in the .modinfo is not on the include
-- path unless an ImportFiles action puts it there -- so the include failed
-- silently and every setting fell back to its default.

CivvisGroundingConfig = {{
\tAutoplayTurns = {turns},
\tObserveAsPlayer = {observe},
\tReturnAsPlayer = {return_as},
\tDumpState = {dump},
\tRunTag = "{tag}",
}}

"""


def install_dir() -> Path:
    """Where the game will actually find the mod.

    The macOS build scans the install's ``DLC`` tree and the Steam Workshop
    directory; it does not scan any user Mods directory, so installing there
    produces a mod that is never discovered and never reports why. The DLC tree
    is additive -- this adds a folder and touches no shipped file, and removing
    the folder fully reverts it.
    """
    return env.assets_dir() / "DLC" / MOD_NAME


def install(turns: int, observe: int, return_as: int, dump: bool, tag: str) -> Path:
    target = install_dir()
    target.mkdir(parents=True, exist_ok=True)
    for src in sorted(MOD_SOURCE.iterdir()):
        if src.name in (CONFIG_FILE, SCRIPT_FILE):
            continue  # handled below
        shutil.copy2(src, target / src.name)
    prelude = CONFIG_PRELUDE.format(
        turns=turns,
        observe=observe,
        return_as=return_as,
        dump="true" if dump else "false",
        tag=tag,
    )
    (target / SCRIPT_FILE).write_text(prelude + (MOD_SOURCE / SCRIPT_FILE).read_text())
    # Kept so the .modinfo's <Files> list resolves, and so the settings are
    # readable in the install without reading the whole script.
    (target / CONFIG_FILE).write_text(prelude)
    return target


def uninstall() -> None:
    target = install_dir()
    if target.is_dir():
        shutil.rmtree(target)
        print(f"removed {target}")
    else:
        print("not installed")


def status() -> None:
    target = install_dir()
    print(f"user dir : {env.user_dir()}")
    print(f"installed: {target}  ({'yes' if target.is_dir() else 'NO'})")
    if target.is_dir():
        for path in sorted(target.iterdir()):
            print(f"  {path.name:<32} {path.stat().st_size:>7} B")
        config = target / CONFIG_FILE
        if config.is_file():
            print("\nactive config:")
            for line in config.read_text().splitlines():
                if "=" in line and not line.lstrip().startswith("--"):
                    print(f"  {line.strip()}")
    # This build writes no Lua.log; Automation.log is where the mod's lines land.
    log = env.logs_dir() / "Automation.log"
    print(f"\nlog      : {log}  ({'present' if log.is_file() else 'not written yet'})")
    if log.is_file():
        hits = sum(1 for line in log.read_text(errors="replace").splitlines()
                   if "CIVVISJSON" in line)
        print(f"  CIVVISJSON lines: {hits}")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--install", action="store_true", help="install/refresh the mod")
    ap.add_argument("--status", action="store_true", help="report what is installed")
    ap.add_argument("--uninstall", action="store_true", help="remove the mod from the install")
    ap.add_argument("--turns", type=int, default=0, help="autoplay turns (0 = manual play)")
    ap.add_argument("--observe", type=int, default=-1, help="player to observe (-1 = none)")
    ap.add_argument("--return-as", type=int, default=0, help="player to return control to")
    ap.add_argument("--no-dump", action="store_true", help="skip the per-turn state record")
    ap.add_argument("--tag", default="run", help="tag stamped into every logged line")
    ap.add_argument("--launch", action="store_true", help="restart the game afterwards")
    args = ap.parse_args(argv)

    if not (args.install or args.status or args.launch or args.uninstall):
        ap.error("nothing to do; pass --install, --status, --uninstall or --launch")

    if args.uninstall:
        env.quit_game()
        uninstall()

    if args.install:
        if env.game_pids() and not args.launch:
            print(
                "Civilization VI is running. A mod is only scanned at startup, so this\n"
                "install would not take effect until the next launch. Pass --launch to\n"
                "restart, or quit the game first.",
                file=sys.stderr,
            )
            return 2
        if env.game_pids():
            print("quitting the game so the mod is rescanned...")
            env.quit_game()
        target = install(args.turns, args.observe, args.return_as, not args.no_dump, args.tag)
        # The modding database indexes scanned files by path and mtime, so a
        # newly created mod folder is not noticed until the index is rebuilt.
        # Dropping it costs one slower startup and is the difference between
        # the mod loading and "Discovered 0 mods".
        mod_db = env.user_dir() / "Mods.sqlite"
        if mod_db.exists():
            mod_db.unlink()
            print(f"cleared {mod_db.name} to force a rescan")
        print(f"installed {MOD_NAME} -> {target}")
        print(f"  autoplay turns : {args.turns}")
        print(f"  run tag        : {args.tag}")

    if args.status:
        status()

    if args.launch:
        if env.game_pids():
            env.quit_game()
        print("launching...")
        env.launch_game()
    return 0


if __name__ == "__main__":
    sys.exit(main())
