//! Small shared helpers for reading values out of the raw ToupTek SDK FFI.
//!
//! Only needed on the real-FFI path; the `simulation` backend fabricates its
//! values directly, so this module is compiled out under that feature.
//!
//! ## Per-platform string width
//!
//! The ToupTek SDK uses **UNICODE `wchar_t` (UTF-16) strings on Windows** but
//! plain `char` (UTF-8) strings on Linux/macOS for the enumeration fields
//! (`ToupcamDeviceV2.id` / `.displayname` and `ToupcamModelV2.name`) — see the
//! `#if defined(_WIN32)` blocks in `toupcam.h`. `bindgen` therefore types those
//! fields as `u16` on Windows and `c_char` elsewhere, so the readers below are
//! cfg-split to decode the matching encoding.

/// Read a fixed-size, NUL-terminated SDK string field into an owned [`String`].
///
/// Non-Windows: the field is `char`, decoded as UTF-8 (lossy on invalid bytes).
/// Portable across `c_char` signedness (`i8` on x86_64, `u8` on aarch64).
#[cfg(not(windows))]
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

/// Windows: the field is UNICODE `wchar_t` (`u16`), decoded as UTF-16 (lossy on
/// unpaired surrogates).
#[cfg(windows)]
pub(crate) fn c_string_field(buf: &[u16]) -> String {
    let units: Vec<u16> = buf.iter().copied().take_while(|&c| c != 0).collect();
    String::from_utf16_lossy(&units)
}

/// Read a NUL-terminated SDK string from a raw pointer into an owned [`String`]
/// (`char*` on Linux/macOS, `wchar_t*` on Windows — see [`c_string_field`]).
///
/// # Safety
/// `ptr` must be non-null and point to a NUL-terminated string that stays valid
/// for the duration of the call (the SDK owns the model strings for the lifetime
/// of enumeration).
#[cfg(not(windows))]
pub(crate) unsafe fn c_string_from_ptr(ptr: *const std::os::raw::c_char) -> String {
    std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

/// Windows: walk the `wchar_t*` to its NUL terminator, then UTF-16-decode it.
///
/// # Safety
/// Same contract as the non-Windows reader above.
#[cfg(windows)]
pub(crate) unsafe fn c_string_from_ptr(ptr: *const u16) -> String {
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
}
