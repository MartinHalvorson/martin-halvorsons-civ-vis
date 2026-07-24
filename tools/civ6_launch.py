#!/usr/bin/env python3
"""Start and stop Civilization VI reliably enough to run it in a loop.

A grounding harness launches the game dozens of times, so every launch has to
be verifiable rather than hopeful. Three things make that harder than it looks
on this build:

- **Stopping is not instant.** ``pkill`` returns before the process is gone,
  and the game rewrites its options and mod database on the way out. Relaunching
  too early silently runs the *old* configuration, which reads as "the change
  did nothing" -- the single most expensive failure mode here, because nothing
  in any log says the restart did not happen.
- **The Aspyr LaunchPad sits in front of the game.** It is a separate window
  with a PLAY button, and its position is not fixed. Clicking hard-coded
  coordinates hits whatever window happens to be there -- the Steam client, or
  a terminal.
- **Startup is slow.** Cold start to the main menu runs into minutes, and the
  only honest signal that the game core is up is the log it writes.

So: stop and *confirm* stopped, launch, find the LaunchPad by asking the window
server where it is, click PLAY inside its bounds, then wait for the game core
to write its mod scan. Each step is checked, and a failure says which step.

Usage::

    python tools/civ6_launch.py --stop
    python tools/civ6_launch.py --start            # to the main menu
    python tools/civ6_launch.py --restart --timeout 420
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import civ6_env as env  # noqa: E402

# The LaunchPad's PLAY button, as a fraction of the launcher window. The button
# sits in the upper right of the artwork; these are measured from the shipped
# 1.4.6 launcher and are only used as a fallback offset within whatever bounds
# the window server reports, so a moved or resized window still resolves.
PLAY_FRACTION = (0.855, 0.165)


def run(args: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(args, capture_output=True, text=True, **kw)


def stop(timeout_s: float = 45.0) -> bool:
    """Stop the game and confirm every process is gone."""
    if not env.game_pids():
        return True
    run(["pkill", "-f", "Civ6_Exe"])
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if not env.game_pids():
            # The options and mod database are rewritten during exit; give the
            # filesystem a moment so a caller that edits them next does not race.
            time.sleep(1.5)
            return True
        time.sleep(0.5)
    run(["pkill", "-9", "-f", "Civ6_Exe"])
    time.sleep(3.0)
    return not env.game_pids()


def launcher_window() -> tuple[int, int, int, int] | None:
    """Bounds of the LaunchPad window as (x, y, w, h), or None."""
    script = (
        'tell application "System Events"\n'
        '  set out to ""\n'
        '  repeat with p in (every process whose name contains "Civ6")\n'
        '    repeat with w in (every window of p)\n'
        '      set b to position of w\n'
        '      set s to size of w\n'
        '      set out to out & (item 1 of b) & "," & (item 2 of b) & ","'
        ' & (item 1 of s) & "," & (item 2 of s) & "\\n"\n'
        "    end repeat\n"
        "  end repeat\n"
        "  return out\n"
        "end tell"
    )
    result = run(["osascript", "-e", script])
    for line in result.stdout.splitlines():
        parts = [p.strip() for p in line.split(",") if p.strip()]
        if len(parts) == 4 and all(p.lstrip("-").isdigit() for p in parts):
            x, y, w, h = (int(p) for p in parts)
            if w > 200 and h > 150:
                return x, y, w, h
    return None


def click(x: int, y: int) -> None:
    run(["cliclick", f"m:{x},{y}"])
    time.sleep(0.35)
    run(["cliclick", f"c:{x},{y}"])


def press_play(timeout_s: float = 90.0) -> bool:
    """Wait for the LaunchPad and click PLAY. True when a window was clicked."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        bounds = launcher_window()
        if bounds:
            x, y, w, h = bounds
            px = int(x + w * PLAY_FRACTION[0])
            py = int(y + h * PLAY_FRACTION[1])
            # The first click only focuses a background window; the second
            # activates the button.
            click(px, py)
            time.sleep(1.2)
            click(px, py)
            return True
        time.sleep(2.0)
    return False


def wait_for_core(timeout_s: float = 300.0) -> bool:
    """Wait until the game core has run its mod scan.

    Modding.log is the first thing the core writes that proves it got past
    engine init, and it is removed before launch so a stale file from the
    previous run cannot be mistaken for this one's.
    """
    log = env.logs_dir() / "Modding.log"
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if log.is_file() and "Discovered" in log.read_text(errors="replace"):
            return True
        if not env.game_pids():
            return False  # it died; do not wait out the whole timeout
        time.sleep(3.0)
    return False


def start(timeout_s: float) -> int:
    log = env.logs_dir() / "Modding.log"
    if log.exists():
        log.unlink()  # so the wait below cannot pass on the previous run's scan

    env.launch_game()
    if not press_play():
        print("no launcher window appeared; is Steam signed in?", file=sys.stderr)
        return 2
    print("clicked PLAY, waiting for the game core...")
    if not wait_for_core(timeout_s):
        alive = bool(env.game_pids())
        print(
            f"game core did not come up within {timeout_s:.0f}s "
            f"(process {'still running' if alive else 'exited'})",
            file=sys.stderr,
        )
        return 3
    print("game core is up (mod scan complete)")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--stop", action="store_true")
    ap.add_argument("--start", action="store_true")
    ap.add_argument("--restart", action="store_true")
    ap.add_argument("--timeout", type=float, default=360.0)
    args = ap.parse_args(argv)

    if not (args.stop or args.start or args.restart):
        ap.error("pass --stop, --start or --restart")

    if args.stop or args.restart:
        print("stopping...")
        if not stop():
            print("could not stop the game", file=sys.stderr)
            return 2
        print("stopped")
    if args.start or args.restart:
        return start(args.timeout)
    return 0


if __name__ == "__main__":
    sys.exit(main())
