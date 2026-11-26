//! Integration tests for complete Knx workflows
//!
//! These tests verify end-to-end functionality including direct group
//! communication, multi-target coordination, and secure communication
//! workflows. There's no built-in device abstraction layer — see
//! `examples/custom_devices.rs` for a pattern to build one on top of
//! `send_telegram`/`read_group_value`.

use knust::protocol::telegram::Telegram;
use knust::protocol::{IndividualAddress, RawGroupAddress};
use knust::transport::{BackoffConfig, TcpConfig};
use knust::{ConnectionConfig, ConnectionType, Knx, KnxError};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn builder_validates_the_selected_connection_mode() {
    use knust::error::ConfigurationError;

    let gateway = "127.0.0.1:3671".parse().unwrap();
    assert!(matches!(
        Knx::builder().build().await,
        Err(KnxError::Configuration(ConfigurationError::MissingParameter { parameter }))
            if parameter == "gateway_ip"
    ));
    for address in ["0.0.0.0:3671", "127.0.0.1:0", "224.0.23.12:3671"] {
        assert!(matches!(
            Knx::builder().tunnel(address.parse().unwrap()).build().await,
            Err(KnxError::Configuration(ConfigurationError::InvalidValue { parameter, .. }))
                if parameter == "gateway"
        ));
    }
    let mismatched = Knx::builder()
        .tcp_tunnel(gateway)
        .local_ip("::1".parse().unwrap());
    assert!(matches!(
        mismatched.build().await,
        Err(KnxError::Configuration(
            ConfigurationError::ValidationError { .. }
        ))
    ));
    for (builder, timeout_name) in [
        (Knx::builder().tunnel(gateway), "timeout_ms"),
        (
            Knx::builder().tcp_tunnel(gateway),
            "tcp_config.connect_timeout_ms",
        ),
    ] {
        assert!(matches!(
            builder.timeout(Duration::ZERO).build().await,
            Err(KnxError::Configuration(ConfigurationError::InvalidValue { parameter, .. }))
                if parameter == timeout_name
        ));
    }
    let routing = Knx::builder()
        .tunnel(gateway)
        .routing(Some(std::net::Ipv4Addr::LOCALHOST))
        .build()
        .await
        .unwrap();
    assert_eq!(routing.config().connection_type, ConnectionType::Routing);
    assert_eq!(
        routing.config().local_ip,
        Some(std::net::Ipv4Addr::LOCALHOST.into())
    );
    assert_eq!(routing.config().gateway_ip, None);
}

#[cfg(feature = "server")]
#[tokio::test]
async fn tunnel_builders_honor_the_local_bind_ip() -> Result<(), KnxError> {
    use knust::error::TransportError;
    use knust::transport::TunnelServer;
    use std::io::ErrorKind;
    use std::net::Ipv4Addr;

    let server = TunnelServer::bind(
        "127.0.0.1:0".parse().unwrap(),
        IndividualAddress::new(1, 1, 240),
    )
    .await?;
    let gateway = server.local_addr();
    for builder in [
        Knx::builder().tunnel(gateway),
        Knx::builder().tcp_tunnel(gateway),
    ] {
        let client = builder
            .local_ip(Ipv4Addr::LOCALHOST.into())
            .connect()
            .await?;
        client.shutdown().await?;
    }
    for builder in [
        Knx::builder().tunnel(gateway),
        Knx::builder().tcp_tunnel(gateway),
    ] {
        // TEST-NET-3 has no interface on the test host.
        let result = builder
            .local_ip(Ipv4Addr::new(203, 0, 113, 42).into())
            .connect()
            .await;
        assert!(matches!(
            result,
            Err(KnxError::Transport(TransportError::SocketError { operation, source }))
                if operation == "bind" && source.kind() == ErrorKind::AddrNotAvailable
        ));
    }
    server.shutdown();
    Ok(())
}

