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
import ctypes
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
import tempfile
import time
import traceback
from typing import Any
from urllib.error import URLError
from urllib.request import Request, urlopen

if os.name == "nt":
    # ctypes.wintypes only imports on Windows.
    from ctypes import wintypes


SCRIPT_PATH = Path(__file__).resolve()
SCRIPT_ROOT = SCRIPT_PATH.parents[1]
RUNNING_SUPERVISOR_SHA256 = hashlib.sha256(SCRIPT_PATH.read_bytes()).hexdigest()
ROOT = Path(os.environ.get("CIVVIS_DEPLOY_ROOT", str(SCRIPT_ROOT))).expanduser().resolve()
SOURCE_ROOT = Path(
    os.environ.get(
        "CIVVIS_SUPERVISOR_SOURCE",
        str(ROOT.parent / f"{ROOT.name.lower()}-spectator-src"),
    )
).expanduser().resolve()
BINARY_NAME = "civvis.exe" if os.name == "nt" else "civvis"
RUNTIME_BINARY = ROOT / "target" / "spectator" / BINARY_NAME
RUNTIME_METADATA = RUNTIME_BINARY.parent / "build.json"
CHECKPOINT_DIR = RUNTIME_BINARY.parent / "checkpoints"
RESULTS_DIR = RUNTIME_BINARY.parent / "results"
RUNTIME_INPUTS = ("Cargo.toml", "Cargo.lock", "build.rs", "src", "data", "web")
SYNC_REMOTE = os.environ.get("CIVVIS_SYNC_REMOTE", "origin")
SYNC_BRANCH = os.environ.get("CIVVIS_SYNC_BRANCH", "main")
LOG_FILE = Path(
    os.environ.get("CIVVIS_SPECTATOR_LOG", str(ROOT / "spectator-supervisor.log"))
).expanduser()
# A console child - the game server, cargo, git - launched from a windowless
# parent (pythonw, or a hidden scheduled task) pops its own console window on
# Windows. CREATE_NO_WINDOW suppresses it so an unattended supervisor never
# flashes a terminal. Empty, and so a no-op, on every other platform.
_NO_WINDOW = {"creationflags": 0x08000000} if os.name == "nt" else {}


def log(message: str) -> None:
    line = f"[spectator] {message}"
    # An unattended supervisor has no console to print to - pythonw discards
    # stdout, a detached task never had one - so the durable record is a file
    # keyed off the deploy root. Printing stays best-effort on top of it.
    try:
        stamp = datetime.now(timezone.utc).strftime("%m-%d %H:%M:%S")
        with open(LOG_FILE, "a", encoding="utf-8") as handle:
            handle.write(f"{stamp} {line}\n")
    except OSError:
        pass
    try:
        print(line, flush=True)
    except (OSError, ValueError, AttributeError):
        pass


def updated_supervisor_command(
    server_pid: int | None, argv: list[str] | None = None
) -> list[str] | None:
    """Return an exec command when canonical source contains newer supervision.

    The game binary is promoted atomically, but Python keeps executing the code
    it imported at process start.  Re-exec the canonical script after a source
    sync and hand it the live server PID so supervision upgrades without
    stopping or replacing the visible match.
    """
    candidate = SOURCE_ROOT / "tools" / "spectator_supervisor.py"
    try:
        payload = candidate.read_bytes()
    except OSError:
        return None
    if hashlib.sha256(payload).hexdigest() == RUNNING_SUPERVISOR_SHA256:
        return None
    try:
        compile(payload, str(candidate), "exec")
    except (SyntaxError, ValueError) as error:
        log(f"canonical supervisor update is invalid; keeping current process: {error}")
        return None

    inherited = list(sys.argv[1:] if argv is None else argv)
    while "--adopt-pid" in inherited:
        index = inherited.index("--adopt-pid")
        del inherited[index : index + 2]
    if server_pid is not None:
        inherited.extend(("--adopt-pid", str(server_pid)))
    return [sys.executable, str(candidate), *inherited]


def reexec_updated_supervisor(
    server_pid: int | None, argv: list[str] | None = None
) -> None:
    command = updated_supervisor_command(server_pid, argv)
    if command is None:
        return
    log("canonical supervisor advanced; adopting the live game under fresh code")
    # Keep the deploy root stable across the hand-off. The re-exec runs the
    # canonical script out of the private build worktree, so without pinning it
    # the new process would recompute ROOT - and its log, runtime binary, and
    # source paths - from that worktree instead of the deployment.
    os.environ["CIVVIS_DEPLOY_ROOT"] = str(ROOT)
    # Release the single-instance lock before handing off. On Windows os.execv
    # spawns a fresh PID rather than replacing the image in place, so the
    # re-exec'd process would otherwise find this still-exiting one holding the
    # port lock, judge itself a duplicate, and exit - ending supervision on
    # every self-update.
    release_single_instance()
    os.execv(command[0], command)


