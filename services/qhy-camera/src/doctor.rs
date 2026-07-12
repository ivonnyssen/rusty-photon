//! `qhy-camera doctor` — interactive QHYCCD Windows installation diagnostic.
//!
//! An interactive subcommand can do what a session-0 service cannot: talk to
//! the operator and open a browser. It reports how (and whether) the
//! delay-loaded `qhyccd.dll` resolves, the loaded SDK version vs. the pinned
//! build-time version (ABI skew made visible — ADR-015 accepted risk),
//! best-effort All-in-One driver-pack presence, and the download URL.
//! Behavioral contracts DR1–DR5 in `docs/services/qhy-camera.md`
//! § "Windows: qhyccd.dll resolution".
//!
//! Report data, rendering, exit-code mapping, and prompt parsing are pure
//! over plain data so they are unit-testable on every platform; only the
//! gathering (real `LoadLibrary` + SDK calls) is `#[cfg(windows)]`.

use std::fmt::Write as _;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::preflight::{DllResolution, PinnedSdkVersion, PINNED_SDK_VERSION, QHY_ALL_IN_ONE_URL};

/// The SDK version reported by the loaded DLL (`GetQHYCCDSDKVersion`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdkVersionFinding {
    /// `GetQHYCCDSDKVersion` succeeded.
    Loaded {
        year: u32,
        month: u32,
        day: u32,
        subday: u32,
    },
    /// The DLL resolved but SDK init / version query failed.
    Failed(String),
    /// Not queried: the DLL is missing (calling into a missing delay-loaded
    /// DLL would trip the delay-load helper), or this is a simulation build.
    Skipped,
}

impl SdkVersionFinding {
    /// `true` when a loaded version differs from the build-time pin
    /// (year/month/day; a non-zero subday alone is not skew).
    pub fn skewed(&self, pinned: &PinnedSdkVersion) -> bool {
        match self {
            Self::Loaded {
                year, month, day, ..
            } => (*year, *month, *day) != (pinned.year, pinned.month, pinned.day),
            Self::Failed(_) | Self::Skipped => false,
        }
    }

    fn render_loaded(year: u32, month: u32, day: u32, subday: u32) -> String {
        if subday == 0 {
            format!("{year:02}.{month:02}.{day:02}")
        } else {
            format!("{year:02}.{month:02}.{day:02}.{subday}")
        }
    }
}

/// Everything the doctor gathers. Rendering ([`render`](Self::render)),
/// health ([`healthy`](Self::healthy)) and [`exit_code`](Self::exit_code)
/// are pure functions of this data.
#[derive(Debug)]
pub struct DoctorReport {
    /// `true` on a `simulation` build: the real FFI is `cfg`'d out, so no SDK
    /// call is ever made and no `qhyccd.dll` is required at runtime (DR5).
    pub simulation: bool,
    /// How `qhyccd.dll` resolved; `None` when not applicable (simulation).
    pub dll: Option<DllResolution>,
    /// What the loaded SDK reports as its version.
    pub sdk_version: SdkVersionFinding,
    /// Cameras the loaded SDK enumerated (`None` when the SDK never loaded).
    pub cameras: Option<usize>,
    /// Best-effort All-in-One presence: each known install root + existence.
    pub driver_pack: Vec<(PathBuf, bool)>,
}

impl DoctorReport {
    /// DR3: healthy = DLL resolved *and* the SDK version was readable.
    /// Version skew alone is a surfaced warning, not a failure (ADR-015
    /// accepts the ABI-skew risk). Simulation builds are trivially healthy.
    pub fn healthy(&self) -> bool {
        if self.simulation {
            return true;
        }
        match &self.dll {
            Some(DllResolution::FoundAt(_)) | Some(DllResolution::FoundByName) => {
                matches!(self.sdk_version, SdkVersionFinding::Loaded { .. })
            }
            Some(DllResolution::NotFound { .. }) | None => false,
        }
    }

    /// DR3: 0 = healthy, 1 = unhealthy.
    pub fn exit_code(&self) -> i32 {
        i32::from(!self.healthy())
    }

