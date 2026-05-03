//! Planner sub-tree: MCP tool wrappers over `rp-catalog` (catalog
//! lookup) and `rp-ephemeris` (positions, transit, twilight, etc.),
//! plus the decision logic that composes those primitives into the
//! convenience tools `get_target_status` / `get_next_target` /
//! `get_meridian_status`.
//!
//! The math and data live in their respective crates; this module is
//! purely the MCP-tool wrapping plus the small amount of decision
//! logic that doesn't belong in either dependency. See
//! `docs/services/rp.md` §"Planning and Ephemeris".

pub mod catalog;
pub mod primitives;
