//! Small shared helpers for reading values out of the raw ZWO SDK FFI.
//!
//! Only needed on the real-FFI path; the `simulation` backend fabricates its
//! values directly, so this module is compiled out under that feature.

/// Read a fixed-size, NUL-terminated C `char` buffer into an owned [`String`]
/// (lossy on invalid UTF-8). Portable across `c_char` signedness.
pub(crate) fn c_string_field(buf: &[std::os::raw::c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Format an 8-byte hardware id as a 16-character lowercase hex string.
pub(crate) fn hex8(bytes: &[u8; 8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
