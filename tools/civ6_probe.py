#!/usr/bin/env python3
"""Probe a local Civilization VI installation for a programmatic control channel.

Grounding CIVVIS against the real game needs two things from a running Civ 6:
a way to *read* exact state, and a way to *issue* actions. This script finds out
which channels this installation actually offers, before any harness is built on
top of one. It only observes -- it launches the game and reads files and sockets.

Channels probed, in descending order of usefulness:

1. **FireTuner** -- the game's own Lua console, served over TCP when
   ``EnableTuner 1`` is set in ``AppOptions.txt``. Bidirectional: it can read
   any gameplay value and issue any order. If this is open, everything else is
   a fallback.
2. **Lua logging** -- ``Logs/Lua.log``, written by any loaded mod's ``print()``.
   One-way, but exact, and it needs no debug build.
3. **Native logs** -- the game's own history/effects/event logs, which need no
   mod at all, only the options this repo's setup turns on.
4. **Modding surface** -- whether the user Mods directory is writable and
   whether the game's own database is readable for scenario construction.

Usage::

    python tools/civ6_probe.py            # report what is available
    python tools/civ6_probe.py --launch   # launch the game first, then report
    python tools/civ6_probe.py --json out.json
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

STEAM_APP_ID = "289070"
USER_DIR = Path.home() / "Library/Application Support/Sid Meier's Civilization VI"
INSTALL_DIR = (
    Path.home()
    / "Library/Application Support/Steam/steamapps/common/Sid Meier's Civilization VI"
)
# FireTuner's listener. 4318 is the shipped default; the others are ports the
# tuner has used across versions and are cheap to rule out.
TUNER_PORTS = (4318, 4319, 4320)


def probe_port(port: int, host: str = "127.0.0.1", timeout: float = 0.4) -> bool:
    """True when something accepts a TCP connection on ``port``."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(timeout)
        try:
            return s.connect_ex((host, port)) == 0
        except OSError:
            return False


def listening_ports_of(pids: list[int]) -> list[int]:
    """Every TCP port the given processes listen on, via lsof."""
    if not pids:
        return []
    args = ["lsof", "-nP", "-iTCP", "-sTCP:LISTEN"] + [f"-p{p}" for p in pids]
    try:
        out = subprocess.run(args, capture_output=True, text=True, timeout=15).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    ports = set()
    for line in out.splitlines()[1:]:
        # ... TCP *:4318 (LISTEN)   /   ... TCP 127.0.0.1:4318 (LISTEN)
        for field in line.split():
            if ":" in field and field.rsplit(":", 1)[-1].isdigit():
                ports.add(int(field.rsplit(":", 1)[-1]))
    return sorted(ports)


def civ_pids() -> list[int]:
    """PIDs of running Civilization VI processes (not the Steam client)."""
    try:
        out = subprocess.run(
            ["pgrep", "-f", "Civilization VI"], capture_output=True, text=True
        ).stdout
    except OSError:
        return []
    pids = []
    for line in out.split():
        if not line.isdigit():
            continue
        pid = int(line)
        try:
            cmd = subprocess.run(
                ["ps", "-o", "command=", "-p", str(pid)], capture_output=True, text=True
            ).stdout
        except OSError:
            continue
        # The Steam client's own helpers mention the game in cache paths; the
        # game itself lives under steamapps/common.
        if "steamapps/common" in cmd and "Steam.AppBundle" not in cmd:
            pids.append(pid)
    return pids


def option_value(path: Path, key: str) -> str | None:
    """Read a ``Key Value`` line out of one of the game's options files."""
    if not path.exists():
        return None
    for line in path.read_text(errors="replace").splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) == 2 and parts[0] == key:
            return parts[1].strip()
        if len(parts) == 1 and parts[0] == key:
            return ""
    return None


def recent_logs(max_age_s: float = 3600.0) -> list[dict]:
    """Log files the game has touched recently, largest first."""
    logs_dir = USER_DIR / "Logs"
    if not logs_dir.is_dir():
        return []
    now = time.time()
    found = []
    for p in logs_dir.iterdir():
        if not p.is_file():
            continue
        st = p.stat()
        age = now - st.st_mtime
        if age <= max_age_s:
            found.append({"name": p.name, "bytes": st.st_size, "age_s": round(age, 1)})
    return sorted(found, key=lambda d: -d["bytes"])


def launch_game() -> None:
    """Ask Steam to start the game. Returns as soon as the request is made."""
    subprocess.run(["open", f"steam://rungameid/{STEAM_APP_ID}"], check=False)


def wait_for_process(timeout_s: float) -> list[int]:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        pids = civ_pids()
        if pids:
            return pids
        time.sleep(2.0)
    return []


