//! Image analysis: pure kernels and the compositional tools that bind
//! them together. FITS I/O, the image cache, and exposure-document
//! storage live in [`crate::persistence`] — this module is async- and
//! I/O-free so kernels can be unit-tested in isolation.
//!
//! Submodules:
//! - [`analysis`]: single-purpose math (background, stars, hfr, fwhm,
//!   snr, stats, pixel trait).
//! - [`tools`]: compositional analyzers (measure_basic, measure_stars).
//!
//! The flat re-exports below preserve the previous `crate::imaging::*`
//! call-site shape so MCP wiring doesn't have to know which submodule
//! a symbol lives in. See `docs/services/rp.md` (Module Structure) and
//! `docs/plans/image-evaluation-tools.md` for the rationale.

pub mod analysis;
pub mod tools;

pub use analysis::background::{estimate_background, sigma_clipped_stats, BackgroundStats};
pub use analysis::fwhm::{fit_2d_gaussian, GaussianFit2D};
pub use analysis::hfr::{aggregate_hfr, star_hfr};
pub use analysis::pixel::Pixel;
pub use analysis::snr::{compute_snr, per_star_snr, SnrResult};
pub use analysis::stars::{detect_stars, DetectionParams, Star};
pub use analysis::stats::{compute_stats, ImageStats};
pub use tools::measure_basic::{measure_basic, MeasureBasicResult};
pub use tools::measure_stars::{
    measure_stars, MeasureStarsResult, StarMeasurement, DEFAULT_STAMP_HALF_SIZE,
};
