//! KNXnet/IP device-management client connections.
//!
//! These connections access the server's own interface objects. They are
//! separate from [`super::Connection`], which carries telegrams to the KNX bus.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;

use crate::error::{ProtocolError, Result, TransportError};
use crate::protocol::cemi::MessageCode;
use crate::protocol::knxip::{
    ConnectRequest, ConnectResponse, ConnectionRequestInfo, ConnectionstateRequest,
    ConnectionstateResponse, DisconnectRequest, DisconnectResponse, Hpai, KnxIpFrame, ServiceType,
    TunnellingAck, TunnellingRequest,
};

/// An unencrypted UDP KNXnet/IP device-management connection.
///
/// Use [`request_cemi`](Self::request_cemi) to send one cEMI request and await
/// its matching response. Requests on a connection are serialized.
pub struct DeviceManagementConnection {
    socket: UdpSocket,
    gateway: SocketAddr,
    data_endpoint: SocketAddr,
    local_hpai: Hpai,
    channel_id: u8,
    send_sequence: u8,
    receive_sequence: u8,
    closed: bool,
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
        let mut connect = ConnectRequest::new(local_hpai.socket_addr(), local_hpai.socket_addr());
        connect.cri = ConnectionRequestInfo::new_device_management();
        send_frame(
            &socket,
            gateway,
            &KnxIpFrame::new(ServiceType::ConnectRequest, connect.serialize()).serialize(),
        )
        .await?;

        let response = loop {
            let (frame, source) = tokio::time::timeout(timeout, receive_frame(&socket))
                .await
                .map_err(|_| TransportError::Timeout {
                    timeout_ms: timeout.as_millis() as u64,
                })??;
            if source == gateway && frame.header.service_type == ServiceType::ConnectResponse {
                break ConnectResponse::parse(&frame.body)?;
            }
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
        let data_endpoint = if response.data_endpoint.port == 0
            || response.data_endpoint.ip_addr.is_unspecified()
        {
            gateway
        } else {
            response.data_endpoint.socket_addr()
        };

        Ok(Self {
            socket,
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
            send_frame(&self.socket, self.data_endpoint, &frame).await?;
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
                    DeviceManagementEvent::Response(raw_cemi) => response = Some(raw_cemi),
                    DeviceManagementEvent::Ignored => {}
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
                Ok(Ok(DeviceManagementEvent::Response(raw_cemi))) => return Ok(raw_cemi),
                Ok(Ok(DeviceManagementEvent::Acknowledged)) => {}
                Ok(Ok(DeviceManagementEvent::Ignored)) => {}
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
        send_frame(
            &self.socket,
            self.gateway,
            &KnxIpFrame::new(ServiceType::ConnectionstateRequest, request.serialize()).serialize(),
        )
        .await?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let (frame, source) = tokio::time::timeout_at(deadline, receive_frame(&self.socket))
                .await
                .map_err(|_| TransportError::Timeout {
                    timeout_ms: timeout.as_millis() as u64,
                })??;
            if source != self.gateway
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
        send_frame(
            &self.socket,
            self.gateway,
            &KnxIpFrame::new(ServiceType::DisconnectRequest, request.serialize()).serialize(),
        )
        .await?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let (frame, source) = tokio::time::timeout_at(deadline, receive_frame(&self.socket))
                .await
                .map_err(|_| TransportError::Timeout {
                    timeout_ms: timeout.as_millis() as u64,
                })??;
            if source != self.gateway
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
        let (frame, source) = receive_frame(&self.socket).await?;
        if source != self.gateway && source != self.data_endpoint {
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
        send_frame(
            &self.socket,
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

async fn send_frame(socket: &UdpSocket, destination: SocketAddr, frame: &[u8]) -> Result<()> {
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

async fn receive_frame(socket: &UdpSocket) -> Result<(KnxIpFrame, SocketAddr)> {
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
    use crate::protocol::IndividualAddress;
    use crate::protocol::address::Address;
    use crate::protocol::cemi::CemiFrame;
    use tokio::time::timeout;

    #[tokio::test]
    async fn udp_device_management_client_exchanges_cemi_with_loopback_server() {
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
            assert_eq!(request.communication_channel_id, 5);
            assert_eq!(request.sequence_counter, 0);
            assert_eq!(request.raw_cemi[0], MessageCode::MPropReadReq.to_u8());

            let stale_ack = TunnellingAck::new_ok(5, 1);
            server
                .send_to(
                    &KnxIpFrame::new(ServiceType::DeviceConfigurationAck, stale_ack.serialize())
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
            assert_eq!(retry.communication_channel_id, 5);
            assert_eq!(retry.sequence_counter, 0);
            assert_eq!(retry.raw_cemi, request.raw_cemi);

            let ack = TunnellingAck::new_ok(5, 0);
            server
                .send_to(
                    &KnxIpFrame::new(ServiceType::DeviceConfigurationAck, ack.serialize())
                        .serialize(),
                    client,
                )
                .await
                .unwrap();
            let response = vec![MessageCode::MPropReadCon.to_u8(), 0x42];
            let response = TunnellingRequest::new(5, 0, response);
            server
                .send_to(
                    &KnxIpFrame::new(
                        ServiceType::DeviceConfigurationRequest,
                        response.serialize(),
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
            let frame = KnxIpFrame::parse(&buffer[..length]).unwrap();
            assert_eq!(
                frame.header.service_type,
                ServiceType::DeviceConfigurationAck
            );
            let ack = TunnellingAck::parse(&frame.body).unwrap();
            assert_eq!(ack.communication_channel_id, 5);
            assert_eq!(ack.sequence_counter, 0);

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
        let cemi = CemiFrame::new(
            MessageCode::MPropReadReq,
            IndividualAddress::new(1, 1, 1),
            Address::Individual(IndividualAddress::new(1, 1, 2)),
            vec![0x00, 0x01],
        )
        .serialize()
        .unwrap();
        let response = connection
            .request_cemi(&cemi, MessageCode::MPropReadCon, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(response, [MessageCode::MPropReadCon.to_u8(), 0x42]);
        connection.disconnect(Duration::from_secs(2)).await.unwrap();
        server_task.await.unwrap();
    }
}
