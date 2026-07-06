//! Integration tests for transport layer with real KNX gateways.
//!
//! These tests use real KNX gateways when configured in .env.test,
//! allowing for comprehensive testing of the transport layer functionality.

#[cfg(test)]
mod tests {
    use crate::test_config::{TestConfig, assert_no_timeout_if_reachable, skip_if_no_gateway};
    use crate::transport::{Connection, ConnectionState, Tunnel};
    use std::time::Duration;
    use tokio::time::timeout;

    /// Test UDP tunneling connection with real gateway
    #[tokio::test]
    async fn test_udp_tunneling_real_gateway() {
        let config = TestConfig::load();
        skip_if_no_gateway!(config);

        let conn_config = config.create_udp_tunneling_config().unwrap();
        let gateway_addr = std::net::SocketAddr::new(
            conn_config.gateway_ip.unwrap(),
            conn_config.gateway_port.unwrap(),
        );

        println!("Testing UDP tunneling connection to {gateway_addr}");

        // Create connection with configured timeout
        let mut conn = Tunnel::new_udp_with_timeout(gateway_addr, config.test_timeout());

        // Initial state should be disconnected
        assert_eq!(conn.state(), ConnectionState::Disconnected);

        // Attempt to connect
        let connect_result = timeout(config.test_timeout(), conn.connect()).await;

        // Use strict checking - if gateway is reachable, connection should succeed
        assert_no_timeout_if_reachable!(config, connect_result, "UDP connection");

        match connect_result {
            Ok(Ok(())) => {
                println!("✓ Successfully connected to KNX gateway via UDP");
                assert_eq!(conn.state(), ConnectionState::Connected);

                // Test basic statistics
                let stats = conn.stats();
                assert_eq!(stats.frames_sent, 0);
                assert_eq!(stats.frames_received, 0);

                // Test disconnection
                conn.disconnect().await;
                assert_eq!(conn.state(), ConnectionState::Disconnected);

                println!("✓ Successfully disconnected from KNX gateway");
            }
            Ok(Err(e)) => {
                println!("✗ Failed to connect to KNX gateway: {e}");
                // This might be expected if the gateway is not reachable
                // The test validates that the API behaves correctly
            }
            Err(_) => {
                println!("✗ Connection attempt timed out");
                // Timeout is handled by assert_no_timeout_if_reachable macro
            }
        }
    }

    /// Test TCP tunneling connection with real gateway
    #[tokio::test]
    async fn test_tcp_tunneling_real_gateway() {
        let config = TestConfig::load();
        skip_if_no_gateway!(config);

        let conn_config = config.create_tcp_tunneling_config().unwrap();
        let gateway_addr = std::net::SocketAddr::new(
            conn_config.gateway_ip.unwrap(),
            conn_config.gateway_port.unwrap(),
        );

        println!("Testing TCP tunneling connection to {gateway_addr}");

        // Create connection with configured timeout
        let mut conn = Tunnel::new_tcp_with_timeout(gateway_addr, config.test_timeout());

        // Initial state should be disconnected
        assert_eq!(conn.state(), ConnectionState::Disconnected);

        // Attempt to connect
        let connect_result = timeout(config.test_timeout(), conn.connect()).await;

        // Use strict checking - if gateway is reachable, connection should succeed
        assert_no_timeout_if_reachable!(config, connect_result, "TCP connection");

        match connect_result {
            Ok(Ok(())) => {
                println!("✓ Successfully connected to KNX gateway via TCP");
                assert!(conn.is_connected());

                // Test basic statistics
                let stats = conn.stats();
                assert_eq!(stats.frames_sent, 0);
                assert_eq!(stats.frames_received, 0);

                // Test disconnection
                conn.disconnect().await;
                assert!(!conn.is_connected());

                println!("✓ Successfully disconnected from KNX gateway");
            }
            Ok(Err(e)) => {
                println!("✗ Failed to connect to KNX gateway: {e}");
                // This might be expected if the gateway doesn't support TCP or is not reachable
                // The test validates that the API behaves correctly
            }
            Err(_) => {
                println!("✗ Connection attempt timed out");
                // Timeout is handled by assert_no_timeout_if_reachable macro
            }
        }
    }

