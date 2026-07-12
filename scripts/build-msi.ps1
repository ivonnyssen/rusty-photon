# build-msi.ps1 — build the rusty-photon Windows suite MSI on a Windows box
# (dev machine or windows-latest CI). The Windows analogue of
# scripts/build-packages.sh; operator guide: docs/packaging.md (Windows guide
# lands in W5); design: docs/plans/windows-packaging.md + ADR-015.
#
# Steps:
#   1. stage the pinned native SDKs into %LOCALAPPDATA%\rusty-photon-pkg\:
#      QHYCCD's qhyccd.lib import library for the delay-load link (exported as
#      QHYCCD_SDK_DIR — the DLL itself is operator-provided, ADR-013), and the
#      ZWO MIT blobs (ADR-014 per-device: zwo-camera -> ASICamera2,
#      zwo-focuser -> EAF_focuser). The cache dir doubles as the link path
#      (ZWO_SDK_LIB_DIR) and as the wix bindpath for the bundled DLLs,
#   2. release-build all 18 services CRT-static (the analogue of the Linux
#      RUNPATH injection — uniform, build-script-free). The two zwo services
#      each build in their OWN cargo invocation: cargo unifies features per
#      invocation, so batching them would re-union the per-device libzwo-sys
#      links and both binaries would need every blob again,
#   3. wix build (WiX v5 + Util/Firewall/UI extensions) over
#      installer/Package.wxs + installer/fragments/*.wxs,
#   4. collect dist\<version>\rusty-photon-<version>-x64.msi + SHA256SUMS.txt.
#
# Usage: scripts\build-msi.ps1 [-SkipSdkStaging] [-SkipBuild]
#   -SkipSdkStaging  offline rebuild: no downloads; requires the SDK cache
#                    from a previous run
#   -SkipBuild       reuse target\release binaries from a previous run and
#                    only re-run wix (installer-authoring inner loop)

[CmdletBinding()]
param(
    [switch]$SkipSdkStaging,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

function Die([string]$msg) {
    Write-Error "build-msi: $msg"
    exit 1
}

# ---- pins -------------------------------------------------------------
# The QHY version must match scripts/build-packages.sh QHY_SDK_VERSION, the
# sdk_win64_<ver> root in crates/qhyccd-rs/libqhyccd-sys/build.rs, and
# PINNED_SDK_VERSION in services/qhy-camera/src/preflight.rs
# (scripts/check-pkg-assets.sh enforces all three): the import lib linked here
# is the ABI statement the preflight/doctor report against.
$QhySdkVersion = "26.06.04"
$QhySha256Win64 = "dd696bce5f3a702ef55ad6ad7ae10a8f424879156675916387955170c3455347"
# QHY's download layout for >= 26.06.04: the directory is the dotless version.
$QhyUrlBase = "https://www.qhyccd.com/file/repository/publish/SDK/$($QhySdkVersion -replace '\.', '')"

# Must match the windows_*_sdk_url defaults in
# .github/actions/install-zwo-sdk/action.yml (checker-enforced). ZWO only
# publishes a rolling "latest" CDN download for Windows — no versioned URL to
# checksum (unlike the Linux blobs, which pin an indi-3rdparty commit).
$ZwoCameraSdkUrl = "https://dl.zwoastro.com/software?app=DeveloperCameraSdk&platform=windows86&region=Overseas"
$ZwoEafSdkUrl = "https://dl.zwoastro.com/software?app=DeveloperEafSdk&platform=windows86&region=Overseas"

# WiX v5 (ADR-015) + matching extension versions.
$WixVersion = "5.0.2"

# ---- environment checks -----------------------------------------------
if (-not (Test-Path "installer\Package.wxs")) { Die "run from the repo root" }
if (-not [Environment]::Is64BitOperatingSystem) { Die "x86_64 Windows only (ADR-015)" }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Die "cargo not found (install Rust via rustup)" }
if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) { Die "dotnet not found (the wix CLI is a .NET tool)" }

