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
/// them; a shape error there is its own diagnosis.
fn known_blocks(scan: &ServiceScan) -> Vec<Check> {
    let result = match scan.entry.name {
        "ui-htmx" => scan::view::<UiHtmxView>(scan).map(|r| r.map(|_| ())),
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
    if let Some(ui) = &ui_view {
        if ui.sentinel.is_some() {
            checks.extend(ui_restart_joins(ctx, ui));
        }
        checks.extend(ui_driver_port_joins(ctx, ui));
    }
    checks
}

/// Config keys retired by D3s (sentinel discovers its services): sentinel's
/// `services` map and ui-htmx's per-driver `sentinel_service`. Both fail the
/// service's own strict load, so the file is dead weight that keeps the
/// service from starting.
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
    if let Some(ui) = ui {
        for (driver_id, driver) in &ui.drivers {
            if driver.sentinel_service.is_some() {
                checks.push(
                    Check::fail(
                        "config.retired-keys",
                        Some("ui-htmx".to_string()),
                        format!(
                            "drivers.{driver_id} carries the retired sentinel_service \
                             field — the restart name is always the driver's own map \
                             key now, and ui-htmx refuses to start while the field is \
                             present"
                        ),
                        Some(format!("delete drivers.{driver_id}.sentinel_service")),
                    )
                    .with_fixes(vec![crate::report::FixOp::RemoveKey {
                        service: "ui-htmx".to_string(),
                        pointer: format!(
                            "/drivers/{}/sentinel_service",
                            crate::fix::escape_token(driver_id)
                        ),
                    }]),
                );
            }
        }
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

/// ui-htmx renders its Restart-via-Sentinel affordance for every
/// config-declared driver whenever a `sentinel` target is set, naming the
/// driver's own map key. A key with no matching installed unit 404s at
/// sentinel — legal for a third-party device (hence warn, not fail), but
/// worth knowing before 2am.
fn ui_restart_joins(ctx: &Context, ui: &UiHtmxView) -> Vec<Check> {
    let mut checks = Vec::new();
    if ctx.mode != Mode::Packaged {
        return checks;
    }
    let discovered = discovered_service_names(ctx);
    let mut all_resolve = true;
    for driver_id in ui.drivers.keys() {
        if !discovered.iter().any(|name| name == driver_id) {
            all_resolve = false;
            checks.push(Check::warn(
                "joins.ui-htmx-restart",
                Some("ui-htmx".to_string()),
                format!(
                    "drivers.{driver_id} has no installed rusty-photon-{driver_id} \
                     unit, so its Restart-via-Sentinel button will 404 — fine if \
                     this is a third-party device sentinel cannot restart"
                ),
                None,
            ));
        }
    }
    if all_resolve && !ui.drivers.is_empty() {
        checks.push(Check::ok(
            "joins.ui-htmx-restart",
            Some("ui-htmx".to_string()),
            "every driver key resolves to an installed unit sentinel can restart".to_string(),
        ));
    }
    checks
}

fn ui_driver_port_joins(ctx: &Context, ui: &UiHtmxView) -> Vec<Check> {
    let mut checks = Vec::new();
    for (driver_id, driver) in &ui.drivers {
        let Some(entry) = catalog::entry(driver_id) else {
            continue;
        };
        let Some(url) = &driver.base_url else {
            continue;
        };
        let Some(port) = localhost_port(url) else {
            continue;
        };
        let Some(scan) = ctx.scan(entry.name) else {
            continue;
        };
        let expected = scan.effective_port();
        if port != expected {
            let fixes = match replace_port(url, expected) {
                Some(corrected) => vec![crate::report::FixOp::SetString {
                    service: "ui-htmx".to_string(),
                    pointer: format!("/drivers/{}/base_url", crate::fix::escape_token(driver_id)),
                    value: corrected,
                }],
                None => Vec::new(),
            };
            checks.push(
                Check::fail(
                    "joins.ui-htmx-driver-port",
                    Some("ui-htmx".to_string()),
                    format!(
                        "drivers.{driver_id}.base_url points at localhost port {port}, \
                         but {driver_id} listens on {expected} — every config page for \
                         it will show a transport banner"
                    ),
                    Some(format!("change the URL's port to {expected}")),
                )
                .with_fixes(fixes),
            );
        }
    }
    checks
}

/// The port of a URL that targets this host, `None` for remote hosts
/// (doctor sees one machine) or unparseable URLs.
fn localhost_port(url: &str) -> Option<u16> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split('/').next()?;
    let (host, port) = authority.rsplit_once(':')?;
    if !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return None;
    }
    port.parse().ok()
}