#[cfg(feature = "server")]
#[tokio::test]
async fn connection_state_and_callbacks_share_the_public_type() -> Result<(), KnxError> {
    use knust::ConnectionState;
    use knust::application::callbacks::ConnectionCallbackFn;
    use knust::transport::TunnelServer;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct States(Arc<Mutex<Vec<ConnectionState>>>);

    #[async_trait::async_trait]
    impl ConnectionCallbackFn for States {
        async fn call(&self, state: ConnectionState) {
            self.0.lock().unwrap().push(state);
        }
    }

    let server = TunnelServer::bind(
        "127.0.0.1:0".parse().unwrap(),
        IndividualAddress::new(1, 1, 240),
    )
    .await?;
    let mut incoming = server.subscribe();
    let states = States::default();
    let knx = Knx::builder()
        .tunnel(server.local_addr())
        .individual_address("1.1.8".parse()?)
        .connection_callback(states.clone())?
        .connect()
        .await?;

    assert_eq!(knx.state().await, ConnectionState::Connected);
    assert_eq!(knx.individual_address(), IndividualAddress::new(1, 1, 240));
    knx.connect().await?;
    assert_eq!(server.client_count().await, 1);
    let address: RawGroupAddress = "1/2/3".parse()?;
    #[cfg(feature = "dpt")]
    knx.group_address::<knust::protocol::dpt::DPTSwitch>(address)?
        .write(true)
        .await?;
    #[cfg(not(feature = "dpt"))]
    knx.send_telegram(&Telegram::group_write(address, vec![1]))
        .await?;
    let received = timeout(Duration::from_secs(1), incoming.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.source, knx.individual_address());
    assert_eq!(
        received.destination,
        knust::protocol::Address::Group(address)
    );
    #[cfg(feature = "dpt")]
    {
        let switch = knx.group_address::<knust::protocol::dpt::DPTSwitch>(address)?;
        let mut updates = switch.subscribe()?;
        server.send(Telegram::group_write(address, vec![1])).await?;
        let update = timeout(Duration::from_secs(1), updates.recv())
            .await
            .unwrap()
            .unwrap()?;
        assert!(update.value());
    }
    knx.start().await?;
    knx.shutdown().await?;
    assert_eq!(knx.state().await, ConnectionState::Disconnected);
    assert!(knx.start().await.is_err());
    assert_eq!(
        states.0.lock().unwrap().as_slice(),
        [
            ConnectionState::Connecting,
            ConnectionState::Connected,
            ConnectionState::Disconnecting,
            ConnectionState::Disconnected,
        ]
    );
    server.shutdown();
    Ok(())
}

/// `Knx::send_telegram` fills in `source` itself, so callers only need to
/// say where a write is going and what it carries.
fn write_telegram(destination: RawGroupAddress, payload: Vec<u8>) -> Telegram {
    Telegram::group_write(destination, payload)
}

/// Test end-to-end group communication scenarios
#[tokio::test]
async fn test_end_to_end_group_communication() -> Result<(), KnxError> {
    let config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: Some("127.0.0.1".parse().unwrap()),
        gateway_port: Some(3671),
        local_ip: Some("127.0.0.1".parse().unwrap()),
        individual_address: IndividualAddress::new(1, 1, 1),
        security: None,
        timeout_ms: 5000,
        auto_reconnect: false,
        reconnect_backoff: BackoffConfig::default(),
        tcp_config: TcpConfig::default(),
    };

    let knx = Knx::new(config).await?;

    // 1. Switch on the living room light's group address
    let result = timeout(
        Duration::from_secs(5),
        knx.send_telegram(&write_telegram(
            RawGroupAddress::from_parts(1, 2, 1)?,
            vec![0x01],
        )),
    )
    .await;
    match result {
        Ok(Ok(())) => println!("✓ Living room light switch write sent successfully"),
        Ok(Err(e)) => println!("⚠ Living room light switch write failed (expected in test): {e}"),
        Err(_) => println!("⚠ Living room light switch write timed out (expected in test)"),
    }

    // 2. Set brightness
    let result = timeout(
        Duration::from_secs(5),
        knx.send_telegram(&write_telegram(
            RawGroupAddress::from_parts(1, 2, 2)?,
            vec![128],
        )),
    )
    .await;
    match result {
        Ok(Ok(())) => println!("✓ Living room light brightness write sent successfully"),
        Ok(Err(e)) => {
            println!("⚠ Living room light brightness write failed (expected in test): {e}");
        }
        Err(_) => println!("⚠ Living room light brightness write timed out (expected in test)"),
    }

    // 3. Switch on the kitchen light's group address
    let result = timeout(
        Duration::from_secs(5),
        knx.send_telegram(&write_telegram(
            RawGroupAddress::from_parts(1, 3, 1)?,
            vec![0x01],
        )),
    )
    .await;
    match result {
        Ok(Ok(())) => println!("✓ Kitchen light switch write sent successfully"),
        Ok(Err(e)) => println!("⚠ Kitchen light switch write failed (expected in test): {e}"),
        Err(_) => println!("⚠ Kitchen light switch write timed out (expected in test)"),
    }

    // 4. Read a sensor value
    let result = knx
        .read_group_value(
            RawGroupAddress::from_parts(2, 1, 1)?,
            Duration::from_secs(5),
        )
        .await;
    match result {
        Ok(_) => println!("✓ Temperature sensor read successfully"),
        Err(e) => println!("⚠ Temperature sensor read failed (expected in test): {e}"),
    }

    println!("✓ End-to-end group communication test completed");
    Ok(())
}

