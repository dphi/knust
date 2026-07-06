//! KNX/IP protocol message structures and handling.
//!
//! This module implements the KNX/IP protocol messages used for connection
//! establishment, data transmission, and connection management.

use crate::error::{ProtocolError, Result};
use crate::log_protocol;
use crate::logging::LogLevel;
use crate::protocol::address::IndividualAddress;
use std::net::{IpAddr, SocketAddr};

/// KNX/IP service types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ServiceType {
    SearchRequest = 0x0201,
    SearchResponse = 0x0202,
    DescriptionRequest = 0x0203,
    DescriptionResponse = 0x0204,
    ConnectRequest = 0x0205,
    ConnectResponse = 0x0206,
    ConnectionstateRequest = 0x0207,
    ConnectionstateResponse = 0x0208,
    DisconnectRequest = 0x0209,
    DisconnectResponse = 0x020A,
    DeviceConfigurationRequest = 0x0310,
    DeviceConfigurationAck = 0x0311,
    TunnellingRequest = 0x0420,
    TunnellingAck = 0x0421,
    RoutingIndication = 0x0530,
    RoutingLostMessage = 0x0531,
    RoutingBusy = 0x0532,
    SecureWrapper = 0x0950,
    SessionRequest = 0x0951,
    SessionResponse = 0x0952,
    SessionAuthenticate = 0x0953,
    SessionStatus = 0x0954,
}

impl ServiceType {
    #[must_use]
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0x0201 => Some(Self::SearchRequest),
            0x0202 => Some(Self::SearchResponse),
            0x0203 => Some(Self::DescriptionRequest),
            0x0204 => Some(Self::DescriptionResponse),
            0x0205 => Some(Self::ConnectRequest),
            0x0206 => Some(Self::ConnectResponse),
            0x0207 => Some(Self::ConnectionstateRequest),
            0x0208 => Some(Self::ConnectionstateResponse),
            0x0209 => Some(Self::DisconnectRequest),
            0x020A => Some(Self::DisconnectResponse),
            0x0310 => Some(Self::DeviceConfigurationRequest),
            0x0311 => Some(Self::DeviceConfigurationAck),
            0x0420 => Some(Self::TunnellingRequest),
            0x0421 => Some(Self::TunnellingAck),
            0x0530 => Some(Self::RoutingIndication),
            0x0531 => Some(Self::RoutingLostMessage),
            0x0532 => Some(Self::RoutingBusy),
            0x0950 => Some(Self::SecureWrapper),
            0x0951 => Some(Self::SessionRequest),
            0x0952 => Some(Self::SessionResponse),
            0x0953 => Some(Self::SessionAuthenticate),
            0x0954 => Some(Self::SessionStatus),
            _ => None,
        }
    }
}

/// KNX/IP header structure
#[derive(Debug, Clone)]
pub struct KnxIpHeader {
    pub header_length: u8,
    pub protocol_version: u8,
    pub service_type: ServiceType,
    pub total_length: u16,
}

impl KnxIpHeader {
    pub const LENGTH: usize = 6;

    #[must_use]
    pub fn new(service_type: ServiceType, body_length: u16) -> Self {
        Self {
            header_length: Self::LENGTH as u8,
            protocol_version: 0x10, // KNX/IP version 1.0
            service_type,
            total_length: Self::LENGTH as u16 + body_length,
        }
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than
    /// [`Self::LENGTH`], the declared header length field is wrong, or the
    /// service type is unrecognized; returns
    /// [`ProtocolError::UnsupportedVersion`] if the protocol version isn't `0x10`.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            log_protocol!(
                LogLevel::Warn,
                "KNX/IP header too short: {} bytes",
                data.len()
            );
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Header too short: {} bytes", data.len()),
            }
            .into());
        }

        let header_length = data[0];
        let protocol_version = data[1];
        let service_type_raw = u16::from_be_bytes([data[2], data[3]]);
        let total_length = u16::from_be_bytes([data[4], data[5]]);

        if header_length != Self::LENGTH as u8 {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Invalid header length: {header_length}"),
            }
            .into());
        }

        if protocol_version != 0x10 {
            log_protocol!(
                LogLevel::Warn,
                "KNX/IP unsupported version: 0x{:02X}",
                protocol_version
            );
            return Err(ProtocolError::UnsupportedVersion {
                version: protocol_version,
            }
            .into());
        }

        let Some(service_type) = ServiceType::from_u16(service_type_raw) else {
            log_protocol!(
                LogLevel::Warn,
                "KNX/IP unknown service type: 0x{:04X}",
                service_type_raw
            );
            return Err(ProtocolError::ParseError {
                offset: 2,
                reason: format!("Unknown service type: 0x{service_type_raw:04X}"),
            }
            .into());
        };

        log_protocol!(
            LogLevel::Trace,
            "KNX/IP header: service=0x{:04X} ({:?}) total_len={}",
            service_type_raw,
            service_type,
            total_length
        );

        Ok(Self {
            header_length,
            protocol_version,
            service_type,
            total_length,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::LENGTH);
        data.push(self.header_length);
        data.push(self.protocol_version);
        data.extend_from_slice(&(self.service_type as u16).to_be_bytes());
        data.extend_from_slice(&self.total_length.to_be_bytes());
        data
    }
}

