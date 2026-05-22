#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Star Adventurer GTi driver CLI.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing::{debug, info, warn, Level};

#[cfg(feature = "mock")]
use star_adventurer_gti::transport::mock::CapturingMockFactory;
use star_adventurer_gti::{
    canonicalise_config_path, load_config, warn_if_park_path_unwritable, Config, ServerBuilder,
    TransportFactory,
};

#[derive(Parser)]
#[command(name = "star-adventurer-gti")]
#[command(about = "ASCOM Alpaca driver for Sky-Watcher Star Adventurer GTi GEM")]
#[command(version)]
struct Args {
    /// Path to JSON configuration file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Override `transport.kind` (`usb` or `udp`)
    #[arg(long)]
    transport: Option<String>,

    /// Override `transport.port` (USB device path) or `transport.address` (UDP host)
    #[arg(long)]
    port: Option<String>,

    /// Override `transport.baud_rate` (USB only)
    #[arg(long)]
    baud: Option<u32>,

    /// Override `server.port`
    #[arg(long)]
    server_port: Option<u16>,

    /// Log level (trace / debug / info / warn / error)
    #[arg(short, long, default_value = "info", value_parser = parse_log_level)]
    log_level: Level,
}

fn parse_log_level(s: &str) -> Result<Level, String> {
    s.parse()
        .map_err(|_| format!("Invalid log level: {s}. Use: trace, debug, info, warn, error"))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!("failed to wait for Ctrl+C: {e}");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                warn!("failed to install SIGTERM handler: {e}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => debug!("received Ctrl+C"),
        () = terminate => debug!("received SIGTERM"),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .init();

    debug!(
        "Parsed command line arguments: config={:?}, transport={:?}, port={:?}, server_port={:?}, log_level={:?}",
        args.config, args.transport, args.port, args.server_port, args.log_level
    );

    let mut config = if let Some(config_path) = &args.config {
        debug!("Loading configuration from {:?}", config_path);
        load_config(config_path)?
    } else {
        debug!("Using default configuration");
        Config::default()
    };

    apply_cli_overrides(&mut config, &args)?;

    // Canonicalise the operator-supplied config path so `SetPark` writes
    // to a stable absolute location; also runs the early-warning
    // writability probe so a bad path / permissions setup surfaces a
    // `warn!` at boot instead of only on the first `SetPark` call. Both
    // helpers live in the library crate so their warn-on-failure
    // branches are unit-testable.
    let config_file_path = canonicalise_config_path(args.config.as_ref());
    if let Some(path) = &config_file_path {
        warn_if_park_path_unwritable(path);
    }

    info!("Starting Star Adventurer GTi driver");
    #[cfg(feature = "mock")]
    info!("Running in MOCK MODE - no real hardware");
    info!("Server port: {}", config.server.port);

    let builder = ServerBuilder::new()
        .with_config(config)
        .with_config_file_path(config_file_path);

    #[cfg(feature = "mock")]
    let builder = {
        // CapturingMockFactory shares its `MockMountState` Arc with
        // every transport it hands out. Reuse that same state for the
        // /debug/v1/mock-commands endpoint so tools like BDD harnesses
        // can inspect the wire command log over HTTP.
        let factory = CapturingMockFactory::new();
        let state = Arc::clone(&factory.state);
        let factory: Arc<dyn TransportFactory> = Arc::new(factory);
        builder
            .with_transport_factory(factory)
            .with_debug_mock_state(state)
    };

    #[cfg(not(feature = "mock"))]
    {
        // Production builds let `ServerBuilder::build()` pick the factory
        // (Serial vs UDP) from `config.transport`. The factory's
        // `connect()` body is filled in by Phase 3; until then ASCOM
        // `Connected = true` returns NOT_IMPLEMENTED but the HTTP server
        // still binds and serves metadata.
        let _: Option<Arc<dyn TransportFactory>> = None;
    }

    let bound = builder.build().await?;

    tokio::select! {
        result = bound.start() => { result?; }
        () = shutdown_signal() => { info!("Shutting down"); }
    }

    Ok(())
}

fn apply_cli_overrides(config: &mut Config, args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    use star_adventurer_gti::TransportConfig;

    if let Some(kind) = &args.transport {
        match kind.as_str() {
            "usb" => {
                if !matches!(config.transport, TransportConfig::Usb(_)) {
                    config.transport = TransportConfig::Usb(Default::default());
                }
            }
            "udp" => {
                if !matches!(config.transport, TransportConfig::Udp(_)) {
                    config.transport = TransportConfig::Udp(Default::default());
                }
            }
            other => return Err(format!("invalid --transport: {other}").into()),
        }
    }

    if let Some(port) = &args.port {
        match &mut config.transport {
            TransportConfig::Usb(usb) => usb.port = port.clone(),
            TransportConfig::Udp(udp) => udp.address = port.parse()?,
        }
    }

    if let Some(baud) = args.baud {
        if let TransportConfig::Usb(usb) = &mut config.transport {
            usb.baud_rate = baud;
        }
    }

    if let Some(server_port) = args.server_port {
        config.server.port = server_port;
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use star_adventurer_gti::TransportConfig;

    fn args() -> Args {
        Args {
            config: None,
            transport: None,
            port: None,
            baud: None,
            server_port: None,
            log_level: Level::INFO,
        }
    }

    #[test]
    fn parse_log_level_accepts_canonical_levels() {
        // The clap value-parser only forwards canonical strings.
        // Non-strings reach via tests below.
        assert_eq!(parse_log_level("trace").unwrap(), Level::TRACE);
        assert_eq!(parse_log_level("debug").unwrap(), Level::DEBUG);
        assert_eq!(parse_log_level("info").unwrap(), Level::INFO);
        assert_eq!(parse_log_level("warn").unwrap(), Level::WARN);
        assert_eq!(parse_log_level("error").unwrap(), Level::ERROR);
    }

    #[test]
    fn parse_log_level_rejects_invalid() {
        let err = parse_log_level("not-a-level").unwrap_err();
        assert!(err.contains("not-a-level"));
        assert!(err.contains("trace, debug, info, warn, error"));
    }

    #[test]
    fn apply_cli_overrides_no_args_leaves_config_untouched() {
        let mut cfg = Config::default();
        let baseline_port = cfg.server.port;
        apply_cli_overrides(&mut cfg, &args()).unwrap();
        assert_eq!(cfg.server.port, baseline_port);
    }

    #[test]
    fn apply_cli_overrides_switches_transport_kind_usb_to_udp() {
        let mut cfg = Config::default();
        // Default is USB; switch to UDP.
        let mut a = args();
        a.transport = Some("udp".into());
        apply_cli_overrides(&mut cfg, &a).unwrap();
        assert!(matches!(cfg.transport, TransportConfig::Udp(_)));
    }

    #[test]
    fn apply_cli_overrides_keeps_existing_usb_block_when_transport_is_usb() {
        // If the config already has a USB block, --transport=usb must
        // not stomp on its tweaked fields.
        let mut cfg = Config::default();
        if let TransportConfig::Usb(usb) = &mut cfg.transport {
            usb.port = "/dev/ttyUSB-tweaked".into();
        }
        let mut a = args();
        a.transport = Some("usb".into());
        apply_cli_overrides(&mut cfg, &a).unwrap();
        if let TransportConfig::Usb(usb) = &cfg.transport {
            assert_eq!(usb.port, "/dev/ttyUSB-tweaked");
        } else {
            panic!("transport must remain Usb");
        }
    }

    #[test]
    fn apply_cli_overrides_rejects_unknown_transport_kind() {
        let mut cfg = Config::default();
        let mut a = args();
        a.transport = Some("bluetooth".into());
        let err = apply_cli_overrides(&mut cfg, &a).unwrap_err();
        assert!(err.to_string().contains("bluetooth"));
    }

    #[test]
    fn apply_cli_overrides_port_sets_usb_port_path() {
        let mut cfg = Config::default();
        let mut a = args();
        a.port = Some("/dev/ttyACM7".into());
        apply_cli_overrides(&mut cfg, &a).unwrap();
        if let TransportConfig::Usb(usb) = &cfg.transport {
            assert_eq!(usb.port, "/dev/ttyACM7");
        } else {
            panic!("transport must remain Usb");
        }
    }

    #[test]
    fn apply_cli_overrides_port_sets_udp_address_when_udp_transport() {
        let mut cfg = Config {
            transport: TransportConfig::Udp(Default::default()),
            ..Config::default()
        };
        let mut a = args();
        // UdpConfig.address is an IpAddr -- bare IP, no port suffix.
        a.port = Some("10.0.0.1".into());
        apply_cli_overrides(&mut cfg, &a).unwrap();
        if let TransportConfig::Udp(udp) = &cfg.transport {
            assert_eq!(udp.address.to_string(), "10.0.0.1");
        } else {
            panic!("transport must remain Udp");
        }
    }

    #[test]
    fn apply_cli_overrides_port_with_invalid_udp_address_returns_err() {
        let mut cfg = Config {
            transport: TransportConfig::Udp(Default::default()),
            ..Config::default()
        };
        let mut a = args();
        a.port = Some("not-an-address".into());
        assert!(apply_cli_overrides(&mut cfg, &a).is_err());
    }

    #[test]
    fn apply_cli_overrides_baud_only_applies_to_usb() {
        let mut cfg = Config::default();
        let mut a = args();
        a.baud = Some(57600);
        apply_cli_overrides(&mut cfg, &a).unwrap();
        if let TransportConfig::Usb(usb) = &cfg.transport {
            assert_eq!(usb.baud_rate, 57600);
        } else {
            panic!("transport must remain Usb");
        }

        // Same flag against a UDP transport must be a no-op (not an error).
        let mut udp_cfg = Config {
            transport: TransportConfig::Udp(Default::default()),
            ..Config::default()
        };
        apply_cli_overrides(&mut udp_cfg, &a).unwrap();
        assert!(matches!(udp_cfg.transport, TransportConfig::Udp(_)));
    }

    #[test]
    fn apply_cli_overrides_server_port_sets_server_port() {
        let mut cfg = Config::default();
        let mut a = args();
        a.server_port = Some(54321);
        apply_cli_overrides(&mut cfg, &a).unwrap();
        assert_eq!(cfg.server.port, 54321);
    }
}
