//! KNXnet/IP device-management client connections.
//!
//! These connections access the server's own interface objects. They are
//! separate from [`super::Connection`], which carries telegrams to the KNX bus.

use std::collections::VecDeque;
use std::net::SocketAddr;
#[cfg(feature = "secure")]
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;

#[cfg(feature = "secure")]
use super::SecurityConfig;
use super::frame_transport::{FrameTransport, TcpFrameTransport};
use crate::error::{ProtocolError, Result, TransportError};
use crate::protocol::cemi::{CemiPropertyInfoIndication, MessageCode};
use crate::protocol::knxip::{
    ConnectRequest, ConnectResponse, ConnectionRequestInfo, ConnectionstateRequest,
    ConnectionstateResponse, DisconnectRequest, DisconnectResponse, Hpai, KnxIpFrame, ServiceType,
    TunnellingAck, TunnellingRequest,
};
#[cfg(feature = "secure")]
use crate::protocol::knxip::{SessionAuthenticate, SessionRequest, SessionResponse, SessionStatus};
#[cfg(feature = "secure")]
use crate::security::SecureSession;

/// A KNXnet/IP device-management connection over UDP or TCP, optionally using
/// IP Secure.
///
/// Use [`request_cemi`](Self::request_cemi) to send one cEMI request and await
/// its matching response. Requests on a connection are serialized.
pub struct DeviceManagementConnection {
    transport: ManagementTransport,
    gateway: SocketAddr,
    data_endpoint: SocketAddr,
    local_hpai: Hpai,
    channel_id: u8,
    send_sequence: u8,
    receive_sequence: u8,
    pending_indications: VecDeque<CemiPropertyInfoIndication>,
    closed: bool,
}

enum ManagementTransport {
    Udp(UdpSocket),
    Tcp(TcpFrameTransport),
    #[cfg(feature = "secure")]
    Secure(SecureManagementTransport),
}

#[cfg(feature = "secure")]
enum PlainManagementTransport {
    Udp(UdpSocket),
    Tcp(TcpFrameTransport),
}

#[cfg(feature = "secure")]
impl PlainManagementTransport {
    async fn send(&self, destination: SocketAddr, frame: &[u8]) -> Result<()> {
        match self {
            Self::Udp(socket) => send_udp_frame(socket, destination, frame).await,
            Self::Tcp(transport) => transport.send_frame(frame).await,
        }
    }

    async fn receive(&self) -> Result<(KnxIpFrame, Option<SocketAddr>)> {
        match self {
            Self::Udp(socket) => {
                let (frame, source) = receive_udp_frame(socket).await?;
                Ok((frame, Some(source)))
            }
            Self::Tcp(transport) => Ok((KnxIpFrame::parse(&transport.recv_frame().await?)?, None)),
        }
    }
}

#[cfg(feature = "secure")]
struct SecureManagementTransport {
    transport: Arc<PlainManagementTransport>,
    session: Arc<tokio::sync::RwLock<SecureSession>>,
    send_lock: Arc<tokio::sync::Mutex<()>>,
    last_activity: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
    activity_changed: Arc<tokio::sync::Notify>,
    keepalive_cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

#[cfg(feature = "secure")]
impl SecureManagementTransport {
    fn new(
        transport: PlainManagementTransport,
        session: SecureSession,
        destination: SocketAddr,
        interval: Duration,
    ) -> Self {
        let transport = Arc::new(transport);
        let session = Arc::new(tokio::sync::RwLock::new(session));
        let send_lock = Arc::new(tokio::sync::Mutex::new(()));
        let last_activity = Arc::new(tokio::sync::Mutex::new(tokio::time::Instant::now()));
        let activity_changed = Arc::new(tokio::sync::Notify::new());
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let task_transport = transport.clone();
        let task_session = session.clone();
        let task_send_lock = send_lock.clone();
        let task_activity = last_activity.clone();
        let task_changed = activity_changed.clone();
        let task_destination = destination;
        tokio::spawn(async move {
            let interval = interval.max(Duration::from_secs(1));
            loop {
                let deadline = *task_activity.lock().await + interval;
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {
                        let _guard = task_send_lock.lock().await;
                        if tokio::time::Instant::now() < *task_activity.lock().await + interval {
                            continue;
                        }
                        let keepalive = KnxIpFrame::new(
                            ServiceType::SessionStatus,
                            SessionStatus { status: SessionStatus::STATUS_KEEPALIVE }.serialize(),
                        )
                        .serialize();
                        let encrypted = match task_session.read().await.encrypt_frame(&keepalive).await {
                            Ok(frame) => frame,
                            Err(_) => break,
                        };
                        if task_transport.send(task_destination, &encrypted).await.is_err() {
                            break;
                        }
                        *task_activity.lock().await = tokio::time::Instant::now();
                        task_changed.notify_one();
                    }
                    _ = task_changed.notified() => {}
                    _ = &mut cancelled => break,
                }
            }
        });
        Self {
            transport,
            session,
            send_lock,
            last_activity,
            activity_changed,
            keepalive_cancel: Some(cancel),
        }
    }

    async fn send(&self, destination: SocketAddr, frame: &[u8]) -> Result<()> {
        let _guard = self.send_lock.lock().await;
        let encrypted = self.session.read().await.encrypt_frame(frame).await?;
        self.transport.send(destination, &encrypted).await?;
        *self.last_activity.lock().await = tokio::time::Instant::now();
        self.activity_changed.notify_one();
        Ok(())
    }

    async fn receive(&self) -> Result<(KnxIpFrame, Option<SocketAddr>)> {
        loop {
            let (frame, source) = self.transport.receive().await?;
            if frame.header.service_type != ServiceType::SecureWrapper {
                continue;
            }
            let plain = self
                .session
                .read()
                .await
                .decrypt_frame(&frame.serialize())
                .await?;
            let frame = KnxIpFrame::parse(&plain)?;
            *self.last_activity.lock().await = tokio::time::Instant::now();
            self.activity_changed.notify_one();
            return Ok((frame, source));
        }
    }
}

