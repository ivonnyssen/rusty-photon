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
                       │  (Leptos/WASM or  │
                       │   any framework)  │
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
| `frame_rejected` | document_id, plugin, reason | Immediate correction rejected a frame |
| `plugin_timeout` | plugin, event_id | Plugin did not respond within `max_duration` |
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

The catalog is built at startup from two sources:

1. **Built-in tools** — hardware primitives, guider operations, and basic
   compute provided by `rp` itself.
2. **Plugin-provided tools** — compute or analysis tools aggregated from
   plugins that run their own MCP servers.

Workflow plugins discover available tools via the standard MCP
`tools/list` call. Each tool includes its JSON Schema, so plugins know
the exact parameter types and return structure.

### Built-in Tools

**Hardware**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `capture` | camera_id, duration, binning | image_path, document_id | Take an exposure, download `image_array`, save FITS file, create exposure document |
| `get_camera_info` | camera_id | max_adu, exposure_min, exposure_max, sensor_x, sensor_y, bin_x, bin_y | Read camera capabilities and current settings |
| `move_focuser` | focuser_id, position | actual_position | Move focuser to absolute position |
| `get_focuser_position` | focuser_id | position | Read current focuser position |
| `get_focuser_temperature` | focuser_id | temperature_c | Read focuser temperature sensor |
| `slew` | ra, dec | actual_ra, actual_dec | Slew mount to coordinates (blocks until settled) |
| `sync_mount` | ra, dec | — | Sync mount position to given coordinates |
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

**Compute**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `plate_solve` | image_path, hint (optional) | ra, dec, rotation, scale | Solve an image via the configured plate solver service |
| `measure_basic` | image_path | hfr, star_count, background_mean, background_stddev | Compute basic image statistics |
| `compute_image_stats` | image_path, document_id (optional) | median_adu, mean_adu, min_adu, max_adu, pixel_count | Read FITS file, compute pixel statistics, update exposure document |

**Planner**

| Action | Parameters | Returns | Description |
|--------|-----------|---------|-------------|
| `get_next_target` | — | target, filter, duration, reason | Evaluate candidates and recommend next target/filter |
| `get_target_status` | target_name | altitude, hour_angle, time_to_set, progress | Sky position and progress for a target |
| `get_meridian_status` | — | time_to_flip, side_of_pier | Time until meridian flip needed |
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
downloads the camera's `image_array`, writes it as a FITS file (using the
`fitrs` crate with BITPIX=32), and creates a sidecar exposure document
JSON alongside it. The FITS file and document are written atomically
(write to temp, rename).

#### CoverCalibrator Tool Details

The CoverCalibrator tools control flat panel devices. `calibrator_on`
accepts an optional `brightness` parameter (0 to `max_brightness`). When
omitted, the calibrator is turned on at maximum brightness. All four
tools block until the operation completes by polling the device state
(same pattern as `set_filter`).

#### Image Statistics Tool Details

`compute_image_stats` reads a FITS file by path, flattens the pixel
data, and computes median, mean, min, and max ADU values. If a
`document_id` is provided, the stats are written into the exposure
document as an `"image_stats"` section. This tool does not access the
camera — it operates on saved image files.

### Image Analysis Strategy

Image analysis in `rp` follows a **pure Rust on ndarray** approach.
All algorithms are implemented as custom code on top of well-established
building blocks — no single crate covers the full range of astronomical
image analysis needed.

#### Current Capabilities

- **Pixel statistics** (median, mean, min, max ADU) — stdlib iterators
  and `select_nth_unstable` for median (iterative O(n) quickselect).
  Used by `compute_image_stats` for flat calibration exposure targeting.
- **FITS I/O** — `fitrs` crate for reading and writing FITS images.

#### Planned Capabilities and Crate Strategy

| Capability | Approach | Crates |
|------------|----------|--------|
| Pixel statistics | Custom | stdlib (`select_nth_unstable`, iterators) |
| FITS I/O | Crate | `fitrs` |
| 2D image operations | Crate | `ndarray` (already transitive via ascom-alpaca) |
| Gaussian smoothing, morphology | Crate | `ndarray-ndimage` (Gaussian filter, connected components, dilation/erosion) |
| Star detection | Custom | Threshold + connected components (`ndarray-ndimage` or `lutz`) + shape filtering |
| Centroiding | Custom | Intensity-weighted center of mass on ndarray subframes |
| HFR / HFD | Custom | Radial flux accumulation (~20 lines of math) |
| FWHM | Custom + crate | Gaussian fitting via `levenberg-marquardt` or `rmpfit` |
| Eccentricity / elongation | Custom | Second central moments from detected star pixels |
| Background estimation | Custom | Sigma-clipped mesh statistics on ndarray |
| Noise / SNR | Custom | Sigma-clipped statistics |

#### Design Rationale

This approach follows what N.I.N.A. does: custom astronomical algorithms
on top of general-purpose image processing primitives. The algorithms
(HFR, centroiding, eccentricity) are well-documented and not complex.
SEP (Source Extractor as a library) was considered via `sep-sys` but
rejected due to LGPL license constraints and C FFI maintenance burden.

