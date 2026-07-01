//! Raw FFI bindings for the ToupTek ToupCam camera SDK.
//!
//! The bindings are generated at build time by [`bindgen`] from the vendored
//! header in `sdk/include/toupcam.h`, parsed as plain C (the header is
//! C-compatible — see `build.rs` / `wrapper.h`).
//!
//! This is a `*-sys` crate: it exposes only the raw, unsafe bindings plus the
//! link directives. Use the safe `touptek-rs` wrapper instead.
//!
//! The same flat C ABI is shared by every OEM rebrand of this SDK (Altair,
//! Omegon, Meade, Bresser, Mallincam, RisingCam/Ogma, SVBony, StarShootG, Nncam,
//! Tscam) with only the `Toupcam_` symbol prefix swapped.
//!
//! [`bindgen`]: https://crates.io/crates/bindgen
#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code
)]
// Generated bindings are not idiomatic Rust; do not lint them.
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
