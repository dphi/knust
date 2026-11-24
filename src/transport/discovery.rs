//! KNX/IP gateway discovery implementation.

use crate::error::{DiscoveryError, Result};
use crate::protocol::IndividualAddress;
use crate::protocol::knxip::{Hpai, KnxIpFrame, ServiceType as FrameType};
use std::collections::{BTreeMap, HashMap};
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

    /// Query the description of a gateway at a known UDP address.
    ///
    /// For Core v2 and newer, follow the description request with an extended
    /// search to fetch security and tunneling slot information. Each request
    /// has its own `response_timeout`.
    ///
    /// # Errors
    /// Returns a discovery error if the query fails, times out, or returns
    /// invalid description data.
    pub async fn describe(
        &self,
        gateway: SocketAddr,
        response_timeout: Duration,
    ) -> Result<GatewayInfo> {
        let interface = self
            .socket
            .local_addr()
            .map_err(DiscoveryError::NetworkError)?
            .ip();
        let socket = UdpSocket::bind(SocketAddr::new(interface, 0))
            .await
            .map_err(DiscoveryError::NetworkError)?;
        socket
            .connect(gateway)
            .await
            .map_err(DiscoveryError::NetworkError)?;
        let local_addr = socket.local_addr().map_err(DiscoveryError::NetworkError)?;
        let frame = Self::query_gateway(
            &socket,
            local_addr,
            FrameType::DescriptionRequest,
            FrameType::DescriptionResponse,
            response_timeout,
        )
        .await?;
        let info = Self::parse_gateway_dibs(&frame.body, gateway)?;
        if info.core_version < 2 {
            return Ok(info);
        }

        let frame = Self::query_gateway(
            &socket,
            local_addr,
            FrameType::SearchRequestExtended,
            FrameType::SearchResponseExtended,
            response_timeout,
        )
        .await?;
        let (_, dibs) = Self::search_response_parts(&frame.body, gateway)?;
        Self::parse_gateway_dibs(dibs, gateway)
    }

    async fn query_gateway(
        socket: &UdpSocket,
        local_addr: SocketAddr,
        request: FrameType,
        expected: FrameType,
        response_timeout: Duration,
    ) -> Result<KnxIpFrame> {
        let frame = Self::build_discovery_request(local_addr, request);
        timeout(response_timeout, async {
            socket
                .send(&frame)
                .await
                .map_err(DiscoveryError::NetworkError)?;
            let mut buf = [0; 4096];
            loop {
                let len = socket
                    .recv(&mut buf)
                    .await
                    .map_err(DiscoveryError::NetworkError)?;
                if let Ok(response) = KnxIpFrame::parse(&buf[..len])
                    && response.header.service_type == expected
                {
                    return Ok(response);
                }
            }
        })
        .await
        .map_err(|_| DiscoveryError::Timeout {
            timeout_ms: response_timeout.as_millis() as u64,
        })?
    }

    /// Discover KNX/IP gateways on the network
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryError::NetworkError`] if all search sends fail,
    /// or receiving fails with anything other than a
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
        let discovery_request = Self::build_discovery_request(local_addr, FrameType::SearchRequest);
        let extended_request =
            Self::build_discovery_request(local_addr, FrameType::SearchRequestExtended);
        let extended = self.socket.send_to(&extended_request, multicast_addr).await;
        let multicast = self
            .socket
            .send_to(&discovery_request, multicast_addr)
            .await;
        let broadcast = self
            .socket
            .send_to(&discovery_request, broadcast_addr)
            .await;
        if let (Err(extended_error), Err(multicast_error), Err(broadcast_error)) =
            (extended, multicast, broadcast)
        {
            log::debug!(
                "Multicast discovery failed: {multicast_error}; extended search failed: {extended_error}"
            );
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
                                let extended = buf[2..4]
                                    == (FrameType::SearchResponseExtended as u16).to_be_bytes();
                                if extended
                                    || gateways.get(&gateway_info.addr).is_none_or(
                                        |(previous_extended, _): &(bool, GatewayInfo)| {
                                            !*previous_extended
                                        },
                                    )
                                {
                                    gateways.insert(gateway_info.addr, (extended, gateway_info));
                                }
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
                    Ok(gateways.into_values().map(|(_, info)| info).collect())
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
                    Ok(gateways.into_values().map(|(_, info)| info).collect())
                }
            }
        }
    }

    /// Build a KNX/IP discovery request frame.
    fn build_discovery_request(local_addr: SocketAddr, kind: FrameType) -> Vec<u8> {
        let hpai = if local_addr.ip().is_unspecified() {
            Hpai::route_back()
        } else {
            Hpai::new(local_addr)
        };
        let mut body = hpai.serialize();
        if kind == FrameType::SearchRequestExtended {
            // Request Device Info, Supported Services, Secured Services and Tunneling Info.
            body.extend_from_slice(&[6, 4, 1, 2, 6, 7]);
        }
        KnxIpFrame::new(kind, body).serialize()
    }

    /// Parse a discovery response frame.
    ///
    /// # Errors
    /// Returns [`DiscoveryError::InvalidResponse`] for an invalid header, endpoint, or DIB.
    pub fn parse_discovery_response(&self, data: &[u8], source: SocketAddr) -> Result<GatewayInfo> {
        let invalid = |reason: String| DiscoveryError::InvalidResponse {
            addr: source.to_string(),
            reason,
        };
        let frame = KnxIpFrame::parse(data).map_err(|error| invalid(error.to_string()))?;
        if !matches!(
            frame.header.service_type,
            FrameType::SearchResponse | FrameType::SearchResponseExtended
        ) {
            return Err(invalid("Not a search response".to_owned()).into());
        }
        let (addr, dibs) = Self::search_response_parts(&frame.body, source)?;
        Self::parse_gateway_dibs(dibs, addr)
    }

    fn search_response_parts(body: &[u8], source: SocketAddr) -> Result<(SocketAddr, &[u8])> {
        let invalid = |reason: String| DiscoveryError::InvalidResponse {
            addr: source.to_string(),
            reason,
        };
        let endpoint = Hpai::parse(body).map_err(|error| invalid(error.to_string()))?;
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
        Ok((addr, &body[Hpai::LENGTH..]))
    }

    fn parse_gateway_dibs(mut dibs: &[u8], addr: SocketAddr) -> Result<GatewayInfo> {
        let invalid = |reason: String| DiscoveryError::InvalidResponse {
            addr: addr.to_string(),
            reason,
        };
        let mut name = format!("KNX Gateway {}", addr.ip());
        let mut individual_address = None;
        let mut core_version = 0;
        let mut supported_services = vec![ServiceType::Core];
        let mut device_serial = String::new();
        let mut mac_address = None;
        let mut multicast_ip = None;
        let mut tunneling_version = 0;
        let mut supports_secure = false;
        let mut secured_services = None;
        let mut tunneling_info = None;

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
                    individual_address = Some(IndividualAddress::from_raw(u16::from_be_bytes([
                        dib[4], dib[5],
                    ])));
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
                            0x02 => {
                                core_version = core_version.max(family[1]);
                                continue;
                            }
                            0x03 => ServiceType::DeviceManagement,
                            0x04 => {
                                tunneling_version = tunneling_version.max(family[1]);
                                ServiceType::Tunneling
                            }
                            0x05 => ServiceType::Routing,
                            0x06 => ServiceType::RemoteLogging,
                            0x07 => ServiceType::RemoteConfiguration,
                            0x08 => ServiceType::ObjectServer,
                            0x09 => {
                                supports_secure |= family[1] >= 1;
                                continue;
                            }
                            _ => continue,
                        };
                        if !supported_services.contains(&service) {
                            supported_services.push(service);
                        }
                    }
                }
                0x06 => secured_services = Some(&dib[2..]),
                0x07 => {
                    if dib.len() % 4 != 0 {
                        return Err(
                            invalid("Invalid tunneling information DIB length".to_owned()).into(),
                        );
                    }
                    let slots = dib[4..]
                        .chunks_exact(4)
                        .map(|slot| {
                            let address =
                                IndividualAddress::from_raw(u16::from_be_bytes([slot[0], slot[1]]));
                            let status = slot[3];
                            (
                                address,
                                TunnelingSlotStatus {
                                    usable: status & 4 != 0,
                                    authorized: status & 2 != 0,
                                    free: status & 1 != 0,
                                },
                            )
                        })
                        .collect();
                    tunneling_info = Some(GatewayTunnelingInfo {
                        max_apdu_length: u16::from_be_bytes([dib[2], dib[3]]),
                        slots,
                    });
                }
                _ => {}
            }
        }

        let requires_secure = |family| {
            secured_services
                .map(|services| services.chunks_exact(2).any(|entry| entry[0] == family))
        };
        let supports_tunneling = supported_services.contains(&ServiceType::Tunneling);
        let supports_routing = supported_services.contains(&ServiceType::Routing);
        let capabilities = GatewayCapabilities {
            supports_tunneling,
            supports_routing,
            supports_device_management: supported_services.contains(&ServiceType::DeviceManagement),
            max_tunneling_connections: tunneling_info
                .as_ref()
                .map_or(if supports_tunneling { 4 } else { 1 }, |info| {
                    info.slots.len() as u8
                }),
            supports_tcp_tunneling: tunneling_version >= 2,
            supports_secure,
            tunneling_requires_secure: requires_secure(0x04),
            routing_requires_secure: requires_secure(0x05),
        };
        let multicast_addr = multicast_ip
            .filter(|_| supports_routing)
            .map(|ip| SocketAddr::new(IpAddr::V4(ip), 3671));

        Ok(GatewayInfo {
            addr,
            name,
            individual_address,
            core_version,
            tunneling_info,
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

    /// KNX individual address reported by the device information block.
    pub individual_address: Option<IndividualAddress>,

    /// Advertised Core service version, or zero when absent.
    pub core_version: u8,

    /// Per-address tunnel slot information when advertised.
    pub tunneling_info: Option<GatewayTunnelingInfo>,

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

/// Tunnel limits and availability reported by the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayTunnelingInfo {
    /// Maximum APDU length supported by the gateway.
    pub max_apdu_length: u16,
    /// Status of each advertised tunneling address.
    pub slots: BTreeMap<IndividualAddress, TunnelingSlotStatus>,
}

/// Status flags for an individual tunneling slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TunnelingSlotStatus {
    pub usable: bool,
    pub authorized: bool,
    pub free: bool,
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

    /// The gateway advertises Tunneling service version 2 or later.
    pub supports_tcp_tunneling: bool,

    /// The gateway advertises the KNX IP Security service.
    pub supports_secure: bool,

    /// Whether the gateway requires IP Secure for tunneling, if reported.
    pub tunneling_requires_secure: Option<bool>,

    /// Whether the gateway requires IP Secure for routing, if reported.
    pub routing_requires_secure: Option<bool>,
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
            let mut extended_requests = 0;
            let mut source = None;
            for _ in 0..3 {
                let (len, from) =
                    tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buf))
                        .await
                        .unwrap()
                        .unwrap();
                source = Some(from);
                assert_eq!(&buf[6..12], &[8, 1, 127, 0, 0, 1]);
                assert_eq!(u16::from_be_bytes([buf[12], buf[13]]), expected_port);
                match buf[3] {
                    1 => assert_eq!(
                        &buf[..len],
                        &[6, 0x10, 2, 1, 0, 14, 8, 1, 127, 0, 0, 1, buf[12], buf[13]]
                    ),
                    0x0b => {
                        extended_requests += 1;
                        assert_eq!(&buf[..6], &[6, 0x10, 2, 0x0b, 0, 20]);
                        assert_eq!(&buf[14..len], &[6, 4, 1, 2, 6, 7]);
                    }
                    other => panic!("unexpected search service {other}"),
                }
            }
            assert_eq!(extended_requests, 1);
            let source = source.unwrap();
            peer.send_to(
                &[
                    6, 0x10, 2, 0x0c, 0, 40, 8, 1, 127, 0, 0, 1, 0, 0, 10, 2, 2, 2, 4, 2, 5, 1, 9,
                    1, 4, 6, 4, 2, 12, 7, 0, 248, 0x11, 1, 0, 5, 0x11, 2, 0, 2,
                ],
                source,
            )
            .await
            .unwrap();
            peer.send_to(&[6, 0x10, 2, 2, 0, 14, 8, 1, 127, 0, 0, 1, 0, 0], source)
                .await
                .unwrap();
        });
        let gateways = scanner
            .discover_to(Duration::from_millis(250), multicast, unicast)
            .await
            .unwrap();
        responder.await.unwrap();
        let [gateway] = &gateways[..] else {
            panic!("expected one gateway, found {gateways:?}");
        };
        let caps = &gateway.capabilities;
        assert!(caps.supports_tcp_tunneling && caps.supports_secure);
        assert_eq!(caps.tunneling_requires_secure, Some(true));
        assert_eq!(caps.routing_requires_secure, Some(false));
        assert_eq!(caps.max_tunneling_connections, 2);
        assert_eq!(gateway.core_version, 2);
        let info = gateway.tunneling_info.as_ref().unwrap();
        assert_eq!(info.max_apdu_length, 248);
        assert_eq!(
            info.slots[&IndividualAddress::new(1, 1, 1)],
            TunnelingSlotStatus {
                usable: true,
                authorized: false,
                free: true,
            }
        );
        assert_eq!(
            info.slots[&IndividualAddress::new(1, 1, 2)],
            TunnelingSlotStatus {
                usable: false,
                authorized: true,
                free: false,
            }
        );
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

    #[tokio::test]
    async fn unicast_description_selects_query_by_core_version() {
        let scanner = GatewayScanner::new_with_interface(Ipv4Addr::LOCALHOST)
            .await
            .unwrap();
        for core_version in [1, 2] {
            let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let gateway = peer.local_addr().unwrap();
            let responder = tokio::spawn(async move {
                let mut buf = [0; 64];
                let (len, source) = peer.recv_from(&mut buf).await.unwrap();
                assert_eq!(len, 14);
                assert_eq!(&buf[..8], &[6, 0x10, 2, 3, 0, 14, 8, 1]);
                assert_eq!(&buf[8..12], &[127, 0, 0, 1]);
                assert_eq!(u16::from_be_bytes([buf[12], buf[13]]), source.port());

                // An unrelated search response from the gateway must not satisfy the query.
                peer.send_to(&[6, 0x10, 2, 2, 0, 14, 8, 1, 0, 0, 0, 0, 0, 0], source)
                    .await
                    .unwrap();
                peer.send_to(&[6, 0x10, 2, 4, 0, 10, 4, 2, 2, core_version], source)
                    .await
                    .unwrap();
                if core_version == 1 {
                    assert!(
                        timeout(Duration::from_millis(100), peer.recv_from(&mut buf))
                            .await
                            .is_err()
                    );
                    return;
                }

                let (len, second_source) = peer.recv_from(&mut buf).await.unwrap();
                assert_eq!(source, second_source);
                assert_eq!(&buf[..6], &[6, 0x10, 2, 0x0b, 0, 20]);
                assert_eq!(&buf[14..len], &[6, 4, 1, 2, 6, 7]);
                peer.send_to(
                    &[
                        6, 0x10, 2, 0x0c, 0, 30, 8, 1, 192, 0, 2, 1, 0, 0, 8, 2, 2, 2, 4, 2, 9, 1,
                        8, 7, 0, 248, 0x11, 1, 0, 7,
                    ],
                    source,
                )
                .await
                .unwrap();
            });

            let info = scanner
                .describe(gateway, Duration::from_secs(1))
                .await
                .unwrap();
            assert_eq!(info.addr, gateway);
            assert_eq!(info.core_version, core_version);
            assert_eq!(info.capabilities.supports_tcp_tunneling, core_version == 2);
            assert_eq!(
                info.tunneling_info.map(|tunneling| tunneling.slots.len()),
                (core_version == 2).then_some(1),
            );
            responder.await.unwrap();
        }
    }

    #[test]
    fn wildcard_discovery_uses_route_back_endpoint() {
        let request = GatewayScanner::build_discovery_request(
            "0.0.0.0:1234".parse().unwrap(),
            FrameType::SearchRequest,
        );
        assert_eq!(&request[8..], &[0; 6]);
    }
}