    /// Render the operator-facing report (DR1).
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "qhy-camera doctor — QHYCCD Windows installation check");
        let _ = writeln!(out, "-----------------------------------------------------");

        if self.simulation {
            let _ = writeln!(
                out,
                "Build             : simulation backend — no SDK calls are made and no \
                 qhyccd.dll is required at runtime."
            );
        } else {
            match &self.dll {
                Some(DllResolution::FoundAt(dll)) => {
                    let _ = writeln!(out, "qhyccd.dll        : found — {}", dll.display());
                }
                Some(DllResolution::FoundByName) => {
                    let _ = writeln!(
                        out,
                        "qhyccd.dll        : found via the default Windows DLL search \
                         order (exe directory, System32, PATH)"
                    );
                }
                Some(DllResolution::NotFound { probed, failures }) => {
                    let _ = writeln!(out, "qhyccd.dll        : NOT FOUND");
                    for dir in probed {
                        let _ = writeln!(out, "  probed          : {}", dir.display());
                    }
                    let _ = writeln!(
                        out,
                        "  ...plus the standard Windows DLL search order (exe directory, \
                         System32, PATH)"
                    );
                    for failure in failures {
                        let _ = writeln!(
                            out,
                            "  load failed     : {} — {}",
                            failure.path.display(),
                            failure.error
                        );
                    }
                }
                None => {
                    let _ = writeln!(out, "qhyccd.dll        : not checked");
                }
            }

            match &self.sdk_version {
                SdkVersionFinding::Loaded {
                    year,
                    month,
                    day,
                    subday,
                } => {
                    let loaded = SdkVersionFinding::render_loaded(*year, *month, *day, *subday);
                    if self.sdk_version.skewed(&PINNED_SDK_VERSION) {
                        let _ = writeln!(
                            out,
                            "SDK version       : {loaded} — DIFFERS from the build-time pin \
                             {PINNED_SDK_VERSION} (possible ABI skew; if qhy-camera \
                             misbehaves, install the All-in-One pack matching the pinned SDK)"
                        );
                    } else {
                        let _ = writeln!(
                            out,
                            "SDK version       : {loaded} (matches the build-time pin \
                             {PINNED_SDK_VERSION})"
                        );
                    }
                }
                SdkVersionFinding::Failed(reason) => {
                    let _ = writeln!(
                        out,
                        "SDK version       : UNAVAILABLE — {reason} (build-time pin: \
                         {PINNED_SDK_VERSION})"
                    );
                }
                SdkVersionFinding::Skipped => {
                    let _ = writeln!(
                        out,
                        "SDK version       : not queried (build-time pin: {PINNED_SDK_VERSION})"
                    );
                }
            }

            match self.cameras {
                Some(count) => {
                    let _ = writeln!(out, "Cameras detected  : {count}");
                }
                None => {
                    let _ = writeln!(out, "Cameras detected  : unknown (SDK not loaded)");
                }
            }
        }

        if self.driver_pack.is_empty() {
            let _ = writeln!(
                out,
                "Driver pack       : no known install roots to check on this system"
            );
        } else {
            let mut label = "Driver pack       :";
            for (dir, exists) in &self.driver_pack {
                let presence = if *exists { "present" } else { "not found" };
                let _ = writeln!(out, "{label} {} — {presence}", dir.display());
                label = "                   ";
            }
        }
        let _ = writeln!(out, "Download page     : {QHY_ALL_IN_ONE_URL}");

        let _ = writeln!(out);
        if self.healthy() {
            let _ = writeln!(out, "Result: OK — qhy-camera can load the QHYCCD SDK.");
        } else {
            let _ = writeln!(
                out,
                "Result: FAIL — install QHY's All-in-One pack from {QHY_ALL_IN_ONE_URL} \
                 (it provides both the signed device driver and qhyccd.dll)."
            );
        }
        out
    }
}

