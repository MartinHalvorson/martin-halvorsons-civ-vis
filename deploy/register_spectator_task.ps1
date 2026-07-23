# register_spectator_task.ps1 - install the CIVVIS spectator loop as an always-on
# Windows service via Task Scheduler. Idempotent (-Force replaces any prior task).
#
# The task launches pythonw.exe (a windowless interpreter) directly, so nothing
# ever flashes on screen; the supervisor's own CREATE_NO_WINDOW on its children
# keeps the game server and cargo hidden too. A per-port lock inside the
# supervisor - not Task Scheduler - is what guarantees a single instance, which
# is why a self-update (os.execv spawns a fresh PID on Windows) and the 5-minute
# recovery trigger can never leave two supervisors fighting over the port.
#
# Prereq: a dedicated deploy checkout pinned to origin/main, e.g.
#   git -C <shared-checkout> worktree add --detach C:\Users\<you>\PycharmProjects\civvis-spectator origin/main
# Then:  powershell -ExecutionPolicy Bypass -File deploy\register_spectator_task.ps1
param(
    [int]$Port = 8765,
    [string]$DeployRoot = "$env:USERPROFILE\PycharmProjects\civvis-spectator",
    [string]$Pythonw = "$env:LOCALAPPDATA\Programs\Python\Python312\pythonw.exe",
    [int]$Players = 6,
    [int]$Width = 74,
    [int]$Height = 46,
    [int]$CityStates = 8,
    [int]$Turns = 300,
    [string]$Map = "continents",
    [string]$Speed = "standard",
    [int]$Cooldown = 8,
    [string]$TaskPath = "\Martbot\",
    [string]$TaskName = "Civvis Spectator"
)

$script = Join-Path $DeployRoot "tools\spectator_supervisor.py"
if (-not (Test-Path $script)) {
    throw "supervisor not found at $script - create the deploy worktree first (see the header)."
}
if (-not (Test-Path $Pythonw)) { throw "pythonw.exe not found at $Pythonw - pass -Pythonw." }

$argline = "`"$script`" --port $Port --players $Players --width $Width --height $Height " +
           "--city-states $CityStates --turns $Turns --map $Map --speed $Speed " +
           "--cooldown $Cooldown --no-open"

$action = New-ScheduledTaskAction -Execute $Pythonw -Argument $argline -WorkingDirectory $DeployRoot
# At logon starts it with the desktop session; the repeating trigger is the
# recovery path - if the supervisor ever dies, the next fire brings it back,
# and the per-port lock makes every fire while it is healthy a no-op.
$atLogon = New-ScheduledTaskTrigger -AtLogOn
$recover = New-ScheduledTaskTrigger -Once -At (Get-Date) `
    -RepetitionInterval (New-TimeSpan -Minutes 5) -RepetitionDuration ([TimeSpan]::MaxValue)
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 0

Register-ScheduledTask -TaskPath $TaskPath -TaskName $TaskName `
    -Action $action -Trigger @($atLogon, $recover) -Settings $settings -Force | Out-Null

Write-Output "Registered $TaskPath$TaskName -> pythonw $argline"
Write-Output "Start now:  Start-ScheduledTask -TaskPath '$TaskPath' -TaskName '$TaskName'"
Write-Output "Watch:      Get-Content '$DeployRoot\spectator-supervisor.log' -Wait -Tail 20"
