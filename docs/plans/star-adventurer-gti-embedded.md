# Star Adventurer GTi — embedded (MCU) port plan

## Status

**Not started.** Boards on order. This plan captures the design discussion
from the conversation on 2026-05-11 so future sessions can pick it up
without re-deriving the architecture. Parent service:
[`docs/services/star-adventurer-gti.md`](../services/star-adventurer-gti.md).

## Goal

Run the `star-adventurer-gti` ASCOM Alpaca Telescope driver on a
microcontroller, with **no functional regression** vs the desktop service:

- Full ASCOM Alpaca over HTTP/JSON
- HTTPS (TLS 1.2/1.3 server)
- Bearer-token authentication (port of `rp-auth`)
- USB-CDC transport to the mount (MCU acts as USB host)
- WiFi station mode for Alpaca clients on the user's network
- ConformU `conformance` phase reports the same exception list as the
  desktop driver (see service doc §"Running ConformU manually")

Non-goals (out of this port; deferred items inherit from the parent MVP):

- WiFi AP-mode transport to the mount (single-radio MCU can't join two
  networks simultaneously; USB-CDC is the only mount path)
- PulseGuide, MoveAxis, DestinationSideOfPier — still deferred
- Replacing the desktop service. Both binaries continue to exist; the
  embedded port is an alternate deployment, not a successor.

## Hardware

Two boards in hand for evaluation:

1. **Espressif ESP32-S3-DevKitC-1, N16R8** (16 MB flash, 8 MB PSRAM,
   WiFi 4, USB-OTG). Primary target. Dual USB-C: one for
   programming/console, one for native USB host to the mount.
2. **Raspberry Pi Pico 2 W (RP2350 + CYW43439)**. Secondary target,
   booted in **RISC-V mode** (Hazard3 cores) — same hardware can also
   boot as Cortex-M33, but the point of this board is the pure-Rust /
   mainline-rustc path.

The ESP32-S3 is the lower-risk path because (a) USB host CDC has a more
production-proven story (TinyUSB via `esp-idf-svc`) and (b) `esp-mbedtls`
is the most battle-tested TLS option in embedded Rust today. The RP2350
work happens **after** the ESP32-S3 port lands as a way to validate that
the shared embedded crates are actually portable.

## Architecture

```
                    ┌───────────────────────────────┐
                    │ ASCOM Alpaca clients          │
                    │ (NINA, SGPro, rp, …)          │
                    └───────────────┬───────────────┘
                                    │ HTTPS + Bearer auth
                                    │ over WiFi (STA mode)
                                    ▼
        ┌────────────────────────────────────────────────────┐
        │ MCU firmware                                       │
        │                                                    │
        │  ┌──────────────────────────────────────────────┐  │
        │  │ picoserve HTTP server + esp-mbedtls TLS      │  │
        │  │  └─ Alpaca handler (hand-written, ~500 LOC)  │  │
        │  │  └─ Bearer auth (port of rp-auth, ~50 LOC)   │  │
        │  └────────────────────┬─────────────────────────┘  │
        │                       │ MountDevice trait calls    │
        │                       ▼                            │
        │  ┌──────────────────────────────────────────────┐  │
        │  │ TransportManager + coordinates + slew ctrl   │  │
        │  │  (no_std port of services/star-adventurer-…) │  │
        │  └────────────────────┬─────────────────────────┘  │
        │                       │ Command/Response frames    │
        │                       ▼                            │
        │  ┌──────────────────────────────────────────────┐  │
        │  │ skywatcher-motor-protocol (no_std)           │  │
        │  └────────────────────┬─────────────────────────┘  │
        │                       │ bytes                      │
        │                       ▼                            │
        │  ┌──────────────────────────────────────────────┐  │
        │  │ USB-CDC host transport (esp-idf USB Host)    │  │
        │  └────────────────────┬─────────────────────────┘  │
        └───────────────────────┼────────────────────────────┘
                                ▼
                        USB-C to mount
```

The desktop service's module boundaries carry over almost 1:1; what
changes is the implementation of each layer's I/O surface.

## Repo layout

New top-level directory, **excluded from the main Cargo workspace** (host
target vs xtensa/riscv target):

