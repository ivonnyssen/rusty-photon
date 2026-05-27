//! Config-action protocol for the Deep Sky Dad FP2 driver.
//!
//! Implements the `config.get` / `config.apply` vendor ASCOM actions described
//! in [`docs/services/dsd-fp2.md`] "Config Actions" and the cross-driver plan
//! [`docs/plans/ui-design/config-actions.md`]. This module is the *pure* logic
//! (validate / classify / redact / diff / atomic save); the ASCOM dispatch and
//! the fire-after-response reload live on `DsdFp2Device` in `device.rs`.
//!
//! [`docs/services/dsd-fp2.md`]: ../../../docs/services/dsd-fp2.md
//! [`docs/plans/ui-design/config-actions.md`]: ../../../docs/plans/ui-design/config-actions.md

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{CliOverrides, Config};
use crate::protocol::MAX_BRIGHTNESS;

/// Sentinel emitted by `config.get` in place of a redacted secret, and
/// recognised by `config.apply` as "leave this secret unchanged".
pub const REDACTED: &str = "********";

/// JSON pointer to the one secret the FP2 config carries. `TlsConfig` stores
/// file *paths*, not key material, so there is nothing to redact there.
const SECRET_POINTER: &str = "/server/auth/password_hash";

/// The two config actions. Mirrors the `name()` / `from_name()` / `ALL` shape
/// of `star-adventurer-gti`'s `ApParkAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigAction {
    Get,
    Apply,
}

impl ConfigAction {
    pub const ALL: [ConfigAction; 2] = [ConfigAction::Get, ConfigAction::Apply];

    pub fn name(self) -> &'static str {
        match self {
            ConfigAction::Get => "config.get",
            ConfigAction::Apply => "config.apply",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "config.get" => Some(ConfigAction::Get),
            "config.apply" => Some(ConfigAction::Apply),
            _ => None,
        }
    }
}

/// `config.get` response body.
#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigGetResponse {
    /// The effective config, secrets redacted.
    pub config: Value,
    /// Dotted JSON paths pinned by a CLI override; `config.apply` won't persist
    /// them.
    pub overrides: Vec<String>,
}

/// Outcome of a `config.apply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApplyStatus {
    /// Persisted and an in-process reload is in flight.
    Applying,
    /// Persisted; nothing needed a reload.
    Ok,
    /// Validation failed; the file is unchanged.
    Invalid,
}

/// A single field-level validation error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    pub path: String,
    pub msg: String,
}

/// `config.apply` response body. Classification arrays are always present for a
/// stable shape; `persisted_to`/`errors` appear only when relevant.
#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigApplyResponse {
    pub status: ApplyStatus,
    /// Took effect live, no reload. Unused by `dsd-fp2` in Phase 1.
    pub applied: Vec<String>,
    /// Applied via in-process reload.
    pub reload: Vec<String>,
    /// Would need a Sentinel process restart. Unused by `dsd-fp2` in Phase 1.
    pub restart_required: Vec<String>,
    /// Submitted but not persisted because the field is CLI-override-pinned.
    pub skipped_override: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persisted_to: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub errors: Vec<FieldError>,
}

impl ConfigApplyResponse {
    /// Build the validation-failure response (`status:"invalid"`, file unchanged).
    pub fn invalid(errors: Vec<FieldError>) -> Self {
        Self {
            status: ApplyStatus::Invalid,
            applied: Vec::new(),
            reload: Vec::new(),
            restart_required: Vec::new(),
            skipped_override: Vec::new(),
            persisted_to: None,
            errors,
        }
    }
}

/// Normalize a submitted config in place before validation/persist. Trims
/// surrounding whitespace from `serial.port`, which is otherwise used verbatim
/// to open the device — so `" /dev/ttyACM0 "` would become an invalid runtime
/// path despite passing the non-empty check.
pub fn normalize(config: &mut Config) {
    let trimmed = config.serial.port.trim();
    if trimmed.len() != config.serial.port.len() {
        config.serial.port = trimmed.to_string();
    }
}

