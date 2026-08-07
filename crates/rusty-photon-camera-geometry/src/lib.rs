//! Sensor geometry shared by the workspace's ASCOM camera drivers.
//!
//! `qhy-camera`, `zwo-camera` and `svbony-camera` drive three unrelated vendor
//! SDKs but present one ASCOM surface, so the rules that turn a client's
//! `StartX`/`NumX`/`BinX` into a frame the sensor can actually deliver are the
//! same rules written three times. They stayed the same rules until one copy
//! drifted: a sub-pixel ROI clamped to one pixel in `qhy-camera` and truncated
//! to zero in the other two, so a bin change could hand `StartExposure` a zero
//! the driver had invented and then blame the client for it. Neither the code
//! nor the tests showed it, because each driver curated its own case list — the
//! missing behaviour and its missing test hid each other.
//!
//! What lives here is the arithmetic with no vendor in it. Anything a vendor
//! genuinely disagrees about stays in the driver: the frame unpack, the
//! `MaxADU` ceiling, the Bayer table, the readout formats. The rule of thumb is
//! the dependency list below — a vendor type or an `ascom-alpaca` type in a
//! signature here would mean the item belongs in a driver instead.
//!
//! Where drivers differ by *degree* rather than in kind, the difference is a
//! parameter. `qhy-camera` has no sub-frame alignment rule while the other two
//! require a binned width that is a multiple of 8 and a height that is a
//! multiple of 2, so [`check`] takes an [`Alignment`] rather than existing in
//! two versions that could disagree about anything else.

use core::fmt;
use core::num::{NonZeroU128, NonZeroU32};
use core::time::Duration;

/// A region of interest in *binned* pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Roi {
    /// Left edge, binned pixels from the sensor's origin.
    pub start_x: u32,
    /// Top edge, binned pixels from the sensor's origin.
    pub start_y: u32,
    /// Width in binned pixels (ASCOM `NumX`).
    pub width: u32,
    /// Height in binned pixels (ASCOM `NumY`).
    pub height: u32,
}

/// A sensor's sub-frame alignment rule: the binned extent must be a multiple of
/// these, or the SDK rejects the ROI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alignment {
    /// `NumX` must be a multiple of this.
    pub width: u32,
    /// `NumY` must be a multiple of this.
    pub height: u32,
}

impl Alignment {
    /// The rule requiring `width`-pixel and `height`-pixel multiples.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// Why a ROI is not a geometry the sensor can be asked for.
///
/// The `Display` text is what the driver hands to ASCOM as an `INVALID_VALUE`
/// message, so it names ASCOM members rather than this crate's field names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    /// `NumX` or `NumY` is zero, so there is no frame to expose.
    ZeroExtent,
    /// The bin is zero, so the sensor extent it divides has no value.
    ZeroBin,
    /// The extent violates the sensor's sub-frame alignment rule.
    Misaligned(Alignment),
    /// `StartX + NumX` runs past the binned sensor width.
    OutOfBoundsX,
    /// `StartY + NumY` runs past the binned sensor height.
    OutOfBoundsY,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ZeroExtent => f.write_str("NumX and NumY must be greater than 0"),
            Self::ZeroBin => f.write_str("BinX and BinY must be greater than 0"),
            Self::Misaligned(align) => write!(
                f,
                "NumX must be a multiple of {} and NumY a multiple of {}",
                align.width, align.height
            ),
            Self::OutOfBoundsX => f.write_str("StartX + NumX exceeds CameraXSize / BinX"),
            Self::OutOfBoundsY => f.write_str("StartY + NumY exceeds CameraYSize / BinY"),
        }
    }
}

impl core::error::Error for GeometryError {}

