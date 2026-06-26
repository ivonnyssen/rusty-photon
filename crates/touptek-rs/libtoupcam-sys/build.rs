//! Build script for `libtoupcam-sys`.
//!
//! 1. Generates raw FFI bindings with `bindgen` from the vendored header in
//!    `sdk/include/toupcam.h` (parsed as plain C — the header is C-compatible).
//! 2. Emits the link directives for the system-installed ToupTek SDK
//!    (`libtoupcam` / `toupcam.dll`) plus its transitive system deps.
//!
//! Mirrors `libzwo-sys`'s system-installed-SDK model: by default the link is
//! emitted, so building/linking (`cargo build`/`test`) requires the SDK on the
//! link path — even with the `simulation` feature on the safe wrapper.
//! `cargo check`/`clippy` (no link step) only need libclang for bindgen.
//!
//! Two env overrides:
//! - `TOUPCAM_SDK_LIB_DIR=/path/to/lib` — add an SDK search path.
//! - `TOUPCAM_SKIP_NATIVE_LINK=1` — omit the native link directives entirely,
//!   for builds that exercise only the pure-Rust `simulation` path and provision
//!   no SDK (sanitizer CI, the default sim-variant Bazel build). See
//!   [`emit_link_directives`].

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
        .clang_arg(format!("-I{}", include_dir.display()))
        // Surface the S_OK / E_* HRESULT error-code macros in the translation
        // unit (referenced by the safe wrapper's error mapping). Harmless when
        // unused: they are `(HRESULT)(...)` cast macros, so bindgen does not emit
        // them as constants — they document intent for error.rs.
        .clang_arg("-DTOUPCAM_HRESULT_ERRORCODE_NEEDED")
        // Only the ToupTek SDK surface — keep stdlib/system symbols out. The API
        // is `Toupcam_*` functions; types/consts are `Toupcam*` / `*TOUPCAM*`
        // (incl. the `PTOUPCAM_*` callback typedefs and `TOUPCAM_*` flag/option
        // constants).
        .allowlist_function("Toupcam.*")
        .allowlist_type(".*(Toupcam|TOUPCAM).*")
        .allowlist_var(".*TOUPCAM.*")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed to generate ToupTek SDK bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("failed to write bindings.rs");

    // --- 2. link directives ----------------------------------------------
    emit_link_directives();
}

fn emit_link_directives() {
    // Allow the native link to be suppressed entirely. A `simulation` build
    // references no `Toupcam_*` symbols — the real FFI calls are
    // `#[cfg(not(feature = "simulation"))]` in the safe wrapper, and bare
    // `extern` declarations do not force the linker to resolve a library — so
    // with these directives omitted the crate links with no native SDK present.
    // Env-gated, *not* feature-gated: a Cargo feature would be turned on by
    // `--all-features` everywhere and silently stop every such build from
    // linking the real SDK.
    println!("cargo:rerun-if-env-changed=TOUPCAM_SKIP_NATIVE_LINK");
    if env::var_os("TOUPCAM_SKIP_NATIVE_LINK").is_some() {
        println!(
            "cargo:warning=TOUPCAM_SKIP_NATIVE_LINK set — omitting ToupTek SDK link directives; \
             this is a simulation-only build that links no native SDK"
        );
        return;
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Allow an explicit override of the SDK lib directory.
    println!("cargo:rerun-if-env-changed=TOUPCAM_SDK_LIB_DIR");
    if let Ok(dir) = env::var("TOUPCAM_SDK_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
    }

    match target_os.as_str() {
        "macos" => {
            println!("cargo:rustc-link-search=native=/usr/local/lib");
            // Homebrew (Apple Silicon vs Intel).
            let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
            if arch == "aarch64" {
                println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
            }
            println!("cargo:rustc-link-lib=dylib=toupcam");
            // libtoupcam talks USB via IOKit/CoreFoundation on macOS.
            println!("cargo:rustc-link-lib=framework=IOKit");
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
        }
        "windows" => {
            // ToupTek ships per-arch import libs (toupcam.lib); assume the SDK
            // lib dir is on the search path (or set via TOUPCAM_SDK_LIB_DIR).
            println!("cargo:rustc-link-lib=dylib=toupcam");
        }
        _ => {
            // Linux and other Unix-like systems. INDI installs the SDK under
            // /opt/toupcamsdk and /usr/local/lib; the udev rule + libusb/libudev
            // cover USB enumeration.
            println!("cargo:rustc-link-search=native=/usr/local/lib");
            println!("cargo:rustc-link-search=native=/opt/toupcamsdk/linux/x64");
            println!("cargo:rustc-link-lib=dylib=toupcam");
            println!("cargo:rustc-link-lib=dylib=usb-1.0");
            println!("cargo:rustc-link-lib=dylib=udev");
        }
    }
}
