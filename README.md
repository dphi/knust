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

- `dpt` (**on** by default) — datapoint-type encode/decode (DPT 1..251). Disable
  for a raw-frame-only client that never interprets group values.
- `ets` (off) — ETS CSV group-address import (`parse_ets_csv`). Implies `dpt`.
- `server` (off) — act as a KNXnet/IP tunneling server (`TunnelServer::bind`).
  Most consumers are clients and don't need it — see [Server](#server) below.
- `secure` (off) — KNX IP Secure (session handshake, verified against real
  hardware) and KNX Data Security (group encryption, **experimental** and
  unverified against a reference implementation), plus KNX keyring
  (`.knxkeys`) parsing/validation. Implies `ets`.

## Quick Start

Add Knx to your `Cargo.toml`:

```toml
[dependencies]
knust = "0.1.0"
tokio = { version = "1.0", features = ["rt-multi-thread", "macros"] }
```

### Basic Usage

There's no built-in device abstraction layer — you send telegrams and read
group values directly. See [`examples/custom_devices.rs`](examples/custom_devices.rs)
for a pattern to build a device layer on top of these primitives.

```rust
use knust::{Knx, ConnectionConfig, ConnectionType};
use knust::protocol::{Address, GroupAddress, IndividualAddress};
use knust::protocol::telegram::{Direction, Priority, Telegram, TelegramType};

#[tokio::main]
async fn main() -> Result<(), knust::KnxError> {
    // Configure connection
    let config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: Some("192.168.1.100".parse().unwrap()),
        individual_address: IndividualAddress::new(1, 1, 240),
        ..Default::default()
    };

    // Create Knx instance
    let knx = Knx::new(config).await?;

    // Connect to KNX network
    knx.connect().await?;

    // Switch a light on (GroupValueWrite to 1/2/3)
    let switch_on = Telegram {
        source: IndividualAddress::new(1, 1, 240),
        destination: Address::Group(GroupAddress::from_parts(1, 2, 3)?),
        payload: vec![0x01],
        priority: Priority::Normal,
        direction: Direction::Outgoing,
        telegram_type: TelegramType::GroupValueWrite,
        timestamp: std::time::SystemTime::now(),
    };
    knx.send_telegram(&switch_on).await?;

    // Disconnect
    knx.disconnect().await?;

    Ok(())
}
```

### Builder Pattern

```rust
use knust::Knx;

let knx = Knx::builder()
    .connection_type(ConnectionType::Tunneling)
    .gateway_ip("192.168.1.100".parse().unwrap())
    .individual_address(IndividualAddress::new(1, 1, 240))
    .timeout_ms(5000)
    .auto_reconnect(true)
    .build()
    .await?;
```

## Connection Types

### Tunneling (Point-to-Point)

Best for reliable communication with a single KNX/IP gateway:

```rust
let config = ConnectionConfig {
    connection_type: ConnectionType::Tunneling,
    gateway_ip: Some("192.168.1.100".parse().unwrap()),
    gateway_port: Some(3671),
    individual_address: IndividualAddress::new(1, 1, 240),
    ..Default::default()
};
```

### Routing (Multicast)

Best for monitoring multiple devices on the network:

```rust
let config = ConnectionConfig {
    connection_type: ConnectionType::Routing,
    local_ip: Some("192.168.1.50".parse().unwrap()),
    individual_address: IndividualAddress::new(1, 1, 241),
    ..Default::default()
};
```

### Secure Connections

For encrypted communication using KNX IP Secure (requires the `secure`
feature — without it, or without `security` set, `connect()` returns a
configuration error rather than silently falling back to plaintext):

```rust
use knust::transport::SecurityConfig;

let security_config = SecurityConfig {
    device_auth_password: "DeviceAuthPassword123!".to_string(),
    user_password: Some("MySecurePassword".to_string()),
    session_timeout: 300,
    keyring_path: Some("/path/to/keyring.knxkeys".to_string()),
};

let config = ConnectionConfig {
    connection_type: ConnectionType::SecureTunneling,
    gateway_ip: Some("192.168.1.100".parse().unwrap()),
    security: Some(security_config),
    ..Default::default()
};
```

## Reading Group Values

```rust
use std::time::Duration;

// Read a temperature sensor (DPT 9.001) and wait for the response
let payload = knx
    .read_group_value(GroupAddress::from_parts(2, 1, 1)?, Duration::from_secs(5))
    .await?;
let temperature = knust::protocol::dpt::Dpt::<knust::protocol::dpt::Temperature>::decode(&payload)?;
println!("Temperature: {:.1}°C", temperature.value().value());
```

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

Encrypt group communication using KNX Data Security. **Experimental** —
unlike KNX IP Secure (the session handshake above), this hasn't been
verified against a reference implementation or real hardware, and isn't
wired into the telegram send/receive path automatically; use
`security::group::encrypt_group_payload`/`decrypt_group_payload` directly
on APDU bytes:

```rust
use knust::security::{KeyRing, SecurityKey};

let mut keyring = KeyRing::new();

// Add group keys
let group_key = SecurityKey::from_hex("0123456789ABCDEF0123456789ABCDEF")?;
keyring.add_group_key(GroupAddress::from_parts(1, 2, 1)?, group_key);

// Check if group is secured
if keyring.is_group_secured(&GroupAddress::from_parts(1, 2, 1)?) {
    println!("Group 1/2/1 is secured");
}
```

### Sequence Number Validation

Protect against replay attacks:

```rust
let sender = IndividualAddress::new(1, 2, 3);
match keyring.validate_sequence(sender, sequence_number) {
    Ok(()) => println!("Sequence number valid"),
    Err(e) => println!("Replay attack detected: {}", e),
}
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

- **custom_devices**: recommended pattern for building a device layer on `send_telegram`/`read_group_value`
- **send_telegrams**: sending raw telegrams
- **value_reader**: reading and decoding group values
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
- **sequence_validation_demo**: UDP sequence number handling

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
- **MSRV**: 1.85.1
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