$hostTriple = (rustc -vV | Select-String '^host: (.*)$').Matches[0].Groups[1].Value
if ($hostTriple -notmatch 'x86_64-pc-windows-msvc') {
    # The delay-load link args in services/qhy-camera/build.rs are MSVC-only;
    # a GNU-toolchain build would silently drop the missing-DLL protection.
    Die "host toolchain must be x86_64-pc-windows-msvc (got: $hostTriple)"
}

# The real native links are mandatory in the shipped binaries: make sure no
# ambient sim/skip switches leak in from the invoking shell.
foreach ($switch in 'QHYCCD_SKIP_NATIVE_LINK', 'ZWO_SKIP_NATIVE_LINK') {
    if (Test-Path "env:$switch") {
        Write-Host "build-msi: clearing ambient $switch"
        Remove-Item "env:$switch"
    }
}

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"$' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $version) { Die "could not read the workspace version from Cargo.toml" }
if ($version -notmatch '^\d+\.\d+\.\d+$') {
    # MSI ProductVersion must be numeric major.minor.build.
    Die "workspace version '$version' is not a plain x.y.z (MSI ProductVersion requirement)"
}

# ---- SDK staging --------------------------------------------------------
$cache = Join-Path $env:LOCALAPPDATA "rusty-photon-pkg"
New-Item -ItemType Directory -Force -Path $cache | Out-Null

# Atomic download (no half-written file poisoning the cache).
function Fetch([string]$url, [string]$dest) {
    Write-Host "Downloading $url"
    Invoke-WebRequest -Uri $url -OutFile "$dest.part"
    Move-Item -Force "$dest.part" $dest
}

# --- QHY: qhyccd.lib (import library; the DLL is operator-provided) ---
$qhyExtract = Join-Path $cache "qhy-sdk-$QhySdkVersion-win64"
if (-not (Test-Path $qhyExtract)) {
    if ($SkipSdkStaging) { Die "-SkipSdkStaging set but $qhyExtract is missing" }
    $qhyZip = Join-Path $cache "sdk_win64_$QhySdkVersion.zip"
    if (-not (Test-Path $qhyZip)) { Fetch "$QhyUrlBase/sdk_win64_$QhySdkVersion.zip" $qhyZip }
    $actual = (Get-FileHash -Algorithm SHA256 $qhyZip).Hash.ToLowerInvariant()
    if ($actual -ne $QhySha256Win64) {
        Die "sha256 mismatch for sdk_win64_$QhySdkVersion.zip (expected $QhySha256Win64, got $actual)"
    }
    $tmp = Join-Path $cache "extract-qhy-$PID"
    Expand-Archive -Path $qhyZip -DestinationPath $tmp -Force
    Move-Item (Join-Path $tmp "sdk_win64_$QhySdkVersion") $qhyExtract
    Remove-Item -Recurse -Force $tmp
}
# Locate the import lib rather than hardcoding the archive layout (x64 only —
# never the Win32/x86 build).
$qhyLib = Get-ChildItem -Path $qhyExtract -Recurse -Filter "qhyccd.lib" |
    Where-Object { $_.FullName -notmatch '(?i)win32|\\x86\\' } |
    Select-Object -First 1
if (-not $qhyLib) { Die "qhyccd.lib not found under $qhyExtract" }
$env:QHYCCD_SDK_DIR = $qhyLib.DirectoryName
Write-Host "QHYCCD SDK $QhySdkVersion staged: QHYCCD_SDK_DIR=$($env:QHYCCD_SDK_DIR)"

# --- ZWO: per-device import libs + the DLLs the MSI bundles (ADR-014) ---
# No version in the rolling CDN URL, so the cache key is just "zwo-win64";
# delete the dir to force a refresh.
$zwoCache = Join-Path $cache "zwo-win64"
$zwoLib = Join-Path $zwoCache "lib"
$staged = @("ASICamera2.lib", "ASICamera2.dll", "EAFFocuser.lib", "EAF_focuser.dll") |
    ForEach-Object { Join-Path $zwoLib $_ }