/// Host Protocol Address Information (HPAI)
#[derive(Debug, Clone)]
pub struct Hpai {
    pub host_protocol_code: u8,
    pub ip_addr: IpAddr,
    pub port: u16,
}

impl Hpai {
    pub const LENGTH: usize = 8;
    pub const PROTOCOL_UDP: u8 = 0x01;
    pub const PROTOCOL_TCP: u8 = 0x02;

    #[must_use]
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            host_protocol_code: Self::PROTOCOL_UDP, // Default to UDP
            ip_addr: addr.ip(),
            port: addr.port(),
        }
    }

    /// UDP route-back HPAI (0.0.0.0:0): tells the gateway to reply to the
    /// source address of the received packet. Required when the client reaches
    /// the gateway across NAT/routing and cannot advertise a directly-reachable IP.
    #[must_use]
    pub fn route_back() -> Self {
        Self {
            host_protocol_code: Self::PROTOCOL_UDP,
            ip_addr: IpAddr::V4([0, 0, 0, 0].into()),
            port: 0,
        }
    }

    #[must_use]
    pub fn new_tcp_route_back() -> Self {
        Self {
            host_protocol_code: Self::PROTOCOL_TCP,
            ip_addr: IpAddr::V4([0, 0, 0, 0].into()),
            port: 0,
        }
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than
    /// [`Self::LENGTH`] or the declared structure length field is wrong.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("HPAI too short: {} bytes", data.len()),
            }
            .into());
        }

        let structure_length = data[0];
        let host_protocol_code = data[1];
        let ip_bytes = [data[2], data[3], data[4], data[5]];
        let port = u16::from_be_bytes([data[6], data[7]]);

        if structure_length != Self::LENGTH as u8 {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Invalid HPAI length: {structure_length}"),
            }
            .into());
        }

        let ip_addr = IpAddr::V4(ip_bytes.into());

        Ok(Self {
            host_protocol_code,
            ip_addr,
            port,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::LENGTH);
        data.push(Self::LENGTH as u8);
        data.push(self.host_protocol_code);

        match self.ip_addr {
            IpAddr::V4(ip) => data.extend_from_slice(&ip.octets()),
            IpAddr::V6(_) => {
                // For now, only support IPv4
                data.extend_from_slice(&[0, 0, 0, 0]);
            }
        }

        data.extend_from_slice(&self.port.to_be_bytes());
        data
    }

    #[must_use]
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip_addr, self.port)
    }
}

/// Connection Request Information (CRI)
#[derive(Debug, Clone)]
pub struct ConnectionRequestInfo {
    pub connection_type: u8,
    pub knx_layer: u8,
    pub reserved: u8,
    /// Optional individual address to request from the gateway. When set, the
    /// CRI is serialized in the 6-byte extended form carrying this address.
    pub individual_address: Option<IndividualAddress>,
}

impl ConnectionRequestInfo {
    pub const LENGTH: usize = 4;
    /// Length of the extended CRI carrying a requested individual address.
    pub const EXTENDED_LENGTH: usize = 6;
    pub const TUNNEL_CONNECTION: u8 = 0x04;
    pub const TUNNEL_LINKLAYER: u8 = 0x02;

