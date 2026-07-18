//! The D2 check set (docs/services/doctor.md §Diagnosis).
//!
//! Every check is a pure function over the scanned configs and the gathered
//! platform facts — no network, no writes. Check names are the stable
//! identifiers the report schema carries.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::catalog::{self, CatalogEntry};
use crate::facts::{Platform, PlatformFacts};
use crate::report::{Check, Mode};
use crate::scan::{
    self, unknown_config_files, RpView, SentinelView, ServerBlock, ServiceScan, UiHtmxView,
};

/// Everything the checks look at.
pub struct Context {
    pub config_dir: PathBuf,
    pub facts: PlatformFacts,
    pub mode: Mode,
    pub scans: Vec<ServiceScan>,
    /// Device-surface facts: staged by the test seam, gathered from the
    /// host on a real run, `None` when a staged scenario has no hardware
    /// story (the family is then skipped — never probed under a mock).
    pub hardware: Option<rusty_photon_doctor_checks::HardwareFacts>,
}

impl Context {
    /// Scan the config dir and derive the mode from the unit inventory.
    pub fn gather(config_dir: PathBuf, mut facts: PlatformFacts) -> Self {
        let mode = if facts.units.is_empty() {
            Mode::ConfigOnly
        } else {
            Mode::Packaged
        };
        let scans: Vec<ServiceScan> = catalog::catalog()
            .iter()
            .map(|entry| scan::scan_service(&config_dir, entry))
            .collect();
        let hardware = facts.hardware.take().or_else(|| {
            facts.probe_hardware.then(|| {
                rusty_photon_doctor_checks::gather(&crate::hardware::probe_request(&scans, &facts))
            })
        });
        Self {
            config_dir,
            facts,
            mode,
            scans,
            hardware,
        }
    }

    fn scan(&self, name: &str) -> Option<&ServiceScan> {
        self.scans.iter().find(|s| s.entry.name == name)
    }

    /// A service takes part in diagnosis when its unit is installed or its
    /// config file exists.
    fn participates(&self, scan: &ServiceScan) -> bool {
        scan.config_present() || self.installed(scan.entry)
    }

    /// The participating service names — the "installed set" the
    /// provisioning pass and `doctor tls issue` issue certificates for.
    pub fn installed_services(&self) -> Vec<String> {
        self.scans
            .iter()
            .filter(|s| self.participates(s))
            .map(|s| s.entry.name.to_string())
            .collect()
    }

    fn installed(&self, entry: &CatalogEntry) -> bool {
        self.facts.unit(&entry.unit_name()).is_some()
    }
}

/// Run every check.
pub fn run_all(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    checks.extend(inventory(ctx));
    checks.extend(config_parsing(ctx));
    checks.extend(ports(ctx));
    checks.extend(units_and_privileges(ctx));
    checks.extend(name_joins(ctx));
    checks.extend(url_conventions(ctx));
    checks.extend(tls_and_auth(ctx));
    checks.extend(rp_platform_defaults(ctx));
    checks.extend(crate::hardware::checks(ctx));
    checks
}

fn svc(scan: &ServiceScan) -> Option<String> {
    Some(scan.entry.name.to_string())
}

// ---- Inventory (packaged mode only) ----

