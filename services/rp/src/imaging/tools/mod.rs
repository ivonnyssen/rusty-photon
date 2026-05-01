//! Compositional tools, one MCP tool per file. Two flavors:
//!
//! - **Pure compositional analyzers** (`measure_basic`,
//!   `measure_stars`): bind multiple [`crate::imaging::analysis`]
//!   kernels into one MCP-tool-shaped result. Pure functions over
//!   `ArrayView2`, no I/O, no async — unit-testable without a runtime.
//!
//! - **Compound equipment-driving tools** (`auto_focus`; planned
//!   `center_on_target`): drive a multi-step move/capture/measure
//!   loop using primitive built-in tools. They expose async traits
//!   for the equipment surface so the driving logic is testable
//!   against synthetic adapters; the MCP wrapper in `mcp.rs` provides
//!   live adapters that bind to the real Alpaca clients. The math and
//!   grid logic these tools bundle (`build_grid`, `fit_parabola`,
//!   `validate_params`) remain pure helpers.

pub mod auto_focus;
pub mod measure_basic;
pub mod measure_stars;
