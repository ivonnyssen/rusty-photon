# Plan: eager hardware validation across Alpaca services

## Motivation

[Issue #254][issue-254] fixed the immediate Star Adventurer GTi pain
(wrong-device handshake leaked seven mount-specific commands before the
identity check) by reordering the handshake so `:e1` runs first and
strictly validates against a Sky-Watcher mount-type whitelist. The fix
landed in PR #296 and means the wrong-device case now surfaces a
clear, port-quoted `INVALID_OPERATION` to the ASCOM client on first
`Connected = true`.

The follow-up discussion surfaced a deeper question: in an **Alpaca**
deployment (standalone HTTP daemon, discovery broadcast, multi-client
refcount, systemd unit lifecycle) why is the wrong-device check
gated on the first ASCOM client request rather than running at
service startup? Classic ASCOM (Windows COM in-process driver) has
to be lazy because the driver *is* the client process. Alpaca looks
much more like postgres — a daemon that should validate its world
at boot and exit non-zero on misconfiguration so systemd /
orchestration treat the failure as a failure instead of advertising
a broken device on the network.

This plan extends the identity-validation pattern across every
Alpaca driver service in the workspace, with a small,
non-invasive addition to `rusty-photon-shared-transport`.

[issue-254]: https://github.com/ivonnyssen/rusty-photon/issues/254

## Scope

Five Alpaca driver services, all (or shortly to be) on
[`rusty-photon-shared-transport`][shared-transport-plan]:

| Service | Port | Identity-probe equivalent | Shared-transport status |
|---|---|---|---|
| `dsd-fp2` | 11119 | DSD identity string | on shared transport (first adopter, PR #283) |
| `ppba-driver` | 11112 | Pegasus version query | on shared transport (Phase B, PR #276) |
| `qhy-focuser` | 11113 | JSON identity query | on shared transport (Phase C, PR #280) |
| `pa-falcon-rotator` | 11118 | Pegasus identity query | on shared transport (Phase D, PR #282) |
| `star-adventurer-gti` | 11117 | `:e1` + `MountType` whitelist | on shared transport (Phase E, PR #285); identity-probe landed in PR #296 |

Explicitly out of scope:

- `sentinel` (HTTP client polling other Alpaca devices, not a driver).
- `phd2-guider` (PHD2 client, not Alpaca).
- `filemonitor` (FITS file watcher, no hardware-bound transport).

[shared-transport-plan]: ./shared-transport-extraction.md

## What "eager validation" means here

At service startup, after config load but **before** binding the
Alpaca HTTP server and starting the discovery responder:

1. Open the transport.
2. Run the existing handshake hook (which performs the identity
   probe by construction — see [§Per-service work](#per-service-work)
   for the audit checklist).
3. Tear the transport back down (`Hooks::teardown` runs; transport
   closes; refcount returns to zero).
4. On success → proceed to bind the Alpaca server and the discovery
   responder. The hardware is *not* held between this validation
   handshake and the first `Connected = true` from a client; we
   re-acquire on first connect. One extra handshake per service
   lifetime is the only overhead.
5. On failure → log the diagnostic at `error!`, return a non-zero
   `ExitCode` from `main`. systemd / orchestration retries naturally.
   Operator fixes config and the orchestrator restarts the service.

The deliberate non-goal: **don't hold the hardware for the full
service lifetime.** That would break:

- The shared-transport ref-count contract (refcount goes 0 → 1 on
  validation, would stay >0 forever, never reach 0 again).
- The teardown hook on real client disconnect (`star-adventurer-gti`
  issues `:L1, :L2, :K1`; `ppba-driver` and friends have their own
  shutdown commands).
- The "service holds hardware nobody's using" semantic — leaves the
  bus / cable busy for tooling that needs to probe it (e.g.
  `mode==diagnostic` ad-hoc reads from another process).

## Design choices

| Decision | Choice | Rejected alternative + why |
|---|---|---|
| Where validation lives | New `SharedTransport::validate_hardware()` method that internally `acquire` + `close`. Per-protocol identity probe stays inside the codec's existing `handshake` hook. | Add a new `Hooks::identify` separate from `handshake` — duplicates the codec's "talk to the device" path and forks the wire sequence between validate and real connect. |
| Hold vs. release after validate | Release (validate-close-reopen-on-first-client). Two handshakes per service lifetime is cheap; preserves the lazy-acquire / refcount contract. | Hold a synthetic reference for service lifetime — see [§What "eager validation" means here](#what-eager-validation-means-here) above. |
| Default | Opt-in via `validate_on_start: true` in the service's transport config block. Defaults to `false` so `Config::default()` (smoke tests, `cargo run` with no `--config`) still comes up cleanly. Production config files set it to `true`. | Default-on workspace-wide — wrecks every `cargo run` smoke test and every BDD scenario that starts the server without hardware. |
| Failure-mode behavior | Hard fail: log the diagnostic to stderr at `error!`, return `ExitCode::from(2)` from `main`. | Warn-and-continue — the discovery responder broadcasts a device that isn't really there, defeats the purpose. Retry-loop at startup — postpones the problem; orchestrators already do retry. |
| Transient mount-off-at-boot tolerance | Allow opt-in `validate_on_start_retries: u32` (default 0) with a fixed backoff. Operators powering the mount on the same circuit as the host can set `validate_on_start_retries: 5`. | Always-retry — masks legitimate config errors. Never-retry — friction for the dome-power case. |

## Shared-transport API addition

```rust
// crates/rusty-photon-shared-transport/src/lib.rs
impl<C: Codec> SharedTransport<C> {
    /// Eagerly open the transport, run the handshake (which
    /// encompasses the codec's identity check), then close. Used at
    /// service startup to validate the configured transport target
    /// before binding the Alpaca HTTP server.
    ///
    /// On success the transport returns to its pre-validate state
    /// (closed, refcount = 0); the first real client's
    /// `Connected = true` triggers a fresh `acquire()`.
    /// Identity-probe failures surface the same
    /// `SessionError<C::Error>` shape `acquire()` would, so
    /// per-service error mapping (`SessionError → service error →
    /// ASCOMError` for runtime, plus `std::process::ExitCode` for
    /// startup) reuses the existing routing.
    pub async fn validate_hardware(&self) -> Result<(), SessionError<C::Error>> {
        let session = self.acquire().await?;
        session.close().await.map_err(SessionError::Transport)
    }
}
```

That's the entire shared-crate change. Three lines of new public
surface; leverages the existing acquire / handshake / teardown /
refcount plumbing verbatim. The validation path is structurally
identical to a real client connect + immediate disconnect, so there's
no second code path to maintain.

### Implications for shared-transport's existing hooks

- **`Hooks::handshake`** is the identity-probe contract. Today only
  `star-adventurer-gti`'s handshake actually rejects on identity
  mismatch (`SkywatcherCodecError::WrongDevice`, landed in PR #296).
  The other four services' handshakes need audit — the eager-validation
  rollout is the forcing function to harden them.
- **`Hooks::teardown`** runs on validate-close just as on real
  disconnect. Audit each service's teardown for "safe to send to a
  freshly initialized device that's never moved" — most halt-equivalent
  commands are idempotent, but worth verifying. For `star-adventurer-gti`
  the existing `:L1, :L2, :K1` sequence is fine.
- **`Hooks::while_open`** (background poll) starts on `acquire`, cancels
  on `close`. Validate-close cancels it cleanly via the existing
  `WhileOpen::cancelled()` signal — no change needed.
- **No new hook needed.** Resist the temptation to add a separate
  `Hooks::identify`; the existing `handshake` is already the
  identity-probe checkpoint by construction (it's the first wire code
  path that runs).

## Per-service work

| Service | Audit | Likely change |
|---|---|---|
| `star-adventurer-gti` | Already done in PR #296. | Add `validate_on_start` config field + `main.rs` call to `validate_hardware()`. |
| `dsd-fp2` | Confirm handshake checks an identity string and rejects on mismatch. | If not: port the `WrongDevice` pattern (codec error variant + diagnostic + service error variant + ASCOM mapping); add config field; `main.rs` hook. |
| `ppba-driver` | Same — Pegasus PPBA has a version/identity query; confirm handshake rejects non-PPBA replies. | Same. |
| `pa-falcon-rotator` | Same — Pegasus Falcon shares Pegasus identity shape; potentially share an identity-probe helper with `ppba-driver`. | Same; possible shared `pegasus-protocol` crate consolidation (track as a separate cleanup, not blocking). |
| `qhy-focuser` | QHY's JSON identity probe — confirm rejection on non-QHY JSON. | Same. |

For each service, the per-service change is roughly:

- New `WrongDevice { port, reason }` error variant in the service's
  error enum (where missing — `star-adventurer-gti` already has it).
- Wrong-device routing through
  `SessionError → service error → ASCOMError` for runtime ASCOM
  consistency (`star-adventurer-gti`'s `codec.rs` is the template).
- `validate_on_start: bool` (and the two companion fields below) on
  the transport config block.
- `main.rs` call site:

```rust
if cfg.transport.validate_on_start {
    info!("validating hardware before binding Alpaca server");
    manager
        .transport()
        .validate_hardware()
        .await
        .map_err(|e| {
            error!(error = %ServiceError::from(e), "hardware validation failed");
            ExitCode::from(2)
        })?;
}
```

## Config surface (per service)

```rust
// In each service's TransportConfig (USB / UDP variants both):
#[serde(default)]
pub validate_on_start: bool,
#[serde(default)]
pub validate_on_start_retries: u32,
#[serde(default = "default_validate_retry_backoff", with = "humantime_serde")]
pub validate_on_start_retry_backoff: Duration,
```

Defaults: `validate_on_start: false`, `validate_on_start_retries: 0`,
`validate_on_start_retry_backoff: 2s`. Production operators flip on;
tests / smoke runs unaffected.

The retry semantics: on `ConnectionFailed` / `Timeout` /
`Transport(connection closed)` (i.e. the "device not yet ready" cluster
of errors), retry up to `validate_on_start_retries` times with
`validate_on_start_retry_backoff` between attempts. On `WrongDevice`
or `Protocol` errors, fail immediately — these are config errors, not
transient hardware-not-ready conditions, and retrying just adds
seconds of confusion before the operator gets the diagnostic.

## CLI surface (per service)

Add `--check-device` to force a one-shot validation pass and exit
(orchestration / dome-startup-script helper):

```bash
star-adventurer-gti --config /etc/dome.json --check-device
# exit 0 → hardware verified
# exit 2 → wrong device or transient failure
```

This is independent of `validate_on_start`; useful for "verify before
service install" workflows and CI hardware-attached smoke tests. Maps
to the same `validate_hardware()` call as the implicit start-time
validation.

## Test strategy

- **Shared-transport**: one unit test in
  `crates/rusty-photon-shared-transport/` exercising
  `validate_hardware` on a mock that fails the handshake (asserts
  refcount returns to zero, transport never re-opens implicitly).
- **Per-service**: one integration test asserting
  `validate_on_start = true` + wrong-device mock → service `main`
  returns non-zero. Easiest path: extract main into a
  `pub async fn run(...) -> Result<(), ServiceError>` that returns,
  and call it directly with a wrong-device mock factory from a
  unit test.
- **BDD**: existing scenarios assume `validate_on_start = false`
  (the default). Add one scenario per service: "service refuses to
  start when configured port targets the wrong device." This BDD
  step needs the harness to start the service with a wrong-device
  mock factory — practically that means the existing BDD world
  builder grows a `with_wrong_device_factory()` option.
- **CI nightly hardware-attached run** ([pi5 nightly
  runner][pi5]): set `validate_on_start: true` in the nightly
  config; failed validation paged via the existing nightly-failures
  Slack hook.

[pi5]: ../../docs/operations/pi-nightly-runner.md

## Failure-mode matrix

| Scenario | `validate_on_start = false` (current) | `validate_on_start = true` |
|---|---|---|
| Mount powered off at boot | Service starts. First client connect fails with `ConnectionFailed`. | Service exits 2. Orchestrator retries. Operator turns on mount. With `validate_on_start_retries > 0`, intermediate retry attempts before exiting. |
| Mount powered on, wrong device | Service starts. First client connect surfaces `WrongDevice`. | Service exits 2 immediately with the same diagnostic at the operator's terminal. |
| Mount powered on, transient handshake failure (CRC error, dropped byte) | Service starts. First client connect retries on next attempt. | With `validate_on_start_retries > 0`, retry; else exit 2. |
| Mount powered on, correct device | Service starts. First client connect succeeds. | Service starts (after a brief validate-handshake), first client connect succeeds — one extra handshake at boot is the cost. |

## Phasing

Mirror the [shared-transport-extraction precedent][shared-transport-plan]
(PR-per-phase):

1. **Phase 0** — `rusty-photon-shared-transport`: add
   `validate_hardware()` + tests. One PR. Touches one file in one
   crate; review effort minimal.
2. **Phase 1** — `star-adventurer-gti`: add config field + `main.rs`
   hook + per-service test + BDD scenario + design-doc update.
   Already has the `WrongDevice` plumbing from PR #296, so this is
   pure wiring.
3. **Phase 2** — `dsd-fp2`: audit handshake's identity check; bring
   it up to spec if needed; wire eager validation.
4. **Phase 3** — `pa-falcon-rotator`: same.
5. **Phase 4** — `qhy-focuser`: same.
6. **Phase 5** — `ppba-driver`: same. Possibly bundle with Phase 3
   if a shared `pegasus-protocol` crate makes sense.
7. **Phase 6** (optional) — workspace docs: a
   `docs/skills/eager-hardware-validation.md` that codifies the
   pattern for future driver services. Cross-link from the design
   doc of each service.

Each phase is independently mergeable (services without
`validate_on_start: true` in their production config are
unaffected by their own phase landing).

## Open questions worth surfacing before starting

1. **systemd unit semantics.** Should we ship a sample `.service`
   file in `deploy/` with `Restart=on-failure` + a `RestartSec=`
   matched to `validate_on_start_retry_backoff`? Otherwise operators
   reinvent the retry budget at the orchestrator layer.
2. **Discovery responder during validation.** The Alpaca discovery
   responder bind happens before or after validation? Recommended: **after**.
   Don't advertise a device we haven't confirmed.
3. **Multi-mount hosts.** A host running `ppba-driver` +
   `qhy-focuser` + `pa-falcon-rotator` on the same USB bus will
   validate three serial ports in parallel at boot. Order-of-operations
   dependency on `/dev/serial/by-id/...` paths under udev settling —
   worth one round of validation against a real Pi at the dome before
   defaulting `validate_on_start` to `true` for the workspace.
4. **`Config::default()` ergonomics.** `Config::default()` is what
   `cargo run -p <service>` without `--config` uses. Should
   `Config::default()`'s `validate_on_start` definitely be `false`?
   (Yes — otherwise every dev-loop `cargo run` fails.) Documenting
   this explicitly here so a future contributor doesn't flip the
   default and break local dev workflows.
5. **Bundling Pegasus identity in a crate.** `ppba-driver` and
   `pa-falcon-rotator` both speak the Pegasus protocol family.
   Their identity probes could share a `pegasus-protocol` crate
   (analogous to `skywatcher-motor-protocol`). Track separately
   if Phase 3 / Phase 5 audits surface enough duplication.