    #[must_use]
    pub fn new_tunnel() -> Self {
        Self {
            connection_type: Self::TUNNEL_CONNECTION,
            knx_layer: Self::TUNNEL_LINKLAYER,
            reserved: 0,
            individual_address: None,
        }
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than
    /// [`Self::LENGTH`] or the declared structure length field is wrong.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("CRI too short: {} bytes", data.len()),
            }
            .into());
        }

        let structure_length = data[0];
        let connection_type = data[1];
        let knx_layer = data[2];
        let reserved = data[3];

        if structure_length != Self::LENGTH as u8 {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Invalid CRI length: {structure_length}"),
            }
            .into());
        }

        Ok(Self {
            connection_type,
            knx_layer,
            reserved,
            individual_address: None,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        if let Some(addr) = self.individual_address {
            log_protocol!(
                LogLevel::Debug,
                "ConnectRequest: requesting address={}",
                addr
            );
            let [addr_high, addr_low] = addr.raw().to_be_bytes();
            return vec![
                Self::EXTENDED_LENGTH as u8,
                self.connection_type,
                self.knx_layer,
                self.reserved,
                addr_high,
                addr_low,
            ];
        }

        vec![
            Self::LENGTH as u8,
            self.connection_type,
            self.knx_layer,
            self.reserved,
        ]
    }
}

/// Connect Request message
#[derive(Debug, Clone)]
pub struct ConnectRequest {
    pub control_endpoint: Hpai,
    pub data_endpoint: Hpai,
    pub cri: ConnectionRequestInfo,
}

impl ConnectRequest {
    #[must_use]
    pub fn new(control_addr: SocketAddr, data_addr: SocketAddr) -> Self {
        Self {
            control_endpoint: Hpai::new(control_addr),
            data_endpoint: Hpai::new(data_addr),
            cri: ConnectionRequestInfo::new_tunnel(),
        }
    }

    #[must_use]
    pub fn new_tcp_route_back() -> Self {
        Self {
            control_endpoint: Hpai::new_tcp_route_back(),
            data_endpoint: Hpai::new_tcp_route_back(),
            cri: ConnectionRequestInfo::new_tunnel(),
        }
    }

    /// UDP route-back `ConnectRequest` (0.0.0.0:0 endpoints): the gateway replies
    /// to the packet source. Needed for NAT/routed clients.
    #[must_use]
    pub fn new_route_back() -> Self {
        Self {
            control_endpoint: Hpai::route_back(),
            data_endpoint: Hpai::route_back(),
            cri: ConnectionRequestInfo::new_tunnel(),
        }
    }

    /// # Errors
    ///
    /// Returns the same errors as [`Hpai::parse`] and
    /// [`ConnectionRequestInfo::parse`], applied to the control endpoint,
    /// data endpoint, and CRI in sequence.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut offset = 0;

        let control_endpoint = Hpai::parse(&data[offset..])?;
        offset += Hpai::LENGTH;

        let data_endpoint = Hpai::parse(&data[offset..])?;
        offset += Hpai::LENGTH;

        let cri = ConnectionRequestInfo::parse(&data[offset..])?;

        Ok(Self {
            control_endpoint,
            data_endpoint,
            cri,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&self.control_endpoint.serialize());
        data.extend_from_slice(&self.data_endpoint.serialize());
        data.extend_from_slice(&self.cri.serialize());
        data
    }
}

/// Connect Response message
#[derive(Debug, Clone)]
pub struct ConnectResponse {
    pub channel_id: u8,
    pub status: u8,
    pub data_endpoint: Hpai,
    pub crd: Vec<u8>, // Connection Response Data
    /// Individual address assigned by the gateway, parsed from a tunnel-connection CRD.
    pub assigned_address: Option<IndividualAddress>,
}

impl ConnectResponse {
    pub const STATUS_OK: u8 = 0x00;
    pub const STATUS_ERROR_HOST_PROTOCOL_TYPE: u8 = 0x01;
    pub const STATUS_ERROR_VERSION_NOT_SUPPORTED: u8 = 0x02;
    pub const STATUS_ERROR_SEQUENCE_NUMBER: u8 = 0x04;
    pub const STATUS_ERROR_GENERAL: u8 = 0x0F;
    pub const STATUS_ERROR_CONNECTION_ID: u8 = 0x21;
    pub const STATUS_ERROR_CONNECTION_TYPE: u8 = 0x22;
    pub const STATUS_ERROR_CONNECTION_OPTION: u8 = 0x23;
    pub const STATUS_ERROR_NO_MORE_CONNECTIONS: u8 = 0x24;
    pub const STATUS_ERROR_NO_MORE_UNIQUE_CONNECTIONS: u8 = 0x25;
    pub const STATUS_ERROR_DATA_CONNECTION: u8 = 0x26;
    pub const STATUS_ERROR_KNX_CONNECTION: u8 = 0x27;
    pub const STATUS_ERROR_AUTHORIZATION: u8 = 0x28;
    pub const STATUS_ERROR_TUNNELLING_LAYER: u8 = 0x29;
    pub const STATUS_ERROR_NO_TUNNELLING_ADDRESS: u8 = 0x2D;
    pub const STATUS_ERROR_CONNECTION_IN_USE: u8 = 0x2E;