/// Validate a ROI against the sensor at a given bin, with an optional
/// alignment rule.
///
/// `sensor_w`/`sensor_h` are the *reported* full-frame extent — for a driver
/// that aligns what it reports (see [`aligned_sensor_extent`]) that is the
/// aligned value, not the raw sensor, so the bound and the alignment rule
/// agree with each other.
///
/// # Order is the contract
///
/// Rules are applied cheapest-precondition-first — zero extent, zero bin,
/// alignment, then bounds — and the order is pinned by tests rather than left
/// to whichever check happens to come first in the source. It decides which
/// value a client is told about when a ROI breaks more than one rule at once,
/// and the answer has to be the one that explains the rest: a zero bin is not a
/// geometry that fails a rule but one with no rule to apply, so it is reported
/// ahead of an alignment complaint that a client could otherwise chase while
/// the real problem sat in `BinX`.
///
/// # Errors
///
/// Returns the first [`GeometryError`] the ROI trips, in that order.
///
/// ```
/// use rusty_photon_camera_geometry::{check, Alignment, GeometryError, Roi};
///
/// let asi = Some(Alignment::new(8, 2));
/// let roi = Roi { start_x: 0, start_y: 0, width: 64, height: 48 };
/// check(roi, 6240, 4176, 1, asi).unwrap();
///
/// // A ROI that is both misaligned and at a zero bin hears about the bin.
/// let misaligned = Roi { width: 100, ..roi };
/// assert_eq!(check(misaligned, 6240, 4176, 0, asi), Err(GeometryError::ZeroBin));
/// assert_eq!(
///     check(misaligned, 6240, 4176, 1, asi),
///     Err(GeometryError::Misaligned(Alignment::new(8, 2)))
/// );
/// ```
pub fn check(
    roi: Roi,
    sensor_w: u32,
    sensor_h: u32,
    bin: u32,
    align: Option<Alignment>,
) -> Result<(), GeometryError> {
    if roi.width == 0 || roi.height == 0 {
        return Err(GeometryError::ZeroExtent);
    }
    // The bin divides the sensor extent, so rejecting a zero here is what makes
    // this validator total — the alternative leaves a division for every caller
    // to prove safe.
    let (Some(max_x), Some(max_y)) = (sensor_w.checked_div(bin), sensor_h.checked_div(bin)) else {
        return Err(GeometryError::ZeroBin);
    };
    if let Some(align) = align {
        if !is_multiple_of(roi.width, align.width) || !is_multiple_of(roi.height, align.height) {
            return Err(GeometryError::Misaligned(align));
        }
    }
    if roi.start_x.saturating_add(roi.width) > max_x {
        return Err(GeometryError::OutOfBoundsX);
    }
    if roi.start_y.saturating_add(roi.height) > max_y {
        return Err(GeometryError::OutOfBoundsY);
    }
    Ok(())
}

/// Whether `value` is a multiple of `divisor`, treating a zero divisor as
/// "no rule" rather than dividing by it.
fn is_multiple_of(value: u32, divisor: u32) -> bool {
    NonZeroU32::new(divisor).is_none_or(|d| value % d == 0)
}

/// Rescale a ROI in binned coordinates from bin `old` to bin `new`.
///
/// The ASCOM `StartX`/`NumX` members are set independently, so only their
/// combination can be validated and it is — at `StartExposure`, by [`check`].
/// Whatever the client last set therefore arrives here, and rescaling must not
/// change *which value* the eventual error is about:
///
/// - A **sub-pixel** extent is clamped to 1. Truncating it to 0 would make
///   `StartExposure` reject a zero this function invented rather than the
///   client's own `NumX`.
/// - A **client-set 0** is preserved, so it still earns the
///   [`ZeroExtent`](GeometryError::ZeroExtent) it was heading for. Clamping it
///   to 1 would clear that check and move the complaint onto a value nobody
///   set — or, on a driver with no alignment rule, clear every remaining check
///   and expose a one-pixel frame in place of the error the client had earned.
///
/// A zero `new` is not a scale, so the ROI is returned unchanged; the zero is
/// then reported by [`check`], which is where a client can be told about it.
///
/// ```
/// use rusty_photon_camera_geometry::{rescale, Roi};
///
/// let roi = Roi { start_x: 100, start_y: 100, width: 640, height: 480 };
/// let binned = rescale(roi, 1, 2);
/// assert_eq!(binned, Roi { start_x: 50, start_y: 50, width: 320, height: 240 });
///
/// // A sub-pixel extent survives as one pixel, not as a zero the client never set.
/// assert_eq!(rescale(Roi { width: 1, height: 1, ..roi }, 1, 4).width, 1);
/// ```
#[must_use]
pub fn rescale(roi: Roi, old: u8, new: u8) -> Roi {
    // Exact integer scaling rather than a float ratio: `v * old / new` truncates
    // in exactly the places the float form did, without the rounding error a
    // divide and a multiply in `f64` can accumulate between them.
    let old = u32::from(old);
    let Some(new) = NonZeroU32::new(u32::from(new)) else {
        return roi;
    };
    let scale = |v: u32| v.saturating_mul(old) / new;
    let extent = |v: u32| if v == 0 { 0 } else { scale(v).max(1) };
    Roi {
        start_x: scale(roi.start_x),
        start_y: scale(roi.start_y),
        width: extent(roi.width),
        height: extent(roi.height),
    }
}

