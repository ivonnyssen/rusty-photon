//! Frame codec for the Sky-Watcher motor-controller wire protocol.
//!
//! The [`SkywatcherCodec`] is the per-service plug that
//! [`rusty_photon_shared_transport::SharedTransport`] uses to translate
//! between typed [`Command`]s and on-wire frames. The wire protocol is
//! `:cmd<axis><payload?>\r` for requests and `=<body>\r` /
//! `!<code>\r` for responses; framing terminator (`\r` on serial,
//! datagram on UDP) belongs to the [`FrameTransport`] implementation.
//!
//! # Why `Response = Vec<u8>`
//!
//! The protocol crate's [`Response::decode`] needs the originating
//! [`Command`] to disambiguate replies that share a wire shape — a
//! 6-hex-byte success body decodes as [`Response::U24`] for `:a` (CPR
//! inquiry) but as [`Response::Position`] for `:j` (position inquiry).
//! The shared crate's [`Codec::decode`] signature is `(&self, &[u8]) ->
//! Result<Response, Error>` with no command in scope, so we can't do
//! the typed decode at the codec layer.
//!
//! Instead the codec validates frame structure (must start with `=` or
//! `!`, end with `\r`) and returns the raw frame bytes verbatim. The
//! [`crate::MountManager::send`] wrapper then calls the protocol
//! crate's [`Response::decode`] with the originating command, yielding
//! the typed [`Response`] all existing call sites already expect.
//!
//! [`FrameTransport`]: rusty_photon_shared_transport::FrameTransport
//! [`Response`]: skywatcher_motor_protocol::Response
//! [`Response::decode`]: skywatcher_motor_protocol::Response::decode
//! [`Response::U24`]: skywatcher_motor_protocol::Response::U24
//! [`Response::Position`]: skywatcher_motor_protocol::Response::Position
//! [`Codec::decode`]: rusty_photon_shared_transport::Codec::decode

use rusty_photon_shared_transport::{Codec, SessionError, TransportError};
use skywatcher_motor_protocol::codec::validate_response_frame;
use skywatcher_motor_protocol::{Command, ProtocolError, Response};
use thiserror::Error;

use crate::error::StarAdvError;

/// Codec-side error type. Mirrors the variants the codec layer can
/// surface (frame-structure violations) and the protocol-crate errors
/// the typed-decode step produces in [`crate::MountManager::send`].
#[derive(Debug, Error)]
pub enum SkywatcherCodecError {
    /// The frame failed the wire-format check in
    /// [`validate_response_frame`] — wrong prefix byte, missing `\r`
    /// terminator, non-ASCII hex in the body, etc.
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
}

/// Zero-sized adapter that plugs the Sky-Watcher wire protocol into
/// [`rusty_photon_shared_transport::SharedTransport`].
///
/// Stateless — `Clone` is required by the [`Codec`] trait (the shared
/// transport stamps a fresh codec onto every new `Connection`) and is
/// trivially satisfied by `Copy`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SkywatcherCodec;

impl Codec for SkywatcherCodec {
    type Command = Command;
    type Response = Vec<u8>;
    type Error = SkywatcherCodecError;

    fn encode(&self, cmd: &Self::Command) -> Vec<u8> {
        // `Command::encode` only fails on out-of-range numeric arguments —
        // every command construction in this crate is checked at the
        // type level (positions go through `encode_position`, etc.), so
        // an encode error here is a driver bug rather than a runtime
        // condition. Surfacing the panic immediately is preferable to
        // a `SessionError::Codec` that the caller would have to
        // interpret as "wire problem."
        cmd.encode()
            .expect("command encoding must succeed for typed commands")
    }

    fn decode(&self, bytes: &[u8]) -> Result<Self::Response, Self::Error> {
        // Normalize wire quirks the legacy bespoke transports used to
        // smooth over before the shared `FrameTransport` adapters took
        // over framing:
        //
        // * **Trailing `\n` after `\r` on UDP** — some firmware
        //   revisions append `\n` to the `\r` terminator on UDP
        //   replies; the reference doc lists this as tolerated. The
        //   shared `UdpFrameTransport` returns the whole datagram
        //   verbatim, so the `\n` survives into the codec and would
        //   fail `validate_response_frame` (which requires `\r` as
        //   the last byte).
        // * **Leading junk before the first `=`/`!` on serial** — the
        //   mount occasionally emits framing junk between frames; the
        //   legacy `SerialTransport` skipped non-`=`/`!` bytes until
        //   it found a real start. The shared `SerialFrameTransport`
        //   reads until `\r` and returns the bytes verbatim, so any
        //   leading junk lands at the head of the frame and would
        //   fail `validate_response_frame`.
        //
        // Normalize once, here, so the typed-decode boundary in
        // `MountManager::send` sees a clean `=...\r` / `!...\r` frame.
        let normalized = normalize_response_frame(bytes);
        validate_response_frame(normalized)?;
        Ok(normalized.to_vec())
    }

