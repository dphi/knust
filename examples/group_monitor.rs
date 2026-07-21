//! Example: monitoring registered group addresses via the DPT registry.
//!
//! Covers the parts of the typed group-address API that `value_reader.rs`
//! and `send_telegrams.rs` don't: inspecting what's registered
//! (`Knx::group_address_dpt`/`group_address_state`/`registered_group_addresses`),
//! setting a refresh TTL so a stale address gets re-read automatically
//! (`Knx::set_group_address_refresh`), and reacting to new values as they
//! arrive via a filtered callback — the push counterpart to polling
//! `group_address_state`.

use std::time::Duration;

use knust::application::callbacks::{TelegramCallbackFn, TelegramFilter};
use knust::protocol::address::{GroupAddress, IndividualAddress, MainGroup, MiddleGroup};
use knust::protocol::dpt::{DPTOccupancy, DPTSwitch, DPTTemperature};
use knust::protocol::telegram::{Telegram, TelegramType};
use knust::{ConnectionConfig, ConnectionType, Knx};

/// Prints every real value (`GroupValueWrite`/`GroupValueResponse`) seen for
/// `address` — pairs with polling `Knx::group_address_state` for a push
/// notification instead of pulling on a schedule.
struct ValueLogger {
    address: GroupAddress,
}

#[async_trait::async_trait]
impl TelegramCallbackFn for ValueLogger {
    async fn call(&self, telegram: &Telegram) {
        // A GroupValueRead carries no value — nothing to log.
        if telegram.telegram_type == TelegramType::GroupValueRead {
            return;
        }
        println!(
            "  [monitor] new value for {}: {:02X?}",
            self.address, telegram.payload
        );
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: Some("192.168.1.100".parse()?),
        individual_address: IndividualAddress::new(1, 1, 240),
        ..Default::default()
    };
    let knx = Knx::new(config).await?;
    knx.connect().await?;

    // Register the addresses we care about, each with its DPT.
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
    knx.group_address::<DPTSwitch>(GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3))?;

    // If no real value shows up for the temperature sensor within 5
    // minutes, the background refresh task sends it a GroupValueRead.
    // `set_group_address_refresh` can be called before `start()` (as
    // here); it just has no effect until `start()` actually runs.
    knx.set_group_address_refresh(temperature.address(), Duration::from_secs(300))?;

    // React to new motion values immediately, instead of only polling.
    knx.register_telegram_callback_filtered(
        ValueLogger {
            address: motion.address(),
        },
        TelegramFilter::GroupAddresses(vec![motion.address()]),
        false, // don't also fire on our own outgoing telegrams
    )
    .await;

    // Starts telegram dispatch *and* the refresh task above.
    knx.start().await?;

    // Inspect what's registered, without touching the bus.
    println!("Registered group addresses:");
    for (address, dpt) in knx.registered_group_addresses() {
        println!("  {address}: DPT {}", dpt.number_str());
    }

    // Pull the last known value for one address — whatever the refresh
    // task or a real write last observed. `None` fields mean "never seen
    // yet", not an error.
    if let Some(state) = knx.group_address_state(temperature.address()) {
        println!(
            "Temperature state: ttl={:?} last_seen={:?} last_value={:?}",
            state.ttl, state.last_seen, state.last_seen_value
        );
    }

    println!("Monitoring — waiting for values (needs a real bus to observe anything)...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    knx.disconnect().await?;
    println!("Group monitor example completed successfully!");
    Ok(())
}
