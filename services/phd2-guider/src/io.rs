//! I/O traits and implementations for PHD2 client
//!
//! This module provides trait abstractions for line reading, message writing,
//! TCP connections, and process spawning. These traits enable mockall-based
//! testing without requiring actual network or process operations.
//!
//! The default implementations use TCP sockets and tokio processes for
//! production use.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tracing::debug;

use crate::error::{Phd2Error, Result};

/// Connection pair containing a reader and writer
pub struct ConnectionPair {
    /// Reader for receiving messages
    pub reader: Box<dyn LineReader>,
    /// Writer for sending messages
    pub writer: Box<dyn MessageWriter>,
}

// ============================================================================
// LineReader trait and implementations
// ============================================================================

/// Trait for reading lines from a connection
///
/// This trait abstracts the line reading functionality, enabling mocking
/// for tests that don't require actual network I/O.
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait LineReader: Send {
    /// Read a line from the connection
    ///
    /// Returns `Ok(Some(line))` if a line was read successfully,
    /// `Ok(None)` if the connection was closed (EOF),
    /// or an error if reading failed.
    async fn read_line(&mut self) -> Result<Option<String>>;
}

/// TCP implementation of LineReader using a buffered reader
pub struct TcpLineReader {
    reader: BufReader<ReadHalf<TcpStream>>,
    buffer: String,
}

impl TcpLineReader {
    /// Create a new TCP line reader from a read half of a TCP stream
    pub fn new(reader: ReadHalf<TcpStream>) -> Self {
        Self {
            reader: BufReader::new(reader),
            buffer: String::new(),
        }
    }
}

#[async_trait]
impl LineReader for TcpLineReader {
    async fn read_line(&mut self) -> Result<Option<String>> {
        self.buffer.clear();
        match self.reader.read_line(&mut self.buffer).await {
            Ok(0) => Ok(None), // EOF
            Ok(_) => Ok(Some(self.buffer.trim().to_string())),
            Err(e) => Err(Phd2Error::Io(e)),
        }
    }
}

// ============================================================================
// MessageWriter trait and implementations
// ============================================================================

/// Trait for writing messages to a connection
///
/// This trait abstracts the message writing functionality, enabling mocking
/// for tests that don't require actual network I/O.
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait MessageWriter: Send {
    /// Write a message to the connection
    ///
    /// The message is written with a CRLF terminator and flushed.
    async fn write_message(&mut self, message: &str) -> Result<()>;

    /// Shutdown the writer
    async fn shutdown(&mut self) -> Result<()>;
}

/// TCP implementation of MessageWriter
pub struct TcpMessageWriter {
    writer: WriteHalf<TcpStream>,
}

impl TcpMessageWriter {
    /// Create a new TCP message writer from a write half of a TCP stream
    pub fn new(writer: WriteHalf<TcpStream>) -> Self {
        Self { writer }
    }
}

#[async_trait]
impl MessageWriter for TcpMessageWriter {
    async fn write_message(&mut self, message: &str) -> Result<()> {
        self.writer
            .write_all(format!("{}\r\n", message).as_bytes())
            .await
            .map_err(|e| Phd2Error::SendError(e.to_string()))?;
        self.writer
            .flush()
            .await
            .map_err(|e| Phd2Error::SendError(e.to_string()))?;
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.writer.shutdown().await.map_err(Phd2Error::Io)
    }
}

// ============================================================================
// ConnectionFactory trait and implementations
// ============================================================================

/// Trait for creating connections
///
/// This trait abstracts TCP connection creation, enabling mocking
/// for tests that don't require actual network operations.
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait ConnectionFactory: Send + Sync {
    /// Attempt to connect to the specified address
    ///
    /// Returns a connection pair (reader and writer) on success.
    async fn connect(&self, addr: &str, timeout: Duration) -> Result<ConnectionPair>;

    /// Check if a connection can be established to the specified address
    ///
    /// This is a quick connectivity check used by process management.
    async fn can_connect(&self, addr: &str) -> bool;
}

/// TCP implementation of ConnectionFactory
#[derive(Default, Clone)]
pub struct TcpConnectionFactory;

