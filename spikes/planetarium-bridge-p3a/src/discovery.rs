//! Logging Alpaca UDP discovery responder.
//!
//! The spike runs its own responder instead of the shared
//! `rusty_photon_driver::discovery` one because observing the client is the
//! point: every datagram is narrated and captured in the wire log, including
//! malformed ones we don't answer. Discovery is default-ON here (unlike the
//! fleet's opt-in convention) — whether and how the planetarium discovers
//! the device is one of the P3a questions.

use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::{info, warn};

use crate::wirelog::WireLog;

pub async fn run(port: u16, alpaca_port: u16, log: Arc<WireLog>) {
    let socket = match UdpSocket::bind(("0.0.0.0", port)).await {
        Ok(socket) => socket,
        Err(e) => {
            warn!(
                "discovery bind on UDP {port} failed ({e}) — another responder may be \
                 running on this host; continuing with manual IP:port entry only"
            );
            return;
        }
    };
    info!("Alpaca discovery responder on UDP {port} (answers with AlpacaPort={alpaca_port})");

    let reply = format!("{{\"AlpacaPort\":{alpaca_port}}}");
    let mut buf = [0_u8; 2048];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, src)) => {
                let payload = String::from_utf8_lossy(&buf[..len]).into_owned();
                // The v1 discovery datagram is the ASCII string
                // "alpacadiscovery1"; anything else is logged but unanswered.
                let answer = payload.trim().eq_ignore_ascii_case("alpacadiscovery1");
                info!(
                    "DISCOVERY from {src}: {payload:?}{}",
                    if answer {
                        " → answered"
                    } else {
                        " (not answered)"
                    }
                );
                log.record_discovery(src, &payload, answer.then_some(reply.as_str()));
                if answer {
                    if let Err(e) = socket.send_to(reply.as_bytes(), src).await {
                        warn!("discovery reply to {src} failed: {e}");
                    }
                }
            }
            Err(e) => {
                warn!("discovery recv failed: {e}; responder continuing");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}
