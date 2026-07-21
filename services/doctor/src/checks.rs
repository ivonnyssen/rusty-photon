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
    self, unknown_config_files, ClientAuthView, ClientTargetView, MonitorView, RpView,
    SentinelView, ServerBlock, ServiceScan, UiHtmxView,
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
    checks.extend(client_target_joins(ctx));
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
/// them; a shape error there is its own diagnosis.
fn known_blocks(scan: &ServiceScan) -> Vec<Check> {
    let result = match scan.entry.name {
        "sentinel" => scan::view::<SentinelView>(scan).map(|r| r.map(|_| ())),
        "rp" => scan::view::<RpView>(scan).map(|r| r.map(|_| ())),
        "ui-htmx" => scan::view::<UiHtmxView>(scan).map(|r| r.map(|_| ())),
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
        // On an ACME install the fix points at the shared wildcard pair —
        // never at freshly issued self-signed material, which the flipped
        // fleet's clients could not verify (issue #616). While the pair is
        // missing, conjuring it is renewal's job, so no fix is planned.
        let acme = crate::provision::acme_active(&ctx.config_dir);
        let tls_value = if acme {
            crate::provision::acme_tls_block_value(&ctx.config_dir)
        } else {
            Some(crate::provision::tls_block_value(&ctx.config_dir, name))
        };
        let fixes = match tls_value {
            Some(tls_value) if server_key_present => vec![crate::report::FixOp::SetObject {
                service: name.to_string(),
                pointer: "/server/tls".to_string(),
                value: tls_value,
            }],
            // No server key at all: the block is created whole, keeping the
            // port the service would have defaulted to.
            Some(tls_value) => vec![crate::report::FixOp::SetObject {
                service: name.to_string(),
                pointer: "/server".to_string(),
                value: serde_json::json!({ "port": scan.entry.default_port, "tls": tls_value }),
            }],
            None => Vec::new(),
        };
        let suggestion = if fixes.is_empty() {
            "this is an ACME install (acme.json present) but the wildcard pair is \
             missing — run `doctor tls renew` to obtain it, then `doctor --fix` to \
             wire the config"
        } else if acme {
            "run `doctor --fix` to point server.tls at the ACME wildcard pair \
             (services pick it up at next restart)"
        } else {
            "run `doctor --fix` to issue a certificate and turn TLS on \
             (services pick it up at next restart)"
        };
        checks.push(
            Check::warn(
                "tls.absent",
                svc(scan),
                format!("{name} has no server.tls block — it serves plain HTTP"),
                Some(suggestion.to_string()),
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

// ---- Client-target joins ----
//
// A client's config points a URL (or, for sentinel's monitors, a
// scheme/host/port triple) at another catalog service. These checks join
// that URL against the *named* service's own `server.tls`/`server.auth` —
// the gap #607 named: provisioning upgrades a service's server side, but
// nothing told doctor to look at who points at it.

/// Doctor diagnoses one config directory, so a client→target join only
/// resolves when the URL's host names *this* machine — a different host
/// names a service in a config file doctor cannot see. This covers every
/// client target's shipped default (all loopback).
fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// The one local, participating catalog service a client's `host:port`
/// names, or `None` when the host is not this machine, no service claims
/// the port, the service's own block does not parse or its config file
/// does not exist (`config.server-shape` owns that diagnosis; writing a
/// join verdict against an unreadable or absent block would be
/// guesswork — mirrors `tls_auth_absent`'s identical distinction), or
/// more than one participating service claims the port — an ambiguous
/// join `ports.collision` already reports as its own `fail`, so this
/// self-limits rather than guessing which of the colliding services the
/// client actually meant. A config file that simply omits `server`
/// entirely (`ServerBlock::BlockAbsent`) is not guesswork, though — it is
/// the documented "plain HTTP, no auth, catalog default port" state, so
/// it still resolves.
fn resolve_join_target<'a>(ctx: &'a Context, host: &str, port: u16) -> Option<&'a ServiceScan> {
    if !is_loopback_host(host) {
        return None;
    }
    let mut matches = ctx.scans.iter().filter(|s| {
        ctx.participates(s)
            && !matches!(s.server, ServerBlock::Invalid(_) | ServerBlock::FileAbsent)
            && s.effective_port() == port
    });
    let target = matches.next()?;
    matches.next().is_none().then_some(target)
}

/// Whether `target`'s configured certificate is the ACME wildcard pair —
/// publicly trusted, so an absent client `ca_cert_path` is not a problem —
/// versus doctor's self-signed CA, which every client must be told to
/// trust explicitly. Mirrors `expiry_window_days`'s file-name convention.
fn target_uses_acme_cert(target: &ServiceScan) -> bool {
    target
        .server()
        .and_then(|s| s.tls.as_ref())
        .is_some_and(|tls| {
            tls.resolved_cert_path()
                .file_name()
                .is_some_and(|n| n == "acme-cert.pem")
        })
}

/// The scheme a target's current TLS state calls for — the single
/// source of truth every scheme-mismatch check and fix compares
/// against, so a garbage/unsupported scheme (e.g. `ftp`) is judged the
/// same way everywhere rather than silently matching by accident.
fn expected_scheme(target_tls_on: bool) -> &'static str {
    if target_tls_on {
        "https"
    } else {
        "http"
    }
}

/// Parse a client URL into `(scheme, host, port)` — `None` when it does
/// not parse or omits an explicit port (every rusty-photon service URL
/// carries one; a bare default like `https://host/` names nothing in the
/// catalog anyway).
fn parse_target_url(url: &str) -> Option<(String, String, u16)> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_string();
    let port = parsed.port()?;
    Some((parsed.scheme().to_string(), host, port))
}

