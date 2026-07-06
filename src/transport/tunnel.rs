//! Unified KNX/IP tunnel over a [`FrameTransport`] (UDP or TCP).
//!
//! The KNX-layer state machine — ConnectRequest/Response handshake, channel id,
//! sequence numbers, TunnellingRequest/Ack, `ConnectionState` heartbeat and
//! `DisconnectRequest` — is identical for both transports. Only framing and
//! liveness differ, handled by the transport plus a couple of flags:
//!
//! * `use_heartbeat` — UDP needs the `ConnectionState` heartbeat for liveness;
//!   TCP uses it only as a NAT/idle keepalive (on by default).
//! * `send_acks` — UDP acknowledges incoming `TunnellingRequests`; TCP relies on
//!   the stream and does not.
//!
//! Constructors perform no network I/O; the transport is established lazily in
//! [`Tunnel::connect`] (or [`Tunnel::connect_secure`]).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use tokio::time::{timeout, timeout_at};

#[cfg(feature = "secure")]
use super::SecurityConfig;
use super::address_probe::auto_select_address;
use super::address_registry::AddressRegistry;
use super::connection::{Connection, ConnectionState, ConnectionStats};
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
use crate::security::{SecureSession, SessionConfig};

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

/// A KNX/IP tunnel connection, transport-agnostic.
pub struct Tunnel {
    kind: TransportKind,
    transport: std::sync::Mutex<Option<Arc<dyn FrameTransport>>>,
    gateway_addr: SocketAddr,
    local_addr: std::sync::Mutex<Option<SocketAddr>>,
    local_hpai: Hpai,
    channel_id: u8,
    sequence_counter: AtomicU8,
    expected_sequence_number: AtomicU8,
    state: std::sync::RwLock<ConnectionState>,
    stats: std::sync::RwLock<ConnectionStats>,
    connection_timeout: Duration,
    established_at: Option<Instant>,
    router: Arc<FrameRouter>,
    heartbeat: std::sync::RwLock<Option<Arc<HeartbeatMonitor>>>,
    #[cfg(feature = "secure")]
    secure_session: Option<Arc<tokio::sync::RwLock<SecureSession>>>,
    /// Whether the `ConnectionState` heartbeat should run for this tunnel.
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
            local_addr: std::sync::Mutex::new(None),
            local_hpai,
            channel_id: 0,
            sequence_counter: AtomicU8::new(0),
            expected_sequence_number: AtomicU8::new(0),
            state: std::sync::RwLock::new(ConnectionState::Disconnected),
            stats: std::sync::RwLock::new(ConnectionStats::default()),
            connection_timeout,
            established_at: None,
            router: Arc::new(FrameRouter::new()),
            heartbeat: std::sync::RwLock::new(None),
            #[cfg(feature = "secure")]
            secure_session: None,
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
                let (t, l) = UdpFrameTransport::connect(self.gateway_addr).await?;
                (Arc::new(t), l)
            }
            TransportKind::Tcp => {
                let (t, l) =
                    TcpFrameTransport::connect(self.gateway_addr, self.connection_timeout).await?;
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
            *s = ConnectionState::Connecting;
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
                    *s = ConnectionState::Failed;
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
            *s = ConnectionState::Connected;
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
    // TODO: Confirmed spec-correct against a real KNX IP Secure device over
    // TCP (a known-good reference client and this one produced byte-identical
    // SessionRequest/Response behavior and hit the same "SessionResponse MAC
    // verification failed" against a device we didn't have current
    // credentials for) but no real device has completed a full authenticated
    // round trip yet — self-consistent tests only (security::tests,
    // transport::server::tests). Re-test end to end once real device-auth/
    // user credentials are available.
    /// # Errors
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

        let config = SessionConfig {
            user_id: 1,
            user_password: security.user_password.clone().unwrap_or_default(),
            device_auth_password: Some(security.device_auth_password.clone()),
            keepalive_interval: security.session_timeout,
        };
        let mut session = SecureSession::new(&config);
        let public_key = session.initialize().await;

        // SESSION_REQUEST: header(6) + HPAI(8) + public_key(32)
        let mut request_frame = vec![0x06, 0x10, 0x09, 0x51, 0x00, 0x2E];
        request_frame.extend_from_slice(&self.local_hpai.serialize());
        request_frame.extend_from_slice(&public_key);
        transport.send_frame(&request_frame).await?;

        let response_data = timeout(self.connection_timeout, transport.recv_frame())
            .await
            .map_err(|_| TransportError::Timeout {
                timeout_ms: self.connection_timeout.as_millis() as u64,
            })??;
        if response_data.len() < 56 {
            return Err(TransportError::InvalidConfiguration {
                details: "SessionResponse too short".to_string(),
            }
            .into());
        }
        let session_id = u16::from_be_bytes([response_data[6], response_data[7]]);
        let mut server_public_key = [0u8; 32];
        server_public_key.copy_from_slice(&response_data[8..40]);
        let server_mac = &response_data[40..56];
        let auth_mac = session
            .process_session_response(session_id, &server_public_key, server_mac)
            .await?;

        // SESSION_AUTHENTICATE: header(6) + reserved(1) + user_id(1) + MAC(16)
        let mut auth_frame = vec![0x06, 0x10, 0x09, 0x53, 0x00, 0x18];
        auth_frame.push(0x00);
        auth_frame.push(config.user_id);
        auth_frame.extend_from_slice(&auth_mac);
        transport.send_frame(&auth_frame).await?;

        let status_data = timeout(self.connection_timeout, transport.recv_frame())
            .await
            .map_err(|_| TransportError::Timeout {
                timeout_ms: self.connection_timeout.as_millis() as u64,
            })??;
        if status_data.len() < 8 {
            return Err(TransportError::InvalidConfiguration {
                details: "SessionStatus too short".to_string(),
            }
            .into());
        }
        session.complete_authentication(status_data[7]).await?;

        self.secure_session = Some(Arc::new(tokio::sync::RwLock::new(session)));
        log_transport!(LogLevel::Info, "Secure session established");

        // The secure *session* is now up, but that's just the encrypted
        // transport layer — the KNX tunnel itself still needs its own
        // ConnectRequest/Response, now riding inside it (send_frame/recv_frame
        // encrypt/decrypt transparently once self.secure_session is set).
        if let Ok(mut s) = self.state.write() {
            *s = ConnectionState::Connecting;
        }
        match self.send_connect_request().await {
            Ok((channel_id, assigned_address)) => {
                self.finish_connect(channel_id, assigned_address).await
            }
            Err(e) => {
                if let Ok(mut s) = self.state.write() {
                    *s = ConnectionState::Failed;
                }
                Err(e)
            }
        }
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
            *s = ConnectionState::Disconnecting;
        }
        if self.channel_id != 0
            && let Err(e) = self.send_disconnect_request().await
        {
            log_transport!(LogLevel::Warn, "Failed to send disconnect request: {}", e);
        }
        if let Ok(mut s) = self.state.write() {
            *s = ConnectionState::Disconnected;
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
        let transport = self.transport()?;
        let req = DisconnectRequest::new(self.channel_id, self.local_hpai.clone());
        let frame = KnxIpFrame::new(ServiceType::DisconnectRequest, req.serialize());
        transport.send_frame(&frame.serialize()).await?;

        // Spec allows no response (gateway may close the socket first), so timeout is not an error.
        // Other frame types may arrive before the DisconnectResponse, so loop until we see it.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        // timeout or recv error: spec allows no response, so `while let` exits cleanly.
        while let Ok(Ok(data)) = timeout_at(deadline, transport.recv_frame()).await {
            if let Ok(f) = KnxIpFrame::parse(&data)
                && f.header.service_type == ServiceType::DisconnectResponse
            {
                if let Ok(resp) = DisconnectResponse::parse(&f.body)
                    && !resp.is_success()
                {
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
        if let Ok(mut stats) = self.stats.write() {
            stats.frames_sent += 1;
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
        let transport = self.transport()?;
        let raw = transport.recv_frame().await?;
        #[cfg(feature = "secure")]
        let data = if let Some(ref session) = self.secure_session {
            session.read().await.decrypt_frame(&raw).await?
        } else {
            raw
        };
        #[cfg(not(feature = "secure"))]
        let data = raw;
        if let Ok(mut stats) = self.stats.write() {
            stats.frames_received += 1;
        }
        Ok(data)
    }

    // --- Heartbeat / router ---

    /// Start the `ConnectionState` heartbeat loop (KNX spec 03.08.02 §5.4):
    /// periodically sends `ConnectionState_Request`, correlates the response
    /// through the [`FrameRouter`], and reports each outcome via the returned
    /// handle. Declares the tunnel lost after `max_failures` consecutive
    /// failures (see [`HeartbeatConfig`]).
    ///
    /// Inert (no task spawned, [`HeartbeatHandle::lost`] never fires) if this
    /// tunnel was built without `use_heartbeat` (see [`Tunnel::new_tcp_with_timeout`]).
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    #[must_use]
    pub fn start_heartbeat(self: Arc<Self>, label: String) -> HeartbeatHandle {
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

        let conn = self;
        let task_lost = lost.clone();
        let task = tokio::spawn(async move {
            let interval = monitor.config().interval;
            let response_timeout = monitor.config().timeout;
            loop {
                tokio::select! {
                    () = tokio::time::sleep(interval) => {}
                    () = task_lost.notified() => break,
                }
                let rx = conn.router().register(ServiceType::ConnectionstateResponse);
                let t0 = Instant::now();
                if conn.send_connectionstate_request().await.is_err() {
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
                    Ok(Ok(frame)) => ConnectionstateResponse::parse(&frame.body)
                        .is_ok_and(|r| r.status == ConnectionstateResponse::STATUS_OK),
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
        matches!(self.state(), ConnectionState::Connected)
    }

    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn state(&self) -> ConnectionState {
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
    pub async fn stop(self) {
        self.lost.notify_waiters();
        if let Some(task) = self.task {
            task.abort();
            let _ = task.await;
        }
    }
}

#[async_trait::async_trait]
impl Connection for Tunnel {
    async fn send(&self, frame: &[u8]) -> Result<()> {
        if !self.is_connected() {
            return Err(TransportError::ConnectionClosed.into());
        }
        Tunnel::send_frame(self, frame).await
    }

    async fn recv(&self) -> Result<Vec<u8>> {
        if !self.is_connected() {
            return Err(TransportError::ConnectionClosed.into());
        }
        Tunnel::recv_frame(self).await
    }

    async fn close(&self) -> Result<()> {
        self.send_disconnect().await
    }

    fn state(&self) -> ConnectionState {
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

    fn gateway() -> SocketAddr {
        "127.0.0.1:3671".parse().unwrap()
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
