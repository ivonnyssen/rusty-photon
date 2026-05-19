# pa-falcon-rotator — Implementation Plan

## Status

**Active.** Phase 1 (design doc, `docs/services/falcon-rotator.md`) lands alongside this plan. Phases 2 (BDD scaffold + service skeleton) and 3 (implementation) follow per the sub-phase breakdown below.

## Outcomes (definition of done)

* `cargo run -p pa-falcon-rotator -- -c services/pa-falcon-rotator/examples/config-linux.json` boots, opens the serial port, runs the handshake, registers the Rotator and Status Switch devices on port 11118, and survives a NINA / SGPro connect → `Sync` → `MoveAbsolute` → `IsMoving`-poll → `Halt` → disconnect cycle against a real Falcon v1.
* Every `unimplemented!()` body landed in Phase 2 is removed and replaced by a real implementation with at least one test exercising it.
* All `tests/features/*.feature` files have their `@wip` tags removed; `cargo test -p pa-falcon-rotator --features mock --test bdd` runs every scenario green.
* `tests/conformu_integration.rs` exists and is `#[ignore]`; `cargo test -p pa-falcon-rotator --features conformu --test conformu_integration -- --ignored --nocapture` passes for both the Rotator and the Switch interfaces against the binary running under `MockSerialPortFactory`.
* `[package.metadata.conformu]` block in `services/pa-falcon-rotator/Cargo.toml` registers the service for the nightly ConformU rotation.
* `docs/workspace.md` Services row reflects the final phase status; `README.md` service table includes a `pa-falcon-rotator` row.
* `cargo rail run --profile commit -q` and `cargo fmt --check` are clean for every commit in the chain.

## Branching strategy

The whole driver lands on a **single feature branch** — sub-phases ship as individual commits, not as separate PRs:

* **PR #1** (`feature/falcon-rotator-driver`, merged) — design doc + this plan.
* **PR #2** (this PR — `feature/pa-falcon-rotator-phase2`) — everything from 2a onward. Each sub-phase below ends in its own commit so reviewers can step through the progression in `git log`, but they all merge to `main` in a single PR.

The original plan split phases 3b–3h across PRs #3–#7 (`feature/pa-falcon-rotator-protocol`, `…-plumbing`, `…-rotator-device`, `…-switch-device`, `…-conformu`). That was abandoned at user request after Phase 3a landed: the seven-PR chain added rebase / review overhead without changing the shape of the work, and the per-sub-phase commit history on a single branch gives the same step-through visibility. **Do not open separate PRs for 3b–3h.**

Sub-phase commit invariants on this branch:

* Each sub-phase commit is independently green: `cargo rail run --profile commit -q` and `cargo fmt --check` clean.
* `@wip` filtering keeps the BDD suite green between sub-phases — a scenario only loses its `@wip` tag in the commit that makes it pass.
* The branch is therefore mergeable to `main` at any sub-phase boundary if priorities change; nothing in 3b–3h is structurally required for the prior sub-phases to be useful.

## Sub-phases

Each sub-phase ends in a single commit. The `@wip` tags listed under "Removes" are the ones taken off in that sub-phase.

### 2a — Crate skeleton + workspace integration (single commit, ships with 2b)

Files created:

