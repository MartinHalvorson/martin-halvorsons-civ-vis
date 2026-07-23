#!/usr/bin/env bash
# exhibition.sh - the POSIX twin of exhibition.ps1, for macOS and Linux hosts.
#
# Same loop, same decisions: revive the server if it stops listening, build
# origin/main in a private worktree the moment the staged binary falls behind
# it, and between games either swap onto that build or deal the next game in
# place. Something is always playing, and it is always the newest code that
# compiled.
#
#   ./exhibition.sh                              # foreground
#   nohup ./exhibition.sh >/dev/null 2>&1 &      # background
#
# Env: PORT (default 8765), POLL_SEC (default 10), CARGO (default
# ~/.cargo/bin/cargo), EVOLVE_THREADS (default 8).
set -uo pipefail

PORT="${PORT:-8765}"
POLL_SEC="${POLL_SEC:-10}"
EVOLVE_THREADS="${EVOLVE_THREADS:-8}"
# A freshly launched server generates its map before it binds the port. Judged
# faster than that, the revive check below kills it and starts another, which
# never finishes either - a loop that shows a black screen rather than a game.
LAUNCH_GRACE_SEC="${LAUNCH_GRACE_SEC:-45}"

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$repo"
cargo_bin="${CARGO:-$HOME/.cargo/bin/cargo}"
log="$repo/exhibition-supervisor.log"
bin_run="$repo/bin-run"
stamp="$bin_run/built.commit"
# The exhibition builds from its own detached worktree, never from the shared
# checkout: sessions have work in progress there most of the day, and building
# out of it means compiling someone's half-finished edit or, with a dirty-tree
# guard, never compiling at all.
src="$(dirname "$repo")/civvis-exhibition-src"
mkdir -p "$bin_run" "$repo/evolved"
last_launch=0

say() { printf '%s %s\n' "$(date '+%m-%d %H:%M:%S')" "$1" >>"$log"; }

# One supervisor per port. Two of them fight: both build, both stop the server
# mid-swap, and the log stops meaning anything. The lock is released when this
# process dies, so a killed supervisor leaves the name free.
lock="${TMPDIR:-/tmp}/civvis-exhibition-$PORT.lock"
exec 9>"$lock" || exit 0
if command -v flock >/dev/null 2>&1; then
    flock -n 9 || exit 0
fi

# The server prefers web/index.html and web/assets/* from its working directory
# and falls back to the copies compiled into the binary. Running it from the
# shared checkout therefore serves whatever index.html is sitting there, while
# the engine underneath came from origin/main. Running it from bin-run, which
# has no web/ directory, pins the GUI to the commit the binary was built from.
start_gui() {
    ( cd "$bin_run" && nohup ./civvis-gui play --spectate --no-open --port "$PORT" \
        >"$repo/civvis-play.log" 2>"$repo/civvis-play.err.log" & )
    last_launch=$(date +%s)
    say "gui launched on :$PORT"
}

start_evolve() {
    ( cd "$repo" && nohup "$bin_run/civvis-evolve" evolve --threads "$EVOLVE_THREADS" \
        --pop 16 --games 8 --turns 160 \
        >"$repo/evolved/evolve.log" 2>"$repo/evolved/evolve.err.log" & )
    say "evolve launched"
}

promote_staged() {
    [ -f "$bin_run/civvis-next" ] || return 1
    cp -f "$bin_run/civvis-next" "$bin_run/civvis-gui"
    cp -f "$bin_run/civvis-next" "$bin_run/civvis-evolve"
    rm -f "$bin_run/civvis-next"
}

# POST /new inherits the session's settings, spectate included - but also its
# seed, so a fresh one goes with the request or every game replays the last.
deal_next_game() {
    if curl -fsS -m 20 -X POST -H 'Content-Type: application/json' \
        -d "{\"seed\": $(( (RANDOM << 15 | RANDOM) + 1 ))}" \
        "http://localhost:$PORT/new" >/dev/null 2>&1; then
        say "dealt the next game in place"
    else
        say "could not deal a new game"
    fi
}

