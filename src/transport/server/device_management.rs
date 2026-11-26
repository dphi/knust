//! Serving cEMI property requests on KNXnet/IP device-management channels.

use super::{ACK_TIMEOUT, ClientLink, ClientSession, ClientSessionKind, TunnelServer};
use crate::error::{ProtocolError, Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::cemi::{
    CemiPropertyInfo, CemiPropertyReadRequest, CemiPropertyReadResponse, CemiPropertyWriteRequest,
    CemiPropertyWriteResponse, MessageCode,
};
use crate::protocol::knxip::{KnxIpFrame, ServiceType, TunnellingAck, TunnellingRequest};
use crate::transport::tunnel::SequenceValidationResult;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::oneshot;

/// Application supplied handler for KNXnet/IP M_PropRead and M_PropWrite
/// requests. Return KNX cEMI error codes for unsupported or inaccessible
/// properties.
pub trait DeviceManagementHandler: Send + Sync {
    fn read_property(&self, property: CemiPropertyInfo) -> std::result::Result<Vec<u8>, u8>;

    fn write_property(
        &self,
        property: CemiPropertyInfo,
        data: &[u8],
    ) -> std::result::Result<(), u8>;
}

impl TunnelServer {
    /// Configure the property handler used by device-management clients.
    pub async fn set_device_management_handler(&self, handler: Arc<dyn DeviceManagementHandler>) {
        *self.device_management_handler.write().await = Some(handler);
    }

    pub(super) async fn handle_device_configuration_request(
        self: &Arc<Self>,
        body: &[u8],
        link: &ClientLink,
    ) -> Result<()> {
        let request = TunnellingRequest::parse(body)?;
        let session = self
            .data_session(
                request.communication_channel_id,
                link,
                ClientSessionKind::DeviceManagement,
            )
            .await;
        let Some(session) = session else {
            return Ok(());
        };
        session.touch();

        match session.validate_recv_seq(request.sequence_counter) {
            SequenceValidationResult::Valid => {
                let ack = TunnellingAck::new_ok(
                    request.communication_channel_id,
                    request.sequence_counter,
                );
                self.send_reply(
                    ServiceType::DeviceConfigurationAck,
                    &ack.serialize(),
                    &session.link,
                )
                .await?;

                let response = build_device_management_response(
                    self.device_management_handler.read().await.as_deref(),
                    &request.raw_cemi,
                )?;
                let server = self.clone();
                let client_sequence = request.sequence_counter;
                tokio::spawn(async move {
                    if let Err(error) = server
                        .send_device_configuration_response(
                            &session,
                            client_sequence,
                            None,
                            &response,
                        )
                        .await
                    {
                        log_transport!(
                            LogLevel::Warn,
                            "TunnelServer: failed to send device-configuration response: {}",
                            error
                        );
                    }
                });
            }
            SequenceValidationResult::Duplicate => {
                let ack = TunnellingAck::new_ok(
                    request.communication_channel_id,
                    request.sequence_counter,
                );
                self.send_reply(
                    ServiceType::DeviceConfigurationAck,
                    &ack.serialize(),
                    &session.link,
                )
                .await?;
                let cached = session.last_device_response.lock().unwrap().clone();
                if let Some((client_sequence, server_sequence, response)) = cached
                    && client_sequence == request.sequence_counter
                {
                    let server = self.clone();
                    tokio::spawn(async move {
                        if let Err(error) = server
                            .send_device_configuration_response(
                                &session,
                                client_sequence,
                                Some(server_sequence),
                                &response,
                            )
                            .await
                        {
                            log_transport!(
                                LogLevel::Warn,
                                "TunnelServer: failed to resend device-configuration response: {}",
                                error
                            );
                        }
                    });
                }
            }
            SequenceValidationResult::Invalid { .. } => {
                let ack = TunnellingAck::new_sequence_error(
                    request.communication_channel_id,
                    request.sequence_counter,
                );
                self.send_reply(
                    ServiceType::DeviceConfigurationAck,
                    &ack.serialize(),
                    &session.link,
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn send_device_configuration_response(
        &self,
        session: &Arc<ClientSession>,
        client_sequence: u8,
        replay_sequence: Option<u8>,
        cemi: &[u8],
    ) -> Result<()> {
        let _guard = session.send_lock.lock().await;
        let sequence = replay_sequence.unwrap_or_else(|| session.send_seq.load(Ordering::SeqCst));
        if replay_sequence.is_none() {
            *session.last_device_response.lock().unwrap() =
                Some((client_sequence, sequence, cemi.to_vec()));
        }
        let request = TunnellingRequest::new(session.channel_id, sequence, cemi.to_vec());
        let frame = KnxIpFrame::new(ServiceType::DeviceConfigurationRequest, request.serialize())
            .serialize();
        let frame = self.maybe_encrypt(frame, &session.link).await?;

        for attempt in 0..2u8 {
            let (tx, rx) = oneshot::channel();
            *session.pending_ack.lock().unwrap() = Some((sequence, tx));
            session.link.send_frame(&frame).await?;
            match tokio::time::timeout(ACK_TIMEOUT, rx).await {
                Ok(Ok(ack)) if ack.status_code == TunnellingAck::STATUS_OK => {
                    if session.send_seq.load(Ordering::SeqCst) == sequence {
                        session
                            .send_seq
                            .store(sequence.wrapping_add(1), Ordering::SeqCst);
                    }
                    return Ok(());
                }
                Ok(Ok(_)) => {
                    return Err(TransportError::InvalidConfiguration {
                        details: "client rejected device-configuration response".into(),
                    }
                    .into());
                }
                _ if attempt == 0 => {
                    log_transport!(
                        LogLevel::Warn,
                        "TunnelServer: device-configuration ACK timeout for channel {}, retrying once",
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

fn build_device_management_response(
    handler: Option<&dyn DeviceManagementHandler>,
    cemi: &[u8],
) -> Result<Vec<u8>> {
    match cemi.first().copied() {
        Some(code) if code == MessageCode::MPropReadReq.to_u8() => {
            let request = CemiPropertyReadRequest::decode(cemi)?;
            let (property, data, error_code) = match handler {
                Some(handler) => match handler.read_property(request.property) {
                    Ok(data) if !data.is_empty() => (request.property, data, None),
                    Ok(_) => (request.property, Vec::new(), Some(0x01)),
                    Err(error_code) => {
                        let property = CemiPropertyInfo {
                            number_of_elements: 0,
                            ..request.property
                        };
                        (property, Vec::new(), Some(error_code))
                    }
                },
                None => {
                    let property = CemiPropertyInfo {
                        number_of_elements: 0,
                        ..request.property
                    };
                    (property, Vec::new(), Some(0x01))
                }
            };
            CemiPropertyReadResponse {
                property,
                data,
                error_code,
            }
            .encode()
        }
        Some(code) if code == MessageCode::MPropWriteReq.to_u8() => {
            let request = CemiPropertyWriteRequest::decode(cemi)?;
            let error_code = match handler {
                Some(handler) => handler
                    .write_property(request.property, &request.data)
                    .err(),
                None => Some(0x01),
            };
            CemiPropertyWriteResponse {
                property: request.property,
                error_code,
            }
            .encode()
        }
        _ => Err(ProtocolError::InvalidFrame {
            details: "unsupported device-management cEMI request".into(),
        }
        .into()),
    }
}
