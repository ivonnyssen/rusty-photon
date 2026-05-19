//! On-disk persistence for `SetPark` — read/write the park-encoder
//! pair to the operator's JSON config file, plus the startup helpers
//! `main.rs` calls to canonicalise the path and probe writability
//! before the first `SetPark` request lands.
//!
//! Read-as-`serde_json::Value` + atomic-rename pattern: only the two
//! `mount.park_*_ticks` keys are touched, every other JSON value is
//! preserved verbatim. See the design doc's
//! [§"Park persistence"](../../../../docs/services/star-adventurer-gti.md#park-persistence)
//! for the contract.

use std::path::{Path, PathBuf};

use crate::error::StarAdvError;

/// Probe whether the parent directory of `config_path` can host the
/// staging temp file that `SetPark`'s atomic-rename pattern requires.
///
/// Called once at startup from `main.rs` so the operator sees a `warn!`
/// at boot if `SetPark` will fail at runtime due to filesystem
/// permissions, rather than only discovering it on the first `SetPark`
/// call. Does **not** change `CanSetPark` — the capability still
/// advertises support; the probe is purely an early-warning signal.
///
/// The probe creates a `NamedTempFile` in the same directory the real
/// staging file would live in (`config_path.parent()`) and immediately
/// drops it. Writability of the **parent directory** is what matters
/// for the atomic-rename pattern: even if the target config file is
/// itself read-only, `rename(2)` only needs write access to the
/// containing directory to swap in a new file. The probe therefore
/// matches what [`write_park_to_config`] actually does — a false-
/// positive would mean the probe passes but the real write fails (or
/// vice versa), defeating the point.
pub fn probe_park_file_writability(config_path: &Path) -> std::io::Result<()> {
    let parent = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // Drop closes and deletes the temp file; the probe leaves no trace.
    let _tmp = tempfile::NamedTempFile::new_in(parent)?;
    Ok(())
}

/// Canonicalise the operator-supplied config path so `SetPark` writes
/// to a stable absolute location even if the process later `chdir`s
/// away (also resolves symlinks, which the atomic-rename pattern
/// needs — the temp file goes in the *physical* parent directory).
/// On canonicalisation failure (path doesn't yet exist, symlink loop,
/// permission denied on a path component) the original path is
/// returned and a `warn!` is logged — `SetPark` will still attempt the
/// write against the path as given, surfacing the real error there.
///
/// Extracted from `main.rs` so the warn-on-failure branch is unit
/// testable; the binary calls this from `main()`.
pub fn canonicalise_config_path(config_path: Option<&PathBuf>) -> Option<PathBuf> {
    config_path.map(|p| {
        std::fs::canonicalize(p).unwrap_or_else(|e| {
            tracing::warn!(
                "could not canonicalise config path {:?}: {e}; SetPark will write to the path as given",
                p
            );
            p.clone()
        })
    })
}

/// Early-warning probe wrapper: run [`probe_park_file_writability`] on
/// the supplied path and log a `warn!` on failure. Used by `main.rs`
/// at startup — operators get a heads-up at boot if `SetPark` will
/// fail at runtime due to filesystem permissions, rather than only
/// discovering it on the first `SetPark` call. `CanSetPark` is not
/// affected; the capability still advertises support and the actual
/// `SetPark` will surface a structured error if the probe was correct.
///
/// Extracted from `main.rs` so the warn-on-failure branch is unit
/// testable.
pub fn warn_if_park_path_unwritable(config_path: &Path) {
    if let Err(e) = probe_park_file_writability(config_path) {
        tracing::warn!(
            "SetPark writes to {:?} will fail at runtime: {e}. \
             Check permissions on the containing directory if SetPark support is required.",
            config_path
        );
    }
}

/// Read `mount.park_ra_ticks` / `mount.park_dec_ticks` from the on-disk
/// config file. Each axis is returned independently — a `None` means
/// the file did not set that key (or set it to JSON `null`), and the
/// caller will fall back to the handshake-captured value for that axis.
///
/// A key that **is** present but holds something other than an integer
/// inside `i32`'s range is surfaced as a `StarAdvError::Config` rather
/// than silently treated as `None`. Operator typos (a string,
/// an i64 too large to be encoder ticks, a float) should fail loudly so
/// the misconfiguration is visible rather than masked by the handshake
/// fallback. Other failures (file missing, malformed JSON, `mount` key
/// missing or not an object) are also surfaced as `StarAdvError::Config`.
///
/// Reading the file only at connect time means an operator can
/// hand-edit the park keys between connects and have the change take
/// effect on reconnect, without restarting the driver.
///
/// Blocking I/O; callers wrap in `tokio::task::spawn_blocking`.
pub(super) fn read_park_from_config(
    config_path: &Path,
) -> crate::error::Result<(Option<i32>, Option<i32>)> {
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| StarAdvError::Config(format!("read config {}: {e}", config_path.display())))?;
    let root: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        StarAdvError::Config(format!("parse config {}: {e}", config_path.display()))
    })?;
    let mount = root
        .as_object()
        .and_then(|o| o.get("mount"))
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            StarAdvError::Config(format!(
                "config {} has no `mount` object",
                config_path.display()
            ))
        })?;
    let ra = extract_park_tick(mount.get("park_ra_ticks"), "mount.park_ra_ticks")?;
    let dec = extract_park_tick(mount.get("park_dec_ticks"), "mount.park_dec_ticks")?;
    Ok((ra, dec))
}

