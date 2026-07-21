//! Minimal test service for bdd-infra integration tests.
//!
//! Binds to a random port, prints `bound_addr=<addr>` to stdout,
//! then blocks until terminated. Accepts `--config <path>` to match
//! the interface expected by `ServiceHandle`.
//!
//! If the config file contains the text "fail", exits immediately
//! with code 1 (simulates a service that fails to start).
//!
//! `--epipe-probe <marker>` (stdout) / `--epipe-probe-stderr <marker>`
//! (stderr) enable the broken-pipe regression probe used by the
//! `ServiceHandle` shutdown tests — see [`run_epipe_probe`].

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // A literal `fail` argument simulates a service that rejects its CLI and
    // exits before binding. Used to prove `start_with_args` /
    // `try_start_with_args` pass their argument vector through to the process.
    if args.iter().any(|a| a == "fail") {
        std::process::exit(1);
    }
    if let Some(idx) = args.iter().position(|a| a == "--config") {
        if let Some(path) = args.get(idx + 1) {
            if let Ok(content) = std::fs::read_to_string(path) {
                if content.contains("fail") {
                    std::process::exit(1);
                }
            }
        }
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    println!("bound_addr={}", addr);

    // In `--epipe-probe`/`--epipe-probe-stderr` mode hand off to the probe
    // runner, which keeps the process alive past SIGTERM (so it can write
    // during shutdown) using tokio's safe signal API. `listener` stays bound
    // for the duration.
    if let Some(idx) = args.iter().position(|a| a == "--epipe-probe") {
        if let Some(marker) = args.get(idx + 1).cloned() {
            run_probe_mode(marker, ProbeStream::Stdout);
            return;
        }
    }
    if let Some(idx) = args.iter().position(|a| a == "--epipe-probe-stderr") {
        if let Some(marker) = args.get(idx + 1).cloned() {
            run_probe_mode(marker, ProbeStream::Stderr);
            return;
        }
    }

    // Block until killed (SIGTERM default disposition terminates the process).
    for stream in listener.incoming() {
        drop(stream);
    }
}

/// Which stream [`run_epipe_probe`] writes to — `ServiceHandle` drains/
/// forwards stdout and stderr through separate pipes with the same
/// don't-abort-before-exit invariant, so the probe must be able to exercise
/// either one.
#[derive(Clone, Copy)]
enum ProbeStream {
    Stdout,
    Stderr,
}

impl ProbeStream {
    /// Write `data` then flush, locking the stream fresh each call (matching
    /// the original stdout-only probe's per-write locking).
    fn write_all(self, data: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        match self {
            ProbeStream::Stdout => {
                let mut out = std::io::stdout().lock();
                out.write_all(data)?;
                out.flush()
            }
            ProbeStream::Stderr => {
                let mut err = std::io::stderr().lock();
                err.write_all(data)?;
                err.flush()
            }
        }
    }
}

/// Drive `--epipe-probe`/`--epipe-probe-stderr` mode: spawn the blocking
/// probe against `stream`, then await SIGTERM via tokio's **safe** signal API
/// (the same `tokio::signal::unix` mechanism `ServiceRunner` uses) and tell
/// the probe to run its shutdown burst. Using tokio keeps this fixture free
/// of `unsafe`/`libc` signal handling. On non-Unix there is no SIGTERM to
/// await and the probe writes until the process is terminated — the
/// regression tests are Unix-gated.
fn run_probe_mode(marker: String, stream: ProbeStream) {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let shutdown = Arc::new(AtomicBool::new(false));
    let probe_flag = Arc::clone(&shutdown);
    std::thread::spawn(move || run_epipe_probe(marker, stream, probe_flag));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        #[cfg(unix)]
        {
            use std::sync::atomic::Ordering;
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
            term.recv().await;
            shutdown.store(true, Ordering::SeqCst);
        }
        #[cfg(not(unix))]
        let _ = &shutdown;
        // The probe thread terminates the process once its burst completes;
        // park here until then.
        std::future::pending::<()>().await
    });
}

/// Regression probe for the stdout-drain / stderr-forwarder shutdown race.
///
/// Writes to `stream` steadily, then — once SIGTERM has been requested — emits
/// a short "shutdown" burst, just like a real service logging while it tears
/// down. A correct `ServiceHandle` keeps draining/forwarding our stream until
/// we have exited, so every write (including the shutdown burst) succeeds and
/// `marker` is left untouched. The old buggy ordering aborted the drain
/// (closing the read end of the pipe) *before* the child exited; the
/// shutdown-burst writes then fail with `BrokenPipe` — exactly the condition
/// that made services' `tracing_subscriber` echo "Unable to write an event
/// ... Broken pipe" to stderr. We record that broken pipe to `marker` so the
/// test can assert it never happens. We use `write_all` (not `println!`,
/// which *panics* on a broken pipe) so we can observe the error instead of
/// dying on it.
fn run_epipe_probe(
    marker: String,
    stream: ProbeStream,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    let mut line = [b'x'; 64].to_vec();
    line.push(b'\n');

    // Steady state: keep the harness's drain/forward task busy until shutdown.
    while !shutdown.load(Ordering::SeqCst) {
        if stream.write_all(&line).is_err() {
            // A failure here (before shutdown) means the reader is already
            // gone for some unrelated reason; don't misattribute it.
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    // Shutdown burst: this is the window where the bug bites.
    for _ in 0..50 {
        if let Err(e) = stream.write_all(b"shutting down\n") {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                let _ = std::fs::write(&marker, b"EPIPE");
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    std::process::exit(0);
}
