//! Shared config helpers for rusty-photon drivers.
//!
//! ASCOM Alpaca requires every device's `UniqueID` to be **globally unique** and to **never
//! change**, but the protocol enforces neither — uniqueness has to come from how the id is
//! generated. This crate gives each driver a spec-compliant identity: it resolves a per-user
//! config path, and [`materialize_identity`] mints a UUIDv4 for each device on first run,
//! persists it atomically, and never overwrites an id that already exists.
//!
//! The helpers operate on `serde_json::Value` + JSON pointers so they apply uniformly across the
//! heterogeneous driver config shapes (one device or several, at different pointers).

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde_json::Value;
use uuid::Uuid;

/// Errors from config-path resolution, reading, or persistence.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// No platform config directory could be determined for the default path.
    #[error("could not determine a platform config directory")]
    NoConfigDir,
    /// The config file exists but is not valid JSON.
    #[error("config file {path} is not valid JSON: {source}")]
    InvalidJson {
        path: PathBuf,
        source: serde_json::Error,
    },
    /// The config file could not be read.
    #[error("could not read config file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The config file could not be persisted.
    #[error("could not persist config file {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Resolve the config-file path: the explicit `--config` path if given, else the per-user platform
/// config directory (e.g. `~/.config/rusty-photon/<service>.json` on Linux). A path is always
/// resolvable, so config persistence is never disabled for lack of one.
pub fn resolve_config_path(
    service: &str,
    explicit: Option<PathBuf>,
) -> Result<PathBuf, ConfigError> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let dirs = ProjectDirs::from("", "", "rusty-photon").ok_or(ConfigError::NoConfigDir)?;
    Ok(dirs.config_dir().join(format!("{service}.json")))
}

