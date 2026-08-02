//! Renders the most recent captured FITS frame as an 8-bit grayscale
//! PNG for `GET /preview.png` — presentation only, so the P5 UI can
//! draw the star/target overlay over the actual sky. Nothing here
//! feeds the alignment math; star *analysis* stays in rp's
//! `detect_stars`.
//!
//! The pipeline is deliberately simple: stride-subsample to the
//! requested width, linear percentile stretch computed on the preview
//! pixels, encode grayscale PNG. Downscaling never affects overlay
//! accuracy — `/status` speaks native pixel coordinates and the UI
//! scales the bitmap under its viewBox.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Default preview width when `?width=` is absent.
pub const DEFAULT_PREVIEW_WIDTH: u32 = 1024;

/// Requests below this width render at it (a smaller preview has no
/// UI value and invites accidental `width=1` requests).
const MIN_PREVIEW_WIDTH: u32 = 64;

/// The linear stretch spans these percentiles of the preview pixels:
/// the low cut swallows outlier-dark pixels, the high cut keeps star
/// cores from crushing the sky background to black.
const STRETCH_LOW_PERCENTILE: f64 = 0.005;
const STRETCH_HIGH_PERCENTILE: f64 = 0.999;

/// Why a preview could not be rendered. `NoFrameOnDisk` maps to 404
/// (rp owns the capture directory and may prune it); `Unreadable` is
/// a real failure and maps to 500.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("the captured frame no longer exists on disk: {0}")]
    NoFrameOnDisk(String),
    #[error("failed to render the preview: {0}")]
    Unreadable(String),
}

/// Render the FITS primary HDU at `path` as a grayscale PNG of at
/// most `width` pixels across (clamped to [64, native width]).
pub fn render_png(path: &Path, width: u32) -> Result<Vec<u8>, PreviewError> {
    let file = File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            PreviewError::NoFrameOnDisk(path.display().to_string())
        } else {
            PreviewError::Unreadable(format!("open {}: {e}", path.display()))
        }
    })?;
    let (pixels, native_w, native_h) =
        rp_fits::reader::read_primary_as_i32(BufReader::new(file))
            .map_err(|e| PreviewError::Unreadable(format!("read {}: {e}", path.display())))?;
    render_pixels_png(&pixels, native_w, native_h, width)
}

