//! Unit tests for PHD2 RPC types

use phd2_guider::{RpcRequest, RpcResponse};

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
    let json = r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid request"},"id":1}"#;
    let response: RpcResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.id, 1);
    assert!(response.result.is_none());
    let error = response.error.unwrap();
    assert_eq!(error.code, -32600);
    assert_eq!(error.message, "Invalid request");
}