/// Validate a parsed config. An empty result means valid.
pub fn validate(config: &Config) -> Vec<FieldError> {
    let mut errors = Vec::new();
    if config.serial.port.trim().is_empty() {
        errors.push(FieldError {
            path: "serial.port".to_string(),
            msg: "must not be empty".to_string(),
        });
    }
    if config.serial.baud_rate == 0 {
        errors.push(FieldError {
            path: "serial.baud_rate".to_string(),
            msg: "must be greater than 0".to_string(),
        });
    }
    if config.serial.polling_interval.is_zero() {
        errors.push(FieldError {
            path: "serial.polling_interval".to_string(),
            msg: "must be greater than 0".to_string(),
        });
    }
    if config.serial.timeout.is_zero() {
        errors.push(FieldError {
            path: "serial.timeout".to_string(),
            msg: "must be greater than 0".to_string(),
        });
    }
    if config.cover_calibrator.max_brightness > MAX_BRIGHTNESS as u32 {
        errors.push(FieldError {
            path: "cover_calibrator.max_brightness".to_string(),
            msg: format!("must be <= {MAX_BRIGHTNESS} (hardware ceiling)"),
        });
    }
    if config.cover_calibrator.unique_id.trim().is_empty() {
        errors.push(FieldError {
            path: "cover_calibrator.unique_id".to_string(),
            msg: "must not be empty (it is the device's stable ASCOM UniqueID)".to_string(),
        });
    }
    errors
}

/// Redact secret material in a serialized config `Value`, in place.
pub fn redact_value(value: &mut Value) {
    if let Some(secret) = value.pointer_mut(SECRET_POINTER) {
        if secret.is_string() {
            *secret = Value::String(REDACTED.to_string());
        }
    }
}

/// Read the persisted config as a `Value`:
///
/// * absent → `Ok(default)` — a fresh install; `config.apply` will create it,
/// * present and valid → `Ok(value)`,
/// * present but unparseable → `Err` — so `config.apply` surfaces it rather than
///   silently treating the file as default and overwriting (losing) a corrupt
///   file's contents on the layer-aware persist.
pub fn read_file_value(path: &Path) -> std::result::Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(|e| {
            format!(
                "existing config file {} is not valid JSON: {e}",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default_value()),
        Err(e) => Err(format!(
            "could not read config file {}: {e}",
            path.display()
        )),
    }
}

fn default_value() -> Value {
    serde_json::to_value(Config::default()).unwrap_or(Value::Null)
}

/// Convert a dotted path (`serial.port`) to a JSON pointer (`/serial/port`).
fn json_pointer(dotted: &str) -> String {
    let mut pointer = String::with_capacity(dotted.len() + 1);
    pointer.push('/');
    pointer.push_str(&dotted.replace('.', "/"));
    pointer
}

/// Build the `Value` to persist from a validated `submitted` config:
///
/// * CLI-override-pinned fields are written through from `file_current` (so a
///   transient `--port` is never baked into the file), and
/// * a redacted/sentinel secret means "keep the file's existing value".
///
/// Returns the value to write plus the dotted paths that were skipped because
/// they are override-pinned.
pub fn build_persist_value(
    submitted: &Config,
    file_current: &Value,
    overrides: &CliOverrides,
) -> serde_json::Result<(Value, Vec<String>)> {
    let mut to_write = serde_json::to_value(submitted)?;

    let skipped = overrides.pinned_paths();
    for path in &skipped {
        let pointer = json_pointer(path);
        if let (Some(file_val), Some(slot)) = (
            file_current.pointer(&pointer).cloned(),
            to_write.pointer_mut(&pointer),
        ) {
            *slot = file_val;
        }
    }

    // A round-tripped redacted secret keeps the prior value.
    let submitted_secret_is_sentinel = to_write
        .pointer(SECRET_POINTER)
        .and_then(Value::as_str)
        .is_some_and(|s| s == REDACTED);
    if submitted_secret_is_sentinel {
        if let (Some(file_secret), Some(slot)) = (
            file_current.pointer(SECRET_POINTER).cloned(),
            to_write.pointer_mut(SECRET_POINTER),
        ) {
            *slot = file_secret;
        }
    }

    Ok((to_write, skipped))
}

/// Whether the submitted config carries the redaction sentinel for the auth
/// password hash while the on-disk config has no stored secret to restore.
///
/// The sentinel means "keep the stored secret unchanged"; with nothing stored
/// there is nothing to keep, so honouring it would persist `********` as the
/// real hash. `config.apply` rejects this case as a domain error.
pub fn redacted_secret_without_prior(submitted: &Config, file_current: &Value) -> bool {
    let submitted_is_sentinel = submitted
        .server
        .auth
        .as_ref()
        .is_some_and(|auth| auth.password_hash == REDACTED);
    submitted_is_sentinel
        && file_current
            .pointer(SECRET_POINTER)
            .and_then(Value::as_str)
            .is_none()
}

