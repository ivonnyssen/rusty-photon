# Service Config Doctor Plan — installer puts bytes, doctor wires configs

## Goal

Make a multi-service rusty-photon install coherent without hand-wiring. Today
an operator provisioning a rig hand-edits up to 14 JSON files that share no
schema fragment, and types the same port, URL, credential, and service name
into two or three of them. Nothing validates the cross-references: a mismatched
name surfaces at 2am as a 404 in a UI banner.

The outcome: packages put bytes on disk; a standalone `rusty-photon-doctor`
binary diagnoses and repairs the configuration afterwards. Doctor owns
**service** facts (ports, TLS, auth, service-to-service references, hardware
reachability). It never learns what a camera is *for* — device usage stays in
`rp`.

This plan also collapses the 13 independent `ServerConfig` definitions into
one shared type. That is not a side quest: it is what lets doctor read the
`server` block out of any `<svc>.json` while treating the other 95% of the
file as opaque bytes, and therefore what keeps doctor *out* of the services
rather than a component of them.

## Implementation Status

| Phase | Description | Status | Branch / PR |
|-------|-------------|--------|-------------|
| D0 | This plan + [ADR-016](../decisions/016-service-config-ownership-and-doctor.md) (config ownership + the SDK line) | Open | #539 |
| D1 | Extract shared `ServerConfig`; 13 definitions → 1; TLS/auth for the 5 services that lack it | Not started | |
| D2 | `rusty-photon-doctor` binary: catalog + service-config diagnosis (read-only) | Not started | |
| D3 | `--fix`; doctor owns sentinel's `services` and ui-htmx's `drivers` maps | Not started | |
| D4 | `rusty-photon-doctor-checks` crate + generic hardware checks (no SDK) | Not started | |
| D5 | Per-service `doctor` subcommand + aggregation | Not started | |
| D6 | Packaging, install-flow docs, on-rig verification | Not started | |

## Decisions (fixed — see [ADR-016](../decisions/016-service-config-ownership-and-doctor.md) for rationale)

