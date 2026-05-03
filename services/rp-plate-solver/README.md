# rp-plate-solver

rp-managed service that wraps the ASTAP CLI and exposes a narrow HTTP
solve API to `rp`. Operator installs ASTAP separately (BYO per ADR-005);
`rp-plate-solver` ships no ASTAP binary or index database.

See [`docs/services/rp-plate-solver.md`](../../docs/services/rp-plate-solver.md)
for the design contract and [`docs/plans/rp-plate-solver.md`](../../docs/plans/rp-plate-solver.md)
for implementation sequencing.

## Status

This crate is at **Phase 4 complete** — HTTP server (`POST /api/v1/solve`,
`GET /health`), per-request supervision (graceful signal → 2 s grace →
force-kill), single-flight semaphore, and the full BDD suite are live.
ASTAP is invoked as an operator-installed subprocess per request; no
ASTAP binary or index database is bundled.

## Operator install

ASTAP is operator-supplied. The
[`install-astap`](../../.github/actions/install-astap/action.yml) GitHub
action is the canonical install recipe — operators can follow the same
per-OS table by hand:

- Linux: download `astap_command-line_version_Linux_*.zip` from
  SourceForge, place `astap_cli` somewhere in `$PATH`.
- macOS: same, plus `xattr -d com.apple.quarantine astap_cli` on first run.
- Windows: download `astap_command-line_version_win64.zip`, extract.

The D05 star database (~100 MB) is required for typical 0.6°–6° FOV
imaging; download from the same SourceForge project.

Configure `rp-plate-solver` to point at the install:

```json
{
  "bind_address": "127.0.0.1",
  "port": 11131,
  "astap_binary_path": "/opt/astap/astap_cli",
  "astap_db_directory": "/opt/astap/d05",
  "default_solve_timeout": "30s",
  "max_solve_timeout": "120s"
}
```

`astap_binary_path` and `astap_db_directory` are the only required
fields. Validation runs at startup; missing or unreadable values exit
the process non-zero so the operator's process supervisor surfaces the
misconfiguration rather than masking it with a silent retry.

## Running

```sh
rp-plate-solver --config /etc/rp-plate-solver/config.json
```

The wrapper prints `bound_addr=<host>:<port>` to stdout once it has
bound its listener (so test harnesses and process supervisors can
discover the actual port when `port: 0` is used). Logs go to stderr
via `tracing-subscriber`; set `RUST_LOG=debug` for verbose output.

Graceful shutdown: SIGTERM and SIGINT on Unix, Ctrl-C on Windows.

## Process-supervisor recipes

Phase 5 documents the operator's recovery mechanism: an OS-level
process supervisor that restarts the wrapper on crash or hang. Today
this is the canonical answer — automated Sentinel-driven restart of
rp-managed services is forward work and is **not** wired into the
current Sentinel design (see [Sentinel integration](#sentinel-integration)
below).

### Linux / systemd

`/etc/systemd/system/rp-plate-solver.service`:

```ini
[Unit]
Description=rp-plate-solver
After=network.target

[Service]
ExecStart=/usr/local/bin/rp-plate-solver --config /etc/rp-plate-solver/config.json
Restart=on-failure
RestartSec=2
User=rp
Group=rp
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

Restart command: `systemctl restart rp-plate-solver`.
Tail logs:     `journalctl -u rp-plate-solver -f`.

### macOS / launchd

`~/Library/LaunchAgents/com.rusty-photon.rp-plate-solver.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>           <string>com.rusty-photon.rp-plate-solver</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/rp-plate-solver</string>
    <string>--config</string>
    <string>/etc/rp-plate-solver/config.json</string>
  </array>
  <key>KeepAlive</key>       <true/>
  <key>StandardOutPath</key> <string>/var/log/rp-plate-solver.out.log</string>
  <key>StandardErrorPath</key><string>/var/log/rp-plate-solver.err.log</string>
</dict>
</plist>
```

Restart command: `launchctl kickstart -k gui/$(id -u)/com.rusty-photon.rp-plate-solver`.

### Windows / NSSM

[NSSM](https://nssm.cc/) wraps a Win32 binary as a Windows service:

```cmd
nssm install rp-plate-solver "C:\Program Files\rp-plate-solver\rp-plate-solver.exe"
nssm set     rp-plate-solver AppParameters "--config C:\ProgramData\rp-plate-solver\config.json"
nssm set     rp-plate-solver AppRestartDelay 2000
nssm start   rp-plate-solver
```

Restart command: `nssm restart rp-plate-solver`.

## Sentinel integration

The wrapper exposes `GET /health` as the standard HTTP health-probe
pattern. Today, **automated Sentinel-driven restart of rp-managed
services is not implemented** — Sentinel's existing design
([`docs/services/sentinel.md`](../../docs/services/sentinel.md)) covers
ASCOM Alpaca SafetyMonitor polling and Pushover notification, not
generic HTTP service supervision. The "Sentinel watchdog integration"
section in `docs/services/rp.md` is a forward-looking design.

Until Sentinel is extended:

- The operator's process supervisor (systemd / launchd / NSSM, recipes
  above) is the recovery mechanism. It restarts the wrapper on
  non-zero exit (config-validation failure, panic, etc.).
- Hangs in the *child* `astap_cli` process are bounded by the
  wrapper's per-request deadline; the wrapper itself doesn't hang on
  a wedged solve.
- `rp`'s HTTP client to the wrapper has its own outer timeout
  (`plate_solver.timeout_secs` in rp config) — even if the wrapper's
  internal deadline regresses, rp does not hang on a `plate_solve`
  call.

When future work extends Sentinel with HTTP `/health` polling and a
configurable per-service restart command, that mechanism will sit on
top of the same `/health` endpoint this service already exposes.

## Coordinates and units

- `ra_hint` and `ra_center`: **decimal degrees, 0–360** (matches FITS
  `CRVAL1`). The wrapper converts `ra_hint` to ASTAP's expected
  decimal hours (`degrees / 15`) before spawning.
- `dec_hint` and `dec_center`: **decimal degrees, −90 to +90**. The
  wrapper converts `dec_hint` to ASTAP's south-pole-distance
  (`90 + dec`) before spawning.
- `pixel_scale_arcsec`: arcseconds per pixel.
- `rotation_deg`: degrees.

Full HTTP contract in [`docs/plans/rp-plate-solver.md`](../../docs/plans/rp-plate-solver.md)
§"HTTP contract".