fn inventory(ctx: &Context) -> Vec<Check> {
    if ctx.mode != Mode::Packaged {
        return Vec::new();
    }
    let mut checks = Vec::new();
    for scan in &ctx.scans {
        let installed = ctx.installed(scan.entry);
        match (installed, scan.config_present()) {
            (true, false) => {
                // A unit gated on ConditionPathExists= hard-requires a
                // hand-written config — "start it once" would do nothing.
                let gate = ctx
                    .facts
                    .unit(&scan.entry.unit_name())
                    .and_then(|u| u.condition_path.as_ref());
                let suggestion = match gate {
                    Some(gate) => format!(
                        "this service needs a hand-written config — the unit is \
                         gated on {} — so create that file, then start the unit",
                        gate.display()
                    ),
                    None => format!(
                        "start it once so it self-creates its defaults: e.g. `{}`",
                        start_command(ctx.facts.platform, &manager_name(ctx, scan.entry))
                    ),
                };
                checks.push(Check::warn(
                    "inventory.unit-without-config",
                    svc(scan),
                    format!(
                        "unit {} is installed but {} does not exist — the service \
                         has never started, or writes its config somewhere \
                         unexpected",
                        scan.entry.unit_name(),
                        scan.config_path.display()
                    ),
                    Some(suggestion),
                ));
            }
            (false, true) => checks.push(Check::warn(
                "inventory.config-without-unit",
                svc(scan),
                format!(
                    "{} exists but no {} unit is installed — a leftover from a \
                     removed package, or a hand-copied stray",
                    scan.config_path.display(),
                    scan.entry.unit_name()
                ),
                None,
            )),
            (true, true) => checks.push(Check::ok(
                "inventory.unit-and-config",
                svc(scan),
                format!("unit installed and {} present", scan.config_path.display()),
            )),
            (false, false) => {}
        }
    }
    let known: Vec<String> = catalog::catalog().iter().map(|e| e.config_file()).collect();
    for name in unknown_config_files(&ctx.config_dir, &known) {
        checks.push(Check::warn(
            "inventory.unknown-config",
            None,
            format!(
                "{name} in {} matches no packaged service — no service will ever \
                 read it",
                ctx.config_dir.display()
            ),
            Some("rename it to a service's <svc>.json, or remove it".to_string()),
        ));
    }
    checks
}

/// The name the service manager itself knows the unit by — the brew
/// nightly channel's formula name when that is what is installed, else the
/// unit stem. Remediation text must name what the operator can type.
fn manager_name(ctx: &Context, entry: &CatalogEntry) -> String {
    ctx.facts
        .unit(&entry.unit_name())
        .and_then(|u| u.source_name.clone())
        .unwrap_or_else(|| entry.unit_name())
}

/// The platform's way to start a service once, for suggestion text.
fn start_command(platform: Platform, unit: &str) -> String {
    match platform {
        Platform::Linux => format!("systemctl start {unit}"),
        Platform::Windows => format!("Start-Service {unit}"),
        Platform::Macos => format!("brew services start {unit}"),
    }
}

// ---- Config parsing ----

fn config_parsing(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    for scan in ctx.scans.iter().filter(|s| ctx.participates(s)) {
        match &scan.raw {
            None => continue,
            Some(Err(scan::ReadError::InvalidJson(e))) => {
                checks.push(Check::fail(
                    "config.json-syntax",
                    svc(scan),
                    format!(
                        "{} is not valid JSON ({e}) — the service will refuse to \
                         start rather than silently reset it",
                        scan.config_path.display()
                    ),
                    Some("fix the JSON by hand; every field is preserved on disk".to_string()),
                ));
                continue;
            }
            Some(Err(scan::ReadError::Unreadable(e))) => {
                checks.push(Check::fail(
                    "config.unreadable",
                    svc(scan),
                    format!("{} could not be read: {e}", scan.config_path.display()),
                    Some(
                        "fix the file's permissions or ownership — the service user \
                         must be able to read and rewrite it"
                            .to_string(),
                    ),
                ));
                continue;
            }
            Some(Ok(_)) => {}
        }
        match &scan.server {
            ServerBlock::Invalid(e) => checks.push(Check::fail(
                "config.server-shape",
                svc(scan),
                format!(
                    "the server block in {} does not parse ({e}) — the service \
                     will refuse to start",
                    scan.config_path.display()
                ),
                None,
            )),
            ServerBlock::Parsed { .. } | ServerBlock::BlockAbsent => checks.push(Check::ok(
                "config.server-shape",
                svc(scan),
                match &scan.server {
                    ServerBlock::BlockAbsent => {
                        format!(
                            "no server block — defaults apply (port {})",
                            scan.entry.default_port
                        )
                    }
                    _ => format!("server block parses (port {})", scan.effective_port()),
                },
            )),
            ServerBlock::FileAbsent => {}
        }
        checks.extend(known_blocks(scan));
    }
    checks
}

/// The known cross-reference blocks must parse for the join checks to see
/// them; a shape error there is its own diagnosis. (ui-htmx has no arm:
/// since #569 its view reads only the retired `drivers` key, as an opaque
/// `Value` that cannot fail to parse.)
fn known_blocks(scan: &ServiceScan) -> Vec<Check> {
    let result = match scan.entry.name {
        "sentinel" => scan::view::<SentinelView>(scan).map(|r| r.map(|_| ())),
        "rp" => scan::view::<RpView>(scan).map(|r| r.map(|_| ())),
        _ => None,
    };
    match result {
        Some(Err(e)) => vec![Check::fail(
            "config.known-blocks",
            svc(scan),
            format!(
                "a cross-reference block in {} does not parse: {e}",
                scan.config_path.display()
            ),
            None,
        )],
        _ => Vec::new(),
    }
}

