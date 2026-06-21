use std::{env, path::PathBuf};

fn main() {
    // Re-run when any env var that influences the emitted link-search paths
    // changes. Cargo only tracks env vars that are explicitly declared here;
    // emitting any `rerun-if-*` also disables the default "re-run on any package
    // file change" (fine — this script reads no files, only env). Declared at the
    // top so they apply on every target, not just the arm that reads them.
    for var in [
        "QHYCCD_SKIP_NATIVE_LINK",
        "QHYCCD_SDK_DIR",
        "GITHUB_WORKSPACE",
        "CARGO_CFG_TARGET_OS",
        "CARGO_CFG_TARGET_ARCH",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    // Declare the cfg the skip branch may set, so `#[cfg_attr(qhyccd_skip_link,
    // ...)]` in lib.rs does not trip the `unexpected_cfgs` lint.
    println!("cargo:rustc-check-cfg=cfg(qhyccd_skip_link)");

    // Simulation-only escape hatch (mirrors zwo-rs's ZWO_SKIP_NATIVE_LINK): when
    // set, emit NO link directives, so a `--features simulation` build of
    // qhyccd-rs — whose real FFI is `cfg`'d out (see the `not(feature =
    // "simulation")` gates in src/) — links with no QHYCCD SDK installed. Used by
    // SDK-less dev builds and the sim-only CI jobs (test/conformu/safety). A real
    // (non-simulation) build leaves it unset and links `static=qhyccd`.
    if env::var_os("QHYCCD_SKIP_NATIVE_LINK").is_some() {
        // Also drop lib.rs's `#[link(name = "qhyccd", kind = "static")]`: that
        // compile-time attribute forces the link independently of these build-script
        // directives, so the cfg must gate it off too.
        println!("cargo:rustc-cfg=qhyccd_skip_link");
        println!(
            "cargo:warning=QHYCCD_SKIP_NATIVE_LINK set — omitting QHYCCD SDK link \
             directives; this is a simulation-only build that links no native SDK"
        );
        return;
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    match target_os.as_str() {
        "macos" => {
            // Check for SDK in workspace first (CI environment)
            if let Ok(workspace) = env::var("GITHUB_WORKSPACE") {
                let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
                // The qhyccd-sdk-install action extracts the macOS SDK under the
                // workspace root using the upstream archive's top dir. The 26.x
                // packaging renamed the Intel variant `macMix` -> `mac_x64` (Apple
                // Silicon stays `mac_arm`). Keep these in lockstep with the pinned
                // `version:` in the CI workflows.
                let sdk_path = if arch == "aarch64" {
                    format!("{}/sdk_mac_arm_26.06.04/usr/local/lib", workspace)
                } else {
                    format!("{}/sdk_mac_x64_26.06.04/usr/local/lib", workspace)
                };
                println!("cargo:rustc-link-search=native={}", sdk_path);
            } else {
                // Fallback to system installation
                println!("cargo:rustc-link-search=native=/usr/local/lib");
            }
            // Add Homebrew library paths for libusb
            let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
            if arch == "aarch64" {
                // Apple Silicon Homebrew path
                println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
            } else {
                // Intel Mac Homebrew path
                println!("cargo:rustc-link-search=native=/usr/local/lib");
            }
            println!("cargo:rustc-link-lib=static=qhyccd");
            // macOS uses libc++ instead of libstdc++
            println!("cargo:rustc-link-lib=dylib=c++");
            // Link libusb (required by QHYCCD SDK)
            println!("cargo:rustc-link-lib=dylib=usb-1.0");
        }
        "windows" => {
            let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
            let arch_dir = match arch.as_str() {
                "x86_64" => "x64",
                "x86" | "i686" => "x86",
                other => {
                    println!(
                        "cargo:warning=Unknown Windows arch '{}', defaulting to x64",
                        other
                    );
                    "x64"
                }
            };

            // Explicit override for local/system installs: point QHYCCD_SDK_DIR at
            // the directory containing qhyccd.lib (e.g. ...\pkg_win\x64). Checked
            // first so a developer can build off-CI against an installed SDK.
            let mut found = false;
            if let Ok(dir) = env::var("QHYCCD_SDK_DIR") {
                println!("cargo:rustc-link-search=native={}", dir);
                found = true;
            }
            // CI: the qhyccd-sdk-install action extracts the SDK under the
            // workspace root — `pkg_win/` for the legacy (<= 25.x) packaging and
            // `sdk_win64_<version>/` for the new (>= 26.06.04) packaging. Emit both
            // roots' arch subdirs; a non-existent search path is harmless to the
            // linker, which uses whichever actually holds `qhyccd.lib`.
            if let Ok(workspace) = env::var("GITHUB_WORKSPACE") {
                for root in ["pkg_win", "sdk_win64_26.06.04"] {
                    let ws_sdk = PathBuf::from(&workspace).join(root);
                    println!("cargo:rustc-link-search=native={}", ws_sdk.display());
                    println!(
                        "cargo:rustc-link-search=native={}",
                        ws_sdk.join(arch_dir).display()
                    );
                }
                found = true;
            }
            // Optional in-tree vendored SDK — only add the path when it actually
            // exists. It is not committed to this repo, so emitting it
            // unconditionally was just noise and masked the "SDK not found" case.
            let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
            let sdk_dir = manifest_dir
                .join("qhyccd-sdk")
                .join("pkg_win")
                .join(arch_dir);
            if sdk_dir.is_dir() {
                println!("cargo:rustc-link-search=native={}", sdk_dir.display());
                found = true;
            }
            if !found {
                println!(
                    "cargo:warning=QHYCCD SDK not found for Windows: set QHYCCD_SDK_DIR to the \
                     directory containing qhyccd.lib (or set GITHUB_WORKSPACE on CI). Linking \
                     will fail until a search path is provided."
                );
            }
            println!("cargo:rustc-link-lib=static=qhyccd");
            // Windows SDK likely includes all dependencies
        }
        _ => {
            // Linux and other Unix-like systems
            println!("cargo:rustc-link-search=native=/usr/local/lib");
            println!("cargo:rustc-link-lib=static=qhyccd");
            println!("cargo:rustc-link-lib=dylib=usb-1.0");
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
    }
}
