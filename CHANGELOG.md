# Changelog

All notable changes to this project are documented in this file.

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