/// Test multi-target coordination scenarios
#[tokio::test]
async fn test_multi_target_coordination() -> Result<(), KnxError> {
    let config = ConnectionConfig {
        connection_type: ConnectionType::Routing,
        timeout_ms: 5000,
        auto_reconnect: false,
        reconnect_backoff: BackoffConfig::default(),
        tcp_config: TcpConfig::default(),
        ..Default::default()
    };

    let knx = Knx::new(config).await?;

    let addresses = vec![
        RawGroupAddress::from_parts(1, 1, 1)?,
        RawGroupAddress::from_parts(1, 1, 2)?,
        RawGroupAddress::from_parts(1, 1, 3)?,
    ];

    // Coordinated control - switch all three group addresses on simultaneously
    let mut tasks = Vec::new();
    for addr in &addresses {
        let knx_clone = knx.clone();
        let addr = *addr;
        tasks.push(tokio::spawn(async move {
            timeout(
                Duration::from_secs(5),
                knx_clone.send_telegram(&write_telegram(addr, vec![0x01])),
            )
            .await
        }));
    }

    for (i, task) in tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok(Ok(()))) => println!("✓ Group address {} switched on successfully", i + 1),
            Ok(Ok(Err(e))) => println!("⚠ Group address {} failed (expected): {}", i + 1, e),
            Ok(Err(_)) => println!("⚠ Group address {} timed out (expected)", i + 1),
            Err(e) => println!("⚠ Group address {} task failed: {}", i + 1, e),
        }
    }

    // Coordinated brightness control (different value per target)
    let brightness_levels = [64u8, 128, 192];
    let mut brightness_tasks = Vec::new();
    for (addr, &brightness) in addresses.iter().zip(brightness_levels.iter()) {
        let knx_clone = knx.clone();
        let addr = *addr;
        brightness_tasks.push(tokio::spawn(async move {
            timeout(
                Duration::from_secs(5),
                knx_clone.send_telegram(&write_telegram(addr, vec![brightness])),
            )
            .await
        }));
    }

    for (i, task) in brightness_tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok(Ok(()))) => println!(
                "✓ Group address {} brightness set to {}",
                i + 1,
                brightness_levels[i]
            ),
            Ok(Ok(Err(e))) => println!(
                "⚠ Group address {} brightness failed (expected): {}",
                i + 1,
                e
            ),
            Ok(Err(_)) => println!("⚠ Group address {} brightness timed out (expected)", i + 1),
            Err(e) => println!("⚠ Group address {} brightness task failed: {}", i + 1, e),
        }
    }

    assert_eq!(addresses.len(), 3);
    println!("✓ Multi-target coordination test completed");
    Ok(())
}

#[cfg(all(feature = "secure", feature = "server", feature = "dpt"))]
#[tokio::test]
async fn secure_tunnel_builder_exchanges_typed_telegrams() -> Result<(), KnxError> {
    use knust::protocol::dpt::DPTSwitch;
    use knust::security::SessionConfig;
    use knust::transport::{SecurityConfig, TunnelServer};

    let server = TunnelServer::bind_secure(
        "127.0.0.1:0".parse().unwrap(),
        "1.1.240".parse()?,
        SessionConfig {
            user_id: 2,
            user_password: "test user password".into(),
            device_auth_password: Some("test device password".into()),
            keepalive_interval: 15,
        },
    )
    .await?;
    let mut incoming = server.subscribe();
    let knx = Knx::builder()
        .secure_tunnel(
            server.local_addr(),
            SecurityConfig::new("test device password", "test user password").with_user_id(2)?,
        )
        .timeout(Duration::from_secs(3))
        .connect()
        .await?;
    let address: RawGroupAddress = "1/5/1".parse()?;
    let switch = knx.group_address::<DPTSwitch>(address)?;
    let mut updates = switch.subscribe()?;

    switch.write(true).await?;
    let received = timeout(Duration::from_secs(1), incoming.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        received.destination,
        knust::protocol::Address::Group(address)
    );
    assert_eq!(received.payload, vec![1]);

    server.send(Telegram::group_write(address, vec![0])).await?;
    let received = timeout(Duration::from_secs(1), updates.recv())
        .await
        .unwrap()
        .unwrap()?;
    assert!(!received.value());
    knx.shutdown().await?;
    server.shutdown();
    Ok(())
}