if (($staged | Where-Object { -not (Test-Path $_) }).Count -gt 0) {
    if ($SkipSdkStaging) { Die "-SkipSdkStaging set but the ZWO cache at $zwoLib is incomplete" }
    $extract = Join-Path $zwoCache "extract"
    New-Item -ItemType Directory -Force -Path $extract, $zwoLib | Out-Null

    $asiZip = Join-Path $zwoCache "asi.zip"
    $eafZip = Join-Path $zwoCache "eaf.zip"
    Fetch $ZwoCameraSdkUrl $asiZip
    Fetch $ZwoEafSdkUrl $eafZip
    Expand-Archive -Path $asiZip -DestinationPath (Join-Path $extract "asi") -Force
    Expand-Archive -Path $eafZip -DestinationPath (Join-Path $extract "eaf") -Force
    # The per-arch import libs/DLLs live inside nested *Windows*SDK*.zip
    # archives; extract those too before staging.
    Get-ChildItem -Path $extract -Recurse -Filter "*Windows*SDK*.zip" | ForEach-Object {
        Expand-Archive -Path $_.FullName -DestinationPath (Join-Path $extract "nested-$($_.BaseName)") -Force
    }

    # Stage the x64 lib/DLL, picking the x64 build (not Win32/x86, not the
    # static lib, not demo/opencv payloads) — same selection rule as
    # .github/actions/install-zwo-sdk.
    function Stage([string]$pattern, [string]$target) {
        $hit = Get-ChildItem -Path $extract -Recurse -Filter $pattern -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -notmatch '(?i)win32|\\x86\\|opencv|demo|static' } |
            Sort-Object { $_.FullName -notmatch '(?i)\\x64\\|amd64' } |
            Select-Object -First 1
        if (-not $hit) { Die "$pattern not found in the ZWO Windows SDK" }
        Copy-Item $hit.FullName (Join-Path $zwoLib $target) -Force
        Write-Host "build-msi: staged $($hit.FullName) -> $target"
    }
    Stage "ASICamera2.lib" "ASICamera2.lib"
    Stage "ASICamera2.dll" "ASICamera2.dll"
    # ZWO names the focuser SDK EAF_focuser; libzwo-sys links `-lEAFFocuser`,
    # so the .lib is renamed for the linker. The .dll must KEEP its original
    # name: the import library embeds the DLL name it was generated from, so
    # the exe's import table asks the loader for EAF_focuser.dll.
    Stage "EAF_focuser.lib" "EAFFocuser.lib"
    Stage "EAF_focuser.dll" "EAF_focuser.dll"
    Remove-Item -Recurse -Force $extract
}
$env:ZWO_SDK_LIB_DIR = $zwoLib
Write-Host "ZWO SDK blobs staged: ZWO_SDK_LIB_DIR=$zwoLib"

# ---- build ----------------------------------------------------------------
# CRT-static (ADR-015 decision 7): no VC++ redistributable needed for our
# exes. Deliberately set here, not in a build.rs (which would ripple into
# Bazel/repin). Overrides any ambient RUSTFLAGS so the produced binaries do
# not depend on the invoking shell's environment.
$env:RUSTFLAGS = "-C target-feature=+crt-static"

$allServices = @(
    "sentinel", "ui-htmx", "filemonitor", "ppba-driver", "qhy-focuser",
    "sky-survey-camera", "star-adventurer-gti", "pa-falcon-rotator",
    "dsd-fp2", "qhy-camera", "pa-scops-oag", "rp", "session-runner",
    "plate-solver", "phd2-guider", "calibrator-flats"
)