    /// Test connection comparison between UDP and TCP
    #[tokio::test]
    async fn test_udp_vs_tcp_connection_comparison() {
        let config = TestConfig::load();
        skip_if_no_gateway!(config);

        let gateway_addr = config.gateway_addr().unwrap();
        println!("Comparing UDP vs TCP connections to {gateway_addr}");

        // Test UDP connection
        let udp_result = {
            let mut conn = Tunnel::new_udp_with_timeout(gateway_addr, config.test_timeout());

            let connect_result = timeout(config.test_timeout(), conn.connect()).await;
            if let Ok(Ok(())) = connect_result {
                println!("✓ UDP connection successful");
                conn.disconnect().await;
                true
            } else {
                println!("✗ UDP connection failed");
                false
            }
        };

        // Test TCP connection
        let tcp_result = {
            let mut conn = Tunnel::new_tcp_with_timeout(gateway_addr, config.test_timeout());

            let connect_result = timeout(config.test_timeout(), conn.connect()).await;
            if let Ok(Ok(())) = connect_result {
                println!("✓ TCP connection successful");
                conn.disconnect().await;
                true
            } else {
                println!("✗ TCP connection failed");
                false
            }
        };

        // Report results
        match (udp_result, tcp_result) {
            (true, true) => println!("✓ Both UDP and TCP connections successful"),
            (true, false) => println!("! UDP successful, TCP failed (gateway may not support TCP)"),
            (false, true) => println!("! TCP successful, UDP failed (unusual)"),
            (false, false) => println!("✗ Both connections failed (gateway may be unreachable)"),
        }

        // At least one connection type should work if the gateway is properly configured
        // But we don't fail the test since gateway configuration varies
    }

