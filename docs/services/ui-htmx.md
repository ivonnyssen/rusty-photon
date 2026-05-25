# ui-htmx Service (web BFF)

## Overview

`ui-htmx` is the browser-facing, **server-rendered configuration UI** for
rusty-photon — the first concrete slice of the web UI described in
[`docs/plans/ui-design/config-actions.md`](../plans/ui-design/config-actions.md)
and the chosen visual direction in
[`docs/plans/ui-design/mocks/README.md`](../plans/ui-design/mocks/README.md). It
is a **standalone backend-for-frontend (BFF)**: a client of the rest of the
system that holds no UI logic inside `rp` (`rp.md` tenet 7). The service renders
HTML on the server with [axum] + [Maud] and adds interactivity with [HTMX]; there
is no npm, no WASM, no client-side framework.

The Phase 2 deliverable is a **working configuration page for the `dsd-fp2`
driver**: read the driver's current configuration, edit it in a hand-built form,
and apply changes — all by calling the driver's own `config.get` / `config.apply`
ASCOM actions over HTTP (the cross-driver protocol implemented in Phase 1; see
[`dsd-fp2.md`](dsd-fp2.md) "Config Actions").

[axum]: https://github.com/tokio-rs/axum
[Maud]: https://maud.lambda.xyz/
[HTMX]: https://htmx.org/

## Naming and the `ui-*` family

This crate is the first member of a `ui-*` family of UI expressions. The naming
scheme distinguishes UI expressions along two axes — **technology** (for browser
targets) and **target** (for native):

| Crate | Target | Technology | Status |
|-------|--------|------------|--------|
| **`ui-htmx`** | browser | server-rendered HTMX | **this crate** |
| `ui-leptos` | browser | Leptos / WASM | future |
| `ui-ios` | iOS | native | future |
| `ui-android` | Android | native | future |
| `ui-core` | — | shared backend-for-frontend logic | extract when expression #2 lands |

A tech name (`htmx`, `leptos`) implies the browser target; a target name (`ios`,
`android`) implies native delivery. With HTMX the BFF and the frontend are
**fused** — the server renders the HTML directly — so `ui-htmx` is simultaneously
"the web BFF" and "the HTMX frontend". When a second expression appears, the
driver-client + config-model logic (target/tech-agnostic) is extracted into
`ui-core`; it would be premature with a single consumer today.

## Architecture

```
                 ┌────────────────────────────────────────┐
   browser ────► │  ui-htmx  (services/ui-htmx)            │  server-rendered HTML
   (HTMX)        │                                         │
                 │  main.rs ─► lib.rs (build_router)        │
                 │      │                                   │
                 │      ▼                                   │
                 │  pages/  (Maud templates + HTMX attrs)   │
                 │      │  renders form / fragments         │
                 │      ▼                                   │
                 │  ConfigClient (driver_client.rs)         │
                 │      │  get_config() / apply_config()    │
                 │      │  speaks the ASCOM action protocol │
                 │      ▼                                   │
                 │  HttpClient (io.rs)                      │
                 │      │  get() / put_form()  (reqwest,    │
                 │      │  rp-tls CA trust + Basic auth)    │
                 └──────┼──────────────────────────────────┘
                        │  PUT /api/v1/covercalibrator/0/action
                        │     Action=config.get | config.apply
                        ▼
                  [ dsd-fp2 ]   (ASCOM Alpaca driver, port 11119)
```

Two thin, independently mockable seams keep the handlers testable without a live
driver (the pattern `sentinel` uses for its Alpaca polling — see
[`sentinel.md`](sentinel.md)):

- **`HttpClient`** (`io.rs`) — `get(url)` / `put_form(url, params)`. Production
  impl wraps `reqwest` and is built through `rp_tls::client::build_reqwest_client`
  so it trusts the Rusty Photon CA, with optional HTTP Basic auth. Requests send
  `Connection: close` (no keep-alive pooling): a driver applies config by
  reloading — tearing its server down and rebinding — which leaves a pooled
  connection stale, and a non-idempotent `PUT` is not retried. A fresh connection
  per request lets the reconnect poll recover the instant the driver is back;
  config actions are low-frequency, so the lost pooling is immaterial. Mocked
  with `mockall` for unit tests of the layer above.
