# Changelog

All notable changes to this project are documented in this file.

## [0.3.0]

### Added

- An optional `knust` CLI with gateway discovery, raw and typed group reads,
  confirmed writes, and filtered monitoring. Install it with the `cli` feature.
- Typed group address subscriptions and many additional KNX datapoint codecs,
  including HVAC status and setpoints, DALI emergency lighting, metering, and weather.
- Connectionless and connected point-to-point management, including property and
  memory access, programming mode, and individual address assignment. Dedicated
  KNXnet/IP device-management channels support UDP, TCP, and IP Secure.
- Extended gateway discovery, unicast descriptions, and tunnel slot details.
  `TunnelServer` can answer unicast and multicast discovery requests.

### Changed

- The minimum supported Rust version is 1.88.
- `knust::GroupAddress<T>` binds an address to a datapoint type. Protocol-level
  addresses use `knust::protocol::RawGroupAddress`.
- Use `Knx::builder().tunnel(endpoint).connect().await` to open a ready-to-use
  client. The connection endpoint is a `SocketAddr`, and the timeout is a
  `Duration`. `state()` and `is_connected()` are synchronous.
- Typed writes wait for transmission and cEMI confirmation in tunnels. Use
  `enqueue_write()` when queue acceptance is enough. Unsent telegrams expire,
  and reconnecting continues with the next queued telegram.
- The high-level KNX IP Secure tunnel uses TCP and supports choosing the ETS
  user. Secure control and data frames are authenticated. Data Secure group
  traffic enforces configured keys and checks incoming replay counters.

### Fixed

- Improved tunnel reconnect and shutdown handling. Dropping the last server
  handle closes its listeners and idle streams.
- Corrected tunnel acknowledgments, channel ownership, and the use of
  gateway-assigned source addresses. Routing honors the selected IPv4 interface.
- Hardened cEMI and KNXnet/IP parsing and made ETS group-address imports atomic.

## [0.1.0]

Initial release.

### Features

- Async/await KNX/IP client built on tokio: Tunneling (UDP/TCP) and Routing
  (multicast) connections, gateway discovery, and automatic reconnection with
  configurable backoff.
- Zero-copy protocol parsing for KNXnet/IP frames, cEMI, and datapoint types
  (DPT 1 through 251) — enabled by default via the `dpt` feature.
- `ets` feature: ETS CSV group-address import.
- `secure` feature: KNX IP Secure session handshake (verified against real
  hardware), KNX Data Security group encryption (**experimental** — not
  yet verified against a reference implementation or real hardware; see
  `security::group` module docs), and KNX keyring (`.knxkeys`) parsing with
  configuration validation.
- `server` feature: act as a KNXnet/IP tunneling server (`TunnelServer`).
- Structured, typed error handling (`KnxError` and per-layer error types)
  with `# Errors`/`# Panics` documentation on the public API.
- Component-scoped logging, memory usage monitoring, and connection pooling.

### Known limitations

- No ETS project (`.knxproj`/XML) import; only ETS CSV group-address export
  and KNX keyring (`.knxkeys`) files are supported.
- Binary `.knxkeys` parsing is not implemented; only the XML export format
  is supported. Binary input returns a clear parse error.
- KNX Data Security (group encryption) is experimental; only KNX IP Secure
  (the session/transport layer) has been checked against real hardware.
- No generic TOML/JSON application-configuration format; only keyring file
  parsing is supported (see the `Configuration` type in `knust::config`).
