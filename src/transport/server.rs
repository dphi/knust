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
use std::net::SocketAddr;
use std::sync::Arc;
#[cfg(feature = "secure")]
use std::sync::atomic::AtomicU16;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{RwLock, broadcast, oneshot};

use crate::error::{Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::GroupValueService;
use crate::protocol::address::IndividualAddress;
use crate::protocol::cemi::{CemiFrame, MessageCode};
use crate::protocol::knxip::{
    ConnectRequest, ConnectResponse, ConnectionRequestInfo, ConnectionstateRequest,
    ConnectionstateResponse, DisconnectRequest, DisconnectResponse, Hpai, KnxIpFrame, ServiceType,
    TunnellingAck, TunnellingRequest,
};
#[cfg(feature = "secure")]
use crate::protocol::knxip::{SessionAuthenticate, SessionRequest, SessionResponse, SessionStatus};
use crate::protocol::telegram::{Direction, Priority, Telegram, TelegramType};
#[cfg(feature = "secure")]
use crate::security::{SecureSession, SessionConfig};

use super::frame_transport::{FrameTransport, TcpFrameTransport};
use super::tunnel::SequenceValidationResult;

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
        socket: Arc<UdpSocket>,
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
}

/// Per-client session state: sequence counters, heartbeat, and the one
/// outstanding-ack correlator (only one send is ever in flight per client at
/// a time, matching the spec's request/ack turn-taking).
struct ClientSession {
    channel_id: u8,
    link: ClientLink,
    send_seq: AtomicU8,
    recv_seq: AtomicU8,
    last_heartbeat: std::sync::Mutex<Instant>,
    send_lock: tokio::sync::Mutex<()>,
    pending_ack: std::sync::Mutex<Option<oneshot::Sender<TunnellingAck>>>,
}