    /// Get human-readable description for status codes
    #[must_use]
    pub fn status_description(status: u8) -> &'static str {
        match status {
            Self::STATUS_OK => "OK",
            Self::STATUS_ERROR_HOST_PROTOCOL_TYPE => "Host protocol type not supported",
            Self::STATUS_ERROR_VERSION_NOT_SUPPORTED => "Version not supported",
            Self::STATUS_ERROR_SEQUENCE_NUMBER => "Sequence number error",
            Self::STATUS_ERROR_GENERAL => "General error",
            Self::STATUS_ERROR_CONNECTION_ID => "Connection ID error",
            Self::STATUS_ERROR_CONNECTION_TYPE => "Connection type not supported",
            Self::STATUS_ERROR_CONNECTION_OPTION => "Connection option not supported",
            Self::STATUS_ERROR_NO_MORE_CONNECTIONS => "No more connections available",
            Self::STATUS_ERROR_NO_MORE_UNIQUE_CONNECTIONS => "No more unique connections available",
            Self::STATUS_ERROR_DATA_CONNECTION => "Data connection error",
            Self::STATUS_ERROR_KNX_CONNECTION => "KNX connection error",
            Self::STATUS_ERROR_AUTHORIZATION => "Authorization error",
            Self::STATUS_ERROR_TUNNELLING_LAYER => "Tunnelling layer not supported",
            Self::STATUS_ERROR_NO_TUNNELLING_ADDRESS => "No tunnelling address available",
            Self::STATUS_ERROR_CONNECTION_IN_USE => "Connection address already in use",
            _ => "Unknown error",
        }
    }

    /// Get formatted error message for non-OK status
    #[must_use]
    pub fn error_message(&self) -> Option<String> {
        if self.status == Self::STATUS_OK {
            None
        } else {
            Some(format!(
                "Gateway rejected connection: {} (0x{:02X})",
                Self::status_description(self.status),
                self.status
            ))
        }
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than 2
    /// bytes, or the same errors as [`Hpai::parse`] for the data endpoint.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 2 {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: "Connect response too short".to_string(),
            }
            .into());
        }

        let channel_id = data[0];
        let status = data[1];
        let mut offset = 2;

        if status != 0x00 {
            log_protocol!(
                LogLevel::Warn,
                "ConnectResponse: rejected status=0x{:02X}",
                status
            );
        }

        // Only parse data endpoint and CRD if status is OK (following Python Knx behavior)
        let (data_endpoint, crd) = if status == Self::STATUS_OK {
            // Parse data endpoint (HPAI)
            let data_endpoint = if data.len() > offset {
                let hpai = Hpai::parse(&data[offset..])?;
                offset += Hpai::LENGTH;
                hpai
            } else {
                return Err(ProtocolError::ParseError {
                    offset,
                    reason: "Missing data endpoint in connect response".to_string(),
                }
                .into());
            };

            // Parse CRD (Connect Response Data)
            let crd = if data.len() > offset {
                data[offset..].to_vec()
            } else {
                Vec::new()
            };

            (data_endpoint, crd)
        } else {
            // For error responses, use default values (no data endpoint or CRD)
            (
                Hpai {
                    host_protocol_code: Hpai::PROTOCOL_UDP,
                    ip_addr: IpAddr::V4([0, 0, 0, 0].into()),
                    port: 0,
                },
                Vec::new(),
            )
        };

        Ok(Self {
            channel_id,
            status,
            data_endpoint,
            assigned_address: Self::parse_assigned_address(&crd),
            crd,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::new();
        data.push(self.channel_id);
        data.push(self.status);
        data.extend_from_slice(&self.data_endpoint.serialize());
        data.extend_from_slice(&self.crd);
        data
    }

    /// Parse the individual address assigned by the gateway from the CRD.
    ///
    /// A tunnelling CRD has the layout `[structlen, 0x04 (TUNNEL_CONNECTION),
    /// addr_high, addr_low]`. Returns `None` for non-tunnel or too-short CRDs.
    fn parse_assigned_address(crd: &[u8]) -> Option<IndividualAddress> {
        if crd.len() >= 4 && crd[1] == ConnectionRequestInfo::TUNNEL_CONNECTION {
            let addr = IndividualAddress::from_raw(u16::from_be_bytes([crd[2], crd[3]]));
            log_protocol!(
                LogLevel::Debug,
                "ConnectResponse: assigned address={}",
                addr
            );
            Some(addr)
        } else {
            None
        }
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        self.status == Self::STATUS_OK
    }
}