- **`ConfigClient`** (`driver_client.rs`) — `get_config()` /
  `apply_config(Value)`. Knows the ASCOM action protocol: shapes the
  `PUT .../action` request, unwraps the Alpaca envelope, and parses the inner
  JSON. The page handlers depend on `Arc<dyn ConfigClient>`, so the BDD/page
  tests inject a mock returning canned `ConfigGetResponse` / `ConfigApplyResponse`
  without constructing wire envelopes.

### The driver config-action client (wire contract)

Each driver exposes config over the standard ASCOM `Action` mechanism. The BFF
calls:

```
GET  /api/v1/{type}/{n}/supportedactions
   → Alpaca envelope, Value = ["config.get","config.apply", …]

PUT  /api/v1/{type}/{n}/action
       Action=config.get      Parameters=     ClientID=… ClientTransactionID=…
   → Alpaca envelope, Value = "<ConfigGetResponse as a JSON string>"

PUT  /api/v1/{type}/{n}/action
       Action=config.apply    Parameters=<full Config JSON>   ClientID=… …
   → Alpaca envelope, Value = "<ConfigApplyResponse as a JSON string>"
```

The **Alpaca envelope** wraps every response:

```jsonc
{ "Value": <result>, "ClientTransactionID": 0, "ServerTransactionID": 12,
  "ErrorNumber": 0, "ErrorMessage": "" }
```

`AlpacaConfigClient` parsing rules:

1. **HTTP non-2xx** → transport error (the driver's auth/TLS rejected us, or it is
   down). Rendered as a "driver unreachable / refused" banner.
2. **`ErrorNumber != 0`** → an ASCOM action error. `0x40C` (1036,
   `ACTION_NOT_IMPLEMENTED`) means the target is not a config-capable driver;
   surfaced as "this driver does not expose configuration". Other codes surface
   `ErrorMessage`.
3. **`ErrorNumber == 0`** → `Value` is a **JSON string**; parse it into the typed
   `ConfigGetResponse` / `ConfigApplyResponse`. (For `supportedactions`, `Value`
   is a JSON array, not a string.)

For `config.get` the inner body is `{ "config": <effective Config, secrets
redacted>, "overrides": ["serial.port"] }`. For `config.apply` it is the
classification body documented in [`dsd-fp2.md`](dsd-fp2.md) "config.apply"
(`status`, `applied`, `reload`, `restart_required`, `skipped_override`,
`persisted_to`, `errors`).

The config blob is treated as an **opaque `serde_json::Value`** by the transport
layer; only the hand-built page knows `dsd-fp2`'s field paths
(`serial.port`, `server.port`, `cover_calibrator.max_brightness`, …). This keeps
the BFF decoupled from the driver crate — it does **not** depend on `dsd-fp2` or
pull in its serial/transport dependencies.

## Routes (Phase 2)

| Method | Path | Purpose |
|--------|------|---------|
| `GET`  | `/` | Index: links to the configurable services (Phase 2: just `dsd-fp2`). |
| `GET`  | `/config/dsd-fp2` | Call `config.get`; render the hand-built form filled with current values. |
| `POST` | `/config/dsd-fp2` | Rebuild the full Config from the form, call `config.apply`; render the result state (see below). |
| `GET`  | `/config/dsd-fp2/status` | HTMX poll target during reconnect: try `config.get`; when the driver answers, swap in the refreshed form. |
| `GET`  | `/health` | Liveness; returns `OK`. |
| `GET`  | `/assets/app.css`, `/assets/htmx.min.js` | Embedded static assets (`include_str!`). |

### Form ⇆ Config mapping

The form edits a subset of fields; the rest must round-trip unchanged so
`config.apply` receives a complete `Config`. The page therefore carries the full
`config.get` blob (already secret-redacted) in a **hidden field**, and on POST:

1. Parse the hidden blob into a `serde_json::Value`.
2. Overlay each editable field onto it by JSON pointer (`/serial/port`,
   `/server/port`, `/cover_calibrator/max_brightness`, …).
3. Send the merged value as `Parameters` to `config.apply`.

This is the round-trip the protocol was designed for:

