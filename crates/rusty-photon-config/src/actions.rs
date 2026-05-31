//! The `config.get` / `config.apply` / `config.schema` action protocol.
//!
//! ASCOM Alpaca drivers expose their configuration over HTTP as three vendor
//! [`Action`]s. This module holds the **driver-agnostic** machinery — the wire
//! envelopes, the layer-aware persist, secret redaction, the effective-config
//! diff, and the JSON-Schema generation — behind a single [`ConfigurableDriver`]
//! trait that each driver implements for its own `Config` type. The cross-driver
//! protocol and editability tiers are documented in
//! [`docs/services/config-actions.md`] and the plan
//! [`docs/plans/ui-design/config-actions.md`].
//!
//! The generic functions return plain values / [`ConfigError`]-flavoured errors;
//! the per-driver `device.rs` wraps those into `ascom_alpaca::ASCOMResult`, so
//! this crate stays free of an `ascom-alpaca` dependency.
//!
//! [`Action`]: https://ascom-standards.org/api/
//! [`docs/services/config-actions.md`]: ../../../docs/services/config-actions.md
//! [`docs/plans/ui-design/config-actions.md`]: ../../../docs/plans/ui-design/config-actions.md

use std::collections::BTreeSet;
use std::path::Path;

use schemars::{schema_for, JsonSchema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{read_file_value, save, ConfigError};

/// Sentinel emitted by `config.get` in place of a redacted secret, and
/// recognised by `config.apply` as "leave this secret unchanged".
pub const REDACTED: &str = "********";

/// The three config actions. Drivers advertise these via `supported_actions()`
/// and dispatch on them in `action()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigAction {
    Get,
    Apply,
    Schema,
}

impl ConfigAction {
    pub const ALL: [ConfigAction; 3] =
        [ConfigAction::Get, ConfigAction::Apply, ConfigAction::Schema];

    pub fn name(self) -> &'static str {
        match self {
            ConfigAction::Get => "config.get",
            ConfigAction::Apply => "config.apply",
            ConfigAction::Schema => "config.schema",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "config.get" => Some(ConfigAction::Get),
            "config.apply" => Some(ConfigAction::Apply),
            "config.schema" => Some(ConfigAction::Schema),
            _ => None,
        }
    }
}

/// Per-driver configuration metadata. All invariant protocol logic lives in the
/// free functions below ([`config_get`], [`config_apply`], [`config_schema`]),
/// generic over this trait; an implementor supplies only what *varies* per
/// driver — its `Config` type, validation, secrets, and editability tiers.
pub trait ConfigurableDriver {
    /// The driver's config type.
    type Config: Serialize + DeserializeOwned + JsonSchema + Default;
    /// The driver's CLI-override carrier (`()` if the driver has no overrides).
    type Overrides;

    /// Trim / canonicalize a submitted config in place before validation and
    /// persist (e.g. trim a whitespace-padded `serial.port`).
    fn normalize(config: &mut Self::Config);

    /// Domain validation. An empty result means valid. Paths are dotted
    /// (`"serial.port"`) so the UI can render each error next to its field.
    fn validate(config: &Self::Config) -> Vec<FieldError>;

    /// RFC-6901 JSON pointers to secret leaves, redacted by `config.get` and
    /// carried forward (rather than overwritten with the sentinel) by
    /// `config.apply`. Empty slice if the driver stores no secrets.
    fn secret_pointers() -> &'static [&'static str];

    /// Dotted paths currently pinned by a CLI override; surfaced in
    /// `config.get`'s `overrides[]` so the UI renders them disabled, and skipped
    /// by `config.apply` so a transient override is never baked into the file.
    fn override_paths(overrides: &Self::Overrides) -> Vec<String>;

    /// Apply CLI overrides onto a config in place, producing the *effective*
    /// config a reloaded server would run.
    fn apply_overrides(config: &mut Self::Config, overrides: &Self::Overrides);

    /// Identity fields (e.g. a device `unique_id`) — read-only by default in the
    /// UI behind an explicit "unlock to edit" escape hatch.
    fn locked_paths() -> &'static [&'static str] {
        &[]
    }

    /// Hard read-only fields the UI must never let the user edit (e.g. a
    /// `server.port` the BFF could not follow across a rebind).
    fn read_only_paths() -> &'static [&'static str] {
        &[]
    }
}

