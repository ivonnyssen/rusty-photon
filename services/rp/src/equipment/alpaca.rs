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

/// Connection-establishment timeout applied to every Alpaca request.
/// Localhost and LAN devices connect near-instantly; a connect that
/// takes longer means the host is unreachable, which the per-device
/// retry/backoff ([`CONNECT_ATTEMPTS`]) already handles at startup.
pub(super) const ALPACA_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-read timeout applied to every Alpaca request — the maximum gap
/// reqwest will tolerate between received response bytes before failing
/// the request. **This is the load-bearing robustness knob.** reqwest
/// applies no timeout by default, and `ascom_alpaca::Client::new` does
/// not add one either, so a device that accepts the TCP connection but
/// then stalls the response (a wedged keep-alive connection, a peer mid
/// restart, a starved sky-survey-camera under CI load) would block the
/// awaiting call *forever*. The blocking MCP helpers in
/// [`crate::mcp::internals`] (capture-readout and slew-until-idle polls)
/// only re-check their own deadlines *between* `await`s, so a single
/// stalled request defeats them entirely — which is exactly what hung
/// the `center_on_target` BDD to the MCP client's 360 s backstop.
///
/// `read_timeout` (resets as bytes arrive) rather than a whole-request
/// `timeout` so a legitimately large image download from a real camera
/// keeps working as long as bytes keep flowing. Comfortably larger than
/// any healthy Alpaca call (property reads are sub-100 ms; `StartExposure`
/// and async slews return immediately) yet well under the capture
/// (`duration + 120 s`) and slew (300 s) poll deadlines, so those still
/// govern the overall operation.
pub(super) const ALPACA_READ_TIMEOUT: Duration = Duration::from_secs(15);

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

/// Build the reqwest client that backs every Alpaca request, with the
/// given connection/read timeouts and optional HTTP Basic Auth header.
///
/// Split out from [`build_alpaca_client`] so the timeout behaviour can be
/// exercised with short durations in tests. We build our own client even
/// in the no-auth case (rather than falling back to
/// `ascom_alpaca::Client::new`'s built-in client) precisely so the
/// timeouts below apply on every code path — see [`ALPACA_READ_TIMEOUT`].
fn alpaca_reqwest_client(
    auth: Option<&ClientAuthConfig>,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<reqwest::Client, Box<dyn std::error::Error + Send + Sync>> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout);
    if let Some(a) = auth {
        let encoded = BASE64.encode(format!("{}:{}", a.username, a.password));
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("authorization", format!("Basic {encoded}").parse()?);
        builder = builder.default_headers(headers);
    }
    Ok(builder.build()?)
}

/// Build an Alpaca client with optional HTTP Basic Auth credentials.
///
/// Every request the client makes is bounded by [`ALPACA_CONNECT_TIMEOUT`]
/// and [`ALPACA_READ_TIMEOUT`] so a stalled device can never hang an
/// awaiting MCP tool indefinitely.
pub(super) fn build_alpaca_client(
    url: &str,
    auth: Option<&ClientAuthConfig>,
) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    let http = alpaca_reqwest_client(auth, ALPACA_CONNECT_TIMEOUT, ALPACA_READ_TIMEOUT)?;
    Ok(Client::new_with_client(url, http)?)
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

    /// The load-bearing regression guard: a device that accepts the TCP
    /// connection but never writes a response must surface a bounded
    /// timeout error, not hang the caller forever. Without
    /// [`ALPACA_READ_TIMEOUT`] on the client the `send().await` blocks
    /// indefinitely and the outer guard fires instead — which is the
    /// exact failure that ran `center_on_target`'s capture poll to the
    /// MCP client's 360 s backstop. Uses a short read timeout so the
    /// test is fast; the production constant is larger.
    #[tokio::test]
    async fn alpaca_request_read_timeout_bounds_a_stalled_server() {
        // Accept connections and hold them open without ever replying.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _acceptor = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock); // keep alive, send nothing
            }
        });

        let http = alpaca_reqwest_client(None, Duration::from_secs(5), Duration::from_millis(300))
            .unwrap();

        let started = std::time::Instant::now();
        // Outer guard: a regression that drops `read_timeout` makes
        // `send()` hang, so this `timeout` elapses and the `expect` fails
        // loudly instead of stalling the whole suite.
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            http.get(format!("http://{addr}/")).send(),
        )
        .await
        .expect("Alpaca request hung past 5 s — read_timeout is not applied to the client");

        let err = outcome.expect_err("a stalled server must produce an error, not a response");
        assert!(
            err.is_timeout(),
            "expected a read-timeout error, got: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "request should fail near the 300 ms read_timeout, took {:?}",
            started.elapsed()
        );
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
