//! Unified KNX/IP tunnel over a [`FrameTransport`] (UDP or TCP).
//!
//! The KNX-layer state machine — ConnectRequest/Response handshake, channel id,
//! sequence numbers, TunnellingRequest/Ack, `TransportState` heartbeat and
//! `DisconnectRequest` — is identical for both transports. Only framing and
//! liveness differ, handled by the transport plus a couple of flags:
//!
//! * `use_heartbeat` — UDP needs the `TransportState` heartbeat for liveness;
//!   TCP uses it only as a NAT/idle keepalive (on by default).
//! * `send_acks` — UDP acknowledges incoming `TunnellingRequests`; TCP relies on
//!   the stream and does not.
//!
//! Constructors perform no network I/O; the transport is established lazily in
//! [`Tunnel::connect`] (or `Tunnel::connect_secure` with the `secure` feature).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use tokio::time::{timeout, timeout_at};

#[cfg(feature = "secure")]
use super::SecurityConfig;
use super::address_probe::auto_select_address;
use super::address_registry::AddressRegistry;
use super::connection::{Connection, ConnectionStats, TransportState};
use super::frame_transport::{FrameTransport, TcpFrameTransport, TransportKind, UdpFrameTransport};
use super::heartbeat::{HeartbeatConfig, HeartbeatEvent, HeartbeatMonitor};
use super::router::FrameRouter;
use crate::error::{ProtocolError, Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::address::IndividualAddress;
use crate::protocol::knxip::{
    ConnectRequest, ConnectResponse, ConnectionstateRequest, ConnectionstateResponse,
    DisconnectRequest, DisconnectResponse, Hpai, KnxIpFrame, ServiceType, TunnellingAck,
};
#[cfg(feature = "secure")]
use crate::protocol::knxip::{SessionAuthenticate, SessionRequest, SessionResponse, SessionStatus};
#[cfg(feature = "secure")]
use crate::security::SecureSession;
#[cfg(all(test, feature = "secure"))]
use crate::security::SessionConfig;

/// Result of validating an incoming tunnelling request's sequence number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SequenceValidationResult {
    /// Sequence number is the expected one; process the frame.
    Valid,
    /// Sequence number is one less than expected (duplicate); ack but drop.
    Duplicate,
    /// Sequence number is out of order; potential lost frame.
    Invalid { expected: u8, received: u8 },
}

#[cfg(feature = "secure")]
#[derive(Default)]
struct SecureActivity {
    last: tokio::sync::Mutex<Option<Instant>>,
    changed: tokio::sync::Notify,
}

#[cfg(feature = "secure")]
impl SecureActivity {
    async fn touch(&self) {
        *self.last.lock().await = Some(Instant::now());
        self.changed.notify_one();
    }

    async fn last(&self) -> Instant {
        let mut last = self.last.lock().await;
        *last.get_or_insert_with(Instant::now)
    }
}

/// A KNX/IP tunnel connection, transport-agnostic.
pub struct Tunnel {
    kind: TransportKind,
    transport: std::sync::Mutex<Option<Arc<dyn FrameTransport>>>,
    gateway_addr: SocketAddr,
    local_ip: Option<std::net::IpAddr>,
    local_addr: std::sync::Mutex<Option<SocketAddr>>,
    local_hpai: Hpai,
    channel_id: u8,
    sequence_counter: AtomicU8,
    expected_sequence_number: AtomicU8,
    state: std::sync::RwLock<TransportState>,
    stats: std::sync::RwLock<ConnectionStats>,
    connection_timeout: Duration,
    established_at: Option<Instant>,
    router: Arc<FrameRouter>,
    send_lock: tokio::sync::Mutex<()>,
    heartbeat: std::sync::RwLock<Option<Arc<HeartbeatMonitor>>>,
    #[cfg(feature = "secure")]
    secure_session: Option<Arc<tokio::sync::RwLock<SecureSession>>>,
    #[cfg(feature = "secure")]
    secure_activity: Arc<SecureActivity>,
    #[cfg(feature = "secure")]
    secure_keepalive_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    #[cfg(feature = "secure")]
    secure_send_lock: Arc<tokio::sync::Mutex<()>>,
    /// Whether the `TransportState` heartbeat should run for this tunnel.
    pub(crate) use_heartbeat: bool,
    /// Whether incoming `TunnellingRequests` must be acknowledged (UDP only).
    pub(crate) send_acks: bool,
    /// Address explicitly configured for this tunnel; requested via Extended CRI
    /// during [`Tunnel::connect`]. `None` unless address management is enabled.
    configured_address: Option<IndividualAddress>,
    /// Registry for collision-safe address claiming. `None` disables all address
    /// management, in which case `connect`/`disconnect` behave exactly as before.
    address_registry: Option<AddressRegistry>,
    /// The individual address in effect for this tunnel once connected.
    individual_address: std::sync::Mutex<Option<IndividualAddress>>,
}

impl Tunnel {
    /// Create a UDP tunnel to `gateway_addr` (default 5s timeout). No I/O yet.
    #[must_use]
    pub fn new_udp(gateway_addr: SocketAddr) -> Self {
        Self::new_udp_with_timeout(gateway_addr, Duration::from_secs(5))
    }

    /// Create a UDP tunnel with a custom connection timeout. No I/O yet.
    #[must_use]
    pub fn new_udp_with_timeout(gateway_addr: SocketAddr, connection_timeout: Duration) -> Self {
        Self::build(
            TransportKind::Udp,
            gateway_addr,
            Hpai::route_back(),
            connection_timeout,
            true, // heartbeat
            true, // send acks
        )
    }

    /// Create a TCP tunnel to `gateway_addr` (default 5s timeout). No I/O yet.
    #[must_use]
    pub fn new_tcp(gateway_addr: SocketAddr) -> Self {
        Self::new_tcp_with_timeout(gateway_addr, Duration::from_secs(5))
    }

