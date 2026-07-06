//! Example for `DateTime` device.
//!
//! Connect to KNX/IP device and send current date/time to the bus.

use chrono::{Datelike, Timelike};
use std::time::Duration;
use tokio::time::sleep;

use knust::protocol::address::{GroupAddress, IndividualAddress};
use knust::{ConnectionConfig, ConnectionType, Knx};

// DPT 10/11 pack calendar fields (0..=59, 1..=31, ...) into single bytes;
// they're already range-bounded, so try_from would only add unreachable error paths.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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

    // Define group addresses for date/time
    let time_address = GroupAddress::from_parts(1, 4, 1)?;
    let date_address = GroupAddress::from_parts(1, 4, 2)?;

    println!("Sending current date and time to KNX bus...");

    // Get current date and time
    let now = std::time::SystemTime::now();
    let datetime = chrono::DateTime::<chrono::Local>::from(now);

    println!(
        "Current date/time: {}",
        datetime.format("%Y-%m-%d %H:%M:%S")
    );

    // TODO: Implement DateTime device and DPT 10.001 (Time) and DPT 11.001 (Date)
    // This would require:
    // 1. DateTime device implementation
    // 2. DPT 10.001 and DPT 11.001 encoding/decoding
    // 3. Telegram sending functionality in Knx

    println!("Sending time to {time_address}...");
    // Format: DPT 10.001 - 3 bytes: Day|Hour, Minute, Second
    let time_bytes = [
        ((datetime.weekday().num_days_from_monday() as u8) << 5) | (datetime.hour() as u8),
        datetime.minute() as u8,
        datetime.second() as u8,
    ];
    println!(
        "  Time bytes: {:02X} {:02X} {:02X}",
        time_bytes[0], time_bytes[1], time_bytes[2]
    );

    sleep(Duration::from_millis(500)).await;

    println!("Sending date to {date_address}...");
    // Format: DPT 11.001 - 3 bytes: Day, Month, Year (since 1900)
    let date_bytes = [
        datetime.day() as u8,
        datetime.month() as u8,
        (datetime.year() - 1900) as u8,
    ];
    println!(
        "  Date bytes: {:02X} {:02X} {:02X}",
        date_bytes[0], date_bytes[1], date_bytes[2]
    );

    // Disconnect
    knx.disconnect().await?;

    println!("DateTime example completed successfully!");
    println!("Note: Actual DateTime device and telegram sending requires implementation");
    Ok(())
}
