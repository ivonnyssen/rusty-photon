//! Sky-Watcher mount-type identification.
//!
//! The `:e<axis>` (motor-board-version) reply is the only Sky-Watcher command
//! that meaningfully identifies the device on the wire. Its 24-bit payload
//! packs the mount-type ID in the high byte and the firmware-version major /
//! minor in the mid / low bytes:
//!
//! ```text
//! 0x03_30_0C
//!   ^^         mount-type ID (0x03 = EQ family, includes the Star Adventurer GTi)
//!      ^^      firmware version major (0x30)
//!         ^^   firmware version minor (0x0C)
//! ```
//!
//! [`MountType::from_motor_board_version`] is the whitelist gate used by the
//! `star-adventurer-gti` driver's connect handshake to refuse to talk to a
//! device that isn't a Sky-Watcher motor controller before any mount-specific
//! command (`:F`, `:a`, `:b`, `:g`, …) goes on the wire. See
//! [issue #254][issue] for the hardware session that motivated this.
//!
//! [issue]: https://github.com/ivonnyssen/rusty-photon/issues/254

/// Sky-Watcher motor-controller mount-type families, keyed off the high byte
/// of the `:e` motor-board-version reply.
///
/// The byte values are documented in the Sky-Watcher motor-controller command
/// set and cross-checked against the INDI `indi-eqmod` reference driver.
/// Variants are named after the mount family rather than a specific model
/// because the firmware byte does not distinguish between e.g. an EQ3 and an
/// EQ5 — both report `0x03` / `0x02` from the same firmware build.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MountType {
    /// `0x00` — EQ6 / EQ6 Pro German Equatorial.
    Eq6,
    /// `0x01` — HEQ5 / HEQ5 Pro German Equatorial.
    Heq5,
    /// `0x02` — EQ5 / EQ5 Pro German Equatorial.
    Eq5,
    /// `0x03` — EQ3 / EQ3-2 / Star Adventurer GTi German Equatorial.
    /// The Star Adventurer GTi reports as this family; see the hardware probe
    /// table in `docs/references/skywatcher-motor-controller-command-set.md`.
    Eq3,
    /// `0x04` — EQ8 German Equatorial.
    Eq8,
    /// `0x05` — AZ-EQ6 dual-mode (GEM + AltAz).
    AzEq6,
    /// `0x06` — AZ-EQ5 dual-mode (GEM + AltAz).
    AzEq5,
    /// `0x80` — Star Adventurer (the original single-axis tracker, not the GTi).
    StarAdventurer,
    /// `0x82` — AZ-GTi / Star Adventurer GTi (AltAz firmware variant).
    AzGti,
}

impl MountType {
    /// Extract the mount-type byte (high byte of the 24-bit value) from a
    /// `:e` reply and look it up against the whitelist.
    ///
    /// `version` is the [`crate::Response::U24`] payload of the
    /// [`crate::Command::InquireMotorBoardVersion`] reply, with the codec's
    /// low-byte-first hex decoding (see [`crate::codec::decode_u24`])
    /// already applied — i.e. for the GTi probe the wire reply
    /// `=0C3003\r` decodes to `0x0003_300C`, which is what the caller
    /// passes in.
    ///
    /// Returns `Ok(MountType)` when the high byte is in the whitelist;
    /// returns `Err(byte)` carrying the unrecognised mount-type byte
    /// otherwise so the driver can quote it in operator-facing diagnostics.
    pub fn from_motor_board_version(version: u32) -> Result<Self, u8> {
        let mount_id = ((version >> 16) & 0xFF) as u8;
        match mount_id {
            0x00 => Ok(Self::Eq6),
            0x01 => Ok(Self::Heq5),
            0x02 => Ok(Self::Eq5),
            0x03 => Ok(Self::Eq3),
            0x04 => Ok(Self::Eq8),
            0x05 => Ok(Self::AzEq6),
            0x06 => Ok(Self::AzEq5),
            0x80 => Ok(Self::StarAdventurer),
            0x82 => Ok(Self::AzGti),
            other => Err(other),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn gti_probe_value_decodes_to_eq3_family() {
        // The Star Adventurer GTi probe value (documented in
        // docs/references/skywatcher-motor-controller-command-set.md) is
        // 0x03_30_0C — the value the driver must accept on every connect.
        assert_eq!(
            MountType::from_motor_board_version(0x0003_300C).unwrap(),
            MountType::Eq3
        );
    }

    #[test]
    fn whitelisted_high_bytes_decode_to_named_variants() {
        for (version, expected) in [
            (0x0000_0000_u32, MountType::Eq6),
            (0x0001_FFFF, MountType::Heq5),
            (0x0002_0000, MountType::Eq5),
            (0x0003_0000, MountType::Eq3),
            (0x0004_0000, MountType::Eq8),
            (0x0005_0000, MountType::AzEq6),
            (0x0006_0000, MountType::AzEq5),
            (0x0080_0000, MountType::StarAdventurer),
            (0x0082_0000, MountType::AzGti),
        ] {
            assert_eq!(
                MountType::from_motor_board_version(version).unwrap(),
                expected,
                "version=0x{version:08X}"
            );
        }
    }

    #[test]
    fn firmware_bytes_do_not_affect_lookup() {
        // The mid + low bytes are firmware major/minor and must not gate the
        // whitelist; only the high byte (mount-type ID) is consulted.
        for low_bytes in [0x0000_u32, 0xABCD, 0xFFFF, 0x300C] {
            let v = 0x0003_0000 | low_bytes;
            assert_eq!(
                MountType::from_motor_board_version(v).unwrap(),
                MountType::Eq3,
                "version=0x{v:08X}"
            );
        }
    }

    #[test]
    fn unknown_mount_type_byte_surfaces_through_err() {
        // 0x07 is the gap between the EQ8 family (0x04..=0x06) and the AZ
        // family (0x80..) per the documented byte assignments. A reply with
        // this byte must be rejected so the driver doesn't proceed to issue
        // mount-specific commands against an unknown device.
        let err = MountType::from_motor_board_version(0x0007_0000).unwrap_err();
        assert_eq!(err, 0x07);

        // The QHY focuser misroute that motivated issue #254 returned data
        // that — were the bytes shuffled to look like a `:e` reply — would
        // decode to something unlike any Sky-Watcher mount-type ID. Pick a
        // plausible "wrong device" high byte and confirm it's rejected.
        let err = MountType::from_motor_board_version(0x00FF_0000).unwrap_err();
        assert_eq!(err, 0xFF);
    }

    #[test]
    fn only_the_high_byte_is_consulted_for_rejection() {
        // A version whose high byte is unknown must reject regardless of how
        // sensible the firmware bytes look. Symmetric to the
        // `firmware_bytes_do_not_affect_lookup` test for the accept path.
        assert_eq!(
            MountType::from_motor_board_version(0x0099_300C).unwrap_err(),
            0x99
        );
    }
}
