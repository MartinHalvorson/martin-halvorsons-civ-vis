# exhibition.ps1 - keep the civvis spectate exhibition running on the latest
# code. Loop: revive dead processes; fetch origin/main and stage a fresh
# release build the moment new commits land, so the binary is already waiting
# when the current game ends; then, between games, either swap onto that build
# or deal the next game in place. Something is always playing on screen.
# Run hidden:  Start-Process powershell -WindowStyle Hidden -ArgumentList
#              "-ExecutionPolicy","Bypass","-File","exhibition.ps1"
param([int]$Port = 8765, [int]$PollSec = 10)

$repo = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $repo
$cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe"
$log = "$repo\exhibition-supervisor.log"
New-Item -ItemType Directory -Force "$repo\bin-run" | Out-Null

function Log($msg) {
    Add-Content -Encoding utf8 $log "$(Get-Date -Format 'MM-dd HH:mm:ss') $msg"
}

function Start-Gui {
    Start-Process -FilePath "$repo\bin-run\civvis-gui.exe" `
        -ArgumentList "play","--spectate","--no-open","--port","$Port" `
        -WorkingDirectory $repo -WindowStyle Hidden `
        -RedirectStandardOutput "$repo\civvis-play.log" `
        -RedirectStandardError "$repo\civvis-play.err.log"
    Log "gui launched on :$Port"
}

function Start-Evolve {
    Start-Process -FilePath "$repo\bin-run\civvis-evolve.exe" `
        -ArgumentList "evolve","--threads","12","--pop","16","--games","8","--turns","160" `
        -WorkingDirectory $repo -WindowStyle Hidden `
        -RedirectStandardOutput "$repo\evolved\evolve.log" `
        -RedirectStandardError "$repo\evolved\evolve.err.log"
    Log "evolve launched"
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

Log "supervisor started (port $Port, poll ${PollSec}s)"
while ($true) {
    try {
        # 1. revive anything that died
        $guiUp = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
        if (-not $guiUp) {
            if (Test-Path "$repo\bin-run\civvis-next.exe") {
                Copy-Item "$repo\bin-run\civvis-next.exe" "$repo\bin-run\civvis-gui.exe" -Force
                Copy-Item "$repo\bin-run\civvis-next.exe" "$repo\bin-run\civvis-evolve.exe" -Force
                Remove-Item "$repo\bin-run\civvis-next.exe" -Force
            }
            if (Test-Path "$repo\bin-run\civvis-gui.exe") { Start-Gui }
        }
        $evoUp = Get-Process civvis-evolve -ErrorAction SilentlyContinue
        if (-not $evoUp -and (Test-Path "$repo\bin-run\civvis-evolve.exe")) { Start-Evolve }

        # 2. keep up with origin/main, then make sure the binaries were built
        #    from whatever HEAD now is. Triggering the build off "did we just
        #    pull" alone is not enough: after a restart, a failed build, or a
        #    hand-staged binary, the checkout can already be current while the
        #    exe on disk is several commits old, and nothing would ever
        #    rebuild it. Comparing against the commit that produced the last
        #    staged build heals all of those. Skip while the checkout is dirty:
        #    a parallel session is mid-work; retry next round.
        git fetch -q origin main 2>$null
        $local = git rev-parse HEAD
        $remote = git rev-parse origin/main
        $dirty = git status --porcelain --untracked-files=no
        if ($local -ne $remote -and -not $dirty) {
            git pull --rebase -q 2>$null
            if ($LASTEXITCODE -ne 0) {
                git rebase --abort 2>$null
                Log "pull failed; will retry"
            } else {
                Log "pulled $(git rev-parse --short HEAD)"
            }
        }
        $stamp = "$repo\bin-run\built.commit"
        $head = git rev-parse HEAD
        $built = if (Test-Path $stamp) { (Get-Content $stamp -Raw).Trim() } else { "" }
        if ($head -ne $built -and -not $dirty) {
            Log "building $(git rev-parse --short HEAD)"
            $sw = [Diagnostics.Stopwatch]::StartNew()
            & $cargo build --release 2>$null | Out-Null
            $sw.Stop()
            if ($LASTEXITCODE -eq 0) {
                Copy-Item "$repo\target\release\civvis.exe" "$repo\bin-run\civvis-next.exe" -Force
                Set-Content -Path $stamp -Value $head -Encoding ascii
                Log ("staged new build in {0:n0}s" -f $sw.Elapsed.TotalSeconds)
            } else {
                # Record nothing on failure, so the next round retries rather
                # than treating a broken commit as already built.
                Log "build FAILED for $(git rev-parse --short HEAD)"
            }
        }

        # 3. game over? start the next one. A staged build is swapped in here,
        #    in the between-games window, so every game boots on the newest
        #    code - but the next game starts either way. Gating all of this on
        #    having a staged build used to leave a decided game frozen on
        #    screen until some unrelated commit happened to land.
        $st = $null
        try { $st = Invoke-RestMethod "http://localhost:$Port/state" -TimeoutSec 5 } catch {}
        if (($null -ne $st) -and ($null -ne $st.winner)) {
            if (Test-Path "$repo\bin-run\civvis-next.exe") {
                Get-Process civvis-gui -ErrorAction SilentlyContinue | Stop-Process -Force
                Get-Process civvis-evolve -ErrorAction SilentlyContinue | Stop-Process -Force
                Start-Sleep -Milliseconds 500
                Copy-Item "$repo\bin-run\civvis-next.exe" "$repo\bin-run\civvis-gui.exe" -Force
                Copy-Item "$repo\bin-run\civvis-next.exe" "$repo\bin-run\civvis-evolve.exe" -Force
                Remove-Item "$repo\bin-run\civvis-next.exe" -Force
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
