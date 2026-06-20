use std::{env, path::PathBuf};

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    match target_os.as_str() {
        "macos" => {
            // Check for SDK in workspace first (CI environment)
            if let Ok(workspace) = env::var("GITHUB_WORKSPACE") {
                let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
                let sdk_path = if arch == "aarch64" {
                    format!("{}/sdk_mac_arm_25.09.29/usr/local/lib", workspace)
                } else {
                    format!("{}/sdk_macMix_25.09.29/usr/local/lib", workspace)
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
            println!("cargo:rerun-if-env-changed=QHYCCD_SDK_DIR");
            let mut found = false;
            if let Ok(dir) = env::var("QHYCCD_SDK_DIR") {
                println!("cargo:rustc-link-search=native={}", dir);
                found = true;
            }
            // CI: the qhyccd-sdk-install action extracts the SDK to pkg_win/ at the
            // workspace root.
            if let Ok(workspace) = env::var("GITHUB_WORKSPACE") {
                let ws_sdk = PathBuf::from(&workspace).join("pkg_win");
                println!("cargo:rustc-link-search=native={}", ws_sdk.display());
                println!(
                    "cargo:rustc-link-search=native={}",
                    ws_sdk.join(arch_dir).display()
                );
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
