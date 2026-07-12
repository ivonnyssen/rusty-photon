# Sentinel Service

## Overview

Sentinel is an observatory monitoring and notification service. It polls ASCOM Alpaca devices via their HTTP API, detects state transitions (safe/unsafe), and sends notifications through configurable channels. It supervises the health of HTTP services (periodic `GET` health probes with autonomous restart, see [Service Health Supervision](#service-health-supervision)) and provides a web dashboard for real-time status viewing.

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
Dashboard (axum + hand-rolled HTML) --- web UI w/ JS polling
```

### Key Traits

- **`Monitor`** — `poll() -> MonitorState`, `connect()`, `disconnect()`, `polling_interval() -> Duration`. A **pull-based** monitor the engine polls on a fixed interval. First implementation: `AlpacaSafetyMonitor`.
- **`EventMonitor`** — `name()`, `run(cancel)`. A **self-driving** monitor task that owns its own lifecycle — a long-lived connection it reacts to, or a poll loop it paces itself — and runs until the cancellation token fires. Added because some monitors cannot be expressed as the engine-paced `poll() -> State` shape; the engine spawns a parallel task per `EventMonitor` alongside the per-`Monitor` poll loops. Implementations: `OperationDeadlineMonitor` (see [Operation Watchdog](#operation-watchdog)) and `ServiceHealthSupervisor` (see [Service Health Supervision](#service-health-supervision)).
- **`Notifier`** — `notify(notification)`. First implementation: `PushoverNotifier`. The watchdog reuses this dispatch path — an expiry or liveness escalation is just another notification.
- **`Corrective`** / **`HealthChecker`** / **`Aborter`** / **`Restarter`** — the watchdog's corrective-action ladder for `abort_then_restart` operations. `CorrectiveLadder` composes the three rung traits (health → abort → restart) behind the single `Corrective::run` seam the watchdog calls; HTTP and shell default impls live in `corrective.rs`. See [Operation Watchdog](#operation-watchdog). `Restarter` (run a shell command bounded by a budget, `Ok` iff it exits 0 in time) is also the seam behind the [Service Restart API](#service-restart-api): the REST endpoint runs both `restart_command` and the `health_command` recovery poll through it, so tests inject a recording stub.
- **`HttpClient`** — wraps `reqwest` for testability (mockall in tests). Used by monitors, notifiers, and the corrective health-check / abort rungs.

### Dependency Injection

`Config` provides `build_monitors()` and `build_notifiers()` factory methods (in `lib.rs`) that map config enums to concrete implementations. `SentinelBuilder` uses these by default.

For custom monitors/notifiers (e.g. in an astrophotography app), use `with_monitors()` / `with_notifiers()` on the builder to inject pre-built instances, bypassing the config factories entirely.

### State Flow

1. Engine starts, connects each monitor (PUT `connected=true`)
2. Engine spawns a tokio polling task per monitor at configured interval
3. Each poll: GET `issafe`, compare with stored state, if transition matches a configured rule, dispatch to notifiers
4. SharedState updated on every poll, dashboard reads from it
5. On shutdown: disconnect monitors (PUT `connected=false`)

## Configuration

Configuration is loaded from a JSON file. All sections are optional with sensible defaults.
Every section (the top-level `Config` plus each nested block —
`MonitorConfig`, `NotifierConfig`, `TransitionConfig`, `DashboardConfig`,
`OperationWatchdogConfig`, `OperationPolicy`, `ServiceConfig`, `HealthConfig`)
rejects unknown keys at deserialize (`deny_unknown_fields`), so a typo or a
key removed by a schema change fails loudly at load instead of being silently
ignored.

See `examples/config.json` for a complete example.

### Supervised services (`services`)

The optional top-level `services` map is sentinel's registry of the services it
can supervise, keyed by a name of the operator's choosing (conventionally the
systemd unit / package name). It has **three consumers**: the
[REST restart endpoint](#service-restart-api), the operation watchdog's
[corrective ladder](#escalation-corrective-action-ladder)
(`operations.<family>.service` references keys in this map), and
[service health supervision](#service-health-supervision) (entries with a
`health` block). Each consumer is independent — none requires the others to be
configured.

```json
{
  "services": {
    "dsd-fp2": {
      "base_url": "http://localhost:11119/api/v1",
      "device_number": 0,
      "restart_command": "systemctl --user restart dsd-fp2",
      "health_command": "systemctl --user is-active dsd-fp2",
      "max_restart_duration": "60s"
    },
    "rp": { "restart_command": "systemctl --user restart rp" }
  }
}
```

| Field | Default | Meaning |
|---|---|---|
| `base_url` | *(none)* | Alpaca API base of the service, e.g. `http://host:port/api/v1`. Used only by the watchdog ladder's HTTP health-check and abort rungs; optional so non-Alpaca services (e.g. `rp`) can still be restart-only entries. |
| `device_number` | `0` | Alpaca device number for the ladder's health-check / abort URLs. |
| `restart_command` | `null` | Shell command that restarts the service; `null` = not restartable (the ladder stops at abort; the REST endpoint answers 409). |
| `health_command` | `null` | Shell command whose **exit 0 means healthy**. When set, the REST restart endpoint polls it after the restart command to confirm recovery; when absent, recovery confirmation is skipped. |
| `max_restart_duration` | `60s` | Per-service time budget (humantime) for the restart command *and* the recovery wait together. Used by the REST endpoint, the watchdog ladder, and health supervision. |
| `health` | *(none)* | Optional block that opts the service into [HTTP health supervision](#service-health-supervision): periodic `GET` probes with autonomous restart on consecutive failures. |

Commands run through the platform shell (`sh -c` on Unix, `cmd /C` on
Windows), so follow each platform's best practice:

| Platform | `restart_command` | `health_command` |
|---|---|---|
| Linux (systemd user unit) | `systemctl --user restart dsd-fp2` | `systemctl --user is-active dsd-fp2` (exit 0 iff active) |
| Linux (system unit) | `systemctl restart dsd-fp2` | `systemctl is-active dsd-fp2` |
| Windows (SCM service) | `powershell -Command "Restart-Service dsd-fp2"` | `sc query dsd-fp2 \| findstr RUNNING` |

### Monitor Types

- `alpaca_safety_monitor` — Polls an ASCOM Alpaca SafetyMonitor device. Supports optional `auth` section with `username` and `password` (plaintext) for connecting to auth-enabled services. See [ADR-003](../decisions/003-authentication-for-device-access.md).

In addition to the polled monitors above, the optional `operation_watchdog` block configures the push-based [Operation Watchdog](#operation-watchdog), which subscribes to an rp event stream rather than polling a device.

### Notifier Types

- `pushover` — Sends push notifications via the Pushover API. Accepts an
  optional `api_url` field that overrides the endpoint
  (`https://api.pushover.net/1/messages.json` by default); set it to point at a
  self-hosted Pushover-compatible relay, or at a local stub in tests.

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

### Authentication

The dashboard supports optional HTTP Basic Auth via the `dashboard.auth`
config section. Monitor connections to auth-enabled Alpaca services use
per-monitor `auth` credentials (plaintext password, `chmod 600` recommended).
See [ADR-003](../decisions/003-authentication-for-device-access.md).

```json
{
  "monitors": [{
    "type": "alpaca_safety_monitor",
    "name": "Roof Sensor",
    "host": "localhost", "port": 11111, "device_number": 0,
    "scheme": "https",
    "auth": { "username": "observatory", "password": "secret" }
  }]
}
```

## Operation Watchdog

The operation watchdog is sentinel's second monitoring loop. Where
`AlpacaSafetyMonitor` answers "is the sky safe?", the watchdog answers
"is the running operation making progress, and is rp itself alive?".
It is the Sentinel half of the two-loop supervision design described in
[`docs/services/rp.md` §Sentinel Watchdog Integration](rp.md#sentinel-watchdog-integration)
and the
[predictive-deadlines plan](../plans/archive/predictive-deadlines-and-watchdog.md);
rp is the inner loop (every blocking operation emits a `*_started` event
carrying a predicted and a maximum duration, then a `*_complete` /
`*_failed`), and the watchdog is the outer loop that tracks those
deadlines independently and escalates when one is missed.

It is implemented as an `EventMonitor` (`OperationDeadlineMonitor`) that
subscribes to rp's Server-Sent-Events stream
(`GET {rp_url}/api/events/subscribe`, see
[rp.md §Real-Time Stream](rp.md#real-time-stream)) and reacts to each
event as it arrives. It is entirely optional: with no `operation_watchdog`
block in the config, sentinel behaves exactly as before (safety polling
only).

### What it tracks

Every event on the stream carries an envelope with an `event` type (e.g.
`"slew_started"`), an `event_seq` (the monotonic SSE id / replay cursor),
an `operation_id` (shared across one operation's lifecycle events), and —
on `*_started` events for operations with predictive deadlines — a
`max_duration_ms`. The watchdog keys open operations by `operation_id`:

| Event suffix | Action |
|---|---|
| `*_started` | Begin tracking this `operation_id`. If the envelope carries `max_duration_ms`, arm an expiry timer for `max_duration_ms + buffer` (the **buffer** covers wire latency and gives rp's own internal deadline a chance to fire first). If it does not (operations not yet on predictive deadlines, e.g. `plate_solve`), track it open with no timer — it clears only on completion or via the liveness pulse. |
| `*_complete` / `*_failed` | Stop tracking this `operation_id`; cancel its timer. The operation finished within its deadline — no escalation. |
| timer expiry (no matching completion) | **Escalate** per the operation family's `on_expiry` policy. |

The operation **family** (the key used to look up `buffer` / `on_expiry`)
is the event name with its `_started` / `_complete` / `_failed` suffix
stripped: `slew_started` → `slew`, `move_focuser_complete` →
`move_focuser`, `centering_started` → `centering`. A family with no
configured entry uses the watchdog's `default_buffer` and a `notify_only`
policy.

The timer is armed from the moment the `*_started` frame is **received**,
not from the envelope's `started_at` wall-clock — this needs no clock
synchronisation between hosts, and on reconnect (below) a replayed
`*_started` that is already overdue fires almost immediately, which is the
correct conservative behaviour.

### Liveness pulse (stream disconnect)

The stream doubles as a heartbeat on rp. If the connection drops — rp
crashed, wedged, or restarted — the watchdog:

1. Reconnects with `Last-Event-ID` set to the highest `event_seq` it has
   seen, so rp replays everything buffered since (see
   [rp.md §Real-Time Stream](rp.md#real-time-stream) — rp keeps a 512-event
   history ring). Reconnect uses a fixed `reconnect_backoff` delay, up to
   `reconnect_max_attempts` times.
2. If rp answers a reconnect with a leading `stream_gap` event (the
   client's cursor predates the history ring, so some events were lost),
   the watchdog **escalates every operation it is currently tracking** —
   it can no longer be sure those operations completed — and then resumes
   from the live tail.
3. If every reconnect attempt fails, the watchdog escalates **"rp
   unresponsive"** — this is the end-to-end liveness trigger the design
   calls out: a hung or crashed rp is itself a watchdog event, not just a
   silent gap.

A clean reconnect that replays without a gap resumes tracking
transparently: completions that arrived during the brief disconnect clear
their operations on replay, and still-open operations keep their original
timers.

### Escalation (corrective-action ladder)

On expiry the watchdog runs the corrective action selected by the
operation family's `on_expiry` policy:

- **`notify_only`** — dispatch a `Notification` through the same `Notifier`
  chain the safety monitor uses (so a Pushover alert fires) and record it
  in the dashboard notification history. This is the default, and it is
  also the *only* action for the liveness triggers below (a `stream_gap`
  or an unresponsive rp has no single service to abort).
- **`abort_then_restart`** — run an **escalating ladder** against the
  service that owns the family (named by `operations.<family>.service` and
  resolved against the `services` map), then notify. The ladder takes the
  least-invasive action that can clear the stall, in order:
  1. **Health check** — `GET {base_url}/{device}/{n}/connected` with a 2 s
     timeout. A clean `200` means the service is alive and the *operation*
     is stuck; anything else (non-200, timeout, connection refused) means
     the service itself is unresponsive.
  2. **Abort** — if the service is responsive and the family maps to an
     ASCOM abort verb (`slew`/`park` → `telescope/{n}/abortslew`,
     `exposure` → `camera/{n}/abortexposure`, `move_focuser` →
     `focuser/{n}/halt`), `PUT` that verb. A successful abort **ends the
     ladder** — the aborted operation surfaces a `*_failed` / `*_complete`
     on the stream, which clears its tracking entry.
  3. **Restart** — if the service is unresponsive, the abort failed, or the
     family has no abort verb (e.g. the compound `centering`), and a
     `restart_command` is configured, run it (bounded by the service's
     `max_restart_duration`) and then poll the health check until the
     service is responsive again or the budget elapses.
     `restart_command: null` marks a service as **not restartable** (a
     remote MCU such as the star-adventurer-gti is the canonical example) —
     the ladder stops at abort. The rung first acquires the service's slot in
     the [shared restart gate](#the-shared-restart-gate); if another restart
     of the same service is already in flight (REST endpoint or health
     supervision), the rung is skipped and the escalation message reports
     `restart=skipped(already in flight)`.
  4. **Notify** — always, through the `Notifier` chain, with a message that
     reports which rungs ran and their outcome (rendered into the
     `{action}` placeholder).

A family configured `abort_then_restart` whose `service` cannot be
resolved (no `service` set, or a name absent from the top-level
[`services`](#supervised-services-services) map) **degrades safely to
`notify_only`** with a logged warning — a config mistake never
aborts the wrong device or wedges the watchdog (tenet #2, robustness).
A resolvable service with no `base_url` cannot be health-checked or
aborted (health reports *unknown*, abort is skipped), so the ladder falls
through to the restart rung.

> **End-to-end coverage.** The ladder's rungs are unit-tested
> (`services/sentinel/src/corrective.rs`) and the watchdog's policy branching +
> SSE/liveness handling are unit- and BDD-tested against stubs
> (`services/sentinel/tests/features/operation_watchdog.feature`). The full
> two-process loop — a **real rp** emitting over `/api/events/subscribe` and a
> **real sentinel** subscribing, escalating, and shelling out the restart
> command — runs in the dedicated harness
> [`tests/operation-watchdog-e2e/`](../../tests/operation-watchdog-e2e). It
> drives the watchdog via `center_on_target`: rp advertises a centering
> deadline but does **not** enforce it (advisory, §2.5), so the watchdog timer
> is the only thing that fires, and because `centering` has no Alpaca binding
> the ladder skips abort and exercises the **restart rung** end-to-end (the
> rung otherwise only unit-tested). Its three scenarios cover wedge → restart,
> a converging op that is *not* escalated, and rp going away → "unresponsive".

> **Deferred — rp-side recovery callbacks.** The plan
> (`docs/plans/archive/predictive-deadlines-and-watchdog.md` §5.3–5.4) sketches two
> follow-up POSTs from sentinel to rp — `/api/internal/operation-aborted`
> (clear sticky state, re-plan) and `/api/internal/service-restarted`
> (reconnect the driver). They are **not** implemented: rp has no
> abort-recovery or reconnect-after-restart machinery to consume them
> today, so the endpoints would be no-ops. They land with that rp-side
> recovery work, not here.

### Configuration

The watchdog is configured by an optional top-level `operation_watchdog`
block. Services it can health-check, abort, and restart are declared in the
**top-level [`services`](#supervised-services-services) map** (shared with the
REST restart endpoint), referenced by `operations.<family>.service`:

```json
{
  "services": {
    "star-adventurer": { "base_url": "http://localhost:11117/api/v1", "restart_command": null },
    "qhyccd-alpaca":   { "base_url": "http://localhost:11111/api/v1", "device_number": 0, "restart_command": "systemctl --user restart qhyccd-alpaca" },
    "qhy-focuser":     { "base_url": "http://localhost:11113/api/v1", "restart_command": "systemctl --user restart qhy-focuser", "max_restart_duration": "45s" }
  },
  "operation_watchdog": {
    "rp_url": "http://localhost:8080",
    "reconnect_max_attempts": 5,
    "reconnect_backoff": "5s",
    "default_buffer": "10s",
    "notifiers": ["pushover"],
    "message_template": "Operation {operation} ({operation_id}) {reason} after {elapsed}{action}",
    "operations": {
      "slew":         { "buffer": "5s",  "on_expiry": "abort_then_restart", "service": "star-adventurer" },
      "park":         { "buffer": "30s", "on_expiry": "notify_only"        },
      "exposure":     { "buffer": "30s", "on_expiry": "abort_then_restart", "service": "qhyccd-alpaca"   },
      "centering":    { "buffer": "0s",  "on_expiry": "notify_only"        },
      "move_focuser": { "buffer": "5s",  "on_expiry": "abort_then_restart", "service": "qhy-focuser"     }
    }
  }
}
```

| Field | Default | Meaning |
|---|---|---|
| `rp_url` | *(required)* | Base URL of the rp instance to watch; the watchdog subscribes to `{rp_url}/api/events/subscribe`. |
| `reconnect_max_attempts` | `5` | How many consecutive reconnect attempts before escalating "rp unresponsive". |
| `reconnect_backoff` | `5s` | Delay between reconnect attempts (humantime). |
| `default_buffer` | `10s` | Buffer added to `max_duration_ms` for families with no `operations` entry. |
| `notifiers` | *(all)* | Which notifier `type`s receive escalations; omitted means every configured notifier. |
| `message_template` | built-in | Escalation message; placeholders `{operation}`, `{operation_id}`, `{elapsed}`, `{reason}`, `{action}` (the corrective-action summary, empty for `notify_only`). |
| `operations.<family>.buffer` | `default_buffer` | Buffer for this operation family. |
| `operations.<family>.on_expiry` | `notify_only` | Corrective-action policy: `notify_only`, or `abort_then_restart` (runs the ladder against `service`). |
| `operations.<family>.service` | *(none)* | Service (key into the top-level `services` map) that owns this family. Required for `abort_then_restart`; ignored otherwise. |

The restart rung's time budget is the referenced service's
`max_restart_duration` (default `60s`) — there is no watchdog-global budget;
each service declares how long its restart may take.

`reconnect_max_attempts` of `0` means "never give up reconnecting" (the
watchdog keeps retrying without ever escalating an unresponsive rp), and a
`reconnect_backoff` of `0s` retries immediately.

### Edge cases

| Scenario | Behavior |
|----------|----------|
| Operation completes within its deadline | Tracking entry removed on `*_complete` / `*_failed`. No notification. |
| Operation's `max_duration_ms + buffer` elapses with no completion | One escalation per expiry, recorded in history (and, for `abort_then_restart`, after the ladder runs). |
| Expiry, `abort_then_restart`, service responsive | Health check passes → abort verb `PUT`; ladder stops; notification reports `abort=ok`. |
| Expiry, `abort_then_restart`, service unresponsive | Abort skipped → `restart_command` run (if set) → recovery awaited up to `max_restart_duration`; notification reports the restart outcome. |
| Expiry, `abort_then_restart`, family has no abort verb (`centering`, `plate_solve`) | Abort skipped; restart attempted if configured, else notify-only. |
| Expiry, `abort_then_restart`, service has no `base_url` | Health reports unknown, abort skipped; restart attempted if configured. |
| Expiry, `abort_then_restart`, `service` unset or unknown | Degrades to `notify_only` with a logged warning. |
| `*_started` without `max_duration_ms` | Tracked open, no timer; clears on completion or on a liveness escalation. |
| Stream drops, reconnect succeeds within `reconnect_max_attempts` | Replays buffered events, resumes tracking. No escalation unless a `stream_gap` is reported. |
| Reconnect reports `stream_gap` | Every currently-tracked operation is escalated (completions may have been lost); tracking resumes from the live tail. |
| rp unreachable for `reconnect_max_attempts` reconnects | "rp unresponsive" escalation. |
| `operation_watchdog` block absent | Watchdog disabled; sentinel runs safety polling only. |

## Service Health Supervision

Health supervision is sentinel's third monitoring loop. Where the safety
monitor answers "is the sky safe?" and the watchdog answers "is the running
operation making progress?", health supervision answers **"is this service
process alive right now?"** — and restarts it autonomously when it is not.
It closes the gap the plate-solver plan deferred (see
[`plate-solver.md` §Supervision and recovery](plate-solver.md#supervision-and-recovery)):
a service that SIGSEGVs is relaunched by the OS supervisor, but a service
that *hangs* — process alive, HTTP dead — previously needed an external
`/health`-polling watchdog. This loop is that watchdog.

Each `services` entry with a `health` block gets its own
`ServiceHealthSupervisor` (an `EventMonitor`, one tokio task per service):

```json
{
  "services": {
    "plate-solver": {
      "restart_command": "systemctl --user restart plate-solver",
      "max_restart_duration": "30s",
      "health": {
        "url": "http://localhost:11131/health",
        "poll_interval": "30s",
        "failure_threshold": 3,
        "restart_backoff": "1m",
        "restart_backoff_max": "15m"
      }
    }
  }
}
```

| Field | Default | Meaning |
|---|---|---|
| `url` | *(required)* | Health URL probed with `GET`. Only a clean `200` counts as alive; any other status, a timeout, or a connection error counts as a failed probe. The response body is never parsed (health bodies are not uniform across services). |
| `poll_interval` | `30s` | Probe cadence (humantime). Probing continues at this cadence during outages and backoff — only *restarting* is throttled. |
| `failure_threshold` | `3` | Consecutive failed probes before the first autonomous restart. |
| `restart_backoff` | `1m` | Wait before a second restart attempt when the first did not cure the service (humantime). |
| `restart_backoff_max` | `15m` | Ceiling for the doubling backoff (humantime). |

Each probe is bounded by a hardcoded **2 s timeout** (the same bound as the
watchdog ladder's health rung). The probed endpoint should therefore be cheap;
`plate-solver`'s `/health` (two filesystem stats, no subprocess) is the
reference shape.

### Behavior

- **Detection.** After `failure_threshold` consecutive failed probes, the
  supervisor runs the service's `restart_command` through the same
  [`RestartManager`](#service-restart-api) engine the REST endpoint uses —
  bounded by `max_restart_duration`, with the `health_command` recovery poll
  if one is configured, and holding the service's slot in the shared
  restart gate (below).
- **Backoff.** Each restart attempt schedules the *next* allowed attempt
  `restart_backoff` later, doubling after every attempt up to
  `restart_backoff_max`. Probing never stops and the supervisor **never gives
  up** — a service that stays broken is retried at the capped cadence until
  it recovers or an operator intervenes.
- **Recovery.** One successful probe ends the outage: the failure counter,
  the backoff, and the restart schedule all reset. Recovery is visible on the
  dashboard but is deliberately **not** notified.
- **Not restartable.** A `health` block on a service with
  `restart_command: null` degrades to probe-and-notify: a single notification
  fires when the threshold is crossed (once per outage) and no restart is
  attempted — the same degrade-with-warning posture as the watchdog ladder
  (tenet #2, robustness).

### Notifications

Autonomous restarts — unlike [manual REST restarts](#service-restart-api),
which are silent — always notify through the full `Notifier` chain and are
recorded in the dashboard notification history:

| Trigger | Priority | Message shape |
|---|---|---|
| First restart of an outage | `0` | Service unhealthy after N consecutive failed probes; restarted autonomously, with the restart outcome (`status` / `recovery`). |
| Every later restart of the same outage | `1` (escalated) | Service **still unhealthy** after K autonomous restarts, with the latest outcome and the next-attempt time. |
| Threshold crossed on a non-restartable service | `1` | Service unhealthy and has no `restart_command`; manual intervention required. |

Notification volume is bounded by construction: at most one notification per
restart attempt, and attempts are backoff-throttled (at the `15m` default cap,
at most one escalation per 15 minutes per service). There is no separate
dedup or cooldown mechanism.

### The shared restart gate

All three restart paths — the REST endpoint, the watchdog ladder's restart
rung, and health supervision — acquire the same per-service in-flight slot
(`RestartGate` in `restart.rs`) before running a `restart_command`, so at most
one restart of a given service runs at any time:

- REST endpoint blocked by the gate → `409` (already in flight), unchanged.
- Watchdog ladder blocked by the gate → skips its restart rung; the
  escalation message reports `restart=skipped(already in flight)`.
- Health supervision blocked by the gate → logs at debug and keeps probing;
  no notification, no counter or backoff change. Whatever restart is already
  running will show its effect in the next probes.

### Edge cases

| Scenario | Behavior |
|----------|----------|
| Sentinel starts before a supervised service finishes booting | First probes fail but nothing restarts until `failure_threshold × poll_interval` (90 s by default) has elapsed — natural startup grace. A restart of a still-booting service is harmless for `systemctl restart`-style commands. |
| Manual restart (REST/UI) during an outage | The autonomous attempt finds the gate held and skips silently; if the manual restart cures the service, the next successful probe resets the outage. |
| Restart command fails (non-zero, spawn failure, over budget) | Counts as an attempt: notification carries the failure detail, backoff advances, probing continues. |
| Service flaps (recovers, fails again) | Each recovery fully resets the state machine; a new outage starts at `failure_threshold` fresh failures and the initial `restart_backoff`. |
| Shutdown during an in-flight autonomous restart | The supervisor returns immediately (restart await is raced against the cancellation token); the gate slot is released and the shell child runs to completion detached. |
| `health` block absent | The service is restart-on-demand only — exactly today's behavior. |

## Service Restart API

Sentinel owns **process restart** for the rest of the stack (`rp.md`
§Sentinel Watchdog Integration; the config-actions plan's service-lifecycle
split: drivers reload *themselves* via `config.apply`, sentinel restarts
*processes*). The dashboard router exposes one endpoint:

```
POST /api/services/{name}/restart
```

`{name}` is a key in the top-level
[`services`](#supervised-services-services) map. Sentinel does **not** spawn
or own the processes — it shells out to the service's configured
`restart_command` (the OS supervisor owns relaunch), then, when a
`health_command` is configured, polls it until it exits 0 or the service's
`max_restart_duration` budget elapses (each probe is bounded to its
per-attempt slice of the budget, so one hanging probe is killed and retried
rather than consuming the whole phase). The endpoint is the manual "recovery
hammer" the web UI's *Restart via Sentinel* affordance calls (see
[`ui-htmx.md`](ui-htmx.md)), and the escalation target for `config.apply`
fields classified `restart_required`.

### Behavioral contract

| Condition | Response |
|---|---|
| Restart command exits 0; `health_command` set and exits 0 within the remaining budget | `200` `{"service":"<name>","status":"ok","recovery":"healthy"}` |
| Restart command exits 0; `health_command` set but never exits 0 in budget | `200` `{"service":"<name>","status":"ok","recovery":"timeout"}` |
| Restart command exits 0; no `health_command` | `200` `{"service":"<name>","status":"ok","recovery":"skipped"}` |
| Restart command exits non-zero, fails to spawn, or exceeds the budget | `200` `{"service":"<name>","status":"failed","detail":"…"}` (no recovery poll) |
| `{name}` not in the `services` map | `404` `{"error":"no configured service named '<name>'"}` |
| Service configured with `restart_command: null` | `409` `{"error":"service '<name>' is not restartable"}` |
| A restart for `{name}` is already in flight | `409` `{"error":"a restart of '<name>' is already in flight"}` |

- **`status` reports the action's outcome, not the transport's** — a failed
  restart command is a domain result (HTTP 200 with `status:"failed"`),
  mirroring the config-actions protocol's `status:"invalid"` convention.
  4xx is reserved for addressing errors (unknown / not restartable / busy).
- **One restart per service at a time.** Concurrent POSTs for the same
  service are rejected with `409` while the first is running; different
  services restart independently. The in-flight slot is the
  [shared restart gate](#the-shared-restart-gate), so a `409` can also mean an
  autonomous restart (health supervision) or a watchdog-ladder restart is
  currently running for that service.
- **The request blocks** until the command (and recovery poll, if any)
  completes — bounded by the service's `max_restart_duration`, so the
  response always arrives within that budget plus scheduling noise.
- **Same protection as the rest of the dashboard.** The endpoint sits on the
  dashboard router, behind the same optional `dashboard.auth` Basic-auth and
  TLS layers — there is no separate gate. Deploying without dashboard auth
  leaves it (like the JSON API) open; the restart commands themselves come
  only from sentinel's own config file, never from the request.
- A user-initiated restart is logged (`info!`) but does **not** dispatch
  notifications and is not recorded in the notification history — the
  operator who pressed the button already knows.

- **Web UI**: Server-rendered HTML with JavaScript polling (auto-refreshes every 5 seconds). Shows monitor statuses, a *Supervised Services* health table, and the notification history.
- **JSON API**: `/api/status` (monitor statuses), `/api/services` (supervised-service health, below), `/api/history` (notification history)
- **Service restart**: `POST /api/services/{name}/restart` (see [Service Restart API](#service-restart-api))
- **Health check**: `/health` returns 200 OK

### `GET /api/services`

Returns one entry per supervised service (a `services` entry with a `health`
block), sorted by name — an empty array when nothing is supervised:

```json
[{
  "name": "plate-solver",
  "health": "up",
  "last_probe_epoch_ms": 1760000000000,
  "consecutive_failures": 0,
  "restarts_in_outage": 0,
  "total_restarts": 3,
  "next_restart_epoch_ms": null,
  "poll_interval_ms": 30000
}]
```

- `health` is `"unknown"` (never probed yet), `"up"`, or `"down"`.
- `last_probe_epoch_ms` is `0` until the first probe completes.
- `restarts_in_outage` counts autonomous restarts in the current outage
  (resets on recovery); `total_restarts` counts them since sentinel started.
- `next_restart_epoch_ms` is the backoff-scheduled earliest next autonomous
  restart, or `null` when none is scheduled.

The dashboard is server-rendered HTML with vanilla JavaScript, built with `format!()` in `services/sentinel/src/dashboard.rs`. (An experimental `sentinel-app` Leptos/WASM frontend was scaffolded and later abandoned; it was removed in 2026-06 — see [docs/plans/archive/sentinel-app-leptos-dashboard.md](../plans/archive/sentinel-app-leptos-dashboard.md).)

## Module Structure

```
services/sentinel/src/
  main.rs              CLI entry point
  lib.rs               Module declarations + SentinelBuilder + Sentinel + Config factory methods
  config.rs            Config types + load_config()
  error.rs             SentinelError + Result alias
  io.rs                HttpClient trait + ReqwestHttpClient
  monitor.rs           Monitor trait + MonitorState + StateChange
  alpaca_client.rs     AlpacaSafetyMonitor (Monitor impl)
  watchdog.rs          EventMonitor trait + OperationDeadlineMonitor + SSE event source + deadline tracking
  corrective.rs        Corrective-action ladder: HealthChecker/Aborter/Restarter traits + HTTP/shell impls + CorrectiveLadder
  restart.rs           RestartManager: services registry + shared RestartGate + restart/recovery orchestration
  health.rs            ServiceHealthSupervisor (EventMonitor impl): periodic health probes + autonomous restart with backoff
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

Without `-c`, the config path resolves to the platform config
directory (`~/.config/rusty-photon/sentinel.json` on Linux,
`%PROGRAMDATA%\rusty-photon\sentinel.json` on Windows) and a default
config file is created there on first start if none exists — the same
convention as the driver services (see
[ADR-012](../decisions/012-service-packaging-architecture.md)).

## Port

11114 (dashboard, configurable)
