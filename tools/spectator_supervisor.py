#!/usr/bin/env python3
"""Keep a CIVVIS spectator running and update it between completed games.

The supervisor deliberately never replaces code during a live match. Once a
winner appears it captures that match's size, fetches the configured upstream,
fast-forwards when safe, builds the newest stable worktree snapshot, and starts
another game after the result-screen cooldown. A network failure can use newer
local code, while a build failure pauses new games until the latest code works.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import signal
import subprocess
import sys
import time
from typing import Any
from urllib.error import URLError
from urllib.request import urlopen


ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target" / "release" / ("civvis.exe" if os.name == "nt" else "civvis")
RUNTIME_BINARY = ROOT / "target" / "spectator" / BINARY.name
RUNTIME_METADATA = RUNTIME_BINARY.parent / "build.json"
RUNTIME_INPUTS = ("Cargo.toml", "Cargo.lock", "build.rs", "src", "data", "web")


def log(message: str) -> None:
    print(f"[spectator] {message}", flush=True)


def command(*args: str, check: bool = False) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=check,
    )


def current_upstream() -> str | None:
    result = command(
        "git", "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"
    )
    return result.stdout.strip() if result.returncode == 0 else None


def sync_current_branch() -> None:
    """Fetch and safely fast-forward the checked-out branch when possible."""
    upstream = current_upstream()
    if not upstream or "/" not in upstream:
        log("no upstream is configured; building the current worktree")
        return

    remote = upstream.split("/", 1)[0]
    fetched = command("git", "fetch", "--prune", remote)
    if fetched.returncode != 0:
        log("update check could not reach the remote; continuing with local code")
        return

    head = command("git", "rev-parse", "HEAD", check=True).stdout.strip()
    remote_head = command("git", "rev-parse", upstream, check=True).stdout.strip()
    if head == remote_head:
        log(f"{upstream} is already current ({head[:8]})")
        return

    behind = command("git", "merge-base", "--is-ancestor", "HEAD", upstream)
    if behind.returncode == 0:
        merged = command("git", "merge", "--ff-only", upstream)
        if merged.returncode == 0:
            new_head = command("git", "rev-parse", "--short", "HEAD", check=True).stdout.strip()
            log(f"fast-forwarded to {upstream} at {new_head}")
        else:
            log("remote is newer but local edits overlap it; preserving local work and continuing")
        return

    ahead = command("git", "merge-base", "--is-ancestor", upstream, "HEAD")
    if ahead.returncode == 0:
        log(f"local branch is ahead of {upstream}; building the newer local worktree")
    else:
        log(f"local branch and {upstream} diverged; preserving local work for manual reconciliation")


def promote_binary() -> None:
    """Atomically preserve a known-good build outside Cargo's output path."""
    RUNTIME_BINARY.parent.mkdir(parents=True, exist_ok=True)
    staged = RUNTIME_BINARY.with_suffix(RUNTIME_BINARY.suffix + ".new")
    shutil.copy2(BINARY, staged)
    os.replace(staged, RUNTIME_BINARY)


def source_snapshot() -> str:
    """Hash every input embedded in or compiled into the game binary."""
    files: list[Path] = []
    for relative in RUNTIME_INPUTS:
        path = ROOT / relative
        if path.is_file():
            files.append(path)
        elif path.is_dir():
            files.extend(candidate for candidate in path.rglob("*") if candidate.is_file())

    digest = hashlib.sha256()
    for path in sorted(files, key=lambda candidate: candidate.relative_to(ROOT).as_posix()):
        relative = path.relative_to(ROOT).as_posix().encode()
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        digest.update(path.read_bytes())
    return digest.hexdigest()


def write_runtime_metadata(snapshot: str) -> None:
    revision = command("git", "rev-parse", "--short", "HEAD", check=True).stdout.strip()
    dirty = bool(command("git", "status", "--porcelain").stdout.strip())
    metadata = {
        "revision": revision,
        "dirty": dirty,
        "source_snapshot": snapshot,
        "binary_sha256": hashlib.sha256(RUNTIME_BINARY.read_bytes()).hexdigest(),
        "built_at": datetime.now(timezone.utc).isoformat(),
    }
    staged = RUNTIME_METADATA.with_suffix(".json.new")
    staged.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    os.replace(staged, RUNTIME_METADATA)
    log(f"build ready at {revision}{' + local edits' if dirty else ''}")


def build_latest(max_attempts: int = 3) -> bool:
    """Build a stable snapshot; never promote an already-obsolete build."""
    for attempt in range(1, max_attempts + 1):
        before = source_snapshot()
        log(f"building the latest worktree (attempt {attempt}/{max_attempts})")
        result = command("cargo", "build", "--release")
        if result.returncode != 0:
            log("latest worktree does not build; no new game will use stale code")
            print(result.stdout, file=sys.stderr, flush=True)
            return False
        after = source_snapshot()
        if before != after:
            log("source changed during compilation; discarding that build")
            continue
        promote_binary()
        write_runtime_metadata(after)
        return True
    log("source kept changing during compilation; waiting for a stable snapshot")
    return False


def prepare_latest(retry_seconds: float) -> None:
    """Block until the newest local/upstream source has a verified build."""
    while True:
        sync_current_branch()
        if build_latest():
            return
        log(f"retrying the latest build in {retry_seconds:g}s")
        time.sleep(retry_seconds)


