//! Mockall-based tests for PHD2 client RPC methods
//!
//! These tests use mockall to mock the connection factory and I/O traits,
//! enabling testing without actual network operations. These tests can run
//! under miri since they don't perform any syscalls.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use phd2_guider::io::{ConnectionFactory, ConnectionPair, LineReader, MessageWriter};
use phd2_guider::{
    AppState, CalibrationTarget, GuideAxis, Phd2Client, Phd2Config, Phd2Error, Rect, SettleParams,
};

// ============================================================================
// Mock implementations for testing
// ============================================================================

/// Mock line reader that returns pre-configured responses
struct MockLineReaderWithResponses {
    responses: StdMutex<VecDeque<Option<String>>>,
}

impl MockLineReaderWithResponses {
    fn new(responses: Vec<Option<String>>) -> Self {
        Self {
            responses: StdMutex::new(responses.into_iter().collect()),
        }
    }
}

#[async_trait]
impl LineReader for MockLineReaderWithResponses {
    async fn read_line(&mut self) -> phd2_guider::Result<Option<String>> {
        let mut responses = self.responses.lock().unwrap();
        match responses.pop_front() {
            Some(response) => Ok(response),
            None => Ok(None), // EOF
        }
    }
}

/// Mock message writer that records sent messages
struct MockMessageWriterWithRecorder {
    sent_messages: Arc<StdMutex<Vec<String>>>,
}

impl MockMessageWriterWithRecorder {
    fn new(sent_messages: Arc<StdMutex<Vec<String>>>) -> Self {
        Self { sent_messages }
    }
}

#[async_trait]
impl MessageWriter for MockMessageWriterWithRecorder {
    async fn write_message(&mut self, message: &str) -> phd2_guider::Result<()> {
        self.sent_messages.lock().unwrap().push(message.to_string());
        Ok(())
    }

    async fn shutdown(&mut self) -> phd2_guider::Result<()> {
        Ok(())
    }
}

/// Mock connection factory that returns pre-configured reader/writer pairs
struct MockConnectionFactoryWithPairs {
    pairs: StdMutex<VecDeque<(Vec<Option<String>>, Arc<StdMutex<Vec<String>>>)>>,
}

impl MockConnectionFactoryWithPairs {
    fn new() -> Self {
        Self {
            pairs: StdMutex::new(VecDeque::new()),
        }
    }

    fn add_connection(&self, responses: Vec<Option<String>>) -> Arc<StdMutex<Vec<String>>> {
        let sent_messages = Arc::new(StdMutex::new(Vec::new()));
        self.pairs
            .lock()
            .unwrap()
            .push_back((responses, sent_messages.clone()));
        sent_messages
    }
}

#[async_trait]
impl ConnectionFactory for MockConnectionFactoryWithPairs {
    async fn connect(
        &self,
        _addr: &str,
        _timeout: Duration,
    ) -> phd2_guider::Result<ConnectionPair> {
        let mut pairs = self.pairs.lock().unwrap();
        if let Some((responses, sent_messages)) = pairs.pop_front() {
            Ok(ConnectionPair {
                reader: Box::new(MockLineReaderWithResponses::new(responses)),
                writer: Box::new(MockMessageWriterWithRecorder::new(sent_messages)),
            })
        } else {
            Err(Phd2Error::ConnectionFailed(
                "No mock connections available".to_string(),
            ))
        }
    }

    async fn can_connect(&self, _addr: &str) -> bool {
        !self.pairs.lock().unwrap().is_empty()
    }
}

/// Helper to create a test client with mock responses
fn create_test_client_with_responses(
    responses: Vec<Option<String>>,
) -> (Phd2Client, Arc<StdMutex<Vec<String>>>) {
    let factory = Arc::new(MockConnectionFactoryWithPairs::new());
    let sent_messages = factory.add_connection(responses);

    let config = Phd2Config {
        host: "localhost".to_string(),
        port: 4400,
        connection_timeout_seconds: 1,
        command_timeout_seconds: 1,
        ..Default::default()
    };

    let client = Phd2Client::with_connection_factory(config, factory);
    (client, sent_messages)
}

/// Helper to create a version event response
fn version_event() -> String {
    r#"{"Event":"Version","PHDVersion":"2.6.11","PHDSubver":"","MsgVersion":1}"#.to_string()
}