- **Override-pinned fields** (reported in `config.get`'s `overrides[]`) are
  rendered **read-only** with an explanation; the driver skips them on persist
  regardless (`skipped_override[]`), so even though the hidden blob carries the
  effective value, a transient `--port` is never baked into the file.
- **Redacted secrets** round-trip as the `********` sentinel; `config.apply`
  treats the sentinel as "leave unchanged", so a saved form never blanks a
  password hash.

## Behavioral contracts

### Rendering the page (`GET /config/dsd-fp2`)

- **Happy path:** `config.get` succeeds → render the form filled with the
  effective config. Fields listed in `overrides[]` are disabled and annotated
  "pinned by a command-line override".
- **Driver unreachable / refused:** `HttpClient` transport error or HTTP non-2xx
  → render an error banner naming the driver URL, with a retry link. The form is
  not shown (there is nothing to edit).
- **Non-config driver:** `ErrorNumber == ACTION_NOT_IMPLEMENTED` → render an
  explanation that the target driver does not expose configuration actions.

### Applying changes (`POST /config/dsd-fp2`)

- **`status:"applying"`** (a field needed a reload): render a "Saved — the driver
  is reloading…" state that **polls** `GET /config/dsd-fp2/status` via
  `hx-trigger="every 1s"`. When the poll's `config.get` succeeds, swap the
  reconnecting fragment for the refreshed form plus a "reconnected" confirmation.
  This is the same brief blip a process restart would cause; the BFF treats it as
  expected (see the reload mechanics in the plan).
- **`status:"ok"`** (persisted, nothing needed a reload): render "Saved." with the
  refreshed form; no reconnect poll.
- **`status:"invalid"`** (validation failed, file unchanged): re-render the form
  with each `errors[]` entry shown next to its field (`path` → field), preserving
  the submitted values so the user can correct them in place.
- **Transport / ASCOM error:** same banners as the GET path.

### Reconnect poll (`GET /config/dsd-fp2/status`)

- `config.get` **succeeds** → 200 with the refreshed form fragment (HTMX swaps it
  in and the polling stops).
- `config.get` **fails** (driver still down mid-reload) → 200 with the same
  reconnecting fragment so HTMX keeps polling. The blip is normally well under a
  second; the poll is bounded only by the user leaving the page.

## Configuration

The BFF has its own small config (it is not an ASCOM device). Phase 2 hard-codes a
single driver target; later the device list is derived from `rp`'s equipment
roster or ASCOM discovery (see [Deferred](#deferred)).

```jsonc
{
  "server": {
    "bind": "127.0.0.1",   // BFF listen address
    "port": 11120          // BFF listen port
  },
  "drivers": {
    "dsd-fp2": {
      "base_url": "http://127.0.0.1:11119",  // the driver's Alpaca base URL
      "device_type": "covercalibrator",
      "device_number": 0,
      "auth": null,            // optional { "username": "...", "password": "..." }
      "ca_cert_path": null     // optional PEM CA for a TLS-enabled driver
    }
  }
}
```

### CLI Arguments

| Argument | Description |
|----------|-------------|
| `-c, --config`     | Path to the BFF configuration file. If omitted, `Config::default()` is used (binds `127.0.0.1:11120`, targets `dsd-fp2` at `http://127.0.0.1:11119`). |
| `--port`           | BFF listen port (overrides `server.port`). |
| `-l, --log-level`  | Log level: trace, debug, info, warn, error. |

## Security

- **The BFF holds driver credentials**, in its own config, never in the page. It
  authenticates to a driver with HTTP Basic auth and trusts the Rusty Photon CA
  via `rp-tls` — the same client construction `sentinel` uses. Config actions are
  protected by whatever server-wide `rp-auth`/`rp-tls` the driver runs; the BFF is
  just an authorised client (see the plan's Security section).
- **Secrets are already redacted** by `config.get` (`********`), so they never
  reach the browser; the round-trip sentinel keeps them unchanged on apply.
- **BFF-side TLS/auth is deferred.** Phase 2 binds plain HTTP for the trusted
  warm-room-laptop → Pi LAN path. Putting the BFF itself behind `rp-tls`/`rp-auth`
  (or a reverse proxy) is future work, tracked in [Deferred](#deferred).

## MVP Scope

### In Scope (Phase 2)

- A working `dsd-fp2` configuration page: `GET` renders the current config,
  `POST` applies edits via `config.apply`.
- Validation surfacing (`status:"invalid"` → field-level errors, values
  preserved).
- The applying/reconnecting flow (`status:"applying"` → HTMX poll until the driver
  is back).
- Override-pinned fields rendered read-only with an explanation.
- Driver-unreachable / non-config-driver error states.
- Dark theme reusing the mock CSS tokens; assets embedded via `include_str!`
  (CSS + the HTMX bundle); no npm, no WASM.
- Plain-axum lifecycle under `rusty-photon-service-lifecycle::ServiceRunner` with
  graceful shutdown; prints `bound_addr=<host>:<port>` on bind (for BDD port
  discovery).

### Deferred

- **Multi-driver / equipment roster.** Phase 2 targets a single hard-coded
  `dsd-fp2`. Deriving the device list from `rp`'s `GET /api/equipment` or ASCOM
  discovery is Phase 3/5.
- **Schema-driven forms.** Hand-built form first; a `config.schema`-driven
  renderer that generalises across drivers is Phase 3.
- **Live telemetry + the activity stream.** The SSE-driven stream UI
  (`7-stream-fold.html`) follows on a separate track once `rp` has a real-time
  event stream.
- **Sentinel `service.restart` affordance** and the `restart_required` escalation
  (Phase 4).
- **BFF-side TLS/auth**, the **LCARS theme**, and **i18n**.

## Testing Strategy

Follows [`docs/skills/testing.md`](../skills/testing.md).

### BDD Tests (Cucumber)

`config_page.feature` is the canonical contract for the page behaviour. The steps
drive the **in-process axum router** via `tower::ServiceExt::oneshot`, with a
mocked `ConfigClient` in the `World` returning canned responses — no child
processes, so no `bdd_main!` macro is needed (purely in-process, per
testing.md §5.2). Scenarios:

- The config page renders the driver's current configuration.
- Override-pinned fields are shown read-only with an explanation.
- A valid change is applied and the page reports the driver is reconnecting.
- An invalid change re-renders the form with field-level errors, values preserved.
- An unreachable driver surfaces an error banner.

### Unit Tests

- `driver_client.rs`: `AlpacaConfigClient` shapes the `PUT .../action` request
  (form fields, device path) and parses the Alpaca envelope — `Value`-as-JSON-
  string extraction for `config.get`/`config.apply`, `ErrorNumber != 0` →
  error, `ACTION_NOT_IMPLEMENTED` mapping, HTTP-non-2xx → transport error. Mocks
  `HttpClient`.
- `pages`: form ⇆ Config reconstruction (hidden blob + editable overlay by JSON
  pointer; override-pinned not overlaid; redacted-secret sentinel round-trip).
- `config.rs`: defaults and JSON load.
- `io.rs`: `ReqwestHttpClient` connection-refused error path (mirrors sentinel).

## Module Structure

| Module | Description |
|--------|-------------|
| `config.rs` | `Config`, `ServerConfig`, `DriverTarget` + defaults + JSON load; CLI `--port` override. |
| `io.rs` | `HttpClient` trait (`#[cfg_attr(test, mockall::automock)]`) + `ReqwestHttpClient` (rp-tls CA trust + optional Basic auth). |
| `driver_client.rs` | `ConfigClient` trait + `AlpacaConfigClient`: ASCOM action request shaping, Alpaca-envelope parsing, `ConfigGetResponse`/`ConfigApplyResponse`/`FieldError` models, error mapping. |
| `pages/mod.rs` | Maud page + fragment templates (index, config form, reconnecting, error banners) and the form ⇆ Config mapping. |
| `assets.rs` | `include_str!` of `assets/app.css` + `assets/htmx.min.js`; asset routes. |
| `lib.rs` | `build_router`, `AppState`, public exports. |
| `main.rs` | CLI (clap) + tracing init; lifecycle owned by `ServiceRunner` (plain axum + graceful shutdown). |

## References

- Design plan: [`docs/plans/ui-design/config-actions.md`](../plans/ui-design/config-actions.md)
- Chosen UI direction + stack: [`docs/plans/ui-design/mocks/README.md`](../plans/ui-design/mocks/README.md)
- Driver config-action protocol (Phase 1): [`dsd-fp2.md`](dsd-fp2.md) "Config Actions"
- HTTP-client / mockall pattern: [`sentinel.md`](sentinel.md)
- Lifecycle: [`docs/skills/service-lifecycle.md`](../skills/service-lifecycle.md) "Plain axum service"
