//! Small shared helpers for reading values out of the raw ToupTek SDK FFI.
//!
//! Only needed on the real-FFI path; the `simulation` backend fabricates its
//! values directly, so this module is compiled out under that feature.

/// Read a fixed-size, NUL-terminated C `char` buffer into an owned [`String`]
/// (lossy on invalid UTF-8). Portable across `c_char` signedness.
pub(crate) fn c_string_field(buf: &[std::os::raw::c_char]) -> String {
    // `c_char` is `i8` on some targets (x86_64) and `u8` on others (aarch64): the
    // `as u8` reinterpret is required on the former and an identity on the latter,
    // so allow the `unnecessary_cast` lint that fires only on the `u8` targets.
    #[allow(clippy::unnecessary_cast)]
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}
