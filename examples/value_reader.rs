//! Example for reading values from KNX group addresses.
//!
//! Connect to KNX/IP device and read values from specific group addresses.

use std::time::Duration;
use tokio::time::sleep;

use knust::protocol::address::{GroupAddress, IndividualAddress};
use knust::{ConnectionConfig, ConnectionType, Knx};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::init();

    // Configure connection
    let config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: Some("192.168.1.100".parse()?),
        individual_address: IndividualAddress::new(1, 1, 240),
        ..Default::default()
    };

    // Create Knx instance
    let knx = Knx::new(config).await?;

    // Connect to KNX network
    knx.connect().await?;

    // Define group addresses to read from
    let addresses = vec![
        GroupAddress::from_parts(1, 2, 3)?, // Light switch
        GroupAddress::from_parts(2, 1, 1)?, // Temperature sensor
        GroupAddress::from_parts(3, 0, 5)?, // Motion sensor
    ];

    println!("Reading values from group addresses...");

    for address in addresses {
        println!("Reading from {address}...");

        // TODO: Implement group read functionality in Knx
        // This would send a GroupValueRead telegram and wait for response
        // For now, we'll just demonstrate the structure

        // Simulate reading delay
        sleep(Duration::from_millis(500)).await;

        println!("  Address {address}: Value read (implementation pending)");
    }

    // Disconnect
    knx.disconnect().await?;

    println!("Value reader example completed successfully!");
    Ok(())
}