/// Apply CLI overrides onto a config `Value`, producing the effective config the
/// reloaded server would run.
pub fn effective_value(file_value: &Value, overrides: &CliOverrides) -> Value {
    let mut value = file_value.clone();
    if let Some(port) = &overrides.serial_port {
        if let Some(slot) = value.pointer_mut("/serial/port") {
            *slot = Value::String(port.clone());
        }
    }
    if let Some(port) = overrides.server_port {
        if let Some(slot) = value.pointer_mut("/server/port") {
            *slot = Value::Number(port.into());
        }
    }
    value
}

/// Dotted JSON paths whose leaf values differ between `before` and `after`.
/// Objects recurse; everything else compares by equality.
pub fn diff_paths(before: &Value, after: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    diff_into(before, after, "", &mut paths);
    paths
}

fn diff_into(before: &Value, after: &Value, prefix: &str, out: &mut Vec<String>) {
    match (before, after) {
        (Value::Object(b), Value::Object(a)) => {
            let keys: BTreeSet<&String> = b.keys().chain(a.keys()).collect();
            for key in keys {
                let child = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match (b.get(key), a.get(key)) {
                    (Some(bv), Some(av)) => diff_into(bv, av, &child, out),
                    _ => out.push(child),
                }
            }
        }
        _ => {
            if before != after {
                out.push(prefix.to_string());
            }
        }
    }
}