    /// Create a TCP tunnel with a custom connection timeout. No I/O yet.
    #[must_use]
    pub fn new_tcp_with_timeout(gateway_addr: SocketAddr, connection_timeout: Duration) -> Self {
        Self::build(
            TransportKind::Tcp,
            gateway_addr,
            Hpai::new_tcp_route_back(),
            connection_timeout,
            true,  // heartbeat (NAT keepalive)
            false, // TCP does not ack at the tunnelling layer here
        )
    }

    fn build(
        kind: TransportKind,
        gateway_addr: SocketAddr,
        local_hpai: Hpai,
        connection_timeout: Duration,
        use_heartbeat: bool,
        send_acks: bool,
    ) -> Self {
        Self {
            kind,
            transport: std::sync::Mutex::new(None),
            gateway_addr,
            local_ip: None,
            local_addr: std::sync::Mutex::new(None),
            local_hpai,
            channel_id: 0,
            sequence_counter: AtomicU8::new(0),
            expected_sequence_number: AtomicU8::new(0),
            state: std::sync::RwLock::new(TransportState::Disconnected),
            stats: std::sync::RwLock::new(ConnectionStats::default()),
            connection_timeout,
            established_at: None,
            router: Arc::new(FrameRouter::new()),
            send_lock: tokio::sync::Mutex::new(()),
            heartbeat: std::sync::RwLock::new(None),
            #[cfg(feature = "secure")]
            secure_session: None,
            #[cfg(feature = "secure")]
            secure_activity: Arc::new(SecureActivity::default()),
            #[cfg(feature = "secure")]
            secure_keepalive_cancel: None,
            #[cfg(feature = "secure")]
            secure_send_lock: Arc::new(tokio::sync::Mutex::new(())),
            use_heartbeat,
            send_acks,
            configured_address: None,
            address_registry: None,
            individual_address: std::sync::Mutex::new(None),
        }
    }

