use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

use crate::error::MountReadError;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PointingState {
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub rotation_deg: f64,
}

impl PointingState {
    pub fn new(ra_deg: f64, dec_deg: f64, rotation_deg: f64) -> Self {
        Self {
            ra_deg,
            dec_deg,
            rotation_deg: wrap_rotation(rotation_deg),
        }
    }
}

/// Position read from an ASCOM Telescope. RA in hours per the ASCOM
/// spec; Dec in degrees. The `TelescopeFollow` snapshot converts hours
/// to degrees (× 15) before constructing a [`PointingState`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MountPosition {
    pub ra_hours: f64,
    pub dec_deg: f64,
}

/// Narrow trait around the two ASCOM Telescope reads `TelescopeFollow`
/// needs. Wrapping the giant `ascom_alpaca::api::Telescope` trait this
/// way keeps unit tests trivial: a `mockall`-generated mock implements
/// just two methods, not the entire ASCOM Telescope surface. Per
/// ADR-004, traits ≤ 10 methods at a service boundary may be mocked.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait MountReader: Send + Sync + std::fmt::Debug {
    async fn read_position(&self) -> Result<MountPosition, MountReadError>;
}

/// Shared `PointingState` snapshot store. Wrapped in an
/// `Arc<SharedPointing>` and used in two roles:
/// - As `DeviceState::last_snapshot`: the source for
///   `GET /sky-survey/position` in **both** static and follow modes.
///   In static mode, `POST` writes go through `update()`; in follow
///   mode, the exposure pipeline writes the post-offset mount RA/Dec
///   here via `store()` after each successful read so the GET
///   response reflects what the camera last saw (F6).
/// - As the inner Arc carried by `PointingSource::Static`: shared
///   with `last_snapshot` so static-mode reads in the exposure
///   pipeline see `POST` writes immediately, no separate writeback
///   needed.
#[derive(Debug)]
pub struct SharedPointing {
    state: RwLock<PointingState>,
}

impl SharedPointing {
    pub fn new(initial: PointingState) -> Self {
        Self {
            state: RwLock::new(initial),
        }
    }

    pub async fn snapshot(&self) -> PointingState {
        *self.state.read().await
    }

    /// Atomically update RA, Dec, and (optionally) rotation. If
    /// `rotation_deg` is `None`, the existing rotation is preserved.
    /// Returns `Err` with a list of validation messages on bad input.
    pub async fn update(
        &self,
        ra_deg: f64,
        dec_deg: f64,
        rotation_deg: Option<f64>,
    ) -> Result<PointingState, Vec<&'static str>> {
        let mut errors = Vec::new();
        if !ra_deg.is_finite() || !(0.0..360.0).contains(&ra_deg) {
            errors.push("ra_deg must be in [0, 360)");
        }
        if !dec_deg.is_finite() || !(-90.0..=90.0).contains(&dec_deg) {
            errors.push("dec_deg must be in [-90, +90]");
        }
        if let Some(rot) = rotation_deg {
            if !rot.is_finite() {
                errors.push("rotation_deg must be finite");
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        let mut guard = self.state.write().await;
        let new_rotation = rotation_deg
            .map(wrap_rotation)
            .unwrap_or(guard.rotation_deg);
        *guard = PointingState {
            ra_deg,
            dec_deg,
            rotation_deg: new_rotation,
        };
        Ok(*guard)
    }

    pub async fn store(&self, value: PointingState) {
        *self.state.write().await = value;
    }
}

/// Telescope-following snapshot source. Holds the [`MountReader`] plus
/// the configured rotation and the constant pointing offset (F5, the
/// cone-error analog). Per F1, the snapshot computes
///
/// ```text
/// ra_deg  = (mount_ra_hours * 15 + offset_ra_arcsec  / 3600).rem_euclid(360)
/// dec_deg = clamp(mount_dec_deg     + offset_dec_arcsec / 3600, -90, +90)
/// ```
#[derive(Debug)]
pub struct TelescopeFollow {
    reader: Arc<dyn MountReader>,
    rotation_deg: f64,
    offset_ra_arcsec: f64,
    offset_dec_arcsec: f64,
}

impl TelescopeFollow {
    pub fn new(
        reader: Arc<dyn MountReader>,
        rotation_deg: f64,
        offset_ra_arcsec: f64,
        offset_dec_arcsec: f64,
    ) -> Self {
        Self {
            reader,
            rotation_deg: wrap_rotation(rotation_deg),
            offset_ra_arcsec,
            offset_dec_arcsec,
        }
    }

