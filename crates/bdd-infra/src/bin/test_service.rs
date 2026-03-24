//! Minimal test service for bdd-infra integration tests.
//!
//! Binds to a random port, prints `bound_addr=<addr>` to stdout,
//! then blocks until terminated. Accepts `--config <path>` to match
//! the interface expected by `ServiceHandle`.

fn main() {
    // Accept --config <path> (ignored, but required by ServiceHandle's spawn)
    let _args: Vec<String> = std::env::args().collect();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    println!("bound_addr={}", addr);

    // Block until killed (SIGTERM default handler terminates the process)
    for stream in listener.incoming() {
        drop(stream);
    }
}
