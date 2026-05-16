//! Mock serial port for testing without real hardware.
//!
//! Feature-gated under `mock`. Phase 3b fills in the deterministic state
//! machine that emits canned Falcon responses to the BDD scenarios; Phase 2
//! only needs the symbol so the binary builds with `--features mock`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::error::Result;
use crate::io::{SerialPair, SerialPortFactory};

/// Internal state shared by the mock reader and writer.
#[derive(Debug, Default)]
struct MockState;

/// Mock serial port factory.
///
/// Maintains persistent state across multiple `open` cycles so the Phase 3
/// state machine can model "device state persists across reconnects".
#[derive(Clone, Default)]
pub struct MockSerialPortFactory {
    _state: Arc<Mutex<MockState>>,
}

#[async_trait]
impl SerialPortFactory for MockSerialPortFactory {
    async fn open(&self, _port: &str, _baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        unimplemented!("MockSerialPortFactory::open is implemented in Phase 3b")
    }

    async fn port_exists(&self, _port: &str) -> bool {
        true
    }
}
