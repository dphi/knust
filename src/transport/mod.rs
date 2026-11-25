//! Transport layer for KNX/IP communication.
//!
//! This module provides the low-level networking functionality for KNX/IP
//! communication, including UDP socket management, connection establishment,
//! and data transmission/reception.

pub mod address_probe;
pub mod address_registry;
pub mod backoff;
pub mod connection;
pub mod device_management;
pub mod discovery;
pub mod frame_transport;
pub mod health;
pub mod heartbeat;
pub mod queue;
pub mod rate_limit;
pub mod receive_limiter;
pub mod router;
pub mod routing;
#[cfg(feature = "server")]
pub mod server;
pub mod tunnel;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tunneling_test;

#[cfg(test)]
mod integration_tests;

use crate::protocol::address::IndividualAddress;
use std::net::IpAddr;

pub use address_probe::{auto_select_address, probe_address};
pub use address_registry::AddressRegistry;
pub use backoff::JitteredBackoff;
pub use connection::{Connection, ConnectionStats, TransportState};
pub use device_management::DeviceManagementConnection;
pub use discovery::{
    GatewayCapabilities, GatewayInfo, GatewayScanner, GatewayTunnelingInfo, ServiceFamily,
    TunnelingSlotStatus,
};
pub use frame_transport::{FrameTransport, TcpFrameTransport, TransportKind, UdpFrameTransport};
pub use health::ConnectionHealth;
pub use queue::{QueueConfig, QueueStats, TelegramQueue};
pub use rate_limit::{RateLimitConfig, RateLimiter};
pub use receive_limiter::{ReceiveLimitConfig, ReceiveRateLimiter, ReceiveResult, ReceiveStats};
pub use router::FrameRouter;
pub use routing::RoutingConnection;
#[cfg(feature = "server")]
pub use server::{DeviceManagementHandler, TunnelServer};
pub use tunnel::{SequenceValidationResult, Tunnel};

/// Configuration for KNX/IP connections
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// Type of connection to establish
    pub connection_type: ConnectionType,

    /// Gateway IP address (required for tunneling)
    pub gateway_ip: Option<IpAddr>,

    /// Gateway port (defaults to 3671 for KNX/IP)
    pub gateway_port: Option<u16>,

    /// Local IP address to bind to
    pub local_ip: Option<IpAddr>,

    /// Individual address for this client
    pub individual_address: IndividualAddress,

    /// Security configuration
    pub security: Option<SecurityConfig>,

    /// Connection timeout in milliseconds
    pub timeout_ms: u64,

    /// Enable automatic reconnection
    pub auto_reconnect: bool,

    /// Reconnection backoff settings
    pub reconnect_backoff: BackoffConfig,

    /// TCP-specific configuration
    pub tcp_config: TcpConfig,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            connection_type: ConnectionType::Tunneling,
            gateway_ip: None,
            gateway_port: Some(3671), // Standard KNX/IP port
            local_ip: None,
            individual_address: IndividualAddress::new(1, 1, 240), // Default client address
            security: None,
            timeout_ms: 5000,
            auto_reconnect: true,
            reconnect_backoff: BackoffConfig::default(),
            tcp_config: TcpConfig::default(),
        }
    }
}

/// Types of KNX/IP connections
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionType {
    /// Point-to-point tunneling connection (UDP)
    Tunneling,

    /// Point-to-point tunneling connection (TCP)
    TcpTunneling,

    /// Multicast routing connection
    Routing,

    /// Secure tunneling connection
    SecureTunneling,

    /// Secure routing connection
    SecureRouting,
}

/// Security configuration for secure connections.
#[derive(Clone)]
pub struct SecurityConfig {
    /// Device authentication password (plaintext) — e.g. the FDSK printed on
    /// the device (strip the grouping hyphens/spaces first), or a custom
    /// device authentication password set via ETS. Derived via PBKDF2 the
    /// same way as `user_password` — pass the password as-is, not pre-derived
    /// key bytes.
    pub device_auth_password: String,

    /// User password (plaintext).
    pub user_password: Option<String>,

    /// Security session timeout in seconds
    pub session_timeout: u32,
}

impl std::fmt::Debug for SecurityConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityConfig")
            .field("has_user_password", &self.user_password.is_some())
            .field("session_timeout", &self.session_timeout)
            .finish_non_exhaustive()
    }
}

/// Backoff configuration for reconnection attempts
#[derive(Debug, Clone)]
pub struct BackoffConfig {
    /// Initial backoff delay in milliseconds
    pub initial_delay_ms: u64,

    /// Maximum backoff delay in milliseconds
    pub max_delay_ms: u64,

    /// Backoff multiplier
    pub multiplier: f64,

    /// Maximum number of retry attempts
    pub max_attempts: u32,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            multiplier: 2.0,
            max_attempts: 10,
        }
    }
}

/// TCP-specific configuration options
#[derive(Debug, Clone)]
pub struct TcpConfig {
    /// TCP connection timeout in milliseconds
    pub connect_timeout_ms: u64,
}

impl Default for TcpConfig {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 5000,
        }
    }
}