    /// The local socket address once connected (0.0.0.0:0 before).
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
            .lock()
            .unwrap()
            .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0)))
    }

    /// Enable address management (builder style), opting in without changing the
    /// public constructor signatures. Provides a shared [`AddressRegistry`] for
    /// collision-safe claiming plus an optional explicitly configured address
    /// (requested from the gateway via Extended CRI during [`Tunnel::connect`]).
    ///
    /// Consumes and returns `self` for chaining after `new_udp*`/`new_tcp*`.
    #[must_use]
    pub fn with_address_management(
        mut self,
        registry: AddressRegistry,
        configured: Option<IndividualAddress>,
    ) -> Self {
        self.address_registry = Some(registry);
        self.configured_address = configured;
        self
    }

    /// Enable address management in place (see [`Tunnel::with_address_management`]).
    pub fn set_address_management(
        &mut self,
        registry: AddressRegistry,
        configured: Option<IndividualAddress>,
    ) {
        self.address_registry = Some(registry);
        self.configured_address = configured;
    }

    /// The individual address in effect for this tunnel once connected, if any.
    ///
    /// Always `None` when address management is not enabled.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn individual_address(&self) -> Option<IndividualAddress> {
        *self.individual_address.lock().unwrap()
    }

    /// Get the active transport, or an error if not connected.
    fn transport(&self) -> Result<Arc<dyn FrameTransport>> {
        self.transport
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| TransportError::ConnectionClosed.into())
    }

    /// Establish the underlying transport (lazy; called by `connect/connect_secure`).
    async fn establish_transport(&self) -> Result<Arc<dyn FrameTransport>> {
        let (transport, local): (Arc<dyn FrameTransport>, SocketAddr) = match self.kind {
            TransportKind::Udp => {
                let (t, l) =
                    UdpFrameTransport::connect_from(self.gateway_addr, self.local_ip).await?;
                (Arc::new(t), l)
            }
            TransportKind::Tcp => {
                let (t, l) = TcpFrameTransport::connect_from(
                    self.gateway_addr,
                    self.connection_timeout,
                    self.local_ip,
                )
                .await?;
                (Arc::new(t), l)
            }
        };
        *self.transport.lock().unwrap() = Some(transport.clone());
        *self.local_addr.lock().unwrap() = Some(local);
        log_transport!(
            LogLevel::Info,
            "{:?} tunnel transport ready: local={}, gateway={}",
            self.kind,
            local,
            self.gateway_addr
        );
        Ok(transport)
    }

    pub(crate) fn bind_to(mut self, local_ip: Option<std::net::IpAddr>) -> Self {
        self.local_ip = local_ip;
        self
    }

    /// Establish the KNX tunnel (ConnectRequest/Response handshake).
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::SocketError`]/[`TransportError::ConnectionFailed`]
    /// if the underlying transport can't connect, [`TransportError::Timeout`]
    /// if the gateway doesn't respond within the configured connection
    /// timeout, [`TransportError::InvalidConfiguration`] if the response
    /// isn't a `ConnectResponse` or a configured individual address is
    /// unavailable, or [`TransportError::ConnectionFailed`] if the gateway
    /// rejects the connection.
    pub async fn connect(&mut self) -> Result<()> {
        log_transport!(
            LogLevel::Info,
            "Establishing tunnel to {}",
            self.gateway_addr
        );
        if let Ok(mut s) = self.state.write() {
            *s = TransportState::Connecting;
        }

        let result = async {
            self.establish_transport().await?;
            self.send_connect_request().await
        }
        .await;

        match result {
            Ok((channel_id, assigned_address)) => {
                self.finish_connect(channel_id, assigned_address).await
            }
            Err(e) => {
                if let Ok(mut s) = self.state.write() {
                    *s = TransportState::Failed;
                }
                if let Ok(mut stats) = self.stats.write() {
                    stats.record_connection_error(&e);
                }
                Err(e)
            }
        }
    }

    /// Send a `ConnectRequest` and process the `ConnectResponse`, returning the
    /// assigned `channel_id` and (if present) the gateway-assigned individual
    /// address.
    ///
    /// Shared by `connect()` and `connect_secure()` — the KNX-layer tunnel
    /// handshake is identical either way, since `send_frame`/`recv_frame`
    /// already transparently encrypt/decrypt once `self.secure_session` is
    /// set (a plain `connect()` has no secure session yet, so this behaves
    /// exactly as before for that path).
    async fn send_connect_request(&self) -> Result<(u8, Option<IndividualAddress>)> {
        let mut connect_request = match self.kind {
            TransportKind::Udp => ConnectRequest::new_route_back(),
            TransportKind::Tcp => ConnectRequest::new_tcp_route_back(),
        };
        // Address management opt-in: if an explicit address is configured,
        // validate it against the registry (collision check) and request it
        // from the gateway via the Extended CRI.
        if let Some(addr) = self.configured_address {
            if let Some(registry) = &self.address_registry
                && !registry.is_available(addr)
            {
                return Err(TransportError::InvalidConfiguration {
                    details: format!(
                        "Configured individual address {addr} is not available \
                         (already claimed or known-occupied)"
                    ),
                }
                .into());
            }
            connect_request.cri.individual_address = Some(addr);
        }
        let request = KnxIpFrame::new(ServiceType::ConnectRequest, connect_request.serialize());
        self.send_frame(&request.serialize()).await?;

        let response_data = timeout(self.connection_timeout, self.recv_frame())
            .await
            .map_err(|_| {
                log_transport!(
                    LogLevel::Error,
                    "Connect request timed out after {:?}",
                    self.connection_timeout
                );
                TransportError::Timeout {
                    timeout_ms: self.connection_timeout.as_millis() as u64,
                }
            })??;

        let response_frame = KnxIpFrame::parse(&response_data)?;
        if response_frame.header.service_type != ServiceType::ConnectResponse {
            return Err(TransportError::InvalidConfiguration {
                details: format!(
                    "Expected ConnectResponse, got {:?}",
                    response_frame.header.service_type
                ),
            }
            .into());
        }

        let connect_response = ConnectResponse::parse(&response_frame.body)?;
        if !connect_response.is_success() {
            let msg = connect_response.error_message().unwrap_or_else(|| {
                format!(
                    "Gateway rejected connection: 0x{:02X}",
                    connect_response.status
                )
            });
            return Err(TransportError::ConnectionFailed {
                address: self.gateway_addr.to_string(),
                source: std::io::Error::new(std::io::ErrorKind::ConnectionRefused, msg),
            }
            .into());
        }

        Ok((
            connect_response.channel_id,
            connect_response.assigned_address,
        ))
    }

    /// Apply a successful `ConnectResponse`: set channel/state, run address
    /// management, log. Shared tail of `connect()`/`connect_secure()`.
    async fn finish_connect(
        &mut self,
        channel_id: u8,
        assigned_address: Option<IndividualAddress>,
    ) -> Result<()> {
        self.channel_id = channel_id;
        self.established_at = Some(Instant::now());
        if let Ok(mut s) = self.state.write() {
            *s = TransportState::Connected;
        }

        // Address management (opt-in): only runs when a registry is
        // configured. Determine the final address, claim it, and store it.
        let mut final_addr: Option<IndividualAddress> = None;
        if let Some(registry) = &self.address_registry {
            let addr = if let Some(cfg) = self.configured_address {
                // (a) Explicit configured address takes priority.
                cfg
            } else if let Some(assigned) = assigned_address {
                // (b) Gateway-assigned address from the CRD.
                assigned
            } else {
                // (c) Legacy gateway assigned nothing: probe-select one.
                // area/line are derived from the configured address if
                // present, else the assigned address; in this branch both
                // are absent, so we fall back to area=0, line=0.
                let (area, line) = self
                    .configured_address
                    .or(assigned_address)
                    .map_or((0, 0), |a| (a.area(), a.line()));
                auto_select_address(&*self, registry, area, line).await?
            };
            // Claim the final address (propagates Err on collision).
            registry.claim(addr)?;
            *self.individual_address.lock().unwrap() = Some(addr);
            final_addr = Some(addr);
        }

        match final_addr {
            Some(addr) => log_transport!(
                LogLevel::Info,
                "Tunnel connected: channel={} address={}",
                channel_id,
                addr
            ),
            None => log_transport!(
                LogLevel::Info,
                "Tunnel established: channel_id={}",
                channel_id
            ),
        }
        Ok(())
    }

    /// Establish a secure session (KNX IP Secure handshake) over this transport.
    // TODO: no real device has completed a full authenticated round trip yet; re-test when current credentials are available.\n    /// # Errors
    ///
    /// Returns [`TransportError::SocketError`]/[`TransportError::ConnectionFailed`]
    /// if the underlying transport can't connect,
    /// [`TransportError::Timeout`] if the gateway doesn't respond to the
    /// SessionRequest/Authenticate within the connection timeout,
    /// [`TransportError::InvalidConfiguration`] if the SessionResponse/Status
    /// frames are malformed, a [`crate::error::SecurityError`] if session key
    /// derivation or MAC verification fails, or the same errors as
    /// [`Self::connect`] for the KNX-layer handshake that follows.
    #[cfg(feature = "secure")]
    pub async fn connect_secure(&mut self, security: &SecurityConfig) -> Result<()> {
        let transport = self.establish_transport().await?;

        let config = security.session_config();
        let mut session = SecureSession::new(&config);
        let request = SessionRequest {
            control_endpoint: self.local_hpai.clone(),
            public_key: session.initialize().await.try_into().map_err(|_| {
                TransportError::InvalidConfiguration {
                    details: "ECDH public key was not 32 bytes".into(),
                }
            })?,
        };
        let frame = KnxIpFrame::new(ServiceType::SessionRequest, request.serialize()).serialize();
        transport.send_frame(&frame).await?;

        let response_data = timeout(self.connection_timeout, transport.recv_frame())
            .await
            .map_err(|_| TransportError::Timeout {
                timeout_ms: self.connection_timeout.as_millis() as u64,
            })??;
        let response = KnxIpFrame::parse(&response_data)?;
        if response.header.service_type != ServiceType::SessionResponse {
            return Err(TransportError::InvalidConfiguration {
                details: "Expected SessionResponse during IP Secure handshake".into(),
            }
            .into());
        }
        let response = SessionResponse::parse(&response.body)?;
        let auth_mac = session
            .process_session_response(response.session_id, &response.public_key, &response.mac)
            .await?;
        let auth = SessionAuthenticate {
            user_id: config.user_id,
            mac: auth_mac
                .try_into()
                .map_err(|_| TransportError::InvalidConfiguration {
                    details: "SessionAuthenticate MAC was not 16 bytes".into(),
                })?,
        };
        let frame = KnxIpFrame::new(ServiceType::SessionAuthenticate, auth.serialize()).serialize();
        transport
            .send_frame(&session.encrypt_frame(&frame).await?)
            .await?;

        let status_data = timeout(self.connection_timeout, transport.recv_frame())
            .await
            .map_err(|_| TransportError::Timeout {
                timeout_ms: self.connection_timeout.as_millis() as u64,
            })??;
        let wrapper = KnxIpFrame::parse(&status_data)?;
        if wrapper.header.service_type != ServiceType::SecureWrapper {
            return Err(TransportError::InvalidConfiguration {
                details: "Expected wrapped SessionStatus during IP Secure handshake".into(),
            }
            .into());
        }
        let plaintext = session.decrypt_frame(&status_data).await?;
        let frame = KnxIpFrame::parse(&plaintext)?;
        if frame.header.service_type != ServiceType::SessionStatus {
            return Err(TransportError::InvalidConfiguration {
                details: "Expected SessionStatus during IP Secure handshake".into(),
            }
            .into());
        }
        let status = SessionStatus::parse(&frame.body)?;
        session.complete_authentication(status.status).await?;

        self.secure_session = Some(Arc::new(tokio::sync::RwLock::new(session)));
        self.start_secure_keepalive(security.keepalive_interval)?;
        log_transport!(LogLevel::Info, "Secure session established");

        // The secure *session* is now up, but that's just the encrypted
        // transport layer — the KNX tunnel itself still needs its own
        // ConnectRequest/Response, now riding inside it (send_frame/recv_frame
        // encrypt/decrypt transparently once self.secure_session is set).
        if let Ok(mut s) = self.state.write() {
            *s = TransportState::Connecting;
        }
        match self.send_connect_request().await {
            Ok((channel_id, assigned_address)) => {
                self.finish_connect(channel_id, assigned_address).await
            }
            Err(e) => {
                if let Some(cancel) = self.secure_keepalive_cancel.take() {
                    let _ = cancel.send(());
                }
                if let Ok(mut s) = self.state.write() {
                    *s = TransportState::Failed;
                }
                if let Ok(mut stats) = self.stats.write() {
                    stats.record_connection_error(&e);
                }
                Err(e)
            }
        }
    }

    #[cfg(feature = "secure")]
    fn start_secure_keepalive(&mut self, interval: Duration) -> Result<()> {
        let transport = self.transport()?.clone();
        let session = self
            .secure_session
            .as_ref()
            .expect("secure session is set before its keepalive starts")
            .clone();
        let activity = self.secure_activity.clone();
        let send_lock = self.secure_send_lock.clone();
        activity.changed.notify_one();
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let keepalive = KnxIpFrame::new(
                ServiceType::SessionStatus,
                SessionStatus {
                    status: SessionStatus::STATUS_KEEPALIVE,
                }
                .serialize(),
            )
            .serialize();
            loop {
                let deadline = activity.last().await + interval;
                tokio::select! {
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                        if Instant::now() < deadline {
                            continue;
                        }
                        let _guard = send_lock.lock().await;
                        if Instant::now() < activity.last().await + interval {
                            continue;
                        }
                        let encrypted = match session.read().await.encrypt_frame(&keepalive).await {
                            Ok(frame) => frame,
                            Err(_) => break,
                        };
                        if transport.send_frame(&encrypted).await.is_err() {
                            break;
                        }
                        activity.touch().await;
                    }
                    _ = activity.changed.notified() => {}
                    _ = &mut cancelled => break,
                }
            }
        });
        self.secure_keepalive_cancel = Some(cancel);
        Ok(())
    }

    /// Disconnect, sending a `DisconnectRequest` if a channel is active.
    ///
    /// Send/receive failures on the `DisconnectRequest` are logged and
    /// swallowed (the tunnel is torn down locally regardless).
    ///
    /// # Panics
    ///
    /// Panics if an internal lock is poisoned.
    pub async fn disconnect(&mut self) {
        if let Ok(mut s) = self.state.write() {
            *s = TransportState::Disconnecting;
        }
        if self.channel_id != 0
            && let Err(e) = self.send_disconnect_request().await
        {
            log_transport!(LogLevel::Warn, "Failed to send disconnect request: {}", e);
        }
        #[cfg(feature = "secure")]
        if let Some(cancel) = self.secure_keepalive_cancel.take() {
            let _ = cancel.send(());
        }
        if let Ok(mut s) = self.state.write() {
            *s = TransportState::Disconnected;
        }
        self.established_at = None;
        self.channel_id = 0;
        // Address management (opt-in): release any claimed address.
        if let Some(registry) = &self.address_registry {
            let claimed = self.individual_address.lock().unwrap().take();
            if let Some(addr) = claimed {
                registry.release(addr);
            }
        }
    }

    /// Send a graceful `DisconnectRequest` (does not require `&mut self`).
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::ConnectionClosed`] if the transport was
    /// never established, or [`ProtocolError::InvalidFrame`] if the gateway
    /// sends a `DisconnectResponse` with a non-success status. A missing
    /// response (the gateway may close the socket first) is not an error.
    pub async fn send_disconnect(&self) -> Result<()> {
        if self.channel_id == 0 {
            return Ok(());
        }
        self.send_disconnect_request().await
    }

    async fn send_disconnect_request(&self) -> Result<()> {
        let req = DisconnectRequest::new(self.channel_id, self.local_hpai.clone());
        let frame = KnxIpFrame::new(ServiceType::DisconnectRequest, req.serialize());
        self.send_frame(&frame.serialize()).await?;

        // Spec allows no response (gateway may close the socket first), so timeout is not an error.
        // Other frame types may arrive before the DisconnectResponse, so loop until we see it.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        // timeout or recv error: spec allows no response, so `while let` exits cleanly.
        while let Ok(Ok(data)) = timeout_at(deadline, self.recv_frame()).await {
            if let Ok(f) = KnxIpFrame::parse(&data)
                && f.header.service_type == ServiceType::DisconnectResponse
                && let Ok(resp) = DisconnectResponse::parse(&f.body)
                && resp.communication_channel_id == self.channel_id
            {
                if !resp.is_success() {
                    return Err(ProtocolError::InvalidFrame {
                        details: format!("disconnect rejected: status 0x{:02X}", resp.status),
                    }
                    .into());
                }
                break; // success
            }
            // not a DisconnectResponse — keep waiting
        }
        Ok(())
    }

    /// Send a `TunnellingAck` for a received `TunnellingRequest`.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::ConnectionClosed`] if the transport was
    /// never established, or the underlying transport's send error.
    pub async fn send_tunnelling_ack(
        &self,
        channel_id: u8,
        sequence: u8,
        status: u8,
    ) -> Result<()> {
        let transport = self.transport()?;
        let ack = TunnellingAck::new(channel_id, sequence, status);
        let frame = KnxIpFrame::new(ServiceType::TunnellingAck, ack.serialize());
        transport.send_frame(&frame.serialize()).await
    }

    /// Send an already-serialized KNX/IP frame (encrypting if a secure session exists).
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::ConnectionClosed`] if the transport was
    /// never established, a [`crate::error::SecurityError`] if encryption
    /// fails, or the underlying transport's send error.
    pub async fn send_frame(&self, frame: &[u8]) -> Result<()> {
        #[cfg(feature = "secure")]
        let _secure_guard = if self.secure_session.is_some() {
            Some(self.secure_send_lock.lock().await)
        } else {
            None
        };

        let result = async {
            let transport = self.transport()?;
            #[cfg(feature = "secure")]
            let data = if let Some(ref session) = self.secure_session {
                session.read().await.encrypt_frame(frame).await?
            } else {
                frame.to_vec()
            };
            #[cfg(not(feature = "secure"))]
            let data = frame.to_vec();
            transport.send_frame(&data).await?;
            #[cfg(feature = "secure")]
            if self.secure_session.is_some() {
                self.secure_activity.touch().await;
            }
            Ok(())
        }
        .await;

        match result {
            Ok(()) => {
                if let Ok(mut stats) = self.stats.write() {
                    stats.frames_sent += 1;
                }
                Ok(())
            }
            Err(error) => {
                if let Ok(mut stats) = self.stats.write() {
                    stats.record_send_error(&error);
                }
                Err(error)
            }
        }
    }

    /// High-level UDP request delivery; raw send_frame remains unacknowledged.
    pub(crate) async fn send_acknowledged(
        &self,
        data: &[u8],
        attempts: u8,
        transmitted: &mut Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<()> {
        let frame = KnxIpFrame::parse(data)?;
        let ack_service = match frame.header.service_type {
            ServiceType::TunnellingRequest => ServiceType::TunnellingAck,
            ServiceType::DeviceConfigurationRequest => ServiceType::DeviceConfigurationAck,
            _ => return self.send_signalled(data, transmitted).await,
        };
        if self.kind == TransportKind::Tcp {
            return self.send_signalled(data, transmitted).await;
        }
        if frame.body.len() < 4 || frame.body[0] != 4 || frame.body[1] != self.channel_id {
            return Err(ProtocolError::InvalidFrame {
                details: "Invalid tunnel request channel/header".into(),
            }
            .into());
        }
        let _guard = self.send_lock.lock().await;
        for attempt in 0..attempts {
            let ack = self
                .router
                .register_ack(ack_service, self.channel_id, frame.body[2]);
            self.send_signalled(data, transmitted).await?;
            match timeout(Duration::from_secs(1), ack).await {
                Ok(Ok(reply)) if reply.body[3] == 0 => return Ok(()),
                Ok(Ok(reply)) if attempt + 1 == attempts => {
                    return Err(ProtocolError::InvalidFrame {
                        details: format!("Tunnel request rejected: status 0x{:02X}", reply.body[3]),
                    }
                    .into());
                }
                _ => {}
            }
        }
        Err(TransportError::Timeout { timeout_ms: 1000 }.into())
    }

    async fn send_signalled(
        &self,
        data: &[u8],
        transmitted: &mut Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<()> {
        if let Some(tx) = transmitted.as_mut() {
            tokio::select! {
                result = self.send_frame(data) => result?,
                _ = tx.closed() => return Err(TransportError::QueueClosed.into()),
            }
        } else {
            self.send_frame(data).await?;
        }
        if let Some(tx) = transmitted.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    /// Receive one complete KNX/IP frame (decrypting if a secure session exists).
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::ConnectionClosed`] if the transport was
    /// never established, a [`crate::error::SecurityError`] if decryption
    /// fails, or the underlying transport's receive error.
    pub async fn recv_frame(&self) -> Result<Vec<u8>> {
        let result = async {
            let transport = self.transport()?;
            #[cfg(feature = "secure")]
            let data = if let Some(session) = &self.secure_session {
                loop {
                    let raw = transport.recv_frame().await?;
                    if KnxIpFrame::parse(&raw)
                        .is_ok_and(|frame| frame.header.service_type == ServiceType::SecureWrapper)
                    {
                        let data = session.read().await.decrypt_frame(&raw).await?;
                        self.secure_activity.touch().await;
                        break data;
                    }
                }
            } else {
                transport.recv_frame().await?
            };
            #[cfg(not(feature = "secure"))]
            let data = transport.recv_frame().await?;
            Ok(data)
        }
        .await;

        match result {
            Ok(data) => {
                if let Ok(mut stats) = self.stats.write() {
                    stats.frames_received += 1;
                }
                Ok(data)
            }
            Err(error) => {
                if let Ok(mut stats) = self.stats.write() {
                    stats.record_receive_error(&error);
                }
                Err(error)
            }
        }
    }

    // --- Heartbeat / router ---

    /// Start the `TransportState` heartbeat loop and return its lifecycle handle.
    ///
    /// The loop reports outcomes through the handle and declares this tunnel lost
    /// after the configured consecutive failures. It is inert when this tunnel
    /// was created without heartbeat support, as with TCP tunnelling.
    ///
    /// # Panics
    ///
    /// Panics if the internal heartbeat lock is poisoned.
    pub fn start_heartbeat(&self, label: String) -> HeartbeatHandle {
        let lost = Arc::new(tokio::sync::Notify::new());
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        if !self.use_heartbeat {
            return HeartbeatHandle {
                lost,
                events: event_rx,
                task: None,
            };
        }

        let monitor = Arc::new(HeartbeatMonitor::new(
            HeartbeatConfig::default(),
            self.channel_id,
            label,
        ));
        *self.heartbeat.write().unwrap() = Some(monitor.clone());

        let transport = self.transport().ok();
        let router = self.router.clone();
        let channel_id = self.channel_id;
        let request = KnxIpFrame::new(
            ServiceType::ConnectionstateRequest,
            ConnectionstateRequest::new(channel_id, self.local_hpai.clone()).serialize(),
        )
        .serialize();
        let task_lost = lost.clone();
        let task = tokio::spawn(async move {
            let interval = monitor.config().interval;
            let response_timeout = monitor.config().timeout;
            loop {
                tokio::select! {
                    () = tokio::time::sleep(interval) => {}
                    () = task_lost.notified() => break,
                }
                let rx = router
                    .register_channel_response(ServiceType::ConnectionstateResponse, channel_id);
                let t0 = Instant::now();
                let sent = match transport.as_ref() {
                    Some(transport) => transport.send_frame(&request).await.is_ok(),
                    None => false,
                };
                if !sent {
                    let _ = event_tx.send(HeartbeatEvent {
                        ok: false,
                        latency_ms: None,
                    });
                    if monitor.record_failure() {
                        task_lost.notify_waiters();
                        break;
                    }
                    continue;
                }
                let ok = match timeout(response_timeout, rx).await {
                    Ok(Ok(frame)) => {
                        ConnectionstateResponse::parse(&frame.body).is_ok_and(|response| {
                            response.status == ConnectionstateResponse::STATUS_OK
                        })
                    }
                    _ => false,
                };
                if ok {
                    let latency_ms = t0.elapsed().as_millis() as u64;
                    monitor.record_success();
                    let _ = event_tx.send(HeartbeatEvent {
                        ok: true,
                        latency_ms: Some(latency_ms),
                    });
                } else {
                    let dead = monitor.record_failure();
                    let _ = event_tx.send(HeartbeatEvent {
                        ok: false,
                        latency_ms: None,
                    });
                    if dead {
                        task_lost.notify_waiters();
                        break;
                    }
                }
            }
        });

        HeartbeatHandle {
            lost,
            events: event_rx,
            task: Some(task),
        }
    }

    pub fn router(&self) -> Arc<FrameRouter> {
        self.router.clone()
    }

    /// Build and send a `ConnectionState_Request` for this channel.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::ConnectionClosed`] if the transport was
    /// never established, or the underlying transport's send error.
    pub async fn send_connectionstate_request(&self) -> Result<()> {
        let transport = self.transport()?;
        let req = ConnectionstateRequest::new(self.channel_id, self.local_hpai.clone());
        let frame = KnxIpFrame::new(ServiceType::ConnectionstateRequest, req.serialize());
        transport.send_frame(&frame.serialize()).await
    }

    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn is_tunnel_lost(&self) -> bool {
        self.heartbeat
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|m| m.is_tunnel_lost())
    }

    // --- Sequence numbers (UDP reliability) ---

    pub fn next_sequence(&self) -> u8 {
        self.sequence_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub fn reset_sequence(&self) {
        self.sequence_counter.store(0, Ordering::SeqCst);
        self.expected_sequence_number.store(0, Ordering::SeqCst);
    }

    pub fn current_sequence(&self) -> u8 {
        self.sequence_counter.load(Ordering::SeqCst)
    }

    pub fn expected_sequence(&self) -> u8 {
        self.expected_sequence_number.load(Ordering::SeqCst)
    }

    /// Validate an incoming tunnelling request's sequence number.
    pub fn validate_sequence_number(&self, received: u8) -> SequenceValidationResult {
        let expected = self.expected_sequence_number.load(Ordering::SeqCst);
        if received == expected {
            self.expected_sequence_number
                .store(expected.wrapping_add(1), Ordering::SeqCst);
            SequenceValidationResult::Valid
        } else if received == expected.wrapping_sub(1) {
            SequenceValidationResult::Duplicate
        } else {
            SequenceValidationResult::Invalid { expected, received }
        }
    }

    pub fn channel_id(&self) -> u8 {
        self.channel_id
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.state(), TransportState::Connected)
    }

    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn state(&self) -> TransportState {
        *self.state.read().unwrap()
    }

    /// # Panics
    ///
    /// Panics if an internal lock is poisoned.
    pub fn stats(&self) -> ConnectionStats {
        let mut stats = self.stats.read().unwrap().clone();
        if let Some(start) = self.established_at {
            stats.uptime_seconds = start.elapsed().as_secs();
        }
        stats
    }

    pub fn kind(&self) -> TransportKind {
        self.kind
    }

    /// Connection uptime since the tunnel was established.
    pub fn uptime(&self) -> Option<Duration> {
        self.established_at.map(|t| t.elapsed())
    }
}

