//! Error types and result aliases for the Knx library.
//!
//! This module provides a comprehensive error hierarchy that covers all
//! possible failure modes in KNX/IP communication, from transport-level
//! network errors to application-level device operation failures.

use thiserror::Error;

#[cfg(test)]
mod tests;

/// Main result type used throughout the Knx library
pub type Result<T> = std::result::Result<T, KnxError>;

/// Root error type for all Knx operations
#[derive(Debug, Error)]
pub enum KnxError {
    /// Transport layer errors (network connectivity, socket operations)
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),

    /// Protocol layer errors (frame parsing, CEMI handling)
    #[error("Protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    /// Device layer errors (device operations, state management)
    #[error("Device error: {0}")]
    Device(#[from] DeviceError),

    /// Security layer errors (authentication, encryption)
    #[error("Security error: {0}")]
    Security(#[from] SecurityError),

    /// Configuration errors (invalid settings, missing parameters)
    #[error("Configuration error: {0}")]
    Configuration(#[from] ConfigurationError),

    /// Discovery errors (gateway scanning, device enumeration)
    #[error("Discovery error: {0}")]
    Discovery(#[from] DiscoveryError),

    /// Address validation errors
    #[error("Address error: {0}")]
    Address(#[from] crate::protocol::address::AddressError),
}

/// Transport layer error types
#[derive(Debug, Error)]
pub enum TransportError {
    /// Network connection failed
    #[error("Connection failed to {address}: {source}")]
    ConnectionFailed {
        address: String,
        #[source]
        source: std::io::Error,
    },

    /// Socket operation failed
    #[error("Socket operation failed: {operation} - {source}")]
    SocketError {
        operation: String,
        #[source]
        source: std::io::Error,
    },

    /// Connection timeout
    #[error("Connection timeout after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// Connection was closed unexpectedly
    #[error("Connection closed unexpectedly")]
    ConnectionClosed,

    /// Invalid connection configuration
    #[error("Invalid connection configuration: {details}")]
    InvalidConfiguration { details: String },

    /// Queue is full (backpressure)
    #[error("Queue is full, cannot enqueue more telegrams")]
    QueueFull,

    /// Queue is closed
    #[error("Queue is closed, no more operations allowed")]
    QueueClosed,

    /// Queue processing timeout
    #[error("Queue processing timeout after {timeout_ms}ms")]
    QueueTimeout { timeout_ms: u64 },
}

/// Protocol layer error types
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Frame parsing failed
    #[error("Frame parsing failed at offset {offset}: {reason}")]
    ParseError { offset: usize, reason: String },

    /// Invalid frame format
    #[error("Invalid frame format: {details}")]
    InvalidFrame { details: String },

    /// Unsupported protocol version
    #[error("Unsupported protocol version: {version}")]
    UnsupportedVersion { version: u8 },

    /// CEMI message handling error
    #[error("CEMI error: {message}")]
    CemiError { message: String },

    /// Data Point Type (DPT) error
    #[error("DPT error: {dpt_type} - {details}")]
    DptError { dpt_type: String, details: String },

    /// Invalid address format
    #[error("Invalid address format: {address} - {reason}")]
    InvalidAddress { address: String, reason: String },

    /// Frame is not a telegram (e.g., control frame)
    #[error("Frame is not a telegram: {service_type}")]
    NotATelegram { service_type: String },
}

/// Errors from communicating with a specific address on the bus (e.g. a
/// timed-out `read_group_value`).
#[derive(Debug, Error)]
pub enum DeviceError {
    /// Communication timeout
    #[error("Communication timeout for {device} after {timeout_ms}ms")]
    CommunicationTimeout { device: String, timeout_ms: u64 },
}

/// Security layer error types
#[derive(Debug, Error)]
pub enum SecurityError {
    /// Authentication failed
    #[error("Authentication failed: {reason}")]
    AuthenticationFailed { reason: String },

    /// Invalid security credentials
    #[error("Invalid security credentials: {details}")]
    InvalidCredentials { details: String },

    /// Encryption/decryption failed
    #[error("Cryptographic operation failed: {operation} - {reason}")]
    CryptographicError { operation: String, reason: String },

    /// Security session expired
    #[error("Security session expired")]
    SessionExpired,

    /// Invalid security configuration
    #[error("Invalid security configuration: {details}")]
    InvalidConfiguration { details: String },
}

/// Configuration error types
#[derive(Debug, Error)]
pub enum ConfigurationError {
    /// Missing required configuration parameter
    #[error("Missing required configuration parameter: {parameter}")]
    MissingParameter { parameter: String },

    /// Invalid configuration value
    #[error("Invalid configuration value for {parameter}: {value} - {reason}")]
    InvalidValue {
        parameter: String,
        value: String,
        reason: String,
    },

    /// Configuration file parsing failed
    #[error("Configuration file parsing failed: {file} - {reason}")]
    ParseError { file: String, reason: String },

    /// Configuration validation failed
    #[error("Configuration validation failed: {details}")]
    ValidationError { details: String },
}

/// Discovery error types
#[derive(Debug, Error)]
pub enum DiscoveryError {
    /// No gateways found during discovery
    #[error("No gateways found during discovery")]
    NoGatewaysFound,

    /// Discovery timeout
    #[error("Discovery timeout after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// Invalid discovery response
    #[error("Invalid discovery response from {addr}: {reason}")]
    InvalidResponse { addr: String, reason: String },

    /// Network error during discovery
    #[error("Network error during discovery: {0}")]
    NetworkError(#[from] std::io::Error),
}

impl KnxError {
    /// Returns true if this error is recoverable (e.g., temporary network issues)
    #[must_use]
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            KnxError::Transport(TransportError::Timeout { .. } | TransportError::ConnectionClosed)
                | KnxError::Discovery(DiscoveryError::Timeout { .. })
                | KnxError::Security(SecurityError::SessionExpired)
        )
    }

    /// Returns the error category for logging and metrics
    #[must_use]
    pub fn category(&self) -> &'static str {
        match self {
            KnxError::Transport(_) => "transport",
            KnxError::Protocol(_) => "protocol",
            KnxError::Device(_) => "device",
            KnxError::Security(_) => "security",
            KnxError::Configuration(_) => "configuration",
            KnxError::Discovery(_) => "discovery",
            KnxError::Address(_) => "address",
        }
    }

    /// Returns detailed context information for debugging
    #[must_use]
    pub fn context(&self) -> String {
        match self {
            KnxError::Transport(e) => format!("Transport layer: {e}"),
            KnxError::Protocol(e) => format!("Protocol layer: {e}"),
            KnxError::Device(e) => format!("Device layer: {e}"),
            KnxError::Security(e) => format!("Security layer: {e}"),
            KnxError::Configuration(e) => format!("Configuration: {e}"),
            KnxError::Discovery(e) => format!("Discovery: {e}"),
            KnxError::Address(e) => format!("Address: {e}"),
        }
    }
}
