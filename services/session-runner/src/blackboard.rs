//! The workflow blackboard: `session.*` state plus atomic persistence.
//!
//! The blackboard is the workflow's only mutable state (design:
//! `docs/services/session-runner.md` § Blackboard and Persistence). It is a
//! JSON object persisted to `<state_dir>/<session_id>.json` with the
//! workspace atomic-write pattern (sibling temp file, fsync, rename, fsync
//! parent directory — mirroring `rp`'s exposure-document sidecars), and it
//! is persisted after **every** mutation: each `set` instruction, each
//! `once` completion marker, each trigger bookkeeping update. That
//! write-on-mutation invariant — the file always reflects every completed
//! `set` — is what makes re-derive resume sound.
//!
//! Engine bookkeeping lives under reserved keys documents cannot set:
//! `session._once.*` (completed once-markers) and `session._triggers.<id>.*`
//! (trigger bookkeeping, Phase D). [`Blackboard::set_path`] rejects
//! `_`-prefixed roots as defense in depth — the document validator already
//! refuses such `set` keys at load.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Root key of the completed-`once`-marker map (`session._once.*`).
const ONCE_KEY: &str = "_once";

/// A blackboard I/O failure. Per the design's error table these fail loud:
/// continuing with unpersistable state would silently break resume.
#[derive(Debug, thiserror::Error)]
pub enum BlackboardError {
    #[error("blackboard read failed for {}: {message}", path.display())]
    Read { path: PathBuf, message: String },
    #[error("blackboard file {} is corrupt: {message}", path.display())]
    Corrupt { path: PathBuf, message: String },
    #[error("blackboard write failed for {}: {message}", path.display())]
    Write { path: PathBuf, message: String },
}

/// An invalid in-memory `set` write (no I/O involved).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SetPathError {
    /// The first path segment is `_`-prefixed. The document validator
    /// already rejects these; this guards the engine-internal API surface.
    #[error(
        "`{key}` writes reserved engine state — `session._*` keys \
         (`session._once`, `session._triggers`) cannot be set by a document"
    )]
    Reserved { key: String },
    /// An intermediate path segment exists but is not an object (and not
    /// `null` — missing or `null` intermediates are created as objects).
    #[error("cannot set `{key}`: `{ancestor}` is not an object")]
    NotAnObject { key: String, ancestor: String },
    /// The segment list was empty — the `session` root itself cannot be
    /// replaced. The document model guarantees at least one segment.
    #[error("cannot set the `session` root itself")]
    EmptyPath,
}

/// The `session.*` state for one workflow session, bound to its
/// persistence path.
#[derive(Debug)]
pub struct Blackboard {
    /// Always `Value::Object` — both constructors build one and every
    /// write path preserves it.
    session: Value,
    path: PathBuf,
}

impl Blackboard {
    /// An empty blackboard bound to `path`, with no I/O. Prefer
    /// [`Blackboard::replace`] for a new session — it also clears any
    /// leftover file.
    pub fn new_empty(path: PathBuf) -> Self {
        Self {
            session: Value::Object(Map::new()),
            path,
        }
    }

