//! Direct ConformU-CLI runner.
//!
//! A feature-gated replacement for `ascom_alpaca::test::ConformUTestBuilder`.
//! The builder is a thin wrapper that runs the external `conformu` binary; the
//! only reason it lived behind `ascom-alpaca`'s `test` feature is that feature's
//! transitive `dtor` dependency — which `crate_universe` (Bazel) resolves only
//! from default features and therefore drops, keeping the conformu integration
//! tests out of the Bazel build entirely. Driving the CLI directly here removes
//! that dependency, so the tests compile under both Cargo and Bazel.
//!
//! The ConformU binary is located via the `CONFORMU_PATH` env var (set by the
//! conformu CI workflow and forwarded into the Bazel test sandbox). When it is
//! unset the run is **skipped**, so the conformu integration tests stay inert in
//! the normal cargo/bazel suites and fire only when ConformU is explicitly
//! provided — preserving the old `#[ignore]` ergonomics without `#[ignore]`
//! (which Bazel cannot selectively run via a tag).

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Outcome of [`run_conformu`].
#[derive(Debug, PartialEq, Eq)]
pub enum ConformuRun {
    /// `CONFORMU_PATH` was not set, so ConformU was not run. Callers treat this
    /// as a pass: the suite is inert unless ConformU is explicitly provided.
    Skipped,
    /// ConformU ran and reported success (zero exit status).
    Passed,
}

/// Run the ASCOM ConformU `conformance` suite against a running Alpaca device.
///
/// Equivalent to:
///
/// ```text
/// conformu conformance --settingsfile <settings_file> <base_url>/api/v1/<device_type>/<device_number>
/// ```
///
/// `device_type` is the lowercase Alpaca device-type URL segment (`"focuser"`,
/// `"camera"`, `"switch"`, `"telescope"`, `"rotator"`, `"covercalibrator"`,
/// `"observingconditions"`, `"safetymonitor"`). `base_url` is the device server
/// root (e.g. `http://127.0.0.1:PORT/`), typically `ServiceHandle::base_url`.
///
/// Returns [`ConformuRun::Skipped`] when `CONFORMU_PATH` is unset,
/// [`ConformuRun::Passed`] on success, and `Err` when ConformU exits non-zero.
pub async fn run_conformu(
    device_type: &str,
    base_url: &str,
    device_number: u32,
    settings_file: Option<&Path>,
) -> Result<ConformuRun, Box<dyn std::error::Error + Send + Sync>> {
    let Some(conformu) = std::env::var_os("CONFORMU_PATH").filter(|v| !v.is_empty()) else {
        eprintln!("CONFORMU_PATH not set; skipping ConformU run for {device_type}/{device_number}");
        return Ok(ConformuRun::Skipped);
    };

    let device_url = format!(
        "{base}/api/v1/{device_type}/{device_number}",
        base = base_url.trim_end_matches('/'),
    );

    // Run both ConformU suites against the device, matching the upstream
    // ascom_alpaca::test runner (`ConformUTestBuilder::run`): `alpacaprotocol`
    // (Alpaca wire-protocol conformance) then `conformance` (full ASCOM
    // device-interface tests). Both must pass.
    for mode in ["alpacaprotocol", "conformance"] {
        run_mode(&conformu, mode, settings_file, &device_url).await?;
    }
    Ok(ConformuRun::Passed)
}

/// Run a single ConformU mode (`alpacaprotocol` or `conformance`) against
/// `device_url`, streaming its output. Returns `Err` on a non-zero exit.
async fn run_mode(
    conformu: &std::ffi::OsStr,
    mode: &str,
    settings_file: Option<&Path>,
    device_url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut command = Command::new(conformu);
    command.arg(mode);
    // ConformU writes a per-run log tree under $HOME (e.g.
    // $HOME/Documents/ascom/logs<date>). Under Bazel's test sandbox the real
    // $HOME is read-only, so ConformU aborts on startup; point HOME at the
    // test's writable TEST_TMPDIR. (Under Cargo there is no sandbox and $HOME is
    // already writable, so this is a no-op there.)
    if let Some(tmp) = std::env::var_os("TEST_TMPDIR") {
        command.env("HOME", tmp);
    }
    // `--settingsfile` is optional: services that need non-default ConformU
    // settings (timeouts, which test groups to run) pass a written file; the
    // rest run with ConformU's defaults.
    if let Some(path) = settings_file {
        command.arg("--settingsfile").arg(path);
    }
    let mut child = command.arg(device_url).stdout(Stdio::piped()).spawn()?;

    // Stream ConformU's (unstructured) stdout into the test log so progress is
    // visible and a verbose run can't deadlock on an undrained pipe.
    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            println!("[conformu {mode}] {line}");
        }
    }

    let status = child.wait().await?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("ConformU `{mode}` exited with {status} testing {device_url}").into())
    }
}