if [ ! -f "$src/Cargo.toml" ]; then
    if git -C "$repo" worktree add --detach "$src" origin/main >/dev/null 2>&1; then
        say "created build worktree at $src"
    else
        say "could not create build worktree at $src"
    fi
fi

say "supervisor started (port $PORT, poll ${POLL_SEC}s)"
while true; do
    # 1. revive anything that died. A server process that is alive but no
    #    longer listening still holds its exe open, so clear the corpse before
    #    promoting a new binary over it.
    settling=0
    [ $(( $(date +%s) - last_launch )) -lt "$LAUNCH_GRACE_SEC" ] && settling=1
    if ! curl -fsS -m 5 "http://localhost:$PORT/state" >/dev/null 2>&1 && [ "$settling" -eq 0 ]; then
        pkill -f "civvis-gui play" >/dev/null 2>&1 || true
        sleep 1
        promote_staged || true
        [ -x "$bin_run/civvis-gui" ] && start_gui
    fi
    pgrep -f "civvis-evolve evolve" >/dev/null 2>&1 || {
        [ -x "$bin_run/civvis-evolve" ] && start_evolve
    }

    # 2. staged binary older than origin/main? build it. The test is the commit
    #    the last build came from, not "did a fetch move something this round":
    #    a build cut short by a restart, or a failed build, otherwise leaves the
    #    exhibition on the previous commit with nothing left to trigger a retry.
    if [ -f "$src/Cargo.toml" ]; then
        git -C "$src" fetch -q origin main 2>/dev/null || true
        head="$(git -C "$src" rev-parse FETCH_HEAD 2>/dev/null || true)"
        built="$( [ -f "$stamp" ] && tr -d '[:space:]' <"$stamp" || true )"
        if [ -n "$head" ] && [ "$head" != "$built" ]; then
            short="${head:0:7}"
            git -C "$src" reset -q --hard "$head" 2>/dev/null || true
            say "building $short"
            started=$(date +%s)
            if CARGO_TARGET_DIR="$src/target" "$cargo_bin" build --release \
                --manifest-path "$src/Cargo.toml" >/dev/null 2>&1; then
                cp -f "$src/target/release/civvis" "$bin_run/civvis-next"
                printf '%s' "$head" >"$stamp"
                say "staged $short in $(( $(date +%s) - started ))s"
            else
                # Record nothing on failure, so the next round retries rather
                # than treating a broken commit as already built.
                say "build FAILED for $short"
            fi
        fi
    fi

    # 3. keep the shared checkout moving too, for the sessions working in it -
    #    but only when it is clean, and nothing above depends on it.
    if [ -z "$(git -C "$repo" status --porcelain --untracked-files=no)" ]; then
        if [ "$(git -C "$repo" rev-parse HEAD)" != "$(git -C "$repo" rev-parse origin/main)" ]; then
            git -C "$repo" pull --rebase -q 2>/dev/null || git -C "$repo" rebase --abort 2>/dev/null || true
        fi
    fi

    # 4. game over? start the next one - swapping onto a staged build here, in
    #    the between-games window, but starting the next game either way.
    state="$(curl -fsS -m 5 "http://localhost:$PORT/state" 2>/dev/null || true)"
    if [ -n "$state" ] && ! printf '%s' "$state" | grep -q '"winner":null'; then
        if [ -f "$bin_run/civvis-next" ]; then
            pkill -f "civvis-gui play" >/dev/null 2>&1 || true
            pkill -f "civvis-evolve evolve" >/dev/null 2>&1 || true
            sleep 1
            promote_staged
            start_gui
            start_evolve
            say "swapped to latest build between games"
        else
            deal_next_game
        fi
    fi
    sleep "$POLL_SEC"
done