impl ManagementTransport {
    async fn send(&self, destination: SocketAddr, frame: &[u8]) -> Result<()> {
        match self {
            Self::Udp(socket) => send_udp_frame(socket, destination, frame).await,
            Self::Tcp(transport) => transport.send_frame(frame).await,
            #[cfg(feature = "secure")]
            Self::Secure(transport) => transport.send(destination, frame).await,
        }
    }

    async fn receive(&self) -> Result<(KnxIpFrame, Option<SocketAddr>)> {
        match self {
            Self::Udp(socket) => {
                let (frame, source) = receive_udp_frame(socket).await?;
                Ok((frame, Some(source)))
            }
            Self::Tcp(transport) => Ok((KnxIpFrame::parse(&transport.recv_frame().await?)?, None)),
            #[cfg(feature = "secure")]
            Self::Secure(transport) => transport.receive().await,
        }
    }

    fn stop_keepalive(&mut self) {
        #[cfg(feature = "secure")]
        if let Self::Secure(transport) = self {
            transport.keepalive_cancel.take();
        }
    }
}

#[cfg(feature = "secure")]
async fn secure_management_transport(
    transport: PlainManagementTransport,
    gateway: SocketAddr,
    local_hpai: Hpai,
    security: &SecurityConfig,
    timeout: Duration,
) -> Result<ManagementTransport> {
    let config = security.session_config();
    let mut session = SecureSession::new(&config);
    let public_key: [u8; 32] = session.initialize().await.try_into().map_err(|_| {
        TransportError::InvalidConfiguration {
            details: "ECDH public key was not 32 bytes".into(),
        }
    })?;
    let request = SessionRequest {
        control_endpoint: local_hpai,
        public_key,
    };
    transport
        .send(
            gateway,
            &KnxIpFrame::new(ServiceType::SessionRequest, request.serialize()).serialize(),
        )
        .await?;

    let response = loop {
        let (frame, source) = tokio::time::timeout(timeout, transport.receive())
            .await
            .map_err(|_| TransportError::Timeout {
                timeout_ms: timeout.as_millis() as u64,
            })??;
        if source.is_some_and(|source| source != gateway)
            || frame.header.service_type != ServiceType::SessionResponse
        {
            continue;
        }
        break SessionResponse::parse(&frame.body)?;
    };
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
        .send(gateway, &session.encrypt_frame(&frame).await?)
        .await?;

    let status = loop {
        let (frame, source) = tokio::time::timeout(timeout, transport.receive())
            .await
            .map_err(|_| TransportError::Timeout {
                timeout_ms: timeout.as_millis() as u64,
            })??;
        if source.is_some_and(|source| source != gateway)
            || frame.header.service_type != ServiceType::SecureWrapper
        {
            continue;
        }
        let plain = session.decrypt_frame(&frame.serialize()).await?;
        let frame = KnxIpFrame::parse(&plain)?;
        if frame.header.service_type != ServiceType::SessionStatus {
            continue;
        }
        break SessionStatus::parse(&frame.body)?;
    };
    session.complete_authentication(status.status).await?;
    Ok(ManagementTransport::Secure(SecureManagementTransport::new(
        transport,
        session,
        gateway,
        security.keepalive_interval,
    )))
}

impl DeviceManagementConnection {
    /// Open a UDP device-management connection.
    pub async fn connect(
        gateway: SocketAddr,
        local: SocketAddr,
        timeout: Duration,
    ) -> Result<Self> {
        let socket =
            UdpSocket::bind(local)
                .await
                .map_err(|source| TransportError::SocketError {
                    operation: "bind device-management UDP socket".into(),
                    source,
                })?;
        let bound_addr = socket
            .local_addr()
            .map_err(|source| TransportError::SocketError {
                operation: "read device-management UDP socket address".into(),
                source,
            })?;
        let local_hpai = if bound_addr.ip().is_unspecified() {
            Hpai::route_back()
        } else {
            Hpai::new(bound_addr)
        };
        Self::establish(
            ManagementTransport::Udp(socket),
            gateway,
            local_hpai.clone(),
            local_hpai.clone(),
            local_hpai,
            timeout,
        )
        .await
    }

    #[cfg(feature = "secure")]
    /// Open a UDP device-management connection over KNX IP Secure.
    pub async fn connect_secure(
        gateway: SocketAddr,
        local: SocketAddr,
        security: &SecurityConfig,
        timeout: Duration,
    ) -> Result<Self> {
        let socket =
            UdpSocket::bind(local)
                .await
                .map_err(|source| TransportError::SocketError {
                    operation: "bind secure device-management UDP socket".into(),
                    source,
                })?;
        let bound_addr = socket
            .local_addr()
            .map_err(|source| TransportError::SocketError {
                operation: "read secure device-management UDP socket address".into(),
                source,
            })?;
        let local_hpai = if bound_addr.ip().is_unspecified() {
            Hpai::route_back()
        } else {
            Hpai::new(bound_addr)
        };
        let transport = secure_management_transport(
            PlainManagementTransport::Udp(socket),
            gateway,
            local_hpai.clone(),
            security,
            timeout,
        )
        .await?;
        Self::establish(
            transport,
            gateway,
            local_hpai.clone(),
            local_hpai.clone(),
            local_hpai,
            timeout,
        )
        .await
    }

    /// Open a TCP device-management connection.
    pub async fn connect_tcp(gateway: SocketAddr, timeout: Duration) -> Result<Self> {
        let (transport, _) = TcpFrameTransport::connect(gateway, timeout).await?;
        let hpai = Hpai::new_tcp_route_back();
        Self::establish(
            ManagementTransport::Tcp(transport),
            gateway,
            hpai.clone(),
            hpai.clone(),
            hpai,
            timeout,
        )
        .await
    }

