//! Imaging: FITS I/O, pixel statistics, image cache, and image-analysis tools.
//!
//! Submodules are organized by capability so each tool (`compute_image_stats`,
//! `measure_basic`, future `detect_stars` / `measure_stars` / `estimate_background`
//! / `compute_snr`) can be implemented in isolation. The image cache holds the
//! pixel buffer that `capture` already decoded so subsequent tools don't
//! re-read and re-decode the FITS file. See `docs/services/rp.md` (Image
//! Analysis Strategy and Image Cache) for the design.

pub mod background;
pub mod cache;
pub mod fits;
pub mod fwhm;
pub mod hfr;
pub mod measure_basic;
pub mod measure_stars;
pub mod pixel;
pub mod snr;
pub mod stars;
pub mod stats;

pub use background::{estimate_background, sigma_clipped_stats, BackgroundStats};
pub use cache::{CachedImage, CachedPixels, ImageCache};
pub use fits::{read_fits_pixels, write_fits};
pub use fwhm::{fit_2d_gaussian, GaussianFit2D};
pub use hfr::{aggregate_hfr, star_hfr};
pub use measure_basic::{measure_basic, MeasureBasicResult};
pub use measure_stars::{measure_stars, MeasureStarsResult, StarMeasurement};
pub use pixel::Pixel;
pub use snr::{compute_snr, per_star_snr, SnrResult};
pub use stars::{detect_stars, DetectionParams, Star};
pub use stats::{compute_stats, ImageStats};
