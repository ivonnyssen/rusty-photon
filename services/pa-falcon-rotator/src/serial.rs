//! Serial port implementation using tokio-serial
//!
//! Concrete `SerialPortFactory` driving real hardware over `tokio-serial`.
//! Phase 3 fills in the actual open / read / write logic; Phase 2 only needs
//! the symbol to exist so `ServerBuilder::default()` compiles.

use std::time::Duration;

use async_trait::async_trait;

use crate::error::Result;
use crate::io::{SerialPair, SerialPortFactory};

/// Serial port factory backed by `tokio-serial`.
#[derive(Default, Clone)]
pub struct TokioSerialPortFactory;

impl TokioSerialPortFactory {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SerialPortFactory for TokioSerialPortFactory {
    async fn open(&self, _port: &str, _baud_rate: u32, _timeout: Duration) -> Result<SerialPair> {
        unimplemented!("TokioSerialPortFactory::open is implemented in Phase 3b")
    }

    async fn port_exists(&self, port: &str) -> bool {
        std::path::Path::new(port).exists()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_factory_constructors() {
        let _new = TokioSerialPortFactory::new();
        let _default = TokioSerialPortFactory;
        let _cloned = TokioSerialPortFactory.clone();
    }

    #[tokio::test]
    async fn test_port_exists_nonexistent() {
        let factory = TokioSerialPortFactory::new();
        assert!(!factory.port_exists("/dev/nonexistent_falcon_12345").await);
    }

    #[tokio::test]
    async fn test_port_exists_known_path() {
        let factory = TokioSerialPortFactory::new();
        let path = std::env::current_exe().unwrap();
        assert!(factory.port_exists(path.to_str().unwrap()).await);
    }
}
