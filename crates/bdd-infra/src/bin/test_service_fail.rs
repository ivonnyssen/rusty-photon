//! Minimal failing test service for bdd-infra integration tests.
//!
//! Exits immediately without printing `bound_addr=`, simulating a
//! service that fails to start (e.g., due to invalid configuration).

fn main() {
    std::process::exit(1);
}
