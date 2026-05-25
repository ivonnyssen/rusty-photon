# Config actions + the BFF web UI

**Status:** Phase 1 **landed** (dsd-fp2 `config.get`/`config.apply` + in-process
reload). Phase 2 **landed** — the BFF skeleton + dsd-fp2 config page ship as the
`ui-htmx` service ([`docs/services/ui-htmx.md`](../../services/ui-htmx.md)). Key
protocol decisions resolved 2026-05-24 — see [Resolved](#resolved-2026-05-24).
**Companion to:** [`mocks/README.md`](mocks/README.md) (the chosen UI direction and stack).

## Summary

This plan covers the first concrete slice of the rusty-photon web UI: the
**configuration pages** that configure the Alpaca driver services and (later)
`rp` itself. It defines:

1. A **standalone BFF service** (server-rendered axum + Maud + HTMX) as the home
   of the web UI — a client of the rest of the system, per `rp.md` tenet 7.
2. A **config-action protocol** by which each Alpaca driver exposes its own
   configuration over HTTP, modelled as **ASCOM `Action`s** (`config.get`,
   `config.apply`), with an **in-process reload** to apply changes without a
   process bounce.
3. A **service-lifecycle split**: drivers reload *themselves*; **Sentinel** owns
   *process* restart (`service.restart`) via the already-designed configured-command
   supervisor. These are independent capabilities — config editing does not
   depend on Sentinel.

The first deliverable is a working configuration page for **`dsd-fp2`**
(single `CoverCalibrator`, smallest config), proving the pattern end-to-end
before it is generalised to the other drivers.

## Context and decisions

The activity-stream mocks (`mocks/`) settled the *look* and *stack* of the UI
but cover only the session/telemetry experience. This plan starts implementation
at the **Settings/Equipment** end instead, because configuration is the
least-blocked, most self-contained surface and is a prerequisite to running
anything. Decisions reached during design exploration:

| Decision | Choice | Why |
|----------|--------|-----|
| UI host | **Standalone BFF service** (axum + Maud + HTMX + SSE) | Keeps `rp` a "dumb pipe" (`rp.md` tenet 7); matches `mocks/README.md` and i18n Option A. |
| Driver config transport | **ASCOM `Action`s** (`config.get` / `config.apply`) | Host-agnostic HTTP; reuses the Alpaca server each driver already runs; precedent in `star-adventurer-gti` (`mount_device/device.rs`). |
| Applying config | **In-process reload**, not a process bounce | The `rusty-photon-service-lifecycle` crate already supports reload (`with_reload`/`run_with_reload`, used by `filemonitor`); needs no external supervisor and works in dev. |
| Process restart | **Sentinel** `service.restart(name)` via configured command | Already the sanctioned design (`rp.md:2759-2767`, watchdog plan's `Restarter`). Reserved for recovery/escalation. |
| First driver | **`dsd-fp2`** | Single device, smallest config, first shared-transport adopter. |
| Form rendering | **Hand-built first**, schema-driven later | Prove protocol + the designed look on one driver; generalise after. |
| No-`--config` driver | **Persist to an XDG default path** (`~/.config/rusty-photon/<service>.json`) | A config path is *always* resolvable, so editing is never disabled; startup/reload read it if present. |
| Driver auth on config actions | **No per-action gate**; rely on the server-wide auth/TLS the driver already runs | Matches ASCOM Alpaca — `action` is just another device method; auth and transport security are orthogonal concerns handled by `rp-auth`/`rp-tls`. |
| Config layers | **Distinguish file vs. CLI-override** | `config.get` marks CLI-override-pinned fields; `config.apply` does not persist them, so a transient `--port` can't be baked into the file. |

`config.set` was dropped in favour of a single `config.apply`: the set/apply
split only helped batch multiple writes, which does not arise when a form
submits the whole config blob at once.

## Architecture

```
                ┌───────────────────────────┐
   browser ───► │  BFF  (services/ui-htmx)   │  server-rendered HTML
                │  axum + Maud + HTMX        │
                └───┬───────────┬────────┬───┘
   config.get/apply │           │        │  service.restart(name)
   (ASCOM Action,    │   REST    │        │  (REST)
    PUT .../action)  ▼           ▼        ▼
              [dsd-fp2]   ...   [rp]   [Sentinel]
              [qhy-…]          (REST   (configured restart
              (Alpaca          config   command, e.g.
               devices)        later)   `systemctl restart …`)
```

**Transport asymmetry (important):** only the drivers are ASCOM devices, so only
they expose config via `Action`. `Sentinel` and `rp` are *not* devices — their
config / lifecycle endpoints are plain **REST** on their existing axum routers.
Same shapes, different transport. Do not try to make Sentinel or `rp` an Alpaca
device.

**Responsibility boundary:** *in-process state rebuild* belongs to the driver
(only it can rebuild its own server + transport); *process existence* belongs to
Sentinel/OS. Hence `config.apply` (driver, reload) and `service.restart`
(Sentinel, process) are different tools for different blast radii.

## The config-action protocol (drivers)

Each driver advertises and serves two vendor actions through the standard ASCOM
`Device` trait (`ascom-alpaca`'s `action` / `supported_actions`). Working names
(align the exact strings with the convention in `star-adventurer-gti`'s `actions`
module during implementation):

```
GET  /api/v1/{type}/{n}/supportedactions     → ["config.get","config.apply", …]

PUT  /api/v1/{type}/{n}/action
       Action=config.get      Parameters=
   → 200, body = {
       "config":    <current effective Config as JSON, secrets redacted>,
       "overrides": ["serial.port"]   // CLI-override-pinned; config.apply won't persist these
     }

PUT  /api/v1/{type}/{n}/action
       Action=config.apply    Parameters=<full Config JSON>
   → 200, body = {
       "status": "applying" | "ok" | "invalid",
       "applied":          ["cover_calibrator.max_brightness"],  // took effect live
       "reload":           ["serial.port","server.port"],        // applied via reload
       "restart_required": [],                                   // needs Sentinel.restart
       "skipped_override": ["serial.port"],                      // override-pinned, not persisted
       "persisted_to":     "~/.config/rusty-photon/dsd-fp2.json",
       "errors":           [ {"path":"serial.baud_rate","msg":"…"} ]  // when invalid
     }
```

For `dsd-fp2` the device is `covercalibrator/0` and the Config is:

```
Config { serial:{port, baud_rate, polling_interval, timeout},
         server:{port, discovery_port, tls, auth},
         cover_calibrator:{name, unique_id, description, enabled, max_brightness} }
```

### `config.apply` behaviour

1. Parse `Parameters` as the driver's typed `Config`. Parse failure → ASCOM error.
2. **Validate** (types + ranges + semantics). On failure return **HTTP 200** with
   `{"status":"invalid","errors":[…]}` — a *domain* error the BFF renders as
   field-level messages, distinct from a transport/ASCOM error.
3. **Persist** the new config atomically to the resolved config path
   (stage temp → fsync → rename → fsync dir), creating parent dirs for the XDG
   default. **CLI-override-pinned fields are written through from the file's
   prior value, not the submitted value** (layer-aware persist), and listed in
   `skipped_override[]`.
4. **Classify** each changed field into `applied` (live), `reload`, or
   `restart_required`, and **fire the in-process reload** if anything is in
   `reload` — *after the response is flushed* (see below).
5. Return immediately with the classification. `status:"applying"` signals the
   BFF that a reload is in flight and it should reconnect and re-`config.get`
   to confirm.

### Rules

- **Works while disconnected.** Config actions must not require `Connected=true`,
  or a wrong `serial.port` becomes unfixable (can't connect to fix the thing that
  blocks connecting). This is a deliberate choice in our `action()` impl.
- **Always has a config path.** A persist target is *always* resolvable: the
  explicit `--config` path if given, else an XDG default
  (`~/.config/rusty-photon/<service>.json`). Startup and reload read this path
  if it exists (falling back to `Config::default()` otherwise), and
  `config.apply` persists there. Editing is therefore never disabled for lack of
  a path. (Resolved — see Open questions; previously this rejected when started
  without `--config`.)
- **Layer-aware persist.** `config.get` reports which fields are pinned by CLI
  overrides (`--port`, `--server-port`); `config.apply` persists every field
  *except* those, so a transient override is never baked into the file. Skipped
  fields are echoed in `skipped_override[]`.
- **No per-action auth gate.** `config.get`/`config.apply` are ordinary ASCOM
  actions and are exactly as protected as `calibrator_on` — by whatever
  server-wide `rp-auth`/`rp-tls` the driver runs, not a special case. Auth and
  transport security are orthogonal concerns; the action layer does not know
  about them. (Resolved — see Security and Open questions.)

## In-process reload mechanics

Adopting reload on `dsd-fp2` means switching `main.rs` from `.run()` to
`.with_reload().run_with_reload(...)` and moving config-loading *inside* the loop,
mirroring `filemonitor`:

```rust
ServiceRunner::new("dsd-fp2")
    .with_reload()
    .run_with_reload(move |shutdown, reload| async move {
        loop {
            let config = load_effective_config(&config_path, &overrides)?; // re-read each cycle
            let bound = ServerBuilder::new().with_config(config).build().await?;
            tokio::select! {
                result = bound.start(shutdown.cancelled()) => return result,
                () = reload.recv() => { /* close transport cleanly, then */ continue }
            }
        }
    })
```

Three mechanics must be handled deliberately:

1. **Programmatic reload trigger.** `ReloadSignal`
   (`crates/rusty-photon-service-lifecycle/src/reload.rs`) is `Clone` and already
   exposes a public `notify()`, documented as the hook for "non-signal-driven
   reload sources" beyond SIGHUP / SCM `ParamChange`. So the `config.apply` handler
   just holds a clone of the `ReloadSignal` in the axum app state and calls
   `.notify()` — **no lifecycle-crate change needed.** Caveat: the signal is
   single-consumer (`notify_one`), so exactly one task may `recv()` it (the run
   loop), which is already the intended pattern.
2. **Fire-after-response.** The `config.apply` request is served by the very
   axum server the reload tears down. If reload fires synchronously, the response
   never flushes. So the handler must **return the response, then fire reload**
   (e.g. `tokio::spawn` a task that yields/short-sleeps, then triggers). The BFF
   treats `status:"applying"` as "expect a brief connection blip; reconnect and
   re-`config.get`." This is the same race a process restart would have; reload
   still wins (no supervisor, no cross-process socket churn, faster).
3. **Clean teardown.** The shared transport prefers explicit `close().await` over
   drop-teardown ("`close().await` primary, `Drop` detached fallback"). Before
   `continue`, the reload arm must close the serial connection so the port is not
   briefly double-held when a client reconnects to the rebuilt server.
   **Resolved without a shared-transport change:** `ascom-alpaca`'s `register`
   accepts an `Arc<dyn CoverCalibrator>`, so `build()` keeps an
   `Arc<DsdFp2Device>` handle and `BoundServer` exposes it via `device()`. The
   reload arm calls `device.set_connected(false).await` — the inline, awaited
   `Session::close` path — before the old `BoundServer` future is dropped,
   giving deterministic port teardown. (Idempotent: a no-op when no client was
   connected.)

If a field cannot be reloaded cleanly, `config.apply` returns it in
`restart_required[]` and the BFF escalates to Sentinel — keeping the protocol
honest about (3).

## Service lifecycle (Sentinel)

`service.restart(name)` is **not** part of the normal config path; in-process
reload handles config-file changes. It is the recovery hammer (wedged/crashed
driver) and the escalation for `restart_required` fields. It reuses the
already-designed supervisor:

> *"Sentinel executes the configured restart command for that service (e.g.
> `systemctl restart qhyccd-alpaca`)… configured per service, not hardcoded."* —
> `rp.md:2759-2767`

Implementation: expose the planned `Restarter` trait (see the watchdog plan,
[`../predictive-deadlines-and-watchdog.md`](../predictive-deadlines-and-watchdog.md))
as a REST endpoint on Sentinel's
existing dashboard router (e.g. `POST /api/services/{name}/restart`), driven by a
per-service `restart_command` config entry. Sentinel does **not** spawn or own the
processes — it shells out to the configured command; the OS supervisor (systemd /
SCM) owns relaunch. This is a larger workstream tied to the watchdog plan and is
**not a prerequisite** for the config pages.

## The BFF service (`services/ui-htmx`)

The crate is **`ui-htmx`** — the first member of a `ui-*` family of UI
expressions: browser variants are qualified by technology (`ui-htmx`, future
`ui-leptos`), native ones by target (future `ui-ios`, `ui-android`), and
shared backend-for-frontend logic is extractable to `ui-core` when a second
expression lands. Per the crate-naming convention it is an unprefixed service
under `services/` (system-wide, not `rp`-specific). Full design:
[`docs/services/ui-htmx.md`](../../services/ui-htmx.md).

- **Stack:** axum + [Maud](https://maud.lambda.xyz/) + [HTMX](https://htmx.org/),
  assets embedded via `include_str!`. Dark theme reusing the mocks' CSS tokens so
  config pages are visually consistent with the eventual activity stream. No npm,
  no WASM. (SSE comes later with live telemetry; config pages need none.)
- **Phase-1 routes:**
  - `GET /config/{service}` → call the driver's `config.get`, render a hand-built
    Maud form filled with current values.
  - `POST /config/{service}` → build the Config JSON from the form, call the
    driver's `config.apply`. On `status:"invalid"`, re-render the form with
    field-level errors (HTMX swap). On `status:"applying"`, render an
    "applied — reconnecting…" state and poll `config.get` until the driver is back.
- **Driver addressing:** Phase 1 hard-codes one target (the `dsd-fp2` Alpaca URL +
  `covercalibrator/0` + optional auth) from BFF config. Later the device list is
  derived from `rp`'s `GET /api/equipment` or ASCOM discovery.
- **HTTP client:** follow sentinel's `HttpClient` trait + `mockall` pattern so
  handlers are unit-testable without a live driver.

## Security

- **No per-action auth gate (resolved).** `config.apply` is a write like
  `calibrator_on`; it is protected by the same server-wide `rp-auth`/`rp-tls`
  layer the driver already runs, *not* by a special check inside `action()`.
  Auth and transport security are orthogonal concerns, handled once at the
  server boundary — this matches ASCOM Alpaca, where `action` is just another
  device method. A driver exposed without auth has *every* write open, not just
  config; that is a deployment choice, not something the config action
  second-guesses.
- **Redact secrets in `config.get` (hygiene, not a gate)**: never emit the
  `auth` password/hash or TLS key material in the config dump, regardless of who
  is authorised to read it. On `config.apply`, treat absent/sentinel secret
  fields as "unchanged" so a round-tripped form does not blank them.
- The BFF holds driver credentials to call the actions; store them in the BFF's
  own config, not in the page.

## Testing strategy

Per `docs/skills/testing.md` (prefer `unwrap()`-style failures; smallest unit
that is still comprehensive in aggregate) and the BDD setup in `tests/`.

- **Driver unit tests** (run in `mock` mode):
  - `supported_actions` lists the config actions.
  - `config.get` returns current config; secrets redacted.
  - `config.apply` with valid JSON persists + classifies; with invalid JSON →
    `status:"invalid"` and the file is unchanged; unknown action →
    `ACTION_NOT_IMPLEMENTED`; works while disconnected; works without auth (no
    gate); CLI-override-pinned fields are reported in `overrides[]`/
    `skipped_override[]` and not persisted.
- **Driver reload test:** `config.apply` triggers reload and the server returns
  with the new config (BDD via `bdd-infra`, or an integration test against the
  bound server). Assert the fire-after-response ordering (the apply response is
  received before the blip).
- **BFF tests:** the `config_page.feature` BDD suite spawns the *real* BFF and a
  *real* `dsd-fp2` (mock mode) and drives it over HTTP end to end — render,
  apply → reload → reconnect (serving the new value back), unchanged → `ok`,
  validation, and unreachable-driver. A handler unit test covers the one error
  state a live driver can't produce (`ACTION_NOT_IMPLEMENTED`) via a stub
  `ConfigClient`; `driver_client` unit tests cover envelope parsing with a
  mocked `HttpClient`.

## Phased plan

**Phase 1 — `dsd-fp2` config actions + reload (driver-side, fully testable in mock mode). ✅ Landed.**
- Add the XDG config-path resolver (`--config` else
  `~/.config/rusty-photon/dsd-fp2.json`, via the `directories` workspace dep) +
  CLI-override tracking (`--port`, `--server-port`).
- Fire reload from `config.apply` via the already-public `ReloadSignal::notify()`
  (no lifecycle-crate change needed).
- Restructure `dsd-fp2` `main.rs` to `run_with_reload`; move config-loading into
  the loop; clean transport teardown on the reload arm via
  `device.set_connected(false).await` (no shared-transport change — `build()`
  keeps an `Arc<DsdFp2Device>`, `BoundServer::device()` exposes it).
- Implement `config.get` / `config.apply` actions (+ `supported_actions`),
  including validation, atomic persist, classification, layer-aware persist
  (skip CLI-override-pinned fields), fire-after-response reload, secret
  redaction. No auth gate.
- Tests as above. *Deliverable: a driver whose config you can read, write, and
  reload over HTTP.*

**Phase 2 — BFF skeleton + hand-built `dsd-fp2` config page. ✅ Landed.**
- New `services/ui-htmx` crate (axum + Maud + HTMX, embedded assets, dark theme).
- `GET/POST /config/dsd-fp2` wired to the driver's config actions; validation,
  override-pinned-field read-only, and "applying/reconnecting" states. 7 BDD
  scenarios that spawn the real BFF + a real mock-mode `dsd-fp2` and drive it
  over HTTP (including the full apply → reload → reconnect round trip), plus
  handler/wire-parsing unit tests. *Deliverable: a working config page for
  `dsd-fp2`.*

**Phase 3 — Generalise across drivers.**
- Extract a shared `config-actions` helper so other drivers adopt `config.get`/
  `config.apply` with minimal boilerplate; roll out to `qhy-focuser` and the rest.
- Optional: add `config.schema` (via `schemars`) + a schema-driven form renderer
  in the BFF to replace hand-built forms.

**Phase 4 — Sentinel `service.restart`.**
- Implement the `Restarter` + per-service `restart_command` config; expose
  `POST /api/services/{name}/restart`. Wire the BFF "Restart (via Sentinel)"
  affordance and the `restart_required` escalation. (Coordinate with the watchdog
  plan.)

**Phase 5 — `rp` config + equipment roster, then the activity stream.**
- `rp` gains `GET/PUT /api/config` (same get/apply shape, REST). Build the rp
  equipment-roster page; add ASCOM discovery to pre-fill it. The activity-stream
  UI follows on a separate track (it needs `rp`'s real-time event stream, which is
  not yet implemented — see `rp.md:2846`).

## Open questions

- ~~**BFF crate name** — `ui` / `console` / `web`?~~ **Resolved:** `ui-htmx`,
  the first member of a `ui-*` family (tech-qualified for browser expressions,
  target-qualified for native). See [The BFF service](#the-bff-service-servicesui-htmx).
- **`server.port` changes** carry a cross-service reference: rp's roster
  `alpaca_url` for that device must change too. Out of scope while the BFF talks to
  the driver directly; revisit with the rp equipment page (Phase 5).

### Resolved (2026-05-24)

- **No-`--config` drivers** → **write to an XDG default path**
  (`~/.config/rusty-photon/<service>.json`), not refuse. A path is always
  resolvable, so editing is never disabled; startup/reload read it if present.
- **No-auth drivers** → **all surfaces work; no per-action gate.** Config
  actions match ASCOM Alpaca semantics — they are protected by the server-wide
  `rp-auth`/`rp-tls` layer, which is an orthogonal concern from the action's
  behaviour. (Previously proposed: refuse without auth.)
- **Effective vs. file config** → **distinguish layers.** `config.get` returns
  the effective config *and* marks CLI-override-pinned fields (`overrides[]`);
  `config.apply` persists every field except those, so a transient `--port`
  cannot be baked into the file.

## Doc impact (CLAUDE.md rule 2)

When implementing: add a "Config actions" section to `docs/services/dsd-fp2.md`
(done); create `docs/services/ui-htmx.md` for the BFF (done); extend
`docs/services/sentinel.md` with
`service.restart` (Phase 4); note any `ReloadSignal` trigger addition in
`docs/skills/service-lifecycle.md`; and link this plan from `mocks/README.md`.