```
firmware/
  Cargo.toml                            — nested workspace
  alpaca-embedded/                      — no_std Alpaca handler crate
    Cargo.toml
    src/
      lib.rs                            — picoserve routes + handlers
      json.rs                           — serde-json-core wrappers
      telescope.rs                      — ITelescopeV3 handler dispatch
      discovery.rs                      — UDP/32227 Alpaca discovery
  rp-auth-embedded/                     — no_std port of crates/rp-auth
  star-adventurer-gti-esp32s3/          — ESP32-S3 binary crate
    Cargo.toml
    .cargo/config.toml                  — target = xtensa-esp32s3-none-elf
    rust-toolchain.toml                 — pins esp-rs fork
    build.rs                            — links cert/key + token blobs
    src/
      main.rs                           — Embassy entry point
      transport.rs                      — USB-CDC host (esp-idf USB Host)
      tls.rs                            — esp-mbedtls server config
      wifi.rs                           — STA join, SNTP, AP-fallback provisioning
      flash_storage.rs                  — config + cert + token persistence
  star-adventurer-gti-rp2350/           — RP2350 RISC-V binary crate (Phase 9)
    (same shape; target = riscv32imac-unknown-none-elf)
```

Top-level `Cargo.toml` adds `exclude = ["firmware"]` so host `cargo build
--workspace` stays unaffected. The nested `firmware/Cargo.toml` is its
own workspace.

Existing `crates/skywatcher-motor-protocol` gains a `no_std` feature
(default `std`) — same crate, both targets. Existing
`services/star-adventurer-gti` is untouched.

## Crate stack (no_std)

| Layer | Crate | Notes |
|---|---|---|
| Async runtime | `embassy-executor`, `embassy-time` | |
| HAL | `esp-hal` (ESP32-S3) / `embassy-rp` (RP2350) | |
| WiFi | `esp-wifi` (ESP32-S3) / `cyw43` (RP2350) | STA mode |
| TCP/UDP | `embassy-net` (smoltcp) | |
| USB host CDC | `esp-idf-svc` USB Host (ESP32-S3) | RP2350 path = `embassy-usb-host` when ready |
| HTTP server | `picoserve` | |
| TLS | `esp-mbedtls` (ESP32-S3) / `embedded-tls` (RP2350) | |
| JSON | `serde-json-core` + `heapless` | bounded buffers |
| Time / SNTP | `embassy-net` SNTP client | LST math needs ±1 s clock |
| Logging | `defmt` + `defmt-rtt` | |
| Auth crypto | `subtle` (constant-time compare) | |
| Protocol | `skywatcher-motor-protocol` (no_std) | shared with desktop |

## Phases

Each phase ends in one or more commits on a feature branch. The phases
are ordered so that **each one ends with something demonstrable on the
bench**, and the highest-risk work (USB host) is first.

### Phase 0 — USB host CDC spike  *(kills the biggest unknown)*

**Goal:** prove the ESP32-S3 can open USB-CDC to the mount and round-trip
a single `:e1\r` command. Nothing else.

- New `firmware/spikes/usb-cdc-hello/` (throwaway crate; deleted at the
  end of Phase 1).
- `cargo generate esp-rs/esp-template`.
- Open USB host CDC, write `:e1\r`, read until `\r`, print the response
  bytes via `defmt-rtt`.
- Expected reply: `=03300C\r` (mount-type 0x03, firmware version 0x30.0x0C).

**Done when:** firmware boots, enumerates the mount, prints the firmware
version. If `esp-idf-svc` USB Host fights, fall back to TinyUSB host with
C bindings before considering this blocked. Time-box to 2 days; if not
working by then, escalate (revisit board choice or transport).

### Phase 1 — `no_std`-ify `skywatcher-motor-protocol`

**Goal:** the crate compiles for `xtensa-esp32s3-none-elf` and the
desktop service still passes its full BDD + property-test suite.

- Add `#![cfg_attr(not(feature = "std"), no_std)]`.
- Replace `Vec<u8>` in `Command::encode_into` API with
  `&mut heapless::Vec<u8, N>` *or* an `embedded-io` writer. Pick the
  writer approach — keeps it allocation-free and lets the desktop side
  pass `&mut Vec<u8>` (which implements the same trait via `alloc`).
- Make `ProtocolError` `Display`-implementable under `no_std` (manual
  `core::fmt::Display` impl, no `thiserror`).
- Update `crates/skywatcher-motor-protocol/Cargo.toml`:
  `default-features = ["std"]`, `std = ["alloc"]`, `alloc = []`.
- Run the desktop service's existing tests; nothing should break.
- Add a `firmware/spikes/protocol-roundtrip/` that imports the crate
  with `default-features = false` and round-trips `Command::encode_into`
  / `Response::decode` for one of each variant. Compile-checks the
  no_std path.

### Phase 2 — Embedded transport + parameter cache

**Goal:** firmware connects to the mount, runs the init handshake
(`:F`, `:a`, `:b`, `:g`, `:e`, `:j`), and prints the cached parameters
plus a live position read every second.

- `firmware/star-adventurer-gti-esp32s3/` proper (not a spike).
- Embassy-based `Transport` over USB-CDC host. `embedded-io-async` traits.
- `TransportManager` ported: ref-counted shared transport, background
  polling task, parameter cache (CPR per axis, TMR_Freq, high-speed
  ratio per axis, motor board version).
