pub use rusty_photon_server_config::ServerConfig;

/// rp's default `server` block when the config file omits it: port 11115 on
/// all interfaces, plain HTTP.
pub(crate) fn default_server() -> ServerConfig {
    ServerConfig::new(11115)
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
}
