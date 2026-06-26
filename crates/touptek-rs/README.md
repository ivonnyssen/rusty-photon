# touptek-rs

Safe Rust bindings for the **ToupTek ToupCam** camera SDK, wrapping the raw
`bindgen` FFI in the nested [`libtoupcam-sys`](libtoupcam-sys) crate.

Sibling to [`zwo-rs`](https://crates.io/crates/zwo-rs) and
[`qhyccd-rs`](https://crates.io/crates/qhyccd-rs); consumed by rusty-photon's
`touptek-camera` ASCOM Alpaca driver. The same ABI also covers the ToupTek OEM
rebrands (Altair, Omegon, Meade, Bresser, Mallincam, RisingCam/Ogma, SVBony,
StarShootG, Nncam, Tscam) with only the `Toupcam_` symbol prefix swapped.

> **Status: Phase B skeleton.** Enumeration, the `Camera` handle, and the
> callback → blocking pull/trigger bridge are wired. See
> [`docs/plans/touptek-driver.md`](../../docs/plans/touptek-driver.md) for the
> roadmap.

## Features

- `simulation` — a hardware-free, in-Rust simulated camera for development and
  tests (fabricates frames). As in zwo-rs/qhyccd-rs this removes the *camera*,
  not the SDK *link*.

## Build requirements

- **libclang** — `libtoupcam-sys` runs `bindgen` at build time.
- **The ToupTek SDK** (`libtoupcam`) on the link path to *link* a binary/test —
  unless `TOUPCAM_SKIP_NATIVE_LINK=1` is set (links nothing; for the simulation
  path / sanitizer builds).

## Example

```rust,no_run
use touptek_rs::{Sdk, Event};
use std::time::Duration;

let sdk = Sdk::new()?;
let mut camera = sdk.open(0)?;
camera.set_exposure_time_us(100_000)?; // 100 ms
camera.set_gain_percent(100)?;         // 1.0x
camera.enable_trigger_mode()?;
camera.start_pull_mode()?;

camera.trigger_single()?;
if let Event::StillImage = camera.wait_for_event(Duration::from_secs(5))? {
    let frame = camera.pull_image(1920, 1080, 16)?;
    println!("got {}x{} ({} bytes)", frame.width, frame.height, frame.data.len());
}
camera.stop()?;
# Ok::<(), touptek_rs::Error>(())
```

## License

`MIT OR Apache-2.0`.