/// KNX/IP frame structure
#[derive(Debug, Clone)]
pub struct KnxIpFrame {
    pub header: KnxIpHeader,
    pub body: Vec<u8>,
}

impl KnxIpFrame {
    #[must_use]
    pub fn new(service_type: ServiceType, body: Vec<u8>) -> Self {
        let header = KnxIpHeader::new(service_type, body.len() as u16);
        Self { header, body }
    }

    /// # Errors
    ///
    /// Returns the same errors as [`KnxIpHeader::parse`], plus
    /// [`ProtocolError::ParseError`] if `data` is shorter than the header's
    /// declared `total_length`.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let header = KnxIpHeader::parse(data)?;

        if data.len() < header.total_length as usize {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!(
                    "Frame too short: expected {} bytes, got {}",
                    header.total_length,
                    data.len()
                ),
            }
            .into());
        }

        let body = data[KnxIpHeader::LENGTH..header.total_length as usize].to_vec();

        log_protocol!(
            LogLevel::Trace,
            "KNX/IP frame: {:?} body_len={}",
            header.service_type,
            body.len()
        );

        Ok(Self { header, body })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = self.header.serialize();
        data.extend_from_slice(&self.body);
        data
    }
}

/// `SessionRequest` message (service type 0x0951) — first message of a KNX IP
/// Secure session handshake: the client's control endpoint and ECDH public key.
#[derive(Debug, Clone)]
pub struct SessionRequest {
    pub control_endpoint: Hpai,
    pub public_key: [u8; 32],
}

impl SessionRequest {
    pub const LENGTH: usize = Hpai::LENGTH + 32;

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than
    /// [`Self::LENGTH`], or the same errors as [`Hpai::parse`] for the
    /// control endpoint.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("SessionRequest too short: {} bytes", data.len()),
            }
            .into());
        }
        let control_endpoint = Hpai::parse(data)?;
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&data[Hpai::LENGTH..Self::LENGTH]);
        Ok(Self {
            control_endpoint,
            public_key,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = self.control_endpoint.serialize();
        data.extend_from_slice(&self.public_key);
        data
    }
}

/// `SessionResponse` message (service type 0x0952) — the server's session ID,
/// ECDH public key, and device-authentication MAC.
#[derive(Debug, Clone)]
pub struct SessionResponse {
    pub session_id: u16,
    pub public_key: [u8; 32],
    pub mac: [u8; 16],
}

impl SessionResponse {
    pub const LENGTH: usize = 2 + 32 + 16;

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than [`Self::LENGTH`].
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("SessionResponse too short: {} bytes", data.len()),
            }
            .into());
        }
        let session_id = u16::from_be_bytes([data[0], data[1]]);
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&data[2..34]);
        let mut mac = [0u8; 16];
        mac.copy_from_slice(&data[34..50]);
        Ok(Self {
            session_id,
            public_key,
            mac,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::LENGTH);
        data.extend_from_slice(&self.session_id.to_be_bytes());
        data.extend_from_slice(&self.public_key);
        data.extend_from_slice(&self.mac);
        data
    }
}

/// `SessionAuthenticate` message (service type 0x0953) — the client's user ID
/// and authentication MAC, proving it knows the user password.
#[derive(Debug, Clone)]
pub struct SessionAuthenticate {
    pub user_id: u8,
    pub mac: [u8; 16],
}

impl SessionAuthenticate {
    pub const LENGTH: usize = 2 + 16;

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than [`Self::LENGTH`].
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("SessionAuthenticate too short: {} bytes", data.len()),
            }
            .into());
        }
        // data[0] is reserved
        let user_id = data[1];
        let mut mac = [0u8; 16];
        mac.copy_from_slice(&data[2..18]);
        Ok(Self { user_id, mac })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::LENGTH);
        data.push(0x00); // reserved
        data.push(self.user_id);
        data.extend_from_slice(&self.mac);
        data
    }
}

/// `SessionStatus` message (service type 0x0954) — completes or fails the
/// secure session handshake.
#[derive(Debug, Clone)]
pub struct SessionStatus {
    pub status: u8,
}

impl SessionStatus {
    pub const LENGTH: usize = 2;
    pub const STATUS_OK: u8 = 0x00;
    pub const STATUS_AUTH_FAILED: u8 = 0x01;

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than [`Self::LENGTH`].
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("SessionStatus too short: {} bytes", data.len()),
            }
            .into());
        }
        // data[0] is reserved
        Ok(Self { status: data[1] })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        vec![0x00, self.status]
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        self.status == Self::STATUS_OK
    }
}

