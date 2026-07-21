//! How to build device-like abstractions on top of knust's typed group
//! addresses.
//!
//! knust has no built-in `Light`/`Switch`/`Sensor` types, but
//! `Knx::group_address` already gives you a compile-time-typed, DPT-checked
//! handle per address (`write`/`read`/`decode`) — a "device" is then just a
//! thin wrapper that names and composes a handful of those handles into one
//! domain object:
//!
//! - `Switch`: a write-only actuator, one `GroupAddress<DPTSwitch>`.
//! - `TemperatureSensor`: read + subscribe, one `GroupAddress<DPTTemperature>`,
//!   decoded through the DPT 9.001 value type itself instead of hand-rolled
//!   byte math.

use std::time::Duration;

use async_trait::async_trait;

use knust::application::callbacks::{TelegramCallbackFn, TelegramFilter};
use knust::protocol::address::{GroupAddress, IndividualAddress, MainGroup, MiddleGroup};
use knust::protocol::dpt::{DPTSwitch, DPTTemperature};
use knust::protocol::telegram::Telegram;
use knust::{ConnectionConfig, ConnectionType, Knx, KnxError};
// The typed group-address handle lives at the crate root as `GroupAddress`
// too — aliased here since the raw, untyped address (above) already claims
// that name in this file.
use knust::GroupAddress as TypedGroupAddress;

/// A minimal on/off actuator: one group address, one write.
struct Switch {
    handle: TypedGroupAddress<DPTSwitch>,
}

impl Switch {
    fn new(knx: &Knx, address: GroupAddress) -> Result<Self, KnxError> {
        Ok(Self {
            handle: knx.group_address::<DPTSwitch>(address)?,
        })
    }

    async fn set(&self, on: bool) -> Result<(), KnxError> {
        self.handle.write(on).await
    }

    async fn turn_on(&self) -> Result<(), KnxError> {
        self.set(true).await
    }

    async fn turn_off(&self) -> Result<(), KnxError> {
        self.set(false).await
    }
}

/// A read/subscribe sensor. Decodes through the DPT 9.001 handle itself
/// rather than parsing the 2-byte float by hand.
struct TemperatureSensor {
    knx: Knx,
    handle: TypedGroupAddress<DPTTemperature>,
}

/// `Knx::register_telegram_callback_filtered` takes an (async) `TelegramCallbackFn`,
/// not a plain closure — this is the standard wrapper shape for a sync one.
struct OnTemperatureUpdate<F: Fn(f32) + Send + Sync> {
    handle: TypedGroupAddress<DPTTemperature>,
    callback: F,
}

#[async_trait]
impl<F: Fn(f32) + Send + Sync> TelegramCallbackFn for OnTemperatureUpdate<F> {
    async fn call(&self, telegram: &Telegram) {
        // `decode` already checks the telegram is addressed to this
        // handle before touching the payload, and returns `None` rather
        // than panicking for anything else.
        if let Some(Ok(value)) = self.handle.decode(telegram) {
            (self.callback)(value.value());
        }
    }
}

impl TemperatureSensor {
    fn new(knx: &Knx, address: GroupAddress) -> Result<Self, KnxError> {
        Ok(Self {
            knx: knx.clone(),
            handle: knx.group_address::<DPTTemperature>(address)?,
        })
    }

    /// Actively read the current value (sends a `GroupValueRead`, waits for the response).
    async fn read(&self) -> Result<f32, KnxError> {
        Ok(self.handle.read(Duration::from_secs(5)).await?.value())
    }

    /// Passively subscribe to future readings (e.g. periodic sends from the device itself).
    async fn on_update<F>(&self, callback: F)
    where
        F: Fn(f32) + Send + Sync + 'static,
    {
        self.knx
            .register_telegram_callback_filtered(
                OnTemperatureUpdate {
                    handle: self.handle.clone(),
                    callback,
                },
                TelegramFilter::GroupAddresses(vec![self.handle.address()]),
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
        individual_address: IndividualAddress::new(1, 1, 240),
        ..Default::default()
    };
    let knx = Knx::new(config).await?;

    // `Switch::new`/`TemperatureSensor::new` are fallible now: registering
    // the same address under a *different* DPT elsewhere would be a
    // conflict, caught here instead of silently corrupting reads/writes.
    let living_room_light = Switch::new(
        &knx,
        GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 1),
    )?;
    match living_room_light.turn_on().await {
        Ok(()) => println!("✓ Living room light switched on"),
        Err(e) => println!("⚠ Switch write failed (expected without a real bus): {e}"),
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    match living_room_light.turn_off().await {
        Ok(()) => println!("✓ Living room light switched off"),
        Err(e) => println!("⚠ Switch write failed (expected without a real bus): {e}"),
    }

    let temperature = TemperatureSensor::new(
        &knx,
        GroupAddress::new(MainGroup::new(2), MiddleGroup::new(1), 1),
    )?;
    temperature
        .on_update(|celsius| println!("← temperature update: {celsius:.1}°C"))
        .await;

    match temperature.read().await {
        Ok(celsius) => println!("✓ Temperature read: {celsius:.1}°C"),
        Err(e) => println!("⚠ Temperature read failed (expected without a real bus): {e}"),
    }

    Ok(())
}
