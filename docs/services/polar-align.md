# polar-align — Plate-Solving Polar Alignment Orchestrator

## Overview

`polar-align` is an orchestrator plugin that measures how far an
equatorial mount's RA axis is from the refracted celestial pole and
guides the operator through correcting it. It slews the mount to three
RA positions near the pole, captures and plate-solves an image at
each, computes the axis direction from the three solved pointings,
then enters a live adjustment phase: it keeps capturing and solving
while the operator turns the mount's azimuth/altitude adjusters,
publishing the residual error and PoleMaster-style star/target-circle
pairs after every solve.

The method is N.I.N.A. Three Point Polar Alignment's: rotating only
the RA axis sweeps the camera pointing along a circle whose center is
the axis; plate solves measure that circle absolutely. See
`docs/plans/polar-align.md` for the decision record (measurement
geometry, refraction, adjustment math).

### Tenets

1. **Measure absolutely, every frame.** Every image is plate-solved;
   nothing is tracked incrementally or template-matched. A failed
   solve skips one update; the next solve recovers the full state.
   Big corrections that push stars out of the frame are handled by
   construction — the next solve simply picks new stars.
2. **Only the RA axis moves between measurement exposures.** All three
   measurement points sit on one side of the meridian so a GoTo can
   never meridian-flip mid-measurement. A flip would move the dec
   axis and invalidate the geometry.
3. **Stop-class cleanup only** (project tenet 3). On failure or
   completion the workflow aborts any in-flight slew and leaves the
   mount tracking where it stands. It never parks and never slews
   back — a cleanup slew could itself fail and mask the original
   error, and the operator is at the mount anyway.
4. **The operator finishes the session.** Adjustment is interactive by
   nature; the loop runs until the operator posts
   `/adjust/finish` — bounded by `adjustment.max_duration` so an
   abandoned session cannot hold the mount and camera forever.

## Architecture

`polar-align` is a standalone HTTP service. `rp` invokes it as an
orchestrator plugin when a polar-alignment session starts; the plugin
connects back to rp's MCP server and calls primitive tools. The
browser/UI never talks to the plugin's workflow directly — it polls
`GET /status` (via ui-htmx in a later phase).

```
  rp (equipment gateway)            polar-align (orchestrator)
  ┌───────────────────┐             ┌──────────────────────────────┐
  │                   │ POST /invoke│ Measurement phase            │
  │  session start ───┼────────────►│  1. unpark + tracking on     │
  │                   │             │  2. 3× (slew, capture,       │
  │  MCP server  ◄────┼─────────────┤        plate_solve)          │
  │  /mcp             │  tool calls │  3. axis + alt/az error      │
  │                   │             │ Adjustment phase             │
  │  REST API    ◄────┼─────────────┤  4. loop: capture, solve,    │
  │  /api/plugins/    │  completion │     update error + targets   │
  │  {wf_id}/complete │             │  5. finish → completion      │
  └───────────────────┘             └──────────────┬───────────────┘
                                                   │ GET /status (JSON)
                                                   ▼
                                            operator / ui-htmx
```

