# doctor — Service Design

## Overview

`doctor` is the standalone diagnosis-and-repair tool for a multi-service
rusty-photon install: packages put bytes on disk, services self-create their
default configs, and doctor makes the result coherent. It audits **service
facts** — ports, TLS, auth, service-to-service references, unit wiring — and
never learns device usage (which camera is the guide cam belongs to `rp`).
[ADR-016](../decisions/016-service-config-ownership-and-doctor.md) is the
decision record; [`docs/plans/service-config-doctor.md`](../plans/service-config-doctor.md)
tracks the phases.

This document specifies the **D2–D5 scope (diagnosis, repair, the no-SDK
hardware checks, and the per-service doctors) plus the D6a scope (TLS +
credential provisioning)**. A default run examines the config directory,
the platform's service manager, and the host's device surface (device
nodes, USB inventory, udev rules — all read-only), prints a report, and
writes nothing; `--fix` (D3) additionally applies the machine-applicable
fixes, runs the provisioning pass (D6a), and re-diagnoses; the
`tls`/`auth` subcommands (D6a) expose provisioning à la carte. From D5
on, every packaged service binary also answers `doctor` itself — its own
full-config validation, plus SDK-side hardware enumeration on the
services that link a vendor blob — and central doctor aggregates those
answers into the one report (§Per-service doctors). Cert renewal (D6b)
extends this binary later; its contract is recorded in the plan and
folded in here as it lands.

Doctor is a one-shot CLI, not a long-running service: no server, no config
file of its own, no unit. It lives at `services/doctor` (cargo binary
`doctor`) and is installed as `rusty-photon-doctor` when packaging arrives
(D7, riding in sentinel's package).

## Architecture

```mermaid
graph LR
    catalog["derived catalog<br/>services/*/pkg/doctor.toml<br/>(embedded at build time)"] --> doctor[doctor]
    configs["config dir<br/>&lt;svc&gt;.json × N"] --> doctor
    sm["service manager<br/>systemd / SCM / brew"] -->|read-only queries| doctor
    hw["device surface<br/>/dev, sysfs, group db, udev rules<br/>(via rusty-photon-doctor-checks)"] -->|read-only probes| doctor
    svc_up["running Alpaca service<br/>/management/v1/configureddevices"] -->|HTTP query| doctor
    svc_down["installed, stopped service<br/>rusty-photon-&lt;svc&gt; doctor --json"] -->|shell-out| doctor
    doctor --> report["report<br/>text or --json"]
```

Six inputs, one output. A default run is read-only: config files are
parsed but never written, every service-manager interaction is a query
(`list-unit-files`, `show`), never a verb, and every hardware probe is a
stat, a directory listing, or a read-only inventory query — doctor never
opens a device (ADR-016 decision 5: a running service holds its hardware;
doctor must never contend for it). Only `--fix` and the `tls`/`auth`
subcommands write — config files through `rusty_photon_config::save`'s
atomic path, plus the pki tree (D6a) — and nothing ever runs a
service-manager verb.

### The derived catalog

Doctor's catalog — which services exist, their class, and their default port —
is **derived from the packaging tree, not typed into doctor**
(ADR-016 decision 11). Each packaged service carries a metadata file,
`services/<svc>/pkg/doctor.toml`:

```toml
# Catalog metadata for rusty-photon-doctor (docs/services/doctor.md).
# This service's own unit tests assert these values match its config defaults.
class = "alpaca"  # "alpaca" | "core" — which shared server shape its config uses
port = 11113      # default port when the config file or server block is absent

# Optional hardware identity (§Hardware checks) — present only on services
# that talk to a device.
serial_pointer = "/serial/port"       # config JSON pointer holding the device path
serial_default_unix = "/dev/ttyACM0"  # effective path when the file or field is absent
serial_default_windows = "COM3"
usb_vendor = "1618"                   # USB idVendor, four lowercase hex digits
usb_product = "c179"                  # optional idProduct — omitted for vendor-only
                                      # families (a QHY camera is any 1618 device)
usb_model = "Q-Focuser"               # optional product-string substring — required
                                      # where the VID:PID is a generic bridge chip
                                      # shared across devices (FTDI FT-X, RP2040)
serial_gate_pointer = "/transport/kind"  # optional: the serial checks apply only
serial_gate_value = "usb"                # when this pointer holds this value
```

Everything else is derived from the directory name: the config file is
`<svc>.json`, the systemd unit is `rusty-photon-<svc>.service`, the Windows
service and brew formula are `rusty-photon-<svc>`.

Three guards keep the catalog honest:

1. **Each service tests its own file.** A unit test in each service crate
   (`include_str!("../pkg/doctor.toml")`) asserts the declared port equals its
   own `Config::default()` server port and the class matches the shape it
   embeds (`AlpacaServerConfig` vs `ServerConfig`). Services declaring serial
   metadata extend the same test: the pointer resolves in their own default
   config shape and the declared defaults equal their `DEFAULT_SERIAL_PORT`
   constants. A drifted copy fails that service's tests, not doctor's.
2. **Doctor embeds the files at build time** and a doctor unit test asserts
   the embedded set parses, ports are unique, and the table matches the files.
   Doctor also embeds the three shipped udev rules
   (`services/*/pkg/90-*.rules`) and asserts each rule-shipping service's
   declared `usb_vendor` equals the `ATTRS{idVendor}` its own rule matches —
   one source of truth for the USB checks, drift-guarded against the rule.
3. **A CI completeness check** asserts every `services/*/pkg` directory
   contains a `doctor.toml`, so a newly packaged service cannot silently stay
   out of the catalog.

USB identity declarations are measured from real hardware (the values a
device reports on the bus), so the code-parity guard cannot cover them;
the vendor-vs-rule assertion above and the on-rig verification leg do.
`qhy-focuser` and `star-adventurer-gti` carry no `usb_*` keys yet — their
identities get declared the day the hardware is measured on a USB port —
so the USB-presence check simply does not run for them; their device-node
checks work regardless.

The catalog today (17 packaged services; `session-runner` has no `pkg/` and
joins the catalog when it is packaged):

| Service | Class | Default port |
|---|---|---|
| filemonitor | alpaca | 11111 |
| ppba-driver | alpaca | 11112 |
| qhy-focuser | alpaca | 11113 |
| sentinel | core | 11114 |
| rp | core | 11115 |
| sky-survey-camera | alpaca | 11116 |
| star-adventurer-gti | alpaca | 11117 |
| pa-falcon-rotator | alpaca | 11118 |
| dsd-fp2 | alpaca | 11119 |
| ui-htmx | core | 11120 |
| qhy-camera | alpaca | 11121 |
| zwo-camera | alpaca | 11122 |
| pa-scops-oag | alpaca | 11123 |
| zwo-focuser | alpaca | 11124 |
| phd2-guider | core | 11130 |
| plate-solver | core | 11131 |
| calibrator-flats | core | 11170 |

Doctor itself never appears in the catalog: it is a one-shot binary with no
unit and no port, and when D7 gives it a `pkg/` directory the packaging
scripts and the completeness check gain a carve-out for unit-less packages.

### Config-root resolution

Doctor diagnoses one config directory per run, resolved in order:

1. `--config-dir <path>` — explicit, always wins.
2. `/etc/rusty-photon`, if it exists (Unix). Packaging ships this symlink
   pointing at the service user's tree
   (`/var/lib/rusty-photon/.config/rusty-photon`), so an operator running
   doctor as root diagnoses the **service user's** configs, not their own
   empty home. A packaged tree that exists but is **unreadable by the
   invoking user is a hard error** (exit 2, "run with sudo"), never a
   fall-through: the tree is 0750-owned by `rusty-photon`, and silently
   diagnosing the operator's own empty config directory instead would report
   seventeen missing configs on a healthy rig — and scan polkit rule
   directories it cannot read. A dangling symlink (packages removed) falls
   through to step 3.
3. The platform default the services themselves use —
   `rusty_photon_config`'s resolution (`~/.config/rusty-photon` on Linux,
   `~/Library/Application Support/rusty-photon` on macOS,
   `%ProgramData%\rusty-photon` on Windows).

