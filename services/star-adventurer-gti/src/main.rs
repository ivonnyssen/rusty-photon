//! Star Adventurer GTi driver CLI.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing::{debug, info, Level};

#[cfg(feature = "mock")]
use star_adventurer_gti::transport::mock::CapturingMockFactory;
use star_adventurer_gti::{load_config, Config, ServerBuilder, TransportFactory};

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
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
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

    info!("Starting Star Adventurer GTi driver");
    #[cfg(feature = "mock")]
    info!("Running in MOCK MODE - no real hardware");
    info!("Server port: {}", config.server.port);

    let builder = ServerBuilder::new().with_config(config);

    #[cfg(feature = "mock")]
    let builder = {
        // CapturingMockFactory holds a pre-built MockTransport whose
        // state Arc is shared with every clone the factory hands out.
        // Reuse that same state for the /debug/v1/mock-commands
        // endpoint so tools like BDD harnesses can inspect the wire
        // command log over HTTP.
        let factory = CapturingMockFactory::new();
        let state = Arc::clone(&factory.mock.state);
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