    /// A fresh blackboard for a new (non-recovery) session: any leftover
    /// file at `path` (an earlier session that never completed) is
    /// deleted **eagerly**, per the design's invocation rules. Lazy
    /// replacement on first persist would not be enough — a safety
    /// termination before the first write must not leave the stale file
    /// (stale `_once` markers included) to be mistaken for this session's
    /// state on the recovery invocation.
    pub async fn replace(path: PathBuf) -> Result<Self, BlackboardError> {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(BlackboardError::Write {
                    path,
                    message: format!("cannot delete leftover blackboard: {e}"),
                })
            }
        }
        Ok(Self::new_empty(path))
    }

    /// Load the blackboard for a recovery invocation. A missing file is
    /// not an error — the session starts with an empty `session.*`
    /// (first-run equivalent), because a crash can predate the first
    /// `set`. A present-but-unparsable file is an error: silently
    /// discarding state would break resume.
    pub fn load(path: PathBuf) -> Result<Self, BlackboardError> {
        let session = match std::fs::read(&path) {
            Ok(bytes) => {
                let value: Value =
                    serde_json::from_slice(&bytes).map_err(|e| BlackboardError::Corrupt {
                        path: path.clone(),
                        message: e.to_string(),
                    })?;
                if !value.is_object() {
                    return Err(BlackboardError::Corrupt {
                        path,
                        message: "top level is not a JSON object".to_owned(),
                    });
                }
                value
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
            Err(e) => {
                return Err(BlackboardError::Read {
                    path,
                    message: e.to_string(),
                })
            }
        };
        Ok(Self { session, path })
    }

    /// The full `session` object, for the expression evaluation context.
    pub fn value(&self) -> &Value {
        &self.session
    }

    /// The persistence path this blackboard is bound to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// In-memory write of one `set` entry: `segments` are the path after
    /// the `session` root (`["a", "b"]` for `session.a.b`). Missing or
    /// `null` intermediate segments are created as objects; an existing
    /// non-object intermediate is an error (the write would silently
    /// discard a scalar the document previously stored). Does **not**
    /// persist — the engine persists once per `set` instruction, after
    /// all of its entries are written.
    pub fn set_path(&mut self, segments: &[String], value: Value) -> Result<(), SetPathError> {
        let key = document_key(segments);
        if segments.first().is_some_and(|s| s.starts_with('_')) {
            return Err(SetPathError::Reserved { key });
        }
        let (last, parents) = segments.split_last().ok_or(SetPathError::EmptyPath)?;

        let mut walked = String::from("session");
        let mut cur = &mut self.session;
        for seg in parents {
            if cur.is_null() {
                *cur = Value::Object(Map::new());
            }
            cur = match cur {
                Value::Object(map) => map
                    .entry(seg.clone())
                    .or_insert_with(|| Value::Object(Map::new())),
                _ => {
                    return Err(SetPathError::NotAnObject {
                        key,
                        ancestor: walked,
                    })
                }
            };
            walked.push('.');
            walked.push_str(seg);
        }
        if cur.is_null() {
            *cur = Value::Object(Map::new());
        }
        match cur {
            Value::Object(map) => {
                map.insert(last.clone(), value);
                Ok(())
            }
            _ => Err(SetPathError::NotAnObject {
                key,
                ancestor: walked,
            }),
        }
    }

    /// Whether the `once` marker `key` has been recorded (the instruction
    /// completed in this or an earlier run of the session).
    pub fn once_done(&self, key: &str) -> bool {
        self.session
            .get(ONCE_KEY)
            .and_then(|m| m.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Record the `once` marker `key` under `session._once` and persist.
    /// Engine-owned bookkeeping heals rather than errors: a corrupt
    /// non-object `_once` value is replaced.
    pub async fn mark_once(&mut self, key: &str) -> Result<(), BlackboardError> {
        if let Value::Object(root) = &mut self.session {
            let once = root
                .entry(ONCE_KEY)
                .or_insert_with(|| Value::Object(Map::new()));
            if !once.is_object() {
                *once = Value::Object(Map::new());
            }
            if let Value::Object(markers) = once {
                markers.insert(key.to_owned(), Value::Bool(true));
            }
        }
        self.persist().await
    }

    /// Atomically persist the session object: stage into a sibling temp
    /// file, fsync, rename into place, fsync the parent directory
    /// (unix-only). Runs on the blocking pool, one task per write.
    pub async fn persist(&self) -> Result<(), BlackboardError> {
        let body =
            serde_json::to_vec_pretty(&self.session).map_err(|e| BlackboardError::Write {
                path: self.path.clone(),
                message: format!("serialization failed: {e}"),
            })?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || write_atomic(&path, &body))
            .await
            .map_err(|e| BlackboardError::Write {
                path: self.path.clone(),
                message: format!("write task join error: {e}"),
            })?
    }
}

/// The document-form key (`session.a.b`) for logs and errors.
fn document_key(segments: &[String]) -> String {
    let mut key = String::from("session");
    for seg in segments {
        key.push('.');
        key.push_str(seg);
    }
    key
}

/// The workspace atomic-write pattern, as in `rp`'s
/// `persistence::document::write_sidecar_sync`.
fn write_atomic(final_path: &Path, body: &[u8]) -> Result<(), BlackboardError> {
    let write_err = |message: String| BlackboardError::Write {
        path: final_path.to_path_buf(),
        message,
    };
    let parent = final_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| write_err("path has no parent directory".to_owned()))?;
    std::fs::create_dir_all(parent).map_err(|e| write_err(e.to_string()))?;

    // `NamedTempFile::new_in(parent)` gives an OS-generated unique name (so
    // concurrent writers cannot collide on the staging path) and a `Drop`
    // guard that removes the staging file on early return; `persist`
    // disarms the guard on success.
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| write_err(e.to_string()))?;
    tmp.write_all(body).map_err(|e| write_err(e.to_string()))?;
    // fsync the file data so a crash after rename cannot surface a
    // renamed-but-empty blackboard.
    tmp.as_file()
        .sync_all()
        .map_err(|e| write_err(e.to_string()))?;
    tmp.persist(final_path)
        .map_err(|e| write_err(e.error.to_string()))?;
    // fsync the parent directory so the rename itself is durable. Windows
    // cannot open a directory as a regular file handle, so unix-only.
    #[cfg(unix)]
    {
        std::fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| write_err(e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use serde_json::json;

    use super::*;

    fn segs(path: &[&str]) -> Vec<String> {
        path.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn test_load_missing_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let bb = Blackboard::load(dir.path().join("nope.json")).unwrap();
        assert_eq!(*bb.value(), json!({}));
    }

    #[test]
    fn test_load_corrupt_json_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let err = Blackboard::load(path).unwrap_err();
        assert!(matches!(err, BlackboardError::Corrupt { .. }), "{err}");
    }

    #[test]
    fn test_load_non_object_top_level_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        std::fs::write(&path, b"[1, 2]").unwrap();
        let err = Blackboard::load(path).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "blackboard file {} is corrupt: top level is not a JSON object",
                dir.path().join("s.json").display()
            )
        );
    }

    #[test]
    fn test_set_path_writes_scalars_and_nested_paths() {
        let mut bb = Blackboard::new_empty(PathBuf::from("unused/s.json"));
        bb.set_path(&segs(&["duration"]), json!(2.5)).unwrap();
        bb.set_path(&segs(&["report", "frames"]), json!(30))
            .unwrap();
        assert_eq!(
            *bb.value(),
            json!({"duration": 2.5, "report": {"frames": 30}})
        );
    }

    #[test]
    fn test_set_path_overwrites_and_creates_through_null() {
        let mut bb = Blackboard::new_empty(PathBuf::from("unused/s.json"));
        bb.set_path(&segs(&["a"]), Value::Null).unwrap();
        // A null intermediate is treated as absent (matching `has()`'s
        // view) and becomes an object.
        bb.set_path(&segs(&["a", "b"]), json!(1)).unwrap();
        assert_eq!(*bb.value(), json!({"a": {"b": 1}}));
        bb.set_path(&segs(&["a"]), json!("replaced")).unwrap();
        assert_eq!(*bb.value(), json!({"a": "replaced"}));
    }

    #[test]
    fn test_set_path_through_non_object_intermediate_is_an_error() {
        let mut bb = Blackboard::new_empty(PathBuf::from("unused/s.json"));
        bb.set_path(&segs(&["a"]), json!(5)).unwrap();
        let err = bb.set_path(&segs(&["a", "b", "c"]), json!(1)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "cannot set `session.a.b.c`: `session.a` is not an object"
        );
        // The final segment's parent gets the same check.
        let err = bb.set_path(&segs(&["a", "b"]), json!(1)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "cannot set `session.a.b`: `session.a` is not an object"
        );
    }

    #[test]
    fn test_set_path_rejects_reserved_roots() {
        let mut bb = Blackboard::new_empty(PathBuf::from("unused/s.json"));
        let err = bb
            .set_path(&segs(&["_once", "k"]), json!(true))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`session._once.k` writes reserved engine state — `session._*` keys \
             (`session._once`, `session._triggers`) cannot be set by a document"
        );
    }

    #[test]
    fn test_set_path_rejects_the_empty_path() {
        let mut bb = Blackboard::new_empty(PathBuf::from("unused/s.json"));
        assert_eq!(bb.set_path(&[], json!(1)), Err(SetPathError::EmptyPath));
    }

    #[tokio::test]
    async fn test_persist_and_reload_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-1.json");
        let mut bb = Blackboard::new_empty(path.clone());
        bb.set_path(&segs(&["target_adu"]), json!(32767.5)).unwrap();
        bb.persist().await.unwrap();

        let reloaded = Blackboard::load(path).unwrap();
        assert_eq!(reloaded.value(), bb.value());
    }

    #[tokio::test]
    async fn test_persist_leaves_no_staging_files_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        let bb = Blackboard::new_empty(path);
        bb.persist().await.unwrap();
        bb.persist().await.unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("s.json")]);
    }

    #[tokio::test]
    async fn test_persist_creates_the_state_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join("nested").join("s.json");
        let bb = Blackboard::new_empty(path.clone());
        bb.persist().await.unwrap();
        assert!(path.is_file());
    }

    #[tokio::test]
    async fn test_persist_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        // Make the rename fail: the destination is a directory.
        let path = dir.path().join("s.json");
        std::fs::create_dir(&path).unwrap();
        let bb = Blackboard::new_empty(path);
        let err = bb.persist().await.unwrap_err();
        assert!(matches!(err, BlackboardError::Write { .. }), "{err}");
    }

    #[tokio::test]
    async fn test_replace_deletes_a_leftover_file_eagerly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-1.json");
        // A stale file from an earlier incarnation of the same session id
        // — with a once-marker that would wrongly skip work if it ever
        // resurfaced on a recovery invocation.
        std::fs::write(&path, br#"{"_once": {"panel-on": true}, "stale": 1}"#).unwrap();

        let bb = Blackboard::replace(path.clone()).await.unwrap();
        assert_eq!(*bb.value(), json!({}));
        assert!(
            !path.exists(),
            "the leftover file must be gone even if this session never persists"
        );
        // Reloading (what a recovery invocation does) now sees a fresh
        // session, not the stale state.
        assert_eq!(*Blackboard::load(path).unwrap().value(), json!({}));
    }

    #[tokio::test]
    async fn test_replace_without_a_leftover_file_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        let bb = Blackboard::replace(dir.path().join("nope.json"))
            .await
            .unwrap();
        assert_eq!(*bb.value(), json!({}));
    }

    #[tokio::test]
    async fn test_once_markers_record_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        let mut bb = Blackboard::new_empty(path.clone());
        assert!(!bb.once_done("panel-on"));
        bb.mark_once("panel-on").await.unwrap();
        assert!(bb.once_done("panel-on"));

        let reloaded = Blackboard::load(path).unwrap();
        assert!(reloaded.once_done("panel-on"));
        assert_eq!(*reloaded.value(), json!({"_once": {"panel-on": true}}));
    }

    #[test]
    fn test_once_done_requires_a_true_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        std::fs::write(&path, br#"{"_once": {"a": false, "b": 1}, "c": 2}"#).unwrap();
        let bb = Blackboard::load(path).unwrap();
        assert!(!bb.once_done("a"));
        assert!(!bb.once_done("b"));
        assert!(!bb.once_done("missing"));
    }

    #[tokio::test]
    async fn test_mark_once_heals_a_corrupt_marker_map() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        std::fs::write(&path, br#"{"_once": 5}"#).unwrap();
        let mut bb = Blackboard::load(path).unwrap();
        assert!(!bb.once_done("k"));
        bb.mark_once("k").await.unwrap();
        assert!(bb.once_done("k"));
        assert_eq!(*bb.value(), json!({"_once": {"k": true}}));
    }
}