The solved images stay on the shared filesystem (`rp.md` §"File
Accessibility"); `/status` carries their paths, never pixels.

### Port

11172 (configurable) — in the orchestrator-plugin range next to
`calibrator-flats` (11170) and `session-runner` (11171).

## MCP Tools Used

| Tool | Usage |
|------|-------|
| `get_park_state` | Read `at_park` before any motion; decide whether to unpark |
| `unpark` | Clear `AtPark` (no motion) before enabling tracking |
| `get_tracking` | Read tracking state and `can_set_tracking` |
| `set_tracking` | Enable sidereal tracking (required by `slew`; keeps the field quasi-static during adjustment) |
| `slew` | Move to each measurement point (equal-dec, one pier side) |
| `abort_slew` | Cleanup only: stop an in-flight slew on failure |
| `capture` | Take the measurement and adjustment exposures |
| `plate_solve` | Solve each capture (hinted with the commanded pointing) |
| `detect_stars` | Locate the brightest stars in each adjustment frame for the target-circle overlay |

## Invocation Protocol

`rp` POSTs to the plugin's `/invoke` endpoint when a session starts,
exactly as for calibrator-flats:

```json
{
  "workflow_id": "wf-550e8400-e29b-41d4",
  "session_id": "session-2026-08-01",
  "mcp_server_url": "http://localhost:11115/mcp",
  "recovery": null
}
```

The plugin acknowledges with timing estimates: `estimated_duration` =
3 × (slew + exposure + solve allowance) + `adjustment.max_duration` / 2,
`max_duration` = the same with the full `adjustment.max_duration`.
`recovery` is accepted and ignored (a polar-alignment session is
re-run from scratch; there is no state worth resuming).

A second `/invoke` while a workflow is running is rejected with
`409 Conflict` — the plugin drives a single mount and camera; two
concurrent alignments are meaningless.

## Behavioral Contracts

### Measurement phase

1. `get_park_state`. If `at_park` and `can_unpark`: `unpark`. If
   `at_park` and not `can_unpark`: abort with an error naming the
   condition (nothing has moved).
2. `get_tracking`; if tracking is off and `can_set_tracking`:
   `set_tracking(true)`. Off and not settable: abort (rp's `slew`
   would fail anyway; failing here gives a clearer message).
3. Compute the three measurement targets from the site and wall
   clock: local sidereal time from `site.longitude_deg`; target
   hour angles `direction × (first_point_ha_deg + i × sweep_deg)`
   for i = 0, 1, 2; declination `measurement_dec_deg` (sign follows
   the site hemisphere); RA = LST − HA, folded to [0, 24h).
4. For each point: `slew` → settle → `capture`
   (`measurement.exposure`) → `plate_solve` with
   `pointing_hint` = the commanded coordinates and
   `search_radius_deg` = `solve.search_radius_deg`.
5. Axis from the three solved centers (plane normal, sign toward the
   visible pole), converted to observed azimuth/altitude; error
   against the refracted pole (see the plan's D2/D3). If the three
   centers are closer than `min_point_separation_arcsec` (degenerate
   sweep — mount didn't move), abort with a distinct error.
6. Phase transitions to `adjusting`; the measurement result is
   published on `/status`.

A failure at any step posts `status: "error"` to the completion
endpoint with a `reason` naming the step, after stop-class cleanup
(tenet 3): `abort_slew` if and only if a slew was in flight.

### Adjustment phase

Loop until `/adjust/finish` or `adjustment.max_duration`:

1. `capture` (`adjustment.exposure`) → `plate_solve` hinted with the
   previous solve's center.
2. Camera attitude from the solve's full WCS (center + CD matrix,
   parity included). Axis update `K ← R · K_prev` where `R` is the
   relative rotation between the previous and current attitudes —
   sidereal tracking rotates about the axis itself and therefore
   drops out of the update; only adjuster motion moves `K`.
3. Recompute the alt/az error. Detect stars via rp's `detect_stars`
   tool and keep the `star_count` brightest unsaturated ones; for
   each, compute its target pixel — where it will sit when the axis
   is on the refracted pole — via the correction rotation applied to
   the current attitude.
4. Publish everything on `/status`.

A failed solve (moving mount blurs stars while the operator turns a
bolt) is expected: the iteration is skipped, `/status.last_solve`
reports `failed`, and the loop continues. `consecutive_solve_failures`
≥ `adjustment.max_solve_failures` aborts the workflow — the sky may
have clouded over.

`/adjust/finish` (or the deadline) posts the completion report and
returns the plugin to idle. Tracking is left on, mount in place —
the operator typically proceeds straight into a normal imaging
session.

### `GET /status`

The live contract for UIs. Always available; before any invocation it
reports `"phase": "idle"`.

```json
{
  "phase": "adjusting",
  "workflow_id": "wf-550e8400-e29b-41d4",
  "measurement": {
    "axis_azimuth_deg": 0.35,
    "axis_altitude_deg": 47.61,
    "azimuth_error_arcmin": 21.0,
    "altitude_error_arcmin": -12.4,
    "total_error_arcmin": 24.4,
    "azimuth_direction": "move azimuth west",
    "altitude_direction": "raise altitude",
    "measured_at": "2026-08-01T21:14:03Z"
  },
  "adjustment": {
    "updated_at": "2026-08-01T21:15:11Z",
    "image_path": "/data/rp/images/pa-000042.fits",
    "in_frame": true,
    "stars": [
      { "x": 512.3, "y": 388.1, "target_x": 498.7, "target_y": 401.0 }
    ],
    "last_solve": "ok",
    "consecutive_solve_failures": 0,
    "iterations": 17
  },
  "error": null
}
```

- `phase`: `idle` | `measuring` | `adjusting` | `complete` | `error`.
- `measurement` appears from the end of the measurement phase onward
  and is updated by every adjustment solve (the error shrinks as the
  operator converges). Signed errors: azimuth positive = axis east of
  the pole, altitude positive = axis above it; the `*_direction`
  strings state the corrective adjuster motion in plain words.
- `adjustment.in_frame` is false when the total error exceeds what the
  sensor can show (targets would fall outside the frame); UIs show an
  arrow from the numbers instead of circles.
- `stars` pairs each detected star's current pixel with its aligned
  target pixel, in 0-based pixel indices of `image_path` (the
  convention `detect_stars` reports; the WCS math is FITS 1-based
  internally and the service converts at that boundary).
- `error` carries the failure message when `phase` is `error`, null
  otherwise.

### `POST /adjust/finish`

Ends the adjustment loop, posts the completion report
(`status: "complete"`, `reason: "polar_alignment_complete"`, the final
measurement block, `adjustment_iterations`), returns `202 Accepted`.
Returns `409 Conflict` when no workflow is in the `adjusting` phase.

### `GET /health`

`200 OK` with a static body once the server is up (config validation
happens at load; there are no external resources to probe — the mount
and solver are reached through rp per-invocation).

## Configuration

The service reads a single JSON config file; `--config` names it,
otherwise the platform default (`~/.config/rusty-photon/polar-align.json`
on Linux, `%PROGRAMDATA%\rusty-photon\polar-align.json` on Windows)
via `rusty-photon-config`. Site coordinates are mandatory, so the
packaged systemd unit gates on the file with `ConditionPathExists`
(no built-in default config). `deny_unknown_fields` throughout.

```json
{
  "server": { "port": 11172, "bind_address": "0.0.0.0", "tls": null, "auth": null },
  "service_auth": null,
  "ca_cert": null,
  "camera_id": "main-cam",
  "mount_id": "mount",
  "site": { "latitude_deg": 48.1, "longitude_deg": -122.8 },
  "measurement": {
    "dec_deg": 85.0,
    "first_point_ha_deg": 15.0,
    "sweep_deg": 45.0,
    "direction": "west",
    "exposure": "2s",
    "settle": "2s"
  },
  "adjustment": {
    "exposure": "2s",
    "interval": "1s",
    "max_duration": "30m",
    "max_solve_failures": 10,
    "star_count": 10
  },
  "solve": { "search_radius_deg": 5.0, "timeout": "30s" },
  "refraction": { "enabled": true, "temperature_c": 10.0, "pressure_hpa": 1010.0 }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server` | object | `{ "port": 11172 }` | Shared `ServerConfig` (ADR-016) for `/invoke`, `/health`, `/status` |
| `service_auth` / `ca_cert` | — | null | Credentials/CA toward rp, exactly as calibrator-flats (ADR-017) |
| `camera_id` | string | required | Camera on the imaging train used for alignment exposures |
| `mount_id` | string | required | The mount (informational; rp's mount tools address the singular configured mount) |
| `site.latitude_deg` | float | required | Geodetic latitude, degrees, north positive. Range ±90; `abs(latitude) < 1°` is rejected (no meaningful pole altitude) |
| `site.longitude_deg` | float | required | Degrees, east positive, range ±180 |
| `measurement.dec_deg` | float | 85.0 | Measurement declination; sign is folded to the site hemisphere at load |
| `measurement.first_point_ha_deg` | float | 15.0 | Hour angle of the first point, degrees from the meridian (1–60) |
| `measurement.sweep_deg` | float | 45.0 | Hour-angle step between points (10–60; total span ≤ 150° keeps one pier side) |
| `measurement.direction` | `"east"`\|`"west"` | `"west"` | Which side of the meridian the three points sit on |
| `measurement.exposure` | humantime | `"2s"` | Measurement exposure duration |
| `measurement.settle` | humantime | `"2s"` | Extra settle after each slew before capturing |
| `adjustment.exposure` | humantime | `"2s"` | Adjustment-loop exposure duration |
| `adjustment.interval` | humantime | `"1s"` | Pause between adjustment iterations |
| `adjustment.max_duration` | humantime | `"30m"` | Hard ceiling on the adjustment phase |
| `adjustment.max_solve_failures` | int | 10 | Consecutive failed solves that abort the workflow |
| `adjustment.star_count` | int | 10 | Brightest stars published with target circles |
| `solve.search_radius_deg` | float | 5.0 | Passed to `plate_solve` (hinted with commanded/previous pointing) |
| `solve.timeout` | humantime | `"30s"` | Per-solve timeout passed to `plate_solve` |
| `refraction.enabled` | bool | true | Apply refraction to the pole target and the axis conversion |
| `refraction.temperature_c` / `pressure_hpa` | float | 10.0 / 1010.0 | Refraction model inputs |

Range rules are enforced parse-don't-validate style (newtypes with
serde `try_from`, per `development-workflow.md`), so a bad config
fails at load naming the field.

The rp-side plugin registration:

```json
{
  "name": "polar-align",
  "type": "orchestrator",
  "invoke_url": "http://localhost:11172/invoke",
  "requires_tools": [
    "capture", "plate_solve", "slew", "abort_slew",
    "set_tracking", "get_tracking", "unpark", "get_park_state"
  ]
}
```

`polar-align doctor [--config <file>] [--json]` diagnoses the config
read-only without starting the service, per
[doctor.md §Per-service doctors](doctor.md).

## Geometry Reference

The math contract (implemented in `math.rs` / `ephemeris.rs`, unit
tests are the executable spec):

- **Axis from centers.** Unit vectors `p1, p2, p3` from the three
  solved centers; `K = normalize((p2 − p1) × (p3 − p2))`, sign flipped
  if `K` points away from the visible pole's hemisphere. Degenerate
  input (`|p_i − p_j|` under `min_point_separation_arcsec`, or a
  cross product below numeric floor) is an error, not a NaN.
- **Attitude from WCS.** Boresight from the solve's center; the
  solve response's `wcs_matrix` block (CRPIX + the 2×2 CD matrix,
  degrees/pixel, FITS conventions) gives the sky directions of the
  pixel axes on the tangent plane (ξ east, η north), orthonormalized
  into a rotation matrix. `det(CD) > 0` means a mirrored (flipped)
  image and is handled by construction — no separate parity flag in
  the math. A solve without `wcs_matrix` fails the adjustment
  iteration (it cannot yield an attitude); the measurement phase
  needs only centers.
- **Axis update.** `K ← (A_now · A_prev⁻¹) · K_prev`. Sidereal
  tracking is a rotation about `K` itself and cancels; adjuster
  motion is a rotation about a roughly horizontal axis and is what
  the update measures.
- **Observed conversion.** ICRS → observed alt/az via `rp-ephemeris`
  (ERFA), refraction per config. Pole target: azimuth 0 (north
  hemisphere) / 180 (south), altitude `|site.latitude_deg|` — with
  **no refraction term**: the solves already pulled the fitted axis
  down by refraction (apparent → catalog), so the refraction-on axis
  conversion re-adds it and a perfect axis lands on the geometric
  pole (the plan's D3 has the full derivation).
- **Targets.** Correction rotation `R_corr` = the rotation in the
  horizontal frame taking the axis onto the pole (azimuth rotation
  about the zenith, altitude rotation about the horizontal east–west
  axis — the two adjuster motions). Target pixel of a star at sky
  direction `s`: project `s` through the corrected attitude
  `R_corr · A_now`. `in_frame` = all targets within the sensor
  bounds reported by the solve's reference pixel geometry.

## Module Structure

```
services/polar-align/src/
  main.rs            CLI entry point (clap + tracing + doctor subcommand)
  lib.rs             ServerBuilder, BoundServer, module declarations
  config.rs          PolarAlignConfig + validated newtypes
  error.rs           Error types (thiserror)
  routes.rs          Axum router: /invoke, /health, /status, /adjust/finish
  mcp_client.rs      rp-mcp-client wrapper (ADR-017)
  workflow.rs        Measurement + adjustment orchestration, cleanup guard
  math.rs            Axis, attitude, error decomposition, target projection
  ephemeris.rs       ICRS→observed, LST, refracted pole (rp-ephemeris)
```

Star detection is rp's `detect_stars` tool — the plugin carries no
image-processing code of its own.

## Testing Strategy

Per `docs/skills/testing.md`.

- **Unit tests** carry the math: synthetic mounts with injected
  (azimuth, altitude) errors generate the three pointings by rotating
  a start vector about the misaligned axis; the module must recover
  the injected error to sub-arcsecond accuracy with refraction off,
  both hemispheres, both sweep directions, mirrored and unmirrored CD
  matrices. Star detection is rp's, already tested there; the
  plugin's selection logic (brightest-N, saturated rejection) is
  unit-tested on canned `detect_stars` payloads.
- **BDD** carries the orchestration, with the full topology (OmniSim
  telescope + camera, rp, polar-align) plus an in-test plate-solver
  stub whose canned solves are choreographed from a known injected
  axis error — the completion report must recover it. Scenarios per
  the plan's Phase 3 list; `doctor.feature` and `auth.feature` ride
  the shared smoke fixtures.
- **No OmniSim image↔pointing coupling exists**, so end-to-end
  optical truth arrives only in Phase 7 rig validation.

## MVP Scope

In scope for v1: everything above. Out of scope (see the plan):
ui-htmx page (P5), attitude-based axis + arbitrary start position
(P6), site-from-rp sourcing, manual-rotation mode for non-GoTo
trackers, PNG preview endpoint.

## References

- Plan + decision record — `docs/plans/polar-align.md`
- Template plugin — `docs/services/calibrator-flats.md`
- Solver contract — `docs/services/plate-solver.md`
- rp plugin protocol — `docs/services/rp.md` §Orchestrator Registration
- ADR-016 server config, ADR-017 MCP client policy
