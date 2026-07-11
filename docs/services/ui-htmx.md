# ui-htmx Service (web BFF)

## Overview

`ui-htmx` is the browser-facing, **server-rendered web UI** for rusty-photon —
the web UI described in
[`docs/plans/ui-design/config-actions.md`](../plans/ui-design/config-actions.md)
with the chosen visual direction in
[`docs/plans/ui-design/mocks/README.md`](../plans/ui-design/mocks/README.md). It
is a **standalone backend-for-frontend (BFF)**: a client of the rest of the
system that holds no UI logic inside `rp` (`rp.md` tenet 7). The service renders
HTML on the server with [axum] + [Maud] and adds interactivity with [HTMX]; there
is no npm, no WASM, no client-side framework.

It serves three surfaces, one nav:

1. **Configuration pages** (`/config/{service}`, index at `/`) — a
   schema-driven config form for any rusty-photon driver (Phases 2–3), for
   `rp` itself over its REST config API, and for devices discovered from
   `rp`'s equipment roster (Phase 5).
2. **Equipment page** (`/equipment`) — `rp`'s equipment roster: live
   connection state, a managed/foreign capability tier per device, and
   add / edit / remove of roster entries by editing `rp`'s config over REST.
3. **Activity stream** (`/stream`) — the live session narrative from the
   [`7-stream-fold.html`](../plans/ui-design/mocks/7-stream-fold.html) mock:
   `rp`'s real-time event stream rendered server-side and pushed to the
   browser over SSE.

