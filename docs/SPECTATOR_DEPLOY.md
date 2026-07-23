# Running the spectator loop as an always-on visual test

`tools/spectator_supervisor.py` is the production visual test: it plays the
latest `origin/main` build on screen, one full game after another, and rebuilds
the newest code *while the current game is still running* so the next game is
always on the freshest binary that compiled. It is one cross-platform program;
only the way you keep it alive differs per OS.

## What the loop does

1. Builds `origin/main` in a **private worktree** (never the shared checkout, so
   a session's uncommitted edits never reach the screen) and promotes a
   known-good binary.
2. Serves that binary in `--spectate --supervised` mode; the game auto-steps to
   a decision.
3. **During** the game it fetches `origin/main` and, if it moved, compiles it in
   the background — so the boundary between games never waits on `cargo`. This
   is the "always one game up of the last build" property.
4. When a winner appears it archives the result, keeps it on screen for
   `--cooldown` seconds (default 5), then deals the next game on the freshest
   build it has. If that build isn't ready, it starts immediately on the last
   verified one and swaps in the fresh code at the *next* boundary — the loop
   never stalls on a slow or broken build.
5. Crash/stall recovery: active games are checkpointed every few seconds and
   resumed; a wedged game is nudged, then quarantined rather than looped on.

The supervisor also updates **itself**: when `tools/spectator_supervisor.py`
changes on `origin/main` it re-execs the canonical script under the live game
(`os.execv`), so a machine set up once tracks fleet development with no manual
step.

Watch any host with:  `tail -f <deploy-checkout>/spectator-supervisor.log`

## Deploy checkout

Run the supervisor from a **dedicated checkout pinned to `origin/main`**, not the
multi-session shared checkout:

```bash
git -C <shared-checkout> worktree add --detach <...>/civvis-spectator origin/main
```

`ROOT` (where it fetches and stages the runtime) then derives from that path;
the private build worktree is `<...>/civvis-spectator-spectator-src`.

## Windows (Task Scheduler)

```powershell
powershell -ExecutionPolicy Bypass -File deploy\register_spectator_task.ps1 `
    -DeployRoot C:\Users\<you>\PycharmProjects\civvis-spectator
Start-ScheduledTask -TaskPath '\Martbot\' -TaskName 'Civvis Spectator'
```

The task runs `pythonw.exe` (windowless); the supervisor gives every child
process `CREATE_NO_WINDOW`, so nothing ever flashes a console — the operator
terminal policy is honored. A per-port lock inside the supervisor guarantees a
single instance even across a self-update, so the logon + 5-minute recovery
triggers can fire freely.

## macOS / Linux (launchd)

```bash
cp deploy/com.civvis.spectator.plist ~/Library/LaunchAgents/
#  edit the four __PLACEHOLDER__ paths (python3, deploy checkout, home)
launchctl load -w ~/Library/LaunchAgents/com.civvis.spectator.plist
```

`KeepAlive` relaunches on exit; `RunAtLoad` starts it at login. On macOS
`os.execv` keeps the same PID, so self-updates are invisible to launchd.
On a bare Linux host, `nohup python3 tools/spectator_supervisor.py … &` under a
systemd unit or a `while` wrapper works the same way.

## Tuning

`--players --width --height --city-states --turns --map --speed` size the game;
`--cooldown` is the seconds the finished result stays on screen before the next
game (the "~5–10s between games"). `--port` defaults to 8766; the fleet's watched
exhibition runs on **8765**. Shorter games rotate builds onto the screen more
often, which makes a better production heartbeat.
