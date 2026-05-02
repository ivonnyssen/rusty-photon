//! Configuration types and startup validation.
//!
//! See `docs/services/rp-plate-solver.md` §"Configuration" for the field
//! semantics. Validation rules are exercised by Phase 3 BDD's
//! `configuration.feature`.

use serde::Deserialize;
use std::{net::IpAddr, path::Path, path::PathBuf, time::Duration};
use thiserror::Error;

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_bind_address")]
    pub bind_address: IpAddr,

    #[serde(default = "default_port")]
    pub port: u16,

    pub astap_binary_path: PathBuf,

    pub astap_db_directory: PathBuf,

    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,

    #[serde(default = "default_solve_timeout", with = "humantime_serde")]
    pub default_solve_timeout: Duration,

    #[serde(default = "default_max_solve_timeout", with = "humantime_serde")]
    pub max_solve_timeout: Duration,
}

fn default_bind_address() -> IpAddr {
    "127.0.0.1".parse().expect("static literal")
}
fn default_port() -> u16 {
    11131
}
fn default_max_concurrency() -> usize {
    1
}
fn default_solve_timeout() -> Duration {
    Duration::from_secs(30)
}
fn default_max_solve_timeout() -> Duration {
    Duration::from_secs(120)
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("config parse error: {0}")]
    Parse(#[from] serde_json::Error),

    #[error(
        "invalid `astap_binary_path`: {message} (path: {path}). \
         See services/rp-plate-solver/README.md for install instructions."
    )]
    InvalidBinaryPath { path: String, message: String },

    #[error(
        "invalid `astap_db_directory`: {message} (path: {path}). \
         See services/rp-plate-solver/README.md for install instructions."
    )]
    InvalidDbDirectory { path: String, message: String },

    #[error("`default_solve_timeout` ({default:?}) exceeds `max_solve_timeout` ({max:?})")]
    TimeoutOrder { default: Duration, max: Duration },

    #[error("`max_concurrency` must be ≥ 1 (got 0)")]
    ZeroMaxConcurrency,
}

/// Read a JSON config file from disk and parse it.
pub fn load_config(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let bytes = std::fs::read(path)?;
    let config: Config = serde_json::from_slice(&bytes)?;
    Ok(config)
}

impl Config {
    /// Validate the parsed config. Caller exits non-zero on failure so
    /// Sentinel surfaces the misconfiguration rather than masking it.
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_binary_path(&self.astap_binary_path)?;
        validate_db_directory(&self.astap_db_directory)?;
        if self.default_solve_timeout > self.max_solve_timeout {
            return Err(ConfigError::TimeoutOrder {
                default: self.default_solve_timeout,
                max: self.max_solve_timeout,
            });
        }
        if self.max_concurrency == 0 {
            return Err(ConfigError::ZeroMaxConcurrency);
        }
        Ok(())
    }
}

