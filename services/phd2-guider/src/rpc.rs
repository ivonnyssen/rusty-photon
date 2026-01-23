//! JSON RPC 2.0 types for PHD2 communication

use serde::{Deserialize, Serialize};

use crate::events::Phd2Event;

/// JSON RPC 2.0 request
#[derive(Debug, Clone, Serialize)]
pub struct RpcRequest {
    pub jsonrpc: &'static str,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    pub id: u64,
}

impl RpcRequest {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>, id: u64) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.into(),
            params,
            id,
        }
    }
}

/// JSON RPC 2.0 response
#[derive(Debug, Clone, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<RpcErrorObject>,
    pub id: u64,
}

/// JSON RPC 2.0 error object
#[derive(Debug, Clone, Deserialize)]
pub struct RpcErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

/// Incoming message from PHD2 - either an event or a response
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Phd2Message {
    Response(RpcResponse),
    Event(Phd2Event),
}
