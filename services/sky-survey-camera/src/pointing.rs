use serde::{Deserialize, Serialize};
use std::sync::RwLock;

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

    pub fn snapshot(&self) -> PointingState {
        *self.state.read().expect("pointing rwlock poisoned")
    }

    /// Atomically update RA, Dec, and (optionally) rotation. If
    /// `rotation_deg` is `None`, the existing rotation is preserved.
    /// Returns `Err` with a list of validation messages on bad input.
    pub fn update(
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

        let mut guard = self.state.write().expect("pointing rwlock poisoned");
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

    #[test]
    fn update_rejects_out_of_range() {
        let s = SharedPointing::new(PointingState::new(10.0, 20.0, 0.0));
        s.update(-1.0, 0.0, None).unwrap_err();
        s.update(360.0, 0.0, None).unwrap_err();
        s.update(0.0, -91.0, None).unwrap_err();
        s.update(0.0, 91.0, None).unwrap_err();
        // unchanged
        let snap = s.snapshot();
        assert_eq!(snap.ra_deg, 10.0);
        assert_eq!(snap.dec_deg, 20.0);
    }

    #[test]
    fn update_preserves_rotation_when_none() {
        let s = SharedPointing::new(PointingState::new(0.0, 0.0, 45.0));
        s.update(10.0, 20.0, None).unwrap();
        assert_eq!(s.snapshot().rotation_deg, 45.0);
    }

    #[test]
    fn update_wraps_rotation() {
        let s = SharedPointing::new(PointingState::new(0.0, 0.0, 0.0));
        s.update(0.0, 0.0, Some(390.0)).unwrap();
        assert!((s.snapshot().rotation_deg - 30.0).abs() < 1e-9);
    }
}