- Handshake sequence matches desktop driver's exactly.
- Logs over `defmt-rtt`.

No WiFi, no HTTP, no TLS yet. Pure USB → mount → console output.

### Phase 3 — Coordinate math + slew controller

**Goal:** can issue a slew programmatically (hardcoded RA/Dec in
firmware) and observe the mount drive there.

- Port `coordinates.rs` to `no_std`. Replace `chrono` with manual
  Julian-date math using `embassy-time::Instant` + a SNTP-synced epoch.
- LST computation from UTC + site longitude (taken from compile-time
  config for this phase; provisioned in Phase 7).
- Slew lifecycle: `:K` → poll `:f` for `running=false` → `:G` → `:S`
  → `:J`, then background-poll `:f` until both axes stop. Matches the
  desktop driver's Phase 4 hardware-bring-up patches (LL stop-and-wait,
  no `:I` in goto mode, mechanical safety envelope).
- Mechanical safety envelope **mandatory** — defaults to `±6 h` RA,
  `±90°` Dec, same as desktop.
- Test: hardcode two RA/Dec targets in firmware; press a GPIO button to
  alternate between them. Observe correct mechanical motion.

### Phase 4 — Embedded Alpaca server (HTTP, no TLS, no auth)

**Goal:** any HTTP client on the local network can talk Alpaca to the
firmware on port 11117.

- `firmware/alpaca-embedded/` crate (target-agnostic, depends on
  `picoserve` + `serde-json-core` + `heapless`).
- Hand-written handlers for the ITelescopeV3 surface declared in the
  parent design doc §"ASCOM Telescope Mapping". Roughly 20 endpoints.
- Alpaca discovery UDP listener on 32227 (response shape per Alpaca
  spec).
- Bounded JSON buffers (request ≤ 1 KB, response ≤ 2 KB — Alpaca
  payloads are tiny).
- Static-string error messages — no `format!` in handlers.

Bench test: `curl http://<ip>:11117/api/v1/telescope/0/connected -d
ClientID=1 -d ClientTransactionID=1` returns `{"Value": false, ...}`.

### Phase 5 — Authentication

**Goal:** Alpaca endpoints require a Bearer token.

- `firmware/rp-auth-embedded/` — port of `crates/rp-auth`'s scheme to
  `no_std`. Bearer-over-HTTP for now (TLS comes in Phase 6).
- Token stored in flash, compile-time placeholder for the first cut.
- Constant-time compare via `subtle`.
- picoserve middleware-style guard on every Alpaca route.
- Discovery endpoint stays unauthenticated (per Alpaca spec).

### Phase 6 — TLS

**Goal:** every Alpaca call goes over HTTPS.