impl ClientSession {
    fn new(channel_id: u8, link: ClientLink) -> Self {
        Self {
            channel_id,
            link,
            send_seq: AtomicU8::new(0),
            recv_seq: AtomicU8::new(0),
            last_heartbeat: std::sync::Mutex::new(Instant::now()),
            send_lock: tokio::sync::Mutex::new(()),
            pending_ack: std::sync::Mutex::new(None),
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

/// A pending or established KNX IP Secure session for one peer, plus the
/// client's ECDH public key (needed again to verify `SessionAuthenticate`).
#[cfg(feature = "secure")]
struct SecureSessionEntry {
    session: tokio::sync::Mutex<SecureSession>,
    client_public_key: [u8; 32],
}

/// A KNXnet/IP tunneling server bound to one UDP socket and one TCP listener
/// on the same address.
pub struct TunnelServer {
    socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    individual_address: IndividualAddress,
    sessions: RwLock<HashMap<u8, Arc<ClientSession>>>,
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
    secure_sessions: RwLock<HashMap<SocketAddr, Arc<SecureSessionEntry>>>,
    #[cfg(feature = "secure")]
    next_secure_session_id: AtomicU16,
    telegram_tx: broadcast::Sender<Telegram>,
    shutdown: AtomicBool,
    shutdown_notify: Arc<tokio::sync::Notify>,
    dispatch_handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    tcp_accept_handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    sweep_handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
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

    /// Bind a tunneling server that requires KNX IP Secure: clients must
    /// complete the SessionRequest/Response/Authenticate/Status handshake
    /// (using the device-auth/user credentials in `security`) before any
    /// tunneling frame is accepted; a plain (unwrapped) `ConnectRequest` is
    /// rejected.
    // TODO: only tested against this crate's own `Tunnel::connect_secure` and
    // (indirectly, via the tunnel.rs cross-check) a known-good reference
    // client's wire behavior — no independent secure *client* implementation
    // has ever completed a real handshake against this server. Low risk
    // (same handshake code paths as the client side, which is validated), but
    // worth noting.
    /// # Errors
    ///
    /// Returns [`TransportError::SocketError`] if binding the UDP or TCP
    /// listener on `addr` fails.
    #[cfg(feature = "secure")]
    pub async fn bind_secure(
        addr: SocketAddr,
        individual_address: IndividualAddress,
        security: SessionConfig,
    ) -> Result<Arc<Self>> {
        Self::bind_with_security(addr, individual_address, Some(security)).await
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

        let server = Arc::new(Self {
            socket: Arc::new(socket),
            local_addr,
            individual_address,
            sessions: RwLock::new(HashMap::new()),
            #[cfg(feature = "secure")]
            secure_config,
            #[cfg(feature = "secure")]
            secure_sessions: RwLock::new(HashMap::new()),
            #[cfg(feature = "secure")]
            next_secure_session_id: AtomicU16::new(1),
            telegram_tx,
            shutdown: AtomicBool::new(false),
            shutdown_notify: Arc::new(tokio::sync::Notify::new()),
            dispatch_handle: std::sync::Mutex::new(None),
            tcp_accept_handle: std::sync::Mutex::new(None),
            sweep_handle: std::sync::Mutex::new(None),
        });
        server.clone().spawn_dispatch_loop();
        server.clone().spawn_tcp_accept_loop(tcp_listener);
        server.clone().spawn_sweep_loop();
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

    fn spawn_dispatch_loop(self: Arc<Self>) {
        let server = self.clone();
        let handle = tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            loop {
                if server.is_shutdown() {
                    break;
                }
                let (len, peer_addr) = tokio::select! {
                    result = server.socket.recv_from(&mut buf) => match result {
                        Ok(v) => v,
                        Err(e) => {
                            log_transport!(LogLevel::Warn, "TunnelServer: recv error: {}", e);
                            continue;
                        }
                    },
                    () = server.shutdown_notify.notified() => break,
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
        });
        *self.dispatch_handle.lock().unwrap() = Some(handle);
    }

    /// Accept TCP tunneling connections; each accepted stream gets its own
    /// task doing its own re-framed `recv_frame()` loop, feeding the same
    /// `handle_frame` dispatch the UDP loop uses.
    fn spawn_tcp_accept_loop(self: Arc<Self>, listener: TcpListener) {
        let server = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                if server.is_shutdown() {
                    break;
                }
                let (stream, peer_addr) = tokio::select! {
                    result = listener.accept() => match result {
                        Ok(v) => v,
                        Err(e) => {
                            log_transport!(LogLevel::Warn, "TunnelServer: tcp accept error: {}", e);
                            continue;
                        }
                    },
                    () = server.shutdown_notify.notified() => break,
                };
                let server = server.clone();
                tokio::spawn(async move {
                    let transport = Arc::new(TcpFrameTransport::from_accepted_stream(stream));
                    let link = ClientLink::Tcp {
                        transport: transport.clone(),
                        peer_addr,
                    };
                    let mut assigned_channel: Option<u8> = None;
                    loop {
                        if server.is_shutdown() {
                            break;
                        }
                        match transport.recv_frame().await {
                            Ok(data) => match server.handle_frame(&data, link.clone()).await {
                                Ok(Some(id)) => assigned_channel = Some(id),
                                Ok(None) => {}
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
                    if let Some(id) = assigned_channel {
                        server.sessions.write().await.remove(&id);
                        log_transport!(
                            LogLevel::Info,
                            "TunnelServer: tcp client {} disconnected (channel {})",
                            peer_addr,
                            id
                        );
                    }
                    #[cfg(feature = "secure")]
                    server.secure_sessions.write().await.remove(&peer_addr);
                });
            }
        });
        *self.tcp_accept_handle.lock().unwrap() = Some(handle);
    }

    fn spawn_sweep_loop(self: Arc<Self>) {
        let server = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(SWEEP_INTERVAL) => {}
                    () = server.shutdown_notify.notified() => break,
                }
                if server.is_shutdown() {
                    break;
                }
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
        *self.sweep_handle.lock().unwrap() = Some(handle);
    }

    /// Dispatch one received frame. Returns `Some(channel_id)` when this
    /// frame was a `ConnectRequest` that just assigned a new channel — the
    /// TCP accept loop uses this to know which channel to clean up when its
    /// stream closes.
    async fn handle_frame(&self, data: &[u8], link: ClientLink) -> Result<Option<u8>> {
        let frame = KnxIpFrame::parse(data)?;
        match frame.header.service_type {
            #[cfg(feature = "secure")]
            ServiceType::SessionRequest => {
                self.handle_session_request(&frame.body, link).await?;
                Ok(None)
            }
            #[cfg(feature = "secure")]
            ServiceType::SessionAuthenticate => {
                self.handle_session_authenticate(&frame.body, link).await?;
                Ok(None)
            }
            #[cfg(feature = "secure")]
            ServiceType::SecureWrapper => {
                let peer = link.peer_addr();
                let entry = self.secure_sessions.read().await.get(&peer).cloned();
                let Some(entry) = entry else {
                    log_transport!(
                        LogLevel::Warn,
                        "TunnelServer: SecureWrapper from {} with no established secure session",
                        peer
                    );
                    return Ok(None);
                };
                // The session key is already usable for encrypt/decrypt as
                // soon as the ECDH exchange completes (SessionRequest/
                // Response) — that alone proves nothing about the client,
                // since key *agreement* needs no shared secret. Only a
                // successful SessionAuthenticate (verified MAC over the user
                // password) proves the client is who it claims to be, so
                // frames must be rejected until that's actually happened.
                if !entry.session.lock().await.is_authenticated().await {
                    log_transport!(
                        LogLevel::Warn,
                        "TunnelServer: SecureWrapper from {} rejected — session not yet authenticated",
                        peer
                    );
                    return Ok(None);
                }
                let decrypted = entry.session.lock().await.decrypt_frame(data).await?;
                let inner = KnxIpFrame::parse(&decrypted)?;
                self.dispatch_plain_frame(inner, link, true).await
            }
            _ => self.dispatch_plain_frame(frame, link, false).await,
        }
    }

    /// Dispatch a frame that's either genuinely plaintext (`was_encrypted =
    /// false`) or the inner frame unwrapped from a `SecureWrapper`
    /// (`was_encrypted = true`). Rejects plaintext when this endpoint
    /// requires secure — matches a real secure gateway not falling back to
    /// plaintext.
    async fn dispatch_plain_frame(
        &self,
        frame: KnxIpFrame,
        link: ClientLink,
        #[cfg_attr(not(feature = "secure"), allow(unused_variables))] was_encrypted: bool,
    ) -> Result<Option<u8>> {
        #[cfg(feature = "secure")]
        if self.secure_config.is_some() && !was_encrypted {
            log_transport!(
                LogLevel::Warn,
                "TunnelServer: rejecting plaintext {:?} from {} (secure required)",
                frame.header.service_type,
                link.peer_addr()
            );
            return Ok(None);
        }
        match frame.header.service_type {
            ServiceType::ConnectRequest => self.handle_connect_request(&frame.body, link).await,
            ServiceType::TunnellingRequest => {
                self.handle_tunnelling_request(&frame.body, link).await?;
                Ok(None)
            }
            ServiceType::TunnellingAck => {
                self.handle_tunnelling_ack(&frame.body).await?;
                Ok(None)
            }
            ServiceType::ConnectionstateRequest => {
                self.handle_connectionstate_request(&frame.body, link)
                    .await?;
                Ok(None)
            }
            ServiceType::DisconnectRequest => {
                self.handle_disconnect_request(&frame.body, link).await?;
                Ok(None)
            }
            other => {
                log_transport!(
                    LogLevel::Trace,
                    "TunnelServer: unhandled frame {:?} from {}",
                    other,
                    link.peer_addr()
                );
                Ok(None)
            }
        }
    }

    #[cfg(feature = "secure")]
    async fn handle_session_request(&self, body: &[u8], link: ClientLink) -> Result<()> {
        let Some(security) = &self.secure_config else {
            log_transport!(
                LogLevel::Warn,
                "TunnelServer: SessionRequest from {} but endpoint is not secure-configured",
                link.peer_addr()
            );
            return Ok(());
        };
        let request = SessionRequest::parse(body)?;

        let mut session = SecureSession::new(security);
        let server_public_key: [u8; 32] = session.initialize().await.try_into().map_err(|_| {
            TransportError::InvalidConfiguration {
                details: "ECDH public key was not 32 bytes".to_string(),
            }
        })?;
        let session_id = self.next_secure_session_id.fetch_add(1, Ordering::SeqCst);
        let mac_vec = session
            .process_session_request(&request.public_key, session_id)
            .await?;
        let mut mac = [0u8; 16];
        mac.copy_from_slice(&mac_vec);

        let response = SessionResponse {
            session_id,
            public_key: server_public_key,
            mac,
        };
        link.send_frame(
            &KnxIpFrame::new(ServiceType::SessionResponse, response.serialize()).serialize(),
        )
        .await?;

        self.secure_sessions.write().await.insert(
            link.peer_addr(),
            Arc::new(SecureSessionEntry {
                session: tokio::sync::Mutex::new(session),
                client_public_key: request.public_key,
            }),
        );
        Ok(())
    }

    #[cfg(feature = "secure")]
    async fn handle_session_authenticate(&self, body: &[u8], link: ClientLink) -> Result<()> {
        let auth = SessionAuthenticate::parse(body)?;
        let peer = link.peer_addr();
        let entry = self.secure_sessions.read().await.get(&peer).cloned();
        let Some(entry) = entry else {
            log_transport!(
                LogLevel::Warn,
                "TunnelServer: SessionAuthenticate from {} with no pending secure session",
                peer
            );
            return Ok(());
        };

        let ok = entry
            .session
            .lock()
            .await
            .verify_authenticate_mac(&entry.client_public_key, auth.user_id, &auth.mac)
            .await?;

        let status = if ok {
            SessionStatus::STATUS_OK
        } else {
            SessionStatus::STATUS_AUTH_FAILED
        };
        let response = SessionStatus { status };
        link.send_frame(
            &KnxIpFrame::new(ServiceType::SessionStatus, response.serialize()).serialize(),
        )
        .await?;

        if ok {
            log_transport!(
                LogLevel::Info,
                "TunnelServer: secure session established with {}",
                peer
            );
        } else {
            log_transport!(
                LogLevel::Warn,
                "TunnelServer: secure authentication failed for {}",
                peer
            );
            self.secure_sessions.write().await.remove(&peer);
        }
        Ok(())
    }

    /// Encrypt `frame` for `peer` if it has an established secure session,
    /// otherwise return it unchanged.
    #[cfg(feature = "secure")]
    async fn maybe_encrypt(&self, frame: Vec<u8>, peer: SocketAddr) -> Result<Vec<u8>> {
        let entry = self.secure_sessions.read().await.get(&peer).cloned();
        match entry {
            Some(entry) => entry.session.lock().await.encrypt_frame(&frame).await,
            None => Ok(frame),
        }
    }

    /// Without the `secure` feature there are no sessions to encrypt for.
    #[cfg(not(feature = "secure"))]
    async fn maybe_encrypt(&self, frame: Vec<u8>, _peer: SocketAddr) -> Result<Vec<u8>> {
        Ok(frame)
    }

    /// Route-back (`0.0.0.0:0` / port 0) means "reply to the packet's source
    /// address"; otherwise use the address the client advertised. Only
    /// meaningful for UDP — a TCP link always replies on the same stream
    /// regardless of what HPAI the client declared.
    fn resolve_link(hpai: &Hpai, link: &ClientLink) -> ClientLink {
        match link {
            ClientLink::Udp { socket, peer_addr } => {
                let resolved = if hpai.port == 0 {
                    *peer_addr
                } else {
                    hpai.socket_addr()
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

    async fn handle_connect_request(&self, body: &[u8], link: ClientLink) -> Result<Option<u8>> {
        let request = ConnectRequest::parse(body)?;
        let link = Self::resolve_link(&request.data_endpoint, &link);

        let channel_id = {
            let mut sessions = self.sessions.write().await;
            if let Some(id) = Self::allocate_channel_id(&sessions) {
                sessions.insert(id, Arc::new(ClientSession::new(id, link.clone())));
                id
            } else {
                let response = ConnectResponse {
                    channel_id: 0,
                    status: ConnectResponse::STATUS_ERROR_NO_MORE_CONNECTIONS,
                    data_endpoint: Hpai::new(self.local_addr),
                    crd: Vec::new(),
                    assigned_address: None,
                };
                self.send_reply(ServiceType::ConnectResponse, &response.serialize(), &link)
                    .await?;
                return Ok(None);
            }
        };

        let response = ConnectResponse {
            channel_id,
            status: ConnectResponse::STATUS_OK,
            data_endpoint: Hpai::new(self.local_addr),
            crd: build_tunnel_crd(self.individual_address),
            assigned_address: Some(self.individual_address),
        };
        self.send_reply(ServiceType::ConnectResponse, &response.serialize(), &link)
            .await?;
        log_transport!(
            LogLevel::Info,
            "TunnelServer: client {} connected, channel_id={}",
            link.peer_addr(),
            channel_id
        );
        Ok(Some(channel_id))
    }

    async fn handle_tunnelling_request(&self, body: &[u8], link: ClientLink) -> Result<()> {
        let request = TunnellingRequest::parse(body)?;
        let session = self
            .sessions
            .read()
            .await
            .get(&request.communication_channel_id)
            .cloned();
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
                self.send_reply(ServiceType::TunnellingAck, &ack.serialize(), &session.link)
                    .await?;
                match parse_cemi_to_telegram(&request.raw_cemi) {
                    Ok(telegram) => {
                        let _ = self.telegram_tx.send(telegram);
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
                self.send_reply(ServiceType::TunnellingAck, &ack.serialize(), &session.link)
                    .await?;
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
                self.send_reply(ServiceType::TunnellingAck, &ack.serialize(), &session.link)
                    .await?;
            }
        }
        Ok(())
    }

    async fn handle_tunnelling_ack(&self, body: &[u8]) -> Result<()> {
        let ack = TunnellingAck::parse(body)?;
        let session = self
            .sessions
            .read()
            .await
            .get(&ack.communication_channel_id)
            .cloned();
        if let Some(session) = session
            && let Some(tx) = session.pending_ack.lock().unwrap().take()
        {
            let _ = tx.send(ack);
        }
        Ok(())
    }

    async fn handle_connectionstate_request(&self, body: &[u8], link: ClientLink) -> Result<()> {
        let request = ConnectionstateRequest::parse(body)?;
        let (status, reply_link) = {
            let sessions = self.sessions.read().await;
            match sessions.get(&request.communication_channel_id) {
                Some(session) => {
                    session.touch();
                    (ConnectionstateResponse::STATUS_OK, session.link.clone())
                }
                None => (ConnectResponse::STATUS_ERROR_CONNECTION_ID, link),
            }
        };
        let response = ConnectionstateResponse::new(request.communication_channel_id, status);
        self.send_reply(
            ServiceType::ConnectionstateResponse,
            &response.serialize(),
            &reply_link,
        )
        .await
    }

    async fn handle_disconnect_request(&self, body: &[u8], link: ClientLink) -> Result<()> {
        let request = DisconnectRequest::parse(body)?;
        let reply_link = self
            .sessions
            .write()
            .await
            .remove(&request.communication_channel_id)
            .map_or(link, |s| s.link.clone());
        #[cfg(feature = "secure")]
        self.secure_sessions
            .write()
            .await
            .remove(&reply_link.peer_addr());
        log_transport!(
            LogLevel::Info,
            "TunnelServer: client channel {} disconnected",
            request.communication_channel_id
        );
        let response = DisconnectResponse::new(
            request.communication_channel_id,
            DisconnectResponse::STATUS_OK,
        );
        self.send_reply(
            ServiceType::DisconnectResponse,
            &response.serialize(),
            &reply_link,
        )
        .await
    }

    async fn send_reply(
        &self,
        service_type: ServiceType,
        body: &[u8],
        link: &ClientLink,
    ) -> Result<()> {
        let frame = KnxIpFrame::new(service_type, body.to_vec()).serialize();
        let frame = self.maybe_encrypt(frame, link.peer_addr()).await?;
        link.send_frame(&frame).await
    }

    /// Send one telegram to one client session, retrying once on ack
    /// timeout. Only bumps the session's outbound sequence counter after a
    /// successful ack, per spec (a timed-out send is retried with the same
    /// sequence number, not the next one).
    async fn send_to_session(
        &self,
        session: &Arc<ClientSession>,
        telegram: &Telegram,
    ) -> Result<()> {
        let _guard = session.send_lock.lock().await;
        let seq = session.send_seq.load(Ordering::SeqCst);
        let frame = build_tunnelling_frame_with_code(
            telegram,
            session.channel_id,
            seq,
            MessageCode::LDataInd,
        );
        let frame = self.maybe_encrypt(frame, session.link.peer_addr()).await?;

        for attempt in 0..2u8 {
            let (tx, rx) = oneshot::channel();
            *session.pending_ack.lock().unwrap() = Some(tx);
            session.link.send_frame(&frame).await?;

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
    pub async fn send(&self, telegram: Telegram) -> Result<()> {
        let sessions: Vec<Arc<ClientSession>> =
            self.sessions.read().await.values().cloned().collect();
        for session in &sessions {
            if let Err(e) = self.send_to_session(session, &telegram).await {
                log_transport!(
                    LogLevel::Warn,
                    "TunnelServer: send to {} failed: {}",
                    session.link.peer_addr(),
                    e
                );
                self.sessions.write().await.remove(&session.channel_id);
            }
        }
        Ok(())
    }

    /// Subscribe to telegrams received from connected tunnelling clients.
    pub fn subscribe(&self) -> broadcast::Receiver<Telegram> {
        self.telegram_tx.subscribe()
    }

    /// Signal the server and all of its connection loops to shut down.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.shutdown_notify.notify_waiters();
        log_transport!(LogLevel::Info, "TunnelServer shutdown requested");
    }

    /// Return whether shutdown has been requested.
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
}

fn build_tunnelling_frame_with_code(
    telegram: &Telegram,
    channel_id: u8,
    sequence: u8,
    message_code: MessageCode,
) -> Vec<u8> {
    let group_service = if telegram.payload.is_empty() {
        GroupValueService::Read
    } else {
        GroupValueService::Write(telegram.payload.clone())
    };
    let cemi = CemiFrame::new(
        message_code,
        telegram.source,
        telegram.destination,
        group_service.encode(),
    );
    let request = TunnellingRequest::new(channel_id, sequence, cemi.serialize());
    KnxIpFrame::new(ServiceType::TunnellingRequest, request.serialize()).serialize()
}

#[cfg(test)]
fn build_tunnelling_frame(telegram: &Telegram, channel_id: u8, sequence: u8) -> Vec<u8> {
    build_tunnelling_frame_with_code(telegram, channel_id, sequence, MessageCode::LDataReq)
}

/// Build the Connection Response Data for a tunnel connection CRD:
/// `[struct_len=4, TUNNEL_CONNECTION, addr_high, addr_low]`.
fn build_tunnel_crd(addr: IndividualAddress) -> Vec<u8> {
    let [hi, lo] = addr.raw().to_be_bytes();
    vec![4, ConnectionRequestInfo::TUNNEL_CONNECTION, hi, lo]
}

/// Decode a raw CEMI frame (as carried by a `TunnellingRequest`) into a
/// `Telegram`. Mirrors the equivalent client-side decode used when a `Tunnel`
/// receives a frame from a real gateway.
fn parse_cemi_to_telegram(cemi_data: &[u8]) -> Result<Telegram> {
    let cemi_frame = CemiFrame::parse(cemi_data)?;
    let service = GroupValueService::decode(cemi_frame.tpci, &cemi_frame.apci_data).ok();
    let payload = service
        .as_ref()
        .and_then(|s| s.payload().map(<[u8]>::to_vec))
        .unwrap_or_else(|| cemi_frame.apci_data.clone());
    let telegram_type = match &service {
        Some(GroupValueService::Read) => TelegramType::GroupValueRead,
        Some(GroupValueService::Response(_)) => TelegramType::GroupValueResponse,
        Some(GroupValueService::Write(_)) | None => TelegramType::GroupValueWrite,
    };

    Ok(Telegram {
        source: cemi_frame.source_addr,
        destination: cemi_frame.dest_addr,
        payload,
        priority: match cemi_frame.control_field.priority {
            crate::protocol::cemi::Priority::System => Priority::System,
            crate::protocol::cemi::Priority::Normal => Priority::Normal,
            crate::protocol::cemi::Priority::Urgent => Priority::Urgent,
            crate::protocol::cemi::Priority::Low => Priority::Low,
        },
        direction: Direction::Incoming,
        telegram_type,
        gateway_id: None,
        timestamp: std::time::SystemTime::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::address::{Address, GroupAddress, IndividualAddress as Ia};
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

    fn test_telegram(payload: u8) -> Telegram {
        Telegram::new_incoming(
            Ia::new(1, 1, 5),
            Address::Group(GroupAddress::from_parts(1, 2, 3).unwrap()),
            vec![payload],
        )
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
        );
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
        );
        tunnel.send_frame(&frame).await.unwrap();

        let received = timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.payload, vec![2]);

        let ack_data = timeout(Duration::from_secs(1), tunnel.recv_frame())
            .await
            .unwrap()
            .unwrap();
        let ack_frame = KnxIpFrame::parse(&ack_data).unwrap();
        assert_eq!(ack_frame.header.service_type, ServiceType::TunnellingAck);
    }

    #[tokio::test]
    async fn tcp_server_send_reaches_connected_client() {
        let server = bind_server().await;
        let tunnel = connected_tcp_client(server.local_addr()).await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        let (send_result, ()) =
            tokio::join!(server.send(test_telegram(11)), client_ack_next(&tunnel));
        send_result.unwrap();
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
        let client_security = SecurityConfig {
            device_auth_password,
            user_password: Some(user_password),
            keyring_path: None,
            session_timeout: 60,
        };
        (server_config, client_security)
    }

    #[cfg(feature = "secure")]
    #[tokio::test]
    async fn secure_client_connects_and_exchanges_telegrams_both_ways() {
        let (server_config, client_security) = matching_security_configs(
            "device-auth-raw-bytes".to_string(),
            "user-secret".to_string(),
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
        );
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

        // Server -> client.
        let (send_result, ()) =
            tokio::join!(server.send(test_telegram(6)), client_ack_next(&tunnel));
        send_result.unwrap();
    }

    #[cfg(feature = "secure")]
    #[tokio::test]
    async fn secure_connect_rejects_wrong_user_password() {
        let (server_config, mut client_security) = matching_security_configs(
            "device-auth-raw-bytes".to_string(),
            "correct-password".to_string(),
        );
        client_security.user_password = Some("wrong-password".to_string());

        let server = TunnelServer::bind_secure(
            "127.0.0.1:0".parse().unwrap(),
            Ia::new(1, 1, 240),
            server_config,
        )
        .await
        .unwrap();

        let mut tunnel = Tunnel::new_udp(server.local_addr());
        let result = tunnel.connect_secure(&client_security).await;
        assert!(
            result.is_err(),
            "connect_secure must fail when the user password doesn't match"
        );
    }

    #[cfg(feature = "secure")]
    #[tokio::test]
    async fn plaintext_connect_rejected_when_secure_required() {
        let (server_config, _) = matching_security_configs(
            "device-auth-raw-bytes".to_string(),
            "user-secret".to_string(),
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
            "device-auth-raw-bytes".to_string(),
            "user-secret".to_string(),
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
            user_password: "irrelevant-never-sent".to_string(),
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
