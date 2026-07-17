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
}

impl Context {
    /// Scan the config dir and derive the mode from the unit inventory.
    pub fn gather(config_dir: PathBuf, facts: PlatformFacts) -> Self {
        let mode = if facts.units.is_empty() {
            Mode::ConfigOnly
        } else {
            Mode::Packaged
        };
        let scans = catalog::catalog()
            .iter()
            .map(|entry| scan::scan_service(&config_dir, entry))
            .collect();
        Self {
            config_dir,
            facts,
            mode,
            scans,
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
            (true, false) => checks.push(Check::warn(
                "inventory.unit-without-config",
                svc(scan),
                format!(
                    "unit {} is installed but {} does not exist — the service has \
                     never started, or writes its config somewhere unexpected",
                    scan.entry.unit_name(),
                    scan.config_path.display()
                ),
                Some(format!(
                    "start it once so it self-creates its defaults: e.g. `{}`",
                    start_command(ctx.facts.platform, &scan.entry.unit_name())
                )),
            )),
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
            checks.push(Check::fail(
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
            ));
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

    if let Some(sentinel) = &sentinel_view {
        checks.extend(sentinel_restart_joins(ctx, sentinel));
        checks.extend(watchdog_joins(sentinel));
    }
    if let Some(ui) = &ui_view {
        if ui.sentinel.is_some() {
            if let Some(sentinel) = &sentinel_view {
                checks.extend(ui_sentinel_joins(ui, sentinel));
            }
        }
        checks.extend(ui_driver_port_joins(ctx, ui));
    }
    checks
}

/// Extract the unit a `systemctl … restart <unit>` command names, plus
/// whether it uses the `--user` scope.
fn restart_command_unit(command: &str) -> Option<(String, bool)> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let user_scope = tokens.contains(&"--user");
    let restart_pos = tokens.iter().position(|t| *t == "restart")?;
    let unit = tokens.get(restart_pos + 1)?;
    Some((
        unit.strip_suffix(".service").unwrap_or(unit).to_string(),
        user_scope,
    ))
}

fn sentinel_restart_joins(ctx: &Context, sentinel: &SentinelView) -> Vec<Check> {
    let mut checks = Vec::new();
    if ctx.mode != Mode::Packaged {
        return checks;
    }
    let mut all_resolve = true;
    for (name, service) in &sentinel.services {
        let Some(command) = &service.restart_command else {
            continue;
        };
        let Some((unit, user_scope)) = restart_command_unit(command) else {
            continue;
        };
        if user_scope {
            all_resolve = false;
            checks.push(Check::fail(
                "joins.sentinel-unit",
                Some("sentinel".to_string()),
                format!(
                    "services.{name}.restart_command uses `systemctl --user`, but \
                     rusty-photon units are system units — the restart will never \
                     find the unit"
                ),
                Some(format!("drop --user: `systemctl restart {unit}`")),
            ));
            continue;
        }
        if ctx.facts.unit(&unit).is_some() {
            continue;
        }
        all_resolve = false;
        let suggestion = if !unit.starts_with("rusty-photon-")
            && ctx.facts.unit(&format!("rusty-photon-{unit}")).is_some()
        {
            Some(format!(
                "the installed unit is rusty-photon-{unit} — restart commands need \
                 the rusty-photon- prefix"
            ))
        } else {
            Some(format!(
                "installed units are: {}",
                ctx.facts
                    .units
                    .iter()
                    .map(|u| u.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        };
        checks.push(Check::fail(
            "joins.sentinel-unit",
            Some("sentinel".to_string()),
            format!(
                "services.{name}.restart_command names unit {unit}, which the \
                 service manager does not report"
            ),
            suggestion,
        ));
    }
    if all_resolve
        && sentinel
            .services
            .values()
            .any(|s| s.restart_command.is_some())
    {
        checks.push(Check::ok(
            "joins.sentinel-unit",
            Some("sentinel".to_string()),
            "every restart_command names an installed unit".to_string(),
        ));
    }
    checks
}

fn watchdog_joins(sentinel: &SentinelView) -> Vec<Check> {
    let mut checks = Vec::new();
    let Some(watchdog) = &sentinel.operation_watchdog else {
        return checks;
    };
    for (family, operation) in &watchdog.operations {
        let Some(service) = &operation.service else {
            continue;
        };
        if !sentinel.services.contains_key(service) {
            checks.push(Check::fail(
                "joins.watchdog-service",
                Some("sentinel".to_string()),
                format!(
                    "operation_watchdog.operations.{family}.service names \
                     \"{service}\", which is not a key of the services map — the \
                     watchdog's restart rung will never fire"
                ),
                Some(format!(
                    "existing keys: {}",
                    sentinel
                        .services
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            ));
        }
    }
    checks
}

fn ui_sentinel_joins(ui: &UiHtmxView, sentinel: &SentinelView) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut all_resolve = true;
    for (driver_id, driver) in &ui.drivers {
        let target = driver.sentinel_service.as_deref().unwrap_or(driver_id);
        if !sentinel.services.contains_key(target) {
            all_resolve = false;
            checks.push(Check::fail(
                "joins.ui-htmx-sentinel",
                Some("ui-htmx".to_string()),
                format!(
                    "drivers.{driver_id} restarts via sentinel service \"{target}\", \
                     which is not a key of sentinel's services map — the restart \
                     button will 404"
                ),
                Some(format!(
                    "sentinel's keys: {}",
                    sentinel
                        .services
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            ));
        }
    }
    if all_resolve && !ui.drivers.is_empty() {
        checks.push(Check::ok(
            "joins.ui-htmx-sentinel",
            Some("ui-htmx".to_string()),
            "every driver's sentinel_service resolves to a sentinel services key".to_string(),
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
            checks.push(Check::fail(
                "joins.ui-htmx-driver-port",
                Some("ui-htmx".to_string()),
                format!(
                    "drivers.{driver_id}.base_url points at localhost port {port}, \
                     but {driver_id} listens on {expected} — every config page for \
                     it will show a transport banner"
                ),
                Some(format!("change the URL's port to {expected}")),
            ));
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

// ---- URL conventions ----

fn url_conventions(ctx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    if let Some(sentinel) = ctx
        .scan("sentinel")
        .and_then(|s| scan::view::<SentinelView>(s)?.ok())
    {
        for (name, service) in &sentinel.services {
            let Some(url) = &service.base_url else {
                continue;
            };
            if !url.trim_end_matches('/').ends_with("/api/v1") {
                checks.push(Check::warn(
                    "urls.sentinel-suffix",
                    Some("sentinel".to_string()),
                    format!(
                        "services.{name}.base_url ({url}) lacks the /api/v1 suffix \
                         sentinel's Alpaca probes append method paths below"
                    ),
                    Some(format!("use {}/api/v1", url.trim_end_matches('/'))),
                ));
            }
        }
    }
    let mut spurious = |service: &str, field: String, url: &str| {
        if url.trim_end_matches('/').ends_with("/api/v1") {
            checks.push(Check::warn(
                "urls.spurious-suffix",
                Some(service.to_string()),
                format!(
                    "{field} ({url}) carries /api/v1, but this client appends it \
                     itself — requests would double the prefix and 404"
                ),
                Some(format!(
                    "use {}",
                    url.trim_end_matches('/').trim_end_matches("/api/v1")
                )),
            ));
        }
    };
    if let Some(rp) = ctx.scan("rp").and_then(|s| scan::view::<RpView>(s)?.ok()) {
        for url in rp.alpaca_urls() {
            spurious("rp", "an equipment alpaca_url".to_string(), &url);
        }
    }
    if let Some(ui) = ctx
        .scan("ui-htmx")
        .and_then(|s| scan::view::<UiHtmxView>(s)?.ok())
    {
        for (driver_id, driver) in &ui.drivers {
            if let Some(url) = &driver.base_url {
                spurious("ui-htmx", format!("drivers.{driver_id}.base_url"), url);
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
            for path in [&tls.cert, &tls.key] {
                let resolved = resolve_config_relative(&ctx.config_dir, Path::new(path));
                if !resolved.exists() {
                    missing.push(path.clone());
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

/// Resolve a possibly-relative config path against the config dir.
fn resolve_config_relative(config_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_dir.join(path)
    }
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
        checks.push(Check::ok(
            "rp.data-directory",
            Some("rp".to_string()),
            format!("session.data_directory {dir} exists"),
        ));
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
    fn test_restart_command_unit_extraction() {
        assert_eq!(
            restart_command_unit("systemctl restart rusty-photon-qhy-focuser"),
            Some(("rusty-photon-qhy-focuser".to_string(), false))
        );
        assert_eq!(
            restart_command_unit("systemctl --user restart qhy-focuser.service"),
            Some(("qhy-focuser".to_string(), true))
        );
        assert_eq!(restart_command_unit("echo nothing"), None);
        assert_eq!(restart_command_unit("systemctl restart"), None);
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
}