if (-not $SkipBuild) {
    # The zwo services build in their own cargo invocations: cargo unifies
    # features per invocation, so batching zwo-camera (zwo-rs/camera) with
    # zwo-focuser (zwo-rs/focuser) would union the per-device libzwo-sys link
    # features (ADR-014) and both binaries would link — and need at runtime —
    # every SDK blob again. Everything else batches into one invocation.
    $batchArgs = $allServices | ForEach-Object { "-p", $_ }
    Write-Host "Building release binaries: $($allServices -join ', ')"
    cargo build --release @batchArgs
    if ($LASTEXITCODE -ne 0) { Die "cargo build failed" }
    Write-Host "Building release binaries: zwo-camera (isolated: per-device SDK link)"
    cargo build --release -p zwo-camera
    if ($LASTEXITCODE -ne 0) { Die "cargo build -p zwo-camera failed" }
    Write-Host "Building release binaries: zwo-focuser (isolated: per-device SDK link)"
    cargo build --release -p zwo-focuser
    if ($LASTEXITCODE -ne 0) { Die "cargo build -p zwo-focuser failed" }
}

foreach ($svc in $allServices + @("zwo-camera", "zwo-focuser")) {
    if (-not (Test-Path "target\release\$svc.exe")) { Die "target\release\$svc.exe missing after build" }
}

# ---- package -----------------------------------------------------------
if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
    Write-Host "Installing the wix CLI ($WixVersion)"
    dotnet tool install --global wix --version $WixVersion
    if ($LASTEXITCODE -ne 0) { Die "dotnet tool install wix failed" }
    # The dotnet tools dir may not be on PATH yet in this session.
    $env:PATH = "$env:USERPROFILE\.dotnet\tools;$env:PATH"
}
foreach ($ext in "WixToolset.Util.wixext", "WixToolset.Firewall.wixext", "WixToolset.UI.wixext") {
    wix extension add -g "$ext/$WixVersion" | Out-Null
    if ($LASTEXITCODE -ne 0) { Die "wix extension add $ext failed" }
}

$dist = "dist\$version"
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$msi = Join-Path $dist "rusty-photon-$version-x64.msi"

$sources = @("installer\Package.wxs") + (Get-ChildItem "installer\fragments\*.wxs" | ForEach-Object { $_.FullName })
Write-Host "wix build -> $msi"
# -sw WIX1149: the native ServiceConfig element draws an advisory about MSI
# SDK caveats, but it is the only declarative way to set
# SERVICE_CONFIG_FAILURE_ACTIONS_FLAG (util:ServiceConfig cannot), and
# verify-msi.ps1 behaviorally proves the flag works (see installer/README.md).
wix build -arch x64 `
    -sw WIX1149 `
    -d "Version=$version" `
    -ext WixToolset.Util.wixext `
    -ext WixToolset.Firewall.wixext `
    -ext WixToolset.UI.wixext `
    -bindpath "bin=target\release" `
    -bindpath "zwo=$zwoLib" `
    -bindpath "repo=." `
    -out $msi `
    @sources
if ($LASTEXITCODE -ne 0) { Die "wix build failed" }

# SHA256SUMS.txt: replace this MSI's entry, keep any others (the Linux
# packages regenerate their own entries in their own dist runs).
$sums = Join-Path $dist "SHA256SUMS.txt"
$msiName = Split-Path -Leaf $msi
$hash = (Get-FileHash -Algorithm SHA256 $msi).Hash.ToLowerInvariant()
$lines = @()
if (Test-Path $sums) {
    $lines = Get-Content $sums | Where-Object { $_ -notmatch [regex]::Escape($msiName) }
}
$lines += "$hash  $msiName"
# Explicit encoding: the file must be sha256sum-compatible plain ASCII on
# every host (Windows PowerShell 5.1's Set-Content default is ANSI, pwsh 7's
# is UTF-8 — pin it instead of depending on the invoking shell).
Set-Content -Path $sums -Value $lines -Encoding ascii

Write-Host ""
Write-Host "Package in ${dist}:"
Get-ChildItem $dist | ForEach-Object { Write-Host "  $($_.Name)" }
