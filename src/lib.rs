//! # knust — Asynchronous KNX/IP Library for Rust
//!
//! knust is a high-performance, memory-safe implementation of the KNX/IP protocol
//! for building automation systems. It provides async/await support and strong
//! type safety while maintaining compatibility with KNX standards.
//!
//! ## Features
//!
//! - Async/await support with tokio
//! - Memory-safe protocol parsing
//! - Support for tunneling and routing connections
//! - Comprehensive error handling
//! - Property-based testing for correctness
//!
//! ## Cargo features
//!
//! - `dpt` (**on** by default) — typed codecs for KNX datapoints.
//!   Disable for a raw-frame-only client that never interprets group values;
//!   owns the `strum` dependency.
//! - `ets` (off) — ETS CSV group-address import (`parse_ets_csv`). The only
//!   consumer of tokio's `fs`. Implies `dpt` (DPT string parsing).
//! - `server` (off) — act as a KNXnet/IP tunneling server (`TunnelServer`).
//!   Most consumers are clients and don't need it.
//! - `secure` (off) — KNX IP Secure + KNX Data Security: session handshake,
//!   group encryption, `.knxkeys` keyring parsing/validation, and the secure
//!   server path. Pulls in the crypto stack (`aes`, `x25519-dalek`, `pbkdf2`,
//!   `sha2`, …). Implies `ets`.
//!   KNX IP Secure handshakes have been verified against real hardware.
//!   Data Secure group encryption and replay protection are tested locally.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use knust::Knx;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let knx = Knx::builder()
//!         .tunnel("192.168.1.100:3671".parse()?)
//!         .individual_address("1.2.3".parse()?)
//!         .connect()
//!         .await?;
//!     knx.shutdown().await?;
//!     Ok(())
//! }
//! ```

// `clippy::pedantic` lints that are noise for a byte-level KNX/IP protocol
// library. These are deliberate, not oversights:
// - casts: protocol code packs/unpacks bytes and scales fixed-point values;
//   `try_from` would inject failure paths for values already masked/bounded.
// - too_many_lines / struct_excessive_bools: size heuristics; splitting the
//   protocol state machines or replacing config bools would churn the API
//   without making it clearer.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines,
    clippy::struct_excessive_bools
)]

pub mod application;
pub mod config;
pub mod error;
pub mod logging;
pub mod memory;
pub mod protocol;
#[cfg(feature = "secure")]
pub mod security;
pub mod transport;

#[cfg(test)]
pub mod test_config;

// Re-export commonly used types
pub use application::{
    ConnectionState, Knx, KnxBuilder, P2pConnection, P2pConnectionless, ScannedInterfaceObject,
};
#[cfg(feature = "dpt")]
pub use application::{GroupAddress, GroupSubscription};
#[cfg(feature = "secure")]
pub use config::KeyringConfig;
pub use config::{ConfigFormat, Configuration};
pub use error::{KnxError, Result};
pub use logging::{Component, LogLevel, LoggingConfig, Timer};
pub use memory::{ConnectionPool, MemoryError, MemoryMonitor, MemoryStats, PerformanceOptimizer};
pub use protocol::IndividualAddress;
#[cfg(feature = "secure")]
pub use security::{KeyRing, SecureSession, SecureSessionState, SecurityKey, SessionConfig};
pub use transport::{ConnectionConfig, ConnectionType};

/// Library version information
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
