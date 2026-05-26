//! Typed physical quantities for the mount's pointing math.
//!
//! Every angle in this driver is "hours" or "degrees", but the *frame* —
//! what the number is measured against — matters as much as the unit, and on
//! this mount a frame mix-up drives the counterweights into the tripod. These
//! newtypes make the frame explicit at the type level so the pointing math
//! can't be written wrong: you cannot pass a celestial hour angle where a
//! mechanical one is expected, mix RA-axis and Dec-axis ticks, or cross
//! between frames without naming the conversion, without a compile error.
//!
//! This mirrors the convention `pa-falcon-rotator` established in its own
//! `units.rs` (mechanical vs sky angle bridged by a sync offset) — see
//! [ADR-006](../../../docs/decisions/006-typed-physical-quantities-for-mount-pointing.md).
//!
//! ## Frames
//!
//! Two axes, each with a *mechanical* (encoder) frame and a *celestial*
//! (sky) frame, plus the sidereal-time and RA reference quantities:
//!
//! ```text
//! RA axis  (hours)    Lst, Ra            reference quantities
//!                     HourAngle          celestial HA = LST - RA, [-12, +12)
//!                     MechHa             mechanical (encoder) HA, [-12, +12)
//!                     RaTicks            RA-axis encoder counts
//! Dec axis (degrees)  Dec                celestial declination (~[-90, +90])
//!                     MechDec            mechanical (encoder) dec, [-180, +180)
//!                     DecTicks           Dec-axis encoder counts
//! shared              Cpr                counts per revolution (per axis)
//! ```
//!
//! ## Conversions (the named operations)
//!
//! ```text
//! Lst.hour_angle_of(Ra)        -> HourAngle      LST - RA, folded
//! HourAngle.to_mech()          -> MechHa         pre-flip: value preserved
//! MechHa.flipped()             -> MechHa         post-flip mirror (+12 h fold)
//! MechHa.to_ticks(Cpr)         -> RaTicks
//! RaTicks.to_mech_ha(Cpr)      -> MechHa
//! MechHa.to_ra(Lst)            -> Ra             pre-flip:  LST - mech_HA
//! MechHa.to_ra_flipped(Lst)    -> Ra             post-flip: LST - mech_HA + 12
//!
//! Dec.to_mech()                -> MechDec        pre-flip: value preserved
//! Dec.to_mech_flipped()        -> MechDec        through the pole: sign*(180-|d|)
//! MechDec.to_dec()             -> Dec            pre-flip: value preserved
//! MechDec.to_dec_flipped()     -> Dec            through the pole (self-inverse)
//! MechDec.to_ticks(Cpr)        -> DecTicks
//! DecTicks.to_mech_dec(Cpr)    -> MechDec
//! ```
//!
//! Each constructor funnels through a fold/normalise helper, so every value
//! is canonical by construction. The conversions are the only way to cross a
//! frame boundary, and each is total. Constructors do **not** sanitise
//! non-finite input — callers reject `NaN`/`inf` first (the ASCOM boundary,
//! the wire parsers, and the config newtypes all do); a non-finite value
//! would propagate.

/// Fold a value into `[-period/2, +period/2)`. Shared by the hour-angle fold
/// (`period = 24`) and the mechanical-declination fold (`period = 360`).
fn fold_to_signed(value: f64, period: f64) -> f64 {
    let half = period / 2.0;
    let folded = value.rem_euclid(period);
    if folded >= half {
        folded - period
    } else {
        folded
    }
}

/// The "through the celestial pole" reflection used by a meridian flip:
/// `sign(d) * (180 - |d|)`. Maps a celestial declination to its post-flip
/// mechanical-encoder degree and back — the operation is its own inverse.
///
/// `f64::signum(0.0)` is `+1.0`, so `d = 0` maps to `+180°` (which the
/// [`MechDec`] fold then canonicalises to `-180°`); both are the same
/// physical position at the encoder wrap.
fn through_pole(deg: f64) -> f64 {
    deg.signum() * (180.0 - deg.abs())
}

/// Fold a raw encoder-tick value into the canonical band `[-cpr/2, +cpr/2)`.
/// The firmware's counter is wider than one axis revolution, so a
/// through-wrap slew can leave it outside the band; this collapses it to the
/// shortest equivalent. `cpr == 0` (parameter cache not populated) passes the
/// value through unchanged.
fn fold_ticks_canonical(value: i32, cpr: u32) -> i32 {
    if cpr == 0 {
        return value;
    }
    let cpr_i = cpr as i32;
    let half = cpr_i / 2;
    let modular = value.rem_euclid(cpr_i);
    if modular >= half {
        modular - cpr_i
    } else {
        modular
    }
}

