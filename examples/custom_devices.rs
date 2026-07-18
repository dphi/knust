//! How to build device-like abstractions on top of knust.
//!
//! knust has no built-in device layer (no `Light`, `Switch`, `Sensor`
//! types) — just telegrams, group addresses, and DPT encode/decode. This
//! example shows the recommended pattern for wrapping those into your own
//! typed device structs:
//!
//! - `Switch`: a write-only actuator built on `Knx::send_telegram`.
//! - `TemperatureSensor`: read + subscribe, decoding DPT 9.001 via the
//!   crate's own `Dpt<Temperature>` type instead of hand-rolled byte math.

use std::time::Duration;

use async_trait::async_trait;

use knust::application::callbacks::{TelegramCallbackFn, TelegramFilter};
use knust::protocol::address::{Address, GroupAddress, IndividualAddress};
use knust::protocol::dpt::{Dpt, Temperature};
use knust::protocol::telegram::{Direction, Priority, Telegram, TelegramType};
use knust::{ConnectionConfig, ConnectionType, Knx, KnxError};

/// A minimal on/off actuator: one group address, one write.
struct Switch {
    knx: Knx,
    source: IndividualAddress,
    address: GroupAddress,
}

impl Switch {
    fn new(knx: Knx, source: IndividualAddress, address: GroupAddress) -> Self {
        Self {
            knx,
            source,
            address,
        }
    }

    async fn set(&self, on: bool) -> Result<(), KnxError> {
        let telegram = Telegram {
            source: self.source,
            destination: Address::Group(self.address),
            payload: vec![u8::from(on)],
            priority: Priority::Normal,
            direction: Direction::Outgoing,
            telegram_type: TelegramType::GroupValueWrite,
            timestamp: std::time::SystemTime::now(),
        };
        self.knx.send_telegram(&telegram).await
    }

    async fn turn_on(&self) -> Result<(), KnxError> {
        self.set(true).await
    }

    async fn turn_off(&self) -> Result<(), KnxError> {
        self.set(false).await
    }
}

/// A read/subscribe sensor. Decodes with the crate's DPT 9.001 type
/// (`Dpt<Temperature>`) rather than parsing the 2-byte float by hand.
struct TemperatureSensor {
    knx: Knx,
    address: GroupAddress,
}

/// `Knx::register_telegram_callback_filtered` takes an (async) `TelegramCallbackFn`,
/// not a plain closure — this is the standard wrapper shape for a sync one.
struct OnTemperatureUpdate<F: Fn(f32) + Send + Sync> {
    address: GroupAddress,
    callback: F,
}

#[async_trait]
impl<F: Fn(f32) + Send + Sync> TelegramCallbackFn for OnTemperatureUpdate<F> {
    async fn call(&self, telegram: &Telegram) {
        if telegram.destination == Address::Group(self.address)
            && let Ok(dpt) = Dpt::<Temperature>::decode(&telegram.payload)
        {
            (self.callback)(dpt.value().value());
        }
    }
}

impl TemperatureSensor {
    fn new(knx: Knx, address: GroupAddress) -> Self {
        Self { knx, address }
    }

    /// Actively read the current value (sends a `GroupValueRead`, waits for the response).
    async fn read(&self) -> Result<f32, KnxError> {
        let payload = self
            .knx
            .read_group_value(self.address, Duration::from_secs(5))
            .await?;
        let dpt = Dpt::<Temperature>::decode(&payload)?;
        Ok(dpt.value().value())
    }

    /// Passively subscribe to future readings (e.g. periodic sends from the device itself).
    async fn on_update<F>(&self, callback: F)
    where
        F: Fn(f32) + Send + Sync + 'static,
    {
        self.knx
            .register_telegram_callback_filtered(
                OnTemperatureUpdate {
                    address: self.address,
                    callback,
                },
                TelegramFilter::GroupAddresses(vec![self.address]),
                false, // don't fire for our own outgoing telegrams
            )
            .await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config = ConnectionConfig {
        connection_type: ConnectionType::Routing,
        ..Default::default()
    };
    let knx = Knx::new(config).await?;
    let source = IndividualAddress::new(1, 1, 240);

    let living_room_light = Switch::new(knx.clone(), source, GroupAddress::from_parts(1, 2, 1)?);
    match living_room_light.turn_on().await {
        Ok(()) => println!("✓ Living room light switched on"),
        Err(e) => println!("⚠ Switch write failed (expected without a real bus): {e}"),
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    match living_room_light.turn_off().await {
        Ok(()) => println!("✓ Living room light switched off"),
        Err(e) => println!("⚠ Switch write failed (expected without a real bus): {e}"),
    }

    let temperature = TemperatureSensor::new(knx.clone(), GroupAddress::from_parts(2, 1, 1)?);
    temperature
        .on_update(|celsius| println!("← temperature update: {celsius:.1}°C"))
        .await;

    match temperature.read().await {
        Ok(celsius) => println!("✓ Temperature read: {celsius:.1}°C"),
        Err(e) => println!("⚠ Temperature read failed (expected without a real bus): {e}"),
    }

    Ok(())
}