/// A single field-level validation error (`config.apply` `status:"invalid"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    pub path: String,
    pub msg: String,
}

/// `config.get` response body: the effective config (secrets redacted) plus the
/// dotted paths pinned by a CLI override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigGetResponse {
    pub config: Value,
    pub overrides: Vec<String>,
}

/// `config.schema` response body: a JSON Schema for the driver's config plus the
/// editability tiers (which the schema alone cannot express) the UI renders from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSchemaResponse {
    /// JSON Schema (schemars) describing the config's shape and field types.
    pub schema: Value,
    /// Dotted paths that are identity/locked (read-only behind an unlock hatch).
    pub locked_fields: Vec<String>,
    /// Dotted paths that are hard read-only (never editable from the UI).
    pub read_only_fields: Vec<String>,
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

/// `config.apply` response body. Classification arrays are always present for a
/// stable shape; `persisted_to` / `errors` appear only when relevant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigApplyResponse {
    pub status: ApplyStatus,
    /// Took effect live, no reload. Unused while every changed field reloads.
    pub applied: Vec<String>,
    /// Applied via in-process reload.
    pub reload: Vec<String>,
    /// Would need a Sentinel process restart. Unused today.
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

/// Error from [`config_apply`] that the caller maps to an ASCOM error. Validation
/// failures are *not* errors here — they come back as `Ok(ConfigApplyResponse)`
/// with `status:"invalid"` (an HTTP-200 domain error, file untouched).
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// The submitted `Parameters` did not parse into the driver's `Config`.
    #[error("invalid config JSON: {0}")]
    Parse(serde_json::Error),
    /// The on-disk config file is present but unreadable / corrupt.
    #[error("{0}")]
    ReadFile(ConfigError),
    /// Persisting the new config failed.
    #[error("failed to persist config: {0}")]
    Persist(std::io::Error),
    /// An internal (de)serialization step failed — effectively a bug.
    #[error("internal serialization error: {0}")]
    Serialize(serde_json::Error),
}

/// `config.get`: serialize the effective config, redact secrets, and attach the
/// CLI-override-pinned paths.
pub fn config_get<D: ConfigurableDriver>(
    effective: &D::Config,
    overrides: &D::Overrides,
) -> Result<ConfigGetResponse, serde_json::Error> {
    let mut config = serde_json::to_value(effective)?;
    redact_value(&mut config, D::secret_pointers());
    Ok(ConfigGetResponse {
        config,
        overrides: D::override_paths(overrides),
    })
}

/// `config.schema`: a JSON Schema for the driver's config plus its editability
/// tiers. The schema shapes the form; the tier lists gate which fields the UI
/// lets the user edit (JSON Schema cannot express "identity" / "read-only").
pub fn config_schema<D: ConfigurableDriver>() -> ConfigSchemaResponse {
    let schema = schema_for!(D::Config);
    ConfigSchemaResponse {
        schema: serde_json::to_value(&schema).unwrap_or(Value::Null),
        locked_fields: D::locked_paths().iter().map(|s| (*s).to_string()).collect(),
        read_only_fields: D::read_only_paths()
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
    }
}

