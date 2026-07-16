# ADR-016: Service config ownership — installers place bytes, a standalone doctor wires them

## Status

Accepted (2026-07-15); implementation tracked by
[`docs/plans/service-config-doctor.md`](../plans/service-config-doctor.md).

Builds on [ADR-012](012-service-packaging-architecture.md) §3 without
superseding it: packages still ship no config file, and services still
self-create their own on first start. This ADR settles what was left open —
who reconciles the facts that span services once those files exist.

Draws the central/per-service boundary from
[ADR-014](014-zwo-per-device-services-and-link-features.md)'s link policy;
that boundary is a consequence of ADR-014, not an independent choice.

## Context

ADR-012 made each service the sole owner of its own config file. That was
right, and it left a gap: **nothing owns the facts that span services.**

A rig running one ZWO camera, one EAF, a PPBA, and a mount declares that
camera **three times** — `rp`'s `equipment.cameras[].alpaca_url`, ui-htmx's
`drivers.{id}.base_url`, and sentinel's `services.{name}.base_url` — in three
vocabularies, with three URL conventions (sentinel wants an `/api/v1` suffix;
the other two do not) and two structurally-identical credential types
(`rp_auth::ClientAuthConfig` and ui-htmx's `DriverAuth`). One service name is
spelled four ways: the ui-htmx `drivers` key, its `sentinel_service` field,
sentinel's `services` key, and `operation_watchdog.operations.<family>.service`.
Nothing validates any of it; a mismatch surfaces at 2am as a 404 in a UI
banner. For that four-device rig the same plaintext password is typed nine
times across two files.

The rot is measurable. `ServerConfig` has **13 independent definitions**, seven
field-for-field identical, and five services (qhy-camera, zwo-camera,
zwo-focuser, sky-survey-camera, ui-htmx) carry no `tls`/`auth` at all and
therefore cannot be secured while their siblings can. `rusty-photon-<svc>` is
independently re-encoded in `.service` files, `.wxs` fragments,
`generate-brew-formulas.sh`, and `rig.sh` — and the copies drifted far enough
that **every documented `restart_command` in the repo is wrong twice over**
(`--user` scope against system units; missing the `rusty-photon-` prefix), one
still naming `qhyccd-alpaca`, a dead predecessor project, pointed at
filemonitor's port. Sentinel's unit runs `User=rusty-photon` with
`NoNewPrivileges=yes` and there is no polkit rule or sudoers fragment in
`packaging/`, so the field is decorative on Linux regardless of its contents.

### Alternatives considered and rejected

**Runtime discovery** — an Alpaca UDP responder, a registry file, or sentinel
as a service registry. Rejected on two grounds. First, it reconstructs at
runtime what the package manager already knew at install time and discarded.
Second, and fatally for sentinel: **sentinel's job is restarting things that
are dead**, so any fact it learns by asking a driver is a fact it lacks exactly
when it needs it. A cold start against an already-dead driver has no answer.
The same paradox rules out ui-htmx sourcing its driver list from a live rp:
when rp won't start on a bad config, the UI goes blind precisely when it is
needed to fix rp.

**Config generation inside postinst.** Rejected because package installs are
incremental and unordered, so generation would have to converge across N
postinsts running in any sequence. That is the wart the MSI's seed-once already
has — `docs/packaging-windows.md` tells operators *"after adding features to an
existing install, add the new service's entry by hand."* It would also invert
the dependency graph, putting a doctor package underneath all 17 services.

**A single joint config file.** It would kill the duplication by construction,
but every service with `config.apply` writes its config back; concurrent atomic
renames against one shared file means lost updates. The per-file model is what
makes those self-rewrites safe (ADR-012 §3).

**Conffiles** remain rejected for ADR-012's original reason (services rewrite
their own config). Note that ADR-012's argument never ruled out *generation* —
only *shipping*. A generated file is not a tracked conffile: no upgrade
prompts, and the service can still rewrite it. This ADR declines generation
anyway, on the ordering grounds above, not the conffile grounds.

**A single doctor that probes hardware directly.** Rejected by ADR-014. Such a
binary must link `libASICamera2` + `libEAFFocuser` + `libqhyccd` + every future
vendor blob, and therefore ship them all at the shared `/usr/lib/rusty-photon/`
path — recreating the exact dpkg "two owners of one file" conflict ADR-014 was
written to fix, and forcing a QHY-only rig to install ZWO's SDK. This is a hard
blocker, not a size objection.

## Decision

1. **Installers place bytes on disk. A standalone `rusty-photon-doctor` wires
   the configs.** Packages do not generate or seed config and postinst does not
   call doctor. Services self-create their defaults as they already do
   (ADR-012 §3, unchanged), and the operator runs `rusty-photon-doctor --fix`
   to make the install coherent. An operator-run doctor sees the whole system
   at once and converges in one pass — no ordering problem, no idempotent
   merge, no dependency inversion. The cost is an explicit post-install step,
   accepted: it is the same shape as `postgresql-setup initdb`.

2. **One config file per service. Not a joint file.** The correctness burden
   moves onto doctor rather than onto a locking scheme.

3. **Doctor is a standalone binary, not a component of the services.** It links
   no service crate. It knows the catalog, one `ServerConfig` shape, and the
   two aggregator maps; every other byte of every config file is opaque
   `serde_json::Value` it steps around.

