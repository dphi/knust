//! Example for sending raw telegrams to KNX bus.
//!
//! Connect to KNX/IP device and send custom telegrams.

use std::time::Duration;
use tokio::time::sleep;

use knust::protocol::address::{Address, GroupAddress, IndividualAddress};
use knust::protocol::telegram::{Direction, Priority, Telegram, TelegramType};
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

    println!("Sending custom telegrams...");

    // Send a switch ON telegram
    let switch_on_telegram = Telegram {
        source: IndividualAddress::new(1, 1, 240),
        destination: Address::Group(GroupAddress::from_parts(1, 2, 3)?),
        payload: vec![0x01], // ON value
        priority: Priority::Normal,
        direction: Direction::Outgoing,
        telegram_type: TelegramType::GroupValueWrite,
        timestamp: std::time::SystemTime::now(),
    };

    println!(
        "Sending switch ON telegram to {}...",
        switch_on_telegram.destination
    );
    knx.send_telegram(&switch_on_telegram).await?;

    sleep(Duration::from_secs(2)).await;

    // Send a switch OFF telegram
    let switch_off_telegram = Telegram {
        source: IndividualAddress::new(1, 1, 240),
        destination: Address::Group(GroupAddress::from_parts(1, 2, 3)?),
        payload: vec![0x00], // OFF value
        priority: Priority::Normal,
        direction: Direction::Outgoing,
        telegram_type: TelegramType::GroupValueWrite,
        timestamp: std::time::SystemTime::now(),
    };

    println!(
        "Sending switch OFF telegram to {}...",
        switch_off_telegram.destination
    );
    knx.send_telegram(&switch_off_telegram).await?;

    sleep(Duration::from_secs(1)).await;

    // Send a brightness value telegram
    let brightness_telegram = Telegram {
        source: IndividualAddress::new(1, 1, 240),
        destination: Address::Group(GroupAddress::from_parts(1, 2, 4)?),
        payload: vec![128], // 50% brightness
        priority: Priority::Normal,
        direction: Direction::Outgoing,
        telegram_type: TelegramType::GroupValueWrite,
        timestamp: std::time::SystemTime::now(),
    };

    println!(
        "Sending brightness telegram to {}...",
        brightness_telegram.destination
    );
    knx.send_telegram(&brightness_telegram).await?;

    // Disconnect
    knx.disconnect().await?;

    println!("Send telegrams example completed successfully!");
    Ok(())
}