/// DR2: ask whether to open the QHY download page. Anything but an explicit
/// yes — including EOF on a non-interactive stdin — counts as "No".
pub fn prompt_open_download_page(input: &mut dyn BufRead, output: &mut dyn Write) -> bool {
    let _ = write!(
        output,
        "Open the QHY download page ({QHY_ALL_IN_ONE_URL}) in your browser? [y/N] "
    );
    let _ = output.flush();
    let mut line = String::new();
    if input.read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim(), "y" | "Y" | "yes" | "Yes" | "YES")
}

/// Run the doctor; returns the process exit code (DR3/DR4).
pub fn run() -> i32 {
    #[cfg(windows)]
    {
        run_windows()
    }
    #[cfg(not(windows))]
    {
        // DR4: on Unix the SDK is linked statically at build time — there is
        // no DLL to resolve and nothing to diagnose.
        println!(
            "qhy-camera doctor is Windows-only: it diagnoses the delay-loaded qhyccd.dll \
             that QHY's All-in-One pack provides. On this platform the QHYCCD SDK is \
             linked statically at build time — nothing to check."
        );
        // `main` terminates via `std::process::exit`, which bypasses
        // destructors — flush explicitly so buffered stdout cannot be lost.
        let _ = std::io::stdout().flush();
        0
    }
}

#[cfg(windows)]
fn run_windows() -> i32 {
    let report = gather();
    print!("{}", report.render());
    // `main` terminates via `std::process::exit`, which bypasses destructors:
    // flush so the report is fully out before the prompt (and cannot be lost
    // when stdout is buffered/non-interactive).
    let _ = std::io::stdout().flush();

    // DR2: offer the download page when something needs the operator's
    // attention — missing/broken DLL or a version skew.
    if !report.healthy() || report.sdk_version.skewed(&PINNED_SDK_VERSION) {
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        if prompt_open_download_page(&mut stdin.lock(), &mut stdout) {
            open_download_page();
        }
    }
    // Same rationale: the "Opening …" line must not vanish at process::exit.
    let _ = std::io::stdout().flush();
    report.exit_code()
}

#[cfg(windows)]
fn gather() -> DoctorReport {
    let driver_pack: Vec<(PathBuf, bool)> =
        crate::preflight::driver_pack_dirs(|var| std::env::var(var).ok())
            .into_iter()
            .map(|dir| {
                let exists = dir.is_dir();
                (dir, exists)
            })
            .collect();

    #[cfg(feature = "simulation")]
    {
        // DR5: the simulation backend makes no SDK calls — qhyccd.dll is not
        // required at runtime.
        DoctorReport {
            simulation: true,
            dll: None,
            sdk_version: SdkVersionFinding::Skipped,
            cameras: None,
            driver_pack,
        }
    }
    #[cfg(not(feature = "simulation"))]
    {
        let dll = crate::preflight::resolve_and_load();
        let (sdk_version, cameras) = match &dll {
            // Never call into the SDK when the delay-loaded DLL is missing —
            // the delay-load helper would fault instead of returning an error.
            DllResolution::NotFound { .. } => (SdkVersionFinding::Skipped, None),
            DllResolution::FoundAt(_) | DllResolution::FoundByName => query_sdk(),
        };
        DoctorReport {
            simulation: false,
            dll: Some(dll),
            sdk_version,
            cameras,
            driver_pack,
        }
    }
}

/// Init the SDK from the resident DLL and read `GetQHYCCDSDKVersion` + the
/// camera count. First real FFI calls of the process: the delay-load helper
/// binds them to the module the preflight probe loaded.
#[cfg(all(windows, not(feature = "simulation")))]
fn query_sdk() -> (SdkVersionFinding, Option<usize>) {
    match qhyccd_rs::Sdk::new() {
        Ok(sdk) => {
            let cameras = Some(sdk.cameras().count());
            match sdk.version() {
                Ok(version) => (
                    SdkVersionFinding::Loaded {
                        year: version.year,
                        month: version.month,
                        day: version.day,
                        subday: version.subday,
                    },
                    cameras,
                ),
                Err(error) => (
                    SdkVersionFinding::Failed(format!("GetQHYCCDSDKVersion failed: {error}")),
                    cameras,
                ),
            }
        }
        Err(error) => (
            SdkVersionFinding::Failed(format!("SDK init failed: {error}")),
            None,
        ),
    }
}