def read_state(port: int, timeout: float = 1.0) -> dict[str, Any] | None:
    try:
        with urlopen(f"http://127.0.0.1:{port}/state", timeout=timeout) as response:
            value = json.load(response)
            return value if isinstance(value, dict) else None
    except (OSError, URLError, ValueError):
        return None


def session_settings(state: dict[str, Any], defaults: dict[str, int]) -> dict[str, int]:
    """Carry the just-finished game's size forward to the next binary."""
    players = state.get("players") or []
    majors = sum(
        1
        for player in players
        if not player.get("is_minor", False) and not player.get("is_barbarian", False)
    )
    city_states = sum(
        1
        for player in players
        if player.get("is_minor", False) and not player.get("is_barbarian", False)
    )
    game_map = state.get("map") or {}
    return {
        "players": majors or defaults["players"],
        "width": int(game_map.get("width") or defaults["width"]),
        "height": int(game_map.get("height") or defaults["height"]),
        "city_states": city_states if players else defaults["city_states"],
        "turns": defaults["turns"],
    }


def server_command(port: int, settings: dict[str, int], open_browser: bool) -> list[str]:
    args = [
        str(RUNTIME_BINARY if RUNTIME_BINARY.exists() else BINARY),
        "play",
        "--players",
        str(settings["players"]),
        "--width",
        str(settings["width"]),
        "--height",
        str(settings["height"]),
        "--city-states",
        str(settings["city_states"]),
        "--turns",
        str(settings["turns"]),
        "--seed",
        str(random.randrange(1_000_000_000)),
        "--port",
        str(port),
        "--spectate",
    ]
    if not open_browser:
        args.append("--no-open")
    return args


def start_server(
    port: int, settings: dict[str, int], open_browser: bool
) -> subprocess.Popen[str]:
    process = subprocess.Popen(server_command(port, settings, open_browser), cwd=ROOT, text=True)
    log(f"started PID {process.pid} on port {port} ({settings['players']} players)")
    return process


def process_alive(process: subprocess.Popen[str] | None, adopted_pid: int | None) -> bool:
    if process is not None:
        return process.poll() is None
    if adopted_pid is None:
        return False
    try:
        os.kill(adopted_pid, 0)
        return True
    except OSError:
        return False


def stop_server(process: subprocess.Popen[str] | None, adopted_pid: int | None) -> None:
    pid = process.pid if process is not None else adopted_pid
    if pid is None:
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError:
        return
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if process is not None and process.poll() is not None:
            return
        try:
            os.kill(pid, 0)
        except OSError:
            return
        time.sleep(0.1)
    try:
        os.kill(pid, signal.SIGKILL)
    except OSError:
        pass


def wait_for_server(port: int, process: subprocess.Popen[str], timeout: float = 30) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"server exited with status {process.returncode}")
        if read_state(port) is not None:
            return
        time.sleep(0.25)
    raise RuntimeError("server did not become ready")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8766)
    parser.add_argument("--players", type=int, default=4)
    parser.add_argument("--width", type=int, default=60)
    parser.add_argument("--height", type=int, default=38)
    parser.add_argument("--city-states", type=int, default=6)
    parser.add_argument("--turns", type=int, default=500)
    parser.add_argument("--cooldown", type=float, default=10.0)
    parser.add_argument("--poll", type=float, default=0.5)
    parser.add_argument("--build-retry", type=float, default=15.0)
    parser.add_argument("--no-open", action="store_true")
    parser.add_argument(
        "--adopt-pid",
        type=int,
        help="take over an already-running server, then supervise its successors",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    settings = {
        "players": args.players,
        "width": args.width,
        "height": args.height,
        "city_states": args.city_states,
        "turns": args.turns,
    }
    process: subprocess.Popen[str] | None = None
    adopted_pid = args.adopt_pid

    try:
        if adopted_pid is None:
            prepare_latest(args.build_retry)
            process = start_server(args.port, settings, not args.no_open)
            wait_for_server(args.port, process)
        else:
            if not process_alive(None, adopted_pid):
                log(f"cannot adopt PID {adopted_pid}: it is not running")
                return 2
            log(f"adopted PID {adopted_pid} on port {args.port}")

        while True:
            state = read_state(args.port)
            if state is None:
                if not process_alive(process, adopted_pid):
                    log("server stopped unexpectedly; rebuilding and resuming")
                    prepare_latest(args.build_retry)
                    process = start_server(args.port, settings, False)
                    adopted_pid = None
                    wait_for_server(args.port, process)
                time.sleep(args.poll)
                continue

            settings = session_settings(state, settings)
            if state.get("winner") is None:
                time.sleep(args.poll)
                continue

            finished_at = time.monotonic()
            log(
                f"game finished on turn {state.get('turn')} "
                f"({state.get('victory_type') or 'unknown'} victory); checking for updates"
            )
            stop_server(process, adopted_pid)
            process = None
            adopted_pid = None
            remaining = args.cooldown - (time.monotonic() - finished_at)
            if remaining > 0:
                time.sleep(remaining)
            # Update immediately before launch: a commit or local edit that
            # arrived during the result cooldown must make this next game too.
            prepare_latest(args.build_retry)
            process = start_server(args.port, settings, False)
            wait_for_server(args.port, process)
    except KeyboardInterrupt:
        log("stopping")
        stop_server(process, adopted_pid)
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
