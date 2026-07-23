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
# Env: PORT (default 8765), POLL_SEC (default 3), GIT_SEC (default 30), CARGO (default
# ~/.cargo/bin/cargo), EVOLVE_THREADS (default 8).
set -uo pipefail

PORT="${PORT:-8765}"
# POLL_SEC paces the cheap checks - is the server up, has the game been decided.
# It is the whole gap between games: a decided game waits out one poll, and
# relaunching the server onto a fresh map measures about two seconds. Fetching
# origin that often would be pointless load, so git and build work runs on its
# own slower cadence, GIT_SEC.
POLL_SEC="${POLL_SEC:-3}"
GIT_SEC="${GIT_SEC:-30}"
last_git=0
EVOLVE_THREADS="${EVOLVE_THREADS:-8}"
# A freshly launched server generates its map before it binds the port. Judged
# faster than that, the revive check below kills it and starts another, which
# never finishes either - a loop that shows a black screen rather than a game.
LAUNCH_GRACE_SEC="${LAUNCH_GRACE_SEC:-12}"

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
# The running compiler, when there is one. Only ever one at a time.
build_pid=""
build_head=""
build_short=""
build_started=0

say() { printf '%s %s\n' "$(date '+%m-%d %H:%M:%S')" "$1" >>"$log"; }

# Compilers working on this exhibition's worktree, ours or left behind by a
# supervisor killed mid-build. A background build outlives the supervisor that
# started it, and a fresh supervisor knows nothing about it - so without this
# check it starts another on the same target directory, they fight over
# Cargo's build lock, and every build fails. pgrep never matches itself, so
# the pattern naming the path is safe here.
exhibition_cargo() { pgrep -f "cargo.*civvis-exhibition-src" 2>/dev/null || true; }

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

