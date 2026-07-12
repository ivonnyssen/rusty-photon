# Windows packaging & deployment guide

How to install, configure, and operate the rusty-photon Windows suite
installer on an imaging box. Architecture decisions live in
[ADR-015](decisions/015-windows-packaging-architecture.md) (one MSI, service
model, LocalSystem, config/log locations) and
[ADR-013](decisions/013-native-sdk-payload-policy.md) /
[ADR-014](decisions/014-zwo-per-device-split.md) (native camera-SDK
payloads); the full design in
[docs/plans/windows-packaging.md](plans/windows-packaging.md); the WiX
source contract in [installer/README.md](../installer/README.md). The Linux
guide is [docs/packaging.md](packaging.md).

## What gets installed

One `rusty-photon-<version>-x64.msi` for the whole family, downloaded from
the GitHub Releases page. The installer presents a feature tree:

- **Core** (required): `sentinel` (watchdog/notifications) and `ui-htmx`
  (web config UI). Any install includes them.
- **Drivers** (optional, off by default): one sub-feature per device
  driver.
- **Automation** (optional): `rp`, `session-runner`, `plate-solver`,
  `phd2-guider`, `calibrator-flats`.

Every selected service installs
`%ProgramFiles%\rusty-photon\rusty-photon-<svc>.exe` and registers a
Windows service named `rusty-photon-<svc>` (LocalSystem, auto-start)
with restart-after-5s failure actions — the systemd
`Restart=on-failure`/`RestartSec=5` parity the serial drivers' eager
hardware validation depends on — plus an inbound firewall exception on
its port:

| Service | Port | Feature ID | Notes |
|---------|------|------------|-------|
| filemonitor | 11111 | `Filemonitor` | Alpaca SafetyMonitor |
| ppba-driver | 11112 | `PpbaDriver` | serial (COM port) |
| qhy-focuser | 11113 | `QhyFocuser` | serial (COM port) |
| sentinel | 11114 | `Core` | dashboard: `/` |
| rp | 11115 | `Rp` | orchestrator API |
| sky-survey-camera | 11116 | `SkySurveyCamera` | config-gated (see below) |
| star-adventurer-gti | 11117 | `StarAdventurerGti` | serial (COM port) |
| pa-falcon-rotator | 11118 | `PaFalconRotator` | serial (COM port) |
| dsd-fp2 | 11119 | `DsdFp2` | serial (COM port) |
| ui-htmx | 11120 | `Core` | web config UI |
| qhy-camera | 11121 | `QhyCamera` | needs QHY's All-in-One pack (below) |
| zwo-camera | 11122 | `ZwoCamera` | its SDK DLL bundled |
| pa-scops-oag | 11123 | `PaScopsOag` | serial (COM port) |
| zwo-focuser | 11124 | `ZwoFocuser` | its SDK DLL bundled |
| phd2-guider | 11130 | `Phd2Guider` | wraps PHD2 (installed separately) |
| plate-solver | 11131 | `PlateSolver` | config-gated; needs ASTAP (below) |
| calibrator-flats | 11170 | `CalibratorFlats` | config-gated |
| session-runner | 11171 | `SessionRunner` | config-gated |

Alpaca UDP discovery is deliberately not served (as on Linux): point
clients (N.I.N.A. etc.) at `host:port` directly using the table above.

## Installing

Run the MSI and pick features in the tree, or install silently:

```text
msiexec /qn /i rusty-photon-<version>-x64.msi ADDLOCAL=ALL
msiexec /qn /i rusty-photon-<version>-x64.msi ADDLOCAL=Core,ZwoCamera,ZwoFocuser
msiexec /qn /i rusty-photon-<version>-x64.msi ADDLOCAL=Core,Drivers,Automation
```

Feature IDs are the table above plus the group features `Drivers` and
`Automation` (selecting a group selects all its children). `Core` is
always installed. Verify with:

```powershell
Get-Service rusty-photon-*
curl.exe http://localhost:<port>/management/apiversions   # Alpaca services
```

The binaries are unsigned pre-1.0, so SmartScreen shows an
"unrecognized app" warning on the interactive install — an accepted
finding (the moral equivalent of the Linux packages' accepted lintian
list). Azure Trusted Signing is the noted post-1.0 path.

**Config-gated services** (`sky-survey-camera`, `plate-solver`,
`calibrator-flats`, `session-runner`) have no sensible default config, so
they install with start type *Manual* — the Windows translation of the
Linux units' `ConditionPathExists=` gating. Write
`%ProgramData%\rusty-photon\<svc>.json` by hand, then:

```powershell
sc.exe config rusty-photon-<svc> start= auto
sc.exe start rusty-photon-<svc>
```

**Serial-device drivers** (`ppba-driver`, `qhy-focuser`,
`pa-falcon-rotator`, `pa-scops-oag`, `dsd-fp2`, `star-adventurer-gti`)
validate their hardware eagerly at startup and exit if the device is
missing — by design, so a broken device is never advertised on the
network. Until the device is attached (and its COM port matches the
config — the Windows default is `COM3`), the service sits in a
restart-every-5s loop driven by the failure actions; it comes up by
itself once the hardware appears. The cameras and the network-only
services serve with no hardware attached.

## Upgrading

