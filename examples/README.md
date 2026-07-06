# Knx Rust Examples

This directory contains examples demonstrating the Knx Rust library functionality, ported from the original Python Knx examples.

## Running Examples

To run any example:

```bash
cargo run --example <example_name>
```

For example:
```bash
cargo run --example basic_demo
```

## Available Examples

- **`custom_devices`** - How to build device-like abstractions (a Switch, a Sensor) on top of `send_telegram`/`read_group_value`/`register_telegram_callback` — there's no built-in device layer, this is the recommended pattern
- **`gateway_discovery`** - Discover KNX/IP gateways on the network
- **`secure_connection`** - Establish secure KNX/IP connections
- **`configuration_examples`** - Various configuration options
- **`logging_demo`** - Logging configuration and usage
- **`memory_optimization_demo`** - Memory management features
- **`telegram_monitor`** - Monitor KNX telegrams
- **`send_telegrams`** - Send custom telegrams
- **`value_reader`** - Read values from group addresses
- **`datetime_sync`** - Date/time synchronization
- **`connection_handling`** - Connection management and error handling
- **`tunnel_smoke`** - Read-only smoke test against a real gateway
- **`secure_tunnel_smoke`** - Read-only smoke test against a real KNX IP Secure gateway

## Configuration

Most examples use a default configuration that connects to a KNX/IP gateway at `192.168.1.100`. You can modify the examples to use your specific gateway IP address.

Example configuration:
```rust
let config = ConnectionConfig {
    connection_type: ConnectionType::Tunneling,
    gateway_ip: Some("192.168.1.100".parse()?),
    individual_address: IndividualAddress::new(1, 1, 240),
    ..Default::default()
};
```

## Notes

- Examples will attempt to connect to a real KNX network but will continue with offline demonstration if connection fails
- The `basic_demo` example provides the most comprehensive overview of library functionality
- Some examples may require additional implementation for full functionality (marked as "TODO" in comments)
- All examples include proper error handling and logging setup

## Python Knx Compatibility

These examples are designed to be functionally equivalent to the Python Knx examples, demonstrating the same concepts and operations in Rust with async/await patterns and strong type safety.
