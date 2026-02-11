//! Mockall-based tests for process management
//!
//! These tests use mockall to mock process spawning and connection checks,
//! enabling testing of process management without actual process operations.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use phd2_guider::io::{ConnectionFactory, ConnectionPair, ProcessHandle, ProcessSpawner};
use phd2_guider::{Phd2Config, Phd2Error, Phd2ProcessManager};

// ============================================================================
// Mock implementations
// ============================================================================

/// Mock process handle for testing
struct MockProcessHandle {
    running: StdMutex<bool>,
    exit_code: i32,
    pid: Option<u32>,
    kill_error: bool,
}

impl MockProcessHandle {
    fn new_running() -> Self {
        Self {
            running: StdMutex::new(true),
            exit_code: 0,
            pid: Some(12345),
            kill_error: false,
        }
    }

    fn new_exited(exit_code: i32) -> Self {
        Self {
            running: StdMutex::new(false),
            exit_code,
            pid: Some(12345),
            kill_error: false,
        }
    }
}

#[async_trait]
impl ProcessHandle for MockProcessHandle {
    async fn try_wait(&mut self) -> phd2_guider::Result<Option<i32>> {
        if *self.running.lock().unwrap() {
            Ok(None)
        } else {
            Ok(Some(self.exit_code))
        }
    }

    async fn kill(&mut self) -> phd2_guider::Result<()> {
        if self.kill_error {
            Err(Phd2Error::Io(std::io::Error::other("Mock kill error")))
        } else {
            *self.running.lock().unwrap() = false;
            Ok(())
        }
    }

    async fn wait(&mut self) -> phd2_guider::Result<i32> {
        *self.running.lock().unwrap() = false;
        Ok(self.exit_code)
    }

    fn id(&self) -> Option<u32> {
        self.pid
    }
}

/// Mock process spawner
struct MockProcessSpawner {
    spawn_results: StdMutex<Vec<Result<Box<dyn ProcessHandle>, String>>>,
    spawn_count: StdMutex<u32>,
    spawned_executables: StdMutex<Vec<String>>,
    spawned_envs: StdMutex<Vec<HashMap<String, String>>>,
}

impl MockProcessSpawner {
    fn new() -> Self {
        Self {
            spawn_results: StdMutex::new(Vec::new()),
            spawn_count: StdMutex::new(0),
            spawned_executables: StdMutex::new(Vec::new()),
            spawned_envs: StdMutex::new(Vec::new()),
        }
    }

    fn add_spawn_success(&self) {
        self.spawn_results
            .lock()
            .unwrap()
            .push(Ok(Box::new(MockProcessHandle::new_running())));
    }

    fn add_spawn_failure(&self, message: &str) {
        self.spawn_results
            .lock()
            .unwrap()
            .push(Err(message.to_string()));
    }

    fn add_process_exits_immediately(&self, exit_code: i32) {
        self.spawn_results
            .lock()
            .unwrap()
            .push(Ok(Box::new(MockProcessHandle::new_exited(exit_code))));
    }

    fn get_spawn_count(&self) -> u32 {
        *self.spawn_count.lock().unwrap()
    }

    fn get_spawned_executables(&self) -> Vec<String> {
        self.spawned_executables.lock().unwrap().clone()
    }

    fn get_spawned_envs(&self) -> Vec<HashMap<String, String>> {
        self.spawned_envs.lock().unwrap().clone()
    }
}

#[async_trait]
impl ProcessSpawner for MockProcessSpawner {
    async fn spawn(
        &self,
        executable: &Path,
        env: &HashMap<String, String>,
    ) -> phd2_guider::Result<Box<dyn ProcessHandle>> {
        *self.spawn_count.lock().unwrap() += 1;
        self.spawned_executables
            .lock()
            .unwrap()
            .push(executable.display().to_string());
        self.spawned_envs.lock().unwrap().push(env.clone());

        let mut results = self.spawn_results.lock().unwrap();
        if results.is_empty() {
            Err(Phd2Error::ProcessStartFailed(
                "No mock spawn results available".to_string(),
            ))
        } else {
            let result = results.remove(0);
            match result {
                Ok(handle) => Ok(handle),
                Err(msg) => Err(Phd2Error::ProcessStartFailed(msg)),
            }
        }
    }
}