/// Atomically persist `value` as pretty JSON to `path`: create parent dirs,
/// stage to a temp file in the same directory, fsync, rename, then fsync the
/// directory.
pub fn save(path: &Path, value: &Value) -> std::io::Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent)?;

    let mut bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    bytes.push(b'\n');

    // Stage to a uniquely-named temp file in the same directory. `NamedTempFile`
    // creates it with `O_EXCL` and a random name, so concurrent `config.apply`
    // calls can't collide on a predictable path and a pre-planted symlink can't
    // redirect the write. fsync the contents, atomically rename into place, then
    // fsync the directory so the rename is durable. The temp file is removed on
    // drop if anything fails before the rename.
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(&bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;

    // fsync the parent directory so the rename itself is durable. Windows can't
    // open a directory as a regular file handle, so this is unix-only (matching
    // the repo's other atomic-write helpers, e.g. rp's persistence::document).
    #[cfg(unix)]
    {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use crate::config::{Config, CoverCalibratorConfig, SerialConfig, ServerConfig};
    use rp_auth::config::AuthConfig;
    use std::time::Duration;

    fn sample_config() -> Config {
        Config::default()
    }

    /// A config that is valid for `validate()`: like `Config::default()` but with
    /// a populated `cover_calibrator.unique_id` (the default is empty because the
    /// id is minted on first run by `materialize_identity`).
    fn valid_config() -> Config {
        Config {
            cover_calibrator: CoverCalibratorConfig {
                unique_id: "dsd-fp2-test-id".to_string(),
                ..CoverCalibratorConfig::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn action_names_round_trip() {
        for action in ConfigAction::ALL {
            assert_eq!(ConfigAction::from_name(action.name()), Some(action));
        }
        assert_eq!(ConfigAction::from_name("config.nope"), None);
        assert_eq!(ConfigAction::Get.name(), "config.get");
        assert_eq!(ConfigAction::Apply.name(), "config.apply");
    }

    #[test]
    fn validate_accepts_populated_config() {
        // `Config::default()` now has an empty `unique_id` (minted on first run),
        // so a *populated* config is the valid baseline.
        assert!(validate(&valid_config()).is_empty());
    }

    #[test]
    fn validate_rejects_empty_unique_id() {
        // Whitespace-only counts as empty, just like `serial.port`.
        for id in ["", "   ", "\t\n"] {
            let config = Config {
                cover_calibrator: CoverCalibratorConfig {
                    unique_id: id.to_string(),
                    ..CoverCalibratorConfig::default()
                },
                ..Config::default()
            };
            let errors = validate(&config);
            let err = errors
                .iter()
                .find(|e| e.path == "cover_calibrator.unique_id")
                .unwrap_or_else(|| panic!("expected a unique_id error for {id:?}, got {errors:?}"));
            assert_eq!(
                err.msg,
                "must not be empty (it is the device's stable ASCOM UniqueID)"
            );
        }
        // And a populated id passes (no unique_id error).
        assert!(validate(&valid_config())
            .iter()
            .all(|e| e.path != "cover_calibrator.unique_id"));
    }

    #[test]
    fn normalize_trims_serial_port() {
        let mut config = Config {
            serial: SerialConfig {
                port: "  /dev/ttyACM0\n".to_string(),
                ..SerialConfig::default()
            },
            ..Config::default()
        };
        normalize(&mut config);
        assert_eq!(config.serial.port, "/dev/ttyACM0");
    }

    #[test]
    fn validate_flags_each_bad_field() {
        let config = Config {
            serial: SerialConfig {
                port: "   ".to_string(),
                baud_rate: 0,
                polling_interval: Duration::ZERO,
                timeout: Duration::ZERO,
            },
            server: ServerConfig::default(),
            cover_calibrator: CoverCalibratorConfig {
                max_brightness: 9999,
                ..CoverCalibratorConfig::default()
            },
        };
        let errors = validate(&config);
        let paths: Vec<&str> = errors.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"serial.port"));
        assert!(paths.contains(&"serial.baud_rate"));
        assert!(paths.contains(&"serial.polling_interval"));
        assert!(paths.contains(&"serial.timeout"));
        assert!(paths.contains(&"cover_calibrator.max_brightness"));
    }

    #[test]
    fn validate_accepts_max_brightness_at_ceiling() {
        let config = Config {
            cover_calibrator: CoverCalibratorConfig {
                max_brightness: MAX_BRIGHTNESS as u32,
                unique_id: "dsd-fp2-test-id".to_string(),
                ..CoverCalibratorConfig::default()
            },
            ..Config::default()
        };
        assert!(validate(&config).is_empty());
    }

    #[test]
    fn redact_replaces_password_hash() {
        let mut config = sample_config();
        config.server.auth = Some(AuthConfig {
            username: "obs".to_string(),
            password_hash: "$argon2id$v=19$secret".to_string(),
        });
        let mut value = serde_json::to_value(&config).unwrap();
        redact_value(&mut value);
        assert_eq!(
            value.pointer(SECRET_POINTER).and_then(Value::as_str),
            Some(REDACTED)
        );
        // Username is not a secret and is left intact.
        assert_eq!(
            value
                .pointer("/server/auth/username")
                .and_then(Value::as_str),
            Some("obs")
        );
    }

    #[test]
    fn redact_is_noop_without_auth() {
        let mut value = serde_json::to_value(sample_config()).unwrap();
        redact_value(&mut value);
        assert!(value.pointer(SECRET_POINTER).is_none());
    }

    #[test]
    fn diff_paths_reports_changed_leaves_only() {
        let before = serde_json::to_value(Config::default()).unwrap();
        let mut after_cfg = Config::default();
        after_cfg.cover_calibrator.max_brightness = 2048;
        after_cfg.serial.baud_rate = 9600;
        let after = serde_json::to_value(&after_cfg).unwrap();

        let mut paths = diff_paths(&before, &after);
        paths.sort();
        assert_eq!(
            paths,
            vec!["cover_calibrator.max_brightness", "serial.baud_rate"]
        );
    }

    #[test]
    fn diff_paths_empty_when_equal() {
        let value = serde_json::to_value(Config::default()).unwrap();
        assert!(diff_paths(&value, &value).is_empty());
    }

    #[test]
    fn build_persist_skips_override_fields() {
        // File has its own serial.port; an override pins a different effective value.
        let mut file_cfg = Config::default();
        file_cfg.serial.port = "/dev/ttyACM0".to_string();
        let file_value = serde_json::to_value(&file_cfg).unwrap();

        // Submitted blob carries the override value (as config.get would have shown).
        let mut submitted = Config::default();
        submitted.serial.port = "/dev/ttyACM9".to_string();
        submitted.cover_calibrator.max_brightness = 1234;

        let overrides = CliOverrides {
            serial_port: Some("/dev/ttyACM9".to_string()),
            server_port: None,
        };

        let (to_write, skipped) = build_persist_value(&submitted, &file_value, &overrides).unwrap();

        // The override-pinned field keeps the FILE's value, not the submitted one.
        assert_eq!(
            to_write.pointer("/serial/port").and_then(Value::as_str),
            Some("/dev/ttyACM0")
        );
        // Non-override edits are persisted.
        assert_eq!(
            to_write
                .pointer("/cover_calibrator/max_brightness")
                .and_then(Value::as_u64),
            Some(1234)
        );
        assert_eq!(skipped, vec!["serial.port".to_string()]);
    }

    #[test]
    fn build_persist_keeps_secret_when_sentinel_submitted() {
        let mut file_cfg = Config::default();
        file_cfg.server.auth = Some(AuthConfig {
            username: "obs".to_string(),
            password_hash: "$argon2id$real".to_string(),
        });
        let file_value = serde_json::to_value(&file_cfg).unwrap();

        // Submission round-trips the redaction sentinel.
        let mut submitted = file_cfg.clone();
        if let Some(auth) = submitted.server.auth.as_mut() {
            auth.password_hash = REDACTED.to_string();
        }

        let (to_write, _) =
            build_persist_value(&submitted, &file_value, &CliOverrides::default()).unwrap();

        assert_eq!(
            to_write.pointer(SECRET_POINTER).and_then(Value::as_str),
            Some("$argon2id$real")
        );
    }

    #[test]
    fn effective_value_applies_overrides() {
        let file_value = serde_json::to_value(Config::default()).unwrap();
        let overrides = CliOverrides {
            serial_port: Some("/dev/ttyACM5".to_string()),
            server_port: Some(12345),
        };
        let effective = effective_value(&file_value, &overrides);
        assert_eq!(
            effective.pointer("/serial/port").and_then(Value::as_str),
            Some("/dev/ttyACM5")
        );
        assert_eq!(
            effective.pointer("/server/port").and_then(Value::as_u64),
            Some(12345)
        );
    }

    #[test]
    fn save_round_trips_and_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dsd-fp2.json");
        let value = serde_json::to_value(Config::default()).unwrap();

        save(&path, &value).unwrap();

        let read_back: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read_back, value);
        // No stray temp file left behind — the directory holds only the config.
        let entries: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec!["dsd-fp2.json".to_string()],
            "stray files left behind: {entries:?}"
        );
    }

    #[test]
    fn sentinel_secret_without_prior_is_rejected() {
        let sentinel_auth = Some(AuthConfig {
            username: "obs".to_string(),
            password_hash: REDACTED.to_string(),
        });

        // Sentinel submitted, but the file has no stored secret → rejected.
        let submitted = Config {
            server: ServerConfig {
                auth: sentinel_auth.clone(),
                ..ServerConfig::default()
            },
            ..Config::default()
        };
        let file_without_secret = serde_json::to_value(Config::default()).unwrap();
        assert!(redacted_secret_without_prior(
            &submitted,
            &file_without_secret
        ));

        // With a stored secret to restore, the sentinel is acceptable.
        let file_cfg = Config {
            server: ServerConfig {
                auth: Some(AuthConfig {
                    username: "obs".to_string(),
                    password_hash: "$argon2id$real".to_string(),
                }),
                ..ServerConfig::default()
            },
            ..Config::default()
        };
        let file_with_secret = serde_json::to_value(&file_cfg).unwrap();
        assert!(!redacted_secret_without_prior(
            &submitted,
            &file_with_secret
        ));

        // A real (non-sentinel) submitted hash is always fine.
        let real = Config {
            server: ServerConfig {
                auth: Some(AuthConfig {
                    username: "obs".to_string(),
                    password_hash: "$argon2id$new".to_string(),
                }),
                ..ServerConfig::default()
            },
            ..Config::default()
        };
        assert!(!redacted_secret_without_prior(&real, &file_without_secret));
    }

    #[test]
    fn read_file_value_defaults_when_missing() {
        let value = read_file_value(Path::new("/tmp/dsd_fp2_does_not_exist_42.json")).unwrap();
        assert_eq!(value, serde_json::to_value(Config::default()).unwrap());
    }

    #[test]
    fn read_file_value_errors_on_present_but_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dsd-fp2.json");
        std::fs::write(&path, "{ not valid json").unwrap();
        let err = read_file_value(&path).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
    }
}