/// The largest extent at or below `max` such that the full frame divided by
/// *every* supported bin is still a valid ROI — i.e. the binned extent is a
/// multiple of `unit` (8 for width, 2 for height on both ASI and SVBony).
///
/// ConformU, and clients generally, take a full frame at each bin via
/// `NumX = CameraXSize / bin`. Reporting the raw sensor size makes those binned
/// full frames unachievable wherever `raw / bin` misses the alignment rule, and
/// this was found by ConformU on real hardware twice: an ASI2600's 6248 / 2 is
/// 3124, not a multiple of 8, and an SV605CC's 3008 / 3 is 1002, likewise. So a
/// driver reports the largest multiple of `lcm(unit · bin)` that fits, giving up
/// a few edge columns at full resolution to make every binned full frame exactly
/// achievable — and as a bonus making [`rescale`] round-trip cleanly.
///
/// Degenerate inputs return `max` unchanged rather than reducing it: an empty
/// bin list, a bin list of zeroes, or a step that already exceeds the sensor.
///
/// ```
/// use rusty_photon_camera_geometry::aligned_sensor_extent;
///
/// // ASI2600: 6248 reduces to 6240, so 6240/{1,2,3,4} are all multiples of 8.
/// assert_eq!(aligned_sensor_extent(6248, &[1, 2, 3, 4], 8), 6240);
/// // Nothing to align against leaves the extent alone.
/// assert_eq!(aligned_sensor_extent(6248, &[], 8), 6248);
/// ```
#[must_use]
pub fn aligned_sensor_extent(max: u32, supported_bins: &[u32], unit: u32) -> u32 {
    let step = supported_bins
        .iter()
        .copied()
        .filter(|&b| b > 0)
        .map(|b| unit.saturating_mul(b))
        .fold(1, lcm);
    let Some(step) = NonZeroU32::new(step).filter(|s| s.get() <= max) else {
        return max;
    };
    // A remainder is never larger than what it came from, so this cannot wrap.
    max.saturating_sub(max % step)
}

/// Greatest common divisor (Euclid) of two non-zero operands. The result is
/// itself non-zero, since it divides both — which is what lets [`lcm`] divide
/// by it without a second check.
fn gcd(a: NonZeroU32, b: NonZeroU32) -> NonZeroU32 {
    let (mut a, mut b) = (a, b);
    // `a % b` is total once `b` carries its own non-zero-ness, so the loop
    // needs no separate guard on the divisor.
    while let Some(rem) = NonZeroU32::new(a.get() % b) {
        a = b;
        b = rem;
    }
    b
}

/// Least common multiple (`0` if either operand is `0`).
fn lcm(a: u32, b: u32) -> u32 {
    let (Some(a_nz), Some(b_nz)) = (NonZeroU32::new(a), NonZeroU32::new(b)) else {
        // Zero shares no multiple with anything.
        return 0;
    };
    // The gcd divides `a` exactly, so dividing before multiplying keeps the
    // intermediate as small as it can be; saturating covers a pair whose
    // multiple genuinely does not fit.
    (a / gcd(a_nz, b_nz)).saturating_mul(b)
}

