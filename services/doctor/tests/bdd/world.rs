//! World for the doctor BDD suite: a scratch config directory, the
//! platform facts being staged, and the last run's captured output.

use std::path::PathBuf;
use std::process::Output;

use cucumber::World;
use doctor::facts::{Platform, PlatformFacts, UnitFacts};
use tempfile::TempDir;

#[derive(Debug, World)]
#[world(init = Self::new)]
pub struct DoctorWorld {
    /// Owns the scenario's scratch tree: `config/` plus the facts file.
    pub temp: TempDir,
    pub facts: PlatformFacts,
    /// PEM files staged by the TLS steps.
    pub pem_paths: Vec<PathBuf>,
    /// The data directory staged by the rp session steps.
    pub data_dir: Option<PathBuf>,
    /// Overrides the scenario's config dir for the exit-2 scenario.
    pub config_dir_override: Option<PathBuf>,
    pub output: Option<Output>,
    pub report: Option<serde_json::Value>,
    /// The check the last "contains a check" assertion matched, so
    /// follow-up detail/suggestion assertions have a subject.
    pub last_check: Option<serde_json::Value>,
    /// What each config file was staged with, for is-unchanged assertions.
    pub staged: std::collections::HashMap<String, String>,
    /// pki file bytes snapshotted by the "has already run" givens, for
    /// unchanged/changed assertions after a second provisioning run.
    pub pki_staged: std::collections::HashMap<String, Vec<u8>>,
    /// The ACME flag values the last `tls issue --acme` run passed, so the
    /// acme.json content assertions have expected values.
    pub acme_flags: Option<AcmeFlags>,
    /// HTTP status captured by the TLS roundtrip steps.
    pub tls_https_status: Option<u16>,
    /// The service whose cert pair the TLS roundtrip serves.
    pub tls_roundtrip_service: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AcmeFlags {
    pub domain: String,
    pub email: String,
    pub dns_provider: String,
}

impl DoctorWorld {
    fn new() -> Self {
        let temp = TempDir::new().expect("scratch dir");
        std::fs::create_dir(temp.path().join("config")).expect("config dir");
        Self {
            temp,
            facts: PlatformFacts {
                platform: Platform::Linux,
                units: Vec::new(),
                polkit_grants_sentinel_restart: None,
            },
            pem_paths: Vec::new(),
            data_dir: None,
            config_dir_override: None,
            output: None,
            report: None,
            last_check: None,
            staged: std::collections::HashMap::new(),
            pki_staged: std::collections::HashMap::new(),
            acme_flags: None,
            tls_https_status: None,
            tls_roundtrip_service: None,
        }
    }

    pub fn config_dir(&self) -> PathBuf {
        self.temp.path().join("config")
    }

    pub fn pki_dir(&self) -> PathBuf {
        self.config_dir().join("pki")
    }

    /// The credential plaintext from the canonical pki copy.
    pub fn credential(&self) -> String {
        std::fs::read_to_string(self.pki_dir().join("credential"))
            .expect("pki/credential exists")
            .trim()
            .to_string()
    }

    /// Snapshot every pki file's bytes for unchanged/changed assertions.
    pub fn snapshot_pki(&mut self) {
        self.pki_staged.clear();
        let dir = self.pki_dir();
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("pki dir missing at snapshot time: {e}"));
        for entry in entries {
            let entry = entry.expect("pki dir entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(entry.path()).expect("pki file readable");
            self.pki_staged.insert(name, bytes);
        }
    }

    pub fn write_config(&mut self, name: &str, content: &str) {
        std::fs::write(self.config_dir().join(name), content)
            .unwrap_or_else(|e| panic!("writing {name}: {e}"));
        self.staged.insert(name.to_string(), content.to_string());
    }

    /// The current on-disk JSON of a staged config file.
    pub fn config_value(&self, name: &str) -> serde_json::Value {
        let content = std::fs::read_to_string(self.config_dir().join(name))
            .unwrap_or_else(|e| panic!("reading {name}: {e}"));
        serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("{name} is not valid JSON ({e}): {content}"))
    }

    pub fn add_unit(&mut self, name: &str) {
        self.facts.units.push(UnitFacts {
            name: name.to_string(),
            enabled: true,
            condition_path: None,
            source_name: None,
        });
    }

    /// Run the doctor binary against the staged config dir and facts.
    pub fn run_doctor(&mut self, json: bool) {
        self.run_doctor_args(json, false);
    }

    /// [`run_doctor`], optionally with `--fix`.
    pub fn run_doctor_args(&mut self, json: bool, fix: bool) {
        let facts_path = self.temp.path().join("facts.json");
        std::fs::write(
            &facts_path,
            serde_json::to_string(&self.facts).expect("facts serialize"),
        )
        .expect("facts file");
        let config_dir = self
            .config_dir_override
            .clone()
            .unwrap_or_else(|| self.config_dir());

        let config_dir = config_dir.to_str().expect("utf8 path").to_string();
        let facts_path = facts_path.to_str().expect("utf8 path").to_string();
        let mut args = vec![
            "--config-dir",
            config_dir.as_str(),
            "--platform-facts",
            facts_path.as_str(),
        ];
        if json {
            args.push("--json");
        }
        if fix {
            args.push("--fix");
        }
        let output = bdd_infra::run_once("doctor", &args, None);
        self.report = if json && output.status.code() != Some(2) {
            Some(serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
                panic!(
                    "--json output is not valid JSON ({e}): {}",
                    String::from_utf8_lossy(&output.stdout)
                )
            }))
        } else {
            None
        };
        self.output = Some(output);
    }

    /// Run the doctor binary with a subcommand (`tls issue`, `auth rotate`,
    /// …) against the staged config dir and facts. The global
    /// `--config-dir`/`--platform-facts` flags precede the subcommand.
    pub fn run_doctor_subcommand(&mut self, subcommand_args: &[&str], stdin: Option<&[u8]>) {
        let facts_path = self.temp.path().join("facts.json");
        std::fs::write(
            &facts_path,
            serde_json::to_string(&self.facts).expect("facts serialize"),
        )
        .expect("facts file");
        let config_dir = self.config_dir().to_str().expect("utf8 path").to_string();
        let facts_path = facts_path.to_str().expect("utf8 path").to_string();
        let mut args = vec![
            "--config-dir",
            config_dir.as_str(),
            "--platform-facts",
            facts_path.as_str(),
        ];
        args.extend_from_slice(subcommand_args);
        let json = subcommand_args.contains(&"--json");
        let output = bdd_infra::run_once("doctor", &args, stdin);
        self.report = if json && output.status.code() == Some(0) {
            Some(serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
                panic!(
                    "--json output is not valid JSON ({e}): {}",
                    String::from_utf8_lossy(&output.stdout)
                )
            }))
        } else {
            None
        };
        self.output = Some(output);
    }

    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.as_ref().expect("run doctor first").stderr)
            .into_owned()
    }

    pub fn report(&self) -> &serde_json::Value {
        self.report.as_ref().expect("run doctor with --json first")
    }

    pub fn checks(&self) -> &Vec<serde_json::Value> {
        self.report()
            .get("checks")
            .and_then(|c| c.as_array())
            .expect("report has a checks array")
    }

    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.as_ref().expect("run doctor first").stdout)
            .into_owned()
    }
}
