//! Schema version and migration steps for the `meta["schema_version"]` key.
//!
//! Distinct from redb's own file-format generation (see
//! [`crate::TargetStoreError::RedbUpgradeRequired`]): this versions the
//! *shape* of the serialized [`crate::Target`] value. Additive,
//! non-breaking field changes need no migration step at all — value structs
//! `#[serde(default)]` their new fields and tolerate unknown ones, so an
//! old value deserializes into the new `Target` directly. A step is only
//! authored for a breaking re-shape (rename, split, type change).

use crate::error::TargetStoreError;

/// The schema version this build writes for a fresh database and expects
/// after migration.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Checks a database's on-disk `schema_version` against
/// [`CURRENT_SCHEMA_VERSION`].
///
/// No migration steps exist yet (this is the first shipped version), so
/// there is nothing to run *between* versions today — this function only
/// classifies `found` against `CURRENT_SCHEMA_VERSION`. The first breaking
/// `Target` re-shape adds an ordered step here, run inside the caller's
/// single write transaction before the version key is bumped.
///
/// # Errors
///
/// Returns [`TargetStoreError::UnsupportedSchemaVersion`] if `found` is
/// newer than this build supports.
pub fn check_schema_version(found: u32) -> Result<(), TargetStoreError> {
    if found > CURRENT_SCHEMA_VERSION {
        return Err(TargetStoreError::UnsupportedSchemaVersion {
            found,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    // found < CURRENT_SCHEMA_VERSION would run ordered migration steps here;
    // none exist yet since CURRENT_SCHEMA_VERSION is the first version.
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn current_version_passes() {
        check_schema_version(CURRENT_SCHEMA_VERSION).unwrap();
    }

    #[test]
    fn older_version_passes_pending_future_migration_steps() {
        check_schema_version(0).unwrap();
    }

    #[test]
    fn newer_version_is_rejected() {
        let err = check_schema_version(CURRENT_SCHEMA_VERSION + 1).unwrap_err();
        assert!(matches!(
            err,
            TargetStoreError::UnsupportedSchemaVersion { .. }
        ));
    }
}
