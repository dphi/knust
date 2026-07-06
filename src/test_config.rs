//! Test configuration utilities for Knx integration tests.
//!
//! This module provides utilities for reading test configuration from environment
//! files, allowing tests to use real KNX gateways when available while falling
//! back to mock testing when not configured.

#[cfg(test)]
use std::net::{IpAddr, SocketAddr};
#[cfg(test)]
use std::str::FromStr;
#[cfg(test)]
use std::sync::Once;
#[cfg(test)]
use tokio::process::Command;

#[cfg(test)]
static INIT: Once = Once::new();

/// Test configuration loaded from .env.test file
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct TestConfig {
    /// Primary KNX gateway address for testing
    pub gateway_addr: Option<SocketAddr>,

    /// Backup KNX gateway address for failover testing
    pub backup_gateway_addr: Option<SocketAddr>,

    /// Test timeout in milliseconds
    pub test_timeout_ms: u64,

    /// Whether real gateway testing is enabled
    pub real_gateway_enabled: bool,

    /// Whether the gateway is reachable via ping (cached result)
    pub gateway_reachable: Option<bool>,
}

#[cfg(test)]
impl Default for TestConfig {
    fn default() -> Self {
        Self {
            gateway_addr: None,
            backup_gateway_addr: None,
            test_timeout_ms: 5000,
            real_gateway_enabled: false,
            gateway_reachable: None,
        }
    }
}

#[cfg(test)]
impl TestConfig {
    /// Load test configuration from .env.test file
    pub fn load() -> Self {
        INIT.call_once(|| {
            // Try to load .env.test file, ignore if it doesn't exist
            let _ = dotenvy::from_filename(".env.test");
        });

        let mut config = TestConfig::default();

        // Load KNX_GATEWAY
        if let Ok(gateway_str) = std::env::var("KNX_GATEWAY")
            && !gateway_str.trim().is_empty()
        {
            if let Ok(gateway_ip) = IpAddr::from_str(&gateway_str) {
                let port = std::env::var("KNX_GATEWAY_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(3671);

                config.gateway_addr = Some(SocketAddr::new(gateway_ip, port));
                config.real_gateway_enabled = true;

                println!(
                    "Test config: Using real KNX gateway at {}",
                    SocketAddr::new(gateway_ip, port)
                );
            } else {
                println!("Test config: Invalid KNX_GATEWAY address: {gateway_str}");
            }
        }

        // Load KNX_GATEWAY_BACKUP
        if let Ok(backup_str) = std::env::var("KNX_GATEWAY_BACKUP")
            && !backup_str.trim().is_empty()
            && let Ok(backup_ip) = IpAddr::from_str(&backup_str)
        {
            let port = std::env::var("KNX_GATEWAY_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3671);

            config.backup_gateway_addr = Some(SocketAddr::new(backup_ip, port));
            println!(
                "Test config: Using backup KNX gateway at {}",
                SocketAddr::new(backup_ip, port)
            );
        }

        // Load test timeout
        if let Ok(timeout_str) = std::env::var("KNX_TEST_TIMEOUT")
            && let Ok(timeout) = timeout_str.parse()
        {
            config.test_timeout_ms = timeout;
        }

        if !config.real_gateway_enabled {
            println!("Test config: No KNX gateway configured, using mock testing only");
            println!("Test config: Set KNX_GATEWAY in .env.test to enable real gateway testing");
        }

        config
    }

    /// Get the primary gateway address if configured
    #[must_use]
    pub fn gateway_addr(&self) -> Option<SocketAddr> {
        self.gateway_addr
    }

    /// Get the backup gateway address if configured
    #[must_use]
    pub fn backup_gateway_addr(&self) -> Option<SocketAddr> {
        self.backup_gateway_addr
    }

    /// Check if real gateway testing is enabled
    #[must_use]
    pub fn is_real_gateway_enabled(&self) -> bool {
        self.real_gateway_enabled
    }

    /// Get test timeout duration
    #[must_use]
    pub fn test_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.test_timeout_ms)
    }

    /// Check if the gateway is reachable via ping
    pub async fn is_gateway_reachable(&mut self) -> bool {
        // Return cached result if available
        if let Some(reachable) = self.gateway_reachable {
            return reachable;
        }

        // If no gateway configured, it's not reachable
        let Some(gateway_addr) = self.gateway_addr else {
            self.gateway_reachable = Some(false);
            return false;
        };

        // Try to ping the gateway
        let reachable = self.ping_host(gateway_addr.ip()).await;
        self.gateway_reachable = Some(reachable);

        if reachable {
            println!(
                "Test config: Gateway {} is reachable via ping",
                gateway_addr.ip()
            );
        } else {
            println!(
                "Test config: Gateway {} is NOT reachable via ping",
                gateway_addr.ip()
            );
        }

        reachable
    }

