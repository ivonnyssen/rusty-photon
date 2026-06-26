# rp — Main Application Design

## Overview

`rp` is the equipment gateway, event bus, and safety enforcer of Rusty
Photon. It exposes all equipment and services as MCP tools, emits events
that plugins consume, and enforces safety constraints that can override
any operation. It does not contain workflow logic — orchestration is
handled by a separate orchestrator plugin that drives the session by
calling tools on `rp`.

### Tenets

1. **Robustness above all else.** The application survives power failures,
   unresponsive devices, and plugin crashes without losing session progress.
2. **Maximize darkness time.** Every design decision optimizes for shutter-open
   time. Post-capture work runs in parallel with the next exposure.
3. **Automate what is safe to automate.** The planner makes target and filter
   decisions autonomously. Manual intervention is never required during a
   session.
4. **Remote interfaces only.** ASCOM Alpaca for devices, HTTP for plugins, HTTP
   for UIs. No direct hardware integrations. Ever.
5. **Minimal footprint.** The application runs on Linux, macOS, and Windows, and
   must be efficient enough for a Raspberry Pi 5. Memory and CPU budgets are tight.
6. **Loose coupling via events.** The application emits events; plugins react.
   The application knows as little as possible about what plugins do.
7. **UI is a client, not a component.** The web UI contains zero application
   logic. It renders state and sends commands. Anyone can build an alternative
   UI without changing the application.

## Architecture

The system is a constellation of independent web services. `rp` is the
equipment gateway at the center — it provides MCP tools, emits events,
and enforces safety. An orchestrator plugin drives the imaging session
by calling tools on `rp`.

```
                       ┌───────────────────┐
                       │     Web UI        │
                       │  (server-rendered │
                       │   HTML or any     │
                       │   framework)      │
                       │  NO app logic     │
                       └────────┬──────────┘
                                │ REST + WebSocket
                       ┌────────▼──────────┐
                       │       RP          │
                       │                   │
                       │  MCP Tool Server  │
                       │  Event Bus        │
                       │  Safety Enforcer  │
                       │  Session State    │
                       │  Planner          │
                       │  API Layer        │
                       └──┬────┬────┬──────┘
                          │    │    │
            ┌─────────────┤    │    ├─────────────┐
            │   Alpaca    │    │    │  Webhooks   │
            ▼             ▼    │    ▼             ▼
       [Camera]      [Mount]   │ [Analyzer]  [Cloud Backup]
       [Focuser]     [FWheel]  │ [Custom]
       [SafetyMon]             │
                               │ MCP (tools/call)
                     ┌─────────┴──────────┐
                     ▼                    ▼
              [Orchestrator]       [Guider Service]
              (workflow plugin     (wraps PHD2)
               drives session)
                     │
                     │ MCP (tools/call)
                     ▼
              [Plate Solver]  [Focus Plugin]  [Centering Plugin]
              (tool providers — compound tools that call back to rp)

            ┌──────────────────────────────────┐
            │          Sentinel                │
            │  Safety monitor (existing)       │
            │  Operation watchdog (new)        │
            │  Corrective actions (new)        │
            │  Subscribes to event bus         │
            └──────────────────────────────────┘
```

### Service Boundaries

Every component is a separate process communicating over HTTP (or JSON-RPC for
PHD2). `rp` is one service among many. Device drivers, plugins,
the guider service, Sentinel, and UIs are all independent processes. This
follows naturally from the Alpaca-only integration tenet — the device drivers
are already separate services.

### Component Categories

`rp` is "batteries included" — it owns the full set of tools and capabilities
that observatory automation routinely needs. Three distinct categories
contribute tools to the MCP catalog, each with its own supervision model and
process boundary:

| Category | What | Examples | Process boundary | Supervised by |
|----------|------|----------|------------------|---------------|
| **Built-in tools** | Rust code running inside `rp`'s own process | Equipment primitives, planner, image analysis (`measure_basic`, HFR, FWHM, eccentricity), V-curve auto-focus, iterative centering | none — same process | Sentinel watches `rp` itself |
| **rp-managed services** | Separate processes that wrap external apps `rp` cannot link against; their tools appear as built-in proxies in the catalog | Guider service (wraps PHD2), plate solver service (wraps ASTAP / astrometry.net) | one process per service | Sentinel restarts on hang/crash |
| **Plugins (workflow & extension)** | Separate processes that follow the plugin protocol (event, tool provider, orchestrator). Includes first-party workflow logic kept out of `rp` by design tenet 7, and third-party extensions. | First-party: `calibrator-flats`, future `deep-sky-orchestrator`, `sky-flat`, `planetary-orchestrator`. Third-party: custom analyzers (ML quality classifiers, wavefront tools), alternative tool providers, custom event consumers. | one process per plugin | `rp` enforces plugin timeouts and MCP session termination; Sentinel may restart configurable plugins |

The category boundary is **process supervision and lifecycle role**, not
authorship. Algorithms that are pure Rust math (auto-focus, centering) live
as built-in tools even though they could in principle be plugins. They become
rp-managed services only when they must wrap an external program (PHD2 the
application, ASTAP the binary) that has its own crash and restart behavior.

The plugin mechanism serves two purposes:

1. **Workflow logic stays out of `rp`** (design tenet 7). Orchestrators of any
   imaging type are plugins because workflow is per-session-type and should be
   swappable without changing the gateway. `calibrator-flats` is the first
   such orchestrator and ships in this workspace.
2. **Third-party extensibility** — external developers can add tools, event
   consumers, or alternative orchestrators without forking `rp`.

A plugin can be first-party (in the rusty-photon workspace) or third-party
(installed and configured by the operator). Both follow the same protocol.

From the perspective of an MCP client (the orchestrator, a workflow plugin),
all three categories look identical — they are all just tools in the unified
catalog discovered via `tools/list`.

### Port

11115 (configurable)

## Exposure Document

The exposure document is the central data exchange mechanism. Each exposure
produces one document — a sidecar JSON file that lives alongside the FITS file.
The document accumulates data as it flows through the system.

### Core Fields (owned by `rp`)

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "target": {
    "name": "M31",
    "ra": 10.6847,
    "dec": 41.2689
  },
  "camera_id": "main-camera-1",
  "filter": "Luminance",
  "exposure_time_secs": 300,
  "planned_at": "2026-03-02T01:15:00Z",
  "captured_at": "2026-03-02T01:20:02Z",
  "file_path": "/data/lights/M31/M31_L_300s_001.fits",
  "session_id": "session-2026-03-01",
  "sequence_number": 42,
  "max_adu": 65535,
  "optics": {
    "focal_length_mm": 1000.0,
    "pixel_size_x_um": 3.76,
    "pixel_size_y_um": 3.76,
    "sensor_width_px": 9576,
    "sensor_height_px": 6388,
    "pixel_scale_x_arcsec_per_pixel": 0.7756,
    "pixel_scale_y_arcsec_per_pixel": 0.7756,
    "fov_width_deg": 2.0630,
    "fov_height_deg": 1.3762
  }
}
```

`max_adu` carries the camera's `MaxADU` capability at the time of
capture. Read once per connection (`connect_camera` stashes it on
`CameraEntry` along with `pixel_size_*` and `camera_*_size` — they are
all invariant physical-sensor properties that cannot change for the
life of the connection) and persisted in the sidecar so the file is
self-describing — the disk-fallback rehydration path in
[Image and Document Cache](#image-and-document-cache) uses it to choose
the `CachedPixels::U16` vs `I32` variant without needing the originating
camera to be connected. `null` (omitted on serialize) when the connect-
time read failed; in that case the cache insert is skipped on every
capture from that camera and the entry serves from disk on demand.

`optics` carries the camera + optical-train geometry that consumers
need to interpret the frame without re-deriving it from a plate
solve. Built at capture time from three sources:

1. `focal_length_mm` is operator-supplied via `equipment.cameras[].focal_length_mm`.
   It captures the optical train (telescope, reducers, extenders) which has no
   ASCOM Alpaca property — even the optional `Telescope.FocalLength` ignores
   anything screwed in front of the camera.
2. `pixel_size_x_um` / `pixel_size_y_um` come from `cam.pixel_size_x()` /
   `cam.pixel_size_y()` (Alpaca `PixelSizeX` / `PixelSizeY`, microns),
   cached on `CameraEntry` at connect time.
3. `sensor_width_px` / `sensor_height_px` come from `cam.camera_x_size()` /
   `cam.camera_y_size()` (Alpaca `CameraXSize` / `CameraYSize`),
   cached on `CameraEntry` at connect time.

Pixel scale and FOV are derived (`fov_width_deg` corresponds to
`sensor_width_px`; height likewise):

```
pixel_scale_arcsec_per_pixel = 206.265 × pixel_size_um / focal_length_mm
fov_deg                       = pixel_scale_arcsec_per_pixel × sensor_size_px / 3600
```

The block is omitted (serializes as absent, not `null`) when any of
its inputs is unavailable: `focal_length_mm` was not configured for
this camera, the camera's `pixel_size_*` read failed at connect time,
or the camera's `camera_*_size` read failed at connect time. Each
missing input is logged at `debug!` (at connect time for the cached
fields; at capture time for the missing focal length). Capture
continues — `optics` is auxiliary metadata, not gating.

Per-frame variation (binning swaps, focal reducers screwed in
mid-session) is out of scope. The persisted block reflects the
operator-declared static optical train and the camera's reported
sensor geometry at capture time. This is sufficient for `plate_solve`
hint sourcing and for consumers like annotation and mosaic planning.

The `plate_solve` built-in tool (Phase 6c-2 in
`docs/plans/archive/image-evaluation-tools.md`) accepts an explicit
`fov_hint_deg` parameter that callers can populate from
`optics.fov_height_deg` on the document — `fov_height_deg` matches
ASTAP's `-fov` semantics ("image height in degrees"). v1 does **not**
auto-default `fov_hint_deg` from the optics block when the parameter
is omitted; that auto-default is tracked by
[issue #153](https://github.com/ivonnyssen/rusty-photon/issues/153)
and lands when the first workflow is blocked without it.

### Plugin Sections (contributed via API)

Plugins write results into named sections. `rp` merges them into the
document and persists the sidecar JSON. Each section is opaque to `rp` — it stores and serves whatever the plugin provides.

```json
{
  "id": "...",
  "...core fields...",
  "sections": {
    "wcs": {
      "ra_center": 10.6848,
      "dec_center": 41.2690,
      "pixel_scale_arcsec": 1.05,
      "rotation_deg": 12.3,
      "solver": "astap-0.9.1"
    },
    "image_analysis": {
      "hfr": 2.3,
      "star_count": 1847,
      "background_mean": 1200,
      "background_stddev": 45
    },
    "guiding_stats": {
      "rms_ra_arcsec": 0.45,
      "rms_dec_arcsec": 0.38,
      "total_rms_arcsec": 0.59
    },
    "weather": {
      "temperature_c": -5.2,
      "humidity_pct": 42,
      "dewpoint_c": -15.1
    },
    "image_stats": {
      "median_adu": 32768,
      "mean_adu": 32450.7,
      "min_adu": 28000,
      "max_adu": 38000,
      "pixel_count": 16777216
    }
  }
}
```

### Persistence

Each capture produces a FITS file and a sidecar JSON document sharing
the same base filename. The base is the first 8 hex characters of the
document's full UUID v4 (`<doc_uuid_8>`):

```
/data/lights/M31/
  550e8400.fits
  550e8400.json    <-- exposure document
```

The optional `session.file_naming_pattern` config is reserved for a
future operator-controlled template (`{target}`, `{filter}`,
`{duration}`, `{sequence}`, etc.). Until a token resolver lands that
can supply that context to `capture`, the field is parsed but not
rendered — capture writes `<doc_uuid_8>.fits` regardless of the
configured value. When a resolver lands, the rendered base will be
prefixed before the UUID-8 suffix (e.g. `M31_L_300s_001_550e8400.fits`)
so existing files stay reachable.

The full UUID is the canonical document identifier — used by the API,
the FITS header, and the sidecar's `id` field. The 8-char suffix
exists only on disk, as the reverse-lookup key for finding a
document's files when given its id. See
[Image and Document Cache](#image-and-document-cache) and
[Document Resolution](#document-resolution) for the rules.

The FITS file's primary HDU header carries `DOC_ID = '<full-uuid>'`,
making each FITS self-describing for lineage, downstream tools, and
disambiguation when multiple files in the data directory happen to
share an 8-char suffix. The sidecar's `id` field carries the same
full UUID as a fallback authority.

Both the FITS file and the sidecar JSON are written atomically:
staged to a sibling temp file, fsynced, renamed into place, parent
directory fsynced. A crash mid-write cannot leave a torn file. The
document is updated as plugins and tools contribute sections; each
section update re-serializes the full sidecar atomically.

The disk pair is durable beyond `rp`'s runtime. After eviction from
the in-memory cache (or after `rp` restart), a document remains
accessible via the API as long as its FITS+sidecar pair sits in
`<data_directory>`. The contract is "live as long as the file is on
disk", not "live as long as `rp` is up".

## Event System

`rp` emits events. Plugins and services subscribe via webhooks.
The application does not know or care what subscribers do with events.

Every blocking operation emits a lifecycle *triple* — a `*_started`
event at entry and a matching `*_complete` or `*_failed` at exit —
correlated by a shared `operation_id` and wrapped in the uniform
[Event Envelope](#event-envelope) below. (`sync_mount` is instant, so it
emits only `_complete` / `_failed`, with no `_started`.) Point events
(e.g. `filter_switch`, `session_started`) carry no `operation_id`.

### Events

| Event | Payload | When |
|-------|---------|------|
| `session_started` | session config, target list, equipment | Session begins |
| `session_stopped` | session summary, reason | Session ends (manual, safety, dawn) |
| `exposure_started` | camera_id, duration | Exposure begins |
| `exposure_complete` | document_id, file_path | Readout finished, document persisted |
| `exposure_failed` | error | Exposure failed (start error, camera error state, readout timeout, or FITS write) |
| `slew_started` | ra, dec | Mount begins slew |
| `slew_complete` | ra, dec, actual_ra, actual_dec | Mount reports slew done |
| `slew_failed` | error | Slew failed or timed out (best-effort abort issued) |
| `park_started` | — | Mount begins parking |
| `park_complete` | — | Mount reports `AtPark` |
| `park_failed` | error | Park failed or timed out (no auto-abort, per the park contract) |
| `unpark_started` | — | Mount begins unpark |
| `unpark_complete` | — | Mount unparked |
| `unpark_failed` | error | Unpark failed |
| `sync_mount_complete` | ra, dec | Mount sync applied (instant — no `_started`) |
| `sync_mount_failed` | error | Mount sync failed |
| `move_focuser_started` | focuser_id, position | Focuser begins move to the target position |
| `move_focuser_complete` | focuser_id, position | Focuser idle at the read-back position |
| `move_focuser_failed` | error | Focuser move failed or timed out |
| `plate_solve_started` | document_id, image_path, use_mount_hints | Plate solve begins |
| `plate_solve_complete` | ra_center, dec_center, pixel_scale_arcsec, rotation_deg, solver | Plate solve succeeded |
| `plate_solve_failed` | error | Plate solve failed |
| `centering_started` | camera_id, ra, dec, tolerance_arcsec, max_attempts | Plate-solve + correct loop begins |
| `centering_iteration` | camera_id, document_id, residual_arcsec, solved_ra, solved_dec, action | One centering iteration completed |
| `centering_complete` | camera_id, final_error_arcsec, attempts, final_ra, final_dec | Centering converged |
| `centering_failed` | error | Centering failed |
| `focus_started` | camera_id, focuser_id, position, temperature | Auto-focus begins |
| `focus_complete` | camera_id, focuser_id, position, hfr, samples_used | Auto-focus result |
| `focus_failed` | error | Auto-focus failed |
| `guide_started` | guider_id | Guiding loop started |
| `guide_settled` | rms_ra, rms_dec | Guiding RMS below threshold |
| `guide_stopped` | reason | Guiding stopped |
| `dither_started` | pixels | Dither command sent |
| `dither_settled` | rms_ra, rms_dec | Post-dither settle complete |
| `safety_changed` | monitor, new_state | SafetyMonitor transition |
| `temperature_changed` | sensor, value | Significant temperature change |
| `meridian_flip_started` | hour_angle | Flip initiated |
| `meridian_flip_complete` | — | Flip and re-center done |
| `target_switch` | old_target, new_target | Planner decided to switch targets |
| `filter_switch` | camera_id, old_filter, new_filter | Filter change on a camera |
| `frame_rejected` | document_id, plugin, reason | Immediate correction rejected a frame |
| `plugin_timeout` | plugin, event_id | Plugin did not respond within `max_duration` |
| `document_updated` | document_id, section_name | Plugin contributed a section |
| `document_persistence_failed` | document_id, file_path, error | Sidecar write failed during capture. The FITS file is on disk but the cache is not populated and no sidecar exists; `document_id`-keyed lookups return 404 (disk fallback requires the sidecar). Recover by reading the FITS via `file_path` from the payload. See [Capture Tool Details](#capture-tool-details). |

### Event Envelope

Every emitted event is wrapped in a uniform envelope. The envelope is
**additive** over the historical webhook body: `event_id`, `event`,
`timestamp`, and `payload` keep their exact meaning and contents, so
existing webhook plugins are unaffected. New fields are carried alongside
and absent optional fields are omitted from the JSON.

```json
{
  "event_id": "f3a8b9c0-1d4e-4a2b-8f3a-2c7d9e1f4b6a",
  "event_seq": 1247,
  "operation_id": "0bbc7e54-c2c2-4e3b-9a8d-7f43a3a8b2f1",
  "event": "slew_started",
  "timestamp": "2026-05-19T20:14:33Z",
  "started_at": "2026-05-19T20:14:33.412Z",
  "predicted_duration_ms": 21000,
  "max_duration_ms": 63000,
  "payload": { "ra": 12.0, "dec": -30.0 }
}
```

| Field | Meaning |
|-------|---------|
| `event_id` | Per-emission UUID. Unchanged; the routing key for the plugin completion contract (`POST /api/plugins/{event_id}/complete`). |
| `event_seq` | Monotonically increasing per-emission counter. Total order across all events; the SSE `id` (and `Last-Event-ID` replay key) for the [Real-Time Stream](#real-time-stream). |
| `operation_id` | Correlation key shared by an operation's `*_started`, `*_complete`, and `*_failed` events. Omitted for point events (e.g. `filter_switch`, `safety_changed`). |
| `event` | The event-type string, e.g. `"slew_started"`. |
| `timestamp` | ISO-8601 emission time. Unchanged historical format. |
| `started_at` / `ended_at` | RFC-3339 (millisecond) operation start / end. `started_at` is on the `*_started`/`*_complete`/`*_failed` triple; `ended_at` only on `*_complete`/`*_failed`. |
| `elapsed_ms` | Wall-clock operation duration, on `*_complete`/`*_failed`. |
| `predicted_duration_ms` / `max_duration_ms` | The operation's expected duration and hard-ceiling deadline, in integer milliseconds (a boundary serialization of an internal `Duration`). Populated (Phase 2) on: `slew_started` — `predicted = great-circle distance / mount.slew_rate_arcsec_per_sec + settle`, `max = max(predicted × 3, MIN_SLEW_DEADLINE = 30 s)`; `park_started` — `predicted = 180° / mount.slew_rate_arcsec_per_sec + settle` (worst-case traverse; rp can't read the park position via Alpaca), `max = max(predicted × 2, MIN_PARK_DEADLINE = 60 s)`; `move_focuser_started` — `predicted = \|target − current\| / focuser.steps_per_sec`, `max = max(predicted × 2, MIN_FOCUSER_DEADLINE = 5 s)`; `exposure_started` — `predicted = duration + camera.readout_time_estimate` (default 15 s when unset), `max = predicted + 30 s` readout headroom (advisory only — rp does not enforce it; the camera driver owns the exposure, and rp keeps a separate, more generous internal readout backstop); `centering_started` — outer-loop only (advisory; per-iteration slews/captures carry their own deadlines): `per_iter = duration + centering.solve_time_estimate (default 30 s) + centering.slew_overhead_estimate (default 10 s)`, `predicted = per_iter` (single-pass convergence), `max = max_attempts × per_iter`. Omitted for operations not yet converted to predictive deadlines. |
| `payload` | Operation detail. For `*_started`, the inputs; for `*_complete`/`*_failed`, the outcome (or `{"error": "..."}` on failure). |

A blocking operation emits a **triple** — a `*_started` envelope at the
entry point and a `*_complete` or `*_failed` envelope at the end, all
sharing one `operation_id`. (`sync_mount`, being instant per ASCOM, emits
only `*_complete` / `*_failed`.) See
[`docs/plans/archive/predictive-deadlines-and-watchdog.md`](../plans/archive/predictive-deadlines-and-watchdog.md)
for the deadline-monitoring design this envelope feeds.

### Delivery: Webhooks

Plugins register a callback URL and subscribed events in the configuration.
`rp` POSTs events to each registered URL. All plugins use the same
asynchronous request-response pattern.

#### Request

```
POST <plugin_webhook_url>
Content-Type: application/json

