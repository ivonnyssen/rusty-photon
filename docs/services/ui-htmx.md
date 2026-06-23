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

It renders a configuration page for **any** rusty-photon driver, **generated from
the driver's own JSON Schema** (`config.schema`) rather than a hand-built form:
read the driver's current configuration, edit it, and apply changes — all by
calling the driver's `config.get` / `config.schema` / `config.apply` ASCOM
actions over HTTP (the cross-driver protocol; see
[`config-actions.md`](config-actions.md)). One BFF configures any number of
drivers, each addressed by service id under `/config/{service}`.

Phase 2 shipped a hand-built `dsd-fp2`-only page; Phase 3b (this design) replaced
the hardcoded field lists with a **schema-driven renderer** that walks any
driver's `config.schema` into a form and reads its editability tiers
(`locked_fields` / `read_only_fields`) from the schema, so a new driver needs **no
BFF changes** to get a config page.

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
- **`ConfigClient`** (`driver_client.rs`) — `get_config()` / `get_schema()` /
  `apply_config(Value)`. Knows the ASCOM action protocol: shapes the
  `PUT .../action` request, unwraps the Alpaca envelope, and parses the inner
  JSON into the shared `rusty_photon_config::actions` wire types. The page handlers depend on `Arc<dyn ConfigClient>`, so a handler unit
  test can inject a stub (via `AppState::with_client`) to cover an error state
  a live driver won't produce — see [Testing Strategy](#testing-strategy). The
  end-to-end BDD suite, by contrast, runs against a real driver, not a stub.

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
layer; the page discovers field paths from the driver's `config.schema` at
request time, so it hardcodes **no** driver-specific field knowledge. This keeps
the BFF decoupled from every driver crate — it depends only on the light,
driver-agnostic `rusty-photon-config` crate for the shared wire types, and pulls
in no driver's serial/transport dependencies.

## Routes