/// The pure half of [`render_png`], unit-testable without a file.
fn render_pixels_png(
    pixels: &[i32],
    native_w: u32,
    native_h: u32,
    width: u32,
) -> Result<Vec<u8>, PreviewError> {
    if native_w == 0 || native_h == 0 || pixels.len() != (native_w as usize) * (native_h as usize) {
        return Err(PreviewError::Unreadable(format!(
            "frame geometry {native_w}×{native_h} does not match {} pixels",
            pixels.len()
        )));
    }

    let width = width.min(native_w).max(MIN_PREVIEW_WIDTH.min(native_w));
    let stride = native_w.div_ceil(width);
    let out_w = native_w.div_ceil(stride);
    let out_h = native_h.div_ceil(stride);
    let stride = stride as usize;

    let mut sampled = Vec::with_capacity((out_w as usize) * (out_h as usize));
    for y in (0..native_h as usize).step_by(stride) {
        for x in (0..native_w as usize).step_by(stride) {
            sampled.push(pixels[y * native_w as usize + x]);
        }
    }

    let mut sorted = sampled.clone();
    sorted.sort_unstable();
    let percentile = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize];
    let low = percentile(STRETCH_LOW_PERCENTILE);
    let high = percentile(STRETCH_HIGH_PERCENTILE);

    let bytes: Vec<u8> = if high > low {
        // Widen before subtracting: pixels can span the full i32
        // range (the reader saturates to it), where `high - low`
        // overflows i32.
        let low = i64::from(low);
        let range = (i64::from(high) - low) as f64;
        sampled
            .iter()
            .map(|&v| (((i64::from(v) - low) as f64 / range) * 255.0).clamp(0.0, 255.0) as u8)
            .collect()
    } else {
        // A constant frame (cover closed, test pattern) renders
        // mid-gray instead of dividing by zero.
        vec![128; sampled.len()]
    };

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, out_w, out_h);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| PreviewError::Unreadable(format!("png header: {e}")))?;
        writer
            .write_image_data(&bytes)
            .map_err(|e| PreviewError::Unreadable(format!("png data: {e}")))?;
    }
    Ok(out)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    fn decode(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(info.color_type, png::ColorType::Grayscale);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    /// A gradient frame keeps its ordering through the stretch and
    /// spans the full output range.
    #[test]
    fn test_gradient_renders_full_range_at_native_size() {
        let pixels: Vec<i32> = (0..64 * 64).map(|i| i % 4096).collect();
        let png_bytes = render_pixels_png(&pixels, 64, 64, 64).unwrap();
        let (w, h, gray) = decode(&png_bytes);
        assert_eq!((w, h), (64, 64));
        assert_eq!(*gray.iter().min().unwrap(), 0);
        assert_eq!(*gray.iter().max().unwrap(), 255);
    }

    #[test]
    fn test_width_request_downsamples_by_integer_stride() {
        let pixels: Vec<i32> = (0..200 * 100).map(|i| i % 1000).collect();
        // Requesting 90 of 200 gives stride 3 → 67×34.
        let png_bytes = render_pixels_png(&pixels, 200, 100, 90).unwrap();
        let (w, h, _) = decode(&png_bytes);
        assert_eq!((w, h), (67, 34));
    }

    #[test]
    fn test_width_is_clamped_to_the_native_and_minimum_bounds() {
        let pixels: Vec<i32> = (0..128 * 96).map(|i| i % 500).collect();
        // Larger than native → native.
        let (w, _, _) = decode(&render_pixels_png(&pixels, 128, 96, 4096).unwrap());
        assert_eq!(w, 128);
        // Absurdly small → the 64-px floor.
        let (w, _, _) = decode(&render_pixels_png(&pixels, 128, 96, 1).unwrap());
        assert_eq!(w, 64);
    }

    /// The reader saturates pixels to the full i32 range; the
    /// stretch must widen before subtracting or `high - low`
    /// overflows.
    #[test]
    fn test_extreme_pixel_range_does_not_overflow_the_stretch() {
        let mut pixels = vec![i32::MIN; 32 * 64];
        pixels.extend(vec![i32::MAX; 32 * 64]);
        let (_, _, gray) = decode(&render_pixels_png(&pixels, 64, 64, 64).unwrap());
        assert_eq!(*gray.iter().min().unwrap(), 0);
        assert_eq!(*gray.iter().max().unwrap(), 255);
    }

    #[test]
    fn test_constant_frame_renders_mid_gray() {
        let pixels = vec![7000; 64 * 64];
        let (_, _, gray) = decode(&render_pixels_png(&pixels, 64, 64, 64).unwrap());
        assert!(gray.iter().all(|&g| g == 128), "constant frame → mid-gray");
    }

    #[test]
    fn test_geometry_mismatch_is_an_error() {
        let err = render_pixels_png(&[0; 10], 64, 64, 64).unwrap_err();
        assert!(err.to_string().contains("does not match"), "{err}");
    }

    #[test]
    fn test_missing_file_maps_to_no_frame_on_disk() {
        let err = render_png(Path::new("/nonexistent/pa-preview.fits"), 1024).unwrap_err();
        assert!(matches!(err, PreviewError::NoFrameOnDisk(_)), "{err}");
    }

    /// End-to-end through a real FITS file written with `rp-fits`.
    #[test]
    fn test_fits_round_trip_renders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.fits");
        let pixels: Vec<u16> = (0..96u32 * 64).map(|i| (i % 9000) as u16).collect();
        let mut file = File::create(&path).unwrap();
        rp_fits::writer::write_u16_image(&mut file, &pixels, 96, 64, &[]).unwrap();
        drop(file);

        let png_bytes = render_png(&path, 96).unwrap();
        let (w, h, gray) = decode(&png_bytes);
        assert_eq!((w, h), (96, 64));
        assert!(gray.iter().any(|&g| g > 200), "stretch reaches the top");
        assert!(gray.iter().any(|&g| g < 50), "stretch reaches the bottom");
    }
}
