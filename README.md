# knust - a asynchronous KNX/IP Library for Rust

[![Crates.io](https://img.shields.io/crates/v/knust.svg)](https://crates.io/crates/knust)
[![Documentation](https://docs.rs/knust/badge.svg)](https://docs.rs/knust)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

knust is a high-performance, memory-safe implementation of the KNX/IP protocol for building automation systems. It provides async/await support and strong type safety while maintaining compatibility with KNX standards.

## Features

- **Async/await support** with tokio for non-blocking I/O
- **Memory-safe protocol parsing** with zero unsafe code in public API
- **Multiple connection types**: Tunneling (point-to-point) and Routing (multicast)
- **KNX IP Secure** and experimental **KNX Data Security** support
- **Gateway discovery** for automatic network configuration
- **Comprehensive error handling** with structured error types
- **Property-based testing** for correctness verification
- **Performance monitoring** and memory management
- **Extensive logging** with component-specific levels

## Cargo Features

- `dpt` (**on** by default) — typed codecs for KNX datapoints. Disable
  for a raw-frame-only client that never interprets group values.
- `ets` (off) — ETS CSV group-address import (`parse_ets_csv`). Implies `dpt`.
- `server` (off) — act as a KNXnet/IP tunneling server (`TunnelServer::bind`).
  Most consumers are clients and don't need it — see [Server](#server) below.
- `cli` (off) — install the `knust` command for discovery, reads, writes, and
  monitoring. Enables `dpt`.
- `secure` (off) — KNX IP Secure, Data Secure group encryption and replay
  protection, and ETS keyring (`.knxkeys`) parsing. Implies `ets`.

## Quick Start

Add Knx to your `Cargo.toml`:

```toml
[dependencies]
knust = "0.3"
tokio = { version = "1.0", features = ["rt-multi-thread", "macros"] }
```

### Basic Usage

There's no built-in device abstraction layer, but `Knx::group_address` gives
you a compile-time-typed, DPT-checked handle per group address — register
an address with its DPT once, then `write`/`read`/`decode` on the handle
instead of hand-building `Telegram`s. See
[`examples/custom_devices.rs`](examples/custom_devices.rs) for composing
handles like this into device structs.

```rust
use knust::Knx;
use knust::protocol::dpt::DPTSwitch;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let knx = Knx::builder()
        .tunnel("192.168.1.100:3671".parse()?)
        .individual_address("1.1.240".parse()?)
        .connect()
        .await?;

    // Register 1/2/3 as DPT 1.001 (Switch). Fails if it's already bound
    // to a *different* DPT elsewhere — every address gets exactly one.
    let light = knx.group_address::<DPTSwitch>("1/2/3".parse()?)?;

    // `write` accepts a bool, `DPTSwitch` itself, or a string like "on" —
    // all checked against the bound DPT; only the string can fail at
    // runtime, the others can't resolve to the wrong DPT at all.
    light.write(true).await?;

    knx.shutdown().await?;

    Ok(())
}
```

For a raw-bytes escape hatch — no DPT binding, you build the `Telegram`
yourself — see [`examples/send_telegrams.rs`](examples/send_telegrams.rs).

### Builder Pattern

```rust
use knust::Knx;
use std::time::Duration;

let knx = Knx::builder()
    .tunnel("192.168.1.100:3671".parse()?)
    .timeout(Duration::from_secs(5))
    .auto_reconnect(true)
    .build()
    .await?;
```

## Command line

Build and install the optional CLI with `cargo install --path . --features cli`.

```sh
knust scan --local-ip 192.168.1.50
knust group read 1/2/3 --type temperature --gateway 192.168.1.100
knust group write 1/2/4 on --gateway 192.168.1.100
knust group monitor --filter '1/2/*' --gateway 192.168.1.100
```

Group commands accept `KNX_GATEWAY` and `KNX_LOCAL_IP` as defaults. When the
gateway is omitted, discovery must find exactly one compatible gateway.
Use `--tcp` for TCP tunneling. An untyped write accepts `on` or `off`, an
integer from 0 to 63, or hex octets such as `0x01`.

## Connection Types

### Tunneling (Point-to-Point)

Best for reliable communication with a single KNX/IP gateway:

```rust
let config = ConnectionConfig {
    connection_type: ConnectionType::Tunneling,
    gateway: Some("192.168.1.100:3671".parse().unwrap()),
    individual_address: "1.1.240".parse()?,
    ..Default::default()
};
```

### Routing (Multicast)

Best for monitoring multiple devices on the network:

```rust
let config = ConnectionConfig {
    connection_type: ConnectionType::Routing,
    local_ip: Some("192.168.1.50".parse().unwrap()),
    individual_address: "1.1.241".parse()?,
    ..Default::default()
};
```

### Secure Connections

KNX IP Secure tunneling uses TCP and requires the `secure` feature.

```rust
use knust::transport::SecurityConfig;

let security = SecurityConfig::new("device-auth password", "tunnel user password")
    .with_user_id(2)?;
let knx = Knx::builder()
    .secure_tunnel("192.168.1.100:3671".parse()?, security)
    .connect()
    .await?;
```

## Typed Group Addresses

`Knx::group_address::<T>(address)` binds a group address to DPT `T` and
returns a `GroupAddress<T>` handle. `T` is one of the DPT value types in
`knust::protocol::dpt` (`DPTSwitch`, `DPTScaling`, `DPTTemperature`, ...);
registering the same address with a different DPT returns an error.
Use `knust::protocol::RawGroupAddress` when working directly with telegrams
or addresses whose DPT is determined at runtime.

Use a running client from `Knx::builder().connect().await?` for typed I/O.
`write().await` waits for transmission and, on a tunnel, a successful cEMI
confirmation. Use `enqueue_write().await` when queue acceptance is sufficient.
After a connection failure, automatic reconnect resumes with the next queued
telegram. The error from an interrupted write can leave its delivery uncertain.
Queued telegrams expire after 30 seconds if their first send has yet to start.

```rust
use std::time::Duration;
use knust::protocol::dpt::DPTTemperature;

// Register once; reuse the handle for every read/write after that.
let temp = knx.group_address::<DPTTemperature>("2/1/1".parse()?)?;

// Sends a GroupValueRead and decodes the response as DPT 9.001 directly.
let value = temp.read(Duration::from_secs(5)).await?;
println!("Temperature: {:.1}°C", value.value());
```

`write` accepts the DPT's own value type, its plain inner value (`bool` for
switches, `u8` for scaling, ...), or a string — see [Basic
Usage](#basic-usage). `decode(&telegram)` does the read-side equivalent for
a telegram you already have (e.g. inside a callback), returning `None` if
it's addressed elsewhere instead of panicking.

For code that doesn't hold a specific handle — a bus monitor iterating
telegrams for addresses it discovers at runtime — `Knx::decode_group_value`
looks the DPT up in the same registry, and fails soft (`None`) for anything
unregistered instead of panicking:

```rust
match knx.decode_group_value(&telegram) {
    Some(Ok(view)) => println!("{}: {:?}", telegram.destination, view.raw()),
    Some(Err(e)) => println!("{}: decode error: {e}", telegram.destination),
    None => {} // address not registered — nothing to decode
}
```

Bindings can also be registered without a compile-time `T` via
`Knx::register_group_address_dyn(address, dpt)` — for bulk-loading from an
ETS export (`parse_ets_csv`, `ets` feature) or other runtime-known DPTs.

### Inspecting the Registry

Every registered address's binding can be read back without touching the
bus:

```rust
// Just the DPT it's bound to.
let dpt = knx.group_address_dpt(address); // Option<DptType>

// DPT, refresh TTL, and the last real value observed (if any) — purely
// local, doesn't send anything. Compare `read_group_value`/`T::read`,
// which always round-trips a GroupValueRead.
if let Some(state) = knx.group_address_state(address) {
    println!("last value: {:?} (seen {:?} ago)", state.last_seen_value, state.last_seen);
}

// Every registered (address, dpt) pair, order unspecified.
for (address, dpt) in knx.registered_group_addresses() {
    println!("{address}: {}", dpt.number_str());
}
```

`Knx::unregister_group_address(address)` removes a binding and frees the
address to be registered under a different DPT; any `GroupAddress<T>`
handles obtained before the unregister re-check the registry on their next
`write`/`read`/`decode` and error rather than keep using their now-stale
`T`.

### Monitoring and Refresh

To keep an address's value fresh without polling the bus yourself, set a
refresh TTL — if no real `GroupValueWrite`/`GroupValueResponse` is observed
for it within that window, a background task sends a `GroupValueRead`:

```rust
knx.set_group_address_refresh(address, Duration::from_secs(300))?;
knx.start().await?; // the refresh task only runs once start() has been called
```

`set_group_address_refresh` can be called before `start()` (it just
records the TTL and logs a warning), but the task that actually acts on it
is spawned by `start()`, so call `start()` at some point or the TTL never
takes effect. `Knx::clear_group_address_refresh(address)` undoes it.

With the TTL in place, `group_address_state(address).last_seen_value` above
is a cheap way to *pull* the current value on demand. To *push* instead —
react the moment a new value arrives — register a filtered callback:

```rust
use knust::application::callbacks::TelegramFilter;

knx.register_telegram_callback_filtered(
    my_callback,
    TelegramFilter::GroupAddresses(vec![address]),
    false, // don't also fire on our own outgoing telegrams
).await;
```

`TelegramFilter::GroupAddresses` matches on destination address across
*every* telegram type, including `GroupValueRead` (which carries no
value) — check `telegram.telegram_type` yourself if you only care about
real values. See [`examples/group_monitor.rs`](examples/group_monitor.rs)
for the full pattern.

### Answering Reads

`Telegram::group_response_value::<T>(address, value)` builds an outgoing
`GroupValueResponse`, with the same DPT-checked value ergonomics as
`group_write_value`. Nothing in the library requires this to be sent in
reply to an actual `GroupValueRead` — `send_telegram` will happily send a
`GroupValueResponse` at any time, solicited or not:

```rust
let response = Telegram::group_response_value::<DPTTemperature>(address, 21.5)?;
knx.send_telegram(&response).await?;
```

For the common case of only answering real read requests, check
`telegram.telegram_type == TelegramType::GroupValueRead` in your callback
before responding — see
[`examples/read_responder.rs`](examples/read_responder.rs). And only ever
respond with a value you actually have: a response carrying a stale
default tells the bus something false, which the library has no way to
detect on your behalf.

## Gateway Discovery

Automatically discover KNX/IP gateways on your network:

```rust
use knust::transport::GatewayScanner;

let scanner = GatewayScanner::new().await?;
let gateways = scanner.discover(Duration::from_secs(10)).await?;

for gateway in gateways {
    println!("Found gateway: {} at {}", gateway.name, gateway.addr);
    println!("  Supports tunneling: {}", gateway.capabilities.supports_tunneling);
    println!("  Supports routing: {}", gateway.capabilities.supports_routing);
}
```

## Security Features

The following require the `secure` feature.

### KNX Data Security

Load the ETS keyring and install it before starting the client. Knust encrypts
outgoing telegrams for configured group addresses. On receive, it authenticates
Data Secure telegrams and checks the sender's sequence automatically.

```rust
use knust::config::keyring::KeyringParser;

let keys = KeyringParser::parse_file_with_password("project.knxkeys", password).await?;
let keyring = keys.into_data_secure_keyring_for("1.1.240".parse()?);
knx.set_data_secure_keyring(keyring).await;
```

## Logging and Debugging

Configure comprehensive logging for troubleshooting:

```rust
use knust::{LoggingConfig, LogLevel, Component, logging::init_logging};

let mut logging_config = LoggingConfig::new();
logging_config.set_default_level(LogLevel::Info);
logging_config.set_component_level(Component::Transport, LogLevel::Debug);
logging_config.set_component_level(Component::Protocol, LogLevel::Trace);
logging_config.set_protocol_events(true);
logging_config.set_hex_dump(true);

init_logging(logging_config);
```

## Memory Management

Monitor and optimize memory usage:

```rust
// Get memory statistics
let stats = knx.memory_stats().await;
println!("Current usage: {} bytes", stats.current_usage);
println!("Peak usage: {} bytes", stats.peak_usage);

// Check memory bounds
if !knx.memory_within_bounds() {
    println!("Memory usage exceeds limits!");
}

// Force cleanup
let freed = knx.force_cleanup().await;
println!("Freed {} bytes", freed);
```

## Error Handling

Knx provides comprehensive error types with context:

```rust
use knust::KnxError;

match knx.connect().await {
    Ok(()) => println!("Connected successfully"),
    Err(KnxError::Transport(e)) => println!("Transport error: {}", e),
    Err(KnxError::Protocol(e)) => println!("Protocol error: {}", e),
    Err(KnxError::Device(e)) => println!("Device error: {}", e),
    Err(KnxError::Security(e)) => println!("Security error: {}", e),
    Err(e) => println!("Other error: {}", e),
}
```

## Examples

The repository includes examples covering common tasks (`cargo run --example <name>`,
some require the `secure`/`ets` features):

- **custom_devices**: building device structs on typed `GroupAddress<T>` handles
- **send_telegrams**: typed writes, plus the raw `Telegram`/`send_telegram` escape hatch
- **value_reader**: typed reads across a few different DPTs
- **read_responder**: answering a `GroupValueRead` with a `GroupValueResponse`
- **group_monitor**: inspecting the registry, refresh TTLs, and reacting to new values via callback
- **gateway_discovery**: discovering gateways on the network
- **secure_connection**: KNX IP Secure / Data Security
- **configuration_examples**: `ConnectionConfig` variations
- **logging_demo**: component-level logging setup
- **memory_optimization_demo**: memory monitoring and cleanup
- **tunneling_connection** / **tcp_connection_demo**: UDP and TCP tunneling
- **connection_handling**: reconnect/backoff handling
- **packet_listener** / **telegram_monitor**: passive bus monitoring
- **datetime_sync**: sending DPT 10/11 time and date
- **color_dpt_demo**: RGB/RGBW/XYY color DPTs

Run `ls examples/` for the full, current list.

## Configuration Files

The optional `ets` feature adds ETS CSV group-address import
(`parse_ets_csv`). The optional `secure` feature adds a KNX keyring
(`.knxkeys`) parser — see `Configuration` and `KeyringParser` in
`knust::config`. There is no ETS project (`.knxproj`/XML) import or generic
TOML/JSON application-config format.

## Server

**`TunnelServer`** (this crate, `server` feature) acts as a KNXnet/IP
gateway: `TunnelServer::bind(addr, individual_address).await?` binds a
UDP+TCP tunneling endpoint and starts serving connecting clients
immediately (`bind_secure` additionally requires KNX IP Secure). It's a
software bridge endpoint, not a full line of real device addresses.

## Testing

Knx includes comprehensive test coverage with both unit tests and property-based tests:

```bash
# Run all tests
cargo test

# Run property-based tests
cargo test property_

# Run integration tests
cargo test --test integration_tests

# Run with logging
RUST_LOG=debug cargo test
```

## Performance

Knx is designed for high performance:

- **Zero-copy parsing** where possible
- **Async I/O** with tokio for scalability
- **Memory pooling** for connection management
- **Hot path optimization** for telegram processing
- **Configurable memory limits** and cleanup

## Compatibility

- **Rust Edition**: 2024
- **MSRV**: 1.89
- **KNX Standards**: KNX/IP, KNX IP Secure; KNX Data Security is experimental (see crate docs)
- **Platforms**: Linux, macOS, Windows
- **Async Runtime**: tokio

## Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- Based on the [xknx Python library](https://github.com/xknx/xknx)
- KNX Association for the protocol specifications
- Rust community for excellent async ecosystem
