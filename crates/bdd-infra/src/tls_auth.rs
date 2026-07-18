//! Shared TLS + HTTP Basic Auth test fixture and smoke-step macro.
//!
//! Every service adopts the shared `rusty-photon-server-config` shapes, and
//! every BDD suite proves the same contract: with `server.tls` and
//! `server.auth` configured, the service serves HTTPS and gates requests on
//! HTTP Basic Auth. [`PkiFixture`] owns the throwaway PKI (a generated CA plus
//! a service certificate signed by it) and a per-run credential pair;
//! [`tls_auth_smoke_steps!`](crate::tls_auth_smoke_steps) expands the shared
//! smoke step definitions against a `World` implementing
//! [`TlsAuthSmokeWorld`]. Deep suites (ppba-driver, ui-htmx, …) keep their own
//! scenario sets but build on the same fixture and probe helpers.

use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;

/// Username baked into every generated credential pair. The password is the
/// secret under test; a fixed username keeps feature files readable.
const USERNAME: &str = "observatory";

/// Throwaway PKI and credentials for one scenario: a generated CA, a service
/// certificate signed by it, and a freshly generated password with its
/// Argon2id hash.
///
/// The password is random per fixture, so no suite carries a hard-coded
/// credential (which would trip CodeQL's hard-coded-cryptographic-value
/// query and need a used-in-tests dismissal per suite).
#[derive(Debug)]
pub struct PkiFixture {
    dir: TempDir,
    service_name: String,
    password: String,
    password_hash: String,
}

impl PkiFixture {
    /// Generate a CA and a certificate for `service_name` (the TLS server
    /// name — probes connect to `https://localhost`, which
    /// `generate_service_cert` always includes as a SAN).
    pub fn generate(service_name: &str) -> Self {
        let dir = TempDir::new().unwrap();
        rusty_photon_tls::test_cert::generate_ca(dir.path()).unwrap();
        let ca_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
        let ca_key = std::fs::read_to_string(dir.path().join("ca-key.pem")).unwrap();
        let certs_dir = dir.path().join("certs");
        rusty_photon_tls::test_cert::generate_service_cert(
            &ca_pem,
            &ca_key,
            service_name,
            &certs_dir,
        )
        .unwrap();

        let password = uuid::Uuid::new_v4().simple().to_string();
        let password_hash = rp_auth::credentials::hash_password(&password).unwrap();

        Self {
            dir,
            service_name: service_name.to_string(),
            password,
            password_hash,
        }
    }

    pub fn ca_path(&self) -> PathBuf {
        self.dir.path().join("ca.pem")
    }

    pub fn cert_path(&self) -> PathBuf {
        self.dir
            .path()
            .join("certs")
            .join(format!("{}.pem", self.service_name))
    }

    pub fn key_path(&self) -> PathBuf {
        self.dir
            .path()
            .join("certs")
            .join(format!("{}-key.pem", self.service_name))
    }

    pub fn username(&self) -> &str {
        USERNAME
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    pub fn password_hash(&self) -> &str {
        &self.password_hash
    }

    /// An HTTPS client trusting this fixture's CA.
    pub fn https_client(&self) -> reqwest::Client {
        rusty_photon_tls::client::build_reqwest_client(Some(&self.ca_path())).unwrap()
    }

    /// The `server.tls` JSON fragment pointing at the generated cert pair.
    pub fn tls_block(&self) -> serde_json::Value {
        serde_json::json!({
            "cert": self.cert_path().to_string_lossy(),
            "key": self.key_path().to_string_lossy(),
        })
    }

    /// The `server.auth` JSON fragment with the Argon2id hash filled in.
    pub fn auth_block(&self) -> serde_json::Value {
        serde_json::json!({
            "username": USERNAME,
            "password_hash": self.password_hash,
        })
    }

    /// A complete `server` block: `port` plus this fixture's `tls` and
    /// `auth` fragments. Port 0 gives an OS-assigned port.
    pub fn server_block(&self, port: u16) -> serde_json::Value {
        serde_json::json!({
            "port": port,
            "tls": self.tls_block(),
            "auth": self.auth_block(),
        })
    }
}

/// Per-scenario smoke-test state; embed one (via `#[derive(Default)]`) in the
/// suite's `World`.
#[derive(Debug, Default)]
pub struct TlsAuthState {
    pub pki: Option<PkiFixture>,
    /// Config JSON staged by the configure Given, consumed by the start When.
    pub staged_config: Option<serde_json::Value>,
    /// Owns the written config file for the default spawn path.
    pub config_dir: Option<TempDir>,
    /// The bound port, whatever the launch mechanism. Set by
    /// [`spawn_service_handle`]; custom launch paths record it themselves.
    pub port: Option<u16>,
}

impl TlsAuthState {
    pub fn pki(&self) -> &PkiFixture {
        self.pki.as_ref().expect("TLS certs not generated")
    }

