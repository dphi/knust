//! Example for sending values to KNX group addresses.
//!
//! Leads with the typed group-address API (`Knx::group_address`) — the
//! recommended way to send: register an address with its DPT once, and
//! `write()` then accepts a native value (`bool`, `u8`, ...), that DPT's
//! own value type, or a human string, all checked against the bound DPT.
//! The end of this file shows two lower-level escape hatches the typed API
//! is built on: `Telegram::group_write_value` for callers who want that same
//! `true`/`1`/`"on"` ergonomics without registering the address first, and
//! `Telegram::group_write` for callers who already have bytes from
//! elsewhere and want no DPT involved at all.

use std::time::Duration;
use tokio::time::sleep;

use knust::protocol::address::{GroupAddress, IndividualAddress, MainGroup, MiddleGroup};
use knust::protocol::dpt::{DPTScaling, DPTSwitch};
use knust::protocol::telegram::Telegram;
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

    println!("Sending typed values...");

    // Register once; `write` is then checked against DPT 1.001 (Switch) —
    // passing e.g. a percentage here wouldn't compile.
    let switch = knx.group_address::<DPTSwitch>(GroupAddress::new(
        MainGroup::new(1),
        MiddleGroup::new(2),
        3,
    ))?;

    println!("Switching on ({})...", switch.address());
    switch.write(true).await?;

    sleep(Duration::from_secs(2)).await;

    println!("Switching off ({})...", switch.address());
    switch.write(false).await?;

    sleep(Duration::from_secs(1)).await;

    // Same story for DPT 5.001 (Scaling) — `write` also accepts a parsed
    // string (`switch.write("on")`) for config/CLI-driven callers.
    let brightness = knx.group_address::<DPTScaling>(GroupAddress::new(
        MainGroup::new(1),
        MiddleGroup::new(2),
        4,
    ))?;

    println!("Setting brightness ({})...", brightness.address());
    brightness.write(128u8).await?; // 50% brightness

    // ---- Escape hatch 1: same value ergonomics, no registration -------
    // `group_write_value` resolves/validates/encodes against DPT 1.001
    // exactly like `switch.write(true)` above, but builds the `Telegram`
    // directly — no `Knx::group_address` call needed first.
    let raw_typed_telegram = Telegram::group_write_value::<DPTSwitch>(
        GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3),
        true,
    )?;

    println!(
        "Sending typed-but-unregistered telegram to {}...",
        raw_typed_telegram.destination
    );
    knx.send_telegram(&raw_typed_telegram).await?;

    // ---- Escape hatch 2: raw bytes, no DPT involved at all -------------
    // For forwarding opaque bytes from elsewhere. No need to fill in
    // `source`: `send_telegram` stamps it with this bus's own configured
    // individual address before sending.
    let raw_telegram = Telegram::group_write(
        GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3),
        vec![0x01],
    );

    println!("Sending raw telegram to {}...", raw_telegram.destination);
    knx.send_telegram(&raw_telegram).await?;

    // Disconnect
    knx.disconnect().await?;

    println!("Send telegrams example completed successfully!");
    Ok(())
}
