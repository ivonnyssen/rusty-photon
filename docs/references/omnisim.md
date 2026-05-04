# OmniSim (ASCOM Alpaca Simulators) Reference

OmniSim is the [ASCOM Alpaca Simulators](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators) — a multi-device ASP.NET Core app that exposes simulated ASCOM hardware (telescope, camera, focuser, filter wheel, etc.) over the standard Alpaca HTTP API. We use it as the device under test in our BDD suites (`crates/bdd-infra/src/rp_harness/omnisim.rs`).

The simulator is *faithful* to real hardware behavior in many ways that aren't immediately obvious from the ASCOM spec, and those behaviors have caused us several days of debugging. This doc captures what we've learned.

## Single-instance enforcement (no parallelism)

OmniSim acquires a hardcoded global named mutex on startup:

```csharp
// ASCOM.Alpaca.Simulators/Program.cs
private static Guid ApplicationGUID = new Guid("{1389A00E-006F-4117-8930-EAFCAA7DC397}");
string mutexId = string.Format("Global\\{{{0}}}", ApplicationGUID);
using (var mutex = new Mutex(false, mutexId, out bool createdNew))
{
    hasHandle = mutex.WaitOne(10, false);
    if (hasHandle == false) {
        // forward args to first instance via named pipe, then exit
        throw new TimeoutException("Timeout waiting for exclusive access");
    }
    ...
}
```

**Implications:**

- Only one OmniSim per machine. Period.
- The GUID is hardcoded — no CLI flag, env var, or config to override it.
- A second instance does *not* run independently. It connects to the first instance's named pipe (`{2249563F-E844-4264-8956-73AC7A44BEA0}`, also hardcoded), forwards its CLI args, and exits.
- BDD scenarios cannot be parallelized at the OmniSim level. Cucumber's `@serial` tag (which the rusty-photon BDD features use) is what keeps things working.

If you see `Timeout waiting for exclusive access` in OmniSim's stderr, another instance is already running.

## Binding port

OmniSim is an ASP.NET Core app. Default port is **32323**. Override via either:

- CLI: `ascom.alpaca.simulators --urls "http://127.0.0.1:33333"`
- Env: `ASPNETCORE_URLS=http://127.0.0.1:33333`

But — see above. Port override is moot if another OmniSim is already running, because the mutex blocks before the bind.

## State persistence

OmniSim writes per-device settings to XDG config:

```
~/.config/ascom/alpaca/ascom-alpaca-simulator/
├── camera/v1/instance-0.xml
├── covercalibrator/v1/instance-0.xml
├── dome/v1/instance-0.xml
├── filterwheel/v1/instance-0.xml
├── focuser/v1/instance-0.xml
├── observingconditions/v1/instance-0.xml
├── rotator/v1/instance-0.xml
├── safetymonitor/v1/instance-0.xml
├── server/v1/instance-0.xml
├── switch/v1/instance-0.xml
└── telescope/v1/instance-0.xml
```

The path is **not configurable** via OmniSim's own config. On Linux, .NET respects `XDG_CONFIG_HOME`, so re-rooting that env var does redirect the state dir — but be aware nothing inside OmniSim documents this.

State is loaded on startup and persisted on shutdown. State persists across OmniSim restarts unless explicitly reset (see CLI flags / `/simulator/v1/.../reset` below).

## CLI flags

From `readme.md`:

| Flag | Effect |
|---|---|
| `--reset` | Resets all settings for the drivers and server (clears persisted state on startup) |
| `--reset-auth` | Resets authentication, allowing access without password |
| `--local-address` | Print the URL of the running instance (when invoked as a second instance) |
| `--show-browser` | Open the web UI in a browser (when invoked as a second instance) |
| `--urls <url>` | ASP.NET Core convention; override the bind URL |

`--reset` only takes effect at startup. To reset state in a *running* instance, use the OmniSim-only HTTP API.

## OmniSim-only HTTP API: `/simulator/v1/`

In addition to the standard ASCOM Alpaca API at `/api/v1/{device}/{n}/...`, OmniSim exposes a private namespace at `/simulator/v1/{device}/{n}/...` that is **not** part of the Alpaca spec. Source: [`SimulatorController.cs`](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators/blob/main/ASCOM.Alpaca.Simulators/Controllers/SimulatorController.cs).

The two operations available per device class are:

### `PUT /simulator/v1/{device}/{n}/reset`

Clears the device's persisted profile (`ClearProfile`) and re-initializes (`Init`). Equivalent to deleting the relevant `instance-N.xml` and starting over. Use this when you want to wipe stored *settings* (e.g., observer location, capability overrides).

### `PUT /simulator/v1/{device}/{n}/restart`

Reloads the device with its current persisted settings (`DriverManager.LoadX(0)`). Equivalent to "OmniSim has just started". Settings are preserved; runtime state (mount position, slew state, AtPark flag, tracking state) goes back to defaults. **This is what we use in BDD `before(scenario)` hooks** — it's fast (an HTTP PUT, ~ms), idempotent, and doesn't touch persisted settings.

Per-class endpoints (all support both `/reset` and `/restart`):

