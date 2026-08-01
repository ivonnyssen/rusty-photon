//! Shared HTTP Basic Auth recognition for the in-process plugin stubs.

/// The `Authorization` value an HTTP Basic client sends for this pair.
/// Built with reqwest's own encoder rather than a base64 dependency of
/// bdd-infra's own — the exact wire bytes are rp's contract to keep, and
/// its unit tests pin them; these stubs only have to recognize them.
pub(super) fn basic_auth_header(username: &str, password: &str) -> String {
    let request = reqwest::Client::new()
        .get("http://127.0.0.1/")
        .basic_auth(username, Some(password))
        .build()
        .expect("building a Basic-Auth request must not fail");
    request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .expect("reqwest must set an Authorization header for basic_auth")
        .to_string()
}
