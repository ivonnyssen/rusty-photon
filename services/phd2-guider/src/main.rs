use clap::{Parser, Subcommand};
use phd2_guider::{load_config, Phd2Client, Phd2Config, Phd2Event};
use std::path::PathBuf;
use tracing::{debug, info, Level};

#[derive(Parser)]
#[command(name = "phd2-guider")]
#[command(about = "PHD2 guider client for Rusty Photon")]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// PHD2 host address
    #[arg(long, default_value = "localhost")]
    host: String,

    /// PHD2 port
    #[arg(long, default_value = "4400")]
    port: u16,

    /// Log level
    #[arg(short, long, default_value = "info", value_parser = clap::value_parser!(Level))]
    log_level: Level,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Connect to PHD2 and show status
    Status,

    /// Connect to PHD2 and monitor events
    Monitor,

    /// Connect equipment in PHD2
    Connect,

    /// Disconnect equipment in PHD2
    Disconnect,

    /// List available profiles
    Profiles,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(args.log_level)
        .init();

    debug!(
        "Parsed command line arguments: host={}, port={}, log_level={:?}",
        args.host, args.port, args.log_level
    );

    // Build configuration from CLI args or config file
    let phd2_config = if let Some(config_path) = &args.config {
        debug!("Loading configuration from {:?}", config_path);
        let config = load_config(config_path)?;
        config.phd2
    } else {
        Phd2Config {
            host: args.host,
            port: args.port,
            ..Default::default()
        }
    };

    let client = Phd2Client::new(phd2_config);

    match args.command {
        Commands::Status => {
            run_status(&client).await?;
        }
        Commands::Monitor => {
            run_monitor(&client).await?;
        }
        Commands::Connect => {
            run_connect(&client).await?;
        }
        Commands::Disconnect => {
            run_disconnect(&client).await?;
        }
        Commands::Profiles => {
            run_profiles(&client).await?;
        }
    }

    Ok(())
}

async fn run_status(client: &Phd2Client) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    // Wait a moment for the Version event
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    if let Some(version) = client.get_phd2_version().await {
        info!("PHD2 Version: {}", version);
    }

    let state = client.get_app_state().await?;
    info!("PHD2 State: {}", state);

    let connected = client.is_equipment_connected().await?;
    info!("Equipment connected: {}", connected);

    if connected {
        let profile = client.get_current_profile().await?;
        info!("Current profile: {} (id: {})", profile.name, profile.id);
    }

    client.disconnect().await?;
    Ok(())
}

async fn run_monitor(client: &Phd2Client) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    info!("Monitoring PHD2 events (press Ctrl+C to stop)...");

    let mut receiver = client.subscribe();

    loop {
        tokio::select! {
            event = receiver.recv() => {
                match event {
                    Ok(event) => {
                        print_event(&event);
                    }
                    Err(e) => {
                        debug!("Event receiver error: {}", e);
                        break;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down...");
                break;
            }
        }
    }

    client.disconnect().await?;
    Ok(())
}

fn print_event(event: &Phd2Event) {
    match event {
        Phd2Event::Version { phd_version, .. } => {
            info!("Event: Version - PHD2 {}", phd_version);
        }
        Phd2Event::AppState { state } => {
            info!("Event: AppState - {}", state);
        }
        Phd2Event::GuideStep(stats) => {
            info!(
                "Event: GuideStep - Frame {} dx={:.2} dy={:.2} SNR={:.1}",
                stats.frame,
                stats.dx,
                stats.dy,
                stats.snr.unwrap_or(0.0)
            );
        }
        Phd2Event::StarSelected { x, y } => {
            info!("Event: StarSelected - ({:.1}, {:.1})", x, y);
        }
        Phd2Event::StarLost { status, .. } => {
            info!("Event: StarLost - {}", status);
        }
        Phd2Event::SettleDone { status, error } => {
            if *status == 0 {
                info!("Event: SettleDone - Success");
            } else {
                info!(
                    "Event: SettleDone - Failed: {}",
                    error.as_deref().unwrap_or("unknown")
                );
            }
        }
        Phd2Event::GuidingDithered { dx, dy } => {
            info!("Event: GuidingDithered - dx={:.2} dy={:.2}", dx, dy);
        }
        Phd2Event::Calibrating { step, state, .. } => {
            info!("Event: Calibrating - step {} ({})", step, state);
        }
        Phd2Event::CalibrationComplete { mount } => {
            info!("Event: CalibrationComplete - {}", mount);
        }
        Phd2Event::CalibrationFailed { reason } => {
            info!("Event: CalibrationFailed - {}", reason);
        }
        Phd2Event::LoopingExposures { frame } => {
            info!("Event: LoopingExposures - Frame {}", frame);
        }
        Phd2Event::LoopingExposuresStopped => {
            info!("Event: LoopingExposuresStopped");
        }
        Phd2Event::Paused => {
            info!("Event: Paused");
        }
        Phd2Event::Resumed => {
            info!("Event: Resumed");
        }
        Phd2Event::Alert { msg, alert_type } => {
            info!("Event: Alert [{}] - {}", alert_type, msg);
        }
        Phd2Event::StartGuiding => {
            info!("Event: StartGuiding");
        }
        Phd2Event::GuidingStopped => {
            info!("Event: GuidingStopped");
        }
        Phd2Event::Settling { distance, time, .. } => {
            info!(
                "Event: Settling - distance={:.2} time={:.1}s",
                distance, time
            );
        }
        _ => {
            debug!("Event: {:?}", event);
        }
    }
}

async fn run_connect(client: &Phd2Client) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    info!("Connecting equipment...");
    client.connect_equipment().await?;
    info!("Equipment connected successfully");

    client.disconnect().await?;
    Ok(())
}

async fn run_disconnect(client: &Phd2Client) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    info!("Disconnecting equipment...");
    client.disconnect_equipment().await?;
    info!("Equipment disconnected successfully");

    client.disconnect().await?;
    Ok(())
}

async fn run_profiles(client: &Phd2Client) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    let profiles = client.get_profiles().await?;
    info!("Available profiles:");
    for profile in &profiles {
        info!("  [{}] {}", profile.id, profile.name);
    }

    let current = client.get_current_profile().await?;
    info!("Current profile: {} (id: {})", current.name, current.id);

    client.disconnect().await?;
    Ok(())
}