def collect(launch: bool, wait_s: float) -> dict:
    report: dict = {}

    app = INSTALL_DIR
    report["install"] = {
        "dir": str(app),
        "present": app.is_dir(),
        "entries": sorted(p.name for p in app.iterdir())[:25] if app.is_dir() else [],
    }
    # The playable bundle, whatever Aspyr named it this version.
    bundles = sorted(str(p.relative_to(app)) for p in app.glob("*.app")) if app.is_dir() else []
    report["install"]["app_bundles"] = bundles

    report["options"] = {
        "EnableTuner": option_value(USER_DIR / "AppOptions.txt", "EnableTuner"),
        "EnableDebugMenu": option_value(USER_DIR / "AppOptions.txt", "EnableDebugMenu"),
        "EnableWorldBuilder": option_value(USER_DIR / "AppOptions.txt", "EnableWorldBuilder"),
        "EnableGameCoreEventLog": option_value(
            USER_DIR / "AppOptions.txt", "EnableGameCoreEventLog"
        ),
        "GameHistoryLogLevel": option_value(USER_DIR / "UserOptions.txt", "GameHistoryLogLevel"),
        "GameEffectsLogLevel": option_value(USER_DIR / "UserOptions.txt", "GameEffectsLogLevel"),
    }

    mods_dir = USER_DIR / "Mods"
    report["modding"] = {
        "mods_dir": str(mods_dir),
        "exists": mods_dir.is_dir(),
        "writable": os.access(mods_dir, os.W_OK) if mods_dir.is_dir() else False,
        "installed": sorted(p.name for p in mods_dir.iterdir())[:20] if mods_dir.is_dir() else [],
    }
    # The rules database CIVVIS already audits against, and which scenario
    # construction reads to name terrain, units and improvements.
    gameplay = app / "Base/Assets/Gameplay/Data"
    report["modding"]["gameplay_data_readable"] = gameplay.is_dir()

    if launch:
        report["launch"] = {"requested": True}
        launch_game()
        pids = wait_for_process(wait_s)
        report["launch"]["pids"] = pids
        if pids:
            # The tuner listener is opened during engine init, not at first
            # frame, but give the process room to get there.
            time.sleep(min(20.0, wait_s / 3))
    pids = civ_pids()

    report["process"] = {"pids": pids, "running": bool(pids)}
    report["process"]["listening_ports"] = listening_ports_of(pids)
    report["tuner"] = {
        str(port): probe_port(port) for port in TUNER_PORTS
    }
    report["tuner"]["any_open"] = any(
        v for k, v in report["tuner"].items() if k != "any_open"
    )

    report["logs"] = recent_logs()
    report["logs_dir"] = str(USER_DIR / "Logs")
    return report


def render(report: dict) -> str:
    out = []
    ok = lambda b: "yes" if b else "NO"
    inst = report["install"]
    out.append(f"install present : {ok(inst['present'])}  ({inst['dir']})")
    if inst["app_bundles"]:
        out.append(f"  app bundles   : {', '.join(inst['app_bundles'])}")
    elif inst["present"]:
        out.append(f"  entries       : {', '.join(inst['entries'][:12]) or '(empty)'}")

    out.append("options:")
    for k, v in report["options"].items():
        out.append(f"  {k:<24} {v!r}")

    m = report["modding"]
    out.append(
        f"mods dir        : exists={ok(m['exists'])} writable={ok(m['writable'])} "
        f"installed={m['installed'] or '(none)'}"
    )
    out.append(f"gameplay data   : readable={ok(m['gameplay_data_readable'])}")

    p = report["process"]
    out.append(f"game running    : {ok(p['running'])} pids={p['pids']}")
    out.append(f"listening ports : {p['listening_ports'] or '(none)'}")
    tuner = {k: v for k, v in report["tuner"].items() if k != "any_open"}
    out.append(f"tuner ports     : {tuner}  -> {'OPEN' if report['tuner']['any_open'] else 'closed'}")

    out.append(f"recent logs ({report['logs_dir']}):")
    for entry in report["logs"][:15]:
        out.append(f"  {entry['name']:<38} {entry['bytes']:>10} B  {entry['age_s']:>7}s ago")
    if not report["logs"]:
        out.append("  (none touched in the last hour)")
    return "\n".join(out)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--launch", action="store_true", help="launch the game before probing")
    ap.add_argument("--wait", type=float, default=180.0, help="seconds to wait for the process")
    ap.add_argument("--json", type=Path, help="also write the raw report here")
    args = ap.parse_args(argv)

    report = collect(args.launch, args.wait)
    print(render(report))
    if args.json:
        args.json.write_text(json.dumps(report, indent=2))
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