// ---- Ports ----

fn ports(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    let participants: Vec<&ServiceScan> =
        ctx.scans.iter().filter(|s| ctx.participates(s)).collect();

    let mut by_port: BTreeMap<u16, Vec<&ServiceScan>> = BTreeMap::new();
    for scan in &participants {
        by_port.entry(scan.effective_port()).or_default().push(scan);
    }
    // Ports a fix may not move a service onto: every effective port in use,
    // plus every default a fix already claimed this round.
    let mut claimed: std::collections::BTreeSet<u16> = by_port.keys().copied().collect();
    let mut collided = false;
    for (port, scans) in &by_port {
        if scans.len() > 1 {
            collided = true;
            let members = scans
                .iter()
                .map(|s| {
                    let source = match &s.server {
                        ServerBlock::Parsed { .. } => "configured",
                        _ => "default",
                    };
                    format!("{} ({source})", s.entry.name)
                })
                .collect::<Vec<_>>()
                .join(", ");
            // The derivable repair: a configured member whose own catalog
            // default is free goes back to it. A member already at its
            // default, or whose default is taken, is a judgment call — the
            // suggestion text covers it.
            let mut fixes = Vec::new();
            for scan in scans {
                let configured = matches!(&scan.server, ServerBlock::Parsed { .. });
                let default = scan.entry.default_port;
                if configured && default != *port && !claimed.contains(&default) {
                    claimed.insert(default);
                    fixes.push(crate::report::FixOp::SetNumber {
                        service: scan.entry.name.to_string(),
                        pointer: "/server/port".to_string(),
                        value: u64::from(default),
                    });
                }
            }
            checks.push(
                Check::fail(
                    "ports.collision",
                    svc(scans[0]),
                    format!(
                        "port {port} is claimed by {} services — {members} — and only \
                         one can bind",
                        scans.len()
                    ),
                    Some(format!(
                        "give each a distinct server.port (defaults: {})",
                        scans
                            .iter()
                            .map(|s| format!("{} {}", s.entry.name, s.entry.default_port))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                )
                .with_fixes(fixes),
            );
        }
    }
    if !collided && !participants.is_empty() {
        let n = by_port.len();
        checks.push(Check::ok(
            "ports.collision",
            None,
            format!(
                "{n} effective port{}, all distinct",
                if n == 1 { "" } else { "s" }
            ),
        ));
    }

    let mut by_discovery: BTreeMap<u16, Vec<&ServiceScan>> = BTreeMap::new();
    for scan in &participants {
        if let Some(port) = scan.discovery_port() {
            by_discovery.entry(port).or_default().push(scan);
        }
    }
    for (port, scans) in &by_discovery {
        if scans.len() > 1 {
            let members = scans
                .iter()
                .map(|s| s.entry.name)
                .collect::<Vec<_>>()
                .join(", ");
            checks.push(Check::fail(
                "ports.discovery-collision",
                svc(scans[0]),
                format!(
                    "discovery_port {port} is enabled by {members} — UDP responders \
                     collide; discovery is a per-host opt-in for one driver"
                ),
                Some("remove discovery_port from all but one config".to_string()),
            ));
        }
    }
    checks
}

// ---- Units and privileges (systemd facts) ----

fn units_and_privileges(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    if ctx.facts.platform != Platform::Linux {
        return checks;
    }
    for unit in &ctx.facts.units {
        let Some(entry) = catalog::entry_for_unit(&unit.name) else {
            continue;
        };
        let Some(path) = &unit.condition_path else {
            continue;
        };
        if !unit.enabled {
            continue;
        }
        if path.exists() {
            checks.push(Check::ok(
                "units.config-gated",
                Some(entry.name.to_string()),
                format!("{} gate {} is satisfied", unit.name, path.display()),
            ));
        } else {
            checks.push(Check::fail(
                "units.config-gated",
                Some(entry.name.to_string()),
                format!(
                    "{} is enabled but its ConditionPathExists gate {} is missing — \
                     installed, enabled, and silently inert",
                    unit.name,
                    path.display()
                ),
                Some(format!(
                    "create {} (the service needs a hand-written config) and start \
                     the unit",
                    path.display()
                )),
            ));
        }
    }
    if ctx.facts.unit("rusty-photon-sentinel").is_some() {
        match ctx.facts.polkit_grants_sentinel_restart {
            Some(true) => checks.push(Check::ok(
                "sentinel.privilege-path",
                Some("sentinel".to_string()),
                "a polkit rule grants sentinel's user manage-units for \
                 rusty-photon-* units"
                    .to_string(),
            )),
            Some(false) => checks.push(Check::fail(
                "sentinel.privilege-path",
                Some("sentinel".to_string()),
                "no polkit rule granting the rusty-photon user \
                 org.freedesktop.systemd1.manage-units for rusty-photon-* units \
                 was found (heuristic scan of the polkit rules directories) — the \
                 packaged sentinel unit runs unprivileged with \
                 NoNewPrivileges=yes, so every restart it attempts will be denied"
                    .to_string(),
                Some(
                    "install the packaged rule (shipped with the sentinel deb/rpm \
                     under /usr/share/polkit-1/rules.d/) or add one under \
                     /etc/polkit-1/rules.d/"
                        .to_string(),
                ),
            )),
            None => {}
        }
    }
    checks
}

// ---- Name joins ----

fn name_joins(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    let sentinel_view: Option<SentinelView> =
        ctx.scan("sentinel").and_then(|s| scan::view(s)?.ok());
    let ui_view: Option<UiHtmxView> = ctx.scan("ui-htmx").and_then(|s| scan::view(s)?.ok());

    checks.extend(retired_keys(&sentinel_view, &ui_view));
    if let Some(sentinel) = &sentinel_view {
        checks.extend(watchdog_joins(ctx, sentinel));
    }
    checks
}

/// Config keys retired by D3s and #569: sentinel's `services` map (sentinel
/// discovers its services) and ui-htmx's whole `drivers` override map (rp's
/// roster is the only device source). Both fail the service's own strict
/// load, so the file is dead weight that keeps the service from starting.
fn retired_keys(sentinel: &Option<SentinelView>, ui: &Option<UiHtmxView>) -> Vec<Check> {
    let mut checks = Vec::new();
    if sentinel.as_ref().is_some_and(|s| s.services.is_some()) {
        checks.push(
            Check::fail(
                "config.retired-keys",
                Some("sentinel".to_string()),
                "sentinel.json carries the retired services map — sentinel discovers \
                 its services from the platform service manager now, and refuses to \
                 start while the key is present"
                    .to_string(),
                Some(
                    "delete the top-level \"services\" key; supervision needs no \
                     replacement config"
                        .to_string(),
                ),
            )
            .with_fixes(vec![crate::report::FixOp::RemoveKey {
                service: "sentinel".to_string(),
                pointer: "/services".to_string(),
            }]),
        );
    }
    if ui.as_ref().is_some_and(|u| u.drivers.is_some()) {
        checks.push(
            Check::fail(
                "config.retired-keys",
                Some("ui-htmx".to_string()),
                "ui-htmx.json carries the retired drivers override map — rp's \
                 equipment roster is the only device source now, and ui-htmx \
                 refuses to start while the key is present"
                    .to_string(),
                Some(
                    "delete the top-level \"drivers\" key; devices belong in rp's \
                     equipment roster"
                        .to_string(),
                ),
            )
            .with_fixes(vec![crate::report::FixOp::RemoveKey {
                service: "ui-htmx".to_string(),
                pointer: "/drivers".to_string(),
            }]),
        );
    }
    checks
}

/// The installed rusty-photon units' service names (unit minus the prefix) —
/// what sentinel's discovery will resolve restart names against.
fn discovered_service_names(ctx: &Context) -> Vec<String> {
    ctx.facts
        .units
        .iter()
        .filter_map(|u| u.name.strip_prefix("rusty-photon-"))
        .map(str::to_string)
        .collect()
}

fn watchdog_joins(ctx: &Context, sentinel: &SentinelView) -> Vec<Check> {
    let mut checks = Vec::new();
    if ctx.mode != Mode::Packaged {
        return checks;
    }
    let Some(watchdog) = &sentinel.operation_watchdog else {
        return checks;
    };
    let discovered = discovered_service_names(ctx);
    for (family, operation) in &watchdog.operations {
        let Some(service) = &operation.service else {
            continue;
        };
        if !discovered.iter().any(|name| name == service) {
            checks.push(Check::fail(
                "joins.watchdog-service",
                Some("sentinel".to_string()),
                format!(
                    "operation_watchdog.operations.{family}.service names \
                     \"{service}\", but no rusty-photon-{service} unit is installed \
                     — sentinel's discovery will never resolve it, so the \
                     watchdog's ladder degrades to notify-only"
                ),
                Some(format!("installed services are: {}", discovered.join(", "))),
            ));
        }
    }
    checks
}

// ---- URL conventions ----

fn url_conventions(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    let carries_suffix = |url: &str| url.trim_end_matches('/').ends_with("/api/v1");
    let stripped = |url: &str| {
        url.trim_end_matches('/')
            .trim_end_matches("/api/v1")
            .to_string()
    };
    let spurious = |service: &str, field: String, url: &str| {
        Check::warn(
            "urls.spurious-suffix",
            Some(service.to_string()),
            format!(
                "{field} ({url}) carries /api/v1, but this client appends it \
                 itself — requests would double the prefix and 404"
            ),
            Some(format!("use {}", stripped(url))),
        )
    };
    // rp's alpaca_url lives inside the device-usage block doctor checks but
    // does not own (ADR-016 decision 4): suggestion only, never a fix.
    if let Some(rp) = ctx.scan("rp").and_then(|s| scan::view::<RpView>(s)?.ok()) {
        for url in rp.alpaca_urls() {
            if carries_suffix(&url) {
                checks.push(spurious("rp", "an equipment alpaca_url".to_string(), &url));
            }
        }
    }
    checks
}

// ---- TLS and auth ----

fn tls_and_auth(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    for scan in ctx.scans.iter().filter(|s| ctx.participates(s)) {
        checks.extend(tls_auth_absent(ctx, scan));
        let Some(server) = scan.server() else {
            continue;
        };
        if let Some(tls) = &server.tls {
            // Per path: empty or absent-on-disk is a failure; a relative
            // path is ungradable — the service resolves it against its own
            // working directory (`TlsConfig::resolved_*_path` only expands
            // `~`), which doctor cannot know, so claiming presence either
            // way would be a guess.
            let mut missing: Vec<String> = Vec::new();
            let mut relative: Vec<String> = Vec::new();
            for (raw, resolved) in [
                (&tls.cert, tls.resolved_cert_path()),
                (&tls.key, tls.resolved_key_path()),
            ] {
                if raw.trim().is_empty() {
                    missing.push("<empty path>".to_string());
                } else if !resolved.is_absolute() {
                    relative.push(raw.clone());
                } else if !resolved.is_file() {
                    missing.push(raw.clone());
                }
            }
            if !missing.is_empty() {
                checks.push(Check::fail(
                    "tls.paths",
                    svc(scan),
                    format!(
                        "server.tls points at missing material: {} — the service \
                         will refuse to serve at next start",
                        missing.join(", ")
                    ),
                    Some(
                        "generate certs (`doctor tls issue`, or `doctor --fix` to also \
                         wire the config) or fix the paths"
                            .to_string(),
                    ),
                ));
            } else if !relative.is_empty() {
                checks.push(Check::warn(
                    "tls.paths",
                    svc(scan),
                    format!(
                        "server.tls uses relative paths ({}): the service resolves \
                         them against its own working directory, which doctor \
                         cannot know, so the material cannot be judged",
                        relative.join(", ")
                    ),
                    Some(
                        "use absolute paths — doctor-issued material always is, and \
                         `doctor --fix` writes absolute paths"
                            .to_string(),
                    ),
                ));
            } else {
                checks.push(Check::ok(
                    "tls.paths",
                    svc(scan),
                    "TLS cert and key exist".to_string(),
                ));
            }
            // Expiry is judged only when tls.paths is clean — a missing or
            // ungradable pair stays tls.paths' concern, and an expiry
            // verdict beside a failing pair would read as contradictory.
            let cert_file = tls.resolved_cert_path();
            if missing.is_empty() && relative.is_empty() {
                checks.push(tls_expiry(ctx, scan, &cert_file));
            }
        }
        if server.auth.is_some() && server.tls.is_none() {
            checks.push(Check::warn(
                "tls.auth-without-tls",
                svc(scan),
                "server.auth without server.tls sends HTTP Basic credentials in \
                 cleartext on the wire"
                    .to_string(),
                Some("add a server.tls block (ADR-003: Basic auth over TLS)".to_string()),
            ));
        }
    }
    checks.extend(auth_mismatch(ctx));
    checks
}

/// The D6a absent checks: an installed service without a `server.tls` /
/// `server.auth` block serves plain, unauthenticated HTTP. Legal (absent
/// means off — ADR-016 decision 10(d)) and fixable: each check plans the
/// whole-block write the provisioning pass applies. The `auth` plan needs
/// the observatory credential, so it appears only once `pki/credential`
/// exists (under `--fix` the material pass runs first).
fn tls_auth_absent(ctx: &Context, scan: &ServiceScan) -> Vec<Check> {
    if scan.value().is_none() {
        // No parseable file: the read-level checks own the diagnosis, and
        // provisioning has nothing to write into.
        return Vec::new();
    }
    let (tls_absent, auth_absent, server_key_present) = match &scan.server {
        ServerBlock::Parsed { server, .. } => (server.tls.is_none(), server.auth.is_none(), true),
        // Valid JSON without a server key: the service applies its plain
        // HTTP defaults, so both blocks are absent.
        ServerBlock::BlockAbsent => (true, true, false),
        // An unparseable block is config.server-shape's diagnosis; writing
        // into it would be guesswork.
        ServerBlock::Invalid(_) | ServerBlock::FileAbsent => return Vec::new(),
    };
    let name = scan.entry.name;
    let mut checks = Vec::new();
    if tls_absent {
        let tls_value = crate::provision::tls_block_value(&ctx.config_dir, name);
        let fixes = if server_key_present {
            vec![crate::report::FixOp::SetObject {
                service: name.to_string(),
                pointer: "/server/tls".to_string(),
                value: tls_value,
            }]
        } else {
            // No server key at all: the block is created whole, keeping the
            // port the service would have defaulted to.
            vec![crate::report::FixOp::SetObject {
                service: name.to_string(),
                pointer: "/server".to_string(),
                value: serde_json::json!({ "port": scan.entry.default_port, "tls": tls_value }),
            }]
        };
        checks.push(
            Check::warn(
                "tls.absent",
                svc(scan),
                format!("{name} has no server.tls block — it serves plain HTTP"),
                Some(
                    "run `doctor --fix` to issue a certificate and turn TLS on \
                     (services pick it up at next restart)"
                        .to_string(),
                ),
            )
            .with_fixes(fixes),
        );
    }
    if auth_absent {
        let fixes = match plan_auth_block(ctx) {
            Some(value) => vec![crate::report::FixOp::SetObject {
                service: name.to_string(),
                pointer: "/server/auth".to_string(),
                value,
            }],
            None => Vec::new(),
        };
        checks.push(
            Check::warn(
                "auth.absent",
                svc(scan),
                format!("{name} has no server.auth block — it answers unauthenticated"),
                Some(
                    "run `doctor --fix` to mint the observatory credential and turn \
                     auth on (services pick it up at next restart)"
                        .to_string(),
                ),
            )
            .with_fixes(fixes),
        );
    }
    checks
}

/// The `server.auth` block value for one service: the observatory username
/// and a fresh Argon2id hash of the minted credential. `None` until the
/// credential exists.
fn plan_auth_block(ctx: &Context) -> Option<serde_json::Value> {
    let password = crate::provision::read_credential(&ctx.config_dir)?;
    match rp_auth::credentials::hash_password(&password) {
        Ok(hash) => Some(serde_json::json!({
            "username": crate::provision::CREDENTIAL_USERNAME,
            "password_hash": hash,
        })),
        Err(e) => {
            tracing::warn!("could not hash the observatory credential: {e}");
            None
        }
    }
}

/// `auth.mismatch`: sentinel's `service_auth` plaintext must verify
/// (Argon2id) against each installed service's `server.auth` hash, or its
/// authenticated probes will 401. Suggestion-only — hand-set credentials
/// are operator intent, so doctor reports the pair and points at
/// `doctor auth rotate`.
fn auth_mismatch(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(sentinel_scan) = ctx.scan("sentinel").filter(|s| ctx.participates(s)) else {
        return checks;
    };
    let Some(sentinel) = scan::view::<SentinelView>(sentinel_scan).and_then(Result::ok) else {
        return checks;
    };
    let Some(client) = sentinel.service_auth else {
        return checks;
    };
    let (Some(username), Some(password)) = (client.username, client.password) else {
        return checks;
    };
    for scan in ctx.scans.iter().filter(|s| ctx.participates(s)) {
        if scan.entry.name == "sentinel" {
            // service_auth is for the supervised peers; sentinel does not
            // probe itself.
            continue;
        }
        let Some(auth) = scan.server().and_then(|s| s.auth.as_ref()) else {
            continue;
        };
        let username_matches = auth.username == username;
        if username_matches && rp_auth::credentials::verify_password(&password, &auth.password_hash)
        {
            continue;
        }
        let what = if username_matches {
            "password does not verify against"
        } else {
            "username does not match"
        };
        checks.push(Check::warn(
            "auth.mismatch",
            svc(scan),
            format!(
                "sentinel's service_auth {what} {}'s server.auth — its \
                 authenticated probes will get 401s",
                scan.entry.name
            ),
            Some(
                "run `doctor auth rotate` to re-mint the observatory credential \
                 and re-align every copy, or fix the pair by hand"
                    .to_string(),
            ),
        ));
    }
    checks
}

/// `tls.expiry` (D6b): grade an existing configured certificate's
/// `not_after`. Expired or unparseable fails — rustls loads an expired
/// certificate cleanly and only *clients* reject the handshake, so without
/// this check the failure surfaces as every client erroring at night.
/// Inside the renewal window warns. Suggestion-only: renewal belongs on
/// the platform timer, so `--fix` never renews.
fn tls_expiry(ctx: &Context, scan: &ServiceScan, cert_file: &Path) -> Check {
    let suggestion = "run `doctor tls renew` (the platform timer's command) for \
                      doctor-issued material (`doctor tls issue --force` re-issues \
                      it with fresh SANs); the ACME wildcard renews only while \
                      `acme.json` still sits beside the configs — re-run `doctor \
                      tls issue --acme` if it is gone; a certificate doctor did \
                      not issue must be replaced by whatever issued it"
        .to_string();
    let pem = match std::fs::read_to_string(cert_file) {
        Ok(pem) => pem,
        Err(e) => {
            return Check::fail(
                "tls.expiry",
                svc(scan),
                format!("{} could not be read: {e}", cert_file.display()),
                Some(suggestion),
            )
        }
    };
    let not_after = match crate::provision::expiry::not_after(&pem) {
        Ok(not_after) => not_after,
        Err(e) => {
            return Check::fail(
                "tls.expiry",
                svc(scan),
                format!(
                    "{} is not a parseable certificate ({e}) — the service \
                     cannot serve it",
                    cert_file.display()
                ),
                Some(suggestion),
            )
        }
    };
    let now = time::OffsetDateTime::now_utc();
    if not_after <= now {
        return Check::fail(
            "tls.expiry",
            svc(scan),
            format!(
                "{} expired {not_after} — the server still loads it, and every \
                 client rejects the handshake",
                cert_file.display()
            ),
            Some(suggestion),
        );
    }
    let window_days = expiry_window_days(ctx, cert_file);
    if not_after - now <= time::Duration::days(window_days) {
        return Check::warn(
            "tls.expiry",
            svc(scan),
            format!(
                "{} expires {not_after}, inside its {window_days}-day renewal \
                 window",
                cert_file.display()
            ),
            Some(suggestion),
        );
    }
    Check::ok(
        "tls.expiry",
        svc(scan),
        format!("certificate valid until {not_after}"),
    )
}

/// The warn window: 30 days for self-signed material,
/// `renewal_days_before_expiry` from `acme.json` for the ACME wildcard
/// pair.
fn expiry_window_days(ctx: &Context, cert_file: &Path) -> i64 {
    if cert_file.file_name().is_some_and(|n| n == "acme-cert.pem") {
        if let Ok(config) =
            crate::provision::acme_config::load_acme_config(&ctx.config_dir.join("acme.json"))
        {
            return i64::from(config.renewal_days_before_expiry);
        }
    }
    30
}

// ---- rp platform defaults ----

fn rp_platform_defaults(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(rp_scan) = ctx.scan("rp") else {
        return checks;
    };
    let Some(rp) = scan::view::<RpView>(rp_scan).and_then(Result::ok) else {
        return checks;
    };
    let Some(dir) = rp.session.and_then(|s| s.data_directory) else {
        return checks;
    };
    let path = Path::new(&dir);
    if path.is_dir() {
        // Packaged Linux: existence is not enough — under systemd rp runs
        // as the rusty-photon user, and a root-owned directory from a
        // sudo'd first run is a classic way to strand session
        // persistence. Judged from ownership and mode (gathered facts),
        // so ACLs are invisible. Dev checkouts keep the existence-only
        // check, and so do macOS/Windows installs — brew services run as
        // the operator and the MSI's services as LocalSystem, so there is
        // no rusty-photon user to judge for.
        let unwritable = (ctx.mode == Mode::Packaged && ctx.facts.platform == Platform::Linux)
            .then_some(ctx.hardware.as_ref())
            .flatten()
            .and_then(|hw| {
                let node = hw.paths.get(&dir)?;
                let user = hw.service_user?;
                let identity = rusty_photon_doctor_checks::Identity {
                    uid: user.uid,
                    gids: vec![user.gid],
                };
                (!identity.can_write_dir(node)).then_some((node.mode, node.uid, node.gid))
            });
        match unwritable {
            Some((mode, uid, gid)) => checks.push(Check::fail(
                "rp.data-directory",
                Some("rp".to_string()),
                format!(
                    "session.data_directory {dir} exists but is not writable by \
                     the {} user (mode {mode:o}, uid {uid}, gid {gid}) — judged \
                     from ownership and mode, so ACLs are invisible to this check",
                    crate::hardware::SERVICE_USER
                ),
                Some(format!(
                    "chown it to the service user: `chown {}: {dir}`",
                    crate::hardware::SERVICE_USER
                )),
            )),
            None => checks.push(Check::ok(
                "rp.data-directory",
                Some("rp".to_string()),
                format!("session.data_directory {dir} exists"),
            )),
        }
    } else {
        checks.push(Check::fail(
            "rp.data-directory",
            Some("rp".to_string()),
            format!(
                "session.data_directory {dir} does not exist — session state \
                 cannot persist, and rp's scaffold default is not valid on every \
                 platform"
            ),
            Some(format!(
                "create it (`mkdir -p {dir}`) or point rp elsewhere"
            )),
        ));
    }
    checks
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use rusty_photon_doctor_checks::report::Status;

    fn config_only_ctx(config_dir: &Path) -> Context {
        let facts: PlatformFacts =
            serde_json::from_value(serde_json::json!({ "platform": "linux" })).unwrap();
        Context::gather(config_dir.to_path_buf(), facts)
    }

    #[test]
    fn test_tls_expiry_fails_on_an_unreadable_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = config_only_ctx(dir.path());
        let scan = &ctx.scans[0];
        // A directory at the cert path: read_to_string errors while the
        // path itself exists.
        let check = tls_expiry(&ctx, scan, dir.path());
        assert_eq!(check.status, Status::Fail);
        assert!(
            check.detail.contains("could not be read"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn test_expiry_window_days_reads_the_acme_config_for_the_wildcard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("acme.json"),
            serde_json::json!({
                "email": "ops@example.com",
                "domain": "observatory.test",
                "dns_provider": "cloudflare",
                "dns_credentials": { "api_token": "tok" },
                "renewal_days_before_expiry": 33,
            })
            .to_string(),
        )
        .unwrap();
        let ctx = config_only_ctx(dir.path());
        assert_eq!(expiry_window_days(&ctx, Path::new("pki/acme-cert.pem")), 33);
        // A self-signed pair keeps the 30-day default even with acme.json.
        assert_eq!(expiry_window_days(&ctx, Path::new("pki/rp.pem")), 30);
    }

    #[test]
    fn test_expiry_window_days_defaults_without_acme_json() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = config_only_ctx(dir.path());
        assert_eq!(expiry_window_days(&ctx, Path::new("pki/acme-cert.pem")), 30);
    }

    #[test]
    fn test_start_command_speaks_each_platforms_language() {
        assert_eq!(
            start_command(Platform::Linux, "rusty-photon-rp"),
            "systemctl start rusty-photon-rp"
        );
        assert_eq!(
            start_command(Platform::Windows, "rusty-photon-rp"),
            "Start-Service rusty-photon-rp"
        );
        assert_eq!(
            start_command(Platform::Macos, "rusty-photon-rp-nightly"),
            "brew services start rusty-photon-rp-nightly"
        );
    }
}
