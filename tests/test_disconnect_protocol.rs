//! Test for disconnect/reconnect protocol handling
//!
//! This test demonstrates the new disconnect handling and reconnection logic
//! following the Python Knx implementation pattern.

use knust::transport::BackoffConfig;
use knust::{ConnectionConfig, ConnectionType, Knx};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn test_disconnect_protocol_handling() {
    // Create Knx instance with auto-reconnect enabled
    let config = ConnectionConfig {
        connection_type: ConnectionType::Routing, // Use routing to avoid needing actual gateway
        auto_reconnect: true,
        ..Default::default()
    };

    let knx = Knx::new(config)
        .await
        .expect("Failed to create Knx instance");

    // Connect to the network
    knx.connect().await.expect("Failed to connect");
    assert!(knx.is_connected().await);

    // Start telegram processing
    knx.start().await.expect("Failed to start processing");

    // Simulate a disconnect by calling disconnect
    knx.disconnect().await.expect("Failed to disconnect");
    assert!(!knx.is_connected().await);

    // Wait a bit to see if reconnection happens (it won't for routing, but tests the logic)
    sleep(Duration::from_millis(100)).await;

    // Stop processing
    knx.stop().await;

    println!("✓ Disconnect protocol handling test completed successfully");
}

#[tokio::test]
async fn test_reconnection_backoff_configuration() {
    // Test custom backoff configuration
    let config = ConnectionConfig {
        connection_type: ConnectionType::Routing,
        auto_reconnect: true,
        reconnect_backoff: BackoffConfig {
            initial_delay_ms: 500,
            max_delay_ms: 5000,
            multiplier: 1.5,
            max_attempts: 5,
        },
        ..Default::default()
    };

    let knx = Knx::new(config)
        .await
        .expect("Failed to create Knx instance");

    // Verify the configuration is stored correctly
    assert!(knx.config().auto_reconnect);
    assert_eq!(knx.config().reconnect_backoff.initial_delay_ms, 500);
    assert_eq!(knx.config().reconnect_backoff.max_attempts, 5);

    println!("✓ Reconnection backoff configuration test completed successfully");
}

#[tokio::test]
async fn test_control_event_system() {
    use knust::application::ConnectionControlEvent;

    // Create Knx instance with routing (doesn't need gateway IP)
    let config = ConnectionConfig {
        connection_type: ConnectionType::Routing,
        ..Default::default()
    };

    let _knx = Knx::new(config)
        .await
        .expect("Failed to create Knx instance");

    // Test that the control channel is properly initialized
    // (This is mostly a compilation test to ensure the types are correct)

    // The control events should be handled internally
    let _ = ConnectionControlEvent::TunnelLost {
        channel_id: 1,
        reason: "Test disconnect".to_string(),
    };

    let _ = ConnectionControlEvent::SendDisconnectResponse { channel_id: 1 };

    println!("✓ Control event system test completed successfully");
}