/// Decode an optional park-tick JSON value:
///
/// - Absent (`None`) or explicit `Value::Null` → `Ok(None)` (caller
///   falls back to the handshake-captured value).
/// - A JSON integer in the `i32` range → `Ok(Some(n))`.
/// - Anything else (string, float, boolean, array/object, i64 outside
///   `i32` range) → `Err(StarAdvError::Config)`. Loud failure on
///   operator typo is the whole reason this helper exists — silently
///   falling back to handshake would mask the misconfiguration.
fn extract_park_tick(
    value: Option<&serde_json::Value>,
    key: &'static str,
) -> crate::error::Result<Option<i32>> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_i64().ok_or_else(|| {
                StarAdvError::Config(format!(
                    "`{key}` must be an integer (encoder ticks), got {v}"
                ))
            })?;
            i32::try_from(n).map(Some).map_err(|_| {
                StarAdvError::Config(format!(
                    "`{key}` value {n} is outside the i32 encoder-tick range"
                ))
            })
        }
    }
}

/// Patch the on-disk JSON config with the supplied park encoder pair.
///
/// Read-as-`Value` + atomic-rename pattern: load the file as
/// `serde_json::Value`, mutate **only** the `mount.park_ra_ticks` and
/// `mount.park_dec_ticks` keys, serialise pretty-printed, write via a
/// `tempfile::NamedTempFile` in the same directory, `persist` to swap
/// it in atomically. Every other field of the JSON file — known and
/// unknown — is preserved as a JSON value. Operator-level formatting
/// (insertion-order of unrelated keys, custom indentation, comments
/// disguised as fields) is not preserved byte-for-byte because the
/// round-trip pretty-prints the whole document; the *semantic* content
/// outside the two park keys is unchanged.
///
/// Durability: fsync the staged file before rename (`tempfile::persist`
/// uses POSIX `rename(2)`), then fsync the parent directory after
/// rename so the directory entry update is itself durable. Mirrors
/// `services/rp/src/persistence/document.rs::write_sidecar_sync`.
///
/// The driver never re-serialises its in-memory typed `Config` here:
/// doing so would round-trip CLI overrides (`--port`, `--baud`, etc.)
/// back to disk and is structurally avoided. See the design doc's
/// [§"Park persistence"](../../../../docs/services/star-adventurer-gti.md#park-persistence)
/// for the contract this helper implements.
///
/// Blocking I/O; callers wrap in `tokio::task::spawn_blocking`.
pub(super) fn write_park_to_config(
    config_path: &Path,
    park_ra_ticks: i32,
    park_dec_ticks: i32,
) -> crate::error::Result<()> {
    use std::io::Write;

    let content = std::fs::read_to_string(config_path)
        .map_err(|e| StarAdvError::Config(format!("read config {}: {e}", config_path.display())))?;
    let mut root: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        StarAdvError::Config(format!("parse config {}: {e}", config_path.display()))
    })?;
    let mount = root
        .as_object_mut()
        .and_then(|o| o.get_mut("mount"))
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| {
            StarAdvError::Config(format!(
                "config {} has no `mount` object",
                config_path.display()
            ))
        })?;
    mount.insert(
        "park_ra_ticks".to_string(),
        serde_json::Value::from(park_ra_ticks),
    );
    mount.insert(
        "park_dec_ticks".to_string(),
        serde_json::Value::from(park_dec_ticks),
    );
    let mut pretty = serde_json::to_string_pretty(&root)
        .map_err(|e| StarAdvError::Config(format!("serialise config: {e}")))?;
    // serde_json's pretty-printer omits a trailing newline; add one so
    // operators editing the file later don't trip POSIX "no newline at
    // end of file" warnings in diffs.
    pretty.push('\n');

    // Temp file must live in the **same directory** as the destination
    // so `persist` can use POSIX `rename` (atomic on the same
    // filesystem) rather than copy-and-delete. Fall back to the
    // current dir if the path has no parent (e.g. a bare filename),
    // which is what Path::parent returns Some("") for — coerce to ".".
    let parent = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| StarAdvError::Config(format!("create temp file in {parent:?}: {e}")))?;
    tmp.write_all(pretty.as_bytes())
        .map_err(|e| StarAdvError::Config(format!("write temp file: {e}")))?;
    // fsync the file data so a crash after rename cannot surface a
    // renamed-but-zero-length sidecar.
    tmp.as_file()
        .sync_all()
        .map_err(|e| StarAdvError::Config(format!("fsync temp file: {e}")))?;
    tmp.persist(config_path).map_err(|e| {
        StarAdvError::Config(format!("atomic rename to {}: {e}", config_path.display()))
    })?;
    // fsync the parent directory so the rename itself is durable.
    // Windows can't open a directory as a regular file handle, so this
    // is unix-only. Mirrors `services/rp/src/persistence/document.rs`.
    #[cfg(unix)]
    {
        std::fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| StarAdvError::Config(format!("fsync parent dir {parent:?}: {e}")))?;
    }
    Ok(())
}
