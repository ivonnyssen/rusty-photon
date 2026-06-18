//! Build script for `libzwo-sys`.
//!
//! 1. Generates raw FFI bindings with `bindgen` from the vendored MIT headers in
//!    `sdk/include/` (parsed as C++ for the EFW/EAF `bool`).
//! 2. Emits the link directives for the system-installed ZWO SDK
//!    (`libASICamera2`, `libEFWFilter`) + `libusb-1.0` + the C++ runtime.
//!
//! Mirrors `libqhyccd-sys`'s system-installed-SDK model: the link is
//! unconditional, so building/linking (`cargo build`/`test`) requires the SDK on
//! the link path — even with the `simulation` feature. `cargo check`/`clippy`
//! (no link step) only need libclang for bindgen.
//!
//! Override the SDK search path with `ZWO_SDK_LIB_DIR=/path/to/lib`.

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
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Allow an explicit override of the SDK lib directory.
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
