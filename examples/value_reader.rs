//! Example for reading values from KNX group addresses.
//!
//! Uses the typed group-address API (`Knx::group_address`): register each
//! address with its DPT once, then `.read()` sends a `GroupValueRead` and
//! decodes the response straight into that DPT's value type — no manual
//! byte parsing, and no way to accidentally decode one address's bytes as
//! another address's DPT.

use std::time::Duration;

use knust::protocol::address::{GroupAddress, IndividualAddress, MainGroup, MiddleGroup};
use knust::protocol::dpt::{DPTOccupancy, DPTSwitch, DPTTemperature};
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

    // Start telegram processing — `.read()` below sends a `GroupValueRead`
    // and waits for the matching response, which requires the outgoing-send
    // and incoming-dispatch tasks `start()` spawns.
    knx.start().await?;

    // Register each address once, with its DPT. `group_address` fails if
    // the address was already bound to a *different* DPT elsewhere.
    let light_switch = knx.group_address::<DPTSwitch>(GroupAddress::new(
        MainGroup::new(1),
        MiddleGroup::new(2),
        3,
    ))?;
    let temperature = knx.group_address::<DPTTemperature>(GroupAddress::new(
        MainGroup::new(2),
        MiddleGroup::new(1),
        1,
    ))?;
    let motion = knx.group_address::<DPTOccupancy>(GroupAddress::new(
        MainGroup::new(3),
        MiddleGroup::new(0),
        5,
    ))?;

    let timeout = Duration::from_secs(5);

    println!("Reading values from group addresses...");

    println!("Reading light switch ({})...", light_switch.address());
    match light_switch.read(timeout).await {
        Ok(value) => println!("  -> {}", if value.value() { "on" } else { "off" }),
        Err(e) => println!("  -> read failed (expected without a real bus): {e}"),
    }

    println!("Reading temperature sensor ({})...", temperature.address());
    match temperature.read(timeout).await {
        Ok(value) => println!("  -> {:.1}°C", value.value()),
        Err(e) => println!("  -> read failed (expected without a real bus): {e}"),
    }

    println!("Reading motion sensor ({})...", motion.address());
    match motion.read(timeout).await {
        Ok(value) => println!("  -> {}", if value.value() { "occupied" } else { "clear" }),
        Err(e) => println!("  -> read failed (expected without a real bus): {e}"),
    }

    // Disconnect
    knx.disconnect().await?;

    println!("Value reader example completed successfully!");
    Ok(())
}