/// `config.apply`: parse → normalize → validate → layer-aware persist → classify.
///
/// Returns `Ok(ConfigApplyResponse)` for both success (`applying`/`ok`) and
/// validation failure (`invalid`, file untouched). The `reload` field lists the
/// changed effective-config paths; when it is non-empty (`status:"applying"`)
/// the caller fires the in-process reload **after** flushing the response. Hard
/// failures (bad JSON, corrupt file, persist error) come back as [`ApplyError`].
pub fn config_apply<D: ConfigurableDriver>(
    path: &Path,
    overrides: &D::Overrides,
    effective_before: &D::Config,
    submitted_json: &str,
) -> Result<ConfigApplyResponse, ApplyError> {
    let mut submitted: D::Config =
        serde_json::from_str(submitted_json).map_err(ApplyError::Parse)?;
    D::normalize(&mut submitted);

    let mut errors = D::validate(&submitted);

    // A present-but-corrupt file is surfaced (not silently treated as default,
    // which would overwrite and lose its contents on the layer-aware persist).
    let default_value =
        serde_json::to_value(D::Config::default()).map_err(ApplyError::Serialize)?;
    let file_current = read_file_value(path, &default_value).map_err(ApplyError::ReadFile)?;

    let submitted_value = serde_json::to_value(&submitted).map_err(ApplyError::Serialize)?;

    // The redaction sentinel means "keep the stored secret unchanged"; with
    // nothing stored there is nothing to keep, so honouring it would bake the
    // sentinel in as the real secret. Reject as a domain error.
    for ptr in D::secret_pointers() {
        if redacted_secret_without_prior(&submitted_value, &file_current, ptr) {
            errors.push(FieldError {
                path: pointer_to_dotted(ptr),
                msg:
                    "cannot keep an unchanged secret when none is stored; provide the password hash"
                        .to_string(),
            });
        }
    }

    if !errors.is_empty() {
        return Ok(ConfigApplyResponse::invalid(errors));
    }

    let pinned = D::override_paths(overrides);
    let (to_persist, skipped) = build_persist_value(
        &submitted_value,
        &file_current,
        &pinned,
        D::secret_pointers(),
    );

    // Classify what would change in the running (effective) config: the persisted
    // value with CLI overrides re-applied on top.
    let mut effective_after: D::Config =
        serde_json::from_value(to_persist.clone()).map_err(ApplyError::Serialize)?;
    D::apply_overrides(&mut effective_after, overrides);
    let new_effective = serde_json::to_value(&effective_after).map_err(ApplyError::Serialize)?;
    let running = serde_json::to_value(effective_before).map_err(ApplyError::Serialize)?;
    let changed = diff_paths(&running, &new_effective);

    save(path, &to_persist).map_err(ApplyError::Persist)?;

    let status = if changed.is_empty() {
        ApplyStatus::Ok
    } else {
        ApplyStatus::Applying
    };

    Ok(ConfigApplyResponse {
        status,
        applied: Vec::new(),
        reload: changed,
        restart_required: Vec::new(),
        skipped_override: skipped,
        persisted_to: Some(path.display().to_string()),
        errors: Vec::new(),
    })
}

// --- pure helpers (driver-agnostic) --------------------------------------------

/// Redact each secret leaf in a serialized config `Value`, in place.
fn redact_value(value: &mut Value, secret_pointers: &[&str]) {
    for ptr in secret_pointers {
        if let Some(secret) = value.pointer_mut(ptr) {
            if secret.is_string() {
                *secret = Value::String(REDACTED.to_string());
            }
        }
    }
}

/// Whether `submitted` carries the redaction sentinel at `secret_pointer` while
/// `file_current` has no stored string there to restore.
fn redacted_secret_without_prior(
    submitted: &Value,
    file_current: &Value,
    secret_pointer: &str,
) -> bool {
    let submitted_is_sentinel = submitted
        .pointer(secret_pointer)
        .and_then(Value::as_str)
        .is_some_and(|s| s == REDACTED);
    submitted_is_sentinel
        && file_current
            .pointer(secret_pointer)
            .and_then(Value::as_str)
            .is_none()
}

/// Build the value to persist from a validated `submitted` config value:
/// CLI-override-pinned fields are written through from `file_current` (so a
/// transient `--port` is never baked into the file), and a round-tripped redacted
/// secret keeps the file's existing value. Returns the value to write plus the
/// dotted paths that were skipped because they are override-pinned.
fn build_persist_value(
    submitted: &Value,
    file_current: &Value,
    pinned_paths: &[String],
    secret_pointers: &[&str],
) -> (Value, Vec<String>) {
    let mut to_write = submitted.clone();

    for path in pinned_paths {
        let pointer = dotted_to_pointer(path);
        if let (Some(file_val), Some(slot)) = (
            file_current.pointer(&pointer).cloned(),
            to_write.pointer_mut(&pointer),
        ) {
            *slot = file_val;
        }
    }

    for secret_pointer in secret_pointers {
        let is_sentinel = to_write
            .pointer(secret_pointer)
            .and_then(Value::as_str)
            .is_some_and(|s| s == REDACTED);
        if is_sentinel {
            if let (Some(file_secret), Some(slot)) = (
                file_current.pointer(secret_pointer).cloned(),
                to_write.pointer_mut(secret_pointer),
            ) {
                *slot = file_secret;
            }
        }
    }

    (to_write, pinned_paths.to_vec())
}

