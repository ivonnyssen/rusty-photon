use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use rp_auth::config::AuthConfig;
use rusty_photon_tls::config::TlsConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// rp's `server` block: the shared non-Alpaca shape
/// (`rusty_photon_server_config::ServerConfig`) plus `advertised_url`.
/// The shared fields are repeated rather than flattened because serde's
/// `deny_unknown_fields` does not compose with `flatten` — the same
/// trade the shared crate makes for its Alpaca shape. rp is the only
/// service that advertises its own URL to another process (the
/// orchestrator invocation's `mcp_server_url`), so the extra field
/// lives here rather than in every service's config.
///
/// `deny_unknown_fields` so typoed or removed keys fail loudly at load
/// instead of being silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub port: u16,
    /// Interface to bind. `0.0.0.0` (the default) listens on all interfaces;
    /// an `IpAddr` rather than a string so a malformed address fails at
    /// config load, not at bind time.
    #[serde(default = "default_bind_address")]
    pub bind_address: IpAddr,
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    #[serde(default)]
    pub auth: Option<AuthConfig>,
    /// Overrides the MCP endpoint URL advertised to orchestrators —
    /// rp appends `/mcp` (rp.md § MCP Server). Unset, the URL is
    /// derived from the listener; a wildcard bind advertises the
    /// system hostname.
    #[serde(default)]
    pub advertised_url: Option<AdvertisedUrl>,
}

impl ServerConfig {
    /// The address the listener binds to.
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind_address, self.port)
    }
}

fn default_bind_address() -> IpAddr {
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}

/// An `http://` / `https://` base URL, validated at config load and
/// held with any trailing `/` trimmed so appending `/mcp` cannot
/// produce `//mcp`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct AdvertisedUrl(String);

impl AdvertisedUrl {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AdvertisedUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let scheme_len = if value.starts_with("http://") {
            "http://".len()
        } else if value.starts_with("https://") {
            "https://".len()
        } else {
            return Err(serde::de::Error::custom(format!(
                "server.advertised_url must be an http:// or https:// URL, got {value:?}"
            )));
        };
        let trimmed = value.trim_end_matches('/');
        if trimmed.len() <= scheme_len {
            return Err(serde::de::Error::custom(format!(
                "server.advertised_url has no host: {value:?}"
            )));
        }
        Ok(Self(trimmed.to_string()))
    }
}

/// rp's default `server` block when the config file omits it: port 11115 on
/// all interfaces, plain HTTP.
pub(crate) fn default_server() -> ServerConfig {
    ServerConfig {
        port: 11115,
        bind_address: default_bind_address(),
        tls: None,
        auth: None,
        advertised_url: None,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use crate::config::load_config;

    #[test]
    fn server_config_rejects_unknown_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {"data_directory": "/tmp/rp-test"},
                "equipment": {},
                "server": {"port": 11115, "discovery_port": 32227}
            }"#,
        )
        .unwrap();

        let err = load_config(&path).unwrap_err().to_string();
        assert!(err.contains("discovery_port"), "{err}");
    }

    fn write_config(dir: &tempfile::TempDir, server: &str) -> std::path::PathBuf {
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            format!(
                r#"{{
                    "session": {{"data_directory": "/tmp/rp-test"}},
                    "equipment": {{}},
                    "server": {server}
                }}"#
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn advertised_url_loads_with_trailing_slash_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            r#"{"port": 11115, "advertised_url": "https://observatory.example:11115/"}"#,
        );

        let config = load_config(&path).unwrap();
        assert_eq!(
            config.server.advertised_url.unwrap().as_str(),
            "https://observatory.example:11115"
        );
    }

    #[test]
    fn advertised_url_rejects_non_http_scheme() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            r#"{"port": 11115, "advertised_url": "ftp://observatory.example"}"#,
        );

        let err = load_config(&path).unwrap_err().to_string();
        assert!(err.contains("server.advertised_url"), "{err}");
        assert!(err.contains("http://"), "{err}");
    }

    #[test]
    fn advertised_url_rejects_missing_host() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, r#"{"port": 11115, "advertised_url": "https://"}"#);

        let err = load_config(&path).unwrap_err().to_string();
        assert!(err.contains("no host"), "{err}");
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod doctor_toml_parity {
    use rusty_photon_server_config::doctor_toml::{parse, ServerClass};

    use super::default_server;

    /// `pkg/doctor.toml` is this service's catalog entry for
    /// `rusty-photon-doctor` and must match the config defaults
    /// (docs/services/doctor.md §The derived catalog).
    #[test]
    fn pkg_doctor_toml_matches_config_defaults() {
        let meta = parse(include_str!("../../pkg/doctor.toml")).unwrap();
        assert_eq!(meta.port, default_server().port);
        assert_eq!(meta.class, ServerClass::Core);
    }
}