def cargo_executable() -> str:
    """Find Cargo even under launchd/Task Scheduler's minimal environment."""
    configured = os.environ.get("CARGO")
    if configured:
        return configured
    discovered = shutil.which("cargo")
    if discovered:
        return discovered
    cargo_name = "cargo.exe" if os.name == "nt" else "cargo"
    return str(Path.home() / ".cargo" / "bin" / cargo_name)


def command(
    *args: str,
    check: bool = False,
    cwd: Path = ROOT,
    environment: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            args,
            cwd=cwd,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=check,
            env=environment,
            **_NO_WINDOW,
        )
    except OSError as error:
        # A missing build tool must fail this build attempt, not terminate the
        # long-running supervisor and strand the browser on its final frame.
        return subprocess.CompletedProcess(
            args, 127, stdout=f"{args[0]} unavailable: {error}\n"
        )


def sync_canonical_source() -> bool:
    """Reset the private build worktree to the canonical remote branch.

    The shared checkout is deliberately never merged, reset, cleaned, or built.
    Development sessions routinely leave it on feature branches with live edits;
    compiling those bytes is what made the macOS and Windows exhibitions differ.
    """
    target = f"{SYNC_REMOTE}/{SYNC_BRANCH}"
    fetched = command("git", "fetch", "--prune", SYNC_REMOTE, SYNC_BRANCH)
    if fetched.returncode != 0:
        log("update check could not reach the remote; trying the last fetched main")

    resolved = command("git", "rev-parse", "--verify", target)
    if resolved.returncode != 0:
        log(f"no cached {target} is available; keeping the verified runtime")
        return False

    if SOURCE_ROOT == ROOT:
        log("private source path resolves to the shared checkout; refusing to build it")
        return False

    if not (SOURCE_ROOT / ".git").exists():
        if SOURCE_ROOT.exists() and any(SOURCE_ROOT.iterdir()):
            log(f"private source path is occupied: {SOURCE_ROOT}")
            return False
        SOURCE_ROOT.parent.mkdir(parents=True, exist_ok=True)
        created = command(
            "git", "worktree", "add", "--detach", str(SOURCE_ROOT), target
        )
        if created.returncode != 0:
            log(f"could not create private source worktree at {SOURCE_ROOT}")
            return False

    source_top = command(
        "git", "rev-parse", "--show-toplevel", cwd=SOURCE_ROOT
    )
    if (
        source_top.returncode != 0
        or Path(source_top.stdout.strip()).resolve() != SOURCE_ROOT.resolve()
    ):
        log(f"private source path is not the expected Git worktree: {SOURCE_ROOT}")
        return False

    reset = command("git", "reset", "--hard", target, cwd=SOURCE_ROOT)
    cleaned = command(
        "git", "clean", "-fdx", "--", *RUNTIME_INPUTS, cwd=SOURCE_ROOT
    )
    if reset.returncode != 0 or cleaned.returncode != 0:
        log("could not reset the private source worktree; keeping the verified runtime")
        return False

    revision = command(
        "git", "rev-parse", "--short", "HEAD", check=True, cwd=SOURCE_ROOT
    ).stdout.strip()
    log(f"canonical source ready at {target} ({revision})")
    return True


def discard_retired_binaries() -> None:
    """Delete superseded runtimes once no game is still executing them."""
    for stale in RUNTIME_BINARY.parent.glob(RUNTIME_BINARY.name + ".retired*"):
        try:
            stale.unlink()
        except OSError:
            continue  # a game launched from it is still on screen


def retire_running_binary() -> Path:
    """Move the in-use runtime aside so its replacement can take the name."""
    for index in range(1, 100):
        candidate = RUNTIME_BINARY.with_name(f"{RUNTIME_BINARY.name}.retired{index}")
        if candidate.exists():
            continue
        os.replace(RUNTIME_BINARY, candidate)
        return candidate
    raise OSError(f"no free retirement slot beside {RUNTIME_BINARY}")


