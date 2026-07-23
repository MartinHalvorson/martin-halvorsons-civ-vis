# exhibition.ps1 - keep the civvis spectate exhibition running on the latest
# code. Loop: revive dead processes; build origin/main in a private worktree
# the moment the staged binary falls behind it, so the binary is already
# waiting when the current game ends; then, between games, either swap onto
# that build or deal the next game in place. Something is always playing on
# screen, and it is always the newest code that compiled.
#
# Runs as the scheduled task "Civvis Exhibition" (hidden, retried every few
# minutes); the mutex below makes a second launch a no-op, so the task can
# fire freely and the exhibition heals itself after a kill or a reboot.
# By hand:     Start-Process powershell -WindowStyle Hidden -ArgumentList
#              "-ExecutionPolicy","Bypass","-File","exhibition.ps1"
# PollSec paces the cheap checks - is the server up, has the game been decided.
# It is the whole gap between games: a decided game waits out one poll, and
# relaunching the server onto a fresh map measures about two seconds. Fetching
# origin every few seconds to match would be pointless load, so the git and
# build work runs on its own slower cadence, GitSec.
param([int]$Port = 8765, [int]$PollSec = 3, [int]$GitSec = 30)

# One supervisor per port. Two of them fight: both build, both stop the gui
# mid-swap, and the log stops meaning anything. The kernel drops this handle
# when the process dies, so a killed supervisor leaves the name free.
$isNew = $false
$mutex = New-Object System.Threading.Mutex($true, "Global\civvis-exhibition-$Port", [ref]$isNew)
if (-not $isNew) { exit 0 }

$repo = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $repo
$cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe"
$log = "$repo\exhibition-supervisor.log"
$binRun = "$repo\bin-run"
$stamp = "$binRun\built.commit"
# The exhibition builds from its own detached worktree, never from the shared
# checkout. Sessions have work in progress in the checkout most of the day,
# and building out of it meant either compiling someone's half-finished edit
# or - with a dirty-tree guard - never compiling at all, which is how the
# screen used to sit on a build hours older than main.
$src = Join-Path (Split-Path -Parent $repo) 'civvis-exhibition-src'
New-Item -ItemType Directory -Force $binRun | Out-Null

function Log($msg) {
    Add-Content -Encoding utf8 $log "$(Get-Date -Format 'MM-dd HH:mm:ss') $msg"
}

# Compilers working on this exhibition's worktree, ours or left behind by a
# supervisor that was killed mid-build. A background build outlives the
# supervisor that started it, and a fresh supervisor knows nothing about it -
# so without this check it starts another on the same target directory, they
# fight over Cargo's build lock, and every build fails. That stranded the
# exhibition on a fifteen-minute-old commit through four different shas.
function Get-ExhibitionCargo {
    Get-CimInstance Win32_Process -Filter "Name='cargo.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -like "*civvis-exhibition-src*" }
}

# The server prefers web/index.html and web/assets/* from its working
# directory and falls back to the copies compiled into the binary. Running it
# from the shared checkout therefore served whatever index.html happened to be
# sitting there - an older commit, plus whatever a parallel session had
# half-edited - while the engine underneath came from origin/main. The
# exhibition looked different on every machine for that reason alone. Running
# it from bin-run, which has no web/ directory, pins the GUI to the exact
# commit the binary was built from, so the same build looks the same anywhere.
function Start-Gui {
    Start-Process -FilePath "$binRun\civvis-gui.exe" `
        -ArgumentList "play","--spectate","--no-open","--port","$Port" `
        -WorkingDirectory $binRun -WindowStyle Hidden `
        -RedirectStandardOutput "$repo\civvis-play.log" `
        -RedirectStandardError "$repo\civvis-play.err.log"
    Log "gui launched on :$Port"
}