    #[cfg(feature = "secure")]
    /// Open a TCP device-management connection over KNX IP Secure.
    pub async fn connect_tcp_secure(
        gateway: SocketAddr,
        security: &SecurityConfig,
        timeout: Duration,
    ) -> Result<Self> {
        let (transport, _) = TcpFrameTransport::connect(gateway, timeout).await?;
        let local_hpai = Hpai::new_tcp_route_back();
        let transport = secure_management_transport(
            PlainManagementTransport::Tcp(transport),
            gateway,
            local_hpai.clone(),
            security,
            timeout,
        )
        .await?;
        Self::establish(
            transport,
            gateway,
            local_hpai.clone(),
            local_hpai.clone(),
            local_hpai,
            timeout,
        )
        .await
    }

    async fn establish(
        transport: ManagementTransport,
        gateway: SocketAddr,
        control_endpoint: Hpai,
        data_endpoint: Hpai,
        local_hpai: Hpai,
        timeout: Duration,
    ) -> Result<Self> {
        let connect = ConnectRequest {
            control_endpoint,
            data_endpoint,
            cri: ConnectionRequestInfo::new_device_management(),
        };
        let frame = KnxIpFrame::new(ServiceType::ConnectRequest, connect.serialize()).serialize();
        transport.send(gateway, &frame).await?;

        let response = loop {
            let (frame, source) = tokio::time::timeout(timeout, transport.receive())
                .await
                .map_err(|_| TransportError::Timeout {
                    timeout_ms: timeout.as_millis() as u64,
                })??;
            if source.is_some_and(|source| source != gateway)
                || frame.header.service_type != ServiceType::ConnectResponse
            {
                continue;
            }
            break ConnectResponse::parse(&frame.body)?;
        };
        if !response.is_success() {
            return Err(TransportError::ConnectionFailed {
                address: gateway.to_string(),
                source: std::io::Error::other(format!(
                    "device-management connection rejected with status 0x{:02X}",
                    response.status
                )),
            }
            .into());
        }
        let response_endpoint = if response.data_endpoint.port == 0
            || response.data_endpoint.ip_addr.is_unspecified()
        {
            gateway
        } else {
            response.data_endpoint.socket_addr()
        };
        let data_endpoint = if matches!(transport, ManagementTransport::Tcp(_)) {
            gateway
        } else {
            response_endpoint
        };

        Ok(Self {
            transport,
            gateway,
            data_endpoint,
            local_hpai,
            channel_id: response.channel_id,
            send_sequence: 0,
            receive_sequence: 0,
            pending_indications: VecDeque::new(),
            closed: false,
        })
    }

    /// Send a cEMI frame and wait for a response with the requested message code.
    ///
    /// The KNXnet/IP request is repeated up to three times if its device-configuration
    /// acknowledgement is lost. `timeout` bounds each acknowledgement and response wait.
    pub async fn request_cemi(
        &mut self,
        cemi: &[u8],
        expected_response: MessageCode,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        self.request_cemi_matching(
            cemi,
            expected_response,
            |response| response.first() == Some(&expected_response.to_u8()),
            timeout,
        )
        .await
    }

    /// Read a property of the KNXnet/IP server's own interface objects.
    pub async fn read_property(
        &mut self,
        property: crate::protocol::cemi::CemiPropertyInfo,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        use crate::protocol::cemi::{CemiPropertyReadRequest, CemiPropertyReadResponse};

        let request = CemiPropertyReadRequest { property }.encode()?;
        let response = self
            .request_cemi_matching(
                &request,
                MessageCode::MPropReadCon,
                |bytes| {
                    CemiPropertyReadResponse::decode(bytes)
                        .is_ok_and(|response| same_property(response.property, property))
                },
                timeout,
            )
            .await?;
        let response = CemiPropertyReadResponse::decode(&response)?;
        if let Some(error_code) = response.error_code {
            return Err(TransportError::InvalidConfiguration {
                details: format!("property read failed with cEMI error 0x{error_code:02X}"),
            }
            .into());
        }
        Ok(response.data)
    }

    /// Write a property of the KNXnet/IP server's own interface objects.
    pub async fn write_property(
        &mut self,
        property: crate::protocol::cemi::CemiPropertyInfo,
        data: &[u8],
        timeout: Duration,
    ) -> Result<()> {
        use crate::protocol::cemi::{CemiPropertyWriteRequest, CemiPropertyWriteResponse};

        let request = CemiPropertyWriteRequest {
            property,
            data: data.to_vec(),
        }
        .encode()?;
        let response = self
            .request_cemi_matching(
                &request,
                MessageCode::MPropWriteCon,
                |bytes| {
                    CemiPropertyWriteResponse::decode(bytes)
                        .is_ok_and(|response| same_property(response.property, property))
                },
                timeout,
            )
            .await?;
        let response = CemiPropertyWriteResponse::decode(&response)?;
        if let Some(error_code) = response.error_code {
            return Err(TransportError::InvalidConfiguration {
                details: format!("property write failed with cEMI error 0x{error_code:02X}"),
            }
            .into());
        }
        Ok(())
    }

