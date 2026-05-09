//! Wire-format codec helpers.
//!
//! The protocol's one quirky feature is its 24-bit hex encoding: low byte
//! first, with each byte's nibbles in normal high-then-low order. So the
//! 24-bit value `0x123456` serialises as the ASCII string `"563412"`.
//!
//! Axis positions ride on top of that with a `+0x800000` bias so the unsigned
//! hex on the wire can carry both signed-positive and signed-negative encoder
//! counts. [`encode_position`] / [`decode_position`] handle the bias so service
//! code sees signed [`i32`] ticks only.

use crate::error::{ProtocolError, Result};

/// Position bias added on encode, subtracted on decode.
///
/// The mount carries axis positions on the wire as unsigned 24-bit values
/// offset by this constant so that encoder count `0` is wire value
/// `0x800000`, count `+1` is `0x800001`, count `-1` is `0x7FFFFF`, and so on.
pub const POSITION_BIAS: u32 = 0x0080_0000;

/// Encode a `u8` as two ASCII hex bytes (high nibble first).
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn encode_u8(_value: u8) -> [u8; 2] {
    unimplemented!("Phase 3: encode a single byte as two upper-case ASCII hex digits")
}

/// Decode two ASCII hex bytes into a `u8`.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn decode_u8(_bytes: [u8; 2]) -> Result<u8> {
    let _ = ProtocolError::HexError(String::new());
    unimplemented!("Phase 3: parse two ASCII hex digits (case-insensitive) into a u8")
}

/// Encode a 24-bit unsigned value into six ASCII hex bytes, low byte first.
///
/// Only the low 24 bits of `value` are used. For example, `0x12_3456` encodes
/// as `[b'5', b'6', b'3', b'4', b'1', b'2']`.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn encode_u24(_value: u32) -> [u8; 6] {
    unimplemented!("Phase 3: low-byte-first 24-bit hex encode")
}

/// Decode six ASCII hex bytes (low byte first) into a 24-bit unsigned value.
///
/// The returned `u32` always has its high byte zeroed.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn decode_u24(_bytes: &[u8; 6]) -> Result<u32> {
    unimplemented!("Phase 3: low-byte-first 24-bit hex decode")
}

/// Encode a signed encoder-tick value as a 24-bit wire value with the
/// `+0x800000` bias applied.
///
/// Returns [`ProtocolError::HexError`] when `ticks` is outside the
/// representable signed-24-bit range `-2^23 ..= 2^23 - 1`.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn encode_position(_ticks: i32) -> Result<[u8; 6]> {
    unimplemented!("Phase 3: bias-then-encode_u24")
}

/// Decode six ASCII hex bytes into signed encoder ticks.
///
/// The wire value is interpreted as a 24-bit unsigned integer, the
/// `0x800000` bias is subtracted, and the result is returned as a signed
/// [`i32`] in the range `-2^23 ..= 2^23 - 1`.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn decode_position(_bytes: &[u8; 6]) -> Result<i32> {
    unimplemented!("Phase 3: decode_u24-then-debias")
}

/// Verify that `frame` matches the strict UDP framing rules: starts with `:`,
/// ends with a single `\r`, and contains no junk before/after.
///
/// Used by the UDP transport on receive. Serial transports buffer up to the
/// first `\r` and pass exactly the resulting slice in here.
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn validate_frame(_frame: &[u8]) -> Result<()> {
    unimplemented!("Phase 3: enforce one well-formed `:...\\r` frame, nothing trailing")
}
