# Config actions — the cross-driver configuration protocol

Every rusty-photon Alpaca driver exposes its own configuration over HTTP as three
**vendor ASCOM `Action`s** — `config.get`, `config.apply`, and `config.schema` —
so a single web UI can read, edit, and apply any driver's config without the
driver-specific knowledge living in the UI. This is the generalisation of the
Phase 1/2 `dsd-fp2` protocol (see
[`docs/plans/ui-design/config-actions.md`](../plans/ui-design/config-actions.md))
to **all** drivers.

The driver-agnostic machinery lives in the **`rusty-photon-config`** crate's
[`actions`](../../crates/rusty-photon-config/src/actions.rs) module; each driver
supplies only its specifics through one trait.

## The `ConfigurableDriver` trait

A driver implements this trait (in its `config_actions.rs`) for a zero-sized
marker type; everything else is generic free functions over it:

```rust
pub trait ConfigurableDriver {
    type Config: Serialize + DeserializeOwned + JsonSchema;   // the driver's config
    type Overrides;                                           // CLI-override carrier (`()` if none)

    fn normalize(config: &mut Self::Config);                  // trim/canonicalize before validate
    fn validate(config: &Self::Config) -> Vec<FieldError>;   // domain validation (empty = valid)
    fn secret_pointers() -> &'static [&'static str];         // RFC-6901 secret leaves to redact
    fn override_paths(overrides: &Self::Overrides) -> Vec<String>;       // dotted, CLI-pinned
    fn apply_overrides(config: &mut Self::Config, overrides: &Self::Overrides);
    fn locked_paths() -> &'static [&'static str] { &[] }     // identity fields (unlock-to-edit)
    fn read_only_paths() -> &'static [&'static str] { &[] }  // hard read-only (self-lockout)
}
```

The generic functions — `config_get::<D>`, `config_apply::<D>`, `config_schema::<D>`
— implement the invariant protocol: secret redaction, layer-aware persist,
effective-config diff, and schemars JSON-Schema generation. They return plain
values / `ApplyError`, so **`rusty-photon-config` carries no `ascom-alpaca`
dependency** — it is the transport-/consumer-agnostic config *model*, shared with
the plain-REST `rp` / `sentinel` services.

The ASCOM **adapter** — wrapping those results into `ASCOMResult`, the generic
`config.get` / `config.apply` / `config.schema` action dispatch, the
`ConfigActionCtx`, and the shared transport-driver error model — lives in the
separate [`rusty-photon-driver`](../../crates/rusty-photon-driver) crate (which
*does* depend on `ascom-alpaca`, used only by the six driver services). Each
driver delegates `Device::action` / `Device::supported_actions` to
`rusty_photon_driver::dispatch::<D>` / `rusty_photon_driver::supported_actions`,
and defines its error type with the `rusty_photon_driver::driver_error!` macro —
so the dispatch, the `ApplyError → ASCOMError` mapping, and the common
`DriverError` variants each exist in exactly one place. See
[ADR-007](../decisions/007-rusty-photon-driver-shared-crate.md).

## The three actions

```
GET  /api/v1/{type}/{n}/supportedactions  → [..., "config.get", "config.apply", "config.schema"]

PUT  /api/v1/{type}/{n}/action   Action=config.get
   → Value = "<{ config: <effective, secrets redacted>, overrides: [dotted CLI-pinned] }>"

PUT  /api/v1/{type}/{n}/action   Action=config.schema
   → Value = "<{ schema: <JSON Schema>, locked_fields: [dotted], read_only_fields: [dotted] }>"

PUT  /api/v1/{type}/{n}/action   Action=config.apply   Parameters=<full Config JSON>
   → Value = "<{ status: applying|ok|invalid, applied[], reload[], restart_required[],
                 skipped_override[], persisted_to, errors[] }>"
```

Each `Value` is a JSON **string** inside the standard Alpaca envelope (the BFF
unwraps the envelope, then parses the string). The wire types are defined once in
`rusty_photon_config::actions` and reused by the BFF.

### `config.apply` sequence

