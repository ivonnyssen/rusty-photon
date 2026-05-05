//! `mock_astap` — test double mimicking the ASTAP CLI surface.
//!
//! Behavior is selected via `MOCK_ASTAP_MODE`:
//!
//! | Mode | Behavior |
//! |------|----------|
//! | `normal` (default) | Read `-f <path>`, write a canned `.wcs` sidecar next to it, exit 0 |
//! | `exit_failure` | Print to stderr, exit 1 (no `.wcs`) |
//! | `hang` | Sleep indefinitely; respond to the platform's graceful signal cleanly |
//! | `ignore_sigterm` | Trap and ignore the graceful signal; sleep anyway. Force-kill terminates. |
//! | `malformed_wcs` | Write a `.wcs` missing CRVAL2, exit 0 |
//! | `no_wcs` | Exit 0 without writing any `.wcs` |
//!
//! `MOCK_ASTAP_ARGV_OUT=<path>` (any mode) appends the received argv to the
//! file at `<path>`, one arg per line, with a trailing blank line as record
//! separator. Used for end-to-end argv-flow assertions.
//!
//! Pattern mirrors `services/phd2-guider/src/bin/mock_phd2.rs`.

use std::io::Write;
use std::path::PathBuf;

/// Canned `.wcs` sidecar content for `MOCK_ASTAP_MODE=normal`. Inlined
/// rather than `include_str!`-ed from `tests/fixtures/` so Bazel's
/// sandboxed compilation doesn't need a `data` dependency to find it.
/// Shape mirrors ASTAP's real `.wcs` output: a header-only FITS primary
/// HDU (`NAXIS = 0`, so no data block follows), padded to one 2880-byte
/// FITS block. Includes `CTYPE1`/`CTYPE2` so `wcs::WCSParams`'s
/// mandatory fields deserialize cleanly.
const CANNED_WCS: &str = concat!(
    "SIMPLE  =                    T                                                  ",
    "BITPIX  =                    8                                                  ",
    "NAXIS   =                    0                                                  ",
    "CTYPE1  = 'RA---TAN'                                                            ",
    "CTYPE2  = 'DEC--TAN'                                                            ",
    "CRVAL1  =              10.6848                                                  ",
    "CRVAL2  =              41.2690                                                  ",
    "CDELT1  =         -0.000291667                                                  ",
    "CDELT2  =          0.000291667                                                  ",
    "CROTA2  =                 12.3                                                  ",
    "COMMENT ASTAP-CLI mock_astap test double                                        ",
    "END                                                                             ",
    // 2880-byte FITS block padding: 12 cards × 80 = 960 bytes; pad
    // 1920 bytes (24 × 80) of spaces to reach the next block.
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
    "                                                                                ",
);

#[cfg(debug_assertions)]
const _: () = {
    // Compile-time guard: total length must be exactly 2880 bytes (one
    // FITS block). The parser depends on this layout; a stray space
    // here would propagate as a silent bug.
    assert!(CANNED_WCS.len() == 2880);
};

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();

    if let Ok(out_path) = std::env::var("MOCK_ASTAP_ARGV_OUT") {
        let _ = write_argv(&out_path, &args);
    }

    let mode = std::env::var("MOCK_ASTAP_MODE").unwrap_or_else(|_| "normal".to_string());

    match mode.as_str() {
        "normal" => run_normal(&args),
        "exit_failure" => run_exit_failure(),
        "hang" => run_hang(),
        "ignore_sigterm" => run_ignore_sigterm(),
        "malformed_wcs" => run_malformed_wcs(&args),
        "no_wcs" => run_no_wcs(),
        other => {
            eprintln!("mock_astap: unknown MOCK_ASTAP_MODE: {other}");
            std::process::ExitCode::from(2)
        }
    }
}

fn write_argv(path: &str, args: &[String]) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for a in args {
        writeln!(f, "{a}")?;
    }
    writeln!(f)?;
    Ok(())
}

fn fits_path_from_argv(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == "-f" {
            return iter.next().map(PathBuf::from);
        }
    }
    None
}

fn run_normal(args: &[String]) -> std::process::ExitCode {
    let Some(fits) = fits_path_from_argv(args) else {
        eprintln!("mock_astap: -f <path> required in `normal` mode");
        return std::process::ExitCode::from(2);
    };
    let wcs_path = fits.with_extension("wcs");
    if let Err(e) = std::fs::write(&wcs_path, CANNED_WCS) {
        eprintln!("mock_astap: failed to write {}: {e}", wcs_path.display());
        return std::process::ExitCode::from(2);
    }
    std::process::ExitCode::SUCCESS
}

fn run_exit_failure() -> std::process::ExitCode {
    eprintln!("mock_astap: simulated solve failure (exit 1)");
    std::process::ExitCode::from(1)
}

fn run_hang() -> std::process::ExitCode {
    // Sleep indefinitely. The supervision module's deadline will signal
    // us with the platform's graceful signal; default Unix SIGTERM handler
    // exits, default Windows behavior on CTRL_BREAK_EVENT terminates the
    // process — both are fine for this mode.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

#[cfg(unix)]
fn run_ignore_sigterm() -> std::process::ExitCode {
    // Install a SIGTERM handler that ignores the signal, then sleep
    // forever. The supervision module must escalate to SIGKILL.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

#[cfg(windows)]
fn run_ignore_sigterm() -> std::process::ExitCode {
    // SetConsoleCtrlHandler with a handler that returns TRUE swallows the
    // event so the process is not terminated.
    use std::os::raw::c_int;
    #[allow(non_snake_case)]
    extern "system" {
        fn SetConsoleCtrlHandler(
            HandlerRoutine: Option<unsafe extern "system" fn(u32) -> i32>,
            Add: i32,
        ) -> i32;
    }
    unsafe extern "system" fn handler(_event: u32) -> c_int {
        // Returning a non-zero ("TRUE") value indicates we handled the
        // signal — the process keeps running.
        1
    }
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

fn run_malformed_wcs(args: &[String]) -> std::process::ExitCode {
    let Some(fits) = fits_path_from_argv(args) else {
        eprintln!("mock_astap: -f <path> required in `malformed_wcs` mode");
        return std::process::ExitCode::from(2);
    };
    let wcs_path = fits.with_extension("wcs");
    // Write a header-only FITS primary HDU matching real ASTAP shape
    // (`NAXIS = 0`, no data block) but missing CRVAL2. The parser must
    // surface "CRVAL2" in its error so the HTTP contract names the
    // missing key.
    let cards = [
        "SIMPLE  =                    T",
        "BITPIX  =                    8",
        "NAXIS   =                    0",
        "CTYPE1  = 'RA---TAN'",
        "CTYPE2  = 'DEC--TAN'",
        "CRVAL1  =              10.6848",
        "CDELT1  =         -0.000291667",
        "END",
    ];
    let mut content = String::with_capacity(2880);
    for c in cards {
        content.push_str(&format!("{c:<80}"));
    }
    while content.len() < 2880 {
        content.push(' ');
    }
    if let Err(e) = std::fs::write(&wcs_path, content) {
        eprintln!("mock_astap: failed to write {}: {e}", wcs_path.display());
        return std::process::ExitCode::from(2);
    }
    std::process::ExitCode::SUCCESS
}

fn run_no_wcs() -> std::process::ExitCode {
    // Exit cleanly without writing a .wcs — wrapper must surface NoWcs.
    std::process::ExitCode::SUCCESS
}