/// How far through an exposure, as ASCOM's `PercentCompleted`, **capped at 99**.
///
/// 100 is the ready state's answer, so an exposure still in flight must never
/// report it however close it is — a client polling for completion would
/// otherwise read 100 and go looking for an image that is not there yet.
///
/// A `total` under a microsecond has no ratio to report. The exposure is
/// effectively instantaneous rather than unstarted, so it reads 99: a 0 there
/// would claim no progress on something already finishing. A driver that cannot
/// tell an unrecorded duration from a sub-microsecond one should answer that
/// question before calling this, where it still has the state to answer it with.
///
/// ```
/// use core::time::Duration;
/// use rusty_photon_camera_geometry::progress_percent;
///
/// let total = Duration::from_secs(10);
/// assert_eq!(progress_percent(Duration::from_secs(3), total), 30);
/// // Never 100 while in flight, however close.
/// assert_eq!(progress_percent(total, total), 99);
/// assert_eq!(progress_percent(Duration::from_secs(999), total), 99);
/// ```
#[must_use]
pub fn progress_percent(done: Duration, total: Duration) -> u8 {
    let Some(total_us) = NonZeroU128::new(total.as_micros()) else {
        return 99;
    };
    // Integer throughout, so there is no float to round: the ratio is capped
    // before it is narrowed, which gives the conversion an answer for every
    // input, and the fallback is that same cap.
    let pct = done.as_micros().saturating_mul(100) / total_us;
    u8::try_from(pct.min(99)).unwrap_or(99)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const ASI: Option<Alignment> = Some(Alignment::new(8, 2));

    fn roi_at(start_x: u32, start_y: u32, width: u32, height: u32) -> Roi {
        Roi {
            start_x,
            start_y,
            width,
            height,
        }
    }

    // --- check ---------------------------------------------------------------

    #[test]
    fn zero_extent_is_rejected_on_either_axis() {
        assert_eq!(
            check(roi_at(0, 0, 0, 64), 6240, 4176, 1, ASI),
            Err(GeometryError::ZeroExtent)
        );
        assert_eq!(
            check(roi_at(0, 0, 64, 0), 6240, 4176, 1, ASI),
            Err(GeometryError::ZeroExtent)
        );
    }

    #[test]
    fn a_zero_bin_is_reported_ahead_of_the_alignment_rule() {
        // Both rules are broken. The bin wins, because nothing — including the
        // bound the alignment rule is measured against — can be computed
        // without one.
        assert_eq!(
            check(roi_at(0, 0, 100, 64), 6240, 4176, 0, ASI),
            Err(GeometryError::ZeroBin)
        );
        // ...and with no alignment rule in play it is still the bin, not a
        // bounds complaint against a sensor extent that was never divided.
        assert_eq!(
            check(roi_at(0, 0, 100, 64), 6240, 4176, 0, None),
            Err(GeometryError::ZeroBin)
        );
    }

    #[test]
    fn a_zero_extent_is_reported_ahead_of_a_zero_bin() {
        // A client that set neither gets told about `NumX` first, matching the
        // order the ASCOM members are conventionally set in.
        assert_eq!(
            check(roi_at(0, 0, 0, 0), 6240, 4176, 0, ASI),
            Err(GeometryError::ZeroExtent)
        );
    }

    #[test]
    fn misalignment_is_reported_ahead_of_a_bounds_failure() {
        // 8000 is both misaligned and past the sensor. The alignment rule is
        // the local, fixable complaint; being told the frame is too big invites
        // shrinking it to another misaligned value.
        assert_eq!(
            check(roi_at(0, 0, 8001, 64), 6240, 4176, 1, ASI),
            Err(GeometryError::Misaligned(Alignment::new(8, 2)))
        );
    }

    #[test]
    fn alignment_applies_per_axis() {
        assert_eq!(
            check(roi_at(0, 0, 100, 64), 6240, 4176, 1, ASI),
            Err(GeometryError::Misaligned(Alignment::new(8, 2)))
        );
        assert_eq!(
            check(roi_at(0, 0, 64, 47), 6240, 4176, 1, ASI),
            Err(GeometryError::Misaligned(Alignment::new(8, 2)))
        );
        // No rule, same ROI, no complaint — this is qhy-camera's case, and it
        // is the only thing that distinguishes it.
        check(roi_at(0, 0, 100, 47), 6240, 4176, 1, None).unwrap();
    }

    #[test]
    fn bounds_are_measured_against_the_binned_extent() {
        check(roi_at(0, 0, 6240, 4176), 6240, 4176, 1, ASI).unwrap();
        check(roi_at(0, 0, 3120, 2088), 6240, 4176, 2, ASI).unwrap();
        // The same frame that fits at bin 1 does not at bin 2.
        assert_eq!(
            check(roi_at(0, 0, 4000, 64), 6240, 4176, 2, ASI),
            Err(GeometryError::OutOfBoundsX)
        );
    }

    #[test]
    fn bounds_name_the_axis_that_failed() {
        assert_eq!(
            check(roi_at(0, 0, 8000, 64), 6240, 4176, 1, ASI),
            Err(GeometryError::OutOfBoundsX)
        );
        assert_eq!(
            check(roi_at(0, 0, 64, 6000), 6240, 4176, 1, ASI),
            Err(GeometryError::OutOfBoundsY)
        );
        // A start offset counts toward the bound, not just the extent.
        assert_eq!(
            check(roi_at(6240, 0, 64, 64), 6240, 4176, 1, ASI),
            Err(GeometryError::OutOfBoundsX)
        );
        assert_eq!(
            check(roi_at(0, 4176, 64, 64), 6240, 4176, 1, ASI),
            Err(GeometryError::OutOfBoundsY)
        );
    }

    #[test]
    fn a_start_offset_near_u32_max_does_not_wrap_into_bounds() {
        // `start + width` is saturating, so an offset that would wrap reports
        // out-of-bounds rather than passing as a small sum.
        assert_eq!(
            check(roi_at(u32::MAX, 0, 8, 2), 6240, 4176, 1, ASI),
            Err(GeometryError::OutOfBoundsX)
        );
    }

    #[test]
    fn messages_name_ascom_members() {
        assert_eq!(
            GeometryError::ZeroExtent.to_string(),
            "NumX and NumY must be greater than 0"
        );
        assert_eq!(
            GeometryError::ZeroBin.to_string(),
            "BinX and BinY must be greater than 0"
        );
        assert_eq!(
            GeometryError::Misaligned(Alignment::new(8, 2)).to_string(),
            "NumX must be a multiple of 8 and NumY a multiple of 2"
        );
        assert_eq!(
            GeometryError::OutOfBoundsX.to_string(),
            "StartX + NumX exceeds CameraXSize / BinX"
        );
        assert_eq!(
            GeometryError::OutOfBoundsY.to_string(),
            "StartY + NumY exceeds CameraYSize / BinY"
        );
    }

    // --- rescale -------------------------------------------------------------

    #[test]
    fn rescale_scales_by_the_bin_ratio() {
        let roi = roi_at(100, 200, 640, 480);
        assert_eq!(rescale(roi, 1, 2), roi_at(50, 100, 320, 240));
        assert_eq!(rescale(roi_at(50, 100, 320, 240), 2, 1), roi);
        // Same bin is identity.
        assert_eq!(rescale(roi, 2, 2), roi);
    }

    #[test]
    fn rescale_clamps_a_sub_pixel_extent_to_one() {
        // 1 / 4 truncates to 0, which `check` would then reject as a zero the
        // client never set.
        let scaled = rescale(roi_at(0, 0, 1, 1), 1, 4);
        assert_eq!(scaled.width, 1);
        assert_eq!(scaled.height, 1);
    }

    #[test]
    fn rescale_preserves_a_client_set_zero() {
        // The zero is the client's, and it has a `ZeroExtent` coming. Clamping
        // it to 1 would clear that check and complain about something else.
        let scaled = rescale(roi_at(0, 0, 0, 0), 1, 2);
        assert_eq!(scaled.width, 0);
        assert_eq!(scaled.height, 0);
    }

    #[test]
    fn rescale_leaves_the_origin_alone_at_zero() {
        // A start of 0 scales to 0 — it is a coordinate, not an extent, so the
        // minimum-of-one rule must not touch it.
        assert_eq!(rescale(roi_at(0, 0, 64, 64), 1, 2).start_x, 0);
    }

    #[test]
    fn rescale_by_a_zero_bin_is_the_identity() {
        // Not a scale, so nothing to do; `check` reports the zero to the client.
        let roi = roi_at(10, 20, 64, 48);
        assert_eq!(rescale(roi, 1, 0), roi);
    }

    #[test]
    fn rescale_saturates_rather_than_wrapping() {
        // `u32::MAX * 4` does not fit; saturating keeps the result absurd-but-
        // bounded, which `check` then rejects, rather than wrapping it small
        // enough to pass.
        let scaled = rescale(roi_at(0, 0, u32::MAX, u32::MAX), 4, 1);
        assert_eq!(scaled.width, u32::MAX);
    }

    // --- aligned_sensor_extent -----------------------------------------------

    #[test]
    fn alignment_makes_every_binned_full_frame_valid_on_the_asi2600() {
        // 6248 reduces to 6240 so 6240/{1,2,3,4} are all multiples of 8; 4176
        // is already aligned. Found by ConformU on real hardware.
        assert_eq!(aligned_sensor_extent(6248, &[1, 2, 3, 4], 8), 6240);
        assert_eq!(aligned_sensor_extent(4176, &[1, 2, 3, 4], 2), 4176);
        for bin in [1_u32, 2, 3, 4] {
            assert_eq!((6240 / bin) % 8, 0, "width / {bin} not a multiple of 8");
            assert_eq!((4176 / bin) % 2, 0, "height / {bin} not even");
        }
    }

    #[test]
    fn alignment_makes_every_binned_full_frame_valid_on_the_sv605cc() {
        // Width step lcm(8,16,24,32) = 96, height step lcm(2,4,6,8) = 24. Also
        // found by ConformU on real hardware — the same lesson, a second time,
        // which is why it is one function now.
        assert_eq!(aligned_sensor_extent(3008, &[1, 2, 3, 4], 8), 2976);
        assert_eq!(aligned_sensor_extent(3008, &[1, 2, 3, 4], 2), 3000);
        for bin in [1_u32, 2, 3, 4] {
            assert_eq!((2976 / bin) % 8, 0, "width at bin {bin}");
            assert_eq!((3000 / bin) % 2, 0, "height at bin {bin}");
        }
    }

    #[test]
    fn alignment_returns_the_raw_extent_for_degenerate_inputs() {
        // Nothing to align against.
        assert_eq!(aligned_sensor_extent(3008, &[], 8), 3008);
        assert_eq!(aligned_sensor_extent(100, &[0], 8), 100);
        // A step larger than the sensor would reduce it to zero; the raw
        // extent is the better answer.
        assert_eq!(aligned_sensor_extent(10, &[1, 2, 3, 4], 8), 10);
    }

    #[test]
    fn lcm_treats_zero_as_sharing_no_multiple() {
        assert_eq!(lcm(0, 5), 0);
        assert_eq!(lcm(5, 0), 0);
        assert_eq!(lcm(8, 12), 24);
        // Dividing by the gcd first keeps the intermediate small; saturating
        // covers a pair whose multiple genuinely does not fit.
        assert_eq!(lcm(u32::MAX, u32::MAX - 1), u32::MAX);
    }

    // --- progress_percent ----------------------------------------------------

    #[test]
    fn progress_is_the_integer_ratio() {
        let total = Duration::from_secs(10);
        assert_eq!(progress_percent(Duration::ZERO, total), 0);
        assert_eq!(progress_percent(Duration::from_secs(3), total), 30);
        assert_eq!(progress_percent(Duration::from_millis(9_990), total), 99);
    }

    #[test]
    fn progress_never_reports_complete_while_in_flight() {
        let total = Duration::from_secs(10);
        // Exactly done, and past done, both cap at 99 — 100 belongs to the
        // ready state, and a client polling for it must not be told early.
        assert_eq!(progress_percent(total, total), 99);
        assert_eq!(progress_percent(Duration::from_secs(999), total), 99);
        assert_eq!(progress_percent(Duration::MAX, total), 99);
    }

    #[test]
    fn a_sub_microsecond_exposure_reads_as_nearly_done() {
        // Not 0: there is no ratio to report, but the exposure is finishing,
        // not unstarted.
        assert_eq!(
            progress_percent(Duration::ZERO, Duration::from_nanos(500)),
            99
        );
    }
}