### Plugin-Provided Tools

A plugin can provide additional tools by running its own MCP server. At
startup, `rp` connects to each tool-providing plugin as an MCP client,
discovers their tools via `tools/list`, and proxies them through its own
MCP server. Workflow plugins see a single unified catalog — they don't
know or care whether a tool is built-in or provided by another plugin.

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

Example: a specialized measurement plugin provides tools that a focus
plugin can use:

| Tool | Provider | Description |
|------|----------|-------------|
| `measure_eccentricity` | star-analyzer | Measure star eccentricity across the field |
| `measure_wavefront` | wavefront-analyzer | Wavefront error analysis |

**All tool results that produce image metrics MUST be written into the
exposure document as a section.** This is the one rule — the document is
the shared data bus. `rp` enforces this: compute tool results are merged
into the document before being returned to the caller.

### Plugin Types

There are three plugin types:

| Type | Role | Interface |
|------|------|-----------|
| **Event** | React to events asynchronously | Webhook (receive events, post completion) |
| **Tool provider** | Provide compound tools for other plugins | MCP server (rp aggregates their tools) |
| **Orchestrator** | Drive the imaging session | MCP client (calls tools on rp) |

A plugin can combine types. For example, a focus plugin can be a
**tool provider** (exposes `auto_focus` tool) and also an **event
plugin** (subscribes to `temperature_changed` to track focus drift).

#### Tool Provider Registration

Tool providers run their own MCP servers. `rp` connects at startup,
discovers their tools, and proxies them through its own MCP server:

```json
{
  "name": "vcurve-focus",
  "type": "tool_provider",
  "mcp_server_url": "http://localhost:11150/mcp",
  "requires_tools": ["capture", "move_focuser", "get_focuser_position", "measure_basic"]
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

#### Example: V-Curve Focus Tool Provider

The `vcurve-focus` plugin exposes an `auto_focus` tool. When called by
the orchestrator (via rp's proxy), it drives the V-curve internally:

```
Orchestrator calls: tools/call auto_focus {camera_id: "main-cam", focuser_id: "main-focuser"}
  → rp proxies to vcurve-focus plugin's MCP server

  vcurve-focus connects to rp as MCP client and drives the V-curve:
    → tools/call  move_focuser {focuser_id: "main-focuser", position: 10000}
    ← {actual_position: 10000}
    → tools/call  capture {camera_id: "main-cam", duration: "2s"}
    ← {image_path: "/tmp/focus_001.fits", document_id: "doc-001"}
    → tools/call  measure_basic {image_path: "/tmp/focus_001.fits"}
    ← {hfr: 5.2, star_count: 340}
    → tools/call  move_focuser {focuser_id: "main-focuser", position: 10200}
    ... 12 more iterations ...
    → tools/call  move_focuser {focuser_id: "main-focuser", position: 12450}

  vcurve-focus returns to rp:
    ← {best_position: 12450, best_hfr: 2.1, curve_points: 15}

  rp returns to orchestrator:
    ← {best_position: 12450, best_hfr: 2.1, curve_points: 15}
```

#### Example: Iterative Centering Tool Provider

The `iterative-centering` plugin exposes a `center_on_target` tool:

```
Orchestrator calls: tools/call center_on_target {ra: 10.6847, dec: 41.2689, tolerance: 5}
  → rp proxies to centering plugin's MCP server

  centering plugin connects to rp and drives the loop:
    → tools/call  capture {camera_id: "main-cam", duration: "5s"}
    ← {image_path: "/tmp/center_001.fits"}
    → tools/call  plate_solve {image_path: "/tmp/center_001.fits"}
    ← {ra: 10.6820, dec: 41.2650, error_arcsec: 45}
    → tools/call  sync_mount {ra: 10.6820, dec: 41.2650}
    → tools/call  slew {ra: 10.6847, dec: 41.2689}
    → repeat until error < tolerance ...

  centering plugin returns:
    ← {final_error_arcsec: 2.1, attempts: 3}
```

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
   discovered plugin-provided tools. Reject duplicate tool names —
   no two providers (including built-ins) may expose the same tool
   name.
3. For each plugin with `requires_tools`, verify that every listed
   tool exists in the catalog.
4. If validation fails, `rp` refuses to start and reports the missing
   or duplicate tools.

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

### Guider Service

The guider service wraps PHD2 and exposes an HTTP API. The existing
`phd2-guider` library provides the PHD2 JSON-RPC integration and will be
reworked to run as an HTTP service.

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

The plate solver is a plugin service that accepts FITS files and returns
solved coordinates. It is exposed as a built-in MCP tool (`plate_solve`)
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

### Compound Tools (Sub-Workflow Plugins)

Sub-workflows like focusing and centering are implemented as
**tool-provider plugins**. They run their own MCP servers and expose
high-level compound tools. Internally, they call back to `rp`'s MCP
server to use primitive tools.

```
Orchestrator                    rp                     Focus Plugin
    │                           │                           │
    │  tools/call auto_focus    │                           │
    ├──────────────────────────►│  tools/call auto_focus    │
    │                           ├──────────────────────────►│
    │                           │                           │
    │                           │  tools/call move_focuser  │
    │                           │◄──────────────────────────┤
    │                           │  ← {actual_position}      │
    │                           ├──────────────────────────►│
    │                           │                           │
    │                           │  tools/call capture       │
    │                           │◄──────────────────────────┤
    │                           │  ← {image_path}           │
    │                           ├──────────────────────────►│
    │                           │                           │
    │                           │  ... repeat ...           │
    │                           │                           │
    │                           │  ← {best_position, hfr}   │
    │  ← {best_position, hfr}  │◄──────────────────────────┤
    │◄──────────────────────────┤                           │
