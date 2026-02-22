# Sentinel Service

## Overview

Sentinel is an observatory monitoring and notification service. It polls ASCOM Alpaca devices via their HTTP API, detects state transitions (safe/unsafe), and sends notifications through configurable channels. It also provides a web dashboard for real-time status viewing.

Unlike other services in this workspace, sentinel is **not** an ASCOM Alpaca server — it is a client/consumer that monitors other ASCOM devices.

## Architecture

```
Config (JSON)
     |
     v
  main.rs --- CLI (clap) + tracing
     |
     v
  Engine --- orchestrator: owns monitors, notifiers, shared state
   +--+--+
   |     |
   v     v
Monitor trait          Notifier trait
   |                      |
   v                      v
AlpacaSafetyMonitor   PushoverNotifier
(reqwest GET/PUT)     (reqwest POST)

SharedState (Arc<RwLock>) <-- Engine updates
     |
     v
Dashboard (axum + Leptos SSR) --- reactive web UI
```

### Key Traits

- **`Monitor`** — `poll() -> MonitorState`, `connect()`, `disconnect()`. First implementation: `AlpacaSafetyMonitor`.
- **`Notifier`** — `notify(notification)`. First implementation: `PushoverNotifier`.
- **`HttpClient`** — wraps `reqwest` for testability (mockall in tests). Used by both monitors and notifiers.

### State Flow

1. Engine starts, connects each monitor (PUT `connected=true`)
2. Engine spawns a tokio polling task per monitor at configured interval
3. Each poll: GET `issafe`, compare with stored state, if transition matches a configured rule, dispatch to notifiers
4. SharedState updated on every poll, dashboard reads from it
5. On shutdown: disconnect monitors (PUT `connected=false`)

## Configuration

Configuration is loaded from a JSON file. All sections are optional with sensible defaults.

See `examples/config.json` for a complete example.

### Monitor Types

- `alpaca_safety_monitor` — Polls an ASCOM Alpaca SafetyMonitor device

### Notifier Types

- `pushover` — Sends push notifications via the Pushover API

### Environment Variable Overrides

Pushover credentials can be provided (or overridden) via environment variables so that secrets do not need to be committed to the config file:

| Variable | Overrides |
|----------|-----------|
| `PUSHOVER_API_TOKEN` | `api_token` in Pushover notifier config |
| `PUSHOVER_USER_KEY` | `user_key` in Pushover notifier config |

When set and non-empty, environment variables take precedence over JSON config values. When using environment variables exclusively, the credentials can be omitted from the JSON config entirely.

After resolution, sentinel returns a configuration error if either field is still empty.

**Usage with 1Password CLI:**

Create a `.env` file (already in `.gitignore`) with `op://` secret references:

```
PUSHOVER_API_TOKEN=op://Personal/Pushover/add more/sentinel app key
PUSHOVER_USER_KEY=op://Personal/Pushover/add more/user key
```

Then run sentinel with `op run` to inject the resolved values:

```bash
op run --env-file .env -- cargo run -p sentinel -- -c services/sentinel/examples/config.json -l debug
```

### Transition Rules

Transitions define when notifications should be sent. Each rule specifies:
- Which monitor to watch (`monitor_name`)
- Which direction of change (`safe_to_unsafe`, `unsafe_to_safe`, or `both`)
- Which notifiers to use
- A message template with `{monitor_name}` and `{new_state}` placeholders
- Optional priority and sound overrides

Empty transitions config means no notifications are sent.

## Dashboard

The web dashboard runs on port 11114 (configurable) and provides:

- **Web UI**: Server-rendered HTML with JavaScript polling (auto-refreshes every 5 seconds)
- **JSON API**: `/api/status` (monitor statuses), `/api/history` (notification history)
- **Health check**: `/health` returns 200 OK

The `sentinel-app` crate contains Leptos components for a future WASM-hydrated frontend. Currently the dashboard uses server-rendered HTML with vanilla JavaScript for simplicity. The Leptos components are available for `cargo-leptos` builds.

## Module Structure

```
services/sentinel/src/
  main.rs              CLI entry point
  lib.rs               Module declarations + run() orchestrator
  config.rs            Config types + load_config()
  error.rs             SentinelError + Result alias
  io.rs                HttpClient trait + ReqwestHttpClient
  monitor.rs           Monitor trait + MonitorState + StateChange
  alpaca_client.rs     AlpacaSafetyMonitor (Monitor impl)
  notifier.rs          Notifier trait + Notification types
  pushover.rs          PushoverNotifier (Notifier impl)
  engine.rs            Engine: polling loops + transition matching + dispatch
  state.rs             SharedState: monitor statuses + notification history
  dashboard.rs         axum Router: HTML dashboard + JSON API
```

## Edge Cases

| Scenario | Behavior |
|----------|----------|
| Initial state (first poll) | Unknown to Safe/Unsafe. No notification by default. |
| Device unreachable | Returns MonitorState::Unknown. No notification. Increments error counter. |
| Device recovers | Unknown to Safe/Unsafe can trigger notification if configured. Error counter resets. |
| Pushover failure | Log warn, record failure in history. No retry. |
| ASCOM error response | Treated as Unknown (same as unreachable). |
| Rapid flapping | Every real transition triggers notification. |
| Empty transitions | Valid config. Monitors run, dashboard works, no notifications. |
| Dashboard port conflict | Log error, continue without dashboard. |

## Running

```bash
# With config file
cargo run -p sentinel -- -c services/sentinel/examples/config.json

# With CLI overrides
cargo run -p sentinel -- -c config.json --dashboard-port 8080

# Debug logging
cargo run -p sentinel -- -c config.json -l debug
```

## Port

11114 (dashboard, configurable)