/// Mock connection factory for testing process manager
struct MockConnectionFactory {
    can_connect_results: StdMutex<Vec<bool>>,
    default_can_connect: bool,
}

impl MockConnectionFactory {
    fn new() -> Self {
        Self {
            can_connect_results: StdMutex::new(Vec::new()),
            default_can_connect: false,
        }
    }

    fn set_can_connect(&self, can_connect: bool) {
        self.can_connect_results.lock().unwrap().push(can_connect);
    }

    fn always_connectable() -> Self {
        let mut factory = Self::new();
        factory.default_can_connect = true;
        factory
    }

    fn never_connectable() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConnectionFactory for MockConnectionFactory {
    async fn connect(
        &self,
        _addr: &str,
        _timeout: Duration,
    ) -> phd2_guider::Result<ConnectionPair> {
        // This is not used by process manager, but required by the trait
        Err(Phd2Error::ConnectionFailed(
            "Not implemented for mock".to_string(),
        ))
    }

    async fn can_connect(&self, _addr: &str) -> bool {
        let mut results = self.can_connect_results.lock().unwrap();
        if results.is_empty() {
            self.default_can_connect
        } else {
            results.remove(0)
        }
    }
}

/// Path to the mock_phd2 binary built by cargo
fn mock_phd2_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_mock_phd2"))
}

fn create_test_config() -> Phd2Config {
    // Use the mock_phd2 binary as the executable path.
    // The actual command won't be run because we're using a mock spawner.
    Phd2Config {
        host: "localhost".to_string(),
        port: 4400,
        executable_path: Some(mock_phd2_path()),
        connection_timeout_seconds: 1,
        ..Default::default()
    }
}

// ============================================================================
// Basic process manager tests
// ============================================================================

#[tokio::test]
async fn test_process_manager_creation() {
    let spawner = Arc::new(MockProcessSpawner::new());
    let factory = Arc::new(MockConnectionFactory::new());
    let config = create_test_config();

    let _manager = Phd2ProcessManager::with_spawner(config, spawner, factory);
    // Just verify creation succeeds
}

#[tokio::test]
async fn test_has_managed_process_initially_false() {
    let spawner = Arc::new(MockProcessSpawner::new());
    let factory = Arc::new(MockConnectionFactory::new());
    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner, factory);
    assert!(!manager.has_managed_process().await);
}

// ============================================================================
// is_phd2_running tests
// ============================================================================

#[tokio::test]
async fn test_is_phd2_running_when_connectable() {
    let spawner = Arc::new(MockProcessSpawner::new());
    let factory = Arc::new(MockConnectionFactory::always_connectable());
    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner, factory);
    assert!(manager.is_phd2_running().await);
}

#[tokio::test]
async fn test_is_phd2_running_when_not_connectable() {
    let spawner = Arc::new(MockProcessSpawner::new());
    let factory = Arc::new(MockConnectionFactory::never_connectable());
    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner, factory);
    assert!(!manager.is_phd2_running().await);
}

// ============================================================================
// start_phd2 tests
// ============================================================================

#[tokio::test]
async fn test_start_phd2_when_already_running() {
    let spawner = Arc::new(MockProcessSpawner::new());
    let factory = Arc::new(MockConnectionFactory::always_connectable());
    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner.clone(), factory);

    // Should return Ok without spawning since PHD2 is "already running"
    manager.start_phd2().await.unwrap();
    assert_eq!(spawner.get_spawn_count(), 0);
}

#[tokio::test]
async fn test_start_phd2_spawn_failure() {
    let spawner = Arc::new(MockProcessSpawner::new());
    spawner.add_spawn_failure("Mock spawn failure");

    let factory = Arc::new(MockConnectionFactory::never_connectable());
    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner, factory);

    let result = manager.start_phd2().await;
    assert!(matches!(result, Err(Phd2Error::ProcessStartFailed(_))));
}

#[tokio::test]
async fn test_start_phd2_process_exits_prematurely() {
    let spawner = Arc::new(MockProcessSpawner::new());
    spawner.add_process_exits_immediately(1);

    let factory = Arc::new(MockConnectionFactory::never_connectable());
    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner, factory);

    let result = manager.start_phd2().await;
    assert!(matches!(result, Err(Phd2Error::ProcessStartFailed(_))));
}

