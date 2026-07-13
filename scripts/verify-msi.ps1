# verify-msi.ps1 — lifecycle-verify the built suite MSI on a Windows box
# (windows-latest CI or a dev VM; requires elevation, which both provide).
# The Windows analogue of scripts/verify-packages.sh: silent full install →
# per-service class checks → failure-actions proofs → feature remove → full
# uninstall (configs and logs survive, deb-`remove` parity).
#
# Service classes mirror verify-packages.sh:
#   - active:  reach RUNNING + HTTP probe (network-only services; the zwo
#     services serve with zero devices attached, phd2-guider answers 503
#     without PHD2, so all of them run on a hardware-less box)
#   - serial:  eager hardware validation exits when no device is present; the
#     contract is config self-created + "eager startup handshake" in the log +
#     SCM restarting the service (NOT "running")
#   - gated:   demand-start (no defaultable config): installed, Manual, stopped
#   - qhy-camera: without QHY's All-in-One pack the delay-load preflight must
#     log its distinctive pointer and exit cleanly (not a loader crash)
#
# Usage: scripts\verify-msi.ps1 [-Msi <path>] [-Keep] [-UpgradeFrom <path>]
#   -Msi          the MSI to verify (default: dist\<workspace version>\...)
#   -Keep         leave the product installed on exit (debugging)
#   -UpgradeFrom  a previously published MSI to install FIRST, so the main
#                 install runs as an in-place upgrade over it. The nightly
#                 channel's AllowSameVersionUpgrades path (every nightly
#                 authors the same compared ProductVersion) is exercised
#                 only this way — release-tag testing never sees it. The
#                 rest of the lifecycle then runs against the upgraded
#                 install, whose invariants match a fresh one.

[CmdletBinding()]
param(
    [string]$Msi,
    [switch]$Keep,
    [string]$UpgradeFrom
)

$ErrorActionPreference = 'Stop'

function Die([string]$msg) {
    Write-Error "verify-msi: $msg"
    exit 1
}

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Die "must run elevated (msiexec /qn and service control need it)"
}
if (-not [Environment]::Is64BitProcess) {
    # Under 32-bit PowerShell, WOW64 redirection points the registry
    # provider (the ARP checks) and %ProgramFiles% (the install-dir
    # checks) at the 32-bit views — wrong for this x64-only product
    # (ADR-015).
    Die "must run from 64-bit PowerShell"
}
if (-not (Test-Path "installer\Package.wxs")) { Die "run from the repo root" }

if (-not $Msi) {
    $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"$' |
        Select-Object -First 1).Matches[0].Groups[1].Value
    $Msi = "dist\$version\rusty-photon-$version-x64.msi"
}
if (-not (Test-Path $Msi)) { Die "$Msi not found — run scripts\build-msi.ps1 first" }
$Msi = (Resolve-Path $Msi).Path

# ---- service classification (mirrors verify-packages.sh) ------------------
$ports = @{
    'filemonitor' = 11111; 'ppba-driver' = 11112; 'qhy-focuser' = 11113
    'sentinel' = 11114; 'rp' = 11115; 'sky-survey-camera' = 11116
    'star-adventurer-gti' = 11117; 'pa-falcon-rotator' = 11118
    'dsd-fp2' = 11119; 'ui-htmx' = 11120; 'qhy-camera' = 11121
    'zwo-camera' = 11122; 'pa-scops-oag' = 11123; 'zwo-focuser' = 11124
    'phd2-guider' = 11130; 'plate-solver' = 11131; 'calibrator-flats' = 11170
    'session-runner' = 11171
}
$allServices = $ports.Keys | Sort-Object
# session-runner is gated like the Linux-gated three: its workflows_dir/
# state_dir are required config fields with no usable defaults.
$gated = @('sky-survey-camera', 'plate-solver', 'calibrator-flats', 'session-runner')
$serial = @('ppba-driver', 'qhy-focuser', 'pa-falcon-rotator', 'pa-scops-oag',
    'dsd-fp2', 'star-adventurer-gti')
$active = @('sentinel', 'ui-htmx', 'filemonitor', 'rp',
    'phd2-guider', 'zwo-camera', 'zwo-focuser')
# Plain-HTTP services expose /health; Alpaca services answer the management
# API. The cameras, zwo-focuser, phd2-guider and session-runner never
# self-create a config (SDK-derived identity / built-in defaults); ui-htmx's
# config comes from the MSI seed action, asserted separately.
$healthProbe = @('sentinel', 'rp', 'ui-htmx', 'phd2-guider')
$selfCreatesConfig = @('sentinel', 'rp', 'filemonitor') + $serial