{
  "event_id": "evt-550e8400-e29b-41d4",
  "event": "exposure_complete",
  "timestamp": "2026-03-02T01:25:02Z",
  "payload": {
    "document": { ... },
    "file_path": "/data/lights/M31/M31_L_300s_001.fits"
  }
}
```

#### Step 1: Acknowledgment (immediate HTTP response)

The plugin responds immediately to the webhook HTTP request with an
acknowledgment declaring how long it expects to take:

```json
{
  "estimated_duration": "20s",
  "max_duration": "30s"
}
```

- `estimated_duration`: humantime string for how long the plugin expects
  processing to take. The planner uses this for scheduling decisions.
  Provided dynamically per invocation — a plate solve on a wide-field
  image may differ from a narrow-field one.
- `max_duration`: hard timeout. If the plugin doesn't complete within
  this time, `rp` proceeds and emits a warning.

`rp` records the durations and continues with the orchestration. The next
exposure can start immediately after `exposure_complete` — the plugin
processes in parallel.

#### Step 2: Completion (callback POST to `rp`)

When the plugin finishes processing, it POSTs a completion to `rp`:

```
POST /api/plugins/{event_id}/complete
Content-Type: application/json

{
  "status": "complete"
}
```

Or, to request a corrective action:

```json
{
  "status": "complete",
  "correction": {
    "action": "focus",
    "reason": "HFR degraded from 2.3 to 4.8 — likely focus drift",
    "urgency": "immediate"
  }
}
```

- `correction` (optional): requests that the orchestrator perform a
  corrective action (see Corrections below).
  - `action`: the corrective action to take (e.g., `"focus"`,
    `"center"`). Must be a recognized action name.
  - `reason`: human-readable explanation, logged and included in events.
  - `urgency`: either `"immediate"` (abort in-flight operations, reject
    the frame) or `"after_current"` (queue until the current operation
    completes naturally, frame counts normally).

#### Barriers

A plugin can optionally declare **barrier gates** — MCP tools that must
not proceed until the plugin has posted its completion for the most
recent webhook. This tells `rp`: "if you haven't heard back from me yet,
block these tools until you have."

```json
{
  "name": "image-analyzer",
  "webhook_url": "http://localhost:11140/webhook",
  "subscribes_to": ["exposure_complete"],
  "barrier_gates": ["slew", "set_filter"]
}
```

When the orchestrator calls a gated tool, `rp` checks whether any
barrier plugin still has an outstanding (uncompleted) webhook. If so,
`rp` blocks the tool call — up to `max_duration` from the
acknowledgment — before executing. All outstanding plugins are waited on
in parallel.

A plugin with no `barrier_gates` (or an empty list) is never waited on.
Its completion is still processed when it arrives, but `rp` never blocks
on it.

If a barrier plugin completes with a correction while a tool call is
blocked, the gated tool returns the correction to the orchestrator
instead of executing (see Corrections below).

#### Corrections

A plugin can request that the orchestrator perform a corrective action
by including a `correction` in its completion. Corrections have two
urgency levels that determine how `rp` delivers them to the
orchestrator:

**`immediate`** — the current frame is unusable. `rp` aborts any
in-flight operation (e.g., aborts the active camera exposure), returns
the correction to the orchestrator in the aborted tool call's result,
and rejects the frame:

```json
{
  "status": "aborted",
  "correction": {
    "action": "focus",
    "reason": "HFR 4.8, frame unusable",
    "source": "image-analyzer"
  }
}
```

**`after_current`** — the current frame is still usable, but a
corrective action should happen before the next exposure. `rp` queues
the correction and surfaces it in the result of the current in-flight
tool call when it completes naturally:

```json
{
  "image_path": "/data/lights/M31/M31_L_300s_004.fits",
  "document_id": "doc-043",
  "pending_correction": {
    "action": "focus",
    "reason": "HFR 3.0, trending worse",
    "source": "image-analyzer"
  }
}
```

In both cases the orchestrator decides **what to do** with the
correction. `rp` controls **when** the orchestrator hears about it.

**Conflict resolution:** when multiple plugins request corrections,
the most disruptive action wins. If one plugin requests refocus and
another requests recenter, recenter wins because it includes refocusing.

**Frame rejection:** an `immediate` correction implicitly rejects the
frame that triggered the event. `rp`:

1. Does not count the rejected frame toward the exposure goal.
2. Marks the exposure document with the rejection reason.
3. Emits a `frame_rejected` event.

An `after_current` correction does not reject the frame. The current
exposure counts normally.

**Barrier interaction:** when a barrier plugin completes with a
correction while a gated tool call is blocked, `rp` returns the
correction to the orchestrator instead of executing the gated tool.
The orchestrator sees the correction and acts accordingly (e.g.,
refocuses instead of slewing to a new target).

#### Timeout Behavior

When `max_duration` (from the acknowledgment) expires without a
completion:

1. `rp` proceeds as if the plugin completed with `"complete"` and no
   correction.
2. If a tool call was blocked on this barrier, it unblocks and executes
   normally.
3. A `plugin_timeout` warning event is emitted.
4. The timeout is logged.

Webhook delivery failures (connection refused, HTTP errors) are treated
as immediate completion with no correction. Plugins are responsible for
their own reliability.

#### Example: Image Analyzer Flow

Setup: 5 exposures on the same target, 300s each, analysis takes 20s.

```
Exposure 3 completes
  → rp POSTs exposure_complete to analyzer
  → analyzer responds immediately:
      {"estimated_duration": "20s", "max_duration": "30s"}
  → rp records outstanding barrier, starts exposure 4 (not gated)

  Case A — frame OK, no target switch pending:
    → analyzer POSTs completion: {"status": "complete"}
    → rp notes completion, clears barrier
    → capture continues normally

  Case B — frame bad (immediate), exposure 4 in-flight:
    → analyzer POSTs completion:
        {"status": "complete", "correction": {"action": "focus",
         "reason": "HFR 4.8", "urgency": "immediate"}}
    → rp aborts exposure 4, returns capture with:
        {"status": "aborted", "correction": {"action": "focus", ...}}
    → orchestrator refocuses, resumes capture

  Case C — frame marginal (after_current), exposure 4 in-flight:
    → analyzer POSTs completion:
        {"status": "complete", "correction": {"action": "focus",
         "reason": "HFR 3.0, trending", "urgency": "after_current"}}
    → rp queues correction, exposure 4 continues
    → exposure 4 completes, capture returns with:
        {"image_path": "...", "pending_correction": {"action": "focus", ...}}
    → orchestrator refocuses before starting exposure 5

  Case D — frame bad, slew pending (barrier in action):
    → orchestrator calls slew → rp blocks (outstanding barrier)
    → analyzer POSTs completion with immediate correction
    → rp returns slew with correction instead of executing:
        {"status": "blocked_by_correction",
         "correction": {"action": "focus", ...}}
    → orchestrator refocuses, stays on current target
```

### Plugin Section Updates

After processing an event, plugins POST their results back to `rp`:

```
POST /api/documents/{document_id}/sections
Content-Type: application/json

{
  "section_name": "wcs",
  "data": {
    "ra_center": 10.6848,
    "dec_center": 41.2690,
    "pixel_scale_arcsec": 1.05,
    "rotation_deg": 12.3,
    "solver": "astap-0.9.1"
  }
}
```

`rp` merges the section into the document and persists the updated
sidecar JSON.

## Action System

The action system complements the event system. Where events flow outward
from `rp` to plugins (notifications), actions flow inward from plugins to
`rp` (requests). Actions are the primitives that plugins use to control
equipment and trigger computations through `rp`.

The action system uses the
[Model Context Protocol (MCP)](https://modelcontextprotocol.io/) as its
wire protocol. `rp` runs an MCP server that exposes all available actions
as **MCP tools**. Workflow plugins connect as MCP clients to discover and
call tools.

MCP provides:

- **Tool discovery** — `tools/list` returns all available tools with
  JSON Schema parameter definitions.
- **Typed invocation** — `tools/call` with schema-validated parameters
  and structured results.
- **Formal schemas** — every tool's parameters and return types are
  described by JSON Schema, derived from Rust types at compile time
  (via `#[tool]` + `JsonSchema` derives in the `rmcp` crate).
- **Language-agnostic** — plugins can be written in any language with an
  MCP client library (Rust, Python, TypeScript, Go, etc.).

`rp` never exposes raw device access. Every tool validates parameters,
enforces safety constraints, and tracks state before touching hardware.

### MCP Server

`rp` runs a single MCP server using the streamable HTTP transport. This
server exposes all available tools — both built-in and aggregated from
plugin providers (see Plugin-Provided Tools below).

The server endpoint is configurable (default: `http://localhost:11115/mcp`).
Workflow plugins connect to this endpoint as MCP clients during their
active workflow. The orchestrator itself also uses the same tool
implementations internally.

### Tool Catalog

The catalog is built at startup from three sources, all of which appear
identical to MCP clients:

1. **Built-in tools** — implemented directly in `rp` (hardware primitives,
   image analysis, planner, V-curve auto-focus, iterative centering).
2. **rp-managed service tools** — built-in tool surface that proxies to a
   separate process `rp` supervises (guider, plate solver). The MCP tool
   itself lives in `rp`; the wrapped logic runs in the supervised service.
3. **Third-party plugin tools** — aggregated from plugins running their own
   MCP servers. Discovered at startup via `tools/list` and proxied through
   `rp`'s server.

Workflow plugins discover available tools via the standard MCP
`tools/list` call. Each tool includes its JSON Schema, so plugins know
the exact parameter types and return structure.

### Built-in Tools

**Hardware**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `capture` | camera_id, duration, binning | image_path, document_id | Take an exposure, download `image_array`, save FITS file, create exposure document. Carries an **advisory predicted deadline** on `exposure_started`: `predicted = duration + camera.readout_time_estimate` (default 15 s when unset), `max = predicted + 30 s` readout headroom. rp does **not** enforce this (the camera driver owns the exposure); it rides the envelope as `predicted_duration_ms`/`max_duration_ms` for the Sentinel watchdog. rp's own readout backstop (a separate, more generous `duration + 120 s` ceiling) is unchanged |
| `get_camera_info` | camera_id | max_adu, exposure_min, exposure_max, sensor_x, sensor_y, bin_x, bin_y | Read camera capabilities and current settings |
| `move_focuser` | focuser_id, position | actual_position | Move focuser to absolute position (blocks polling `is_moving` until idle). Bounded by a **predicted deadline**: `predicted = \|target − current\| / focuser.steps_per_sec` (current position read before the move); `max = max(predicted × 2, MIN_FOCUSER_DEADLINE = 5 s)`. If the pre-move read fails it falls back to a 120 s ceiling; `predicted`/`max` ride the `move_focuser_started` envelope as `predicted_duration_ms`/`max_duration_ms` |
| `get_focuser_position` | focuser_id | position | Read current focuser position |
| `get_focuser_temperature` | focuser_id | temperature_c | Read focuser temperature sensor |
| `slew` | ra, dec, settle_after (optional) | actual_ra, actual_dec | Slew the singular mount to coordinates (blocks until `Slewing == false` plus configured / per-call settle). Tracking must be on; ASCOM error propagates otherwise. Bounded by a **predicted deadline**: `predicted = great-circle(current, target) / mount.slew_rate_arcsec_per_sec + settle`; `max = max(predicted × 3, MIN_SLEW_DEADLINE = 30 s)`. The current pointing is read before the slew to size the deadline; if that read fails it falls back to a 300 s ceiling. On timeout `slew` best-effort aborts (unlike `park`); `predicted`/`max` ride the `slew_started` envelope as `predicted_duration_ms`/`max_duration_ms` |
| `sync_mount` | ra, dec | — | Sync mount position to given coordinates |
| `get_mount_position` | — | ra, dec | Read the mount's current pointing |
| `get_tracking` | — | tracking, can_set_tracking | Read tracking state and `CanSetTracking` capability; fails loud on read error |
| `set_tracking` | enabled | — | Enable or disable sidereal tracking |
| `park` | — | — | Park the mount (blocks polling `AtPark` every 100 ms until it returns `true`). Bounded by a **predicted deadline**: rp can't read the park position via Alpaca, so it sizes a worst-case full-axis traverse — `predicted = 180° / mount.slew_rate_arcsec_per_sec + settle`; `max = max(predicted × 2, MIN_PARK_DEADLINE = 60 s)` (falls back to a 300 s ceiling with no mount configured); `predicted`/`max` ride the `park_started` envelope. `AtPark` is the ASCOM-canonical completion signal — `Slewing` is sticky on `MoveAxis` rate state and unrelated `SlewState` activity, so polling it would be over-conservative. Per ASCOM, a successful park clears `Tracking`. Unlike `slew`, does NOT auto-abort on timeout — call `abort_slew` to interrupt a stuck park |
| `unpark` | — | — | Clear the mount's `AtPark` flag. Returns immediately. Does NOT auto-enable `Tracking`; call `set_tracking` before slewing |
| `get_park_state` | — | at_park, can_park, can_unpark | Read park state and capabilities; fails loud on `AtPark` read error |
| `abort_slew` | — | — | Abort an in-progress mount slew or park. Per ASCOM, only valid while `Slewing == true`; the natural Alpaca error propagates otherwise |
| `set_filter` | filter_wheel_id, filter_name | — | Change filter wheel position |
| `get_filter` | filter_wheel_id | filter_name, position | Read current filter |
| `close_cover` | calibrator_id | — | Close the dust cover (blocks until closed) |
| `open_cover` | calibrator_id | — | Open the dust cover (blocks until open) |
| `calibrator_on` | calibrator_id, brightness (optional) | — | Turn on flat panel at brightness (0..max_brightness, default max). Blocks until ready |
| `calibrator_off` | calibrator_id | — | Turn off flat panel. Blocks until off |

**Guider**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `start_guiding` | — | rms_ra, rms_dec | Start guiding loop, block until settled |
| `stop_guiding` | — | — | Stop guiding loop, block until confirmed |
| `dither` | pixels | rms_ra, rms_dec | Send dither command, block until settled |
| `pause_guiding` | — | — | Pause guiding (e.g., during readout) |
| `resume_guiding` | — | — | Resume paused guiding |
| `get_guiding_stats` | — | rms_ra, rms_dec, total_rms | Read current guiding statistics |

**Compute (image analysis)**