/// Helper to create an RPC response
fn rpc_response(id: u64, result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","result":{},"id":{}}}"#, result, id)
}

/// Helper to create an RPC error response
fn rpc_error(id: u64, code: i32, message: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","error":{{"code":{},"message":"{}"}},"id":{}}}"#,
        code, message, id
    )
}

// ============================================================================
// Connection tests
// ============================================================================

#[tokio::test]
async fn test_client_connect_success() {
    let (client, _sent) = create_test_client_with_responses(vec![Some(version_event())]);

    client.connect().await.unwrap();
    assert!(client.is_connected().await);
}

#[tokio::test]
async fn test_client_disconnect() {
    let (client, _sent) = create_test_client_with_responses(vec![Some(version_event())]);

    client.connect().await.unwrap();
    client.disconnect().await.unwrap();
    assert!(!client.is_connected().await);
}

#[tokio::test]
async fn test_client_not_connected_error() {
    let factory = Arc::new(MockConnectionFactoryWithPairs::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    let result = client.get_app_state().await;
    assert!(matches!(result, Err(Phd2Error::NotConnected)));
}

// ============================================================================
// State and status method tests
// ============================================================================

#[tokio::test]
async fn test_get_app_state_stopped() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, r#""Stopped""#)),
    ]);

    client.connect().await.unwrap();
    let state = client.get_app_state().await.unwrap();
    assert_eq!(state, AppState::Stopped);
}

#[tokio::test]
async fn test_get_app_state_guiding() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, r#""Guiding""#)),
    ]);

    client.connect().await.unwrap();
    let state = client.get_app_state().await.unwrap();
    assert_eq!(state, AppState::Guiding);
}

#[tokio::test]
async fn test_get_app_state_looping() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, r#""Looping""#)),
    ]);

    client.connect().await.unwrap();
    let state = client.get_app_state().await.unwrap();
    assert_eq!(state, AppState::Looping);
}

#[tokio::test]
async fn test_get_app_state_calibrating() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, r#""Calibrating""#)),
    ]);

    client.connect().await.unwrap();
    let state = client.get_app_state().await.unwrap();
    assert_eq!(state, AppState::Calibrating);
}

// ============================================================================
// Equipment and profile method tests
// ============================================================================

#[tokio::test]
async fn test_is_equipment_connected_true() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "true")),
    ]);

    client.connect().await.unwrap();
    let connected = client.is_equipment_connected().await.unwrap();
    assert!(connected);
}

#[tokio::test]
async fn test_is_equipment_connected_false() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "false")),
    ]);

    client.connect().await.unwrap();
    let connected = client.is_equipment_connected().await.unwrap();
    assert!(!connected);
}

#[tokio::test]
async fn test_connect_equipment() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.connect_equipment().await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("set_connected"));
    assert!(messages[0].contains("true"));
}

#[tokio::test]
async fn test_disconnect_equipment() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.disconnect_equipment().await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("set_connected"));
    assert!(messages[0].contains("false"));
}

#[tokio::test]
async fn test_get_profiles() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(
            1,
            r#"[{"id":1,"name":"Profile 1"},{"id":2,"name":"Profile 2"}]"#,
        )),
    ]);

    client.connect().await.unwrap();
    let profiles = client.get_profiles().await.unwrap();

    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0].id, 1);
    assert_eq!(profiles[0].name, "Profile 1");
    assert_eq!(profiles[1].id, 2);
    assert_eq!(profiles[1].name, "Profile 2");
}

#[tokio::test]
async fn test_get_current_profile() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, r#"{"id":1,"name":"Default"}"#)),
    ]);

    client.connect().await.unwrap();
    let profile = client.get_current_profile().await.unwrap();

    assert_eq!(profile.id, 1);
    assert_eq!(profile.name, "Default");
}

#[tokio::test]
async fn test_set_profile() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.set_profile(2).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("set_profile"));
    assert!(messages[0].contains("\"id\":2"));
}

#[tokio::test]
async fn test_get_current_equipment() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(
            1,
            r#"{"camera":{"name":"Camera","connected":true},"mount":{"name":"Mount","connected":true},"aux_mount":null,"AO":null,"rotator":null}"#,
        )),
    ]);

    client.connect().await.unwrap();
    let equipment = client.get_current_equipment().await.unwrap();

    assert!(equipment.camera.is_some());
    let camera = equipment.camera.unwrap();
    assert_eq!(camera.name, "Camera");
    assert!(camera.connected);
}

