//! [`Codec`] impl for the Deep Sky Dad FP2.
//!
//! Each FP2 request gets exactly one response (no unsolicited frames like
//! `qhy-focuser`'s position updates), so the default `matches` / `max_skip`
//! suffice. Encoding emits the literal `[CMD]` bytes; decoding strips the
//! enclosing `(`/`)` and returns a [`RawResponse`].
//!
//! Per-command interpretation of the response body lives on `RawResponse`
//! (`parse_ok` / `parse_int` / `parse_firmware` / ...). The codec stays
//! command-agnostic, so the shared-transport `Codec::decode` contract — one
//! function, one return type — is honoured.

use rusty_photon_shared_transport::Codec;

use crate::error::DsdFp2Error;
use crate::protocol::{Command, RawResponse};

/// Zero-sized FP2 codec. Cheap to clone (the trait requires `Clone`); the
/// shared transport stamps a fresh copy onto every new connection.
#[derive(Debug, Clone, Copy, Default)]
pub struct Fp2Codec;

impl Codec for Fp2Codec {
    type Command = Command;
    type Response = RawResponse;
    type Error = DsdFp2Error;

    fn encode(&self, cmd: &Self::Command) -> Vec<u8> {
        cmd.encode().into_bytes()
    }

    fn decode(&self, bytes: &[u8]) -> Result<RawResponse, DsdFp2Error> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| DsdFp2Error::MalformedResponse(format!("non-UTF8 frame ({e})")))?;
        RawResponse::from_frame(text)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn encode_uses_command_encoding() {
        let codec = Fp2Codec;
        assert_eq!(codec.encode(&Command::GetFirmware), b"[GFRM]");
        assert_eq!(codec.encode(&Command::SetBrightness(7)), b"[SLBR0007]");
        assert_eq!(codec.encode(&Command::SetLight(true)), b"[SLON1]");
    }

    #[test]
    fn decode_strips_parens() {
        let codec = Fp2Codec;
        let r = codec.decode(b"(OK)").unwrap();
        assert_eq!(r.body, "OK");
        let r = codec
            .decode(b"(Board=DeepSkyDad.FP2, Version=1.0.14.2)")
            .unwrap();
        assert!(r.body.starts_with("Board="));
    }

    #[test]
    fn decode_rejects_non_utf8() {
        let codec = Fp2Codec;
        let err = codec.decode(&[0xC0, 0xC1, 0xFF]).unwrap_err();
        assert!(matches!(err, DsdFp2Error::MalformedResponse(_)));
    }

    #[test]
    fn decode_rejects_unframed_bytes() {
        let codec = Fp2Codec;
        let err = codec.decode(b"no parens").unwrap_err();
        assert!(matches!(err, DsdFp2Error::MalformedResponse(_)));
    }

    #[test]
    fn codec_default_matches_and_max_skip_are_zero() {
        let codec = Fp2Codec;
        // FP2 doesn't emit unsolicited frames, so defaults are correct.
        let cmd = Command::GetBrightness;
        let resp = RawResponse {
            body: "0".to_string(),
        };
        assert!(codec.matches(&cmd, &resp));
        assert_eq!(codec.max_skip(), 0);
    }
}
