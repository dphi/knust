//! Logging demonstration example for Knx.
//!
//! This example shows how to configure and use the comprehensive logging
//! system in Knx, including component-specific log levels, protocol event
//! logging, and debugging features.

use knust::{Component, ConnectionType, Knx, LogLevel, LoggingConfig, logging::init_logging};
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize env_logger to see the actual log output
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();

    println!("Knx Logging Demonstration");
    println!("==========================");

    // Create and configure logging
    let mut logging_config = LoggingConfig::new();

    // Set different log levels for different components
    logging_config.set_default_level(LogLevel::Info);
    logging_config.set_component_level(Component::Transport, LogLevel::Debug);
    logging_config.set_component_level(Component::Protocol, LogLevel::Trace);
    logging_config.set_component_level(Component::Device, LogLevel::Debug);
    logging_config.set_component_level(Component::Application, LogLevel::Info);

    // Enable protocol event logging and hex dumps
    logging_config.set_protocol_events(true);
    logging_config.set_hex_dump(true);
    logging_config.set_max_hex_dump_size(64);

    // Initialize the logging system
    init_logging(logging_config);

    println!("Logging configuration:");
    println!("- Default level: Info");
    println!("- Transport: Debug");
    println!("- Protocol: Trace (with hex dumps)");
    println!("- Device: Debug");
    println!("- Application: Info");
    println!("- Protocol events: Enabled");
    println!();

    // Create Knx instance with routing connection (doesn't require gateway)
    println!("Creating Knx instance...");
    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .timeout_ms(5000)
        .auto_reconnect(true)
        .build()
        .await?;

    println!("Knx instance created. Check the logs above for detailed creation process.");
    println!();

    // Demonstrate connection attempt (will likely fail without actual KNX network)
    println!("Attempting to connect to KNX network...");
    println!("(This will likely fail without a real KNX network, but shows logging)");

    match knx.connect().await {
        Ok(()) => {
            println!("✓ Connected successfully!");

            // If connected, start processing
            println!("Starting telegram processing...");
            knx.start().await?;

            // Let it run for a bit to show any incoming telegrams
            println!("Running for 10 seconds to capture any telegrams...");
            sleep(Duration::from_secs(10)).await;

            // Shutdown gracefully
            println!("Shutting down...");
            knx.shutdown().await?;
        }
        Err(e) => {
            println!("✗ Connection failed (expected without real KNX network): {e}");
            println!("Check the detailed error logging above.");
        }
    }

    println!();
    println!("Logging demonstration complete!");
    println!();
    println!("Key logging features demonstrated:");
    println!("- Component-specific log levels");
    println!("- Structured error messages with context");
    println!("- Performance timing for operations");
    println!("- Protocol event logging");
    println!("- Connection state change tracking");
    println!("- Hex dumps of protocol data");
    println!();
    println!("To see more detailed logs, run with:");
    println!("RUST_LOG=trace cargo run --example logging_demo");

    Ok(())
}
