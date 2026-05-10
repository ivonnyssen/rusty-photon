# Star Adventurer GTi — Phase 3 Implementation Plan

## Status

**Active.** Phase 1 (design doc, PR #178) and Phase 2 (BDD scaffold +
codec + service skeleton, PR #180) are landed. Phase 3 fills in every
`unimplemented!()` body and removes the `@wip` tags from the 54 BDD
scenarios, in the order described below.

## Outcomes (definition of done)

* Every `#[cfg_attr(coverage_nightly, coverage(off))]` annotation
  attached to a Phase 2 stub is removed, replaced by a real body and at
  least one test that exercises it.
* All 9 `tests/features/*.feature` files have their `@wip` tags
  removed; `cargo test --features mock --test bdd` runs the full 54
  scenarios.
* `cargo run -p star-adventurer-gti -- --config /path/to.json` boots
  against a real GTi (USB or UDP) and survives a NINA / SGPro / `rp`
  connect → slew → track → park → disconnect cycle.
* `tests/test_lib.rs` server-startup tests land alongside the
  feature-gated mock; `tests/conformu_integration.rs` lands and the
  `[package.metadata.conformu]` block is re-added so the nightly
  ConformU workflow picks the service up.
* Coverage on the codec + service stays >90% (codecov/patch goes
  green without `coverage(off)` crutches).

## Branching strategy

Phase 3 work happens on `feature/star-adventurer-gti-phase3` based on
`feature/star-adventurer-gti-phase2` (the PR #180 branch). Sub-phases
land as separate commits on the same branch so reviewers can step
through them. Once #180 merges, the Phase 3 branch is rebased onto
main.

## Sub-phases

Each sub-phase ends in a single commit. The `@wip` tags listed under
"Removes" are the ones taken off in that sub-phase.

### 3a — `skywatcher-motor-protocol` codec

Pure functions. No service dependencies. First because every later
sub-phase calls into it.

Implementation:
* `codec::encode_u8`, `decode_u8` (`u8` ↔ two ASCII hex digits, case
  insensitive on decode, upper-case on encode).
* `codec::encode_u24`, `decode_u24` (24-bit value ↔ six ASCII hex
  bytes, low byte first).
* `codec::encode_position`, `decode_position` (signed `i32` ticks ↔
  six hex bytes, with `+0x800000` bias). Range-check on encode.
* `codec::validate_command_frame`, `validate_response_frame` (UDP
  receive-side framing rules from `docs/references/skywatcher-motor-controller-command-set.md`
  §"UDP framing strictness").
* `Command::encode_into` for every variant per the design-doc
  "Commands used by the MVP" table:
    - `:F<axis>` — Initialize
    - `:a<axis>` — InquireCpr
    - `:b1` — InquireTmrFreq (axis 1 only)
    - `:g<axis>`, `:e<axis>`, `:j<axis>`, `:f<axis>` — inquiries
    - `:G<axis><mode2>` — SetMotionMode (encode `MotionMode` as one
      byte: spec §4.1; high nibble selects goto/tracking + fast/slow,
      low nibble selects direction. Cross-check against EQMOD source
      `EQModulator.cpp::EQ_SetMotionMode`.)
    - `:S<axis><pos24>` — SetGotoTarget (`encode_position`)
    - `:I<axis><period24>` — SetStepPeriod (`encode_u24`)
    - `:E<axis><pos24>` — SetPosition (`encode_position`)
    - `:J<axis>`, `:K<axis>`, `:L<axis>` — start/stop/instant-stop
* `Response::decode(frame, in_reply_to)` — branch on `=` vs `!`,
  dispatch payload shape on the originating `Command`. Use
  `Response::axis_of` (already implemented) to know which axis the
  reply belongs to.

Tests:
* Inline `#[cfg(test)] mod tests` in each source file with
  representative cases:
    - `encode_u24(0x12_3456) == [b'5', b'6', b'3', b'4', b'1', b'2']`
      (the design doc's worked example).
    - `decode_u24(b"00000F") == Ok(0x0F00_0000 & 0x00FF_FFFF) == Ok(0x0F00_00)`
      → wait, `00000F` is `[b'0', b'0', b'0', b'0', b'0', b'F']` →
      bytes `0x00, 0x00, 0x0F` → low byte first → `0x0F0000`. Pin
      with the spec's worked examples.
    - `encode_position(0) == bias` (`0x800000` → `"000080"`).
    - `encode_position(-1) → "FFFF7F"` (`0x7FFFFF` low-byte first).
    - Round-trip `Command::encode_into` + parse the bytes back into
      a `Command` mirror parser (only used in tests) → equality.
* `tests/property_tests.rs`:
    - For random `i32` in `-2^23..2^23`, `decode_position(encode_position(x)) == x`.
    - For random `u32 & 0xFF_FFFF`, round-trip through u24.
    - For random `Command` (proptest-generated), encode → decode must
      not panic, and re-encoding the decoded form yields the same
      bytes (canonical form check).

Removes:
* `coverage(off)` from the seven codec stubs and the two `Command` /
  `Response` methods.
* No `@wip` removal yet — the BDD scenarios still need the rest of the
  stack.

### 3b — Mock transport state machine + handshake

Implementation:
* `transport/mock.rs::MockTransport::round_trip` — parse the inbound
  `:cmd<axis><payload>\r` frame, mutate `MockMountState` accordingly,
  emit `=...\r` / `!XX\r`. Coverage:
    - Inquiries (`:a`, `:b`, `:g`, `:e`, `:j`, `:f`) read the cached
      values.
    - Setters (`:F`, `:G`, `:S`, `:I`, `:E`, `:J`, `:K`, `:L`) update
      per-axis state.
    - Motion is best-effort — the mock advances `ra.position_ticks`
      toward `goto_target_ticks` on each `:f` poll (so BDD tests can
      assert "Slewing eventually false"). This is enough to satisfy
      the slew scenarios; nothing more.
    - Error coverage: motion command before `:F` returns `!04` (not
      initialised); unknown command returns `!00`.
* `transport_manager::TransportManager::connect` —
    1. `factory.open(&config)` (already wired).
    2. Run handshake commands in design-doc order: `:F1`, `:F2`,
       `:a1`, `:a2`, `:b1`, `:g1`, `:g2`, `:e1`, `:j1`, `:j2`.
    3. Populate `MountParameters` cache.
    4. Spawn `tokio::task::spawn` polling task: every
       `config.transport.polling_interval` (USB or UDP) issue
       `:f<axis>` and `:j<axis>` for both axes, update `MountSnapshot`.
    5. `available.store(true, SeqCst)`.
* `transport_manager::TransportManager::disconnect` — decrement count;
  on 0 transition, abort poll task, send `:K1` (stop tracking),
  `*self.transport.lock().await = None` (drops Arc → triggers
  `Transport::close`), clear parameters cache, `available.store(false)`.
* `transport_manager::TransportManager::send` — `command.encode()` →
  `transport.round_trip` → `Response::decode`.

Tests:
* `transport/mock.rs` unit tests: feed each `Command` variant to
  `round_trip`, assert reply shape + state-machine side effect.
* `transport_manager.rs` `mock`-feature tests: connect/disconnect ref
  counting, parameter cache populated after handshake, snapshot
  updated by polling task.

Removes:
* `@wip` from `connection_lifecycle.feature`.
* `coverage(off)` from `MockTransport::round_trip`,
  `TransportManager::connect`/`disconnect`/`send`.

### 3c — Coordinates math

Implementation:
* `coordinates::ra_ticks_to_mechanical_ha` — `ticks * 24 / cpr` then
  fold into `[-12, +12)`.
* `coordinates::dec_ticks_to_degrees` — `ticks * 360 / cpr`; pole
  fold-through (>90° wraps back; < -90° wraps the other way) per the
  design doc's Dec encoder convention.
* `coordinates::local_sidereal_time_hours` — wrap
  `erfars::utc_to_gst` (or `erfars::s00b` + equation-of-equinoxes; copy
  the rp-ephemeris pattern) plus site longitude (east positive).
  Unit-test against rp-ephemeris's GMST regression numbers.
* `coordinates::mechanical_ha_to_ra` — `ra = lst - mech_ha`, fold
  `[0, 24)`.
* `coordinates::side_of_pier` — northern hemisphere: mech HA in
  `[-6, +6)` → East, else West. Southern hemisphere inverts. Pin every
  scenario in `side_of_pier.feature` as a unit test row too.

Tests:
* All inline `#[cfg(test)]` — encoder=0, encoder=cpr/4, encoder=-cpr/4,
  encoder=cpr/2, +/- pole flips, both hemispheres.

Removes:
* `coverage(off)` from every stub in `coordinates.rs`.
* No `@wip` removal — this just unblocks 3d.

### 3d — MountDevice reads + tracking + sync

Implementation:
* `MountDevice::set_connected` — drives `TransportManager::connect`/
  `disconnect`; updates `requested_connection`.
* `MountDevice::right_ascension` / `declination` — read snapshot,
  apply `coordinates` math + sync-offset.
* `MountDevice::azimuth` / `altitude` — derive from RA/Dec + site +
  LST.
* `MountDevice::sidereal_time` — `coordinates::local_sidereal_time_hours`.
* `MountDevice::side_of_pier` — `coordinates::side_of_pier`.
* `MountDevice::slewing` — read snapshot.ra.running ||
  snapshot.dec.running while in goto mode.
* `MountDevice::set_tracking(true)` — `:G1<sidereal>`, `:I1<period>`,
  `:J1`. Step period: `tmr_freq / sidereal_rate_in_steps_per_sec`.
* `MountDevice::set_tracking(false)` — `:K1`.
* `MountDevice::sync_to_coordinates(ra, dec)` — validate ranges
  (0..24, -90..90), reject if `at_park`. Compute target encoder ticks
  for current RA/Dec, issue `:E1<pos>` and `:E2<pos>`. Update sync
  offset.

Removes:
* `@wip` from `coordinate_reads.feature`, `tracking.feature`,
  `sync.feature`.

### 3e — Slew

Implementation:
* `MountDevice::slew_to_coordinates_async(ra, dec)` — validate, refuse
  if parked, compute target ticks via `coordinates`, decide direction
  (sign of delta), issue per axis: `:K`, poll `:f` until stopped (1 s
  cap), `:G<goto-mode>`, `:S<target>`, `:J`. Set `Slewing = true`.
* `MountDevice::slew_to_target_async` — uses last-set
  `target_ra_hours` / `target_dec_degrees`.
* `MountDevice::set_target_right_ascension` / `set_target_declination`
  — validated setters that store on `DriverState`.
* Background slew-completion watcher (spawned from `slew_to_*`) —
  poll `:f1` and `:f2` every `polling_interval`; when both report
  Running=0 in Goto mode, optionally re-issue tracking-mode `:G`/`:I`/
  `:J` on RA if `tracking_requested`, then sleep
  `config.settle_after_slew`, then clear Slewing.

Removes:
* `@wip` from `slew.feature`.

### 3f — Park / Abort / SideOfPier

Implementation:
* `MountDevice::park` — refuse if already parked. Set
  `tracking_requested = false`, `:K1`. Issue per axis: `:G<goto>`,
  `:S<encoder=0>`, `:J`. Background watcher waits for both stopped at
  encoder 0, then sets `at_park = true`.
* `MountDevice::unpark` — clear `at_park` flag (no motion).
* `MountDevice::abort_slew` — `:L1`, `:L2`. Clear Slewing immediately.
  Do *not* re-enable tracking.
* `MountDevice::set_park` — return `NOT_IMPLEMENTED` (MVP boundary).

Removes:
* `@wip` from `park.feature`, `abort.feature`, `side_of_pier.feature`,
  `device_metadata.feature`.

### 3g — Real serial + UDP transports

Implementation:
* `transport/serial.rs::SerialTransport::connect` —
  `tokio_serial::new(port, baud_rate).open_native_async()`. Set 8N1,
  raw mode. Store reader+writer in the struct.
* `SerialTransport::round_trip` — write request, read until `\r` (or
  timeout). Concurrent send-and-receive: a `Mutex<()>` command lock so
  only one outstanding round-trip at a time (replies have no
  request-ID; matching them to commands is purely temporal).
* `SerialTransport::close` — drop the port handle.
* `transport/udp.rs::UdpTransport::connect` — `tokio::net::UdpSocket::bind((bind_address, 0))` then
  `connect((address, port))`. The mandatory `bind_address` enforcement
  is the source-IP gotcha from the design doc.
* `UdpTransport::round_trip` — `send_to`, `recv` with timeout,
  validate single-frame UDP rule via `validate_response_frame`.
* `UdpTransport::close` — drop the socket.

Tests:
* `tokio::task::spawn` a localhost UDP echo server in a unit test that
  echoes back canned `=000080\r` for any input. Drive `UdpTransport`
  against it; assert round-trip works and that an extra trailing byte
  fails framing validation.
* For `SerialTransport`, mock at the trait level (or skip — there is
  no portable equivalent of localhost UDP for serial; rely on the
  ConformU integration test against the binary running with a virtual
  serial pair).

Removes:
* `coverage(off)` from each `unimplemented!()` in `serial.rs` and
  `udp.rs`.
* No `@wip` removal — these only matter when running against real
  hardware.

### 3h — `tests/test_lib.rs` + ConformU

Implementation:
* `tests/test_lib.rs` (gated on `feature = "mock"`) — server startup
  smoke tests: spawn `ServerBuilder::with_transport_factory(MockTransportFactory)`,
  bind to port 0, hit `/management/v1/description`, `/management/v1/configureddevices`.
  Same `SERVER_LOCK: Mutex<()>` pattern as qhy-focuser.
* `tests/conformu_integration.rs` (gated on `feature = "conformu"`) —
  spawn the binary with mock transport, run ConformU's Telescope
  conformance suite. `#[ignore]` so it only runs under
  `cargo test --features conformu --test conformu_integration -- --ignored --nocapture`.
* Re-add the `[package.metadata.conformu]` block to `Cargo.toml` (was
  removed in PR #180 round 3 because the test file didn't exist yet).

Removes:
* Nothing left — Phase 3 should be complete.

## Order of execution

`3a → 3b → 3c → 3d → 3e → 3f → 3g → 3h → 3i`. Each sub-phase ends in
a single commit. PRs split as follows:

* **PR (Phase 3a–3h)**: everything except BDD wiring. The driver is
  fully functional through the ASCOM API; unit + integration coverage
  reaches 90 tests across `lib`, `test_lib`, and `tests/property_tests.rs`.
* **PR (Phase 3i)**: BDD step bodies + `@wip` removal. Adds a
  feature-gated `/debug/v1/mock-commands` endpoint so scenarios that
  assert on wire-protocol frames (e.g. `Then the mount should have
  received command :K1`) can read the mock's `command_log` from the
  test process. Roughly half of the 54 scenarios are API-only and can
  remove `@wip` immediately; the other half land alongside the debug
  endpoint.

### 3i — BDD step bodies + remove all @wip tags

Implementation:
* `World::start_service`: spawn the service binary via
  `bdd_infra::ServiceHandle`, build `Arc<dyn Telescope>` from the
  AlpacaClient, store both on the World.
* Mock-mode debug endpoint: under `feature = "mock"`, mount an axum
  `/debug/v1/mock-commands` handler on the same router that returns
  the mock's `command_log` as JSON. Gated tightly so production
  builds never expose it.
* Per-feature-file step bodies: replace each `todo!()` with the real
  driver call (`mount.set_connected(true)`, `mount.slew_to_*`, ...).
* Remove `@wip` from each feature file as its step bodies go green.

Removes:
* Every remaining `@wip` tag.

## References

* `docs/services/star-adventurer-gti.md` §"Commands used by the MVP"
  and §"Slew lifecycle" / §"Park lifecycle" — load-bearing for 3a, 3e,
  3f.
* `docs/references/skywatcher-motor-controller-command-set.md` —
  authoritative wire-format reference, especially §"UDP framing
  strictness" for 3g and the §"Empirically verified" probe table for
  3b's mock state.
* [INDI eqmod driver source][indi-eqmod] — cross-check `:G` motion
  mode bit layout when implementing 3a.
* `crates/rp-ephemeris/src/erfars_impl.rs` — pattern for 3c's LST.
* `services/qhy-focuser/src/serial_manager.rs` — pattern for 3b's
  ref-counted connect/disconnect + polling task.
* `services/qhy-focuser/src/serial.rs` — pattern for 3g's
  `SerialTransport::round_trip`.
* `services/qhy-focuser/tests/test_lib.rs` — pattern for 3h.

[indi-eqmod]: https://github.com/indilib/indi-3rdparty/tree/master/indi-eqmod