function Start-Evolve {
    Start-Process -FilePath "$binRun\civvis-evolve.exe" `
        -ArgumentList "evolve","--threads","12","--pop","16","--games","8","--turns","160" `
        -WorkingDirectory $repo -WindowStyle Hidden `
        -RedirectStandardOutput "$repo\evolved\evolve.log" `
        -RedirectStandardError "$repo\evolved\evolve.err.log"
    Log "evolve launched"
}

# Windows keeps an exe locked for a moment after the process using it dies, so
# a promote right behind a Stop-Process can still be refused. Uncaught, that
# threw out of the whole iteration - taking the game-over check with it and
# leaving the exhibition dark until a later pass happened to succeed. Retry
# briefly instead, and let the caller carry on either way.
function Promote-Staged {
    # Evolve is a background training run holding its own exe open, and it is
    # restarted from whatever is on disk anyway - so stop it rather than let
    # it block the promote. Without this the gui copy succeeded, the evolve
    # copy threw, and the whole promote reported failure: the staged build was
    # left in place, and the next changeover took a full server restart to
    # apply a binary that was already running.
    Get-Process civvis-evolve -ErrorAction SilentlyContinue | Stop-Process -Force
    for ($attempt = 0; $attempt -lt 12; $attempt++) {
        try {
            Copy-Item "$binRun\civvis-next.exe" "$binRun\civvis-gui.exe" -Force -ErrorAction Stop
            Copy-Item "$binRun\civvis-next.exe" "$binRun\civvis-evolve.exe" -Force -ErrorAction Stop
            Remove-Item "$binRun\civvis-next.exe" -Force -ErrorAction SilentlyContinue
            return $true
        } catch {
            Start-Sleep -Milliseconds 250
        }
    }
    Log "staged build is still locked; leaving it for the next pass"
    return $false
}

# Deal a fresh game into the running server. POST /new inherits the current
# session's settings, spectate included - but also its seed, so a new one has
# to be supplied or every game would replay the last.
function Start-NextGame {
    $body = @{ seed = (Get-Random -Minimum 1 -Maximum 2000000000) } | ConvertTo-Json -Compress
    try {
        Invoke-RestMethod "http://localhost:$Port/new" -Method Post -Body $body `
            -ContentType "application/json" -TimeoutSec 20 | Out-Null
        Log "dealt the next game in place"
    } catch {
        Log "could not deal a new game: $_"
    }
}

if (-not (Test-Path "$src\Cargo.toml")) {
    git -C $repo worktree add --detach $src origin/main 2>$null | Out-Null
    if (Test-Path "$src\Cargo.toml") { Log "created build worktree at $src" }
    else { Log "could not create build worktree at $src" }
}

# Nothing in PowerShell reports a detached process's exit status reliably:
# Start-Process -PassThru hands back an ExitCode that stays wrong even after
# waiting on it, and reading the compiler's own output races the file still
# being flushed. Both mistakes were made here, and both recorded every
# successful build as broken - so the stamp was never written, the same commit
# rebuilt every cadence, and the exhibition sat on an old binary while the
# build log plainly read Finished. A script settles it: the shell writes the
# real exit code once the compiler is genuinely done, and that file is the
# verdict. It lives in a file rather than a cmd /c string because cmd strips
# the outer quotes off a command that both begins and ends with one.
# CIVVIS_COMMIT is read at compile time and reported by /status, so the
# running server can say which code it is rather than leaving it to be
# guessed from file timestamps. It is written per build, below.
$buildScript = "$binRun\build-once.cmd"
function Write-BuildScript($commit) {
    Set-Content -Path $buildScript -Encoding ascii -Value @(
        '@echo off',
        "set CARGO_TARGET_DIR=$src\target",
        "set CIVVIS_COMMIT=$commit",
        "`"$cargo`" build --release --manifest-path `"$src\Cargo.toml`" > `"$binRun\build.log`" 2>&1",
        "echo %ERRORLEVEL% > `"$binRun\build.code`""
    )
}
Write-BuildScript "unknown"