#[tokio::test]
async fn test_start_phd2_passes_environment() {
    let spawner = Arc::new(MockProcessSpawner::new());
    spawner.add_spawn_success();

    let factory = Arc::new(MockConnectionFactory::new());
    // First check: not running, then after spawn: running
    factory.set_can_connect(false);
    factory.set_can_connect(true);

    let mut config = create_test_config();
    config
        .spawn_env
        .insert("DISPLAY".to_string(), ":0".to_string());
    config
        .spawn_env
        .insert("TEST_VAR".to_string(), "test_value".to_string());

    let manager = Phd2ProcessManager::with_spawner(config, spawner.clone(), factory);

    manager.start_phd2().await.unwrap();

    let envs = spawner.get_spawned_envs();
    assert_eq!(envs.len(), 1);
    assert_eq!(envs[0].get("DISPLAY"), Some(&":0".to_string()));
    assert_eq!(envs[0].get("TEST_VAR"), Some(&"test_value".to_string()));
}

#[tokio::test]
async fn test_start_phd2_uses_configured_executable() {
    let spawner = Arc::new(MockProcessSpawner::new());
    spawner.add_spawn_success();

    let factory = Arc::new(MockConnectionFactory::new());
    factory.set_can_connect(false);
    factory.set_can_connect(true);

    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner.clone(), factory);

    manager.start_phd2().await.unwrap();

    let executables = spawner.get_spawned_executables();
    assert_eq!(executables.len(), 1);
    // Uses the configured mock_phd2 path
    assert_eq!(executables[0], mock_phd2_path().display().to_string());
}

#[tokio::test]
async fn test_start_phd2_timeout_waiting_for_ready() {
    let spawner = Arc::new(MockProcessSpawner::new());
    spawner.add_spawn_success();

    // Never becomes connectable
    let factory = Arc::new(MockConnectionFactory::never_connectable());

    let mut config = create_test_config();
    config.connection_timeout_seconds = 1; // Short timeout for test

    let manager = Phd2ProcessManager::with_spawner(config, spawner, factory);

    let result = manager.start_phd2().await;
    assert!(matches!(result, Err(Phd2Error::Timeout(_))));
}

#[tokio::test]
async fn test_start_phd2_sets_managed_process() {
    let spawner = Arc::new(MockProcessSpawner::new());
    spawner.add_spawn_success();

    let factory = Arc::new(MockConnectionFactory::new());
    factory.set_can_connect(false);
    factory.set_can_connect(true);

    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner, factory);

    manager.start_phd2().await.unwrap();
    assert!(manager.has_managed_process().await);
}

#[tokio::test]
async fn test_start_phd2_already_has_managed_process() {
    let spawner = Arc::new(MockProcessSpawner::new());
    spawner.add_spawn_success();
    spawner.add_spawn_success();

    let factory = Arc::new(MockConnectionFactory::new());
    // First start: not running -> running
    factory.set_can_connect(false);
    factory.set_can_connect(true);
    // Second start attempt: not running
    factory.set_can_connect(false);

    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner.clone(), factory);

    // First start succeeds
    manager.start_phd2().await.unwrap();

    // Second start should fail because we already have a managed process
    let result = manager.start_phd2().await;
    assert!(matches!(result, Err(Phd2Error::ProcessAlreadyRunning)));

    // Only one spawn should have occurred
    assert_eq!(spawner.get_spawn_count(), 1);
}

// ============================================================================
// stop_phd2 tests
// ============================================================================

#[tokio::test]
async fn test_stop_phd2_no_managed_process() {
    let spawner = Arc::new(MockProcessSpawner::new());
    let factory = Arc::new(MockConnectionFactory::new());
    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner, factory);

    // Should succeed even with no managed process
    manager.stop_phd2(None).await.unwrap();
}

#[tokio::test]
async fn test_stop_phd2_without_client() {
    let spawner = Arc::new(MockProcessSpawner::new());
    spawner.add_spawn_success();

    let factory = Arc::new(MockConnectionFactory::new());
    factory.set_can_connect(false);
    factory.set_can_connect(true);

    let config = create_test_config();

    let manager = Phd2ProcessManager::with_spawner(config, spawner, factory);

    manager.start_phd2().await.unwrap();
    assert!(manager.has_managed_process().await);

    // Stop without client - should force kill
    manager.stop_phd2(None).await.unwrap();
    assert!(!manager.has_managed_process().await);
}