    // `matches`: default true. The Sky-Watcher protocol is strictly
    // request/response with no unsolicited frames — every recv after a
    // send is by definition the response to that send.
    //
    // `max_skip`: default 0. Same reasoning: no unsolicited frames to
    // skip.
}

/// Strip leading non-frame-start bytes and a trailing `\n` from a raw
/// response frame so the shared `FrameTransport` adapters don't have to
/// know about protocol-specific framing quirks.
///
/// * **Leading skip:** advance past any bytes that aren't `=` or `!`.
///   The Sky-Watcher firmware occasionally emits framing junk between
///   frames (the legacy `SerialTransport`'s read loop dropped these
///   before the `=`/`!` prefix). If no `=`/`!` is found the whole
///   buffer is returned untouched so `validate_response_frame` can
///   surface a meaningful error.
/// * **Trailing `\n` strip:** if the last two bytes are `\r\n`, drop
///   the `\n`. Some firmware revisions on UDP append `\n` after the
///   `\r` terminator; the reference doc lists this as tolerated.
fn normalize_response_frame(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|&b| b == b'=' || b == b'!')
        .unwrap_or(0);
    let mut end = bytes.len();
    if end >= 2 && bytes[end - 2] == b'\r' && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    &bytes[start..end]
}

/// Decode a raw frame against the command that produced it.
///
/// Companion to [`SkywatcherCodec`] used by [`crate::MountManager::send`]
/// (and by the connect-time handshake) to turn a [`Codec::Response`]
/// (the raw frame bytes) into the protocol-crate's typed [`Response`]
/// after the request has come back over the wire.
pub fn decode_frame_for(cmd: &Command, frame: &[u8]) -> Result<Response, SkywatcherCodecError> {
    Response::decode(frame, cmd).map_err(SkywatcherCodecError::Protocol)
}