    pub fn port(&self) -> u16 {
        self.port.expect("service not started with TLS and auth")
    }
}

/// Contract the [`tls_auth_smoke_steps!`](crate::tls_auth_smoke_steps) macro
/// programs against. Implementors supply only the genuinely service-specific
/// parts: the base config JSON, the launch mechanism, and (for non-Alpaca
/// services) the probe path.
// The macro-generated steps call these methods on the concrete World type,
// so the returned futures' auto traits leak from each impl — no Send bound
// needed on the trait itself.
#[allow(async_fn_in_trait)]
pub trait TlsAuthSmokeWorld {
    /// Path probed over HTTPS to prove the credential gate. Alpaca services
    /// use the default management endpoint; ad-hoc services override with
    /// their health route.
    const PROBE_PATH: &'static str = "/management/v1/configureddevices";

    /// Mutable access to the embedded [`TlsAuthState`].
    fn tls_auth(&mut self) -> &mut TlsAuthState;

    /// The service-specific config JSON *without* the `server` block — the
    /// configure step fills that in from the fixture.
    fn base_test_config(&self) -> serde_json::Value;

    /// Launch the service with `config` and record the bound port in the
    /// state. Most services delegate to [`spawn_service_handle`]; services
    /// with a custom launch (CLI subcommand, in-process server) override
    /// accordingly.
    async fn start_with_tls_auth(&mut self, config: serde_json::Value);
}

/// Write `config` to a temp file owned by `state` and return its path.
pub fn stage_config_file(state: &mut TlsAuthState, config: &serde_json::Value) -> PathBuf {
    let dir = state
        .config_dir
        .get_or_insert_with(|| TempDir::new().unwrap());
    let path = dir.path().join("tls-auth-config.json");
    std::fs::write(&path, config.to_string()).unwrap();
    path
}

/// Default launch path: write the config file, spawn the service binary via
/// [`ServiceHandle`](crate::ServiceHandle), and record the bound port in
/// `state`. Returns the handle so the World can keep it where its cucumber
/// `after` hook already stops it.
pub async fn spawn_service_handle(
    state: &mut TlsAuthState,
    package_name: &str,
    config: &serde_json::Value,
) -> crate::ServiceHandle {
    let path = stage_config_file(state, config);
    let handle = crate::ServiceHandle::start(package_name, path.to_str().unwrap()).await;
    state.port = Some(handle.port);
    handle
}

/// Poll `url` with valid credentials until the freshly spawned server
/// answers 200, panicking after ~30 s. Readiness must be probed *with*
/// credentials — before asserting a 401, the suite has to know the 401 comes
/// from the auth layer of a live server, not from the socket not being up.
pub async fn wait_until_ready(client: &reqwest::Client, url: &str, username: &str, password: &str) {
    for _ in 0..60 {
        if let Ok(resp) = client
            .get(url)
            .basic_auth(username, Some(password))
            .send()
            .await
        {
            if resp.status().as_u16() == 200 {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("server did not become ready over HTTPS with valid credentials: {url}");
}

/// Expands the shared TLS + auth smoke step definitions against `$world`,
/// which must implement [`TlsAuthSmokeWorld`]. Invoke once in the suite's
/// `auth_steps.rs`:
///
/// ```rust,ignore
/// use crate::world::MyWorld;
///
/// bdd_infra::tls_auth_smoke_steps!(MyWorld);
/// ```
///
/// `$world` is an `ident` (a bare type name imported at the invocation
/// site), not a `ty`: cucumber's attribute macro pattern-matches the step
/// function's `&mut World` parameter syntactically and cannot see through
/// the invisible group a `ty` metavariable is wrapped in.
///
/// The generated steps match this scenario (service-neutral wording, so the
/// feature file is identical across services):
///
/// ```gherkin
/// Given generated TLS certificates for the service
/// And the service is configured with TLS and auth enabled
/// When the service is started with TLS and auth
/// Then the service rejects requests without credentials with 401
/// And the service responds 200 to requests with valid credentials
/// ```
///
/// `env!("CARGO_PKG_NAME")` expands at the invocation site, so the generated
/// certificate carries the invoking service's name.
#[macro_export]
macro_rules! tls_auth_smoke_steps {
    ($world:ident) => {
        #[::cucumber::given("generated TLS certificates for the service")]
        fn tls_auth_smoke_generate_certs(world: &mut $world) {
            use $crate::tls_auth::TlsAuthSmokeWorld as _;
            world.tls_auth().pki = Some($crate::tls_auth::PkiFixture::generate(env!(
                "CARGO_PKG_NAME"
            )));
        }

        #[::cucumber::given("the service is configured with TLS and auth enabled")]
        fn tls_auth_smoke_configure(world: &mut $world) {
            use $crate::tls_auth::TlsAuthSmokeWorld as _;
            let mut config = world.base_test_config();
            let state = world.tls_auth();
            config["server"] = state.pki().server_block(0);
            state.staged_config = Some(config);
        }

        #[::cucumber::when("the service is started with TLS and auth")]
        async fn tls_auth_smoke_start(world: &mut $world) {
            use $crate::tls_auth::TlsAuthSmokeWorld as _;
            let config = world
                .tls_auth()
                .staged_config
                .take()
                .expect("config not staged");
            world.start_with_tls_auth(config).await;
        }

        #[::cucumber::then("the service rejects requests without credentials with 401")]
        async fn tls_auth_smoke_rejects_unauthenticated(world: &mut $world) {
            use $crate::tls_auth::TlsAuthSmokeWorld as _;
            let path = <$world as $crate::tls_auth::TlsAuthSmokeWorld>::PROBE_PATH;
            let state = world.tls_auth();
            let url = format!("https://localhost:{}{path}", state.port());
            let pki = state.pki();
            let client = pki.https_client();
            $crate::tls_auth::wait_until_ready(&client, &url, pki.username(), pki.password()).await;

            let resp = client.get(&url).send().await.unwrap();
            assert_eq!(resp.status().as_u16(), 401);
        }

        #[::cucumber::then("the service responds 200 to requests with valid credentials")]
        async fn tls_auth_smoke_accepts_valid_credentials(world: &mut $world) {
            use $crate::tls_auth::TlsAuthSmokeWorld as _;
            let path = <$world as $crate::tls_auth::TlsAuthSmokeWorld>::PROBE_PATH;
            let state = world.tls_auth();
            let url = format!("https://localhost:{}{path}", state.port());
            let pki = state.pki();
            let client = pki.https_client();
            $crate::tls_auth::wait_until_ready(&client, &url, pki.username(), pki.password()).await;

            let resp = client
                .get(&url)
                .basic_auth(pki.username(), Some(pki.password()))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), 200);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pki_fixture_generates_ca_and_service_cert() {
        let pki = PkiFixture::generate("test-service");
        assert!(pki.ca_path().is_file(), "missing {:?}", pki.ca_path());
        assert!(pki.cert_path().is_file(), "missing {:?}", pki.cert_path());
        assert!(pki.key_path().is_file(), "missing {:?}", pki.key_path());
        assert!(pki
            .cert_path()
            .to_string_lossy()
            .ends_with("test-service.pem"));
    }

    #[test]
    fn test_pki_fixture_password_is_generated_and_hash_verifies() {
        let pki = PkiFixture::generate("test-service");
        assert!(!pki.password().is_empty());
        assert!(rp_auth::credentials::verify_password(
            pki.password(),
            pki.password_hash()
        ));
        let other = PkiFixture::generate("test-service");
        assert_ne!(pki.password(), other.password());
    }

    #[test]
    fn test_server_block_shape() {
        let pki = PkiFixture::generate("test-service");
        let block = pki.server_block(0);
        assert_eq!(block["port"], 0);
        assert_eq!(
            block["tls"]["cert"].as_str().unwrap(),
            pki.cert_path().to_string_lossy()
        );
        assert_eq!(
            block["tls"]["key"].as_str().unwrap(),
            pki.key_path().to_string_lossy()
        );
        assert_eq!(block["auth"]["username"], "observatory");
        assert_eq!(
            block["auth"]["password_hash"].as_str().unwrap(),
            pki.password_hash()
        );
    }

    #[test]
    fn test_stage_config_file_writes_json() {
        let mut state = TlsAuthState::default();
        let config = serde_json::json!({ "server": { "port": 0 } });
        let path = stage_config_file(&mut state, &config);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written, config);
    }
}
