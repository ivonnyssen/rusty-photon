# Sentinel

Observatory monitoring and notification service. Polls ASCOM Alpaca SafetyMonitor devices, detects safe/unsafe state transitions, sends push notifications, and serves a live web dashboard.

Unlike other services in this workspace, sentinel is **not** an ASCOM Alpaca server — it is a client that monitors other ASCOM devices.

## Quick Start

1. Copy and edit the example configuration:

```bash
cp services/sentinel/examples/config.json my-config.json
# Edit my-config.json with your device addresses and Pushover credentials
```

2. Run the service:

```bash
cargo run -p sentinel -- -c my-config.json
```

3. Open the dashboard at [http://127.0.0.1:11114](http://127.0.0.1:11114).

## CLI Options

```
sentinel [OPTIONS]

Options:
  -c, --config <PATH>            Path to JSON configuration file
      --dashboard-port <PORT>    Dashboard port (overrides config file)
  -l, --log-level <LEVEL>        Log level [default: info]
                                 Values: trace, debug, info, warn, error
  -h, --help                     Print help
  -V, --version                  Print version
```

If no config file is provided, sentinel starts with an empty default configuration (no monitors, no notifiers, dashboard on port 11114).

## Configuration

Configuration is a JSON file with four optional sections. See [`examples/config.json`](examples/config.json) for a complete example.

### Monitors

Monitors poll ASCOM Alpaca devices at a configured interval.

```json
{
  "monitors": [
    {
      "type": "alpaca_safety_monitor",
      "name": "Roof Safety Monitor",
      "host": "localhost",
      "port": 11111,
      "device_number": 0,
      "polling_interval_seconds": 30
    }
  ]
}
```

| Field | Default | Description |
|-------|---------|-------------|
| `type` | *(required)* | Monitor type. Currently only `alpaca_safety_monitor`. |
| `name` | *(required)* | Display name, referenced in transition rules. |
| `host` | `localhost` | ASCOM Alpaca device hostname or IP. |
| `port` | `11111` | ASCOM Alpaca device port. |
| `device_number` | `0` | ASCOM device number. |
| `polling_interval_seconds` | `30` | How often to poll the device (seconds). |

### Notifiers

Notifiers deliver alerts when transitions occur.

```json
{
  "notifiers": [
    {
      "type": "pushover",
      "api_token": "your-app-token",
      "user_key": "your-user-key",
      "default_title": "Observatory Alert",
      "default_priority": 0,
      "default_sound": "pushover"
    }
  ]
}
```

| Field | Default | Description |
|-------|---------|-------------|
| `type` | *(required)* | Notifier type. Currently only `pushover`. |
| `api_token` | *(required)* | Pushover application API token. |
| `user_key` | *(required)* | Pushover user/group key. |
| `default_title` | `Observatory Alert` | Default notification title. |
| `default_priority` | `0` | Default Pushover priority (-2 to 2). |
| `default_sound` | `pushover` | Default notification sound. |

### Transitions

Transitions define *when* to send notifications. Each rule watches a monitor and fires when the state changes in the specified direction.

```json
{
  "transitions": [
    {
      "monitor_name": "Roof Safety Monitor",
      "direction": "safe_to_unsafe",
      "notifiers": ["pushover"],
      "message_template": "ALERT: {monitor_name} changed to {new_state}",
      "priority": 1,
      "sound": "siren"
    }
  ]
}
```

| Field | Default | Description |
|-------|---------|-------------|
| `monitor_name` | *(required)* | Name of the monitor to watch (must match a monitor's `name`). |
| `direction` | *(required)* | `safe_to_unsafe`, `unsafe_to_safe`, or `both`. |
| `notifiers` | *(required)* | List of notifier type names to dispatch to. |
| `message_template` | `{monitor_name} changed to {new_state}` | Template with `{monitor_name}` and `{new_state}` placeholders. |
| `priority` | *(none)* | Override the notifier's default priority for this rule. |
| `sound` | *(none)* | Override the notifier's default sound for this rule. |

If no transitions are configured, monitors still run and the dashboard works, but no notifications are sent.

### Dashboard

```json
{
  "dashboard": {
    "enabled": true,
    "port": 11114,
    "history_size": 100
  }
}
```

| Field | Default | Description |
|-------|---------|-------------|
| `enabled` | `true` | Enable or disable the web dashboard. |
| `port` | `11114` | HTTP port for the dashboard. |
| `history_size` | `100` | Number of notification history entries to keep. |

## Dashboard & API

When the dashboard is enabled, the following endpoints are available:

| Endpoint | Description |
|----------|-------------|
| `GET /` | Web UI showing monitor statuses and notification history (auto-refreshes every 5s). |
| `GET /api/status` | JSON array of monitor statuses. |
| `GET /api/history` | JSON array of recent notification records. |
| `GET /health` | Returns `200 OK` — useful for health checks. |

## Design Documentation

For architecture details, module structure, edge-case behavior, and implementation notes, see the [design document](../../docs/services/sentinel.md).