/// `ConnectionState` Request message (service type 0x0207).
/// Used for heartbeat: client sends this every 60s to verify connection is alive.
#[derive(Debug, Clone)]
pub struct ConnectionstateRequest {
    pub communication_channel_id: u8,
    pub control_endpoint: Hpai,
}

impl ConnectionstateRequest {
    pub const LENGTH: usize = 2 + Hpai::LENGTH;

    #[must_use]
    pub fn new(communication_channel_id: u8, control_endpoint: Hpai) -> Self {
        Self {
            communication_channel_id,
            control_endpoint,
        }
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than
    /// [`Self::LENGTH`], or the same errors as [`Hpai::parse`] for the
    /// control endpoint.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Connectionstate request too short: {} bytes", data.len()),
            }
            .into());
        }

        let communication_channel_id = data[0];
        // data[1] is reserved
        let control_endpoint = Hpai::parse(&data[2..])?;

        Ok(Self {
            communication_channel_id,
            control_endpoint,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::LENGTH);
        data.push(self.communication_channel_id);
        data.push(0x00); // Reserved byte
        data.extend_from_slice(&self.control_endpoint.serialize());
        data
    }
}

/// `ConnectionState` Response message (service type 0x0208).
/// Status 0x00 = OK, connection is alive.
#[derive(Debug, Clone)]
pub struct ConnectionstateResponse {
    pub communication_channel_id: u8,
    pub status: u8,
}

impl ConnectionstateResponse {
    pub const LENGTH: usize = 2;
    pub const STATUS_OK: u8 = 0x00;

    #[must_use]
    pub fn new(communication_channel_id: u8, status: u8) -> Self {
        Self {
            communication_channel_id,
            status,
        }
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than [`Self::LENGTH`].
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Connectionstate response too short: {} bytes", data.len()),
            }
            .into());
        }

        let communication_channel_id = data[0];
        let status = data[1];

        Ok(Self {
            communication_channel_id,
            status,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        vec![self.communication_channel_id, self.status]
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        self.status == Self::STATUS_OK
    }
}

/// Disconnect Request message
#[derive(Debug, Clone)]
pub struct DisconnectRequest {
    pub communication_channel_id: u8,
    pub control_endpoint: Hpai,
}

impl DisconnectRequest {
    pub const LENGTH: usize = 2 + Hpai::LENGTH;

    #[must_use]
    pub fn new(communication_channel_id: u8, control_endpoint: Hpai) -> Self {
        Self {
            communication_channel_id,
            control_endpoint,
        }
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than
    /// [`Self::LENGTH`], or the same errors as [`Hpai::parse`] for the
    /// control endpoint.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Disconnect request too short: {} bytes", data.len()),
            }
            .into());
        }

        let communication_channel_id = data[0];
        // data[1] is reserved
        let control_endpoint = Hpai::parse(&data[2..])?;

        Ok(Self {
            communication_channel_id,
            control_endpoint,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::LENGTH);
        data.push(self.communication_channel_id);
        data.push(0x00); // Reserved byte
        data.extend_from_slice(&self.control_endpoint.serialize());
        data
    }
}

/// Disconnect Response message
#[derive(Debug, Clone)]
pub struct DisconnectResponse {
    pub communication_channel_id: u8,
    pub status: u8,
}

impl DisconnectResponse {
    pub const LENGTH: usize = 2;
    pub const STATUS_OK: u8 = 0x00;

    #[must_use]
    pub fn new(communication_channel_id: u8, status: u8) -> Self {
        Self {
            communication_channel_id,
            status,
        }
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than [`Self::LENGTH`].
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Disconnect response too short: {} bytes", data.len()),
            }
            .into());
        }

        let communication_channel_id = data[0];
        let status = data[1];

        Ok(Self {
            communication_channel_id,
            status,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        vec![self.communication_channel_id, self.status]
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        self.status == Self::STATUS_OK
    }
}

/// Tunnelling Request message
#[derive(Debug, Clone)]
pub struct TunnellingRequest {
    pub communication_channel_id: u8,
    pub sequence_counter: u8,
    pub raw_cemi: Vec<u8>,
}

impl TunnellingRequest {
    pub const HEADER_LENGTH: usize = 4;