Step 3 is what makes doctor useful on a dev checkout with no packages
installed.

### Platform inspectors

All service-manager knowledge sits behind one trait with a per-platform
implementation:

- **systemd** (Linux) — `systemctl list-unit-files 'rusty-photon-*'` for the
  inventory and enablement, `systemctl cat <unit>` for the
  `ConditionPathExists=` gate, `SupplementaryGroups=`, and the `ExecStart=`
  binary (the aggregation shell-out target), and `systemctl is-active` for
  the run state. Polkit facts come from a heuristic scan of
  `/etc/polkit-1/rules.d` and `/usr/share/polkit-1/rules.d` (the vendor dir
  the sentinel packages ship their rule to) for the manage-units action, the
  `rusty-photon-` unit prefix, and the `"rusty-photon"` user literal.
- **SCM** (Windows) — PowerShell over CIM (`Get-CimInstance Win32_Service`,
  not `Get-Service` — only the CIM class carries the image path) for the
  inventory, start mode, run state, and `PathName`.
- **brew services** (macOS) — `brew services list` filtered to
  `rusty-photon-*` formulas for the inventory and run state; the shell-out
  binary is the `brew --prefix`-linked `bin/<unit-stem>` when it exists.

The inspector reports a platform-neutral inventory (unit name, enabled,
active, the unit's binary, plus platform-specific facts where they exist);
checks that depend on a fact one platform lacks (systemd conditions, polkit,
systemd's `SupplementaryGroups=`) simply do not run on the other platforms.

### The hardware gatherer

The device-surface knowledge lives in the shared
`rusty-photon-doctor-checks` crate (ADR-016 decision 6: the similarity
between central doctor and D5's per-service doctors is a library, not a
binary). Doctor derives a probe list from the catalog and the scanned
configs — the effective serial paths, the expected udev rule files, the
firmware artifacts — and the crate gathers `HardwareFacts`, read-only:

- **Paths** — `stat` results (exists, file kind, mode, owner) for every
  probed path. Never an `open`.
- **USB inventory** — vendor:product plus the product string per device:
  sysfs (`/sys/bus/usb/devices/*/idVendor` …) on Linux, the
  `SYSTEM\CurrentControlSet\Enum\USB` registry tree plus the bus-reported
  device description on Windows, `system_profiler -json SPUSBDataType` on
  macOS.
- **Serial ports** (Windows) — `[System.IO.Ports.SerialPort]::GetPortNames()`.
- **Identity** — the `rusty-photon` user's uid/gid and the gid of every
  group the checks reference (udev `GROUP=` names, unit
  `SupplementaryGroups=` names), from the host's user/group database.
- **udev rules** (Linux) — the content of the *effective* installed copy of
  each expected rule file (`/etc/udev/rules.d` shadows `/usr/lib` and
  `/lib`, same as udev's own precedence).

For hermetic tests, the `mock` feature (the same convention drivers use)
enables a `--platform-facts <file>` flag: the file deserializes into the
inspector's output type and replaces the host queries, so BDD scenarios can
stage any host state on any OS. The flag does not exist in release builds.
A staged facts file may include a `hardware` object; when it does not, the
hardware checks are skipped entirely — a staged file is the whole truth of
its scenario, and probing the real host underneath a mock would make every
scenario's outcome depend on the machine running it. A real (non-mock) run
always gathers.

### Packaged host vs dev checkout

If the inspector finds **zero** `rusty-photon-*` units, the host is treated as
a dev checkout: doctor runs the config-only checks (parse, shapes, ports,
joins, TLS paths) against whatever config files exist, skips the unit-joined
checks, and says so in the report (`mode: "config-only"`). Inventory
mismatch checks (orphan configs, unit-without-config) only make sense against
a package inventory and run only in `mode: "packaged"`.

## Diagnosis — the D2 checks

Every check yields `ok`, `warn`, or `fail` plus a human-readable detail and,
where doctor can suggest one, a concrete remedy (as text — machine-applicable
fixes arrive with D3). Checks are service-scoped where applicable so the
report groups naturally.

### Inventory (packaged mode only)

| Check | Status | Trigger |
|---|---|---|
| `inventory.unit-without-config` | warn | A `rusty-photon-*` unit is installed but `<svc>.json` does not exist. The service has never started (it self-creates config on first run) — or its state directory is wrong. |
| `inventory.config-without-unit` | warn | `<svc>.json` exists for a catalog service whose unit is not installed. Leftover from a removed package, or a hand-copied file. |
| `inventory.unknown-config` | warn | A `*.json` in the config dir matches no catalog service and no known non-service file (`acme.json`; the `pki/` tree is ignored). Catches typo'd filenames that a service will silently never read. |
| `inventory.unit-and-config` | ok | Unit installed and config present — the healthy pairing, reported so an empty report is never mistaken for a clean one. |

### Config parsing

| Check | Status | Trigger |
|---|---|---|
| `config.unreadable` | fail | `<svc>.json` exists but could not be read (permissions, I/O) — a different operator problem than bad JSON, diagnosed under its own name. |
| `config.json-syntax` | fail | `<svc>.json` is not valid JSON. The service will refuse to start (by design — corrupt config never silently resets), and doctor says so before the next night does. |
| `config.server-shape` | fail | The top-level `server` block does not parse under the catalog-declared shape (`ServerConfig` for core, `AlpacaServerConfig` for Alpaca): unknown keys (`deny_unknown_fields`), missing `port` when the block is present, `discovery_port` on a core service, malformed `bind_address`. An absent `server` block is `ok` — the service applies its defaults. |
| `config.known-blocks` | fail | One of the cross-reference blocks doctor joins across fails to parse: sentinel's `operation_watchdog`, rp's `equipment` array / `session` block. Everything else in every file is opaque `serde_json::Value` doctor steps around (ui-htmx's whole file included — its view reads only the retired `drivers` key). |
| `config.retired-keys` | fail | A config still carries a key its service retired and now refuses to start over (`deny_unknown_fields`): sentinel's `services` map (D3s — supervision is discovered, not configured) or ui-htmx's whole `drivers` override map (#569 — rp's equipment roster is the only device source). The remedy is deletion — no replacement config exists. |

Full-config typo detection (a misspelled key in, say, qhy-camera's
`device_overrides`) is **out of D2's reach by design**: doctor knows only the
shared blocks. It arrives with D5, where each service's own binary — which has
the typed shape — validates its own file and doctor aggregates.

### Ports

| Check | Status | Trigger |
|---|---|---|
| `ports.collision` | fail | Two services resolve to the same **effective** port. Effective = the configured `server.port`, else the catalog default. A service is in the collision set when its unit is installed or its config file exists. |
| `ports.discovery-collision` | fail | Two or more Alpaca configs set the same `discovery_port`. The responder is a per-host opt-in for single-driver deployments precisely because N responders collide; two enabled is always a mistake. |

### Units and privileges (systemd facts; run where the platform has them)

| Check | Status | Trigger |
|---|---|---|
| `units.config-gated` | fail | A unit is enabled but its `ConditionPathExists=` file is missing: installed, enabled, and silently inert. Today that is sky-survey-camera, plate-solver, calibrator-flats, and phd2-guider, all of which hard-require a config file. |
| `sentinel.privilege-path` | fail | Sentinel's unit is installed and no rule under `/etc/polkit-1/rules.d/` or `/usr/share/polkit-1/rules.d/` (where the sentinel packages ship theirs) grants the `rusty-photon` user `org.freedesktop.systemd1.manage-units` for `rusty-photon-*` units — the packaged unit runs unprivileged with `NoNewPrivileges=yes`, so every restart sentinel attempts will be denied at the privilege boundary. Points at the scoped rule from [#523](https://github.com/ivonnyssen/rusty-photon/issues/523). Detection is a heuristic (scan for the action id, unit prefix, and user literal in the rules files) and the detail says so. |

### Name joins

Since D3s sentinel discovers its services from the platform service manager,
so the one service-name join that survives resolves against the **installed
`rusty-photon-*` units** (packaged mode only — the join has nothing to
resolve against on a dev checkout): the watchdog's
`operations.<family>.service`, matched by convention and validated by
nothing at runtime until the 2am 404. (ui-htmx's restart names stopped
being configured with #569 — they are derived at request time by matching a
roster device's `alpaca_url` port against sentinel's discovered services,
so there is no ui-htmx-side name join left to check.)

| Check | Status | Trigger |
|---|---|---|
| `joins.watchdog-service` | fail | An `operation_watchdog.operations.<family>.service` names a service with no installed `rusty-photon-<service>` unit — sentinel's discovery will never resolve it, so the watchdog's ladder degrades to notify-only. |

### URL conventions

| Check | Status | Trigger |
|---|---|---|
| `urls.spurious-suffix` | warn | An rp `equipment[].alpaca_url` ends in `/api/v1`. The client appends it itself; doubling it 404s. (Doctor reads `alpaca_url` out of rp's equipment entries and steps around the rest of the block — checking the URL is service wiring, owning the entry is device usage.) Sentinel's URLs are all derived since D3s, and ui-htmx's device URLs come from rp's roster since #569 — no other client-side convention is left to check. |

### TLS and auth

| Check | Status | Trigger |
|---|---|---|
| `tls.paths` | fail | A `server.tls` block is present but the cert or key is not an existing **file**, after resolving the path the way the service itself will (`TlsConfig::resolved_*_path`, which expands `~`; a relative remainder anchors at the config dir; empty paths and directories are absent). Readability by the unit's user is not checked in D2 — doctor runs privileged on packaged hosts, so an ownership heuristic needs the passwd machinery D4's hardware checks bring. |
| `tls.auth-without-tls` | warn | `server.auth` is set while `server.tls` is absent: HTTP Basic credentials in cleartext on the wire. Legal, but worth a nag — ADR-003's scheme is Basic **over TLS**. Fixed by the provisioning pass turning TLS on. |
| `tls.absent` (D6a) | warn | An installed service has no `server.tls` block: it serves plain HTTP. Legal (absent still means off — ADR-016 decision 10(d)), and fixable: the provisioning pass issues a cert and writes the block. |
| `auth.absent` (D6a) | warn | An installed service has no `server.auth` block: it answers unauthenticated. Same legality and fix as `tls.absent`. |
| `auth.mismatch` (D6a) | warn | A client auth block's plaintext password does not verify (Argon2id) against the target service's `server.auth` hash — the client will get 401s. Suggestion-only: hand-set credentials are operator intent, so doctor reports the pair and suggests `doctor auth rotate` to re-align everything to the observatory credential. |

### Platform defaults

| Check | Status | Trigger |
|---|---|---|
| `rp.data-directory` | fail | rp's `session.data_directory` does not exist — catches the Linux-path-on-macOS default documented in `docs/packaging-macos.md` — or, on a packaged **Linux** install, exists but is not writable by the `rusty-photon` user rp runs as under systemd, judged from the directory's owner/group/mode. Doctor writes no probe files — the judgment is an ownership heuristic, and the detail says so. Dev checkouts keep the existence-only check (the operator owns their own tree), as do macOS and Windows installs — brew services run as the operator and the MSI's services as LocalSystem, so there is no service user to judge for. (`tls.paths` readability keeps its D2 shape; the TLS lifecycle work owns that check's evolution.) |

### Hardware (D4 — the no-SDK checks)

Everything here needs no vendor blob (ADR-016 decision 5); SDK-side checks
are D5's per-service `doctor` subcommands. One severity rule covers the
whole family: **`fail` when the service's unit is installed and enabled** —
it will start at boot and hit the problem, so this is tomorrow's 2am
failure reported at noon — **`warn` otherwise** (a parked service, or a dev
checkout). A check runs only for services that participate in diagnosis
(config present or unit installed) and declare the relevant catalog
metadata.

| Check | Platforms | Trigger |
|---|---|---|
| `hardware.serial-node` | Linux, macOS, Windows | The effective serial device — the config value at the catalog's `serial_pointer`, else the platform's declared default — does not exist, or exists but is not a character device (Unix). On Windows: the configured name is not among the host's present COM ports. A service with a `serial_gate_pointer` participates only while its config holds the gate value (star-adventurer-gti on `kind: "udp"` has no serial device to check — the same pointer is a UDP port number there). |
| `hardware.serial-access` | Linux (packaged) | The node exists but the `rusty-photon` user cannot open it, judged from the node's owner/group/mode, the user's uid/gid, the unit's `SupplementaryGroups=`, and the host's group database. Also fires when the unit file itself is missing the `SupplementaryGroups=` entry its access depends on — group membership is conferred per-unit, **not** via `/etc/group`, so that file is checked only for group *existence*, never membership. |
| `hardware.usb-device` | Linux, macOS, Windows | No device on the bus matches the service's declared USB identity: `usb_vendor`, plus `usb_product` when declared, plus `usb_model` as a product-string substring when declared. The substring is what makes the check honest for devices behind generic bridge chips — the three Pegasus devices all report FTDI's `0403:6015` and the FP2 reports the RP2040's `2e8a:000a`, so VID:PID alone would confuse "the Falcon is plugged in" with "the PPBA is plugged in". |
| `hardware.udev-rule` | Linux (packaged) | For each service shipping a udev rule, against the effective installed copy: the file is missing (`fail`/`warn` per the severity rule); a `GROUP=` it names does not resolve in the host's group database — udev **silently drops the entire rule line** on an unresolvable `GROUP=`, so file presence alone proves nothing (`fail`/`warn`); or the content differs from the packaged copy doctor embeds (`warn` always — an operator override in `/etc/udev/rules.d` is legitimate, but worth surfacing). |
| `hardware.firmware-helper` | Linux (packaged) | qhy-camera's unit is installed but the firmware helper's three artifacts are not all present: `/lib/firmware/qhy/` (directory), `/usr/local/sbin/fxload` (executable), `/etc/udev/rules.d/85-qhyccd.rules` (file). The conjunction is the helper's own idempotency gate — any subset is a partial install that must re-converge — and the suggestion points at `/usr/sbin/rusty-photon-qhy-firmware-install` (ADR-013: proprietary firmware is never packaged, so nothing but this check verifies the operator ran it). |

None of these checks plan a `--fix`: absent hardware cannot be conjured, a
missing group or udev rule is package/host surgery outside config files,
and the firmware helper needs network and root. Every failure carries a
concrete suggestion instead.

## Per-service doctors and aggregation (D5)

The D4 checks stop exactly where a vendor SDK starts (ADR-016 decision 5:
a doctor that links every vendor blob recreates the file conflict ADR-014
fixed). D5 crosses that line from the other side: **every packaged service
binary answers `doctor` itself**, using only what it already links, and
central doctor aggregates the answers.

### The per-service subcommand

```
rusty-photon-<svc> doctor [--config <file>] [--json]
```

Every catalog service binary carries the subcommand. It is additive to the
existing CLI — a plain flag invocation (what the units run) still starts
the service — and the run is strictly read-only: it resolves the config
path the way the service itself does (`--config`, else
`rusty_photon_config::resolve_config_path`), but never materializes the
scaffold or mints identities. Output and exit codes follow central
doctor's contract exactly (text by default, `--json` for the schema, exit
0/1/2), with `mode: "service"` and the service binary's own version in
`doctor_version`.

Two checks, the second only on services that link an SDK:

| Check | Services | Trigger |
|---|---|---|
| `config.full-shape` | all | The service's config file does not survive the service's **own typed load path** — the same parse a start would perform, `deny_unknown_fields` included, so a typo'd key anywhere in the file is named here (the detail carries serde's path and message). This is the full-config validation D2 deferred: central doctor knows only the shared blocks; the binary that owns the shape validates all of it. An absent file is `ok` — most services write their defaults on first start, config-gated ones stay inert, and either way there is nothing to validate (central doctor's inventory and `units.config-gated` checks own that judgment). |
| `hardware.sdk-devices` | qhy-camera, zwo-camera, zwo-focuser | The SDK cannot see the hardware: enumeration (`ScanQHYCCD`, the ASI/EAF list calls) errors (`fail` — with the D4 `hardware.usb-device` check this splits "not on the bus" from "on the bus but the SDK is blind", which is the firmware/driver signature), or enumerates zero devices (`warn` — the binary cannot see unit state, so unplugged-on-purpose and unplugged-by-accident are indistinguishable here; the D4 checks carry the unit-aware severity). `ok` lists the models the SDK reports. **Enumeration only, never an open** — an open against a device the running service holds is the camera-lock class of bug, and the subcommand must stay safe to run by hand at any time. |

Per-service checks plan no fixes in D5 — machine-applicable repair stays a
central-doctor concern, so `--fix` semantics live in exactly one binary.

The shared machinery — schema, check constructors, config-load harness,
text/JSON rendering, exit-code mapping — lives in
`rusty-photon-doctor-checks` (ADR-016 decision 6: the similarity between
central doctor and the per-service doctors is a library, not a binary).
With D5 that crate becomes the **canonical home of the report schema**,
and central doctor re-exports it; a service's subcommand is its clap
variant plus a closure handing its typed load to the shared runner.

### Aggregation — the two probe paths

On a packaged host, central doctor extends its diagnosis through each
installed service, and the two paths are naturally exclusive:

- **Unit active** → the service already enumerated its hardware at
  startup, so doctor asks it over HTTP: `GET
  /management/v1/configureddevices` against the service's effective port
  (Alpaca-class services only — core services expose no management API and
  are fully covered by the config-side checks). The connection follows the
  service's own config: HTTPS when its `server.tls` is set (doctor's
  root-of-trust from the D6 lifecycle), credentials from the D6 machinery
  when `server.auth` is on. `service.devices` reports the inventory; an
  active unit that does not answer its own port is a `fail` (that is
  tomorrow's 2am failure); an authenticated endpoint doctor holds no
  credential for is a `warn` — the answer proves liveness but not
  inventory.
- **Unit installed but not active** → nothing holds the device, and this
  is precisely the case being debugged: doctor shells out to the unit's
  own binary — the path the service manager records (systemd `ExecStart=`,
  the SCM image path, the brew formula bin), `doctor --json --config
  <dir>/<svc>.json` — and merges the returned checks into the report with
  the `service` field set. The parse is permissive in both directions
  (ADR-016 decision 7): unknown fields, statuses, and ops degrade instead
  of refusing. A binary that predates the subcommand (version skew across
  nightlies) fails the shell-out without producing a report; that is a
  `warn` under `service.doctor-probe` naming the skew, never a `fail` —
  an old binary is not a broken rig.

Both paths are bounded: the HTTP probe by a short request timeout, the
shell-out by a generous one (an SDK bus scan takes seconds). A timeout is
reported under the same names — an answer that never comes is a diagnosis,
not a crash.

For hermetic tests the mock seam extends accordingly: staged unit facts
carry the binary path (BDD points it at a stub that emits a canned
report), and the HTTP probe targets whatever port the staged config
declares, so a scenario can stand up a stub management endpoint.

## Repair — `--fix` (D3)

`doctor --fix` runs the same diagnosis, applies every **machine-applicable
fix** the checks planned, then re-runs the diagnosis and reports the
post-fix state. The loop an operator runs after installing packages is:
services self-create their defaults, `doctor --fix` makes the install
coherent, done.

A fix is planned only where the correct value is derivable, not a judgment
call:

| Check | Fix `--fix` applies |
|---|---|
| `ports.collision` | Move each colliding service whose configured port differs from its catalog default back to that default — but only when the default itself is free among the effective ports. A collision between judgment-call ports (two services deliberately moved to the same custom port) gets a suggestion, not a fix. |
| `config.retired-keys` | Delete the retired key (sentinel's `services` map; ui-htmx's `drivers` map). |

Everything else stays suggestion-only: a `ConditionPathExists` gate needs a
hand-written config, a `discovery_port` collision is operator intent (which
host keeps the responder?), and rp's `session.data_directory` is a placement
decision. Missing TLS material and absent `tls`/`auth` blocks stopped being
suggestion-only with D6a — the provisioning pass (next section) conjures
them.
`--fix` also never *generates* config — device targets come from rp's
roster, never from doctor-written entries (see [ui-htmx.md](ui-htmx.md)).

Write mechanics:

- Fixes are grouped per file and applied as one read-modify-write through
  `rusty_photon_config::save` — the same atomic
  temp→fsync→rename→fsync-dir path the services' own `config.apply` uses,
  so a crash mid-fix never corrupts a config. Every field doctor does not
  touch is preserved — the mutation is on the raw JSON value, not a typed
  round-trip — though `save` normalizes formatting to the same
  pretty-printed shape `config.apply` writes.
- The write preserves the replaced file's **owner and mode** across the
  rename's inode swap. On a packaged Linux install the config root is
  `0750 rusty-photon:rusty-photon`, so doctor runs under sudo — without
  the preservation every fixed config would come out root-owned and
  unreadable by its service. A failed chown aborts the write rather than
  proceeding: dropping the service user's access is worse than not
  writing at all.
- Doctor reads files directly and holds no override layers, so a fix can
  never bake a transient value into a file (the plan's layer-aware persist
  rule is satisfied by construction).
- **Services may be running while `--fix` writes** — that is the canonical
  install flow, so doctor warns rather than refuses: atomic renames make
  corruption impossible, and the residual race (a driver's own
  `config.apply` landing between doctor's read and write loses one of the
  two writes) is called out on stderr in packaged mode with the advice to
  re-run doctor afterwards. Services read config at startup, so applied
  fixes take effect on each service's next restart.
- `--fix` is **idempotent**: the post-fix diagnosis plans no further fixes,
  and a second `--fix` run applies nothing.

## Provisioning — TLS and the observatory credential (D6a)

Doctor owns the TLS and credential lifecycle (ADR-016 decision 10; the
settled sub-decisions are in the plan's D6 section). The provisioning code —
self-signed issuance, ACME, DNS-01 — lives inside doctor as modules; the
serving half every service links is the `rusty-photon-tls` crate (renamed
from `rp-tls`), which carries none of the `cloudflare`/`instant-acme`
dependency tree. `rp init-tls` and `rp hash-password` are removed; their
functionality lives here.

### The pki tree

All TLS material and the credential live under **`<config-root>/pki`** —
the same resolved config root as the service configs (`--config-dir` >
`/etc/rusty-photon` symlink > platform default), so a scratch `--config-dir`
scopes provisioning the same way it scopes diagnosis, and on a packaged
Linux host the tree is `/var/lib/rusty-photon/.config/rusty-photon/pki`.
One tree, one symlink, one thing to back up. (`~/.rusty-photon/pki` is
retired; pre-1.0, no migration shim — a config still pointing at old
material keeps working, since `tls.paths` checks the configured paths,
wherever they point.)

```
<config-root>/
  pki/
    ca.pem            # self-signed CA, 10-year validity, create-if-absent
    ca-key.pem        # 0600
    <svc>.pem         # per-service cert, 10-year, SANs: hostname + localhost + extras
    <svc>-key.pem     # 0600
    credential        # observatory credential plaintext, 0600 — the canonical copy
  acme.json           # ACME account/config state (0600), alongside the configs
```

Key files and `credential` are `0600` and owned by the service user on
packaged hosts (doctor runs privileged there and chowns what it writes).
The CA is **never regenerated** while `ca.pem` exists — replacing it
invalidates every distributed trust anchor, so that is an explicit operator
act (delete the file, re-run).

### The observatory credential

One credential for the whole observatory (ADR-016 decision 10(e)):
username **`observatory`**, password minted with ≥128 bits of entropy. The
canonical plaintext copy is `pki/credential`; because doctor mints it, it
can write **both forms everywhere they belong**:

- the **Argon2id hash** into each installed service's `server.auth`;
- the **plaintext** into each client auth block — rp's `equipment[].auth`
  entries, sentinel's service-probe `auth`, ui-htmx's `rp`/`sentinel`
  targets — alongside the CA path each client trusts.

`doctor auth rotate` overwrites `pki/credential` with a fresh mint and
re-runs the same distribution; services pick the new `server.auth` up at
their next restart (rotation is operator-initiated and rare — no reload
machinery). A forgotten credential is recovered by reading
`pki/credential`, or by rotating.

### What `--fix` adds

After the config fixes, `--fix` runs the provisioning pass over every
installed service:

1. **Certs** — create the CA if absent; issue a cert for each installed
   service whose `<svc>.pem`/`<svc>-key.pem` pair is missing. Existing
   material is never touched.
2. **Credential** — reuse `pki/credential` if present, else mint and write
   it. A service installed after the first `--fix` run is wired with the
   *same* credential on the next run.
3. **Config writes** — where a service's `server.tls` is absent, write the
   block pointing at the issued pair; where `server.auth` is absent, write
   `observatory` + the hash. Client blocks that are absent get the
   plaintext + CA path. **Present blocks are never overwritten** — a
   hand-set credential or hand-placed cert path is operator intent;
   incoherence surfaces as `auth.mismatch`/`tls.paths`, suggestion-only.

The same "absent means off" contract from decision 10(d) is what makes this
safe: packages start services before any doctor run, and BDD/ConformU
configs that omit `tls`/`auth` keep meaning plain HTTP. After one `--fix`,
every wired service is TLS-on and auth-on, and services pick both up at
their next restart (the existing `--fix` restart advice covers it).

### Subcommands

- **`doctor tls issue`** — the cert step alone (CA-if-absent + missing
  service certs), without touching configs. `--services <name>...`
  restricts to named services; `--extra-san <host-or-ip>...` adds SANs;
  `--force` re-issues service certs from the existing CA (never the CA
  itself). The service set defaults to the installed set, derived from the
  catalog — `rp_tls::cert::DEFAULT_SERVICES` (five hand-typed names of
  eighteen) is retired.
- **`doctor tls issue --acme --domain <d> --dns-provider <p> --dns-token <t>
  --email <e> [--staging]`** — the ACME path, unchanged in mechanism from
  `rp init-tls --acme` (DNS-01, wildcard cert, account state persisted to
  `acme.json`). Publicly-trusted certs need no CA distribution to clients.
- **`doctor auth rotate`** — mint + distribute, as above.
- **`doctor auth hash-password [--stdin]`** — hash one password for
  hand-written configs (the third-party-driver escape hatch); prompt with
  confirmation, or read stdin.
- **`doctor tls renew`** arrives with D6b (one-shot renewal for a platform
  scheduler, plus the in-process cert hot-reload in `rusty-photon-tls`).

Subcommands report through the same report schema (`--json` works on all
of them); provisioning actions appear in `fixes_applied` as non-pointer
operations (`generate-ca`, `generate-cert`, `mint-credential`) next to the
config-pointer ops, which the permissive parse lets older consumers skip.

## Report

One schema, shared by the text renderer and `--json`, and — from D5 on — by
the per-service `doctor` subcommands central doctor aggregates. Its
canonical home is `rusty-photon-doctor-checks` (central doctor re-exports
it), because from D5 every service binary serializes it. It versions and
parses **permissively** (`#[serde(default)]`, unknown fields tolerated,
unknown `status`/`mode`/`op` values degrade to explicit `Unknown`
variants) — the inverse of the config convention, so a doctor and a
service from different nightlies degrade to a partial report instead of
refusing to run (ADR-016 decision 7). `mode` is `"service"` in a
per-service report; `doctor_version` always carries the **emitting**
binary's version, which is what makes version skew visible in an
aggregated report.

```json
{
  "schema_version": 1,
  "doctor_version": "0.1.0",
  "mode": "packaged",
  "config_dir": "/var/lib/rusty-photon/.config/rusty-photon",
  "checks": [
    {
      "name": "ports.collision",
      "service": "qhy-focuser",
      "status": "fail",
      "detail": "qhy-focuser and dsd-fp2 both resolve to port 11113 (qhy-focuser: configured; dsd-fp2: configured)",
      "suggestion": "set a distinct server.port in dsd-fp2.json (default 11119)",
      "fixes": [
        { "op": "set-number", "service": "dsd-fp2", "pointer": "/server/port", "value": 11119 }
      ]
    }
  ]
}
```

- `fixes` (per check, empty and omitted when none) carries the
  machine-applicable fix plan as primitive JSON-pointer operations —
  `set-number`, `set-string`, `set-object` (D6a, for whole
  `server.tls`/`server.auth`/client blocks), `remove-key` — against one
  service's config file. Primitive ops keep the schema forward-parseable:
  an aggregating doctor that does not recognize a newer op simply cannot
  apply it, and says so, instead of misparsing the check.
- `fixes_applied` (top level, populated by `--fix` runs) records what was
  actually written, one entry per applied fix: the originating check name
  and the operation (which carries the service). Like `fixes`, it is
  omitted when empty — a consumer must treat the missing field as an empty
  list, which the permissive parse does by construction. On a `--fix` run
  the `checks` array is the **post-fix** diagnosis — the exit code and the
  report describe the state the operator is left with.

`ok` checks are included (an empty report is indistinguishable from a doctor
that skipped everything); the text renderer summarizes them, prints
`warn`/`fail` in full, and lists applied fixes.

## CLI contract

```
doctor [--config-dir <path>] [--json] [--fix]
doctor tls issue [--acme --domain <d> --dns-provider <p> --dns-token <t> --email <e> [--staging]]
                 [--services <name>...] [--extra-san <host-or-ip>...] [--force]
doctor auth rotate
doctor auth hash-password [--stdin]
```

- Default run (no subcommand) diagnoses and prints the human-readable
  report to stdout; it writes nothing. On a packaged host the diagnosis
  includes the per-service probes (§Aggregation); a dev checkout has no
  units to probe.
- `--fix` applies the machine-applicable fixes (see §Repair) and the
  provisioning pass (see §Provisioning), re-diagnoses, and reports the
  post-fix state.
- `--json` prints the report JSON instead, on every subcommand.
- `--config-dir` scopes subcommands too — the pki tree anchors at the
  resolved config root.
- `--platform-facts <file>` exists only under the `mock` feature (tests).
- Logging goes to stderr via `tracing` (`debug!` throughout; the report is
  the product, not the log).

The per-service subcommand (`rusty-photon-<svc> doctor [--config <file>]
[--json]`, §Per-service doctors) mirrors this contract: same output modes,
same exit codes, no flags beyond these.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | Diagnosis ran; no `fail`-status checks (warnings allowed). After `--fix`: the post-fix state is clean. |
| 1 | Diagnosis ran; at least one check failed (after fixes, on a `--fix` run). |
| 2 | Doctor itself could not run (unresolvable config dir, inspector error, or a fix write failed). |

Scripts and CI can gate on "the rig is coherent" without parsing JSON.

## Configuration

Doctor has **no config file**. Its inputs are the flags above. This is
deliberate: a config-repair tool with its own config file would need a
doctor. (`acme.json` is provisioning *state* doctor writes and reads back —
account URL, domain, DNS provider — not configuration of doctor's own
behavior; every knob in it was a CLI flag first.)

## Verification

- **Unit** — catalog parsing/uniqueness and the per-service `doctor.toml`
  parity tests (now including serial pointer/defaults against each service's
  own config shape, and `usb_vendor` against each shipped udev rule);
  per-check tests against tempdir fixtures; the `doctor-checks` crate's pure
  predicates against staged facts, and its gatherer against fake root trees;
  report schema round-trip including a forward-compatibility case (unknown
  fields, unknown status/mode/op values from a newer binary); the shared
  service-doctor runner against a fake typed-load closure (typo'd key named,
  absent file ok, exit-code mapping).
- **BDD** (`services/doctor/tests`, built with the `mock` feature) — seed a
  scratch config dir and a platform-facts file with known-broken states (port
  collision, dangling watchdog service, retired D3s keys, unparseable JSON,
  missing `ConditionPathExists` target, absent polkit rule), run the real
  binary, assert the diagnosis, the exit code, and the `--json` schema. For
  `--fix`: assert the rewritten file contents (untouched fields preserved),
  post-fix convergence, idempotence of a second run, that a default run
  writes nothing, and that unfixable checks stay reported without a write.
  For hardware: stage `hardware` facts (nodes, USB inventory, groups, rule
  contents) and assert each check's fail/warn split against enabled and
  disabled units, plus that a facts file without a `hardware` object skips
  the family. For aggregation: staged unit facts point the shell-out at
  stub binaries (canned report, canned garbage, canned clap error for the
  skew case) and the HTTP probe at a stub management endpoint; assert the
  merge, the `service` attribution, and that a degraded probe warns
  instead of failing.
- **Per-service BDD** — the subcommand itself follows the smoke-suite
  convention (the tls-auth precedent): one scenario per service asserting
  `doctor --json` exits 0 on a valid config and exits 1 naming the key on
  a typo'd one, with the deep behavior (rendering, absent-file, SDK
  enumeration against the sim/mock backends) covered once in
  representatives and the shared runner's unit tests.
- **On-host** (D2 gate, per the plan's all-platforms requirement) — the real
  inspectors validated against a packaged Linux host, the Windows VM (SCM),
  and macOS (brew services). The D4 gatherer gets the same three legs, plus
  the rig's unplug-proof: disconnect a device, run doctor, confirm the
  report is honest.
- **D6a** — unit: cert/ACME module tests move with the code; credential
  mint → Argon2id hash → verify round-trip; pki paths anchor at the
  resolved config root. BDD: against a scratch `--config-dir`, `--fix`
  creates the CA/certs/credential and writes `tls`/`auth` + client blocks
  (hash server-side, plaintext client-side); a second run applies nothing
  and reuses the credential for a newly-appearing service; hand-set blocks
  survive untouched; `auth.mismatch` fires on an incoherent pair;
  `doctor tls issue --force` re-issues service certs but never the CA;
  `doctor auth rotate` re-aligns a mismatched pair. rp's
  `tls_setup.feature`/`acme_setup.feature` and `bdd-infra`'s one-shot
  command tests move here with the commands. On-host: a packaged install
  goes TLS-on/auth-on with one `--fix` and every service answers
  authenticated HTTPS after restart.

## MVP scope

**In D2–D5:** derived catalog, config-root resolution, platform
inspectors, the check list including the no-SDK hardware family, the
`rusty-photon-doctor-checks` crate (now also the schema's canonical home
and the shared service-doctor runner), the per-service `doctor`
subcommands on all catalog services, aggregation over both probe paths,
text + `--json` rendering, exit codes, and `--fix`. Hardware probes never
open a device; SDK checks enumerate only; the aggregation probe against
local services stays on the host.

**In D6a (this document's §Provisioning):** the TLS + credential lifecycle —
self-signed issuance and ACME moved from `rp` (which loses `init-tls` and
`hash-password`), the pki tree under the config root, the minted observatory
credential and its distribution, `tls`/`auth`-on written by `--fix`, and the
`tls`/`auth` subcommands. Writes cover config files and the pki tree; ACME
is the one place doctor's network I/O leaves the host, and only when asked.

**Deferred, tracked in the plan:**
- Cert renewal: `doctor tls renew` + platform timers, in-process cert
  hot-reload in `rusty-photon-tls`
  ([#541](https://github.com/ivonnyssen/rusty-photon/issues/541)) — D6b.
- Packaging (doctor ships in sentinel's package) and install-flow docs — D7.
- `usb_*` identity declarations for qhy-focuser and star-adventurer-gti —
  measured whenever that hardware is next on a USB port; two lines of
  `doctor.toml` each.