- `/simulator/v1/camera/{n}/...`
- `/simulator/v1/covercalibrator/{n}/...`
- `/simulator/v1/dome/{n}/...`
- `/simulator/v1/filterwheel/{n}/...`
- `/simulator/v1/focuser/{n}/...`
- `/simulator/v1/observingconditions/{n}/...`
- `/simulator/v1/rotator/{n}/...`
- `/simulator/v1/safetymonitor/{n}/...`
- `/simulator/v1/switch/{n}/...`
- `/simulator/v1/telescope/{n}/...`

Response format mirrors the standard Alpaca pattern (`ClientTransactionID`, `ServerTransactionID`, `ErrorNumber`, `ErrorMessage`).

## Behaviors that mirror real hardware

These are not bugs. They reflect how real ASCOM mounts (especially GEMs) behave, and OmniSim faithfully simulates them. They have all bitten us at some point.

### Telescope

- **`Slewing` (`IsSlewing`) is over-conservative.** It returns `true` if *any* of these are true: `SlewState != SlewNone`, an internal `slewing` flag, or `rateMoveAxes.LengthSquared != 0`. A prior `MoveAxis` call that left non-zero rate state will keep `Slewing` true even after a subsequent slew completes. **Conclusion:** poll `AtPark` (the canonical post-park signal) rather than `Slewing` to detect park completion. See `services/rp/src/mcp.rs::do_park_blocking`.

- **`AtPark` is the single source of truth for park completion.** It transitions to `true` in exactly one code path — the `SlewType.SlewPark` completion handler in OmniSim's slew loop.

- **Slew refuses to drive through forbidden zones.** OmniSim mirrors real-mount soft limits: the slew motion path-plans through reachable positions and will not traverse below-horizon or other forbidden ranges. This means a mount that has been *synced* into an impossible position (e.g., `SyncToCoordinates` to coords that map below the horizon at the current LST) cannot be parked from there — the park slew will start (`SlewState = SlewPark`, `Slewing = true`) but never advance, so `AtPark` never flips. Recovery requires `/simulator/v1/telescope/0/restart` (or a programmatic sync to a reachable position with tracking on first).

- **`SyncToCoordinates` requires `Tracking == true`.** Returns `InvalidOperationException` ("SyncToCoordinates is not allowed when tracking is False") otherwise. This is OmniSim-imposed; the standard ASCOM spec doesn't strictly require it, but real GEMs commonly do. Tests that call `sync_mount` against OmniSim must enable tracking first.

- **`Park()` clears `Tracking` on success** per ASCOM spec. Don't write code that depends on tracking surviving across a park.

- **`Unpark()` is instant and idempotent.** Just sets `AtPark = false`, no slew. Calling unpark when already unparked is a no-op, never an error.

- **Default startup position** is approximately altitude 38.9°, azimuth 165° (configurable via the setup UI). Above horizon for any reasonable observer/time, so a park slew from defaults always succeeds.

### Other devices

(To be filled in as we learn — issue #149 tracks extending the BDD reset hook to non-mount devices, which will also surface their quirks.)

## Cucumber-rs `@serial` tag

The `@serial` tag is natively recognized by cucumber-rs (`cucumber-0.22.1/src/runner/basic.rs:429`):

```rust
let which_scenario: WhichScenarioFn = |feature, rule, scenario| {
    scenario.tags.iter()
        .chain(rule.iter().flat_map(|r| &r.tags))
        .chain(&feature.tags)
        .find(|tag| *tag == "serial")
        .map_or(ScenarioType::Concurrent, |_| ScenarioType::Serial)
};
```

Scenarios with `@serial` (on scenario, rule, or feature level) run sequentially. Untagged scenarios run concurrent (up to 64 by default). All BDD feature files in this repo that touch OmniSim are tagged `@serial` because OmniSim itself is single-instance.

## Integration in this repo

- `crates/bdd-infra/src/rp_harness/omnisim.rs` — process management. `OmniSimHandle::start` is a singleton-spawning shim; `OmniSimHandle::reset_telescope` wraps the `/simulator/v1/telescope/0/restart` endpoint.

- `services/rp/tests/bdd.rs` — the cucumber `before(scenario)` hook calls `reset_telescope` to give every scenario a clean default state. Other device classes are not yet wired up (issue #149).

- `services/rp/tests/features/mount.feature` — the `@serial` tag at file level forces sequential execution.

## References

- Source: [ASCOMInitiative/ASCOM.Alpaca.Simulators](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators)
- Single-instance check: [`Program.cs`](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators/blob/main/ASCOM.Alpaca.Simulators/Program.cs)
- Telescope hardware (slew/park internals): [`TelescopeSimulator/TelescopeHardware.cs`](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators/blob/main/TelescopeSimulator/TelescopeHardware.cs)
- OmniSim-only API: [`SimulatorController.cs`](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators/blob/main/ASCOM.Alpaca.Simulators/Controllers/SimulatorController.cs)
- Standard Alpaca API: see [`docs/references/ascom-alpaca.md`](ascom-alpaca.md)