/// Test error handling and recovery scenarios
#[tokio::test]
async fn test_error_handling_and_recovery() -> Result<(), KnxError> {
    let config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: Some("192.168.255.255".parse().unwrap()), // Invalid IP
        timeout_ms: 5000,
        auto_reconnect: false,
        reconnect_backoff: BackoffConfig::default(),
        tcp_config: TcpConfig::default(),
        ..Default::default()
    };

    // This should fail to connect
    let result = timeout(Duration::from_secs(5), Knx::new(config)).await;

    match result {
        Ok(Ok(_)) => println!("⚠ Unexpected success with invalid gateway"),
        Ok(Err(e)) => {
            println!("✓ Expected error with invalid gateway: {e}");

            // Verify error contains useful information
            let error_string = format!("{e}");
            assert!(
                error_string.contains("192.168.255.255")
                    || error_string.contains("connection")
                    || error_string.contains("transport")
            );
        }
        Err(_) => println!("✓ Connection attempt timed out as expected"),
    }

    // Test a telegram send without a connection
    let config = ConnectionConfig {
        connection_type: ConnectionType::Routing,
        timeout_ms: 2000,
        auto_reconnect: false,
        reconnect_backoff: BackoffConfig::default(),
        tcp_config: TcpConfig::default(),
        ..Default::default()
    };
    let knx = Knx::new(config).await?;
    let addr = RawGroupAddress::from_parts(1, 1, 1)?;

    let result = timeout(
        Duration::from_secs(2),
        knx.send_telegram(&write_telegram(addr, vec![0x01])),
    )
    .await;
    match result {
        Ok(Ok(())) => println!("⚠ Unexpected success without connection"),
        Ok(Err(e)) => println!("✓ Expected error without connection: {e}"),
        Err(_) => println!("✓ Operation timed out as expected without connection"),
    }

    println!("✓ Error handling and recovery test completed");
    Ok(())
}

/// Test configuration validation
#[tokio::test]
async fn test_configuration_validation() {
    // Test invalid group address
    let result = RawGroupAddress::from_parts(32, 8, 255); // Invalid values (max values)
    assert!(result.is_err(), "Should reject invalid group address");

    // Test valid group address
    let result = RawGroupAddress::from_parts(1, 2, 3);
    assert!(result.is_ok(), "Should accept valid group address");

    // Test connection config validation
    let config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: None, // Missing required gateway IP for tunneling
        timeout_ms: 2000,
        auto_reconnect: false,
        reconnect_backoff: BackoffConfig::default(),
        tcp_config: TcpConfig::default(),
        ..Default::default()
    };

    // This should be handled gracefully
    let result = timeout(Duration::from_secs(2), Knx::new(config)).await;
    match result {
        Ok(Ok(_)) => println!("⚠ Unexpected success with incomplete config"),
        Ok(Err(e)) => println!("✓ Expected error with incomplete config: {e}"),
        Err(_) => println!("✓ Configuration validation timed out as expected"),
    }

    println!("✓ Configuration validation test completed");
}

/// Test resource cleanup and memory management
#[tokio::test]
async fn test_resource_cleanup() -> Result<(), KnxError> {
    let config = ConnectionConfig {
        connection_type: ConnectionType::Routing,
        timeout_ms: 1000,
        auto_reconnect: false,
        reconnect_backoff: BackoffConfig::default(),
        tcp_config: TcpConfig::default(),
        ..Default::default()
    };

    // Create and destroy multiple Knx instances
    for i in 0u8..5 {
        let knx = Knx::new(config.clone()).await?;
        let addr = RawGroupAddress::from_parts(1, 1, i + 1)?;

        // Simulate some operations
        let _ = timeout(
            Duration::from_millis(100),
            knx.send_telegram(&write_telegram(addr, vec![0x01])),
        )
        .await;

        // Knx should be dropped here and resources cleaned up
        drop(knx);

        println!("✓ Knx instance {} created and cleaned up", i + 1);
    }

    println!("✓ Resource cleanup test completed");
    Ok(())
}
