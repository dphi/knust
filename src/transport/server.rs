//! KNXnet/IP tunneling server: accepts incoming connections from external
//! KNX clients over one shared bound UDP socket.
//!
//! This is the inverse role of [`super::tunnel::Tunnel`]: instead of
//! connecting out to a gateway, [`TunnelServer`] *is* the gateway — it binds
//! one socket, replies to `ConnectRequest`s from arbitrary peers, and
//! demultiplexes subsequent frames by `communication_channel_id`. Each
//! connected client gets its own `ClientSession` (sequence counters,
//! heartbeat tracking), mirroring `Tunnel`'s state machine but as the
//! responder.
//!
//! `subscribe()` merges every connected client's outgoing writes into one
//! stream, while `send()` fans a telegram out to every connected client.

use std::collections::HashMap;
#[cfg(test)]
use std::net::Ipv4Addr;
use std::net::SocketAddr;
#[cfg(feature = "secure")]
use std::sync::atomic::AtomicU16;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{RwLock, broadcast, oneshot, watch};

use crate::error::{Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::address::IndividualAddress;
use crate::protocol::cemi::{CemiFrame, MessageCode};
use crate::protocol::knxip::{
    ConnectRequest, ConnectResponse, ConnectionRequestInfo, ConnectionstateRequest,
    ConnectionstateResponse, DisconnectRequest, DisconnectResponse, Hpai, KnxIpFrame, ServiceType,
    TunnellingAck, TunnellingRequest,
};
#[cfg(all(test, feature = "secure"))]
use crate::protocol::knxip::{SessionRequest, SessionResponse};
use crate::protocol::telegram::Telegram;
#[cfg(test)]
use crate::protocol::telegram::TelegramType;
#[cfg(all(test, feature = "secure"))]
use crate::security::SecureSession;
#[cfg(feature = "secure")]
use crate::security::SessionConfig;

#[cfg(feature = "secure")]
use super::frame_transport::TransportKind;
use super::frame_transport::{FrameTransport, TcpFrameTransport};
use super::tunnel::SequenceValidationResult;

mod device_management;
mod discovery;
#[cfg(feature = "secure")]
mod secure;

#[cfg(feature = "secure")]
use secure::SecureSessionEntry;

pub use device_management::DeviceManagementHandler;

/// How long a connected client may go without a heartbeat before eviction.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(2 * 60);
/// How often the staleness sweep runs.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// How long to wait for a `TunnellingAck` before retrying once.
const ACK_TIMEOUT: Duration = Duration::from_secs(1);

/// How to reach one connected client, abstracting over the shared UDP
/// socket (`send_to` a specific peer) vs. a TCP connection's own stream.
#[derive(Clone)]
enum ClientLink {
    Udp {
        socket: Weak<UdpSocket>,
        peer_addr: SocketAddr,
    },
    Tcp {
        transport: Arc<TcpFrameTransport>,
        peer_addr: SocketAddr,
    },
}

impl ClientLink {
    async fn send_frame(&self, frame: &[u8]) -> Result<()> {
        match self {
            ClientLink::Udp { socket, peer_addr } => {
                let socket = socket.upgrade().ok_or(TransportError::ConnectionClosed)?;
                socket.send_to(frame, *peer_addr).await.map_err(|e| {
                    TransportError::SocketError {
                        operation: "send_to".to_string(),
                        source: e,
                    }
                })?;
                Ok(())
            }
            ClientLink::Tcp { transport, .. } => transport.send_frame(frame).await,
        }
    }

    fn peer_addr(&self) -> SocketAddr {
        match self {
            ClientLink::Udp { peer_addr, .. } | ClientLink::Tcp { peer_addr, .. } => *peer_addr,
        }
    }

    fn same_peer(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Udp { peer_addr: a, .. }, Self::Udp { peer_addr: b, .. }) => a == b,
            (Self::Tcp { transport: a, .. }, Self::Tcp { transport: b, .. }) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    #[cfg(feature = "secure")]
    fn session_peer(&self) -> (TransportKind, SocketAddr) {
        let kind = match self {
            Self::Udp { .. } => TransportKind::Udp,
            Self::Tcp { .. } => TransportKind::Tcp,
        };
        (kind, self.peer_addr())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientSessionKind {
    Tunnel,
    DeviceManagement,
}

struct ClientSession {
    channel_id: u8,
    control_link: ClientLink,
    link: ClientLink,
    kind: ClientSessionKind,
    send_seq: AtomicU8,
    recv_seq: AtomicU8,
    last_heartbeat: std::sync::Mutex<Instant>,
    send_lock: tokio::sync::Mutex<()>,
    pending_ack: std::sync::Mutex<Option<(u8, oneshot::Sender<TunnellingAck>)>>,
    last_device_response: std::sync::Mutex<Option<(u8, u8, Vec<u8>)>>,
}

impl ClientSession {
    fn new(
        channel_id: u8,
        control_link: ClientLink,
        link: ClientLink,
        kind: ClientSessionKind,
    ) -> Self {
        Self {
            channel_id,
            control_link,
            link,
            kind,
            send_seq: AtomicU8::new(0),
            recv_seq: AtomicU8::new(0),
            last_heartbeat: std::sync::Mutex::new(Instant::now()),
            send_lock: tokio::sync::Mutex::new(()),
            pending_ack: std::sync::Mutex::new(None),
            last_device_response: std::sync::Mutex::new(None),
        }
    }

    fn validate_recv_seq(&self, received: u8) -> SequenceValidationResult {
        let expected = self.recv_seq.load(Ordering::SeqCst);
        if received == expected {
            self.recv_seq
                .store(expected.wrapping_add(1), Ordering::SeqCst);
            SequenceValidationResult::Valid
        } else if received == expected.wrapping_sub(1) {
            SequenceValidationResult::Duplicate
        } else {
            SequenceValidationResult::Invalid { expected, received }
        }
    }

    fn touch(&self) {
        *self.last_heartbeat.lock().unwrap() = Instant::now();
    }

    fn is_stale(&self) -> bool {
        self.last_heartbeat.lock().unwrap().elapsed() > CLIENT_TIMEOUT
    }
}

/// A KNXnet/IP tunneling server bound to one UDP socket and one TCP listener
/// on the same address. Dropping the last handle stops the server.
pub struct TunnelServer {
    socket: Weak<UdpSocket>,
    local_addr: SocketAddr,
    individual_address: IndividualAddress,
    sessions: RwLock<HashMap<u8, Arc<ClientSession>>>,
    device_management_handler: RwLock<Option<Arc<dyn DeviceManagementHandler>>>,
    /// Present only if this endpoint requires KNX IP Secure; if so, plain
    /// ConnectRequest/TunnellingRequest/etc. are rejected and clients must
    /// complete a SessionRequest/Response/Authenticate/Status handshake
    /// first, wrapping everything else in `SecureWrapper`.
    #[cfg(feature = "secure")]
    secure_config: Option<SessionConfig>,
    /// Per-peer secure sessions, keyed by address regardless of transport —
    /// covers both in-progress handshakes (before a tunnel channel exists)
    /// and established sessions (used to decrypt/encrypt every frame after).
    #[cfg(feature = "secure")]
    secure_sessions: RwLock<HashMap<(TransportKind, SocketAddr), Arc<SecureSessionEntry>>>,
    #[cfg(feature = "secure")]
    next_secure_session_id: AtomicU16,
    telegram_tx: broadcast::Sender<Telegram>,
    shutdown: watch::Sender<bool>,
    discovery_handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl TunnelServer {
    /// Bind a plaintext-only tunneling server (UDP + TCP) on `addr`.
    ///
    /// `individual_address` is reported to every connecting client as its
    /// assigned tunnel address (this server does not allocate distinct KNX
    /// individual addresses per client — it's a software bridge endpoint, not
    /// a full line's worth of real device addresses).
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::SocketError`] if binding the UDP or TCP
    /// listener on `addr` fails.
    pub async fn bind(
        addr: SocketAddr,
        individual_address: IndividualAddress,
    ) -> Result<Arc<Self>> {
        #[cfg(feature = "secure")]
        return Self::bind_with_security(addr, individual_address, None).await;
        #[cfg(not(feature = "secure"))]
        return Self::bind_with_security(addr, individual_address).await;
    }

    async fn bind_with_security(
        addr: SocketAddr,
        individual_address: IndividualAddress,
        #[cfg(feature = "secure")] secure_config: Option<SessionConfig>,
    ) -> Result<Arc<Self>> {
        let socket = UdpSocket::bind(addr)
            .await
            .map_err(|e| TransportError::SocketError {
                operation: "bind".to_string(),
                source: e,
            })?;
        let local_addr = socket
            .local_addr()
            .map_err(|e| TransportError::SocketError {
                operation: "get_local_addr".to_string(),
                source: e,
            })?;
        // Bind TCP on the same resolved address (UDP and TCP port namespaces
        // are independent, so this works even when `addr`'s port was 0).
        let tcp_listener =
            TcpListener::bind(local_addr)
                .await
                .map_err(|e| TransportError::SocketError {
                    operation: "tcp_bind".to_string(),
                    source: e,
                })?;
        let (telegram_tx, _) = broadcast::channel(1024);

        let socket = Arc::new(socket);
        let server = Arc::new(Self {
            socket: Arc::downgrade(&socket),
            local_addr,
            individual_address,
            sessions: RwLock::new(HashMap::new()),
            device_management_handler: RwLock::new(None),
            #[cfg(feature = "secure")]
            secure_config,
            #[cfg(feature = "secure")]
            secure_sessions: RwLock::new(HashMap::new()),
            #[cfg(feature = "secure")]
            next_secure_session_id: AtomicU16::new(1),
            telegram_tx,
            shutdown: watch::Sender::new(false),
            discovery_handle: std::sync::Mutex::new(None),
        });
        server.spawn_dispatch_loop(socket);
        server.spawn_tcp_accept_loop(tcp_listener);
        server.spawn_sweep_loop();
        log_transport!(
            LogLevel::Info,
            "TunnelServer listening on {} (udp+tcp)",
            local_addr
        );
        Ok(server)
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Number of currently-connected clients.
    pub async fn client_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    fn spawn_dispatch_loop(self: &Arc<Self>, socket: Arc<UdpSocket>) {
        let server = Arc::downgrade(self);
        let mut shutdown = self.shutdown.subscribe();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            loop {
                let (len, peer_addr) = tokio::select! {
                    biased;
                    _ = shutdown.wait_for(|closed| *closed) => break,
                    result = socket.recv_from(&mut buf) => match result {
                        Ok(v) => v,
                        Err(e) => {
                            log_transport!(LogLevel::Warn, "TunnelServer: recv error: {}", e);
                            continue;
                        }
                    },
                };
                let Some(server) = server.upgrade() else {
                    break;
                };
                let link = ClientLink::Udp {
                    socket: server.socket.clone(),
                    peer_addr,
                };
                if let Err(e) = server.handle_frame(&buf[..len], link).await {
                    log_transport!(
                        LogLevel::Warn,
                        "TunnelServer: error handling frame from {}: {}",
                        peer_addr,
                        e
                    );
                }
            }
            if let Some(server) = server.upgrade() {
                server.sessions.write().await.clear();
                #[cfg(feature = "secure")]
                server.secure_sessions.write().await.clear();
            }
        });
    }

    /// Accept TCP tunneling connections; each accepted stream gets its own
    /// task doing its own re-framed `recv_frame()` loop, feeding the same
    /// `handle_frame` dispatch the UDP loop uses.
    fn spawn_tcp_accept_loop(self: &Arc<Self>, listener: TcpListener) {
        let server = Arc::downgrade(self);
        let mut shutdown = self.shutdown.subscribe();
        tokio::spawn(async move {
            loop {
                let (stream, peer_addr) = tokio::select! {
                    biased;
                    _ = shutdown.wait_for(|closed| *closed) => break,
                    result = listener.accept() => match result {
                        Ok(v) => v,
                        Err(e) => {
                            log_transport!(LogLevel::Warn, "TunnelServer: tcp accept error: {}", e);
                            continue;
                        }
                    },
                };
                let server = server.clone();
                let mut shutdown = shutdown.clone();
                tokio::spawn(async move {
                    let transport = Arc::new(TcpFrameTransport::from_accepted_stream(stream));
                    let link = ClientLink::Tcp {
                        transport: transport.clone(),
                        peer_addr,
                    };
                    loop {
                        let received = tokio::select! {
                            biased;
                            _ = shutdown.wait_for(|closed| *closed) => break,
                            received = transport.recv_frame() => received,
                        };
                        let Some(server) = server.upgrade() else {
                            break;
                        };
                        match received {
                            Ok(data) => match server.handle_frame(&data, link.clone()).await {
                                Ok(()) => {}
                                Err(e) => log_transport!(
                                    LogLevel::Warn,
                                    "TunnelServer: tcp frame error from {}: {}",
                                    peer_addr,
                                    e
                                ),
                            },
                            Err(_) => break,
                        }
                    }
                    let Some(server) = server.upgrade() else {
                        return;
                    };
                    server
                        .sessions
                        .write()
                        .await
                        .retain(|_, session| !session.control_link.same_peer(&link));
                    #[cfg(feature = "secure")]
                    server
                        .secure_sessions
                        .write()
                        .await
                        .remove(&link.session_peer());
                });
            }
        });
    }

    fn spawn_sweep_loop(self: &Arc<Self>) {
        let server = Arc::downgrade(self);
        let mut shutdown = self.shutdown.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(SWEEP_INTERVAL) => {}
                    _ = shutdown.wait_for(|closed| *closed) => break,
                }
                let Some(server) = server.upgrade() else {
                    break;
                };
                let stale: Vec<u8> = {
                    let sessions = server.sessions.read().await;
                    sessions
                        .iter()
                        .filter(|(_, s)| s.is_stale())
                        .map(|(id, _)| *id)
                        .collect()
                };
                if !stale.is_empty() {
                    let mut sessions = server.sessions.write().await;
                    for id in stale {
                        sessions.remove(&id);
                        log_transport!(
                            LogLevel::Info,
                            "TunnelServer: evicted stale channel {}",
                            id
                        );
                    }
                }
            }
        });
    }

    /// Dispatch one received frame.
    async fn handle_frame(self: &Arc<Self>, data: &[u8], link: ClientLink) -> Result<()> {
        let frame = KnxIpFrame::parse(data)?;
        match frame.header.service_type {
            ServiceType::SearchRequest
            | ServiceType::SearchRequestExtended
            | ServiceType::DescriptionRequest => {
                self.handle_discovery_request(frame, &link).await?;
                Ok(())
            }
            #[cfg(feature = "secure")]
            ServiceType::SessionRequest => {
                self.handle_session_request(&frame.body, link).await?;
                Ok(())
            }
            #[cfg(feature = "secure")]
            ServiceType::SecureWrapper => self.handle_secure_wrapper(data, link).await,
            _ => self.dispatch_plain_frame(frame, link, false).await,
        }
    }

    /// Dispatches plaintext frames and frames unwrapped from a `SecureWrapper`.
    /// Secure-only endpoints reject plaintext to avoid transport fallback.
    async fn dispatch_plain_frame(
        self: &Arc<Self>,
        frame: KnxIpFrame,
        link: ClientLink,
        #[cfg_attr(not(feature = "secure"), allow(unused_variables))] was_encrypted: bool,
    ) -> Result<()> {
        #[cfg(feature = "secure")]
        if self.secure_config.is_some() && !was_encrypted {
            log_transport!(
                LogLevel::Warn,
                "TunnelServer: rejecting plaintext {:?} from {} (secure required)",
                frame.header.service_type,
                link.peer_addr()
            );
            return Ok(());
        }
        match frame.header.service_type {
            ServiceType::ConnectRequest => self.handle_connect_request(&frame.body, link).await,
            ServiceType::TunnellingRequest => {
                self.handle_tunnelling_request(&frame.body, link).await
            }
            ServiceType::DeviceConfigurationRequest => {
                self.handle_device_configuration_request(&frame.body, &link)
                    .await
            }
            ServiceType::TunnellingAck => {
                self.handle_tunnelling_ack(&frame.body, &link, ClientSessionKind::Tunnel)
                    .await
            }
            ServiceType::DeviceConfigurationAck => {
                self.handle_tunnelling_ack(&frame.body, &link, ClientSessionKind::DeviceManagement)
                    .await
            }
            ServiceType::ConnectionstateRequest => {
                self.handle_connectionstate_request(&frame.body, link).await
            }
            ServiceType::DisconnectRequest => {
                self.handle_disconnect_request(&frame.body, link).await
            }
            other => {
                log_transport!(
                    LogLevel::Trace,
                    "TunnelServer: unhandled frame {:?} from {}",
                    other,
                    link.peer_addr()
                );
                Ok(())
            }
        }
    }

    #[cfg(not(feature = "secure"))]
    async fn send_session_frame(&self, frame: &[u8], link: &ClientLink) -> Result<()> {
        link.send_frame(frame).await
    }

    /// Route-back (`0.0.0.0:0` / port 0) means "reply to the packet's source
    /// address". Alternate ports on the same host support split UDP endpoints.
    fn resolve_link(&self, hpai: &Hpai, link: &ClientLink) -> ClientLink {
        #[cfg(feature = "secure")]
        if self.secure_config.is_some() {
            return link.clone();
        }
        match link {
            ClientLink::Udp { socket, peer_addr } => {
                let resolved = if hpai.port != 0
                    && hpai.host_protocol_code == Hpai::PROTOCOL_UDP
                    && (hpai.ip_addr.is_unspecified() || hpai.ip_addr == peer_addr.ip())
                {
                    SocketAddr::new(peer_addr.ip(), hpai.port)
                } else {
                    *peer_addr
                };
                ClientLink::Udp {
                    socket: socket.clone(),
                    peer_addr: resolved,
                }
            }
            ClientLink::Tcp { .. } => link.clone(),
        }
    }

    fn allocate_channel_id(sessions: &HashMap<u8, Arc<ClientSession>>) -> Option<u8> {
        (1u8..=255).find(|id| !sessions.contains_key(id))
    }

    async fn handle_connect_request(&self, body: &[u8], link: ClientLink) -> Result<()> {
        let request = ConnectRequest::parse(body)?;
        let kind = match request.cri.connection_type {
            ConnectionRequestInfo::TUNNEL_CONNECTION => ClientSessionKind::Tunnel,
            ConnectionRequestInfo::DEVICE_MANAGEMENT_CONNECTION => {
                if self.device_management_handler.read().await.is_none() {
                    let response = ConnectResponse {
                        channel_id: 0,
                        status: ConnectResponse::STATUS_ERROR_CONNECTION_TYPE,
                        data_endpoint: Hpai::new(self.local_addr),
                        crd: Vec::new(),
                        assigned_address: None,
                    };
                    self.send_reply(ServiceType::ConnectResponse, &response.serialize(), &link)
                        .await?;
                    return Ok(());
                }
                ClientSessionKind::DeviceManagement
            }
            _ => {
                let response = ConnectResponse {
                    channel_id: 0,
                    status: ConnectResponse::STATUS_ERROR_CONNECTION_TYPE,
                    data_endpoint: Hpai::new(self.local_addr),
                    crd: Vec::new(),
                    assigned_address: None,
                };
                self.send_reply(ServiceType::ConnectResponse, &response.serialize(), &link)
                    .await?;
                return Ok(());
            }
        };
        let control_link = self.resolve_link(&request.control_endpoint, &link);
        let data_link = self.resolve_link(&request.data_endpoint, &link);

        let channel_id = {
            let mut sessions = self.sessions.write().await;
            if let Some(id) = Self::allocate_channel_id(&sessions) {
                sessions.insert(
                    id,
                    Arc::new(ClientSession::new(
                        id,
                        control_link.clone(),
                        data_link.clone(),
                        kind,
                    )),
                );
                id
            } else {
                let response = ConnectResponse {
                    channel_id: 0,
                    status: ConnectResponse::STATUS_ERROR_NO_MORE_CONNECTIONS,
                    data_endpoint: Hpai::new(self.local_addr),
                    crd: Vec::new(),
                    assigned_address: None,
                };
                drop(sessions);
                self.send_reply(
                    ServiceType::ConnectResponse,
                    &response.serialize(),
                    &control_link,
                )
                .await?;
                return Ok(());
            }
        };

        let (data_endpoint, crd, assigned_address) = match kind {
            ClientSessionKind::Tunnel => (
                match data_link {
                    ClientLink::Tcp { .. } => Hpai::new_tcp_route_back(),
                    ClientLink::Udp { .. } => Hpai::new(self.local_addr),
                },
                build_tunnel_crd(self.individual_address),
                Some(self.individual_address),
            ),
            ClientSessionKind::DeviceManagement => (
                match data_link {
                    ClientLink::Tcp { .. } => Hpai::new_tcp_route_back(),
                    ClientLink::Udp { .. } => Hpai::new(self.local_addr),
                },
                ConnectionRequestInfo::new_device_management().serialize(),
                None,
            ),
        };
        let response = ConnectResponse {
            channel_id,
            status: ConnectResponse::STATUS_OK,
            data_endpoint,
            crd,
            assigned_address,
        };
        self.send_reply(
            ServiceType::ConnectResponse,
            &response.serialize(),
            &control_link,
        )
        .await?;
        log_transport!(
            LogLevel::Info,
            "TunnelServer: client {} connected, channel_id={}",
            control_link.peer_addr(),
            channel_id
        );
        Ok(())
    }

    async fn data_session(
        &self,
        channel_id: u8,
        link: &ClientLink,
        kind: ClientSessionKind,
    ) -> Option<Arc<ClientSession>> {
        self.sessions
            .read()
            .await
            .get(&channel_id)
            .filter(|session| session.kind == kind && session.link.same_peer(link))
            .cloned()
    }

    async fn handle_tunnelling_request(
        self: &Arc<Self>,
        body: &[u8],
        link: ClientLink,
    ) -> Result<()> {
        let request = TunnellingRequest::parse(body)?;
        let session = self
            .data_session(
                request.communication_channel_id,
                &link,
                ClientSessionKind::Tunnel,
            )
            .await;
        let Some(session) = session else {
            log_transport!(
                LogLevel::Warn,
                "TunnelServer: TunnellingRequest for unknown channel {}",
                request.communication_channel_id
            );
            return Ok(());
        };
        session.touch();

        match session.validate_recv_seq(request.sequence_counter) {
            SequenceValidationResult::Valid => {
                let ack = TunnellingAck::new_ok(
                    request.communication_channel_id,
                    request.sequence_counter,
                );
                self.send_tunnel_ack(ack, &session.link).await?;
                match parse_cemi_to_telegram(&request.raw_cemi) {
                    Ok(telegram) => {
                        let _ = self.telegram_tx.send(telegram);
                        let server = self.clone();
                        let session = session.clone();
                        let raw_cemi = request.raw_cemi.clone();
                        tokio::spawn(async move {
                            if let Err(error) =
                                server.send_cemi_confirmation(&session, &raw_cemi).await
                            {
                                log_transport!(
                                    LogLevel::Warn,
                                    "TunnelServer: failed to send L_Data.con: {}",
                                    error
                                );
                            }
                        });
                    }
                    Err(e) => {
                        log_transport!(
                            LogLevel::Warn,
                            "TunnelServer: bad CEMI from {}: {}",
                            link.peer_addr(),
                            e
                        );
                    }
                }
            }
            SequenceValidationResult::Duplicate => {
                let ack = TunnellingAck::new_ok(
                    request.communication_channel_id,
                    request.sequence_counter,
                );
                self.send_tunnel_ack(ack, &session.link).await?;
                let server = self.clone();
                let session = session.clone();
                let raw_cemi = request.raw_cemi.clone();
                tokio::spawn(async move {
                    if let Err(error) = server.send_cemi_confirmation(&session, &raw_cemi).await {
                        log_transport!(
                            LogLevel::Warn,
                            "TunnelServer: failed to resend L_Data.con: {}",
                            error
                        );
                    }
                });
            }
            SequenceValidationResult::Invalid { expected, received } => {
                log_transport!(
                    LogLevel::Warn,
                    "TunnelServer: sequence error on channel {} (expected {}, got {})",
                    request.communication_channel_id,
                    expected,
                    received
                );
                let ack = TunnellingAck::new_sequence_error(
                    request.communication_channel_id,
                    request.sequence_counter,
                );
                self.send_tunnel_ack(ack, &session.link).await?;
            }
        }
        Ok(())
    }

    async fn send_tunnel_ack(&self, ack: TunnellingAck, link: &ClientLink) -> Result<()> {
        if matches!(link, ClientLink::Udp { .. }) {
            self.send_reply(ServiceType::TunnellingAck, &ack.serialize(), link)
                .await?;
        }
        Ok(())
    }

    async fn send_cemi_confirmation(
        &self,
        session: &Arc<ClientSession>,
        raw_cemi: &[u8],
    ) -> Result<()> {
        let mut cemi = CemiFrame::parse(raw_cemi)?;
        if cemi.message_code != MessageCode::LDataReq {
            return Ok(());
        }
        cemi.message_code = MessageCode::LDataCon;
        self.send_cemi_to_session(session, &cemi.serialize()?).await
    }

    async fn handle_tunnelling_ack(
        &self,
        body: &[u8],
        link: &ClientLink,
        kind: ClientSessionKind,
    ) -> Result<()> {
        let ack = TunnellingAck::parse(body)?;
        let session = self
            .data_session(ack.communication_channel_id, link, kind)
            .await;
        if let Some(session) = session {
            let mut pending_ack = session.pending_ack.lock().unwrap();
            if let Some((sequence, _)) = pending_ack.as_ref()
                && *sequence == ack.sequence_counter
                && let Some((_, tx)) = pending_ack.take()
            {
                let _ = tx.send(ack);
            }
        }
        Ok(())
    }

    async fn handle_connectionstate_request(&self, body: &[u8], link: ClientLink) -> Result<()> {
        let request = ConnectionstateRequest::parse(body)?;
        let status = {
            let sessions = self.sessions.read().await;
            match sessions
                .get(&request.communication_channel_id)
                .filter(|session| session.control_link.same_peer(&link))
            {
                Some(session) => {
                    session.touch();
                    ConnectionstateResponse::STATUS_OK
                }
                None => ConnectResponse::STATUS_ERROR_CONNECTION_ID,
            }
        };
        let response = ConnectionstateResponse::new(request.communication_channel_id, status);
        self.send_reply(
            ServiceType::ConnectionstateResponse,
            &response.serialize(),
            &link,
        )
        .await
    }

    async fn handle_disconnect_request(&self, body: &[u8], link: ClientLink) -> Result<()> {
        let request = DisconnectRequest::parse(body)?;
        let removed = {
            let mut sessions = self.sessions.write().await;
            if sessions
                .get(&request.communication_channel_id)
                .is_some_and(|session| session.control_link.same_peer(&link))
            {
                sessions.remove(&request.communication_channel_id);
                true
            } else {
                false
            }
        };
        let status = if removed {
            log_transport!(
                LogLevel::Info,
                "TunnelServer: client channel {} disconnected",
                request.communication_channel_id
            );
            DisconnectResponse::STATUS_OK
        } else {
            DisconnectResponse::STATUS_ERROR_CONNECTION_ID
        };
        let response = DisconnectResponse::new(request.communication_channel_id, status);
        let result = self
            .send_reply(
                ServiceType::DisconnectResponse,
                &response.serialize(),
                &link,
            )
            .await;
        #[cfg(feature = "secure")]
        if removed {
            self.secure_sessions
                .write()
                .await
                .remove(&link.session_peer());
        }
        result
    }

    async fn send_reply(
        &self,
        service_type: ServiceType,
        body: &[u8],
        link: &ClientLink,
    ) -> Result<()> {
        let frame = KnxIpFrame::new(service_type, body.to_vec()).serialize();
        self.send_session_frame(&frame, link).await
    }

    /// Send one CEMI frame, retrying UDP once after an ACK timeout.
    async fn send_cemi_to_session(&self, session: &Arc<ClientSession>, cemi: &[u8]) -> Result<()> {
        let _guard = session.send_lock.lock().await;
        let seq = session.send_seq.load(Ordering::SeqCst);
        let request = TunnellingRequest::new(session.channel_id, seq, cemi.to_vec());
        let frame =
            KnxIpFrame::new(ServiceType::TunnellingRequest, request.serialize()).serialize();
        if matches!(session.link, ClientLink::Tcp { .. }) {
            self.send_session_frame(&frame, &session.link).await?;
            session
                .send_seq
                .store(seq.wrapping_add(1), Ordering::SeqCst);
            return Ok(());
        }

        for attempt in 0..2u8 {
            let (tx, rx) = oneshot::channel();
            *session.pending_ack.lock().unwrap() = Some((seq, tx));
            self.send_session_frame(&frame, &session.link).await?;

            match tokio::time::timeout(ACK_TIMEOUT, rx).await {
                Ok(Ok(ack)) if ack.status_code == TunnellingAck::STATUS_OK => {
                    session
                        .send_seq
                        .store(seq.wrapping_add(1), Ordering::SeqCst);
                    return Ok(());
                }
                Ok(Ok(_)) => {
                    return Err(TransportError::InvalidConfiguration {
                        details: "client rejected TunnellingRequest".to_string(),
                    }
                    .into());
                }
                _ if attempt == 0 => {
                    log_transport!(
                        LogLevel::Warn,
                        "TunnelServer: ack timeout for channel {}, retrying once",
                        session.channel_id
                    );
                }
                _ => {}
            }
        }
        Err(TransportError::Timeout {
            timeout_ms: ACK_TIMEOUT.as_millis() as u64,
        }
        .into())
    }
}

impl TunnelServer {
    /// Send a telegram to every currently connected tunnelling client.
    ///
    /// # Errors
    ///
    /// Returns an error if the telegram cannot be serialized. Transport failures
    /// are logged and the failed tunnel session is removed.
    pub async fn send(&self, telegram: Telegram) -> Result<()> {
        let cemi = CemiFrame::from_telegram(&telegram, MessageCode::LDataInd).serialize()?;
        let sessions: Vec<_> = self
            .sessions
            .read()
            .await
            .values()
            .filter(|session| session.kind == ClientSessionKind::Tunnel)
            .cloned()
            .collect();
        for session in &sessions {
            if let Err(e) = self.send_cemi_to_session(session, &cemi).await {
                log_transport!(
                    LogLevel::Warn,
                    "TunnelServer: send to {} failed: {}",
                    session.link.peer_addr(),
                    e
                );
                let mut sessions = self.sessions.write().await;
                if sessions
                    .get(&session.channel_id)
                    .is_some_and(|current| Arc::ptr_eq(current, session))
                {
                    sessions.remove(&session.channel_id);
                }
            }
        }
        Ok(())
    }

    /// Subscribe to telegrams received from connected tunnelling clients.
    pub fn subscribe(&self) -> broadcast::Receiver<Telegram> {
        self.telegram_tx.subscribe()
    }

    /// Signal the server and all of its connection loops to shut down.
    ///
    /// # Panics
    ///
    /// Panics if the discovery handle lock was poisoned by another thread.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
        if let Some(discovery) = self.discovery_handle.lock().unwrap().take() {
            discovery.abort();
        }
        log_transport!(LogLevel::Info, "TunnelServer shutdown requested");
    }

    /// Return whether shutdown has been requested.
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        *self.shutdown.borrow()
    }
}

#[cfg(test)]
fn build_tunnelling_frame_with_code(
    telegram: &Telegram,
    channel_id: u8,
    sequence: u8,
    message_code: MessageCode,
) -> Result<Vec<u8>> {
    let request = TunnellingRequest::new(
        channel_id,
        sequence,
        CemiFrame::from_telegram(telegram, message_code).serialize()?,
    );
    Ok(KnxIpFrame::new(ServiceType::TunnellingRequest, request.serialize()).serialize())
}

#[cfg(test)]
fn build_tunnelling_frame(telegram: &Telegram, channel_id: u8, sequence: u8) -> Result<Vec<u8>> {
    build_tunnelling_frame_with_code(telegram, channel_id, sequence, MessageCode::LDataReq)
}

/// Build the Connection Response Data for a tunnel connection CRD:
/// `[struct_len=4, TUNNEL_CONNECTION, addr_high, addr_low]`.
fn build_tunnel_crd(addr: IndividualAddress) -> Vec<u8> {
    let [hi, lo] = addr.raw().to_be_bytes();
    vec![4, ConnectionRequestInfo::TUNNEL_CONNECTION, hi, lo]
}

fn parse_cemi_to_telegram(cemi_data: &[u8]) -> Result<Telegram> {
    Telegram::from_cemi_frame(CemiFrame::parse(cemi_data)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::address::{IndividualAddress as Ia, RawGroupAddress};
    use crate::transport::tunnel::Tunnel;
    use std::time::Duration;
    use tokio::time::timeout;

    async fn bind_server() -> Arc<TunnelServer> {
        TunnelServer::bind("127.0.0.1:0".parse().unwrap(), Ia::new(1, 1, 240))
            .await
            .unwrap()
    }

    async fn connected_client(server_addr: SocketAddr) -> Tunnel {
        let mut tunnel = Tunnel::new_udp(server_addr);
        tunnel.connect().await.unwrap();
        tunnel
    }

    #[tokio::test]
    async fn inherent_shutdown_state_tracks_lifecycle() {
        let server = bind_server().await;
        assert!(!server.is_shutdown());

        server.shutdown();

        assert!(server.is_shutdown());
    }

    #[tokio::test]
    async fn stopping_server_releases_sockets_and_idle_streams() {
        for explicit_shutdown in [true, false] {
            let server = bind_server().await;
            let addr = server.local_addr();
            let tcp = connected_tcp_client(addr).await;
            let _udp = connected_client(addr).await;
            assert_eq!(server.client_count().await, 2);

            let retained = if explicit_shutdown {
                server.shutdown();
                Some(server)
            } else {
                drop(server);
                None
            };
            assert!(
                timeout(Duration::from_secs(1), tcp.recv_frame())
                    .await
                    .unwrap()
                    .is_err()
            );
            timeout(Duration::from_secs(2), async {
                loop {
                    if let Ok(rebound) = TunnelServer::bind(addr, Ia::new(1, 1, 240)).await {
                        rebound.shutdown();
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("both listener sockets should be reusable");
            if let Some(server) = retained {
                assert_eq!(server.client_count().await, 0);
            }
        }
    }

    #[tokio::test]
    async fn udp_channels_honor_split_control_and_data_endpoints() {
        let server = bind_server().await;
        let control = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let data = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let control_addr = control.local_addr().unwrap();
        let data_addr = data.local_addr().unwrap();
        control.connect(server.local_addr()).await.unwrap();
        data.connect(server.local_addr()).await.unwrap();

        let request = ConnectRequest::new(control_addr, data_addr);
        let frame = KnxIpFrame::new(ServiceType::ConnectRequest, request.serialize()).serialize();
        control.send(&frame).await.unwrap();
        let mut buf = [0; 1024];
        let len = timeout(Duration::from_secs(1), control.recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let frame = KnxIpFrame::parse(&buf[..len]).unwrap();
        assert_eq!(frame.header.service_type, ServiceType::ConnectResponse);
        let response = ConnectResponse::parse(&frame.body).unwrap();
        assert_eq!(response.status, ConnectResponse::STATUS_OK);
        let id = response.channel_id;
        let data_link = ClientLink::Udp {
            socket: server.socket.clone(),
            peer_addr: data_addr,
        };
        let control_link = ClientLink::Udp {
            socket: server.socket.clone(),
            peer_addr: control_addr,
        };
        assert!(
            server
                .data_session(id, &data_link, ClientSessionKind::Tunnel)
                .await
                .is_some()
        );
        assert!(
            server
                .data_session(id, &control_link, ClientSessionKind::Tunnel)
                .await
                .is_none()
        );
        assert!(
            server
                .data_session(id, &data_link, ClientSessionKind::DeviceManagement)
                .await
                .is_none()
        );

        let request = ConnectionstateRequest::new(id, Hpai::new(control_addr));
        let frame =
            KnxIpFrame::new(ServiceType::ConnectionstateRequest, request.serialize()).serialize();
        control.send(&frame).await.unwrap();
        let len = timeout(Duration::from_secs(1), control.recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let frame = KnxIpFrame::parse(&buf[..len]).unwrap();
        assert_eq!(
            frame.header.service_type,
            ServiceType::ConnectionstateResponse
        );
        assert_eq!(
            ConnectionstateResponse::parse(&frame.body).unwrap().status,
            ConnectionstateResponse::STATUS_OK
        );
    }

    #[tokio::test]
    async fn scanner_finds_multiple_knust_servers_by_multicast() {
        use crate::transport::GatewayScanner;
        use std::io::ErrorKind;

        let first = bind_server().await;
        let second = bind_server().await;
        for server in [&first, &second] {
            server
                .enable_multicast_discovery(Ipv4Addr::LOCALHOST)
                .unwrap();
        }
        let duplicate = first.enable_multicast_discovery(Ipv4Addr::LOCALHOST);
        assert!(matches!(
            duplicate,
            Err(crate::error::KnxError::Transport(TransportError::SocketError { source, .. }))
                if source.kind() == ErrorKind::AlreadyExists
        ));

        let scanner = GatewayScanner::new_with_interface(Ipv4Addr::LOCALHOST)
            .await
            .unwrap();
        let gateways = scanner.discover(Duration::from_millis(250)).await.unwrap();
        for server in [&first, &second] {
            let gateway = gateways
                .iter()
                .find(|gateway| gateway.addr == server.local_addr());
            assert_eq!(
                gateway.and_then(|gateway| gateway.individual_address),
                Some(server.individual_address)
            );
            assert_eq!(server.client_count().await, 0);
        }

        let listener = first
            .discovery_handle
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .abort_handle();
        first.shutdown();
        timeout(Duration::from_secs(1), async {
            while !listener.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let gateways = scanner.discover(Duration::from_millis(250)).await.unwrap();
        assert!(
            !gateways
                .iter()
                .any(|gateway| gateway.addr == first.local_addr())
        );
        assert!(
            gateways
                .iter()
                .any(|gateway| gateway.addr == second.local_addr())
        );
        assert!(
            first
                .enable_multicast_discovery(Ipv4Addr::LOCALHOST)
                .is_err()
        );
        second.shutdown();
    }

    fn test_telegram(payload: u8) -> Telegram {
        Telegram::received(
            Ia::new(1, 1, 5),
            RawGroupAddress::from_parts(1, 2, 3).unwrap(),
            TelegramType::GroupValueWrite,
            vec![payload],
        )
    }

    #[test]
    fn server_serialization_preserves_individual_tpci() {
        use crate::protocol::knxip::{KnxIpFrame, TunnellingRequest};
        use crate::protocol::{Address, IndividualAddress, TpciFrame};

        for (telegram_type, tpci, apdu) in [
            (
                TelegramType::IndividualData,
                TpciFrame::DataConnected { sequence: 3 },
                vec![0x02, 0xC8],
            ),
            (
                TelegramType::IndividualControl,
                TpciFrame::Ack { sequence: 3 },
                vec![],
            ),
        ] {
            let mut telegram =
                Telegram::individual_data(IndividualAddress::new(1, 2, 3), apdu.clone());
            telegram.telegram_type = telegram_type;
            telegram.tpci = tpci.encode();
            let wire =
                build_tunnelling_frame_with_code(&telegram, 1, 0, MessageCode::LDataInd).unwrap();
            let frame = KnxIpFrame::parse(&wire).unwrap();
            let request = TunnellingRequest::parse(&frame.body).unwrap();
            let cemi = CemiFrame::parse(&request.raw_cemi).unwrap();
            assert_eq!(
                cemi.dest_addr,
                Address::Individual(IndividualAddress::new(1, 2, 3))
            );
            assert_eq!(cemi.tpci, tpci.encode());
            assert_eq!(cemi.apci_data, apdu);
        }
    }

    #[cfg(feature = "dpt")]
    #[test]
    fn octet_server_frames_preserve_full_octets_and_service() {
        use crate::protocol::dpt::{DptValue, Scaling};
        let address = crate::protocol::address::RawGroupAddress::try_from_raw(0x1234).unwrap();
        for byte in 0..=u8::MAX {
            let value = Scaling::from_bytes(&[byte]).unwrap();
            for telegram in [
                Telegram::group_write_value(address, value.clone()).unwrap(),
                Telegram::group_response_value(address, value).unwrap(),
            ] {
                let wire = build_tunnelling_frame_with_code(&telegram, 1, 0, MessageCode::LDataInd)
                    .unwrap();
                let frame = KnxIpFrame::parse(&wire).unwrap();
                let request = TunnellingRequest::parse(&frame.body).unwrap();
                let received = parse_cemi_to_telegram(&request.raw_cemi).unwrap();
                assert_eq!(received.payload, [byte]);
                assert!(!received.compact_payload);
                assert_eq!(received.telegram_type, telegram.telegram_type);
            }
        }
    }

    /// The client acks the next `TunnellingRequest` it receives (mirrors what a
    /// real tunneling client does when the server sends it a telegram).
    async fn client_ack_next(tunnel: &Tunnel) {
        let data = timeout(Duration::from_secs(1), tunnel.recv_frame())
            .await
            .unwrap()
            .unwrap();
        let frame = KnxIpFrame::parse(&data).unwrap();
        assert_eq!(frame.header.service_type, ServiceType::TunnellingRequest);
        let req = TunnellingRequest::parse(&frame.body).unwrap();
        let ack = TunnellingAck::new_ok(req.communication_channel_id, req.sequence_counter);
        let ack_frame = KnxIpFrame::new(ServiceType::TunnellingAck, ack.serialize()).serialize();
        tunnel.send_frame(&ack_frame).await.unwrap();
    }

    #[tokio::test]
    async fn connect_handshake_assigns_channel_id() {
        let server = bind_server().await;
        let tunnel = connected_client(server.local_addr()).await;
        assert!(tunnel.is_connected());
        // Give the server's dispatch loop a moment to record the session.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(server.client_count().await, 1);
    }

    #[tokio::test]
    async fn client_write_reaches_server_subscribe() {
        let server = bind_server().await;
        let tunnel = connected_client(server.local_addr()).await;
        let mut rx = server.subscribe();

        let frame = build_tunnelling_frame(
            &test_telegram(1),
            tunnel.channel_id(),
            tunnel.next_sequence(),
        )
        .unwrap();
        tunnel.send_frame(&frame).await.unwrap();

        let received = timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.payload, vec![1]);

        // Server should have replied with an ack.
        let ack_data = timeout(Duration::from_secs(1), tunnel.recv_frame())
            .await
            .unwrap()
            .unwrap();
        let ack_frame = KnxIpFrame::parse(&ack_data).unwrap();
        assert_eq!(ack_frame.header.service_type, ServiceType::TunnellingAck);
    }

    #[tokio::test]
    async fn server_send_reaches_connected_client() {
        let server = bind_server().await;
        let tunnel = connected_client(server.local_addr()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        let (send_result, ()) =
            tokio::join!(server.send(test_telegram(9)), client_ack_next(&tunnel));
        send_result.unwrap();
    }

    #[tokio::test]
    async fn server_ignores_tunnelling_ack_with_wrong_sequence() {
        let server = bind_server().await;
        let tunnel = connected_client(server.local_addr()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        let client = async {
            let first = timeout(Duration::from_secs(1), tunnel.recv_frame())
                .await
                .unwrap()
                .unwrap();
            let first_frame = KnxIpFrame::parse(&first).unwrap();
            let first_request = TunnellingRequest::parse(&first_frame.body).unwrap();
            let wrong_ack = TunnellingAck::new_ok(
                first_request.communication_channel_id,
                first_request.sequence_counter.wrapping_add(1),
            );
            tunnel
                .send_frame(
                    &KnxIpFrame::new(ServiceType::TunnellingAck, wrong_ack.serialize()).serialize(),
                )
                .await
                .unwrap();

            let retry = timeout(Duration::from_secs(2), tunnel.recv_frame())
                .await
                .unwrap()
                .unwrap();
            let retry_frame = KnxIpFrame::parse(&retry).unwrap();
            let retry_request = TunnellingRequest::parse(&retry_frame.body).unwrap();
            assert_eq!(
                retry_request.sequence_counter,
                first_request.sequence_counter
            );
            assert_eq!(retry_request.raw_cemi, first_request.raw_cemi);

            let ack = TunnellingAck::new_ok(
                retry_request.communication_channel_id,
                retry_request.sequence_counter,
            );
            tunnel
                .send_frame(
                    &KnxIpFrame::new(ServiceType::TunnellingAck, ack.serialize()).serialize(),
                )
                .await
                .unwrap();
        };

        let (sent, ()) = tokio::join!(server.send(test_telegram(10)), client);
        sent.unwrap();
    }

    #[tokio::test]
    async fn heartbeat_round_trip() {
        let server = bind_server().await;
        let tunnel = connected_client(server.local_addr()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        tunnel.send_connectionstate_request().await.unwrap();
        let data = timeout(Duration::from_secs(1), tunnel.recv_frame())
            .await
            .unwrap()
            .unwrap();
        let frame = KnxIpFrame::parse(&data).unwrap();
        assert_eq!(
            frame.header.service_type,
            ServiceType::ConnectionstateResponse
        );
        let resp = ConnectionstateResponse::parse(&frame.body).unwrap();
        assert!(resp.is_success());
    }

    #[tokio::test]
    async fn disconnect_removes_session() {
        let server = bind_server().await;
        let mut tunnel = connected_client(server.local_addr()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(server.client_count().await, 1);

        tunnel.disconnect().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(server.client_count().await, 0);
    }

    #[tokio::test]
    async fn multiple_clients_get_distinct_channel_ids() {
        let server = bind_server().await;
        let tunnel_a = connected_client(server.local_addr()).await;
        let tunnel_b = connected_client(server.local_addr()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_ne!(tunnel_a.channel_id(), tunnel_b.channel_id());
        assert_eq!(server.client_count().await, 2);
    }

    // --- TCP: same dispatch path, different acceptor ---

    async fn connected_tcp_client(server_addr: SocketAddr) -> Tunnel {
        let mut tunnel = Tunnel::new_tcp(server_addr);
        tunnel.connect().await.unwrap();
        tunnel
    }

    #[tokio::test]
    async fn tcp_connect_handshake_assigns_channel_id() {
        let server = bind_server().await;
        let tunnel = connected_tcp_client(server.local_addr()).await;
        assert!(tunnel.is_connected());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(server.client_count().await, 1);
    }

    #[tokio::test]
    async fn tcp_client_write_reaches_server_subscribe() {
        let server = bind_server().await;
        let tunnel = connected_tcp_client(server.local_addr()).await;
        let mut rx = server.subscribe();

        let frame = build_tunnelling_frame(
            &test_telegram(2),
            tunnel.channel_id(),
            tunnel.next_sequence(),
        )
        .unwrap();
        tunnel.send_frame(&frame).await.unwrap();

        let received = timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.payload, vec![2]);

        let confirmation = timeout(Duration::from_secs(1), tunnel.recv_frame())
            .await
            .unwrap()
            .unwrap();
        let frame = KnxIpFrame::parse(&confirmation).unwrap();
        assert_eq!(frame.header.service_type, ServiceType::TunnellingRequest);
        let request = TunnellingRequest::parse(&frame.body).unwrap();
        assert_eq!(
            CemiFrame::parse(&request.raw_cemi).unwrap().message_code,
            MessageCode::LDataCon
        );
    }

    #[tokio::test]
    async fn tcp_server_send_reaches_connected_client() {
        let server = bind_server().await;
        let tunnel = connected_tcp_client(server.local_addr()).await;
        let mut invalid = test_telegram(11);
        invalid.payload.resize(256, 11);
        assert!(matches!(
            server.send(invalid).await,
            Err(crate::error::KnxError::Protocol(
                crate::error::ProtocolError::InvalidFrame { .. }
            ))
        ));
        assert_eq!(server.client_count().await, 1);

        timeout(Duration::from_millis(500), server.send(test_telegram(11)))
            .await
            .unwrap()
            .unwrap();
        let data = timeout(Duration::from_secs(1), tunnel.recv_frame())
            .await
            .unwrap()
            .unwrap();
        let frame = KnxIpFrame::parse(&data).unwrap();
        assert_eq!(frame.header.service_type, ServiceType::TunnellingRequest);
        let request = TunnellingRequest::parse(&frame.body).unwrap();
        assert_eq!(
            parse_cemi_to_telegram(&request.raw_cemi).unwrap().payload,
            vec![11]
        );
    }

    #[tokio::test]
    async fn tcp_heartbeat_round_trip() {
        let server = bind_server().await;
        let tunnel = connected_tcp_client(server.local_addr()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        tunnel.send_connectionstate_request().await.unwrap();
        let data = timeout(Duration::from_secs(1), tunnel.recv_frame())
            .await
            .unwrap()
            .unwrap();
        let frame = KnxIpFrame::parse(&data).unwrap();
        assert_eq!(
            frame.header.service_type,
            ServiceType::ConnectionstateResponse
        );
        let resp = ConnectionstateResponse::parse(&frame.body).unwrap();
        assert!(resp.is_success());
    }

    #[tokio::test]
    async fn tcp_disconnect_removes_session() {
        let server = bind_server().await;
        let mut tunnel = connected_tcp_client(server.local_addr()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(server.client_count().await, 1);

        tunnel.disconnect().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(server.client_count().await, 0);
    }

    #[tokio::test]
    async fn device_management_server_serves_udp_and_tcp_clients() {
        use crate::protocol::cemi::CemiPropertyInfo;
        use crate::transport::DeviceManagementConnection;

        struct Handler;

        impl DeviceManagementHandler for Handler {
            fn read_property(
                &self,
                property: CemiPropertyInfo,
            ) -> std::result::Result<Vec<u8>, u8> {
                if property.property_id == 2 {
                    Ok(vec![0x42])
                } else {
                    Err(0x01)
                }
            }

            fn write_property(
                &self,
                property: CemiPropertyInfo,
                data: &[u8],
            ) -> std::result::Result<(), u8> {
                if property.property_id == 2 && data == [0x12, 0x34] {
                    Ok(())
                } else {
                    Err(0x01)
                }
            }
        }

        let server = bind_server().await;
        server
            .set_device_management_handler(Arc::new(Handler))
            .await;
        let gateway = server.local_addr();

        for tcp in [false, true] {
            let mut connection = if tcp {
                DeviceManagementConnection::connect_tcp(gateway, Duration::from_secs(2))
                    .await
                    .unwrap()
            } else {
                DeviceManagementConnection::connect(
                    gateway,
                    "127.0.0.1:0".parse().unwrap(),
                    Duration::from_secs(2),
                )
                .await
                .unwrap()
            };
            let property = CemiPropertyInfo {
                object_type: 0x0011,
                object_instance: 1,
                property_id: 2,
                number_of_elements: 1,
                start_index: 1,
            };
            timeout(Duration::from_millis(500), server.send(test_telegram(11)))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                connection
                    .read_property(property, Duration::from_secs(2))
                    .await
                    .unwrap(),
                [0x42]
            );
            connection
                .write_property(property, &[0x12, 0x34], Duration::from_secs(2))
                .await
                .unwrap();
            connection.disconnect(Duration::from_secs(2)).await.unwrap();
        }

        server.shutdown();
    }

    #[tokio::test]
    async fn tcp_stream_close_removes_session_without_explicit_disconnect() {
        let server = bind_server().await;
        let tunnel = connected_tcp_client(server.local_addr()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(server.client_count().await, 1);

        drop(tunnel); // closes the TCP stream without sending DisconnectRequest
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(server.client_count().await, 0);
    }

    #[tokio::test]
    async fn udp_and_tcp_clients_share_the_channel_id_pool() {
        let server = bind_server().await;
        let udp_tunnel = connected_client(server.local_addr()).await;
        let tcp_tunnel = connected_tcp_client(server.local_addr()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_ne!(udp_tunnel.channel_id(), tcp_tunnel.channel_id());
        assert_eq!(server.client_count().await, 2);
    }

    // --- KNX IP Secure: real Tunnel::connect_secure against a secure TunnelServer.
    //
    // `Tunnel::send_frame`/`recv_frame` already transparently encrypt/decrypt
    // once `connect_secure` succeeds, so the plaintext test helpers
    // (`build_tunnelling_frame`, `client_ack_next`) work unchanged here too.

    #[cfg(feature = "secure")]
    use crate::transport::SecurityConfig;

    #[cfg(feature = "secure")]
    fn matching_security_configs(
        device_auth_password: String,
        user_password: String,
    ) -> (SessionConfig, SecurityConfig) {
        let server_config = SessionConfig {
            user_id: 1,
            user_password: user_password.clone(),
            device_auth_password: Some(device_auth_password.clone()),
            keepalive_interval: 60,
        };
        let client_security = SecurityConfig::new(device_auth_password, user_password);
        (server_config, client_security)
    }

    #[cfg(feature = "secure")]
    #[tokio::test]
    async fn secure_client_connects_and_exchanges_telegrams_both_ways() {
        let (server_config, client_security) = matching_security_configs(
            crate::security::generate_password(),
            crate::security::generate_password(),
        );

        let server = TunnelServer::bind_secure(
            "127.0.0.1:0".parse().unwrap(),
            Ia::new(1, 1, 240),
            server_config,
        )
        .await
        .unwrap();

        let mut tunnel = Tunnel::new_udp(server.local_addr());
        tunnel.connect_secure(&client_security).await.unwrap();
        assert!(tunnel.is_connected());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(server.client_count().await, 1);

        // Client -> server.
        let mut rx = server.subscribe();
        let frame = build_tunnelling_frame(
            &test_telegram(5),
            tunnel.channel_id(),
            tunnel.next_sequence(),
        )
        .unwrap();
        tunnel.send_frame(&frame).await.unwrap();
        let received = timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.payload, vec![5]);
        // Drain the server's (encrypted) ack.
        let ack_data = timeout(Duration::from_secs(1), tunnel.recv_frame())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            KnxIpFrame::parse(&ack_data).unwrap().header.service_type,
            ServiceType::TunnellingAck
        );
        // Acknowledge the cEMI confirmation for the write before reading broadcasts.
        client_ack_next(&tunnel).await;

        // Server -> client.
        let (send_result, ()) =
            tokio::join!(server.send(test_telegram(6)), client_ack_next(&tunnel));
        send_result.unwrap();

        // A UDP retry keeps the tunnel sequence, with a fresh secure wrapper.
        let (retry_result, ()) = tokio::join!(server.send(test_telegram(7)), async {
            let first = timeout(Duration::from_secs(2), tunnel.recv_frame())
                .await
                .unwrap()
                .unwrap();
            let retry = timeout(Duration::from_secs(2), tunnel.recv_frame())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retry, first);
            let frame = KnxIpFrame::parse(&retry).unwrap();
            let request = TunnellingRequest::parse(&frame.body).unwrap();
            tunnel
                .send_tunnelling_ack(
                    request.communication_channel_id,
                    request.sequence_counter,
                    TunnellingAck::STATUS_OK,
                )
                .await
                .unwrap();
        });
        retry_result.unwrap();
        assert_eq!(server.client_count().await, 1);

        let received_before_disconnect = tunnel.stats().frames_received;
        tunnel.send_disconnect().await.unwrap();
        assert_eq!(server.client_count().await, 0);
        assert!(tunnel.stats().frames_received > received_before_disconnect);
    }

    #[cfg(feature = "secure")]
    #[tokio::test]
    async fn secure_connect_rejects_wrong_user_password() {
        let (server_config, _) = matching_security_configs(
            crate::security::generate_password(),
            crate::security::generate_password(),
        );
        let client_security = SecurityConfig::new(
            server_config.device_auth_password.clone().unwrap(),
            crate::security::generate_password(),
        );

        let server = TunnelServer::bind_secure(
            "127.0.0.1:0".parse().unwrap(),
            Ia::new(1, 1, 240),
            server_config,
        )
        .await
        .unwrap();

        let mut tunnel = Tunnel::new_udp(server.local_addr());
        let result = tunnel.connect_secure(&client_security).await;
        assert!(matches!(
            result,
            Err(crate::error::KnxError::Security(
                crate::error::SecurityError::AuthenticationFailed { .. }
            ))
        ));
    }

    #[cfg(feature = "secure")]
    #[tokio::test]
    async fn plaintext_connect_rejected_when_secure_required() {
        let (server_config, _) = matching_security_configs(
            crate::security::generate_password(),
            crate::security::generate_password(),
        );
        let server = TunnelServer::bind_secure(
            "127.0.0.1:0".parse().unwrap(),
            Ia::new(1, 1, 240),
            server_config,
        )
        .await
        .unwrap();

        let mut tunnel = Tunnel::new_udp(server.local_addr());
        let result = timeout(Duration::from_millis(500), tunnel.connect()).await;
        // Either a timeout (server silently drops the plaintext ConnectRequest)
        // or an explicit error is acceptable — what matters is it never succeeds.
        // Err(_) means it timed out waiting for a ConnectResponse that will never come.
        if let Ok(inner) = result {
            assert!(
                inner.is_err(),
                "plaintext connect must not succeed against a secure-required endpoint"
            );
        }
        assert_eq!(server.client_count().await, 0);
    }

    /// Regression test for an authentication-bypass finding: the ECDH key
    /// exchange (SessionRequest/Response) alone derives a fully usable
    /// session key — it proves nothing about the client, since key
    /// *agreement* needs no shared secret. Only a verified
    /// `SessionAuthenticate` proves the client knows the user password. A
    /// client that completes the exchange and then skips `SessionAuthenticate`
    /// entirely must not be able to smuggle tunnel frames through by
    /// wrapping them in `SecureWrapper` anyway.
    #[cfg(feature = "secure")]
    #[tokio::test]
    async fn secure_wrapper_rejected_before_authentication_completes() {
        let (server_config, _) = matching_security_configs(
            crate::security::generate_password(),
            crate::security::generate_password(),
        );
        let server = TunnelServer::bind_secure(
            "127.0.0.1:0".parse().unwrap(),
            Ia::new(1, 1, 240),
            server_config,
        )
        .await
        .unwrap();

        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.connect(server.local_addr()).await.unwrap();

        // Attacker: no device-auth password configured, so it never even
        // tries to verify the server's SessionResponse MAC — it just wants
        // the session key.
        let attacker_config = SessionConfig {
            user_id: 1,
            user_password: crate::security::generate_password(),
            device_auth_password: None,
            keepalive_interval: 60,
        };
        let mut attacker_session = SecureSession::new(&attacker_config);
        let attacker_pub: [u8; 32] = attacker_session.initialize().await.try_into().unwrap();

        let request = SessionRequest {
            control_endpoint: Hpai::new(socket.local_addr().unwrap()),
            public_key: attacker_pub,
        };
        let frame = KnxIpFrame::new(ServiceType::SessionRequest, request.serialize()).serialize();
        socket.send(&frame).await.unwrap();

        let mut buf = [0u8; 1024];
        let n = timeout(Duration::from_secs(1), socket.recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let response_frame = KnxIpFrame::parse(&buf[..n]).unwrap();
        assert_eq!(
            response_frame.header.service_type,
            ServiceType::SessionResponse
        );
        let response = SessionResponse::parse(&response_frame.body).unwrap();

        // Derives the session key; skips MAC verification (no device-auth
        // password configured on the attacker's side) rather than failing.
        let _auth_mac = attacker_session
            .process_session_response(response.session_id, &response.public_key, &response.mac)
            .await
            .unwrap();

        // Never sends SessionAuthenticate. Tries to sneak a ConnectRequest
        // through anyway, now that it has a working session key.
        let connect_request = ConnectRequest::new_route_back();
        let inner =
            KnxIpFrame::new(ServiceType::ConnectRequest, connect_request.serialize()).serialize();
        let wrapped = attacker_session.encrypt_frame(&inner).await.unwrap();
        socket.send(&wrapped).await.unwrap();

        let result = timeout(Duration::from_millis(300), socket.recv(&mut buf)).await;
        assert!(
            result.is_err(),
            "server must not process a SecureWrapper frame before authentication completes"
        );
        assert_eq!(server.client_count().await, 0);
    }
}