# Both launch paths are guarded on the binary being executable, so a copy that
# arrives without its mode bit leaves the screen dark and says nothing about
# why. Set it explicitly rather than trusting what cp carried over.
promote_staged() {
    [ -f "$bin_run/civvis-next" ] || return 1
    cp -f "$bin_run/civvis-next" "$bin_run/civvis-gui"
    cp -f "$bin_run/civvis-next" "$bin_run/civvis-evolve"
    chmod +x "$bin_run/civvis-gui" "$bin_run/civvis-evolve"
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

# Anything still compiling belongs to a supervisor that is gone - this one
# holds the lock - so clear it rather than let it collide with our first build.
stale="$(exhibition_cargo)"
if [ -n "$stale" ]; then
    echo "$stale" | xargs -r kill -9 2>/dev/null || true
    say "cleared build process(es) left by a previous supervisor"
fi

say "supervisor started (port $PORT, poll ${POLL_SEC}s, git every ${GIT_SEC}s)"
while true; do
    # 1. revive anything that died. A server process that is alive but no
    #    longer listening still holds its exe open, so clear the corpse before
    #    promoting a new binary over it.
    settling=0
    [ $(( $(date +%s) - last_launch )) -lt "$LAUNCH_GRACE_SEC" ] && settling=1
    if ! curl -fsS -m 5 "http://localhost:$PORT/status" >/dev/null 2>&1 && [ "$settling" -eq 0 ]; then
        pkill -f "civvis-gui play" >/dev/null 2>&1 || true
        sleep 1
        promote_staged || true
        [ -x "$bin_run/civvis-gui" ] && start_gui
    fi
    # Evolve is a CPU hog and only useful in the background. Starting it while
    # the server is still generating its map made the two compete for the
    # machine and stretched the changeover, so it waits until there is a game
    # for it to be in the background of.
    if curl -fsS -m 3 "http://localhost:$PORT/status" >/dev/null 2>&1; then
        pgrep -f "civvis-evolve evolve" >/dev/null 2>&1 || {
            [ -x "$bin_run/civvis-evolve" ] && start_evolve
        }
    fi

    # 2a. collect a finished build. This runs on the fast loop, not the git
    #     cadence, so a build that lands mid-game is staged and ready the
    #     moment the game ends rather than half a minute later.
    if [ -n "$build_pid" ] && ! kill -0 "$build_pid" 2>/dev/null; then
        if wait "$build_pid"; then
            cp -f "$src/target/release/civvis" "$bin_run/civvis-next"
            chmod +x "$bin_run/civvis-next"
            printf '%s' "$build_head" >"$stamp"
            say "staged $build_short in $(( $(date +%s) - build_started ))s"
        else
            # Record nothing on failure, so a later round retries rather than
            # treating a broken commit as already built. Carry the compiler's
            # own first complaint into the log: the next build overwrites
            # build.log, so "build FAILED" alone is unattributable by the time
            # anyone reads it.
            why="$(grep -m1 '^error' "$bin_run/build.log" 2>/dev/null || true)"
            say "build FAILED for $build_short${why:+: $why}"
        fi
        build_pid=""
    fi

    # 2. staged binary older than origin/main? build it. The test is the commit
    #    the last build came from, not "did a fetch move something this round":
    #    a build cut short by a restart, or a failed build, otherwise leaves the
    #    exhibition on the previous commit with nothing left to trigger a retry.
    did_git=0
    if [ -f "$src/Cargo.toml" ] && [ $(( $(date +%s) - last_git )) -ge "$GIT_SEC" ]; then
        last_git=$(date +%s)
        did_git=1
        git -C "$src" fetch -q origin main 2>/dev/null || true
        head="$(git -C "$src" rev-parse FETCH_HEAD 2>/dev/null || true)"
        built="$( [ -f "$stamp" ] && tr -d '[:space:]' <"$stamp" || true )"
        if [ -n "$head" ] && [ "$head" != "$built" ] && [ -z "$build_pid" ] &&
            [ -z "$(exhibition_cargo)" ]; then
            build_short="${head:0:7}"
            build_head="$head"
            git -C "$src" reset -q --hard "$head" 2>/dev/null || true
            say "building $build_short"
            build_started=$(date +%s)
            # Launched rather than run. A release build takes a minute or two,
            # and running it inline stops the whole loop for that long - so a
            # game that ends mid-build sits decided, or the server sits dead,
            # until the compiler finishes.
            ( CARGO_TARGET_DIR="$src/target" "$cargo_bin" build --release \
                --manifest-path "$src/Cargo.toml" >"$bin_run/build.log" 2>&1 ) &
            build_pid=$!
        fi
    fi

    # 3. keep the shared checkout moving too, for the sessions working in it -
    #    but only when it is clean, and nothing above depends on it. Shares the
    #    git cadence: the fast poll exists for the changeover, and dragging a
    #    status call through it every few seconds would only slow that down.
    if [ "$did_git" -eq 1 ] &&
        [ -z "$(git -C "$repo" status --porcelain --untracked-files=no)" ]; then
        if [ "$(git -C "$repo" rev-parse HEAD)" != "$(git -C "$repo" rev-parse origin/main)" ]; then
            git -C "$repo" merge --ff-only -q origin/main 2>/dev/null || \
                say "shared checkout is not a fast-forward of origin/main; preserving it"
        fi
    fi

    # 4. game over? start the next one - swapping onto a staged build here, in
    #    the between-games window, but starting the next game either way.
    # /status carries the winner alone; /state builds close to a megabyte of
    # observation JSON, and asking for that every few seconds to read one
    # field made the server stall for whole seconds under load.
    state="$(curl -fsS -m 5 "http://localhost:$PORT/status" 2>/dev/null || true)"
    # Older binaries predate /status; fall back so a swap onto one still finds
    # its way to the next game.
    [ -z "$state" ] && state="$(curl -fsS -m 5 "http://localhost:$PORT/state" 2>/dev/null || true)"
    if [ -n "$state" ] && ! printf '%s' "$state" | grep -q '"winner":null'; then
        if [ -f "$bin_run/civvis-next" ]; then
            pkill -f "civvis-gui play" >/dev/null 2>&1 || true
            pkill -f "civvis-evolve evolve" >/dev/null 2>&1 || true
            sleep 1
            promote_staged
            start_gui
            # Evolve is deliberately not restarted here; the revive check
            # brings it back once the server is listening, so the new game
            # gets the machine to itself while it generates its map.
            say "swapped to latest build between games"
        else
            deal_next_game
        fi
    fi
    sleep "$POLL_SEC"
done