impl From<SessionError<SkywatcherCodecError>> for StarAdvError {
    fn from(err: SessionError<SkywatcherCodecError>) -> Self {
        match err {
            SessionError::Transport(TransportError::Open(e)) => {
                StarAdvError::ConnectionFailed(e.to_string())
            }
            SessionError::Transport(TransportError::Io(e)) => StarAdvError::Io(e),
            SessionError::Transport(TransportError::Timeout(d)) => {
                StarAdvError::Timeout(format!("transport timeout after {d:?}"))
            }
            SessionError::Transport(TransportError::Eof) => {
                StarAdvError::Transport("connection closed".to_string())
            }
            SessionError::Transport(TransportError::Framing(s)) => {
                StarAdvError::Transport(format!("framing: {s}"))
            }
            SessionError::Codec(SkywatcherCodecError::Protocol(pe)) => StarAdvError::Protocol(pe),
            SessionError::SkipExhausted(n) => StarAdvError::Transport(format!(
                "device returned non-matching response ({n} frame{s} read)",
                s = if n == 1 { "" } else { "s" }
            )),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use skywatcher_motor_protocol::Axis;

    #[test]
    fn encode_initialize_emits_colon_prefixed_carriage_return_terminated_frame() {
        let bytes = SkywatcherCodec.encode(&Command::Initialize(Axis::Ra));
        assert_eq!(&bytes, b":F1\r");
    }

    #[test]
    fn encode_inquire_position_includes_axis_byte() {
        let bytes = SkywatcherCodec.encode(&Command::InquirePosition(Axis::Dec));
        assert_eq!(&bytes, b":j2\r");
    }

    #[test]
    fn decode_passes_through_well_formed_success_frame() {
        let frame = SkywatcherCodec.decode(b"=000080\r").unwrap();
        assert_eq!(frame, b"=000080\r");
    }

    #[test]
    fn decode_passes_through_well_formed_error_frame() {
        let frame = SkywatcherCodec.decode(b"!00\r").unwrap();
        assert_eq!(frame, b"!00\r");
    }

    #[test]
    fn decode_rejects_missing_terminator() {
        let err = SkywatcherCodec.decode(b"=000080").unwrap_err();
        assert!(matches!(err, SkywatcherCodecError::Protocol(_)));
    }

    #[test]
    fn decode_rejects_bad_prefix() {
        // `?` is neither `=` nor `!` — leading-skip walks the buffer
        // and finds no frame start, so validation sees the original
        // `?` and rejects it.
        let err = SkywatcherCodec.decode(b"?000080\r").unwrap_err();
        assert!(matches!(err, SkywatcherCodecError::Protocol(_)));
    }

    #[test]
    fn decode_strips_trailing_newline_on_udp_style_frame() {
        // Some firmware revisions append `\n` to the `\r` terminator
        // on UDP replies; the codec strips it so the rest of the
        // pipeline sees a clean `=000080\r`. Without normalization,
        // `validate_response_frame` would reject this for the wrong
        // terminator byte.
        let frame = SkywatcherCodec.decode(b"=000080\r\n").unwrap();
        assert_eq!(frame, b"=000080\r");
    }

    #[test]
    fn decode_skips_leading_junk_before_frame_start() {
        // The Sky-Watcher firmware occasionally emits framing junk
        // (stray `\n`, residue from a prior frame) between responses;
        // the legacy SerialTransport's read loop dropped these before
        // the `=`/`!` prefix. The codec must do the same so the
        // typed-decode boundary in MountManager::send sees a clean
        // frame.
        let frame = SkywatcherCodec.decode(b"\n=000080\r").unwrap();
        assert_eq!(frame, b"=000080\r");
    }

    #[test]
    fn decode_skips_leading_junk_then_error_prefix() {
        // The leading-junk skip looks for the first `=` *or* `!` —
        // error frames must survive normalization just as cleanly as
        // success frames.
        let frame = SkywatcherCodec.decode(b"\x00!00\r").unwrap();
        assert_eq!(frame, b"!00\r");
    }

    #[test]
    fn decode_handles_both_leading_junk_and_trailing_newline_together() {
        let frame = SkywatcherCodec.decode(b"\n=000080\r\n").unwrap();
        assert_eq!(frame, b"=000080\r");
    }

    #[test]
    fn normalize_passes_through_clean_frame_unchanged() {
        assert_eq!(normalize_response_frame(b"=000080\r"), b"=000080\r");
        assert_eq!(normalize_response_frame(b"!00\r"), b"!00\r");
    }

    #[test]
    fn normalize_leaves_no_frame_start_alone() {
        // If no `=`/`!` is found, return the whole buffer so
        // `validate_response_frame` surfaces a meaningful error rather
        // than the helper silently returning an empty slice.
        assert_eq!(normalize_response_frame(b"garbage"), b"garbage");
    }

    #[test]
    fn decode_frame_for_inquire_position_returns_signed_position() {
        // `:j1` returns biased 6-hex u24; `decode_frame_for` debias to i32.
        let resp = decode_frame_for(&Command::InquirePosition(Axis::Ra), b"=000080\r").unwrap();
        match resp {
            Response::Position(p) => assert_eq!(p, 0),
            other => panic!("expected Position(0), got {other:?}"),
        }
    }

    #[test]
    fn decode_frame_for_inquire_cpr_returns_unsigned_u24() {
        // `:a1` returns unsigned 6-hex u24 — same wire shape as Position
        // but a different decode.
        let resp = decode_frame_for(&Command::InquireCpr(Axis::Ra), b"=005F37\r").unwrap();
        assert_eq!(resp, Response::U24(0x0037_5F00));
    }

    #[test]
    fn decode_frame_for_error_reply_maps_to_protocol_error() {
        let err = decode_frame_for(&Command::Initialize(Axis::Ra), b"!00\r").unwrap_err();
        let SkywatcherCodecError::Protocol(pe) = err;
        assert!(matches!(pe, ProtocolError::MountError(_)));
    }

    #[test]
    fn session_error_transport_open_maps_to_connection_failed() {
        let err: SessionError<SkywatcherCodecError> =
            SessionError::Transport(TransportError::Open(std::io::Error::other("device busy")));
        match StarAdvError::from(err) {
            StarAdvError::ConnectionFailed(s) => assert!(s.contains("device busy")),
            other => panic!("expected ConnectionFailed, got {other:?}"),
        }
    }

    #[test]
    fn session_error_transport_timeout_maps_to_timeout() {
        let err: SessionError<SkywatcherCodecError> =
            SessionError::Transport(TransportError::Timeout(std::time::Duration::from_secs(2)));
        match StarAdvError::from(err) {
            StarAdvError::Timeout(_) => {}
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn session_error_codec_protocol_passes_through() {
        let err: SessionError<SkywatcherCodecError> = SessionError::Codec(
            SkywatcherCodecError::Protocol(ProtocolError::FrameError("bad".to_string())),
        );
        assert!(matches!(StarAdvError::from(err), StarAdvError::Protocol(_)));
    }

    #[test]
    fn session_error_skip_exhausted_maps_to_transport() {
        let err: SessionError<SkywatcherCodecError> = SessionError::SkipExhausted(3);
        match StarAdvError::from(err) {
            StarAdvError::Transport(s) => {
                assert!(s.contains("3") && s.contains("non-matching"));
            }
            other => panic!("expected Transport, got {other:?}"),
        }
    }
}
