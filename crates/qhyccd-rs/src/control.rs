#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
/// A QHYCCD control (`CONTROL_ID`) addressed by `is_control_available`,
/// `get_parameter`, `set_parameter`, and `get_parameter_min_max_step`.
///
/// Only the controls this workspace actually touches are named; every other
/// SDK `CONTROL_ID` is preserved as [`ControlType::Other`] carrying its raw
/// value. This mirrors the `zwo-rs` / `svbony-rs` `ControlType` shape (a small
/// semantic subset plus an `Other` escape) rather than transcribing the SDK's
/// full ~90-entry enum. Documentation is taken from the QHYCCD SDK
/// (<https://www.qhyccd.cn/file/repository/publish/SDK/code/QHYCCD%20SDK_API_EN_V2.3.pdf>).
///
/// The named variants are no longer `#[repr]`-discriminated (a data-carrying
/// enum cannot be `as u32`-cast); use [`ControlType::to_raw`] to obtain the SDK
/// `CONTROL_ID`, whose numeric values still match the SDK's own numbering
/// (`Gain == 6`, `CfwPort == 17`, …).
pub enum ControlType {
    /// `CONTROL_BRIGHTNESS` (0) — image brightness.
    Brightness,
    /// `CONTROL_WBR` (2) — red white balance.
    Wbr,
    /// `CONTROL_WBB` (3) — blue white balance.
    Wbb,
    /// `CONTROL_WBG` (4) — green white balance.
    Wbg,
    /// `CONTROL_GAIN` (6) — sensor gain.
    Gain,
    /// `CONTROL_OFFSET` (7) — sensor offset (black level).
    Offset,
    /// `CONTROL_EXPOSURE` (8) — exposure time in microseconds.
    Exposure,
    /// `CONTROL_SPEED` (9) — readout speed.
    Speed,
    /// `CONTROL_TRANSFERBIT` (10) — USB transfer bit depth (8 or 16).
    TransferBit,
    /// `CONTROL_USBTRAFFIC` (12) — USB traffic / bandwidth control.
    UsbTraffic,
    /// `CONTROL_CURTEMP` (14) — current sensor temperature (°C).
    CurTemp,
    /// `CONTROL_CURPWM` (15) — current cooler PWM (0–255).
    CurPWM,
    /// `CONTROL_MANULPWM` (16) — manual cooler PWM set-point (0–255).
    ManualPWM,
    /// `CONTROL_CFWPORT` (17) — filter-wheel position (ASCII-offset value).
    CfwPort,
    /// `CONTROL_COOLER` (18) — auto-cooling target temperature (°C).
    Cooler,
    /// `CONTROL_CAM_COLOR` (20) — Bayer matrix / colour support.
    CamColor,
    /// `CAM_BIN1X1MODE` (21) — 1×1 binning support.
    CamBin1x1mode,
    /// `CAM_BIN2X2MODE` (22) — 2×2 binning support.
    CamBin2x2mode,
    /// `CAM_BIN3X3MODE` (23) — 3×3 binning support.
    CamBin3x3mode,
    /// `CAM_BIN4X4MODE` (24) — 4×4 binning support.
    CamBin4x4mode,
    /// `CAM_MECHANICALSHUTTER` (25) — mechanical-shutter presence.
    CamMechanicalShutter,
    /// `CAM_8BITS` (34) — 8-bit image output support.
    Cam8bits,
    /// `CAM_16BITS` (35) — 16-bit image output support.
    Cam16bits,
    /// `CONTROL_CFWSLOTSNUM` (44) — number of filter-wheel slots.
    CfwSlotsNum,
    /// `CONTROL_DDR` (48) — DDR frame buffer support (live-mode capture).
    DDR,
    /// `OutputDataActualBits` (55) — actual bit depth of the output data.
    OutputDataActualBits,
    /// `CAM_SINGLEFRAMEMODE` (57) — single-frame capture support.
    CamSingleFrameMode,
    /// `CAM_LIVEVIDEOMODE` (58) — live-video capture support.
    CamLiveVideoMode,
    /// `CAM_IS_COLOR` (59) — colour-sensor flag.
    CamIsColor,
    /// `CAM_BIN6X6MODE` (75) — 6×6 binning support.
    CamBin6x6mode,
    /// `CAM_BIN8X8MODE` (76) — 8×8 binning support.
    CamBin8x8mode,
    /// Any control outside the subset named above; carries the raw SDK
    /// `CONTROL_ID`.
    Other(i32),
}