def promote_binary() -> None:
    """Atomically preserve a known-good build outside Cargo's output path."""
    RUNTIME_BINARY.parent.mkdir(parents=True, exist_ok=True)
    staged = RUNTIME_BINARY.with_suffix(RUNTIME_BINARY.suffix + ".new")
    build_binary = SOURCE_ROOT / "target" / "release" / BINARY_NAME
    shutil.copy2(build_binary, staged)
    try:
        os.replace(staged, RUNTIME_BINARY)
    except PermissionError:
        # Windows refuses to overwrite the image a running process was launched
        # from, but it does allow renaming that image: the live game keeps
        # executing the bytes it already opened and the next one starts on the
        # new build. Without this, promotion could only ever land while no game
        # was on screen - which, by design, is never, so every build compiled
        # during play was discarded and the display stayed on old code.
        discard_retired_binaries()
        retired = retire_running_binary()
        try:
            os.replace(staged, RUNTIME_BINARY)
        except OSError:
            os.replace(retired, RUNTIME_BINARY)
            raise
    discard_retired_binaries()


def source_snapshot() -> str:
    """Hash every input embedded in or compiled into the game binary."""
    files: list[Path] = []
    for relative in RUNTIME_INPUTS:
        path = SOURCE_ROOT / relative
        if path.is_file():
            files.append(path)
        elif path.is_dir():
            files.extend(candidate for candidate in path.rglob("*") if candidate.is_file())

    digest = hashlib.sha256()
    for path in sorted(
        files, key=lambda candidate: candidate.relative_to(SOURCE_ROOT).as_posix()
    ):
        relative = path.relative_to(SOURCE_ROOT).as_posix().encode()
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        digest.update(path.read_bytes())
    return digest.hexdigest()


def runtime_inputs_dirty() -> bool:
    """Whether files that can change the promoted binary differ from Git."""
    status = command(
        "git", "status", "--porcelain", "--", *RUNTIME_INPUTS, cwd=SOURCE_ROOT
    )
    return bool(status.stdout.strip())


def source_revision() -> str:
    return command(
        "git", "rev-parse", "--short", "HEAD", check=True, cwd=SOURCE_ROOT
    ).stdout.strip()


def write_runtime_metadata(snapshot: str) -> None:
    revision = source_revision()
    dirty = runtime_inputs_dirty()
    metadata = {
        "revision": revision,
        "embedded_revision": revision,
        "dirty": dirty,
        "source_snapshot": snapshot,
        "binary_sha256": hashlib.sha256(RUNTIME_BINARY.read_bytes()).hexdigest(),
        "built_at": datetime.now(timezone.utc).isoformat(),
    }
    write_runtime_metadata_file(metadata)
    log(f"build ready at {revision}{' + local edits' if dirty else ''}")


def write_runtime_metadata_file(metadata: dict[str, Any]) -> None:
    """Atomically replace the promoted runtime's provenance record."""
    staged = RUNTIME_METADATA.with_suffix(".json.new")
    staged.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    os.replace(staged, RUNTIME_METADATA)


