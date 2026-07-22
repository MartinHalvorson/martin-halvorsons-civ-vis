#!/usr/bin/env python3
"""Keep a CIVVIS spectator running, recover it, and update between games.

The supervisor deliberately never replaces code during a healthy live match.
It checkpoints active matches, revives a crashed or unresponsive server from
the latest checkpoint, and nudges a spectator whose browser stopped stepping.
Once a winner appears it builds the newest stable worktree while the result
screen remains available, then replaces that finished server after the
cooldown. A known-good runtime remains available while broken or changing
source is repaired.
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
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target" / "release" / ("civvis.exe" if os.name == "nt" else "civvis")
RUNTIME_BINARY = ROOT / "target" / "spectator" / BINARY.name
RUNTIME_METADATA = RUNTIME_BINARY.parent / "build.json"
CHECKPOINT_DIR = RUNTIME_BINARY.parent / "checkpoints"
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


def runtime_matches(snapshot: str) -> bool:
    """Return whether the promoted binary was built from this exact source."""
    if not RUNTIME_BINARY.is_file() or not RUNTIME_METADATA.is_file():
        return False
    try:
        metadata = json.loads(RUNTIME_METADATA.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    return metadata.get("source_snapshot") == snapshot


def build_latest(max_attempts: int = 3) -> bool:
    """Build a stable snapshot; never promote an already-obsolete build."""
    for attempt in range(1, max_attempts + 1):
        before = source_snapshot()
        if runtime_matches(before):
            log("known-good spectator build already matches the latest worktree")
            return True
        log(f"building the latest worktree (attempt {attempt}/{max_attempts})")
        # The visible game does not need to wait for unrelated evaluation
        # binaries to link before its known-good runtime can be promoted.
        result = command("cargo", "build", "--release", "--bin", "civvis")
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


def prepare_latest_once() -> bool:
    """Try one stable-source build without abandoning live monitoring forever."""
    sync_current_branch()
    return build_latest(max_attempts=1)


def prepare_latest(retry_seconds: float) -> None:
    """Block until a verified build exists when no runtime can serve instead."""
    while True:
        if prepare_latest_once():
            return
        log(f"retrying the latest build in {retry_seconds:g}s")
        time.sleep(retry_seconds)


def read_json(
    port: int,
    path: str,
    timeout: float = 1.0,
    method: str = "GET",
) -> dict[str, Any] | None:
    try:
        request = Request(f"http://127.0.0.1:{port}{path}", method=method)
        with urlopen(request, timeout=timeout) as response:
            value = json.load(response)
            return value if isinstance(value, dict) else None
    except (OSError, URLError, ValueError):
        return None


def read_state(port: int, timeout: float = 1.0) -> dict[str, Any] | None:
    return read_json(port, "/state", timeout)


def step_spectator(port: int, timeout: float = 5.0) -> dict[str, Any] | None:
    return read_json(port, "/step", timeout, "POST")


def progress_marker(state: dict[str, Any]) -> tuple[Any, ...]:
    """Identify simulation progress without hashing the large observation."""
    return (
        state.get("seed"),
        state.get("turn"),
        state.get("current"),
        state.get("winner"),
    )


def should_nudge(state: dict[str, Any], stalled_for: float, timeout: float) -> bool:
    """Distinguish a dead spectator loop from an intentional GUI pause."""
    return not state.get("spectator_paused", False) and stalled_for >= max(0.1, timeout)


def successor_started(
    state: dict[str, Any] | None,
    finished_instance: Any,
    finished_seed: Any,
) -> bool:
    """Whether the result server has already rolled into another match."""
    return state is not None and (
        state.get("server_instance") != finished_instance
        or state.get("seed") != finished_seed
        or state.get("winner") is None
    )


def checkpoint_path(port: int) -> Path:
    return CHECKPOINT_DIR / f"spectator-{port}.json"


def capture_checkpoint(port: int, path: Path, timeout: float = 3.0) -> bool:
    """Atomically persist a full server save, rejecting malformed responses."""
    try:
        with urlopen(f"http://127.0.0.1:{port}/save", timeout=timeout) as response:
            payload = response.read()
        value = json.loads(payload)
        if not isinstance(value, dict) or value.get("seed") is None:
            return False
        path.parent.mkdir(parents=True, exist_ok=True)
        staged = path.with_suffix(path.suffix + ".new")
        staged.write_bytes(payload)
        os.replace(staged, path)
        return True
    except (OSError, URLError, ValueError):
        return False


def checkpoint_marker(path: Path) -> tuple[Any, ...] | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if not isinstance(value, dict) or value.get("winner") is not None:
        return None
    return progress_marker(value)


def quarantine_checkpoint(path: Path) -> None:
    """Retain a repeatedly failing save for diagnosis without replaying it."""
    if not path.exists():
        return
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    failed = path.with_name(f"{path.stem}.failed-{stamp}{path.suffix}")
    try:
        os.replace(path, failed)
        log(f"quarantined repeatedly failing checkpoint at {failed}")
    except OSError:
        pass


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


def server_command(
    port: int,
    settings: dict[str, int],
    open_browser: bool,
    resume: Path | None = None,
) -> list[str]:
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
    if resume is not None:
        args.extend(("--resume", str(resume)))
    if not open_browser:
        args.append("--no-open")
    return args


def start_server(
    port: int,
    settings: dict[str, int],
    open_browser: bool,
    resume: Path | None = None,
) -> subprocess.Popen[str]:
    process = subprocess.Popen(
        server_command(port, settings, open_browser, resume), cwd=ROOT, text=True
    )
    detail = f", resuming {resume.name}" if resume is not None else ""
    log(f"started PID {process.pid} on port {port} ({settings['players']} players{detail})")
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


def wait_for_server(
    port: int, process: subprocess.Popen[str], timeout: float = 30
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"server exited with status {process.returncode}")
        state = read_state(port)
        if state is not None:
            return state
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
    parser.add_argument(
        "--unresponsive-timeout",
        type=float,
        default=20.0,
        help="restart a live process whose HTTP state stays unavailable this long",
    )
    parser.add_argument(
        "--stall-timeout",
        type=float,
        default=30.0,
        help="nudge a spectator when its turn/current player stops changing",
    )
    parser.add_argument(
        "--checkpoint-interval",
        type=float,
        default=5.0,
        help="minimum seconds between atomic active-game checkpoints",
    )
    parser.add_argument(
        "--max-resume-attempts",
        type=int,
        default=2,
        help="discard a checkpoint after it repeats the same freeze this many times",
    )
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
    save_path = checkpoint_path(args.port)
    unavailable_since: float | None = None
    last_progress: tuple[Any, ...] | None = None
    progress_at = time.monotonic()
    checkpoint_at = 0.0
    checkpointed_progress: tuple[Any, ...] | None = None
    resume_attempts: dict[tuple[Any, ...], int] = {}
    finished_key: tuple[Any, ...] | None = None
    finished_seen_at = 0.0
    update_retry_at = 0.0

    def launch_recovery(open_browser: bool = False) -> dict[str, Any]:
        nonlocal process, adopted_pid
        stop_server(process, adopted_pid)
        process = None
        adopted_pid = None

        marker = checkpoint_marker(save_path)
        resume = save_path if marker is not None else None
        if marker is not None:
            attempts = resume_attempts.get(marker, 0)
            if attempts >= max(1, args.max_resume_attempts):
                quarantine_checkpoint(save_path)
                resume = None
                marker = None
            else:
                resume_attempts[marker] = attempts + 1

        if not RUNTIME_BINARY.exists():
            prepare_latest(args.build_retry)
        process = start_server(args.port, settings, open_browser, resume)
        try:
            recovered = wait_for_server(args.port, process)
        except RuntimeError:
            if resume is None:
                raise
            log("checkpoint could not be loaded; quarantining it and starting a fresh game")
            stop_server(process, None)
            quarantine_checkpoint(save_path)
            process = start_server(args.port, settings, open_browser)
            recovered = wait_for_server(args.port, process)
            marker = None

        if marker is not None and progress_marker(recovered) == marker:
            log(
                f"resumed checkpoint at turn {recovered.get('turn')} "
                f"(player {recovered.get('current')})"
            )
        elif marker is not None:
            log("runtime could not resume the checkpoint; continued with a fresh game")
        else:
            log("continued with a fresh game because no safe checkpoint was available")
        return recovered

    try:
        if adopted_pid is None:
            # Start a known-good promoted runtime immediately. Source updates
            # are compiled while a completed result screen remains reachable.
            if not RUNTIME_BINARY.exists():
                prepare_latest(args.build_retry)
            state = launch_recovery(not args.no_open)
        else:
            if not process_alive(None, adopted_pid):
                log(f"cannot adopt PID {adopted_pid}: it is not running")
                return 2
            log(f"adopted PID {adopted_pid} on port {args.port}")
            state = read_state(args.port)

        while True:
            state = read_state(args.port)
            if state is None:
                now = time.monotonic()
                unavailable_since = unavailable_since or now
                alive = process_alive(process, adopted_pid)
                unavailable_for = now - unavailable_since
                if not alive or unavailable_for >= max(0.1, args.unresponsive_timeout):
                    reason = "stopped" if not alive else "became unresponsive"
                    log(f"server {reason}; recovering from the latest safe checkpoint")
                    state = launch_recovery()
                    unavailable_since = None
                    last_progress = progress_marker(state)
                    progress_at = time.monotonic()
                    checkpointed_progress = None
                time.sleep(args.poll)
                continue

            unavailable_since = None
            settings = session_settings(state, settings)
            if state.get("winner") is None:
                finished_key = None
                now = time.monotonic()
                marker = progress_marker(state)
                if marker != last_progress:
                    last_progress = marker
                    progress_at = now
                elif state.get("spectator_paused", False):
                    # A human pause is not a freeze. Keep its stall clock fresh
                    # so unpausing does not immediately trigger a false nudge.
                    progress_at = now
                elif should_nudge(state, now - progress_at, args.stall_timeout):
                    log(
                        f"simulation stalled at turn {state.get('turn')} "
                        f"player {state.get('current')}; requesting a recovery step"
                    )
                    stepped = step_spectator(args.port)
                    if stepped is not None:
                        state = stepped
                        marker = progress_marker(state)
                        last_progress = marker
                        progress_at = time.monotonic()
                    else:
                        # A permanently blocked step becomes an independent
                        # HTTP-liveness failure on the following polls.
                        unavailable_since = time.monotonic()

                if (
                    now - checkpoint_at >= max(0.1, args.checkpoint_interval)
                    and marker != checkpointed_progress
                    and capture_checkpoint(args.port, save_path)
                ):
                    checkpoint_at = now
                    checkpointed_progress = marker
                time.sleep(args.poll)
                continue

            finished_instance = state.get("server_instance")
            finished_seed = state.get("seed")
            current_finished_key = (finished_instance, finished_seed)
            now = time.monotonic()
            if current_finished_key != finished_key:
                finished_key = current_finished_key
                finished_seen_at = now
                update_retry_at = 0.0
                log(
                    f"game finished on turn {state.get('turn')} "
                    f"({state.get('victory_type') or 'unknown'} victory); checking for updates"
                )

            if now < update_retry_at:
                time.sleep(min(args.poll, update_retry_at - now))
                continue

            # Keep the completed game's result server available while builds
            # retry. A broken worktree gets one attempt per retry interval so
            # this loop can still checkpoint/recover a browser-started game.
            if not prepare_latest_once():
                update_retry_at = time.monotonic() + args.build_retry
                time.sleep(args.poll)
                continue

            # The page also has a result countdown. If it already created a
            # successor while compilation ran, never interrupt that live game.
            latest_state = read_state(args.port)
            if successor_started(latest_state, finished_instance, finished_seed):
                log("the browser already began another game; leaving that live match uninterrupted")
                state = latest_state
                last_progress = progress_marker(state)
                progress_at = time.monotonic()
                finished_key = None
                continue

            remaining = args.cooldown - (time.monotonic() - finished_seen_at)
            if remaining > 0:
                time.sleep(remaining)
            # The server has its own viewer-independent 10-second restart.
            # Recheck after sleeping so the supervisor does not kill that
            # brand-new match in a race at the shared cooldown boundary.
            latest_state = read_state(args.port)
            if successor_started(latest_state, finished_instance, finished_seed):
                log("the server began another game during cooldown; leaving it uninterrupted")
                state = latest_state
                last_progress = progress_marker(state)
                progress_at = time.monotonic()
                finished_key = None
                continue
            stop_server(process, adopted_pid)
            process = None
            adopted_pid = None
            try:
                save_path.unlink()
            except FileNotFoundError:
                pass
            process = start_server(args.port, settings, False)
            state = wait_for_server(args.port, process)
            last_progress = progress_marker(state)
            progress_at = time.monotonic()
            checkpointed_progress = None
    except KeyboardInterrupt:
        log("stopping")
        stop_server(process, adopted_pid)
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
