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

use crate::error::{ConfigurationError, Result};
use crate::protocol::address::IndividualAddress;
use std::net::{IpAddr, SocketAddr};

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

impl ConnectionConfig {
    /// Validate the settings before opening a socket or starting tasks.
    ///
    /// # Errors
    /// Returns a configuration error if this mode has unsupported, missing, or
    /// inconsistent settings.
    pub fn validate(&self) -> Result<()> {
        if self.security.is_some() && self.connection_type != ConnectionType::SecureTunneling {
            return Err(ConfigurationError::ValidationError {
                details: "Security settings require SecureTunneling".to_owned(),
            }
            .into());
        }
        match self.connection_type {
            ConnectionType::Tunneling
            | ConnectionType::TcpTunneling
            | ConnectionType::SecureTunneling => {
                let gateway = self.gateway_addr()?;
                if let Some(local) = self.local_ip
                    && local.is_ipv4() != gateway.is_ipv4()
                {
                    return Err(ConfigurationError::ValidationError {
                        details: "The local address and gateway must use the same IP family"
                            .to_owned(),
                    }
                    .into());
                }
            }
            ConnectionType::Routing if self.local_ip.is_some_and(|ip| ip.is_ipv6()) => {
                return Err(ConfigurationError::ValidationError {
                    details: "Routing requires an IPv4 interface".to_owned(),
                }
                .into());
            }
            ConnectionType::SecureRouting => {
                return Err(ConfigurationError::ValidationError {
                    details: "SecureRouting (KNX IP Secure over multicast) is not yet implemented"
                        .to_owned(),
                }
                .into());
            }
            ConnectionType::Routing => {}
        }

        if self.connection_type == ConnectionType::SecureTunneling {
            if !cfg!(feature = "secure") {
                return Err(ConfigurationError::ValidationError {
                    details: "SecureTunneling requires the `secure` feature".to_owned(),
                }
                .into());
            }
            if self.security.is_none() {
                return Err(ConfigurationError::MissingParameter {
                    parameter: "security".to_owned(),
                }
                .into());
            }
        }

        let name = if self.connection_type.uses_tcp() {
            "tcp_config.connect_timeout_ms"
        } else {
            "timeout_ms"
        };
        let millis = self.tunnel_timeout_ms();
        if millis == 0 {
            return Err(ConfigurationError::InvalidValue {
                parameter: name.to_owned(),
                value: millis.to_string(),
                reason: "Timeout must be greater than 0".to_owned(),
            }
            .into());
        }
        Ok(())
    }

    pub(crate) fn gateway_addr(&self) -> Result<SocketAddr> {
        let ip = self
            .gateway_ip
            .ok_or_else(|| ConfigurationError::MissingParameter {
                parameter: "gateway_ip".to_owned(),
            })?;
        let port = self.gateway_port.unwrap_or(3671);
        if port == 0 || ip.is_unspecified() || ip.is_multicast() || ip == IpAddr::from([255; 4]) {
            return Err(ConfigurationError::InvalidValue {
                parameter: "gateway".to_owned(),
                value: SocketAddr::new(ip, port).to_string(),
                reason: "Expected a unicast gateway address and a nonzero port".to_owned(),
            }
            .into());
        }
        if self.connection_type == ConnectionType::Tunneling && ip.is_ipv6() {
            return Err(ConfigurationError::ValidationError {
                details: "UDP tunneling requires an IPv4 gateway".to_owned(),
            }
            .into());
        }
        Ok(SocketAddr::new(ip, port))
    }

    pub(crate) fn tunnel_timeout_ms(&self) -> u64 {
        if self.connection_type.uses_tcp() {
            self.tcp_config.connect_timeout_ms
        } else {
            self.timeout_ms
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

impl ConnectionType {
    fn uses_tcp(self) -> bool {
        matches!(self, Self::TcpTunneling | Self::SecureTunneling)
    }
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

impl SecurityConfig {
    /// Create the credentials for an authenticated tunnel.
    pub fn new(device_auth_password: impl Into<String>, user_password: impl Into<String>) -> Self {
        Self {
            device_auth_password: device_auth_password.into(),
            user_password: Some(user_password.into()),
            session_timeout: 60,
        }
    }
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
