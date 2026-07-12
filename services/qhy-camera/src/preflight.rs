//! Windows `qhyccd.dll` startup preflight.
//!
//! On Windows the QHYCCD SDK's `qhyccd.lib` is an **import library** for the
//! proprietary `qhyccd.dll`, which ADR-013 forbids redistributing — the
//! operator installs QHY's "All-in-One" pack (needed for the signed device
//! driver anyway), which provides the DLL. The binary is linked with
//! `/DELAYLOAD:qhyccd.dll` (see `build.rs`), so a missing DLL no longer kills
//! the process in the Windows loader before `main`; this module resolves the
//! DLL **before any SDK call** and keeps it resident so the delay-load helper
//! binds to the already-loaded module. Behavioral contracts PF1–PF5 in
//! `docs/services/qhy-camera.md` § "Windows: qhyccd.dll resolution".
//!
//! The candidate-ordering and selection logic is pure (environment and
//! fs-existence are injected) so it is unit-testable on every platform; only
//! the actual `LoadLibrary` calls are `#[cfg(windows)]`.

use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// QHY's download center, where the "All-in-One" pack (signed device driver +
/// `qhyccd.dll`) lives. Named in the preflight failure message and by the
/// `doctor` subcommand.
pub const QHY_ALL_IN_ONE_URL: &str = "https://www.qhyccd.com/download/";

/// Base name of the delay-loaded QHYCCD SDK DLL.
pub const QHY_DLL_NAME: &str = "qhyccd.dll";

/// The QHYCCD SDK version this binary was **built against** (the pinned
/// import library). Keep in lockstep with the SDK pin in
/// `crates/qhyccd-rs/libqhyccd-sys/build.rs` (the `sdk_win64_26.06.04`
/// search-path names) and the CI workflows; the Windows packaging plan's
/// `check-pkg-assets.sh` assertions (W4) will assert that parity. The
/// `doctor` subcommand compares this against the version the *loaded* DLL
/// reports, making All-in-One ABI skew visible (ADR-015 accepted risk).
pub const PINNED_SDK_VERSION: PinnedSdkVersion = PinnedSdkVersion {
    year: 26,
    month: 6,
    day: 4,
};

/// Build-time pinned SDK version, in QHYCCD's `YY.MM.DD` scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedSdkVersion {
    pub year: u32,
    pub month: u32,
    pub day: u32,
}

impl fmt::Display for PinnedSdkVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}.{:02}.{:02}", self.year, self.month, self.day)
    }
}

/// Best-effort roots where QHY's All-in-One pack installs. Their *existence*
/// signals driver-pack presence (reported by `doctor`), distinct from the DLL
/// itself; the DLL candidate list is derived from them in [`candidate_dirs`].
///
/// The exact All-in-One layout is a flagged unknown of
/// `docs/plans/windows-packaging.md` — confirm/extend on a real Windows box.
pub fn driver_pack_dirs(env_var: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|var| env_var(var))
        .map(|root| Path::new(&root).join("QHYCCD"))
        .collect()
}

/// Ordered candidate directories that may hold `qhyccd.dll`:
///
/// 1. the exe's own directory (an operator can always drop the DLL next to
///    the binary),
/// 2. a **best-effort seed** of known All-in-One install locations under each
///    `QHYCCD` root from [`driver_pack_dirs`] — extend by appending entries
///    once confirmed on a real Windows install (flagged unknown of the plan).
///
/// Pure: the environment is injected so ordering is testable cross-platform.
pub fn candidate_dirs(
    exe_dir: Option<&Path>,
    env_var: impl Fn(&str) -> Option<String>,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = exe_dir {
        dirs.push(dir.to_path_buf());
    }
    for root in driver_pack_dirs(env_var) {
        dirs.push(root.join("AllInOne").join("sdk").join("x64"));
        dirs.push(root.join("AllInOne").join("sdk"));
    }
    dirs
}

/// First candidate directory whose `qhyccd.dll` exists, as the full DLL path.
/// Pure: the fs-existence check is injected.
pub fn select_dll(candidates: &[PathBuf], exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates
        .iter()
        .map(|dir| dir.join(QHY_DLL_NAME))
        .find(|dll| exists(dll))
}

/// Outcome of resolving `qhyccd.dll`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DllResolution {
    /// Loaded from an explicit probed candidate; the module handle was leaked
    /// so it stays resident for the life of the process (PF2).
    FoundAt(PathBuf),
    /// Not in any probed candidate, but the default Windows loader search
    /// order (exe dir, System32, `PATH`, …) found it by name (PF3).
    FoundByName,
    /// Nowhere: neither the probed candidates nor the default search order.
    NotFound {
        /// The candidate directories that were probed, for the error report.
        probed: Vec<PathBuf>,
    },
}

