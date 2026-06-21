//! Unit tests requiring FFI mocking
//!
//! These tests use mockall to mock the QHYCCD FFI layer. They must remain
//! in the src/ directory because mockall's `#[automock]` attribute generates
//! mock contexts at compile time that must be in the same crate.

mod camera_tests;
mod filter_wheel_tests;
mod sdk_tests;

use std::sync::{Mutex, MutexGuard};

/// Serializes every test that programs the process-global mockall FFI mocks.
///
/// `#[automock]` stores each `*_context()` expectation in process-global
/// state, so two of these tests running concurrently in the same process
/// corrupt each other's expectations — surfacing as `Fragile`
/// "destructor ran on wrong thread" panics and `called 0 time(s) which is
/// fewer than expected 1` failures, which abort the test binary (SIGABRT).
///
/// `cargo nextest` (the project's standard runner, used by `cargo rail`)
/// isolates each test in its own process and hides this. Plain `cargo test`
/// runs them as threads in one process — that includes the nightly safety
/// sanitizer workflow and a local `cargo test -p qhyccd-rs`. Every `#[test]`
/// in this module's submodules therefore takes this guard as its first line
/// and holds it for the whole body. See issue #384.
static MOCK_FFI_MTX: Mutex<()> = Mutex::new(());

/// Acquire the [`MOCK_FFI_MTX`] guard. Tolerates poisoning: these tests
/// `assert!` and panic on failure, which would otherwise poison the mutex and
/// cascade a single real failure into spurious failures of every later test.
pub(super) fn mock_guard() -> MutexGuard<'static, ()> {
    MOCK_FFI_MTX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