/// Dotted JSON paths whose leaf values differ between `before` and `after`.
/// Objects recurse; everything else compares by equality.
fn diff_paths(before: &Value, after: &Value) -> Vec<String> {
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

/// Convert a dotted path (`serial.port`) to an RFC-6901 JSON pointer (`/serial/port`).
fn dotted_to_pointer(dotted: &str) -> String {
    let mut pointer = String::with_capacity(dotted.len() + 1);
    pointer.push('/');
    pointer.push_str(&dotted.replace('.', "/"));
    pointer
}

/// Convert an RFC-6901 JSON pointer (`/server/auth/password_hash`) to a dotted
/// path (`server.auth.password_hash`).
fn pointer_to_dotted(pointer: &str) -> String {
    pointer.trim_start_matches('/').replace('/', ".")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- a minimal fake driver covering every protocol feature ---------------

    #[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
    struct Serial {
        port: String,
        baud_rate: u32,
    }

    #[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
    struct Auth {
        username: String,
        password_hash: String,
    }

    #[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
    struct Server {
        port: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        auth: Option<Auth>,
    }

    #[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
    struct Device {
        unique_id: String,
        name: String,
    }

    #[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
    struct TestConfig {
        serial: Serial,
        server: Server,
        device: Device,
    }

    #[derive(Clone, Default)]
    struct TestOverrides {
        serial_port: Option<String>,
    }

    struct TestDriver;

    impl ConfigurableDriver for TestDriver {
        type Config = TestConfig;
        type Overrides = TestOverrides;

        fn normalize(config: &mut Self::Config) {
            let trimmed = config.serial.port.trim();
            if trimmed.len() != config.serial.port.len() {
                config.serial.port = trimmed.to_string();
            }
        }

        fn validate(config: &Self::Config) -> Vec<FieldError> {
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
            errors
        }

        fn secret_pointers() -> &'static [&'static str] {
            &["/server/auth/password_hash"]
        }

        fn override_paths(overrides: &Self::Overrides) -> Vec<String> {
            let mut paths = Vec::new();
            if overrides.serial_port.is_some() {
                paths.push("serial.port".to_string());
            }
            paths
        }

        fn apply_overrides(config: &mut Self::Config, overrides: &Self::Overrides) {
            if let Some(port) = &overrides.serial_port {
                config.serial.port = port.clone();
            }
        }

        fn locked_paths() -> &'static [&'static str] {
            &["device.unique_id"]
        }

        fn read_only_paths() -> &'static [&'static str] {
            &["server.port"]
        }
    }

    fn valid_config() -> TestConfig {
        TestConfig {
            serial: Serial {
                port: "/dev/ttyACM0".to_string(),
                baud_rate: 115_200,
            },
            server: Server {
                port: 11119,
                auth: None,
            },
            device: Device {
                unique_id: "id-1".to_string(),
                name: "Test".to_string(),
            },
        }
    }

    #[test]
    fn config_action_names_round_trip() {
        for action in ConfigAction::ALL {
            assert_eq!(ConfigAction::from_name(action.name()), Some(action));
        }
        assert_eq!(ConfigAction::from_name("config.nope"), None);
        assert_eq!(ConfigAction::Schema.name(), "config.schema");
    }

    #[test]
    fn config_get_redacts_secret_and_lists_overrides() {
        let mut cfg = valid_config();
        cfg.server.auth = Some(Auth {
            username: "obs".to_string(),
            password_hash: "$argon2id$real".to_string(),
        });
        let overrides = TestOverrides {
            serial_port: Some("/dev/ttyACM9".to_string()),
        };
        let resp = config_get::<TestDriver>(&cfg, &overrides).unwrap();
        assert_eq!(
            resp.config
                .pointer("/server/auth/password_hash")
                .and_then(Value::as_str),
            Some(REDACTED)
        );
        // Non-secret fields are untouched.
        assert_eq!(
            resp.config.pointer("/serial/port").and_then(Value::as_str),
            Some("/dev/ttyACM0")
        );
        assert_eq!(resp.overrides, vec!["serial.port".to_string()]);
    }

    #[test]
    fn config_get_without_secret_is_a_noop() {
        let resp = config_get::<TestDriver>(&valid_config(), &TestOverrides::default()).unwrap();
        assert!(resp
            .config
            .pointer("/server/auth")
            .map(Value::is_null)
            .unwrap_or(true));
        assert!(resp.overrides.is_empty());
    }

    #[test]
    fn config_schema_carries_tiers_and_describes_fields() {
        let resp = config_schema::<TestDriver>();
        assert_eq!(resp.locked_fields, vec!["device.unique_id".to_string()]);
        assert_eq!(resp.read_only_fields, vec!["server.port".to_string()]);
        // The schema names the top-level config sections.
        let props = resp
            .schema
            .pointer("/properties")
            .and_then(Value::as_object)
            .expect("schema has properties");
        assert!(props.contains_key("serial"));
        assert!(props.contains_key("server"));
        assert!(props.contains_key("device"));
    }

    #[test]
    fn config_apply_persists_and_reports_changed_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let before = valid_config();
        save(&path, &serde_json::to_value(&before).unwrap()).unwrap();

        let mut changed = valid_config();
        changed.serial.baud_rate = 9600;
        let submitted = serde_json::to_string(&changed).unwrap();

        let resp =
            config_apply::<TestDriver>(&path, &TestOverrides::default(), &before, &submitted)
                .unwrap();

        assert_eq!(resp.status, ApplyStatus::Applying);
        assert_eq!(resp.reload, vec!["serial.baud_rate".to_string()]);
        assert_eq!(resp.persisted_to, Some(path.display().to_string()));

        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            on_disk.pointer("/serial/baud_rate").and_then(Value::as_u64),
            Some(9600)
        );
    }

    #[test]
    fn config_apply_no_change_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let before = valid_config();
        save(&path, &serde_json::to_value(&before).unwrap()).unwrap();

        let submitted = serde_json::to_string(&valid_config()).unwrap();
        let resp =
            config_apply::<TestDriver>(&path, &TestOverrides::default(), &before, &submitted)
                .unwrap();

        assert_eq!(resp.status, ApplyStatus::Ok);
        assert!(resp.reload.is_empty());
    }

    #[test]
    fn config_apply_invalid_leaves_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let before = valid_config();
        let original = serde_json::to_value(&before).unwrap();
        save(&path, &original).unwrap();
        let on_disk_before = std::fs::read_to_string(&path).unwrap();

        let mut bad = valid_config();
        bad.serial.baud_rate = 0;
        let submitted = serde_json::to_string(&bad).unwrap();

        let resp =
            config_apply::<TestDriver>(&path, &TestOverrides::default(), &before, &submitted)
                .unwrap();

        assert_eq!(resp.status, ApplyStatus::Invalid);
        assert_eq!(
            resp.errors,
            vec![FieldError {
                path: "serial.baud_rate".to_string(),
                msg: "must be greater than 0".to_string(),
            }]
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), on_disk_before);
    }

    #[test]
    fn config_apply_rejects_non_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let err =
            config_apply::<TestDriver>(&path, &TestOverrides::default(), &valid_config(), "nope")
                .unwrap_err();
        assert!(matches!(err, ApplyError::Parse(_)), "{err:?}");
    }

    #[test]
    fn config_apply_normalizes_before_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let before = valid_config();
        save(&path, &serde_json::to_value(&before).unwrap()).unwrap();

        let mut padded = valid_config();
        padded.serial.port = "  /dev/ttyACM0  ".to_string();
        let submitted = serde_json::to_string(&padded).unwrap();

        let resp =
            config_apply::<TestDriver>(&path, &TestOverrides::default(), &before, &submitted)
                .unwrap();

        // Trimmed value equals the running one, so nothing changed.
        assert_eq!(resp.status, ApplyStatus::Ok);
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            on_disk.pointer("/serial/port").and_then(Value::as_str),
            Some("/dev/ttyACM0")
        );
    }

    #[test]
    fn config_apply_skips_override_pinned_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        // File has its own serial.port.
        let mut file_cfg = valid_config();
        file_cfg.serial.port = "/dev/ttyACM0".to_string();
        save(&path, &serde_json::to_value(&file_cfg).unwrap()).unwrap();

        let overrides = TestOverrides {
            serial_port: Some("/dev/ttyACM9".to_string()),
        };
        // Effective (running) config reflects the override.
        let mut running = file_cfg.clone();
        TestDriver::apply_overrides(&mut running, &overrides);

        // Submission carries the override value (as config.get would have shown)
        // plus a real edit elsewhere.
        let mut submitted_cfg = running.clone();
        submitted_cfg.device.name = "Renamed".to_string();
        let submitted = serde_json::to_string(&submitted_cfg).unwrap();

        let resp = config_apply::<TestDriver>(&path, &overrides, &running, &submitted).unwrap();

        assert_eq!(resp.skipped_override, vec!["serial.port".to_string()]);
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // The pinned field keeps the FILE's value, not the override value.
        assert_eq!(
            on_disk.pointer("/serial/port").and_then(Value::as_str),
            Some("/dev/ttyACM0")
        );
        // The non-pinned edit is persisted.
        assert_eq!(
            on_disk.pointer("/device/name").and_then(Value::as_str),
            Some("Renamed")
        );
    }

    #[test]
    fn config_apply_keeps_secret_when_sentinel_submitted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let mut file_cfg = valid_config();
        file_cfg.server.auth = Some(Auth {
            username: "obs".to_string(),
            password_hash: "$argon2id$real".to_string(),
        });
        save(&path, &serde_json::to_value(&file_cfg).unwrap()).unwrap();

        // Submission round-trips the redaction sentinel.
        let mut submitted_cfg = file_cfg.clone();
        submitted_cfg.server.auth.as_mut().unwrap().password_hash = REDACTED.to_string();
        submitted_cfg.device.name = "Renamed".to_string();
        let submitted = serde_json::to_string(&submitted_cfg).unwrap();

        config_apply::<TestDriver>(&path, &TestOverrides::default(), &file_cfg, &submitted)
            .unwrap();

        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // The real hash is preserved, not overwritten with the sentinel.
        assert_eq!(
            on_disk
                .pointer("/server/auth/password_hash")
                .and_then(Value::as_str),
            Some("$argon2id$real")
        );
    }

    #[test]
    fn config_apply_rejects_sentinel_without_prior_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        let file_cfg = valid_config(); // no stored secret
        save(&path, &serde_json::to_value(&file_cfg).unwrap()).unwrap();

        let mut submitted_cfg = valid_config();
        submitted_cfg.server.auth = Some(Auth {
            username: "obs".to_string(),
            password_hash: REDACTED.to_string(),
        });
        let submitted = serde_json::to_string(&submitted_cfg).unwrap();

        let resp =
            config_apply::<TestDriver>(&path, &TestOverrides::default(), &file_cfg, &submitted)
                .unwrap();

        assert_eq!(resp.status, ApplyStatus::Invalid);
        assert_eq!(
            resp.errors,
            vec![FieldError {
                path: "server.auth.password_hash".to_string(),
                msg:
                    "cannot keep an unchanged secret when none is stored; provide the password hash"
                        .to_string(),
            }]
        );
    }

    #[test]
    fn config_apply_surfaces_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        std::fs::write(&path, "{ not json").unwrap();

        let submitted = serde_json::to_string(&valid_config()).unwrap();
        let err = config_apply::<TestDriver>(
            &path,
            &TestOverrides::default(),
            &valid_config(),
            &submitted,
        )
        .unwrap_err();
        assert!(matches!(err, ApplyError::ReadFile(_)), "{err:?}");
    }

    #[test]
    fn diff_paths_reports_changed_leaves_only() {
        let before = json!({ "a": { "x": 1, "y": 2 }, "b": 3 });
        let after = json!({ "a": { "x": 1, "y": 9 }, "b": 3 });
        assert_eq!(diff_paths(&before, &after), vec!["a.y".to_string()]);
    }

    #[test]
    fn dotted_pointer_conversions_round_trip() {
        assert_eq!(dotted_to_pointer("serial.port"), "/serial/port");
        assert_eq!(
            pointer_to_dotted("/server/auth/password_hash"),
            "server.auth.password_hash"
        );
    }
}