/// Counts per revolution for one axis, queried from the mount at handshake.
///
/// A `Cpr(0)` models the "parameters not yet populated" state; the
/// tick↔angle conversions treat it defensively (returning a zero quantity)
/// rather than dividing by zero, matching the pre-typed accessors that
/// short-circuit before the math runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cpr(u32);

impl Cpr {
    /// Construct from a raw counts-per-revolution value.
    pub fn new(cpr: u32) -> Self {
        Self(cpr)
    }

    /// The underlying counts-per-revolution.
    pub fn get(self) -> u32 {
        self.0
    }
}

// ===================== RA axis (hours) =====================

/// Local apparent sidereal time, hours `[0, 24)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lst(f64);

impl Lst {
    /// Construct from hours, normalising into `[0, 24)`.
    pub fn new(hours: f64) -> Self {
        Self(hours.rem_euclid(24.0))
    }

    /// The underlying value in hours `[0, 24)`.
    pub fn value(self) -> f64 {
        self.0
    }

    /// Celestial hour angle of a target: `HA = LST - RA`, folded to
    /// `[-12, +12)`.
    pub fn hour_angle_of(self, ra: Ra) -> HourAngle {
        HourAngle::new(self.0 - ra.0)
    }
}

/// Right ascension, hours `[0, 24)` — the ASCOM client's frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ra(f64);

impl Ra {
    /// Construct from hours, normalising into `[0, 24)`.
    pub fn new(hours: f64) -> Self {
        Self(hours.rem_euclid(24.0))
    }

    /// The underlying value in hours `[0, 24)`.
    pub fn value(self) -> f64 {
        self.0
    }
}

/// Celestial hour angle (`LST - RA`), signed hours `[-12, +12)`. The frame the
/// flip-window decision reasons about; distinct from [`MechHa`] because the
/// two coincide only on the pre-flip pointing side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HourAngle(f64);

impl HourAngle {
    /// Construct from hours, folding into `[-12, +12)`.
    pub fn new(hours: f64) -> Self {
        Self(fold_to_signed(hours, 24.0))
    }

    /// The underlying value in signed hours `[-12, +12)`.
    pub fn value(self) -> f64 {
        self.0
    }

    /// Mechanical hour angle for the **pre-flip** (normal) pointing side,
    /// where mechanical HA equals celestial HA. Use [`MechHa::flipped`] for
    /// the post-flip mirror.
    pub fn to_mech(self) -> MechHa {
        MechHa::new(self.0)
    }
}

/// Mechanical hour angle: the encoder's view of where the polar axis points,
/// signed hours `[-12, +12)`. The quantity the counterweight exclusion zone,
/// the slew-path checks, and the tracking guard reason about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MechHa(f64);

impl MechHa {
    /// Construct from hours, folding into `[-12, +12)`.
    pub fn new(hours: f64) -> Self {
        Self(fold_to_signed(hours, 24.0))
    }

    /// The underlying value in signed hours `[-12, +12)`.
    pub fn value(self) -> f64 {
        self.0
    }

    /// The post-meridian-flip mirror of this mechanical HA: `mech_HA + 12 h`,
    /// folded back into `[-12, +12)`. Self-inverse.
    pub fn flipped(self) -> MechHa {
        MechHa::new(self.0 + 12.0)
    }

    /// Convert to RA-axis encoder ticks. `Cpr(0)` yields `RaTicks(0)`.
    pub fn to_ticks(self, cpr: Cpr) -> RaTicks {
        if cpr.0 == 0 {
            return RaTicks(0);
        }
        RaTicks((self.0 * (cpr.0 as f64) / 24.0).round() as i32)
    }

    /// ASCOM right ascension for the **pre-flip** side: `RA = (LST - mech_HA)`,
    /// folded to `[0, 24)`.
    pub fn to_ra(self, lst: Lst) -> Ra {
        Ra::new(lst.0 - self.0)
    }

    /// ASCOM right ascension for the **post-flip** side:
    /// `RA = (LST - mech_HA + 12)`, folded to `[0, 24)`.
    pub fn to_ra_flipped(self, lst: Lst) -> Ra {
        Ra::new(lst.0 - self.0 + 12.0)
    }
}

