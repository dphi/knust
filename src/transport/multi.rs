//! Multi-gateway connection coordinator.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, broadcast, mpsc};

#[cfg(feature = "secure")]
use crate::config::keyring::KeyringConfig;
use crate::error::{Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::telegram::Telegram;

use super::dedup::TelegramDedup;
use super::health::{ConnectionHealth, GatewayConnectionState};
use super::{GatewayConfig, MultiConnectionConfig, Tunnel};

/// Manages connections to multiple KNX/IP gateways with failover and deduplication.
///
/// This is the high-availability (HA) variant of a bus connection: one or
/// more gateways for a single bus, with automatic failover between them. It
/// implements [`GatewayConnection`](super::GatewayConnection), the same
/// send/subscribe/shutdown API a plain single-gateway connection would have.
pub struct MultiConnection {
    config: MultiConnectionConfig,
    /// Per-gateway health tracking (indexed by gateway priority order)
    health: Arc<RwLock<Vec<ConnectionHealth>>>,
    /// Live per-gateway tunnels, indexed identically to `health`. The tunnels are
    /// created and owned by the bus supervisor (see `application::bus`), which
    /// registers/unregisters them here so the outbound `send_telegram` path can
    /// reach the active connection without owning its lifecycle. `None` means the
    /// gateway has no live tunnel registered.
    connections: Arc<RwLock<Vec<Option<Arc<Tunnel>>>>>,
    /// Broadcast channel for merged incoming telegrams
    telegram_tx: broadcast::Sender<Telegram>,
    /// Deduplication window tracking
    dedup: TelegramDedup,
    /// Shutdown flag
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    /// Notifies waiters (recv/heartbeat loops) promptly when shutdown is requested
    shutdown_notify: Arc<tokio::sync::Notify>,
    /// Outgoing telegram sender
    outgoing_tx: mpsc::Sender<Telegram>,
    /// Outgoing telegram receiver (taken by the drain loop)
    outgoing_rx: Arc<std::sync::Mutex<Option<mpsc::Receiver<Telegram>>>>,
    /// Count of telegrams dropped because the outgoing channel was full
    outgoing_dropped: Arc<AtomicU64>,
}

impl MultiConnection {
    /// Create a new multi-connection coordinator.
    #[must_use]
    pub fn new(config: MultiConnectionConfig) -> Self {
        let (telegram_tx, _) = broadcast::channel(1024);
        let (outgoing_tx, outgoing_rx) = mpsc::channel(256);
        let health: Vec<ConnectionHealth> = config
            .gateways
            .iter()
            .map(|gw| ConnectionHealth::new(gw.address))
            .collect();
        let connections: Vec<Option<Arc<Tunnel>>> =
            (0..config.gateways.len()).map(|_| None).collect();
        let dedup = TelegramDedup::new(config.deduplication_window);

        log_transport!(
            LogLevel::Info,
            "MultiConnection created with {} gateways",
            config.gateways.len()
        );

        Self {
            config,
            health: Arc::new(RwLock::new(health)),
            connections: Arc::new(RwLock::new(connections)),
            telegram_tx,
            dedup,
            shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            shutdown_notify: Arc::new(tokio::sync::Notify::new()),
            outgoing_tx,
            outgoing_rx: Arc::new(std::sync::Mutex::new(Some(outgoing_rx))),
            outgoing_dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Subscribe to the merged telegram stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Telegram> {
        self.telegram_tx.subscribe()
    }

    /// Send a telegram to the outgoing channel.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::QueueFull`] if the outgoing channel is full.
    pub fn send(&self, telegram: Telegram) -> Result<()> {
        let result = self
            .outgoing_tx
            .try_send(telegram)
            .map_err(|_| TransportError::QueueFull.into());
        if result.is_err() {
            self.outgoing_dropped.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// Snapshot of the outgoing transport queue: (queued, capacity, dropped).
    pub fn outgoing_queue_stats(&self) -> (usize, usize, u64) {
        let capacity = self.outgoing_tx.max_capacity();
        let available = self.outgoing_tx.capacity();
        let queued = capacity - available;
        let dropped = self.outgoing_dropped.load(Ordering::Relaxed);
        (queued, capacity, dropped)
    }

    /// Take the outgoing receiver (for the drain loop). Returns None if already taken.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn take_outgoing_rx(&self) -> Option<mpsc::Receiver<Telegram>> {
        self.outgoing_rx.lock().unwrap().take()
    }

    /// Get health status for all gateways.
    pub async fn health(&self) -> Vec<(GatewayConfig, ConnectionHealth)> {
        let health = self.health.read().await;
        self.config
            .gateways
            .iter()
            .zip(health.iter())
            .map(|(gw, h)| (gw.clone(), h.clone()))
            .collect()
    }

    /// Get the primary (best-scoring healthy) gateway index.
    pub async fn primary_gateway(&self) -> Option<usize> {
        let health = self.health.read().await;
        health
            .iter()
            .enumerate()
            .filter(|(_, h)| h.state == GatewayConnectionState::Connected)
            .max_by(|(_, a), (_, b)| {
                a.score()
                    .partial_cmp(&b.score())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    /// Register a live tunnel for gateway `idx`. Called by the bus supervisor once
    /// the per-gateway tunnel is connected. Out-of-bounds indices are ignored.
    pub async fn register_connection(&self, idx: usize, conn: Arc<Tunnel>) {
        let mut conns = self.connections.write().await;
        if let Some(slot) = conns.get_mut(idx) {
            *slot = Some(conn);
        }
    }

    /// Clear the live tunnel for gateway `idx`. Called by the bus supervisor when
    /// the tunnel is lost. Out-of-bounds indices are ignored.
    pub async fn unregister_connection(&self, idx: usize) {
        let mut conns = self.connections.write().await;
        if let Some(slot) = conns.get_mut(idx) {
            *slot = None;
        }
    }

    /// Send a telegram out over the bus via the primary (best-scoring healthy)
    /// gateway, failing over once to the next-best healthy gateway on error.
    ///
    /// Health bookkeeping is owned by the bus supervisor; on a send failure we
    /// only drop the failed tunnel slot here (we do NOT mark the gateway
    /// unhealthy) to avoid fighting the supervisor's reconnect logic.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::ConnectionClosed`] if no gateway is
    /// currently healthy, or the underlying tunnel's send error if both the
    /// primary and its failover gateway (if any) fail to send.
    pub async fn send_telegram(&self, telegram: &Telegram) -> Result<()> {
        let primary = self
            .primary_gateway()
            .await
            .ok_or(TransportError::ConnectionClosed)?;

        let first_err = match self.try_send_via(primary, telegram).await {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };

        // Send failed: drop the failed slot and fail over to the next-best healthy
        // gateway (if any), retrying exactly once.
        log_transport!(
            LogLevel::Warn,
            "Gateway {} send failed: {}",
            primary,
            first_err
        );
        self.unregister_connection(primary).await;

        match self.best_healthy_gateway_excluding(primary).await {
            Some(next) => {
                log_transport!(
                    LogLevel::Warn,
                    "Primary gateway {} send failed, failing over to {}",
                    primary,
                    next
                );
                self.try_send_via(next, telegram).await
            }
            None => Err(first_err),
        }
    }

    /// Send a telegram over the tunnel registered for `idx`. Clones the
    /// `Arc<Tunnel>` out of the read guard and drops the guard before awaiting,
    /// so the `connections` lock is never held across the `.await` on send.
    async fn try_send_via(&self, idx: usize, telegram: &Telegram) -> Result<()> {
        let conn = {
            let conns = self.connections.read().await;
            conns.get(idx).and_then(std::clone::Clone::clone)
        };
        let conn = conn.ok_or(TransportError::ConnectionClosed)?;

        let name = self.gateway_name(idx).unwrap_or("unnamed");
        log_transport!(
            LogLevel::Debug,
            "Sending telegram via primary gateway {} ({})",
            idx,
            name
        );

        let frame = build_tunnelling_frame(telegram, conn.channel_id(), conn.next_sequence());
        let result = conn.send_frame(&frame).await;
        {
            let mut health = self.health.write().await;
            if let Some(h) = health.get_mut(idx) {
                h.record_telegram_sent();
                if result.is_err() {
                    h.record_telegram_send_error();
                }
            }
        }
        result
    }

    /// Best-scoring connected gateway other than `exclude`. Used to pick a
    /// failover target without mutating health (which the supervisor owns), since
    /// `primary_gateway()` alone would still return the just-failed index.
    async fn best_healthy_gateway_excluding(&self, exclude: usize) -> Option<usize> {
        let health = self.health.read().await;
        health
            .iter()
            .enumerate()
            .filter(|(i, h)| *i != exclude && h.state == GatewayConnectionState::Connected)
            .max_by(|(_, a), (_, b)| {
                a.score()
                    .partial_cmp(&b.score())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }
    /// Returns false if the telegram is a duplicate.
    pub async fn dispatch_telegram(&self, telegram: Telegram) -> bool {
        if self.dedup.check_and_record(&telegram).await {
            log_transport!(LogLevel::Trace, "Telegram deduplicated");
            return false;
        }

        let _ = self.telegram_tx.send(telegram);
        true
    }

    /// Returns the number of configured gateways.
    pub fn gateway_count(&self) -> usize {
        self.config.gateways.len()
    }

    /// Returns the socket addresses of all configured gateways.
    pub fn gateway_addresses(&self) -> Vec<std::net::SocketAddr> {
        self.config.gateways.iter().map(|gw| gw.address).collect()
    }

    /// Returns the name of a gateway by index.
    pub fn gateway_name(&self, idx: usize) -> Option<&str> {
        self.config
            .gateways
            .get(idx)
            .and_then(|gw| gw.name.as_deref())
    }

    /// Returns the connection type of a gateway by index.
    pub fn gateway_connection_type(&self, idx: usize) -> Option<&super::ConnectionType> {
        self.config.gateways.get(idx).map(|gw| &gw.connection_type)
    }

    /// Populate an [`AddressRegistry`](super::AddressRegistry)'s known-occupied set from a parsed keyring.
    ///
    /// A keyring (`.knxkeys`) lists every KNX/IP interface (tunnelling slot) of a
    /// gateway, each with its own individual address. Any of those addresses that
    /// we are *not* going to occupy ourselves is a real, in-use tunnelling slot on
    /// the gateway, so auto-selection must avoid claiming it. This marks every such
    /// address as known-occupied in the registry (see
    /// [`AddressRegistry::add_known_occupied`](super::AddressRegistry::add_known_occupied)).
    ///
    /// ## "Our own" matching rule
    /// An interface is considered **ours** — and therefore *not* marked occupied —
    /// if its `individual_address` equals one of our gateways' configured
    /// `individual_address` (i.e. some `config.gateways[*].individual_address ==
    /// Some(iface.individual_address)`). That slot will be claimed by us at connect
    /// time, so reserving it here would needlessly block our own connection.
    ///
    /// Matching by configured individual address is the only reliable,
    /// host-format-independent key: keyring `host` values and our gateway socket
    /// addresses are not directly comparable (hostname vs. resolved IP, port, etc.).
    ///
    /// ## Common case: no configured gateway addresses
    /// If none of our gateways have a configured `individual_address` (the common
    /// case — addresses are auto-selected at connect), then by the rule above
    /// *none* of the keyring interfaces are "ours", so **all** keyring interface
    /// addresses are marked known-occupied. This is the safe default: auto-selection
    /// then avoids every address the keyring knows about and picks a free one.
    #[cfg(feature = "secure")]
    pub fn populate_known_occupied_from_keyring(
        &self,
        registry: &super::AddressRegistry,
        keyring: &KeyringConfig,
    ) {
        for iface in &keyring.interfaces {
            let is_ours = self
                .config
                .gateways
                .iter()
                .any(|gw| gw.individual_address == Some(iface.individual_address));

            if is_ours {
                continue;
            }

            let addr = iface.individual_address;
            let host = &iface.host;
            registry.add_known_occupied(addr);
            log_transport!(
                LogLevel::Debug,
                "Keyring: marking address {} as occupied (interface '{}')",
                addr,
                host
            );
        }
    }

    /// Signal shutdown.
    pub fn shutdown(&self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.shutdown_notify.notify_waiters();
        log_transport!(LogLevel::Info, "MultiConnection shutdown requested");
    }

    /// Get the shutdown notification handle (wakes loops promptly on shutdown).
    pub fn shutdown_notify(&self) -> Arc<tokio::sync::Notify> {
        self.shutdown_notify.clone()
    }

    /// Check if shutdown has been requested.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Record a successful connection for a gateway.
    pub async fn record_connected(&self, gateway_idx: usize) {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(gateway_idx) {
            h.record_connected();
            let name = self
                .config
                .gateways
                .get(gateway_idx)
                .and_then(|g| g.name.as_deref())
                .unwrap_or("unnamed");
            log_transport!(
                LogLevel::Info,
                "Gateway '{}' ({}) connected",
                name,
                h.gateway
            );
        }
    }

    /// Record a disconnection for a gateway.
    pub async fn record_disconnected(&self, gateway_idx: usize) {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(gateway_idx) {
            h.record_disconnected();
            let name = self
                .config
                .gateways
                .get(gateway_idx)
                .and_then(|g| g.name.as_deref())
                .unwrap_or("unnamed");
            log_transport!(
                LogLevel::Warn,
                "Gateway '{}' ({}) disconnected",
                name,
                h.gateway
            );
        }
    }

    /// Record a telegram received from a specific gateway.
    pub async fn record_telegram_received(&self, gateway_idx: usize) {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(gateway_idx) {
            h.record_telegram_received();
        }
    }

    /// Record a receive-side parse error for a specific gateway.
    pub async fn record_telegram_receive_error(&self, gateway_idx: usize) {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(gateway_idx) {
            h.record_telegram_receive_error();
        }
    }

    /// Record a heartbeat result for a gateway.
    pub async fn record_heartbeat(
        &self,
        gateway_idx: usize,
        success: bool,
        latency_ms: Option<u64>,
    ) {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(gateway_idx) {
            if success {
                h.record_heartbeat_success(latency_ms.unwrap_or(0));
            } else {
                h.record_heartbeat_failure();
            }
        }
    }

    /// Test-only accessor: whether a live tunnel is registered for `idx`.
    #[cfg(test)]
    async fn has_connection(&self, idx: usize) -> bool {
        self.connections
            .read()
            .await
            .get(idx)
            .is_some_and(std::option::Option::is_some)
    }
}

#[async_trait::async_trait]
impl super::GatewayConnection for MultiConnection {
    async fn send(&self, telegram: Telegram) -> Result<()> {
        MultiConnection::send(self, telegram)
    }

    fn subscribe(&self) -> broadcast::Receiver<Telegram> {
        MultiConnection::subscribe(self)
    }

    fn shutdown(&self) {
        MultiConnection::shutdown(self);
    }

    fn is_shutdown(&self) -> bool {
        MultiConnection::is_shutdown(self)
    }
}

/// Build a serialized KNX/IP `TunnellingRequest` frame for an outgoing telegram.
///
/// Kept private to the transport layer to avoid a layering inversion (transport
/// must not depend on `application::knx`). Mirrors the tunnelling branch of
/// `application::knust::build_outgoing_frame_for_connection`: map the telegram to a
/// `GroupValueService` (Read when the payload is empty, otherwise Write), encode
/// its APCI, wrap it in an `L_Data.req` CEMI frame, then serialize that into a
/// `TunnellingRequest` carrying the given channel id and sequence number.
///
/// `pub(crate)` so `transport::server` can reuse the same encode path for a
/// server session sending telegrams out to a connected client (using
/// `MessageCode::LDataInd`, since from the server's side this represents a
/// bus event being indicated to the client, not a client-initiated request).
pub(crate) fn build_tunnelling_frame_with_code(
    telegram: &Telegram,
    channel_id: u8,
    sequence: u8,
    message_code: crate::protocol::cemi::MessageCode,
) -> Vec<u8> {
    use crate::protocol::cemi::CemiFrame;
    use crate::protocol::knxip::{KnxIpFrame, ServiceType, TunnellingRequest};

    let group_service = if telegram.payload.is_empty() {
        crate::protocol::GroupValueService::Read
    } else {
        crate::protocol::GroupValueService::Write(telegram.payload.clone())
    };
    let apci_data = group_service.encode();
    let cemi = CemiFrame::new(
        message_code,
        telegram.source,
        telegram.destination,
        apci_data,
    );
    let raw_cemi = cemi.serialize();
    let request = TunnellingRequest::new(channel_id, sequence, raw_cemi);
    KnxIpFrame::new(ServiceType::TunnellingRequest, request.serialize()).serialize()
}

pub(crate) fn build_tunnelling_frame(telegram: &Telegram, channel_id: u8, sequence: u8) -> Vec<u8> {
    build_tunnelling_frame_with_code(
        telegram,
        channel_id,
        sequence,
        crate::protocol::cemi::MessageCode::LDataReq,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::address::{Address, GroupAddress, IndividualAddress};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn test_config() -> MultiConnectionConfig {
        MultiConnectionConfig {
            gateways: vec![
                GatewayConfig {
                    address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 3671),
                    name: Some("Gateway A".to_string()),
                    priority: 0,
                    connection_type: super::super::ConnectionType::Tunneling,
                    individual_address: None,
                },
                GatewayConfig {
                    address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 11)), 3671),
                    name: Some("Gateway B".to_string()),
                    priority: 1,
                    connection_type: super::super::ConnectionType::Tunneling,
                    individual_address: None,
                },
            ],
            ..Default::default()
        }
    }

    fn test_telegram() -> Telegram {
        Telegram::new_incoming(
            IndividualAddress::new(1, 1, 5),
            Address::Group(GroupAddress::from_parts(1, 2, 3).unwrap()),
            vec![0x01],
        )
    }

    #[tokio::test]
    async fn test_deduplication() {
        let mc = MultiConnection::new(test_config());
        let mut rx = mc.subscribe();

        let telegram = test_telegram();

        // First dispatch should succeed
        assert!(mc.dispatch_telegram(telegram.clone()).await);
        // Second identical dispatch within window should be deduplicated
        assert!(!mc.dispatch_telegram(telegram.clone()).await);

        // Should receive exactly one telegram
        let received = rx.try_recv();
        assert!(received.is_ok());
        // Second try should be empty
        let received2 = rx.try_recv();
        assert!(received2.is_err());
    }

    #[tokio::test]
    async fn test_health_tracking() {
        let mc = MultiConnection::new(test_config());

        // Initially no primary (nothing connected)
        assert!(mc.primary_gateway().await.is_none());

        // Connect gateway 0
        mc.record_connected(0).await;
        assert_eq!(mc.primary_gateway().await, Some(0));

        // Connect gateway 1 too
        mc.record_connected(1).await;
        // Both connected with equal score; max_by returns last match
        assert_eq!(mc.primary_gateway().await, Some(1));

        // Disconnect gateway 0
        mc.record_disconnected(0).await;
        // Gateway 1 should now be primary
        assert_eq!(mc.primary_gateway().await, Some(1));
    }

    #[tokio::test]
    // A disconnected score() is a literal 0.0, not a computed value; exact equality is correct here.
    #[allow(clippy::float_cmp)]
    async fn test_health_scores() {
        let mc = MultiConnection::new(test_config());
        mc.record_connected(0).await;
        mc.record_heartbeat(0, true, Some(15)).await;

        let health = mc.health().await;
        assert!(health[0].1.score() > 0.5);
        assert_eq!(health[1].1.score(), 0.0); // disconnected
    }

    #[tokio::test]
    async fn test_send_and_receive() {
        let mc = MultiConnection::new(test_config());
        let mut rx = mc.take_outgoing_rx().expect("receiver should be available");

        // Second take should return None
        assert!(mc.take_outgoing_rx().is_none());

        let telegram = test_telegram();
        mc.send(telegram.clone()).unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.source, telegram.source);
        assert_eq!(received.destination, telegram.destination);
        assert_eq!(received.payload, telegram.payload);
    }

    // --- Task 10: outbound send_telegram / failover ---
    //
    // NOTE: the full live send + failover path cannot be unit-tested without a
    // mock KNX/IP gateway (it performs real socket I/O via `Tunnel::send_frame`).
    // These tests cover the parts that are deterministic offline: the private
    // frame builder, the connection registry, and the no-primary error path.

    #[tokio::test]
    async fn multi_send_build_tunnelling_frame_produces_parseable_request() {
        use crate::protocol::knxip::{KnxIpFrame, ServiceType, TunnellingRequest};

        let telegram = test_telegram();

        let bytes = build_tunnelling_frame(&telegram, 1, 0);

        // KNX/IP header carries the service type at bytes [2..4]; TunnellingRequest
        // is 0x0420.
        assert_eq!(&bytes[2..4], &[0x04, 0x20]);

        // It must parse back into a TunnellingRequest whose CEMI round-trips.
        let frame = KnxIpFrame::parse(&bytes).unwrap();
        assert_eq!(frame.header.service_type, ServiceType::TunnellingRequest);
        let req = TunnellingRequest::parse(&frame.body).unwrap();
        assert!(!req.raw_cemi.is_empty());
    }

    #[tokio::test]
    async fn multi_send_register_unregister_updates_connections() {
        let mc = MultiConnection::new(test_config());
        let tunnel = Arc::new(Tunnel::new_udp("127.0.0.1:3671".parse().unwrap()));

        // Initially no slot has a connection.
        assert!(!mc.has_connection(0).await);

        mc.register_connection(0, tunnel.clone()).await;
        assert!(mc.has_connection(0).await);
        assert!(!mc.has_connection(1).await);

        mc.unregister_connection(0).await;
        assert!(!mc.has_connection(0).await);

        // Out-of-bounds indices are ignored (no panic).
        mc.register_connection(99, tunnel).await;
        assert!(!mc.has_connection(99).await);
    }

    #[tokio::test]
    async fn multi_send_with_no_connected_gateways_errors() {
        use crate::error::{KnxError, TransportError};

        let mc = MultiConnection::new(test_config());
        // Nothing connected → primary_gateway() is None → ConnectionClosed.
        assert!(mc.primary_gateway().await.is_none());

        let telegram = test_telegram();
        let err = mc.send_telegram(&telegram).await.unwrap_err();
        assert!(matches!(
            err,
            KnxError::Transport(TransportError::ConnectionClosed)
        ));
    }

    // --- Task 11: populate AddressRegistry known-occupied set from keyring ---

    #[cfg(feature = "secure")]
    use crate::config::keyring::{KeyringConfig, KeyringInterface, KeyringMetadata};
    #[cfg(feature = "secure")]
    use crate::security::SecurityKey;
    #[cfg(feature = "secure")]
    use crate::transport::AddressRegistry;

    #[cfg(feature = "secure")]
    fn keyring_iface(addr: IndividualAddress, host: &str) -> KeyringInterface {
        KeyringInterface {
            individual_address: addr,
            interface_type: "Tunneling".to_string(),
            host: host.to_string(),
            user_id: 1,
            user_password: SecurityKey::new(vec![0u8; 16]),
            device_authentication: None,
            backbone_key: None,
        }
    }

    #[cfg(feature = "secure")]
    fn keyring_with(ifaces: Vec<KeyringInterface>) -> KeyringConfig {
        KeyringConfig {
            metadata: KeyringMetadata {
                created: None,
                creator: None,
                project: None,
                signature: None,
            },
            interfaces: ifaces,
            devices: Vec::new(),
            group_addresses: Vec::new(),
        }
    }

    #[cfg(feature = "secure")]
    fn keyring_address_config_with_gateway_addr(
        addr: Option<IndividualAddress>,
    ) -> MultiConnectionConfig {
        MultiConnectionConfig {
            gateways: vec![GatewayConfig {
                address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 3671),
                name: Some("Gateway A".to_string()),
                priority: 0,
                connection_type: super::super::ConnectionType::Tunneling,
                individual_address: addr,
            }],
            ..Default::default()
        }
    }

    #[cfg(feature = "secure")]
    #[test]
    fn keyring_address_excludes_our_own_configured_slot() {
        let i0 = IndividualAddress::new(1, 1, 1);
        let i1 = IndividualAddress::new(1, 1, 2);
        let i2 = IndividualAddress::new(1, 1, 3);
        let keyring = keyring_with(vec![
            keyring_iface(i0, "host-0"),
            keyring_iface(i1, "host-1"),
            keyring_iface(i2, "host-2"),
        ]);

        // Our single gateway is configured with interface[1]'s address, so that
        // slot is "ours" and must NOT be marked occupied.
        let mc = MultiConnection::new(keyring_address_config_with_gateway_addr(Some(i1)));
        let registry = AddressRegistry::new();

        mc.populate_known_occupied_from_keyring(&registry, &keyring);

        // interface[0] and interface[2] are other slots → known-occupied.
        assert!(!registry.is_available(i0));
        assert!(!registry.is_available(i2));
        // interface[1] is ours → left available.
        assert!(registry.is_available(i1));
    }

    #[cfg(feature = "secure")]
    #[test]
    fn keyring_address_marks_all_when_no_gateway_address_configured() {
        let i0 = IndividualAddress::new(1, 1, 1);
        let i1 = IndividualAddress::new(1, 1, 2);
        let i2 = IndividualAddress::new(1, 1, 3);
        let keyring = keyring_with(vec![
            keyring_iface(i0, "host-0"),
            keyring_iface(i1, "host-1"),
            keyring_iface(i2, "host-2"),
        ]);

        // No configured gateway individual_address → none are "ours" → all marked.
        let mc = MultiConnection::new(keyring_address_config_with_gateway_addr(None));
        let registry = AddressRegistry::new();

        mc.populate_known_occupied_from_keyring(&registry, &keyring);

        assert!(!registry.is_available(i0));
        assert!(!registry.is_available(i1));
        assert!(!registry.is_available(i2));
    }
}