/// `url` with its authority's port replaced — only for URLs
/// [`localhost_port`] already parsed, so the split is known to succeed.
fn replace_port(url: &str, port: u16) -> Option<String> {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, url),
    };
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (rest, None),
    };
    let (host, _) = authority.rsplit_once(':')?;
    let mut out = String::new();
    if let Some(scheme) = scheme {
        out.push_str(scheme);
        out.push_str("://");
    }
    out.push_str(host);
    out.push(':');
    out.push_str(&port.to_string());
    if let Some(path) = path {
        out.push('/');
        out.push_str(path);
    }
    Some(out)
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
    if let Some(ui) = ctx
        .scan("ui-htmx")
        .and_then(|s| scan::view::<UiHtmxView>(s)?.ok())
    {
        for (driver_id, driver) in &ui.drivers {
            if let Some(url) = &driver.base_url {
                if carries_suffix(url) {
                    checks.push(
                        spurious("ui-htmx", format!("drivers.{driver_id}.base_url"), url)
                            .with_fixes(vec![crate::report::FixOp::SetString {
                                service: "ui-htmx".to_string(),
                                pointer: format!(
                                    "/drivers/{}/base_url",
                                    crate::fix::escape_token(driver_id)
                                ),
                                value: stripped(url),
                            }]),
                    );
                }
            }
        }
    }
    checks
}

// ---- TLS and auth ----

fn tls_and_auth(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    for scan in ctx.scans.iter().filter(|s| ctx.participates(s)) {
        let Some(server) = scan.server() else {
            continue;
        };
        if let Some(tls) = &server.tls {
            let mut missing: Vec<String> = Vec::new();
            for (raw, resolved) in [
                (&tls.cert, tls.resolved_cert_path()),
                (&tls.key, tls.resolved_key_path()),
            ] {
                if !tls_material_present(&ctx.config_dir, raw, &resolved) {
                    missing.push(if raw.trim().is_empty() {
                        "<empty path>".to_string()
                    } else {
                        raw.clone()
                    });
                }
            }
            if missing.is_empty() {
                checks.push(Check::ok(
                    "tls.paths",
                    svc(scan),
                    "TLS cert and key exist".to_string(),
                ));
            } else {
                checks.push(Check::fail(
                    "tls.paths",
                    svc(scan),
                    format!(
                        "server.tls points at missing material: {} — the service \
                         will refuse to serve at next start",
                        missing.join(", ")
                    ),
                    Some(
                        "generate certs (rp init-tls today; doctor owns this from D6) \
                         or fix the paths"
                            .to_string(),
                    ),
                ));
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
    checks
}

/// Whether one piece of TLS material is present as a real file. `resolved`
/// is the path the service itself will open (`TlsConfig::resolved_*_path`,
/// which expands `~`); a relative remainder is anchored at the config dir.
/// Empty paths and directories are absent — the service would fail to read
/// either.
fn tls_material_present(config_dir: &Path, raw: &str, resolved: &Path) -> bool {
    if raw.trim().is_empty() {
        return false;
    }
    let anchored = if resolved.is_absolute() {
        resolved.to_path_buf()
    } else {
        config_dir.join(resolved)
    };
    anchored.is_file()
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

    #[test]
    fn test_tls_material_present_matches_service_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let pem = dir.path().join("cert.pem");
        std::fs::write(&pem, "stub").unwrap();

        // Absolute existing file: present; empty and whitespace paths: absent.
        assert!(tls_material_present(
            dir.path(),
            pem.to_str().unwrap(),
            &pem
        ));
        assert!(!tls_material_present(dir.path(), "", Path::new("")));
        assert!(!tls_material_present(dir.path(), "  ", Path::new("  ")));

        // A directory is not usable TLS material.
        assert!(!tls_material_present(
            dir.path(),
            dir.path().to_str().unwrap(),
            dir.path()
        ));

        // A relative resolved path anchors at the config dir.
        assert!(tls_material_present(
            dir.path(),
            "cert.pem",
            Path::new("cert.pem")
        ));
        assert!(!tls_material_present(
            dir.path(),
            "missing.pem",
            Path::new("missing.pem")
        ));
    }

    #[test]
    fn test_localhost_port_scopes_to_this_host() {
        assert_eq!(localhost_port("http://localhost:11113"), Some(11113));
        assert_eq!(localhost_port("https://127.0.0.1:11113/x"), Some(11113));
        assert_eq!(localhost_port("http://[::1]:11113"), Some(11113));
        assert_eq!(localhost_port("http://10.0.85.245:11113"), None);
        assert_eq!(localhost_port("http://localhost"), None);
        assert_eq!(localhost_port("not a url"), None);
    }

    #[test]
    fn test_replace_port_rebuilds_every_url_shape() {
        // Scheme + path: both preserved around the swapped port.
        assert_eq!(
            replace_port("http://localhost:11114/api/v1", 11113).as_deref(),
            Some("http://localhost:11113/api/v1")
        );
        // Scheme, no path.
        assert_eq!(
            replace_port("http://localhost:11114", 11113).as_deref(),
            Some("http://localhost:11113")
        );
        // No scheme (authority-only), with and without a path.
        assert_eq!(
            replace_port("localhost:11114/x", 11113).as_deref(),
            Some("localhost:11113/x")
        );
        assert_eq!(
            replace_port("localhost:11114", 11113).as_deref(),
            Some("localhost:11113")
        );
        // No port to replace.
        assert_eq!(replace_port("http://localhost", 11113), None);
    }
}