fn validate_binary_path(path: &Path) -> Result<(), ConfigError> {
    let meta = std::fs::metadata(path).map_err(|e| ConfigError::InvalidBinaryPath {
        path: path.display().to_string(),
        message: format!("stat failed: {e}"),
    })?;
    if !meta.is_file() {
        return Err(ConfigError::InvalidBinaryPath {
            path: path.display().to_string(),
            message: "not a regular file".into(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        // Any execute bit set (user, group, or other).
        if mode & 0o111 == 0 {
            return Err(ConfigError::InvalidBinaryPath {
                path: path.display().to_string(),
                message: "not executable (no execute bit set)".into(),
            });
        }
    }
    Ok(())
}

fn validate_db_directory(path: &Path) -> Result<(), ConfigError> {
    let meta = std::fs::metadata(path).map_err(|e| ConfigError::InvalidDbDirectory {
        path: path.display().to_string(),
        message: format!("stat failed: {e}"),
    })?;
    if !meta.is_dir() {
        return Err(ConfigError::InvalidDbDirectory {
            path: path.display().to_string(),
            message: "not a directory".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_config(dir: &TempDir, body: &str) -> PathBuf {
        let p = dir.path().join("config.json");
        fs::write(&p, body).unwrap();
        p
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        {
            // No-op on Windows; the existence check is sufficient there.
            let _ = path;
        }
    }

    fn fake_binary(dir: &TempDir, name: &str) -> PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, b"#!/bin/sh\nexit 0\n").unwrap();
        make_executable(&p);
        p
    }

    fn fake_db_dir(dir: &TempDir) -> PathBuf {
        let p = dir.path().join("d05");
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn happy_path_loads_and_validates() {
        let dir = TempDir::new().unwrap();
        let bin = fake_binary(&dir, "astap_cli");
        let db = fake_db_dir(&dir);
        // Use serde_json to construct the body so Windows paths
        // (which contain backslashes) are escaped properly. A raw
        // format!() would put a literal `\b` into the JSON string,
        // which the parser rejects as an invalid escape.
        let body = serde_json::json!({
            "astap_binary_path": bin.to_string_lossy(),
            "astap_db_directory": db.to_string_lossy(),
        })
        .to_string();
        let cfg_path = write_config(&dir, &body);
        let cfg = load_config(&cfg_path).unwrap();
        cfg.validate().unwrap();

        assert_eq!(cfg.bind_address, "127.0.0.1".parse::<IpAddr>().unwrap());
        assert_eq!(cfg.port, 11131);
        assert_eq!(cfg.max_concurrency, 1);
        assert_eq!(cfg.default_solve_timeout, Duration::from_secs(30));
        assert_eq!(cfg.max_solve_timeout, Duration::from_secs(120));
    }

    #[test]
    fn missing_binary_path_field_fails_to_parse() {
        let dir = TempDir::new().unwrap();
        let db = fake_db_dir(&dir);
        let body = serde_json::json!({
            "astap_db_directory": db.to_string_lossy(),
        })
        .to_string();
        let cfg_path = write_config(&dir, &body);
        let err = load_config(&cfg_path).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn nonexistent_binary_path_fails_validation() {
        let dir = TempDir::new().unwrap();
        let db = fake_db_dir(&dir);
        let cfg = Config {
            bind_address: default_bind_address(),
            port: default_port(),
            astap_binary_path: "/absolutely/does/not/exist/astap_cli".into(),
            astap_db_directory: db,
            max_concurrency: default_max_concurrency(),
            default_solve_timeout: default_solve_timeout(),
            max_solve_timeout: default_max_solve_timeout(),
        };
        let err = cfg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, ConfigError::InvalidBinaryPath { .. }));
        assert!(msg.contains("astap_binary_path"));
        assert!(msg.contains("README"));
    }

    #[test]
    fn nonexistent_db_directory_fails_validation() {
        let dir = TempDir::new().unwrap();
        let bin = fake_binary(&dir, "astap_cli");
        let cfg = Config {
            bind_address: default_bind_address(),
            port: default_port(),
            astap_binary_path: bin,
            astap_db_directory: "/absolutely/does/not/exist/d05".into(),
            max_concurrency: default_max_concurrency(),
            default_solve_timeout: default_solve_timeout(),
            max_solve_timeout: default_max_solve_timeout(),
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidDbDirectory { .. }));
        assert!(err.to_string().contains("astap_db_directory"));
    }

    #[test]
    fn timeout_order_inverted_fails() {
        let dir = TempDir::new().unwrap();
        let bin = fake_binary(&dir, "astap_cli");
        let db = fake_db_dir(&dir);
        let cfg = Config {
            bind_address: default_bind_address(),
            port: default_port(),
            astap_binary_path: bin,
            astap_db_directory: db,
            max_concurrency: 1,
            default_solve_timeout: Duration::from_secs(60),
            max_solve_timeout: Duration::from_secs(30),
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::TimeoutOrder { .. }));
    }

    #[test]
    fn zero_max_concurrency_fails() {
        let dir = TempDir::new().unwrap();
        let bin = fake_binary(&dir, "astap_cli");
        let db = fake_db_dir(&dir);
        let cfg = Config {
            bind_address: default_bind_address(),
            port: default_port(),
            astap_binary_path: bin,
            astap_db_directory: db,
            max_concurrency: 0,
            default_solve_timeout: default_solve_timeout(),
            max_solve_timeout: default_max_solve_timeout(),
        };
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ConfigError::ZeroMaxConcurrency
        ));
    }

    #[test]
    #[cfg(unix)]
    fn non_executable_binary_fails_validation() {
        let dir = TempDir::new().unwrap();
        let bin = dir.path().join("astap_cli");
        fs::write(&bin, b"some bytes").unwrap();
        // Deliberately do NOT chmod +x.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o644)).unwrap();
        let db = fake_db_dir(&dir);
        let cfg = Config {
            bind_address: default_bind_address(),
            port: default_port(),
            astap_binary_path: bin,
            astap_db_directory: db,
            max_concurrency: 1,
            default_solve_timeout: default_solve_timeout(),
            max_solve_timeout: default_max_solve_timeout(),
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidBinaryPath { .. }));
        assert!(err.to_string().contains("not executable"));
    }
}
