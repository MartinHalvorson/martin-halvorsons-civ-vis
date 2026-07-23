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
param([int]$Port = 8765, [int]$PollSec = 10)

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

function Promote-Staged {
    Copy-Item "$binRun\civvis-next.exe" "$binRun\civvis-gui.exe" -Force
    Copy-Item "$binRun\civvis-next.exe" "$binRun\civvis-evolve.exe" -Force
    Remove-Item "$binRun\civvis-next.exe" -Force
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

Log "supervisor started (port $Port, poll ${PollSec}s)"
while ($true) {
    try {
        # 1. revive anything that died
        $guiUp = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
        if (-not $guiUp) {
            if (Test-Path "$binRun\civvis-next.exe") { Promote-Staged }
            if (Test-Path "$binRun\civvis-gui.exe") { Start-Gui }
        }
        $evoUp = Get-Process civvis-evolve -ErrorAction SilentlyContinue
        if (-not $evoUp -and (Test-Path "$binRun\civvis-evolve.exe")) { Start-Evolve }

        # 2. staged binary older than origin/main? build it. The test is the
        #    commit the last build came from, not "did a fetch move something
        #    this round": a build cut short by a restart, or a failed build,
        #    otherwise leaves the exhibition on the previous commit with
        #    nothing left to trigger a retry.
        if (Test-Path "$src\Cargo.toml") {
            git -C $src fetch -q origin main 2>$null
            $head = git -C $src rev-parse FETCH_HEAD
            $built = if (Test-Path $stamp) { (Get-Content $stamp -Raw).Trim() } else { "" }
            if ($head -and $head -ne $built) {
                $short = $head.Substring(0, 7)
                git -C $src reset -q --hard $head 2>$null
                Log "building $short"
                $sw = [Diagnostics.Stopwatch]::StartNew()
                $env:CARGO_TARGET_DIR = "$src\target"
                & $cargo build --release --manifest-path "$src\Cargo.toml" 2>$null | Out-Null
                $ok = $LASTEXITCODE -eq 0
                $sw.Stop()
                if ($ok) {
                    Copy-Item "$src\target\release\civvis.exe" "$binRun\civvis-next.exe" -Force
                    Set-Content -Path $stamp -Value $head -Encoding ascii
                    Log ("staged $short in {0:n0}s" -f $sw.Elapsed.TotalSeconds)
                } else {
                    # Record nothing on failure, so the next round retries
                    # rather than treating a broken commit as already built.
                    Log "build FAILED for $short"
                }
            }
        }

        # 3. keep the shared checkout moving too, for the sessions working in
        #    it - but only when it is clean, and nothing above depends on it.
        $localDirty = git -C $repo status --porcelain --untracked-files=no
        if (-not $localDirty) {
            $l = git -C $repo rev-parse HEAD
            $r = git -C $repo rev-parse origin/main
            if ($l -ne $r) {
                git -C $repo pull --rebase -q 2>$null
                if ($LASTEXITCODE -ne 0) { git -C $repo rebase --abort 2>$null }
            }
        }

        # 4. game over? start the next one. A staged build is swapped in here,
        #    in the between-games window, so every game boots on the newest
        #    code - but the next game starts either way. Gating all of this on
        #    having a staged build used to leave a decided game frozen on
        #    screen until some unrelated commit happened to land.
        $st = $null
        try { $st = Invoke-RestMethod "http://localhost:$Port/state" -TimeoutSec 5 } catch {}
        if (($null -ne $st) -and ($null -ne $st.winner)) {
            if (Test-Path "$binRun\civvis-next.exe") {
                Get-Process civvis-gui -ErrorAction SilentlyContinue | Stop-Process -Force
                Get-Process civvis-evolve -ErrorAction SilentlyContinue | Stop-Process -Force
                Start-Sleep -Milliseconds 500
                Promote-Staged
                Start-Gui
                Start-Evolve
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
