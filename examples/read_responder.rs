//! Example: answering `GroupValueRead` requests with a `GroupValueResponse`.
//!
//! `value_reader.rs` covers the client side (send a `GroupValueRead`, wait
//! for the response). This is the other side: acting as the device that
//! *answers* one. knust has no built-in "device" abstraction for this (same
//! philosophy as `custom_devices.rs`) — register a callback, check the
//! telegram is actually a `GroupValueRead` for your address, and reply with
//! [`Telegram::group_response_value`].
//!
//! The only thing enforced here is the DPT: `group_response_value::<DPTTemperature>`
//! won't encode a value that doesn't fit the type. Freshness, staleness,
//! races with other responders — none of that is this example's job.

use std::sync::Arc;
use tokio::sync::RwLock;

use knust::application::callbacks::{TelegramCallbackFn, TelegramFilter};
use knust::protocol::address::{GroupAddress, IndividualAddress, MainGroup, MiddleGroup};
use knust::protocol::dpt::DPTTemperature;
use knust::protocol::telegram::{Telegram, TelegramType};
use knust::{ConnectionConfig, ConnectionType, Knx};

/// Answers every `GroupValueRead` for `address` with whatever `current`
/// holds at the time.
struct Responder {
    address: GroupAddress,
    knx: Knx,
    current: Arc<RwLock<f32>>,
}

#[async_trait::async_trait]
impl TelegramCallbackFn for Responder {
    async fn call(&self, telegram: &Telegram) {
        // Only react to an actual read request — a write or another
        // device's response for the same address must not trigger one of
        // our own. (`include_outgoing: false` below already keeps this from
        // seeing our own response; this check is the second, independent
        // guard against ever answering the wrong thing.)
        if telegram.telegram_type != TelegramType::GroupValueRead {
            return;
        }

        let value = *self.current.read().await;

        match Telegram::group_response_value::<DPTTemperature>(self.address, value) {
            Ok(response) => {
                if let Err(e) = self.knx.send_telegram(&response).await {
                    eprintln!("  failed to send GroupValueResponse: {e}");
                } else {
                    println!("  answered read with {value:.1}°C");
                }
            }
            Err(e) => eprintln!("  failed to encode response: {e}"),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config = ConnectionConfig {
        connection_type: ConnectionType::Routing,
        individual_address: IndividualAddress::new(1, 1, 240),
        ..Default::default()
    };
    let knx = Knx::new(config).await?;
    knx.connect().await?;
    knx.start().await?;

    let address = GroupAddress::new(MainGroup::new(2), MiddleGroup::new(1), 1);
    // Shared with the registered callback — a real sensor driver would
    // write to this on every new reading.
    let current = Arc::new(RwLock::new(21.5));

    knx.register_telegram_callback_filtered(
        Responder {
            address,
            knx: knx.clone(),
            current: current.clone(),
        },
        TelegramFilter::GroupAddresses(vec![address]),
        false, // never react to our own outgoing telegrams
    )
    .await;

    println!("Responder ready — will answer GroupValueRead on {address} with 21.5°C");
    println!("(needs another device on a real bus to send the read request to observe this)");

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Simulate a real sensor driver reporting a new reading.
    *current.write().await = 22.1;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    knx.disconnect().await?;
    println!("Read responder example completed successfully!");
    Ok(())
}
