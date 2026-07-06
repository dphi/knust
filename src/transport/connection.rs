//! Core connection trait and state management.

use crate::error::Result;
use async_trait::async_trait;
use std::fmt;

/// Trait for KNX/IP connections
#[async_trait]
pub trait Connection: Send + Sync {
    /// Send raw frame data
    async fn send(&self, frame: &[u8]) -> Result<()>;

    /// Receive raw frame data
    async fn recv(&self) -> Result<Vec<u8>>;

    /// Close the connection
    async fn close(&self) -> Result<()>;

    /// Get current connection state
    fn state(&self) -> ConnectionState;

    /// Get connection statistics
    fn stats(&self) -> ConnectionStats;

    /// Get reference to underlying concrete type (for downcasting)
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Connection state enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Connection is being established
    Connecting,

    /// Connection is active and ready
    Connected,

    /// Connection is being closed
    Disconnecting,

    /// Connection is closed
    Disconnected,

    /// Connection failed
    Failed,
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectionState::Connecting => write!(f, "connecting"),
            ConnectionState::Connected => write!(f, "connected"),
            ConnectionState::Disconnecting => write!(f, "disconnecting"),
            ConnectionState::Disconnected => write!(f, "disconnected"),
            ConnectionState::Failed => write!(f, "failed"),
        }
    }
}

/// Connection statistics
#[derive(Debug, Clone, Default)]
pub struct ConnectionStats {
    /// Number of frames sent
    pub frames_sent: u64,

    /// Number of frames received
    pub frames_received: u64,

    /// Number of send errors
    pub send_errors: u64,

    /// Number of receive errors
    pub recv_errors: u64,

    /// Connection uptime in seconds
    pub uptime_seconds: u64,

    /// Last error message
    pub last_error: Option<String>,
}