All image analysis tools accept either `document_id` (resolved via the
[Image and Document Cache](#image-and-document-cache), avoiding FITS
decode) or `image_path` (read from disk via `rp-fits`). Where both are
accepted, `document_id` takes precedence.

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `compute_image_stats` | document_id or image_path | median_adu, mean_adu, min_adu, max_adu, pixel_count | Pixel-level statistics. Implemented. |
| `measure_basic` | document_id or image_path, threshold_sigma (optional) | hfr, star_count, background_mean, background_stddev | Detect stars, compute aggregate HFR and background. **MVP image analysis tool.** |
| `detect_stars` | document_id or image_path, min_area, max_area, threshold_sigma (optional) | stars: \[{x, y, flux, peak, saturated_pixel_count}\], star_count, saturated_star_count, background_mean, background_stddev | Locate stars via thresholded connected-components on background-subtracted pixels. Implemented. |
| `measure_stars` | document_id or image_path, min_area, max_area, threshold_sigma (optional), stamp_half_size (optional) | stars: \[{x, y, hfr, fwhm, eccentricity, flux}\], star_count, median_fwhm, median_hfr, background_mean, background_stddev | Per-star photometry and PSF metrics. Runs `detect_stars` internally; the optional `stars` input from the catalog row is deferred. Implemented. |
| `estimate_background` | document_id or image_path, k (optional), max_iters (optional) | mean, stddev, median, pixel_count (sigma-clipped) | Robust background estimation. Implemented. |
| `compute_snr` | document_id or image_path, min_area, max_area, threshold_sigma (optional) | snr, signal, noise, star_count, background_mean, background_stddev | Median per-star SNR via the CCD-equation approximation. Implemented. |

**Compute (plate solving)**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `plate_solve` | document_id or image_path; optional pointing_hint, use_mount_hints, fov_hint_deg, search_radius_deg, timeout | ra_center, dec_center, pixel_scale_arcsec, rotation_deg, solver | Solve an image. Proxies to the plate-solver rp-managed service (which wraps ASTAP). Persists a `wcs` section to the exposure document. See [`plate_solve` Contract](#plate_solve-contract). |

**Compound (built-in)**

Compound tools drive a multi-step workflow internally using the primitive
built-in tools. They live in `rp`'s process — no MCP hop, no plugin
boundary — but expose the same MCP tool surface as any other tool.

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `auto_focus` | camera_id, focuser_id, duration, step_size, half_width, min_area, max_area, threshold_sigma (optional), min_fit_points (optional) | best_position, best_hfr, final_position, samples_used, curve_points, temperature_c | Parabolic-fit V-curve auto-focus driving `move_focuser` + `capture` + `measure_basic` internally. See [`auto_focus` Contract](#auto_focus-contract). Implemented. |
| `center_on_target` | camera_id, ra, dec, duration, tolerance_arcsec, max_attempts | final_error_arcsec, attempts, final_ra, final_dec, iterations | Iterative `capture` + `plate_solve` + `sync_mount` + `slew` loop until residual ≤ `tolerance_arcsec`. Carries an **advisory outer-loop deadline** on `centering_started`: `per_iter = duration + centering.solve_time_estimate + centering.slew_overhead_estimate`, `predicted = per_iter`, `max = max_attempts × per_iter`. The watchdog tracks only this outer loop; each inner `slew`/`capture` carries its own deadline. See [`center_on_target` Contract](#center_on_target-contract). Implemented. |

**Planner — Ephemeris primitives**

One operation each, backed by the `Ephemeris` trait in
`rp-ephemeris`. Times are humantime ISO-8601 strings (e.g.
`"2026-05-03T22:00:00Z"`); when omitted, defaults to "now". `ra` is
hours, `dec` is degrees. See
[Planning and Ephemeris](#planning-and-ephemeris).

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `resolve_target` | name | ra_hours, dec_degrees, object_type, magnitude, size_arcmin | Catalog lookup against embedded Messier + NGC + IC |
| `compute_alt_az` | ra, dec, time (optional) | altitude_degrees, azimuth_degrees | Topocentric alt/az for an ICRS target |
| `compute_transit` | ra, dec, date (UTC `YYYY-MM-DD`) | transit_utc | UT of upper transit on a given UTC date |
| `compute_rise_set` | ra, dec, date (UTC), min_alt_degrees | rise_utc, set_utc | Rise/set times above a given altitude (null for circumpolar / never-up) |
| `compute_meridian_flip` | ra, dec, time, side_of_pier | time_to_flip_seconds | Time-to-flip from current side of pier (seconds) |
| `get_sun_position` | time (optional) | ra_hours, dec_degrees, altitude_degrees, azimuth_degrees | Sun position |
| `get_twilight` | date (UTC), kind | kind, begin_utc, end_utc | Civil / nautical / astronomical twilight window |
| `get_moon_position` | time (optional) | ra_hours, dec_degrees, altitude_degrees, azimuth_degrees, phase_degrees, illumination_fraction | Moon position + Sun-Moon elongation (phase) |
| `compute_moon_separation` | ra, dec, time (optional) | separation_degrees | Angular separation between target and Moon |
| `get_local_sidereal_time` | time (optional) | lst_hours | Local sidereal time at the configured site |

**Planner — Convenience tools**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `get_next_target` | time (optional) | target, reason, filter, duration_secs | Evaluate candidates and recommend next target. v1 returns `filter`/`duration_secs` as null — see §"Dynamic Planner" |
| `get_target_status` | target_name *or* (ra + dec); time (optional) | target_name, altitude_degrees, azimuth_degrees, hour_angle_hours, time_to_set_seconds, progress | Sky position + progress for a catalog target or raw ICRS coords. `progress` is null in v1 |
| `get_meridian_status` | time (optional) | time_to_flip_seconds, side_of_pier, mount_ra_hours, mount_dec_degrees | Time-to-flip + side-of-pier from the mount's current pointing |
| `record_exposure` | target, filter | completed, goal | Increment counter, return updated progress |
| `get_session_progress` | — | per-target, per-filter progress | Full progress overview |

**Session**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `save_session_state` | — | — | Persist current session state to disk |
| `get_session_state` | — | session state JSON | Read persisted session state |

All built-in tools validate parameters before execution. `move_focuser`
checks position bounds. `capture` checks that the camera is connected and
idle. Invalid requests return an MCP error — they never reach the
hardware.

#### Capture Tool Details

The `capture` tool takes exposure time as a humantime string (`duration`,
e.g. `"500ms"`, `"30s"`, `"1m30s"`).
After the exposure completes and `image_ready` returns true, `capture`
downloads the camera's `image_array`, writes it as a FITS file via
`rp-fits` (BITPIX=16+BZERO=32768 for the common 16-bit sensor case;
BITPIX=32 when `max_adu > u16::MAX`) with `DOC_ID = '<full-uuid>'` in the
primary HDU header, and creates a sidecar exposure document JSON
alongside it. The base filename is `<doc_uuid_8>`; both files share
that base (`<doc_uuid_8>.fits` and `<doc_uuid_8>.json`). Both are
written atomically (stage to a sibling temp file, fsync, rename, fsync
parent directory). See
[Persistence](#persistence) for the full rule set.

**Sidecar failure contract.** If the sidecar write fails after a
successful FITS write, `capture` still returns success with
`image_path` and `document_id` — the FITS file remains on disk and is
the durable record. The cache insert is gated on sidecar success, so
no in-memory entry is created; the disk-fallback resolver also cannot
rehydrate (it requires the sidecar to recover `max_adu` and other
document fields), so subsequent `document_id`-keyed lookups
(`/api/documents/{id}`, `/api/images/{id}`, `/pixels`, image-analysis
tools called with `document_id`) return 404. `rp` emits a
`document_persistence_failed` event carrying `document_id`,
`file_path`, and the error. Subscribers and operators recover by
reading the FITS directly via `file_path` (e.g. image-analysis tools
called with `image_path` instead of `document_id`).

#### CoverCalibrator Tool Details

The CoverCalibrator tools control flat panel devices. `calibrator_on`
accepts an optional `brightness` parameter (0 to `max_brightness`). When
omitted, the calibrator is turned on at maximum brightness. All four
tools block until the operation completes by polling the device state
(same pattern as `set_filter`).

#### Image Statistics Tool Details

`compute_image_stats` computes median, mean, min, and max ADU values
on the captured image. It accepts either a `document_id` (resolved
through the unified image+document cache, falling back to disk scan
on miss) or an `image_path` (read from disk via `rp-fits`); when both
are supplied, `document_id` wins. When called with a `document_id`,
the stats are written into the exposure document as an `image_stats`
section. This tool does not access the camera — it operates on saved
image files.

### Image Analysis Strategy

Image analysis in `rp` follows a **pure Rust on ndarray** approach.
All algorithms are implemented as custom code on top of well-established
building blocks — no single crate covers the full range of astronomical
image analysis needed. Tools accept either a `document_id` (resolved
via the [Image and Document Cache](#image-and-document-cache)) or an
`image_path` (FITS file on disk read via `rp-fits`); `document_id` is
preferred for the post-capture fast path because it avoids re-decoding
the image just written.

#### Current Capabilities

- **Pixel statistics** (median, mean, min, max ADU) — stdlib iterators
  and `select_nth_unstable` for median (iterative O(n) quickselect).
  Used by `compute_image_stats` for flat calibration exposure targeting.
- **FITS I/O** — `rp-fits` workspace crate (reads via `fitsrs`, writes
  via a hand-rolled pure-Rust BITPIX 8/16/32 writer). See ADR-001
  Amendment A.

#### Planned Capabilities and Crate Strategy

| Capability | Approach | Crates |
|------------|----------|--------|
| Pixel statistics | Custom | stdlib (`select_nth_unstable`, iterators) |
| FITS I/O | Crate | `rp-fits` (wraps `fitsrs` for reads, hand-rolled writer) |
| 2D image operations | Crate | `ndarray` (already in workspace) |
| Gaussian smoothing, morphology | Crate | `ndarray-ndimage` (Gaussian filter, dilation/erosion). Connected components is hand-rolled BFS on `Array2<bool>` because `ndarray-ndimage` 0.6's `label` is 3D-only |
| Star detection | Custom | Threshold + connected components on background-subtracted image, then shape filtering |
| Centroiding | Custom | Intensity-weighted center of mass on ndarray subframes |
| HFR / HFD | Custom | Radial flux accumulation (~20 lines of math) |
| FWHM | Custom + crate | 2D Gaussian fitting via `rmpfit` (chosen over `levenberg-marquardt` for native parameter bounds — σ > 0, amplitude > 0 — and lighter dependency footprint: no `nalgebra`. `rmpfit` is also a Rust port of MPFIT, the de-facto astronomy fitting library) |
| Eccentricity / elongation | Custom | Second central moments from detected star pixels |
| Background estimation | Custom | Sigma-clipped mesh statistics on ndarray |
| Noise / SNR | Custom | Sigma-clipped statistics |

#### MVP: `measure_basic` Contract

The first analysis tool to implement. Behavioral contract:

**Input**:
- `document_id` (preferred — resolves to cached pixels) **or** `image_path`
  (FITS file on disk).
- `min_area` — minimum component pixel area to admit as a star. Required;
  no default. The right value depends on the camera+optics pixel scale
  (arcsec/px) and the seeing regime, neither of which the tool can infer
  from the image alone. Callers (workflows, plugins) own that policy.
- `max_area` — maximum component pixel area to admit. Required; no
  default. Same rationale as `min_area`. Note: at extreme defocus,
  donut-shaped PSFs from the secondary obstruction can span many hundreds
  of pixels — auto-focus callers should set `max_area` accordingly so
  the V-curve sweep can measure them.
- Optional `threshold_sigma` (default `5.0`) — detection threshold above
  background. Unit-free (multiples of the sigma-clipped background
  stddev), so a default is meaningful here.

**Output**:
- `hfr` — half-flux radius in pixels, aggregated across detected stars
  (median of per-star HFRs). `null` if no stars detected.
- `star_count` — number of valid stars after detection and filtering.
- `saturated_star_count` — number of detected stars that contain at
  least one pixel at `max_adu`. `0` when `max_adu` is unknown (e.g. when
  called via `image_path` outside an exposure context).
- `background_mean` — sigma-clipped background mean (ADU).
- `background_stddev` — sigma-clipped background standard deviation (ADU).
- `pixel_count` — total pixels analyzed.

**Algorithm (in order)**:
1. Load pixels (image-cache hit or `rp-fits` read).
2. Estimate background via sigma-clipped mean/stddev.
3. Apply Gaussian smoothing (small kernel, σ ≈ 1.0 px) to suppress noise.
4. Threshold at `background_mean + threshold_sigma × background_stddev`.
5. Connected-components labelling on the thresholded mask.
6. Filter components: pixel area in `[min_area, max_area]`; reject
   components touching the image border. Saturated components are *not*
   rejected — they are flagged (see `saturated_star_count`). Saturated
   stars carry real signal: bright in-focus stars routinely clip at
   long-enough exposures, and donut-shaped PSFs at extreme defocus are
   usually saturated in their bright annulus. Filtering them out would
   make HFR-vs-focus non-monotonic and break auto-focus, so the policy
   is to measure them and let downstream consumers decide whether to
   weight or warn.
7. For each surviving component, compute intensity-weighted centroid and
   per-star HFR (radial flux accumulation to half of total flux).
   Centroiding uses background-subtracted flux to avoid bbox-center bias.
8. Return aggregate HFR (median of per-star HFRs), star count,
   saturated-star count, and background.

**Error cases**:
- Neither `document_id` nor `image_path` provided → MCP error mentioning
  `image_path` (the most fundamental missing input).
- `image_path` provided but file not found → MCP error.
- `document_id` provided but neither cache nor FITS-on-disk fallback
  resolves → MCP error.
- `min_area` or `max_area` missing → MCP error naming the missing field.
  These parameters are deserialized as optional and validated by the tool
  body in this order — `document_id`/`image_path` first, then `min_area`,
  then `max_area` — so the error message tracks the first thing the user
  needs to fix.
- Background estimation fails (e.g. all pixels saturated) → MCP error.
- No stars detected → return successfully with `hfr: null`,
  `star_count: 0`, `saturated_star_count: 0`, populated background fields.
  Not an error — the caller decides whether that's a failure (focus run)
  or fine (cloudy frame still useful for stats).

**Persistence**: when called with `document_id`, results are written into
the exposure document as the `image_analysis` section per the rule that
"all tool results that produce image metrics MUST be written into the
exposure document as a section."

#### `estimate_background` Contract

A focused tool that returns sigma-clipped background statistics on their
own — useful for flat-field analysis, sky-quality screening, and any
caller that wants the background number without paying for star detection.

**Input**:
- `document_id` (preferred — resolves to cached pixels) **or** `image_path`
  (FITS file on disk).
- Optional `k` (default `3.0`) — sigma-clip threshold in stddev units.
- Optional `max_iters` (default `5`) — maximum clip iterations.

**Output**:
- `mean` — sigma-clipped background mean (ADU).
- `stddev` — sigma-clipped background standard deviation (ADU).
- `median` — median of the surviving (post-clip) pixel set (ADU).
- `pixel_count` — total pixels analyzed (input area, not the surviving set).

**Algorithm**: same iterative sigma-clip kernel `measure_basic` uses
internally — clip pixels outside `mean ± k × stddev`, recompute, repeat
until the surviving set stops shrinking or `max_iters` runs out. Median
is taken over the surviving set via `select_nth_unstable`.

**Error cases**:
- Neither `document_id` nor `image_path` provided → MCP error mentioning
  `image_path` (consistent with `measure_basic`).
- `image_path` provided but file not found → MCP error.
- `document_id` provided but neither cache nor FITS fallback resolves →
  MCP error.
- `k <= 0` or `max_iters == 0` → MCP error naming the bad parameter.
- Background estimation fails (e.g. all pixels clipped, empty image) →
  MCP error.

**Persistence**: when called with `document_id`, results are written into
the exposure document as the `background` section. Separate from
`measure_basic`'s `image_analysis` section so the two tools don't
overwrite each other on the same document.

#### `detect_stars` Contract

Returns the per-star list `measure_basic` produces internally — useful for
callers that want star coordinates and fluxes without HFR (centering,
quality screens, custom plate-solver hints). Also persists the list so
follow-up tools (`measure_stars`) can skip re-detection on the same
exposure.

**Input**:
- `document_id` (preferred — resolves to cached pixels) **or** `image_path`
  (FITS file on disk).
- `min_area` and `max_area` — required. Pixel area encodes a pixel-scale
  (arcsec/px) assumption that the tool cannot infer; same rationale as
  `measure_basic` (no defaults).
- Optional `threshold_sigma` (default `5.0`) — detection threshold above
  background, in stddev units.

**Output**:
- `stars` — array of `{x, y, flux, peak, saturated_pixel_count}` objects:
  - `x` / `y` — flux-weighted centroid (pixel coordinates).
  - `flux` — sum of background-subtracted, non-negative flux over the
    component (ADU).
  - `peak` — maximum *raw* pixel value over the component (ADU, not
    background-subtracted). Useful for saturation awareness.
  - `saturated_pixel_count` — pixels at or above the camera's `max_adu`.
    Always `0` when `max_adu` is unknown (bare `image_path` mode).
- `star_count` — convenience aggregate (`stars.length`).
- `saturated_star_count` — count of stars with `saturated_pixel_count > 0`.
- `background_mean` / `background_stddev` — sigma-clipped background used
  to set the detection threshold; included so callers know what cut was
  effectively applied.

**Algorithm**: same pipeline `measure_basic` runs internally — sigma-
clipped background → Gaussian smoothing (σ ≈ 1 px) → threshold at
`mean + threshold_sigma × stddev` → 4-connectivity BFS → area / border
filter → intensity-weighted centroiding. Saturated components are
flagged, not rejected (same rationale as `measure_basic`).

**Error cases**:
- Neither `document_id` nor `image_path` → MCP error mentioning
  `image_path`.
- `min_area` or `max_area` missing → MCP error naming the missing
  parameter (validated in body for deterministic error ordering, same as
  `measure_basic`).
- `image_path` provided but file not found → MCP error.
- `document_id` provided but neither cache nor FITS fallback resolves →
  MCP error.
- Background estimation fails (e.g. empty image) → MCP error.

**Persistence**: when called with `document_id`, the JSON payload is
written to the `detected_stars` section. Separate from `image_analysis`
(measure_basic) and `background` (estimate_background) so all three tools
can run on the same exposure without overwriting each other.

#### `measure_stars` Contract

Per-star photometry and PSF metrics for callers that need FWHM and
eccentricity (auto-focus, guider error budgeting, image-quality screens)
in addition to the HFR / flux that `measure_basic` aggregates.

**Input**:
- `document_id` (preferred — resolves to cached pixels) **or** `image_path`
  (FITS file on disk).
- `min_area` and `max_area` — required (encode pixel-scale assumptions;
  same rationale as `measure_basic` and `detect_stars`).
- Optional `threshold_sigma` (default `5.0`) — detection threshold.
- Optional `stamp_half_size` (default `8`) — half-side of the postage
  stamp used for the 2D Gaussian fit. The fit is rejected for any star
  whose stamp would cross the image boundary.

**Output**:
- `stars` — array of `{x, y, hfr, fwhm, eccentricity, flux}` objects:
  - `x` / `y` — flux-weighted centroid (pixel coordinates).
  - `hfr` — empirical half-flux radius (pixels), or `null` when no
    positive flux above background (rare; `detect_stars` already filters
    this out).
  - `fwhm` — geometric-mean FWHM = 2.3548·√(σx·σy) from the Gaussian
    fit (pixels), or `null` when the fit fails.
  - `eccentricity` — √(1 − (σmin/σmax)²) from the Gaussian fit, or
    `null` when the fit fails.
  - `flux` — sum of background-subtracted, non-negative flux (ADU).
- `star_count` — total stars detected (including those whose fit failed).
- `median_fwhm` — median across stars whose fit succeeded; `null` when
  no fits converged.
- `median_hfr` — median empirical HFR; `null` when no stars detected.
- `background_mean` / `background_stddev` — sigma-clipped background.

**Algorithm**:
1. Sigma-clipped background → `detect_stars` (same pipeline as
   `measure_basic` and `detect_stars`).
2. For each detected star:
   - Empirical HFR over the connected-component pixels (same kernel
     `measure_basic` aggregates).
   - 2D Gaussian fit on a `(2·stamp_half_size+1)²` postage stamp using
     `rmpfit` (Levenberg-Marquardt). Model:
     `I(x, y) = A · exp(−((x−x0)²/(2σx²) + (y−y0)²/(2σy²))) + B`.
     6 free parameters; no rotation (rationale: amateur PSFs rarely
     resolve a meaningful θ at typical pixel scales — geometric-mean
     FWHM and eccentricity capture quality without it).
3. Stars with failed fits keep their row with `fwhm`/`eccentricity` set
   to `null`. They are *not* dropped — the caller decides whether the
   frame is usable.

**Error cases**:
- Neither `document_id` nor `image_path` → MCP error mentioning
  `image_path`.
- `min_area` or `max_area` missing → MCP error naming the missing
  parameter.
- `image_path` provided but file not found → MCP error.
- `document_id` provided but neither cache nor FITS fallback resolves →
  MCP error.
- Background estimation fails (e.g. empty image) → MCP error.

**Persistence**: when called with `document_id`, the JSON payload is
written to the `measured_stars` section. Distinct from `detected_stars`,
`image_analysis`, and `background` so all four tools coexist on one
document.

**Deferred**: the optional `stars` input listed in the tool catalog row
is not implemented in this MVP. When implemented it will let the caller
pass back the array from a previous `detect_stars` call to skip
re-detection; for now, every invocation re-runs detection.

#### `compute_snr` Contract

A signal-to-noise summary across detected stars — the headline number
that quality-screening workflows use to decide whether to keep a frame.

**Input**:
- `document_id` (preferred — resolves to cached pixels) **or** `image_path`
  (FITS file on disk).
- `min_area` and `max_area` — required (encode pixel-scale assumptions;
  same rationale as `measure_basic`, `detect_stars`, and `measure_stars`).
- Optional `threshold_sigma` (default `5.0`) — detection threshold.

**Output**:
- `snr` — median per-star signal-to-noise ratio. `null` when no stars
  are detected.
- `signal` — median per-star background-subtracted total flux (ADU).
  `null` when no stars are detected.
- `noise` — median per-star noise (ADU). `null` when no stars are
  detected.
- `star_count` — number of stars contributing to the medians.
- `background_mean` / `background_stddev` — sigma-clipped background
  used in the noise model.

**Algorithm**: sigma-clipped background → `detect_stars` → for each
star, `signal = total_flux`, `noise = √(signal + N_pix · σ_bg²)`,
`snr = signal / noise`. The aggregate uses the median for robustness
against outliers (saturated stars, hot-pixel spikes).

**Caveats** (kept honest because SNR numbers are easy to misread):
- The noise model collapses dark current and read-noise into the
  background variance and assumes gain ≈ 1 ADU/electron. SNR values are
  comparable across frames from the *same camera*, **not** absolute
  photometric SNRs. Cross-camera comparisons need per-camera gain and
  read-noise inputs that this MVP does not surface.
- Saturated stars are *included* in the median, the same way
  `measure_basic` includes them. Their effective signal is clipped, so
  they bias the median low; aggressive callers can pre-filter via
  `detect_stars` and call `compute_snr` on a subset (deferred — the
  optional `stars` input from `measure_stars` will land here too).

**Error cases**:
- Neither `document_id` nor `image_path` → MCP error mentioning
  `image_path`.
- `min_area` or `max_area` missing → MCP error naming the missing
  parameter.
- `image_path` provided but file not found → MCP error.
- `document_id` provided but neither cache nor FITS fallback resolves →
  MCP error.
- Background estimation fails (e.g. empty image) → MCP error.

**Persistence**: when called with `document_id`, the JSON payload is
written to the `snr` section. Distinct from `detected_stars`,
`measured_stars`, `image_analysis`, and `background` so all five
imaging tools coexist on one document.

#### Design Rationale

This approach follows what N.I.N.A. does: custom astronomical algorithms
on top of general-purpose image processing primitives. The algorithms
(HFR, centroiding, eccentricity) are well-documented and not complex.
SEP (Source Extractor as a library) was considered via `sep-sys` but
rejected due to LGPL license constraints and C FFI maintenance burden.

### Image and Document Cache

The cache is a **first-class API** holding both pixel data and the
exposure document for each capture, evicted as a unit. It serves
built-in tools (in-process, zero-copy), rp-managed services, and
third-party plugins (over HTTP), and eliminates redundant FITS or
sidecar reads for the common post-capture flow where a tool wants to
analyze the image just captured.

When `capture` completes, the camera's pixel array is already decoded
in memory and the document has just been constructed. The cache holds
both so subsequent tools (`measure_basic`, the next iteration of
`auto_focus`, an external analyzer plugin) and document-API consumers
don't re-read from disk. The on-disk FITS+sidecar pair remains the
durable source of truth — the cache is strictly a hot-path
optimization, with the disk as fallback on miss.

Pixels and document share one cache entry. They evict together. Tool
calls that mutate the document (e.g. `measure_basic` writing the
`image_analysis` section) update the cached document under a per-entry
lock and persist the sidecar atomically. After eviction, both the
pixels and the document are gone from memory; either can be rehydrated
from disk on the next access — see
[Document Resolution](#document-resolution).

#### Internal API (built-in tools)

```rust
pub enum CachedPixels {
    U16(Array2<u16>),
    I32(Array2<i32>),
}

pub struct CachedImage {
    pub pixels: CachedPixels,
    pub width: u32,
    pub height: u32,
    pub fits_path: PathBuf,
    pub max_adu: u32,
    pub document: RwLock<ExposureDocument>,
}

ImageCache::insert(document_id: &str, image: CachedImage);
ImageCache::get(document_id: &str) -> Option<Arc<CachedImage>>;
ImageCache::put_section(document_id: &str, name: &str, value: Value)
    -> Result<()>;  // mutates the cached document AND persists sidecar
```

Built-in tools and HTTP handlers that accept a `document_id` resolve
through the cache. On miss, the cache attempts to rehydrate from disk
before returning `None`. Cache misses are logged at `debug!` for
tuning visibility.

#### Document Resolution

A `document_id` (the full UUID) resolves through this order:

1. **Cache hit.** Return the cached entry. O(1).
2. **Disk fallback.** `readdir` `<data_directory>` filtering for
   filenames matching `*_<uuid[..8]>.fits`. For each candidate, verify
   by reading the FITS header `DOC_ID` against the requested full
   UUID. The sidecar's `id` field is the fallback authority if the
   FITS is unreadable. On match, read both files, populate the cache,
   and return the entry.
3. **Not found.** Return `None`. The HTTP API returns `404`.

Ghost-match disambiguation runs only when multiple files in the data
directory share an 8-char suffix. With UUID v4 entropy, the expected
number of ghost matches per query is `k/N` where `k` is the total
captures on disk and `N = 2^32`. At 100,000 captures, that's ~2·10⁻⁵
— the disambiguation path exists for correctness but in practice
essentially never fires.

If `<data_directory>` is changed at runtime (rare), entries captured
under the old directory become unreachable by id even though their
files remain on disk. This is intentional — the data directory is a
contract, not an indexed pool. Operators wanting to bring old captures
back into reach copy or move the relevant FITS+sidecar pairs into the
current `<data_directory>`.

#### Storage Type Selection (u16 vs i32)

The cache primarily stores **`u16`**. All current consumer/prosumer
astro cameras (ZWO ASI series, QHY, Atik, Moravian, SBIG) emit
non-negative pixel values within the 16-bit range — CCDs are uniformly
16-bit; CMOS is 12-, 14-, or 16-bit ADC; sensor output is a
photoelectron count, physically non-negative. Storing `u16` halves
cache memory and `/pixels` bandwidth versus `i32` at no information
loss for any camera in this category.

The `CachedPixels::I32` variant exists so the structure can accept
future scientific cameras (Andor, Hamamatsu sCMOS HDR modes, etc.)
that genuinely emit values outside `u16` range, without a refactor.

Selection policy at `capture` time:

- Read `max_adu` from the cached `CameraEntry.max_adu` populated by
  `connect_camera` (one Alpaca round-trip per connection, not per
  exposure — see "Tenet 1: don't re-fetch invariant data"). The
  cached value drives both the cache variant choice and the `max_adu`
  field on the resulting `ExposureDocument`.
- If `max_adu ≤ 65535`: narrow the i32 array returned by
  `ascom-alpaca` to `u16` and store as `CachedPixels::U16`. The
  narrow clamps to `[0, max_adu]` before casting — guards against a
  buggy driver returning out-of-range values.
- Otherwise: store as `CachedPixels::I32` unchanged.
- If `max_adu` is `None` on the entry (connect-time read failed): skip
  the cache insert and persist `max_adu: None` on the document. The
  FITS file plus the sidecar remain the durable record. The next
  reconnect re-reads independently.

The decision is per-capture in mechanism (consulted at every
exposure) but the underlying value is per-connection — `MaxADU` is a
physical property of the sensor and cannot change while the device
stays connected. A connect-time read failure therefore degrades the
whole session for that camera until reconnect, in exchange for cutting
five Alpaca property round-trips out of every capture (mitigates the
load pattern that triggers OmniSim's per-capture `GC.Collect` ↔
telescope-timer-thread race).

On disk-fallback rehydration (cache miss, document/pixels read from
the FITS+sidecar pair), the variant choice comes from the sidecar's
`max_adu` field — no live camera required. If the sidecar's
`max_adu` is null (capture-time read failed), the rehydration falls
back to serving from disk for each request rather than caching.

Analysis code is generic over the pixel type via a small trait
(e.g. `Pixel: Copy + Into<i64> + ...`) implemented for both `u16` and
`i32`. Each algorithm is written once, monomorphized for both types.
Tools dispatch:

```rust
match &cached.pixels {
    CachedPixels::U16(arr) => measure_basic_impl(arr.view()),
    CachedPixels::I32(arr) => measure_basic_impl(arr.view()),
}
```

FITS writes preserve the cache pixel type: 16-bit sensors land on
disk as BITPIX=16+BZERO=32768 (half the byte cost of the previous
BITPIX=32 widening); cameras with `max_adu > u16::MAX` fall through
to BITPIX=32 (lossless). Reads always normalise to `i32` — the
imaging pipeline is uniform regardless of on-disk bit depth. The
ASCOM `ImageArray` interface contract — which mandates `Int32` — is
honored at any point we surface pixels through that API; internally
we use `u16` whenever possible.

#### HTTP API (services and plugins)

| Endpoint | Returns | Description |
|----------|---------|-------------|
| `GET /api/documents/{document_id}` | JSON | Full exposure document with all sections. Resolves through the cache (hit → return; miss → disk fallback; not found → 404). See [Document Resolution](#document-resolution). |
| `POST /api/documents/{document_id}/sections` | — | Plugin section update. Requires the document be resolvable; persists the sidecar atomically and updates the cached entry. |
| `GET /api/images/{document_id}` | JSON metadata | Width, height, bitpix, FITS path, exposure document link, in-cache flag. Resolves through the same cache + disk fallback. |
| `GET /api/images/{document_id}/pixels` | `application/imagebytes` | Raw pixel data in [ASCOM Alpaca ImageBytes](https://ascom-standards.org/api/) format: 44-byte header (metadata version, error number, transaction IDs, data offset, image element type, transmission element type, rank, dimensions) followed by little-endian pixel bytes. |

Symmetry: `/pixels` serves the same wire format Alpaca cameras produce
upstream. A plugin that already speaks Alpaca can reuse its existing
ImageBytes parser unchanged.

There is deliberately **no FITS endpoint**. Consumers that genuinely
need FITS-formatted bytes (typically the plate-solver service, since
ASTAP and astrometry.net are FITS-native) read the file directly from
the path in the exposure document — `rp` and its plugins/services are
assumed to share a filesystem (see [File Accessibility](#file-accessibility)).
HTTP-proxying a file consumers can already open is unnecessary overhead.

#### Lifetime and Eviction

- **Insertion**: on `capture` completion, after the FITS+sidecar pair
  is written. The cache holds the pixel buffer that came from the
  camera plus the freshly-constructed document — no re-decode or
  re-parse at insert time.
- **Eviction**: LRU. Pixels and document are evicted together as a
  unit. Two configurable budgets, whichever trips first:
  ```json
  "imaging": {
    "cache_max_mib": 1024,
    "cache_max_images": 8
  }
  ```
  `cache_max_mib` bounds the **combined** memory footprint of pixels
  and serialized document JSON for each entry. Document size is not
  negligible — analysis sections like `detect_stars` and
  `measure_stars` carry per-star arrays that can run into tens of KB
  per section. `cache_max_images` is a safety net against
  misconfiguration. Defaults are sized for an 8 GB Pi 5; tune for
  larger hosts.
- **Fallback**: cache miss is not an error. Tools and the
  document/image HTTP endpoints fall back to the on-disk pair via
  [Document Resolution](#document-resolution). After successful
  rehydration the entry is re-inserted into the cache.
- **Durability**: a document remains accessible by id as long as its
  FITS+sidecar pair sits in `<data_directory>`, regardless of cache
  state or `rp` restart history. The contract is "live as long as the
  file is on disk", not "live as long as `rp` is up".

#### Wire Format Choice

ImageBytes was chosen over a custom format or NumPy `.npy` because:
- It's the format the camera already produced; same parser code is
  reusable by plugins that already consume Alpaca devices directly.
- The 44-byte header carries everything we need (rank, dimensions,
  element type) without ad-hoc HTTP headers.
- It's a published ASCOM standard — no rp-specific format to document.
- It's **type-tagged**, which lets the `/pixels` endpoint honestly
  reflect the cached storage type in the header
  (`ImageElementType=UInt16` for `CachedPixels::U16`,
  `ImageElementType=Int32` for `CachedPixels::I32`). Consumers parse
  the header and handle the type — no client-side assumption baked
  in. This means a future Andor / Hamamatsu integration that bumps
  the cache to `I32` for those frames is a transparent wire change,
  not an API break.

### Plugin-Provided Tools

Tool-provider plugins extend the catalog with tools `rp` does not ship
built-in. A plugin runs its own MCP server. At startup, `rp` connects to
each tool-providing plugin as an MCP client, discovers their tools via
`tools/list`, and proxies them through its own MCP server. Orchestrators
and other clients see a single unified catalog — they don't know or care
whether a tool is built-in, an rp-managed service proxy, or a plugin
contribution.

Tool-provider plugins are typically third-party: experimental algorithms,
ML-based analyzers, alternative implementations of an existing tool that
a specific deployment wants to substitute alongside the built-in, or
anything written in a non-Rust language. Stable astronomy primitives
(HFR, FWHM, eccentricity, V-curve focus, iterative centering, plate-solve
proxy) ship as built-ins and are the default. A plugin may shadow any
built-in tool by advertising the same tool name; see
[Config-Time Validation](#config-time-validation) and
[Third-party alternatives](#third-party-alternatives).

(Orchestrator plugins like `calibrator-flats` are also "plugins" in the
protocol sense, but they don't *provide* tools — they *consume* them.
They are covered separately under [Plugin Types](#plugin-types).)

```
┌─────────────────┐  tools/list   ┌──────────────────┐
│  star-analyzer   ├─────────────►│                  │
│  (MCP server)    │              │       rp         │  tools/list + tools/call
│  measure_eccen.. │◄─────────────┤  (MCP server +   ├──────────────────────────►  workflow plugins
└─────────────────┘  tools/call   │   MCP client)    │                             (MCP clients)
                                  │                  │
┌─────────────────┐  tools/list   │  Aggregates all  │
│  wavefront-anlzr ├─────────────►│  tools into one  │
│  (MCP server)    │              │  unified catalog  │
│  measure_wavefr..│◄─────────────┤                  │
└─────────────────┘  tools/call   └──────────────────┘
```

Examples of genuinely third-party-shaped plugins (none of these ship
with `rp`):

| Tool | Provider | Description |
|------|----------|-------------|
| `classify_image_quality` | ml-quality-classifier | ML model that scores frames as keep/reject |
| `detect_diffraction_pattern` | bahtinov-mask-helper | Specialized analyzer for Bahtinov / tri-Bahtinov focus aids |
| `measure_wavefront` | wavefront-analyzer | Optical aberration analysis from defocused star images |
| `score_field_flatness` | tilt-analyzer | Detect sensor tilt by per-quadrant HFR comparison |

**All tool results that produce image metrics MUST be written into the
exposure document as a section.** This is the one rule — the document is
the shared data bus. `rp` enforces this: compute tool results are merged
into the document before being returned to the caller.

### Plugin Types

Plugins are separate processes following the plugin protocol. Some are
first-party (workflow plugins shipping in this workspace, like
`calibrator-flats`); others are third-party extensions. Three plugin
types by role:

| Type | Role | Interface | Typical authorship |
|------|------|-----------|-------------------|
| **Event** | React to events asynchronously | Webhook (receive events, post completion) | Either |
| **Tool provider** | Add tools beyond `rp`'s built-in catalog | MCP server (rp aggregates their tools) | Mostly third-party |
| **Orchestrator** | Drive the imaging session | MCP client (calls tools on rp) | Mostly first-party (`calibrator-flats`, future `deep-sky-orchestrator`, `sky-flat`, `planetary-orchestrator`) |

A plugin can combine types. For example, a focus plugin can be a
**tool provider** (exposes `auto_focus` tool) and also an **event
plugin** (subscribes to `temperature_changed` to track focus drift).

#### Tool Provider Registration

Tool providers run their own MCP servers. `rp` connects at startup,
discovers their tools, and proxies them through its own MCP server:

```json
{
  "name": "ml-quality-classifier",
  "type": "tool_provider",
  "mcp_server_url": "http://localhost:11150/mcp",
  "requires_tools": ["compute_image_stats"]
}
```

The `requires_tools` field is for config-time validation only — `rp`
checks that all required tools exist in the catalog before starting.
At runtime, the plugin can call any tool on `rp`.

#### Orchestrator Registration

Exactly one orchestrator plugin is configured per session type. `rp`
invokes it when a session starts:

```json
{
  "name": "deep-sky-orchestrator",
  "type": "orchestrator",
  "invoke_url": "http://localhost:11160/invoke",
  "requires_tools": ["slew", "capture", "start_guiding", "stop_guiding",
                      "dither", "get_next_target", "record_exposure"]
}
```

#### Orchestrator Invocation Protocol

**Step 1: Invocation.** When a session starts, `rp` POSTs to the
orchestrator's `invoke_url`:

```
POST <invoke_url>
Content-Type: application/json

{
  "workflow_id": "wf-550e8400-e29b-41d4",
  "session_id": "session-2026-03-01",
  "mcp_server_url": "http://localhost:11115/mcp",
  "recovery": null
}
```

On recovery after a safety event or power failure, `recovery` contains
the last known session state so the orchestrator can resume.

The orchestrator acknowledges with timing estimates computed from the
session context it just received:

```json
{
  "estimated_duration": "8h",
  "max_duration": "0s"
}
```

- `estimated_duration`: humantime string for how long the orchestrator
  expects the workflow to take. Used for UI display and logging.
- `max_duration`: hard timeout. If the orchestrator doesn't complete
  within this time, `rp` cancels it and moves equipment to a safe state.
  `"0s"` means no timeout — the orchestrator runs until it completes,
  the user stops the session, or a safety event occurs.

These values are dynamic, not static config — the orchestrator
computes them at invocation time based on the session it receives.
This follows the same pattern as event plugin acknowledgments.

A deep-sky orchestrator returns `max_duration: "0s"` because it
runs all night. A flat calibration orchestrator computes a meaningful
timeout based on the work it needs to do:

```
rp invokes flat-calibration orchestrator with session context
  → orchestrator inspects session: 4 filters × 20 flats × ~2s each
  → orchestrator acknowledges:
      {"estimated_duration": "5m", "max_duration": "10m"}
  → if orchestrator hangs, rp kills it after 600s — not after an
    hour-long static ceiling that wastes a time-critical dusk window
```

**Step 2: Tool calls via MCP.** The orchestrator connects to `rp`'s
MCP server and drives the session using standard MCP tool calls. It
can call any tool — built-in or plugin-provided. See the Orchestration
section for a full example flow.

**Step 3: Completion.** When the orchestrator finishes (all targets
done, dawn approaching, or explicit stop):

```
POST /api/plugins/{workflow_id}/complete
Content-Type: application/json

{
  "status": "complete",
  "result": {
    "reason": "all_targets_complete",
    "exposures_captured": 87
  }
}
```

#### Example: ML Quality Classifier (third-party tool provider)

A third party ships an ML model that scores frames as keep/reject. It
runs as a separate process, exposes one tool, and reads pixels from
the image cache:

```
Orchestrator calls: tools/call classify_image_quality {document_id: "doc-042"}
  → rp proxies to ml-quality-classifier's MCP server

  ml-quality-classifier (in its own process):
    → GET /api/images/doc-042/pixels  (Alpaca ImageBytes)
    ← raw pixel bytes
    → runs inference locally
    → POST /api/documents/doc-042/sections {section_name: "ml_quality", data: {...}}

  ml-quality-classifier returns to rp:
    ← {score: 0.87, classification: "keep", model: "psf-cnn-v3"}

  rp returns to orchestrator:
    ← {score: 0.87, classification: "keep", model: "psf-cnn-v3"}
```

The plugin reuses `rp`'s image cache HTTP API for pixel access (no FITS
re-decode), and writes its results back into the exposure document via
the section endpoint. Built-in compound tools (`auto_focus`,
`center_on_target`) follow the same orchestration pattern but without
the MCP-over-HTTP hop — see [Compound Tools](#compound-tools).

### Safety Guardrails

There is no per-workflow scoping — any workflow plugin can call any tool
in the catalog. Safety is enforced at the tool level, universally:

- **Parameter validation**: focuser position within min/max bounds,
  exposure duration within configured limits, slew coordinates above
  horizon.
- **State validation**: cannot capture while another capture is in
  progress on the same camera, cannot slew during an exposure.
- **Timeout**: if `max_duration` expires without completion, `rp`
  cancels the workflow, moves equipment to a safe state, and proceeds
  with the next orchestration phase. The MCP session is terminated.
- **Safety override**: a safety event (unsafe transition) immediately
  cancels any active workflow. The MCP session is terminated — the
  plugin's next tool call returns an error indicating the workflow was
  cancelled.

### Config-Time Validation

At startup, `rp` validates the full plugin dependency graph:

1. Connect to each tool-providing plugin's MCP server and discover
   their tools via `tools/list`.
2. Build the unified tool catalog from built-in tools and all
   discovered plugin-provided tools. If a plugin advertises a tool
   whose name matches a built-in, the plugin **shadows** the built-in —
   `rp` routes calls to the plugin and emits an `info!` log line at
   startup naming the shadowed built-in and the shadowing plugin. Two
   *plugins* advertising the same tool name is still a hard error
   (`rp` refuses to start) — there's no deterministic precedence
   between two plugins.
3. For each plugin with `requires_tools`, verify that every listed
   tool exists in the catalog (post-shadow).
4. If validation fails, `rp` refuses to start and reports the missing
   or conflicting tools.

Shadowing exists so a deployment can swap any built-in algorithm
(`auto_focus`, `center_on_target`, image-analysis tools) for a
locally-developed alternative without forking `rp` or renaming the
tool in the orchestrator's call sites. It is an opt-in: shadowing
only happens when the plugin is configured. The default deployment
runs the built-ins.

This ensures the system is fully configured before the session begins.
A missing dependency is a startup error, not a 3 AM surprise.

## Equipment Integration

### ASCOM Alpaca Devices

All devices with an Alpaca interface are accessed exclusively via ASCOM Alpaca
HTTP API. `rp` is an Alpaca client, not a server. Equipment is
configured in the JSON config file — no discovery protocol is used.

Supported ASCOM device types:

| Device Type | Usage |
|-------------|-------|
| Camera | Exposure control (start, abort, readout status, cooler) |
| Telescope (mount) | Slew, track, park, unpark, side of pier, meridian flip |
| Focuser | Absolute/relative move, temperature readout |
| FilterWheel | Filter selection by position |
| SafetyMonitor | Safety state polling |
| CoverCalibrator | Dust cover control (open, close) and flat panel control (on, off, brightness) |

**Mount site properties.** On telescope connect, `rp` reads
`SiteLatitude` and `SiteLongitude` to validate the configured `site`
block. A mismatch greater than `0.01°` in either dimension is a
hard error; if either read fails (typically `NOT_IMPLEMENTED` from
a mount that does not expose the property), the validation is
skipped with a `debug!()` log. See
[Site Validation Against the ASCOM Mount](#site-validation-against-the-ascom-mount).

### Guider Service

The guider service is an **rp-managed service** that wraps PHD2 and
exposes an HTTP API to `rp`. The existing `phd2-guider` library provides
the PHD2 JSON-RPC integration and will be reworked to run as an HTTP
service. Like the plate solver, it is a separate process because PHD2
itself is an external program with its own crash/restart behavior;
Sentinel can supervise and restart it via the standard rp-managed-service
flow.

PHD2 uses JSON-RPC over TCP, which is the one exception to the Alpaca-only
rule — there is no Alpaca guider device type. The guider service encapsulates
this protocol so `rp` speaks only HTTP.

Guider operations are exposed as built-in MCP tools (`start_guiding`,
`stop_guiding`, `dither`, `pause_guiding`, `resume_guiding`,
`get_guiding_stats`). `rp` proxies these tool calls to the guider service's
HTTP API. This means workflow plugins (e.g., a meridian flip plugin) can
control guiding through the same MCP tool mechanism as any other equipment.
Swapping in a different guiding backend requires only a different guider
service that implements the same HTTP endpoints.

### Plate Solver

The plate solver is an **rp-managed service** — a separate process that
wraps the operator-installed ASTAP CLI binary. The MCP tool surface
(`plate_solve`) is a built-in tool that proxies to the service; ASTAP
lives in the supervised wrapper process.

This shape (service rather than built-in Rust code) is chosen because:
- ASTAP is an external program `rp` cannot link against.
- It can hang or crash independently of `rp`.
- Sentinel can restart the wrapper via the standard rp-managed-service
  supervision flow (see [Sentinel Watchdog Integration](#sentinel-watchdog-integration)).

The plate solver can also subscribe to `exposure_complete` events for
background solving (deferred to v2; v1 is request/response only).

The choice of solver and the supervision posture are settled by
[ADR-005](../decisions/005-plate-solver.md). The service's own design
doc — HTTP contract, supervision contract, configuration, mock test
double — lives at [`docs/services/plate-solver.md`](plate-solver.md).
Implementation sequencing is in
[`docs/plans/archive/plate-solver.md`](../plans/archive/plate-solver.md).

### File Accessibility

Plugins and `rp` are assumed to share a filesystem (local paths
work). Distributed deployments where plugins run on separate machines are a
future concern and out of scope for the initial design.

## Orchestration

`rp` does not contain workflow logic. The imaging workflow — what to do,
in what order, and when to switch targets — is driven by an
**orchestrator plugin** that connects to `rp`'s MCP server and calls
tools.

Different imaging types use different orchestrators:

| Orchestrator | Workflow |
|-------------|----------|
| `deep-sky-orchestrator` | slew → center → focus → guide → capture loop, with dithering, meridian flips, target switching |
| `planetary-orchestrator` | slew → focus → high-fps capture, no guiding or plate solving |
| `calibrator-flats` | close cover → calibrator on → per-filter: find exposure time iteratively → capture N flats → calibrator off → open cover |
| `sky-flat` | point at clear sky → per-filter during twilight: capture with per-frame exposure adjustment → handle changing sky brightness |

### What `rp` Owns vs. What the Orchestrator Owns

**`rp` owns** (enforced regardless of which orchestrator runs):

- **MCP tool server** — all equipment, guider, compute, planner, and
  session tools.
- **Event bus** — emits events to webhook subscribers and the real-time
  stream.
- **Safety enforcement** — polls SafetyMonitors. On an unsafe
  transition, `rp` cancels the active orchestrator workflow, aborts
  exposures, stops guiding, parks the mount, and persists session state.
  The orchestrator cannot prevent or delay this.
- **Session persistence** — provides tools for saving and loading
  session state. Also persists automatically on safety events.

**The orchestrator owns** (implemented as plugin logic):

- **Workflow state machine** — the sequence of operations (slew, center,
  focus, guide, capture, dither, meridian flip, etc.).
- **Capture loop** — deciding when to start/stop exposures, managing
  multi-camera coordination, barrier synchronization.
- **Conditional logic** — when to refocus (temperature drift, HFR
  degradation), when to take flats, how to handle meridian flips.
- **Sub-workflow delegation** — the orchestrator can call compound tools
  provided by other plugins (e.g., `auto_focus`, `center_on_target`)
  or implement sub-workflows directly using primitive tools.

### Orchestrator Lifecycle

```
rp starts
  → validates config, connects to equipment
  → builds MCP tool catalog (built-in + plugin-provided)
  → starts MCP server, event bus, safety polling
  → waits for session start command (from UI or API)

User starts session via API
  → rp invokes the configured orchestrator plugin
  → orchestrator connects to rp's MCP server
  → orchestrator drives the session using tool calls
  → rp emits events as tools execute (exposure_started, slew_complete, etc.)

Safety event (unsafe transition)
  → rp immediately: aborts exposures, stops guiding, parks mount
  → rp terminates the orchestrator's MCP session
  → rp persists session state
  → on safe transition: rp re-invokes the orchestrator with recovery context

Session ends (orchestrator completes, user stops, or dawn)
  → orchestrator disconnects from MCP
  → rp persists final session state
  → rp emits session_stopped event
```

### Example: Deep-Sky Orchestrator Flow

The deep-sky orchestrator implements the classic imaging workflow. This
is what a typical orchestrator looks like — it's a program that calls
tools:

```
Orchestrator connects to rp MCP server

Loop:
  → tools/call get_next_target {}
  ← {name: "M31", ra: 10.6847, dec: 41.2689, filter: "Luminance", ...}

  → tools/call slew {ra: 10.6847, dec: 41.2689}
  ← {actual_ra: 10.6845, actual_dec: 41.2688}

  → tools/call center_on_target {ra: 10.6847, dec: 41.2689, tolerance: 5}
    (compound tool — centering plugin handles internally)
  ← {final_error_arcsec: 2.1, attempts: 3}

  → tools/call auto_focus {camera_id: "main-cam", focuser_id: "main-focuser"}
    (compound tool — focus plugin handles internally)
  ← {best_position: 12450, best_hfr: 2.1}

  → tools/call start_guiding {}
  ← {rms_ra: 0.4, rms_dec: 0.3}

  Capture loop:
    → tools/call capture {camera_id: "main-cam", duration: "300s"}
    ← {image_path: "...", document_id: "doc-042"}
    → tools/call record_exposure {target: "M31", filter: "Luminance"}
    ← {completed: 13, goal: 40}
    → check if dither needed → tools/call dither {pixels: 5}
    → check if temperature drifted → tools/call auto_focus {...}
    → check if meridian flip needed → stop guide, flip, re-center, re-focus, start guide
    → tools/call get_next_target → if target changed, break capture loop

  → tools/call stop_guiding {}
  → continue outer loop with new target
```

### Compound Tools

Sub-workflows like `auto_focus` and `center_on_target` are **built-in
compound tools** — they live in `rp`'s process, drive a multi-step
loop using primitive built-in tools, and expose a single high-level
tool to the orchestrator. The orchestrator does not need to know the
focus algorithm or the centering algorithm; it calls one tool and
gets a result.

```
Orchestrator                    rp (single process)
    │                           ┌───────────────────────────────┐
    │  tools/call auto_focus    │                               │
    ├──────────────────────────►│  auto_focus impl (Rust)       │
    │                           │   ├─ move_focuser             │
    │                           │   ├─ capture                  │
    │                           │   ├─ measure_basic            │
    │                           │   │   (cache hit, no decode)  │
    │                           │   ├─ ... 12 more iterations   │
    │                           │   └─ pick best_position       │
    │  ← {best_position, hfr}  │                               │
    │◄──────────────────────────│                               │
    │                           └───────────────────────────────┘
```

No MCP-over-HTTP hop, no FITS re-decode (the in-process call resolves
each capture's pixels via the image cache).

#### `auto_focus` Contract

A built-in compound tool that drives a V-curve focus sweep using
`move_focuser`, `capture`, and `measure_basic` internally. The
orchestrator calls one tool and gets back the best focuser position
without having to know the focus algorithm.

**Input**:
- `camera_id` — required.
- `focuser_id` — required.
- `duration` — required, humantime string (same shape as `capture`'s
  `duration`, e.g. `"3s"`, `"500ms"`). Per-frame exposure for every
  point in the sweep. No default: the right value depends on focal
  ratio, sky brightness, and target field, none of which `auto_focus`
  can infer. Deriving it from a probe `measure_basic` star count was
  considered and rejected — the probe itself runs at unknown focus,
  so its star count is unreliable as a driver for the rest of the
  sweep.
- `step_size` — required, positive integer focuser steps. Required
  for the same reason `min_area`/`max_area` are required on
  `measure_basic`: focuser step → µm and rig depth-of-focus vary per
  setup and `rp` cannot infer them.
- `half_width` — required, positive integer. The sweep covers
  `[current_position − half_width, current_position + half_width]`
  in `step_size` increments. The grid is then clamped to the
  focuser's `min_position`/`max_position` from the `FocuserConfig`.
- `min_area` and `max_area` — required, passed through to each
  per-frame `measure_basic` call. At extreme defocus, donut-shaped
  PSFs from the secondary obstruction can span many hundreds of
  pixels — set `max_area` accordingly so the wings of the V-curve
  remain measurable.
- Optional `threshold_sigma` (default `5.0`) — passed through to
  `measure_basic`.
- Optional `min_fit_points` (default `5`) — minimum number of
  non-null HFR samples required to fit the V-curve. Also enforced
  on the *grid size* before any motion happens — a sweep that
  cannot produce at least this many capture positions errors before
  moving the focuser.

**Output**:
- `best_position` (i32) — focuser position at the fitted V-curve
  minimum, rounded to the nearest integer step.
- `best_hfr` (f64) — fitted HFR at `best_position`, in pixels.
- `final_position` (i32) — position the focuser was actually moved
  to at the end of the run. Equal to `best_position` on success.
- `samples_used` (usize) — number of HFR samples that contributed
  to the fit (i.e. captures with `star_count > 0` and a non-null
  HFR). `≤ curve_points.length`.
- `curve_points` — array of
  `{position: i32, hfr: f64 | null, star_count: u32, document_id: string}`,
  one entry per capture, in sweep order. `hfr: null` flags a
  starless capture: the entry is preserved as a record but does
  not contribute to the fit.
- `temperature_c` (f64 | null) — focuser temperature read once at
  the start of the run. `null` when the focuser does not implement
  temperature readout (`NOT_IMPLEMENTED`) **or** when the read
  itself fails for any other reason. Temperature is informational
  on the result and never load-bearing on the sweep, so a transient
  read failure does not abort the run — the field is just
  surrendered to `null`. Useful for downstream
  temperature-compensation logic that records
  `(position, temperature)` pairs across runs (callers that need
  absolute temperature confidence should fall back to
  `get_focuser_temperature` per call rather than relying on this
  field).

**Algorithm**:
1. Resolve `camera_id` and `focuser_id`. Read the focuser's current
   position and temperature once each; record both on the result.
   Emit `focus_started` carrying the resolved ids, the current
   position, and the temperature.
2. Compute the sweep grid:
   `start = current_position − half_width`; positions are
   `start, start + step_size, start + 2·step_size, …`, continuing
   while `≤ current_position + half_width`. Clamp the grid to
   `[min_position, max_position]` (any point outside is dropped,
   not coerced — coercion would create duplicate sweep positions
   at the bound). Reject before any motion if the clamped grid has
   fewer than `min_fit_points` positions.
3. For each grid position, in order:
   1. `move_focuser(position)` — block until the focuser reports
      idle (same poll loop the primitive `move_focuser` tool uses).
   2. `capture(camera_id, duration)` — yields `document_id`. The
      pixels populate the image cache as a side effect.
   3. `measure_basic(document_id, min_area, max_area, threshold_sigma)`
      — yields `hfr` and `star_count`. The cache hit avoids any
      FITS decode.
   4. Append `{position, hfr, star_count, document_id}` to
      `curve_points`. A capture with `star_count == 0` (or a
      `null` HFR for any reason) is recorded with `hfr: null` and
      contributes nothing to the fit.
4. If fewer than `min_fit_points` entries have a non-null HFR,
   abort with a `not_enough_stars` error. The focuser is left at
   the last sweep position; `auto_focus` does not auto-recover
   the original position.
5. Fit a parabola in raw HFR vs. position by least squares,
   weighted by `star_count` per point. From the fit
   `hfr = a·position² + b·position + c`:
   `best_position = round(−b / 2a)`; `best_hfr = c − b²/(4a)`.
   Abort with a `monotonic_curve` error in any of three cases:
   (i) the design matrix is singular at fit time (essentially
   flat HFR over the sweep — no parabola can be fitted), (ii)
   `a ≤ 0` (the curve is monotonic or concave-down — no minimum
   exists), or (iii) `a > 0` but the fitted vertex falls outside
   `[min(grid), max(grid)]` (a true minimum exists somewhere
   off-grid, so the visible curve is monotonic *over the sampled
   range* — the caller needs to widen the sweep or coarse-focus
   first).
6. Move the focuser to `best_position` (already inside the sweep
   range by construction, so the operator-supplied
   `min_position`/`max_position` bounds are guaranteed to hold).
7. Emit `focus_complete` with
   `{camera_id, focuser_id, position: best_position, hfr: best_hfr, samples_used}`.

**Error cases**:
- `camera_id` not found → MCP error naming the camera.
- `focuser_id` not found → MCP error naming the focuser.
- Camera or focuser not connected → MCP error.
- `duration`, `step_size`, `half_width`, `min_area`, `max_area`
  missing → MCP error naming the missing parameter (validated in
  body in input order, same convention as `measure_basic`).
- `step_size <= 0`, `half_width <= 0`, or `min_fit_points < 3`
  → MCP error naming the bad parameter (a parabolic fit needs at
  least 3 non-collinear points).
- Estimated unclamped grid size (`2·half_width / step_size + 1`)
  exceeds the safety cap (1000 points) → MCP error before any
  motion or exposure. The cap is purely a guardrail against
  operator misconfiguration that would otherwise produce
  thousands of captures and tie up the rig for hours; any
  plausible auto-focus run fits well inside it (typical 10–30
  points).
- Sweep grid (after clamping to `min_position`/`max_position`)
  has fewer than `min_fit_points` positions → MCP error before
  any motion or exposure. The error message names `min_fit_points`
  so the caller can tell the grid-size failure apart from a
  parameter-validation failure.
- A `move_focuser`, `capture`, or `measure_basic` call inside
  the sweep returns an error → `auto_focus` propagates that
  error and stops sweeping. Captures already taken are persisted
  on disk normally (that path is owned by `capture`, not
  `auto_focus`); the focuser is left at its current position.
- Fewer than `min_fit_points` non-null HFR samples after the
  sweep completes → `not_enough_stars` error.
- Parabolic fit yields no meaningful minimum within the sampled
  range → `monotonic_curve` error. This fires when the design
  matrix is singular (the input is essentially flat HFR), when
  `a ≤ 0` (the curve is monotonic or concave-down — no minimum
  exists), or when `a > 0` but the fitted vertex falls outside
  `[min(grid), max(grid)]` (a true minimum exists somewhere
  off-grid, so the *visible* curve over the sampled range is
  monotonic). The caller is expected to widen `half_width`,
  coarse-focus externally, or both, then retry. The focuser is
  **not** automatically moved to the lowest observed sample,
  because that point is unverified as a true minimum.

**Persistence**: `auto_focus` does **not** write a section on any
single exposure document — its result spans the sweep. Each capture
inside the sweep gets its own `image_analysis` section written by
the embedded `measure_basic` call exactly as if `measure_basic` had
been called directly. The compound result is returned in the MCP
response and emitted as `focus_complete`. Each entry in
`curve_points` carries the per-step `document_id`, so callers that
need per-step provenance can fetch the individual exposure documents
and read their `image_analysis` sections.

**Caveats**:
- Parabolic fit is the V1 choice for simplicity. Real V-curves are
  often slightly asymmetric (extra-focal vs. intra-focal slopes
  differ); a parabola fits an effective vertex that may sit one
  or two steps off the true minimum. Acceptable for amateur rigs.
  An asymmetric V or piecewise-linear fit can ship later either
  side-by-side under a different tool name (e.g.
  `auto_focus_asymmetric_v`) or as a drop-in replacement that
  shadows the built-in — see [Third-party alternatives](#third-party-alternatives).
- No automatic re-sweep on a monotonic curve. The caller already
  knows what coarse-focus heuristic they prefer; `auto_focus`
  reports the failure cleanly and lets the caller widen
  `half_width` or coarse-focus externally before retrying.
  Adding re-sweep state-machine logic would double the BDD
  surface for marginal benefit.
- Saturated stars are included in `star_count` and contribute to
  the fit through their HFR, mirroring `measure_basic`'s policy.
  Filtering them at the auto-focus layer would reintroduce the
  HFR-vs-focus monotonicity break the policy was designed to
  avoid (see `measure_basic` Contract). Callers that need a
  per-curve saturation aggregate can fetch each
  `curve_points[i].document_id` and read its `image_analysis`
  section.
- `auto_focus` is a built-in compound tool and may be **shadowed**
  by a tool-provider plugin advertising the same `auto_focus`
  name; the plugin wins per Config-Time Validation, with the
  shadow logged at startup. Two plugins both claiming
  `auto_focus` remains a config-time error.

#### `plate_solve` Contract

A built-in tool that proxies to the `plate-solver` rp-managed
service over HTTP. The wrapper hides ASTAP's subprocess details and
returns a parsed WCS solution. See
[`docs/services/plate-solver.md`](plate-solver.md) for the wrapper's
own contract.

**Input**:
- `document_id` *or* `image_path` — at least one required. Both
  fields are optional at the serde level so the tool can produce
  consistent error messages mentioning `image_path` when both are
  omitted (matching the imaging-tool convention). When both are
  supplied, **`document_id` takes precedence** (consistent with
  `measure_basic` et al. — see [Built-in Tools](#built-in-tools)).
- `pointing_hint` — optional nested object
  `{ ra_deg: f64, dec_deg: f64 }`. Decimal degrees on the wire for
  both fields (the `_deg` suffix is intentional — Alpaca returns
  `RightAscension` in **hours**, but the wrapper takes degrees).
  Both inner fields are required when the object is present; the
  nested-object shape makes the both-or-neither contract structural
  rather than runtime-validated.
- `use_mount_hints` — optional `bool`, default `false`. When
  `true`, rp reads the current mount position
  (`right_ascension()` × 15 → degrees, `declination()` pass-through)
  and forwards as the wrapper's `ra_hint` / `dec_hint`.
  - Mutually exclusive with `pointing_hint`. Both supplied ⇒ error
    `provide explicit pointing_hint or use_mount_hints, not both`.
  - Requires a configured and connected mount. Mount absent / not
    connected / Alpaca read failure ⇒ error to caller (the caller
    explicitly opted in, so failures are surfaced rather than
    silently dropping to blind solve).
- `fov_hint_deg` — optional `f64`. Forwarded verbatim to the
  wrapper's `fov_hint_deg`. v1 has no per-camera FOV stash on the
  exposure document — callers pass this per request. Tracked by
  [issue #153](https://github.com/ivonnyssen/rusty-photon/issues/153).
- `search_radius_deg` — optional `f64`. Per-call value overrides
  `plate_solver.default_search_radius_deg` from rp config. Both
  absent ⇒ omit from wrapper request ⇒ ASTAP uses its own default.
  The override matters for loaded-from-disk images where the
  configured rig default may not match.
- `timeout` — optional humantime string (e.g. `"30s"`). Forwarded
  to the wrapper's `timeout` field. Omitted ⇒ wrapper applies its
  own `default_solve_timeout`. **Distinct from**
  `plate_solver.timeout` in rp config, which is the rp HTTP-client
  outer timeout (the connection-side backstop per Tenet 1).

When neither `pointing_hint` nor `use_mount_hints` is supplied, the
wrapper falls back to a blind solve.

**Output** (matches the wrapper's `SolveResponseBody` field-for-field):
- `ra_center` (f64) — image-center right ascension in decimal
  degrees.
- `dec_center` (f64) — image-center declination in decimal degrees.
- `pixel_scale_arcsec` (f64) — arcseconds per pixel from the
  parsed `.wcs` `|CDELT1|`.
- `rotation_deg` (f64) — field rotation from `.wcs` `CROTA2`.
- `solver` (String) — solver banner from the wrapper (e.g.
  `"astap-2026.05.03"`).

**Persistence**:
- `document_id` mode: writes a `wcs` section to the exposure
  document via `ImageCache::put_section`. Section payload mirrors
  the output verbatim.
- `image_path` mode: after a successful solve, derives the sibling
  sidecar path (`<base>.fits` → `<base>.json`) and resolves it to
  an `ExposureDocument` via `ImageCache::resolve_document_by_path`.
  If the sidecar exists and parses (the **late-solve workflow**:
  capture frame N → start capture N+1 → solve frame N → update the
  original sidecar), `wcs` is written via `put_section`. If no
  sidecar is present (external FITS, missing sidecar, non-`.fits`
  path), the result is returned without persistence and the cache
  miss is `debug!()`-logged. `put_section` itself falls back to a
  disk-only write when the cache entry is absent (post-eviction or
  post-`rp` restart) so the sidecar always sees the section update.
- Persistence failure (sidecar write error) is logged at `debug!()`
  and does *not* fail the tool — same shape as the imaging-tool
  convention.

**Error policy**:
- Configuration errors before the call:
  - rp `plate_solver` config absent ⇒
    `plate_solve: plate solver not configured`.
  - Hint mutual-exclusion violated ⇒
    `plate_solve: provide explicit pointing_hint or use_mount_hints,
    not both`.
  - `use_mount_hints: true` with mount issue ⇒
    `plate_solve: use_mount_hints requested but mount is <reason>`.
  - Neither `document_id` nor `image_path` ⇒ MCP error mentioning
    `image_path`.
- HTTP-client failures (DNS, refused, connection timeout) ⇒
  `plate_solve: service unreachable: <reason>`.
- Wrapper structured errors (`invalid_request`, `fits_not_found`,
  `solve_failed`, `solve_timeout`, `internal`) propagate verbatim
  as `plate_solve: <code>: <message>`. For `solve_failed`, the
  wrapper's `details.stderr_tail` is appended for diagnostics. rp
  does **not** pre-validate `fits_path` — the wrapper is
  authoritative.

**Algorithm**:
1. Validate `document_id` xor `image_path` (at least one). Validate
   hint shape (mutual exclusion + `use_mount_hints` requires
   connected mount).
2. Resolve `PlateSolveClient` from `AppState`. Error if absent.
3. Resolve `fits_path`:
   - `document_id` mode: `ImageCache::resolve_document(doc_id)` →
     `doc.file_path`.
   - `image_path` mode: forward verbatim.
4. Resolve hints into the wrapper's flat `ra_hint` / `dec_hint`
   pair. Explicit `pointing_hint` maps directly; `use_mount_hints`
   reads the mount and applies the ×15 RA conversion.
5. Resolve `search_radius_deg`: per-call value > config default >
   absent.
6. Build `SolveRequest`, call `client.solve(req)`. Map `SolveError`
   variants to MCP errors per the policy above.
7. On success: in `document_id` mode, persist the `wcs` section to
   that document. In `image_path` mode, attempt UUID-8 reverse-
   lookup; persist if matched, debug-log if not.
8. Return the solver output as the MCP tool result.

**Compound caller note**: `center_on_target` (planned, Phase 6c-3)
sets `use_mount_hints: true` on its inner `plate_solve` calls
rather than calling a Rust-side mount-read helper. That keeps the
hours→degrees conversion in one place (this contract) and avoids a
parallel code path for the same data flow.

`plate_solve` is a built-in tool and may be **shadowed** by a
tool-provider plugin advertising the same `plate_solve` name; the
plugin wins per Config-Time Validation, with the shadow logged at
startup.

#### `center_on_target` Contract

A built-in compound tool that drives an iterative
capture → plate_solve → sync_mount → slew loop until the solved
field-center sits within `tolerance_arcsec` of the requested
`(ra, dec)`. The orchestrator calls one tool with the target
coordinates and gets back the converged pointing without having to
implement its own centering loop.

**Input**:
- `camera_id` — required. Camera that captures each iteration's
  frame.
- `ra` — required, decimal hours `∈ [0, 24)`. Target right
  ascension. Same unit as `slew` and `sync_mount` (Alpaca's
  `RightAscension`).
- `dec` — required, decimal degrees `∈ [-90, 90]`. Target
  declination.
- `duration` — required, humantime string (e.g. `"5s"`,
  `"500ms"`). Per-iteration exposure. No default: the right value
  depends on focal ratio, sky brightness, and target field, none
  of which `center_on_target` can infer. v1 uses the same
  `duration` for every iteration; if low star count blocks a
  solve, the caller re-runs with a longer duration.
- `tolerance_arcsec` — required, positive `f64`. Convergence
  threshold on the great-circle residual between the solved center
  and `(ra, dec)`. No default: the right value depends on rig
  pixel scale (a 1-arcsec/pixel rig wants tighter tolerance than a
  4-arcsec/pixel one) and downstream framing constraints.
- `max_attempts` — required, positive `usize`. Hard cap on the
  number of iterations. No default: the right value depends on
  mount tracking quality and how aggressive the caller wants the
  loop to be (typical 3–5 attempts; tight tolerances or wobbly
  mounts may want 10+). Capped at `MAX_ATTEMPTS = 50` — exceeding
  the cap is a parameter error before any motion. The cap is a
  guardrail against operator misconfiguration that would otherwise
  tie up the rig for an indefinite period; any plausible run fits
  well inside it.

The mount is resolved via the singular `mount` config field — no
`mount_id` or `telescope_id` parameter, since `rp` deployments run
exactly one mount.

**Output**:
- `final_error_arcsec` (f64) — great-circle residual at the
  iteration where convergence fired (i.e. the iteration whose
  `action` is `"converged"`).
- `attempts` (usize) — number of iterations executed. `≥ 1` and
  `≤ max_attempts` on success.
- `final_ra` (f64) — solved RA at the converged iteration, in
  decimal degrees (matches `plate_solve`'s output unit, **not**
  the input's hours).
- `final_dec` (f64) — solved Dec at the converged iteration, in
  decimal degrees.
- `iterations` — array of
  `{document_id: string, residual_arcsec: f64, solved_ra: f64,
  solved_dec: f64, action: "sync" | "slew" | "converged"}`,
  one entry per iteration in execution order. Each
  `document_id` carries the per-iteration capture's `wcs` section
  (written by the embedded `plate_solve`); the
  [`/api/documents/{id}`](#document-resolution) endpoint gives
  callers per-step provenance. `action` is the *terminal* action
  for that iteration: `"sync"` only ever appears on iter 1 (and
  only if iter 1 also slewed — i.e. `iterations[0].action ==
  "sync"` means iter 1 fired sync followed by slew); `"converged"`
  appears at most once and is always the last entry; every other
  entry is `"slew"`. The iter-1-converged-after-sync case
  collapses to `action: "converged"` on the single record (sync
  fired, residual was already inside tolerance, no slew issued).

**Algorithm**:
1. Resolve `camera_id` and the singular mount. Emit
   `centering_started` carrying `{camera_id, ra, dec,
   tolerance_arcsec, max_attempts}` plus an **advisory outer-loop
   deadline** (§2.5): `per_iter = duration +
   centering.solve_time_estimate + centering.slew_overhead_estimate`,
   `predicted_duration_ms = per_iter`, `max_duration_ms = max_attempts
   × per_iter`. rp does not enforce this (each inner `capture`/`slew`
   carries its own deadline, and the watchdog tracks only this outer
   loop); the two estimates come from the `centering` config block
   (defaults 30 s / 10 s).
2. For `iter = 0..max_attempts`:
   1. `capture(camera_id, duration)` → `document_id`. Cache
      populated as a side effect.
   2. `plate_solve(document_id)` with `use_mount_hints: true` so
      the wrapper gets the mount's currently-reported pointing as
      an `ra_hint`/`dec_hint` pair (hours→degrees conversion lives
      in the `plate_solve` handler — see
      [`plate_solve` Contract](#plate_solve-contract)). Yields
      `(solved_ra_deg, solved_dec_deg)` and writes a `wcs` section
      to the document as a side effect.
   3. If `iter == 0`: `sync_mount(solved_ra_deg, solved_dec_deg)`.
      The first solve is the absolute pointing reference;
      subsequent iterations rely on the mount honouring relative
      slews instead of re-syncing on every pass. Repeated syncs
      interact badly with model-building drivers (each sync gets
      treated as a new pointing-model entry, polluting the model)
      and are unnecessary once the absolute position is
      established. Sync fires *unconditionally* on iter 1, even if
      the residual is already inside tolerance — the mount's
      pointing model is calibrated for any caller that follows
      centering with further targeted slews.
   4. Compute `residual_arcsec = haversine(solved_ra_deg,
      solved_dec_deg, ra·15.0, dec)` (the input `ra` is hours; the
      solved values are degrees, so convert the input to degrees
      once for the comparison).
   5. If `residual_arcsec ≤ tolerance_arcsec`: record an
      `iterations[iter]` entry with `action = "converged"`, emit
      `centering_iteration` followed by `centering_complete`, and
      return.
   6. Otherwise: `slew(ra, dec)` honouring the mount's
      `settle_after_slew` config; record an `iterations[iter]`
      entry with `action = "slew"` (or `action = "sync"` on
      iter 1 if the slew was preceded by a sync — see Output's
      collapse rule); emit `centering_iteration` and continue.
3. If the loop exits without firing `"converged"`, return a
   `tolerance_not_reached` error carrying the last residual and
   `max_attempts`. The mount is left at its last commanded
   position; `center_on_target` does not auto-recover.

**Error cases**:
- `camera_id` missing or names a camera that doesn't exist /
  isn't connected → MCP error naming the field/condition (parameter
  validation runs in input order, same convention as
  `auto_focus` / `measure_basic`).
- `ra`, `dec`, `duration`, `tolerance_arcsec`, `max_attempts`
  missing → MCP error naming the missing parameter.
- `ra` outside `[0, 24)` or `dec` outside `[-90, 90]` → MCP error
  naming the bad parameter.
- `tolerance_arcsec ≤ 0` or `max_attempts == 0` → MCP error
  naming the bad parameter.
- `max_attempts > MAX_ATTEMPTS` (50) → MCP error before any
  motion or exposure.
- No mount configured / mount not connected → MCP error.
- Mid-loop `capture`, `plate_solve`, `sync_mount`, or `slew`
  failure → propagates the underlying error and aborts the loop.
  The mount is left where the failed step left it; partial
  `iterations[]` entries are not returned (the failure surfaces as
  an MCP error, not a partial success).
- Unresponsive device → bounded, never an indefinite hang. Every
  Alpaca request rp issues carries a per-request connect + read
  timeout (`equipment::alpaca`), so a device that accepts the
  connection but stops answering — an overloaded simulator in CI, a
  stalled mount/USB-serial bridge at night — surfaces as a timeout
  error instead of wedging the loop forever. The idempotent
  per-iteration mount reads (`plate_solve`'s `use_mount_hints` read;
  the slew's `Slewing` poll) additionally **retry** a transient
  failure with short backoff before giving up, so a brief device
  hiccup is ridden out rather than aborting the whole tool. (This is
  the fix for the issue #319 `center_on_target` timeout: a stalled
  mount read had no client-side timeout and hung indefinitely; the
  blocking-op poll deadlines guard loops, not a single in-flight
  request.)
- MCP session keep-alive vs. per-tool deadlines → companion fix on
  the same PR (#319). rmcp's `LocalSessionManager` defaults to a
  300 s `keep_alive` that fires if the session sees no activity. The
  per-iteration `do_slew_blocking`, `do_park_blocking`, and
  `do_move_focuser_blocking` now carry **predicted deadlines** that
  usually fall well under 300 s (a slew/park/focuser-move's
  `max_duration_ms`), but a long legitimate slew or park can still
  exceed the keep-alive; `do_capture` (`duration + 120 s` readout
  grace) likewise approaches or matches 300 s for long exposures.
  Without
  emission they race the keep-alive: when both fire near the same
  moment the SSE response stream EOFs and the client's `call_tool`
  future never resolves (BDD's 360 s `MCP_CALL_TIMEOUT` is the only
  backstop). Each poll loop emits `notifications/progress` every
  `PROGRESS_INTERVAL` (5 s) — see `mcp::progress` — so the session
  worker's handler-event arm resets the keep-alive timer well
  before it can fire. The emission is a no-op when the client did
  not supply a `progressToken` in `_meta`; unit tests pass `None`.
- Loop exits without convergence → `tolerance_not_reached` error
  citing the last residual and `max_attempts`.

**Persistence**: `center_on_target` does **not** write a section
on any single exposure document. Each per-iteration capture gets
its own `wcs` section written by the embedded `plate_solve` call
exactly as if `plate_solve` had been called directly. The compound
result is returned in the MCP response and emitted as
`centering_complete`. Each entry in `iterations` carries the
per-step `document_id`, so callers that need per-step provenance
can fetch the individual exposure documents and read their `wcs`
sections.

**Caveats**:
- OmniSim does not model pointing — it returns canned exposures
  unaffected by mount position. Convergence over real pixels is
  asserted via the stub plate solver in BDD; the live OmniSim
  camera and mount still exercise the capture / sync / slew
  surface.
- v1 has no `min_improvement_arcsec` early exit. A run that stops
  improving but keeps barely missing tolerance will burn through
  `max_attempts` before erroring. Add the parameter when the first
  workflow needs it.
- v1 has no per-iteration exposure scaling. If the first solve
  fails for star-count reasons, the caller re-runs with a longer
  `duration` rather than letting the tool widen automatically.
- `center_on_target` is a built-in compound tool and may be
  **shadowed** by a tool-provider plugin advertising the same
  `center_on_target` name; the plugin wins per
  [Config-Time Validation](#config-time-validation), with the
  shadow logged at startup. Two plugins both claiming
  `center_on_target` remains a config-time error.

#### Example: `auto_focus` (V-curve)

See [`auto_focus` Contract](#auto_focus-contract) for the full parameter
set, error policy, and persistence rules. The pseudo-code below
illustrates the loop shape only.

```
Orchestrator: tools/call auto_focus {
    camera_id: "main-cam",  focuser_id: "main-focuser",
    duration: "2s",         step_size: 200,          half_width: 1500,
    min_area: 8,            max_area: 400
}
  rp's auto_focus implementation (current_position = 11000):
    move_focuser(position=9500) → 9500
    capture(camera_id="main-cam", duration="2s") → {document_id: "doc-001"}
    measure_basic(document_id="doc-001", min_area=8, max_area=400)
                                                   → {hfr: 6.8, star_count: 220}
    move_focuser(position=9700) → 9700
    ... 14 more sweep points (15 total) ...
    move_focuser(position=12500) → 12500   # last sweep point
    fit parabola → best_position = 11212
    move_focuser(position=11212) → 11212   # final move to fitted vertex
  ← {best_position: 11212, best_hfr: 2.1, final_position: 11212,
     samples_used: 15, curve_points: [...], temperature_c: 4.3}
```

#### Example: `center_on_target`

See [`center_on_target` Contract](#center_on_target-contract) for the
full parameter set, error policy, and persistence rules. The
pseudo-code below illustrates the loop shape only.

```
Orchestrator: tools/call center_on_target {
    camera_id: "main-cam",  ra: 10.6847,           dec: 41.2689,
    duration: "5s",         tolerance_arcsec: 5,   max_attempts: 5
}
  rp's center_on_target implementation:
    iter 0:
      capture(camera_id="main-cam", duration="5s")   → {document_id: "doc-c01"}
      plate_solve(document_id="doc-c01", use_mount_hints: true)
                                                     → {ra_center: 160.230°, dec_center: 41.265°}
      sync_mount(ra=10.6820, dec=41.265)            # 160.230° / 15
      residual_arcsec = haversine(...)               → 45.0    (> tolerance)
      slew(ra=10.6847, dec=41.2689)                  # iter-1 action: "sync"
    iter 1:
      capture → {document_id: "doc-c02"}
      plate_solve(document_id="doc-c02", use_mount_hints: true)
                                                     → {ra_center: 160.270°, dec_center: 41.2685°}
      residual_arcsec                                → 2.1     (≤ tolerance)
                                                     # action: "converged"
  ← {final_error_arcsec: 2.1, attempts: 2,
     final_ra: 160.270, final_dec: 41.2685,
     iterations: [
       {document_id: "doc-c01", residual_arcsec: 45.0, solved_ra: 160.230,
        solved_dec: 41.265,  action: "sync"},
       {document_id: "doc-c02", residual_arcsec: 2.1,  solved_ra: 160.270,
        solved_dec: 41.2685, action: "converged"},
     ]}
```

#### Third-party alternatives

A site that wants a different algorithm (parabolic-fit focus, ML-based
focus, plate-solve-driven centering with custom heuristics) has two
options:

1. **Side-by-side** — ship the alternative under a *different* tool
   name (e.g. `auto_focus_parabolic`). The orchestrator opts in by
   calling the plugin's tool name. Both algorithms are reachable.
2. **Drop-in replacement** — ship the alternative under the *same*
   tool name (`auto_focus`). The plugin shadows the built-in per
   [Config-Time Validation](#config-time-validation), and orchestrators
   continue calling `auto_focus` unchanged. The shadow is logged at
   startup so operators can tell which implementation is active.

Two *plugins* both claiming `auto_focus` remains a startup error —
there is no deterministic precedence between plugins.

## Planning and Ephemeris

`rp` ships the planner with an in-process astronomical math layer
(`rp-ephemeris`) and an embedded deep-sky object catalog
(`rp-catalog`). Together they let an orchestrator answer "image M41
right now" from a name string, without the operator having to paste
coordinates by hand or trust the mount's pointing model.

The implementation plan that introduces both crates lives at
[`docs/plans/archive/rp-planning-tools.md`](../plans/archive/rp-planning-tools.md).
This section describes the resulting design as it appears to MCP
clients and to operators editing the config file.

### Site Configuration

Site location is a top-level `site` block:

```json
"site": {
  "latitude_degrees": 47.6062,
  "longitude_degrees": -122.3321
}
```

Range validation: `latitude_degrees ∈ [-90, 90]`,
`longitude_degrees ∈ [-180, 180]`. Out-of-range values fail config
load with a named field.

The IANA timezone (`America/Los_Angeles`, `Europe/Madrid`,
`America/Santiago`, …) is derived once at startup from `(lat, lon)`
via the `tzf-rs` crate. System tzdata then supplies DST rules.
Startup logs an `info!("site: {site}")` line carrying lat/lon and
the derived timezone, so a misconfigured location surfaces as a
visibly-wrong timezone before it produces wrong twilight times.

`tzf-rs`'s default polygon dataset costs ≈128 MiB of resident memory
and bundles ODbL-licensed data; that is acceptable in this service's
deployment posture. The crate itself is MIT-licensed (an additional
"Anti CSDN License" footnote forbids the Chinese aggregator CSDN
specifically and has no practical effect on this workspace's use).

**No elevation in v1.** Sidereal time depends only on longitude, the
mount's refraction model handles pressure/temperature, and elevation
matters only for solar-system parallax (≤1° for the Moon, sub-arcsec
for planets) and horizon-dip in twilight (≈1° at 4000 m). Adding
`elevation_meters` later is a backwards-compatible config addition.

**No horizon profile in v1.** A single `min_altitude_degrees` (per
target, with a planner-wide default in the `planner` block) covers
the common case. Per-azimuth obstruction profiles are deferred.

### Site Validation Against the ASCOM Mount

`SiteLatitude` and `SiteLongitude` are standard ASCOM Telescope
properties. On telescope connect, `rp` reads both and compares to
config:

- If both reads succeed, any difference greater than `0.01°` (≈1 km)
  in either dimension is a **hard error** on connect, naming both
  pairs in the error message. Silently running ephemeris math
  against a site that disagrees with what the mount computes
  hour-angle from is the precise class of bug that produces
  plausible-looking wrong slew targets — i.e., the worst kind.
- If either read fails (typically `NOT_IMPLEMENTED` from a mount
  that does not expose the property — ASCOM has no
  `CanGetSiteLatitude` / `CanGetSiteLongitude` capability bit;
  the read attempt itself is the capability probe), config is the
  source of truth and a `debug!()` log notes that mount validation
  was skipped.

The mismatch threshold is not configurable: 0.01° is below the
positional accuracy any operator would set deliberately, and well
above any rounding noise on either side.

### The `Ephemeris` Trait

The math layer lives in the `rp-ephemeris` crate. Its design — the
`Ephemeris` trait surface, the `ErfarsEphemeris` ERFA wrapping,
the in-process posture, panic safety and NaN-degradation
guarantees, derived helpers, and time-scale treatment — is
documented in [`docs/crates/rp-ephemeris.md`](../crates/rp-ephemeris.md).

For `rp`'s purposes, the relevant guarantees are:

- All trait methods are pure functions returning owned values; no
  `unsafe`, no FFI types in the surface.
- ERFA failures (host-clock misconfiguration, upstream wrapper
  inconsistency) degrade to NaN coords / `None` windows with an
  `error`-level log — they never crash the rp orchestrator.
- ΔUT1 is treated as zero (≤ 0.9 s = ≤ 13″ of LST error) — well
  inside what plate-solving refines on a real frame.

### Catalog (`rp-catalog`)

`rp-catalog` ships an embedded Messier + NGC + IC dataset (~13k
objects) sourced from openNGC (CC-BY-SA-4.0 attribution; license
text and source pointer live in
`crates/rp-catalog/src/data/LICENSE-DATA`). Resolution is
case- and whitespace-insensitive — `"M41"`, `"m 41"`, `"Messier 41"`,
and `"NGC 2287"` (an alias) all resolve to the same object. Object
type and approximate magnitude are returned alongside RA/Dec.

The catalog is offline-first: typing a Messier number and getting
coordinates is too core to require a plugin install, the data is
small (well under 1 MB compressed), and offline operation matters
at remote dark sites. A future SIMBAD-backed plugin can register the
same `resolve_target` MCP tool name and shadow the built-in via the
existing tool-provider override mechanism (see
[Config-Time Validation](#config-time-validation)).

`targets[]` definitions in config still accept literal RA/Dec —
catalog lookup is a tool call, not a config-time resolution. A
future enhancement could let `targets[]` reference catalog names;
explicitly out of v1 scope.

### Primitive vs. Convenience MCP Tools

Both layers call the same internal `Ephemeris` trait. The split is
purely how operations are projected onto the MCP catalog: primitives
are one operation each (a planner plugin composes them); convenience
tools are the high-level shapes the default planner uses.

**Primitive tools** (one operation each):

| Tool | Returns |
|------|---------|
| `resolve_target {name}` | ICRS RA/Dec, object type, magnitude (catalog) |
| `compute_alt_az {ra, dec, time?}` | altitude, azimuth |
| `compute_transit {ra, dec, date}` | UT of upper transit |
| `compute_rise_set {ra, dec, date, min_alt_degrees}` | rise/set times |
| `compute_meridian_flip {ra, dec, time, side_of_pier}` | time-to-flip |
| `get_sun_position {time?}` | RA/Dec, alt/az |
| `get_twilight {date, kind}` | civil/nautical/astronomical begin/end |
| `get_moon_position {time?}` | RA/Dec, alt/az, phase, illumination |
| `compute_moon_separation {ra, dec, time?}` | angular separation |
| `get_local_sidereal_time {time?}` | LST |

**Convenience tools** (compose primitives, listed in
[Planner Tools](#planner-tools)): `get_target_status`,
`get_next_target`, `get_meridian_status`. `record_exposure` and
`get_session_progress` are orthogonal to ephemeris and not built on
this layer.

The chattiness cost of primitives is zero: planning runs at
target-switch cadence (minutes/hours), not per-frame. A plugin that
makes 20 MCP calls to compute "best target for the next 90 minutes"
is imperceptible.

## Dynamic Planner

The planner is a pure function exposed as MCP tools. Given current state,
it produces recommendations. The orchestrator calls planner tools to
decide what to do next — `rp` does not make workflow decisions.

### Planner Tools

| Tool | Parameters | Returns | Description |
|------|-----------|---------|-------------|
| `get_next_target` | — | target, filter, duration, reason | Evaluate all candidates and recommend the best target/filter |
| `get_target_status` | target_name | altitude, hour_angle, time_to_set, progress | Sky position and progress for a specific target |
| `get_meridian_status` | — | time_to_flip, side_of_pier | Time until meridian flip is needed |
| `record_exposure` | target, filter | completed, goal | Increment exposure counter, return updated progress |
| `get_session_progress` | — | per-target, per-filter progress | Full progress overview |

### Decision Logic (inside `get_next_target`)

The convenience tool delegates each numbered check to the named
primitive (or to the persisted progress map for non-ephemeris
checks). Primitives are defined in the
[Primitive vs. Convenience MCP Tools](#primitive-vs-convenience-mcp-tools)
table.

1. Eliminate targets whose `compute_alt_az` altitude is below
   `min_altitude_degrees` (per-target value, falling back to the
   planner-wide `min_altitude_degrees` from config), or whose
   `compute_rise_set` set time leaves less than the
   `dawn_buffer_minutes` plus a single full exposure.
2. Among the survivors, prefer targets that are transiting —
   smallest absolute hour-angle from `compute_transit` against the
   current `get_local_sidereal_time` (highest altitude, best
   seeing).
3. Prefer targets with the least progress toward their integration
   goal (from the persisted `record_exposure` counters).
4. Minimize filter changes: among the remaining ties, batch the
   same-filter exposure as the previous frame.
5. Account for meridian flip timing — avoid starting a long
   exposure if `compute_meridian_flip` returns a `time_to_flip`
   shorter than the per-target `exposures[].duration_secs` plus a
   safety margin.
6. If no targets are viable, return a `WaitForTwilight` (when
   `get_twilight` says astronomical dusk has not yet begun) or
   `EndOfSession` recommendation.

The orchestrator decides when to call `get_next_target` — typically
after each exposure, after each target switch, or when conditions change.

> **v1 implementation status.** Three of the six bullets land
> partially in v1: the altitude half of bullet 1 (per-target /
> per-planner-default floor), bullet 2 (smallest-|HA| transit
> preference), and the `WaitForTwilight` /
> `AllBelowMinAltitude` half of bullet 6 (the Sun-elevation
> cut-off uses astronomical dusk, -18°, matching the
> "astronomical dusk has not yet begun" wording above).
> Documented v1 gaps:
>
> - **Bullet 1 set-time elimination** — `next_target` does *not*
>   currently check `compute_rise_set` against
>   `exposures[].duration_secs`. A target that rose ten minutes
>   before now and sets in five minutes can still be recommended;
>   the orchestrator must catch the short-set case and re-call
>   `get_next_target`.
> - **Bullet 3 (least-progress preference) and bullet 4
>   (filter-change minimisation)** — both no-op until `rp` threads
>   per-target `record_exposure` counters and the last-frame filter
>   into the planner. Tied transit-distance targets break by
>   ordering in `targets[]`, not by progress.
> - **Bullet 5 (meridian-flip avoidance)** — satisfied *indirectly*:
>   a target whose transit was in the recent past has a large
>   positive HA and ranks lower than a target approaching transit,
>   so the smallest-|HA| pick tends to avoid imminent flips. The
>   planner does *not* check `time_to_flip` against
>   `exposures[].duration_secs` explicitly.
> - **`EndOfSession`** — wired in the reason-discriminant for
>   forward compatibility but unreachable from the v1 code path
>   (would need session-state plumbing to detect "all targets
>   exhausted" or "the night is over").
>
> A follow-up will close these gaps once session state is wired
> through.

### Target Definition

```json
{
  "name": "M31",
  "ra_hours": 0.7122,
  "dec_degrees": 41.2689,
  "exposures": [
    { "filter": "Luminance", "duration_secs": 300, "count": 40 },
    { "filter": "Red", "duration_secs": 300, "count": 20 },
    { "filter": "Green", "duration_secs": 300, "count": 20 },
    { "filter": "Blue", "duration_secs": 300, "count": 20 }
  ],
  "min_altitude_degrees": 30,
  "priority": 1
}
```

## Session Persistence

The session state is persisted to disk after every significant state change.
The application must survive power failures and resume from where it left off.

### Persisted State

```json
{
  "session_id": "session-2026-03-01",
  "started_at": "2026-03-01T22:00:00Z",
  "targets": [ "...target list with progress..." ],
  "equipment_config": { "...device addresses, camera assignments..." },
  "progress": {
    "M31": {
      "Luminance": { "completed": 12, "goal": 40 },
      "Red": { "completed": 5, "goal": 20 }
    }
  },
  "last_completed_exposure": {
    "document_id": "...",
    "timestamp": "2026-03-02T03:45:00Z"
  },
  "mount_state": {
    "last_target": "M31",
    "side_of_pier": "east",
    "tracking": true
  }
}
```

### Recovery Behavior

On startup, `rp` checks for an existing session state file:

1. If no session file exists, start fresh (wait for user to start a session).
2. If a session file exists and the session is still valid (nighttime, targets
   remaining), reconnect to all equipment and re-invoke the orchestrator with
   recovery context (the persisted session state and the reason for
   interruption). The orchestrator decides how to resume — verify mount
   position, re-acquire guiding, continue from the last target, etc.
3. If a session file exists but conditions have changed (daytime, all targets
   complete), mark the session as finished and archive the state file.

### Write Strategy

Session state is written to a temp file and renamed (atomic on POSIX). Writes
happen:
- After each exposure completes
- After each target switch
- After session start/stop
- Before any mount operation (slew, flip, park)

This ensures at most one exposure is lost on power failure.

## Safety

Safety monitoring is a top-level concern owned exclusively by `rp`. It
can override any operation, including cancelling the active orchestrator.

### SafetyMonitor Polling

`rp` polls configured ASCOM Alpaca SafetyMonitor devices at a configurable
interval. On an unsafe transition:

1. Terminate the orchestrator's MCP session (cancel any in-flight tool
   calls).
2. Abort all in-progress exposures (discard partial frames).
3. Stop guiding.
4. Park the mount.
5. Persist session state.
6. Emit `safety_changed` event.
7. Wait in parked state.

On a safe transition while in parked state:
1. Unpark mount.
2. Re-invoke the orchestrator with recovery context (last session state,
   reason for interruption).
3. The orchestrator decides how to resume (verify pointing, re-acquire
   guiding, continue from last target).

### Sentinel Watchdog Integration

Sentinel is extended beyond safety monitoring to serve as an operation watchdog
and supervisor for the entire system. It connects to `rp`'s real-time event
stream (`/api/events/subscribe`) and monitors operation deadlines. The stream
connection also serves as a health signal — if `rp` itself crashes or hangs,
the disconnection is an immediate trigger for Sentinel to attempt recovery.

#### Monitored Operations

| Operation | Starts on event | Expected completion | Timeout = |
|-----------|----------------|--------------------|----|
| Exposure | `exposure_started` | `exposure_complete` | `max_duration_ms` from the `exposure_started` envelope (rp-computed advisory: `duration + camera.readout_time_estimate + 30 s` readout headroom; rp does not itself enforce it) |
| Slew | `slew_started` | `slew_complete` | `max_duration_ms` from the `slew_started` envelope (rp-computed: `(distance / rate + settle) × 3`, floored at `MIN_SLEW_DEADLINE`) |
| Park | `park_started` | `park_complete` | `max_duration_ms` from the `park_started` envelope (rp-computed worst-case traverse: `(180° / rate + settle) × 2`, floored at `MIN_PARK_DEADLINE`) |
| Move focuser | `move_focuser_started` | `move_focuser_complete` | `max_duration_ms` from the `move_focuser_started` envelope (rp-computed: `(\|target − current\| / steps_per_sec) × 2`, floored at `MIN_FOCUSER_DEADLINE`) |
| Focus | `focus_started` | `focus_complete` | configurable max focus time |
| Guide settle | `guide_started` | `guide_settled` | configurable settle timeout |
| Centering | `centering_started` | `centering_complete` | `max_duration_ms` from the `centering_started` envelope (rp-computed advisory outer-loop deadline: `max_attempts × (duration + centering.solve_time_estimate + centering.slew_overhead_estimate)`; per-iteration ops carry their own deadlines) |

#### Corrective Actions

When a deadline expires without the expected completion event:

1. **Health check** — Sentinel pings the relevant Alpaca service endpoint
   to determine if it is responsive.
2. **Responsive but stuck** — Sentinel commands an abort via the device's
   Alpaca API (e.g., `PUT camera/0/abortexposure`). Notifies `rp` to re-plan.
3. **Unresponsive** — Sentinel executes the configured restart command for
   that service (e.g., `systemctl restart qhyccd-alpaca`). After restart,
   notifies `rp` to reconnect and resume.
4. **Notification** — Sentinel sends a push notification (Pushover or other
   configured notifier) describing the failure and corrective action taken.

The restart commands are configured per service, not hardcoded. Sentinel does
not know how to restart any specific service — it just executes the configured
command.

#### Recovery Flow

```
Sentinel detects: exposure_started 300s ago, no exposure_complete
  │
  ├─► Health check camera driver endpoint
  │     │
  │     ├─► Responsive → PUT abortexposure → notify `rp`
  │     │
  │     └─► Unresponsive → run restart command → wait for service
  │           │
  │           └─► Service back → notify `rp` → `rp` reconnects
  │                                                 and resumes session
  └─► Send push notification describing what happened
```

## API Layer

`rp` exposes an HTTP API for UIs and external consumers. The
API is a dumb pipe — it exposes state and accepts commands. It contains no
application logic.

### REST Endpoints

#### Equipment
- `GET /api/equipment` — current equipment status (connected, device info)
- `PUT /api/equipment/{device_id}/connect` — connect to a device
- `PUT /api/equipment/{device_id}/disconnect` — disconnect from a device

#### Targets
- `GET /api/targets` — list all targets with progress
- `POST /api/targets` — add a target
- `PUT /api/targets/{id}` — update a target
- `DELETE /api/targets/{id}` — remove a target

#### Session
- `POST /api/session/start` — start a new session (or resume existing)
- `POST /api/session/stop` — stop the session gracefully (finish current
  exposures, park)
- `POST /api/session/abort` — abort immediately (discard in-progress exposures,
  park)
- `GET /api/session/status` — current session state, active target, progress
- `GET /api/session/plan` — planner's current evaluation (why it chose the
  current target, upcoming decisions)

#### Documents
- `GET /api/documents` — list recent exposure documents
- `GET /api/documents/{id}` — full document with all sections. Returns
  the same JSON written to the sidecar. Resolves through the cache with
  on-disk fallback; returns `404` only when neither cache nor disk has
  the document. See
  [Document Resolution](#document-resolution).
- `POST /api/documents/{id}/sections` — add/update a section (plugin
  endpoint). Requires the document be resolvable; persists the sidecar
  atomically.

#### Images
- `GET /api/images/{document_id}` — image metadata (width, height, bitpix,
  FITS path, exposure document link, in-cache flag)
- `GET /api/images/{document_id}/pixels` — raw pixel data in
  `application/imagebytes` (ASCOM Alpaca ImageBytes wire format). See
  [Image and Document Cache](#image-and-document-cache). Consumers
  wanting FITS read the file directly from the path in the exposure
  document.

#### Plugins
- `POST /api/plugins/{id}/complete` — plugin completion callback
  (status, optional `correction`). The `{id}` is the `event_id` for
  event plugins or the `workflow_id` for orchestrators — both use the
  same endpoint.

#### MCP
- `/mcp` — MCP server endpoint (streamable HTTP transport). Workflow
  plugins connect here as MCP clients to discover and call tools.

#### System
- `GET /health` — health check
- `GET /api/events/subscribe` — SSE (Server-Sent Events) stream of real-time events

### Real-Time Stream

`GET /api/events/subscribe` is a [Server-Sent Events](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events)
stream (`Content-Type: text/event-stream`) carrying every event the
[Event System](#event-system) emits — the same envelopes delivered to plugin
webhooks, plus stream-control frames. Any consumer that needs live events
connects here: UIs for rendering state, and the Sentinel operation watchdog
for tracking deadlines and rp liveness. It is the primary mechanism for
passive consumers — push updates without the webhook ack/completion protocol.

**Frame format.** Each event is one SSE frame:

- `id:` — the envelope's `event_seq` (the monotonic total order; doubles as
  the `Last-Event-ID` replay cursor).
- `event:` — the event type (e.g. `slew_started`).
- `data:` — the full [Event Envelope](#event-envelope) as JSON.

A `:keep-alive` comment is sent every 15 s so idle connections survive
middleboxes.

**Reconnect & replay.** rp retains the most recent 512 events in memory. A
reconnecting client sends its last seen `event_seq` — via the standard
`Last-Event-ID` header (the browser `EventSource` API sets this automatically)
or the explicit `?last_event_id=<seq>` query parameter (the header wins if
both are present). The server replays every retained event after that cursor,
oldest first, then resumes the live tail. The replay→live handoff is
exactly-once: an event is delivered via replay or live, never both, never
neither.

**Gaps.** If the cursor predates the retained window (the client was gone long
enough that its next expected event was evicted), the stream leads with a
`stream_gap` event — `event: stream_gap`, no `id`,
`data: {"event":"stream_gap","requested_after":<cursor>,"oldest_available":<seq>}`
— so the consumer knows it lost history. Sentinel treats a `stream_gap` as a
trigger to escalate any operation it was tracking when the gap occurred.

**Slow consumers.** A consumer that falls more than the broadcast buffer
(256 events) behind the live tail is sent a final `stream_gap`
(`{"event":"stream_gap","lagged":<n>}`) and disconnected, rather than being
allowed to back up the in-process channel. It recovers by reconnecting with
its `Last-Event-ID` (replayed from the 512-event history, or told of the gap
if it fell too far behind).

**Liveness.** When rp shuts down it ends all in-flight subscribe streams, so a
dropped stream is itself a signal: Sentinel treats the disconnection as an
immediate trigger to attempt recovery (see
[Sentinel Watchdog Integration](#sentinel-watchdog-integration)).

Authentication and TLS, when configured, apply to this endpoint exactly as to
every other route.

## Configuration

All configuration is in a single JSON file. Equipment is listed with Alpaca
connection details. Plugins register their webhook URLs and command endpoints.

The `mount` field is singular: exactly one mount is the typical
deployment. Piggyback rigs share that one mount across multiple optical
trains — `cameras`, `focusers`, and `filter_wheels` stay plural for the
trains; `mount` stays singular. Multi-mount support is in
[Future Considerations](#future-considerations). `mount.settle_after_slew`
is applied by `slew` after the mount reports `Slewing == false`; per-call
`settle_after` on `slew` overrides this value (including `"0s"` to skip
when the config sets a non-zero default). `mount.slew_rate_arcsec_per_sec`
(default `7200` = 2°/s, a conservative slow-stepper rate) feeds the
predictive slew deadline; set it per-rig for a tighter bound. It must be a
finite positive number — a bad value is rejected at config load.
`focuser.steps_per_sec` (default `500`, a conservative slow rate) feeds the
predictive `move_focuser` deadline the same way — likewise a finite
positive number rejected at load otherwise.

The `site` block is required for the ephemeris and planner tools
(`compute_alt_az`, `get_twilight`, `get_next_target`, …); when present
it is validated against the ASCOM mount on connect — see
[Site Validation Against the ASCOM Mount](#site-validation-against-the-ascom-mount).
A config without `site` loads cleanly and `rp` runs, but those tools
return a structured "site not configured" error.

```json
{
  "session": {
    "data_directory": "/data/lights",
    "session_state_file": "/data/session_state.json",
    "file_naming_pattern": "{target}_{filter}_{duration}s_{sequence:04}"
  },
  "site": {
    "latitude_degrees": 47.6062,
    "longitude_degrees": -122.3321
  },
  "equipment": {
    "cameras": [
      {
        "id": "main-cam",
        "name": "Main Imaging Camera",
        "alpaca_url": "https://localhost:11120",
        "device_type": "camera",
        "device_number": 0,
        "cooler_target_c": -10,
        "gain": 100,
        "offset": 50,
        "focal_length_mm": 1000.0,
        "readout_time_estimate": "8s",
        "auth": {
          "username": "observatory",
          "password": "secret"
        }
      },
      {
        "id": "guide-cam",
        "name": "Secondary / Wide field Camera",
        "alpaca_url": "http://localhost:11121",
        "device_type": "camera",
        "device_number": 0,
        "cooler_target_c": -10,
        "gain": 200,
        "offset": 30,
        "focal_length_mm": 200.0
      }
    ],
    "mount": {
      "alpaca_url": "http://localhost:11122",
      "device_number": 0,
      "settle_after_slew": "3s",
      "slew_rate_arcsec_per_sec": 7200
    },
    "focusers": [
      {
        "id": "main-focuser",
        "camera_id": "main-cam",
        "alpaca_url": "http://localhost:11113",
        "device_number": 0,
        "min_position": 0,
        "max_position": 100000,
        "steps_per_sec": 1200
      },
      {
        "id": "guide-focuser",
        "camera_id": "guide-cam",
        "alpaca_url": "http://localhost:11113",
        "device_number": 1,
        "auth": {
          "username": "observatory",
          "password": "secret"
        }
      }
    ],
    "filter_wheels": [
      {
        "id": "main-fw",
        "camera_id": "main-cam",
        "alpaca_url": "http://localhost:11123",
        "device_number": 0,
        "filters": ["Luminance", "Red", "Green", "Blue", "Ha", "OIII", "SII"]
      }
    ],
    "safety_monitors": [
      {
        "alpaca_url": "http://localhost:11111",
        "device_number": 0,
        "polling_interval_secs": 10
      }
    ],
    "cover_calibrators": [
      {
        "id": "flat-panel",
        "alpaca_url": "http://localhost:11125",
        "device_number": 0
      }
    ]
  },
  "guider": {
    "url": "http://localhost:11130",
    "settle_threshold_arcsec": 0.8,
    "settle_time": "10s",
    "dither_pixels": 5,
    "dither_every_n_exposures": 3
  },
  "plate_solver": {
    "url": "http://localhost:11131",
    "timeout": "60s",
    "default_search_radius_deg": 3.0
  },
  "imaging": {
    "cache_max_mib": 1024,
    "cache_max_images": 8
  },
  "centering": {
    "solve_time_estimate": "30s",
    "slew_overhead_estimate": "10s"
  },
  "plugins": [
    {
      "name": "image-analyzer",
      "type": "event",
      "webhook_url": "http://localhost:11140/webhook",
      "subscribes_to": ["exposure_complete"],
      "barrier_gates": ["slew", "set_filter"]
    },
    {
      "name": "cloud-backup",
      "type": "event",
      "webhook_url": "http://localhost:11141/webhook",
      "subscribes_to": ["exposure_complete", "session_stopped"]
    },
    {
      "name": "ml-quality-classifier",
      "type": "tool_provider",
      "mcp_server_url": "http://localhost:11150/mcp",
      "requires_tools": ["compute_image_stats"]
    },
    {
      "name": "deep-sky-orchestrator",
      "type": "orchestrator",
      "invoke_url": "http://localhost:11160/invoke",
      "requires_tools": ["slew", "capture", "auto_focus", "center_on_target",
                          "start_guiding", "stop_guiding", "dither",
                          "get_next_target", "record_exposure"]
    }
  ],
  "targets": [
    {
      "name": "M31",
      "ra_hours": 0.7122,
      "dec_degrees": 41.2689,
      "exposures": [
        { "filter": "Luminance", "duration_secs": 300, "count": 40 },
        { "filter": "Ha", "duration_secs": 600, "count": 20 }
      ],
      "min_altitude_degrees": 30,
      "priority": 1
    },
    {
      "name": "IC 1805",
      "ra_hours": 2.5267,
      "dec_degrees": 61.4603,
      "exposures": [
        { "filter": "Ha", "duration_secs": 600, "count": 30 },
        { "filter": "OIII", "duration_secs": 600, "count": 30 },
        { "filter": "SII", "duration_secs": 600, "count": 30 }
      ],
      "min_altitude_degrees": 25,
      "priority": 2
    }
  ],
  "planner": {
    "min_altitude_degrees": 20,
    "dawn_buffer_minutes": 30,
    "prefer_transiting": true,
    "minimize_filter_changes": true
  },
  "safety": {
    "polling_interval_secs": 10,
    "park_on_unsafe": true,
    "resume_on_safe": true,
    "resume_delay_secs": 300
  },
  "server": {
    "port": 11115,
    "bind_address": "0.0.0.0",
    "auth": {
      "username": "observatory",
      "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$..."
    }
  }
}
```

## Module Structure

```
services/rp/src/
  main.rs               CLI entry point (clap + tracing)
  lib.rs                Public API, ServerBuilder + BoundServer, module declarations
  config.rs             Configuration types + load_config()
  error.rs              AppError enum (thiserror)

  # Core domain
  target.rs             Target definitions, progress tracking
  session.rs            Session state, persistence, recovery

  # Equipment layer
  equipment/
    mod.rs              EquipmentManager: connect, disconnect, health check
    alpaca.rs           Generic Alpaca client (reqwest-based)
    camera.rs           Camera device wrapper (expose, abort, cooler, readout)
    mount.rs            Mount wrapper (slew, park, flip, tracking, side of pier)
    focuser.rs          Focuser wrapper (move, temperature)
    filter_wheel.rs     Filter wheel wrapper (set/get position)
    safety_monitor.rs   SafetyMonitor wrapper (poll is_safe)
    cover_calibrator.rs CoverCalibrator wrapper (cover open/close, calibrator on/off)

  # Services (non-Alpaca integrations, backing built-in MCP tools)
  # Per-service HTTP clients live in workspace crates following the
  # `crates/rp-*` convention (e.g. crates/rp-plate-solver). The
  # services/ sub-tree here remains the in-rp glue (lifecycle, MCP
  # adapter shell). plate_solve's MCP tool wiring lives in mcp.rs;
  # the transport/types live in crates/rp-plate-solver.
  services/
    mod.rs              Service trait, service manager
    guider.rs           Guider service client (planned — to follow
                        rp-plate-solver as crates/rp-guider when the
                        guider client work lands)

  # Safety enforcement
  safety.rs             SafetyMonitor polling, park/resume, orchestrator cancellation

  # Planning (exposed as MCP tools — see Planning and Ephemeris)
  # Math and catalog data live in workspace crates rp-ephemeris and
  # rp-catalog; this sub-tree is the MCP-tool wrapping plus the
  # decision logic that composes the primitives.
  planner/
    mod.rs              Module root; tool registration helpers
    primitives.rs       MCP wrappers for the 10 ephemeris primitives
    catalog.rs          MCP wrapper for resolve_target (over rp-catalog)
    convenience.rs      MCP wrappers: get_target_status, get_next_target,
                          get_meridian_status (compose primitives)
    decision.rs         The decision logic from §"Dynamic Planner",
                          parameterised by an `Ephemeris` impl + an
                          explicit `now` so tests are deterministic

  # Event system
  events/
    mod.rs              Event types, EventBus
    webhook.rs          Webhook delivery (fire-and-forget HTTP POST)

  # MCP tool system
  # Pattern: each tool category gets its own #[tool_router(router = X,
  # vis = "pub")] impl block on McpHandler in its own file. McpHandler::new
  # merges the per-category routers via `+`, and a single
  # #[tool_handler(router = self.tool_router)] impl ServerHandler glues
  # them into rmcp's transport. Adding a new tool category = one new
  # file in built_in/, one line in built_in/mod.rs, one `+` in
  # handler::McpHandler::new — no edits to existing categories.
  mcp/
    mod.rs              Module root: declares submodules, holds the
                          shared private macros (tool_success!, tool_error!,
                          resolve_device!) exposed via `pub(crate) use`,
                          and the explicit
                          `#[tool_handler(router = self.tool_router)]
                           impl ServerHandler for McpHandler {}` block.
                          Re-exports `McpHandler`.
    handler.rs          The McpHandler struct (state fields plus
                          `tool_router: ToolRouter<Self>`),
                          `new()`/`with_planner_config()`/`with_plate_solver()`.
                          `new()` merges per-category routers via
                          `Self::tool_router_<category>() + …`.
    internals.rs        Private/`pub(crate)` helpers shared across
                          categories: do_capture, do_move_focuser_blocking,
                          persist_capture_artifact,
                          read_mount_hints_for_plate_solve,
                          resolve_mount, the `*_via_document`/`*_via_path`
                          dispatch helpers, and small private types
                          (Resolved*Params, BackgroundOutcome,
                          PollIdleError) plus free fns clip_outcome,
                          detect_outcome, star_to_json,
                          poll_slewing_until_idle.
    tests.rs            Current home for the full mcp test module
                          (~3,400 lines including six mock-device
                          types, six EquipmentRegistry builders, and
                          the assert_tool_error / ok_text / ok_json
                          helpers). A planned follow-up (see below)
                          will distribute these tests across each
                          built_in/<category>.rs file matching the
                          imaging/ convention, and split the shared
                          mock-device fixtures into a sibling
                          test_support.rs module.
    built_in/
      mod.rs            Declares the per-category submodules.
      camera.rs         CaptureParams, CameraIdParams + capture +
                          get_camera_info.
      imaging.rs        6 imaging param structs + compute_image_stats,
                          measure_basic, estimate_background,
                          detect_stars, measure_stars, compute_snr.
      filter_wheel.rs   SetFilterParams, FilterWheelIdParams +
                          set_filter, get_filter.
      cover_calibrator.rs CalibratorIdParams, CalibratorOnParams +
                          close_cover, open_cover, calibrator_on,
                          calibrator_off.
      focuser.rs        FocuserIdParams, MoveFocuserParams +
                          move_focuser, get_focuser_position,
                          get_focuser_temperature.
      mount.rs          SlewParams, SyncMountParams, SetTrackingParams,
                          GetTrackingParams, GetMountPositionParams,
                          ParkParams, UnparkParams, GetParkStateParams,
                          AbortSlewParams + the 9 mount tools.
      auto_focus.rs     AutoFocusToolParams + auto_focus tool +
                          AutoFocusAdapter (binds the imaging::tools::auto_focus
                          traits to the handler's primitives).
      plate_solve.rs    PointingHint, PlateSolveParams + plate_solve
                          tool.
      planner.rs        13 planner param structs + 10 ephemeris
                          primitive tools + 3 convenience tools
                          (get_target_status, get_next_target,
                          get_meridian_status).
    # Planned follow-up: distribute the centralized tests.rs into
    # per-category `#[cfg(test)] mod tests` blocks inside each
    # built_in/<category>.rs (matching the imaging/ test-colocation
    # convention) and extract the shared mock-device fixtures and
    # registry builders into a sibling test_support.rs module.
    #
    # Planned (not in tree yet):
    # - aggregator.rs    Connects to plugin MCP servers, proxies their
    #                    tools. Lands when the third-party plugin
    #                    protocol does.

  # Imaging (pure analysis kernels and the compositional tools that bind them)
  # Async, I/O, and on-disk layout live in `persistence/` so the analysis
  # path stays unit-testable without a runtime.
  imaging/
    mod.rs              Module root: re-exports the flat `imaging::*` API
                          shape that callers use, regardless of which
                          submodule a symbol is defined in.
    analysis/           Pure single-purpose kernels — generic over Pixel,
                          take ArrayView2, no I/O, no async.
      mod.rs
      pixel.rs          Pixel trait (impls for u16 and i32) for generic analysis
      stats.rs          Pixel statistics (median, mean, min, max ADU)
      background.rs     Sigma-clipped background estimation
      stars.rs          Star detection + centroiding (4-connectivity BFS)
      hfr.rs            HFR / HFD radial flux accumulation
      fwhm.rs           2D Gaussian fitting via rmpfit
      snr.rs            Per-star + median SNR (CCD-equation approximation)
    tools/              Compositional analyzers — bind multiple kernels
                          together to answer one MCP-tool-shaped question.
                          Pure functions; the MCP wrapper in `mcp.rs`
                          resolves pixels and serializes results.
      mod.rs
      measure_basic.rs  measure_basic tool: background + stars + hfr
      measure_stars.rs  measure_stars tool: per-star photometry + PSF fit

  # Persistence (FITS I/O, image+document cache, exposure-document storage)
  persistence/
    mod.rs              Module root: re-exports CachedImage / ImageCache /
                          ExposureDocument / write_fits_u16 / write_fits_i32 etc.
    document.rs         ExposureDocument struct, atomic sidecar JSON
                          persistence (write_sidecar_at: stage to .tmp →
                          rename). Document storage and lookup are
                          mediated by the unified Image and Document
                          Cache (`persistence/cache.rs`).
    cache.rs            ImageCache: CachedPixels enum (U16 | I32),
                          Arc<CachedImage> holding pixels + document
                          together, LRU eviction over combined memory
                          footprint, readdir+DOC_ID disk fallback.
    fits.rs             FITS read/write via rp-fits (writes BITPIX=16 for
                          16-bit sensors, BITPIX=32 fallback when max_adu
                          exceeds u16::MAX; reads normalise to i32; embeds
                          the document UUID in DOC_ID).

  # Post-capture pipeline
  pipeline/
    mod.rs              Pipeline orchestrator: dispatch async tasks after capture
    save.rs             Write FITS to final location, create sidecar JSON

  # API layer
  api/
    mod.rs              Axum router setup
    equipment.rs        Equipment endpoints
    targets.rs          Target CRUD endpoints
    session.rs          Session control endpoints
    documents.rs        Document endpoints (including plugin section updates)
    stream.rs           WebSocket / SSE event stream
    types.rs            API request/response types (serde)

  # I/O abstractions
  io.rs                 Traits for HTTP client, clock, filesystem (testability)
```

## Testing Strategy

Testing follows the conventions in `docs/skills/testing.md`.

### Unit Tests

- **Planner tools**: Given a target list, progress, and sky state, assert
  correct target/filter selection. Pure function, easy to test exhaustively.
- **Safety enforcement**: Assert correct behavior on unsafe transitions
  (orchestrator cancellation, mount parking, session persistence).
- **Document**: Serialization round-trips, section merging, atomic persistence.
- **Configuration**: Deserialization, validation, defaults.
- **Config-time validation**: Missing tools, conflicting plugins, circular
  dependencies.
- **Sky calculations**: covered by `rp-ephemeris`'s own
  reference-value tests against Astropy-generated values
  (alt/az / transit / rise/set / sun / moon / twilight). `rp`'s
  unit tests don't re-test the math; they assert the MCP wrappers
  deserialise inputs, dispatch through the trait, and serialise
  outputs correctly. `planner/decision.rs` is unit-tested with a
  mock `Ephemeris` impl over hand-rolled fake positions.

### BDD Tests (Cucumber)

Behavioral specifications for `rp`'s responsibilities:

- Session lifecycle (start → invoke orchestrator, stop, safety override)
- Safety override (cancel orchestrator, park mount, persist state, resume)
- MCP tool validation and safety guardrails
- Event delivery to webhook endpoints
- Power failure recovery (re-invoke orchestrator with recovery context)

Note: orchestration workflow tests (capture loops, target switching,
meridian flips) belong to the orchestrator plugin, not to `rp`. For
example, end-to-end flat calibration scenarios live in
`services/calibrator-flats/tests/` and spawn `rp` via the `rp-harness`
feature of `bdd-infra`. Each new workflow plugin owns its own BDD
suite rather than adding scenarios here.

#### Prerequisites

BDD tests require the [ASCOM Alpaca Simulators (OmniSim)](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators)
binary. The test harness discovers the binary in this order:

1. `OMNISIM_PATH` env var — full path to the binary
2. `OMNISIM_DIR` env var — directory containing the binary
3. `ascom.alpaca.simulators` on `PATH`

To install locally, download the appropriate release binary for your
platform, extract it, and either add its directory to `PATH` or set one
of the environment variables above. In CI, the
`.github/actions/install-omnisim` composite action handles this
automatically.

**CI pins a patched fork**, not upstream: the action defaults to
[`ivonnyssen/ASCOM.Alpaca.Simulators` `v0.5.0-326.4`](https://github.com/ivonnyssen/ASCOM.Alpaca.Simulators/releases/tag/v0.5.0-326.4),
a build of upstream `v0.5.0` plus a series of `TelescopeHardware` fixes
for the `center_on_target` slew-state hang/flake (issues #326, #319):
326.1/.2 put the slew-engine writers and the
`IsSlewing`/RA/Dec/`AtPark`/`SlewState` readers under `hardwareLock`;
326.3 disposes the per-`Init()` slew timer (it leaked one live timer on
each per-scenario "restart to clean state" reset, accumulating tick sources
that raced the single static slew engine) and resets the slew state —
including the `slewing` flag — under `hardwareLock`. Those addressed the
locking/leak races but **not** the underlying wedge: 326.4 fixes it
(issue #319). The real cause is geometric — a GEM goto whose
shortest-path primary rotation crosses the hour-angle software limit
(`180 + hourAngleLimit`) gets undone by `CheckAxisLimits` every tick, so
the slew never finishes and `IsSlewing` stays `true` forever.
`center_on_target`'s sync-then-slew-to-near-the-same-coords triggers it
for the `RA < 180` off-target row at certain sidereal times (hence the
intermittent CI flake). 326.4 makes `DoSlew` take the limit-avoiding
rotation to the *same* target (pier side unchanged, so ConformU stays
clean) and adds a no-progress guard. The action's `repo`
and `version` inputs revert to upstream
[`v0.5.0`](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators/releases/tag/v0.5.0)
in one line once the fix lands upstream. For local runs, use the pinned
fork as well — upstream `v0.5.0` still carries both the #326 races and
the sidereal-time-gated #319 wedge, so it can hang `center_on_target`
intermittently and is not recommended for local BDD runs.

#### Graceful Shutdown and Coverage

BDD tests spawn `rp` as a child process. For LLVM coverage data to be
captured from the child process, two conditions must be met:

1. **Graceful shutdown via SIGTERM.** LLVM coverage writes `.profraw`
   files through an `atexit` handler, which only runs on clean process
   exit. `SIGKILL` skips `atexit`, so no coverage data is written.
   `lib.rs` handles `SIGTERM` (and `Ctrl-C`) via `tokio::signal` to
   trigger a clean shutdown.

2. **Explicit `stop()` before `Drop`.** The `ServiceHandle` (from the
   shared `bdd-infra` crate) is created with `kill_on_drop(true)` as a
   safety net against leaked processes. However, when `Drop` fires, it
   sends `SIGTERM` immediately followed by `SIGKILL` from `kill_on_drop`
   — too fast for the process to flush. The cucumber `after` hook in
   `bdd.rs` calls `handle.stop()` explicitly, which sends `SIGTERM` and
   waits for the process to actually exit (up to 5 seconds) before the
   `ServiceHandle` is dropped.

The CI coverage job uses `cargo llvm-cov show-env` to set up an
instrumented build environment, then builds all workspace binaries with
`cargo build --workspace`. The BDD test discovers the instrumented `rp`
binary via `CARGO_LLVM_COV_TARGET_DIR`. The child process inherits
`LLVM_PROFILE_FILE` (with `%p`/`%7m` placeholders to avoid file
conflicts), and `cargo llvm-cov report` merges all `.profraw` files from
both test binaries and spawned child processes.

### Integration Tests

- MCP tool tests with mock equipment
- Tool provider aggregation (proxy plugin-provided tools)
- Event delivery to webhook endpoints
- Session persistence and recovery round-trips

### I/O Abstractions

All external I/O (HTTP calls, filesystem, clock) goes through traits defined in
`io.rs`. Tests inject mocks to verify behavior without real devices or network.

## Future Considerations

Items explicitly out of scope for the initial implementation:

- **Distributed plugins** — plugins on remote machines accessing FITS files
  over the network
- **Plugin marketplace / registry** — discovery and installation of third-party
  plugins
- **Multiple mounts** — the current design assumes one mount; extending to
  multiple mounts is a separate concern
- **Dome control** — ASCOM Dome device integration
- **Mosaic planning** — multi-panel target definitions

Note: flat/dark frame automation is no longer out of scope — it can be
implemented as a calibration orchestrator plugin without changes to `rp`.
