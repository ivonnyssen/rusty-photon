//! Build script for `libzwo-sys`.
//!
//! 1. Generates raw FFI bindings with `bindgen` from the vendored MIT headers in
//!    `sdk/include/` (parsed as C++ for the EFW/EAF `bool`).
//! 2. Emits the link directives for the system-installed ZWO SDK
//!    (`libASICamera2`, `libEFWFilter`) + `libusb-1.0` + the C++ runtime.
//!
//! Mirrors `libqhyccd-sys`'s system-installed-SDK model: by default the link is
//! emitted, so building/linking (`cargo build`/`test`) requires the SDK on the
//! link path — even with the `simulation` feature. `cargo check`/`clippy` (no
//! link step) only need libclang for bindgen.
//!
//! Two env overrides:
//! - `ZWO_SDK_LIB_DIR=/path/to/lib` — add an SDK search path.
//! - `ZWO_SKIP_NATIVE_LINK=1` — omit the native link directives entirely, for
//!   builds that exercise only the pure-Rust `simulation` path and provision no
//!   SDK (sanitizer CI). See [`emit_link_directives`].

use std::{env, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let include_dir = manifest_dir.join("sdk").join("include");
    let wrapper = manifest_dir.join("wrapper.h");

    // --- 1. bindgen -------------------------------------------------------
    println!("cargo:rerun-if-changed={}", wrapper.display());
    println!("cargo:rerun-if-changed={}", include_dir.display());

    let bindings = bindgen::Builder::default()
        .header(wrapper.to_string_lossy())
        // The EFW/EAF headers use bare `bool` without <stdbool.h>; parse as C++
        // so it resolves to the builtin type (ASICamera2.h parses fine either
        // way). Verified to produce clean, compiling bindings.
        .clang_args(["-x", "c++", "-std=c++14"])
        .clang_arg(format!("-I{}", include_dir.display()))
        // Only the ZWO SDK surface — keep stdlib/system symbols out.
        .allowlist_function("(ASI|EFW|EAF).*")
        .allowlist_type("_?(ASI|EFW|EAF).*")
        .allowlist_var("(ASI|EFW|EAF).*")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed to generate ZWO SDK bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("failed to write bindings.rs");

    // --- 2. link directives ----------------------------------------------
    emit_link_directives();
}

fn emit_link_directives() {
    // Allow the native link to be suppressed entirely. A `simulation` build
    // references no ASI/EFW symbols — the real FFI calls are
    // `#[cfg(not(feature = "simulation"))]`, and bare `extern` declarations do
    // not force the linker to resolve a library — so with these directives
    // omitted the crate links with no native SDK present. Used by the sanitizer
    // CI job, which exercises only the pure-Rust simulation path and provisions
    // no SDK. Env-gated, *not* feature-gated: a Cargo feature would be turned on
    // by `--all-features` everywhere and silently stop every such build from
    // linking the real SDK.
    println!("cargo:rerun-if-env-changed=ZWO_SKIP_NATIVE_LINK");
    if env::var_os("ZWO_SKIP_NATIVE_LINK").is_some() {
        println!(
            "cargo:warning=ZWO_SKIP_NATIVE_LINK set — omitting ZWO SDK link directives; \
             this is a simulation-only build that links no native SDK"
        );
        return;
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Allow an explicit override of the SDK lib directory.
    println!("cargo:rerun-if-env-changed=ZWO_SDK_LIB_DIR");
    if let Ok(dir) = env::var("ZWO_SDK_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
    }

    match target_os.as_str() {
        "macos" => {
            println!("cargo:rustc-link-search=native=/usr/local/lib");
            // Homebrew libusb (Apple Silicon vs Intel).
            let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
            if arch == "aarch64" {
                println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
            }
            println!("cargo:rustc-link-lib=dylib=ASICamera2");
            println!("cargo:rustc-link-lib=dylib=EFWFilter");
            // libASICamera2 is C++; pull in libc++ and libusb.
            println!("cargo:rustc-link-lib=dylib=c++");
            println!("cargo:rustc-link-lib=dylib=usb-1.0");
            // libEFWFilter (USB-HID) uses IOKit/CoreFoundation on macOS.
            println!("cargo:rustc-link-lib=framework=IOKit");
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
        }
        "windows" => {
            // ZWO ships per-arch import libs; assume the SDK lib dir is on the
            // search path (or set via ZWO_SDK_LIB_DIR).
            println!("cargo:rustc-link-lib=dylib=ASICamera2");
            println!("cargo:rustc-link-lib=dylib=EFWFilter");
        }
        _ => {
            // Linux and other Unix-like systems.
            println!("cargo:rustc-link-search=native=/usr/local/lib");
            println!("cargo:rustc-link-lib=dylib=ASICamera2");
            println!("cargo:rustc-link-lib=dylib=EFWFilter");
            // libASICamera2 is C++; pull in libstdc++ and libusb.
            println!("cargo:rustc-link-lib=dylib=stdc++");
            println!("cargo:rustc-link-lib=dylib=usb-1.0");
            // libEFWFilter (USB-HID) depends on libudev on Linux.
            println!("cargo:rustc-link-lib=dylib=udev");
        }
    }

    // EAF focuser (libEAFFocuser): bindings are generated above, but the library
    // is only linked when the focuser is implemented (Camera → EFW → EAF). The
    // unreferenced extern declarations do not force the linker to resolve it.
}