impl TcpConnectionFactory {
    /// Create a new TCP connection factory
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ConnectionFactory for TcpConnectionFactory {
    async fn connect(&self, addr: &str, timeout: Duration) -> Result<ConnectionPair> {
        debug!("Connecting to {} with timeout {:?}", addr, timeout);

        let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| Phd2Error::Timeout(format!("Connection to {} timed out", addr)))?
            .map_err(|e| {
                Phd2Error::ConnectionFailed(format!("Failed to connect to {}: {}", addr, e))
            })?;

        debug!("TCP connection established to {}", addr);

        let (reader, writer) = tokio::io::split(stream);

        Ok(ConnectionPair {
            reader: Box::new(TcpLineReader::new(reader)),
            writer: Box::new(TcpMessageWriter::new(writer)),
        })
    }

    async fn can_connect(&self, addr: &str) -> bool {
        TcpStream::connect(addr).await.is_ok()
    }
}

// ============================================================================
// ProcessHandle trait and implementations
// ============================================================================

/// Trait for interacting with a spawned process
///
/// This trait abstracts process handle operations, enabling mocking
/// for tests that don't require actual process spawning.
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait ProcessHandle: Send {
    /// Check if the process has exited without waiting
    ///
    /// Returns `Ok(Some(exit_code))` if the process has exited,
    /// `Ok(None)` if it's still running, or an error.
    async fn try_wait(&mut self) -> Result<Option<i32>>;

    /// Kill the process
    async fn kill(&mut self) -> Result<()>;

    /// Wait for the process to exit and return its exit code
    async fn wait(&mut self) -> Result<i32>;

    /// Get the process ID if available
    fn id(&self) -> Option<u32>;
}

/// Tokio process handle implementation
pub struct TokioProcessHandle {
    child: Child,
}

impl TokioProcessHandle {
    /// Create a new process handle from a tokio Child
    pub fn new(child: Child) -> Self {
        Self { child }
    }
}

#[async_trait]
impl ProcessHandle for TokioProcessHandle {
    async fn try_wait(&mut self) -> Result<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Ok(Some(status.code().unwrap_or(-1))),
            Ok(None) => Ok(None),
            Err(e) => Err(Phd2Error::Io(e)),
        }
    }

    async fn kill(&mut self) -> Result<()> {
        self.child.kill().await.map_err(Phd2Error::Io)
    }

    async fn wait(&mut self) -> Result<i32> {
        let status = self.child.wait().await.map_err(Phd2Error::Io)?;
        Ok(status.code().unwrap_or(-1))
    }

    fn id(&self) -> Option<u32> {
        self.child.id()
    }
}

// ============================================================================
// ProcessSpawner trait and implementations
// ============================================================================

/// Trait for spawning processes
///
/// This trait abstracts process spawning, enabling mocking for tests
/// that don't require actual process operations.
#[async_trait]
#[cfg_attr(test, mockall::automock)]
pub trait ProcessSpawner: Send + Sync {
    /// Spawn a process with the given executable and environment variables
    async fn spawn(
        &self,
        executable: &Path,
        env: &HashMap<String, String>,
    ) -> Result<Box<dyn ProcessHandle>>;
}

/// Tokio implementation of ProcessSpawner
#[derive(Default, Clone)]
pub struct TokioProcessSpawner;

impl TokioProcessSpawner {
    /// Create a new tokio process spawner
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ProcessSpawner for TokioProcessSpawner {
    async fn spawn(
        &self,
        executable: &Path,
        env: &HashMap<String, String>,
    ) -> Result<Box<dyn ProcessHandle>> {
        debug!("Spawning process: {}", executable.display());

        let mut cmd = Command::new(executable);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        for (key, value) in env {
            debug!("Setting environment variable: {}={}", key, value);
            cmd.env(key, value);
        }

        let child = cmd.spawn().map_err(|e| {
            Phd2Error::ProcessStartFailed(format!(
                "Failed to start {}: {}",
                executable.display(),
                e
            ))
        })?;

        debug!("Process started with PID: {:?}", child.id());

        Ok(Box::new(TokioProcessHandle::new(child)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tcp_connection_factory_new() {
        let factory = TcpConnectionFactory::new();
        // Just verify we can create an instance
        let _ = factory;
    }

    #[test]
    fn test_tcp_connection_factory_default() {
        let factory = TcpConnectionFactory::default();
        // Just verify we can create an instance
        let _ = factory;
    }

    #[test]
    fn test_tokio_process_spawner_new() {
        let spawner = TokioProcessSpawner::new();
        // Just verify we can create an instance
        let _ = spawner;
    }

    #[test]
    fn test_tokio_process_spawner_default() {
        let spawner = TokioProcessSpawner::default();
        // Just verify we can create an instance
        let _ = spawner;
    }
}
