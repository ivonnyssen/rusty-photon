//! Minimal test service for bdd-infra integration tests.
//!
//! Binds to a random port, prints `bound_addr=<addr>` to stdout,
//! then blocks until terminated. Accepts `--config <path>` to match
//! the interface expected by `ServiceHandle`.
//!
//! On Unix, SIGHUP triggers a rebind to a fresh random port and emits a
//! new `bound_addr=<addr>` line so `ServiceHandle::reload` can observe
//! the rebind. On Windows, reload is implemented over named pipes which
//! this minimal binary doesn't provide — callers should skip reload-path
//! assertions there.
//!
//! If the config file contains the text "fail", exits immediately
//! with code 1 (simulates a service that fails to start).

use tokio::net::TcpListener;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(idx) = args.iter().position(|a| a == "--config") {
        if let Some(path) = args.get(idx + 1) {
            if let Ok(content) = std::fs::read_to_string(path) {
                if content.contains("fail") {
                    std::process::exit(1);
                }
            }
        }
    }

    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    println!("bound_addr={}", listener.local_addr().unwrap());

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sighup = signal(SignalKind::hangup()).expect("install SIGHUP handler");
        loop {
            tokio::select! {
                accepted = listener.accept() => { drop(accepted); }
                _ = sighup.recv() => {
                    listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    println!("bound_addr={}", listener.local_addr().unwrap());
                }
            }
        }
    }

    #[cfg(not(unix))]
    loop {
        drop(listener.accept().await);
    }
}