// ============================================================================
// Guiding control method tests
// ============================================================================

#[tokio::test]
async fn test_start_guiding() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    let settle = SettleParams {
        pixels: 0.5,
        time: 10,
        timeout: 60,
    };
    client.start_guiding(&settle, false, None).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("guide"));
    assert!(messages[0].contains("\"pixels\":0.5"));
    assert!(messages[0].contains("\"recalibrate\":false"));
}

#[tokio::test]
async fn test_start_guiding_with_recalibrate() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    let settle = SettleParams::default();
    client.start_guiding(&settle, true, None).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("\"recalibrate\":true"));
}

#[tokio::test]
async fn test_start_guiding_with_roi() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    let settle = SettleParams::default();
    let roi = Rect {
        x: 100,
        y: 100,
        width: 200,
        height: 200,
    };
    client
        .start_guiding(&settle, false, Some(roi))
        .await
        .unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("\"roi\":[100,100,200,200]"));
}

#[tokio::test]
async fn test_stop_guiding() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.stop_guiding().await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("loop"));
}

#[tokio::test]
async fn test_stop_capture() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.stop_capture().await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("stop_capture"));
}

#[tokio::test]
async fn test_start_loop() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.start_loop().await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("loop"));
}

#[tokio::test]
async fn test_is_paused_true() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "true")),
    ]);

    client.connect().await.unwrap();
    let paused = client.is_paused().await.unwrap();
    assert!(paused);
}

#[tokio::test]
async fn test_is_paused_false() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "false")),
    ]);

    client.connect().await.unwrap();
    let paused = client.is_paused().await.unwrap();
    assert!(!paused);
}

#[tokio::test]
async fn test_pause_partial() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.pause(false).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("set_paused"));
    assert!(messages[0].contains("\"paused\":true"));
    assert!(!messages[0].contains("\"full\""));
}

#[tokio::test]
async fn test_pause_full() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.pause(true).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("set_paused"));
    assert!(messages[0].contains("\"full\":\"full\""));
}

#[tokio::test]
async fn test_resume() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.resume().await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("set_paused"));
    assert!(messages[0].contains("\"paused\":false"));
}

#[tokio::test]
async fn test_dither() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    let settle = SettleParams {
        pixels: 0.5,
        time: 10,
        timeout: 60,
    };
    client.dither(5.0, false, &settle).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("dither"));
    assert!(messages[0].contains("\"amount\":5"));
    assert!(messages[0].contains("\"raOnly\":false"));
}

#[tokio::test]
async fn test_dither_ra_only() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    let settle = SettleParams::default();
    client.dither(3.0, true, &settle).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("\"raOnly\":true"));
}

// ============================================================================
// Star selection method tests
// ============================================================================

#[tokio::test]
async fn test_find_star() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.find_star(None).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("find_star"));
}

#[tokio::test]
async fn test_find_star_with_roi() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    let roi = Rect {
        x: 50,
        y: 50,
        width: 100,
        height: 100,
    };
    client.find_star(Some(roi)).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("[50,50,100,100]"));
}

#[tokio::test]
async fn test_get_lock_position() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "[320.5,240.3]")),
    ]);

    client.connect().await.unwrap();
    let (x, y) = client.get_lock_position().await.unwrap();

    assert!((x - 320.5).abs() < 0.01);
    assert!((y - 240.3).abs() < 0.01);
}

#[tokio::test]
async fn test_get_lock_position_no_star() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "null")),
    ]);

    client.connect().await.unwrap();
    let result = client.get_lock_position().await;

    assert!(matches!(result, Err(Phd2Error::InvalidState(_))));
}

#[tokio::test]
async fn test_set_lock_position() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.set_lock_position(320.0, 240.0, true).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("set_lock_position"));
    assert!(messages[0].contains("\"X\":320"));
    assert!(messages[0].contains("\"Y\":240"));
    assert!(messages[0].contains("\"EXACT\":true"));
}