    pub async fn snapshot(&self) -> Result<PointingState, MountReadError> {
        let pos = self.reader.read_position().await?;
        let raw_ra_deg = pos.ra_hours * 15.0 + self.offset_ra_arcsec / 3600.0;
        let raw_dec_deg = pos.dec_deg + self.offset_dec_arcsec / 3600.0;
        // F5: RA wraps; Dec clamps. A clamp on Dec produces a `warn!`
        // because reaching ±90 on top of a sane mount usually means
        // the offset is misconfigured.
        let ra_deg = raw_ra_deg.rem_euclid(360.0);
        let dec_deg = if !(-90.0..=90.0).contains(&raw_dec_deg) {
            warn!(
                offset_dec_arcsec = self.offset_dec_arcsec,
                mount_dec_deg = pos.dec_deg,
                raw_dec_deg,
                "follow-mode Dec offset pushed past ±90°; clamping"
            );
            raw_dec_deg.clamp(-90.0, 90.0)
        } else {
            raw_dec_deg
        };
        Ok(PointingState {
            ra_deg,
            dec_deg,
            rotation_deg: self.rotation_deg,
        })
    }
}

/// Pointing snapshot source. Selected once at construction from
/// `pointing.telescope`. Switching at runtime would require teaching
/// `POST /sky-survey/position` to fall back / override; that's
/// feature creep without a driving use case.
#[derive(Debug)]
pub enum PointingSource {
    Static(Arc<SharedPointing>),
    Telescope(TelescopeFollow),
}

impl PointingSource {
    pub fn is_follow_mode(&self) -> bool {
        matches!(self, Self::Telescope(_))
    }

