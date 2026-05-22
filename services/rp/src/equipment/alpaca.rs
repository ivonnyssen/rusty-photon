//! Generic Alpaca client glue: HTTP basic-auth header construction and
//! the retry/backoff helper that every per-device `connect_*` shares.

use std::future::Future;
use std::time::Duration;

use ascom_alpaca::Client;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rp_auth::config::ClientAuthConfig;
use tracing::info;

/// Maximum attempts each `connect_*` makes for the get-devices +
/// set-connected pair. Backoff between attempts is 1 s, then 2 s
/// (see [`retry_connect_attempt`]), so the worst-case wait per device
/// before giving up is 3 s of sleep plus up to 3 ×
/// [`GET_DEVICES_TIMEOUT`] of in-flight HTTP. Three attempts is
/// enough to ride out a transient OmniSim stall (e.g. a long .NET
/// full GC) without making BDD scenarios that intentionally point at
/// unreachable URLs pay too high a wall-clock cost.
pub(super) const CONNECT_ATTEMPTS: u32 = 3;

/// Per-call timeout on `client.get_devices()`. Short enough that a stuck
/// Alpaca server falls into the retry loop instead of hanging the whole
/// rp startup.
pub(super) const GET_DEVICES_TIMEOUT: Duration = Duration::from_secs(5);

/// Result of a single attempt inside [`retry_connect_attempt`]. The
/// `Permanent` / `Transient` split is what makes this a retry helper
/// rather than a sleep-and-hope wrapper: the connect closures map each
/// failure mode to its own variant. Today only "device-not-found at
/// the requested index in the Alpaca server's reply" is `Permanent`
/// inside the closure — every other failure inside the retry loop
/// (HTTP transport errors including connection-refused on an
/// unreachable host, get_devices timeouts, set_connected errors,
/// OmniSim under GC pressure) is `Transient` and goes through the
/// retry/backoff path. Note: `Client::new` failures are filtered
/// out *before* the retry loop, so that case never produces an
/// `AttemptOutcome` at all.
pub(super) enum AttemptOutcome<T> {
    Ok(T),
    Permanent(String),
    Transient(String),
}

/// Drive `operation` up to [`CONNECT_ATTEMPTS`] times with exponential
/// backoff between attempts: 1 s, then 2 s. Returns the first `Ok`
/// result, or the last transient error wrapped with attempt count if
/// every attempt returned `Transient`. Returns immediately on
/// `Permanent`.
///
/// `label` is used purely for log lines so the operator can see which
/// device is retrying — e.g. `"camera main-cam"`.
pub(super) async fn retry_connect_attempt<T, F, Fut>(label: &str, operation: F) -> Result<T, String>
where
    F: Fn(u32) -> Fut,
    Fut: Future<Output = AttemptOutcome<T>>,
{
    let mut last_transient = String::from("no attempts made");
    for attempt in 1..=CONNECT_ATTEMPTS {
        match operation(attempt).await {
            AttemptOutcome::Ok(value) => {
                if attempt > 1 {
                    // Worth surfacing at info: the user will want to
                    // know the system had to retry but did recover.
                    info!(label, attempt, "connect succeeded after retry");
                }
                return Ok(value);
            }
            AttemptOutcome::Permanent(msg) => return Err(msg),
            AttemptOutcome::Transient(msg) => {
                last_transient = msg;
                if attempt < CONNECT_ATTEMPTS {
                    // 1 s, then 2 s — bounded total backoff of 3 s per
                    // device keeps unreachable-URL BDD scenarios fast
                    // while still smoothing over a transient stall.
                    let delay = Duration::from_secs(1u64 << (attempt - 1));
                    // Each retry is at info level: a transient
                    // connect failure that the system is recovering
                    // from is something the operator should see in
                    // default-verbosity logs without having to enable
                    // debug.
                    info!(
                        label,
                        attempt,
                        max = CONNECT_ATTEMPTS,
                        ?delay,
                        error = %last_transient,
                        "transient connect failure, retrying after backoff"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    Err(format!(
        "gave up after {CONNECT_ATTEMPTS} attempts (last error: {last_transient})"
    ))
}

/// Build an Alpaca client with optional HTTP Basic Auth credentials.
pub(super) fn build_alpaca_client(
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
    fn build_alpaca_client_without_auth() {
        build_alpaca_client("http://localhost:11111", None).unwrap();
    }

    #[test]
    fn build_alpaca_client_with_auth() {
        let auth = ClientAuthConfig {
            username: "observatory".to_string(),
            password: "secret".to_string(),
        };
        build_alpaca_client("http://localhost:11111", Some(&auth)).unwrap();
    }

    #[test]
    fn build_alpaca_client_with_invalid_url_fails() {
        let result = build_alpaca_client("not-a-url", None);
        assert!(result.is_err());
    }

    // ----- retry_connect_attempt direct unit tests --------------------
    //
    // These exercise the retry helper independently of any of the
    // `connect_*` callers, so a regression to the Permanent /
    // Transient split or the backoff schedule shows up here rather
    // than as a slow / flaky integration test. `start_paused = true`
    // makes `tokio::time::sleep` advance virtual time only when
    // awaited, so the 1 s / 2 s backoff doesn't slow the suite.

    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test(start_paused = true)]
    async fn retry_connect_attempt_returns_immediately_on_permanent() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_closure = calls.clone();
        let result: Result<u32, String> = retry_connect_attempt("test", |_attempt| {
            let calls = calls_for_closure.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                AttemptOutcome::Permanent("config bad".to_string())
            }
        })
        .await;
        let err = result.expect_err("Permanent should propagate as Err");
        assert_eq!(err, "config bad");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "Permanent must NOT trigger a retry"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retry_connect_attempt_succeeds_after_transient_failures() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_for_closure = attempts.clone();
        let result: Result<&'static str, String> = retry_connect_attempt("test", |attempt| {
            let attempts = attempts_for_closure.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                if attempt < CONNECT_ATTEMPTS {
                    AttemptOutcome::Transient(format!("attempt {attempt} flake"))
                } else {
                    AttemptOutcome::Ok("ok")
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), CONNECT_ATTEMPTS);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_connect_attempt_gives_up_after_all_transient() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_for_closure = attempts.clone();
        let result: Result<u32, String> = retry_connect_attempt("test", |_| {
            let attempts = attempts_for_closure.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                AttemptOutcome::Transient("always fails".to_string())
            }
        })
        .await;
        let err = result.expect_err("all-Transient must give up with Err");
        assert!(
            err.contains(&format!("gave up after {CONNECT_ATTEMPTS} attempts")),
            "error should report attempt count, got: {err}"
        );
        assert!(
            err.contains("always fails"),
            "error should include last transient message, got: {err}"
        );
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            CONNECT_ATTEMPTS,
            "every attempt should have been tried"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retry_connect_attempt_succeeds_first_try_does_not_sleep() {
        // Belt-and-braces: the helper shouldn't even enter the
        // backoff branch when the first attempt succeeds.
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_for_closure = attempts.clone();
        let result: Result<u32, String> = retry_connect_attempt("test", |_| {
            let attempts = attempts_for_closure.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                AttemptOutcome::Ok(42)
            }
        })
        .await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