/// Rewrite a URL's scheme, preserving everything after `://` byte-for-byte —
/// the `--fix` value for a client target whose scheme lives inside a full
/// URL string. A parse-and-reserialize round trip (`Url::set_scheme` +
/// `to_string`) would normalize an origin-only URL by appending a trailing
/// `/` (`http://host:port` becomes `http://host:port/`), which several
/// client call sites (e.g. ui-htmx's `sse_proxy`) concatenate a
/// `/`-prefixed path onto without trimming, turning a healthy URL into a
/// double-slashed one that 404s.
fn rewrite_scheme(url: &str, new_scheme: &str) -> Option<String> {
    reqwest::Url::parse(url).ok()?;
    // Split on the literal separator rather than stripping `Url::scheme()`
    // (which the `url` crate lowercases) off the raw string — that would
    // silently fail to strip an input like `HTTP://host:port`.
    let (_, rest) = url.split_once("://")?;
    Some(format!("{new_scheme}://{rest}"))
}

/// `joins.client-transport`: does `scheme` (as `client_field` declares it)
/// match what `target` actually serves, and — when it does, and `target`'s
/// material is doctor's self-signed CA rather than a publicly-trusted ACME
/// cert — can this client trust it. Either gap breaks every request to
/// `target`, so both grade `fail` (mirrors `tls.paths`: a definite break,
/// not a hardware-style installed/enabled split).
///
/// `scheme_fix` plans the scheme rewrite when the client's schema supports
/// one. `ca_cert` is `Some((pointer, already_present))` when the client
/// schema carries a CA-trust field.
fn transport_check(
    ctx: &Context,
    client_service: &str,
    client_field: &str,
    scheme: &str,
    target: &ServiceScan,
    scheme_fix: Option<crate::report::FixOp>,
    ca_cert: Option<(String, bool)>,
) -> Option<Check> {
    let target_tls_on = target.server().is_some_and(|s| s.tls.is_some());
    let mut problems = Vec::new();
    let mut fixes = Vec::new();

    if !scheme.eq_ignore_ascii_case(expected_scheme(target_tls_on)) {
        problems.push(format!(
            "{client_field} uses {scheme}, but {} {} TLS",
            target.entry.name,
            if target_tls_on {
                "serves"
            } else {
                "does not serve"
            }
        ));
        match scheme_fix {
            Some(fix) => fixes.push(fix),
            None => problems.push(format!(
                "{client_service} has no field `doctor --fix` can safely rewrite for \
                 this target yet"
            )),
        }
    }

    if target_tls_on && !target_uses_acme_cert(target) {
        if let Some((pointer, present)) = ca_cert {
            if !present {
                let field_name = pointer.rsplit('/').next().unwrap_or(pointer.as_str());
                let ca_path = rusty_photon_tls::config::ca_cert_path(
                    &crate::provision::absolute_pki_dir(&ctx.config_dir),
                );
                if ca_path.is_file() {
                    problems.push(format!(
                        "{} serves a self-signed certificate, but {client_field} has \
                         no {field_name} to trust it",
                        target.entry.name
                    ));
                    fixes.push(crate::report::FixOp::SetString {
                        service: client_service.to_string(),
                        pointer,
                        value: ca_path.to_string_lossy().into_owned(),
                    });
                } else {
                    problems.push(format!(
                        "{} serves a self-signed certificate, but {client_field} has \
                         no {field_name} to trust it, and doctor's own CA material \
                         does not exist yet for `--fix` to wire in",
                        target.entry.name
                    ));
                }
            }
        }
    }

    if problems.is_empty() {
        return None;
    }
    let suggestion = if fixes.is_empty() {
        format!("fix {client_field} by hand — no machine-applicable fix exists for this yet")
    } else {
        "run `doctor --fix` to align the client with its target's TLS state".to_string()
    };
    Some(
        Check::fail(
            "joins.client-transport",
            Some(client_service.to_string()),
            format!("{} — every request will fail", problems.join("; ")),
            Some(suggestion),
        )
        .with_fixes(fixes),
    )
}

/// The client-side credential value `{username, password}` — the plaintext
/// observatory credential, when minted. Mirrors
/// `provision::plan_service_client_wiring`'s inline shape.
fn plan_client_auth_value(ctx: &Context) -> Option<serde_json::Value> {
    let password = crate::provision::read_credential(&ctx.config_dir)?;
    Some(serde_json::json!({
        "username": crate::provision::CREDENTIAL_USERNAME,
        "password": password,
    }))
}

