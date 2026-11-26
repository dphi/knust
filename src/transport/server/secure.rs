//! KNX IP Secure handshakes and authenticated tunnel dispatch.

use super::{ClientLink, TunnelServer};
use crate::error::{Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::address::IndividualAddress;
use crate::protocol::knxip::{
    KnxIpFrame, ServiceType, SessionAuthenticate, SessionRequest, SessionResponse, SessionStatus,
};
use crate::security::{SecureSession, SessionConfig};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// A pending or established KNX IP Secure session for one peer, plus the
/// client's ECDH public key (needed again to verify `SessionAuthenticate`).
pub(super) struct SecureSessionEntry {
    session: tokio::sync::Mutex<SecureSession>,
    client_public_key: [u8; 32],
}

impl TunnelServer {
    /// Bind a tunneling server that requires KNX IP Secure: clients must
    /// complete the SessionRequest/Response/Authenticate/Status handshake
    /// (using the device-auth/user credentials in `security`) before any
    /// tunneling frame is accepted; a plain (unwrapped) `ConnectRequest` is
    /// rejected.
    // TODO: no independent secure client has completed a real handshake against this server.
    /// # Errors
    ///
    /// Returns [`TransportError::SocketError`] if binding the UDP or TCP
    /// listener on `addr` fails.
    pub async fn bind_secure(
        addr: SocketAddr,
        individual_address: IndividualAddress,
        security: SessionConfig,
    ) -> Result<Arc<Self>> {
        Self::bind_with_security(addr, individual_address, Some(security)).await
    }

    pub(super) async fn handle_session_request(&self, body: &[u8], link: ClientLink) -> Result<()> {
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

    async fn handle_session_authenticate(
        &self,
        body: &[u8],
        link: ClientLink,
        entry: &SecureSessionEntry,
    ) -> Result<()> {
        let auth = SessionAuthenticate::parse(body)?;
        let peer = link.peer_addr();
        let mut session = entry.session.lock().await;
        let ok = session
            .verify_authenticate_mac(&entry.client_public_key, auth.user_id, &auth.mac)
            .await?;

        let status = if ok {
            SessionStatus::STATUS_OK
        } else {
            SessionStatus::STATUS_AUTH_FAILED
        };
        let response = SessionStatus { status };
        let frame = KnxIpFrame::new(ServiceType::SessionStatus, response.serialize()).serialize();
        link.send_frame(&session.encrypt_frame(&frame).await?)
            .await?;
        drop(session);

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
    pub(super) async fn maybe_encrypt(&self, frame: Vec<u8>, peer: SocketAddr) -> Result<Vec<u8>> {
        let entry = self.secure_sessions.read().await.get(&peer).cloned();
        match entry {
            Some(entry) => entry.session.lock().await.encrypt_frame(&frame).await,
            None => Ok(frame),
        }
    }

    pub(super) async fn handle_secure_wrapper(
        self: &Arc<Self>,
        data: &[u8],
        link: ClientLink,
    ) -> Result<Option<u8>> {
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
        let session = entry.session.lock().await;
        let plaintext = session.decrypt_frame(data).await?;
        let authenticated = session.is_authenticated().await;
        drop(session);
        let inner = KnxIpFrame::parse(&plaintext)?;
        if !authenticated {
            if inner.header.service_type == ServiceType::SessionAuthenticate {
                self.handle_session_authenticate(&inner.body, link, &entry)
                    .await?;
                return Ok(None);
            }
            log_transport!(
                LogLevel::Warn,
                "TunnelServer: SecureWrapper from {} rejected — session not yet authenticated",
                peer
            );
            return Ok(None);
        }
        self.dispatch_plain_frame(inner, link, true).await
    }
}
