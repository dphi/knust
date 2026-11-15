//! KNXnet/IP device-management client connections.
//!
//! These connections access the server's own interface objects. They are
//! separate from [`super::Connection`], which carries telegrams to the KNX bus.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;

use super::frame_transport::{FrameTransport, TcpFrameTransport};
use crate::error::{ProtocolError, Result, TransportError};
use crate::protocol::cemi::MessageCode;
use crate::protocol::knxip::{
    ConnectRequest, ConnectResponse, ConnectionRequestInfo, ConnectionstateRequest,
    ConnectionstateResponse, DisconnectRequest, DisconnectResponse, Hpai, KnxIpFrame, ServiceType,
    TunnellingAck, TunnellingRequest,
};

/// An unencrypted KNXnet/IP device-management connection over UDP or TCP.
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
    closed: bool,
}

enum ManagementTransport {
    Udp(UdpSocket),
    Tcp(TcpFrameTransport),
}

impl ManagementTransport {
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
    let mut buffer = [0; u16::MAX as usize];
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
    async fn udp_device_management_client_reads_and_writes_properties_with_loopback_server() {
        use crate::protocol::cemi::{
            CemiPropertyInfo, CemiPropertyReadRequest, CemiPropertyReadResponse,
            CemiPropertyWriteRequest, CemiPropertyWriteResponse,
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
                        TunnellingRequest::new(5, 0, response).serialize(),
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
                0
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
}