* `services/pa-falcon-rotator/Cargo.toml` — package metadata, feature flags (`mock`, `conformu`), dependencies (`ascom-alpaca` with `server,rotator,switch`, `tokio`, `tokio-serial`, `serde`, `serde_json`, `humantime-serde`, `clap`, `async-trait`, `tracing`, `tracing-subscriber`, `thiserror`, `rp-tls`, `rp-auth`, `axum`), dev-deps (`bdd-infra`, `tokio-test`, `mockall`, `proptest`, `reqwest`, `cucumber`, `cargo-husky`, `tempfile`), `[[test]] name = "bdd" harness = false`. **No** `i18n` / `rust-embed` (see design doc E4).
* `services/pa-falcon-rotator/src/lib.rs` — module declarations, `ServerBuilder` + `BoundServer` stubs (signatures only, `unimplemented!()` bodies, `#[cfg_attr(coverage_nightly, coverage(off))]` on each).
* `services/pa-falcon-rotator/src/main.rs` — `clap` `Args` struct, `tokio::main`, `ServerBuilder::new(...).build().await?.start().await` body. No i18n.
* `services/pa-falcon-rotator/src/config.rs` — `Config`, `SerialConfig`, `ServerConfig`, `RotatorConfig`, `SwitchConfig` types with `Default` impls per the design doc's defaults table. `load_config(path)`. Inline `#[cfg(test)]` tests for defaults + JSON round-trip.
* `services/pa-falcon-rotator/src/error.rs` — `FalconRotatorError` enum (`NotConnected`, `ConnectionFailed`, `SerialPort`, `Timeout`, `Io`, `InvalidResponse`, `ParseError`, `InvalidValue`, `Communication`), `Result<T>` alias, `to_ascom_error` mapping per design-doc Error Model table. Inline `#[cfg(test)]` Display + mapping tests.
* `services/pa-falcon-rotator/src/io.rs` — `SerialReader`, `SerialWriter`, `SerialPortFactory` traits with `#[cfg_attr(test, mockall::automock)]`. `SerialPair` struct.
* `services/pa-falcon-rotator/src/serial.rs` — `TokioSerialPortFactory` implementing `SerialPortFactory` via `tokio-serial`. Stubbed (`unimplemented!()`).
* `services/pa-falcon-rotator/src/mock.rs` — `#[cfg(feature = "mock")] pub struct MockSerialPortFactory` with a small deterministic state machine: stored mechanical position, is-moving flag, motor-reverse flag, derotation flag, voltage_raw. Returns canned `FR_OK` for `F#`, `FV:1.3` for `FV`, etc. Stubbed body for now.
* `services/pa-falcon-rotator/src/protocol.rs` — `Command` enum (variants for `Ping`, `FullStatus`, `FirmwareVersion`, `PositionDeg`, `PositionSteps`, `Voltage`, `DerotationOff` / `DerotationRate(u32)`, `MoveDeg(f64)`, `MoveSteps(u32)`, `Halt`, `IsRunning`, `SetReverse(bool)`). Variants for `FF` (firmware reload) and `SD:<deg>` (device-side sync) are deliberately omitted — `SD` would change `MechanicalPosition` and break the ASCOM `Sync` contract (see [design doc](../../services/falcon-rotator.md#sync-semantics--why-driver-side-not-sd)). `to_command_string()` method. `FalconStatus` parsed struct (steps, deg, is_moving, limit_detect, do_derotation, motor_reverse). Stubbed parsers (replaced in §3a on this same PR — see Branching strategy).
* `services/pa-falcon-rotator/src/serial_manager.rs` — `SerialManager` struct: `config`, `connection_count` (`AtomicU32`), `serial_available` (`AtomicBool`), `reader`/`writer` (`Mutex<Option<Box<dyn ...>>>`), `command_lock` (`Mutex<()>`), `sync_offset` (`Mutex<f64>`), `target_position` (`Mutex<Option<f64>>`), `last_limit_detected` (`Mutex<Option<bool>>`), `serial_factory`. Stubbed methods: `connect`, `disconnect`, `is_available`, `send_command`, `read_status` (issues `FA` + edge-checks `limit_detect`), `read_voltage`, `move_to`, `halt`, `sync`, `get_sync_offset`, `target_position`, `get_target_position`.
* `services/pa-falcon-rotator/src/rotator_device.rs` — `FalconRotatorDevice` impl skeleton. `Device` trait stub. `Rotator` trait stub.
* `services/pa-falcon-rotator/src/switch_device.rs` — `FalconStatusSwitchDevice` impl skeleton. `Device` trait stub. `Switch` trait stub.
* `services/pa-falcon-rotator/examples/config-linux.json`, `config-macos.json`, `config-windows.json` — copy the ppba-driver pattern adjusted for port `11118`, serial paths per OS.
* Root `Cargo.toml`: add `services/pa-falcon-rotator` to `[workspace] members`.

Tests in this sub-phase:

* Only the inline `#[cfg(test)]` defaults / Display tests in `config.rs` and `error.rs`.

Verification:

* `cargo build -p pa-falcon-rotator --all-features --all-targets --locked` compiles.
* `cargo nextest run -p pa-falcon-rotator --all-features --locked` runs the inline tests green.

Removes:

* Nothing (no `@wip` exists yet).

### 2b — BDD infrastructure (same commit as 2a)

Files created:

* `services/pa-falcon-rotator/tests/bdd.rs` — entry point. Uses `bdd_infra::bdd_main!` macro. Filters out scenarios tagged `@wip` via `.filter_run_and_exit("tests/features", |feat, _rule, sc| { !is_wip(feat, sc) })`. **Must use `_and_exit`**, per `docs/skills/testing.md` §2.7 (issue #171 precedent). `after` hook calls `world.service_handle.stop()`.
* `services/pa-falcon-rotator/tests/bdd/world.rs` — `FalconWorld` struct with `service_handle: Option<bdd_infra::ServiceHandle>`, `temp_dir: Option<tempfile::TempDir>`, helpers for writing a config file, building a connected device handle, etc.
* `services/pa-falcon-rotator/tests/bdd/steps/mod.rs` — re-exports of step files.
* `services/pa-falcon-rotator/tests/bdd/steps/connection_steps.rs` — `Given a configured pa-falcon-rotator service`, `When the client connects`, `Then connection is established`. Stubbed bodies.
* `services/pa-falcon-rotator/tests/bdd/steps/metadata_steps.rs` — `Then the rotator's CanReverse is true`, `Then the StepSize is 0.01155`, etc.
* `services/pa-falcon-rotator/tests/bdd/steps/position_steps.rs` — `When the client reads MechanicalPosition`, `Then Position is X.XX`.
* `services/pa-falcon-rotator/tests/bdd/steps/movement_steps.rs` — `When the client calls MoveAbsolute X.X`, `Then the device received MD:X.X`, `Then IsMoving is true / false`.
* `services/pa-falcon-rotator/tests/bdd/steps/halt_steps.rs` — `When the client calls Halt`, `Then TargetPosition is current Position`.
* `services/pa-falcon-rotator/tests/bdd/steps/reverse_steps.rs` — `Given the mock reports motor_reverse = true`, `When the client calls SetReverse(true)`, `Then no FN command was sent` (read-before-write).
* `services/pa-falcon-rotator/tests/bdd/steps/sync_steps.rs` — `When the client calls Sync(37.5)`, `Then Position reports 37.5`, `Then MechanicalPosition is unchanged`.
* `services/pa-falcon-rotator/tests/bdd/steps/status_switch_steps.rs` — Switch device steps: `Then MaxSwitch is 2`, `Then GetSwitchValue(0) is X`, `Then GetSwitch(1) is true` (after a limit hit), etc.
* Eight `tests/features/*.feature` files per the design-doc Module Structure section. Every scenario tagged `@wip`. Feature descriptions written as specifications (per `docs/skills/testing.md` §2.2); scenario titles state outcomes (§2.3); contract constants explicit in step text (§2.5).

Verification:

* `cargo test -p pa-falcon-rotator --features mock --test bdd` exits 0 (all scenarios skipped because `@wip`).

Removes:

* Nothing.

### 3a — Protocol layer

Implementation:

* `Command::to_command_string` per the design-doc Command Table.
* `parse_full_status(&str) -> Result<FalconStatus>` — splits on `:`, validates `FR_OK` prefix, parses each field. Trims trailing `\n` / whitespace per the ppba pattern.
* `parse_firmware_version(&str)`, `parse_position_deg(&str)`, `parse_position_steps(&str)`, `parse_voltage_raw(&str)`, `parse_is_running(&str)`, `parse_reverse(&str)` — one for every response shape.
* `validate_ping_response(&str) -> Result<()>` — accepts `FR_OK` ± trailing whitespace.
* `validate_echo(command, response)` — generic echo validator for `MD:`, `MS:`, `DR:`, `FH`, `FN:` shapes. (`SD:` is absent because the driver never issues `SD` — see the [design doc rationale](../../services/falcon-rotator.md#sync-semantics--why-driver-side-not-sd).) Non-echo commands (`Ping`, `FullStatus`, `FirmwareVersion`, `PositionDeg`, `PositionSteps`, `Voltage`, `IsRunning`) are rejected so a caller misrouting them fails loudly.

Tests:

* Inline `#[cfg(test)] mod tests` covering:
    - `parse_full_status` happy path: `FR_OK:4332:50.00:0:0:0:0` → expected `FalconStatus`.
    - Every failure mode: wrong prefix, too-few-fields, too-many-fields, bad steps, bad float, bad bool, empty input. Each as a separate test fn.
    - Non-finite `position_deg` rejection: `NaN`, `+inf`, `-inf` (for both `parse_full_status` and the standalone `parse_position_deg`) — `f64::parse` accepts the textual forms, so an explicit `is_finite()` check protects ASCOM clients from non-finite positions.
    - `parse_full_status` with trailing `\n` / CRLF.
    - `parse_full_status` with `limit_detect = 1` and all-flags-high.
    - Each command's `to_command_string()` exact-string check.
    - `validate_ping_response`: `FR_OK`, `FR_OK\n`, `  FR_OK \r\n`, reject `INVALID`, reject empty.
    - `validate_echo` for each echo-bearing shape (`MD`, `MS`, `DR:0`, `DR:<ms>`, `FH:1`, `FN:0`/`FN:1`) plus rejection of `Ping` / `FullStatus` / `IsRunning`.
* `tests/property_tests.rs`: serialize → parse round-trip on `FalconStatus` for random valid field values. Degrees are generated as integer hundredths so the `{:.2}` write format and the `f64` parse compare exactly without epsilon. Bazel `rust_test` target named `property_tests` mirrors the bdd target's dev-deps wiring (`rules_rust` does not auto-discover `tests/*.rs`).

Removes:

* Nothing from `@wip` yet — protocol alone can't drive BDD scenarios.

### 3b — Config + error + IO + serial + mock

Implementation:

* `config::load_config` — `std::fs::read_to_string` + `serde_json::from_str`.
* `error::FalconRotatorError::to_ascom_error` — `NotConnected → NOT_CONNECTED`, `InvalidValue → INVALID_VALUE`, all others → `ASCOMError::invalid_operation(self.to_string())`. Mirrors qhy-focuser exactly.
* `serial::TokioSerialPortFactory::open` — `tokio_serial::new(port, baud_rate).timeout(timeout).open_native_async()` → wrap in `TokioSerialReader` (`BufReader<ReadHalf>::read_line`) and `TokioSerialWriter` (`WriteHalf::write_all` + `flush`).
* `serial::TokioSerialPortFactory::port_exists` — `tokio_serial::available_ports()` lookup.
* `mock::MockSerialPortFactory` — full implementation. State:
    - `mech_position_deg: Arc<Mutex<f64>>` (default 0.0)
    - `is_moving: Arc<Mutex<bool>>` (default false; flipped by `MD:`, cleared on next `FA` after each move — best-effort simulation enough for BDD)
    - `motor_reverse: Arc<Mutex<bool>>` (default false)
    - `do_derotation: Arc<Mutex<bool>>` (default false; reset by `DR:0`)
    - `limit_detect: Arc<Mutex<bool>>` (default false; settable from a test hook)
    - `voltage_raw: Arc<Mutex<u32>>` (default 800)
    - `firmware_version: &'static str` (default `"1.3"`)
    - `command_log: Arc<Mutex<Vec<String>>>` — every command written, in order. Tests inspect.
* The mock reader's `read_line()` returns the queued response for the most recent command. The mock writer's `write_message()` appends to `command_log` and computes the canned response.
* The mock honours every command in the design-doc command table including `FF` (which we don't issue, but the mock should reject it if it ever shows up — fail loud rather than silently accept).

Tests:

* `config::tests` — defaults match design doc, JSON round-trip works, partial JSON fills in defaults.
* `error::tests` — Display strings, `to_ascom_error` mapping per the design-doc table.
* `io::tests` — `SerialPair` construction with `MockSerialReader` + `MockSerialWriter`.
* `mock::tests` (under `#[cfg(test)] mod tests` inside `src/mock.rs`) — `F#` → `FR_OK`, `MD:50.00` → echo + state update, `FH` → `FH:1` + `is_moving=false`, `FN:1` → echo + state, `FA` after `MD` returns updated position.

Removes:

* Nothing.

### 3c — SerialManager (handshake + command lock + driver-side state)

Implementation:

* `SerialManager::connect`:
    1. `connection_count.fetch_add(1, SeqCst)`. If `> 0`, return `Ok(())` (subsequent device connecting).
    2. `factory.open(...)` → store reader+writer.
    3. Handshake (sequential, each failure → `ConnectionFailed`):
        - `send_command_internal(Ping)` → `validate_ping_response`
        - `send_command_internal(FirmwareVersion)` → `parse_firmware_version` → `info!("Falcon firmware v{}", version)`
        - `send_command_internal(DerotationOff)` → echo validation
        - `send_command_internal(FullStatus)` → `parse_full_status` — drop the result, the read is just a smoke test
        - `send_command_internal(Voltage)` → `parse_voltage_raw` — same
    4. `serial_available.store(true, SeqCst)`.
* `SerialManager::disconnect`:
    1. Atomically decrement; if was already 0, no-op (underflow protection).
    2. If now 0: `serial_available.store(false, SeqCst)`, drop reader/writer, reset `sync_offset` to 0.0, reset `target_position` to None, reset `last_limit_detected` to None.
* `SerialManager::send_command_internal(cmd)`:
    1. `let _g = command_lock.lock().await;`
    2. `writer.write_message(&cmd.to_command_string()).await?;`
    3. `let resp = reader.read_line().await?.ok_or(Communication("closed"))?;`
    4. Return raw response — parsing is the caller's job.
* `SerialManager::read_status()` → `Result<FalconStatus>`:
    1. `send_command_internal(FullStatus)` → `parse_full_status`.
    2. Edge-check `limit_detect`: lock `last_limit_detected`, compare; on `Some(false) → true` transition or `None → true`, `warn!("Falcon reported limit_detect after move toward {:?}", *target_position.lock().await)`.
    3. Return parsed status.
* `SerialManager::read_voltage_raw()` → `Result<u32>`: `send_command_internal(Voltage)` → `parse_voltage_raw`.
* `SerialManager::move_mechanical(target_mech_deg)`:
    1. Connection check.
    2. `send_command_internal(MoveDeg(normalise(target_mech_deg)))` → echo validate.
* `SerialManager::halt()`:
    1. Connection check.
    2. `send_command_internal(Halt)` → echo validate.
    3. Clear `target_position` (`Mutex<Option<f64>>` ← `None`).
* `SerialManager::set_reverse(want: bool)`:
    1. Read `FA`, compare `motor_reverse`. If equal, return `Ok(())`.
    2. Else `send_command_internal(SetReverse(want))` → echo validate.
* `SerialManager::sync(sky_deg)`:
    1. Read `FA` to get current mechanical position.
    2. Compute `offset = (sky_deg - mech) mod 360`.
    3. Write `sync_offset.lock().await.write(offset)`.
* `SerialManager::sync_offset()` → `f64` getter.
* `SerialManager::set_target_position(sky_deg)`, `clear_target_position`, `target_position()` — accessor trio for `Mutex<Option<f64>>`.
* `normalise(deg) -> f64` — `((deg % 360.0) + 360.0) % 360.0`. Private helper.

Tests:

* `serial_manager::tests` (under `#[cfg(test)] mod tests` inside `src/serial_manager.rs`):
    - `test_connect_increments_refcount` — connect twice, disconnect once, still available.
    - `test_disconnect_underflow_protection` — disconnect without connect, no panic.
    - `test_connect_factory_error` — `MockSerialPortFactory` variant that returns `ConnectionFailed`.
    - `test_connect_handshake_ping_failure` — mock returns `BAD` for `F#`, expect `InvalidResponse`.
    - `test_set_reverse_skips_write_when_equal` — mock starts with reverse=true, call `set_reverse(true)`, assert only `FA` in `command_log` (no `FN`).
    - `test_set_reverse_writes_when_different` — mock starts with reverse=false, call `set_reverse(true)`, assert `FA` + `FN:1` in `command_log`.
    - `test_sync_offset_arithmetic` — mock at mech=120°, `sync(30°)` → offset = -90° (mod 360 = 270°).
    - `test_limit_detect_edge_log` — drive `read_status` twice with mock-state `limit_detect = false` then `true`, capture `tracing` output via `tracing-subscriber::fmt::TestWriter`, assert one `warn!` line.

Removes:

* Nothing (Rotator + Switch traits not implemented yet).

### 3d — Rotator device (Phase 3 first BDD-greening sub-phase)

**BDD harness pivot (landed in this sub-phase):** the Phase 2 scaffold targeted
the subprocess-style harness (`bdd_infra::bdd_main!` + `ServiceHandle`),
modelled on `qhy-focuser`. Phase 3d's feature scenarios verify wire-level
contracts that aren't observable over Alpaca HTTP alone — "F# was the first
command issued", "MD:284.80 was sent", "no FN command was sent", "no SD on
the wire" — so the BDD entry point was switched to run the
`ServerBuilder` in-process on an ephemeral port. The world holds an
`Arc<MockSerialPortFactory>` shared with the SerialManager, which gives
step bodies direct access to the wire command log and lets them seed mock
state (mechanical position, voltage, motor_reverse, limit_detect) without
plumbing those through Alpaca's API. The same client-side proxies
(`Arc<dyn Rotator>` / `Arc<dyn Switch>` from `AlpacaClient::get_devices`)
still drive the Alpaca HTTP surface, so the dispatch / serialisation
path is exercised end-to-end. The harness sets `config.server.auth =
None`, so the authentication layer is **not** covered by BDD; add a
dedicated auth-enabled scenario if that becomes a regression risk.

The pivot required:

* Switching `tests/bdd.rs` from the `bdd_infra::bdd_main!` macro (Miri shim
  for subprocess-spawning suites) to a plain `#[tokio::main] async fn main`
  — see `docs/skills/testing.md` §5.2, which explicitly notes the macro is
  needed only for harnesses that call `ServiceHandle::start`.
* Marking the BDD test with `required-features = ["mock"]` in
  `services/pa-falcon-rotator/Cargo.toml` because `tests/bdd/world.rs` now
  imports `MockSerialPortFactory` (feature-gated in `src/lib.rs`).
* Adding a read-only `MockSerialPortFactory::mech_position_deg()` getter
  so the `MechanicalPosition should be unchanged` step compares the
  Alpaca-reported value to the mock's current state, instead of hard-
  coding the value from the prior `Given the rotator reports mechanical
  position 142.30°` step.

Implementation in `rotator_device.rs`:

* `FalconRotatorDevice::new(config, serial_manager)`.
* `Device::static_name`, `unique_id`, `description`, `driver_info`, `driver_version`, `connected`, `set_connected` — exact qhy-focuser shape.
* `Rotator::can_reverse → Ok(true)`.
* `Rotator::is_moving` — `serial_manager.read_status().await.map(|s| s.is_moving)`.
* `Rotator::position` — `serial_manager.read_status().await.map(|s| normalise(s.position_deg + serial_manager.sync_offset()))`.
* `Rotator::mechanical_position` — `serial_manager.read_status().await.map(|s| normalise(s.position_deg))`.
* `Rotator::target_position`:
    - If `serial_manager.target_position().await.is_some()` → return that value. The stored target survives a successful move; it is cleared only by `Halt` or by the next `Move*`. This lets clients compare `Position` against `TargetPosition` post-move to verify they landed where they asked.
    - Else → return current `position()` (one fresh `FA`).
* `Rotator::reverse` (get) — `serial_manager.read_status().await.map(|s| s.motor_reverse)`.
* `Rotator::set_reverse(b)` — validate input not-NaN (n/a — `bool`), call `serial_manager.set_reverse(b)`.
* `Rotator::step_size → Ok(0.01155)`.
* `Rotator::halt`:
    - Connection check.
    - `serial_manager.halt().await`.
* `Rotator::move_(delta)`:
    - Validate finite (`delta.is_finite()`); else `InvalidValue`.
    - `current_sky = read_status().position_deg + sync_offset`.
    - `target_sky = normalise(current_sky + delta)`.
    - `set_target_position(target_sky)`, `move_mechanical(normalise(target_sky - sync_offset))`.
* `Rotator::move_absolute(sky_deg)`:
    - Validate finite.
    - `set_target_position(normalise(sky_deg))`, `move_mechanical(normalise(sky_deg - sync_offset))`.
* `Rotator::move_mechanical(mech_deg)`:
    - Validate finite.
    - The wire command is `MD:<normalise(mech_deg)>` directly (no offset applied to the wire value — matches the design-doc mapping table).
    - However the driver-side cache is kept in sky coordinates for consistency across `Move*` variants: `target_sky = normalise(mech_deg + sync_offset)`. So `set_target_position(target_sky)` followed by `move_mechanical(normalise(mech_deg))`.
* `Rotator::sync(sky_deg)`:
    - Validate finite.
    - `serial_manager.sync(sky_deg)`.

`lib.rs::ServerBuilder::build`:

* Register `FalconRotatorDevice` if `config.rotator.enabled`.
* Register `FalconStatusSwitchDevice` if `config.switch.enabled` (3e fills in the trait body).
* Standard `rp-tls` + `rp-auth` wiring per ppba/qhy.

Tests:

* `rotator_device::tests` — `to_ascom_error` mapping for each error variant; `Rotator::can_reverse`, `step_size`, `target_position` fallback.

BDD step bodies (in `tests/bdd/steps/`):

* `connection_steps.rs` — `Given a service with config X`, `When client PUT /connected?Connected=true`, `Then status is Ok`, `Then F# was the first command sent`.
* `metadata_steps.rs` — `Then CanReverse is true`, `Then StepSize is 0.01155`, `Then Name is "Pegasus Falcon Rotator"`.
* `position_steps.rs` — `Given the mock reports mechanical 142.30°`, `When client reads MechanicalPosition`, `Then value is 142.30`. `Given sync offset = -104.80°`, `Then Position is 37.50`.
* `movement_steps.rs` — `When client calls MoveAbsolute(180.0)`, `Then MD:284.80 was sent`. `When is_moving polled`, `Then FA was issued`.
* `halt_steps.rs` — `When client calls Halt`, `Then FH was sent`, `Then TargetPosition equals current Position`.
* `reverse_steps.rs` — read-before-write scenarios; assert `command_log` contains `FA` but no `FN` when value already matches.
* `sync_offset_steps.rs` — `When Sync(37.5)`, `Then MechanicalPosition unchanged`, `Then Position is 37.50`, `Then SD was NOT sent`.

Removes:

* `@wip` from `connection_lifecycle.feature`, `metadata.feature`, `position_reads.feature`, `movement.feature`, `halt.feature`, `reverse.feature`, `sync_offset.feature`.

### 3e — Status Switch device

Already in place on the PR #2 scaffold (added across the round-2 / round-3 review fixes), and **must be kept** when filling in the rest:

* `FalconStatusSwitchDevice::new(config, serial_manager)` — constructor.
* `Device` trait — full implementation (mirrors Rotator's shape).
* `ensure_connected!` macro at the top of `switch_device.rs` — runs as the first line of every device-bound method, **before** id validation, returning `NOT_CONNECTED` (1031) if `connected() == false`. Mirrors `ppba-driver`'s precedent (see `services/ppba-driver/src/switch_device.rs:22-29`).
* `validate_id(id)` helper — rejects ids outside `0..2` with `INVALID_VALUE`.
* `Switch::max_switch → Ok(2)`.
* `Switch::can_write(_) → Ok(false)` (with `ensure_connected!` + `validate_id`).
* `Switch::state_change_complete(_) → Ok(true)` — sync device, no async to wait for (matches `ppba-driver` precedent, **not** the prior `NOT_IMPLEMENTED` sketch).
* `Switch::set_switch / set_switch_value / set_switch_name → NOT_IMPLEMENTED` — no writable switches; names are fixed in config. All three carry `ensure_connected!` + `validate_id`. (Phase 3e originally pinned this to `INVALID_OPERATION`; Phase 3g's first ConformU run flagged it as the wrong wire code — "no writable switches" is a capability gap, not a state-dependent rejection — and the contract was retconned to `NOT_IMPLEMENTED`. The design doc and BDD scenario landed alongside the switch code change in the Phase 3g commit.)

3e's actual job is to fill in the seven currently-`unimplemented!()` getter methods and thread them through the **existing** `ensure_connected!` + `validate_id` helpers (in that order — guard first, validation second):

* `Switch::get_switch_name(0) → Ok("Input Voltage (raw)")`, `(1) → Ok("Limit Hit")`.
* `Switch::get_switch_description(0) → Ok("Raw input-voltage ADC count from the Falcon's VS command; scale not yet calibrated")`, `(1) → Ok("Mirrors FA.limit_detect for the most recent status read")`. The word "voltage" in the id-0 description is load-bearing: `status_switch.feature` asserts `Then the switch description should mention "voltage"` (lowercase, substring) so the description must surface "voltage" rather than only the wire-command name `VS`.
* `Switch::min_switch_value(0) → 0.0`, `(1) → 0.0`.
* `Switch::max_switch_value(0) → 1023.0`, `(1) → 1.0`.
* `Switch::switch_step(0) → 1.0`, `(1) → 1.0`.
* `Switch::get_switch_value(0)` — `serial_manager.read_voltage_raw().await.map(f64::from)`.
* `Switch::get_switch_value(1)` — `serial_manager.read_status().await.map(|s| if s.limit_detect { 1.0 } else { 0.0 })`.
* `Switch::get_switch(0)` — `get_switch_value(0).await.map(|v| v > 0.0)`.
* `Switch::get_switch(1)` — `get_switch_value(1).await.map(|v| v > 0.5)`.

~~The Switch V3 async surface (`can_async`, `set_async`, `set_async_value`, `cancel_async`) is intentionally left to the trait defaults (which return `NOT_IMPLEMENTED`) — only `state_change_complete` is explicitly overridden, because ConformU expects it to answer for sync devices.~~ **Superseded in Phase 3g:** ConformU flagged the trait defaults — `can_async` returns `Ok(false)` regardless of id, and the three writers return `NOT_IMPLEMENTED` without running id validation, so an out-of-range id was reported as "did not throw an exception" / "NotImplementedException returned for a method that must function per spec". The four methods are now explicit overrides chaining `ensure_connected!` + `validate_id` before the trait-default body (`Ok(false)` for `can_async`; `Err(NOT_IMPLEMENTED)` for the three writers). The override pattern mirrors `set_switch` et al.

BDD step bodies in `status_switch_steps.rs`:

* `Then MaxSwitch is 2`, `Then CanWrite(0) is false`, `Then GetSwitchValue(0) issued VS to the device`, `Given the mock voltage is 812`, `Then GetSwitchValue(0) returns 812`, `Given the mock limit_detect is 1`, `Then GetSwitch(1) is true`, `Then GetSwitchValue(1) returns 1.0`, `Then SetSwitch(0, true) returns INVALID_OPERATION`.

Tests:

* `switch_device::tests` keeps the existing `validate_id_*` and `*_returns_not_connected_when_disconnected` tests (they continue to drive the `NoopFactory` path so they need no MockSerialPortFactory state). The connected-device coverage for the seven getters lands as a sibling `#[cfg(all(test, feature = "mock"))] mod mock_tests` module — mirroring `serial_manager::mock_tests` — because the existing `tests` module's `disconnected_device()` helper uses `NoopFactory`, whose `open` panics, so it cannot drive `set_connected(true)` for the new per-id metadata / range / wire-command-issued / `INVALID_OPERATION`-when-connected assertions.

Removes:

* `@wip` from `status_switch.feature`.

### 3f — `tests/test_lib.rs` server-startup tests

Implementation in `services/pa-falcon-rotator/tests/test_lib.rs` (gated on `feature = "mock"`):

* `static SERVER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());` (same pattern as qhy-focuser).
* `test_server_starts_with_mock_factory` — build `ServerBuilder::new(test_config()).with_factory(MockSerialPortFactory::default()).build()`, bind to port `0`, hit `/management/v1/description`, assert HTTP 200.
* `test_server_registers_both_devices` — hit `/management/v1/configureddevices`, assert two entries (rotator + switch).
* `test_rotator_only_when_switch_disabled` — config with `switch.enabled = false`, assert one entry.

### 3g — ConformU integration

Implementation in `services/pa-falcon-rotator/tests/conformu_integration.rs`:

* Two `#[tokio::test] #[ignore]` tests — `conformu_compliance_tests_rotator`
  and `conformu_compliance_tests_switch` — mirroring the ppba-driver
  split (one per ASCOM class, the other device disabled in the JSON
  config so ConformU targets a single device at `/0`).
* Both acquire a `static CONFORMU_LOCK: Mutex<()>` to serialise the
  binds because the ASCOM Alpaca discovery service binds to a fixed
  UDP port (matches ppba-driver and qhy-focuser precedent).
* The binary is launched via `bdd_infra::ServiceHandle::try_start(env!("CARGO_PKG_NAME"), ...)`,
  which discovers the `target/debug/pa-falcon-rotator` binary, parses
  `bound_addr=<host>:<port>` from stdout (`parse_bound_port` is the
  underlying primitive — see [`docs/skills/testing.md` §5.1](../../skills/testing.md#51-shared-infrastructure-bdd-infra-crate)),
  and provides `handle.base_url` for `ConformUTestBuilder::new::<dyn Rotator>` /
  `::<dyn Switch>`.
* A shared `base_conformu_settings()` returns the standard boilerplate
  (`SettingsCompatibilityVersion`, `TestProperties`, `TestMethods`, the
  `Telescope*` / `Camera*` placeholders ConformU silently writes back
  even when the device is unrelated). Each test overrides only the
  keys relevant to its device class:
    - Rotator test adds `RotatorTimeout: 30` (down from the default 60 —
      the mock backend responds in microseconds, mirroring the
      `FocuserTimeout: 30` precedent in `services/qhy-focuser/tests/conformu_integration.rs`).
    - Switch test adds `SwitchEnableSet: false`, `SwitchReadDelay: 50`,
      `SwitchWriteDelay: 100`, `SwitchExtendedNumberTestRange: 100`,
      `SwitchAsyncTimeout: 10`, `SwitchTestOffsets: true` — same
      values as `services/ppba-driver/tests/conformu_integration.rs`,
      which cuts the Switch run from ~8 min to ~35 s on CI.
* `handle.stop()` is called unconditionally after capturing the
  ConformU result so the service gets a graceful SIGTERM (and writes
  coverage `.profraw` data) even when the conformance run fails.

`Cargo.toml`:

* `[features] conformu = ["mock", "ascom-alpaca/test"]` already in 2a; this
  sub-phase adds `[package.metadata.conformu] command = "cargo test -p pa-falcon-rotator --features conformu --test conformu_integration -- --ignored --nocapture"`.
* `.github/workflows/conformu.yml` discovers new entries dynamically via `cargo metadata` (per `docs/skills/pre-push.md`) — no workflow edit needed.

Switch contract fixes surfaced by the first ConformU run:

* `set_switch` / `set_switch_value` / `set_switch_name` now return
  `NOT_IMPLEMENTED` (1024) instead of `INVALID_OPERATION` (1035).
  ConformU reads "no writable switches" as a capability gap, not a
  state-dependent rejection — see the [Switch layout](../../services/falcon-rotator.md#write-surface-read-only-device)
  doc section for the rationale. The design-doc Error Model table and
  the `tests/features/status_switch.feature` scenario landed in the same
  commit.
* The four `ISwitchV3` async methods (`can_async`, `set_async`,
  `set_async_value`, `cancel_async`) are now explicit overrides that run
  `ensure_connected!` + `validate_id` before falling through to the
  trait-default body. The trait defaults bypass id validation, which
  ConformU flags as "did not throw an exception" for `CanAsync(id ≥ MaxSwitch)`
  and "NotImplementedException returned where InvalidValueException is
  required" for the three writers.

### 3h — README + workspace.md final updates

Implementation:

* `README.md` Services table — add a `pa-falcon-rotator` row between `star-adventurer-gti` and the `[cov-*]` block. Update `[cov-pa-falcon-rotator]` markdown reference links at the bottom. Add a short `### pa-falcon-rotator` paragraph under the Services list.
* `docs/workspace.md` Services table — flip the phase note on the `pa-falcon-rotator` row from `(Phase 1 — design landed; implementation tracked in [`docs/plans/pa-falcon-rotator-implementation.md`](plans/pa-falcon-rotator-implementation.md))` (added in PR #1) to its production form (drop the parenthetical entirely).
* `docs/workspace.md` Plans table — flip the entry on this plan from "active" to "archived 2026-MM-DD" and move the file to `docs/plans/archive/`.
* If we adopt sentinel integration: add a sentinel monitor for the rotator port (deferred — outside this plan).

Removes:

* Nothing in code; this is the documentation closing-out commit.

## Order of execution

`2a + 2b → 3a → 3b → 3c → 3d → 3e → 3f → 3g → 3h`.

All sub-phases ship as separate commits on the same branch (PR #2 — see [Branching strategy](#branching-strategy)). Ordering is strict in the commit stream — every sub-phase depends on the previous one compiling. Sub-phases 3a–3c are behaviourally invisible to ASCOM clients (no `@wip` removed); 3d / 3e are where the BDD scenarios start going green.

## Hardware validation

After Phase 3e (Switch device) lands as a commit, run the binary against the actual Falcon v1:

1. Plug the Falcon into a Linux box.
2. `cargo run -p pa-falcon-rotator -- -c examples/config-linux.json`.
3. From another shell, exercise the Alpaca API via `curl`:
    - `curl -X PUT http://127.0.0.1:11118/api/v1/rotator/0/connected -d "Connected=true"`
    - `curl http://127.0.0.1:11118/api/v1/rotator/0/mechanicalposition`
    - `curl -X PUT http://127.0.0.1:11118/api/v1/rotator/0/sync -d "Position=0.0"`
    - `curl -X PUT http://127.0.0.1:11118/api/v1/rotator/0/moveabsolute -d "Position=90.0"`
    - `curl http://127.0.0.1:11118/api/v1/rotator/0/ismoving` (poll until `false`)
    - `curl http://127.0.0.1:11118/api/v1/rotator/0/position`
    - `curl http://127.0.0.1:11118/api/v1/switch/0/getswitchvalue?Id=0` (raw voltage)
    - `curl http://127.0.0.1:11118/api/v1/rotator/0/connected -d "Connected=false" -X PUT`
4. Then run ConformU against real hardware: `conformu conformance http://127.0.0.1:11118/api/v1/rotator/0` and same against `/switch/0`. Both should pass with zero errors.

This validation surfaces any discrepancies between the design doc's behavioural assumptions and real hardware. Capture findings as follow-up issues against the design doc rather than blocking the PR — the design is allowed to evolve once hardware truth is in.

## Follow-ups (post-MVP)

Tracked in the design doc's [`Follow-ups`](../../services/falcon-rotator.md#follow-ups) section:

1. Voltage scale calibration (raw ADC → volts).
2. ADC width verification.
3. Sync offset persistence (if operators ask).
4. De-rotation surface (Custom Action / extra Switch / extra device).
5. Falcon v2 protocol support.

None of these block this implementation plan from being considered complete.

## References

* [`docs/services/falcon-rotator.md`](../../services/falcon-rotator.md) — the design contract this plan implements.
* [`docs/skills/development-workflow.md`](../../skills/development-workflow.md) — Phase 1/2/3 ordering.
* [`docs/skills/testing.md`](../../skills/testing.md) — BDD conventions (`@wip` usage §2.7, contract constants §2.5, `bdd_main!` §5.2).
* [`docs/skills/pre-push.md`](../../skills/pre-push.md) — quality-gate commands run between each sub-phase.
* `services/qhy-focuser/src/serial_manager.rs` — closest template for `SerialManager`.
* `services/qhy-focuser/src/focuser_device.rs` — closest template for the ASCOM device trait wrapper pattern.
* `services/ppba-driver/src/switch_device.rs` and `switches.rs` — template for `FalconStatusSwitchDevice`.
* `services/ppba-driver/src/lib.rs` and `services/qhy-focuser/src/lib.rs` — `ServerBuilder` template.
* [Pegasus Astro Falcon Rotator product page](https://pegasusastro.com/products/falcon-rotator/) — source of the 86.6 steps/° and 220° CW soft-limit figures.
* [Falcon Serial Command Table (firmware ≥ v1.3)](https://pegasusastro.com/wp-content/uploads/2022/05/Falcon_Serial_Command_Table.pdf) — wire-protocol reference.
