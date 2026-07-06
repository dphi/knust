//! Example for the telegram monitor callback.
//!
//! Listen to telegrams on the KNX bus and print them.

use std::env;
use std::str::FromStr;
use std::time::Duration;
use tokio::time::sleep;

use knust::application::callbacks::TelegramCallbackFn;
use knust::protocol::address::IndividualAddress;
use knust::protocol::telegram::Telegram;
use knust::{ConnectionConfig, ConnectionType, Knx};

/// Prints each received telegram to stdout.
struct TelegramPrinter;

#[async_trait::async_trait]
impl TelegramCallbackFn for TelegramPrinter {
    async fn call(&self, telegram: &Telegram) {
        let source = telegram.source;
        let destination = telegram.destination;
        let payload_info = if telegram.payload.is_empty() {
            "Empty".to_string()
        } else {
            format!("{} bytes", telegram.payload.len())
        };

        println!("Telegram: {source} -> {destination} | Payload: {payload_info}");

        if !telegram.payload.is_empty() {
            let hex_payload: String = telegram
                .payload
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!("  Data: {hex_payload}");
        }
    }
}

fn show_help() {
    println!("Listen to telegrams on the KNX bus.");
    println!();
    println!("Usage:");
    println!("  cargo run --example telegram_monitor [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --ia <address>     Individual address to connect to (e.g., 1.0.253)");
    println!("  --gateway <ip>     Gateway IP address (e.g., 192.168.1.100)");
    println!("  --help             Print this help message");
    println!();
    println!("Example:");
    println!("  cargo run --example telegram_monitor -- --ia 1.0.253 --gateway 192.168.1.100");
}

async fn monitor(
    individual_address: Option<IndividualAddress>,
    gateway_ip: Option<std::net::IpAddr>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Configure connection
    let config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip,
        individual_address: individual_address.unwrap_or(IndividualAddress::new(1, 0, 253)),
        ..Default::default()
    };

    // Create Knx instance in daemon mode
    let knx = Knx::new(config).await?;

    // Register telegram callback
    let _callback_handle = knx.register_telegram_callback(TelegramPrinter).await;

    println!("Starting telegram monitor...");
    println!("Press Ctrl+C to stop");

    // Connect to KNX network
    knx.connect().await?;

    // Keep running until interrupted; telegrams are printed by the callback.
    loop {
        sleep(Duration::from_secs(1)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::init();

    let args: Vec<String> = env::args().collect();

    let mut individual_address = None;
    let mut gateway_ip = None;
    let mut i = 1;

    while i < args.len() {
        match args[i].as_str() {
            "--help" => {
                show_help();
                return Ok(());
            }
            "--ia" => {
                if i + 1 < args.len() {
                    individual_address = Some(IndividualAddress::from_str(&args[i + 1])?);
                    i += 2;
                } else {
                    eprintln!("Error: --ia requires an address argument");
                    return Ok(());
                }
            }
            "--gateway" => {
                if i + 1 < args.len() {
                    gateway_ip = Some(args[i + 1].parse()?);
                    i += 2;
                } else {
                    eprintln!("Error: --gateway requires an IP address argument");
                    return Ok(());
                }
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                show_help();
                return Ok(());
            }
        }
    }

    monitor(individual_address, gateway_ip).await
}