The equipment and stream surfaces exist only when the BFF config carries an
[`rp` target](#configuration); without one, `ui-htmx` is the pure driver-config
UI it was in Phase 3.

**JavaScript (htmx) is required.** The UI does not carry a no-JS fallback: the
form submits via `hx-post` (no `method`/`action`), and the unlock/lock/retry
affordances are `<button hx-get>` (no `<a href>`), so without htmx loaded the page
renders but is inert. This is a deliberate decision (UI-testing plan §7): the UI is
**optional** — rusty-photon runs fully headless — and the genuine recovery path is
ssh + editing the config file, strictly more capable than a degraded web form; a
whole-app no-JS guarantee is also incompatible with the future real-time stream UI.
Direct navigation/refresh still returns a full styled page (the `HX-Request`
full-page-vs-fragment branch is core htmx, not a no-JS feature).

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
BFF config's `drivers` map, the literal `rp`, or a roster-derived
`rp:{kind}:{id}` key — see [Config-page targets](#config-page-targets)), so one
BFF serves every configured driver.

| Method | Path | Purpose |
|--------|------|---------|
| `GET`  | `/` | Index: links to every configured driver (`/config/{service}`), the `rp` config page, and roster-derived devices when an `rp` target is configured. |
| `GET`  | `/config/{service}` | Call `config.schema` + `config.get`; render the form generated from the schema, filled with current values. An optional `?unlock=<field>` query renders one locked/identity field (e.g. a device `unique_id`) editable — the read-only-by-default escape hatch. An unknown `{service}` renders an error card. |
| `POST` | `/config/{service}` | Re-fetch `config.schema` to coerce the form back into the full Config, call `config.apply`; render the result state (see below). |
| `GET`  | `/config/{service}/status` | HTMX poll target during reconnect: try `config.schema` + `config.get`; when the driver answers, swap in the refreshed form. Honours the same optional `?unlock=` query. |
| `GET`  | `/equipment` | The [equipment page](#equipment-page-equipment): rp's roster with live connection LEDs, capability tiers, and add/edit/remove affordances. |
| `GET`  | `/equipment/{kind}/new` | Schema-generated "add device" form for one equipment kind (`cameras`, `filter_wheels`, `cover_calibrators`, `focusers`, `safety_monitors`, `mount`). |
| `POST` | `/equipment/{kind}/new` | Insert the new entry into rp's config (`GET /api/config` → splice → `PUT /api/config`); render the roster with the apply outcome. |
| `GET`  | `/equipment/{kind}/{id}/edit` | Edit form for one roster entry, prefilled from rp's config (the singular `mount` uses the fixed id `mount`). |
| `POST` | `/equipment/{kind}/{id}/edit` | Replace that entry in rp's config and apply. |
| `POST` | `/equipment/{kind}/{id}/delete` | Remove that entry from rp's config and apply. |
| `GET`  | `/stream` | The [activity stream](#activity-stream-stream) page. |
| `GET`  | `/stream/events` | The SSE proxy: rp's event stream rendered as HTML fragments (see [SSE proxy](#the-sse-proxy-streamevents)). |
| `GET`  | `/stream/equipment` | Fold-panel equipment-LED fragment; the panel re-fetches it on an htmx timer. |
| `GET`  | `/health` | Liveness; returns `OK`. |
| `GET`  | `/assets/app.css`, `/assets/htmx.min.js`, `/assets/htmx-ext-sse.js` | Embedded static assets (`include_str!`). The SSE extension ([htmx-ext-sse] 2.2.3, vendored) is loaded only by pages that stream. |

[htmx-ext-sse]: https://github.com/bigskysoftware/htmx-extensions/tree/main/src/sse

Every page shares the [`layout`] shell, whose top nav carries the three
surfaces — **Activity** (`/stream`), **Equipment** (`/equipment`),
**Configuration** (`/`) — with the active tab highlighted, plus the mock's
pure-CSS **night-vision toggle** (a page-level red filter preserving dark
adaptation; no JavaScript).

[`layout`]: ../../services/ui-htmx/src/pages/mod.rs

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
  with the field **enabled**, a warning, and a **"Lock again"** affordance
  (`GET /config/{service}`, no query). The unlock state is carried with **no
  bespoke client-side JavaScript** (htmx performs the GET + swap; there is no
  hand-written JS):
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

## Config-page targets

`/config/{service}` resolves its target in three ways; the page machinery
(schema walk, tiers, merge, apply states) is identical for all three:

1. **Static driver** — `{service}` is a key in the BFF config's `drivers` map;
   the client is `AlpacaConfigClient` speaking the ASCOM action protocol.
2. **`rp` itself** — the literal key `rp`, present when the BFF config carries
   an [`rp` target](#configuration); the client is `RestConfigClient` speaking
   the same protocol as plain REST (`GET /api/config`, `GET /api/config/schema`,
   `PUT /api/config` — see [`config-actions.md`](config-actions.md) "REST
   transport"). A static `drivers` entry named `rp` is rejected at startup to
   keep the key unambiguous. Because rp classifies every change as
   `restart_required` (it has no in-process reload), the apply result renders
   the **restart callout** instead of the reconnect poll: "Saved — restart rp
   to apply:" plus the changed paths. (Phase 4 will attach the "Restart via
   Sentinel" affordance here.) rp's equipment arrays are `oneOf`-free but
   *array-typed*, which the schema walker skips — so on the rp config page they
   round-trip untouched via the hidden blob, and are edited on the
   [equipment page](#equipment-page-equipment) instead. rp's optional blocks
   (`site`, `guider`, `plate_solver`, `planner`) blob-round-trip the same way
   under the standard composite-skip rule; the page edits the scalar leaves
   (`session`, `safety`, `imaging`, `centering`, `server`).
3. **Roster-derived device** — a key of the form `rp:{kind}:{id}` (e.g.
   `rp:cameras:main-cam`, `rp:mount:mount`), synthesized on demand from rp's
   config: the device's `alpaca_url` + device number come from its roster
   entry, and the ASCOM device type from which array it sits in
   (`cameras`→`camera`, `filter_wheels`→`filterwheel`,
   `cover_calibrators`→`covercalibrator`, `focusers`→`focuser`,
   `safety_monitors`→`safetymonitor`, `mount`→`telescope`). The BFF calls the
   device **without credentials** (rp redacts per-device auth, rightly), so a
   driver behind auth renders the transport-error banner with a hint to add a
   static `drivers` entry carrying credentials.

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
  refreshed form; no reconnect poll. When `restart_required[]` is non-empty
  (the `rp` target — `ApplyDisposition::Restart`), the banner becomes the
  **restart callout**: "Saved — restart rp to apply:" with the changed paths
  listed; the form re-renders from the *running* (pre-restart) config, which is
  honest about what is currently in effect.
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

## Equipment page (`/equipment`)

The roster view of the observatory, per the
[federated-roster design](../plans/ui-design/config-actions.md#federated-roster-managed-own-vs-foreign-devices).
Its two data sources are joined by device `id`:

- **`GET /api/config`** (rp) — the authoritative device list: every equipment
  entry with its `alpaca_url`, device number, and settings (secrets redacted).
- **`GET /api/equipment`** (rp) — live state: `{ id, connected }` per device
  (the singular mount has no id).

Per device the page renders: name/id, kind, address, a **connected LED**, the
**capability tier**, and Edit / Remove / Configure affordances. The tier comes
from a bounded, concurrent **capability probe** against the device's own Alpaca
server (short per-probe timeout, all devices probed in parallel at render
time):

| Probe result | Tier | Affordance |
|---|---|---|
| `supportedactions` lists `config.get` | **Managed** | "Configure" → `/config/rp:{kind}:{id}` |
| 2xx but no `config.*`; `/setup/v1/{type}/{n}/setup` reachable | **Setup page** | external link to the device's own setup UI |
| 2xx but no `config.*`, no setup page | **Control only** | badge |
| 401/403 | **Auth required** | badge + hint to add a static `drivers` entry with credentials |
| transport error / timeout | **Unreachable** | badge |

Because `config.*` is self-advertising, any third-party server adopting the
convention auto-upgrades to *Managed* — the probe is the capability detection,
not a hardcoded table.

**Editing the roster edits rp's config.** Add / edit / remove perform a
read-modify-write on the equipment arrays: `GET /api/config` → splice the entry
→ `PUT /api/config`, surfacing the apply outcome (validation errors render
field-level, re-anchored from rp's absolute paths onto the entry form; success
renders the restart callout, since roster changes take effect on the next rp
start). **The list always shows the roster rp is *running***: `GET /api/config`
returns the effective config, so a just-persisted entry appears (or a removed
one disappears) only after rp's next start — the callout names the pending
paths, which is the honest state until Phase 4's restart affordance lands. An
empty form input means "unset — rp's default applies"; it is never sent as an
empty string (which would fail rp's typed parses, e.g. a humantime
`poll_interval`). The add/edit forms are **schema-generated per
kind**: the field list comes from walking that kind's item subschema inside
`GET /api/config/schema` (the same `FieldModel` walker the config pages use,
entered at the array's item definition), so a new field on, say,
`CameraConfig` appears on the form with **no BFF change**. Composite leaves
(e.g. a device's optional `auth` block) follow the same rule as config pages —
skipped by the walker, preserved on edit, absent on add — and are edited in
rp's config file when needed. The mount is singular: "add" is offered only
when `mount` is `null`, and its routes use the fixed id `mount`.

**Deferred:** per-device **connect/disconnect** buttons — rp's registry is
built once at startup and has no runtime connect/disconnect endpoints yet
(marked *(planned)* in [`rp.md`](rp.md)); the LEDs show live truth and the
roster edits the config, which is what exists today. ASCOM UDP discovery
pre-fill remains low-priority per the plan.

**rp unreachable:** the page renders the same error banner + retry as a config
page whose driver is down; roster mutations are disabled with the banner shown.

## Activity stream (`/stream`)

The narrative session view from the chosen mock
([`7-stream-fold.html`](../plans/ui-design/mocks/7-stream-fold.html)):
a single-column **event feed** telling the session's story newest-first, a
sticky **status strip** under the nav, and a **fold panel** (the CSS Grid
`0fr → 1fr` checkbox trick — no JavaScript) holding the equipment LED list.
All live behaviour arrives over one SSE connection driven by the vendored
[htmx-ext-sse] extension: the page declares `hx-ext="sse"
sse-connect="/stream/events"` once, and named `sse-swap` regions receive
server-rendered fragments — no hand-written JavaScript, exactly the pattern the
`test-sse` spike proved.

- **The feed** (`sse-swap="feed"` with `hx-swap="afterbegin"`): every rp event
  envelope renders as one card — severity dot (`*_failed` and
  `safety_changed:unsafe` are bad; `*_complete`/`*_settled` ok; `*_started`
  live; `stream_gap` warn), event title, payload-specific detail line (target
  coordinates, exposure duration, HFR, RMS error, error messages, …),
  monospace timestamp, and the operation duration when `elapsed_ms` is
  present. Unknown event types render a generic card (event name + compact
  payload) so new rp events degrade gracefully rather than vanish.
- **The status strip** (`sse-swap` slots): the current-operation label
  (updated on `*_started` / terminal events), the last guide RMS (updated on
  `guide_settled`/`dither_settled`), and the session-state chip (updated on
  `session_started`/`session_stopped`/`safety_changed`). Slots are updated
  from **each event's own payload alone** — the proxy is stateless, so a slot
  a given event doesn't describe simply keeps its previous content.
- **The fold panel**: the equipment LED list, fetched from `/stream/equipment`
  at render and re-fetched on an htmx timer (`hx-trigger="every 10s"`) — there
  are no device-connectivity events to push yet. The mock's guider graph and
  trend-chart cards need telemetry history rp does not expose; they are
  deferred (see [MVP scope](#mvp-scope)).
- **Initial state**: the page renders the strip from `GET /api/session/status`
  (`idle` / `active` / `interrupted`) and the LED panel from
  `GET /api/equipment`; the feed starts empty and fills from the SSE replay.
- **rp unreachable at page load**: the shell renders with an error banner in
  the hero; the SSE connection keeps retrying (below), so the page heals
  without a manual reload.

### The SSE proxy (`/stream/events`)

The browser never talks to rp (BFF pattern; rp also serves no CORS). The BFF
terminates the browser's `EventSource` and holds its own connection to rp's
`GET /api/events/subscribe`, translating JSON envelopes into HTML fragments:

- **Cursor passthrough.** The proxy forwards the browser's `Last-Event-ID`
  (set automatically by `EventSource` on reconnect) to rp as its
  `last-event-id`; a fresh page (no header) subscribes from cursor `0`, so
  rp's retained history (512 events) replays and populates the feed. The
  BFF keeps **no** stream state of its own — reconnect/replay correctness
  lives in rp, where it is already implemented and tested.
- **Frames.** Each rp envelope becomes up to two BFF frames: `event: feed`
  (the card) and the strip-slot frames it warrants. The **final** frame of
  each envelope group carries `id:` = the envelope's `event_seq`, so the
  browser's cursor only advances past an envelope it has fully received —
  a torn delivery replays that envelope (at-least-once; a duplicated feed
  card in that rare race is preferred over a silently missing one).
- **`stream_gap`** (rp signalling replay loss or a lagging consumer) renders
  as a feed divider card ("events were missed"), with no `id`, mirroring rp.
- **rp connection loss** (initial failure or mid-stream): the proxy pushes a
  status-slot fragment ("rp unreachable — retrying"), then **ends the BFF
  stream**. The browser's `EventSource` auto-reconnects with its cursor, so
  retry/backoff and replay come from the platform + rp rather than BFF state.
- **Keep-alive** every 15 s (axum `KeepAlive`), independent of rp's.
- **Shutdown.** Open SSE responses do not end on axum's graceful-shutdown
  signal (axum #2673 — the hazard the `test-sse` spike pinned), so the proxy
  select!s each stream against a service-wide cancellation token wired to the
  `ServiceRunner` shutdown — the same `sse_shutdown` pattern rp uses. The BFF
  therefore shuts down promptly (and flushes coverage in BDD) even with
  browsers connected.

## Configuration

The BFF has its own small config (it is not an ASCOM device). The `drivers` map
is keyed by service id (the `{service}` path segment); add an entry per driver.
The default config carries a single local `dsd-fp2` so `cargo run` works with no
config file. The optional `rp` target switches on the equipment page, the
activity stream, the `/config/rp` page, and the roster-derived config targets;
without it those routes render a "no rp configured" card.

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
  },
  "rp": {                                    // optional — enables /equipment, /stream, /config/rp
    "base_url": "http://127.0.0.1:11115",    // rp's REST base URL
    "auth": null,                            // optional Basic credentials for rp
    "ca_cert_path": null                     // optional PEM CA for a TLS-enabled rp
  }
}
```

### CLI Arguments

| Argument | Description |
|----------|-------------|
| `-c, --config`     | Path to the BFF configuration file. If omitted, the path resolves to the per-user platform config directory (`~/.config/rusty-photon/ui-htmx.json` on Linux) and is created with `Config::default()` on first start (binds `127.0.0.1:11120`, with a single `dsd-fp2` driver at `http://127.0.0.1:11119`). An explicit `--config` naming a missing file stays a hard error. |
| `--port`           | BFF listen port (overrides `server.port`). |
| `-l, --log-level`  | Log level: trace, debug, info, warn, error. |

## Security

- **The BFF holds driver credentials** (and rp's, for the `rp` target), in its
  own config, never in the page. It authenticates with HTTP Basic auth and
  trusts the Rusty Photon CA via `rp-tls` — the same client construction
  `sentinel` uses. Config actions are protected by whatever server-wide
  `rp-auth`/`rp-tls` the target runs; the BFF is just an authorised client (see
  the plan's Security section). Roster-derived config targets are called
  without credentials (rp redacts per-device auth) — an authed device needs its
  own static `drivers` entry.
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
- **The `rp` config page** over REST (`RestConfigClient`), with the
  restart-callout apply result.
- **The equipment page**: roster from rp's config joined with live
  `GET /api/equipment` state, capability tiers via probe, roster-derived
  config targets (`rp:{kind}:{id}`), and schema-generated add/edit/remove of
  roster entries via `PUT /api/config`.
- **The activity stream**: the feed + status strip + fold panel from the
  chosen mock, live over the SSE proxy with cursor passthrough, `stream_gap`
  rendering, rp-unreachable self-healing, and the shared-nav night-vision
  toggle.
- Dark theme reusing the mock CSS tokens; assets embedded via `include_str!`
  (CSS + the HTMX bundle + the SSE extension); no npm, no WASM.
- Plain-axum lifecycle under `rusty-photon-service-lifecycle::ServiceRunner` with
  graceful shutdown (SSE streams end on the shutdown token); prints
  `bound_addr=<host>:<port>` on bind (for BDD port discovery).

### Deferred

- **Roster connect/disconnect buttons** — rp has no runtime
  connect/disconnect endpoints yet (its registry is built once at startup;
  the endpoints are *(planned)* in `rp.md`). The LEDs show live state.
- **ASCOM UDP discovery pre-fill** for the roster (low-priority per the plan;
  manual entry is the primary path).
- **Telemetry charts** — the mock's guider graph and HFR/temp/sky/dew trend
  cards need telemetry history rp does not expose; the fold panel ships with
  the equipment LEDs, and the strip carries the last guide RMS from
  `guide_settled`/`dither_settled` events.
- **Image thumbnails in the feed** — `exposure_complete` links a document id;
  rendering pixels (`GET /api/images/{id}/pixels` ImageBytes → browser image)
  is a follow-up.
- **Composite-field rendering.** The schema walker skips `oneOf`/`anyOf`/`enum`
  subtrees (tagged enums like `star-adventurer`'s `transport`, optional nested
  structs — including a roster entry's optional `auth` block), so those fields
  round-trip read-only via the hidden blob rather than rendering an editable
  discriminated form. A generic `oneOf`/enum renderer (and a dedicated password
  input for redacted-secret leaves) is a follow-up; until then such fields are
  edited in the config file.
- **Sentinel `service.restart` affordance** and the `restart_required` escalation
  button (Phase 4 — the restart callout is where it will attach).
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
under Miri), and both binaries are built with the driver's mock transport (it is
feature-gated):

```
bazel test //services/ui-htmx:bdd
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
the browser before the BFF/driver stop. Run them under Bazel via the standalone
`--config=browser` (it sets `UI_BROWSER_TESTS=1` + `--spawn_strategy=local` and
forwards `FIREFOX_BINARY`/`GECKODRIVER_BINARY` by name, like `OMNISIM_PATH`):

```
FIREFOX_BINARY=/path/to/firefox GECKODRIVER_BINARY=/path/to/geckodriver \
  bazel test --config=browser //services/ui-htmx:bdd
```

This Bazel path is verified green **on Linux only** (plan §9 Tier 0 step 5
go/no-go): the browser layer runs on a single environment by design, so
macOS/Windows browser-under-Bazel is intentionally not pursued — the cross-OS
guarantee rides the P1/P2 server-bytes layers, which do run on every OS under
both build systems. The always-compiled `thirtyfour` dev-dep stays out of the
required gate: with `@browser` filtered out (env unset), the default BDD suite
is green on all three OSes under both Cargo and Bazel. (Under Bazel the run prints
a benign `cargo metadata failed … will use manifest directory as fallback` — the
insta golden-path resolver's expected fallback in the sandboxed build layout; all
snapshot steps still pass.)

An advisory **nightly** workflow ([`ui-browser-nightly.yml`]) runs this suite
against `main` on ubuntu (non-snap Firefox + geckodriver, `UI_BROWSER_TESTS=1`)
and opens-or-updates a tracking issue on failure. It is **not** a required gate —
browser flake never reddens a PR; the per-PR P1/P2 layers carry correctness.

[`ui-browser-nightly.yml`]: ../../.github/workflows/ui-browser-nightly.yml

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

**Test-only `/fixtures/*` routes (the `test-fixtures` feature).** A second
`@browser` feature ([`fixtures.feature`]) drives a `crate::fixtures` route set that
exists **only** when the `test-fixtures` cargo feature is on — it ships nothing in
the real binary, and the module is `#[coverage(off)]` so it never enters the
coverage numbers. These fixtures exercise htmx behaviors the server-bytes layers
(P1/P2) cannot observe: an `hx-swap-oob` swap updating a *second* region (plus the
negative — htmx silently drops an OOB element whose target is absent), an
`HX-Retarget` header moving a **byte-identical** body to a different target (the
body is a plain fragment; the divergence lives entirely in the response header — a
§A tripwire asserts the header, the browser asserts the landing), and an
`HX-Push-Url` header changing the browser location. The BDD suite spawns a binary
built with the feature: cargo `--all-features` provides it; under Bazel the
`:ui-htmx_fixtures` binary (the dsd-fp2 `_mock` pattern) does, so
`bazel test --config=browser` stays green.

[`fixtures.feature`]: ../../services/ui-htmx/tests/features/fixtures.feature

**Test-only `/fixtures/sse*` routes (the `test-sse` feature).** A third `@browser`
feature ([`sse.feature`]) drives a `crate::sse_fixtures` route set gated on the
separate `test-sse` cargo feature (off by default, `#[coverage(off)]`, ships
nothing). It is the streaming spike for the future live-telemetry UI: a fixture
page wires the vendored htmx SSE extension (`htmx-ext-sse@2.2.3`, vendored
byte-for-byte from upstream; the htmx project is Zero-Clause-BSD — htmx 2.0 split
SSE out of core, so the embedded `htmx.min.js` carries none, and the extension is
`include_str!`'d only under this feature) to **one** `sse-connect` EventSource
feeding **two** `sse-swap` regions, and an axum `Sse` endpoint pushes two named
events on a timer then holds the connection open. Two scenarios prove what only a
browser can: that both regions update from the single connection (async
server-pushed DOM updates, which have no server "bytes" for P1/P2 to assert), and
that an open SSE stream — which never closes on the shutdown signal (axum #2673) —
still allows a graceful, coverage-flushing BFF shutdown **when the browser is quit
first**. The latter is the §5.4 coverage hazard with no in-process escape hatch (the
connection is held by the out-of-process browser), so `driver.quit()` must precede
`ServiceHandle::stop()`; the teardown order in `tests/bdd.rs` enforces it. The BDD
binary carries both `test-fixtures` and `test-sse` (cargo `--all-features`; Bazel
`:ui-htmx_fixtures`), the latter pulling the optional `async-stream` dependency.

[`sse.feature`]: ../../services/ui-htmx/tests/features/sse.feature

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

### Phase 5 BDD (`equipment_page.feature`, `rp_config_page.feature`, `stream_page.feature`)

The rp-backed surfaces follow the same real-binaries rule: scenarios spawn the
real `ui-htmx`, a real **`rp`** (via `bdd_infra::rp_harness` —
`RpConfigBuilder` + `start_rp`), and where the roster needs a live device, a
real mock-mode `dsd-fp2` registered in rp's config as a `cover_calibrator`
entry — an all-first-party stack with **no OmniSim dependency**, so these
suites run everywhere the existing one does. Coverage:

- `rp_config_page.feature`: `/config/rp` renders rp's config over REST
  (secrets redacted, `server.port` read-only); an edit persists to rp's config
  file and renders the restart callout listing the changed paths; an invalid
  edit renders the driver-side field error with the file untouched; rp down →
  error banner.
- `equipment_page.feature`: the roster lists rp's devices with live
  connected LEDs (the dsd-fp2-backed entry probes as **Managed** and links to
  `/config/rp:cover_calibrators:{id}`, which renders that device's real
  schema-driven form end to end); add / edit / remove splice rp's config and
  render the restart callout; an entry pointing nowhere shows **Unreachable**;
  no rp configured → the "no rp configured" card.
- `stream_page.feature`: drives `/stream/events` directly over HTTP (SSE is
  server bytes — no browser needed for P1): a session start/stop against rp
  produces `session_started`/`session_stopped` envelopes that arrive as
  rendered feed-card frames with `id:` = the envelope seq; reconnecting with
  `Last-Event-ID` replays only the missed tail; rp down → the
  "rp unreachable" status frame and stream end. The BDD client drops its SSE
  connection before stopping the BFF (testing.md §5.4). Browser-level SSE
  swap behaviour stays proven by the existing `@browser` `sse.feature` spike.

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
- `config.rs`: defaults (single dsd-fp2), the multi-driver map, the optional
  `rp` target, and JSON load.
- `io.rs`: `ReqwestHttpClient` connection-refused error path (mirrors sentinel).
- `driver_client.rs` (`RestConfigClient`): REST request shaping, 200-body
  parsing, 400/500 mapping — mocked `HttpClient`.
- `pages/stream.rs`: table-driven `EventEnvelope → Markup` renderers — one case
  per event family from the rp catalog (started/terminal/failed payload
  fields, severity classes, elapsed formatting, unknown-event fallback,
  `stream_gap` divider).
- `sse_proxy.rs`: the incremental SSE frame parser (frames split across
  chunks, CRLF, multi-line `data:`, comment lines, missing `id:`), and the
  proxy against an in-test axum stub serving a canned rp stream (ADR-004's
  escape hatch — streaming is beyond mockall): cursor forwarding, frame `id`
  placement, gap + disconnect translation.
- `probe.rs`: tier classification per probe outcome (mocked responses),
  timeout → Unreachable, 401 → Auth required.
- `pages/equipment.rs`: roster join (config ⨝ status by id, mount pairing),
  config surgery (insert/replace/remove per kind incl. the singular mount),
  and the per-kind subschema field generation.

## Module Structure

| Module | Description |
|--------|-------------|
| `config.rs` | `Config`, `ServerConfig`, the `Drivers` map + `DriverTarget`, the optional `RpTarget`, defaults + JSON load. |
| `io.rs` | `HttpClient` trait (`#[cfg_attr(test, mockall::automock)]`) + `ReqwestHttpClient` (rp-tls CA trust + optional Basic auth). |
| `driver_client.rs` | `ConfigClient` trait + `AlpacaConfigClient` (ASCOM action transport) + `RestConfigClient` (rp's plain-REST transport): request shaping, envelope parsing, error mapping. Re-exports the shared wire types from `rusty_photon_config::actions`. |
| `rp_client.rs` | The non-config rp surface: `RpApi` trait (`equipment_status`, `session_status`) + its reqwest impl — the seam the equipment page and stream shell render from. |
| `roster.rs` | The roster domain: `EquipKind` (kind ⇄ ASCOM-type mapping), `parse_roster` over rp's config value, the `rp:{kind}:{id}` key codec, and the insert/replace/remove config surgery with duplicate-id/singular-mount guards. |
| `pages/mod.rs` | The schema-driven renderer: `FieldModel` (schema walker + `FieldKind`, incl. the array-item subschema entry point), `config_card`/`index`/fragment templates, the schema-driven `merge_form` coercion, and the shared `layout` shell (nav tabs + night-vision toggle). |
| `pages/equipment.rs` | The equipment page: roster join, tier badges, add/edit/remove forms, roster mutation via config surgery. |
| `pages/stream.rs` | The activity stream page shell + per-event feed-card and strip-slot fragment renderers (pure `EventEnvelope → Markup` functions). |
| `probe.rs` | The capability probe: bounded concurrent `supportedactions`/setup-page checks → tier. |
| `sse_proxy.rs` | `/stream/events`: rp SSE client (incremental frame parser), envelope→fragment translation, cursor passthrough, shutdown token. |
| `assets.rs` | `include_str!` of `assets/app.css` + `assets/htmx.min.js` + `assets/htmx-ext-sse.js`; asset routes. |
| `lib.rs` | `build_router`, multi-driver `AppState` (+ rp target), the `/config/{service}`, `/equipment*`, `/stream*` handlers, public exports. |
| `main.rs` | CLI (clap) + tracing init; lifecycle owned by `ServiceRunner` (plain axum + graceful shutdown; SSE shutdown token). |

## References

- Design plan: [`docs/plans/ui-design/config-actions.md`](../plans/ui-design/config-actions.md)
- Chosen UI direction + stack: [`docs/plans/ui-design/mocks/README.md`](../plans/ui-design/mocks/README.md)
- Driver config-action protocol (Phase 1): [`dsd-fp2.md`](dsd-fp2.md) "Config Actions"
- HTTP-client / mockall pattern: [`sentinel.md`](sentinel.md)
- Lifecycle: [`docs/skills/service-lifecycle.md`](../skills/service-lifecycle.md) "Plain axum service"
