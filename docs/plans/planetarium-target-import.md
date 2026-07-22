# Planetarium Target Import Plan — pick targets in a planetarium, image them when conditions are right

## Goal

Target selection today means editing `targets[]` in rp's JSON config by hand.
The operator's actual planning tool is a planetarium — SkySafari on the couch,
Stellarium or Cartes du Ciel at a desk — where they compose the frame: an
offset center to fit a nebula plus a star field, a mosaic anchor, a rotation
that puts a companion galaxy in the corner. None of that survives into
rusty-photon; it is retyped, approximately, into config.

The outcome of this plan: **pressing GoTo in the planetarium adds the target
to rusty-photon's target database.** The planetarium connects to a virtual
ASCOM Alpaca telescope served by a new `planetarium-bridge` service; a GoTo
delivers the exact framed coordinates (never a catalog-centroid
approximation), the bridge names the target by reverse catalog lookup, and
the target lands *paused* in an inbox for the operator to review — attach
acquisition goals, adjust the position angle, activate. rp's planner then
images it whenever conditions are right; the planetarium and the scheduler
stay fully decoupled.

Position angles are handled in layers: per-target angle (set in the ui-htmx
target editor, for rigs with a rotator) → global default angle in rp config
(a rotator-less rig's fixed camera mounting angle, matched manually in the
planetarium's FOV indicator) → 0° = north-up.

## Implementation Status

| Phase | Description | Status | Branch / PR |
|-------|-------------|--------|-------------|
| P0 | This plan | In progress | feature/planetarium-target-import |
| P1 | Build the `rp-targets` crate + rp target CRUD MCP tools (per [rp-targets.md](../crates/rp-targets.md) MVP) | Not started | |
| P2 | Position-angle plumbing: `position_angle_degrees` on `Target`, rp config default, `get_next_target` returns effective angle, `deep_sky` workflow rotator step | Not started | |
| P3 | `planetarium-bridge` service: Alpaca Telescope impersonation → `add_target` (milestone P3a: logging skeleton, verified against SkySafari) | Not started | |
| P4 | ui-htmx target inbox: review pending targets, goal templates, PA override, activate/discard | Not started | |
| P5 | Stellarium enrichment frontend (telescope-protocol doorbell + RemoteControl name/Oculars-angle query) | Deferred | |
| P6 | Cartes du Ciel frontend (TCP 3292 client: named selections, `GETFRAMES` mosaic import with per-panel PA) | Deferred | |

Order: P1 → P2 → P3 → P4; P2 can proceed in parallel with P3 (the bridge
writes targets with `position_angle_degrees: None` until P2 lands). P5/P6
are deferred until the P3/P4 loop is proven in the field. Each phase follows
[development-workflow.md](../skills/development-workflow.md): design-doc
update first (rp.md for P1/P2, new docs/services/planetarium-bridge.md for
P3, ui-htmx.md for P4), BDD second, code third.

Explicitly rejected / out of scope (see Decisions 6–8):

- LX200/Meade TCP emulation and SkyFi UDP discovery (the pre-Alpaca SkySafari
  path).
- Observing-list file importers (`.skylist`, Stellarium `observingList.json`,
  CdC lists). `.skylist` carries catalog names only — no coordinates — so a
  file import can only recover catalog centroids, which defeats framing.
  May return later as an explicitly-labeled "unframed seeding" convenience.
- Any LiveSky (SkySafari cloud) integration: no public API, and the service
  cannot export observing lists at all.

## Decisions (fixed — settled interactively 2026-07-22)

1. **The universal ingress is a virtual ASCOM Alpaca Telescope, not LX200.**
   SkySafari 7+ speaks Alpaca natively (v7 floor is accepted; v8 shipped
   2025-12), Cartes du Ciel has a cross-platform Alpaca mount driver, and
   this codebase is already an Alpaca shop: the `ascom-alpaca` crate's
   server feature, `AlpacaServerConfig`, doctor registration, and the
   ConformU harness all apply as-is, versus hand-emulating a quirk-laden
   text protocol (stateful `:Sr`/`:Sd`/`:MS#`, ~4 Hz polling with
   connection churn, undocumented JNow semantics). Alpaca also lets the
   device *declare* its epoch (`EquatorialSystem` = J2000) instead of
   guessing, and standard Alpaca UDP discovery (port 32227) replaces the
   reverse-engineered SkyFi discovery hack.
2. **GoTo is the add-target gesture.** Selection events are browsing noise;
   a GoTo is deliberate, in-app, and carries the exact coordinates the
   planetarium is targeting — including offset centers and mosaic anchors
   that exist nowhere in any catalog. The bridge accepts the slew, reports
   a brief simulated convergence so the client shows completion, and never
   touches hardware (tenet 3 is satisfied trivially: the device is
   virtual; real rotator motion happens only inside operator-started
   sessions, in the P2 workflow step).
3. **Captured targets land paused (`active: false`) with a default goal
   template.** Planetariums say *where*, never filters/exposures. The
   operator reviews in the P4 inbox. Repeated GoTos within a small
   coordinate tolerance upsert the same pending target instead of
   duplicating it.
4. **Naming by reverse cone-search.** No name crosses the Alpaca wire. The
   bridge resolves the nearest `rp-catalog` object within a tolerance
   (configurable, default ~10 arcmin) for `display_name`/`catalog_ref`;
   otherwise a coordinate-derived slug (e.g. `j0042p4116`). The catalog hit
   never replaces the received coordinates — framing is authoritative.
5. **Position angle is a three-layer fallback.** New field
   `position_angle_degrees: Option<f64>` on the `rp-targets` `Target`
   (degrees east of north, sky frame). Effective angle =
   target value → rp config `framing.default_position_angle_degrees` →
   0.0 (north-up). The config default serves rotator-less rigs whose camera
   is mounted at a fixed non-zero angle: set it once, dial the same angle
   into the planetarium's FOV indicator, and frames match. With no rotator
   in the train the angle is planning metadata only; with one,
   `get_next_target` returns the effective angle and the `deep_sky`
   workflow moves the rotator (existing `move_rotator` verb, sky frame)
   after slew/centering. SkySafari cannot export its FOV-indicator angle
   through any channel (verified: Alpaca device types are telescope + camera
   only, in both v7 and v8), so per-target angles are entered in the inbox.
6. **The bridge is an MCP client of rp** (the `calibrator-flats` pattern),
   calling the P1 `add_target` tool. It is not an orchestrator plugin and
   is never on the imaging path.
7. **Stellarium and CdC are richer and come later.** Their servers
   (RemoteControl HTTP :8090, CdC TCP :3292) deliver what Alpaca cannot —
   object names, Oculars/mosaic rotation angles, per-panel mosaic frames —
   so P5/P6 use those as the data plane (Stellarium's telescope-protocol
   goto on :10001 remains only as the intent doorbell, with enrichment
   queried back from the sender's own IP). Both apps can also use the P3
   Alpaca device unenriched in the meantime.
8. **Empirical verification gates the build-out (milestone P3a).**
   SkySafari's Alpaca client behavior is undocumented. P3a is a logging
   skeleton Telescope device: confirm discovery, connection lifecycle,
   whether `EquatorialSystem` J2000 is honored (else precess client-side),
   which slew verb it uses (`SlewToCoordinatesAsync` expected), and whether
   arbitrary-point GoTo (not just cataloged objects) is possible from the
   SkySafari UI. Findings go into docs/services/planetarium-bridge.md
   before the full device is implemented.

## P3 sketch: `planetarium-bridge`

- New service `services/planetarium-bridge`, port **11126** (next free in
  the driver band), standard scaffolding per
  [service-lifecycle.md](../skills/service-lifecycle.md):
  `ServiceRunner`, `resolve_and_init` config bootstrap (Alpaca `UniqueID`),
  `pkg/doctor.toml`, workspace/Bazel registration.
- Serves one Alpaca `Telescope` device via `ascom-alpaca` (server feature):
  `EquatorialSystem` = J2000; sidereal time/alt-az derived from rp's site
  config; `SlewToCoordinatesAsync` records the target and simulates a short
  convergence; `SyncToCoordinates` treated identically (some clients sync
  rather than slew); park/tracking are polite no-op state. ConformU-clean
  (`bazel test --config=conformu`) like every other driver.
- On GoTo: optional precession to ICRS → reverse cone-search `rp-catalog` →
  upsert-or-create paused target via rp MCP `add_target`, goals from the
  bridge's `default_goal_template` config. rp unreachable ⇒ queue and retry
  with backoff; never drop a received target silently.
- BDD: drive the device with the `ascom-alpaca` *client* feature (same
  crate, same pattern the other drivers use for their harnesses) plus a
  stub rp MCP server; scenarios for goto→add, sync→add, dedup-upsert,
  unresolved-name slugs, rp-outage queueing, epoch handling.

## Channel reference (research summary, 2026-07-22)

| App | Channel | Payload | Rotation angle |
|---|---|---|---|
| SkySafari 7/8 | Alpaca Telescope GoTo (P3) | exact RA/Dec only | none — FOV angle not exported via any channel |
| SkySafari 5/6 | LX200-over-TCP :4030 (+SkyFi UDP :4031) — rejected | RA/Dec only, JNow, protocol quirks | none |
| SkySafari (files) | `.skylist` — rejected for framing | catalog names only, no coordinates | none |
| Stellarium | RemoteControl HTTP :8090 (P5 data plane); telescope protocol :10001 (P5 doorbell, 20-byte LE fixed-point J2000 goto) | selected object name + J2000 RA/Dec (JSON) | `Oculars.selectedCCDRotationAngle` via StelProperty API |
| Cartes du Ciel | TCP :3292 server, CCDciel client pattern (P6); `GETCHARTEQSYS` for epoch | pushes name + RA/Dec (+catalog `pa:`) on selection | mosaic `GETFRAMES` frames carry true per-panel framing PA |
| LiveSky | — rejected | no public API; cannot export lists | — |

Sources: Stellarium RemoteControl API (stellarium.org/doc/head/remoteControlApi.html),
Stellarium telescope protocol v1.0 (free-astro.org mirror of
Stellarium_telescope_protocol.txt), CdC server commands
(ap-i.net/skychart/en/documentation/server_commands) and CCDciel
`cu_planetarium_cdc.pas` (github.com/pchev/ccdciel), INDI `skysafari.cpp` and
AlpacaScope (github.com/synfinatic/alpacascope) for the rejected LX200 path,
SkySafari 8 Pro product/App Store pages (skysafariastronomy.com,
store.simulationcurriculum.com) for Alpaca device-type coverage.

## Open questions (carried into P3a)

- Does SkySafari honor a device-declared J2000 `EquatorialSystem`, or send
  JNow regardless?
- Can the SkySafari UI GoTo an arbitrary tapped point / entered coordinates,
  or only cataloged objects? (Determines how much framing-nudge UI the P4
  inbox needs for SkySafari-originated targets.)
- Minimum position-report cadence/shape SkySafari needs to consider a slew
  complete and stay connected.
