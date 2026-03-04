# rp — Main Application Design

## Overview

`rp` is the orchestration layer of Rusty Photon. It connects to
equipment via ASCOM Alpaca, plans imaging sessions dynamically, coordinates
multi-camera capture, and emits events that plugins consume. It does not
integrate with hardware directly — every device interaction goes through a
network API.

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
5. **Minimal footprint.** A Raspberry Pi is the target platform. Memory and CPU
   budgets are tight.
6. **Loose coupling via events.** The application emits events; plugins react.
   The application knows as little as possible about what plugins do.
7. **UI is a client, not a component.** The web UI contains zero application
   logic. It renders state and sends commands. Anyone can build an alternative
   UI without changing the application.

## Architecture

The system is a constellation of independent web services. `rp` is the orchestrator at the center.

```
                       ┌───────────────────┐
                       │     Web UI        │
                       │  (Leptos/WASM or  │
                       │   any framework)  │
                       │  NO app logic     │
                       └────────┬──────────┘
                                │ REST + WebSocket
                       ┌────────▼──────────┐
                       │       RP          │
                       │                   │
                       │  Orchestrator     │
                       │  Event Bus        │
                       │  Dynamic Planner  │
                       │  Session State    │
                       │  API Layer        │
                       └──┬────┬────┬──────┘
                          │    │    │
            ┌─────────────┤    │    ├─────────────┐
            │   Alpaca    │    │    │  Webhooks   │
            ▼             ▼    │    ▼             ▼
       [Camera]      [Mount]   │ [Plate Solver] [Analyzer]
       [Focuser]     [FWheel]  │ [Cloud Backup] [Custom]
       [SafetyMon]             │
                               │ HTTP (commands + events)
                               ▼
                        [Guider Service]
                        (wraps PHD2)

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
  "sequence_number": 42
}
```

### Plugin Sections (contributed via API)

Plugins write results into named sections. `rp` merges them into the
document and persists the sidecar JSON. Each section is opaque to `rp` — it stores and serves whatever the plugin provides.

```json
{
  "id": "...",
  "...core fields...",
  "sections": {
    "plate_solve": {
      "solved_ra": 10.6848,
      "solved_dec": 41.2690,
      "rotation": 12.3,
      "scale_arcsec_per_pixel": 1.05,
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
    }
  }
}
```

### Persistence

The document is persisted as a sidecar JSON file next to the FITS file:

```
/data/lights/M31/
  M31_L_300s_001.fits
  M31_L_300s_001.json    <-- exposure document
```

The document is written after capture completes and updated as plugins
contribute sections. Updates are atomic (write to temp file, rename).

## Event System

`rp` emits events. Plugins and services subscribe via webhooks.
The application does not know or care what subscribers do with events.

### Events