#[tokio::test]
async fn test_set_lock_position_not_exact() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.set_lock_position(100.0, 100.0, false).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("\"EXACT\":false"));
}

// ============================================================================
// Calibration method tests
// ============================================================================

#[tokio::test]
async fn test_is_calibrated_true() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "true")),
    ]);

    client.connect().await.unwrap();
    let calibrated = client.is_calibrated().await.unwrap();
    assert!(calibrated);
}

#[tokio::test]
async fn test_is_calibrated_false() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "false")),
    ]);

    client.connect().await.unwrap();
    let calibrated = client.is_calibrated().await.unwrap();
    assert!(!calibrated);
}

#[tokio::test]
async fn test_get_calibration_data() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(
            1,
            r#"{"calibrated":true,"xAngle":45.0,"xParity":"+","xRate":10.0,"yAngle":135.0,"yParity":"-","yRate":10.0}"#,
        )),
    ]);

    client.connect().await.unwrap();
    let data = client
        .get_calibration_data(CalibrationTarget::Mount)
        .await
        .unwrap();

    assert!(data.calibrated);
    assert!((data.x_angle - 45.0).abs() < 0.01);
    assert_eq!(data.x_parity, "+");
}

#[tokio::test]
async fn test_clear_calibration_mount() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client
        .clear_calibration(CalibrationTarget::Mount)
        .await
        .unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("clear_calibration"));
    assert!(messages[0].contains("\"which\":\"mount\""));
}

#[tokio::test]
async fn test_clear_calibration_both() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client
        .clear_calibration(CalibrationTarget::Both)
        .await
        .unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("\"which\":\"both\""));
}

#[tokio::test]
async fn test_flip_calibration() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.flip_calibration().await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("flip_calibration"));
}

// ============================================================================
// Camera exposure method tests
// ============================================================================

#[tokio::test]
async fn test_get_exposure() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "2000")),
    ]);

    client.connect().await.unwrap();
    let exposure = client.get_exposure().await.unwrap();
    assert_eq!(exposure, 2000);
}

#[tokio::test]
async fn test_set_exposure() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.set_exposure(3000).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("set_exposure"));
    assert!(messages[0].contains("3000"));
}

#[tokio::test]
async fn test_get_exposure_durations() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "[100,200,500,1000,2000,3000]")),
    ]);

    client.connect().await.unwrap();
    let durations = client.get_exposure_durations().await.unwrap();

    assert_eq!(durations, vec![100, 200, 500, 1000, 2000, 3000]);
}

#[tokio::test]
async fn test_get_camera_frame_size() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "[640,480]")),
    ]);

    client.connect().await.unwrap();
    let (width, height) = client.get_camera_frame_size().await.unwrap();

    assert_eq!(width, 640);
    assert_eq!(height, 480);
}

#[tokio::test]
async fn test_get_use_subframes_true() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "true")),
    ]);

    client.connect().await.unwrap();
    let use_subframes = client.get_use_subframes().await.unwrap();
    assert!(use_subframes);
}

#[tokio::test]
async fn test_capture_single_frame() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.capture_single_frame(None, None).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("capture_single_frame"));
}

#[tokio::test]
async fn test_capture_single_frame_with_exposure() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.capture_single_frame(Some(5000), None).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("\"exposure\":5000"));
}

#[tokio::test]
async fn test_capture_single_frame_with_subframe() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    let subframe = Rect {
        x: 100,
        y: 100,
        width: 200,
        height: 200,
    };
    client
        .capture_single_frame(None, Some(subframe))
        .await
        .unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("\"subframe\":[100,100,200,200]"));
}

// ============================================================================
// Guide algorithm parameter tests
// ============================================================================

#[tokio::test]
async fn test_get_algo_param_names() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, r#"["Aggressiveness","MinMove","MaxMove"]"#)),
    ]);

    client.connect().await.unwrap();
    let names = client.get_algo_param_names(GuideAxis::Ra).await.unwrap();

    assert_eq!(names, vec!["Aggressiveness", "MinMove", "MaxMove"]);
}

#[tokio::test]
async fn test_get_algo_param() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "0.7")),
    ]);

    client.connect().await.unwrap();
    let value = client
        .get_algo_param(GuideAxis::Ra, "Aggressiveness")
        .await
        .unwrap();

    assert!((value - 0.7).abs() < 0.01);
}

