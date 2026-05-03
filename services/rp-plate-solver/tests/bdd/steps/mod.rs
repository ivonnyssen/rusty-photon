//! Step definition modules. Each maps to one `.feature` file under
//! `tests/features/`. Cucumber's `#[given]` / `#[when]` / `#[then]`
//! macros register globally, so a step phrase declared here is
//! resolved no matter which feature file references it — shared
//! phrases (e.g., "the response status is N") live in
//! `solve_steps.rs` and are reused across other feature files'
//! scenarios.
//!
//! All step bodies are Phase 3 stubs (`todo!("Phase 4")`). The
//! `@wip` filter in `tests/bdd.rs` skips every scenario at runtime,
//! so the stubs compile but never panic. Phase 4 fills the bodies in
//! and removes the `@wip` tag in the same commit.

pub mod config_steps;
pub mod health_steps;
pub mod real_astap_steps;
pub mod solve_steps;
pub mod supervision_steps;