/// Open the QHY download page in the default browser via `cmd /C start`.
/// The empty argument fills `start`'s window-title slot so the URL can never
/// be mistaken for one.
#[cfg(windows)]
fn open_download_page() {
    match std::process::Command::new("cmd")
        .args(["/C", "start", "", QHY_ALL_IN_ONE_URL])
        .spawn()
    {
        Ok(_) => println!("Opening {QHY_ALL_IN_ONE_URL} ..."),
        Err(error) => eprintln!("Could not open the browser: {error}"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn found_report() -> DoctorReport {
        DoctorReport {
            simulation: false,
            dll: Some(DllResolution::FoundAt(PathBuf::from(
                r"C:\Program Files\QHYCCD\AllInOne\sdk\x64\qhyccd.dll",
            ))),
            sdk_version: SdkVersionFinding::Loaded {
                year: 26,
                month: 6,
                day: 4,
                subday: 0,
            },
            cameras: Some(1),
            driver_pack: vec![
                (PathBuf::from(r"C:\Program Files\QHYCCD"), true),
                (PathBuf::from(r"C:\Program Files (x86)\QHYCCD"), false),
            ],
        }
    }

    #[test]
    fn healthy_report_renders_ok_and_exits_zero() {
        let report = found_report();
        let rendered = report.render();
        assert!(report.healthy());
        assert_eq!(report.exit_code(), 0);
        assert!(
            rendered.contains(r"found — C:\Program Files\QHYCCD\AllInOne\sdk\x64\qhyccd.dll"),
            "{rendered}"
        );
        assert!(
            rendered.contains("26.06.04 (matches the build-time pin 26.06.04)"),
            "{rendered}"
        );
        assert!(rendered.contains("Cameras detected  : 1"), "{rendered}");
        assert!(
            rendered.contains(r"C:\Program Files\QHYCCD — present"),
            "{rendered}"
        );
        assert!(
            rendered.contains(r"C:\Program Files (x86)\QHYCCD — not found"),
            "{rendered}"
        );
        assert!(rendered.contains("Result: OK"), "{rendered}");
    }

    #[test]
    fn version_skew_warns_but_still_exits_zero() {
        let report = DoctorReport {
            sdk_version: SdkVersionFinding::Loaded {
                year: 26,
                month: 9,
                day: 12,
                subday: 0,
            },
            ..found_report()
        };
        let rendered = report.render();
        assert!(report.sdk_version.skewed(&PINNED_SDK_VERSION));
        assert!(report.healthy());
        assert_eq!(report.exit_code(), 0);
        assert!(
            rendered.contains("26.09.12 — DIFFERS from the build-time pin 26.06.04"),
            "{rendered}"
        );
    }

    #[test]
    fn subday_alone_is_not_skew_and_renders_with_suffix() {
        let finding = SdkVersionFinding::Loaded {
            year: 26,
            month: 6,
            day: 4,
            subday: 1,
        };
        assert!(!finding.skewed(&PINNED_SDK_VERSION));
        assert_eq!(SdkVersionFinding::render_loaded(26, 6, 4, 1), "26.06.04.1");
    }

    #[test]
    fn missing_dll_renders_probed_dirs_failed_attempts_and_url_and_exits_one() {
        let report = DoctorReport {
            simulation: false,
            dll: Some(DllResolution::NotFound {
                probed: vec![
                    PathBuf::from(r"C:\Program Files\rusty-photon"),
                    PathBuf::from(r"C:\Program Files\QHYCCD\AllInOne\sdk\x64"),
                ],
                failures: vec![crate::preflight::LoadFailure {
                    path: PathBuf::from(r"C:\Program Files\rusty-photon\qhyccd.dll"),
                    error: "wrong architecture".to_string(),
                }],
            }),
            sdk_version: SdkVersionFinding::Skipped,
            cameras: None,
            driver_pack: vec![(PathBuf::from(r"C:\Program Files\QHYCCD"), false)],
        };
        let rendered = report.render();
        assert!(!report.healthy());
        assert_eq!(report.exit_code(), 1);
        assert!(rendered.contains("NOT FOUND"), "{rendered}");
        assert!(
            rendered.contains(r"probed          : C:\Program Files\rusty-photon"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                r"load failed     : C:\Program Files\rusty-photon\qhyccd.dll — wrong architecture"
            ),
            "{rendered}"
        );
        assert!(rendered.contains(QHY_ALL_IN_ONE_URL), "{rendered}");
        assert!(rendered.contains("Result: FAIL"), "{rendered}");
    }

    #[test]
    fn found_by_name_with_readable_version_is_healthy() {
        let report = DoctorReport {
            dll: Some(DllResolution::FoundByName),
            ..found_report()
        };
        let rendered = report.render();
        assert!(report.healthy());
        assert!(
            rendered.contains("found via the default Windows DLL search order"),
            "{rendered}"
        );
    }

    #[test]
    fn dll_found_but_sdk_failure_exits_one() {
        let report = DoctorReport {
            sdk_version: SdkVersionFinding::Failed("SDK init failed: QHYCCD_ERROR".to_string()),
            cameras: None,
            ..found_report()
        };
        let rendered = report.render();
        assert!(!report.healthy());
        assert_eq!(report.exit_code(), 1);
        assert!(
            rendered.contains("UNAVAILABLE — SDK init failed: QHYCCD_ERROR"),
            "{rendered}"
        );
    }

    #[test]
    fn simulation_build_reports_no_dll_needed_and_exits_zero() {
        let report = DoctorReport {
            simulation: true,
            dll: None,
            sdk_version: SdkVersionFinding::Skipped,
            cameras: None,
            driver_pack: vec![],
        };
        let rendered = report.render();
        assert!(report.healthy());
        assert_eq!(report.exit_code(), 0);
        // Runtime-accurate phrasing: `simulation` cfg's out the SDK *calls*;
        // it does not by itself remove the SDK *link* (that is
        // QHYCCD_SKIP_NATIVE_LINK's job) — the report must not claim it does.
        assert!(
            rendered.contains(
                "simulation backend — no SDK calls are made and no qhyccd.dll is \
                 required at runtime."
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("no known install roots to check"),
            "{rendered}"
        );
        assert!(rendered.contains("Result: OK"), "{rendered}");
    }

    #[test]
    fn prompt_accepts_explicit_yes_variants() {
        for yes in ["y\n", "Y\n", "yes\n", "Yes\n", "YES\n"] {
            let mut input = Cursor::new(yes.as_bytes().to_vec());
            let mut output = Vec::new();
            assert!(
                prompt_open_download_page(&mut input, &mut output),
                "{yes:?} should count as yes"
            );
        }
    }

    #[test]
    fn prompt_defaults_to_no_on_anything_else() {
        for no in ["n\n", "N\n", "\n", "maybe\n", ""] {
            let mut input = Cursor::new(no.as_bytes().to_vec());
            let mut output = Vec::new();
            assert!(
                !prompt_open_download_page(&mut input, &mut output),
                "{no:?} should count as no"
            );
        }
    }

    #[test]
    fn prompt_text_names_the_url() {
        let mut input = Cursor::new(b"n\n".to_vec());
        let mut output = Vec::new();
        prompt_open_download_page(&mut input, &mut output);
        let prompt = String::from_utf8(output).unwrap();
        assert!(prompt.contains(QHY_ALL_IN_ONE_URL), "{prompt}");
        assert!(prompt.contains("[y/N]"), "{prompt}");
    }

    /// `run()` is exercised end-to-end only where it is cheap and
    /// deterministic: on non-Windows it prints the DR4 note and returns 0.
    #[cfg(not(windows))]
    #[test]
    fn run_on_unix_is_a_windows_only_note_with_exit_zero() {
        assert_eq!(run(), 0);
    }
}
