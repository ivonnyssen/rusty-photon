//! Minimal test service for bdd-infra integration tests.
//!
//! Binds to a random port, prints `bound_addr=<addr>` to stdout,
//! then blocks until terminated. Accepts `--config <path>` to match
//! the interface expected by `ServiceHandle`.
//!
//! Reload is implemented on both platforms so `ServerPool`'s reuse path
//! can be exercised cross-platform:
//!
//! * **Unix** — SIGHUP triggers a TCP rebind.
//! * **Windows** — a named-pipe server at
//!   `\\.\pipe\rusty-photon-reload-{pid}` reads one byte per connection
//!   and treats any message as a reload request.
//!
//! In both cases the service emits a fresh `bound_addr=<addr>` line so
//! `ServiceHandle::reload`'s stdout watcher observes the new port.
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

    #[cfg(windows)]
    {
        use tokio::io::AsyncReadExt;
        use tokio::net::windows::named_pipe::ServerOptions;

        let pipe_name = format!(r"\\.\pipe\rusty-photon-reload-{}", std::process::id());
        let mut pipe = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .expect("create named pipe server");

        loop {
            tokio::select! {
                accepted = listener.accept() => { drop(accepted); }
                connect_result = pipe.connect() => {
                    connect_result.expect("accept named-pipe client");
                    // Drain any payload the client sent. Content is ignored —
                    // any message is treated as a reload request.
                    let mut buf = [0u8; 1];
                    let _ = pipe.read(&mut buf).await;
                    listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    println!("bound_addr={}", listener.local_addr().unwrap());
                    // The current pipe instance is bound to the now-disconnected
                    // client; a fresh instance is required to accept the next
                    // reload request.
                    pipe = ServerOptions::new()
                        .create(&pipe_name)
                        .expect("recreate named pipe server");
                }
            }
        }
    }
}