/// Handle to a heartbeat loop started by [`Tunnel::start_heartbeat`].
pub struct HeartbeatHandle {
    lost: Arc<tokio::sync::Notify>,
    events: tokio::sync::mpsc::UnboundedReceiver<HeartbeatEvent>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl HeartbeatHandle {
    /// Notified when the tunnel is declared lost (`max_failures` consecutive
    /// heartbeat failures), or immediately when [`HeartbeatHandle::stop`] runs.
    #[must_use]
    pub fn lost(&self) -> Arc<tokio::sync::Notify> {
        self.lost.clone()
    }

    /// Await the next heartbeat outcome. Resolves to `None` once the loop has
    /// stopped.
    pub async fn recv_event(&mut self) -> Option<HeartbeatEvent> {
        self.events.recv().await
    }

    /// Stop the heartbeat loop and wait for its task to finish.
    pub async fn stop(mut self) {
        self.lost.notify_waiters();
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for HeartbeatHandle {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[async_trait::async_trait]
impl Connection for Tunnel {
    async fn send(&self, frame: &[u8]) -> Result<()> {
        if !self.is_connected() {
            let error = TransportError::ConnectionClosed.into();
            if let Ok(mut stats) = self.stats.write() {
                stats.record_send_error(&error);
            }
            return Err(error);
        }
        Tunnel::send_frame(self, frame).await
    }

    async fn recv(&self) -> Result<Vec<u8>> {
        if !self.is_connected() {
            let error = TransportError::ConnectionClosed.into();
            if let Ok(mut stats) = self.stats.write() {
                stats.record_receive_error(&error);
            }
            return Err(error);
        }
        Tunnel::recv_frame(self).await
    }

    async fn close(&self) -> Result<()> {
        self.send_disconnect().await
    }

    fn start_heartbeat(&self, label: String) -> Option<HeartbeatHandle> {
        Some(Tunnel::start_heartbeat(self, label))
    }

    fn state(&self) -> TransportState {
        Tunnel::state(self)
    }

    fn stats(&self) -> ConnectionStats {
        Tunnel::stats(self)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tunneling_address_tests {
    use super::*;

    #[cfg(feature = "secure")]
    struct KeepaliveTestTransport {
        sent: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        incoming: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>,
    }

    #[cfg(feature = "secure")]
    #[async_trait::async_trait]
    impl FrameTransport for KeepaliveTestTransport {
        async fn send_frame(&self, frame: &[u8]) -> Result<()> {
            self.sent
                .send(frame.to_vec())
                .map_err(|_| TransportError::ConnectionClosed.into())
        }

        async fn recv_frame(&self) -> Result<Vec<u8>> {
            self.incoming
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| TransportError::ConnectionClosed.into())
        }

        fn kind(&self) -> TransportKind {
            TransportKind::Udp
        }
    }

    #[cfg(feature = "secure")]
    #[tokio::test(start_paused = true)]
    async fn ip_secure_keepalive_waits_for_fifteen_idle_seconds() {
        let config = SessionConfig {
            user_id: 1,
            user_password: crate::security::generate_password(),
            device_auth_password: Some(crate::security::generate_password()),
            keepalive_interval: 15,
        };
        let mut client = SecureSession::new(&config);
        let mut server = SecureSession::new(&config);
        let client_pub: [u8; 32] = client.initialize().await.try_into().unwrap();
        let server_pub: [u8; 32] = server.initialize().await.try_into().unwrap();
        let session_id = 42;
        let response_mac = server
            .process_session_request(&client_pub, session_id)
            .await
            .unwrap();
        let auth_mac = client
            .process_session_response(session_id, &server_pub, &response_mac)
            .await
            .unwrap();
        assert!(
            server
                .verify_authenticate_mac(&client_pub, config.user_id, &auth_mac)
                .await
                .unwrap()
        );
        client.complete_authentication(0).await.unwrap();

        let (sent_tx, mut sent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_incoming_tx, incoming_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tunnel = Tunnel::new_udp(gateway());
        tunnel.transport = std::sync::Mutex::new(Some(Arc::new(KeepaliveTestTransport {
            sent: sent_tx,
            incoming: tokio::sync::Mutex::new(incoming_rx),
        })));
        tunnel.secure_session = Some(Arc::new(tokio::sync::RwLock::new(client)));
        tunnel
            .start_secure_keepalive(Duration::from_secs(15))
            .unwrap();
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_secs(14)).await;
        tunnel.secure_activity.touch().await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(14)).await;
        assert!(sent_rx.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(1)).await;

        let encrypted = tokio::time::timeout(Duration::from_secs(1), sent_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            server.decrypt_frame(&encrypted).await.unwrap(),
            [0x06, 0x10, 0x09, 0x54, 0x00, 0x08, 0x04, 0x00]
        );
        if let Some(cancel) = tunnel.secure_keepalive_cancel.take() {
            let _ = cancel.send(());
        }
    }

    #[cfg(feature = "secure")]
    #[tokio::test(start_paused = true)]
    async fn secure_tunnel_rejects_unwrapped_control_frames_from_the_start() {
        let config = SessionConfig {
            user_id: 1,
            user_password: crate::security::generate_password(),
            device_auth_password: Some(crate::security::generate_password()),
            keepalive_interval: 15,
        };
        let mut client = SecureSession::new(&config);
        let mut server = SecureSession::new(&config);
        let client_pub: [u8; 32] = client.initialize().await.try_into().unwrap();
        let server_pub: [u8; 32] = server.initialize().await.try_into().unwrap();
        let response_mac = server
            .process_session_request(&client_pub, 42)
            .await
            .unwrap();
        let auth_mac = client
            .process_session_response(42, &server_pub, &response_mac)
            .await
            .unwrap();
        assert!(
            server
                .verify_authenticate_mac(&client_pub, 1, &auth_mac)
                .await
                .unwrap()
        );
        client.complete_authentication(0).await.unwrap();

        let (sent_tx, _sent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (incoming_tx, incoming_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tunnel = Tunnel::new_udp(gateway());
        *tunnel.transport.lock().unwrap() = Some(Arc::new(KeepaliveTestTransport {
            sent: sent_tx,
            incoming: tokio::sync::Mutex::new(incoming_rx),
        }));
        tunnel.secure_session = Some(Arc::new(tokio::sync::RwLock::new(client)));

        let plain_request =
            KnxIpFrame::new(ServiceType::TunnellingRequest, vec![4, 1, 9, 0]).serialize();
        let plain_disconnect =
            KnxIpFrame::new(ServiceType::DisconnectRequest, vec![1, 0, 0]).serialize();
        incoming_tx.send(plain_request).unwrap();
        incoming_tx.send(plain_disconnect).unwrap();

        let wrapped_request =
            KnxIpFrame::new(ServiceType::TunnellingRequest, vec![4, 1, 0, 0]).serialize();
        incoming_tx
            .send(server.encrypt_frame(&wrapped_request).await.unwrap())
            .unwrap();
        assert_eq!(tunnel.recv_frame().await.unwrap(), wrapped_request);
    }

    fn gateway() -> SocketAddr {
        "127.0.0.1:3671".parse().unwrap()
    }

    #[tokio::test]
    async fn dropping_heartbeat_handle_stops_its_task() {
        let tunnel = Arc::new(Tunnel::new_udp(gateway()));
        let weak = Arc::downgrade(&tunnel);
        let connection: Arc<dyn Connection> = tunnel.clone();
        let handle = connection
            .start_heartbeat("test".into())
            .expect("tunnel provides a heartbeat");
        drop(connection);
        drop(tunnel);
        drop(handle);
        tokio::task::yield_now().await;
        assert!(
            weak.upgrade().is_none(),
            "heartbeat task retained the tunnel"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_first_attempt_uses_thirty_second_interval() {
        let tunnel = Arc::new(Tunnel::new_udp(gateway()));
        let mut handle = tunnel.start_heartbeat("test".into());
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(29)).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(1), handle.recv_event())
                .await
                .is_err()
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        let event = tokio::time::timeout(Duration::from_secs(1), handle.recv_event())
            .await
            .unwrap()
            .unwrap();
        assert!(!event.ok);
        handle.stop().await;
    }

    #[tokio::test]
    async fn tunneling_without_address_management_has_no_address_and_default_cri() {
        let tunnel = Tunnel::new_udp(gateway());

        // No registry configured → no individual address tracked.
        assert_eq!(tunnel.individual_address(), None);

        // The default ConnectRequest CRI is the legacy 4-byte CRI (no address).
        let req = ConnectRequest::new_route_back();
        assert!(req.cri.individual_address.is_none());
        // 4-byte CRI: length, connection type, two reserved/parameter bytes.
        assert_eq!(req.cri.serialize().len(), 4);
    }

    #[tokio::test]
    async fn tunneling_configured_address_collision_precondition() {
        let registry = AddressRegistry::new();
        let addr = IndividualAddress::new(1, 1, 200);
        // Mark the desired address as already occupied.
        registry.add_known_occupied(addr);

        let tunnel =
            Tunnel::new_udp(gateway()).with_address_management(registry.clone(), Some(addr));

        // This is exactly the precondition connect() validates before requesting
        // the address: an unavailable configured address must be rejected.
        assert!(!registry.is_available(addr));
        // Nothing claimed yet (connect() not run; no live I/O in tests).
        assert_eq!(tunnel.individual_address(), None);
    }

    #[tokio::test]
    async fn tunneling_set_address_management_requests_extended_cri() {
        let registry = AddressRegistry::new();
        let addr = IndividualAddress::new(1, 1, 50);
        let mut tunnel = Tunnel::new_udp(gateway());
        tunnel.set_address_management(registry.clone(), Some(addr));

        // The address is available, so the Extended CRI would carry it (6 bytes).
        assert!(registry.is_available(addr));
        let mut req = ConnectRequest::new_route_back();
        req.cri.individual_address = Some(addr);
        assert_eq!(req.cri.serialize().len(), 6);
        assert_eq!(tunnel.individual_address(), None);
    }
}