    async fn request_cemi_matching<F>(
        &mut self,
        cemi: &[u8],
        expected_response: MessageCode,
        matches: F,
        timeout: Duration,
    ) -> Result<Vec<u8>>
    where
        F: Fn(&[u8]) -> bool,
    {
        if self.closed {
            return Err(TransportError::ConnectionClosed.into());
        }
        if cemi.is_empty() {
            return Err(ProtocolError::InvalidFrame {
                details: "Device-management request must contain a cEMI frame".into(),
            }
            .into());
        }

        let sequence = self.send_sequence;
        let request = TunnellingRequest::new(self.channel_id, sequence, cemi.to_vec());
        let frame = KnxIpFrame::new(ServiceType::DeviceConfigurationRequest, request.serialize())
            .serialize();
        let mut response = None;
        let mut acknowledged = false;

        for _ in 0..=3 {
            self.transport.send(self.data_endpoint, &frame).await?;
            let deadline = tokio::time::Instant::now() + timeout;
            loop {
                let event =
                    match tokio::time::timeout_at(deadline, self.next_event(expected_response))
                        .await
                    {
                        Ok(result) => result?,
                        Err(_) => break,
                    };
                match event {
                    DeviceManagementEvent::Acknowledged => {
                        acknowledged = true;
                        break;
                    }
                    DeviceManagementEvent::Response(raw_cemi) if matches(&raw_cemi) => {
                        response = Some(raw_cemi);
                    }
                    DeviceManagementEvent::Indication(indication) => {
                        self.pending_indications.push_back(indication);
                    }
                    DeviceManagementEvent::Response(_) | DeviceManagementEvent::Ignored => {}
                }
            }
            if acknowledged || response.is_some() {
                break;
            }
        }

        if !acknowledged && response.is_none() {
            self.closed = true;
            return Err(TransportError::Timeout {
                timeout_ms: timeout.as_millis() as u64,
            }
            .into());
        }
        self.send_sequence = self.send_sequence.wrapping_add(1);
        if let Some(response) = response {
            return Ok(response);
        }

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match tokio::time::timeout_at(deadline, self.next_event(expected_response)).await {
                Ok(Ok(DeviceManagementEvent::Response(raw_cemi))) if matches(&raw_cemi) => {
                    return Ok(raw_cemi);
                }
                Ok(Ok(DeviceManagementEvent::Indication(indication))) => {
                    self.pending_indications.push_back(indication);
                }
                Ok(Ok(DeviceManagementEvent::Acknowledged | DeviceManagementEvent::Ignored))
                | Ok(Ok(DeviceManagementEvent::Response(_))) => {}
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return Err(TransportError::Timeout {
                        timeout_ms: timeout.as_millis() as u64,
                    }
                    .into());
                }
            }
        }
    }

    /// Wait for the next unsolicited M_PropInfo.ind message from the server.
    ///
    /// Indications received while another request is in progress are queued
    /// and returned here afterward.
    pub async fn receive_property_info_indication(
        &mut self,
        timeout: Duration,
    ) -> Result<CemiPropertyInfoIndication> {
        if let Some(indication) = self.pending_indications.pop_front() {
            return Ok(indication);
        }
        if self.closed {
            return Err(TransportError::ConnectionClosed.into());
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match tokio::time::timeout_at(deadline, self.next_event(MessageCode::MPropInfoInd))
                .await
            {
                Ok(Ok(DeviceManagementEvent::Indication(indication))) => return Ok(indication),
                Ok(Ok(DeviceManagementEvent::Acknowledged | DeviceManagementEvent::Ignored))
                | Ok(Ok(DeviceManagementEvent::Response(_))) => {}
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return Err(TransportError::Timeout {
                        timeout_ms: timeout.as_millis() as u64,
                    }
                    .into());
                }
            }
        }
    }

    /// Check that the device-management channel is still open.
    pub async fn heartbeat(&mut self, timeout: Duration) -> Result<()> {
        if self.closed {
            return Err(TransportError::ConnectionClosed.into());
        }
        let request = ConnectionstateRequest::new(self.channel_id, self.local_hpai.clone());
        self.transport
            .send(
                self.gateway,
                &KnxIpFrame::new(ServiceType::ConnectionstateRequest, request.serialize())
                    .serialize(),
            )
            .await?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let (frame, source) = tokio::time::timeout_at(deadline, self.transport.receive())
                .await
                .map_err(|_| TransportError::Timeout {
                    timeout_ms: timeout.as_millis() as u64,
                })??;
            if source.is_some_and(|source| source != self.gateway)
                || frame.header.service_type != ServiceType::ConnectionstateResponse
            {
                continue;
            }
            let response = ConnectionstateResponse::parse(&frame.body)?;
            if response.communication_channel_id == self.channel_id {
                return if response.is_success() {
                    Ok(())
                } else {
                    Err(TransportError::ConnectionClosed.into())
                };
            }
        }
    }

    /// Close the device-management channel with a KNXnet/IP disconnect exchange.
    pub async fn disconnect(&mut self, timeout: Duration) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let request = DisconnectRequest::new(self.channel_id, self.local_hpai.clone());
        self.transport
            .send(
                self.gateway,
                &KnxIpFrame::new(ServiceType::DisconnectRequest, request.serialize()).serialize(),
            )
            .await?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let (frame, source) = tokio::time::timeout_at(deadline, self.transport.receive())
                .await
                .map_err(|_| TransportError::Timeout {
                    timeout_ms: timeout.as_millis() as u64,
                })??;
            if source.is_some_and(|source| source != self.gateway)
                || frame.header.service_type != ServiceType::DisconnectResponse
            {
                continue;
            }
            let response = DisconnectResponse::parse(&frame.body)?;
            if response.communication_channel_id == self.channel_id {
                self.closed = true;
                self.transport.stop_keepalive();
                return if response.is_success() {
                    Ok(())
                } else {
                    Err(TransportError::ConnectionClosed.into())
                };
            }
        }
    }

    async fn next_event(&mut self, expected: MessageCode) -> Result<DeviceManagementEvent> {
        let (frame, source) = self.transport.receive().await?;
        if source.is_some_and(|source| source != self.gateway && source != self.data_endpoint) {
            return Ok(DeviceManagementEvent::Ignored);
        }
        match frame.header.service_type {
            ServiceType::DeviceConfigurationAck => {
                let ack = TunnellingAck::parse(&frame.body)?;
                if ack.communication_channel_id == self.channel_id
                    && ack.sequence_counter == self.send_sequence
                {
                    if !ack.is_success() {
                        return Err(TransportError::InvalidConfiguration {
                            details: format!(
                                "device-management request rejected with status 0x{:02X}",
                                ack.status_code
                            ),
                        }
                        .into());
                    }
                    return Ok(DeviceManagementEvent::Acknowledged);
                }
            }
            ServiceType::DeviceConfigurationRequest => {
                let request = TunnellingRequest::parse(&frame.body)?;
                if request.communication_channel_id != self.channel_id {
                    return Ok(DeviceManagementEvent::Ignored);
                }
                let expected_sequence = self.receive_sequence;
                let repeated_sequence = expected_sequence.wrapping_sub(1);
                if request.sequence_counter == expected_sequence {
                    self.receive_sequence = expected_sequence.wrapping_add(1);
                    self.send_device_configuration_ack(request.sequence_counter)
                        .await?;
                    if request.raw_cemi.first() == Some(&MessageCode::MPropInfoInd.to_u8()) {
                        return Ok(DeviceManagementEvent::Indication(
                            CemiPropertyInfoIndication::parse(&request.raw_cemi)?,
                        ));
                    }
                    if request.raw_cemi.first() == Some(&expected.to_u8()) {
                        return Ok(DeviceManagementEvent::Response(request.raw_cemi));
                    }
                } else if request.sequence_counter == repeated_sequence {
                    self.send_device_configuration_ack(request.sequence_counter)
                        .await?;
                } else {
                    return Ok(DeviceManagementEvent::Ignored);
                }
            }
            _ => {}
        }
        Ok(DeviceManagementEvent::Ignored)
    }

    async fn send_device_configuration_ack(&self, sequence: u8) -> Result<()> {
        let ack = TunnellingAck::new(self.channel_id, sequence, TunnellingAck::STATUS_OK);
        self.transport
            .send(
                self.data_endpoint,
                &KnxIpFrame::new(ServiceType::DeviceConfigurationAck, ack.serialize()).serialize(),
            )
            .await
    }
}