```

This keeps the orchestrator simple — it calls `auto_focus` without
knowing the focus algorithm. The focus plugin can be swapped (V-curve,
quadratic, FWHM-based) without changing the orchestrator.

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

1. Eliminate targets below minimum altitude or that will set before a
   minimum number of exposures can be taken.
2. Prefer targets that are transiting (highest altitude, best seeing).
3. Prefer targets with the least progress toward their integration goal.
4. Minimize filter changes (batch same-filter exposures).
5. Account for meridian flip timing — avoid starting a long exposure if a
   flip is imminent.
6. If no targets are viable, return a "wait" or "end session"
   recommendation.

The orchestrator decides when to call `get_next_target` — typically
after each exposure, after each target switch, or when conditions change.

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
- `POST /api/plugins/{id}/complete` — plugin completion callback
  (status, optional `correction`). The `{id}` is the `event_id` for
  event plugins or the `workflow_id` for orchestrators — both use the
  same endpoint.

#### MCP
- `/mcp` — MCP server endpoint (streamable HTTP transport). Workflow
  plugins connect here as MCP clients to discover and call tools.

#### System
- `GET /health` — health check
- `GET /api/events/subscribe` — WebSocket or SSE stream of real-time events

### Real-Time Stream

The `/api/events/subscribe` endpoint provides a WebSocket (or SSE) connection
that streams all events in real time. Any consumer that needs live events
connects here — UIs for rendering state, and monitoring services like
Sentinel for tracking operation deadlines. The stream includes the same
events that are delivered to plugin webhooks.

This is the primary mechanism for passive consumers. Clients receive push
updates over the stream without the overhead of the webhook
ack/completion protocol.

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
        "alpaca_url": "https://localhost:11120",
        "device_type": "camera",
        "device_number": 0,
        "cooler_target_c": -10,
        "gain": 100,
        "offset": 50,
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
      "barrier_gates": ["slew", "set_filter"]
    },
    {
      "name": "cloud-backup",
      "type": "event",
      "webhook_url": "http://localhost:11141/webhook",
      "subscribes_to": ["exposure_complete", "session_stopped"]
    },
    {
      "name": "vcurve-focus",
      "type": "tool_provider",
      "mcp_server_url": "http://localhost:11150/mcp",
      "requires_tools": ["capture", "move_focuser", "get_focuser_position", "measure_basic"]
    },
    {
      "name": "iterative-centering",
      "type": "tool_provider",
      "mcp_server_url": "http://localhost:11151/mcp",
      "requires_tools": ["capture", "slew", "sync_mount", "plate_solve"]
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
    cover_calibrator.rs CoverCalibrator wrapper (cover open/close, calibrator on/off)

  # Services (non-Alpaca integrations, backing built-in MCP tools)
  services/
    mod.rs              Service trait, service manager
    guider.rs           Guider service client (backs start/stop/dither tools)
    plate_solver.rs     Plate solver client (backs plate_solve tool)

  # Safety enforcement
  safety.rs             SafetyMonitor polling, park/resume, orchestrator cancellation

  # Planning (exposed as MCP tools)
  planner/
    mod.rs              Planner tool implementations (get_next_target, etc.)
    sky.rs              Altitude, azimuth, hour angle, meridian calculations
    scorer.rs           Target scoring (altitude, progress, priority, filter)

  # Event system
  events/
    mod.rs              Event types, EventBus
    webhook.rs          Webhook delivery (fire-and-forget HTTP POST)

  # MCP tool system
  mcp/
    mod.rs              MCP server setup, tool registry, config-time validation
    built_in.rs         Built-in tool implementations (capture, move_focuser, etc.)
    aggregator.rs       Connects to plugin MCP servers, proxies their tools

  # Imaging (FITS I/O and image analysis)
  imaging.rs            FITS read/write (fitrs), pixel statistics (stdlib), future: HFR, star detection

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
- **Sky calculations**: Altitude, hour angle, meridian time against known
  ephemeris data.

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
platform from the [v0.5.0 release](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators/releases/tag/v0.5.0),
extract it, and either add its directory to `PATH` or set one of the
environment variables above. In CI, the `.github/actions/install-omnisim`
composite action handles this automatically.

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