def refresh_runtime_metadata(snapshot: str) -> None:
    """Refresh Git identity when an exact binary needs no recompilation.

    A supervisor can promote local source and then see those identical bytes
    committed while the match is running. The binary remains exact, but its
    revision and dirty flag should follow the now-stable Git identity without
    pretending that it was rebuilt.
    """
    try:
        metadata = json.loads(RUNTIME_METADATA.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return
    revision = command(
        "git", "rev-parse", "--short", "HEAD", check=True, cwd=SOURCE_ROOT
    ).stdout.strip()
    dirty = runtime_inputs_dirty()
    current_identity = {
        "revision": revision,
        "dirty": dirty,
        "source_snapshot": snapshot,
        "binary_sha256": hashlib.sha256(RUNTIME_BINARY.read_bytes()).hexdigest(),
    }
    if all(metadata.get(key) == value for key, value in current_identity.items()):
        return
    metadata.update(current_identity)
    write_runtime_metadata_file(metadata)
    log(
        f"refreshed exact build metadata at {revision}"
        f"{' + local edits' if dirty else ''}"
    )


def runtime_matches(snapshot: str) -> bool:
    """Return whether the promoted binary and its stamp match this source."""
    if not RUNTIME_BINARY.is_file() or not RUNTIME_METADATA.is_file():
        return False
    try:
        metadata = json.loads(RUNTIME_METADATA.read_text(encoding="utf-8"))
        binary_hash = hashlib.sha256(RUNTIME_BINARY.read_bytes()).hexdigest()
    except (OSError, ValueError):
        return False
    matches = (
        metadata.get("source_snapshot") == snapshot
        and metadata.get("binary_sha256") == binary_hash
        and metadata.get("embedded_revision") == source_revision()
    )
    return matches


def promoted_runtime_id() -> str | None:
    """Return the immutable identity of the currently promoted binary image.

    Two commits can have the same runtime-input snapshot while still produce
    different executables because the build embeds ``CIVVIS_COMMIT`` for the
    live ``/status`` endpoint. Using the source snapshot here made the
    supervisor skip the checkpoint restart for that newer promoted image and
    left production reporting the previous commit until the game ended.
    """
    try:
        metadata = json.loads(RUNTIME_METADATA.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    identity = metadata.get("binary_sha256")
    return identity if isinstance(identity, str) and identity else None


def runtime_replacement_pending(
    running_runtime_id: str | None, promoted_runtime: str | None
) -> bool:
    """Whether a running process predates the latest verified promotion."""
    return promoted_runtime is not None and running_runtime_id != promoted_runtime


def build_latest(max_attempts: int = 3) -> bool:
    """Build a stable snapshot; never promote an already-obsolete build."""
    for attempt in range(1, max_attempts + 1):
        before = source_snapshot()
        if runtime_matches(before):
            refresh_runtime_metadata(before)
            log("known-good spectator build already matches the latest worktree")
            return True
        log(f"building the latest worktree (attempt {attempt}/{max_attempts})")
        revision = source_revision()
        build_environment = os.environ.copy()
        build_environment["CIVVIS_COMMIT"] = revision
        # The visible game does not need to wait for unrelated evaluation
        # binaries to link before its known-good runtime can be promoted.
        result = command(
            cargo_executable(),
            "build",
            "--release",
            "--bin",
            "civvis",
            cwd=SOURCE_ROOT,
            environment=build_environment,
        )
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
    if not sync_canonical_source():
        return False
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
    """Fetch canonical main and keep its promoted fallback current."""
    return prepare_latest_once()


def start_background_prebuild() -> subprocess.Popen[str]:
    """Compile in a separate process so winner polling never waits on Cargo."""
    return subprocess.Popen(
        [sys.executable, str(Path(__file__).resolve()), "--prepare-once"],
        cwd=ROOT,
        text=True,
        start_new_session=os.name != "nt",
        **_NO_WINDOW,
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


def read_state(port: int, timeout: float = 5.0) -> dict[str, Any] | None:
    """Read the full observation, which can take seconds in a late large game.

    The generic JSON probe stays fast, but the supervisor must not classify a
    valid 8-player observation as unavailable merely because serialization is
    slower than a lightweight health response.
    """
    return read_json(port, "/state", timeout)


def set_spectator_pause(
    port: int, paused: bool, timeout: float = 5.0
) -> dict[str, Any] | None:
    """Restore the viewer's explicit pause after replacing a server process."""
    try:
        request = Request(
            f"http://127.0.0.1:{port}/pace",
            data=json.dumps({"paused": paused}).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urlopen(request, timeout=timeout) as response:
            value = json.load(response)
            return value if isinstance(value, dict) else None
    except (OSError, URLError, ValueError):
        return None


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


def resumed_checkpoint(state: dict[str, Any], marker: tuple[Any, ...] | None) -> bool:
    """Recognize a resume even if Lightning pace advanced before readiness."""
    return (
        marker is not None
        and state.get("seed") == marker[0]
        and state.get("winner") is None
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


def wait_for_successor(
    port: int,
    finished_instance: Any,
    finished_seed: Any,
    timeout: float = 1.0,
) -> dict[str, Any] | None:
    """Give the server-owned cooldown restart a brief scheduling grace."""
    deadline = time.monotonic() + max(0.0, timeout)
    latest = read_state(port)
    while not successor_started(latest, finished_instance, finished_seed):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        time.sleep(min(0.05, remaining))
        latest = read_state(port)
    return latest


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
    """Carry the just-finished game's setup forward to the next binary.

    The seat counts come from the operator's flags, never from the observation.
    `/state` is fog-of-war trimmed -- with a civilization selected in "View as"
    it carries only the players that civilization can see -- so counting the
    majors in it and feeding that back as the next `--players` ratchets the
    exhibition down and never recovers: a six-player match was observed
    restarting as four, then two, with each smaller game re-confirming the
    smaller count. Board *shape* may follow the finished game; how many seats
    it has may not.
    """
    players = state.get("players") or []
    game_map = state.get("map") or {}
    settings = {
        # Seat counts stay with the operator's flags; see the note above.
        "players": defaults["players"],
        "width": int(game_map.get("width") or defaults["width"]),
        "height": int(game_map.get("height") or defaults["height"]),
        "city_states": defaults["city_states"],
        "turns": int(state.get("max_turns") or defaults["turns"]),
        "map": game_map.get("script") or defaults["map"],
        "speed": state.get("game_speed") or defaults["speed"],
    }
    victory_conditions = state.get("victory_conditions")
    if isinstance(victory_conditions, dict):
        settings["victories"] = [
            name
            for name in (
                "science",
                "culture",
                "religious",
                "diplomatic",
                "domination",
                "score",
            )
            if victory_conditions.get(name) is True
        ]
    elif "victories" in defaults:
        settings["victories"] = list(defaults["victories"])
    return settings


def manual_new_game_request(
    state: dict[str, Any],
) -> tuple[str, dict[str, Any]] | None:
    """Return a normalized manual restart request emitted by the live server."""
    request = state.get("supervisor_request")
    if not isinstance(request, dict):
        return None
    mode = request.get("mode")
    values = request.get("settings")
    if (
        mode not in ("restart", "fresh_code")
        or request.get("server_instance") != state.get("server_instance")
        or not isinstance(values, dict)
    ):
        return None
    try:
        settings = {
            "players": int(values["players"]),
            "width": int(values["width"]),
            "height": int(values["height"]),
            "city_states": int(values["city_states"]),
            "turns": int(values["turns"]),
            "map": str(values["map"]),
            "speed": str(values["speed"]),
            "victories": [str(value) for value in values["victories"]],
        }
    except (KeyError, TypeError, ValueError):
        return None
    return mode, settings


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


# Supervisor-level policy, set once from --league in main(). Deliberately NOT
# part of the per-game settings dict: session_settings() and manual restart
# requests rebuild that dict from the finished game's state, which silently
# dropped the key and unrated every game after the first victory boundary.
LEAGUE_SPEC = "auto"
LEAGUE_RECORD = True


def league_dir(spec: str) -> tuple[Path, bool] | None:
    """Resolve the --league setting to (directory holding league.json, record).

    Resolved at every spawn, not at startup, because in 'auto' mode the
    canonical source worktree may only gain data/league once it syncs a
    commit that ships the snapshot.

    'auto' hands back a *runtime* copy of that snapshot and asks the server to
    rate its games into it. data/league is a committed file: writing results
    straight back would leave every checkout permanently dirty, so the shipped
    roster stays the starting position and the live table accumulates beside
    it, under the repo-root league/ path .gitignore already reserves for
    exactly this. Delete that directory to start again from the snapshot.
    An explicitly named directory is left read-only unless the operator asks
    for recording, since only they know whether it is disposable.
    """
    if spec == "off":
        return None
    if spec != "auto":
        candidate = Path(spec).expanduser().resolve()
        if not (candidate / "league.json").exists():
            return None
        return candidate, LEAGUE_RECORD
    snapshot = SOURCE_ROOT / "data" / "league" / "league.json"
    if not snapshot.exists():
        return None
    if not LEAGUE_RECORD:
        return snapshot.parent, False
    live = SOURCE_ROOT / "league"
    if not (live / "league.json").exists():
        live.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(snapshot, live / "league.json")
        log(f"seeded the live rating table at {live} from {snapshot}")
    return live, True


def server_command(
    port: int,
    settings: dict[str, Any],
    open_browser: bool,
    resume: Path | None = None,
) -> list[str]:
    args = [
        str(
            RUNTIME_BINARY
            if RUNTIME_BINARY.exists()
            else SOURCE_ROOT / "target" / "release" / BINARY_NAME
        ),
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
    roster = league_dir(LEAGUE_SPEC)
    if roster is not None:
        directory, record = roster
        # Absolute path: the server runs from the runtime directory, not ROOT.
        args.extend(("--league", str(directory)))
        if record:
            args.append("--league-record")
    if "victories" in settings:
        args.extend(("--victories", ",".join(settings["victories"])))
    if not open_browser:
        args.append("--no-open")
    return args


def start_server(
    port: int,
    settings: dict[str, int],
    open_browser: bool,
    resume: Path | None = None,
) -> subprocess.Popen[str]:
    # The server prefers loose web/ files in its working directory over the
    # page embedded in the executable. Starting in the shared checkout would
    # pair a canonical engine with whichever UI a development session is
    # editing, recreating the cross-machine mismatch at the presentation layer.
    process = subprocess.Popen(
        server_command(port, settings, open_browser, resume),
        cwd=RUNTIME_BINARY.parent,
        text=True,
        **_NO_WINDOW,
    )
    detail = f", resuming {resume.name}" if resume is not None else ""
    log(f"started PID {process.pid} on port {port} ({settings['players']} players{detail})")
    return process


def windows_process_handle(pid: int, access: int = 0x1000) -> Any | None:
    """Open a Windows process for querying, or None when that is not possible.

    The default access is PROCESS_QUERY_LIMITED_INFORMATION, which a process
    may open on any other process of the same user.
    """
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    kernel32.OpenProcess.restype = wintypes.HANDLE
    handle = kernel32.OpenProcess(access, False, pid)
    return handle or None


def windows_cpu_seconds(pid: int) -> float | None:
    """Total kernel plus user CPU time a Windows process has consumed."""
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    handle = windows_process_handle(pid)
    if handle is None:
        return None
    try:
        creation, exited = wintypes.FILETIME(), wintypes.FILETIME()
        kernel, user = wintypes.FILETIME(), wintypes.FILETIME()
        if not kernel32.GetProcessTimes(
            handle,
            ctypes.byref(creation),
            ctypes.byref(exited),
            ctypes.byref(kernel),
            ctypes.byref(user),
        ):
            return None
    finally:
        kernel32.CloseHandle(handle)
    # FILETIME counts 100-nanosecond intervals.
    ticks = sum(
        (stamp.dwHighDateTime << 32) | stamp.dwLowDateTime for stamp in (kernel, user)
    )
    return ticks / 10_000_000


def pid_alive(pid: int) -> bool:
    """Liveness of a process this supervisor may not have spawned."""
    if os.name != "nt":
        try:
            os.kill(pid, 0)
            return True
        except OSError:
            return False
    # os.kill(pid, 0) is not a probe on Windows: it opens the target with
    # PROCESS_ALL_ACCESS and calls TerminateProcess. That open is denied for a
    # server this process did not create, so an adopting successor read every
    # live game as dead and exited - ending supervision on each self-update.
    handle = windows_process_handle(pid)
    if handle is None:
        return False
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    try:
        code = wintypes.DWORD()
        if not kernel32.GetExitCodeProcess(handle, ctypes.byref(code)):
            return False
        return code.value == 259  # STILL_ACTIVE
    finally:
        kernel32.CloseHandle(handle)


def process_cpu_percent(pid: int, window: float = 0.25) -> float | None:
    """Recent CPU share of a process, or None when it cannot be measured."""
    if os.name != "nt":
        result = command("ps", "-o", "%cpu=", "-p", str(pid))
        if result.returncode != 0:
            return None
        try:
            return float(result.stdout.strip())
        except ValueError:
            return None
    # Windows exposes cumulative CPU time rather than a rate, so sample twice.
    before = windows_cpu_seconds(pid)
    if before is None:
        return None
    time.sleep(window)
    after = windows_cpu_seconds(pid)
    if after is None:
        return None
    return max(0.0, (after - before) / window * 100.0)


def pid_listening_on(port: int) -> int | None:
    """PID of the process already serving a port, when one can be identified."""
    if os.name == "nt":
        result = command("netstat", "-ano", "-p", "tcp")
        if result.returncode != 0:
            return None
        for line in result.stdout.splitlines():
            fields = line.split()
            if len(fields) < 5 or fields[3].upper() != "LISTENING":
                continue
            if fields[1].rsplit(":", 1)[-1] != str(port):
                continue
            try:
                return int(fields[4])
            except ValueError:
                return None
        return None
    result = command("lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-t")
    if result.returncode != 0:
        return None
    for line in result.stdout.split():
        try:
            return int(line)
        except ValueError:
            return None
    return None


def process_alive(process: subprocess.Popen[str] | None, adopted_pid: int | None) -> bool:
    if process is not None:
        return process.poll() is None
    if adopted_pid is None:
        return False
    return pid_alive(adopted_pid)


def process_busy(
    process: subprocess.Popen[str] | None,
    adopted_pid: int | None,
    threshold: float = 1.0,
) -> bool:
    """Best-effort check that an unavailable server is still computing."""
    pid = process.pid if process is not None else adopted_pid
    if pid is None:
        return False
    percent = process_cpu_percent(pid)
    return percent is not None and percent >= threshold


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

def signal_pid(pid: int, force: bool) -> bool:
    """Ask a process to stop; report whether the request could be delivered."""
    if os.name == "nt":
        # Windows has neither SIGTERM nor SIGKILL, and the os.kill() path here
        # raised for any process this supervisor did not spawn - so a retiring
        # server was left holding the port and its successor could not bind.
        result = command("taskkill", "/PID", str(pid), "/T", "/F")
        return result.returncode == 0
    try:
        os.kill(pid, signal.SIGKILL if force else signal.SIGTERM)
        return True
    except OSError:
        return False


def stop_server(process: subprocess.Popen[str] | None, adopted_pid: int | None) -> None:
    pid = process.pid if process is not None else adopted_pid
    if pid is None:
        return
    if process is not None:
        process.terminate()
    elif not signal_pid(pid, force=False):
        return
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if process is not None:
            if process.poll() is not None:
                return
        elif not pid_alive(pid):
            return
        time.sleep(0.1)
    if process is not None:
        process.kill()
    else:
        signal_pid(pid, force=True)


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
        "--victories",
        type=parse_victories,
        help=(
            "comma-separated enabled victories: science, culture, religious, "
            "diplomatic, domination, score"
        ),
    )
    parser.add_argument(
        "--league",
        default="auto",
        help=(
            "league directory for rated seating and the elo HUD: 'auto' uses "
            "the canonical source's data/league when present, 'off' disables, "
            "anything else is a directory containing league.json"
        ),
    )
    parser.add_argument(
        "--no-league-record",
        dest="league_record",
        action="store_false",
        help=(
            "seat rated players but leave their ratings alone. By default "
            "every finished game is rated: 'auto' accumulates into a runtime "
            "copy of the shipped roster, a named directory is written in place"
        ),
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


def parse_victories(value: str) -> list[str]:
    allowed = {
        "science",
        "culture",
        "religious",
        "diplomatic",
        "domination",
        "score",
    }
    victories = [name.strip() for name in value.split(",") if name.strip()]
    unknown = sorted(set(victories) - allowed)
    if not victories or unknown:
        detail = (
            "at least one victory is required"
            if not victories
            else f"unknown: {', '.join(unknown)}"
        )
        raise argparse.ArgumentTypeError(detail)
    return victories


# Lock handles kept open for the process lifetime, one per owned port.
_INSTANCE_LOCKS: dict[int, Any] = {}


def acquire_single_instance(port: int) -> bool:
    """Best-effort guard: at most one supervisor process per port per machine.

    A scheduled relaunch, or a repeat trigger firing during the brief window
    when a self-updating supervisor re-execs itself (on Windows os.execv spawns
    a fresh PID rather than replacing the image, so the launcher can believe the
    task has ended), must not start a second supervisor that would fight over
    the same port. Returns False only when a *different* live process already
    holds it; re-acquiring a port this process already owns is idempotent.
    """
    if port in _INSTANCE_LOCKS:
        return True
    lock_path = Path(tempfile.gettempdir()) / f"civvis-spectator-{port}.lock"
    try:
        handle = open(lock_path, "a+")
    except OSError:
        # If the lock file cannot even be created, do not block startup - the
        # visible game matters more than a theoretical double-launch.
        return True
    try:
        if os.name == "nt":
            import msvcrt

            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl

            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        handle.close()
        return False
    _INSTANCE_LOCKS[port] = handle
    return True


def release_single_instance() -> None:
    """Drop any held port locks so a re-exec of this process can re-acquire."""
    for handle in _INSTANCE_LOCKS.values():
        try:
            handle.close()
        except OSError:
            pass
    _INSTANCE_LOCKS.clear()


def main() -> int:
    args = parse_args()
    if getattr(args, "prepare_once", False):
        try:
            return 0 if prepare_latest_once() else 1
        except Exception:
            # This worker runs under pythonw, which discards stderr, so an
            # unhandled failure here used to end the build with no trace at all
            # while the supervisor kept restarting it.
            log("background build worker failed:\n" + traceback.format_exc())
            return 1
    if not acquire_single_instance(args.port):
        log(f"another supervisor already owns port {args.port}; exiting")
        return 0
    settings = {
        "players": args.players,
        "width": args.width,
        "height": args.height,
        "city_states": args.city_states,
        "turns": args.turns,
        "map": args.map,
        "speed": args.speed,
    }
    if getattr(args, "victories", None):
        settings["victories"] = list(args.victories)
    global LEAGUE_SPEC, LEAGUE_RECORD
    LEAGUE_SPEC = getattr(args, "league", "auto")
    LEAGUE_RECORD = getattr(args, "league_record", True)
    process: subprocess.Popen[str] | None = None
    adopted_pid = args.adopt_pid
    if adopted_pid is None:
        # A service manager can start a supervisor while an earlier game is
        # still serving the port - after a supervisor crash, or on a scheduled
        # recovery sweep. Starting a rival server there only loses the bind and
        # leaves the game on screen unmanaged, so take that game over instead.
        listener = pid_listening_on(args.port)
        if listener is not None:
            if read_state(args.port) is not None:
                adopted_pid = listener
                log(f"a game already owns port {args.port}; taking it over")
            else:
                log(f"port {args.port} is held by PID {listener}, which is not a game")
    # An adopted binary cannot be proven current, so replace it from a safe
    # checkpoint as soon as a verified runtime is available.
    running_runtime_id: str | None = None
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
    busy_reported = False
    busy_check_at = 0.0
    busy_until = 0.0
    refresh_pending = False
    refresh_at = 0.0
    source_check_at = 0.0
    prebuild_process: subprocess.Popen[str] | None = None

    def launch_recovery(
        open_browser: bool = False, preserve_pause: bool = False
    ) -> dict[str, Any]:
        nonlocal process, adopted_pid, running_runtime_id
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
        launch_runtime_id = promoted_runtime_id()
        process = start_server(args.port, settings, open_browser, resume)
        running_runtime_id = launch_runtime_id
        try:
            recovered = wait_for_server(args.port, process)
        except RuntimeError:
            if resume is None:
                raise
            log("checkpoint could not be loaded; quarantining it and starting a fresh game")
            stop_server(process, None)
            quarantine_checkpoint(save_path)
            launch_runtime_id = promoted_runtime_id()
            process = start_server(args.port, settings, open_browser)
            running_runtime_id = launch_runtime_id
            recovered = wait_for_server(args.port, process)
            marker = None

        if preserve_pause:
            recovered = set_spectator_pause(args.port, True) or recovered

        if resumed_checkpoint(recovered, marker):
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
                reexec_updated_supervisor(None)
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
        refresh_pending = not runtime_matches(source_snapshot()) or runtime_replacement_pending(
            running_runtime_id, promoted_runtime_id()
        )
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

            manual_request = manual_new_game_request(state)
            if manual_request is not None:
                mode, requested_settings = manual_request
                if mode == "fresh_code":
                    snapshot = source_snapshot()
                    latest_ready = runtime_matches(snapshot)
                    if prebuild_process is not None and prebuild_process.poll() is not None:
                        prebuild_process = None
                        reexec_updated_supervisor(
                            process.pid if process is not None else adopted_pid
                        )
                        snapshot = source_snapshot()
                        latest_ready = runtime_matches(snapshot)
                    if latest_ready:
                        log("fresh-code build is ready; starting a new simulation")
                    else:
                        if prebuild_process is None:
                            log("fresh-code sim requested; rebuilding in the background")
                            prebuild_process = start_background_prebuild()
                        log(
                            "fresh code is not ready; starting the verified fallback "
                            "immediately and refreshing it during play"
                        )
                else:
                    log("restart requested; starting a new simulation on existing code")

                preserve_pause = bool(state.get("spectator_paused"))
                stop_server(process, adopted_pid)
                process = None
                adopted_pid = None
                try:
                    save_path.unlink()
                except FileNotFoundError:
                    pass
                settings = requested_settings
                launch_runtime_id = promoted_runtime_id()
                process = start_server(args.port, settings, False)
                running_runtime_id = launch_runtime_id
                state = wait_for_server(args.port, process)
                if preserve_pause:
                    state = set_spectator_pause(args.port, True) or state
                unavailable_since = None
                last_progress = progress_marker(state)
                progress_at = time.monotonic()
                checkpointed_progress = None
                checkpoint_at = 0.0
                refresh_pending = not runtime_matches(source_snapshot())
                refresh_at = time.monotonic()
                source_check_at = time.monotonic() + max(1.0, args.source_check_interval)
                continue

            settings = session_settings(state, settings)
            if state.get("winner") is None:
                finished_key = None
                now = time.monotonic()
                if prebuild_process is not None and prebuild_process.poll() is not None:
                    build_failed = prebuild_process.returncode != 0
                    if build_failed:
                        log(
                            "background build worker exited with "
                            f"{prebuild_process.returncode}; retrying later"
                        )
                    prebuild_process = None
                    reexec_updated_supervisor(
                        process.pid if process is not None else adopted_pid
                    )
                    if runtime_replacement_pending(
                        running_runtime_id, promoted_runtime_id()
                    ):
                        refresh_pending = True
                        # A worker that cannot produce a runtime must not be
                        # restarted on the next poll: that spins a build attempt
                        # several times a second for as long as it keeps failing.
                        refresh_at = now + (
                            max(0.1, args.build_retry) if build_failed else 0.0
                        )
                        log(
                            "canonical source advanced; scheduling a safe live refresh"
                        )
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
                        state = launch_recovery(
                            preserve_pause=bool(state.get("spectator_paused"))
                        )
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
                    if prebuild_process is None:
                        log("checking canonical source for a newer runtime")
                        prebuild_process = start_background_prebuild()
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
            launch_runtime_id = promoted_runtime_id()
            process = start_server(args.port, settings, False)
            running_runtime_id = launch_runtime_id
            preserve_pause = bool(state.get("spectator_paused"))
            state = wait_for_server(args.port, process)
            if preserve_pause:
                state = set_spectator_pause(args.port, True) or state
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