enum DeviceManagementEvent {
    Acknowledged,
    Response(Vec<u8>),
    Indication(CemiPropertyInfoIndication),
    Ignored,
}

fn same_property(
    left: crate::protocol::cemi::CemiPropertyInfo,
    right: crate::protocol::cemi::CemiPropertyInfo,
) -> bool {
    left.object_type == right.object_type
        && left.object_instance == right.object_instance
        && left.property_id == right.property_id
}

async fn send_udp_frame(socket: &UdpSocket, destination: SocketAddr, frame: &[u8]) -> Result<()> {
    socket
        .send_to(frame, destination)
        .await
        .map(|_| ())
        .map_err(|source| {
            TransportError::SocketError {
                operation: "send device-management frame".into(),
                source,
            }
            .into()
        })
}

async fn receive_udp_frame(socket: &UdpSocket) -> Result<(KnxIpFrame, SocketAddr)> {
    let mut buffer = vec![0; usize::from(u16::MAX)];
    let (length, source) =
        socket
            .recv_from(&mut buffer)
            .await
            .map_err(|source| TransportError::SocketError {
                operation: "receive device-management frame".into(),
                source,
            })?;
    Ok((KnxIpFrame::parse(&buffer[..length])?, source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;

    #[tokio::test]
    async fn udp_receive_keeps_the_full_datagram_with_a_small_future() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let body = vec![0xa5; 4096];
        let frame = KnxIpFrame::new(ServiceType::DeviceConfigurationRequest, body.clone());
        sender
            .send_to(&frame.serialize(), socket.local_addr().unwrap())
            .await
            .unwrap();

        let receive = receive_udp_frame(&socket);
        assert!(std::mem::size_of_val(&receive) < 4096);
        let (received, source) = timeout(Duration::from_secs(1), receive)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source, sender.local_addr().unwrap());
        assert_eq!(received.body, body);
    }

    #[tokio::test]
    async fn udp_device_management_client_reads_and_writes_properties_with_loopback_server() {
        use crate::protocol::cemi::{
            CemiPropertyInfo, CemiPropertyInfoIndication, CemiPropertyReadRequest,
            CemiPropertyReadResponse, CemiPropertyWriteRequest, CemiPropertyWriteResponse,
        };

        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let gateway = server.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let mut buffer = [0; 2048];
            let (length, client) = timeout(Duration::from_secs(2), server.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            let frame = KnxIpFrame::parse(&buffer[..length]).unwrap();
            assert_eq!(frame.header.service_type, ServiceType::ConnectRequest);
            let request = ConnectRequest::parse(&frame.body).unwrap();
            assert_eq!(request.cri.serialize(), [0x02, 0x03]);
            let response = ConnectResponse {
                channel_id: 5,
                status: ConnectResponse::STATUS_OK,
                data_endpoint: Hpai::new(gateway),
                crd: vec![0x02, 0x03],
                assigned_address: None,
            };
            server
                .send_to(
                    &KnxIpFrame::new(ServiceType::ConnectResponse, response.serialize())
                        .serialize(),
                    client,
                )
                .await
                .unwrap();

            let property = CemiPropertyInfo {
                object_type: 0x0011,
                object_instance: 1,
                property_id: 2,
                number_of_elements: 1,
                start_index: 1,
            };
            let (length, client) = timeout(Duration::from_secs(2), server.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            let frame = KnxIpFrame::parse(&buffer[..length]).unwrap();
            assert_eq!(
                frame.header.service_type,
                ServiceType::DeviceConfigurationRequest
            );
            let request = TunnellingRequest::parse(&frame.body).unwrap();
            assert_eq!(request.sequence_counter, 0);
            assert_eq!(
                CemiPropertyReadRequest::decode(&request.raw_cemi)
                    .unwrap()
                    .property,
                property
            );
            let read_request_raw = request.raw_cemi.clone();

            server
                .send_to(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationAck,
                        TunnellingAck::new_ok(5, 1).serialize(),
                    )
                    .serialize(),
                    client,
                )
                .await
                .unwrap();
            let (length, client) = timeout(Duration::from_secs(4), server.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            let frame = KnxIpFrame::parse(&buffer[..length]).unwrap();
            assert_eq!(
                frame.header.service_type,
                ServiceType::DeviceConfigurationRequest
            );
            let retry = TunnellingRequest::parse(&frame.body).unwrap();
            assert_eq!(retry.sequence_counter, 0);
            assert_eq!(retry.raw_cemi, read_request_raw);

            server
                .send_to(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationAck,
                        TunnellingAck::new_ok(5, 0).serialize(),
                    )
                    .serialize(),
                    client,
                )
                .await
                .unwrap();
            let indication = CemiPropertyInfoIndication {
                object_type: 0x0012,
                object_instance: 1,
                property_id: 5,
                element_count: 1,
                start_index: 1,
                data: vec![0x7F],
            }
            .serialize()
            .unwrap();
            server
                .send_to(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationRequest,
                        TunnellingRequest::new(5, 0, indication).serialize(),
                    )
                    .serialize(),
                    client,
                )
                .await
                .unwrap();
            let (length, _) = timeout(Duration::from_secs(2), server.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            let indication_ack = KnxIpFrame::parse(&buffer[..length]).unwrap();
            assert_eq!(
                TunnellingAck::parse(&indication_ack.body)
                    .unwrap()
                    .sequence_counter,
                0
            );
            let response = CemiPropertyReadResponse {
                property,
                data: vec![0x42],
                error_code: None,
            }
            .encode()
            .unwrap();
            server
                .send_to(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationRequest,
                        TunnellingRequest::new(5, 1, response).serialize(),
                    )
                    .serialize(),
                    client,
                )
                .await
                .unwrap();
            let (length, _) = timeout(Duration::from_secs(2), server.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            let ack_frame = KnxIpFrame::parse(&buffer[..length]).unwrap();
            assert_eq!(
                ack_frame.header.service_type,
                ServiceType::DeviceConfigurationAck
            );
            assert_eq!(
                TunnellingAck::parse(&ack_frame.body)
                    .unwrap()
                    .sequence_counter,
                1
            );

            let (length, client) = timeout(Duration::from_secs(2), server.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            let frame = KnxIpFrame::parse(&buffer[..length]).unwrap();
            assert_eq!(
                frame.header.service_type,
                ServiceType::DeviceConfigurationRequest
            );
            let request = TunnellingRequest::parse(&frame.body).unwrap();
            assert_eq!(request.sequence_counter, 1);
            let write = CemiPropertyWriteRequest::decode(&request.raw_cemi).unwrap();
            assert_eq!(write.property, property);
            assert_eq!(write.data, [0x12, 0x34]);

            server
                .send_to(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationAck,
                        TunnellingAck::new_ok(5, 1).serialize(),
                    )
                    .serialize(),
                    client,
                )
                .await
                .unwrap();
            let response = CemiPropertyWriteResponse {
                property,
                error_code: None,
            }
            .encode()
            .unwrap();
            server
                .send_to(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationRequest,
                        TunnellingRequest::new(5, 2, response).serialize(),
                    )
                    .serialize(),
                    client,
                )
                .await
                .unwrap();
            let (length, _) = timeout(Duration::from_secs(2), server.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            let ack_frame = KnxIpFrame::parse(&buffer[..length]).unwrap();
            assert_eq!(
                ack_frame.header.service_type,
                ServiceType::DeviceConfigurationAck
            );
            assert_eq!(
                TunnellingAck::parse(&ack_frame.body)
                    .unwrap()
                    .sequence_counter,
                2
            );

            let (length, client) = timeout(Duration::from_secs(2), server.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            let frame = KnxIpFrame::parse(&buffer[..length]).unwrap();
            assert_eq!(frame.header.service_type, ServiceType::DisconnectRequest);
            let request = DisconnectRequest::parse(&frame.body).unwrap();
            assert_eq!(request.communication_channel_id, 5);
            let response = DisconnectResponse::new(5, DisconnectResponse::STATUS_OK);
            server
                .send_to(
                    &KnxIpFrame::new(ServiceType::DisconnectResponse, response.serialize())
                        .serialize(),
                    client,
                )
                .await
                .unwrap();
        });

        let mut connection = DeviceManagementConnection::connect(
            gateway,
            "127.0.0.1:0".parse().unwrap(),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        let property = CemiPropertyInfo {
            object_type: 0x0011,
            object_instance: 1,
            property_id: 2,
            number_of_elements: 1,
            start_index: 1,
        };
        assert_eq!(
            connection
                .read_property(property, Duration::from_secs(2))
                .await
                .unwrap(),
            [0x42]
        );
        let indication = connection
            .receive_property_info_indication(Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(indication.object_type, 0x0012);
        assert_eq!(indication.object_instance, 1);
        assert_eq!(indication.property_id, 5);
        assert_eq!(indication.data, [0x7F]);
        connection
            .write_property(property, &[0x12, 0x34], Duration::from_secs(2))
            .await
            .unwrap();
        connection.disconnect(Duration::from_secs(2)).await.unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn tcp_device_management_client_reads_and_writes_properties_with_loopback_server() {
        use crate::protocol::cemi::{
            CemiPropertyInfo, CemiPropertyReadRequest, CemiPropertyReadResponse,
            CemiPropertyWriteRequest, CemiPropertyWriteResponse,
        };
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let (stream, _) = timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let transport = TcpFrameTransport::from_accepted_stream(stream);
            let frame = KnxIpFrame::parse(
                &timeout(Duration::from_secs(2), transport.recv_frame())
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(frame.header.service_type, ServiceType::ConnectRequest);
            let request = ConnectRequest::parse(&frame.body).unwrap();
            assert_eq!(request.cri.serialize(), [0x02, 0x03]);
            assert_eq!(
                request.control_endpoint.host_protocol_code,
                Hpai::PROTOCOL_TCP
            );
            assert_eq!(request.data_endpoint.host_protocol_code, Hpai::PROTOCOL_TCP);
            let response = ConnectResponse {
                channel_id: 7,
                status: ConnectResponse::STATUS_OK,
                data_endpoint: Hpai::new_tcp_route_back(),
                crd: vec![0x02, 0x03],
                assigned_address: None,
            };
            transport
                .send_frame(
                    &KnxIpFrame::new(ServiceType::ConnectResponse, response.serialize())
                        .serialize(),
                )
                .await
                .unwrap();

            let property = CemiPropertyInfo {
                object_type: 0x0011,
                object_instance: 1,
                property_id: 2,
                number_of_elements: 1,
                start_index: 1,
            };
            let frame = KnxIpFrame::parse(
                &timeout(Duration::from_secs(2), transport.recv_frame())
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                frame.header.service_type,
                ServiceType::DeviceConfigurationRequest
            );
            let request = TunnellingRequest::parse(&frame.body).unwrap();
            assert_eq!(request.sequence_counter, 0);
            assert_eq!(
                CemiPropertyReadRequest::decode(&request.raw_cemi)
                    .unwrap()
                    .property,
                property
            );

            transport
                .send_frame(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationAck,
                        TunnellingAck::new_ok(7, 0).serialize(),
                    )
                    .serialize(),
                )
                .await
                .unwrap();
            let response = CemiPropertyReadResponse {
                property,
                data: vec![0x24],
                error_code: None,
            }
            .encode()
            .unwrap();
            transport
                .send_frame(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationRequest,
                        TunnellingRequest::new(7, 0, response).serialize(),
                    )
                    .serialize(),
                )
                .await
                .unwrap();

            let frame = KnxIpFrame::parse(
                &timeout(Duration::from_secs(2), transport.recv_frame())
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                frame.header.service_type,
                ServiceType::DeviceConfigurationAck
            );
            assert_eq!(
                TunnellingAck::parse(&frame.body).unwrap().sequence_counter,
                0
            );

            let frame = KnxIpFrame::parse(
                &timeout(Duration::from_secs(2), transport.recv_frame())
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            let request = TunnellingRequest::parse(&frame.body).unwrap();
            assert_eq!(request.sequence_counter, 1);
            let write = CemiPropertyWriteRequest::decode(&request.raw_cemi).unwrap();
            assert_eq!(write.property, property);
            assert_eq!(write.data, [0x12, 0x34]);
            transport
                .send_frame(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationAck,
                        TunnellingAck::new_ok(7, 1).serialize(),
                    )
                    .serialize(),
                )
                .await
                .unwrap();
            let response = CemiPropertyWriteResponse {
                property,
                error_code: None,
            }
            .encode()
            .unwrap();
            transport
                .send_frame(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationRequest,
                        TunnellingRequest::new(7, 1, response).serialize(),
                    )
                    .serialize(),
                )
                .await
                .unwrap();
            let frame = KnxIpFrame::parse(
                &timeout(Duration::from_secs(2), transport.recv_frame())
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                TunnellingAck::parse(&frame.body).unwrap().sequence_counter,
                1
            );

            let frame = KnxIpFrame::parse(
                &timeout(Duration::from_secs(2), transport.recv_frame())
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                frame.header.service_type,
                ServiceType::ConnectionstateRequest
            );
            let request = ConnectionstateRequest::parse(&frame.body).unwrap();
            assert_eq!(request.communication_channel_id, 7);
            let response = ConnectionstateResponse::new(7, ConnectionstateResponse::STATUS_OK);
            transport
                .send_frame(
                    &KnxIpFrame::new(ServiceType::ConnectionstateResponse, response.serialize())
                        .serialize(),
                )
                .await
                .unwrap();

            let frame = KnxIpFrame::parse(
                &timeout(Duration::from_secs(2), transport.recv_frame())
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(frame.header.service_type, ServiceType::DisconnectRequest);
            let response = DisconnectResponse::new(7, DisconnectResponse::STATUS_OK);
            transport
                .send_frame(
                    &KnxIpFrame::new(ServiceType::DisconnectResponse, response.serialize())
                        .serialize(),
                )
                .await
                .unwrap();
        });

        let mut connection =
            DeviceManagementConnection::connect_tcp(gateway, Duration::from_secs(2))
                .await
                .unwrap();
        let property = CemiPropertyInfo {
            object_type: 0x0011,
            object_instance: 1,
            property_id: 2,
            number_of_elements: 1,
            start_index: 1,
        };
        assert_eq!(
            connection
                .read_property(property, Duration::from_secs(2))
                .await
                .unwrap(),
            [0x24]
        );
        connection
            .write_property(property, &[0x12, 0x34], Duration::from_secs(2))
            .await
            .unwrap();
        connection.heartbeat(Duration::from_secs(2)).await.unwrap();
        connection.disconnect(Duration::from_secs(2)).await.unwrap();
        server_task.await.unwrap();
    }

    #[cfg(feature = "secure")]
    #[tokio::test]
    async fn secure_device_management_reads_property_over_udp_and_tcp_loopback() {
        use crate::protocol::cemi::{
            CemiPropertyInfo, CemiPropertyReadRequest, CemiPropertyReadResponse,
        };
        use crate::security::{SecureSession, SessionConfig};
        use crate::transport::SecurityConfig;
        use tokio::net::TcpListener;

        async fn serve(transport: PlainManagementTransport, gateway: SocketAddr) {
            async fn receive_secure(
                transport: &PlainManagementTransport,
                session: &SecureSession,
            ) -> (KnxIpFrame, Option<SocketAddr>) {
                let (wrapper, source) = timeout(Duration::from_secs(3), transport.receive())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(wrapper.header.service_type, ServiceType::SecureWrapper);
                let plain = session.decrypt_frame(&wrapper.serialize()).await.unwrap();
                (KnxIpFrame::parse(&plain).unwrap(), source)
            }

            async fn send_secure(
                transport: &PlainManagementTransport,
                peer: SocketAddr,
                session: &SecureSession,
                service: ServiceType,
                body: Vec<u8>,
            ) {
                let plain = KnxIpFrame::new(service, body).serialize();
                let encrypted = session.encrypt_frame(&plain).await.unwrap();
                transport.send(peer, &encrypted).await.unwrap();
            }

            let (frame, source) = timeout(Duration::from_secs(3), transport.receive())
                .await
                .unwrap()
                .unwrap();
            let peer = source.unwrap_or(gateway);
            assert_eq!(frame.header.service_type, ServiceType::SessionRequest);
            let request = SessionRequest::parse(&frame.body).unwrap();

            let config = SessionConfig {
                user_id: 2,
                user_password: "user password".into(),
                device_auth_password: Some("device password".into()),
                keepalive_interval: 5,
            };
            let mut session = SecureSession::new(&config);
            let public_key: [u8; 32] = session.initialize().await.try_into().unwrap();
            let session_id = 9;
            let mac: [u8; 16] = session
                .process_session_request(&request.public_key, session_id)
                .await
                .unwrap()
                .try_into()
                .unwrap();
            let response = SessionResponse {
                session_id,
                public_key,
                mac,
            };
            transport
                .send(
                    peer,
                    &KnxIpFrame::new(ServiceType::SessionResponse, response.serialize())
                        .serialize(),
                )
                .await
                .unwrap();

            let (frame, _) = receive_secure(&transport, &session).await;
            assert_eq!(frame.header.service_type, ServiceType::SessionAuthenticate);
            let auth = SessionAuthenticate::parse(&frame.body).unwrap();
            assert!(
                session
                    .verify_authenticate_mac(&request.public_key, auth.user_id, &auth.mac)
                    .await
                    .unwrap()
            );
            let status = SessionStatus {
                status: SessionStatus::STATUS_OK,
            };
            send_secure(
                &transport,
                peer,
                &session,
                ServiceType::SessionStatus,
                status.serialize(),
            )
            .await;

            let (frame, _) = receive_secure(&transport, &session).await;
            assert_eq!(frame.header.service_type, ServiceType::ConnectRequest);
            let connect = ConnectRequest::parse(&frame.body).unwrap();
            assert_eq!(connect.cri.serialize(), [0x02, 0x03]);
            let response = ConnectResponse {
                channel_id: 9,
                status: ConnectResponse::STATUS_OK,
                data_endpoint: Hpai::new(gateway),
                crd: vec![0x02, 0x03],
                assigned_address: None,
            };
            send_secure(
                &transport,
                peer,
                &session,
                ServiceType::ConnectResponse,
                response.serialize(),
            )
            .await;

            let mut keepalive_count = 0;
            let frame = loop {
                let (frame, _) = receive_secure(&transport, &session).await;
                if frame.header.service_type == ServiceType::SessionStatus {
                    assert_eq!(frame.body, [0x04, 0x00]);
                    keepalive_count += 1;
                } else {
                    break frame;
                }
            };
            assert!(keepalive_count > 0);
            assert_eq!(
                frame.header.service_type,
                ServiceType::DeviceConfigurationRequest
            );

            let property = CemiPropertyInfo {
                object_type: 0x0011,
                object_instance: 1,
                property_id: 2,
                number_of_elements: 1,
                start_index: 1,
            };
            let request = TunnellingRequest::parse(&frame.body).unwrap();
            assert_eq!(request.sequence_counter, 0);
            assert_eq!(
                CemiPropertyReadRequest::decode(&request.raw_cemi)
                    .unwrap()
                    .property,
                property
            );
            send_secure(
                &transport,
                peer,
                &session,
                ServiceType::DeviceConfigurationAck,
                TunnellingAck::new_ok(9, 0).serialize(),
            )
            .await;
            let response = CemiPropertyReadResponse {
                property,
                data: vec![0x42],
                error_code: None,
            }
            .encode()
            .unwrap();
            send_secure(
                &transport,
                peer,
                &session,
                ServiceType::DeviceConfigurationRequest,
                TunnellingRequest::new(9, 0, response).serialize(),
            )
            .await;

            let (frame, _) = receive_secure(&transport, &session).await;
            assert_eq!(
                frame.header.service_type,
                ServiceType::DeviceConfigurationAck
            );
            assert_eq!(
                TunnellingAck::parse(&frame.body).unwrap().sequence_counter,
                0
            );

            let (frame, _) = receive_secure(&transport, &session).await;
            assert_eq!(frame.header.service_type, ServiceType::DisconnectRequest);
            let disconnect = DisconnectRequest::parse(&frame.body).unwrap();
            let response = DisconnectResponse::new(
                disconnect.communication_channel_id,
                DisconnectResponse::STATUS_OK,
            );
            send_secure(
                &transport,
                peer,
                &session,
                ServiceType::DisconnectResponse,
                response.serialize(),
            )
            .await;
        }

        let security = SecurityConfig::new("device password", "user password")
            .with_user_id(2)
            .unwrap()
            .with_keepalive_interval(Duration::from_secs(1))
            .unwrap();
        for tcp in [false, true] {
            let (gateway, server_task) = if tcp {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let gateway = listener.local_addr().unwrap();
                let task = tokio::spawn(async move {
                    let (stream, _) = timeout(Duration::from_secs(3), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                    serve(
                        PlainManagementTransport::Tcp(TcpFrameTransport::from_accepted_stream(
                            stream,
                        )),
                        gateway,
                    )
                    .await;
                });
                (gateway, task)
            } else {
                let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
                let gateway = server.local_addr().unwrap();
                let task = tokio::spawn(async move {
                    serve(PlainManagementTransport::Udp(server), gateway).await;
                });
                (gateway, task)
            };

            let mut connection = if tcp {
                DeviceManagementConnection::connect_tcp_secure(
                    gateway,
                    &security,
                    Duration::from_secs(3),
                )
                .await
                .unwrap()
            } else {
                DeviceManagementConnection::connect_secure(
                    gateway,
                    "127.0.0.1:0".parse().unwrap(),
                    &security,
                    Duration::from_secs(3),
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
            tokio::time::sleep(Duration::from_millis(2_200)).await;
            assert_eq!(
                connection
                    .read_property(property, Duration::from_secs(3))
                    .await
                    .unwrap(),
                [0x42]
            );
            connection.disconnect(Duration::from_secs(3)).await.unwrap();
            server_task.await.unwrap();
        }
    }
}