impl ControlType {
    /// The SDK `CONTROL_ID` for this control. The numeric values match the
    /// QHYCCD SDK's own numbering (forced fact #3 of the convention plan).
    ///
    /// Only the real FFI arms (compiled without `simulation`) and the round-trip
    /// test consume this — the simulated backend keys controls by `ControlType`
    /// directly — so it is dead code on a `simulation` non-test build.
    #[must_use]
    #[cfg_attr(feature = "simulation", allow(dead_code))]
    pub(crate) fn to_raw(self) -> u32 {
        match self {
            Self::Brightness => 0,
            Self::Wbr => 2,
            Self::Wbb => 3,
            Self::Wbg => 4,
            Self::Gain => 6,
            Self::Offset => 7,
            Self::Exposure => 8,
            Self::Speed => 9,
            Self::TransferBit => 10,
            Self::UsbTraffic => 12,
            Self::CurTemp => 14,
            Self::CurPWM => 15,
            Self::ManualPWM => 16,
            Self::CfwPort => 17,
            Self::Cooler => 18,
            Self::CamColor => 20,
            Self::CamBin1x1mode => 21,
            Self::CamBin2x2mode => 22,
            Self::CamBin3x3mode => 23,
            Self::CamBin4x4mode => 24,
            Self::CamMechanicalShutter => 25,
            Self::Cam8bits => 34,
            Self::Cam16bits => 35,
            Self::CfwSlotsNum => 44,
            Self::DDR => 48,
            Self::OutputDataActualBits => 55,
            Self::CamSingleFrameMode => 57,
            Self::CamLiveVideoMode => 58,
            Self::CamIsColor => 59,
            Self::CamBin6x6mode => 75,
            Self::CamBin8x8mode => 76,
            Self::Other(v) => v as u32,
        }
    }

    /// The [`ControlType`] for an SDK `CONTROL_ID` — the inverse of
    /// [`Self::to_raw`]. Unnamed ids round-trip through [`ControlType::Other`].
    /// Only the round-trip test consumes this today (no runtime path converts a
    /// raw id back to a `ControlType`), so it is test-only.
    #[cfg(test)]
    #[must_use]
    fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Brightness,
            2 => Self::Wbr,
            3 => Self::Wbb,
            4 => Self::Wbg,
            6 => Self::Gain,
            7 => Self::Offset,
            8 => Self::Exposure,
            9 => Self::Speed,
            10 => Self::TransferBit,
            12 => Self::UsbTraffic,
            14 => Self::CurTemp,
            15 => Self::CurPWM,
            16 => Self::ManualPWM,
            17 => Self::CfwPort,
            18 => Self::Cooler,
            20 => Self::CamColor,
            21 => Self::CamBin1x1mode,
            22 => Self::CamBin2x2mode,
            23 => Self::CamBin3x3mode,
            24 => Self::CamBin4x4mode,
            25 => Self::CamMechanicalShutter,
            34 => Self::Cam8bits,
            35 => Self::Cam16bits,
            44 => Self::CfwSlotsNum,
            48 => Self::DDR,
            55 => Self::OutputDataActualBits,
            57 => Self::CamSingleFrameMode,
            58 => Self::CamLiveVideoMode,
            59 => Self::CamIsColor,
            75 => Self::CamBin6x6mode,
            76 => Self::CamBin8x8mode,
            other => Self::Other(other),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::ControlType;

    /// Every named variant round-trips through its raw SDK `CONTROL_ID`, so the
    /// two hand-written `to_raw`/`from_raw` matches can never silently drift.
    #[test]
    fn named_variants_round_trip() {
        let named = [
            ControlType::Brightness,
            ControlType::Wbr,
            ControlType::Wbb,
            ControlType::Wbg,
            ControlType::Gain,
            ControlType::Offset,
            ControlType::Exposure,
            ControlType::Speed,
            ControlType::TransferBit,
            ControlType::UsbTraffic,
            ControlType::CurTemp,
            ControlType::CurPWM,
            ControlType::ManualPWM,
            ControlType::CfwPort,
            ControlType::Cooler,
            ControlType::CamColor,
            ControlType::CamBin1x1mode,
            ControlType::CamBin2x2mode,
            ControlType::CamBin3x3mode,
            ControlType::CamBin4x4mode,
            ControlType::CamMechanicalShutter,
            ControlType::Cam8bits,
            ControlType::Cam16bits,
            ControlType::CfwSlotsNum,
            ControlType::DDR,
            ControlType::OutputDataActualBits,
            ControlType::CamSingleFrameMode,
            ControlType::CamLiveVideoMode,
            ControlType::CamIsColor,
            ControlType::CamBin6x6mode,
            ControlType::CamBin8x8mode,
        ];
        for control in named {
            assert_eq!(ControlType::from_raw(control.to_raw() as i32), control);
        }
    }

    /// A control id outside the named subset survives as `Other`, and the key
    /// SDK ids keep their documented numeric values.
    #[test]
    fn unnamed_id_survives_as_other() {
        assert_eq!(ControlType::from_raw(1), ControlType::Other(1));
        assert_eq!(ControlType::Other(1).to_raw(), 1);
        assert_eq!(ControlType::Gain.to_raw(), 6);
        assert_eq!(ControlType::CfwPort.to_raw(), 17);
        assert_eq!(ControlType::CamBin8x8mode.to_raw(), 76);
    }
}