/// RA-axis encoder counts (signed; the firmware's raw counter, which can sit
/// outside one revolution after a through-wrap slew).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RaTicks(i32);

impl RaTicks {
    /// Construct from a raw RA-axis encoder count.
    pub fn new(ticks: i32) -> Self {
        Self(ticks)
    }

    /// The underlying signed encoder count.
    pub fn value(self) -> i32 {
        self.0
    }

    /// Convert to a mechanical hour angle, folded to `[-12, +12)`. `Cpr(0)`
    /// yields `MechHa(0)`.
    pub fn to_mech_ha(self, cpr: Cpr) -> MechHa {
        if cpr.0 == 0 {
            return MechHa(0.0);
        }
        MechHa::new((self.0 as f64) * 24.0 / (cpr.0 as f64))
    }

    /// Collapse a raw counter (or tick delta) to its canonical-band
    /// equivalent in `[-cpr/2, +cpr/2)`.
    pub fn fold_to_canonical_band(self, cpr: Cpr) -> RaTicks {
        RaTicks(fold_ticks_canonical(self.0, cpr.0))
    }
}

// ===================== Dec axis (degrees) =====================

/// Celestial declination, degrees. Normally `[-90, +90]`; the constructor
/// does **not** clamp, so a "through the pole" encoder reading can surface as
/// a magnitude past 90° for the caller to detect (matching the pre-typed
/// `dec_ticks_to_degrees` contract).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dec(f64);

impl Dec {
    /// Construct from a celestial declination in degrees.
    pub fn new(degrees: f64) -> Self {
        Self(degrees)
    }

    /// The underlying declination in degrees.
    pub fn value(self) -> f64 {
        self.0
    }

    /// Mechanical (encoder) declination for the **pre-flip** side, where the
    /// encoder degree equals the celestial declination.
    pub fn to_mech(self) -> MechDec {
        MechDec::new(self.0)
    }

    /// Mechanical (encoder) declination for the **post-flip** side: the OTA
    /// has rotated through the celestial pole, `sign(dec) * (180 - |dec|)`.
    pub fn to_mech_flipped(self) -> MechDec {
        MechDec::new(through_pole(self.0))
    }
}

/// Mechanical (encoder) declination, degrees folded to `[-180, +180)` — the
/// raw Dec-axis encoder angle, which runs past `±90°` once the axis rotates
/// through the celestial pole.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MechDec(f64);

impl MechDec {
    /// Construct from degrees, folding into `[-180, +180)`.
    pub fn new(degrees: f64) -> Self {
        Self(fold_to_signed(degrees, 360.0))
    }

    /// The underlying encoder declination in degrees `[-180, +180)`.
    pub fn value(self) -> f64 {
        self.0
    }

    /// Celestial declination for the **pre-flip** side (value preserved).
    pub fn to_dec(self) -> Dec {
        Dec::new(self.0)
    }

    /// Celestial declination for the **post-flip** side: undo the through-pole
    /// rotation, `sign(d) * (180 - |d|)`. Inverse of [`Dec::to_mech_flipped`].
    pub fn to_dec_flipped(self) -> Dec {
        Dec::new(through_pole(self.0))
    }

    /// Convert to Dec-axis encoder ticks. `Cpr(0)` yields `DecTicks(0)`.
    pub fn to_ticks(self, cpr: Cpr) -> DecTicks {
        if cpr.0 == 0 {
            return DecTicks(0);
        }
        DecTicks((self.0 * (cpr.0 as f64) / 360.0).round() as i32)
    }
}

/// Dec-axis encoder counts (signed; the firmware's raw counter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DecTicks(i32);

impl DecTicks {
    /// Construct from a raw Dec-axis encoder count.
    pub fn new(ticks: i32) -> Self {
        Self(ticks)
    }

    /// The underlying signed encoder count.
    pub fn value(self) -> i32 {
        self.0
    }

    /// Convert to a mechanical declination, folded to `[-180, +180)`. `Cpr(0)`
    /// yields `MechDec(0)`.
    pub fn to_mech_dec(self, cpr: Cpr) -> MechDec {
        if cpr.0 == 0 {
            return MechDec(0.0);
        }
        MechDec::new((self.0 as f64) * 360.0 / (cpr.0 as f64))
    }