4. **Doctor's scope is service facts, never device usage.** "Is `/dev/ttyUSB0`
   writable", "do two services claim one port", "does this `sentinel_service`
   name resolve" are in scope. Which camera is the guide cam, dark-library
   setpoints, focal length, and device identity binding are **usage**, owned by
   `rp`. Doctor never needs to know a serial exists.

5. **Hardware checks split at the SDK line.** Everything needing no vendor blob
   — device node presence, writability, `plugdev` membership, udev rule
   installed *and parsed*, VID:PID in sysfs, firmware helper run — belongs to
   central doctor. Everything needing a blob belongs to a `doctor` subcommand
   on the service binary that already links it. Central doctor aggregates over
   two naturally exclusive paths: when a service is **up** it already
   enumerated its hardware, so ask `/management/v1/configureddevices` over
   HTTP; when it is **down**, shell out to its `doctor` subcommand. Neither
   path needs an SDK in doctor, and neither contends for a device lock — which
   also enforces the rule that **doctor must never open hardware a running
   service holds**.

6. **The similarity lives in a shared library, not a shared binary.** Serial
   and USB reachability checks are near-identical across services, so they go
   in a crate that per-service doctors call. Adding service #201 means
   implementing a small trait, not editing a central binary. Centralizing the
   binary is what fails to scale; centralizing the library is what makes it
   scale.

7. **The doctor report schema parses permissively** — `#[serde(default)]`,
   tolerate unknown fields — the inverse of the `deny_unknown_fields`
   convention every config uses. The asymmetry is deliberate: a config typo
   must be fatal at startup, but a doctor and a service from different nightly
   builds must degrade to a partial report rather than refuse to run.

8. **Doctor owns sentinel's `services` map and ui-htmx's `drivers` map
   outright.** Both are pure service facts and both are copies of what the
   catalog already knows. `rp`'s `equipment[].alpaca_url` is the third copy and
   stays operator-facing — it lives inside the usage block, which decision 4
   fences off — but doctor *checks* it. The goal is explicitly **not** zero
   duplication on disk; it is zero **hand-maintained** duplication. Copies a
   machine writes and verifies are not what bites operators.

9. **The catalog is derived, not typed.** `services/<svc>/pkg` existing is
   already the packaging authority — `build-packages.sh` and
   `generate-brew-formulas.sh` derive their service lists from it with a
   byte-identical line. Doctor's catalog comes from the same place, with a CI
   test asserting the table matches the tree, so it does not become the fifth
   independent encoding of `rusty-photon-<svc>` and rot like the other four.

## Consequences

- A fresh multi-service install is **not coherent until someone runs doctor**.
  This is a real regression in "it just works" and is accepted in exchange for
  deleting the whole class of ordering bugs. It must be prominent in the
  install docs, not a footnote.
- The 13 `ServerConfig` definitions collapse to one shared type. That is a
  prerequisite, not a cleanup: it is what lets doctor parse the `server` block
  out of any `<svc>.json` while treating the rest as opaque, and therefore what
  keeps doctor out of the services. A breaking config-schema change
  (ui-htmx's `bind` → `bind_address`) rides along, sanctioned pre-1.0.
- **Five services gain TLS and auth** because they inherit the shared type.
- Doctor becomes a **third writer** of config files, alongside operator
  hand-edits and drivers' own `config.apply`. It must reuse
  `rusty_photon_config::save`'s atomic temp→fsync→rename→fsync-dir path and the
  layer-aware persist rules, so it cannot bake a transient CLI override into a
  file. Atomic rename bounds the damage to a lost update, never corruption.
- Doctor will report that **sentinel cannot restart anything on a packaged
  Linux host**. Making that true requires a privilege path (polkit, sudoers, a
  D-Bus call to systemd, or running sentinel differently) with real security
  trade-offs. That decision is deliberately *not* made here and must land
  before doctor starts generating `restart_command` values that still cannot
  execute.
- Each hardware-touching service grows a `doctor` subcommand. The shared crate
  keeps that to a handful of calls per service.
- The report schema is a contract between two independently-upgradable
  binaries, so it needs versioning discipline that configs do not.
- ADR-012 §3, ADR-013, and ADR-014 are all unchanged; this ADR is additive to
  each.

## References

- Plan (phases, verification matrix, flagged unknowns):
  [`docs/plans/service-config-doctor.md`](../plans/service-config-doctor.md)
- Config ownership this builds on: [ADR-012](012-service-packaging-architecture.md) §3
- Native SDK payload policy: [ADR-013](013-native-sdk-payload-policy.md)
- The link policy that draws the SDK line:
  [ADR-014](014-zwo-per-device-services-and-link-features.md)
- Config-and-state-in-code, not installer artifacts:
  [ADR-015](015-windows-packaging-architecture.md) §4
- The edit protocol doctor must not fight with:
  [`docs/services/config-actions.md`](../services/config-actions.md)
- Config machinery doctor reuses: `crates/rusty-photon-config`
  (`resolve_config_path`, atomic `save()`, layer-aware persist)
- Precedent for shared-mechanism / per-service-data:
  `packaging/postinst.udev-stanza` (shared) +
  `services/*/pkg/90-rusty-photon-*.rules` (per-service)
