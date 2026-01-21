use clap::{Parser, Subcommand};
use phd2_guider::{load_config, Phd2Client, Phd2Config, Phd2Event, Rect, SettleParams};
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

    /// Start guiding
    Guide {
        /// Recalibrate before guiding
        #[arg(long)]
        recalibrate: bool,

        /// Settling pixels threshold (default: 0.5)
        #[arg(long)]
        settle_pixels: Option<f64>,

        /// Settling time in seconds (default: 10)
        #[arg(long)]
        settle_time: Option<u32>,

        /// Settling timeout in seconds (default: 60)
        #[arg(long)]
        settle_timeout: Option<u32>,

        /// Region of interest: x,y,width,height (e.g., "100,100,200,200")
        #[arg(long)]
        roi: Option<String>,
    },

    /// Stop guiding (continues looping)
    StopGuiding,

    /// Stop all capture and guiding
    StopCapture,

    /// Start looping exposures
    Loop,

    /// Pause guiding
    Pause {
        /// Full pause (stop looping entirely)
        #[arg(long)]
        full: bool,
    },

    /// Resume guiding after pause
    Resume,

    /// Check if guiding is paused
    IsPaused,

    /// Dither the guide position
    Dither {
        /// Dither amount in pixels
        #[arg(default_value = "5.0")]
        amount: f64,

        /// Only dither in RA axis
        #[arg(long)]
        ra_only: bool,

        /// Settling pixels threshold (default: 0.5)
        #[arg(long)]
        settle_pixels: Option<f64>,

        /// Settling time in seconds (default: 10)
        #[arg(long)]
        settle_time: Option<u32>,

        /// Settling timeout in seconds (default: 60)
        #[arg(long)]
        settle_timeout: Option<u32>,
    },
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
        Commands::Guide {
            recalibrate,
            settle_pixels,
            settle_time,
            settle_timeout,
            roi,
        } => {
            run_guide(
                &client,
                recalibrate,
                settle_pixels,
                settle_time,
                settle_timeout,
                roi,
            )
            .await?;
        }
        Commands::StopGuiding => {
            run_stop_guiding(&client).await?;
        }
        Commands::StopCapture => {
            run_stop_capture(&client).await?;
        }
        Commands::Loop => {
            run_loop(&client).await?;
        }
        Commands::Pause { full } => {
            run_pause(&client, full).await?;
        }
        Commands::Resume => {
            run_resume(&client).await?;
        }
        Commands::IsPaused => {
            run_is_paused(&client).await?;
        }
        Commands::Dither {
            amount,
            ra_only,
            settle_pixels,
            settle_time,
            settle_timeout,
        } => {
            run_dither(
                &client,
                amount,
                ra_only,
                settle_pixels,
                settle_time,
                settle_timeout,
            )
            .await?;
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

// ============================================================================
// Guiding Control Commands
// ============================================================================

fn parse_roi(roi_str: &str) -> Result<Rect, Box<dyn std::error::Error>> {
    let parts: Vec<&str> = roi_str.split(',').collect();
    if parts.len() != 4 {
        return Err("ROI must be in format: x,y,width,height".into());
    }
    Ok(Rect::new(
        parts[0].trim().parse()?,
        parts[1].trim().parse()?,
        parts[2].trim().parse()?,
        parts[3].trim().parse()?,
    ))
}

async fn run_guide(
    client: &Phd2Client,
    recalibrate: bool,
    settle_pixels: Option<f64>,
    settle_time: Option<u32>,
    settle_timeout: Option<u32>,
    roi: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    let settle = SettleParams {
        pixels: settle_pixels.unwrap_or(0.5),
        time: settle_time.unwrap_or(10),
        timeout: settle_timeout.unwrap_or(60),
    };

    let roi_rect = match roi {
        Some(s) => Some(parse_roi(&s)?),
        None => None,
    };

    info!(
        "Starting guiding (recalibrate={}, settle: pixels={}, time={}, timeout={})",
        recalibrate, settle.pixels, settle.time, settle.timeout
    );

    client.start_guiding(&settle, recalibrate, roi_rect).await?;
    info!("Guide command sent successfully");

    client.disconnect().await?;
    Ok(())
}

async fn run_stop_guiding(client: &Phd2Client) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    info!("Stopping guiding (continuing loop)...");
    client.stop_guiding().await?;
    info!("Stop guiding command sent successfully");

    client.disconnect().await?;
    Ok(())
}

async fn run_stop_capture(client: &Phd2Client) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    info!("Stopping capture...");
    client.stop_capture().await?;
    info!("Stop capture command sent successfully");

    client.disconnect().await?;
    Ok(())
}

async fn run_loop(client: &Phd2Client) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    info!("Starting loop...");
    client.start_loop().await?;
    info!("Loop command sent successfully");

    client.disconnect().await?;
    Ok(())
}

async fn run_pause(client: &Phd2Client, full: bool) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    info!("Pausing guiding (full={})...", full);
    client.pause(full).await?;
    info!("Pause command sent successfully");

    client.disconnect().await?;
    Ok(())
}

async fn run_resume(client: &Phd2Client) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    info!("Resuming guiding...");
    client.resume().await?;
    info!("Resume command sent successfully");

    client.disconnect().await?;
    Ok(())
}

async fn run_is_paused(client: &Phd2Client) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    let paused = client.is_paused().await?;
    info!("Guiding is paused: {}", paused);

    client.disconnect().await?;
    Ok(())
}

async fn run_dither(
    client: &Phd2Client,
    amount: f64,
    ra_only: bool,
    settle_pixels: Option<f64>,
    settle_time: Option<u32>,
    settle_timeout: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to PHD2...");
    client.connect().await?;

    let settle = SettleParams {
        pixels: settle_pixels.unwrap_or(0.5),
        time: settle_time.unwrap_or(10),
        timeout: settle_timeout.unwrap_or(60),
    };

    info!(
        "Dithering (amount={}, ra_only={}, settle: pixels={}, time={}, timeout={})",
        amount, ra_only, settle.pixels, settle.time, settle.timeout
    );

    client.dither(amount, ra_only, &settle).await?;
    info!("Dither command sent successfully");

    client.disconnect().await?;
    Ok(())
}