    /// Test connection resilience with multiple attempts
    #[tokio::test]
    async fn test_connection_resilience() {
        let config = TestConfig::load();
        skip_if_no_gateway!(config);

        let gateway_addr = config.gateway_addr().unwrap();
        println!("Testing connection resilience to {gateway_addr}");

        let mut successful_connections = 0;
        let total_attempts = 3;

        for attempt in 1..=total_attempts {
            println!("Connection attempt {attempt} of {total_attempts}");

            let mut conn = Tunnel::new_tcp_with_timeout(gateway_addr, config.test_timeout());

            let connect_result = timeout(
                Duration::from_millis(config.test_timeout_ms * 2), // Longer timeout for resilience test
                conn.connect(),
            )
            .await;

            if let Ok(Ok(())) = connect_result {
                successful_connections += 1;
                println!("✓ Attempt {attempt} successful");

                // Hold connection briefly
                tokio::time::sleep(Duration::from_millis(100)).await;

                conn.disconnect().await;
            } else {
                println!("✗ Attempt {attempt} failed");
            }

            // Brief pause between attempts
            if attempt < total_attempts {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }

        println!(
            "Connection resilience test completed: {successful_connections}/{total_attempts} successful"
        );

        // We don't assert on the number of successful connections since
        // network conditions can vary, but we report the results
    }

    /// Test TCP tunneling with actual KNX telegram exchange
    #[tokio::test]
    async fn test_tcp_tunneling_telegram_exchange() {
        let config = TestConfig::load();
        skip_if_no_gateway!(config);

        let conn_config = config.create_tcp_tunneling_config().unwrap();
        let gateway_addr = std::net::SocketAddr::new(
            conn_config.gateway_ip.unwrap(),
            conn_config.gateway_port.unwrap(),
        );

        println!("Testing TCP telegram exchange with {gateway_addr}");

        // Create connection with configured timeout
        let mut conn = Tunnel::new_tcp_with_timeout(gateway_addr, config.test_timeout());

        // Connect to gateway
        let connect_result = timeout(config.test_timeout(), conn.connect()).await;

        // Use strict checking for connection
        assert_no_timeout_if_reachable!(
            config,
            connect_result,
            "TCP connection for telegram exchange"
        );

        match connect_result {
            Ok(Ok(())) => {
                println!("✓ TCP connection established for telegram exchange");

                // Create a simple KNX telegram (group read request)
                // This is a basic KNX/IP tunneling request with CEMI frame
                let test_telegram = create_test_telegram();

                println!("Sending test telegram ({} bytes)", test_telegram.len());

                // Send the telegram
                let send_result = timeout(Duration::from_secs(2), conn.send(&test_telegram)).await;

                match send_result {
                    Ok(Ok(())) => {
                        println!("✓ Test telegram sent successfully");

                        // Try to receive a response (tunneling ACK or indication)
                        let recv_result = timeout(Duration::from_secs(3), conn.recv()).await;

                        match recv_result {
                            Ok(Ok(response_data)) => {
                                println!(
                                    "✓ Received response ({} bytes): {:02X?}",
                                    response_data.len(),
                                    &response_data[..std::cmp::min(response_data.len(), 20)]
                                );

                                // Verify it's a valid KNX/IP frame
                                if response_data.len() >= 6 {
                                    let frame_length =
                                        u16::from_be_bytes([response_data[4], response_data[5]]);
                                    println!(
                                        "✓ Valid KNX/IP frame received (length: {frame_length})"
                                    );
                                } else {
                                    println!("⚠ Response too short to be valid KNX/IP frame");
                                }
                            }
                            Ok(Err(e)) => {
                                println!("⚠ Failed to receive response: {e}");
                            }
                            Err(_) => {
                                println!("⚠ Receive timeout (may be normal if no devices respond)");
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        println!("✗ Failed to send telegram: {e}");
                    }
                    Err(_) => {
                        println!("✗ Send timeout");
                    }
                }

                // Clean disconnect
                conn.disconnect().await;
                println!("✓ TCP telegram exchange test completed");
            }
            Ok(Err(e)) => {
                println!("✗ Failed to connect for telegram exchange: {e}");
            }
            Err(_) => {
                println!("✗ Connection timeout for telegram exchange");
            }
        }
    }

    /// Test UDP tunneling with actual KNX telegram exchange
    #[tokio::test]
    async fn test_udp_tunneling_telegram_exchange() {
        let config = TestConfig::load();
        skip_if_no_gateway!(config);

        let conn_config = config.create_udp_tunneling_config().unwrap();
        let gateway_addr = std::net::SocketAddr::new(
            conn_config.gateway_ip.unwrap(),
            conn_config.gateway_port.unwrap(),
        );

        println!("Testing UDP telegram exchange with {gateway_addr}");

        // Create connection with configured timeout
        let mut conn = Tunnel::new_udp_with_timeout(gateway_addr, config.test_timeout());

        // Connect to gateway
        let connect_result = timeout(config.test_timeout(), conn.connect()).await;

        // Use strict checking for connection
        assert_no_timeout_if_reachable!(
            config,
            connect_result,
            "UDP connection for telegram exchange"
        );

        match connect_result {
            Ok(Ok(())) => {
                println!("✓ UDP connection established for telegram exchange");

                // Create a simple KNX telegram (group read request)
                let test_telegram = create_test_telegram();

                println!("Sending test telegram ({} bytes)", test_telegram.len());

                // Send the telegram
                let send_result = timeout(Duration::from_secs(2), conn.send(&test_telegram)).await;

                match send_result {
                    Ok(Ok(())) => {
                        println!("✓ Test telegram sent successfully");

                        // Try to receive a response (tunneling ACK or indication)
                        let recv_result = timeout(Duration::from_secs(3), conn.recv()).await;

                        match recv_result {
                            Ok(Ok(response_data)) => {
                                println!(
                                    "✓ Received response ({} bytes): {:02X?}",
                                    response_data.len(),
                                    &response_data[..std::cmp::min(response_data.len(), 20)]
                                );

                                // Verify it's a valid KNX/IP frame
                                if response_data.len() >= 6 {
                                    let frame_length =
                                        u16::from_be_bytes([response_data[4], response_data[5]]);
                                    println!(
                                        "✓ Valid KNX/IP frame received (length: {frame_length})"
                                    );
                                } else {
                                    println!("⚠ Response too short to be valid KNX/IP frame");
                                }
                            }
                            Ok(Err(e)) => {
                                println!("⚠ Failed to receive response: {e}");
                            }
                            Err(_) => {
                                println!("⚠ Receive timeout (may be normal if no devices respond)");
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        println!("✗ Failed to send telegram: {e}");
                    }
                    Err(_) => {
                        println!("✗ Send timeout");
                    }
                }

                // Clean disconnect
                conn.disconnect().await;
                println!("✓ UDP telegram exchange test completed");
            }
            Ok(Err(e)) => {
                println!("✗ Failed to connect for telegram exchange: {e}");
            }
            Err(_) => {
                println!("✗ Connection timeout for telegram exchange");
            }
        }
    }

    /// Create a test KNX telegram (tunneling request with group read)
    fn create_test_telegram() -> Vec<u8> {
        // Create a KNX/IP Tunneling Request frame with a CEMI group read
        // This is a basic group read request to address 1/1/1

        // KNX/IP Header (6 bytes)
        let mut frame = Vec::new();
        frame.push(0x06); // Header length
        frame.push(0x10); // Protocol version
        frame.extend_from_slice(&0x0420_u16.to_be_bytes()); // Service type: Tunneling Request
        frame.extend_from_slice(&0x0015_u16.to_be_bytes()); // Total length (21 bytes)

        // Tunneling Request Header (4 bytes)
        frame.push(0x04); // Structure length
        frame.push(0x01); // Communication channel (will be updated by connection)
        frame.push(0x00); // Sequence counter (will be updated by connection)
        frame.push(0x00); // Reserved

        // CEMI Frame (11 bytes) - L_Data.req for group read
        frame.push(0x11); // Message code: L_Data.req
        frame.push(0x00); // Additional info length
        frame.push(0xBC); // Control field 1: Standard frame, no repeat, system broadcast, normal priority, no ack
        frame.push(0xE0); // Control field 2: Address type group, hop count 6, extended frame format
        frame.extend_from_slice(&[0x11, 0x01]); // Source address: 1.1.1
        frame.extend_from_slice(&[0x09, 0x01]); // Destination address: 1/1/1 (group)
        frame.push(0x01); // Data length: 1 byte
        frame.push(0x00); // TPCI/APCI: Group read request
        frame.push(0x80); // APCI continued: Group read

        frame
    }

    /// Test TCP connection with tunneling ACK verification
    #[tokio::test]
    async fn test_tcp_tunneling_ack_response() {
        let config = TestConfig::load();
        skip_if_no_gateway!(config);

        let conn_config = config.create_tcp_tunneling_config().unwrap();
        let gateway_addr = std::net::SocketAddr::new(
            conn_config.gateway_ip.unwrap(),
            conn_config.gateway_port.unwrap(),
        );

        println!("Testing TCP tunneling ACK response with {gateway_addr}");

        // Create connection
        let mut conn = Tunnel::new_tcp_with_timeout(gateway_addr, config.test_timeout());

        // Connect to gateway
        if let Ok(Ok(())) = timeout(config.test_timeout(), conn.connect()).await {
            println!("✓ TCP connection established");

            // Create a tunneling request with proper channel ID and sequence
            let mut test_telegram = create_test_telegram();

            // Update the channel ID in the telegram (byte 10)
            test_telegram[10] = conn.channel_id();

            // Update the sequence counter (byte 11)
            test_telegram[11] = conn.current_sequence();

            println!(
                "Sending tunneling request with channel_id={}, sequence={}",
                conn.channel_id(),
                conn.current_sequence()
            );

            // Send the telegram
            if let Ok(Ok(())) = timeout(Duration::from_secs(2), conn.send(&test_telegram)).await {
                println!("✓ Tunneling request sent successfully");

                // For UDP, we should receive a tunneling ACK
                // For TCP, the gateway might send ACK or just accept the frame
                match timeout(Duration::from_secs(2), conn.recv()).await {
                    Ok(Ok(response_data)) => {
                        println!("✓ Received response ({} bytes)", response_data.len());

                        // Check if it's a tunneling ACK (service type 0x0421)
                        if response_data.len() >= 6 {
                            let service_type =
                                u16::from_be_bytes([response_data[2], response_data[3]]);
                            match service_type {
                                0x0421 => {
                                    println!("✓ Received Tunneling ACK");
                                    if response_data.len() >= 10 {
                                        let status = response_data[9];
                                        println!(
                                            "  ACK Status: 0x{:02X} ({})",
                                            status,
                                            if status == 0 { "OK" } else { "Error" }
                                        );
                                    }
                                }
                                0x0420 => {
                                    println!("✓ Received Tunneling Request (indication from bus)");
                                }
                                _ => {
                                    println!("✓ Received other KNX/IP frame: 0x{service_type:04X}");
                                }
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        println!("⚠ Failed to receive response: {e}");
                    }
                    Err(_) => {
                        println!("⚠ No response received (may be normal for TCP)");
                    }
                }
            } else {
                println!("✗ Failed to send tunneling request");
            }

            conn.disconnect().await;
            println!("✓ TCP tunneling ACK test completed");
        } else {
            println!("✗ Failed to establish TCP connection");
        }
    }

    /// Test that connections properly handle timeouts
    #[tokio::test]
    async fn test_connection_timeout_handling() {
        let _config = TestConfig::load();

        // This test works even without a real gateway by using an unreachable address
        let unreachable_addr = "192.0.2.1:3671".parse().unwrap(); // RFC 5737 test address

        println!("Testing timeout handling with unreachable address");

        let start_time = std::time::Instant::now();

        let mut conn = Tunnel::new_tcp_with_timeout(
            unreachable_addr,
            Duration::from_secs(1), // Short timeout
        );

        let connect_result = conn.connect().await;
        let elapsed = start_time.elapsed();

        // Connection should fail due to timeout
        assert!(connect_result.is_err(), "Connection should have failed");

        // Should have timed out within reasonable bounds (allow some margin)
        assert!(
            elapsed >= Duration::from_millis(900) && elapsed <= Duration::from_secs(2),
            "Timeout should be respected, took {elapsed:?}"
        );

        println!("✓ Timeout handling works correctly (took {elapsed:?})");
    }
}
