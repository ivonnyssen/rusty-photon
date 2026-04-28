# calibrator-flats -- Calibrator Flat Calibration Orchestrator

## Overview

`calibrator-flats` is an orchestrator plugin that captures flat field correction
frames using a stable light source (flat panel, electroluminescent panel,
or light box) controlled via an ASCOM CoverCalibrator device. It connects
to `rp` as an MCP client, iteratively determines the correct exposure
time per filter to achieve the target ADU level, then captures the
requested number of flat frames at that duration.

### Tenets

1. **Target 50% well depth.** Flat frames must have a median pixel value
   close to 50% of the camera's maximum ADU for optimal calibration
   quality. The target fraction is configurable.
2. **Automate the entire lifecycle.** The orchestrator manages the
   CoverCalibrator (close cover, turn on light, turn off, open cover)
   so the user only needs to start the session.
3. **Safe cleanup on failure.** If the workflow fails at any point, the
   calibrator is turned off and the cover is opened before the
   orchestrator exits.
4. **Per-filter optimization.** Each filter has different throughput. The
   exposure time is found independently for each filter in the plan.

## Architecture

`calibrator-flats` is a standalone HTTP service. `rp` invokes it as an
orchestrator plugin when a calibrator flats session starts. The plugin connects
back to `rp`'s MCP server and calls primitive tools.

```
  rp (equipment gateway)            calibrator-flats (orchestrator)
  ┌───────────────────┐             ┌───────────────────────┐
  │                   │ POST /invoke│                       │
  │  session start ───┼────────────►│  1. close_cover       │
  │                   │             │  2. calibrator_on     │
  │  MCP server  ◄────┼─────────────┤  3. per-filter loop:  │
  │  /mcp             │  tool calls │     find exposure     │
  │                   │             │     batch capture     │
  │  REST API    ◄────┼─────────────┤  4. calibrator_off    │
  │  /api/plugins/    │  completion │  5. open_cover        │
  │  {wf_id}/complete │             │  6. post completion   │
  └───────────────────┘             └───────────────────────┘
```

### Port

11170 (configurable)

## MCP Tools Used

The plugin calls these `rp` built-in MCP tools:

| Tool | Usage |
|------|-------|
| `get_camera_info` | Read `max_adu` to compute target ADU, read exposure limits for clamping |
| `capture` | Take exposures (both test exposures for calibration and final flat frames) |
| `compute_image_stats` | Measure median ADU of captured images for exposure time adjustment |
| `set_filter` | Switch filter wheel to the current filter in the plan |
| `close_cover` | Close the dust cover before starting flat calibration |
| `open_cover` | Open the dust cover after flat calibration completes |
| `calibrator_on` | Turn on the flat panel at the configured brightness |
| `calibrator_off` | Turn off the flat panel when done |

## Invocation Protocol

`rp` POSTs to the plugin's `/invoke` endpoint when a session starts:

```json
{
  "workflow_id": "wf-550e8400-e29b-41d4",
  "session_id": "session-2026-04-09",
  "mcp_server_url": "http://localhost:11115/mcp",
  "recovery": null
}
```

The plugin acknowledges with timing estimates:

```json
{
  "estimated_duration_secs": 300,
  "max_duration_secs": 600
}
```

The estimated duration is computed from the plan: number of filters,
frames per filter, and initial exposure time. The max duration adds
margin for the iterative exposure search.

## Algorithm

### Full Workflow

```
connect to rp MCP server at mcp_server_url

# 1. Query camera capabilities
info = get_camera_info(camera_id)
target_adu = info.max_adu * target_adu_fraction

# 2. Prepare flat panel
close_cover(calibrator_id)
calibrator_on(calibrator_id, brightness)

# 3. Capture flats per filter
for each filter in plan.filters:
    set_filter(filter_wheel_id, filter.name)

    # 3a. Find optimal exposure time for this filter
    duration = initial_duration
    converged = false
    for iteration in 1..=max_iterations:
        result = capture(camera_id, duration)
        stats = compute_image_stats(result.image_path, result.document_id)

        deviation = |stats.median_adu - target_adu| / target_adu
        if deviation <= tolerance:
            converged = true
            break

        # Adjust proportionally
        if stats.median_adu == 0:
            duration = duration * 2           # guard division by zero
        else:
            duration = duration * (target_adu / stats.median_adu)

        # Clamp to camera limits
        duration = clamp(duration, info.exposure_min_ms, info.exposure_max_ms)

    if not converged:
        log warning "exposure did not converge for filter {filter.name}"

    # 3b. Capture the requested number of flat frames
    for i in 1..=filter.count:
        capture(camera_id, duration)

# 4. Clean up
calibrator_off(calibrator_id)
open_cover(calibrator_id)

# 5. Post completion
POST /api/plugins/{workflow_id}/complete
{
  "status": "complete",
  "result": {
    "reason": "flat_calibration_complete",
    "filters_completed": [...],
    "total_frames": N
  }
}
```