/// `joins.client-auth`: does `target` require authentication, and if so,
/// can this client supply a credential that verifies against it. Warn,
/// matching `auth.mismatch`'s severity — a wrong or missing credential
/// 401s every request, but (as with that check) a *present* mismatched
/// credential may be intentional, so only the absent case is fix-eligible.
/// `auth_pointer` is `None` for rp's plate-solver/guider clients, which
/// carry no credential field at all.
fn credential_check(
    ctx: &Context,
    client_service: &str,
    client_field: &str,
    target: &ServiceScan,
    auth_pointer: Option<String>,
    current: Option<&ClientAuthView>,
) -> Option<Check> {
    let target_auth = target.server().and_then(|s| s.auth.as_ref())?;
    let credential = current.and_then(|c| Some((c.username.as_deref()?, c.password.as_deref()?)));
    match credential {
        None => {
            let fixes = match (&auth_pointer, plan_client_auth_value(ctx)) {
                (Some(pointer), Some(value)) => vec![crate::report::FixOp::SetObject {
                    service: client_service.to_string(),
                    pointer: pointer.clone(),
                    value,
                }],
                _ => Vec::new(),
            };
            let suggestion = if auth_pointer.is_some() {
                "run `doctor --fix` to wire the observatory credential".to_string()
            } else {
                format!(
                    "{client_service} has no credential field for this target yet — \
                     wiring one needs a config-schema change"
                )
            };
            Some(
                Check::warn(
                    "joins.client-auth",
                    Some(client_service.to_string()),
                    format!(
                        "{} requires authentication, but {client_field} carries no \
                         credential — every request will get 401",
                        target.entry.name
                    ),
                    Some(suggestion),
                )
                .with_fixes(fixes),
            )
        }
        Some((username, password)) => {
            if username == target_auth.username
                && rp_auth::credentials::verify_password(password, &target_auth.password_hash)
            {
                return None;
            }
            Some(Check::warn(
                "joins.client-auth",
                Some(client_service.to_string()),
                format!(
                    "{client_field}'s credential does not verify against {}'s \
                     server.auth — every request will get 401",
                    target.entry.name
                ),
                Some(
                    "run `doctor auth rotate` to re-align every copy, or fix the pair \
                     by hand"
                        .to_string(),
                ),
            ))
        }
    }
}

fn client_target_joins(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    checks.extend(ui_htmx_target_joins(ctx));
    checks.extend(rp_client_joins(ctx));
    checks.extend(sentinel_client_joins(ctx));
    checks
}

/// ui-htmx's `rp` (required) and `sentinel` (optional) targets — both
/// carry `base_url` + `auth` + `ca_cert_path`, so both the transport and
/// credential checks are fully fix-eligible.
fn ui_htmx_target_joins(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(ui_scan) = ctx.scan("ui-htmx").filter(|s| ctx.participates(s)) else {
        return checks;
    };
    let Some(ui) = scan::view::<UiHtmxView>(ui_scan).and_then(Result::ok) else {
        return checks;
    };
    checks.extend(ui_htmx_one_target(ctx, "rp", ui.rp.as_ref()));
    checks.extend(ui_htmx_one_target(ctx, "sentinel", ui.sentinel.as_ref()));
    checks
}

fn ui_htmx_one_target(ctx: &Context, name: &str, target: Option<&ClientTargetView>) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(target) = target else {
        return checks;
    };
    let Some(base_url) = target.base_url.as_deref() else {
        return checks;
    };
    let Some((scheme, host, port)) = parse_target_url(base_url) else {
        return checks;
    };
    let Some(resolved) = resolve_join_target(ctx, &host, port) else {
        return checks;
    };

    let transport_field = format!("{name}.base_url");
    let auth_field = format!("{name}.auth");
    let target_tls_on = resolved.server().is_some_and(|s| s.tls.is_some());
    let expected = expected_scheme(target_tls_on);
    let scheme_fix = (!scheme.eq_ignore_ascii_case(expected))
        .then(|| rewrite_scheme(base_url, expected))
        .flatten()
        .map(|value| crate::report::FixOp::SetString {
            service: "ui-htmx".to_string(),
            pointer: format!("/{name}/base_url"),
            value,
        });

    checks.extend(transport_check(
        ctx,
        "ui-htmx",
        &transport_field,
        &scheme,
        resolved,
        scheme_fix,
        Some((
            format!("/{name}/ca_cert_path"),
            target
                .ca_cert_path
                .as_deref()
                .is_some_and(|p| !p.is_empty()),
        )),
    ));
    checks.extend(credential_check(
        ctx,
        "ui-htmx",
        &auth_field,
        resolved,
        Some(format!("/{name}/auth")),
        target.auth.as_ref(),
    ));
    checks
}

/// rp's plate-solver/guider clients: `docs/services/doctor.md
/// §Client-target joins`. CA trust is `rp`'s single top-level `ca_cert`
/// field (issue #609 / PR #612), shared by both targets, so the transport
/// check is fully fix-eligible once that field or its provisioning
/// material exists. Neither target carries a per-target credential field
/// yet, so `joins.client-auth` still runs suggestion-only.
fn rp_client_joins(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(rp) = ctx.scan("rp").and_then(|s| scan::view::<RpView>(s)?.ok()) else {
        return checks;
    };
    let ca_cert_present = rp.ca_cert.as_deref().is_some_and(|p| !p.is_empty());
    if let Some(url) = rp.mount_guiding_url() {
        checks.extend(rp_one_target(
            ctx,
            "equipment.mount.guiding.url",
            &url,
            ca_cert_present,
        ));
    }
    if let Some(url) = rp.plate_solver.and_then(|p| p.url) {
        checks.extend(rp_one_target(
            ctx,
            "plate_solver.url",
            &url,
            ca_cert_present,
        ));
    }
    checks
}