/// Read the file at `path` as a JSON `Value`; a missing file yields a clone of `default`, while a
/// present-but-corrupt file is an error (so a typo never silently resets config).
pub fn read_file_value(path: &Path, default: &Value) -> Result<Value, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(|source| ConfigError::InvalidJson {
            path: path.to_path_buf(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default.clone()),
        Err(source) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Atomically persist `value` as pretty JSON: create parent dirs, stage to a uniquely-named temp
/// file in the same directory, fsync, rename into place, then fsync the directory (Unix) so the
/// rename itself is durable.
pub fn save(path: &Path, value: &Value) -> std::io::Result<()> {
    use std::io::Write as _;

    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent)?;

    let mut bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    bytes.push(b'\n');

    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(&bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;

    #[cfg(unix)]
    {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// The result of [`materialize_identity`].
pub struct MaterializeOutcome {
    /// Whether the file was (re)written (i.e. at least one id was minted).
    pub wrote: bool,
    /// The JSON pointers that received a freshly-minted id.
    pub filled: Vec<String>,
    /// The post-materialization file `Value`.
    pub value: Value,
}

/// Ensure every pointer in `identity_pointers` holds a non-empty string UniqueID in the **file
/// layer**, minting a fresh UUIDv4 for any that are absent, non-string, or empty. Idempotent (only
/// fills empties; never overwrites an existing id) and persists only when it actually filled
/// something. Operates solely on the on-disk file (never a CLI-override-applied effective config),
/// so a transient `--port` is never baked in.
pub fn materialize_identity(
    path: &Path,
    default_value: &Value,
    identity_pointers: &[&str],
) -> Result<MaterializeOutcome, ConfigError> {
    let mut value = read_file_value(path, default_value)?;
    let mut filled = Vec::new();

    for ptr in identity_pointers {
        let needs = match value.pointer(ptr) {
            Some(Value::String(s)) => s.trim().is_empty(),
            _ => true, // absent, null, or non-string
        };
        if needs {
            insert_at_pointer(&mut value, ptr, Value::String(Uuid::new_v4().to_string()));
            filled.push((*ptr).to_string());
        }
    }

    let wrote = if filled.is_empty() {
        false
    } else {
        save(path, &value).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
        true
    };

    Ok(MaterializeOutcome {
        wrote,
        filled,
        value,
    })
}

/// Set `new` at the RFC-6901 JSON `pointer`, creating intermediate objects as needed (unlike
/// `Value::pointer_mut`, which returns `None` for a missing key).
fn insert_at_pointer(root: &mut Value, pointer: &str, new: Value) {
    let tokens: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect();
    let Some((last, parents)) = tokens.split_last() else {
        return;
    };

    let mut cur = root;
    for tok in parents {
        if !cur.is_object() {
            *cur = Value::Object(serde_json::Map::new());
        }
        let Some(map) = cur.as_object_mut() else {
            return;
        };
        cur = map
            .entry(tok.clone())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }

    if !cur.is_object() {
        *cur = Value::Object(serde_json::Map::new());
    }
    if let Some(map) = cur.as_object_mut() {
        map.insert(last.clone(), new);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_uses_explicit_path() {
        let p = resolve_config_path("dsd-fp2", Some(PathBuf::from("/tmp/x.json"))).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/x.json"));
    }

    #[test]
    fn resolve_defaults_to_platform_dir() {
        let p = resolve_config_path("dsd-fp2", None).unwrap();
        assert!(p.ends_with("dsd-fp2.json"), "{p:?}");
        assert!(p.to_string_lossy().contains("rusty-photon"), "{p:?}");
    }

    #[test]
    fn materialize_fills_empty_and_persists_valid_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let default = json!({ "cover_calibrator": { "unique_id": "" } });

        let out = materialize_identity(&path, &default, &["/cover_calibrator/unique_id"]).unwrap();

        assert!(out.wrote);
        assert_eq!(out.filled, vec!["/cover_calibrator/unique_id".to_string()]);
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let id = on_disk
            .pointer("/cover_calibrator/unique_id")
            .and_then(Value::as_str)
            .unwrap();
        Uuid::parse_str(id).unwrap();
    }

    #[test]
    fn materialize_is_idempotent_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let default = json!({ "d": { "unique_id": "" } });

        let first = materialize_identity(&path, &default, &["/d/unique_id"]).unwrap();
        assert!(first.wrote);
        let id1 = first
            .value
            .pointer("/d/unique_id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();

        let second = materialize_identity(&path, &default, &["/d/unique_id"]).unwrap();
        assert!(!second.wrote);
        assert!(second.filled.is_empty());
        let id2 = second
            .value
            .pointer("/d/unique_id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        assert_eq!(id1, id2);
    }

    #[test]
    fn materialize_never_overwrites_existing_and_fills_only_empties() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        std::fs::write(
            &path,
            r#"{"a":{"unique_id":"keep-me"},"b":{"unique_id":""}}"#,
        )
        .unwrap();

        let out =
            materialize_identity(&path, &json!({}), &["/a/unique_id", "/b/unique_id"]).unwrap();

        assert!(out.wrote);
        assert_eq!(out.filled, vec!["/b/unique_id".to_string()]);
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            on_disk.pointer("/a/unique_id").and_then(Value::as_str),
            Some("keep-me")
        );
        let b = on_disk
            .pointer("/b/unique_id")
            .and_then(Value::as_str)
            .unwrap();
        Uuid::parse_str(b).unwrap();
    }

    #[test]
    fn materialize_absent_file_writes_default_scaffold() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");
        let default = json!({ "serial": { "port": "/dev/ttyACM0" }, "d": { "unique_id": "" } });

        let out = materialize_identity(&path, &default, &["/d/unique_id"]).unwrap();

        assert!(out.wrote);
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Non-identity defaults are written through; identity is minted.
        assert_eq!(
            on_disk.pointer("/serial/port").and_then(Value::as_str),
            Some("/dev/ttyACM0")
        );
        assert!(on_disk
            .pointer("/d/unique_id")
            .and_then(Value::as_str)
            .is_some());
    }

    #[test]
    fn materialize_inserts_absent_pointer_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        // Present file whose device object lacks `unique_id` entirely.
        std::fs::write(&path, r#"{"device":{"name":"cam"}}"#).unwrap();

        let out = materialize_identity(&path, &json!({}), &["/device/unique_id"]).unwrap();

        assert!(out.wrote);
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(on_disk
            .pointer("/device/unique_id")
            .and_then(Value::as_str)
            .is_some());
        assert_eq!(
            on_disk.pointer("/device/name").and_then(Value::as_str),
            Some("cam")
        );
    }

    #[test]
    fn read_file_value_missing_returns_default() {
        let v = read_file_value(
            Path::new("/tmp/rusty-photon-config-definitely-missing-zzz.json"),
            &json!({ "x": 1 }),
        )
        .unwrap();
        assert_eq!(v, json!({ "x": 1 }));
    }

    #[test]
    fn read_file_value_corrupt_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{ not json").unwrap();
        let err = read_file_value(&path, &json!({})).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidJson { .. }), "{err:?}");
    }

    #[test]
    fn save_round_trips_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        save(&path, &json!({ "k": "v" })).unwrap();

        let back: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back, json!({ "k": "v" }));

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != "s.json")
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files present");
    }
}
