#!/usr/bin/env python3
"""Keep a CIVVIS spectator running, recover it, and update between games.

The supervisor checkpoints active matches, revives a crashed or unresponsive
server from the latest checkpoint, and nudges a spectator whose browser stopped
stepping. Once a winner appears it retires that server immediately, leaving the
rendered result screen visible while it tries the newest stable worktree. A
broken or changing side edit cannot stall the cycle because the last verified
runtime starts the successor instead. If that fallback was necessary, the
supervisor keeps retrying and atomically resumes the match on fresh code as soon
as a stable build and safe checkpoint are both available. The browser's guarded
result countdown cannot race either path ahead on stale code.
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
RESULTS_DIR = RUNTIME_BINARY.parent / "results"
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
        binary_hash = hashlib.sha256(RUNTIME_BINARY.read_bytes()).hexdigest()
    except (OSError, ValueError):
        return False
    return (
        metadata.get("source_snapshot") == snapshot
        and metadata.get("binary_sha256") == binary_hash
    )


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


def prepare_boundary_runtime(retry_seconds: float) -> bool:
    """Try fresh code once, falling back when a verified runtime already exists.

    Returns whether the latest worktree was made ready. Cold starts still wait
    for a build because no executable fallback exists, but an in-progress side
    edit must never leave the game cycle offline indefinitely.
    """
    if prepare_latest_once():
        return True
    if RUNTIME_BINARY.exists():
        log(
            "latest source is not ready; starting the next game on the last "
            "verified runtime"
        )
        return False
    prepare_latest(retry_seconds)
    return True


def prepare_live_refresh(port: int, path: Path) -> bool:
    """Build current source and checkpoint before replacing a fallback runtime."""
    if not prepare_latest_once():
        return False
    if not capture_checkpoint(port, path):
        log("fresh build is ready but no safe checkpoint was captured; retrying")
        return False
    return True


def prebuild_latest_once() -> bool:
    """Keep the promoted fallback current without interrupting the live server."""
    snapshot = source_snapshot()
    if runtime_matches(snapshot):
        return True
    log("source changed during the active game; prebuilding the next runtime")
    return prepare_latest_once()


def start_background_prebuild() -> subprocess.Popen[str]:
    """Compile in a separate process so winner polling never waits on Cargo."""
    return subprocess.Popen(
        [sys.executable, str(Path(__file__).resolve()), "--prepare-once"],
        cwd=ROOT,
        text=True,
        start_new_session=os.name != "nt",
    )


def stop_background_prebuild(process: subprocess.Popen[str] | None) -> None:
    """Stop the isolated build worker and its Cargo descendants."""
    if process is None or process.poll() is not None:
        return
    try:
        if os.name == "nt":
            process.terminate()
        else:
            os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=5.0)
    except (OSError, subprocess.TimeoutExpired):
        try:
            if os.name == "nt":
                process.kill()
            else:
                os.killpg(process.pid, signal.SIGKILL)
        except OSError:
            pass


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


def checkpoint_path(port: int) -> Path:
    return CHECKPOINT_DIR / f"spectator-{port}.json"


def capture_checkpoint(port: int, path: Path, timeout: float = 30.0) -> bool:
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


def archive_result(
    port: int,
    state: dict[str, Any],
    directory: Path | None = None,
    timeout: float = 30.0,
) -> Path | None:
    """Preserve the exact final save and source metadata before handoff."""
    if state.get("winner") is None:
        return None
    try:
        with urlopen(f"http://127.0.0.1:{port}/save", timeout=timeout) as response:
            payload = response.read()
        save = json.loads(payload)
        if (
            not isinstance(save, dict)
            or save.get("seed") != state.get("seed")
            or save.get("winner") is None
        ):
            return None

        archived_at = datetime.now(timezone.utc)
        stamp = archived_at.strftime("%Y%m%dT%H%M%S.%fZ")
        instance = state.get("server_instance", "unknown")
        stem = (
            f"{stamp}-seed-{save.get('seed')}-turn-{save.get('turn')}"
            f"-instance-{instance}"
        )
        destination = directory if directory is not None else RESULTS_DIR
        destination.mkdir(parents=True, exist_ok=True)
        save_path = destination / f"{stem}.save.json"
        staged_save = save_path.with_suffix(save_path.suffix + ".new")
        staged_save.write_bytes(payload)
        os.replace(staged_save, save_path)

        try:
            runtime = json.loads(RUNTIME_METADATA.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            runtime = None
        result = {
            "archived_at": archived_at.isoformat(),
            "server_instance": state.get("server_instance"),
            "seed": save.get("seed"),
            "turn": save.get("turn"),
            "winner": save.get("winner"),
            "victory_type": save.get("victory_type"),
            "game_speed": save.get("game_speed"),
            "max_turns": save.get("max_turns"),
            "map_script": save.get("map_script"),
            "standings": result_standings(state),
            "runtime": runtime,
            "save": save_path.name,
        }
        result_path = destination / f"{stem}.result.json"
        staged_result = result_path.with_suffix(result_path.suffix + ".new")
        staged_result.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        os.replace(staged_result, result_path)
        return save_path
    except (OSError, URLError, ValueError, TypeError):
        return None


def checkpoint_marker(path: Path) -> tuple[Any, ...] | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if not isinstance(value, dict) or value.get("winner") is not None:
        return None
    return progress_marker(value)


def recovered_checkpoint_game(
    state: dict[str, Any], marker: tuple[Any, ...] | None
) -> bool:
    """Recognize a resumed game even if the browser already advanced it."""
    return marker is not None and state.get("seed") == marker[0]


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


def session_settings(state: dict[str, Any], defaults: dict[str, Any]) -> dict[str, Any]:
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
        "turns": int(state.get("max_turns") or defaults["turns"]),
        "map": game_map.get("script") or defaults["map"],
        "speed": state.get("game_speed") or defaults["speed"],
    }


def result_standings(state: dict[str, Any]) -> str | None:
    """Format a compact durable record before the result server is retired."""
    players = [
        player
        for player in state.get("players") or []
        if not player.get("is_minor", False) and not player.get("is_barbarian", False)
    ]
    if not players:
        return None

    winner = state.get("winner")
    ranked = sorted(
        players,
        key=lambda player: (player.get("score") or 0, -(player.get("id") or 0)),
        reverse=True,
    )
    entries = []
    for player in ranked:
        identity = player.get("civ") or player.get("leader") or f"player {player.get('id')}"
        prefix = "winner " if player.get("id") == winner else ""
        details = [
            f"score {player.get('score', '?')}",
            f"cities {player.get('cities', '?')}",
            f"faith {player.get('faith', '?')}",
            f"military {player.get('military', '?')}",
        ]
        entries.append(f"{prefix}{identity} ({', '.join(details)})")
    return "; ".join(entries)


def server_command(
    port: int,
    settings: dict[str, Any],
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
        "--map",
        str(settings["map"]),
        "--speed",
        str(settings["speed"]),
        "--seed",
        str(random.randrange(1_000_000_000)),
        "--port",
        str(port),
        "--spectate",
        "--supervised",
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


def process_busy(
    process: subprocess.Popen[str] | None,
    adopted_pid: int | None,
    threshold: float = 1.0,
) -> bool:
    """Best-effort check that an unavailable server is still computing."""
    pid = process.pid if process is not None else adopted_pid
    if pid is None:
        return False
    result = command("ps", "-o", "%cpu=", "-p", str(pid))
    if result.returncode != 0:
        return False
    try:
        return float(result.stdout.strip()) >= threshold
    except ValueError:
        return False


def unavailable_recovery_due(
    alive: bool,
    unavailable_for: float,
    recently_busy: bool,
    unresponsive_timeout: float,
    busy_timeout: float,
) -> bool:
    """Decide whether an unavailable process should be replaced.

    A CPU-active simulation is making useful progress even when its
    single-threaded HTTP server cannot answer health checks. By default there
    is no wall-clock ceiling on that work. Operators can still opt into a hard
    ceiling with ``--busy-timeout`` when diagnosing a suspected compute loop.
    """
    if not alive:
        return True
    if unavailable_for < unresponsive_timeout:
        return False
    if recently_busy:
        return busy_timeout > 0.0 and unavailable_for >= busy_timeout
    return True


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
    parser.add_argument("--turns", type=int, default=250)
    parser.add_argument(
        "--map",
        choices=("pangaea", "continents", "small_continents", "inland_sea"),
        default="pangaea",
    )
    parser.add_argument(
        "--speed",
        choices=("online", "quick", "standard", "epic", "marathon"),
        default="online",
    )
    parser.add_argument(
        "--cooldown",
        type=float,
        default=5.0,
        help="seconds to keep the rendered result visible before the immediate successor",
    )
    parser.add_argument("--poll", type=float, default=0.5)
    parser.add_argument("--build-retry", type=float, default=15.0)
    parser.add_argument(
        "--source-check-interval",
        type=float,
        default=30.0,
        help="prebuild changed source during live play so game boundaries stay instant",
    )
    parser.add_argument(
        "--unresponsive-timeout",
        type=float,
        default=60.0,
        help="check a live process whose HTTP state stays unavailable this long",
    )
    parser.add_argument(
        "--busy-timeout",
        type=float,
        default=0.0,
        help="optional hard ceiling for a CPU-busy request; 0 never kills active compute",
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
    parser.add_argument("--prepare-once", action="store_true", help=argparse.SUPPRESS)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if getattr(args, "prepare_once", False):
        return 0 if prepare_latest_once() else 1
    settings = {
        "players": args.players,
        "width": args.width,
        "height": args.height,
        "city_states": args.city_states,
        "turns": args.turns,
        "map": args.map,
        "speed": args.speed,
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
    busy_reported = False
    busy_check_at = 0.0
    busy_until = 0.0
    refresh_pending = False
    refresh_at = 0.0
    source_check_at = 0.0
    prebuild_process: subprocess.Popen[str] | None = None

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
            prepare_boundary_runtime(args.build_retry)
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

        if recovered_checkpoint_game(recovered, marker):
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

        # A cold supervisor can inherit a verified but stale runtime, and a
        # boundary may deliberately start that fallback when source is still
        # changing. Do not make the successor play a whole game before trying
        # the stable source again.
        refresh_pending = not runtime_matches(source_snapshot())
        refresh_at = time.monotonic()
        if refresh_pending:
            log("active runtime is behind the worktree; scheduling a safe live refresh")

        while True:
            state = read_state(args.port)
            if state is None:
                now = time.monotonic()
                unavailable_since = unavailable_since or now
                alive = process_alive(process, adopted_pid)
                unavailable_for = now - unavailable_since
                unresponsive_timeout = max(0.1, args.unresponsive_timeout)
                busy_timeout = (
                    max(unresponsive_timeout, args.busy_timeout)
                    if args.busy_timeout > 0.0
                    else 0.0
                )
                if (
                    alive
                    and unavailable_for >= unresponsive_timeout
                    and now >= busy_check_at
                ):
                    observed_busy = process_busy(process, adopted_pid)
                    busy_check_at = now + 5.0
                    if observed_busy:
                        # A brief scheduler gap must not turn a long, valid AI
                        # action into a crash. Require a full idle recovery
                        # window after the last observed CPU activity.
                        busy_until = now + unresponsive_timeout
                    if observed_busy and not busy_reported:
                        log(
                            "server is unavailable but actively computing; "
                            "extending the recovery window"
                        )
                        busy_reported = True
                recently_busy = alive and now < busy_until

                if unavailable_recovery_due(
                    alive,
                    unavailable_for,
                    recently_busy,
                    unresponsive_timeout,
                    busy_timeout,
                ):
                    reason = "stopped" if not alive else "became unresponsive"
                    log(f"server {reason}; recovering from the latest safe checkpoint")
                    state = launch_recovery()
                    unavailable_since = None
                    busy_reported = False
                    busy_check_at = 0.0
                    busy_until = 0.0
                    last_progress = progress_marker(state)
                    progress_at = time.monotonic()
                    checkpointed_progress = None
                time.sleep(args.poll)
                continue

            unavailable_since = None
            busy_reported = False
            busy_check_at = 0.0
            busy_until = 0.0
            settings = session_settings(state, settings)
            if state.get("winner") is None:
                now = time.monotonic()
                if prebuild_process is not None and prebuild_process.poll() is not None:
                    prebuild_process = None
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

                if refresh_pending and now >= refresh_at:
                    refresh_at = now + max(0.1, args.build_retry)
                    snapshot = source_snapshot()
                    runtime_ready = prebuild_process is None and runtime_matches(snapshot)
                    if runtime_ready:
                        checkpoint_ready = capture_checkpoint(args.port, save_path)
                    else:
                        checkpoint_ready = False
                    if checkpoint_ready:
                        log("fresh runtime is ready; resuming the active game from checkpoint")
                        state = launch_recovery()
                        refresh_pending = False
                        unavailable_since = None
                        busy_reported = False
                        busy_check_at = 0.0
                        busy_until = 0.0
                        last_progress = progress_marker(state)
                        progress_at = time.monotonic()
                        checkpointed_progress = last_progress
                        checkpoint_at = progress_at
                        continue
                    if prebuild_process is None and not runtime_ready:
                        log("retrying stable source for the active fallback runtime")
                        prebuild_process = start_background_prebuild()
                    elif prebuild_process is None:
                        log("fresh build is ready but no safe checkpoint was captured; retrying")
                elif not refresh_pending and now >= source_check_at:
                    source_check_at = now + max(1.0, args.source_check_interval)
                    snapshot = source_snapshot()
                    if prebuild_process is None and not runtime_matches(snapshot):
                        log("source changed during the active game; prebuilding in the background")
                        prebuild_process = start_background_prebuild()
                time.sleep(args.poll)
                continue

            finished_seen_at = time.monotonic()
            log(
                f"game finished on turn {state.get('turn')} "
                f"({state.get('victory_type') or 'unknown'} victory); claiming the boundary"
            )
            standings = result_standings(state)
            if standings:
                log(f"standings: {standings}")
            archive = archive_result(args.port, state)
            if archive is not None:
                log(f"archived final save at {archive}")
            else:
                log("could not archive the final save; continuing the handoff")

            # The supervised server rejects every in-process /new request, so
            # it is safe to leave the result reachable during the short
            # cooldown. Builds happen during active play; the boundary itself
            # never waits on Cargo.
            remaining = args.cooldown - (time.monotonic() - finished_seen_at)
            if remaining > 0:
                time.sleep(remaining)
            stop_server(process, adopted_pid)
            process = None
            adopted_pid = None
            try:
                save_path.unlink()
            except FileNotFoundError:
                pass

            snapshot = source_snapshot()
            latest_ready = runtime_matches(snapshot)
            if latest_ready:
                # A commit changes repository identity without changing the
                # compiled input snapshot. Reconcile metadata so the promoted
                # binary records the clean synced revision it exactly matches.
                write_runtime_metadata(snapshot)
            else:
                log(
                    "latest source is not prebuilt; starting the verified "
                    "fallback immediately and refreshing it during play"
                )
            process = start_server(args.port, settings, False)
            state = wait_for_server(args.port, process)
            refresh_pending = not latest_ready
            refresh_at = time.monotonic()
            source_check_at = time.monotonic() + max(1.0, args.source_check_interval)
            last_progress = progress_marker(state)
            progress_at = time.monotonic()
            checkpointed_progress = None
    except KeyboardInterrupt:
        log("stopping")
        stop_background_prebuild(prebuild_process)
        stop_server(process, adopted_pid)
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