fn rp_one_target(ctx: &Context, field: &str, url: &str, ca_cert_present: bool) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some((scheme, host, port)) = parse_target_url(url) else {
        return checks;
    };
    let Some(resolved) = resolve_join_target(ctx, &host, port) else {
        return checks;
    };

    let target_tls_on = resolved.server().is_some_and(|s| s.tls.is_some());
    let expected = expected_scheme(target_tls_on);
    let scheme_fix = (!scheme.eq_ignore_ascii_case(expected))
        .then(|| rewrite_scheme(url, expected))
        .flatten()
        .map(|value| crate::report::FixOp::SetString {
            service: "rp".to_string(),
            pointer: format!("/{}", field.replace('.', "/")),
            value,
        });

    checks.extend(transport_check(
        ctx,
        "rp",
        field,
        &scheme,
        resolved,
        scheme_fix,
        Some(("/ca_cert".to_string(), ca_cert_present)),
    ));
    checks.extend(credential_check(ctx, "rp", field, resolved, None, None));
    checks
}

/// sentinel's other client targets: the operation watchdog's `rp_url`
/// (scheme only — its credential is the shared `service_auth` pair,
/// already covered by `auth.mismatch`) and each Alpaca monitor (scheme
/// plus its own `auth`, which `auth.mismatch` does not see).
fn sentinel_client_joins(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(sentinel) = ctx
        .scan("sentinel")
        .and_then(|s| scan::view::<SentinelView>(s)?.ok())
    else {
        return checks;
    };
    if let Some(rp_url) = sentinel
        .operation_watchdog
        .as_ref()
        .and_then(|w| w.rp_url.as_deref())
    {
        checks.extend(sentinel_watchdog_target(ctx, rp_url));
    }
    for (idx, monitor) in sentinel.monitors.iter().enumerate() {
        checks.extend(sentinel_monitor_target(ctx, idx, monitor));
    }
    checks
}

fn sentinel_watchdog_target(ctx: &Context, rp_url: &str) -> Vec<Check> {
    let Some((scheme, host, port)) = parse_target_url(rp_url) else {
        return Vec::new();
    };
    let Some(resolved) = resolve_join_target(ctx, &host, port) else {
        return Vec::new();
    };
    let target_tls_on = resolved.server().is_some_and(|s| s.tls.is_some());
    let expected = expected_scheme(target_tls_on);
    let scheme_fix = (!scheme.eq_ignore_ascii_case(expected))
        .then(|| rewrite_scheme(rp_url, expected))
        .flatten()
        .map(|value| crate::report::FixOp::SetString {
            service: "sentinel".to_string(),
            pointer: "/operation_watchdog/rp_url".to_string(),
            value,
        });
    transport_check(
        ctx,
        "sentinel",
        "operation_watchdog.rp_url",
        &scheme,
        resolved,
        scheme_fix,
        None,
    )
    .into_iter()
    .collect()
}