1. Parse `Parameters` into the driver's typed `Config` (parse failure →
   ASCOM `INVALID_VALUE` — a *transport* error, distinct from a domain error).
2. `normalize`, then `validate`; on failure or a redacted-secret-without-prior,
   return **HTTP 200** `{ status:"invalid", errors:[…] }`, file untouched.
3. **Layer-aware persist** (atomic temp→fsync→rename→fsync-dir): write every
   field *except* CLI-override-pinned ones (those carry through from the file's
   prior value, listed in `skipped_override[]`), and carry forward a redacted
   secret (the `********` sentinel means "keep the stored secret").
4. Diff the new effective config against the running one; the changed paths go in
   `reload[]`. Status is `applying` if anything changed (the driver fires the
   in-process reload **after** the response flushes), else `ok`.

### In-process reload

Drivers run under `ServiceRunner::with_reload().run_with_reload(...)` (see
[`docs/skills/service-lifecycle.md`](../skills/service-lifecycle.md)). A
`config.apply` that needs a reload fires `ReloadSignal::notify()` after a short
delay (so the HTTP response flushes first); the run loop breaks its
shutdown-or-reload stop future, the old server drains HTTP and releases its
transport, and the loop rebuilds from the freshly-persisted file — rebinding the
same port. The BFF treats `status:"applying"` as "expect a brief blip; reconnect
and re-`config.get`".

## Editability tiers

JSON Schema cannot express identity/read-only intent, so `config.schema` returns
the tiers alongside the schema. The web UI evaluates them in precedence order:

| Tier | Source | UI |
|------|--------|----|
| Override-pinned | `config.get`'s `overrides[]` (CLI flags) | disabled; never persisted |
| Hard read-only | `read_only_paths()` | disabled; never editable (e.g. `server.port`, a device `enabled` flag) |
| Locked / identity | `locked_paths()` | disabled behind an "unlock to edit" escape hatch (e.g. a device `unique_id`) |
| Editable | everything else | enabled |

**Self-lockout guards:** a `server.port` change would make the driver rebind a
port the BFF can't follow; disabling a device tears down the very endpoint the
config actions live on; a `unique_id` is driver-owned (minted by
`rusty_photon_config::materialize_identity`). These are read-only / locked so the
UI can't edit away its own reachability.

## Driver coverage

| Driver | Devices | Secrets | Notes |
|--------|---------|---------|-------|
| `dsd-fp2` | CoverCalibrator | auth password hash | the reference implementation |
| `qhy-focuser` | Focuser | auth password hash | single device |
| `pa-falcon-rotator` | Rotator + Switch | auth password hash | two devices share one config + reload |
| `ppba-driver` | Switch + ObservingConditions | auth password hash | two devices; `--enable-*` flags pin the enabled fields |
| `sky-survey-camera` | Camera | follow-mode client passwords | `Overrides = ()`; cross-field validation |
| `star-adventurer-gti` | Telescope | auth password hash | config actions alongside the `ApPark` actions; `transport` block read-only |

## The web UI

The `ui-htmx` BFF ([`ui-htmx.md`](ui-htmx.md)) is the browser-facing consumer: it
calls `config.get` / `config.schema` to render a form and `config.apply` to save,
reusing the `rusty_photon_config::actions` wire types. See `ui-htmx.md` for the
rendering and multi-driver routing.

## References

- Protocol design + phasing: [`docs/plans/ui-design/config-actions.md`](../plans/ui-design/config-actions.md)
- Protocol model (no ASCOM dep): [`crates/rusty-photon-config/src/actions.rs`](../../crates/rusty-photon-config/src/actions.rs)
- ASCOM adapter (dispatch + `driver_error!` macro + error model): [`crates/rusty-photon-driver`](../../crates/rusty-photon-driver) — see [ADR-007](../decisions/007-rusty-photon-driver-shared-crate.md)
- Reload lifecycle: [`docs/skills/service-lifecycle.md`](../skills/service-lifecycle.md)
- Per-driver specifics: each `docs/services/<driver>.md` "Config actions" section.
