//! Raw FFI bindings for the ZWO ASI camera, EFW filter wheel, and EAF focuser
//! SDK.
//!
//! The bindings are generated at build time by [`bindgen`] from the vendored MIT
//! headers in `sdk/include/` (`ASICamera2.h`, `EFW_filter.h`, `EAF_focuser.h`),
//! parsed as C++ so the EFW/EAF headers' bare `bool` resolves to the builtin
//! type. See `build.rs`.
//!
//! This is a `*-sys` crate: it exposes only the raw, unsafe bindings plus the
//! link directives. Use the safe [`zwo-rs`](https://crates.io/crates/zwo-rs)
//! wrapper instead.
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
