#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Sky-Watcher motor-controller wire protocol codec.
//!
//! Pure encoder/decoder for the request/response ASCII protocol described in
//! the [Sky-Watcher motor-controller command set] PDF. The same wire protocol
//! runs on USB-CDC serial (9600 or 115200 8N1) and on UDP/11880 (mount in WiFi
//! AP mode); this crate is transport-agnostic.
//!
//! Two implementation gotchas the codec isolates from callers:
//!
//! * **24-bit values are sent low byte first.** The 24-bit value `0x123456`
//!   serialises as ASCII `"563412"`. Use [`codec::encode_u24`] /
//!   [`codec::decode_u24`] (and the `_i24` variants) — never roll a hex
//!   translation by hand.
//! * **Axis positions carry a `+0x800000` bias** so the unsigned hex on the
//!   wire can carry both signed-positive and signed-negative encoder counts.
//!   Use [`codec::encode_position`] / [`codec::decode_position`]; service
//!   code only ever sees signed [`i32`] encoder ticks.
//!
//! UDP framing is unforgiving: exactly one well-formed `:cmd<axis><payload>\r`
//! frame per packet, nothing trailing or leading. The codec enforces this on
//! decode.
//!
//! [Sky-Watcher motor-controller command set]: https://inter-static.skywatcher.com/downloads/skywatcher_motor_controller_command_set.pdf

pub mod codec;
pub mod command;
pub mod error;
pub mod response;

pub use command::{Axis, Command, MotionMode};
pub use error::{ProtocolError, Result};
pub use response::{AxisStatus, Response};