- `esp-mbedtls` server. Concurrent-connection cap = 2 (SRAM budget).
- Cert + key embedded at compile time via `build.rs` (reads from a
  user-supplied `firmware/certs/` directory that's `.gitignore`d).
- Document the **private-CA recipe** in
  [`docs/references/embedded-firmware-provisioning.md`](../references/embedded-firmware-provisioning.md)
  (new file in this phase). Recipe: user generates a private CA with
  OpenSSL, issues a cert for the firmware, imports the CA cert into
  their client trust stores once.
- Verify with `curl --cacert ca.pem https://<ip>:11117/...` and then
  with NINA against a known-good cert.

ACME / Let's Encrypt is **explicitly rejected** for the embedded port:
it adds ~100 KB of code, requires a DNS-resolvable hostname for a
local-network device, and the renewal-flash-write cycle is operational
risk. Local-CA cert is the right model for this kind of device.

### Phase 7 — Persistent config + WiFi provisioning

**Goal:** zero hardcoded credentials. Fresh-out-of-box first boot does
sensible provisioning UX.

- WiFi SSID/PSK, site lat/lon/elevation, bearer token, TLS cert+key all
  read from a flash region (`esp_storage` partition or equivalent).
- First-boot UX: if WiFi creds absent, firmware brings up its own
  AP-mode network (`StarAdventurerGTi-XXXX`), serves a small HTTP page
  on `192.168.4.1` that accepts WiFi creds + initial config. Standard
  ESP32 pattern.
- OTA partition reserved but OTA flow itself deferred (manual flash via
  `espflash` is fine for now).
- Site lat/lon/elevation also settable via a small Alpaca-adjacent
  config endpoint (out of the spec but mirrors the desktop service's
  config file).

### Phase 8 — ConformU + client interop

**Goal:** the embedded firmware passes the same compliance bar as the
desktop service.

- Run `conformu conformance https://<ip>:11117/api/v1/telescope/0`.
- Expect identical exception list to desktop driver (see parent design
  doc): 0 errors, 7 known issues (4× DestinationSideOfPier deferred,
  1× SOPPierTest cascading, 2× TrackingRate-write upstream serde bug).
- Smoke test with NINA → connect → slew to Polaris → track → park →
  disconnect.
- Smoke test with `rp` mount tools end-to-end.
- Document install + provisioning UX in
  [`docs/services/star-adventurer-gti-embedded.md`](../services/star-adventurer-gti-embedded.md)
  (new sibling to the existing service doc, focused on the firmware
  deployment).

### Phase 9 — RP2350 RISC-V port  *(optional, parallel-trackable)*

**Goal:** validate that `firmware/alpaca-embedded/` +
`firmware/rp-auth-embedded/` + the `no_std` protocol crate are actually
portable.

- New binary crate `firmware/star-adventurer-gti-rp2350/` mirroring the
  ESP32-S3 one.
- `embassy-rp` HAL, `cyw43` WiFi driver, `embedded-tls` for TLS.
- USB host CDC via `embassy-usb-host` — check maturity at the time;
  this is the real unknown.
- If `embassy-usb-host` isn't there yet, this phase parks until it is.
  No urgency.

## Definition of done

- Firmware boots on ESP32-S3 from cold, joins WiFi, enumerates the mount
  over USB-CDC, runs the init handshake, starts the Alpaca server.
- NINA, SGPro, and `rp` can all connect over HTTPS with Bearer auth and
  drive a slew → sync → track → park → abort cycle.
- ConformU `conformance` phase reports the same 0 errors / 7 known
  issues as the desktop driver.
- Site lat/lon/elevation, WiFi creds, bearer token, and TLS cert are all
  provisionable from a fresh flash without re-building firmware.
- `docs/services/star-adventurer-gti-embedded.md` documents install +
  provisioning UX.
- `docs/references/embedded-firmware-provisioning.md` documents the
  private-CA cert recipe.

## Risks, ranked

1. **USB host CDC stability in embedded Rust.** Mitigated by Phase 0
   spike on a known-good stack (`esp-idf-svc` USB Host). Fallback:
   TinyUSB via C bindings. Hard blocker if both fail; revisit board.
2. **TLS connection memory budget.** `esp-mbedtls` wants ~30 KB per
   active TLS session for handshake + buffers. Cap concurrent
   connections at 2; PSRAM gives slack on ESP32-S3.
3. **Time-source quality.** LST computation needs ±1 s. SNTP-at-boot
   plus the ESP32-S3 RTC's free-running drift is plenty for one
   observing session; re-SNTP every few hours to be safe.
4. **ASCOM client TLS cipher compatibility.** `esp-mbedtls` covers
   everything modern ASCOM clients use; verify with the actual NINA
   build in Phase 6. `embedded-tls` (RP2350 path) is thinner — Phase 9
   may hit cipher-suite gaps.
5. **Bazel migration interaction.** Firmware crates won't have Bazel
   targets initially. The migration plan
   ([`docs/plans/bazel-migration.md`](bazel-migration.md)) treats
   non-Bazel crates as a known interim state, so this is fine — but
   document it as a follow-up.
6. **Cert provisioning UX.** Users have to import a CA cert into their
   trust stores once. Annoying but well-trodden; document carefully.

## Open questions

- **Where do the firmware crates live in Bazel-land?** Probably
  excluded from `//...` with a `requires-cargo` tag pattern; revisit
  after Phase 4.
- **OTA strategy.** Out of scope for the MVP. ESP32-S3 has standard
  two-partition OTA via `esp-storage`; add when there's a second device
  in the field.
- **Multi-mount on one MCU.** Out of scope. The desktop service assumes
  one mount per binary; the firmware inherits that assumption.
- **Do we want a hardware status LED / button surface?** The DevKitC
  has an RGB LED on GPIO 48. Probably worth using for "WiFi connected"
  / "mount connected" / "in motion" indications. Decide in Phase 7.

## References

- [`docs/services/star-adventurer-gti.md`](../services/star-adventurer-gti.md)
  — parent design doc, source of truth for ASCOM mapping + protocol behaviour.
- [`docs/references/skywatcher-motor-controller-command-set.md`](../references/skywatcher-motor-controller-command-set.md)
  — wire-protocol reference + empirical findings from Phase 4 hardware
  bring-up.
- [`docs/plans/bazel-migration.md`](bazel-migration.md) — interaction
  with the in-flight Bazel work.
- [Embassy book](https://embassy.dev/book/) — async embedded Rust framework.
- [esp-rs book](https://esp-rs.github.io/book/) — Espressif Rust toolchain.
- [picoserve docs](https://docs.rs/picoserve/) — no_std HTTP server.
- [esp-mbedtls](https://github.com/esp-rs/esp-mbedtls) — TLS for ESP32.