### Exposure Time Convergence

The iterative search uses proportional adjustment:

```
new_duration = current_duration * (target_adu / measured_median)
```

This converges quickly because the relationship between exposure time
and signal level is linear for a stable light source. Typically 2-3
iterations suffice. The algorithm handles edge cases:

- **Saturated image** (median >= max_adu): duration is reduced
  dramatically by the ratio.
- **Very dark image** (median ~0): duration is doubled as a fallback
  to avoid division by zero.
- **Already close**: if within tolerance on the first attempt, no
  iteration is needed.

### Error Recovery

The workflow wraps the capture loop in a guard that ensures cleanup:

```rust
// Pseudocode
close_cover(calibrator_id);
calibrator_on(calibrator_id, brightness);

let result = run_capture_loop(...).await;

// Always clean up, even on error
calibrator_off(calibrator_id);
open_cover(calibrator_id);

result?; // propagate error after cleanup
```

If cleanup itself fails (e.g., device unreachable), the error is logged
but does not mask the original error.

## Configuration

The plugin reads its configuration from the invocation payload or from
its own config file. The plan is part of `rp`'s plugin configuration:

```json
{
  "name": "calibrator-flats",
  "type": "orchestrator",
  "invoke_url": "http://localhost:11170/invoke",
  "requires_tools": [
    "capture", "set_filter", "get_camera_info", "compute_image_stats",
    "close_cover", "open_cover", "calibrator_on", "calibrator_off"
  ],
  "config": {
    "camera_id": "main-cam",
    "filter_wheel_id": "main-fw",
    "calibrator_id": "flat-panel",
    "target_adu_fraction": 0.5,
    "tolerance": 0.05,
    "max_iterations": 10,
    "initial_duration": "1s",
    "brightness": null,
    "filters": [
      { "name": "Luminance", "count": 20 },
      { "name": "Red", "count": 20 },
      { "name": "Green", "count": 20 },
      { "name": "Blue", "count": 20 }
    ]
  }
}
```

### Configuration Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `camera_id` | string | required | Camera to use for flat exposures |
| `filter_wheel_id` | string | required | Filter wheel to use |
| `calibrator_id` | string | required | CoverCalibrator device to control |
| `target_adu_fraction` | float | 0.5 | Target median as fraction of max ADU |
| `tolerance` | float | 0.05 | Acceptable deviation from target (5%) |
| `max_iterations` | int | 10 | Max attempts to find correct exposure time per filter |
| `initial_duration` | humantime string | `"1s"` | Starting exposure time (e.g. `"500ms"`, `"1s"`) |
| `brightness` | int or null | null | Calibrator brightness (null = max_brightness) |
| `filters` | array | required | List of filters with frame counts |
| `filters[].name` | string | required | Filter name (must match filter wheel config) |
| `filters[].count` | int | required | Number of flat frames to capture for this filter |

## Module Structure

```
services/calibrator-flats/src/
  main.rs            CLI entry point (clap + tracing)
  lib.rs             Public API, ServerBuilder, module declarations
  config.rs          Configuration types (FlatPlan, FilterPlan)
  error.rs           Error types (thiserror)
  routes.rs          Axum router: POST /invoke
  workflow.rs        Flat calibration algorithm (iterative exposure + batch capture)
  mcp_client.rs      MCP client: rmcp Streamable HTTP client to rp's /mcp endpoint
```

## Testing Strategy

Testing follows the conventions in `docs/skills/testing.md`.

### BDD Tests (Cucumber)

BDD tests live in `services/calibrator-flats/tests/` and exercise the
full three-process topology (OmniSim + rp + calibrator-flats) end-to-end
via rp's REST API. The test harness comes from the `rp-harness` feature
of the `bdd-infra` workspace crate (`bdd_infra::rp_harness`), which
provides the OmniSim singleton, rp launcher, config builder, webhook
receiver, and MCP client.

Current scenarios (`tests/features/flat_calibration.feature`):

- Orchestrator captures flats and returns the session to `idle`
- Orchestrator emits an `exposure_complete` event per captured flat

Planned scenarios (not yet implemented):

- Median ADU of captured flats is within tolerance of 50% `max_adu`
- Cleanup on error (calibrator off, cover open)
- Graceful failure when camera or calibrator is unavailable

### Unit Tests

- Configuration deserialization and defaults
- Exposure time adjustment calculation (proportional scaling, clamping,
  divide-by-zero guard)
- MCP client tool call result deserialization

## Future Considerations

- **Brightness optimization**: Instead of only adjusting exposure time,
  the algorithm could also adjust the flat panel brightness to keep
  exposure times in an optimal range (avoiding very short exposures
  where shutter timing becomes significant).
- **Rotator-aware sequencing**: If a rotator is present, flats should be
  taken at the same rotator angle as the corresponding light frames.
- **Per-filter brightness**: Different filters may benefit from different
  panel brightness levels.