The config routes are **service-scoped** (`{service}` is the driver's key in the
BFF config's `drivers` map), so one BFF serves every configured driver.

| Method | Path | Purpose |
|--------|------|---------|
| `GET`  | `/` | Index: links to every configured driver (`/config/{service}`). |
| `GET`  | `/config/{service}` | Call `config.schema` + `config.get`; render the form generated from the schema, filled with current values. An optional `?unlock=<field>` query renders one locked/identity field (e.g. a device `unique_id`) editable — the read-only-by-default escape hatch. An unknown `{service}` renders an error card. |
| `POST` | `/config/{service}` | Re-fetch `config.schema` to coerce the form back into the full Config, call `config.apply`; render the result state (see below). |
| `GET`  | `/config/{service}/status` | HTMX poll target during reconnect: try `config.schema` + `config.get`; when the driver answers, swap in the refreshed form. Honours the same optional `?unlock=` query. |
| `GET`  | `/health` | Liveness; returns `OK`. |
| `GET`  | `/assets/app.css`, `/assets/htmx.min.js` | Embedded static assets (`include_str!`). |

### Schema-driven rendering (`FieldModel`)

The form is **generated from the driver's `config.schema`**, not a hardcoded
field list. `FieldModel::from_schema` walks the JSON Schema into a flat, ordered
list of scalar leaves:

- **`$ref` into `$defs` is resolved**, and plain objects are recursed, so nested
  config sections (`serial.port`, `server.discovery_port`, …) become dotted leaf
  paths.
- **`oneOf` / `anyOf` / `allOf` / `enum` / `const` subtrees are skipped** — an
  optional nested struct (`Option<TlsConfig>`/`Option<AuthConfig>`), a tagged
  enum (`star-adventurer`'s `transport`), or a custom-serde type is not rendered
  as editable inputs; it **round-trips untouched** through the hidden blob. This
  is exactly how redacted secrets (which live inside such optional structs) stay
  safe — they are never rendered, only carried through.
- Each leaf's **`FieldKind`** is inferred from the schema: `string` → text,
  `boolean` → checkbox, `integer`/`number` → numeric input, with the schema's
  `minimum`/`maximum` and nullability (`type:["integer","null"]`) driving
  coercion. Fields are grouped into a `<fieldset>` per top-level section.

The form edits a subset of fields; the rest must round-trip unchanged so
`config.apply` receives a complete `Config`. The page therefore carries the full
`config.get` blob (already secret-redacted) in a **hidden field**, and on POST
re-fetches `config.schema` to rebuild the `FieldModel`, then overlays each
editable leaf onto the blob by JSON pointer and sends the merged value as
`Parameters` to `config.apply`. This is the round-trip the protocol was designed
for:

- **Override-pinned fields** (reported in `config.get`'s `overrides[]`) are
  rendered **read-only** with an explanation; the driver skips them on persist
  regardless (`skipped_override[]`), so even though the hidden blob carries the
  effective value, a transient `--port` is never baked into the file.
- **Redacted secrets** are never rendered (they live inside the `anyOf`/optional
  subtrees the walker skips) and round-trip as the `********` sentinel in the
  hidden blob; `config.apply` treats the sentinel as "leave unchanged", so a
  saved form never blanks a password hash.
- **Numeric fields are parsed into their bounded types** (`u16` ports, `u32`
  baud/brightness). An out-of-range or non-numeric value becomes a field-level
  error (re-rendered, not sent), rather than silently coercing to `0` or
  producing a non-field driver parse error. An empty *required* number keeps the
  prior value (clearing a port can't silently become OS-assigned); an empty
  *optional* number (`discovery_port`) persists `null`.
- **Read-only fields come from the driver, not a BFF list.** The hard-read-only
  tier is whatever the driver reports in `config.schema`'s `read_only_fields`
  (e.g. `server.port` — a rebind the BFF can't follow — and a device `enabled`
  flag — disabling the device unregisters the very endpoint the config actions
  live on). The BFF renders these disabled and `merge_form` never overlays them
  (they round-trip from the blob), so the UI can't edit away its own
  reachability. A new driver decides its own self-lockout guards by listing them
  in `read_only_fields`; the BFF needs no change. (This governs the **UI path**
  only — a hand-crafted POST that edits a read-only field inside the `__config`
  blob is equivalent to any forged config and is the driver's job to reject.)

#### Field-editability tiers

The form classifies each field into one of four tiers, evaluated in this order
(the first that applies wins for the `disabled` state, and `merge_form` mirrors
the same precedence when deciding whether to overlay a submitted value). **Every
tier is sourced from the driver** — `config.get`'s `overrides[]` and
`config.schema`'s `locked_fields` / `read_only_fields` — never a BFF-side list:

| Tier | Source | Disabled? | Overlaid by `merge_form`? |
|------|--------|-----------|---------------------------|
| **Override-pinned** | `config.get` `overrides[]` (CLI flags) | yes | never (driver skips it anyway) |
| **Hard read-only** | `config.schema` `read_only_fields` (e.g. `server.port`, a device `enabled`) | yes, always — no escape hatch | never |
| **Locked / identity** | `config.schema` `locked_fields` (e.g. a device `unique_id`) | yes **by default**; no once unlocked | only when unlocked **and** not pinned |
| **Editable** | every other schema leaf | no | yes (unless pinned/read-only) |

Pinned always wins: an override-pinned field stays read-only even if it is also a
locked/identity field that the user unlocked.

- **A device `unique_id` is a *locked / identity* field — read-only by default
  behind a deliberate escape hatch**, distinct from the hard read-only tier
  above. The driver **owns and generates** its ASCOM `UniqueID`, so editing it
  from the page is an escape hatch for a *misbehaving driver*, not routine
  configuration. By default the field renders **disabled** with the hint
  *"Identity — the driver owns this. Editing is an escape hatch for a misbehaving
  driver."* and an **"Unlock to edit"** link
  (`GET /config/{service}?unlock=<field>`). Following it re-renders the same card
  with the field **enabled**, a warning, and a **"Lock again"** link
  (`GET /config/{service}`, no query). The unlock state is carried with **no
  client-side JS**:
  - On a **GET**, the `?unlock=<field>` query (axum `Query`) names the field to
    unlock; only a name in the schema's `locked_fields` is honoured (a
    hard-read-only field, a typo, or no query unlocks nothing).
  - The rendered card emits a hidden `__unlocked` field
    (`serde_json::to_string` of the unlocked set) alongside `__config` /
    `__overrides`, so on **POST** the unlocked set round-trips. `merge_form`
    overlays a locked field from its form value **only if** `__unlocked` lists it
    **and** it is not override-pinned; otherwise it round-trips from the hidden
    blob untouched. An invalid submission re-renders with the field still
    unlocked (the operator's in-progress edit is preserved); a successful apply
    re-locks it. Unlike `__config` / `__overrides` (required and validated),
    `__unlocked` is **optional** and a malformed value is treated as "nothing
    unlocked" — the safe default keeps the field read-only, and the overlay gate
    still requires the name to be present, so a forged or absent `__unlocked` can
    never *edit* a locked field. The set is filtered to the schema's
    `locked_fields`, so a forged `__unlocked` can only ever unlock a field that is
    genuinely a locked/identity field — never a hard-read-only one.

  (As with the read-only tier, this governs the **UI path** only. A hand-crafted
  POST that edits a locked field inside the `__config` blob is equivalent to any
  forged config and is the driver's job to reject — driver-side identity
  validation lands separately.)

## Behavioral contracts

### Rendering the page (`GET /config/{service}`)

- **Unknown service:** a `{service}` not in the `drivers` map → render an error
  card ("No configured driver named …").
- **Happy path:** `config.schema` + `config.get` succeed → render the
  schema-generated form filled with the effective config. Fields listed in
  `overrides[]` are disabled and annotated "pinned by a command-line override".
- **Locked/identity escape hatch:** a `locked_fields` entry (e.g. a device
  `unique_id`) is disabled by default with an "Unlock to edit" link.
  `GET /config/{service}?unlock=<field>` re-renders the card with that locked
  field editable (only names in the schema's `locked_fields` are honoured); the
  no-query URL re-locks it. See
  [Field-editability tiers](#field-editability-tiers).
- **Driver unreachable / refused:** `HttpClient` transport error or HTTP non-2xx
  (on either `config.schema` or `config.get`) → render an error banner naming the
  driver URL, with a retry link. The form is not shown (there is nothing to edit).
- **Non-config driver:** `ErrorNumber == ACTION_NOT_IMPLEMENTED` → render an
  explanation that the target driver does not expose configuration actions.

### Applying changes (`POST /config/{service}`)

- **`status:"applying"`** (a field needed a reload): render a "Saved — the driver
  is reloading…" state that **polls** `GET /config/{service}/status` via
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

### Reconnect poll (`GET /config/{service}/status`)

- `config.get` **succeeds** → 200 with the refreshed form fragment (HTMX swaps it
  in and the polling stops).
- `config.get` **fails** (driver still down mid-reload) → 200 with the same
  reconnecting fragment so HTMX keeps polling. The blip is normally well under a
  second; the poll is bounded only by the user leaving the page.

## Configuration

The BFF has its own small config (it is not an ASCOM device). The `drivers` map
is keyed by service id (the `{service}` path segment); add an entry per driver.
The default config carries a single local `dsd-fp2` so `cargo run` works with no
config file. Later the list is also derivable from `rp`'s equipment roster or
ASCOM discovery (see [Deferred](#deferred)).

```jsonc
{
  "server": {
    "bind": "127.0.0.1",   // BFF listen address
    "port": 11120          // BFF listen port
  },
  "drivers": {
    "dsd-fp2": {
      "name": "Deep Sky Dad FP2",            // optional display name (defaults to the id)
      "base_url": "http://127.0.0.1:11119",  // the driver's Alpaca base URL
      "device_type": "covercalibrator",
      "device_number": 0,
      "auth": null,            // optional { "username": "...", "password": "..." }
      "ca_cert_path": null     // optional PEM CA for a TLS-enabled driver
    },
    "qhy-focuser": {
      "base_url": "http://127.0.0.1:11113",
      "device_type": "focuser"
    }
  }
}
```

### CLI Arguments

| Argument | Description |
|----------|-------------|
| `-c, --config`     | Path to the BFF configuration file. If omitted, `Config::default()` is used (binds `127.0.0.1:11120`, with a single `dsd-fp2` driver at `http://127.0.0.1:11119`). |
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
- **Binds loopback by default; BFF-side TLS/auth is deferred.** The default
  config binds `127.0.0.1`, reachable only from the host (e.g. via an SSH tunnel
  from the warm-room laptop). Exposing the BFF on the LAN means binding `0.0.0.0`,
  which serves the config pages over **plain HTTP with no BFF-side auth** — so
  until BFF TLS/auth lands ([Deferred](#deferred)), reach it through an SSH tunnel
  or a reverse proxy that terminates TLS + auth, rather than a raw `0.0.0.0` bind.
  (The driver credentials the BFF holds are unaffected — the BFF is a client, and
  each driver still enforces its own `rp-auth`/`rp-tls`.)

## MVP Scope

### In Scope

- A working configuration page for **any** configured driver, generated from its
  `config.schema`: `GET` renders the current config, `POST` applies edits via
  `config.apply`. One BFF serves many drivers at `/config/{service}`.
- Validation surfacing (`status:"invalid"` → field-level errors, values
  preserved), plus BFF-side numeric coercion (schema-bounded) before apply.
- The applying/reconnecting flow (`status:"applying"` → HTMX poll until the driver
  is back).
- Editability tiers (override-pinned, hard read-only, locked/identity) sourced
  from the driver's `config.get`/`config.schema`, with the "unlock to edit"
  escape hatch.
- Driver-unreachable / non-config-driver / unknown-service error states.
- Dark theme reusing the mock CSS tokens; assets embedded via `include_str!`
  (CSS + the HTMX bundle); no npm, no WASM.
- Plain-axum lifecycle under `rusty-photon-service-lifecycle::ServiceRunner` with
  graceful shutdown; prints `bound_addr=<host>:<port>` on bind (for BDD port
  discovery).

### Deferred

- **Equipment-roster-driven driver list.** The `drivers` map is configured by
  hand today; deriving it from `rp`'s `GET /api/equipment` or ASCOM discovery is
  Phase 5.
- **Composite-field rendering.** The schema walker skips `oneOf`/`anyOf`/`enum`
  subtrees (tagged enums like `star-adventurer`'s `transport`, optional nested
  structs), so those fields round-trip read-only via the hidden blob rather than
  rendering an editable discriminated form. A generic `oneOf`/enum renderer (and
  a dedicated password input for redacted-secret leaves) is a follow-up; until
  then such fields are edited in the driver's config file.
- **Live telemetry + the activity stream.** The SSE-driven stream UI
  (`7-stream-fold.html`) follows on a separate track once `rp` has a real-time
  event stream.
- **Sentinel `service.restart` affordance** and the `restart_required` escalation
  (Phase 4).
- **BFF-side TLS/auth**, the **LCARS theme**, and **i18n**.

## Testing Strategy

Follows [`docs/skills/testing.md`](../skills/testing.md).

### BDD Tests (Cucumber)

`config_page.feature` is the canonical contract for the page behaviour, and —
like every other service — it exercises the **real binaries end to end**. Each
scenario spawns the real `ui-htmx` process and a real `dsd-fp2` driver in mock
mode (via `bdd_infra::ServiceHandle`), points the BFF at the driver, and drives
the BFF over HTTP, asserting on the HTML it actually renders. There is no
in-process router and no stubbed `ConfigClient`: the production
`ReqwestHttpClient` → `AlpacaConfigClient` path and the driver's real
`config.get` / `config.apply` / in-process reload all run for real. The entry
point therefore uses `bdd_infra::bdd_main!` (child-process spawning, skipped
under Miri), and **both binaries must be pre-built with `--all-features`** (the
driver's mock transport is feature-gated):

```
cargo build --all-features --all-targets
cargo test  --all-features --test bdd -p ui-htmx
```

**Assertions are DOM-based, and the helpers follow the htmx contract.** The
Then-steps parse each response with [`scraper`] (the Servo html5ever/selectors
stack browsers ship) and assert with CSS selectors — `input[name="…"]`
editability, the `div.banner.applying` reload state, the `#config-card`
`hx-get`/`hx-trigger` poll wiring — rather than `String::contains` substrings
(which mishandle attribute order, boolean attributes, and the value buried in the
hidden `__config` blob). The request helpers drive the BFF the way htmx would:
`submit_form` reads the hidden blobs and enabled controls from the **rendered**
form and POSTs them with the `HX-*` header set (disabled fields are omitted, just
as a browser omits them); the unlock step follows the page's own rendered
`hx-get` link; and the reconnect poll matches the refreshed input's `value`. This
is Layer A of the [UI-testing plan](../plans/ui-testing.md), proving the page's
markup and `hx-*` wiring are correct (obligation P1) without a browser; `scraper`
is a test-only dev-dependency and is never compiled into the shipped binary.

[`scraper`]: https://docs.rs/scraper/

**Byte-equivalence snapshots ride the same scenarios** (Layer B / P2). Selected
Then-steps also capture the response's exact bytes as committed [`insta`] goldens
under `tests/snapshots/` — the server's output is the *cross-OS-comparable*
artifact, since htmx swaps a fragment verbatim, so byte-identical output across
OSes implies identical browser behavior without a browser on every OS. The
driver's OS-assigned `:0` bound port is filtered to `<port>` (the only
run-varying token); the driver-unreachable error card is *not* snapshotted (its
banner carries an OS-specific connection-refused string — the case where the P1
DOM check stands in for P2). Goldens are updated Cargo-locally (`cargo insta
review` / `accept`, then commit) and compared read-only under Bazel
(`INSTA_UPDATE=no`, goldens shipped via the `bdd` target's `data`); a runtime
resolver finds them under both build systems' layouts.

[`insta`]: https://insta.rs/

**Real-browser scenarios are opt-in** (Layer C / P3). `tests/features/browser.feature`
(tagged `@browser`) drives a real headless Firefox via [`thirtyfour`] + geckodriver
to prove the one thing server-output layers cannot: that the vendored
`htmx.min.js` actually loads and executes the declared swaps. They are **gated
behind `UI_BROWSER_TESTS=1`** (an env var, not a cargo feature, so browser flake
never enters the `--all-features` required gate) and run on a single environment —
the P1/P2 server-bytes layers carry the cross-OS guarantee. geckodriver is an
external system tool (`GECKODRIVER_BINARY`, like `OMNISIM_PATH`); teardown quits
the browser before the BFF/driver stop. Run them with:

```
UI_BROWSER_TESTS=1 GECKODRIVER_BINARY=/path/to/geckodriver \
  cargo test --all-features --test bdd -p ui-htmx
```

`browser.feature` carries four scenarios. Two prove htmx executes — a smoke render
(proves `htmx.min.js` loads) and an unlock-click `outerHTML` swap. Two are Tier 0
robustness checks from the [UI-testing plan](../plans/ui-testing.md) §9 that harden
teardown so BDD subprocess coverage is never silently lost (the §5.4 hazard in
[`testing.md`](../skills/testing.md)):

- **Coverage invariant.** Quitting the browser *before* stopping the BFF lets the
  BFF shut down gracefully and run its `atexit` coverage flush; the scenario
  asserts the stop returns well under the 5s SIGKILL grace, plus a
  `COVERAGE_DIR`-gated non-empty `ui-htmx-*.profraw` check under a coverage run.
- **Worst-case orphan reaper.** geckodriver is spawned in **its own process
  group**, so a *simulated* crash (SIGKILL geckodriver, orphaning Firefox) can be
  cleaned up by a kill-the-tree reaper (`killpg` of the group); the scenario
  asserts zero survivors (a `/proc` scan scoped to that group, so it can never
  match a developer's own Firefox) and that a screenshot + page source landed at
  an absolute, chdir-safe path before the reap.

> **Determinism note.** `thirtyfour` hard-requires `serde_json`'s `preserve_order`
> feature, which unifies across the workspace under `--all-features`. Because the
> form is generated by walking a `serde_json::Value` schema and the hidden blob is
> a serialized `Value`, that feature would otherwise reorder the rendered output
> (map iteration order changes). `pages/mod.rs` therefore sorts schema properties
> explicitly and serializes the blob through `canonical_json`, so the output is
> byte-identical regardless of the feature — keeping the P2 snapshots stable across
> Cargo (`--all-features`) and Bazel (where the binary has no dev-deps).

[`thirtyfour`]: https://docs.rs/thirtyfour/

The driver binds port 0, so the OS assigns a free port atomically (no racy
preselection); the test discovers it from the driver's `bound_addr=` stdout line.
The one scenario that reloads and reconnects first pins that bound port into the
driver's config via a direct `config.apply`, so the in-process reload rebinds the
*same* port and the BFF can reconnect (the override scenario additionally spawns
the driver with `--port` via `ServiceHandle::start_with_args`). Because the form
is now schema-driven, these scenarios also exercise the real `config.schema`
call end to end. Scenarios:

- The config page renders the driver's current configuration.
- A serial-port override is shown read-only with an explanation.
- The `cover_calibrator.unique_id` identity field is read-only by default with an
  "unlock to edit" affordance, and becomes editable when opened with
  `?unlock=cover_calibrator.unique_id` (the read-only-by-default escape hatch).
- A valid change is applied and the page reports the driver is reloading + polls
  `…/status`.
- The reloaded driver's new configuration is served back through the page —
  drives the real `config.apply` → reload → rebind → `config.get` round trip.
- An unchanged submission reports it was saved with no reload (`status:"ok"` —
  the only no-reload path, since the driver classifies *any* changed field as a
  reload).
- An invalid change re-renders the form with the driver's field-level error,
  the submitted value preserved.
- An unreachable driver surfaces an error banner.
- One BFF exposes the driver under two service ids: the index links to both and
  each `/config/{service}` route renders independently (multi-driver routing).

### Unit Tests

- `driver_client.rs`: `AlpacaConfigClient` shapes the `PUT .../action` request
  (form fields, device path) and parses the Alpaca envelope — `Value`-as-JSON-
  string extraction for `config.get`/`config.schema`/`config.apply`,
  `ErrorNumber != 0` → error, `ACTION_NOT_IMPLEMENTED` mapping, HTTP-non-2xx →
  transport error. Mocks `HttpClient`. (The wire types are re-exported from
  `rusty_photon_config::actions`, so there is nothing driver-specific to test.)
- `lib.rs`: multi-driver `AppState` (`from_config` builds every driver; rejects
  URL-embedded credentials); the handler renders the "this driver does not expose
  configuration" banner on `ACTION_NOT_IMPLEMENTED` and the "no configured driver"
  card for an unknown `{service}` — error states the end-to-end suite can't
  produce — driven in-process through `AppState::with_client` with a stub
  `ConfigClient`.
- `pages`: the schema walker (`$ref` resolution, plain-object recursion,
  `anyOf`/`oneOf` skipping, `FieldKind` inference); schema-driven form ⇆ Config
  reconstruction (hidden blob + editable overlay by JSON pointer; override-pinned
  and read-only not overlaid; numeric coercion against schema bounds; float
  leaves; redacted-secret round-trip); the locked/identity tier (disabled by
  default, editable when `__unlocked`/`?unlock=` names it, pinned still wins, a
  forged `__unlocked` can't unlock a non-locked field); the index listing.
- `config.rs`: defaults (single dsd-fp2), the multi-driver map, and JSON load.
- `io.rs`: `ReqwestHttpClient` connection-refused error path (mirrors sentinel).

## Module Structure

| Module | Description |
|--------|-------------|
| `config.rs` | `Config`, `ServerConfig`, the `Drivers` map + `DriverTarget` + defaults + JSON load. |
| `io.rs` | `HttpClient` trait (`#[cfg_attr(test, mockall::automock)]`) + `ReqwestHttpClient` (rp-tls CA trust + optional Basic auth). |
| `driver_client.rs` | `ConfigClient` trait + `AlpacaConfigClient`: `config.get`/`config.schema`/`config.apply` request shaping, Alpaca-envelope parsing, error mapping. Re-exports the shared wire types from `rusty_photon_config::actions`. |
| `pages/mod.rs` | The schema-driven renderer: `FieldModel` (schema walker + `FieldKind`), `config_card`/`index`/fragment templates, and the schema-driven `merge_form` form ⇆ Config coercion. |
| `assets.rs` | `include_str!` of `assets/app.css` + `assets/htmx.min.js`; asset routes. |
| `lib.rs` | `build_router`, multi-driver `AppState`, the `/config/{service}` handlers, public exports. |
| `main.rs` | CLI (clap) + tracing init; lifecycle owned by `ServiceRunner` (plain axum + graceful shutdown). |

## References

- Design plan: [`docs/plans/ui-design/config-actions.md`](../plans/ui-design/config-actions.md)
- Chosen UI direction + stack: [`docs/plans/ui-design/mocks/README.md`](../plans/ui-design/mocks/README.md)
- Driver config-action protocol (Phase 1): [`dsd-fp2.md`](dsd-fp2.md) "Config Actions"
- HTTP-client / mockall pattern: [`sentinel.md`](sentinel.md)
- Lifecycle: [`docs/skills/service-lifecycle.md`](../skills/service-lifecycle.md) "Plain axum service"
