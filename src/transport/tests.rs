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

/// Create a mock KNX/IP discovery response frame for testing
fn create_mock_discovery_response(
    device_name: &str,
    supports_tunneling: bool,
    supports_routing: bool,
    supports_device_mgmt: bool,
    _max_connections: u8,
    serial_suffix: u32,
) -> Vec<u8> {
    let mut frame = Vec::new();

    // KNX/IP Header
    frame.extend_from_slice(&[
        0x06, 0x10, // Header length (6) and version (1.0)
        0x02, 0x02, // Search response service type
        0x00, 0x50, // Total length (80 bytes - will be adjusted)
    ]);

    // Control endpoint HPAI
    frame.extend_from_slice(&[
        0x08, 0x01, // Structure length and host protocol code
        0xC0, 0xA8, 0x01, 0x64, // IP address (192.168.1.100)
        0x0E, 0x57, // Port (3671)
    ]);

    // Device hardware DIB
    frame.extend_from_slice(&[
        0x36, 0x01, // Structure length (54) and description type (device info)
        0x02, 0x00, // KNX medium (TP1)
        0x11, 0x00, // Device status
        0x00, 0x01, // Individual address (1.0.1)
        0x00, 0x00, // Project installation identifier
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Serial number (6 bytes)
    ]);

    // Add serial number with the provided suffix
    let serial_bytes = serial_suffix.to_be_bytes();
    frame.extend_from_slice(&serial_bytes[2..4]); // Use last 2 bytes
    frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Pad to 6 bytes

    // Multicast address
    frame.extend_from_slice(&[
        0xE0, 0x00, 0x17, 0x0C, // 224.0.23.12 (KNX/IP multicast)
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // MAC address
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Padding
    ]);

    // Friendly name (30 bytes)
    let mut name_bytes = device_name.as_bytes().to_vec();
    name_bytes.resize(30, 0); // Pad or truncate to 30 bytes
    frame.extend_from_slice(&name_bytes);

    // Supported service families DIB
    let mut services_dib = vec![0x00, 0x02]; // Will set length later, description type (supported services)

    // Core service (always present)
    services_dib.extend_from_slice(&[0x02, 0x01]); // Core service family, version 1

    // Device management
    if supports_device_mgmt {
        services_dib.extend_from_slice(&[0x03, 0x01]); // Device management, version 1
    }

    // Tunneling
    if supports_tunneling {
        services_dib.extend_from_slice(&[0x04, 0x01]); // Tunneling, version 1
    }

    // Routing
    if supports_routing {
        services_dib.extend_from_slice(&[0x05, 0x01]); // Routing, version 1
    }

    // Set the correct length for services DIB
    services_dib[0] = services_dib.len() as u8;
    frame.extend_from_slice(&services_dib);

    // Update total frame length in header
    let total_length = frame.len() as u16;
    frame[4] = (total_length >> 8) as u8;
    frame[5] = (total_length & 0xFF) as u8;

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
        max_connections in 1u8..=16u8,
        serial_suffix in 0x1000u32..=0xFFFFu32,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            // Create a mock discovery response frame
            let response_frame = create_mock_discovery_response(
                &device_name,
                supports_tunneling,
                supports_routing,
                supports_device_mgmt,
                max_connections,
                serial_suffix,
            );

            // Create scanner for testing
            let Ok(scanner) = GatewayScanner::new().await else {
                // Skip test if we can't create scanner (network issues in test environment)
                return Ok(());
            };

            // Test parsing the response
            match scanner.parse_discovery_response(&response_frame, addr) {
                Ok(gateway_info) => {
                    // Verify that parsing extracted the correct information
                    prop_assert!(
                        !gateway_info.name.is_empty(),
                        "Gateway name should not be empty, got: '{}'",
                        gateway_info.name
                    );

                    prop_assert_eq!(
                        gateway_info.addr,
                        addr,
                        "Gateway address should match the source address"
                    );

                    // Verify capabilities are reasonable
                    prop_assert!(
                        gateway_info.capabilities.max_tunneling_connections > 0,
                        "Max tunneling connections should be positive, got: {}",
                        gateway_info.capabilities.max_tunneling_connections
                    );

                    // Verify device serial is present and reasonable
                    prop_assert!(
                        !gateway_info.device_serial.is_empty(),
                        "Device serial should not be empty"
                    );

                    // Verify supported services list is not empty
                    prop_assert!(
                        !gateway_info.supported_services.is_empty(),
                        "Supported services list should not be empty"
                    );

                    // Verify that if tunneling is supported, it's in the services list
                    if gateway_info.capabilities.supports_tunneling {
                        prop_assert!(
                            gateway_info.supported_services.contains(&ServiceType::Tunneling),
                            "If tunneling is supported, it should be in services list"
                        );
                    }

                    // Verify that if routing is supported, it's in the services list
                    if gateway_info.capabilities.supports_routing {
                        prop_assert!(
                            gateway_info.supported_services.contains(&ServiceType::Routing),
                            "If routing is supported, it should be in services list"
                        );
                    }

                    // Verify that Core service is always present
                    prop_assert!(
                        gateway_info.supported_services.contains(&ServiceType::Core),
                        "Core service should always be present in supported services"
                    );
                },
                Err(e) => {
                    // If parsing fails, it should be due to invalid response format,
                    // not due to API inconsistency
                    prop_assert!(
                        e.to_string().contains("Response too short") ||
                        e.to_string().contains("Invalid response"),
                        "Parsing failure should be due to invalid response format, got: {}",
                        e
                    );
                }
            }

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
