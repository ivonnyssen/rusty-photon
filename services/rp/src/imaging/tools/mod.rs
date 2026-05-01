//! Compositional image-analysis tools: bind multiple
//! [`crate::imaging::analysis`] kernels together to answer an end-user
//! question (one MCP tool per file). Pure functions over `ArrayView2`;
//! no I/O, no async.

pub mod auto_focus;
pub mod measure_basic;
pub mod measure_stars;
