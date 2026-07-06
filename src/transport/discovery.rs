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
    /// Create a new gateway scanner
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryError::NetworkError`] if the UDP socket can't be
    /// bound or configured for broadcast.
    pub async fn new() -> Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(DiscoveryError::NetworkError)?;

        socket
            .set_broadcast(true)
            .map_err(DiscoveryError::NetworkError)?;

        Ok(Self { socket })
    }

    /// Discover KNX/IP gateways on the network
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryError::NetworkError`] if sending the discovery
    /// broadcast fails or receiving fails with anything other than a
    /// timeout. Malformed responses from individual gateways are logged and
    /// skipped rather than returned as an error.
    pub async fn discover(&self, discovery_timeout: Duration) -> Result<Vec<GatewayInfo>> {
        let mut gateways = HashMap::new();

        // Build and send discovery request
        let discovery_request = Self::build_discovery_request();
        let broadcast_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), 3671);

        // Send discovery request
        if let Err(e) = self
            .socket
            .send_to(&discovery_request, broadcast_addr)
            .await
        {
            return Err(DiscoveryError::NetworkError(e).into());
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

    /// Build a KNX/IP discovery request frame
    fn build_discovery_request() -> Vec<u8> {
        // Build a proper KNX/IP search request frame according to specification
        let mut frame = Vec::new();

        // KNX/IP Header
        frame.extend_from_slice(&[
            0x06, 0x10, // Header length (6) and version (1.0)
            0x02, 0x01, // Search request service type
            0x00, 0x0E, // Total length (14 bytes)
        ]);

        // Discovery endpoint HPAI (Host Protocol Address Information)
        frame.extend_from_slice(&[
            0x08, 0x01, // Structure length (8) and host protocol code (UDP)
            0x00, 0x00, 0x00, 0x00, // IP address (0.0.0.0 = any)
            0x00, 0x00, // Port (0 = any)
        ]);

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