    /// Ping a host to check if it's reachable
    async fn ping_host(&self, ip: IpAddr) -> bool {
        let ping_cmd = "ping";

        let ip_string = ip.to_string();
        let args = if cfg!(target_os = "windows") {
            vec!["-n", "1", "-w", "2000", &ip_string]
        } else {
            vec!["-c", "1", "-W", "2", &ip_string]
        };

        match Command::new(ping_cmd).args(&args).output().await {
            Ok(output) => output.status.success(),
            Err(_) => {
                // If ping command fails, assume not reachable
                false
            }
        }
    }

    /// Create a connection config for testing with the configured gateway
    #[must_use]
    pub fn create_udp_tunneling_config(&self) -> Option<crate::transport::ConnectionConfig> {
        self.gateway_addr
            .map(|addr| crate::transport::ConnectionConfig {
                connection_type: crate::transport::ConnectionType::Tunneling,
                gateway_ip: Some(addr.ip()),
                gateway_port: Some(addr.port()),
                local_ip: None,
                individual_address: crate::protocol::IndividualAddress::new(1, 1, 240),
                security: None,
                timeout_ms: self.test_timeout_ms,
                auto_reconnect: false,
                reconnect_backoff: crate::transport::BackoffConfig::default(),
                tcp_config: crate::transport::TcpConfig::default(),
            })
    }

    /// Create a TCP connection config for testing with the configured gateway
    #[must_use]
    pub fn create_tcp_tunneling_config(&self) -> Option<crate::transport::ConnectionConfig> {
        self.gateway_addr
            .map(|addr| crate::transport::ConnectionConfig {
                connection_type: crate::transport::ConnectionType::TcpTunneling,
                gateway_ip: Some(addr.ip()),
                gateway_port: Some(addr.port()),
                local_ip: None,
                individual_address: crate::protocol::IndividualAddress::new(1, 1, 240),
                security: None,
                timeout_ms: self.test_timeout_ms,
                auto_reconnect: false,
                reconnect_backoff: crate::transport::BackoffConfig::default(),
                tcp_config: crate::transport::TcpConfig::default(),
            })
    }

    /// Create a routing connection config for testing
    #[must_use]
    pub fn create_routing_config(&self) -> crate::transport::ConnectionConfig {
        crate::transport::ConnectionConfig {
            connection_type: crate::transport::ConnectionType::Routing,
            gateway_ip: None,
            gateway_port: Some(3671),
            local_ip: None,
            individual_address: crate::protocol::IndividualAddress::new(1, 1, 240),
            security: None,
            timeout_ms: self.test_timeout_ms,
            auto_reconnect: false,
            reconnect_backoff: crate::transport::BackoffConfig::default(),
            tcp_config: crate::transport::TcpConfig::default(),
        }
    }
}

/// Macro to skip test if no real gateway is configured
#[cfg(test)]
macro_rules! skip_if_no_gateway {
    ($config:expr) => {
        if !$config.is_real_gateway_enabled() {
            println!("Skipping test: No KNX gateway configured in .env.test");
            return;
        }
    };
}

/// Macro to assert that timeout should not occur if gateway is reachable
#[cfg(test)]
macro_rules! assert_no_timeout_if_reachable {
    ($config:expr, $result:expr, $operation:expr) => {{
        let mut config = $config;
        let is_reachable = config.is_gateway_reachable().await;

        match (&$result, is_reachable) {
            (Ok(Ok(_)), _) => {
                println!("✓ {} completed successfully", $operation);
            }
            (Ok(Err(e)), _) => {
                println!("⚠ {} failed with error: {}", $operation, e);
            }
            (Err(_), true) => {
                panic!(
                    "✗ {} timed out but gateway is reachable via ping",
                    $operation
                );
            }
            (Err(_), false) => {
                println!(
                    "⚠ {} timed out and gateway is not reachable via ping",
                    $operation
                );
            }
        }
    }};
}

#[cfg(test)]
pub(crate) use assert_no_timeout_if_reachable;
#[cfg(test)]
pub(crate) use skip_if_no_gateway;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_loading() {
        let config = TestConfig::load();

        // Test should not panic and should return valid config
        assert!(config.test_timeout_ms > 0);

        // If gateway is configured, it should be valid
        if let Some(addr) = config.gateway_addr() {
            assert!(addr.port() > 0);
            println!("Loaded gateway config: {addr}");
        }
    }

    #[test]
    fn test_config_creation() {
        let config = TestConfig::load();

        // Test UDP config creation
        if let Some(udp_config) = config.create_udp_tunneling_config() {
            assert_eq!(
                udp_config.connection_type,
                crate::transport::ConnectionType::Tunneling
            );
            assert!(udp_config.gateway_ip.is_some());
        }

        // Test TCP config creation
        if let Some(tcp_config) = config.create_tcp_tunneling_config() {
            assert_eq!(
                tcp_config.connection_type,
                crate::transport::ConnectionType::TcpTunneling
            );
            assert!(tcp_config.gateway_ip.is_some());
        }

        // Test routing config creation (always works)
        let routing_config = config.create_routing_config();
        assert_eq!(
            routing_config.connection_type,
            crate::transport::ConnectionType::Routing
        );
    }
}
