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
                                gateways.insert(addr, gateway_info);
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

    /// Parse a discovery response frame
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryError::InvalidResponse`] if `data` is too short or
    /// doesn't match the expected KNX/IP search-response structure.
    pub fn parse_discovery_response(&self, data: &[u8], addr: SocketAddr) -> Result<GatewayInfo> {
        if data.len() < 6 {
            return Err(DiscoveryError::InvalidResponse {
                addr: addr.to_string(),
                reason: "Response too short".to_string(),
            }
            .into());
        }

        // Verify KNX/IP header
        if data[0] != 0x06 || data[1] != 0x10 {
            return Err(DiscoveryError::InvalidResponse {
                addr: addr.to_string(),
                reason: "Invalid KNX/IP header".to_string(),
            }
            .into());
        }

        // Check service type (should be search response 0x0202)
        if data.len() >= 4 && (data[2] != 0x02 || data[3] != 0x02) {
            return Err(DiscoveryError::InvalidResponse {
                addr: addr.to_string(),
                reason: "Not a search response".to_string(),
            }
            .into());
        }

        // Extract total length
        let total_length = if data.len() >= 6 {
            (u16::from(data[4]) << 8) | u16::from(data[5])
        } else {
            data.len() as u16
        };

        if (total_length as usize) > data.len() {
            return Err(DiscoveryError::InvalidResponse {
                addr: addr.to_string(),
                reason: format!(
                    "Frame length mismatch: expected {}, got {}",
                    total_length,
                    data.len()
                ),
            }
            .into());
        }

        // Parse device information from the frame
        let mut name = format!("KNX Gateway {}", addr.ip());
        let mut capabilities = GatewayCapabilities {
            supports_tunneling: false,
            supports_routing: false,
            supports_device_management: false,
            max_tunneling_connections: 1,
        };
        let mut supported_services = vec![ServiceType::Core];
        let mut device_serial = format!("SN{:08X}", addr.ip().to_string().len());

        // Try to parse DIBs (Device Information Blocks)
        let mut offset = 14; // Skip header and control endpoint HPAI

        while offset < data.len() {
            if offset + 2 > data.len() {
                break;
            }

            let dib_length = data[offset] as usize;
            let dib_type = data[offset + 1];

            if dib_length < 2 || offset + dib_length > data.len() {
                break;
            }

            match dib_type {
                0x01 => {
                    // Device hardware DIB
                    if dib_length >= 54 && offset + 54 <= data.len() {
                        // Extract serial number (6 bytes starting at offset + 12)
                        if offset + 18 <= data.len() {
                            let serial_bytes = &data[offset + 12..offset + 18];
                            device_serial = format!(
                                "SN{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                                serial_bytes[0],
                                serial_bytes[1],
                                serial_bytes[2],
                                serial_bytes[3],
                                serial_bytes[4],
                                serial_bytes[5]
                            );
                        }

                        // Extract friendly name (30 bytes starting at offset + 24)
                        if offset + 54 <= data.len() {
                            let name_bytes = &data[offset + 24..offset + 54];
                            if let Some(null_pos) = name_bytes.iter().position(|&b| b == 0)
                                && let Ok(parsed_name) =
                                    std::str::from_utf8(&name_bytes[..null_pos])
                                && !parsed_name.is_empty()
                            {
                                name = parsed_name.to_string();
                            }
                        }
                    }
                }
                0x02 => {
                    // Supported service families DIB
                    let mut services_offset = offset + 2;
                    supported_services.clear();
                    supported_services.push(ServiceType::Core); // Always include core

                    while services_offset + 2 <= offset + dib_length {
                        let service_family = data[services_offset];
                        // data[services_offset + 1] is the service version, unused here.

                        match service_family {
                            0x03 => {
                                capabilities.supports_device_management = true;
                                supported_services.push(ServiceType::DeviceManagement);
                            }
                            0x04 => {
                                capabilities.supports_tunneling = true;
                                capabilities.max_tunneling_connections = 4; // Default assumption
                                supported_services.push(ServiceType::Tunneling);
                            }
                            0x05 => {
                                capabilities.supports_routing = true;
                                supported_services.push(ServiceType::Routing);
                            }
                            // 0x02 (Core) is pre-added above; unknown families ignored.
                            _ => {}
                        }

                        services_offset += 2;
                    }
                }
                _ => {
                    // Unknown DIB type, skip
                }
            }

            offset += dib_length;
        }

        // If no services were parsed from DIB, use defaults based on common gateway capabilities
        if supported_services.len() == 1 {
            // Only Core service
            capabilities.supports_tunneling = true;
            capabilities.supports_device_management = true;
            capabilities.max_tunneling_connections = 4;
            supported_services
                .extend_from_slice(&[ServiceType::DeviceManagement, ServiceType::Tunneling]);
        }

        // Determine multicast address before moving capabilities
        let multicast_addr = if capabilities.supports_routing {
            Some(SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(224, 0, 23, 12)),
                3671,
            ))
        } else {
            None
        };

        Ok(GatewayInfo {
            addr,
            name,
            capabilities,
            supported_services,
            device_serial,
            mac_address: None,
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

    #[test]
    fn wildcard_discovery_uses_route_back_endpoint() {
        let request = GatewayScanner::build_discovery_request("0.0.0.0:1234".parse().unwrap());
        assert_eq!(&request[8..], &[0; 6]);
    }
}
