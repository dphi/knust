//! Example for proper connection handling and graceful disconnection.
//!
//! Demonstrates connecting to KNX/IP device, performing operations, and properly disconnecting.

use std::time::Duration;
use tokio::signal;
use tokio::time::sleep;

use knust::protocol::address::{GroupAddress, IndividualAddress, MainGroup, MiddleGroup};
use knust::protocol::dpt::DPTSwitch;
use knust::{ConnectionConfig, ConnectionType, Knx};

async fn graceful_shutdown(knx: Knx) -> Result<(), Box<dyn std::error::Error>> {
    println!("Shutting down gracefully...");

    // Perform any cleanup operations here
    // For example, turn off all lights

    // Disconnect from KNX network
    knx.disconnect().await?;
    println!("Disconnected from KNX network");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    const MAX_RETRIES: u32 = 3;

    // Initialize logging
    env_logger::init();

    // Configure connection
    let config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: Some("192.168.1.100".parse()?),
        individual_address: IndividualAddress::new(1, 1, 240),
        auto_reconnect: true,
        ..Default::default()
    };

    // Create Knx instance
    let knx = Knx::new(config).await?;

    // Set up signal handler for graceful shutdown
    let knx_clone = knx.clone();
    tokio::spawn(async move {
        signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        println!("Received Ctrl+C, initiating graceful shutdown...");
        if let Err(e) = graceful_shutdown(knx_clone).await {
            eprintln!("Error during shutdown: {e}");
        }
        std::process::exit(0);
    });

    println!("Connecting to KNX network...");

    // Connect to KNX network with retry logic
    let mut retry_count = 0;

    loop {
        match knx.connect().await {
            Ok(()) => {
                println!("Successfully connected to KNX network");
                break;
            }
            Err(e) => {
                retry_count += 1;
                if retry_count >= MAX_RETRIES {
                    eprintln!("Failed to connect after {MAX_RETRIES} attempts: {e}");
                    return Err(e.into());
                }
                println!("Connection attempt {retry_count} failed: {e}. Retrying in 2 seconds...");
                sleep(Duration::from_secs(2)).await;
            }
        }
    }

    // Perform some operations - toggle a switch's group address directly
    let switch_address = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(0), 9);
    let switch = knx.group_address::<DPTSwitch>(switch_address)?;

    println!("Performing operations...");

    for i in 1..=5 {
        println!("Operation {i}/5: Toggling switch");

        switch.write(true).await?;
        sleep(Duration::from_secs(1)).await;

        switch.write(false).await?;
        sleep(Duration::from_secs(1)).await;
    }

    // Check connection status
    if knx.is_connected().await {
        println!("Connection is still active");
    } else {
        println!("Connection lost, attempting to reconnect...");
        knx.connect().await?;
    }

    // Graceful shutdown
    graceful_shutdown(knx).await?;

    println!("Disconnect example completed successfully!");
    Ok(())
}
