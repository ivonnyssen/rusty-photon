//! Shared ASCOM Alpaca client construction for the follow-mode device
//! readers ([`crate::mount::AlpacaMountReader`],
//! [`crate::rotator::AlpacaRotatorReader`]).
//!
//! Both readers build an `ascom_alpaca::Client` the same way — plain
//! for anonymous servers, or with a `Basic` auth header derived from
//! `rp_auth::config::ClientAuthConfig`. Keeping the single helper here
//! means the auth handling can't drift between the two device classes.

use ascom_alpaca::Client;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rp_auth::config::ClientAuthConfig;

/// Build an Alpaca [`Client`] for `url`, attaching a `Basic`
/// `Authorization` header when `auth` is present. This is offline —
/// it constructs the HTTP client only; device discovery happens lazily
/// on the first read (per F3, construction never blocks on the device).
pub(crate) fn build_alpaca_client(
    url: &str,
    auth: Option<&ClientAuthConfig>,
) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    match auth {
        Some(a) => {
            let encoded = BASE64.encode(format!("{}:{}", a.username, a.password));
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("authorization", format!("Basic {encoded}").parse()?);
            let http = reqwest::Client::builder()
                .default_headers(headers)
                .build()?;
            Ok(Client::new_with_client(url, http)?)
        }
        None => Ok(Client::new(url)?),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn build_alpaca_client_with_auth() {
        let auth = ClientAuthConfig {
            username: "u".into(),
            password: "p".into(),
        };
        build_alpaca_client("http://127.0.0.1/", Some(&auth)).unwrap();
    }

    #[test]
    fn build_alpaca_client_without_auth() {
        build_alpaca_client("http://127.0.0.1/", None).unwrap();
    }

    #[test]
    fn build_alpaca_client_rejects_invalid_url() {
        build_alpaca_client("not a url", None).unwrap_err();
    }
}
