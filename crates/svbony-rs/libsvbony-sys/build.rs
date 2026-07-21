//! Build script for `libsvbony-sys`.
//!
//! `lib.rs` is a hand-written `extern "C"` block (no bindgen — SVBony's SDK
//! header carries no license text anywhere, see the crate docs), so this
//! script's only job is emitting the native link directives for the
//! system-installed `libSVBCameraSDK` (+ `libusb-1.0`).
//!
//! Two env overrides, mirroring `libqhyccd-sys`/`libzwo-sys`:
//! - `SVBONY_SDK_LIB_DIR=/path/to/lib` — add an explicit SDK search path.
//! - `SVBONY_SKIP_NATIVE_LINK=1` — omit the native link directives entirely
//!   (and the `#[link(...)]` attribute in `lib.rs`, via the
//!   `svbony_skip_link` cfg it sets), for builds that exercise only the
//!   pure-Rust `simulation` path and provision no SDK.
//!
//! No Windows branch: indi-3rdparty's own `libsvbony` `CMakeLists.txt`
//! declares `message(FATAL_ERROR "MS Windows not supported.")`, and no
//! alternative Windows SDK distribution has been independently verified for
//! this crate (see `docs/plans/svbony-camera.md`). A Windows target build
//! fails loudly here rather than silently producing an unlinkable crate.

use std::env;

fn main() {
    for var in [
        "SVBONY_SKIP_NATIVE_LINK",
        "SVBONY_SDK_LIB_DIR",
        "CARGO_CFG_TARGET_OS",
        "CARGO_CFG_TARGET_ARCH",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    // Declare the cfg the skip branch may set, so `#[cfg_attr(not(svbony_skip_link),
    // ...)]` in lib.rs does not trip the `unexpected_cfgs` lint.
    println!("cargo:rustc-check-cfg=cfg(svbony_skip_link)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // indi-3rdparty's libsvbony CMakeLists.txt hard-fails on Windows
    // ("MS Windows not supported."), and SVBony's own direct SDK download has
    // not been checked for an alternative Windows distribution. Fail loudly
    // and early rather than emitting link directives for a platform nobody
    // has verified.
    if target_os == "windows" {
        panic!(
            "libsvbony-sys does not support Windows: indi-3rdparty's libsvbony \
             packaging declares Windows unsupported (\"MS Windows not supported\"), \
             and no alternative Windows SDK distribution has been verified for this \
             crate. See docs/plans/svbony-camera.md (\"Packaging\")."
        );
    }

    // Simulation-only escape hatch (mirrors QHYCCD_SKIP_NATIVE_LINK /
    // ZWO_SKIP_NATIVE_LINK): when set, emit NO link directives, so a
    // `--features simulation` build of svbony-rs — whose real FFI is
    // `#[cfg(not(feature = "simulation"))]` — links with no SVBony SDK
    // installed. Used by SDK-less dev builds, the sim-only CI jobs
    // (test/conformu/safety), and — until an `install-svbony-sdk` CI
    // provisioning action exists (see docs/plans/svbony-camera.md Phase C) —
    // the default local Bazel build too (crates/svbony-rs/libsvbony-sys/BUILD.bazel
    // bakes this env var into its `cargo_build_script`, unlike
    // libqhyccd-sys/libzwo-sys's Bazel targets, which link the real,
    // pre-provisioned system SDK).
    if env::var_os("SVBONY_SKIP_NATIVE_LINK").is_some() {
        println!("cargo:rustc-cfg=svbony_skip_link");
        println!(
            "cargo:warning=SVBONY_SKIP_NATIVE_LINK set — omitting SVBony SDK link \
             directives; this is a simulation-only build that links no native SDK"
        );
        return;
    }

    if let Some(dir) = env::var("SVBONY_SDK_LIB_DIR")
        .ok()
        .filter(|d| !d.is_empty())
    {
        println!("cargo:rustc-link-search=native={dir}");
    } else {
        println!("cargo:rustc-link-search=native=/usr/local/lib");
    }

    match target_os.as_str() {
        "macos" => {
            // Best-effort only: indi-3rdparty's libsvbony packaging carries
            // just a `mac64` (Intel) blob. No `mac_arm64` blob has been
            // confirmed via that source as of SDK 1.13.4 (SVBony's own direct
            // SDK download has not yet been byte-verified — see
            // docs/plans/svbony-camera.md "Packaging"), so this link may fail
            // on Apple Silicon until that is checked/staged.
            let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
            if arch == "aarch64" {
                println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
                println!(
                    "cargo:warning=No confirmed arm64 macOS libSVBCameraSDK blob as of SDK \
                     1.13.4 packaging (indi-3rdparty ships mac64/Intel only); this link may \
                     fail until an Apple Silicon SDK build is independently verified"
                );
            } else {
                println!("cargo:rustc-link-search=native=/usr/local/lib");
            }
            println!("cargo:rustc-link-lib=dylib=SVBCameraSDK");
            println!("cargo:rustc-link-lib=dylib=usb-1.0");
        }
        _ => {
            // Linux (amd64/x86/armv6/armv7/armv8-aarch64, per indi-3rdparty's
            // libsvbony blobs). The library installs with a proper SONAME
            // (`libSVBCameraSDK.so.1`, unlike ZWO's SONAME-less blobs), so a
            // plain `-lSVBCameraSDK` resolves via normal `ldconfig` — no
            // RUNPATH trick needed (verify at packaging time, Phase G).
            println!("cargo:rustc-link-lib=dylib=SVBCameraSDK");
            println!("cargo:rustc-link-lib=dylib=usb-1.0");
        }
    }
}
