use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    pub data_directory: String,
    /// Where the session state file lives (rp.md § Session
    /// Persistence): the session registry + planner progress counters,
    /// written on every transition and read back for startup recovery.
    /// Empty (the default) resolves to
    /// `<data_directory>/session_state.json`.
    #[serde(default)]
    pub session_state_file: String,
    /// Optional template for capture filenames. `None` is the default and
    /// produces filenames of the form `<doc_uuid_8>.fits` plus a matching
    /// `.json` sidecar — fully self-identifying via the UUID-8 suffix that
    /// drives the disk-fallback resolution path. When set, the template is
    /// reserved for a future token resolver (planner/capture context feeding
    /// `{target}` / `{filter}` / etc.); until that lands `capture` ignores
    /// the value and writes `<doc_uuid_8>.fits` regardless. See
    /// `docs/services/rp.md` (Persistence) and Phase 7 of
    /// `docs/plans/archive/image-evaluation-tools.md`.
    #[serde(default)]
    pub file_naming_pattern: Option<String>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use crate::config::load_config;
    use crate::config::test_support::MINIMAL_CONFIG_JSON;

    #[test]
    fn file_naming_pattern_defaults_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, MINIMAL_CONFIG_JSON).unwrap();

        let config = load_config(&path).unwrap();
        assert!(
            config.session.file_naming_pattern.is_none(),
            "omitted file_naming_pattern must deserialize to None"
        );
    }

    #[test]
    fn file_naming_pattern_round_trips_when_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
                "session": {
                    "data_directory": "/tmp/rp-test",
                    "file_naming_pattern": "{target}_{filter}"
                },
                "equipment": {},
                "server": {}
            }"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(
            config.session.file_naming_pattern.as_deref(),
            Some("{target}_{filter}")
        );
    }
}