#[tokio::test]
async fn test_set_algo_param() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client
        .set_algo_param(GuideAxis::Dec, "MinMove", 0.3)
        .await
        .unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("set_algo_param"));
    assert!(messages[0].contains("\"axis\":\"dec\""));
    assert!(messages[0].contains("\"name\":\"MinMove\""));
    assert!(messages[0].contains("\"value\":0.3"));
}

// ============================================================================
// Camera cooling tests
// ============================================================================

#[tokio::test]
async fn test_get_ccd_temperature() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, "-10.5")),
    ]);

    client.connect().await.unwrap();
    let temp = client.get_ccd_temperature().await.unwrap();
    assert!((temp - (-10.5)).abs() < 0.01);
}

#[tokio::test]
async fn test_get_cooler_status() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(
            1,
            r#"{"coolerOn":true,"temperature":-20.0,"power":50.0,"setpoint":-20.0}"#,
        )),
    ]);

    client.connect().await.unwrap();
    let status = client.get_cooler_status().await.unwrap();

    assert!(status.cooler_on);
    assert!((status.temperature - (-20.0)).abs() < 0.01);
}

#[tokio::test]
async fn test_set_cooler_state_enable() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.set_cooler_state(true, Some(-20.0)).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("set_cooler_state"));
    assert!(messages[0].contains("\"enabled\":true"));
    assert!(messages[0].contains("\"temperature\":-20"));
}

#[tokio::test]
async fn test_set_cooler_state_disable() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.set_cooler_state(false, None).await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("\"enabled\":false"));
}

// ============================================================================
// Image operations tests
// ============================================================================

#[tokio::test]
async fn test_get_star_image() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(
            1,
            r#"{"frame":1,"width":31,"height":31,"pixels":"AAAA","star_pos":[15.5,15.5]}"#,
        )),
    ]);

    client.connect().await.unwrap();
    let image = client.get_star_image(15).await.unwrap();

    assert_eq!(image.frame, 1);
    assert_eq!(image.width, 31);
    assert_eq!(image.height, 31);
}

#[tokio::test]
async fn test_save_image() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_response(1, r#""/tmp/phd2_image.fits""#)),
    ]);

    client.connect().await.unwrap();
    let path = client.save_image().await.unwrap();

    assert_eq!(path, "/tmp/phd2_image.fits");
}

// ============================================================================
// Application control tests
// ============================================================================

#[tokio::test]
async fn test_shutdown_phd2() {
    let (client, sent) =
        create_test_client_with_responses(vec![Some(version_event()), Some(rpc_response(1, "0"))]);

    client.connect().await.unwrap();
    client.shutdown_phd2().await.unwrap();

    let messages = sent.lock().unwrap();
    assert!(messages[0].contains("shutdown"));
}

// ============================================================================
// Error handling tests
// ============================================================================

#[tokio::test]
async fn test_rpc_error_handling() {
    let (client, _sent) = create_test_client_with_responses(vec![
        Some(version_event()),
        Some(rpc_error(1, -1, "Equipment not connected")),
    ]);

    client.connect().await.unwrap();
    let result = client.get_app_state().await;

    match result {
        Err(Phd2Error::RpcError { code, message }) => {
            assert_eq!(code, -1);
            assert_eq!(message, "Equipment not connected");
        }
        _ => panic!("Expected RpcError"),
    }
}

// ============================================================================
// Auto-reconnect control tests
// ============================================================================

#[tokio::test]
async fn test_auto_reconnect_enabled_by_default() {
    let factory = Arc::new(MockConnectionFactoryWithPairs::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    assert!(client.is_auto_reconnect_enabled());
}

#[tokio::test]
async fn test_set_auto_reconnect_disabled() {
    let factory = Arc::new(MockConnectionFactoryWithPairs::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    client.set_auto_reconnect_enabled(false);
    assert!(!client.is_auto_reconnect_enabled());
}

#[tokio::test]
async fn test_toggle_auto_reconnect() {
    let factory = Arc::new(MockConnectionFactoryWithPairs::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    client.set_auto_reconnect_enabled(false);
    assert!(!client.is_auto_reconnect_enabled());

    client.set_auto_reconnect_enabled(true);
    assert!(client.is_auto_reconnect_enabled());
}
