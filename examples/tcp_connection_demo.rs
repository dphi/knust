//! TCP Connection Demo
//!
//! This example demonstrates how to use TCP connections with the Knx library.
//! TCP connections provide reliable, connection-oriented communication with KNX gateways.
//!
//! The example will try to read gateway configuration from .env.test file,
//! or use a default address if not configured.

use knust::protocol::IndividualAddress;
use knust::transport::{BackoffConfig, TcpConfig};
use knust::{ConnectionConfig, ConnectionType, Knx, KnxError};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), KnxError> {
    // Initialize logging
    env_logger::init();

    println!("Knx TCP Connection Demo");
    println!("========================");

    // Try to load configuration from .env.test
    let _ = dotenvy::from_filename(".env.test");

    let gateway_ip = std::env::var("KNX_GATEWAY")
        .ok()
        .and_then(|addr| addr.parse().ok())
        .unwrap_or_else(|| {
            println!("No KNX_GATEWAY configured in .env.test, using default 192.168.1.100");
            "192.168.1.100".parse().unwrap()
        });

    let gateway_port = std::env::var("KNX_GATEWAY_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(3671);

    println!("Using gateway: {gateway_ip}:{gateway_port}");

    // Configure TCP connection
    let config = ConnectionConfig {
        connection_type: ConnectionType::TcpTunneling,
        gateway_ip: Some(gateway_ip),
        gateway_port: Some(gateway_port),
        local_ip: None, // Let the system choose
        individual_address: IndividualAddress::new(1, 1, 240),
        security: None,
        timeout_ms: 5000,
        auto_reconnect: true,
        reconnect_backoff: BackoffConfig::default(),
        tcp_config: TcpConfig::default(), // Use default TCP settings
    };

    println!("Creating Knx instance with TCP connection...");
    let knx = Knx::new(config).await?;

    println!("Attempting to connect to gateway...");
    match knx.connect().await {
        Ok(()) => {
            println!("✓ Successfully connected to KNX gateway via TCP!");

            // Start the telegram processing
            println!("Starting telegram processing...");
            knx.start().await?;

            println!("TCP connection is active. Press Ctrl+C to exit.");

            // Keep the connection alive for demonstration
            tokio::time::sleep(Duration::from_secs(10)).await;

            println!("Shutting down...");
            knx.shutdown().await?;
            println!("✓ Shutdown complete");
        }
        Err(e) => {
            println!("✗ Failed to connect to gateway: {e}");
            println!("Make sure:");
            println!("  1. The gateway IP address is correct (set KNX_GATEWAY in .env.test)");
            println!("  2. The gateway is reachable on the network");
            println!("  3. The gateway supports TCP connections");
            return Err(e);
        }
    }

    Ok(())
}