    /// Collapse a raw counter (or tick delta) to its canonical-band
    /// equivalent in `[-cpr/2, +cpr/2)`.
    pub fn fold_to_canonical_band(self, cpr: Cpr) -> DecTicks {
        DecTicks(fold_ticks_canonical(self.0, cpr.0))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const EPS: f64 = 1e-9;
    /// GTi counts-per-revolution (`0x0037_5F00`), both axes.
    const GTI_CPR: u32 = 0x0037_5F00;

    fn cpr() -> Cpr {
        Cpr::new(GTI_CPR)
    }

    // ---- Construction / normalisation ------------------------------------

    #[test]
    fn lst_and_ra_wrap_into_24() {
        assert!((Lst::new(25.0).value() - 1.0).abs() < EPS);
        assert!((Ra::new(-1.0).value() - 23.0).abs() < EPS);
    }

    #[test]
    fn hour_angle_and_mech_ha_fold_into_signed_twelve() {
        // +13 h folds to -11 h; the wrap boundary +12 folds to -12.
        assert!((HourAngle::new(13.0).value() + 11.0).abs() < EPS);
        assert!((MechHa::new(12.0).value() + 12.0).abs() < EPS);
    }

    #[test]
    fn mech_dec_folds_into_signed_one_eighty() {
        assert!((MechDec::new(190.0).value() + 170.0).abs() < EPS);
        // The wrap boundary +180 folds to -180.
        assert!((MechDec::new(180.0).value() + 180.0).abs() < EPS);
    }

    #[test]
    fn dec_is_not_clamped_so_through_pole_is_detectable() {
        // A raw encoder reading past the pole is kept as-is.
        assert!((Dec::new(135.0).value() - 135.0).abs() < EPS);
    }

    // ---- Frame algebra: RA axis ------------------------------------------

    #[test]
    fn hour_angle_of_is_lst_minus_ra() {
        let ha = Lst::new(12.0).hour_angle_of(Ra::new(15.0));
        assert!((ha.value() + 3.0).abs() < EPS, "got {}", ha.value());
    }

    #[test]
    fn mech_ha_to_ra_round_trips_pre_flip() {
        // On the natural side mech_HA == celestial HA, so RA -> HA -> mech ->
        // RA is the identity (modulo the [0,24) fold).
        for &(mech, lst) in &[(0.0, 0.0), (3.0, 6.0), (-4.5, 18.0), (5.999, 12.0)] {
            let m = MechHa::new(mech);
            let ra = m.to_ra(Lst::new(lst));
            let back = Lst::new(lst).hour_angle_of(ra).to_mech();
            assert!(
                (back.value() - m.value()).abs() < EPS,
                "mech={mech} lst={lst} back={}",
                back.value()
            );
        }
    }

    #[test]
    fn flipped_mech_ha_is_self_inverse() {
        let m = MechHa::new(11.5);
        assert!((m.flipped().value() + 0.5).abs() < EPS);
        assert!((m.flipped().flipped().value() - m.value()).abs() < EPS);
    }

    #[test]
    fn lat45_flip_worked_example_matches_plan() {
        // Plan §2.0, lat 45°N: target HA = -0.5 -> post-flip mech_HA = +11.5;
        // dec = +45° -> post-flip mech encoder = +135°.
        let mech_flipped = HourAngle::new(-0.5).to_mech().flipped();
        assert!(
            (mech_flipped.value() - 11.5).abs() < EPS,
            "got {}",
            mech_flipped.value()
        );
        let dec_flipped = Dec::new(45.0).to_mech_flipped();
        assert!(
            (dec_flipped.value() - 135.0).abs() < EPS,
            "got {}",
            dec_flipped.value()
        );
    }

    #[test]
    fn meridian_flip_lands_at_encoder_wrap() {
        // target HA = 0 -> post-flip mech_HA = +12 -> folds to -12 (the wrap).
        let m = HourAngle::new(0.0).to_mech().flipped();
        assert!((m.value().abs() - 12.0).abs() < EPS, "got {}", m.value());
    }

    #[test]
    fn post_flip_ra_read_adds_twelve_hours() {
        // (LST - mech + 12) mod 24, vs the pre-flip (LST - mech) mod 24.
        let m = MechHa::new(2.0);
        let lst = Lst::new(5.0);
        let pre = m.to_ra(lst).value();
        let post = m.to_ra_flipped(lst).value();
        assert!(((post - pre).rem_euclid(24.0) - 12.0).abs() < EPS);
    }

    // ---- Frame algebra: Dec axis -----------------------------------------

    #[test]
    fn dec_through_pole_negative_target() {
        // dec = -45° -> post-flip mech encoder = -(180 - 45) = -135°.
        assert!((Dec::new(-45.0).to_mech_flipped().value() + 135.0).abs() < EPS);
    }

    #[test]
    fn dec_at_pole_stays_at_pole_through_flip() {
        // dec = +90° -> sign * (180 - 90) = +90°: the pole is the same
        // physical position flipped or not.
        assert!((Dec::new(90.0).to_mech_flipped().value() - 90.0).abs() < EPS);
    }

    #[test]
    fn mech_dec_to_celestial_round_trips_through_flip() {
        // The through-pole reflection is its own inverse on the Dec axis.
        for d in [45.0, -45.0, 30.0, -89.9, 0.0] {
            let back = Dec::new(d).to_mech_flipped().to_dec_flipped();
            assert!(
                (back.value() - d).abs() < EPS,
                "d={d} back={}",
                back.value()
            );
        }
    }

    // ---- Tick <-> angle conversions --------------------------------------

    #[test]
    fn ra_quarter_revolution_is_six_hours() {
        let ha = RaTicks::new((GTI_CPR / 4) as i32).to_mech_ha(cpr());
        assert!((ha.value() - 6.0).abs() < EPS, "got {}", ha.value());
    }

    #[test]
    fn dec_quarter_revolution_is_ninety_degrees() {
        let d = DecTicks::new((GTI_CPR / 4) as i32).to_mech_dec(cpr());
        assert!((d.value() - 90.0).abs() < EPS, "got {}", d.value());
    }

    #[test]
    fn cpr_zero_is_handled_defensively() {
        let zero = Cpr::new(0);
        assert_eq!(RaTicks::new(123).to_mech_ha(zero).value(), 0.0);
        assert_eq!(DecTicks::new(123).to_mech_dec(zero).value(), 0.0);
        assert_eq!(MechHa::new(6.0).to_ticks(zero).value(), 0);
        assert_eq!(MechDec::new(90.0).to_ticks(zero).value(), 0);
    }

    #[test]
    fn fold_to_canonical_band_recovers_through_wrap_counter() {
        // A through-wrap flip can leave the counter ~one revolution off; the
        // fold collapses a near-cpr delta to its near-zero equivalent.
        let raw_delta = 1_738_800 - (-1_890_000);
        let folded = RaTicks::new(raw_delta).fold_to_canonical_band(cpr());
        assert!(folded.value().abs() < 1000, "got {}", folded.value());
    }

    // ---- Property tests --------------------------------------------------

    proptest! {
        /// RA ticks within one revolution survive a tick -> mech_HA -> tick
        /// round trip exactly.
        #[test]
        fn ra_ticks_round_trip(t in -((GTI_CPR / 2) as i32)..((GTI_CPR / 2) as i32)) {
            let back = RaTicks::new(t).to_mech_ha(cpr()).to_ticks(cpr());
            prop_assert_eq!(back.value(), t);
        }

        /// Dec ticks within one revolution survive a tick -> mech_dec -> tick
        /// round trip exactly.
        #[test]
        fn dec_ticks_round_trip(t in -((GTI_CPR / 2) as i32)..((GTI_CPR / 2) as i32)) {
            let back = DecTicks::new(t).to_mech_dec(cpr()).to_ticks(cpr());
            prop_assert_eq!(back.value(), t);
        }

        /// The hour-angle fold is idempotent: a constructed value is already
        /// canonical, so re-folding it is a no-op.
        #[test]
        fn mech_ha_fold_is_idempotent(h in -1000.0f64..1000.0) {
            let once = MechHa::new(h);
            let twice = MechHa::new(once.value());
            prop_assert!((twice.value() - once.value()).abs() < EPS);
            prop_assert!(once.value() >= -12.0 && once.value() < 12.0);
        }

        /// The through-pole reflection is its own inverse for any celestial
        /// declination in the observable hemisphere.
        #[test]
        fn dec_through_pole_is_self_inverse(d in -90.0f64..90.0) {
            let back = Dec::new(d).to_mech_flipped().to_dec_flipped();
            prop_assert!((back.value() - d).abs() < 1e-6, "d={} back={}", d, back.value());
        }
    }
}
