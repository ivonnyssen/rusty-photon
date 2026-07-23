//! [`TargetStoreError`], the single error type returned by every
//! [`crate::TargetStore`] method.

/// Errors returned by [`crate::TargetStore`] implementations.
///
/// See `docs/crates/rp-targets.md` "The `TargetStore` trait" for the
/// contract each variant participates in.
#[derive(Debug, thiserror::Error)]
pub enum TargetStoreError {
    /// Failed to open or create the redb database file. Never constructed
    /// for a redb-format generation bump — see [`Self::RedbUpgradeRequired`].
    #[error("failed to open target database: {0}")]
    Open(redb::DatabaseError),

    /// Failed to begin a redb transaction.
    #[error("failed to begin transaction: {0}")]
    Txn(#[from] redb::TransactionError),

    /// Failed to open a redb table within a transaction.
    #[error("failed to open table: {0}")]
    Table(#[from] redb::TableError),

    /// A redb storage-layer operation (insert/get/remove/range) failed.
    #[error("storage error: {0}")]
    Storage(#[from] redb::StorageError),

    /// Failed to commit a redb write transaction.
    #[error("failed to commit transaction: {0}")]
    Commit(#[from] redb::CommitError),

    /// Failed to (de)serialize a [`crate::Target`] to/from its stored JSON
    /// representation.
    #[error("failed to encode/decode target value: {0}")]
    Encode(#[from] serde_json::Error),

    /// The redb file-format generation on disk is older than this build's
    /// redb crate understands. Distinct from [`Self::UnsupportedSchemaVersion`],
    /// which versions this crate's own `Target` value shape, not redb's file
    /// layout. Run the documented one-time `redb::Database::upgrade()`.
    #[error(
        "target database file format requires a one-time redb upgrade (see docs/crates/rp-targets.md)"
    )]
    RedbUpgradeRequired,

    /// The on-disk `schema_version` is newer than this build supports —
    /// refuse to run against a database written by a newer build rather
    /// than silently dropping fields.
    #[error("on-disk schema version {found} is newer than this build supports (max {supported})")]
    UnsupportedSchemaVersion {
        /// The `schema_version` value found in the database's `meta` table.
        found: u32,
        /// [`crate::migrate::CURRENT_SCHEMA_VERSION`] for this build.
        supported: u32,
    },

    /// A goals-only operation ([`crate::TargetStore::set_goals`]) referenced
    /// a slug with no stored target.
    #[error("no target with slug {slug:?}")]
    NotFound {
        /// The slug that was not found.
        slug: String,
    },

    /// A goal set passed to [`crate::TargetStore::set_goals`] or
    /// [`crate::TargetStore::upsert_target`] contains duplicate
    /// `(filter, binning, exposure)` keys, or a goal with a zero
    /// `desired_count` or zero `exposure`.
    #[error("invalid acquisition goal set: {reason}")]
    InvalidGoals {
        /// Human-readable description of which goal failed validation and why.
        reason: String,
    },

    /// The blocking task running a redb operation panicked or was cancelled.
    #[error("target store blocking task join error: {0}")]
    Join(String),

    /// A raw value read back from the `meta` table was not shaped as this
    /// crate writes it (e.g. `schema_version` was not 4 LE bytes). Only
    /// reachable if the file was hand-edited or corrupted — this crate's
    /// own writers always produce the expected shape.
    #[error("target database meta table is corrupt: {0}")]
    Corrupt(String),
}
