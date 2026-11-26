//! KNXnet/IP search and description replies for tunnel servers.

use super::{ClientLink, TunnelServer};
use crate::error::{ProtocolError, Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::knxip::{Hpai, KnxIpFrame, ServiceType};
use crate::transport::routing::{RoutingConnection, bind_multicast_listener};
use std::net::Ipv4Addr;
use std::sync::Arc;

impl TunnelServer {
    /// Answer KNXnet/IP multicast searches on the given IPv4 interface.
    ///
    /// Replies come from the server's existing UDP endpoint. Bind the server to
    /// the interface's unicast address or to a separate port if a wildcard bind
    /// on port 3671 prevents sharing the discovery port on the host.
    ///
    /// # Errors
    /// Returns [`TransportError::SocketError`] if the server is shut down,
    /// discovery is already enabled, the control endpoint is IPv6, or the
    /// multicast socket cannot be opened.
    ///
    /// # Panics
    /// Panics when called outside a Tokio runtime with I/O enabled.
    pub fn enable_multicast_discovery(self: &Arc<Self>, interface: Ipv4Addr) -> Result<()> {
        let mut discovery = self.discovery_handle.lock().unwrap();
        let socket = (|| {
            use std::io::{Error, ErrorKind};

            if self.is_shutdown() {
                return Err(Error::new(ErrorKind::NotConnected, "server is shut down"));
            }
            if discovery
                .as_ref()
                .is_some_and(|handle| !handle.is_finished())
            {
                return Err(Error::new(
                    ErrorKind::AlreadyExists,
                    "discovery is already enabled",
                ));
            }
            if !self.local_addr.is_ipv4() {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "discovery requires an IPv4 server",
                ));
            }

            bind_multicast_listener(
                RoutingConnection::MULTICAST_ADDR,
                RoutingConnection::MULTICAST_PORT,
                interface,
            )
        })()
        .map_err(|source| TransportError::SocketError {
            operation: "enable_multicast_discovery".to_owned(),
            source,
        })?;

        let server = Arc::downgrade(self);
        let mut shutdown = self.shutdown.subscribe();
        *discovery = Some(tokio::spawn(async move {
            let mut buf = [0; 1024];
            loop {
                let received = tokio::select! {
                    biased;
                    _ = shutdown.wait_for(|closed| *closed) => break,
                    received = socket.recv_from(&mut buf) => received,
                };
                let (len, peer_addr) = match received {
                    Ok(packet) => packet,
                    Err(error) => {
                        log_transport!(
                            LogLevel::Warn,
                            "TunnelServer: discovery receive failed: {}",
                            error
                        );
                        break;
                    }
                };
                let Ok(frame) = KnxIpFrame::parse(&buf[..len]) else {
                    continue;
                };
                if !matches!(
                    frame.header.service_type,
                    ServiceType::SearchRequest | ServiceType::SearchRequestExtended
                ) {
                    continue;
                }
                let Some(server) = server.upgrade() else {
                    break;
                };
                let link = ClientLink::Udp {
                    socket: server.socket.clone(),
                    peer_addr,
                };
                if let Err(error) = server.handle_discovery_request(frame, &link).await {
                    log_transport!(
                        LogLevel::Debug,
                        "TunnelServer: discovery request failed: {}",
                        error
                    );
                }
            }
        }));
        Ok(())
    }

    pub(super) async fn handle_discovery_request(
        &self,
        frame: KnxIpFrame,
        link: &ClientLink,
    ) -> Result<()> {
        let ClientLink::Udp { peer_addr, .. } = link else {
            return Ok(());
        };
        let endpoint = Hpai::parse(&frame.body)?;
        // Reply only to the UDP sender so a discovery request cannot redirect traffic.
        if endpoint.host_protocol_code != Hpai::PROTOCOL_UDP
            || (!endpoint.ip_addr.is_unspecified() && endpoint.ip_addr != peer_addr.ip())
            || (endpoint.port != 0 && endpoint.port != peer_addr.port())
        {
            return Ok(());
        }

        let mut parameters = &frame.body[Hpai::LENGTH..];
        let extended = frame.header.service_type == ServiceType::SearchRequestExtended;
        if extended {
            while let Some(&length) = parameters.first() {
                let length = usize::from(length);
                if length < 2 || length % 2 != 0 || length > parameters.len() {
                    return Err(ProtocolError::InvalidFrame {
                        details: "Invalid search request parameter length".to_owned(),
                    }
                    .into());
                }
                if parameters[1] & 0x80 != 0 && parameters[1] & 0x7f != 4 {
                    return Ok(());
                }
                parameters = &parameters[length..];
            }
        } else if !parameters.is_empty() {
            return Err(ProtocolError::InvalidFrame {
                details: "Unexpected data after discovery request endpoint".to_owned(),
            }
            .into());
        }

        let description = frame.header.service_type == ServiceType::DescriptionRequest;
        let (response, mut body) = if description {
            (ServiceType::DescriptionResponse, Vec::new())
        } else {
            (
                if extended {
                    ServiceType::SearchResponseExtended
                } else {
                    ServiceType::SearchResponse
                },
                Hpai::new(self.local_addr).serialize(),
            )
        };
        let mut device = [0; 54];
        device[..4].copy_from_slice(&[54, 1, 0x20, 0]);
        device[4..6].copy_from_slice(&u16::from(self.individual_address).to_be_bytes());
        let name = b"Knust TunnelServer";
        device[24..24 + name.len()].copy_from_slice(name);
        body.extend_from_slice(&device);

        #[cfg(feature = "secure")]
        let secure = self.secure_config.is_some();
        #[cfg(not(feature = "secure"))]
        let secure = false;
        let management = self.device_management_handler.read().await.is_some();
        let mut families = vec![0, 2, 2, 2, 4, 2];
        if management {
            families.extend_from_slice(&[3, 1]);
        }
        if extended && secure {
            families.extend_from_slice(&[9, 1]);
        }
        families[0] = families.len() as u8;
        body.extend_from_slice(&families);
        if extended && secure {
            if management {
                body.extend_from_slice(&[6, 6, 4, 2, 3, 1]);
            } else {
                body.extend_from_slice(&[4, 6, 4, 2]);
            }
        }

        link.send_frame(&KnxIpFrame::new(response, body).serialize())
            .await
    }
}