    #[must_use]
    pub fn new(communication_channel_id: u8, sequence_counter: u8, raw_cemi: Vec<u8>) -> Self {
        Self {
            communication_channel_id,
            sequence_counter,
            raw_cemi,
        }
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than
    /// [`Self::HEADER_LENGTH`] or the declared structure length field is wrong.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::HEADER_LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Tunnelling request too short: {} bytes", data.len()),
            }
            .into());
        }

        let structure_length = data[0];
        if structure_length != Self::HEADER_LENGTH as u8 {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Invalid tunnelling request header length: {structure_length}"),
            }
            .into());
        }

        let communication_channel_id = data[1];
        let sequence_counter = data[2];
        // data[3] is reserved
        let raw_cemi = data[Self::HEADER_LENGTH..].to_vec();

        log_protocol!(
            LogLevel::Trace,
            "TunnellingRequest: channel={} seq={} cemi_len={}",
            communication_channel_id,
            sequence_counter,
            raw_cemi.len()
        );

        Ok(Self {
            communication_channel_id,
            sequence_counter,
            raw_cemi,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::HEADER_LENGTH + self.raw_cemi.len());
        data.push(Self::HEADER_LENGTH as u8);
        data.push(self.communication_channel_id);
        data.push(self.sequence_counter);
        data.push(0x00); // Reserved
        data.extend_from_slice(&self.raw_cemi);
        data
    }
}

/// Tunnelling Acknowledgment message
#[derive(Debug, Clone)]
pub struct TunnellingAck {
    pub communication_channel_id: u8,
    pub sequence_counter: u8,
    pub status_code: u8,
}

impl TunnellingAck {
    pub const BODY_LENGTH: usize = 4;
    pub const STATUS_OK: u8 = 0x00;
    pub const STATUS_ERROR_HOST_PROTOCOL_TYPE: u8 = 0x01;
    pub const STATUS_ERROR_VERSION_NOT_SUPPORTED: u8 = 0x02;
    pub const STATUS_ERROR_SEQUENCE_NUMBER: u8 = 0x04;
    pub const STATUS_ERROR_CONNECTION_ID: u8 = 0x21;
    pub const STATUS_ERROR_CONNECTION_TYPE: u8 = 0x22;
    pub const STATUS_ERROR_CONNECTION_OPTION: u8 = 0x23;
    pub const STATUS_ERROR_NO_MORE_CONNECTIONS: u8 = 0x24;

    #[must_use]
    pub fn new(communication_channel_id: u8, sequence_counter: u8, status_code: u8) -> Self {
        Self {
            communication_channel_id,
            sequence_counter,
            status_code,
        }
    }

    #[must_use]
    pub fn new_ok(communication_channel_id: u8, sequence_counter: u8) -> Self {
        Self::new(communication_channel_id, sequence_counter, Self::STATUS_OK)
    }