$dataDir = Join-Path $env:ProgramData 'rusty-photon'
$logsDir = Join-Path $dataDir 'logs'
$installDir = Join-Path ${env:ProgramFiles} 'rusty-photon'
$installLog = Join-Path $env:TEMP 'rusty-photon-msi-install.log'

# Fresh-box preflight: the run asserts fresh-install invariants (gated
# services have no config, the seeded ui-htmx map matches the feature set,
# configs self-create), which leftovers from a prior install would corrupt —
# fail fast with a pointer instead of failing (or passing) for the wrong
# reason mid-run. CI runners are always fresh; on a dev box, uninstall and
# delete %ProgramData%\rusty-photon (the documented manual purge) first.
if (Get-Service -Name 'rusty-photon-*' -ErrorAction SilentlyContinue) {
    Die "rusty-photon-* services already installed — msiexec /x the previous install first"
}
if (Test-Path $dataDir) {
    Die "$dataDir already exists — delete it (manual purge) so fresh-install checks are meaningful"
}

function Fail([string]$svc, [string]$msg) {
    Write-Host "verify-msi: FAIL [$svc]: $msg" -ForegroundColor Red
    $svcLog = Get-ChildItem -Path $logsDir -Filter "$svc.*" -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime | Select-Object -Last 1
    if ($svcLog) {
        Write-Host "--- last 40 lines of $($svcLog.Name) ---"
        Get-Content $svcLog.FullName -Tail 40
    }
    if (Test-Path $installLog) {
        # The verbose log is huge; the failure signal is the action(s) that
        # ended with "Return value 3" plus any Error-coded lines.
        Write-Host "--- msiexec log: failed actions + error lines ---"
        Select-String -Path $installLog -Pattern 'Return value 3|^Error \d+|error 1\d{3}|CustomAction .+ returned actual error|failed to start|could not be|MainEngineThread is returning' |
            Select-Object -Last 30 | ForEach-Object { Write-Host $_.Line }
    }
    exit 1
}

# Poll $probe every second until it returns truthy or $timeoutSec elapses.
function WaitFor([string]$svc, [string]$what, [scriptblock]$probe, [int]$timeoutSec = 60) {
    for ($i = 0; $i -lt $timeoutSec; $i++) {
        if (& $probe) { return }
        Start-Sleep -Seconds 1
    }
    Fail $svc "timed out after ${timeoutSec}s waiting for: $what"
}

function ServiceLogContent([string]$svc) {
    # Newest daily file only, tail-bounded: crash-looping services append
    # every 5 s while WaitFor polls every second, so unbounded -Raw reads of
    # every file would grow quadratically over a verification run.
    $f = Get-ChildItem -Path $logsDir -Filter "$svc.*" -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not $f) { return "" }
    (Get-Content $f.FullName -Tail 500) -join "`n"
}

function Msiexec([string[]]$msiArgs) {
    $p = Start-Process -FilePath msiexec.exe -ArgumentList $msiArgs -Wait -PassThru
    return $p.ExitCode
}

# The product's Programs & Features registrations (x64 MSI -> native hive).
function ArpEntries {
    Get-ChildItem 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall' |
        ForEach-Object { Get-ItemProperty $_.PSPath } |
        Where-Object { $_.DisplayName -eq 'Rusty Photon' }
}

