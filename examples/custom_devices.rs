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

use knust::protocol::RawGroupAddress;
use knust::protocol::dpt::{DPTSwitch, DPTTemperature};
use knust::{ConnectionType, GroupAddress, GroupSubscription, Knx, KnxError};

/// A minimal on/off actuator: one group address, one write.
struct Switch {
    handle: GroupAddress<DPTSwitch>,
}

impl Switch {
    fn new(knx: &Knx, address: RawGroupAddress) -> Result<Self, KnxError> {
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
    handle: GroupAddress<DPTTemperature>,
}

impl TemperatureSensor {
    fn new(knx: &Knx, address: RawGroupAddress) -> Result<Self, KnxError> {
        Ok(Self {
            handle: knx.group_address::<DPTTemperature>(address)?,
        })
    }

    /// Actively read the current value (sends a `GroupValueRead`, waits for the response).
    async fn read(&self) -> Result<f32, KnxError> {
        Ok(self.handle.read(Duration::from_secs(5)).await?.value())
    }

    /// Passively subscribe to future readings.
    fn subscribe(&self) -> Result<GroupSubscription<DPTTemperature>, KnxError> {
        self.handle.subscribe()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .individual_address("1.1.240".parse()?)
        .connect()
        .await?;

    // `Switch::new`/`TemperatureSensor::new` are fallible now: registering
    // the same address under a *different* DPT elsewhere would be a
    // conflict, caught here instead of silently corrupting reads/writes.
    let living_room_light = Switch::new(&knx, "1/2/1".parse()?)?;
    match living_room_light.turn_on().await {
        Ok(()) => println!("✓ Living room light switched on"),
        Err(e) => println!("⚠ Switch write failed (expected without a real bus): {e}"),
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    match living_room_light.turn_off().await {
        Ok(()) => println!("✓ Living room light switched off"),
        Err(e) => println!("⚠ Switch write failed (expected without a real bus): {e}"),
    }

    let temperature = TemperatureSensor::new(&knx, "2/1/1".parse()?)?;
    let updates = temperature.subscribe()?;

    match temperature.read().await {
        Ok(celsius) => println!("✓ Temperature read: {celsius:.1}°C"),
        Err(e) => println!("⚠ Temperature read failed (expected without a real bus): {e}"),
    }

    if let Some(Ok(value)) = updates.latest() {
        println!("Latest observed temperature: {:.1}°C", value.value());
    }
    knx.shutdown().await?;
    Ok(())
}