1. **The installer puts bytes on disk. Doctor wires the configs.** Packages do
   not generate or seed config; postinst does not call doctor. Services
   self-create their defaults as they already do, and the operator runs
   `rusty-photon-doctor --fix` once to make the install coherent.

   This is what keeps the design tractable. Generation inside postinst would
   have to converge across N package installs in arbitrary order — the wart
   the MSI's seed-once already has, documented at `docs/packaging-windows.md:177`
   (*"after adding features to an existing install, add the new service's
   entry by hand"*). An operator-run doctor sees the whole system at once and
   converges in a single pass. No ordering problem, no idempotent merge, and no
   `Depends:` inversion putting doctor under all 17 packages.

2. **One config file per service. Not a joint file.** A single
   `rusty-photon.json` would kill the duplication by construction, but every
   service with `config.apply` writes its config back; concurrent atomic
   renames to one shared file means lost updates. The per-file model is what
   makes those self-rewrites safe. The correctness burden moves onto doctor
   instead.

3. **Doctor is a standalone binary, not a component of the services.** It links
   no service crate. It knows the catalog, one `ServerConfig` shape, and the
   two aggregator maps; everything else in every config file is opaque
   `serde_json::Value` it steps around.

4. **Doctor's scope is service config, never device usage.** "Is `/dev/ttyUSB0`
   writable" is service health and in scope. "Which camera is the guide cam",
   dark-library setpoints, focal length, and UniqueID binding are device usage
   — `rp` owns those, and doctor never needs to know a serial exists.

5. **Hardware checks split at the SDK line.** Central doctor does everything
   that needs no vendor blob. Services own everything that does. This is forced
   by [ADR-014](../decisions/014-zwo-per-device-services-and-link-features.md),
   not a judgment call — see Design below.

6. **The similarity lives in a shared library, not a shared binary.** Serial
   and USB reachability checks are near-identical across services. They go in a
   crate that per-service doctors call, so adding service #201 means
   implementing a small trait, not editing a central binary.

7. **The doctor report schema parses permissively.** `#[serde(default)]`,
   tolerate unknown fields — the opposite of the `deny_unknown_fields`
   convention every config uses. A config typo should be fatal; a doctor from
   last night's nightly meeting a service from tonight's should degrade to a
   partial report, not refuse to run.

## Design

### Ownership model

Three kinds of fact, one owner each:

| Kind | Examples | Owner |
|---|---|---|
| Service | port, bind, discovery, TLS cert, auth, unit name, restart command, serial/USB path | the service's own `<svc>.json`; **doctor** audits and repairs |
| Device | what hardware exists, its stable identity, its capabilities | the hardware, surfaced by the driver over `/management/v1/configureddevices` |
| Usage | roles ("guide cam"), dark-library setpoints, gain/offset, focal length | `rp.json`'s `equipment` block |

The drivers already implement this split correctly on their side.
`services/qhy-camera/src/config.rs:1-9` states it outright: *"The hardware is
the source of truth: the service enumerates every connected QHY camera … Config
therefore carries no per-camera binding — only optional per-serial display
overrides."* `DeviceOverride` across qhy-camera, zwo-camera, and zwo-focuser
carries nothing but `name`/`description`/`filter_names`. The gap is entirely on
the consumer side.

### D1 — the shared `ServerConfig`

There are 13 independent definitions today, seven of them field-for-field
identical:

| Definition | Shape |
|---|---|
| `services/dsd-fp2/src/config.rs:46` | port, discovery_port, tls, auth |
| `services/pa-scops-oag/src/config.rs:45` | *(identical)* |
| `services/qhy-focuser/src/config.rs:45` | *(identical)* |
| `services/pa-falcon-rotator/src/config.rs:43` | *(identical)* |
| `services/ppba-driver/src/config.rs:37` | *(identical)* |
| `services/filemonitor/src/lib.rs:82` | *(identical)* |
| `services/star-adventurer-gti/src/config.rs:92` | *(identical)* |
| `services/qhy-camera/src/config.rs:56` | port, discovery_port — **no tls/auth** |
| `services/sky-survey-camera/src/config.rs:142` | port, discovery_port — **no tls/auth** |
| `services/zwo-camera/src/config.rs:50` | port, discovery_port — **no tls/auth** |
| `services/zwo-focuser/src/config.rs:43` | port, discovery_port — **no tls/auth** |
| `services/rp/src/config/server.rs:6` | port, **bind_address**, tls, auth — no discovery_port |
| `services/ui-htmx/src/config.rs:40` | **bind**, port — no tls/auth/discovery |

Extract one type (`rp-server-config`, or a module in an existing shared crate —
see Flagged unknowns). Every service embeds it. Three consequences:

- **Five services gain TLS and auth for free.** qhy-camera, zwo-camera,
  zwo-focuser, sky-survey-camera, and ui-htmx currently cannot be secured at
  all while their siblings can. sky-survey-camera is the sharpest case: it
  consumes `ClientAuthConfig` outbound (`config.rs:90,114`) but exposes no
  inbound auth.
- **The bind-address naming split resolves.** `bind_address` (rp),
  `bind` (ui-htmx), absent-and-hardcoded (the other 11).
- **Doctor becomes possible.** One shape to parse out of 18 files.

`discovery_port` keeps its `#[serde(default, skip_serializing_if = "Option::is_none")]`
so `config.apply` cannot re-persist a stale key, and its default stays `None` —
the collision rationale at `crates/rusty-photon-driver/src/discovery.rs:12` is
unchanged by this plan.

Pre-1.0 breaking config-schema changes are sanctioned, so the rename of
`ui-htmx`'s `bind` → `bind_address` needs no migration shim.

### D2 — the catalog and service-config diagnosis

**The catalog must be derived, not typed.** `services/<svc>/pkg` existing is
already the packaging authority, and both packaging scripts derive the service
list from it with a byte-identical line —
`scripts/build-packages.sh:111` and `scripts/generate-brew-formulas.sh:94`:

```sh
ALL_SERVICES=$(for d in services/*/pkg; do [ -d "$d" ] && basename "$(dirname "$d")"; done | tr '\n' ' ')
```

Doctor's catalog (service name, default port, unit name per platform) comes
from the same place, with a CI test asserting the table matches the tree.

This matters because `rusty-photon-<svc>` is *already* independently re-encoded
in `.service` files, `.wxs` fragments, `generate-brew-formulas.sh`, and
`scripts/rig.sh:39` — and the copies drifted so far that every documented
`restart_command` in the repo is wrong twice over (wrong scope: `--user`
against system units; wrong name: missing the `rusty-photon-` prefix), and one
still names `qhyccd-alpaca`, a dead predecessor project, pointed at
filemonitor's port. A hand-typed catalog would be the fifth encoding and would
rot the same way.

What D2 diagnoses — all service-level, zero device knowledge:

- **Port collisions** across the hand-allocated 11111–11170 range.
- **The sentinel privilege gap.** `services/sentinel/pkg/rusty-photon-sentinel.service`
  runs `User=rusty-photon` with `NoNewPrivileges=yes`, the driver units are
  system units, and there is no polkit rule or sudoers fragment anywhere in
  `packaging/`. Sentinel cannot restart anything on a packaged Linux host
  regardless of what `restart_command` says. Doctor reports this; fixing it is
  a Flagged unknown below.
- **Dangling name joins.** ui-htmx `drivers` key → `sentinel_service` →
  sentinel `services` key → `operation_watchdog.operations.<family>.service`.
  Four spellings of one service name, matched by convention, unvalidated.
- **Config-gated services that will never start.** sky-survey-camera,
  plate-solver, and calibrator-flats hard-require a config file and carry
  `ConditionPathExists=`. Installed, enabled, silently inert.
- **Unparseable configs.** `deny_unknown_fields` makes a typo fatal at startup;
  doctor catches it before the next night.
- **TLS cert/key paths** absent or unreadable by the `rusty-photon` user.
- **Platform-wrong defaults**, e.g. rp self-creating `session.data_directory`
  as a Linux path that is not writable on macOS (`docs/packaging-macos.md:136`).
- **URL convention mismatches** — sentinel's `base_url` wants an `/api/v1`
  suffix; rp's `alpaca_url` and ui-htmx's `base_url` do not.

D2 is read-only. It reports and suggests; it writes nothing.

### D3 — `--fix`, and the two aggregator maps

Doctor owns **sentinel's `services` map** and **ui-htmx's `drivers` map**
outright. Both are pure service facts — name, URL, restart command — and both
are copies of information the catalog already has. Generating them kills two of
the three copies of every port and service name in the system, and makes the
restart commands correct for the first time.

`rp.json`'s `equipment[].alpaca_url` is the third copy and stays
operator-facing: it lives inside the device-usage block, and doctor does not
cross that line. Doctor *checks* it (is the port real, does a service listen
there) but does not own it.

That reframes the goal deliberately: **the enemy is hand-maintained
duplication, not duplication on disk.** Copies a machine writes and verifies
are not what bites operators.

Write safety: `--fix` uses `rusty_photon_config::save`'s existing atomic
temp→fsync→rename→fsync-dir path, making doctor a third writer alongside
operator hand-edits and drivers' own `config.apply`. Atomic rename means no
corruption, only a possible lost update, and `--fix` is an
operator-initiated foreground action. Doctor must reuse the layer-aware persist
rules — it must not bake a transient CLI override into a file.

### D4/D5 — hardware checks, and why the SDK line is not a judgment call

[ADR-014](../decisions/014-zwo-per-device-services-and-link-features.md) exists
because of exactly the failure a central hardware-probing doctor would recreate:

> Installing `rusty-photon-zwo-camera` and `rusty-photon-zwo-focuser` together
> failed: both debs shipped **all three** ZWO SDK blobs at the identical path
> `/usr/lib/rusty-photon/`, and dpkg refuses two owners of one file. […] **Each
> package ships exactly its own blob.**

A doctor that *opens* hardware must link `libASICamera2` + `libEAFFocuser` +
`libqhyccd` + every future vendor blob, and therefore ship them all, at that
same path — reintroducing the file conflict ADR-014 was written to fix, and
forcing a QHY-only rig to install ZWO's SDK. That is a hard blocker, not a size
objection.

So:

**D4 — no SDK needed → central doctor**, via a shared
`rusty-photon-doctor-checks` crate:

- device node exists at the configured serial path; writable by `rusty-photon`
- `rusty-photon` is in `plugdev`
- the udev rule is installed **and parsed** — udev silently drops an entire
  rule line on an unresolvable `GROUP=`, so "the file is present" is not the
  check
- VID:PID present in sysfs
- the firmware helper has been run. `packaging/postinst.udev-stanza` already
  tells operators *"camera firmware is not bundled. Run '<helper>' once as root
  before first use"* (ADR-013 forbids packaging proprietary firmware) and
  nothing verifies they did.

The serial-side helpers belong in or beside `rusty-photon-shared-transport`,
which is already the shared home for that concern.

**D5 — SDK needed → the service**, via a `doctor` subcommand on each service
binary that links only its own blob, emitting the shared report schema as JSON.
The similarity is factored into the D4 crate, so a serial driver's `doctor` is
a handful of calls to shared helpers.

The precedent for this shape is already in the tree:
`packaging/postinst.udev-stanza` is **shared** shell,
`services/zwo-camera/pkg/90-rusty-photon-zwo.rules` is **per-service** data.

### The two probe paths are naturally exclusive

Central doctor needs no vendor blob on either path:

- **Service running** → it already enumerated its hardware at startup. Ask
  `/management/v1/configureddevices` over HTTP. No SDK, no hardware touch.
- **Service down** → shell out to `rusty-photon-<svc> doctor --json`. No lock
  contention, because nothing holds the device. This is also precisely the case
  being debugged.

This dodges a trap: **never open hardware a running service holds.** zwo-camera
Phase E already hit a camera-lock concurrency bug; a doctor that grabs the
camera mid-exposure to "check connectivity" is a tenet-#1 regression, and
reports a false "cannot reach hardware" the rest of the time.

### Report schema

One `DoctorReport { checks: Vec<Check> }`, where a `Check` carries a name,
status (`ok` / `warn` / `fail`), a human-readable detail, and an optional
machine-applicable `Fix`. Central doctor merges its own checks with each
installed service's, and renders one report. Per Decision 7, parsing is
permissive in both directions across the binary boundary.

## Verification

- **Unit** — catalog-matches-`services/*/pkg` test; per-check tests against
  tempdirs and fake sysfs/udev fixtures; report schema round-trip including a
  forward-compatibility case (unknown fields from a newer service).
- **BDD** — doctor against a scratch config dir: seed known-broken configs
  (port collision, dangling `sentinel_service`, unparseable JSON, missing
  `ConditionPathExists` file), assert the diagnosis, run `--fix`, assert
  convergence and idempotence on a second run.
- **On-rig (arm64 Debian test rig)** — the real proof: install the package set,
  run `--fix`, confirm a rig that previously needed hand-wiring comes up
  coherent. Then unplug hardware and confirm the report is honest.
- **Cross-platform** — the Windows VM (SCM service names, `%ProgramData%`
  paths) and macOS (brew formula names, the `session.data_directory` default).
  Doctor is explicitly an all-platforms tool, so all three legs are required.

## Flagged unknowns (resolve during the noted phase)

- **The sentinel privilege path (D2/D3).** Doctor will report that sentinel
  cannot restart anything on packaged Linux. Actually fixing it is a separate
  decision — polkit rule, sudoers fragment, a D-Bus call to systemd, or running
  sentinel differently — with real security trade-offs. Needs a decision before
  D3 generates `restart_command` values that still cannot execute.
- **Where the shared `ServerConfig` lives (D1).** A new `rp-server-config`
  crate, or a module in an existing shared crate. `rp-tls` and `rp-auth` are
  already the homes of the types it embeds; a new crate may be cleaner than
  making one of those depend on the other.
- **How doctor detects what is installed (D2).** Binary presence under a
  platform-specific prefix is the most portable; querying dpkg/rpm/SCM/brew is
  more accurate but is four implementations.
- **Whether `--fix` should refuse to run while services are live (D3).** Atomic
  rename makes it safe from corruption, but a driver's `config.apply` could
  race a `--fix` write. Refusing, warning, or ignoring are all defensible.
- **Which package ships doctor (D6).** Its own `rusty-photon-doctor` package
  that the operator installs deliberately, or bundled into a common package.
  Decision 1 removes the `Depends:` pressure that would have forced the latter.

## Future considerations

- **rp's ordinal device binding is unsound, and it is out of scope here.**
  `services/rp/src/equipment/camera.rs:81` binds a roster entry to a physical
  device by *counting* devices of the matching type until it reaches
  `config.device_number` — a `Vec` index over SDK bus order. Unplug the guide
  cam and reboot, and rp connects to the main imaging camera as `"guide-cam"`,
  reports `connected: true`, and dithers against it. Nothing detects it; the
  only failure implemented fires when the slot is *empty*, never when it is
  occupied by the wrong device.

  Every enumerated device already has a stable, hardware-derived UniqueID
  (`ZWO:{name}:{serial}`, the QHY SDK id), the driver already publishes it, and
  the client library already exposes it as `Device::unique_id()` — rp just
  never reads it. Adding `unique_id: Option<String>` to the roster and matching
  on it at connect time turns a silent 2am misbind into a startup error.

  This is device usage, so it belongs to rp and not to doctor, per Decision 4.
  It is recorded here because this analysis surfaced it and it bears directly
  on tenets #1 and #2. It wants its own issue.

- **Sorting enumerated devices by serial before registration** in zwo-camera
  and qhy-camera would make `device_number` stable under replug for the
  serial-bearing majority — a couple of lines in each `build()`, independent of
  everything above.

- **Alpaca UDP discovery** stays off, per
  `crates/rusty-photon-driver/src/discovery.rs:12`. Should a single-responder-
  per-host design ever be revisited, doctor's catalog is the natural source for
  what such a responder would advertise.

## References

- [ADR-016](../decisions/016-service-config-ownership-and-doctor.md) — the
  decision record behind this plan: config ownership, the installer/doctor
  split, and the SDK line, with the rejected alternatives (runtime discovery,
  postinst generation, a joint file, a hardware-probing central doctor).
- [ADR-012](../decisions/012-service-packaging-architecture.md) — config is
  user-based XDG, owned by the service; conffiles rejected because services
  rewrite their own config. Generation is not shipping: this plan does not
  reopen that decision.
- [ADR-013](../decisions/013-native-sdk-payload-policy.md) — native SDK payload
  policy; proprietary firmware is never packaged.
- [ADR-014](../decisions/014-zwo-per-device-services-and-link-features.md) —
  one service per independently usable device; each package ships exactly its
  own blob. Draws the SDK line this plan follows.
- [ADR-015](../decisions/015-windows-packaging-architecture.md) — config and
  state are platform-dependent defaults in code, not installer artifacts.
- [config-actions](../services/config-actions.md) — the `config.get`/`apply`/
  `schema` protocol doctor must not fight with.
- [packaging.md](../packaging.md) / [packaging-windows.md](../packaging-windows.md)
  / [packaging-macos.md](../packaging-macos.md) — current per-platform reality.