| Event | Payload | When |
|-------|---------|------|
| `session_started` | session config, target list, equipment | Session begins |
| `session_stopped` | session summary, reason | Session ends (manual, safety, dawn) |
| `exposure_started` | camera_id, target, filter, duration | Shutter opens |
| `exposure_complete` | document, file_path | Readout finished |
| `slew_started` | target coordinates | Mount begins slew |
| `slew_complete` | target coordinates, actual coordinates | Mount reports slew done |
| `centering_started` | target, attempt number | Plate solve + correct begins |
| `centering_complete` | target, error arcsec | Centering converged |
| `focus_started` | camera_id, focuser_id, temperature | Auto-focus begins |
| `focus_complete` | camera_id, position, hfr | Auto-focus result |
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
| `frame_rejected` | document_id, plugin, reason | Plugin rejected a frame via `skip_to` |
| `plugin_timeout` | plugin, event_id | Plugin did not respond within `max_duration_secs` |
| `document_updated` | document_id, section_name | Plugin contributed a section |

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
  "estimated_duration_secs": 20,
  "max_duration_secs": 30
}
```

- `estimated_duration_secs`: how long the plugin expects processing to
  take. The planner uses this for scheduling decisions. Provided
  dynamically per invocation — a plate solve on a wide-field image may
  differ from a narrow-field one.
- `max_duration_secs`: hard timeout. If the plugin doesn't complete within
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

Or, to request a corrective workflow change:

```json
{
  "status": "complete",
  "skip_to": "focus_started",
  "reason": "HFR degraded from 2.3 to 4.8 — likely focus drift"
}
```

- `skip_to` (optional): requests that the orchestrator transition to a
  different state instead of proceeding normally (see Workflow Deviation
  below).
- `reason` (optional): human-readable explanation, logged and included in
  events.

#### Barriers

A plugin can optionally declare **barriers** — orchestrator events that
must not fire until the plugin has posted its completion for the most
recent webhook. This tells `rp`: "if you haven't heard back from me yet,
wait before firing these events."

```json
{
  "name": "image-analyzer",
  "webhook_url": "http://localhost:11140/webhook",
  "subscribes_to": ["exposure_complete"],
  "barriers": ["target_switch", "filter_switch"]
}
```

When a barrier event is about to fire, `rp` checks whether any barrier
plugin still has an outstanding (uncompleted) webhook. If so, `rp` waits
for the completion — up to `max_duration_secs` from the acknowledgment —
before proceeding. All outstanding plugins are waited on in parallel.

A plugin with no `barriers` (or an empty list) is never waited on. Its
completion is still processed when it arrives, but `rp` never blocks on
it.

#### Workflow Deviation (`skip_to`)

A plugin can request that the orchestrator transition to a different state
by including `skip_to` in its completion. The `skip_to` field references
a valid orchestrator state:

- After `exposure_complete`, an image analyzer detects degraded HFR →
  `skip_to: "focus_started"`. `rp` cancels the in-progress exposure,
  does not count the rejected frame toward the exposure goal, and
  transitions to auto-focus.
- On `target_switch` barrier, the same analyzer finds a bad frame →
  `skip_to: "focus_started"`. `rp` refocuses instead of switching,
  rolls back the exposure counter, and stays on the current target.

`rp` validates the `skip_to` target against the current state machine.
Invalid or irrelevant requests (e.g., the target has already switched)
are logged and ignored.

**Conflict resolution:** when multiple plugins request different `skip_to`
targets, the most regressive target wins — the one earliest in the state
machine pipeline. If one plugin requests refocus and another requests
recenter, recenter wins because it is earlier and includes refocusing.

**Frame rejection:** a `skip_to` completion implicitly rejects the frame
that triggered the event. `rp`:

1. Does not count the rejected frame toward the exposure goal.
2. Marks the exposure document with the rejection reason.
3. Emits a `frame_rejected` event.

#### Timeout Behavior

When `max_duration_secs` (from the acknowledgment) expires without a
completion:

1. `rp` proceeds as if the plugin completed with `"complete"` and no
   `skip_to`.
2. A `plugin_timeout` warning event is emitted.
3. The timeout is logged.

Webhook delivery failures (connection refused, HTTP errors) are treated
as immediate completion with no `skip_to`. Plugins are responsible for
their own reliability.

#### Example: Image Analyzer Flow

Setup: 5 exposures on the same target, 300s each, analysis takes 20s.

```
Exposure 3 completes
  → rp POSTs exposure_complete to analyzer
  → analyzer responds immediately:
      {"estimated_duration_secs": 20, "max_duration_secs": 30}
  → rp records durations, starts exposure 4 (no blocking)
  → analyzer processes frame 3 in parallel (20s)
  → analyzer POSTs to /api/plugins/{event_id}/complete:

    Case A — frame OK:
      {"status": "complete"}
      → rp notes the completion, capture continues normally

    Case B — frame bad, no barrier pending:
      {"status": "complete", "skip_to": "focus_started",
       "reason": "HFR 4.8, expected < 3.0"}
      → rp cancels exposure 4, rolls counter back to 3
      → transitions to auto-focus, then resumes capture

    Case C — frame bad, target_switch pending:
      (rp was about to switch targets but is waiting for
       the analyzer's outstanding completion)
      → analyzer completes with skip_to: "focus_started"
      → rp cancels the target switch, refocuses, stays on target
      → counter rolls back, resumes capture
```

### Plugin Section Updates

After processing an event, plugins POST their results back to `rp`:

```
POST /api/documents/{document_id}/sections
Content-Type: application/json

{
  "section_name": "plate_solve",
  "data": {
    "solved_ra": 10.6848,
    "solved_dec": 41.2690,
    "rotation": 12.3
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

`rp` never exposes raw device access. Every action validates parameters,
enforces safety constraints, and tracks state before touching hardware.

### Action Catalog

`rp` maintains a registry of all available actions. The catalog is built
at startup from two sources:

1. **Built-in actions** — hardware primitives and basic compute provided
   by `rp` itself.
2. **Plugin-provided actions** — compute or analysis actions registered by
   plugins.

When a workflow plugin is invoked (see Workflow Plugins below), `rp` sends
it the current action catalog so the plugin knows what it can call.

### Built-in Actions

**Hardware**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `capture` | camera_id, duration_secs, binning | image_path, document_id | Take an exposure and save the FITS file |
| `move_focuser` | focuser_id, position | actual_position | Move focuser to absolute position |
| `get_focuser_position` | focuser_id | position | Read current focuser position |
| `get_focuser_temperature` | focuser_id | temperature_c | Read focuser temperature sensor |
| `slew` | ra, dec | actual_ra, actual_dec | Slew mount to coordinates (blocks until settled) |
| `sync_mount` | ra, dec | — | Sync mount position to given coordinates |
| `set_filter` | filter_wheel_id, filter_name | — | Change filter wheel position |
| `get_filter` | filter_wheel_id | filter_name, position | Read current filter |

**Guider**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `start_guiding` | — | rms_ra, rms_dec | Start guiding loop, block until settled |
| `stop_guiding` | — | — | Stop guiding loop, block until confirmed |
| `dither` | pixels | rms_ra, rms_dec | Send dither command, block until settled |
| `pause_guiding` | — | — | Pause guiding (e.g., during readout) |
| `resume_guiding` | — | — | Resume paused guiding |
| `get_guiding_stats` | — | rms_ra, rms_dec, total_rms | Read current guiding statistics |

**Compute**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `plate_solve` | image_path, hint (optional) | ra, dec, rotation, scale | Solve an image via the configured plate solver service |
| `measure_basic` | image_path | hfr, star_count, background_mean, background_stddev | Compute basic image statistics |

All built-in actions validate parameters before execution. `move_focuser`
checks position bounds. `capture` checks that the camera is connected and
idle. Invalid requests return an error — they never reach the hardware.

### Plugin-Provided Actions

A plugin can register actions that other plugins can call through `rp`.
For example, a specialized measurement plugin might provide:

| Action | Provider | Description |
|--------|----------|-------------|
| `measure_eccentricity` | star-analyzer | Measure star eccentricity across the field |
| `measure_wavefront` | wavefront-analyzer | Wavefront error analysis |

Plugin-provided actions follow the same request-response pattern as
built-in actions. When a workflow plugin calls a plugin-provided action,
`rp` routes the request to the providing plugin and returns the result.

**All action results that produce image metrics MUST be written into the
exposure document as a section.** This is the one rule — the document is
the shared data bus. `rp` enforces this: compute action results are
merged into the document before being returned to the caller.

### Workflow Plugins

Workflow plugins are a second plugin type alongside event plugins. Where
event plugins react to events asynchronously (webhook → ack → complete),
workflow plugins take imperative control of a sub-workflow by calling
actions on `rp`.

A workflow plugin handles a specific orchestrator phase — focusing,
centering, or any future sub-workflow. When the orchestrator reaches that
phase, it delegates to the configured workflow plugin.

#### Registration

Workflow plugins declare what they handle, what actions they need, and
optionally what actions they provide:

```json
{
  "name": "vcurve-focus",
  "type": "workflow",
  "workflow_url": "http://localhost:11150/workflow",
  "handles": ["focusing"],
  "requires_actions": ["capture", "move_focuser", "get_focuser_position", "measure_basic"],
  "provides_actions": [],
  "max_duration_secs": 300
}
```

A plugin that both handles a workflow and provides actions for others:

```json
{
  "name": "advanced-centering",
  "type": "workflow",
  "workflow_url": "http://localhost:11151/workflow",
  "handles": ["centering"],
  "requires_actions": ["capture", "slew", "sync_mount", "plate_solve"],
  "provides_actions": ["measure_field_curvature"],
  "max_duration_secs": 180
}
```

#### Delegation Protocol

When the orchestrator enters a phase handled by a workflow plugin:

**Step 1: Invocation.** `rp` POSTs to the plugin's `workflow_url`:

```
POST <workflow_url>
Content-Type: application/json

{
  "workflow_id": "wf-550e8400-e29b-41d4",
  "phase": "focusing",
  "context": {
    "camera_id": "main-cam",
    "focuser_id": "main-focuser",
    "current_position": 12000,
    "temperature_c": -5.2,
    "last_known_hfr": 2.3
  },
  "action_catalog": [
    { "name": "capture", "type": "built_in" },
    { "name": "move_focuser", "type": "built_in" },
    { "name": "get_focuser_position", "type": "built_in" },
    { "name": "measure_basic", "type": "built_in" },
    { "name": "measure_eccentricity", "type": "plugin", "provider": "star-analyzer" }
  ]
}
```

The plugin acknowledges immediately (same pattern as event plugins):

```json
{
  "estimated_duration_secs": 120,
  "max_duration_secs": 300
}
```

**Step 2: Action calls.** The plugin drives the sub-workflow by calling
action endpoints on `rp`:

```
POST /api/actions/{workflow_id}/execute
Content-Type: application/json

{
  "action": "move_focuser",
  "params": {
    "focuser_id": "main-focuser",
    "position": 10000
  }
}

Response 200:
{
  "result": {
    "actual_position": 10000
  }
}
```

Each action call is synchronous from the plugin's perspective — it sends
a request and waits for the result. `rp` validates the request, executes
it, and returns the result. The `workflow_id` scopes the action to the
active workflow for auditing and safety enforcement.

**Step 3: Completion.** When the plugin finishes, it POSTs to the
standard completion endpoint:

```
POST /api/plugins/{workflow_id}/complete
Content-Type: application/json

{
  "status": "complete",
  "result": {
    "best_position": 12450,
    "best_hfr": 2.1,
    "curve_points": 15
  }
}
```

The result is opaque to `rp` — it is logged and included in the relevant
event payload (e.g., `focus_complete`).

#### Example: V-Curve Focus Workflow

```
rp enters FOCUSING state
  → POSTs to vcurve-focus plugin:
      phase: "focusing", context: {camera: "main-cam", focuser: "main-focuser", ...}
  → plugin acks: {estimated: 120, max: 300}

  Plugin drives the V-curve:
    → POST /api/actions/{wf}/execute  {action: "move_focuser", position: 10000}
    ← {actual_position: 10000}
    → POST /api/actions/{wf}/execute  {action: "capture", camera_id: "main-cam", duration: 2}
    ← {image_path: "/tmp/focus_001.fits", document_id: "doc-001"}
    → POST /api/actions/{wf}/execute  {action: "measure_basic", image_path: "/tmp/focus_001.fits"}
    ← {hfr: 5.2, star_count: 340}
    → POST /api/actions/{wf}/execute  {action: "move_focuser", position: 10200}
    ← {actual_position: 10200}
    → POST /api/actions/{wf}/execute  {action: "capture", ...}
    ... 12 more iterations ...
    → POST /api/actions/{wf}/execute  {action: "move_focuser", position: 12450}
    ← {actual_position: 12450}

  Plugin completes:
    → POST /api/plugins/{wf}/complete
        {status: "complete", result: {best_position: 12450, best_hfr: 2.1}}

rp emits focus_complete event, transitions to GUIDE_START
```

#### Example: Iterative Centering Workflow

```
rp enters CENTERING state
  → POSTs to centering plugin:
      phase: "centering", context: {target_ra, target_dec, tolerance_arcsec: 5}
  → plugin acks: {estimated: 60, max: 180}

  Plugin drives the centering loop:
    → POST /api/actions/{wf}/execute  {action: "capture", duration: 5}
    ← {image_path: "/tmp/center_001.fits", document_id: "doc-002"}
    → POST /api/actions/{wf}/execute  {action: "plate_solve", image_path: ...}
    ← {ra: 10.6820, dec: 41.2650, error_arcsec: 45}
    → POST /api/actions/{wf}/execute  {action: "sync_mount", ra: 10.6820, dec: 41.2650}
    → POST /api/actions/{wf}/execute  {action: "slew", ra: 10.6847, dec: 41.2689}
    ← {actual_ra: 10.6845, actual_dec: 41.2688}
    → repeat until error < tolerance ...

  Plugin completes:
    → POST /api/plugins/{wf}/complete
        {status: "complete", result: {final_error_arcsec: 2.1, attempts: 3}}

rp emits centering_complete event, transitions to FOCUSING
```

### Safety Guardrails

`rp` enforces safety on every action call:

- **Parameter validation**: focuser position within min/max bounds,
  exposure duration within configured limits, slew coordinates above
  horizon.
- **State validation**: cannot capture while another capture is in
  progress on the same camera, cannot slew during an exposure.
- **Scope restriction**: a workflow plugin can only control equipment
  relevant to its phase. A focusing workflow cannot slew the mount.
- **Timeout**: if `max_duration_secs` expires without completion, `rp`
  cancels the workflow, moves equipment to a safe state, and proceeds
  with the next orchestration phase.
- **Safety override**: a safety event (unsafe transition) immediately
  cancels any active workflow. The plugin's next action call returns an
  error indicating the workflow was cancelled.

### Config-Time Validation

At startup, `rp` validates the full plugin dependency graph:

1. Build the action catalog from built-in actions and all
   `provides_actions` declarations.
2. For each workflow plugin, verify that every action in
   `requires_actions` exists in the catalog.
3. For each orchestrator phase that expects a workflow plugin (focusing,
   centering), verify that exactly one plugin declares `handles` for
   that phase.
4. Detect circular dependencies — a plugin cannot both provide an action
   and require it.
5. If validation fails, `rp` refuses to start and reports the missing
   actions or conflicting handlers.

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

### Guider Service

The guider service wraps PHD2 and exposes an HTTP API. The existing
`phd2-guider` library provides the PHD2 JSON-RPC integration and will be
reworked to run as an HTTP service.

PHD2 uses JSON-RPC over TCP, which is the one exception to the Alpaca-only
rule — there is no Alpaca guider device type. The guider service encapsulates
this protocol so `rp` speaks only HTTP.

Guider operations are exposed as built-in actions (`start_guiding`,
`stop_guiding`, `dither`, `pause_guiding`, `resume_guiding`,
`get_guiding_stats`). `rp` proxies these action calls to the guider service's
HTTP API. This means workflow plugins (e.g., a meridian flip plugin) can
control guiding through the same action mechanism as any other equipment.
Swapping in a different guiding backend requires only a different guider
service that implements the same HTTP endpoints.

### Plate Solver

The plate solver is a plugin service that accepts FITS files and returns
solved coordinates. It is exposed as a built-in action (`plate_solve`)
so that workflow plugins (e.g., centering) can use it. `rp` proxies the
call to the configured plate solver service. The plate solver can also
subscribe to `exposure_complete` events for background solving.

> **Note:** The choice of plate solving engine requires further research.
> The first implementation should wrap an open-source, cross-platform, locally
> available solver. Candidates include ASTAP and a local astrometry.net
> installation. This decision will be captured in a separate ADR.

### File Accessibility

Plugins and `rp` are assumed to share a filesystem (local paths
work). Distributed deployments where plugins run on separate machines are a
future concern and out of scope for the initial design.

## Orchestration Engine

The engine is an async state machine that coordinates the imaging workflow
across multiple cameras on a shared mount.

### Mount-Level State Machine

```
                    ┌──────────┐
         ┌─────────│  IDLE     │◄──────────────────┐
         │         └────┬─────┘                    │
         │              │ planner decides           │
         │              │ next target               │
         │         ┌────▼─────┐                    │
         │         │ SLEWING  │                    │
         │         └────┬─────┘                    │
         │              │ mount reports done        │
         │         ┌────▼──────┐                   │
         │         │ CENTERING │◄──┐               │
         │         └────┬──────┘   │               │
         │              │          │ error > threshold
         │              │ solved   │ (retry)        │
         │         ┌────▼──────┐   │               │
         │         │ check err ├───┘               │
         │         └────┬──────┘                   │
         │              │ error < threshold         │
         │         ┌────▼─────┐                    │
         │         │ FOCUSING │                    │
         │         └────┬─────┘                    │
         │              │ focus complete            │
         │         ┌────▼──────────┐               │
         │         │ GUIDE_START   │               │
         │         └────┬──────────┘               │
         │              │ guider settled            │
         │         ┌────▼──────┐                   │
         │         │ CAPTURING │───────────────────┘
         │         └───────────┘  target complete
         │                        or planner switches
         │
    SAFETY_OVERRIDE (can interrupt any state)
         │
    ┌────▼─────┐
    │ PARKING  │──► abort exposures, stop guiding, park mount
    └──────────┘
```

#### Workflow Delegation

The CENTERING and FOCUSING states are implemented as workflow plugin
delegations (see Action System). When the orchestrator enters one of
these states, it invokes the configured workflow plugin, which drives the
sub-workflow by calling actions on `rp`. The state machine waits for the
workflow plugin to complete before transitioning to the next state.

If no workflow plugin is configured for a phase, `rp` uses a built-in
default implementation (iterative plate-solve-and-correct for centering,
V-curve for focusing). The defaults use the same action primitives that
plugins use — they are simply bundled with `rp`.

### Per-Camera State Machine (during CAPTURING)

Each camera runs its own capture loop within the mount-level CAPTURING state:

```
┌──────┐   next exposure    ┌──────────┐   shutter closed   ┌─────────┐
│ IDLE ├───────────────────►│ EXPOSING ├────────────────────►│ READING │
└──▲───┘                    └──────────┘                     └────┬────┘
   │                                                              │
   │         ┌────────────┐   readout complete                    │
   └─────────┤ PROCESSING │◄──────────────────────────────────────┘
             └────────────┘
              (parallel: next exposure can start immediately)
```

The PROCESSING state runs in parallel with the next EXPOSING state. This is
where the post-capture pipeline fires — save FITS, emit `exposure_complete`
event, and let plugins do their work. The camera does not wait for processing
to finish before starting the next exposure.

### Multi-Camera Barrier Synchronization

Mount-level operations (slew, meridian flip) and guiding operations (dither)
require all cameras to be idle. This is a barrier:

```
Camera A (300s): [========expose========][idle]
Camera B (120s): [==expose==][==expose==][idle]
                                         ↑ barrier: all idle
                                         [dither / slew / flip]
```

The orchestrator enforces the barrier:

1. When a mount operation or dither is pending, cameras that finish early
   enter IDLE and wait rather than starting a new exposure.
2. A camera does NOT start a new exposure if it would extend past the
   earliest pending barrier time.
3. Once all cameras are idle, the mount operation executes.
4. After the operation completes, all cameras resume.

Barrier triggers:
- **Dither**: configurable interval (e.g., every N exposures of the longest
  camera). All cameras must be idle.
- **Meridian flip**: when the mount approaches the meridian limit. All cameras
  must be idle. After flip: re-center, re-focus, re-start guiding.
- **Target switch**: when the planner decides to move to a new target. All
  cameras must be idle.

## Dynamic Planner

The planner is a pure function: given current state, it produces the next
action. No user input is required during a session.

### Inputs

- **Target list**: targets with desired filter/exposure combinations and total
  integration time per filter
- **Progress**: how many exposures of each filter have been captured per target
- **Sky state**: target altitude, hour angle, azimuth (computed from
  coordinates and time)
- **Meridian**: time until meridian flip required
- **Camera state**: which cameras are available, their current filter, cooldown
  status
- **Moon**: distance from each target (optional, for prioritization)
- **Constraints**: minimum altitude, maximum airmass, dawn time

### Decision Logic

The planner evaluates candidates and selects the best action:

1. Eliminate targets below minimum altitude or that will set before a
   minimum number of exposures can be taken.
2. Prefer targets that are transiting (highest altitude, best seeing).
3. Prefer targets with the least progress toward their integration goal.
4. Minimize filter changes (batch same-filter exposures).
5. Account for meridian flip timing — avoid starting a long exposure if a
   flip is imminent.
6. If no targets are viable, wait or end the session.

The planner runs after each exposure completes, after each target switch, and
when conditions change (safety, temperature-triggered refocus).

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

On startup, the application checks for an existing session state file:

1. If no session file exists, start fresh (wait for user to start a session).
2. If a session file exists and the session is still valid (nighttime, targets
   remaining), resume automatically:
   - Reconnect to all equipment.
   - Verify mount position (plate solve to confirm pointing).
   - Re-acquire guiding.
   - Continue from the next planned exposure.
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

Safety monitoring is a top-level concern that can override any state.

### SafetyMonitor Polling

`rp` polls configured ASCOM Alpaca SafetyMonitor devices at a configurable
interval. On an unsafe transition:

1. Abort all in-progress exposures (discard partial frames).
2. Stop guiding.
3. Park the mount.
4. Persist session state.
5. Emit `safety_changed` event.
6. Enter PARKED state and wait.

On a safe transition while in PARKED state:
1. Unpark mount.
2. Verify conditions (is the previous target still viable?).
3. Resume session from persisted state.

### Sentinel Watchdog Integration

Sentinel is extended beyond safety monitoring to serve as an operation watchdog
and supervisor for the entire system. It subscribes to `rp`'s event bus and monitors operation deadlines.

#### Monitored Operations

| Operation | Starts on event | Expected completion | Timeout = |
|-----------|----------------|--------------------|----|
| Exposure | `exposure_started` | `exposure_complete` | duration + configurable buffer |
| Slew | `slew_started` | `slew_complete` | configurable max slew time |
| Focus | `focus_started` | `focus_complete` | configurable max focus time |
| Guide settle | `guide_started` | `guide_settled` | configurable settle timeout |
| Centering | `centering_started` | `centering_complete` | configurable max attempts * solve time |

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
- `GET /api/documents/{id}` — full document with all sections
- `POST /api/documents/{id}/sections` — add/update a section (plugin endpoint)

#### Plugins
- `POST /api/plugins/{event_id}/complete` — plugin completion callback
  (status, optional `skip_to` and `reason`)

#### Actions
- `GET /api/actions/catalog` — list all available actions (built-in and
  plugin-provided)
- `POST /api/actions/{workflow_id}/execute` — execute an action within a
  workflow context (called by workflow plugins)

#### System
- `GET /health` — health check
- `GET /api/events/subscribe` — WebSocket or SSE stream of real-time events

### Real-Time Stream

The `/api/events/subscribe` endpoint provides a WebSocket (or SSE) connection
that streams all events in real time. UIs connect here for live updates. The
stream includes the same events that are delivered to plugin webhooks.

This is the primary mechanism for UI updates. The UI does not poll — it
receives push updates over the stream.

## Configuration

All configuration is in a single JSON file. Equipment is listed with Alpaca
connection details. Plugins register their webhook URLs and command endpoints.

```json
{
  "session": {
    "data_directory": "/data/lights",
    "session_state_file": "/data/session_state.json",
    "file_naming_pattern": "{target}_{filter}_{duration}s_{sequence:04}"
  },
  "equipment": {
    "cameras": [
      {
        "id": "main-cam",
        "name": "Main Imaging Camera",
        "alpaca_url": "http://localhost:11120",
        "device_type": "camera",
        "device_number": 0,
        "cooler_target_c": -10,
        "gain": 100,
        "offset": 50
      },
      {
        "id": "guide-cam",
        "name": "Secondary / Wide field Camera",
        "alpaca_url": "http://localhost:11121",
        "device_type": "camera",
        "device_number": 0,
        "cooler_target_c": -10,
        "gain": 200,
        "offset": 30
      }
    ],
    "mount": {
      "alpaca_url": "http://localhost:11122",
      "device_number": 0,
      "settle_time_secs": 2
    },
    "focusers": [
      {
        "id": "main-focuser",
        "camera_id": "main-cam",
        "alpaca_url": "http://localhost:11113",
        "device_number": 0
      },
      {
        "id": "guide-focuser",
        "camera_id": "guide-cam",
        "alpaca_url": "http://localhost:11113",
        "device_number": 1
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
    ]
  },
  "guider": {
    "url": "http://localhost:11130",
    "settle_threshold_arcsec": 0.8,
    "settle_time_secs": 10,
    "dither_pixels": 5,
    "dither_every_n_exposures": 3
  },
  "plate_solver": {
    "url": "http://localhost:11131",
    "timeout_secs": 60
  },
  "plugins": [
    {
      "name": "image-analyzer",
      "type": "event",
      "webhook_url": "http://localhost:11140/webhook",
      "subscribes_to": ["exposure_complete"],
      "barriers": ["target_switch", "filter_switch"]
    },
    {
      "name": "cloud-backup",
      "type": "event",
      "webhook_url": "http://localhost:11141/webhook",
      "subscribes_to": ["exposure_complete", "session_stopped"]
    },
    {
      "name": "vcurve-focus",
      "type": "workflow",
      "workflow_url": "http://localhost:11150/workflow",
      "handles": ["focusing"],
      "requires_actions": ["capture", "move_focuser", "get_focuser_position", "measure_basic"],
      "provides_actions": [],
      "max_duration_secs": 300
    },
    {
      "name": "iterative-centering",
      "type": "workflow",
      "workflow_url": "http://localhost:11151/workflow",
      "handles": ["centering"],
      "requires_actions": ["capture", "slew", "sync_mount", "plate_solve"],
      "provides_actions": [],
      "max_duration_secs": 180
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
    "bind_address": "0.0.0.0"
  }
}
```

## Module Structure

```
services/rp/src/
  main.rs               CLI entry point (clap + tracing)
  lib.rs                Public API, AppBuilder, module declarations
  config.rs             Configuration types + load_config()
  error.rs              AppError enum (thiserror)

  # Core domain
  document.rs           ExposureDocument, Section, persistence (sidecar JSON)
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

  # Services (non-Alpaca integrations, backing built-in actions)
  services/
    mod.rs              Service trait, service manager
    guider.rs           Guider service client (backs start/stop/dither actions)
    plate_solver.rs     Plate solver client (backs plate_solve action)

  # Orchestration
  engine/
    mod.rs              Engine: top-level orchestrator, owns state machine
    state.rs            Mount-level state machine (Idle, Slewing, Centering, ...)
    capture.rs          Per-camera capture loop (Idle, Exposing, Reading, Processing)
    barrier.rs          Multi-camera barrier synchronization
    centering.rs        Plate solve + correct pointing loop
    focusing.rs         Auto-focus routine
    safety.rs           Safety monitoring + park/resume logic

  # Planning
  planner/
    mod.rs              Planner: evaluate candidates, select next action
    sky.rs              Altitude, azimuth, hour angle, meridian calculations
    scorer.rs           Target scoring (altitude, progress, priority, filter)

  # Event system
  events/
    mod.rs              Event types, EventBus
    webhook.rs          Webhook delivery (fire-and-forget HTTP POST)

  # Action system
  actions/
    mod.rs              Action registry, catalog builder, config-time validation
    built_in.rs         Built-in action implementations (capture, move_focuser, etc.)
    router.rs           Routes action calls to built-in or plugin-provided handlers

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

Testing follows the conventions in `docs/testing-rules.md`.

### Unit Tests

- **Planner**: Given a target list, progress, and sky state, assert correct
  target/filter selection. Pure function, easy to test exhaustively.
- **State machine**: Assert correct transitions, barrier behavior, safety
  overrides.
- **Document**: Serialization round-trips, section merging, atomic persistence.
- **Configuration**: Deserialization, validation, defaults.
- **Sky calculations**: Altitude, hour angle, meridian time against known
  ephemeris data.

### BDD Tests (Cucumber)

Behavioral specifications for the orchestration workflows:

- Session lifecycle (start, capture, pause, resume, stop)
- Multi-camera barrier synchronization
- Safety override during capture
- Meridian flip sequence
- Dynamic target switching
- Power failure recovery

### Integration Tests

- API endpoint tests with mock equipment and plugins
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
- **Flat/dark frame automation** — calibration frame capture sequences
- **Mosaic planning** — multi-panel target definitions