    #[must_use]
    pub fn new_sequence_error(communication_channel_id: u8, sequence_counter: u8) -> Self {
        Self::new(
            communication_channel_id,
            sequence_counter,
            Self::STATUS_ERROR_SEQUENCE_NUMBER,
        )
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::ParseError`] if `data` is shorter than
    /// [`Self::BODY_LENGTH`] or the declared structure length field is wrong.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < Self::BODY_LENGTH {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Tunnelling ack too short: {} bytes", data.len()),
            }
            .into());
        }

        let structure_length = data[0];
        if structure_length != Self::BODY_LENGTH as u8 {
            return Err(ProtocolError::ParseError {
                offset: 0,
                reason: format!("Invalid tunnelling ack body length: {structure_length}"),
            }
            .into());
        }

        let communication_channel_id = data[1];
        let sequence_counter = data[2];
        let status_code = data[3];

        Ok(Self {
            communication_channel_id,
            sequence_counter,
            status_code,
        })
    }

    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        vec![
            Self::BODY_LENGTH as u8,
            self.communication_channel_id,
            self.sequence_counter,
            self.status_code,
        ]
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        self.status_code == Self::STATUS_OK
    }

    /// Get human-readable description for status codes
    #[must_use]
    pub fn status_description(&self) -> &'static str {
        match self.status_code {
            Self::STATUS_OK => "OK",
            Self::STATUS_ERROR_HOST_PROTOCOL_TYPE => "Host protocol type not supported",
            Self::STATUS_ERROR_VERSION_NOT_SUPPORTED => "Version not supported",
            Self::STATUS_ERROR_SEQUENCE_NUMBER => "Sequence number error",
            Self::STATUS_ERROR_CONNECTION_ID => "Connection ID error",
            Self::STATUS_ERROR_CONNECTION_TYPE => "Connection type not supported",
            Self::STATUS_ERROR_CONNECTION_OPTION => "Connection option not supported",
            Self::STATUS_ERROR_NO_MORE_CONNECTIONS => "No more connections available",
            _ => "Unknown error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a status-OK `ConnectResponse` body: `channel_id`, status, HPAI, then CRD.
    fn connect_response_body(crd: &[u8]) -> Vec<u8> {
        let mut body = vec![0x01, ConnectResponse::STATUS_OK];
        // Data endpoint HPAI (8 bytes): UDP 192.168.1.10:3671
        body.extend_from_slice(&[0x08, 0x01, 192, 168, 1, 10, 0x0E, 0x57]);
        body.extend_from_slice(crd);
        body
    }

    #[test]
    fn connect_response_parses_tunnel_assigned_address() {
        // Tunnelling CRD: [structlen, 0x04 (TUNNEL_CONNECTION), addr_high, addr_low].
        // 1.1.5 -> raw 0x1105.
        let crd = [0x04, 0x04, 0x11, 0x05];
        let resp = ConnectResponse::parse(&connect_response_body(&crd)).unwrap();
        assert_eq!(resp.assigned_address, Some(IndividualAddress::new(1, 1, 5)));
    }

    #[test]
    fn connect_response_non_tunnel_crd_yields_none() {
        // Connection type 0x03 (device management) is not a tunnel CRD.
        let crd = [0x04, 0x03, 0x11, 0x05];
        let resp = ConnectResponse::parse(&connect_response_body(&crd)).unwrap();
        assert_eq!(resp.assigned_address, None);
    }

    #[test]
    fn connect_response_short_crd_yields_none() {
        // CRD shorter than 4 bytes cannot contain an address.
        let crd = [0x02, 0x04];
        let resp = ConnectResponse::parse(&connect_response_body(&crd)).unwrap();
        assert_eq!(resp.assigned_address, None);
    }

    /// The last 4 bytes of a serialized `ConnectRequest` are the CRI; isolate them
    /// by stripping the two leading 8-byte HPAI structures.
    fn cri_bytes(req: &ConnectRequest) -> Vec<u8> {
        let full = req.serialize();
        full[Hpai::LENGTH * 2..].to_vec()
    }

    #[test]
    fn connect_request_default_cri_is_four_bytes() {
        let addr: SocketAddr = "192.168.1.10:3671".parse().unwrap();
        let req = ConnectRequest::new(addr, addr);
        assert!(req.cri.individual_address.is_none());
        assert_eq!(cri_bytes(&req), vec![0x04, 0x04, 0x02, 0x00]);
    }

    #[test]
    fn connect_request_extended_cri_carries_requested_address() {
        // 1.1.5 -> raw 0x1105.
        let addr: SocketAddr = "192.168.1.10:3671".parse().unwrap();
        let mut req = ConnectRequest::new(addr, addr);
        req.cri.individual_address = Some(IndividualAddress::new(1, 1, 5));
        assert_eq!(cri_bytes(&req), vec![0x06, 0x04, 0x02, 0x00, 0x11, 0x05]);
    }

    #[test]
    fn session_request_round_trips() {
        let addr: SocketAddr = "192.168.1.10:3671".parse().unwrap();
        let req = SessionRequest {
            control_endpoint: Hpai::new(addr),
            public_key: [7u8; 32],
        };
        let parsed = SessionRequest::parse(&req.serialize()).unwrap();
        assert_eq!(parsed.control_endpoint.socket_addr(), addr);
        assert_eq!(parsed.public_key, req.public_key);
    }

    #[test]
    fn session_response_round_trips() {
        let resp = SessionResponse {
            session_id: 42,
            public_key: [9u8; 32],
            mac: [1u8; 16],
        };
        let parsed = SessionResponse::parse(&resp.serialize()).unwrap();
        assert_eq!(parsed.session_id, 42);
        assert_eq!(parsed.public_key, resp.public_key);
        assert_eq!(parsed.mac, resp.mac);
    }

    #[test]
    fn session_authenticate_round_trips() {
        let auth = SessionAuthenticate {
            user_id: 3,
            mac: [2u8; 16],
        };
        let parsed = SessionAuthenticate::parse(&auth.serialize()).unwrap();
        assert_eq!(parsed.user_id, 3);
        assert_eq!(parsed.mac, auth.mac);
    }

    #[test]
    fn session_status_round_trips() {
        let status = SessionStatus {
            status: SessionStatus::STATUS_OK,
        };
        let parsed = SessionStatus::parse(&status.serialize()).unwrap();
        assert!(parsed.is_success());
    }
}
