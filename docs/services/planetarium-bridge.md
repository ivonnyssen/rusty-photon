# planetarium-bridge

**Status: P3 design (development-workflow Phase 1).** This document is
the design for the real `planetarium-bridge` service — P3 of
[planetarium-target-import.md](../plans/planetarium-target-import.md).
The P3a verification-spike findings that ground it are preserved in the
[appendix](#appendix-p3a-verification-spike-findings-2026-07-29). The
throwaway spike crate `spikes/planetarium-bridge-p3a` remains in the
tree until the implementation PR lands (it is still needed for the
[P3b horizon experiment](#open-item-p3b-horizon-experiment)), and is
deleted there.

## Overview

`planetarium-bridge` serves a **virtual ASCOM Alpaca Telescope** that
planetarium apps (SkySafari 7+, Stellarium, Cartes du Ciel) connect to
as if it were a mount. Pressing **GoTo** in the planetarium does not
move anything — it **imports the exact framed coordinates as a paused
target** into rp's target store, named by reverse catalog lookup, for
the operator to review and activate later. The service never touches
hardware and is never on the imaging path; rp's planner images the
target whenever conditions are right, fully decoupled from the
planetarium (workspace tenet 3 is satisfied trivially: there is nothing
to actuate).

## Architecture

```
 SkySafari / Stellarium / CdC                    planetarium-bridge (port 11126)
┌───────────────────────────┐    Alpaca HTTP    ┌──────────────────────────────────────┐
│  "scope" preset →         │ ────────────────► │ ascom-alpaca server: one Telescope    │
│  connect, 1 Hz poll,      │                   │  ├─ virtual pointing state machine    │
│  GoTo / (Sync rejected)   │ ◄──────────────── │  │   (simulated slews, altitude floor)│
└───────────────────────────┘   position reports│  └─ site/LST/alt-az (rp-ephemeris)    │
                                                │                                      │
                                                │ import pipeline                      │
                                                │  GoTo coords ─ epoch ─► ICRS         │
                                                │        │                             │
                                                │        ▼            rp down?        │
                                                │  rp-mcp-client ◄──── on-disk spool   │
                                                │  (ADR-017 auth/TLS)  (bounded FIFO)  │
                                                └───────────┬──────────────────────────┘
                                                            │ MCP add_target{coords, source}
                                                            ▼
                                        rp (port 11115): cone-search naming, dedup,
                                        slug allocation, pending target in the store
```

Component boundaries:

| Component | Home | Role |
|---|---|---|
| Virtual Telescope device | `services/planetarium-bridge` | Full ASCOM contract, simulated motion, reported-position policy |
| Import pipeline + spool | `services/planetarium-bridge` | Epoch conversion, `add_target` submission, offline spooling, `/health` |
| MCP client | [`rp-mcp-client`](../decisions/017-standard-mcp-client-construction.md) | Authed, CA-pinned transport — no bridge-local HTTP code |
| Ephemeris (LST, alt-az) | [`rp-ephemeris`](../crates/rp-ephemeris.md) | Site math for reports and the altitude floor |
| Epoch conversion | `erfars` | Apparent-of-date → ICRS when `assume_epoch = "jnow"` |
| Reverse cone-search + naming + dedup | **rp** (with `rp-catalog`, `rp-targets`) | Everything that needs the store — see [rp-side contract](#rp-side-contract) |

The bridge is deliberately thin: it owns the wire persona and the
delivery guarantee. All naming, dedup, and slug policy live in rp,
where the store is — the bridge sends bare ICRS coordinates plus
provenance and nothing else.

Standard service scaffolding applies
([service-lifecycle.md](../skills/service-lifecycle.md)): `ServiceRunner`
with SCM feature, `init_service_tracing`, `resolve_and_init` config
bootstrap minting the Alpaca `UniqueID`, `pkg/doctor.toml`
(`class = "alpaca"`, port 11126), workspace/Bazel registration, and the
hand-typed port-table updates (workspace.md, packaging docs, doctor.md)
in the implementation PR.

## The virtual Telescope device

One Alpaca `Telescope` (device number 0) via the `ascom-alpaca` server
feature.

### Identity and capabilities

The device name and description state loudly that this is a **virtual
target-entry device, not a mount** — e.g. name
`"Planetarium Bridge (virtual target entry — NOT a mount)"`. The
`UniqueID` is minted at first start by `resolve_and_init`.

| Member | Value |
|---|---|
| `AlignmentMode` | `GermanPolar` (spike-proven with SkySafari) |
| `EquatorialSystem` | `J2000` |
| `CanSlew` / `CanSlewAsync` | `true` |
| `CanSync` | **`false`** — see [Sync is rejected](#sync-is-rejected) |
| `CanSyncAltAz`, `CanSlewAltAz`, `CanSlewAltAzAsync` | `false` |
| `CanPark`, `CanUnpark`, `CanFindHome`, `CanSetPark` | `false` (`AtPark` reads `false`) |
| `CanMoveAxis`, `CanPulseGuide`, `CanSetGuideRates` | `false` |
| `CanSetTracking`, `CanSetDeclinationRate`, `CanSetRightAscensionRate` | `false` |
| `Tracking` | reads `true` (constant) |
| `SideOfPier` | `NOT_IMPLEMENTED` (legal for ITelescopeV3; nothing polls it — P3a) |
| `UTCDate` read | host clock UTC |
| `SetUTCDate` | `NOT_IMPLEMENTED` (P3a: SkySafari retries once, carries on) |
| `AxisRates` | empty set |

### Epoch handling

The device declares `EquatorialSystem = J2000` and P3a confirmed
SkySafari honors it (arcsecond-exact J2000 on the wire, read once at
connect). `assume_epoch` config covers clients that ignore the
declaration:

- `"j2000"` (default) — received coordinates are ICRS/J2000; used as-is.
- `"jnow"` — received coordinates are apparent-of-date; converted to
  ICRS via ERFA (`Atic13`, with TT derived from host UTC) before import.

The conversion happens once, at receipt; everything downstream
(spool, rp, the store) is ICRS only.

### Slew lifecycle — the add-target gesture

All three ASCOM slew forms are the gesture, treated identically:
`SlewToCoordinatesAsync` (the only one SkySafari sends),
`SlewToCoordinates` (blocking — completes after the convergence
window), and `SlewToTarget`/`SlewToTargetAsync` (via the
`TargetRightAscension`/`TargetDeclination` setters, which validate and
store per the ASCOM contract; slew verbs propagate the requested
coordinates into `Target*` as ConformU expects).

On any slew verb:

1. Validate ranges (`ra ∈ [0,24)`, `dec ∈ [-90,90]`) — out-of-range →
   ASCOM `InvalidValue`.
2. **Fire the import** (§ [import pipeline](#the-import-pipeline)) —
   the GoTo tap is the operator's intent, so the import happens at
   receipt, not at convergence. `AbortSlew` ends the simulated motion
   but never cancels the import.
3. Start the simulated slew: `Slewing` reads `true` for the
   convergence window (`slew_duration`, default `3s` — the cadence P3a
   proved SkySafari accepts), with reported position interpolating
   from the current pointing to the target (shortest-path RA wrap).
   A new slew verb during convergence supersedes: the new slew starts
   from the current interpolated position, and fires its own import
   (rp-side dedup collapses repeats).

A **below-horizon target is accepted and imported** — couch-planning
an object that has not risen is a core use case, and rp's altitude
gating decides when it is actually imaged. Only the *reported*
position is constrained (next section).

### Reported-position policy (the altitude floor)

P3a found that SkySafari **refuses every GoTo while the scope's
reported position is below the horizon** — a below-horizon report
wedges the whole session. This can happen with no operator error at
all: a tracked position imported at dusk sets hours later. The bridge
therefore maintains two notions of position:

- **Virtual pointing** — where the last slew converged. Follows the
  slew interpolation, then holds (RA/Dec constant, i.e. tracking).
- **Reported position** — what `RightAscension`/`Declination` return:
  the virtual pointing *while its computed altitude ≥
  `report_altitude_floor_deg`* (default `10.0`), otherwise the **idle
  point**: RA = current LST, Dec = site latitude (the zenith).

The floor is evaluated at read time from the live site. The idle point
self-heals: parked at the zenith, the (tracking-constant) RA drifts
west of the meridian over the hours; when its altitude eventually
reaches the floor, the reported position re-snaps to the then-current
zenith. The operator-visible effect: the scope marker sits on the last
imported target while that target is meaningfully up, and drifts to
"parked overhead" when it sets — and the client can always GoTo.

`report_altitude_floor_deg: null` disables the policy (reports raw
virtual pointing) — the knob the
[P3b experiment](#open-item-p3b-horizon-experiment) may justify
loosening or per-client documentation may want.

### Sync is rejected

`CanSync = false`; `SyncToCoordinates`/`SyncToTarget` return
`NOT_IMPLEMENTED`. In every planetarium, Sync/Align means "the scope
IS pointing here" — a pointing-model correction, never target intent —
and a virtual device has no pointing model to correct. Rejecting (vs
the P3a spike's accept-and-reflect) keeps the device honest, removes
the sync-induced below-horizon wedge vector entirely, and surfaces a
clear client-side error instead of silently absorbing a meaningless
gesture. P3a showed operators do tap Align casually; the error dialog
is the teaching moment that the button doesn't apply to a virtual
device. *(This supersedes plan Decision 2's "accepted, logged, and
ignored" wording — settled interactively 2026-07-29.)*

### Site and time writes

P3a: SkySafari pushes its own GPS-derived site
(`PUT SiteLatitude`/`SiteLongitude`) and `PUT UTCDate` after connect.

- **Site writes are adopted live**: the pushed values replace the
  configured site for LST/alt-az/floor math (and are reflected on
  reads). The client's site is typically *more* accurate than a static
  config, and using it keeps the bridge's horizon math consistent with
  the client's own — exactly what the floor policy wants. Adoption is
  logged at `info!`; the configured site is the startup default, and a
  restart reverts to it.
- **`SetUTCDate` stays `NOT_IMPLEMENTED`** — the host clock is
  authoritative for a virtual device, and P3a proved the rejection is
  tolerated (one retry, no fallout).

### Discovery

Alpaca UDP discovery follows the fleet convention (plan Decision 1):
**opt-in, off by default** (`server.discovery_port` absent), because
many rusty-photon Alpaca servers on one host would collide on the
shared port — the `ports.discovery-collision` doctor check guards
this. The documented connection story is manual IP:port entry, which
P3a confirmed is also the only story that works across routed subnets
(discovery is broadcast-scoped).

### ConformU

The device must pass ConformU via the existing harness pattern
(`bazel test --config=conformu`). The capability matrix above is
deliberately minimal-but-coherent: every `false` capability's verbs
return `NOT_IMPLEMENTED`, every `true` capability behaves per the
ASCOM contract (Target* propagation, `Slewing` state, `AbortSlew`
always callable as a stop-class verb).

## The import pipeline

### GoTo → `add_target`

Each accepted slew verb produces one import request:

```jsonc
// MCP tool call to rp
add_target {
  "ra_hours": 20.9877,          // ICRS, post-assume_epoch conversion
  "dec_degrees": 44.5253,
  "source": {
    "kind": "planetarium-bridge",
    "client": "<ip:port of the planetarium>",
    "received_at": "2026-07-29T05:41:12.481Z"
  }
}
```

The bridge sends **no name** — naming, dedup, slug allocation, and
default goals are rp policy (§ [rp-side contract](#rp-side-contract)).
On success the bridge logs the outcome at `info!` (the one log line an
operator derives clear value from):
`imported as ngc7000-2 (created)` / `updated pending target ngc7000-2`.

Tool failures (rp rejected the call — e.g. a validation error) are
logged at `error!` and **not** spooled: a request rp actively rejected
will be rejected again on replay. Only *delivery* failures spool.

### Spooling — rp unreachable

rp being down must never lose a GoTo. Delivery failures (transport
loss, TLS failure, timeouts) append the import request to a **bounded
on-disk FIFO spool**:

- One JSONL file (`spool.path`, default
  `<platform config root>/planetarium-bridge/spool.jsonl` beside the
  service's other state), one request per line, `fsync`ed per append —
  a Ctrl-C or crash loses nothing, and the spool **replays across
  bridge restarts**.
- Replay runs in order (FIFO) whenever rp is reachable again, paced by
  exponential backoff between reconnect attempts (1 s doubling to
  `spool.replay_backoff_max`, default `5m`). Replayed entries carry
  their original `received_at`, so provenance reflects the GoTo, not
  the replay.
- **Bounded**: at `spool.max_entries` (default `1000`, comfortably
  above any human session), the oldest entry is dropped to admit the
  newest — with an `error!` log *per drop* and the `dropped_total`
  counter incremented. "Never drop silently" means observable, not
  infallible.

Replay is idempotent by construction: a replayed request hits the same
rp-side dedup as a live one, so the worst case of a
crash-between-send-and-remove is an in-place upsert of the same
pending target.

### `/health`

The bridge serves `GET /health` alongside the Alpaca routes (same
listener), returning:

```json
{ "rp_reachable": true, "spooled": 0, "replayed_total": 3, "dropped_total": 0 }
```

`spooled` is the durable backlog length; the totals are
process-lifetime counters. This is the plan's "sentinel-visible
counter" hook: curl-able immediately; teaching sentinel's supervisor to
scrape it is a documented follow-up, not MVP (sentinel today probes
alpaca-class services at the Alpaca `connected` endpoint only).

## rp-side contract

Everything in this section lands in `rp` / `rp-targets` / `rp-catalog`
(not the bridge) during P3 implementation, and is absorbed into
`rp.md` § Target Store, `rp-targets.md`, and the `rp-catalog` docs in
the matching Rule-2 update. It activates the `source` parameter
`rp.md` already reserves on `add_target`.

### Writer identity: `created_by` / `updated_by`

`Target` gains two writer-identity fields beside the existing
timestamps *(settled interactively 2026-07-29, refining plan
Decision 3's notes-only provenance)*:

```rust
pub created_by: String,   // "operator" | "planetarium-bridge" | future writers
pub updated_by: String,   // stamped on every write, same domain
```

- `add_target` **with** `source` stamps both with `source.kind`.
- Every operator-surface write (`add_target` without `source`,
  `update_target`, `set_goals`) stamps `"operator"`.
- Existing rows migrate to `created_by = updated_by = "operator"`
  (serde defaults; no redb schema step needed).

"Unedited since import" is now a first-class predicate:
`updated_by == "planetarium-bridge"`. The P4 inbox gets "who touched
this last, and when" for free. Rich human-readable provenance (client
address, receipt time) additionally goes into `notes` as a text line
per Decision 3 — display data, never parsed.

### `add_target` import semantics (`source` present)

`source` selects a third parameter form: bare
`ra_hours` + `dec_degrees` + `source` (no `catalog_ref`, no
`display_name` — supplying either alongside `source` is an error;
naming is rp's job here). Semantics that differ from an operator add:

1. **`active: false` always** — imports land paused in the inbox
   (Decision 3); the parameter is not accepted with `source`.
2. **Proximity-only dedup** replaces the same-object slug rule: rp
   searches all stored targets for a row within
   `target_store.import.dedup_arcsec` (default `30`) of the received
   coordinates.
   - Match found, **and** it is still pending and bridge-owned
     (`!active && updated_by == source.kind`): **in-place upsert** —
     coordinates take the new value, `updated_at`/`updated_by`
     stamped, the provenance line in `notes` refreshed; slug,
     `display_name`, goals untouched. Returns `created: false`.
   - Match found, but active, operator-edited, or operator-created:
     the row is **never modified** — a new pending target is created
     with a suffixed slug. This is the Decision 3 protection, enforced
     in rp (not bridge courtesy).
   - No match: create.
3. **Goals default** from `target_store.default_goals` (Decision 10),
   as for any add without `goals[]`.
4. The `catalog_ref`-match branch of slug allocation is **never**
   consulted for imports: two GoTos 15′ apart that both resolve to
   "NGC 7000" are two targets (mosaic panels), not one.

### Naming — reverse cone-search at add-time

For a `source` create *(settled interactively 2026-07-29: rp
finalizes naming atomically against the store; the bridge sends bare
coordinates — this refines Decision 4's "the bridge resolves"
wording)*:

1. **Cone-search**: nearest catalog object within
   `target_store.import.naming_tolerance_arcmin` (default `10`);
   nearest angular separation wins ties. A hit sets `catalog_ref` and
   denormalizes `object_type`/`magnitude`/`size_arcmin` exactly as a
   catalog add does.
2. **Display name**:
   - Hit, **and** this is the only stored target with that
     `catalog_ref`, **and** the offset from the catalog centroid is
     within `dedup_arcsec`: the plain name — `"NGC 7000"`.
   - Hit otherwise: the offset form — `"NGC 7000 +8′E −4′N"`, where
     East = Δα·cos δ and North = Δδ, each component rendered to 0.1′
     with a trailing `.0` stripped (`+8′E`, `+0.3′E`) and a
     component under 0.05′ omitted. The offset reads as *how this
     framing differs* — what the operator composed.
   - No hit: the coordinate form `"J2059+4432"` (IAU-style truncation:
     `Jhhmm±ddmm`).
   - Names are initial values only: `display_name` stays freely
     operator-editable, and existing rows are **never retroactively
     renamed** when a second framing of the same object arrives.
3. **Slug**: a hit bases the slug on the `catalog_ref` (`ngc7000`,
   suffix-allocated on collision per the landed rules — `ngc7000-2`);
   no hit uses the coordinate slug (`j2059p4432`, `p`/`m` for the
   sign). The coordinate display name matches its slug shape by
   construction.

Catalog coverage bounds *naming quality only*, never import
correctness: identity, dedup, and slug allocation are pure
coordinate proximity, so a target the catalog has never heard of
(a Sharpless nebula, a dark-nebula framing, empty sky) imports
exactly as well as M31 — it just arrives with the coordinate name
and slug, ready for an operator rename during the `active: false`
review. `rp-catalog` currently embeds Messier + NGC + IC (from
OpenNGC); widening coverage (Sh2, Barnard, LDN/LBN, vdB, RCW,
Abell PNe, …) is a pure data-layer change — another CSV in
`crates/rp-catalog/src/data/` — that improves initial names with
no bridge or rp code change. Existing rows are never retroactively
renamed when coverage grows.

### `rp-catalog`: nearest-neighbor query

New API (explicit P3 scope per Decision 4):

```rust
pub struct NearestMatch<'a> {
    pub target: &'a ResolvedTarget,
    pub separation_arcmin: f64,
    pub east_offset_arcmin: f64,   // Δα·cosδ of the query FROM the centroid
    pub north_offset_arcmin: f64,  // Δδ of the query FROM the centroid
}

impl Catalog {
    pub fn nearest(&self, coord: &IcrsCoord, tolerance_arcmin: f64)
        -> Option<NearestMatch<'_>>;
}
```

A linear scan over the embedded ~13k rows (microseconds at this size)
— deliberately *not* the DB-seeded indexed cone-search browse that
`rp-targets.md` defers; the two must not be conflated.

### rp config additions

```jsonc
"target_store": {
  "import": {
    "dedup_arcsec": 30.0,            // proximity-upsert window; below any mosaic panel spacing
    "naming_tolerance_arcmin": 10.0  // cone-search radius; display only, never identity
  }
}
```

## Configuration (bridge)

Follows the fleet conventions: durations are humantime strings, angles
bare decimal degrees, `AlpacaServerConfig` for the server block,
sentinel's `service_auth`/`ca_cert` field shape for the client wiring
(ADR-017), `resolve_and_init` minting `server.unique_id` on first
start.

```jsonc
{
  "server": {                        // rusty-photon-server-config AlpacaServerConfig
    "port": 11126,
    "bind_address": "0.0.0.0",
    // "discovery_port": 32227,      // opt-in; absent = no discovery responder
    "tls": null,
    "auth": null
  },
  "site": {                          // startup default; a client site push overrides live
    "site_latitude_deg": 33.0,       // WGS84, +N
    "site_longitude_deg": -117.0,    // WGS84, +E (ASCOM convention)
    "site_elevation_m": 0.0
  },
  "rp": {
    "mcp_server_url": "https://rp.example.com:11115/mcp",
    "service_auth": { "username": "observatory", "password": "<plaintext>" },
    "ca_cert": "/etc/rusty-photon/pki/ca.crt"
  },
  "device": {
    "slew_duration": "3s",                  // simulated convergence window
    "assume_epoch": "j2000",                // or "jnow" for clients ignoring the declaration
    "report_altitude_floor_deg": 10.0      // null disables the reported-position floor
  },
  "spool": {
    "path": null,                           // null = platform default location
    "max_entries": 1000,
    "replay_backoff_max": "5m"
  }
}
```

Config invariants follow parse-don't-validate
([development-workflow.md](../skills/development-workflow.md#parse-dont-validate-for-config)):
latitude/floor ranges, positive `max_entries`, a well-formed
`mcp_server_url` — all rejected at load with the field named.

## Doctor integration

- `pkg/doctor.toml`: `class = "alpaca"`, `port = 11126`. Sentinel's
  health supervision and doctor's port checks apply as to any Alpaca
  service.
- **Client wiring**: doctor's `plan_client_wiring` provisions
  `rp.service_auth` + `rp.ca_cert` (absent-only), exactly as for
  sentinel and session-runner (ADR-017).
- **New check — the fake-mount hazard**: doctor **fails provisioning
  when rp's `equipment.mount` points at the bridge's port or
  `UniqueID`**. Wiring the virtual device in as rp's real mount would
  defeat every motion safeguard rp believes it has (slews that "just
  succeed", a mount that is never parked, never at limits). The check
  is a hard failure, not a warning.

## Error handling summary

| Condition | Behavior |
|---|---|
| Slew coords out of range | ASCOM `InvalidValue`; no import |
| Sync verbs | ASCOM `NOT_IMPLEMENTED` (`CanSync = false`) |
| Motion verbs for `false` capabilities | ASCOM `NOT_IMPLEMENTED` |
| rp rejects `add_target` (tool error) | `error!` log; **not** spooled (would fail again) |
| rp unreachable | Spool append (`fsync` per entry); GoTo still converges normally |
| Spool full | Drop oldest; `error!` per drop; `dropped_total`++ |
| Spool file unreadable at startup | `error!`, start with an empty spool (never refuse to start) |
| Corrupt spool line on replay | Skip + `error!` with the line number; continue |
| Client disconnect | Nothing arrives (P3a) — no state depends on a disconnect signal |

## MVP scope

**In scope:** the single virtual Telescope device (capability matrix
above), all three slew verbs as the import gesture, sync rejection,
the altitude-floor reported-position policy, live site adoption,
`assume_epoch`, the bounded spool with restart-surviving replay,
`/health`, doctor registration + the fake-mount check, ConformU clean,
and the rp-side contract (writer identity, `source` semantics,
cone-search naming, `rp-catalog::nearest`).

**Deferred:**

- Sentinel scraping `/health` (follow-up once a second consumer wants
  it).
- `position_angle_degrees` on imports — P2 owns the field; imports
  carry no angle until then (and the P4 inbox is where per-target
  angles are entered regardless — SkySafari cannot export its FOV
  angle, P3a/Decision 5).
- Stellarium/CdC enrichment (P5/P6 — both can use this device
  unenriched meanwhile).
- Retroactive display-name disambiguation of earlier imports.
- Any per-client identity beyond the provenance stamp (`ClientID` is
  unstable across app contexts — P3a).

## Testing

BDD drives the device with the `ascom-alpaca` **client** feature (the
same harness pattern the other drivers use) plus a **stub rp MCP
server**; rp-side semantics are covered in rp's own BDD suite against
the real store.

| Feature file (bridge) | Scenarios |
|---|---|
| `device_contract.feature` | Capability matrix; sync verbs rejected; Target* propagation; abort ends motion but not import; site push adoption; UTCDate write rejected |
| `target_import.feature` | GoTo → `add_target` (all three verbs); epoch conversion under `assume_epoch: jnow`; below-horizon GoTo imported; superseding slews each import |
| `position_policy.feature` | Floor snap to idle point; below-floor target converges but reports idle; `null` floor reports raw pointing |
| `spooling.feature` | rp down → spool; replay in order on recovery; replay after restart; overflow drops oldest with counter; corrupt line skipped; tool-error not spooled |

rp-side additions (rp's suite): import creates pending with writer
identity; proximity upsert of a pending-unedited import; active /
operator-edited / operator-created rows never mutated (suffixed slug
instead); mosaic-spaced GoTos stay distinct; plain vs offset vs
coordinate display names; goals defaulted; `source` +
`catalog_ref`/`display_name` rejected. `rp-catalog::nearest` gets
unit tests (hit/miss/tie, offset vector signs, tolerance edge).

ConformU runs under `bazel test --config=conformu` per the existing
mock-backend pattern.

## Open item: P3b horizon experiment

SkySafari's below-horizon GoTo gate is documented as unconditional
("you cannot GoTo an object which is below the horizon" — SkySafari
Pro 8 user guide) but P3a proved it leaky (coordinate-entry GoTos are
not gated; the wedge keys on the *reported scope position*). Whether
the **Horizon & Sky display settings** (horizon off / transparent)
affect the gate is undocumented in both directions. Before the
implementation PR merges its operator docs, re-run the spike
(`spikes/planetarium-bridge-p3a`, still in tree) with SkySafari's
horizon display off and/or transparent and answer:

1. Is an object-GoTo to a below-horizon target still refused?
2. Does the reported-position wedge still occur?
3. Is coordinate-entry GoTo still ungated below 0°?

Outcome shapes the *default posture and operator docs only* — the
altitude-floor mechanism above is safe under every outcome (a
disabled gate just makes `report_altitude_floor_deg: null` a
documented client-specific option).

---

## Appendix: P3a verification-spike findings (2026-07-29)

Session: 2026-07-29 (UTC), **SkySafari Pro 8.0.3 (build 1205)** on iPad
driving the spike over Wi-Fi, ~20 minutes of traffic; the JSONL wire
log is the raw evidence (kept off-repo with the operator). Findings are
from this one client/version; the plan's SkySafari floor is v7
(Decision 1), not separately tested. The spike crate
`spikes/planetarium-bridge-p3a` (throwaway, sanctioned per Decision 8 /
ADR-005) remains runnable for the P3b experiment above and is deleted
in the P3 implementation PR.

### The P3a questions — answered

| # | Question | Verdict | Finding |
|---|----------|---------|---------|
| 1 | Discovery and connection lifecycle | Answered | Alpaca UDP discovery is **subnet-local** (broadcast; does not cross routed segments — the iPad and spike host were on different subnets and no discovery datagram ever arrived). Manual IP:port entry works across subnets, confirming the plan's documented-default posture. Lifecycle detail below. |
| 2 | Which slew/sync verbs | Answered | GoTo = **`SlewToCoordinatesAsync`, exclusively** (never the blocking variant, never `SetTarget*`+`SlewToTarget`). Align = **`SyncToCoordinates`** (never `SyncToTarget`). The verbs are cleanly distinct — Decision 2's sync-is-not-intent rule is safe. Numbers arrive in **scientific notation** (`RightAscension=1.341988e+01`); parsers must accept it (our `ascom-alpaca` fork does). |
| 3 | J2000 honored, or JNow? | Answered | **J2000 honored.** Five object GoTos (Spica, M 92, M 13, Draco Dwarf, HD 142596) all matched the target's J2000 position to arcseconds; the automated probe verdict on M 13 read 0.04′ (J2000 frame) vs 12.13′ (JNow frame). `EquatorialSystem` is read **once, at connect** — changing the declaration requires a reconnect. No epoch setting exists anywhere in SkySafari's scope UI, so this reads as client behavior (honor the device declaration), not per-install configuration; the `assume_epoch` override stays as a safety valve. |
| 4 | Position-report cadence | Answered | A steady **1 Hz cycle** of `Slewing` → `Tracking` → `RightAscension` → `Declination` (four GETs ~10 ms apart, every ~1.0 s), identical while idle and while slewing. The spike's 3 s simulated convergence satisfied it: SkySafari showed the slew as arrived when `Slewing` flipped false. No connection timeouts observed. |
| 5 | **Go/no-go: arbitrary-point GoTo** | **GO, with caveats** | Tapping empty sky offers **no** GoTo — an object must be selected. But **coordinate entry (Search → coordinates) exists and GoTos arbitrary points**. Caveat: the entry form has **no epoch choice and interprets input as equinox-of-date (JNow)**, converting to J2000 on the wire — proven by a round-number probe: entered 14h00m00s / −40°00′00″ arrived as 13h58m23s / −39°52′11″, which is exactly the J2000 equivalent of the entered values read as JNow (the RA −1m37s / Dec +7.7′ shifts match 26.6 years of precession at that position to the second). A second practical path: SkySafari's selectable catalog reaches faint HD/Tycho stars and obscure PGC galaxies — a star within arcminutes of any intended frame center almost always exists and GoTos with exact J2000 coordinates. |

### Connection lifecycle detail

- **Preset editor probe** (before any connect): `apiversions` →
  `configureddevices` → `apiversions` → `alignmentmode` →
  `canslewasync` → `canslewaltazasync`, under throwaway `ClientID`s.
- **Connect**: `PUT Connected=true`, then a property battery —
  `EquatorialSystem` (once), site latitude/longitude, `UTCDate`, and a
  capability sweep (`CanSync`, `CanSyncAltAz`, `CanSetTracking`,
  `CanPark`, `CanMoveAxis`, `CanSlewAsync`, `CanSlewAltAzAsync`) — then
  the 1 Hz poll loop.
- **SkySafari pushes site and time to the device**: `PUT SiteLatitude` /
  `PUT SiteLongitude` (its own GPS-derived values) and `PUT UTCDate`.
  The spike accepted the site writes; `SetUTCDate` answered
  `NOT_IMPLEMENTED` (1024) — SkySafari retried once ~23 s later and
  carried on unaffected.
- **Disconnect sends nothing**: polling simply stops — no
  `Connected=false` was observed. The bridge must not depend on an
  explicit disconnect signal (idle-timeout thinking only).
- `ClientID` is **not stable** across app contexts: the main scope
  panel used one value, other gestures another. Treat it as
  diagnostic, not identity.
- SkySafari's **Center** button is display-only (no wire traffic) —
  only **GoTo** and **Align** reach the device.

### Below-horizon behavior

- **GoTo is horizon-gated client-side.** A below-horizon target's GoTo
  is refused by SkySafari — sometimes silently (nothing on the wire, no
  dialog), sometimes with a warning dialog. Couch-planning an object
  that has not yet risen therefore **cannot** be imported by object-GoTo
  at that moment; the operator must import while the target is up, or
  use coordinate entry (not horizon-gated in the observed case, which
  reached a 17°-altitude point).
- **Align/Sync was NOT horizon-gated** — a sync to below-horizon M 31
  went through (moot for the real bridge: sync is now rejected).
- **Wedge hazard**: after that sync put the virtual scope's reported
  position below the horizon, SkySafari refused *every* subsequent GoTo
  ("stuck") until the reported position returned above the horizon
  (fixed server-side with a corrective sync). This drove the
  [reported-position policy](#reported-position-policy-the-altitude-floor):
  the hazard exists with no sync at all, since a tracked position
  imported at dusk sets hours later.

### Design implications carried into this document

1. Manual IP:port is the primary connection story → § Discovery.
2. The GoTo/Sync verb split is exactly as Decision 2 assumed; sync
   carries zero intent → § Sync is rejected (rejection chosen over the
   plan's accept-and-ignore, 2026-07-29).
3. The wire is J2000 end-to-end for this client → `assume_epoch`
   default `"j2000"`, kept as cheap insurance.
4. Site/UTC pushes and scientific-notation floats → § Site and time
   writes.
5. Reported position must never linger below the horizon → § the
   altitude floor.
6. SkySafari's coordinate-entry box is JNow (no epoch choice) —
   operator docs must warn that J2000 coordinates typed there land
   ~20′ off; framing via a nearby faint catalog star avoids the
   pitfall entirely.
7. No explicit disconnect arrives → no bridge state may depend on one.
