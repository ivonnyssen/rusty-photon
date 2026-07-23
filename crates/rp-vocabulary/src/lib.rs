//! `rp-vocabulary`: the shared, validated vocabulary of `rp`'s imaging
//! plan — the small domain value types ([`IcrsCoord`], [`Binning`],
//! [`FrameType`], [`Exposure`]) that `rp` and every surface that talks to
//! it about plans agree on.
//!
//! Each type is *parse-don't-validate*: a value that exists is valid by
//! construction, and that one constructor is the single validator every
//! surface shares. The crate holds no logic — no store, no template
//! engine, no ephemeris math, no protocol endpoints; only the validated
//! nouns those layers exchange.
//!
//! See [`docs/crates/rp-vocabulary.md`](https://github.com/ivonnyssen/rusty-photon/blob/main/docs/crates/rp-vocabulary.md)
//! and ADR-019 for the design and the reasoning.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![deny(unsafe_code)]

mod binning;
mod coord;
mod exposure;
mod frame_type;

pub use binning::{Binning, BinningParseError};
pub use coord::{CoordError, IcrsCoord};
pub use exposure::{Exposure, ExposureError};
pub use frame_type::{FrameType, FrameTypeParseError};