    /// Snapshot the current pointing. In `Static` mode this is
    /// infallible. In `Telescope` mode, a failed mount read surfaces
    /// per F2.
    pub async fn snapshot(&self) -> Result<PointingState, MountReadError> {
        match self {
            Self::Static(s) => Ok(s.snapshot().await),
            Self::Telescope(t) => t.snapshot().await,
        }
    }
}

fn wrap_rotation(rotation_deg: f64) -> f64 {
    let wrapped = rotation_deg.rem_euclid(360.0);
    // rem_euclid can produce values exactly equal to 360 in pathological
    // floating point cases; clamp back into [0, 360).
    if wrapped >= 360.0 {
        0.0
    } else {
        wrapped
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn wrap_rotation_keeps_in_range() {
        assert_eq!(wrap_rotation(0.0), 0.0);
        assert_eq!(wrap_rotation(180.0), 180.0);
        assert_eq!(wrap_rotation(360.0), 0.0);
        assert!((wrap_rotation(370.0) - 10.0).abs() < 1e-9);
        assert!((wrap_rotation(-10.0) - 350.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn update_rejects_out_of_range() {
        let s = SharedPointing::new(PointingState::new(10.0, 20.0, 0.0));
        s.update(-1.0, 0.0, None).await.unwrap_err();
        s.update(360.0, 0.0, None).await.unwrap_err();
        s.update(0.0, -91.0, None).await.unwrap_err();
        s.update(0.0, 91.0, None).await.unwrap_err();
        // unchanged
        let snap = s.snapshot().await;
        assert_eq!(snap.ra_deg, 10.0);
        assert_eq!(snap.dec_deg, 20.0);
    }

    #[tokio::test]
    async fn update_preserves_rotation_when_none() {
        let s = SharedPointing::new(PointingState::new(0.0, 0.0, 45.0));
        s.update(10.0, 20.0, None).await.unwrap();
        assert_eq!(s.snapshot().await.rotation_deg, 45.0);
    }

    #[tokio::test]
    async fn update_wraps_rotation() {
        let s = SharedPointing::new(PointingState::new(0.0, 0.0, 0.0));
        s.update(0.0, 0.0, Some(390.0)).await.unwrap();
        assert!((s.snapshot().await.rotation_deg - 30.0).abs() < 1e-9);
    }

    fn mock_reader_returning(pos: MountPosition) -> MockMountReader {
        let mut reader = MockMountReader::new();
        reader.expect_read_position().returning(move || Ok(pos));
        reader
    }

    #[tokio::test]
    async fn telescope_follow_converts_hours_to_degrees() {
        let reader = mock_reader_returning(MountPosition {
            ra_hours: 10.0,
            dec_deg: 30.0,
        });
        let follow = TelescopeFollow::new(Arc::new(reader), 12.5, 0.0, 0.0);
        let snap = follow.snapshot().await.unwrap();
        assert!((snap.ra_deg - 150.0).abs() < 1e-9);
        assert!((snap.dec_deg - 30.0).abs() < 1e-9);
        assert!((snap.rotation_deg - 12.5).abs() < 1e-9);
    }

    #[tokio::test]
    async fn telescope_follow_propagates_read_error() {
        let mut reader = MockMountReader::new();
        reader
            .expect_read_position()
            .returning(|| Err(MountReadError::Transport("oops".into())));
        let follow = TelescopeFollow::new(Arc::new(reader), 0.0, 0.0, 0.0);
        let err = follow.snapshot().await.unwrap_err();
        assert!(matches!(err, MountReadError::Transport(_)));
    }

    #[tokio::test]
    async fn telescope_follow_applies_ra_offset() {
        // 60 arcsec = 1/60 degree
        let reader = mock_reader_returning(MountPosition {
            ra_hours: 0.0,
            dec_deg: 0.0,
        });
        let follow = TelescopeFollow::new(Arc::new(reader), 0.0, 60.0, 0.0);
        let snap = follow.snapshot().await.unwrap();
        assert!((snap.ra_deg - (1.0 / 60.0)).abs() < 1e-9);
        assert_eq!(snap.dec_deg, 0.0);
    }

    #[tokio::test]
    async fn telescope_follow_applies_dec_offset() {
        let reader = mock_reader_returning(MountPosition {
            ra_hours: 0.0,
            dec_deg: 30.0,
        });
        let follow = TelescopeFollow::new(Arc::new(reader), 0.0, 0.0, -45.0);
        let snap = follow.snapshot().await.unwrap();
        assert_eq!(snap.ra_deg, 0.0);
        assert!((snap.dec_deg - (30.0 - 45.0 / 3600.0)).abs() < 1e-9);
    }

    #[tokio::test]
    async fn telescope_follow_wraps_ra_at_zero() {
        // mount RA = 23h59m59.5s ≈ 359.997917°; +20 arcsec ≈ +0.005556°
        // sum ≈ 360.0035°, wraps to ≈ 0.0035°
        let reader = mock_reader_returning(MountPosition {
            ra_hours: 23.99986111, // exactly enough that +20 arcsec crosses 360
            dec_deg: 0.0,
        });
        let follow = TelescopeFollow::new(Arc::new(reader), 0.0, 20.0, 0.0);
        let snap = follow.snapshot().await.unwrap();
        // expected: (23.99986111 * 15 + 20/3600) mod 360
        let expected = (23.99986111_f64 * 15.0 + 20.0 / 3600.0).rem_euclid(360.0);
        assert!(
            (snap.ra_deg - expected).abs() < 1e-9,
            "got {} expected {}",
            snap.ra_deg,
            expected
        );
        assert!(snap.ra_deg >= 0.0 && snap.ra_deg < 360.0);
    }

    #[tokio::test]
    async fn telescope_follow_wraps_negative_ra() {
        // mount RA = 0h, offset = -3600 arcsec = -1°, expect 359°
        let reader = mock_reader_returning(MountPosition {
            ra_hours: 0.0,
            dec_deg: 0.0,
        });
        let follow = TelescopeFollow::new(Arc::new(reader), 0.0, -3600.0, 0.0);
        let snap = follow.snapshot().await.unwrap();
        assert!((snap.ra_deg - 359.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn telescope_follow_clamps_dec_at_north_pole() {
        let reader = mock_reader_returning(MountPosition {
            ra_hours: 0.0,
            dec_deg: 89.99,
        });
        // +60 arcsec offset on Dec=89.99 = 89.99 + 0.01666... = 90.00666... → clamped 90
        let follow = TelescopeFollow::new(Arc::new(reader), 0.0, 0.0, 60.0);
        let snap = follow.snapshot().await.unwrap();
        assert_eq!(snap.dec_deg, 90.0);
    }

    #[tokio::test]
    async fn telescope_follow_clamps_dec_at_south_pole() {
        let reader = mock_reader_returning(MountPosition {
            ra_hours: 0.0,
            dec_deg: -89.99,
        });
        let follow = TelescopeFollow::new(Arc::new(reader), 0.0, 0.0, -60.0);
        let snap = follow.snapshot().await.unwrap();
        assert_eq!(snap.dec_deg, -90.0);
    }

    #[tokio::test]
    async fn pointing_source_static_snapshot_infallible() {
        let s = Arc::new(SharedPointing::new(PointingState::new(1.0, 2.0, 3.0)));
        let src = PointingSource::Static(Arc::clone(&s));
        assert!(!src.is_follow_mode());
        let snap = src.snapshot().await.unwrap();
        assert_eq!(snap.ra_deg, 1.0);
    }

    #[tokio::test]
    async fn pointing_source_telescope_uses_reader() {
        let reader = mock_reader_returning(MountPosition {
            ra_hours: 0.0,
            dec_deg: 0.0,
        });
        let follow = TelescopeFollow::new(Arc::new(reader), 0.0, 0.0, 0.0);
        let src = PointingSource::Telescope(follow);
        assert!(src.is_follow_mode());
        src.snapshot().await.unwrap();
    }
}