# Anything still compiling belongs to a supervisor that is gone: this one holds
# the mutex, so no other supervisor is alive to own it. Clear it out rather
# than letting it collide with the first build of this run.
$stale = @(Get-ExhibitionCargo)
if ($stale.Count -gt 0) {
    $stale | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Log "cleared $($stale.Count) build process(es) left by a previous supervisor"
}

Log "supervisor started (port $Port, poll ${PollSec}s, git every ${GitSec}s)"
$lastGit = [DateTime]::MinValue
$LaunchGraceSec = 12
$lastLaunch = (Get-Date).AddSeconds(-$LaunchGraceSec)
# The running compiler, when there is one. Only ever one at a time.
$build = $null
$buildHead = ""
$buildShort = ""
$buildStarted = Get-Date
while ($true) {
    try {
        # 1. revive anything that died. A gui process that is alive but no
        #    longer listening still holds a lock on its exe, and the promote
        #    below then throws every round: the copy fails, Start-Gui is never
        #    reached, and the exhibition stays dark for good. Clear the corpse
        #    first so reviving cannot get stuck behind it.
        #    A server generates its map before it binds, which measures about
        #    two seconds - close enough to the poll interval that judging a
        #    launch immediately would kill it and start another that never
        #    finishes either. Give each launch a few seconds of quiet first.
        $settling = ((Get-Date) - $lastLaunch).TotalSeconds -lt $LaunchGraceSec
        $guiUp = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
        if (-not $guiUp -and -not $settling) {
            Get-Process civvis-gui -ErrorAction SilentlyContinue | Stop-Process -Force
            Start-Sleep -Milliseconds 300
            if (Test-Path "$binRun\civvis-next.exe") { Promote-Staged | Out-Null }
            if (Test-Path "$binRun\civvis-gui.exe") { Start-Gui; $lastLaunch = Get-Date }
        }
        # Evolve is a twelve-thread CPU hog and only useful in the background.
        # Starting it while the server is still generating its map made the
        # two compete for the machine and stretched the changeover badly, so
        # it waits until there is a game to be in the background of.
        $evoUp = Get-Process civvis-evolve -ErrorAction SilentlyContinue
        if ($guiUp -and -not $evoUp -and (Test-Path "$binRun\civvis-evolve.exe")) { Start-Evolve }

        # 2a. collect a finished build. This runs on the fast loop, not the
        #     git cadence, so a build that lands mid-game is staged and ready
        #     the moment the game ends rather than half a minute later.
        if ($null -ne $build -and $build.HasExited) {
            $took = ((Get-Date) - $buildStarted).TotalSeconds
            # Judge the build by what the compiler said, not by the process
            # object. Start-Process -PassThru hands back an ExitCode that
            # stays unreliable on Windows PowerShell even after waiting, so
            # every successful build was recorded as broken: the stamp was
            # never written, the same commit rebuilt every cadence, and the
            # exhibition sat on an old binary while build.err.log plainly read
            # Finished release profile.
            $build.WaitForExit()
            $code = (Get-Content "$binRun\build.code" -Raw -ErrorAction SilentlyContinue)
            $report = Get-Content "$binRun\build.log" -Raw -ErrorAction SilentlyContinue
            if ($null -ne $code -and $code.Trim() -eq "0") {
                Copy-Item "$src\target\release\civvis.exe" "$binRun\civvis-next.exe" -Force
                Set-Content -Path $stamp -Value $buildHead -Encoding ascii
                Log ("staged $buildShort in {0:n0}s" -f $took)
            } else {
                # Record nothing on failure, so a later round retries rather
                # than treating a broken commit as already built. Carry the
                # compiler's own first complaint into the log: the next build
                # overwrites build.err.log, so "build FAILED" on its own is
                # unattributable by the time anyone reads it.
                $why = ([regex]::Match([string]$report, '(?m)^error(\[|:).*$')).Value
                if (-not $why) { $why = "cargo exited $($code -replace '\s','') with no error line" }
                Log "build FAILED for ${buildShort}: $why"
            }
            $build = $null
        }

        # 2b. staged binary older than origin/main? build it. The test is the
        #    commit the last build came from, not "did a fetch move something
        #    this round": a build cut short by a restart, or a failed build,
        #    otherwise leaves the exhibition on the previous commit with
        #    nothing left to trigger a retry.
        if ((Test-Path "$src\Cargo.toml") -and
            ((Get-Date) - $lastGit).TotalSeconds -ge $GitSec) {
            $lastGit = Get-Date
            git -C $src fetch -q origin main 2>$null
            $head = git -C $src rev-parse FETCH_HEAD
            $built = if (Test-Path $stamp) { (Get-Content $stamp -Raw).Trim() } else { "" }
            if ($head -and $head -ne $built -and $null -eq $build -and
                $null -eq (Get-ExhibitionCargo | Select-Object -First 1)) {
                $short = $head.Substring(0, 7)
                git -C $src reset -q --hard $head 2>$null
                Log "building $short"
                $buildHead = $head
                $buildShort = $short
                $buildStarted = Get-Date
                # Launched rather than run. A release build takes one to two
                # minutes here, and running it inline stopped the whole loop
                # for that long - so a game that ended mid-build sat decided,
                # or the server sat dead, until the compiler finished. That is
                # where a twenty-one second changeover came from.
                Remove-Item "$binRun\build.code" -Force -ErrorAction SilentlyContinue
                Write-BuildScript $short
                $build = Start-Process -FilePath $buildScript -PassThru -WindowStyle Hidden
            }

            # 3. keep the shared checkout moving too, for the sessions working
            #    in it - but only when it is clean, and nothing above depends
            #    on it. Shares the git cadence: the fast poll exists for the
            #    changeover, and dragging a status call through it every few
            #    seconds would only slow that down.
            $localDirty = git -C $repo status --porcelain --untracked-files=no
            if (-not $localDirty) {
                $l = git -C $repo rev-parse HEAD
                $r = git -C $repo rev-parse origin/main
                if ($l -ne $r) {
                    git -C $repo merge --ff-only -q origin/main 2>$null
                    if ($LASTEXITCODE -ne 0) {
                        Log "shared checkout is not a fast-forward of origin/main; preserving it"
                    }
                }
            }
        }

        # 4. game over? start the next one. A staged build is swapped in here,
        #    in the between-games window, so every game boots on the newest
        #    code - but the next game starts either way. Gating all of this on
        #    having a staged build used to leave a decided game frozen on
        #    screen until some unrelated commit happened to land.
        # /status carries the winner alone. /state builds close to a megabyte
        # of observation JSON, and asking for that every few seconds to read
        # one field made the server stall for whole seconds under load - which
        # from the browser looks like the game freezing.
        $st = $null
        try { $st = Invoke-RestMethod "http://localhost:$Port/status" -TimeoutSec 5 } catch {}
        if ($null -eq $st) {
            # Older binaries predate /status; fall back so a swap onto one
            # still finds its way to the next game.
            try { $st = Invoke-RestMethod "http://localhost:$Port/state" -TimeoutSec 5 } catch {}
        }
        if (($null -ne $st) -and ($null -ne $st.winner)) {
            if (Test-Path "$binRun\civvis-next.exe") {
                Get-Process civvis-gui -ErrorAction SilentlyContinue | Stop-Process -Force
                Get-Process civvis-evolve -ErrorAction SilentlyContinue | Stop-Process -Force
                Start-Sleep -Milliseconds 200
                Promote-Staged | Out-Null
                Start-Gui
                # Evolve is deliberately not restarted here. The revive check
                # brings it back once the server is listening, so the new game
                # gets the machine to itself while it generates its map.
                $lastLaunch = Get-Date
                Log "swapped to latest build between games"
            } else {
                Start-NextGame
            }
        }
    } catch {
        Log "supervisor error: $_"
    }
    Start-Sleep -Seconds $PollSec
}
