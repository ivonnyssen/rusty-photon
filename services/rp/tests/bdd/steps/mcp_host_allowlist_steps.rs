//! BDD step definitions for the MCP endpoint's `Host` allowlist
//! (`mcp_host_allowlist.feature`).
//!
//! The scenarios spawn their own rp on port 0 bound to all interfaces —
//! the deployment shape whose advertised hostname URL rp used to reject
//! — and address the loopback listener with an explicit `Host` header,
//! which is exactly what an orchestrator dialing that URL sends. They
//! never touch `OmniSim`, so the feature is untagged (no `@serial`), and
//! they reuse `rp is started with that config file` from
//! `config_rest_steps`.

use cucumber::{given, then, when};
use serde_json::Value;

use crate::world::RpWorld;

use super::config_rest_steps::write_scenario_config_with_server;

/// A wildcard bind on an ephemeral port, optionally advertising a URL.
fn wildcard_server(advertised_url: Option<&str>) -> Value {
    match advertised_url {
        Some(url) => serde_json::json!({
            "port": 0, "bind_address": "0.0.0.0", "advertised_url": url
        }),
        None => serde_json::json!({ "port": 0, "bind_address": "0.0.0.0" }),
    }
}

/// POST an MCP `initialize` to rp's loopback listener carrying `host` as
/// the `Host` header, and record the status the transport answered with.
async fn send_initialize_with_host(world: &mut RpWorld, host: &str) {
    let response = reqwest::Client::new()
        .post(world.rp_mcp_url())
        .header(reqwest::header::HOST, host)
        .header("accept", "application/json, text/event-stream")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "host-allowlist-bdd", "version": "0"}
            }
        }))
        .send()
        .await
        .expect("POST /mcp request failed");
    world.last_mcp_host_status = Some(response.status().as_u16());
}

// ---------------------------------------------------------------------------
// Given
// ---------------------------------------------------------------------------

#[given("a temp rp config bound to all interfaces")]
fn temp_config_wildcard_bind(world: &mut RpWorld) {
    write_scenario_config_with_server(world, serde_json::json!({}), wildcard_server(None));
}

#[given(expr = "a temp rp config bound to all interfaces advertising {string}")]
fn temp_config_wildcard_bind_advertising(world: &mut RpWorld, url: String) {
    write_scenario_config_with_server(world, serde_json::json!({}), wildcard_server(Some(&url)));
}

// ---------------------------------------------------------------------------
// When
// ---------------------------------------------------------------------------

#[when("an MCP initialize request is sent with the system hostname as the Host header")]
async fn initialize_with_system_hostname(world: &mut RpWorld) {
    // The same source rp derives its advertised host from, so the
    // scenario asserts the two agree rather than a hard-coded name.
    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .expect("system hostname is not available on this host");
    send_initialize_with_host(world, &hostname).await;
}

#[when(expr = "an MCP initialize request is sent with the Host header {string}")]
async fn initialize_with_host(world: &mut RpWorld, host: String) {
    send_initialize_with_host(world, &host).await;
}

// ---------------------------------------------------------------------------
// Then
// ---------------------------------------------------------------------------

#[then(expr = "the MCP response status should be {int}")]
fn mcp_response_status_is(world: &mut RpWorld, expected: u16) {
    assert_eq!(
        world.last_mcp_host_status,
        Some(expected),
        "unexpected /mcp status for the Host header under test"
    );
}
