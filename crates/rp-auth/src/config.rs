use serde::{Deserialize, Serialize};

/// Server-side authentication configuration.
///
/// Stored in each service's config file. The password is hashed with Argon2id;
/// use `rp hash-password` to generate the hash.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, derive_more::Debug)]
pub struct AuthConfig {
    pub username: String,
    /// Argon2id hash — redacted from `Debug` so credential material never lands
    /// in logs or test output.
    #[debug("<redacted>")]
    pub password_hash: String,
}

/// Client-side authentication configuration.
///
/// Used by services that connect to auth-enabled services (e.g. sentinel
/// monitoring an auth-enabled filemonitor). The password is stored in plaintext
/// because the client needs the actual password for HTTP Basic Auth headers.
/// File permissions (`chmod 600`) are the recommended protection.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, derive_more::Debug)]
pub struct ClientAuthConfig {
    pub username: String,
    /// Plaintext (needed for the HTTP Basic-Auth header) — redacted from `Debug`
    /// so it never lands in logs or test output.
    #[debug("<redacted>")]
    pub password: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn auth_config_deserializes_from_json() {
        let json =
            r#"{"username": "observatory", "password_hash": "$argon2id$v=19$m=19456,t=2,p=1$abc"}"#;
        let config: AuthConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.username, "observatory");
        assert_eq!(config.password_hash, "$argon2id$v=19$m=19456,t=2,p=1$abc");
    }

    #[test]
    fn auth_config_round_trips() {
        let config = AuthConfig {
            username: "user".to_string(),
            password_hash: "hash".to_string(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AuthConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.username, "user");
        assert_eq!(deserialized.password_hash, "hash");
    }

    #[test]
    fn client_auth_config_deserializes_from_json() {
        let json = r#"{"username": "observatory", "password": "secret"}"#;
        let config: ClientAuthConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.username, "observatory");
        assert_eq!(config.password, "secret");
    }

    #[test]
    fn auth_config_redacts_password_hash_in_debug() {
        let config = AuthConfig {
            username: "observatory".to_string(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$secret-hash".to_string(),
        };
        let rendered = format!("{config:?}");
        assert!(
            !rendered.contains("secret-hash") && !rendered.contains("argon2id"),
            "password hash leaked into Debug: {rendered}"
        );
        assert!(rendered.contains("observatory"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn client_auth_config_redacts_password_in_debug() {
        let config = ClientAuthConfig {
            username: "observatory".to_string(),
            password: "hunter2".to_string(),
        };
        let rendered = format!("{config:?}");
        assert!(
            !rendered.contains("hunter2"),
            "plaintext password leaked into Debug: {rendered}"
        );
        assert!(rendered.contains("observatory"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn optional_auth_config_defaults_to_none() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            auth: Option<AuthConfig>,
        }
        let json = r#"{}"#;
        let wrapper: Wrapper = serde_json::from_str(json).unwrap();
        assert!(wrapper.auth.is_none());
    }
}
