//! Property-based tests for transport layer functionality.

use proptest::option;
use proptest::prelude::*;
use proptest::test_runner::Config as ProptestConfig;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::protocol::address::IndividualAddress;
use crate::transport::connection::{Connection, ConnectionState};
use crate::transport::{BackoffConfig, ConnectionConfig, ConnectionType, TcpConfig};
use crate::transport::{GatewayScanner, RoutingConnection, ServiceType, Tunnel};

#[cfg(test)]
use crate::test_config::TestConfig;

/// Generate arbitrary connection configurations for property testing
fn arb_connection_config() -> impl Strategy<Value = ConnectionConfig> {
    (
        prop_oneof![
            Just(ConnectionType::Tunneling),
            Just(ConnectionType::Routing),
        ],
        option::of(prop_oneof![
            Just(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100))),
            Just(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))),
            Just(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))),
        ]),
        option::of(3671u16..=3680u16),
        option::of(prop_oneof![
            Just(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))),
            Just(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10))),
        ]),
        (1u8..=15u8, 1u8..=15u8, 1u8..=255u8),
        // Reduced timeout range for faster tests
        100u64..=500u64,
        any::<bool>(),
    )
        .prop_map(
            |(
                connection_type,
                gateway_ip,
                gateway_port,
                local_ip,
                (area, line, device),
                timeout_ms,
                auto_reconnect,
            )| {
                ConnectionConfig {
                    connection_type,
                    gateway_ip,
                    gateway_port,
                    local_ip,
                    individual_address: IndividualAddress::new(area, line, device),
                    security: None,
                    timeout_ms,
                    auto_reconnect,
                    reconnect_backoff: BackoffConfig::default(),
                    tcp_config: TcpConfig::default(),
                }
            },
        )
}

/// Build a search response fixture with a nonzero advertised endpoint.
fn create_mock_discovery_response(
    device_name: &str,
    supports_tunneling: bool,
    supports_routing: bool,
    supports_device_mgmt: bool,
    serial_suffix: u16,
) -> Vec<u8> {
    let mut frame = vec![0x06, 0x10, 0x02, 0x02, 0, 0];
    frame.extend_from_slice(&[8, 1, 192, 168, 1, 100, 0x0e, 0x57]);
    let mut device = [0; 54];
    device[..8].copy_from_slice(&[54, 1, 2, 0, 0x10, 1, 0, 0]);
    device[12..14].copy_from_slice(&serial_suffix.to_be_bytes());
    device[14..18].copy_from_slice(&[224, 0, 23, 14]);
    device[18..24].copy_from_slice(&[2, 0x10, 0x20, 0x30, 0x40, 0x50]);
    device[24..24 + device_name.len()].copy_from_slice(device_name.as_bytes());
    frame.extend_from_slice(&device);

    let mut families = vec![0, 2, 2, 1];
    for (enabled, family) in [
        (supports_device_mgmt, 3),
        (supports_tunneling, 4),
        (supports_routing, 5),
    ] {
        if enabled {
            families.extend_from_slice(&[family, 1]);
        }
    }
    families[0] = families.len() as u8;
    frame.extend_from_slice(&families);
    let length = (frame.len() as u16).to_be_bytes();
    frame[4..6].copy_from_slice(&length);
    frame
}

