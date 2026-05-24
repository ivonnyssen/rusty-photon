# Config actions + the BFF web UI

**Status:** draft / proposed. No production code yet.
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

`config.set` was dropped in favour of a single `config.apply`: the set/apply
split only helped batch multiple writes, which does not arise when a form
submits the whole config blob at once.

## Architecture

```
                ┌───────────────────────────┐
   browser ───► │  BFF  (services/ui)        │  server-rendered HTML
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
   → 200, body = <current effective Config as JSON, secrets redacted>

PUT  /api/v1/{type}/{n}/action
       Action=config.apply    Parameters=<full Config JSON>
   → 200, body = {
       "status": "applying" | "ok" | "invalid",
       "applied":          ["cover_calibrator.max_brightness"],  // took effect live
       "reload":           ["serial.port","server.port"],        // applied via reload
       "restart_required": [],                                   // needs Sentinel.restart
       "persisted_to":     "/etc/rp/dsd-fp2.json",
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
3. **Persist** the new config atomically to the driver's own config file
   (stage temp → fsync → rename → fsync dir).
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
- **Requires a config-file path.** Reload re-reads the file, so `config.apply`
  needs to know where to persist. If the driver was started without `--config`
  (running on `Config::default()`), config-editing is **disabled**:
  `config.apply` returns `{"status":"invalid","errors":[{"path":"","msg":"driver
  started without --config; config editing disabled"}]}`. (Decision: reject
  rather than invent a default path — see Open questions.)
- **Privileged.** Config writes must sit behind the driver's existing
  `server.auth`. If `auth` is `None`, `config.apply` is refused (see Security).

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

1. **Programmatic reload trigger.** Today `ReloadSignal` fires only on SIGHUP /
   SCM `ParamChange`. The `config.apply` handler needs an in-process way to fire
   it. Add a trigger handle to `ReloadSignal` (or wrap it) and share it into the
   axum app state so the action handler can fire it. *Verify against*
   `crates/rusty-photon-service-lifecycle/src/reload.rs` — the lifecycle doc
   already anticipates "control-handler callbacks feeding the same token"
   (`service-lifecycle.md:54`).
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
   briefly double-held when the new `ServerBuilder` reconnects. This likely needs
   `BoundServer` to expose a transport-close handle. (Implementation decision in
   Phase 1.)

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

Implementation: expose the planned `Restarter`
(`predictive-deadlines-and-watchdog.md:501`) as a REST endpoint on Sentinel's
existing dashboard router (e.g. `POST /api/services/{name}/restart`), driven by a
per-service `restart_command` config entry. Sentinel does **not** spawn or own the
processes — it shells out to the configured command; the OS supervisor (systemd /
SCM) owns relaunch. This is a larger workstream tied to the watchdog plan and is
**not a prerequisite** for the config pages.

## The BFF service (`services/ui`)

Working crate name `ui` (package name TBD — see Open questions; alternatives
`console`, `web`). Per the crate-naming convention it is an unprefixed service
under `services/` (it is system-wide, not `rp`-specific).

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

- `config.apply` is a privileged write. **Refuse it when the driver has no
  `server.auth`** configured (otherwise anyone on the LAN can rewrite config and
  trigger reloads). Decision: refuse vs. warn — see Open questions.
- **Redact secrets in `config.get`**: never emit the `auth` password/hash or TLS
  key material. On `config.apply`, treat absent/sentinel secret fields as
  "unchanged" so a round-tripped form does not blank them.
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
    `ACTION_NOT_IMPLEMENTED`; works while disconnected; refused without auth.
- **Driver reload test:** `config.apply` triggers reload and the server returns
  with the new config (BDD via `bdd-infra`, or an integration test against the
  bound server). Assert the fire-after-response ordering (the apply response is
  received before the blip).
- **BFF tests:** handlers render the form from a mocked `config.get`; POST
  surfaces validation errors; the "applying" state renders. Mock the `HttpClient`.

## Phased plan

**Phase 1 — `dsd-fp2` config actions + reload (driver-side, fully testable in mock mode).**
- Add a programmatic trigger to `ReloadSignal` (or confirm one exists).
- Restructure `dsd-fp2` `main.rs` to `run_with_reload`; move config-loading into
  the loop; add clean transport teardown on the reload arm.
- Implement `config.get` / `config.apply` actions (+ `supported_actions`),
  including validation, atomic persist, classification, fire-after-response reload,
  secret redaction, auth gate.
- Tests as above. *Deliverable: a driver whose config you can read, write, and
  reload over HTTP.*

**Phase 2 — BFF skeleton + hand-built `dsd-fp2` config page.**
- New `services/ui` crate (axum + Maud + HTMX, embedded assets, dark theme).
- `GET/POST /config/dsd-fp2` wired to the driver's config actions; validation +
  "applying/reconnecting" states. *Deliverable: a working config page for `dsd-fp2`.*

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

- **BFF crate name** — `ui` / `console` / `web`?
- **No-`--config` drivers** — refuse config editing (current proposal) vs. write to
  a documented default path?
- **No-auth drivers** — refuse `config.apply` (current proposal) vs. allow with a
  loud warning?
- **`server.port` changes** carry a cross-service reference: rp's roster
  `alpaca_url` for that device must change too. Out of scope while the BFF talks to
  the driver directly; revisit with the rp equipment page (Phase 5).
- **Effective vs. file config** — `config.get` returns the *effective* config
  (CLI overrides applied); saving it persists those overrides into the file. Accept
  this, or distinguish file vs. override layers?

## Doc impact (CLAUDE.md rule 2)

When implementing: add a "Config actions" section to `docs/services/dsd-fp2.md`;
create `docs/services/ui.md` for the BFF; extend `docs/services/sentinel.md` with
`service.restart` (Phase 4); note any `ReloadSignal` trigger addition in
`docs/skills/service-lifecycle.md`; and link this plan from `mocks/README.md`.