fn sentinel_monitor_target(ctx: &Context, idx: usize, monitor: &MonitorView) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(resolved) = resolve_join_target(ctx, &monitor.host, monitor.port) else {
        return checks;
    };
    let transport_field = format!("monitors[{idx}].scheme");
    let auth_field = format!("monitors[{idx}].auth");
    let target_tls_on = resolved.server().is_some_and(|s| s.tls.is_some());
    let expected = expected_scheme(target_tls_on);
    let scheme_fix =
        (!monitor.scheme.eq_ignore_ascii_case(expected)).then(|| crate::report::FixOp::SetString {
            service: "sentinel".to_string(),
            pointer: format!("/monitors/{idx}/scheme"),
            value: expected.to_string(),
        });
    // No per-monitor ca_cert_path: monitors trust sentinel's single
    // top-level `ca_cert`, which the existing client-wiring pass already
    // provisions unconditionally once the CA exists.
    checks.extend(transport_check(
        ctx,
        "sentinel",
        &transport_field,
        &monitor.scheme,
        resolved,
        scheme_fix,
        None,
    ));
    checks.extend(credential_check(
        ctx,
        "sentinel",
        &auth_field,
        resolved,
        Some(format!("/monitors/{idx}/auth")),
        monitor.auth.as_ref(),
    ));
    checks
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
    fn test_tls_absent_fix_points_at_the_wildcard_pair_on_an_acme_install() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ppba-driver.json"),
            r#"{ "server": { "port": 11112 } }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("acme.json"), "{}").unwrap();
        let pki = dir.path().join("pki");
        std::fs::create_dir_all(&pki).unwrap();
        std::fs::write(pki.join("acme-cert.pem"), "cert").unwrap();
        std::fs::write(pki.join("acme-key.pem"), "key").unwrap();
        let ctx = config_only_ctx(dir.path());
        let scan = ctx.scan("ppba-driver").unwrap();
        let checks = tls_auth_absent(&ctx, scan);
        let tls = checks.iter().find(|c| c.name == "tls.absent").unwrap();
        match &tls.fixes[..] {
            [crate::report::FixOp::SetObject { pointer, value, .. }] => {
                assert_eq!(pointer, "/server/tls");
                let cert = value["cert"].as_str().unwrap();
                let key = value["key"].as_str().unwrap();
                assert!(cert.ends_with("acme-cert.pem"), "{cert}");
                assert!(key.ends_with("acme-key.pem"), "{key}");
                assert!(
                    !cert.contains("ppba-driver"),
                    "no per-service self-signed pair on an ACME install: {cert}"
                );
            }
            other => unreachable!("expected one SetObject fix, got {other:?}"),
        }
    }

    #[test]
    fn test_tls_absent_plans_no_fix_while_the_acme_pair_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ppba-driver.json"),
            r#"{ "server": { "port": 11112 } }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("acme.json"), "{}").unwrap();
        let ctx = config_only_ctx(dir.path());
        let scan = ctx.scan("ppba-driver").unwrap();
        let checks = tls_auth_absent(&ctx, scan);
        let tls = checks.iter().find(|c| c.name == "tls.absent").unwrap();
        assert!(
            tls.fixes.is_empty(),
            "a missing wildcard pair is renewal's to recover: {:?}",
            tls.fixes
        );
        let suggestion = tls.suggestion.as_deref().unwrap();
        assert!(suggestion.contains("doctor tls renew"), "{suggestion}");
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

    // ---- Client-target joins ----

    fn write_json(dir: &Path, name: &str, value: serde_json::Value) {
        std::fs::write(dir.join(name), value.to_string()).unwrap();
    }

    /// A minted observatory credential on disk — what `--fix`'s
    /// provisioning pass leaves behind before these checks ever run.
    fn stage_pki(dir: &Path, password: &str) {
        std::fs::create_dir_all(dir.join("pki")).unwrap();
        std::fs::write(dir.join("pki/ca.pem"), "stub-ca-pem").unwrap();
        std::fs::write(dir.join("pki/credential"), format!("{password}\n")).unwrap();
    }

    #[test]
    fn test_is_loopback_host() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("::1"));
        assert!(!is_loopback_host("10.0.0.5"));
        assert!(!is_loopback_host("rig.local"));
    }

    #[test]
    fn test_parse_target_url_requires_an_explicit_port() {
        assert_eq!(
            parse_target_url("https://host:11115/x"),
            Some(("https".to_string(), "host".to_string(), 11115))
        );
        assert!(parse_target_url("https://host/x").is_none());
        assert!(parse_target_url("not a url").is_none());
    }

    #[test]
    fn test_rewrite_scheme_preserves_the_url_verbatim_without_adding_a_trailing_slash() {
        // A parse-and-reserialize round trip would normalize
        // "http://host:port" into "http://host:port/" — several client
        // call sites concatenate a "/"-prefixed path onto the base URL
        // without trimming, so a stray trailing slash would 404 them.
        assert_eq!(
            rewrite_scheme("http://127.0.0.1:11115", "https").unwrap(),
            "https://127.0.0.1:11115"
        );
        assert_eq!(
            rewrite_scheme("https://host:11114/dash?x=1", "http").unwrap(),
            "http://host:11114/dash?x=1"
        );
        assert!(rewrite_scheme("not a url", "https").is_none());
        // `Url::scheme()` lowercases; stripping it off the raw string
        // would silently fail to match an uppercase input scheme.
        assert_eq!(
            rewrite_scheme("HTTP://127.0.0.1:11115", "https").unwrap(),
            "https://127.0.0.1:11115"
        );
    }

    #[test]
    fn test_ui_htmx_rp_scheme_mismatch_is_flagged_and_fixed() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115,
                "tls": { "cert": "/pki/acme-cert.pem", "key": "/pki/acme-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "http://127.0.0.1:11115" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = ui_htmx_target_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("a transport mismatch must be reported");
        assert_eq!(transport.status, Status::Fail);
        assert!(
            transport.detail.contains("uses http"),
            "{}",
            transport.detail
        );
        match &transport.fixes[..] {
            [crate::report::FixOp::SetString {
                service,
                pointer,
                value,
            }] => {
                assert_eq!(service, "ui-htmx");
                assert_eq!(pointer, "/rp/base_url");
                assert_eq!(value, "https://127.0.0.1:11115");
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn test_an_unsupported_scheme_against_a_plain_http_target_is_flagged_and_fixed() {
        let dir = tempfile::tempdir().unwrap();
        // "ftp" is neither "http" nor "https" — a naive `!= "https"`
        // comparison would treat it as equivalent to "http" and silently
        // accept it against this plain-HTTP (no tls block) target.
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115 } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "ftp://127.0.0.1:11115" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = ui_htmx_target_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("an unsupported scheme must be reported even against a plain-HTTP target");
        assert_eq!(transport.status, Status::Fail);
        match &transport.fixes[..] {
            [crate::report::FixOp::SetString {
                service,
                pointer,
                value,
            }] => {
                assert_eq!(service, "ui-htmx");
                assert_eq!(pointer, "/rp/base_url");
                assert_eq!(value, "http://127.0.0.1:11115");
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn test_ui_htmx_rp_flags_missing_ca_trust_for_a_self_signed_target() {
        let dir = tempfile::tempdir().unwrap();
        stage_pki(dir.path(), "s3cret-pw");
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115,
                "tls": { "cert": "/pki/rp.pem", "key": "/pki/rp-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "https://127.0.0.1:11115" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = ui_htmx_target_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("missing CA trust must be reported");
        assert_eq!(transport.status, Status::Fail);
        assert!(
            transport.detail.contains("self-signed"),
            "{}",
            transport.detail
        );
        match &transport.fixes[..] {
            [crate::report::FixOp::SetString {
                service,
                pointer,
                value,
            }] => {
                assert_eq!(service, "ui-htmx");
                assert_eq!(pointer, "/rp/ca_cert_path");
                assert!(
                    std::path::Path::new(value).ends_with("pki/ca.pem"),
                    "{value}"
                );
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn test_ui_htmx_empty_ca_cert_path_is_treated_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        stage_pki(dir.path(), "s3cret-pw");
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115,
                "tls": { "cert": "/pki/rp.pem", "key": "/pki/rp-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "https://127.0.0.1:11115", "ca_cert_path": "" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = ui_htmx_target_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("an empty ca_cert_path must not be mistaken for a working one");
        assert_eq!(transport.status, Status::Fail);
        match &transport.fixes[..] {
            [crate::report::FixOp::SetString { pointer, .. }] => {
                assert_eq!(pointer, "/rp/ca_cert_path");
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn test_ui_htmx_rp_acme_target_needs_no_ca_cert_path() {
        let dir = tempfile::tempdir().unwrap();
        stage_pki(dir.path(), "s3cret-pw");
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115,
                "tls": { "cert": "/pki/acme-cert.pem", "key": "/pki/acme-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "https://127.0.0.1:11115" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = ui_htmx_target_joins(&ctx);
        assert!(
            checks.iter().all(|c| c.name != "joins.client-transport"),
            "a publicly-trusted ACME cert needs no client-side CA: {checks:?}"
        );
    }

    #[test]
    fn test_ui_htmx_rp_auth_absent_is_flagged_and_fixed() {
        let dir = tempfile::tempdir().unwrap();
        stage_pki(dir.path(), "s3cret-pw");
        let hash = rp_auth::credentials::hash_password("s3cret-pw").unwrap();
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115,
                "auth": { "username": "observatory", "password_hash": hash } } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "http://127.0.0.1:11115" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = ui_htmx_target_joins(&ctx);
        let auth = checks
            .iter()
            .find(|c| c.name == "joins.client-auth")
            .expect("a missing credential must be reported");
        assert_eq!(auth.status, Status::Warn);
        match &auth.fixes[..] {
            [crate::report::FixOp::SetObject {
                service,
                pointer,
                value,
            }] => {
                assert_eq!(service, "ui-htmx");
                assert_eq!(pointer, "/rp/auth");
                assert_eq!(value["username"], "observatory");
                assert_eq!(value["password"], "s3cret-pw");
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn test_ui_htmx_rp_auth_mismatch_is_suggestion_only() {
        let dir = tempfile::tempdir().unwrap();
        let hash = rp_auth::credentials::hash_password("correct-pw").unwrap();
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115,
                "auth": { "username": "observatory", "password_hash": hash } } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "http://127.0.0.1:11115",
                        "auth": { "username": "observatory", "password": "wrong-pw" } } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = ui_htmx_target_joins(&ctx);
        let auth = checks
            .iter()
            .find(|c| c.name == "joins.client-auth")
            .expect("a wrong credential must be reported");
        assert_eq!(auth.status, Status::Warn);
        assert!(
            auth.fixes.is_empty(),
            "a present credential is operator intent, never clobbered"
        );
    }

    #[test]
    fn test_ui_htmx_rp_matching_credential_and_scheme_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let hash = rp_auth::credentials::hash_password("s3cret-pw").unwrap();
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115,
                "tls": { "cert": "/pki/acme-cert.pem", "key": "/pki/acme-key.pem" },
                "auth": { "username": "observatory", "password_hash": hash } } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "https://127.0.0.1:11115",
                        "auth": { "username": "observatory", "password": "s3cret-pw" } } }),
        );
        let ctx = config_only_ctx(dir.path());
        assert!(ui_htmx_target_joins(&ctx).is_empty());
    }

    #[test]
    fn test_ui_htmx_sentinel_target_absent_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            dir.path(),
            "sentinel.json",
            serde_json::json!({ "server": { "port": 11114,
                "tls": { "cert": "/pki/sentinel.pem", "key": "/pki/sentinel-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "http://127.0.0.1:11115" } }),
        );
        let ctx = config_only_ctx(dir.path());
        // rp itself does not participate (no config, no unit), and
        // ui-htmx's optional sentinel block is absent — nothing to join.
        assert!(ui_htmx_target_joins(&ctx).is_empty());
    }

    #[test]
    fn test_non_loopback_host_is_never_joined() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115,
                "tls": { "cert": "/pki/rp.pem", "key": "/pki/rp-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "http://10.0.0.5:11115" } }),
        );
        let ctx = config_only_ctx(dir.path());
        assert!(ui_htmx_target_joins(&ctx).is_empty());
    }

    #[test]
    fn test_a_target_with_no_server_block_still_joins_on_its_catalog_default() {
        let dir = tempfile::tempdir().unwrap();
        // rp.json has no "server" key at all — it applies its documented
        // plain-HTTP, no-auth, catalog-default-port (11115) behavior. That
        // is a known state, not guesswork, so the join must still resolve.
        write_json(dir.path(), "rp.json", serde_json::json!({}));
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "https://127.0.0.1:11115" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = ui_htmx_target_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("a plain-HTTP-by-default target against an https client must be reported");
        assert_eq!(transport.status, Status::Fail);
        assert!(
            transport.detail.contains("uses https"),
            "{}",
            transport.detail
        );
    }

    #[test]
    fn test_an_ambiguous_port_collision_is_never_joined() {
        let dir = tempfile::tempdir().unwrap();
        // rp and sentinel both claim port 11115 — ports.collision reports
        // that on its own; the join must not guess which one ui-htmx meant.
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115,
                "tls": { "cert": "/pki/rp.pem", "key": "/pki/rp-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "sentinel.json",
            serde_json::json!({ "server": { "port": 11115 } }),
        );
        write_json(
            dir.path(),
            "ui-htmx.json",
            serde_json::json!({ "server": { "port": 11120 },
                "rp": { "base_url": "http://127.0.0.1:11115" } }),
        );
        let ctx = config_only_ctx(dir.path());
        assert!(ui_htmx_target_joins(&ctx).is_empty());
    }

    #[test]
    fn test_rp_plate_solver_scheme_mismatch_is_flagged_and_fixed() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            dir.path(),
            "plate-solver.json",
            serde_json::json!({ "server": { "port": 11131,
                "tls": { "cert": "/pki/acme-cert.pem", "key": "/pki/acme-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115 },
                "plate_solver": { "url": "http://localhost:11131" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = rp_client_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("a scheme mismatch must be reported");
        assert_eq!(transport.status, Status::Fail);
        match &transport.fixes[..] {
            [crate::report::FixOp::SetString {
                service,
                pointer,
                value,
            }] => {
                assert_eq!(service, "rp");
                assert_eq!(pointer, "/plate_solver/url");
                assert_eq!(value, "https://localhost:11131");
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn test_rp_plate_solver_flags_missing_ca_trust_for_a_self_signed_target() {
        let dir = tempfile::tempdir().unwrap();
        stage_pki(dir.path(), "s3cret-pw");
        write_json(
            dir.path(),
            "plate-solver.json",
            serde_json::json!({ "server": { "port": 11131,
                "tls": { "cert": "/pki/plate-solver.pem", "key": "/pki/plate-solver-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115 },
                "plate_solver": { "url": "https://localhost:11131" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = rp_client_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("missing CA trust must be reported");
        assert_eq!(transport.status, Status::Fail);
        assert!(
            transport.detail.contains("self-signed"),
            "{}",
            transport.detail
        );
        match &transport.fixes[..] {
            [crate::report::FixOp::SetString {
                service,
                pointer,
                value,
            }] => {
                assert_eq!(service, "rp");
                assert_eq!(pointer, "/ca_cert");
                assert!(
                    std::path::Path::new(value).ends_with("pki/ca.pem"),
                    "{value}"
                );
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn test_rp_plate_solver_reports_missing_ca_trust_even_without_local_ca_pem() {
        let dir = tempfile::tempdir().unwrap();
        // No stage_pki: doctor's own pki/ca.pem does not exist on this
        // config dir, so the gap can only be reported, never fixed.
        write_json(
            dir.path(),
            "plate-solver.json",
            serde_json::json!({ "server": { "port": 11131,
                "tls": { "cert": "/pki/plate-solver.pem", "key": "/pki/plate-solver-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115 },
                "plate_solver": { "url": "https://localhost:11131" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = rp_client_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("missing CA trust must be reported even when ca.pem is absent");
        assert_eq!(transport.status, Status::Fail);
        assert!(
            transport.detail.contains("self-signed") && transport.detail.contains("ca_cert"),
            "{}",
            transport.detail
        );
        assert!(
            transport.fixes.is_empty(),
            "no fix is possible without doctor's own CA material: {:?}",
            transport.fixes
        );
    }

    #[test]
    fn test_rp_ca_cert_already_present_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        stage_pki(dir.path(), "s3cret-pw");
        write_json(
            dir.path(),
            "plate-solver.json",
            serde_json::json!({ "server": { "port": 11131,
                "tls": { "cert": "/pki/plate-solver.pem", "key": "/pki/plate-solver-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115 }, "ca_cert": "/pki/ca.pem",
                "plate_solver": { "url": "https://localhost:11131" } }),
        );
        let ctx = config_only_ctx(dir.path());
        assert!(rp_client_joins(&ctx).is_empty());
    }

    #[test]
    fn test_rp_empty_ca_cert_is_treated_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        stage_pki(dir.path(), "s3cret-pw");
        write_json(
            dir.path(),
            "plate-solver.json",
            serde_json::json!({ "server": { "port": 11131,
                "tls": { "cert": "/pki/plate-solver.pem", "key": "/pki/plate-solver-key.pem" } } }),
        );
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115 }, "ca_cert": "",
                "plate_solver": { "url": "https://localhost:11131" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = rp_client_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("an empty ca_cert must not be mistaken for a working one");
        assert_eq!(transport.status, Status::Fail);
        match &transport.fixes[..] {
            [crate::report::FixOp::SetString { pointer, .. }] => {
                assert_eq!(pointer, "/ca_cert");
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn test_rp_guider_auth_gap_is_suggestion_only() {
        let dir = tempfile::tempdir().unwrap();
        let hash = rp_auth::credentials::hash_password("s3cret-pw").unwrap();
        write_json(
            dir.path(),
            "phd2-guider.json",
            serde_json::json!({ "server": { "port": 11130,
                "auth": { "username": "observatory", "password_hash": hash } } }),
        );
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115 },
                "equipment": { "mount": { "alpaca_url": "http://localhost:11117",
                                           "guiding": { "url": "http://localhost:11130" } } } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = rp_client_joins(&ctx);
        let auth = checks
            .iter()
            .find(|c| c.name == "joins.client-auth")
            .expect("a missing credential field must still be reported");
        assert_eq!(auth.status, Status::Warn);
        assert!(auth.fixes.is_empty());
        assert!(
            auth.detail.contains("equipment.mount.guiding.url"),
            "{}",
            auth.detail
        );
    }

    #[test]
    fn test_sentinel_monitor_scheme_and_auth_are_flagged_and_fixed() {
        let dir = tempfile::tempdir().unwrap();
        stage_pki(dir.path(), "s3cret-pw");
        let hash = rp_auth::credentials::hash_password("s3cret-pw").unwrap();
        write_json(
            dir.path(),
            "ppba-driver.json",
            serde_json::json!({ "server": { "port": 11112,
                "tls": { "cert": "/pki/acme-cert.pem", "key": "/pki/acme-key.pem" },
                "auth": { "username": "observatory", "password_hash": hash } } }),
        );
        write_json(
            dir.path(),
            "sentinel.json",
            serde_json::json!({ "server": { "port": 11114 },
                "monitors": [ { "type": "alpaca_safety_monitor", "name": "PPBA",
                                 "host": "localhost", "port": 11112, "scheme": "http" } ] }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = sentinel_client_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("a monitor scheme mismatch must be reported");
        assert_eq!(transport.status, Status::Fail);
        match &transport.fixes[..] {
            [crate::report::FixOp::SetString {
                service,
                pointer,
                value,
            }] => {
                assert_eq!(service, "sentinel");
                assert_eq!(pointer, "/monitors/0/scheme");
                assert_eq!(value, "https");
            }
            other => unreachable!("{other:?}"),
        }
        let auth = checks
            .iter()
            .find(|c| c.name == "joins.client-auth")
            .expect("a missing per-monitor credential must be reported");
        match &auth.fixes[..] {
            [crate::report::FixOp::SetObject {
                service,
                pointer,
                value,
            }] => {
                assert_eq!(service, "sentinel");
                assert_eq!(pointer, "/monitors/0/auth");
                assert_eq!(value["username"], "observatory");
                assert_eq!(value["password"], "s3cret-pw");
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn test_sentinel_watchdog_rp_url_scheme_is_flagged_without_a_duplicate_auth_check() {
        let dir = tempfile::tempdir().unwrap();
        let hash = rp_auth::credentials::hash_password("x").unwrap();
        write_json(
            dir.path(),
            "rp.json",
            serde_json::json!({ "server": { "port": 11115,
                "tls": { "cert": "/pki/acme-cert.pem", "key": "/pki/acme-key.pem" },
                "auth": { "username": "observatory", "password_hash": hash } } }),
        );
        write_json(
            dir.path(),
            "sentinel.json",
            serde_json::json!({ "server": { "port": 11114 },
                "operation_watchdog": { "rp_url": "http://localhost:11115" } }),
        );
        let ctx = config_only_ctx(dir.path());
        let checks = sentinel_client_joins(&ctx);
        let transport = checks
            .iter()
            .find(|c| c.name == "joins.client-transport")
            .expect("the watchdog's scheme mismatch must be reported");
        match &transport.fixes[..] {
            [crate::report::FixOp::SetString {
                service,
                pointer,
                value,
            }] => {
                assert_eq!(service, "sentinel");
                assert_eq!(pointer, "/operation_watchdog/rp_url");
                assert_eq!(value, "https://localhost:11115");
            }
            other => unreachable!("{other:?}"),
        }
        // rp's auth requirement is `auth.mismatch`'s job (sentinel's
        // shared `service_auth`), not this join's.
        assert!(
            checks.iter().all(|c| c.name != "joins.client-auth"),
            "{checks:?}"
        );
    }
}
