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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_request_serialization() {
        let request = RpcRequest::new("get_app_state", None, 1);
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"get_app_state\""));
        assert!(json.contains("\"id\":1"));
        assert!(!json.contains("params"));
    }

    #[test]
    fn test_rpc_request_with_params() {
        let request = RpcRequest::new("set_connected", Some(serde_json::json!(true)), 2);
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"params\":true"));
    }

    #[test]
    fn test_rpc_response_parsing() {
        let json = r#"{"jsonrpc":"2.0","result":"Guiding","id":1}"#;
        let response: RpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, 1);
        assert_eq!(response.result.unwrap().as_str().unwrap(), "Guiding");
        assert!(response.error.is_none());
    }

    #[test]
    fn test_rpc_error_response_parsing() {
        let json =
            r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid request"},"id":1}"#;
        let response: RpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, 1);
        assert!(response.result.is_none());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32600);
        assert_eq!(error.message, "Invalid request");
    }
}