/// Preflight failure: `qhyccd.dll` could not be resolved anywhere (PF4).
///
/// The `Display` text is THE one distinctive, actionable operator message —
/// `scripts/verify-msi.ps1` (plan W4) greps the service log for it, so keep
/// its leading phrase stable.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PreflightError {
    #[error(
        "qhyccd.dll not found — qhy-camera cannot drive QHY hardware without the QHYCCD SDK \
         DLL. Install QHY's 'All-in-One' pack (it also carries the required signed device \
         driver) from {QHY_ALL_IN_ONE_URL} and restart this service; it retries on every \
         start until the DLL appears. Probed: {}; plus the standard Windows DLL search \
         order (exe directory, System32, PATH)",
        display_probed(.probed)
    )]
    DllNotFound { probed: Vec<PathBuf> },
}

fn display_probed(probed: &[PathBuf]) -> String {
    if probed.is_empty() {
        return "(no candidate directories)".to_string();
    }
    probed
        .iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Probe the candidates, load the first hit, and keep it resident (PF1–PF3).
///
/// The winning module is deliberately **leaked** (`std::mem::forget`): the
/// delay-load helper's later `LoadLibrary("qhyccd.dll")` then binds to the
/// already-loaded module by base name instead of re-searching, and the SDK
/// must stay loaded for the life of the process anyway.
#[cfg(windows)]
pub fn resolve_and_load() -> DllResolution {
    use libloading::os::windows::{Library, LOAD_WITH_ALTERED_SEARCH_PATH};
    use tracing::debug;

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let candidates = candidate_dirs(exe_dir.as_deref(), |var| std::env::var(var).ok());

    if let Some(dll) = select_dll(&candidates, |p| p.is_file()) {
        // LOAD_WITH_ALTERED_SEARCH_PATH: dependencies of qhyccd.dll resolve
        // from ITS directory first, matching how the All-in-One lays out any
        // companion DLLs.
        //
        // SAFETY: loading a DLL runs its DllMain / initializers. qhyccd.dll is
        // the exact SDK this binary already links (delay-loaded) — the first
        // SDK call would run the same code; loading it eagerly is not a new
        // hazard.
        match unsafe { Library::load_with_flags(&dll, LOAD_WITH_ALTERED_SEARCH_PATH) } {
            Ok(lib) => {
                std::mem::forget(lib); // keep resident: delay-load binds to this module
                return DllResolution::FoundAt(dll);
            }
            Err(error) => {
                debug!(
                    "candidate {} exists but failed to load: {error}",
                    dll.display()
                );
            }
        }
    }

    // Fallback: the default loader search order (exe dir, System32, PATH)
    // catches All-in-One installs that put the DLL on PATH.
    //
    // SAFETY: same as above — this is the SDK DLL the binary links against.
    match unsafe { Library::new(QHY_DLL_NAME) } {
        Ok(lib) => {
            std::mem::forget(lib); // keep resident: delay-load binds to this module
            DllResolution::FoundByName
        }
        Err(error) => {
            debug!("default-search-order load of {QHY_DLL_NAME} failed: {error}");
            DllResolution::NotFound { probed: candidates }
        }
    }
}

/// Startup preflight (PF1–PF4): resolve `qhyccd.dll` before any SDK call, or
/// fail with the ONE distinctive `error!` + `Err` for a clean non-zero exit
/// (the SCM/systemd restart loop then applies — same contract as a missing
/// serial device).
#[cfg(windows)]
pub fn ensure_qhyccd_dll() -> Result<DllResolution, PreflightError> {
    use tracing::{debug, error};

    let resolution = resolve_and_load();
    match &resolution {
        DllResolution::FoundAt(dll) => debug!("qhyccd.dll resolved: {}", dll.display()),
        DllResolution::FoundByName => {
            debug!("qhyccd.dll resolved via the default Windows DLL search order");
        }
        DllResolution::NotFound { probed } => {
            let failure = PreflightError::DllNotFound {
                probed: probed.clone(),
            };
            // The one distinctive, actionable error (PF4) — deliberately
            // error!, not debug!: this is the line an operator (and plan-W4's
            // verify-msi.ps1) looks for.
            error!("{failure}");
            return Err(failure);
        }
    }
    Ok(resolution)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn fake_env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name: &str| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn candidate_dirs_orders_exe_dir_first_then_program_files_seeds() {
        let env = fake_env(&[
            ("ProgramFiles", r"C:\Program Files"),
            ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
        ]);
        let exe_dir = PathBuf::from(r"C:\Program Files\rusty-photon");
        let dirs = candidate_dirs(Some(&exe_dir), env);

        let expected: Vec<PathBuf> = vec![
            PathBuf::from(r"C:\Program Files\rusty-photon"),
            PathBuf::from(r"C:\Program Files")
                .join("QHYCCD")
                .join("AllInOne")
                .join("sdk")
                .join("x64"),
            PathBuf::from(r"C:\Program Files")
                .join("QHYCCD")
                .join("AllInOne")
                .join("sdk"),
            PathBuf::from(r"C:\Program Files (x86)")
                .join("QHYCCD")
                .join("AllInOne")
                .join("sdk")
                .join("x64"),
            PathBuf::from(r"C:\Program Files (x86)")
                .join("QHYCCD")
                .join("AllInOne")
                .join("sdk"),
        ];
        assert_eq!(dirs, expected);
    }

    #[test]
    fn candidate_dirs_skips_missing_env_roots() {
        let env = fake_env(&[("ProgramFiles", r"C:\Program Files")]);
        let dirs = candidate_dirs(None, env);
        // No exe dir, one root: exactly the two seeds under that root.
        assert_eq!(dirs.len(), 2);
        assert!(dirs.iter().all(|d| d.starts_with(r"C:\Program Files")));
    }

    #[test]
    fn candidate_dirs_empty_when_nothing_known() {
        let dirs = candidate_dirs(None, |_| None);
        assert!(dirs.is_empty());
    }

    #[test]
    fn driver_pack_dirs_are_the_qhyccd_roots() {
        let env = fake_env(&[
            ("ProgramFiles", r"C:\Program Files"),
            ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
        ]);
        let dirs = driver_pack_dirs(env);
        assert_eq!(
            dirs,
            vec![
                PathBuf::from(r"C:\Program Files").join("QHYCCD"),
                PathBuf::from(r"C:\Program Files (x86)").join("QHYCCD"),
            ]
        );
    }

    #[test]
    fn select_dll_returns_first_existing_candidate_as_full_dll_path() {
        let candidates = vec![
            PathBuf::from("first"),
            PathBuf::from("second"),
            PathBuf::from("third"),
        ];
        let hit = PathBuf::from("second").join(QHY_DLL_NAME);
        let selected = select_dll(&candidates, |p| p == hit).unwrap();
        assert_eq!(selected, hit);
    }

    #[test]
    fn select_dll_prefers_earlier_candidates() {
        let candidates = vec![PathBuf::from("first"), PathBuf::from("second")];
        // Both exist: the first must win.
        let selected = select_dll(&candidates, |_| true).unwrap();
        assert_eq!(selected, PathBuf::from("first").join(QHY_DLL_NAME));
    }

    #[test]
    fn select_dll_none_when_no_candidate_exists() {
        let candidates = vec![PathBuf::from("first")];
        assert_eq!(select_dll(&candidates, |_| false), None);
    }

    #[test]
    fn pinned_sdk_version_displays_in_qhy_scheme() {
        assert_eq!(PINNED_SDK_VERSION.to_string(), "26.06.04");
    }

    #[test]
    fn preflight_error_names_the_url_and_the_probed_dirs() {
        let failure = PreflightError::DllNotFound {
            probed: vec![
                PathBuf::from(r"C:\Program Files\rusty-photon"),
                PathBuf::from(r"C:\Program Files\QHYCCD\AllInOne\sdk\x64"),
            ],
        };
        let message = failure.to_string();
        assert!(message.starts_with("qhyccd.dll not found"), "{message}");
        assert!(message.contains(QHY_ALL_IN_ONE_URL), "{message}");
        assert!(
            message.contains(r"C:\Program Files\rusty-photon"),
            "{message}"
        );
        assert!(
            message.contains(r"C:\Program Files\QHYCCD\AllInOne\sdk\x64"),
            "{message}"
        );
    }

    #[test]
    fn preflight_error_with_no_candidates_still_renders() {
        let failure = PreflightError::DllNotFound { probed: vec![] };
        let message = failure.to_string();
        assert!(message.contains("(no candidate directories)"), "{message}");
    }

    /// Real `LoadLibrary` path — Windows only (runs on the Windows CI legs).
    /// The SDK DLL may or may not be present (it is on `PATH` when the
    /// qhyccd-sdk-install action provisioned it; absent on SDK-less jobs), so
    /// this asserts the resolution is *well-formed*, not a specific variant.
    #[cfg(windows)]
    #[test]
    fn resolve_and_load_returns_a_well_formed_resolution() {
        match resolve_and_load() {
            DllResolution::FoundAt(dll) => {
                assert!(dll.ends_with(QHY_DLL_NAME), "{}", dll.display());
            }
            DllResolution::FoundByName => {}
            DllResolution::NotFound { probed } => {
                // The exe dir candidate is always present.
                assert!(!probed.is_empty());
            }
        }
    }
}