Install the newer MSI — it performs a major upgrade: the old version is
removed, feature selections carry over, and self-created configs and logs
are untouched. Downgrades are blocked by the installer.

## Configuration

The MSI ships no config files. Daemons self-create their config on first
start at `%ProgramData%\rusty-photon\<svc>.json` (the Windows analogue of
the Linux `/etc/rusty-photon` path). Exceptions: the config-gated four
(above) never write one, and the two cameras run on built-in defaults
without writing a file until settings are saved (via ui-htmx
`config.apply`) or one is created by hand. To change settings:

```powershell
notepad $env:ProgramData\rusty-photon\<svc>.json
sc.exe control rusty-photon-<svc> paramchange   # reload-capable services
Restart-Service rusty-photon-<svc>              # the rest
```

Reload-capable (SCM `ParamChange`, the SIGHUP analogue): filemonitor,
ppba-driver, qhy-focuser, sky-survey-camera, pa-falcon-rotator,
pa-scops-oag, dsd-fp2, star-adventurer-gti, qhy-camera, zwo-camera.
Services with `config.apply` support (via ui-htmx) rewrite these files at
runtime — hand-edits and UI edits share the same file.

**ui-htmx driver map**: on first install the MSI seeds
`%ProgramData%\rusty-photon\ui-htmx.json` with a `drivers` map matching
the set of services actually installed (plus the `rp` target when rp is
selected), so the config UI works out of the box. The seed runs only if
the file does not exist — upgrades and re-installs never overwrite your
edits; after adding features to an existing install, add the new
service's entry by hand (or delete the file and let a repair re-seed it).

## Logs

Services log to rolling files
`%ProgramData%\rusty-photon\logs\<svc>.<date>.log` (daily rotation, 14
files retained) — under the SCM there is no usable stderr. Console runs
(`rusty-photon-<svc>.exe` from a terminal) log to stderr unchanged.

## Camera specifics

**qhy-camera** — QHYCCD's SDK is proprietary and never redistributed
(ADR-013). Install [QHY's All-in-One driver
pack](https://www.qhyccd.com/download/) first (needed for the signed
camera driver anyway); it provides the `qhyccd.dll` the service
delay-loads at startup. Without it the service logs an actionable
"qhyccd.dll not found" pointer and stops cleanly instead of crashing in
the loader. The Start-Menu shortcut **QHY Camera Doctor** (or
`rusty-photon-qhy-camera.exe doctor` in a console) diagnoses the
driver-pack/DLL state and reports the SDK version — note the service
builds against a pinned SDK, so the doctor's version report is the tool
for spotting ABI skew against whatever the All-in-One installed. Caveat:
`qhyccd.dll` itself needs `OpenCL.dll`, which ships with GPU drivers, not
Windows — on a box with no GPU driver the preflight fails even with the
All-in-One installed, and the doctor makes that visible.

**zwo-camera / zwo-focuser** — each feature bundles its own MIT-licensed
SDK DLL (`ASICamera2.dll` / `EAF_focuser.dll`, license in the install
dir), so nothing extra is needed for the *software* (ADR-014). ZWO
cameras additionally need [ZWO's signed camera driver
installer](https://www.zwoastro.com/downloads/); the EAF speaks inbox HID
and needs no vendor driver.

## plate-solver: ASTAP · phd2-guider: PHD2

Both wrap external programs that are deliberately not bundled: install
[ASTAP](https://www.hnsky.org/astap.htm) (plus a star database) and point
`astap_binary_path` / `astap_db_directory` in `plate-solver.json` at it;
install [PHD2](https://openphdguiding.org/) for phd2-guider.

## Removing

Remove single features (Apps → Installed apps → Modify, or
`msiexec /qn /i rusty-photon-<version>-x64.msi REMOVE=ZwoCamera`) or
uninstall entirely:

```text
msiexec /qn /x rusty-photon-<version>-x64.msi
```

Uninstall stops and deletes the services and removes the binaries, but
leaves self-created configs and logs in `%ProgramData%\rusty-photon`
(parity with `apt-get remove`). The "purge" analogue is manual: delete
that folder.

## Building and verifying the MSI

From a repo checkout on an x86_64 Windows box with Rust (MSVC host) and
the .NET SDK:

```powershell
scripts\build-msi.ps1                    # stage SDKs, build, wix build
scripts\build-msi.ps1 -SkipSdkStaging    # offline rebuild from cache
scripts\build-msi.ps1 -SkipBuild         # re-run wix only (installer loop)
scripts\verify-msi.ps1                   # elevated, on a disposable box
```

`build-msi.ps1` stages the pinned native SDKs (QHYCCD import lib for the
delay-load link; the ZWO MIT DLLs that become payloads), release-builds
all services CRT-static (no VC++ redistributable needed), and runs WiX
v5 over `installer/`. Artifacts land in `dist/<version>/` with a
`SHA256SUMS.txt`. `verify-msi.ps1` proves the full lifecycle — silent
install, per-service-class checks, failure-actions proofs, feature
remove, uninstall — and expects a box with no prior rusty-photon state
(CI uses `windows-latest`; don't run it on your imaging box).

CI runs both scripts in three places: the `msi` workflow on PRs touching
the packaging inputs and nightly (packaging rot — a vendor SDK URL going
stale, a runner image change — surfaces between releases), and
`release.yml`, where `verify-msi.ps1` gates the release upload.
