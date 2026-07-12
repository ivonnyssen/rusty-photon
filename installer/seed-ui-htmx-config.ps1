# seed-ui-htmx-config.ps1 — MSI custom action (deferred, elevated; see
# installer/Package.wxs). Seeds %ProgramData%\rusty-photon\ui-htmx.json with a
# `drivers` map entry per installed driver service, ONLY if the file does not
# already exist — ui-htmx's own self-created default lists a lone dsd-fp2,
# which is wrong for almost every install, and the installed feature set is
# something only the MSI knows.
#
# Ground truth is the rusty-photon-<svc>.exe set in this script's own
# directory: it runs after InstallFiles (and before StartServices), so the
# on-disk exes are exactly the end state of the transaction — no feature-state
# property plumbing needed. Ports and Alpaca device types are deterministic
# (fixed localhost ports; scripts/check-pkg-assets.sh asserts this table
# matches the WiX fragments). A Core-only install seeds an empty map — honest,
# unlike the phantom dsd-fp2 default.
#
# ui-htmx never rewrites an existing config file on its own (it only fills
# missing fields in memory via serde defaults), so whatever is seeded here
# stays verbatim until the operator or config.apply changes it.

$ErrorActionPreference = 'Stop'

$installDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$configDir = Join-Path $env:ProgramData 'rusty-photon'
$configPath = Join-Path $configDir 'ui-htmx.json'

if (Test-Path -LiteralPath $configPath) {
    Write-Output "seed-ui-htmx-config: $configPath exists; leaving it alone"
    exit 0
}

# service dir -> port + Alpaca device type (the ui-htmx BFF builds
# {base_url}/api/v1/{device_type}/0/action from these).
$driverMap = [ordered]@{
    'filemonitor'         = @{ port = 11111; device_type = 'safetymonitor' }
    'ppba-driver'         = @{ port = 11112; device_type = 'switch' }
    'qhy-focuser'         = @{ port = 11113; device_type = 'focuser' }
    'sky-survey-camera'   = @{ port = 11116; device_type = 'camera' }
    'star-adventurer-gti' = @{ port = 11117; device_type = 'telescope' }
    'pa-falcon-rotator'   = @{ port = 11118; device_type = 'rotator' }
    'dsd-fp2'             = @{ port = 11119; device_type = 'covercalibrator' }
    'qhy-camera'          = @{ port = 11121; device_type = 'camera' }
    'zwo-camera'          = @{ port = 11122; device_type = 'camera' }
    'pa-scops-oag'        = @{ port = 11123; device_type = 'focuser' }
    'zwo-focuser'         = @{ port = 11124; device_type = 'focuser' }
}

$drivers = [ordered]@{}
foreach ($svc in $driverMap.Keys) {
    if (Test-Path -LiteralPath (Join-Path $installDir "rusty-photon-$svc.exe")) {
        $drivers[$svc] = [ordered]@{
            base_url    = "http://127.0.0.1:$($driverMap[$svc].port)"
            device_type = $driverMap[$svc].device_type
        }
    }
}

$config = [ordered]@{ drivers = $drivers }

# With the orchestrator co-installed, an `rp` target enables ui-htmx's
# /equipment roster, /stream activity feed, and /config/rp pages; `{}` takes
# the built-in default base_url (http://127.0.0.1:11115).
if (Test-Path -LiteralPath (Join-Path $installDir 'rusty-photon-rp.exe')) {
    $config['rp'] = [ordered]@{}
}

New-Item -ItemType Directory -Force -Path $configDir | Out-Null

# WriteAllText writes UTF-8 WITHOUT a BOM — serde_json rejects a BOM, and
# PowerShell 5.1's Out-File -Encoding utf8 would add one.
$json = ($config | ConvertTo-Json -Depth 5) + "`n"
[System.IO.File]::WriteAllText($configPath, $json)

Write-Output "seed-ui-htmx-config: wrote $configPath ($($drivers.Keys.Count) drivers)"
exit 0