# ---- optional upgrade seed (nightly-over-nightly proof) --------------------
if ($UpgradeFrom) {
    if (-not (Test-Path $UpgradeFrom)) { Die "-UpgradeFrom $UpgradeFrom not found" }
    $UpgradeFrom = (Resolve-Path $UpgradeFrom).Path
    $priorLog = Join-Path $env:TEMP 'rusty-photon-msi-prior-install.log'
    Write-Host "== upgrade seed: installing the prior MSI ($(Split-Path -Leaf $UpgradeFrom))"
    $code = Msiexec @('/i', "`"$UpgradeFrom`"", '/qn', '/norestart', "/l*v", "`"$priorLog`"", 'ADDLOCAL=ALL')
    if ($code -ne 0) {
        # Fail's msiexec-log excerpt must come from the install that
        # actually failed (the script exits inside Fail; the main install
        # never runs, so repointing is safe).
        $installLog = $priorLog
        Fail 'msiexec' "prior-MSI install exited $code (log: $priorLog)"
    }
    if (-not (Get-Service -Name 'rusty-photon-sentinel' -ErrorAction SilentlyContinue)) {
        $installLog = $priorLog
        Fail 'msiexec' "prior MSI installed no services — the upgrade proof would be vacuous"
    }
}

# ---- install (all features) ----------------------------------------------
Write-Host "== install: msiexec /qn ADDLOCAL=ALL"
$code = Msiexec @('/i', "`"$Msi`"", '/qn', '/norestart', "/l*v", "`"$installLog`"", 'ADDLOCAL=ALL')
if ($code -ne 0) { Fail 'msiexec' "silent install exited $code (log: $installLog)" }

if ($UpgradeFrom) {
    # The install above ran over the seeded product: prove it upgraded in
    # place (RemoveExistingProducts consumed the old registration) rather
    # than installing side by side — the failure mode
    # AllowSameVersionUpgrades exists to prevent.
    $entries = @(ArpEntries)
    if ($entries.Count -ne 1) {
        Fail 'msiexec' "expected exactly one Rusty Photon ARP entry after the upgrade, found $($entries.Count) (side-by-side install?)"
    }
    # ARPCOMMENTS carries the full version string, and the MSI under test
    # always authors it; the filename is rusty-photon-<fullversion>-x64.msi,
    # so this pins the surviving entry to the MSI just installed. A filename
    # that cannot be parsed fails outright — silently skipping the pin would
    # leave "the surviving entry is the OLD product" undetected.
    if ((Split-Path -Leaf $Msi) -notmatch '^rusty-photon-(.+)-x64\.msi$') {
        Fail 'msiexec' "cannot pin the surviving ARP entry: '$(Split-Path -Leaf $Msi)' is not named rusty-photon-<version>-x64.msi"
    }
    $expected = "rusty-photon $($Matches[1])"
    if ($entries[0].Comments -ne $expected) {
        Fail 'msiexec' "surviving ARP entry comments '$($entries[0].Comments)' != '$expected' (old product survived the upgrade?)"
    }
    Write-Host "== upgrade: OK (single ARP entry after installing over the prior MSI)"
}

# ---- static asserts: services, start types, failure actions ---------------
foreach ($svc in $allServices) {
    $name = "rusty-photon-$svc"
    $s = Get-Service -Name $name -ErrorAction SilentlyContinue
    if (-not $s) { Fail $svc "service $name not installed" }

    $expectedStart = if ($gated -contains $svc) { 'Manual' } else { 'Automatic' }
    if ($s.StartType -ne $expectedStart) {
        Fail $svc "StartType is $($s.StartType), expected $expectedStart"
    }

    # Failure actions: restart after 5000 ms, and the failure-actions-on-
    # non-crash-failures flag MUST be set or the ServiceSpecific(1) exits of
    # the serial drivers would never trigger a restart (ADR-015 / W1).
    $qf = sc.exe qfailure $name
    if ($LASTEXITCODE -ne 0) { Fail $svc "sc qfailure failed" }
    if (-not (($qf | Out-String) -match 'RESTART -- Delay = 5000')) {
        Fail $svc "failure actions do not restart after 5000 ms:`n$($qf | Out-String)"
    }
    $qff = sc.exe qfailureflag $name
    if ($LASTEXITCODE -ne 0) { Fail $svc "sc qfailureflag failed" }
    if (-not (($qff | Out-String) -match 'TRUE')) {
        Fail $svc "SERVICE_CONFIG_FAILURE_ACTIONS_FLAG not set:`n$($qff | Out-String)"
    }
}
Write-Host "== static: $($allServices.Count) services installed, start types + failure actions OK"

# ---- gated trio: installed but never started, no config -------------------
foreach ($svc in $gated) {
    $s = Get-Service -Name "rusty-photon-$svc"
    if ($s.Status -ne 'Stopped') { Fail $svc "gated service is $($s.Status), expected Stopped" }
    if (Test-Path (Join-Path $dataDir "$svc.json")) {
        Fail $svc "config exists on a fresh install of a gated service"
    }
    Write-Host "== ${svc}: OK (gated: Manual + stopped, no config)"
}

# ---- seeded ui-htmx driver map ---------------------------------------------
$uiCfgPath = Join-Path $dataDir 'ui-htmx.json'
if (-not (Test-Path $uiCfgPath)) { Fail 'ui-htmx' "seeded $uiCfgPath missing" }
$uiCfg = Get-Content $uiCfgPath -Raw | ConvertFrom-Json
# The 11 Drivers-tree services (everything the seed script's table lists).
$driverSet = @('filemonitor', 'ppba-driver', 'qhy-focuser', 'sky-survey-camera',
    'star-adventurer-gti', 'pa-falcon-rotator', 'dsd-fp2', 'qhy-camera',
    'zwo-camera', 'pa-scops-oag', 'zwo-focuser')
$seeded = @($uiCfg.drivers.PSObject.Properties.Name) | Sort-Object
$expected = @($driverSet) | Sort-Object
if (($seeded -join ',') -ne ($expected -join ',')) {
    Fail 'ui-htmx' "seeded drivers map [$($seeded -join ', ')] != installed driver set [$($expected -join ', ')]"
}
foreach ($d in $seeded) {
    $entry = $uiCfg.drivers.$d
    if ($entry.base_url -ne "http://127.0.0.1:$($ports[$d])") {
        Fail 'ui-htmx' "seeded $d base_url is $($entry.base_url), expected port $($ports[$d])"
    }
}
if (-not $uiCfg.PSObject.Properties['rp']) {
    Fail 'ui-htmx' "rp feature installed but the seeded config has no rp target"
}
Write-Host "== ui-htmx: OK (seeded drivers map matches the installed feature set + rp target)"

# ---- active class: RUNNING + config + probe --------------------------------
foreach ($svc in $active) {
    $name = "rusty-photon-$svc"
    WaitFor $svc "service RUNNING" { (Get-Service -Name $name).Status -eq 'Running' }

    if ($selfCreatesConfig -contains $svc) {
        $cfg = Join-Path $dataDir "$svc.json"
        WaitFor $svc "config self-created at $cfg" { Test-Path $cfg } 30
    }

    $port = $ports[$svc]
    $path = if ($healthProbe -contains $svc) { '/health' } else { '/management/apiversions' }
    WaitFor $svc "HTTP response on port $port$path" {
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$port$path" -TimeoutSec 5 | Out-Null
            $true
        } catch {
            # No PHD2 on a verify box: phd2-guider's /health legitimately
            # answers 503 (listener up, guider not connected). Non-HTTP
            # failures (connection refused while the service is coming up)
            # carry no Response — treat those as "not yet" and keep polling.
            $resp = $_.Exception.PSObject.Properties['Response']
            $status = if ($resp -and $resp.Value) { [int]$resp.Value.StatusCode } else { 0 }
            $svc -eq 'phd2-guider' -and $status -eq 503
        }
    }
    Write-Host "== ${svc}: OK (running, port $port$path)"
}

# ---- serial class: config + handshake attempts + SCM restart proof ---------
foreach ($svc in $serial) {
    $cfg = Join-Path $dataDir "$svc.json"
    WaitFor $svc "config self-created at $cfg" { Test-Path $cfg } 30
    WaitFor $svc "'eager startup handshake' in the service log" {
        (ServiceLogContent $svc) -match 'eager startup handshake'
    } 30
    Write-Host "== ${svc}: OK (config self-created; retrying on absent serial device)"
}

# Behavioral proof of the failure-actions flag: an eager-exit stop is a
# ServiceSpecific(1) NON-CRASH failure, so a second handshake attempt can only
# happen if SCM counted the first exit as a failure and restarted the service.
$flagProbe = $serial[0]
WaitFor $flagProbe "a second handshake attempt (SCM restart-on-error proof)" {
    ([regex]::Matches((ServiceLogContent $flagProbe), 'eager startup handshake')).Count -ge 2
} 90
Write-Host "== ${flagProbe}: OK (restarted after a clean error exit — failure-actions flag works)"

# ---- qhy-camera: delay-load preflight (no All-in-One pack on a verify box) --
WaitFor 'qhy-camera' "the preflight's distinctive missing-DLL log line" {
    (ServiceLogContent 'qhy-camera') -match 'qhyccd\.dll not found'
} 30
Write-Host "== qhy-camera: OK (preflight reported the missing DLL — no loader crash)"

# ---- log files for everything that ran -------------------------------------
foreach ($svc in ($active + $serial + @('qhy-camera'))) {
    if (-not (Get-ChildItem -Path $logsDir -Filter "$svc.*" -ErrorAction SilentlyContinue)) {
        Fail $svc "no rolling log file under $logsDir"
    }
}
Write-Host "== logs: OK (rolling files present for every started service)"

# ---- kill-and-observe: crash restart (sentinel) -----------------------------
$sentinelPid = (Get-CimInstance Win32_Service -Filter "Name='rusty-photon-sentinel'").ProcessId
if (-not $sentinelPid) { Fail 'sentinel' "no PID for the running service" }
Write-Host "== sentinel: killing PID $sentinelPid to observe the SCM restart"
Stop-Process -Id $sentinelPid -Force
WaitFor 'sentinel' "SCM restart after kill (new PID, RUNNING)" {
    $s = Get-CimInstance Win32_Service -Filter "Name='rusty-photon-sentinel'"
    $s.State -eq 'Running' -and $s.ProcessId -ne $sentinelPid -and $s.ProcessId -ne 0
} 60
Write-Host "== sentinel: OK (SCM restarted it after a hard kill)"

# ---- reload smoke: SCM ParamChange -> ReloadSignal --------------------------
sc.exe control rusty-photon-filemonitor paramchange | Out-Null
if ($LASTEXITCODE -ne 0) { Fail 'filemonitor' "sc control paramchange failed ($LASTEXITCODE)" }
Write-Host "== filemonitor: OK (accepted ParamChange)"

# ---- feature remove: per-device split stays honest --------------------------
Write-Host "== modify: REMOVE=ZwoCamera"
$code = Msiexec @('/i', "`"$Msi`"", '/qn', '/norestart', 'REMOVE=ZwoCamera')
if ($code -ne 0) { Fail 'zwo-camera' "feature remove exited $code" }
if (Get-Service -Name 'rusty-photon-zwo-camera' -ErrorAction SilentlyContinue) {
    Fail 'zwo-camera' "service still installed after feature remove"
}
if (Test-Path (Join-Path $installDir 'rusty-photon-zwo-camera.exe')) {
    Fail 'zwo-camera' "exe still present after feature remove"
}
if (Test-Path (Join-Path $installDir 'ASICamera2.dll')) {
    Fail 'zwo-camera' "ASICamera2.dll still present after feature remove"
}
# The focuser must be untouched: its own DLL and the shared license stay.
if ((Get-Service -Name 'rusty-photon-zwo-focuser').Status -ne 'Running') {
    Fail 'zwo-focuser' "not running after removing the zwo-camera feature"
}
if (-not (Test-Path (Join-Path $installDir 'EAF_focuser.dll'))) {
    Fail 'zwo-focuser' "EAF_focuser.dll disappeared with the zwo-camera feature"
}
if (-not (Test-Path (Join-Path $installDir 'ZWO-SDK-LICENSE.txt'))) {
    Fail 'zwo-focuser' "shared ZWO license disappeared while a zwo feature is installed"
}
Write-Host "== modify: OK (zwo-camera gone; zwo-focuser + its DLL + shared license intact)"

if ($Keep) {
    Write-Host "verify-msi: -Keep set; leaving the product installed"
    exit 0
}

# ---- full uninstall ---------------------------------------------------------
Write-Host "== uninstall: msiexec /qn /x"
$code = Msiexec @('/x', "`"$Msi`"", '/qn', '/norestart')
if ($code -ne 0) { Fail 'msiexec' "uninstall exited $code" }
foreach ($svc in $allServices) {
    if (Get-Service -Name "rusty-photon-$svc" -ErrorAction SilentlyContinue) {
        Fail $svc "service still installed after uninstall"
    }
}
if (Get-ChildItem -Path $installDir -Filter '*.exe' -ErrorAction SilentlyContinue) {
    Fail 'msiexec' "exes left under $installDir after uninstall"
}
# deb `remove` parity: self-created configs and logs are untracked by the MSI
# and survive uninstall; purge is a documented manual step.
if (-not (Test-Path (Join-Path $dataDir 'sentinel.json'))) {
    Fail 'sentinel' "self-created config did not survive uninstall (must only go on manual purge)"
}
if (-not (Get-ChildItem -Path $logsDir -ErrorAction SilentlyContinue)) {
    Fail 'msiexec' "log files did not survive uninstall"
}
Write-Host "== uninstall: OK (services + exes gone; configs and logs survive)"

Write-Host ""
Write-Host "verify-msi: OK ($($allServices.Count) services)" -ForegroundColor Green