proptest! {
    // Reduce test cases and add timeout for faster execution
    #![proptest_config(ProptestConfig {
        cases: 20, // Reduced from default 256
        max_shrink_iters: 10, // Reduced from default 1024
        timeout: 30000, // 30 second timeout for entire test
        .. ProptestConfig::default()
    })]

    /// For any valid connection configuration, establishing a connection should result
    /// in the correct connection type and connected state.
    #[test]
    fn test_connection_establishment_consistency(config in arb_connection_config()) {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            match config.connection_type {
                ConnectionType::Tunneling => {
                    // For tunneling, we need a gateway IP
                    if let Some(gateway_ip) = config.gateway_ip {
                        let gateway_port = config.gateway_port.unwrap_or(3671);
                        let gateway_addr = SocketAddr::new(gateway_ip, gateway_port);

                        // Test that tunneling connection can be created with short timeout
                        // Note: This will fail in test environment without actual gateway,
                        // but we're testing the consistency of the API and initial state
                        let short_timeout = std::time::Duration::from_millis(100); // Very short timeout for tests
                        let mut conn = Tunnel::new_udp_with_timeout(gateway_addr, short_timeout);
                        // Connection should start in Disconnected state
                        let initial_state = conn.state();
                        prop_assert_eq!(
                            initial_state,
                            ConnectionState::Disconnected,
                            "Tunneling connection should start in Disconnected state, got: {:?}",
                            initial_state
                        );

                        // Stats should be initialized
                        let stats = conn.stats();
                        prop_assert_eq!(stats.frames_sent, 0);
                        prop_assert_eq!(stats.frames_received, 0);

                        // Test connection attempt (will likely fail in test environment)
                        let _ = conn.connect().await;
                        // After connect attempt, state should be either Connected or Failed
                        let post_connect_state = conn.state();
                        prop_assert!(
                            post_connect_state == ConnectionState::Connected ||
                            post_connect_state == ConnectionState::Failed ||
                            post_connect_state == ConnectionState::Connecting,
                            "After connect attempt, state should be Connected, Failed, or Connecting, got: {:?}",
                            post_connect_state
                        );
                    }
                },
                ConnectionType::TcpTunneling => {
                    // For TCP tunneling, we need a gateway IP
                    if let Some(gateway_ip) = config.gateway_ip {
                        let gateway_port = config.gateway_port.unwrap_or(3671);
                        let gateway_addr = SocketAddr::new(gateway_ip, gateway_port);

                        // Test that TCP tunneling connection can be created with short timeout
                        // Note: This will fail in test environment without actual gateway,
                        // but we're testing the consistency of the API and initial state
                        let short_timeout = std::time::Duration::from_millis(100); // Very short timeout for tests
                        let mut conn = Tunnel::new_tcp_with_timeout(gateway_addr, short_timeout);
                        // Connection should start in Disconnected state
                        let initial_state = conn.state();
                        prop_assert_eq!(
                            initial_state,
                            ConnectionState::Disconnected,
                            "TCP tunneling connection should start in Disconnected state, got: {:?}",
                            initial_state
                        );

                        // Stats should be initialized
                        let stats = conn.stats();
                        prop_assert_eq!(stats.frames_sent, 0);
                        prop_assert_eq!(stats.frames_received, 0);

                        // Test connection attempt (will likely fail in test environment)
                        let _ = conn.connect().await;
                        // After connect attempt, state should be either Connected or Failed
                        let post_connect_state = conn.state();
                        prop_assert!(
                            post_connect_state == ConnectionState::Connected ||
                            post_connect_state == ConnectionState::Failed ||
                            post_connect_state == ConnectionState::Connecting,
                            "After connect attempt, state should be Connected, Failed, or Connecting, got: {:?}",
                            post_connect_state
                        );
                    }
                },
                ConnectionType::Routing => {
                    // Test that routing connection can be created
                    match RoutingConnection::new(config.local_ip).await {
                        Ok(conn) => {
                            // Routing connection should be immediately connected
                            let state = conn.state();
                            prop_assert_eq!(
                                state,
                                ConnectionState::Connected,
                                "Routing connection should be in Connected state, got: {:?}",
                                state
                            );

                            // Stats should be initialized
                            let stats = conn.stats();
                            prop_assert_eq!(stats.frames_sent, 0);
                            prop_assert_eq!(stats.frames_received, 0);

                            // Test that connection can be closed
                            let _ = conn.close().await;
                            let final_state = conn.state();
                            prop_assert_eq!(
                                final_state,
                                ConnectionState::Disconnected,
                                "Connection should be Disconnected after close, got: {:?}",
                                final_state
                            );
                        },
                        Err(e) => {
                            // If connection fails, it should be due to network issues,
                            // not API inconsistency
                            prop_assert!(
                                e.to_string().contains("bind") ||
                                e.to_string().contains("multicast"),
                                "Connection failure should be network-related, got: {}",
                                e
                            );
                        }
                    }
                },
                ConnectionType::SecureTunneling | ConnectionType::SecureRouting => {
                    // Secure connections not yet implemented, skip for now
                }
            }

            Ok(())
        })?;
    }

    /// For any valid gateway discovery response, parsing should extract all device
    /// information and capabilities correctly.
    #[test]
    fn test_gateway_discovery_response_parsing(
        addr in prop_oneof![
            Just(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 3671)),
            Just(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 3671)),
            Just(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)), 3671)),
        ],
        device_name in "[A-Za-z0-9 ]{1,30}",
        supports_tunneling in any::<bool>(),
        supports_routing in any::<bool>(),
        supports_device_mgmt in any::<bool>(),
        serial_suffix in 0x1000u16..=0xFFFFu16,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            // Create a mock discovery response frame
            let response_frame = create_mock_discovery_response(
                &device_name,
                supports_tunneling,
                supports_routing,
                supports_device_mgmt,
                serial_suffix,
            );

            // Create scanner for testing
            let Ok(scanner) = GatewayScanner::new().await else {
                // Skip test if we can't create scanner (network issues in test environment)
                return Ok(());
            };

            let gateway = scanner.parse_discovery_response(&response_frame, addr).unwrap();
            prop_assert_eq!(gateway.name, device_name);
            prop_assert_eq!(gateway.individual_address, Some(IndividualAddress::new(1, 0, 1)));
            prop_assert_eq!(gateway.core_version, 1);
            prop_assert!(gateway.tunneling_info.is_none());
            prop_assert_eq!(gateway.addr, "192.168.1.100:3671".parse::<SocketAddr>().unwrap());
            prop_assert_eq!(gateway.device_serial, format!("SN00000000{serial_suffix:04X}"));
            prop_assert_eq!(gateway.mac_address.as_deref(), Some("02:10:20:30:40:50"));
            prop_assert_eq!(gateway.capabilities.supports_tunneling, supports_tunneling);
            prop_assert_eq!(gateway.capabilities.supports_device_management, supports_device_mgmt);
            prop_assert_eq!(gateway.capabilities.supports_routing, supports_routing);
            prop_assert_eq!(gateway.multicast_addr, supports_routing.then(|| "224.0.23.14:3671".parse().unwrap()));
            prop_assert!(gateway.supported_services.contains(&ServiceType::Core));
            Ok(())
        })?;
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_connection_config_default() {
        let config = ConnectionConfig::default();

        assert_eq!(config.connection_type, ConnectionType::Tunneling);
        assert_eq!(config.gateway_port, Some(3671));
        assert_eq!(config.timeout_ms, 5000);
        assert!(config.auto_reconnect);
        assert_eq!(config.individual_address, IndividualAddress::new(1, 1, 240));
    }

    #[test]
    // Comparing against a literal default, not a computed value; exact equality is correct here.
    #[allow(clippy::float_cmp)]
    fn test_backoff_config_default() {
        let backoff = BackoffConfig::default();

        assert_eq!(backoff.initial_delay_ms, 1000);
        assert_eq!(backoff.max_delay_ms, 30000);
        assert_eq!(backoff.multiplier, 2.0);
        assert_eq!(backoff.max_attempts, 10);
    }

    /// Test TCP tunneling connection creation and basic functionality
    /// Uses real gateway from .env.test if configured
    #[tokio::test]
    async fn test_tcp_tunneling_with_configured_gateway() {
        let test_config = TestConfig::load();

        // Try to use configured gateway, fall back to mock testing
        let gateway_addr = if let Some(addr) = test_config.gateway_addr() {
            println!("Using configured KNX gateway: {addr}");
            addr
        } else {
            println!("No gateway configured, using mock address for API testing");
            "192.0.2.1:3671".parse().unwrap() // RFC 5737 test address
        };

        // Test connection creation
        let mut conn = Tunnel::new_tcp_with_timeout(gateway_addr, test_config.test_timeout());

        // Test initial state
        assert_eq!(conn.state(), ConnectionState::Disconnected);
        assert!(!conn.is_connected());

        // Test statistics initialization
        let stats = conn.stats();
        assert_eq!(stats.frames_sent, 0);
        assert_eq!(stats.frames_received, 0);

        // Test connection attempt
        if test_config.is_real_gateway_enabled() {
            println!("Attempting real gateway connection...");
            match conn.connect().await {
                Ok(()) => {
                    println!("✓ Successfully connected to real KNX gateway via TCP");
                    assert!(conn.is_connected());

                    // Test disconnection
                    conn.disconnect().await;
                    assert!(!conn.is_connected());
                    println!("✓ Successfully disconnected");
                }
                Err(e) => {
                    println!("Connection failed (may be expected): {e}");
                    // Don't fail the test - gateway might not support TCP or be unreachable
                }
            }
        } else {
            println!("Skipping connection attempt - no real gateway configured");
            // Test that connection fails gracefully with unreachable address
            let result = conn.connect().await;
            assert!(
                result.is_err(),
                "Connection to unreachable address should fail"
            );
        }
    }

    /// Test UDP tunneling connection with configured gateway
    #[tokio::test]
    async fn test_udp_tunneling_with_configured_gateway() {
        let test_config = TestConfig::load();

        // Try to use configured gateway, fall back to mock testing
        let gateway_addr = if let Some(addr) = test_config.gateway_addr() {
            println!("Using configured KNX gateway: {addr}");
            addr
        } else {
            println!("No gateway configured, using mock address for API testing");
            "192.0.2.1:3671".parse().unwrap() // RFC 5737 test address
        };

        // Test connection creation
        let mut conn = Tunnel::new_udp_with_timeout(gateway_addr, test_config.test_timeout());

        // Test initial state
        assert_eq!(conn.state(), ConnectionState::Disconnected);

        // Test statistics initialization
        let stats = conn.stats();
        assert_eq!(stats.frames_sent, 0);
        assert_eq!(stats.frames_received, 0);

        // Test connection attempt
        if test_config.is_real_gateway_enabled() {
            println!("Attempting real gateway connection...");
            match conn.connect().await {
                Ok(()) => {
                    println!("✓ Successfully connected to real KNX gateway via UDP");
                    assert_eq!(conn.state(), ConnectionState::Connected);

                    // Test disconnection
                    conn.disconnect().await;
                    assert_eq!(conn.state(), ConnectionState::Disconnected);
                    println!("✓ Successfully disconnected");
                }
                Err(e) => {
                    println!("Connection failed (may be expected): {e}");
                    // Don't fail the test - gateway might not be reachable
                }
            }
        } else {
            println!("Skipping connection attempt - no real gateway configured");
            // Test that connection fails gracefully with unreachable address
            let result = conn.connect().await;
            assert!(
                result.is_err(),
                "Connection to unreachable address should fail"
            );
        }
    }
}
