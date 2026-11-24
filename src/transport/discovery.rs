//! KNX/IP gateway discovery implementation.

use crate::error::{DiscoveryError, Result};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;

/// Gateway discovery scanner
pub struct GatewayScanner {
    socket: UdpSocket,
}

impl GatewayScanner {
    /// Create a gateway scanner using the default network interface.
    ///
    /// # Errors
    /// Returns [`DiscoveryError::NetworkError`] if the socket cannot be configured.
    pub async fn new() -> Result<Self> {
        Self::new_with_interface(Ipv4Addr::UNSPECIFIED).await
    }

    /// Create a gateway scanner using the specified local IPv4 interface.
    ///
    /// # Errors
    /// Returns [`DiscoveryError::NetworkError`] if the socket cannot be configured.
    pub async fn new_with_interface(interface: Ipv4Addr) -> Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(interface), 0))
            .await
            .map_err(DiscoveryError::NetworkError)?;
        socket
            .set_broadcast(true)
            .map_err(DiscoveryError::NetworkError)?;
        let socket_ref = socket2::SockRef::from(&socket);
        socket_ref
            .set_multicast_ttl_v4(16)
            .map_err(DiscoveryError::NetworkError)?;
        socket_ref
            .set_multicast_if_v4(&interface)
            .map_err(DiscoveryError::NetworkError)?;
        Ok(Self { socket })
    }

    /// Discover KNX/IP gateways on the network
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryError::NetworkError`] if both multicast and broadcast
    /// sends fail, or receiving fails with anything other than a
    /// timeout. Malformed responses from individual gateways are logged and
    /// skipped rather than returned as an error.
    pub async fn discover(&self, discovery_timeout: Duration) -> Result<Vec<GatewayInfo>> {
        let multicast = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(224, 0, 23, 12)), 3671);
        let broadcast = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), 3671);
        self.discover_to(discovery_timeout, multicast, broadcast)
            .await
    }

    async fn discover_to(
        &self,
        discovery_timeout: Duration,
        multicast_addr: SocketAddr,
        broadcast_addr: SocketAddr,
    ) -> Result<Vec<GatewayInfo>> {
        let mut gateways = HashMap::new();

        // Build and send discovery request
        let local_addr = self
            .socket
            .local_addr()
            .map_err(DiscoveryError::NetworkError)?;
        let discovery_request = Self::build_discovery_request(local_addr);
        let multicast = self
            .socket
            .send_to(&discovery_request, multicast_addr)
            .await;
        let broadcast = self
            .socket
            .send_to(&discovery_request, broadcast_addr)
            .await;
        if let (Err(multicast_error), Err(broadcast_error)) = (multicast, broadcast) {
            log::debug!("Multicast discovery failed: {multicast_error}");
            return Err(DiscoveryError::NetworkError(broadcast_error).into());
        }

        // Collect responses with timeout
        let result = timeout(discovery_timeout, async {
            let mut buf = vec![0u8; 1024];
            let mut last_error: Option<DiscoveryError> = None;

            loop {
                match self.socket.recv_from(&mut buf).await {
                    Ok((len, addr)) => {
                        match self.parse_discovery_response(&buf[..len], addr) {
                            Ok(gateway_info) => {
                                gateways.insert(gateway_info.addr, gateway_info);
                            }
                            Err(e) => {
                                // Log parsing errors but continue discovery
                                log::debug!("Failed to parse discovery response from {addr}: {e}");
                                if let crate::error::KnxError::Discovery(disc_err) = e {
                                    last_error = Some(disc_err);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // Check if this is a timeout or a real error
                        match e.kind() {
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                                // This is expected when no more responses are coming
                                break;
                            }
                            _ => {
                                // Real network error
                                last_error = Some(DiscoveryError::NetworkError(e));
                                break;
                            }
                        }
                    }
                }
            }

            // Return any parsing errors if no gateways were found
            if gateways.is_empty()
                && let Some(err) = last_error
            {
                return Err(err);
            }

            Ok(())
        })
        .await;

        match result {
            Ok(Ok(())) => {
                if gateways.is_empty() {
                    Err(DiscoveryError::NoGatewaysFound.into())
                } else {
                    Ok(gateways.into_values().collect())
                }
            }
            Ok(Err(e)) => Err(e.into()),
            Err(_) => {
                // Timeout occurred
                if gateways.is_empty() {
                    Err(DiscoveryError::Timeout {
                        timeout_ms: discovery_timeout.as_millis() as u64,
                    }
                    .into())
                } else {
                    // Return partial results even on timeout
                    Ok(gateways.into_values().collect())
                }
            }
        }
    }

    /// Build a KNX/IP discovery request frame.
    fn build_discovery_request(local_addr: SocketAddr) -> Vec<u8> {
        let mut frame = vec![0x06, 0x10, 0x02, 0x01, 0x00, 0x0e, 0x08, 0x01];
        if let SocketAddr::V4(addr) = local_addr {
            if !addr.ip().is_unspecified() {
                frame.extend_from_slice(&addr.ip().octets());
                frame.extend_from_slice(&addr.port().to_be_bytes());
                return frame;
            }
        }
        frame.extend_from_slice(&[0; 6]);
        frame
    }

    /// Parse a discovery response frame.
    ///
    /// # Errors
    /// Returns [`DiscoveryError::InvalidResponse`] for an invalid header, endpoint, or DIB.
    pub fn parse_discovery_response(&self, data: &[u8], source: SocketAddr) -> Result<GatewayInfo> {
        use crate::protocol::knxip::{Hpai, KnxIpFrame, ServiceType as FrameType};

        let invalid = |reason: String| DiscoveryError::InvalidResponse {
            addr: source.to_string(),
            reason,
        };
        let frame = KnxIpFrame::parse(data).map_err(|error| invalid(error.to_string()))?;
        if frame.header.service_type != FrameType::SearchResponse {
            return Err(invalid("Not a search response".to_owned()).into());
        }
        let endpoint = Hpai::parse(&frame.body).map_err(|error| invalid(error.to_string()))?;
        if endpoint.host_protocol_code != Hpai::PROTOCOL_UDP {
            return Err(invalid("Search response requires a UDP endpoint".to_owned()).into());
        }
        let addr = SocketAddr::new(
            if endpoint.ip_addr.is_unspecified() {
                source.ip()
            } else {
                endpoint.ip_addr
            },
            if endpoint.port == 0 {
                source.port()
            } else {
                endpoint.port
            },
        );
        let mut name = format!("KNX Gateway {}", addr.ip());
        let mut supported_services = vec![ServiceType::Core];
        let mut device_serial = String::new();
        let mut mac_address = None;
        let mut multicast_ip = None;
        let mut dibs = &frame.body[Hpai::LENGTH..];

        while let Some(&length) = dibs.first() {
            let length = usize::from(length);
            if length < 2 || length % 2 != 0 || length > dibs.len() {
                return Err(invalid(format!("Invalid DIB length {length}")).into());
            }
            let (dib, remaining) = dibs.split_at(length);
            dibs = remaining;
            match dib[1] {
                0x01 => {
                    if dib.len() != 54 {
                        return Err(invalid(
                            "Device information DIB must contain 54 bytes".to_owned(),
                        )
                        .into());
                    }
                    device_serial = format!("SN{}", hex::encode_upper(&dib[8..14]));
                    let ip = Ipv4Addr::new(dib[14], dib[15], dib[16], dib[17]);
                    multicast_ip = ip.is_multicast().then_some(ip);
                    mac_address = Some(
                        dib[18..24]
                            .iter()
                            .map(|byte| format!("{byte:02X}"))
                            .collect::<Vec<_>>()
                            .join(":"),
                    );
                    let device_name: String = dib[24..54]
                        .iter()
                        .copied()
                        .take_while(|&byte| byte != 0)
                        .map(char::from)
                        .collect();
                    if !device_name.is_empty() {
                        name = device_name;
                    }
                }
                0x02 => {
                    for family in dib[2..].chunks_exact(2) {
                        let service = match family[0] {
                            0x03 => ServiceType::DeviceManagement,
                            0x04 => ServiceType::Tunneling,
                            0x05 => ServiceType::Routing,
                            0x06 => ServiceType::RemoteLogging,
                            0x07 => ServiceType::RemoteConfiguration,
                            0x08 => ServiceType::ObjectServer,
                            _ => continue,
                        };
                        if !supported_services.contains(&service) {
                            supported_services.push(service);
                        }
                    }
                }
                _ => {}
            }
        }

        let supports_tunneling = supported_services.contains(&ServiceType::Tunneling);
        let supports_routing = supported_services.contains(&ServiceType::Routing);
        let capabilities = GatewayCapabilities {
            supports_tunneling,
            supports_routing,
            supports_device_management: supported_services.contains(&ServiceType::DeviceManagement),
            max_tunneling_connections: if supports_tunneling { 4 } else { 1 },
        };
        let multicast_addr = multicast_ip
            .filter(|_| supports_routing)
            .map(|ip| SocketAddr::new(IpAddr::V4(ip), 3671));

        Ok(GatewayInfo {
            addr,
            name,
            capabilities,
            supported_services,
            device_serial,
            mac_address,
            multicast_addr,
        })
    }
}

/// Information about a discovered KNX/IP gateway
#[derive(Debug, Clone)]
pub struct GatewayInfo {
    /// Gateway network address
    pub addr: SocketAddr,

    /// Gateway device name
    pub name: String,

    /// Gateway capabilities
    pub capabilities: GatewayCapabilities,

    /// Supported service types
    pub supported_services: Vec<ServiceType>,

    /// Device serial number
    pub device_serial: String,

    /// MAC address (if available)
    pub mac_address: Option<String>,

    /// Multicast address for routing (if supported)
    pub multicast_addr: Option<SocketAddr>,
}

/// Gateway capability information
#[derive(Debug, Clone)]
pub struct GatewayCapabilities {
    /// Supports KNX/IP tunneling
    pub supports_tunneling: bool,

    /// Supports KNX/IP routing
    pub supports_routing: bool,

    /// Supports device management
    pub supports_device_management: bool,

    /// Maximum number of concurrent tunneling connections
    pub max_tunneling_connections: u8,
}

/// KNX/IP service types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceType {
    /// Core services
    Core,

    /// Device management
    DeviceManagement,

    /// Tunneling
    Tunneling,

    /// Routing
    Routing,

    /// Remote logging
    RemoteLogging,

    /// Remote configuration
    RemoteConfiguration,

    /// Object server
    ObjectServer,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discovery_sends_to_configured_interface_and_receives_reply() {
        let scanner = GatewayScanner::new_with_interface(Ipv4Addr::LOCALHOST)
            .await
            .unwrap();
        let options = socket2::SockRef::from(&scanner.socket);
        assert_eq!(options.multicast_if_v4().unwrap(), Ipv4Addr::LOCALHOST);
        assert_eq!(options.multicast_ttl_v4().unwrap(), 16);

        let peer = UdpSocket::bind("0.0.0.0:0").await.unwrap();
        let group = Ipv4Addr::new(224, 0, 23, 12);
        peer.join_multicast_v4(group, Ipv4Addr::LOCALHOST).unwrap();
        let multicast = SocketAddr::new(IpAddr::V4(group), peer.local_addr().unwrap().port());
        let unicast = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            peer.local_addr().unwrap().port(),
        );
        let expected_port = scanner.socket.local_addr().unwrap().port();
        let responder = tokio::spawn(async move {
            let mut buf = [0; 128];
            for _ in 0..2 {
                let (len, source) =
                    tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buf))
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(&buf[..8], &[6, 0x10, 2, 1, 0, 14, 8, 1]);
                assert_eq!(&buf[8..12], &[127, 0, 0, 1]);
                assert_eq!(u16::from_be_bytes([buf[12], buf[13]]), expected_port);
                assert_eq!(len, 14);
                peer.send_to(&[6, 0x10, 2, 2, 0, 14, 8, 1, 127, 0, 0, 1, 0, 0], source)
                    .await
                    .unwrap();
            }
        });
        let gateways = scanner
            .discover_to(Duration::from_millis(250), multicast, unicast)
            .await
            .unwrap();
        responder.await.unwrap();
        assert_eq!(gateways.len(), 1);
    }

    #[tokio::test]
    async fn discovery_continues_when_one_send_fails() {
        let scanner = GatewayScanner::new_with_interface(Ipv4Addr::LOCALHOST)
            .await
            .unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();
        let responder = tokio::spawn(async move {
            let mut bytes = [0; 64];
            let (_, source) = peer.recv_from(&mut bytes).await.unwrap();
            peer.send_to(&[6, 0x10, 2, 2, 0, 14, 8, 1, 127, 0, 0, 1, 0, 0], source)
                .await
                .unwrap();
        });
        let gateways = scanner
            .discover_to(
                Duration::from_millis(250),
                "[::1]:1234".parse().unwrap(),
                peer_addr,
            )
            .await
            .unwrap();
        responder.await.unwrap();
        assert_eq!(gateways.len(), 1);
    }

    #[tokio::test]
    async fn discovery_rejects_malformed_dibs_and_datagram_lengths() {
        let scanner = GatewayScanner::new_with_interface(Ipv4Addr::LOCALHOST)
            .await
            .unwrap();
        let source = "127.0.0.1:13671".parse().unwrap();
        let header_and_endpoint = [6, 0x10, 2, 2, 0, 14, 8, 1, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            scanner
                .parse_discovery_response(&header_and_endpoint, source)
                .unwrap()
                .addr,
            source,
        );

        for malformed in [&[0, 1][..], &[3, 2, 3], &[4, 2], &[2, 1], &[2]] {
            let mut response = header_and_endpoint.to_vec();
            response.extend_from_slice(malformed);
            response[5] = response.len() as u8;
            assert!(scanner.parse_discovery_response(&response, source).is_err());
        }
        let mut response = header_and_endpoint.to_vec();
        response.extend_from_slice(&[2, 0xf0]);
        assert!(scanner.parse_discovery_response(&response, source).is_err());
        response[5] = response.len() as u8;
        assert!(scanner.parse_discovery_response(&response, source).is_ok());
    }

    #[test]
    fn wildcard_discovery_uses_route_back_endpoint() {
        let request = GatewayScanner::build_discovery_request("0.0.0.0:1234".parse().unwrap());
        assert_eq!(&request[8..], &[0; 6]);
    }
}
