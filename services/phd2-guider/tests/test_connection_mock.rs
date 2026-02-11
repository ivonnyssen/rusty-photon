//! Mockall-based tests for connection management
//!
//! These tests use mockall to mock connection-related traits,
//! enabling testing of connection logic without actual network operations.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use phd2_guider::io::{ConnectionFactory, ConnectionPair, LineReader, MessageWriter};
use phd2_guider::{Phd2Client, Phd2Config, Phd2Error, ReconnectConfig};

// ============================================================================
// Mock implementations
// ============================================================================

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
            None => Ok(None),
        }
    }
}

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

type MockPair = (Vec<Option<String>>, Arc<StdMutex<Vec<String>>>);

struct MockConnectionFactory {
    pairs: StdMutex<VecDeque<MockPair>>,
    connect_count: StdMutex<u32>,
    fail_connect: StdMutex<bool>,
}

impl MockConnectionFactory {
    fn new() -> Self {
        Self {
            pairs: StdMutex::new(VecDeque::new()),
            connect_count: StdMutex::new(0),
            fail_connect: StdMutex::new(false),
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

    fn set_fail_connect(&self, fail: bool) {
        *self.fail_connect.lock().unwrap() = fail;
    }

    fn get_connect_count(&self) -> u32 {
        *self.connect_count.lock().unwrap()
    }
}

#[async_trait]
impl ConnectionFactory for MockConnectionFactory {
    async fn connect(
        &self,
        _addr: &str,
        _timeout: Duration,
    ) -> phd2_guider::Result<ConnectionPair> {
        *self.connect_count.lock().unwrap() += 1;

        if *self.fail_connect.lock().unwrap() {
            return Err(Phd2Error::ConnectionFailed(
                "Mock connection failure".to_string(),
            ));
        }

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
        !*self.fail_connect.lock().unwrap() && !self.pairs.lock().unwrap().is_empty()
    }
}

fn version_event() -> String {
    r#"{"Event":"Version","PHDVersion":"2.6.11","PHDSubver":"","MsgVersion":1}"#.to_string()
}

// ============================================================================
// Connection state tests
// ============================================================================

#[tokio::test]
async fn test_initial_connection_state() {
    let factory = Arc::new(MockConnectionFactory::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    assert!(!client.is_connected().await);
    assert!(!client.is_reconnecting().await);
}

#[tokio::test]
async fn test_connection_state_after_connect() {
    let factory = Arc::new(MockConnectionFactory::new());
    factory.add_connection(vec![Some(version_event())]);

    let config = Phd2Config {
        connection_timeout_seconds: 1,
        ..Default::default()
    };
    let client = Phd2Client::with_connection_factory(config, factory);

    client.connect().await.unwrap();
    assert!(client.is_connected().await);
}

#[tokio::test]
async fn test_connection_state_after_disconnect() {
    let factory = Arc::new(MockConnectionFactory::new());
    factory.add_connection(vec![Some(version_event())]);

    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    client.connect().await.unwrap();
    client.disconnect().await.unwrap();
    assert!(!client.is_connected().await);
}

#[tokio::test]
async fn test_disconnect_clears_state() {
    let factory = Arc::new(MockConnectionFactory::new());
    factory.add_connection(vec![Some(version_event())]);

    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    client.connect().await.unwrap();
    // Wait for version event to be processed
    tokio::time::sleep(Duration::from_millis(50)).await;

    client.disconnect().await.unwrap();
    assert!(client.get_phd2_version().await.is_none());
}

// ============================================================================
// PHD2 version tracking tests
// ============================================================================

#[tokio::test]
async fn test_phd2_version_initially_none() {
    let factory = Arc::new(MockConnectionFactory::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    assert!(client.get_phd2_version().await.is_none());
}

#[tokio::test]
async fn test_phd2_version_set_after_connect() {
    let factory = Arc::new(MockConnectionFactory::new());
    factory.add_connection(vec![Some(version_event())]);

    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    client.connect().await.unwrap();
    // Give time for the reader task to process the version event
    tokio::time::sleep(Duration::from_millis(100)).await;

    let version = client.get_phd2_version().await;
    assert_eq!(version, Some("2.6.11".to_string()));
}

// ============================================================================
// Connection failure tests
// ============================================================================

#[tokio::test]
async fn test_connect_failure() {
    let factory = Arc::new(MockConnectionFactory::new());
    factory.set_fail_connect(true);

    let config = Phd2Config {
        connection_timeout_seconds: 1,
        ..Default::default()
    };
    let client = Phd2Client::with_connection_factory(config, factory);

    let result = client.connect().await;
    assert!(matches!(result, Err(Phd2Error::ConnectionFailed(_))));
    assert!(!client.is_connected().await);
}

#[tokio::test]
async fn test_connect_retries_on_failure() {
    let factory = Arc::new(MockConnectionFactory::new());
    factory.set_fail_connect(true);
    let factory_clone = factory.clone();

    let config = Phd2Config {
        connection_timeout_seconds: 1,
        ..Default::default()
    };
    let client = Phd2Client::with_connection_factory(config, factory);

    let _ = client.connect().await;
    assert_eq!(factory_clone.get_connect_count(), 1);
}

// ============================================================================
// Auto-reconnect configuration tests
// ============================================================================

#[tokio::test]
async fn test_auto_reconnect_default_enabled() {
    let factory = Arc::new(MockConnectionFactory::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    assert!(client.is_auto_reconnect_enabled());
}

#[tokio::test]
async fn test_auto_reconnect_can_be_disabled_in_config() {
    let factory = Arc::new(MockConnectionFactory::new());
    let config = Phd2Config {
        reconnect: ReconnectConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = Phd2Client::with_connection_factory(config, factory);

    assert!(!client.is_auto_reconnect_enabled());
}

#[tokio::test]
async fn test_auto_reconnect_toggle() {
    let factory = Arc::new(MockConnectionFactory::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    assert!(client.is_auto_reconnect_enabled());

    client.set_auto_reconnect_enabled(false);
    assert!(!client.is_auto_reconnect_enabled());

    client.set_auto_reconnect_enabled(true);
    assert!(client.is_auto_reconnect_enabled());
}

#[tokio::test]
async fn test_stop_reconnection_when_not_reconnecting() {
    let factory = Arc::new(MockConnectionFactory::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    // Should not panic or error when called without an active reconnection
    client.stop_reconnection().await;
    assert!(!client.is_reconnecting().await);
}

// ============================================================================
// Multiple connection tests
// ============================================================================

#[tokio::test]
async fn test_reconnect_after_disconnect() {
    let factory = Arc::new(MockConnectionFactory::new());
    factory.add_connection(vec![Some(version_event())]);
    factory.add_connection(vec![Some(version_event())]);

    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    // First connection
    client.connect().await.unwrap();
    assert!(client.is_connected().await);

    // Disconnect
    client.disconnect().await.unwrap();
    assert!(!client.is_connected().await);

    // Second connection
    client.connect().await.unwrap();
    assert!(client.is_connected().await);
}

#[tokio::test]
async fn test_connect_when_already_connected_aborts_previous() {
    let factory = Arc::new(MockConnectionFactory::new());
    factory.add_connection(vec![Some(version_event())]);
    factory.add_connection(vec![Some(version_event())]);
    let factory_clone = factory.clone();

    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    client.connect().await.unwrap();
    client.connect().await.unwrap();

    // Should have made 2 connection attempts
    assert_eq!(factory_clone.get_connect_count(), 2);
}

// ============================================================================
// Event subscription tests
// ============================================================================

#[tokio::test]
async fn test_subscribe_returns_receiver() {
    let factory = Arc::new(MockConnectionFactory::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    let _receiver = client.subscribe();
    // Just verify we can subscribe without panicking
}

#[tokio::test]
async fn test_multiple_subscribers() {
    let factory = Arc::new(MockConnectionFactory::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    let _receiver1 = client.subscribe();
    let _receiver2 = client.subscribe();
    let _receiver3 = client.subscribe();
    // Multiple subscribers should be allowed
}

// ============================================================================
// Cached app state tests
// ============================================================================

#[tokio::test]
async fn test_cached_app_state_initially_none() {
    let factory = Arc::new(MockConnectionFactory::new());
    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    assert!(client.get_cached_app_state().await.is_none());
}

#[tokio::test]
async fn test_cached_app_state_updated_by_event() {
    let factory = Arc::new(MockConnectionFactory::new());
    let app_state_event = r#"{"Event":"AppState","State":"Guiding"}"#.to_string();
    factory.add_connection(vec![Some(version_event()), Some(app_state_event)]);

    let config = Phd2Config::default();
    let client = Phd2Client::with_connection_factory(config, factory);

    client.connect().await.unwrap();
    // Wait for events to be processed
    tokio::time::sleep(Duration::from_millis(100)).await;

    let cached_state = client.get_cached_app_state().await;
    assert!(cached_state.is_some());
}
