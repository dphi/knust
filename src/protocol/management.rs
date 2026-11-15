//! Management APCI helpers for KNX device-addressed communication.
//!
//! The management services are carried over the connection-oriented transport
//! layer (`T_Data_Connected`, see [`crate::protocol::tpci`]). They are used by
//! the bus probe to interrogate a device: the probe sends an
//! [`DeviceDescriptorRead`] and waits for the device to answer with a
//! [`DeviceDescriptorResponse`].
//!
//! Both services use the 10-bit APCI field that straddles the TPCI/APCI byte
//! (low 2 bits) and the following byte (high 2 bits + 6 data bits):
//!
//! ```text
//! byte0:  ?? ?? ?? ?? ?? ??  A9 A8     (A9..A8 = APCI high bits)
//! byte1:  A7 A6  D5 D4 D3 D2 D1 D0      (A7..A6 = APCI low bits, D5..D0 = data)
//! ```
//!
//! `A_DeviceDescriptor_Read` has the APCI code `0x0300` and
//! `A_DeviceDescriptor_Response` has the APCI code `0x0340`. The low six data
//! bits of the second byte carry the descriptor type.

use crate::error::{ProtocolError, Result};
use crate::log_protocol;
use crate::logging::LogLevel;

/// 10-bit APCI service code for `A_DeviceDescriptor_Read`.
const APCI_DEVICE_DESCRIPTOR_READ: u16 = 0x0300;
/// 10-bit APCI service code for `A_DeviceDescriptor_Response`.
const APCI_DEVICE_DESCRIPTOR_RESPONSE: u16 = 0x0340;
/// Mask selecting the management APCI service bits within the 10-bit field,
/// leaving the low six descriptor/data bits clear.
const APCI_SERVICE_MASK: u16 = 0x03C0;
/// Mask selecting the descriptor (low six data bits) of the second APCI byte.
const DESCRIPTOR_MASK: u8 = 0x3F;
/// The constant high APCI byte (`A9..A8`) shared by both management services.
const APCI_HIGH_BYTE: u8 = 0x03;
/// The `A_DeviceDescriptor_Response` bits (`A7..A6`) of the second APCI byte.
const RESPONSE_BYTE_BITS: u8 = 0x40;

/// `A_DeviceDescriptor_Read` — request a device descriptor (APCI `0x0300`).
///
/// The probe sends this over `T_Data_Connected` to ask the addressed device
/// for its descriptor (descriptor type `0` is the mask-version descriptor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceDescriptorRead {
    /// Descriptor type (6-bit field; only the low six bits are significant).
    pub descriptor: u8,
}

impl DeviceDescriptorRead {
    /// Encode this read request into its two APCI bytes `[0x03, descriptor]`.
    ///
    /// The APCI code `0x0300` packs to a high byte of `0x03` and a low byte
    /// holding the descriptor in its low six bits.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let byte0 = ((APCI_DEVICE_DESCRIPTOR_READ >> 8) & 0x03) as u8;
        let byte1 = (APCI_DEVICE_DESCRIPTOR_READ as u8) | (self.descriptor & DESCRIPTOR_MASK);
        let result = vec![byte0, byte1];
        log_protocol!(
            LogLevel::Trace,
            "Management encode: {:?} \u{2192} {:02X?}",
            self,
            result
        );
        result
    }
}

const APCI_FUNCTION_PROPERTY_COMMAND: [u8; 2] = [0x02, 0xC7];
const APCI_FUNCTION_PROPERTY_STATE_READ: [u8; 2] = [0x02, 0xC8];
const APCI_FUNCTION_PROPERTY_STATE_RESPONSE: [u8; 2] = [0x02, 0xC9];
const APCI_FUNCTION_PROPERTY_EXT_COMMAND: [u8; 2] = [0x01, 0xD4];
const APCI_FUNCTION_PROPERTY_EXT_STATE_READ: [u8; 2] = [0x01, 0xD5];
const APCI_FUNCTION_PROPERTY_EXT_STATE_RESPONSE: [u8; 2] = [0x01, 0xD6];

fn encode_function_property_ext_header(
    interface_object_type: u16,
    object_instance: u16,
    property_id: u16,
) -> Result<[u8; 5]> {
    if object_instance > 0x0FFF || property_id > 0x0FFF {
        return Err(ProtocolError::InvalidFrame {
            details: "Function-property object instance or property ID exceeds 12 bits".to_owned(),
        }
        .into());
    }
    Ok([
        (interface_object_type >> 8) as u8,
        interface_object_type as u8,
        (object_instance >> 4) as u8,
        ((object_instance << 4) as u8) | (property_id >> 8) as u8,
        property_id as u8,
    ])
}

fn decode_function_property_ext_header(apci_bytes: &[u8]) -> (u16, u16, u16) {
    (
        u16::from_be_bytes([apci_bytes[2], apci_bytes[3]]),
        (u16::from(apci_bytes[4]) << 4) | u16::from(apci_bytes[5] >> 4),
        (u16::from(apci_bytes[5] & 0x0F) << 8) | u16::from(apci_bytes[6]),
    )
}

/// Function-specific state response; None is the KNX negative response without a return code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionPropertyCommand {
    pub object_index: u8,
    pub property_id: u8,
    pub data: Vec<u8>,
}

/// Function-property state read request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionPropertyStateRead {
    pub object_index: u8,
    pub property_id: u8,
    pub data: Vec<u8>,
}

/// Extended function-property command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionPropertyExtCommand {
    pub interface_object_type: u16,
    pub object_instance: u16,
    pub property_id: u16,
    pub data: Vec<u8>,
}

/// Extended function-property state read request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionPropertyExtStateRead {
    pub interface_object_type: u16,
    pub object_instance: u16,
    pub property_id: u16,
    pub data: Vec<u8>,
}

/// Function-specific state response; None is the KNX negative response without a return code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionPropertyStateResponse {
    pub object_index: u8,
    pub property_id: u8,
    pub return_code: Option<u8>,
    pub data: Vec<u8>,
}

/// Extended function-property response. Return codes stay raw because functions may define codes outside the generic enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionPropertyExtStateResponse {
    pub interface_object_type: u16,
    pub object_instance: u16,
    pub property_id: u16,
    pub return_code: u8,
    pub data: Vec<u8>,
}

/// One of the standard or extended function-property APDU services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionPropertyService {
    Command(FunctionPropertyCommand),
    StateRead(FunctionPropertyStateRead),
    StateResponse(FunctionPropertyStateResponse),
    ExtCommand(FunctionPropertyExtCommand),
    ExtStateRead(FunctionPropertyExtStateRead),
    ExtStateResponse(FunctionPropertyExtStateResponse),
}

impl FunctionPropertyService {
    /// Decode one function-property APDU by its APCI service code.
    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        match apci_bytes.get(..2) {
            Some(code) if code == APCI_FUNCTION_PROPERTY_COMMAND => {
                FunctionPropertyCommand::decode(apci_bytes).map(Self::Command)
            }
            Some(code) if code == APCI_FUNCTION_PROPERTY_STATE_READ => {
                FunctionPropertyStateRead::decode(apci_bytes).map(Self::StateRead)
            }
            Some(code) if code == APCI_FUNCTION_PROPERTY_STATE_RESPONSE => {
                FunctionPropertyStateResponse::decode(apci_bytes).map(Self::StateResponse)
            }
            Some(code) if code == APCI_FUNCTION_PROPERTY_EXT_COMMAND => {
                FunctionPropertyExtCommand::decode(apci_bytes).map(Self::ExtCommand)
            }
            Some(code) if code == APCI_FUNCTION_PROPERTY_EXT_STATE_READ => {
                FunctionPropertyExtStateRead::decode(apci_bytes).map(Self::ExtStateRead)
            }
            Some(code) if code == APCI_FUNCTION_PROPERTY_EXT_STATE_RESPONSE => {
                FunctionPropertyExtStateResponse::decode(apci_bytes).map(Self::ExtStateResponse)
            }
            _ => Err(ProtocolError::InvalidFrame {
                details: "Unknown function-property APCI service".to_owned(),
            }
            .into()),
        }
    }
}

/// `A_PropertyDescription_Response` — metadata for one interface-object property.
///
/// The `type_` and `access` octets are retained verbatim so unknown PDT values
/// and access bits remain available to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyDescriptionResponse {
    pub object_index: u8,
    pub property_id: u8,
    pub property_index: u8,
    pub type_: u8,
    pub max_count: u16,
    pub access: u8,
}

/// `A_PropertyDescription_Read` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyDescriptionRead {
    pub object_index: u8,
    pub property_id: u8,
    pub property_index: u8,
}

/// `A_PropertyValue_Read` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyValueRead {
    pub object_index: u8,
    pub property_id: u8,
    pub count: u8,
    pub start_index: u16,
}

/// `A_PropertyValue_Response` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyValueResponse {
    pub object_index: u8,
    pub property_id: u8,
    pub count: u8,
    pub start_index: u16,
    pub data: Vec<u8>,
}

/// `A_PropertyValue_Write` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyValueWrite {
    pub object_index: u8,
    pub property_id: u8,
    pub count: u8,
    pub start_index: u16,
    pub data: Vec<u8>,
}

/// `A_Authorize_Request` carrying a device access key.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizeRequest {
    pub key: u32,
}

/// `A_Authorize_Response` reporting the granted access level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizeResponse {
    pub level: u8,
}

/// `A_Restart_Master_Reset` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartMasterReset {
    pub erase_code: u8,
    pub channel_number: u8,
}

/// `A_Restart_Response` confirming a Master Reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartMasterResetResponse {
    pub error_code: u8,
    pub process_time: u16,
}

/// `A_Memory_Read` request for the 64 KiB memory address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRead {
    pub address: u16,
    pub count: u8,
}

/// `A_Memory_Response` from the 64 KiB memory address space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryResponse {
    pub address: u16,
    pub count: u8,
    pub data: Vec<u8>,
}

/// `A_Memory_Write` request for the 64 KiB memory address space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWrite {
    pub address: u16,
    pub count: u8,
    pub data: Vec<u8>,
}

/// `A_UserMemory_Read` request for the 1 MiB user-memory address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserMemoryRead {
    pub address: u32,
    pub count: u8,
}

/// `A_UserMemory_Response` from the 1 MiB user-memory address space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMemoryResponse {
    pub address: u32,
    pub count: u8,
    pub data: Vec<u8>,
}

/// `A_UserMemory_Write` request for the 1 MiB user-memory address space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMemoryWrite {
    pub address: u32,
    pub count: u8,
    pub data: Vec<u8>,
}

/// `A_DeviceDescriptor_Response` — a device descriptor answer (APCI `0x0340`).
///
/// Sent by the device in reply to a [`DeviceDescriptorRead`]. It echoes the
/// descriptor type and carries the descriptor payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescriptorResponse {
    /// Descriptor type echoed from the request (6-bit field).
    pub descriptor: u8,
    /// Descriptor payload bytes that follow the two APCI bytes.
    pub data: Vec<u8>,
}

impl DeviceDescriptorResponse {
    /// Encode this response into its APCI bytes `[0x03, 0x40 | descriptor, data..]`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(self.data.len() + 2);
        result.push(APCI_HIGH_BYTE);
        result.push(RESPONSE_BYTE_BITS | (self.descriptor & DESCRIPTOR_MASK));
        result.extend_from_slice(&self.data);
        log_protocol!(
            LogLevel::Trace,
            "Management encode: {:?} \u{2192} {} bytes",
            self,
            result.len()
        );
        result
    }

    /// Decode a device descriptor response from its APCI bytes.
    ///
    /// The first two bytes must match the `A_DeviceDescriptor_Response` APCI
    /// pattern (`0x0340` once the low six data bits are masked off). The
    /// descriptor is taken from the low six bits of the second byte and the
    /// remaining bytes form [`DeviceDescriptorResponse::data`].
    ///
    /// # Errors
    ///
    /// Returns a [`ProtocolError::InvalidFrame`] if fewer than two bytes are
    /// supplied or if the APCI pattern is not a device descriptor response.
    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        if apci_bytes.len() < 2 {
            return Err(ProtocolError::InvalidFrame {
                details: format!(
                    "DeviceDescriptorResponse needs at least 2 APCI bytes, got {}",
                    apci_bytes.len()
                ),
            }
            .into());
        }

        let byte0 = apci_bytes[0];
        let byte1 = apci_bytes[1];
        let apci = (u16::from(byte0 & 0x03) << 8) | u16::from(byte1);
        log_protocol!(
            LogLevel::Trace,
            "Management decode: apci=0x{:04X} len={}",
            apci,
            apci_bytes.len()
        );

        if (apci & APCI_SERVICE_MASK) != APCI_DEVICE_DESCRIPTOR_RESPONSE {
            log_protocol!(
                LogLevel::Warn,
                "Management: not a DeviceDescriptorResponse (apci=0x{:04X})",
                apci
            );
            return Err(ProtocolError::InvalidFrame {
                details: format!(
                    "Expected DeviceDescriptorResponse APCI 0x{APCI_DEVICE_DESCRIPTOR_RESPONSE:04X}, \
                     got 0x{apci:04X}"
                ),
            }
            .into());
        }

        Ok(Self {
            descriptor: byte1 & DESCRIPTOR_MASK,
            data: apci_bytes[2..].to_vec(),
        })
    }
}

impl PropertyDescriptionResponse {
    /// Encode the response as APCI bytes, including service `0x03D9`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(9);
        result.extend_from_slice(&[
            0x03,
            0xD9,
            self.object_index,
            self.property_id,
            self.property_index,
            self.type_,
        ]);
        result.extend_from_slice(&(self.max_count & 0x0FFF).to_be_bytes());
        result.push(self.access);
        result
    }

    /// Decode a nine-byte `A_PropertyDescription_Response` APCI frame.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::InvalidFrame`] for a different APCI service or
    /// a frame whose length is not exactly nine bytes.
    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        if apci_bytes.len() != 9 || apci_bytes[..2] != [0x03, 0xD9] {
            return Err(ProtocolError::InvalidFrame {
                details: format!(
                    "Expected 9-byte PropertyDescriptionResponse APCI 0x03D9, got {} bytes",
                    apci_bytes.len()
                ),
            }
            .into());
        }

        Ok(Self {
            object_index: apci_bytes[2],
            property_id: apci_bytes[3],
            property_index: apci_bytes[4],
            type_: apci_bytes[5],
            max_count: u16::from_be_bytes([apci_bytes[6], apci_bytes[7]]) & 0x0FFF,
            access: apci_bytes[8],
        })
    }

    /// Whether the property is writable.
    #[must_use]
    pub const fn writable(self) -> bool {
        self.type_ & 0x80 != 0
    }

    /// Raw six-bit property data type code, including reserved codes.
    #[must_use]
    pub const fn pdt(self) -> u8 {
        self.type_ & 0x3F
    }

    /// Required read access level.
    #[must_use]
    pub const fn read_level(self) -> u8 {
        self.access >> 4
    }

    /// Required write access level.
    #[must_use]
    pub const fn write_level(self) -> u8 {
        self.access & 0x0F
    }
}

const APCI_PROPERTY_VALUE_READ: u8 = 0xD5;
const APCI_PROPERTY_VALUE_RESPONSE: u8 = 0xD6;
const APCI_PROPERTY_VALUE_WRITE: u8 = 0xD7;

const APCI_MEMORY_READ: u8 = 0x00;
const APCI_MEMORY_RESPONSE: u8 = 0x40;
const APCI_MEMORY_WRITE: u8 = 0x80;
const APCI_USER_MEMORY_READ: u8 = 0xC0;
const APCI_USER_MEMORY_RESPONSE: u8 = 0xC1;
const APCI_USER_MEMORY_WRITE: u8 = 0xC2;

fn encode_memory(service: u8, address: u16, count: u8, data: &[u8]) -> Result<Vec<u8>> {
    if count > 0x3F {
        return Err(ProtocolError::InvalidFrame {
            details: format!("Memory element count {count} exceeds 63"),
        }
        .into());
    }
    let mut result = vec![0x02, service | count, (address >> 8) as u8, address as u8];
    result.extend_from_slice(data);
    Ok(result)
}

fn decode_memory(apdu: &[u8], service: u8, read: bool) -> Result<(u16, u8, &[u8])> {
    if apdu.len() < 4 || apdu[0] != 0x02 || apdu[1] & 0xC0 != service || (read && apdu.len() != 4) {
        return Err(ProtocolError::InvalidFrame {
            details: format!("Malformed memory APCI service 0x02{service:02X}"),
        }
        .into());
    }
    Ok((
        u16::from_be_bytes([apdu[2], apdu[3]]),
        apdu[1] & 0x3F,
        &apdu[4..],
    ))
}

fn encode_user_memory(service: u8, address: u32, count: u8, data: &[u8]) -> Result<Vec<u8>> {
    if address > 0x0F_FFFF || count > 0x0F {
        return Err(ProtocolError::InvalidFrame {
            details: format!("User memory address {address} or count {count} is out of range"),
        }
        .into());
    }
    let mut result = vec![
        0x02,
        service,
        (((address >> 16) as u8) << 4) | count,
        (address >> 8) as u8,
        address as u8,
    ];
    result.extend_from_slice(data);
    Ok(result)
}

fn decode_user_memory(apdu: &[u8], service: u8, read: bool) -> Result<(u32, u8, &[u8])> {
    if apdu.len() < 5 || apdu[..2] != [0x02, service] || (read && apdu.len() != 5) {
        return Err(ProtocolError::InvalidFrame {
            details: format!("Malformed user memory APCI service 0x02{service:02X}"),
        }
        .into());
    }
    Ok((
        (u32::from(apdu[2] & 0xF0) << 12) | u32::from_be_bytes([0, 0, apdu[3], apdu[4]]),
        apdu[2] & 0x0F,
        &apdu[5..],
    ))
}

impl AuthorizeRequest {
    pub fn encode(&self) -> Vec<u8> {
        let mut apdu = vec![0x03, 0xD1, 0];
        apdu.extend_from_slice(&self.key.to_be_bytes());
        apdu
    }

    pub fn decode(apdu: &[u8]) -> Result<Self> {
        if apdu.len() != 7 || apdu[..2] != [0x03, 0xD1] {
            return Err(ProtocolError::InvalidFrame {
                details: "Expected 7-byte AuthorizeRequest APCI 0x03D1".to_owned(),
            }
            .into());
        }
        Ok(Self {
            key: u32::from_be_bytes([apdu[3], apdu[4], apdu[5], apdu[6]]),
        })
    }
}

impl AuthorizeResponse {
    pub fn encode(self) -> Vec<u8> {
        vec![0x03, 0xD2, self.level]
    }

    pub fn decode(apdu: &[u8]) -> Result<Self> {
        if apdu.len() != 3 || apdu[..2] != [0x03, 0xD2] {
            return Err(ProtocolError::InvalidFrame {
                details: "Expected 3-byte AuthorizeResponse APCI 0x03D2".to_owned(),
            }
            .into());
        }
        Ok(Self { level: apdu[2] })
    }
}

impl RestartMasterReset {
    pub fn encode(self) -> Vec<u8> {
        vec![0x03, 0x81, self.erase_code, self.channel_number]
    }

    pub fn decode(apdu: &[u8]) -> Result<Self> {
        if apdu.len() != 4 || apdu[..2] != [0x03, 0x81] {
            return Err(ProtocolError::InvalidFrame {
                details: "Expected 4-byte RestartMasterReset APCI 0x0381".to_owned(),
            }
            .into());
        }
        Ok(Self {
            erase_code: apdu[2],
            channel_number: apdu[3],
        })
    }
}

impl RestartMasterResetResponse {
    pub fn encode(self) -> Vec<u8> {
        vec![
            0x03,
            0xA1,
            self.error_code,
            (self.process_time >> 8) as u8,
            self.process_time as u8,
        ]
    }

    pub fn decode(apdu: &[u8]) -> Result<Self> {
        if apdu.len() != 5 || apdu[..2] != [0x03, 0xA1] {
            return Err(ProtocolError::InvalidFrame {
                details: "Expected 5-byte RestartMasterResetResponse APCI 0x03A1".to_owned(),
            }
            .into());
        }
        Ok(Self {
            error_code: apdu[2],
            process_time: u16::from_be_bytes([apdu[3], apdu[4]]),
        })
    }
}

impl MemoryRead {
    pub fn encode(self) -> Result<Vec<u8>> {
        encode_memory(APCI_MEMORY_READ, self.address, self.count, &[])
    }

    pub fn decode(apdu: &[u8]) -> Result<Self> {
        let (address, count, _) = decode_memory(apdu, APCI_MEMORY_READ, true)?;
        Ok(Self { address, count })
    }
}

impl MemoryResponse {
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_memory(APCI_MEMORY_RESPONSE, self.address, self.count, &self.data)
    }

    pub fn decode(apdu: &[u8]) -> Result<Self> {
        let (address, count, data) = decode_memory(apdu, APCI_MEMORY_RESPONSE, false)?;
        Ok(Self {
            address,
            count,
            data: data.to_vec(),
        })
    }
}

impl MemoryWrite {
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_memory(APCI_MEMORY_WRITE, self.address, self.count, &self.data)
    }

    pub fn decode(apdu: &[u8]) -> Result<Self> {
        let (address, count, data) = decode_memory(apdu, APCI_MEMORY_WRITE, false)?;
        Ok(Self {
            address,
            count,
            data: data.to_vec(),
        })
    }
}

impl UserMemoryRead {
    pub fn encode(self) -> Result<Vec<u8>> {
        encode_user_memory(APCI_USER_MEMORY_READ, self.address, self.count, &[])
    }

    pub fn decode(apdu: &[u8]) -> Result<Self> {
        let (address, count, _) = decode_user_memory(apdu, APCI_USER_MEMORY_READ, true)?;
        Ok(Self { address, count })
    }
}

impl UserMemoryResponse {
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_user_memory(
            APCI_USER_MEMORY_RESPONSE,
            self.address,
            self.count,
            &self.data,
        )
    }

    pub fn decode(apdu: &[u8]) -> Result<Self> {
        let (address, count, data) = decode_user_memory(apdu, APCI_USER_MEMORY_RESPONSE, false)?;
        Ok(Self {
            address,
            count,
            data: data.to_vec(),
        })
    }
}

impl UserMemoryWrite {
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_user_memory(APCI_USER_MEMORY_WRITE, self.address, self.count, &self.data)
    }

    pub fn decode(apdu: &[u8]) -> Result<Self> {
        let (address, count, data) = decode_user_memory(apdu, APCI_USER_MEMORY_WRITE, false)?;
        Ok(Self {
            address,
            count,
            data: data.to_vec(),
        })
    }
}

fn encode_property_value(
    service: u8,
    object_index: u8,
    property_id: u8,
    count: u8,
    start_index: u16,
    data: &[u8],
) -> Result<Vec<u8>> {
    if count > 0x0F || start_index > 0x0FFF {
        return Err(ProtocolError::InvalidFrame {
            details: format!(
                "Property value count {count} or start index {start_index} is out of range"
            ),
        }
        .into());
    }

    let mut result = Vec::with_capacity(6 + data.len());
    result.extend_from_slice(&[
        APCI_HIGH_BYTE,
        service,
        object_index,
        property_id,
        (count << 4) | ((start_index >> 8) as u8),
        start_index as u8,
    ]);
    result.extend_from_slice(data);
    Ok(result)
}

fn decode_property_value(apci_bytes: &[u8], service: u8) -> Result<(u8, u8, u8, u16, &[u8])> {
    if apci_bytes.len() < 6 || apci_bytes[0] != APCI_HIGH_BYTE || apci_bytes[1] != service {
        return Err(ProtocolError::InvalidFrame {
            details: format!(
                "Expected PropertyValue APCI 0x03{service:02X} with at least 6 bytes, got {} bytes",
                apci_bytes.len()
            ),
        }
        .into());
    }

    let count_and_index = apci_bytes[4];
    Ok((
        apci_bytes[2],
        apci_bytes[3],
        count_and_index >> 4,
        (u16::from(count_and_index & 0x0F) << 8) | u16::from(apci_bytes[5]),
        &apci_bytes[6..],
    ))
}

impl PropertyValueRead {
    /// Encode the request as APCI bytes.
    pub fn encode(self) -> Result<Vec<u8>> {
        encode_property_value(
            APCI_PROPERTY_VALUE_READ,
            self.object_index,
            self.property_id,
            self.count,
            self.start_index,
            &[],
        )
    }

    /// Decode a property value read request from APCI bytes.
    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        let (object_index, property_id, count, start_index, data) =
            decode_property_value(apci_bytes, APCI_PROPERTY_VALUE_READ)?;
        if !data.is_empty() {
            return Err(ProtocolError::InvalidFrame {
                details: "PropertyValueRead must not contain data".to_owned(),
            }
            .into());
        }
        Ok(Self {
            object_index,
            property_id,
            count,
            start_index,
        })
    }
}

impl PropertyValueResponse {
    /// Encode the response as APCI bytes.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_property_value(
            APCI_PROPERTY_VALUE_RESPONSE,
            self.object_index,
            self.property_id,
            self.count,
            self.start_index,
            &self.data,
        )
    }

    /// Decode a property value response from APCI bytes.
    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        let (object_index, property_id, count, start_index, data) =
            decode_property_value(apci_bytes, APCI_PROPERTY_VALUE_RESPONSE)?;
        Ok(Self {
            object_index,
            property_id,
            count,
            start_index,
            data: data.to_vec(),
        })
    }
}

impl PropertyValueWrite {
    /// Encode the request as APCI bytes.
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_property_value(
            APCI_PROPERTY_VALUE_WRITE,
            self.object_index,
            self.property_id,
            self.count,
            self.start_index,
            &self.data,
        )
    }

    /// Decode a property value write from APCI bytes.
    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        let (object_index, property_id, count, start_index, data) =
            decode_property_value(apci_bytes, APCI_PROPERTY_VALUE_WRITE)?;
        Ok(Self {
            object_index,
            property_id,
            count,
            start_index,
            data: data.to_vec(),
        })
    }
}

impl FunctionPropertyCommand {
    pub fn encode(&self) -> Vec<u8> {
        let mut result = APCI_FUNCTION_PROPERTY_COMMAND.to_vec();
        result.extend_from_slice(&[self.object_index, self.property_id]);
        result.extend_from_slice(&self.data);
        result
    }

    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        if apci_bytes.len() < 4 || apci_bytes[..2] != APCI_FUNCTION_PROPERTY_COMMAND {
            return Err(ProtocolError::InvalidFrame {
                details: format!(
                    "Expected FunctionPropertyCommand APCI 0x02C7, got {} bytes",
                    apci_bytes.len()
                ),
            }
            .into());
        }
        Ok(Self {
            object_index: apci_bytes[2],
            property_id: apci_bytes[3],
            data: apci_bytes[4..].to_vec(),
        })
    }
}

impl FunctionPropertyStateRead {
    pub fn encode(&self) -> Vec<u8> {
        let mut result = APCI_FUNCTION_PROPERTY_STATE_READ.to_vec();
        result.extend_from_slice(&[self.object_index, self.property_id]);
        result.extend_from_slice(&self.data);
        result
    }

    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        if apci_bytes.len() < 4 || apci_bytes[..2] != APCI_FUNCTION_PROPERTY_STATE_READ {
            return Err(ProtocolError::InvalidFrame {
                details: format!(
                    "Expected FunctionPropertyStateRead APCI 0x02C8, got {} bytes",
                    apci_bytes.len()
                ),
            }
            .into());
        }
        Ok(Self {
            object_index: apci_bytes[2],
            property_id: apci_bytes[3],
            data: apci_bytes[4..].to_vec(),
        })
    }
}

impl FunctionPropertyExtCommand {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let header = encode_function_property_ext_header(
            self.interface_object_type,
            self.object_instance,
            self.property_id,
        )?;
        let mut result = APCI_FUNCTION_PROPERTY_EXT_COMMAND.to_vec();
        result.extend_from_slice(&header);
        result.extend_from_slice(&self.data);
        Ok(result)
    }

    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        if apci_bytes.len() < 7 || apci_bytes[..2] != APCI_FUNCTION_PROPERTY_EXT_COMMAND {
            return Err(ProtocolError::InvalidFrame {
                details: format!(
                    "Expected FunctionPropertyExtCommand APCI 0x01D4, got {} bytes",
                    apci_bytes.len()
                ),
            }
            .into());
        }
        let (interface_object_type, object_instance, property_id) =
            decode_function_property_ext_header(apci_bytes);
        Ok(Self {
            interface_object_type,
            object_instance,
            property_id,
            data: apci_bytes[7..].to_vec(),
        })
    }
}

impl FunctionPropertyExtStateRead {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let header = encode_function_property_ext_header(
            self.interface_object_type,
            self.object_instance,
            self.property_id,
        )?;
        let mut result = APCI_FUNCTION_PROPERTY_EXT_STATE_READ.to_vec();
        result.extend_from_slice(&header);
        result.extend_from_slice(&self.data);
        Ok(result)
    }

    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        if apci_bytes.len() < 7 || apci_bytes[..2] != APCI_FUNCTION_PROPERTY_EXT_STATE_READ {
            return Err(ProtocolError::InvalidFrame {
                details: format!(
                    "Expected FunctionPropertyExtStateRead APCI 0x01D5, got {} bytes",
                    apci_bytes.len()
                ),
            }
            .into());
        }
        let (interface_object_type, object_instance, property_id) =
            decode_function_property_ext_header(apci_bytes);
        Ok(Self {
            interface_object_type,
            object_instance,
            property_id,
            data: apci_bytes[7..].to_vec(),
        })
    }
}

impl FunctionPropertyStateResponse {
    /// Encode A_FunctionPropertyState_Response.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let Some(return_code) = self.return_code else {
            if !self.data.is_empty() {
                return Err(ProtocolError::InvalidFrame {
                    details: "Function-property data requires a return code".to_owned(),
                }
                .into());
            }
            return Ok(vec![
                APCI_FUNCTION_PROPERTY_STATE_RESPONSE[0],
                APCI_FUNCTION_PROPERTY_STATE_RESPONSE[1],
                self.object_index,
                self.property_id,
            ]);
        };
        let mut result = vec![
            APCI_FUNCTION_PROPERTY_STATE_RESPONSE[0],
            APCI_FUNCTION_PROPERTY_STATE_RESPONSE[1],
            self.object_index,
            self.property_id,
            return_code,
        ];
        result.extend_from_slice(&self.data);
        Ok(result)
    }

    /// Decode A_FunctionPropertyState_Response; a four-byte response has no return code.
    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        if apci_bytes.len() < 4 || apci_bytes[..2] != APCI_FUNCTION_PROPERTY_STATE_RESPONSE {
            return Err(ProtocolError::InvalidFrame {
                details: format!(
                    "Expected FunctionPropertyStateResponse APCI 0x02C9, got {} bytes",
                    apci_bytes.len()
                ),
            }
            .into());
        }
        let (return_code, data) = if apci_bytes.len() == 4 {
            (None, Vec::new())
        } else {
            (Some(apci_bytes[4]), apci_bytes[5..].to_vec())
        };
        Ok(Self {
            object_index: apci_bytes[2],
            property_id: apci_bytes[3],
            return_code,
            data,
        })
    }
}

impl FunctionPropertyExtStateResponse {
    /// Encode A_FunctionPropertyExtState_Response.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let header = encode_function_property_ext_header(
            self.interface_object_type,
            self.object_instance,
            self.property_id,
        )?;
        let mut result = APCI_FUNCTION_PROPERTY_EXT_STATE_RESPONSE.to_vec();
        result.extend_from_slice(&header);
        result.push(self.return_code);
        result.extend_from_slice(&self.data);
        Ok(result)
    }

    /// Decode A_FunctionPropertyExtState_Response, preserving function-specific return codes.
    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        if apci_bytes.len() < 8 || apci_bytes[..2] != APCI_FUNCTION_PROPERTY_EXT_STATE_RESPONSE {
            return Err(ProtocolError::InvalidFrame {
                details: format!(
                    "Expected FunctionPropertyExtStateResponse APCI 0x01D6, got {} bytes",
                    apci_bytes.len()
                ),
            }
            .into());
        }
        let (interface_object_type, object_instance, property_id) =
            decode_function_property_ext_header(apci_bytes);
        Ok(Self {
            interface_object_type,
            object_instance,
            property_id,
            return_code: apci_bytes[7],
            data: apci_bytes[8..].to_vec(),
        })
    }
}

impl PropertyDescriptionRead {
    /// Encode the request as APCI bytes, including service `0x03D8`.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        vec![
            APCI_HIGH_BYTE,
            0xD8,
            self.object_index,
            self.property_id,
            self.property_index,
        ]
    }

    /// Decode a five-byte `A_PropertyDescription_Read` APCI frame.
    ///
    /// # Errors
    ///
    /// Returns `ProtocolError::InvalidFrame` for a different service or length.
    pub fn decode(apci_bytes: &[u8]) -> Result<Self> {
        if apci_bytes.len() != 5 || apci_bytes[..2] != [APCI_HIGH_BYTE, 0xD8] {
            return Err(ProtocolError::InvalidFrame {
                details: format!(
                    "Expected 5-byte PropertyDescriptionRead APCI 0x03D8, got {} bytes",
                    apci_bytes.len()
                ),
            }
            .into());
        }

        Ok(Self {
            object_index: apci_bytes[2],
            property_id: apci_bytes[3],
            property_index: apci_bytes[4],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorizeRequest, AuthorizeResponse, DeviceDescriptorRead, DeviceDescriptorResponse,
        FunctionPropertyCommand, FunctionPropertyExtCommand, FunctionPropertyExtStateRead,
        FunctionPropertyExtStateResponse, FunctionPropertyService, FunctionPropertyStateRead,
        FunctionPropertyStateResponse, MemoryRead, MemoryResponse, MemoryWrite,
        PropertyDescriptionRead, PropertyDescriptionResponse, PropertyValueRead,
        PropertyValueResponse, PropertyValueWrite, RestartMasterReset, RestartMasterResetResponse,
        UserMemoryRead, UserMemoryResponse, UserMemoryWrite,
    };

    #[test]
    fn function_property_service_dispatches_all_six_apci_codes() {
        for (apdu, expected) in [
            (&[0x02, 0xC7, 1, 2][..], 0),
            (&[0x02, 0xC8, 1, 2][..], 1),
            (&[0x02, 0xC9, 1, 2][..], 2),
            (&[0x01, 0xD4, 0, 1, 0, 0, 2][..], 3),
            (&[0x01, 0xD5, 0, 1, 0, 0, 2][..], 4),
            (&[0x01, 0xD6, 0, 1, 0, 0, 2, 0x42][..], 5),
        ] {
            let service = FunctionPropertyService::decode(apdu).unwrap();
            let actual = match service {
                FunctionPropertyService::Command(_) => 0,
                FunctionPropertyService::StateRead(_) => 1,
                FunctionPropertyService::StateResponse(_) => 2,
                FunctionPropertyService::ExtCommand(_) => 3,
                FunctionPropertyService::ExtStateRead(_) => 4,
                FunctionPropertyService::ExtStateResponse(_) => 5,
            };
            assert_eq!(actual, expected);
        }
        assert!(FunctionPropertyService::decode(&[]).is_err());
        assert!(FunctionPropertyService::decode(&[0x01, 0xD7]).is_err());
    }

    #[test]
    fn function_property_request_codecs_round_trip_standard_and_extended_services() {
        let command = FunctionPropertyCommand {
            object_index: 2,
            property_id: 7,
            data: vec![0xAA],
        };
        assert_eq!(command.encode(), [0x02, 0xC7, 2, 7, 0xAA]);
        assert_eq!(
            FunctionPropertyCommand::decode(&command.encode()).unwrap(),
            command
        );

        let read = FunctionPropertyStateRead {
            object_index: 2,
            property_id: 7,
            data: vec![0xBB],
        };
        assert_eq!(read.encode(), [0x02, 0xC8, 2, 7, 0xBB]);
        assert_eq!(
            FunctionPropertyStateRead::decode(&read.encode()).unwrap(),
            read
        );

        let command = FunctionPropertyExtCommand {
            interface_object_type: 0x1234,
            object_instance: 0xABC,
            property_id: 0xDEF,
            data: vec![0x55],
        };
        assert_eq!(
            command.encode().unwrap(),
            [0x01, 0xD4, 0x12, 0x34, 0xAB, 0xCD, 0xEF, 0x55]
        );
        assert_eq!(
            FunctionPropertyExtCommand::decode(&command.encode().unwrap()).unwrap(),
            command
        );

        let read = FunctionPropertyExtStateRead {
            interface_object_type: 0x1234,
            object_instance: 0xABC,
            property_id: 0xDEF,
            data: vec![0x66],
        };
        assert_eq!(
            read.encode().unwrap(),
            [0x01, 0xD5, 0x12, 0x34, 0xAB, 0xCD, 0xEF, 0x66]
        );
        assert_eq!(
            FunctionPropertyExtStateRead::decode(&read.encode().unwrap()).unwrap(),
            read
        );

        assert!(FunctionPropertyCommand::decode(&[0x02, 0xC7, 1]).is_err());
        assert!(FunctionPropertyStateRead::decode(&[0x02, 0xC7, 1, 2]).is_err());
        assert!(FunctionPropertyExtCommand::decode(&[0x01, 0xD4, 1, 2, 3, 4]).is_err());
        assert!(FunctionPropertyExtStateRead::decode(&[0x01, 0xD4, 1, 2, 3, 4, 5]).is_err());
    }

    #[test]
    fn function_property_state_response_distinguishes_missing_and_zero_return_codes() {
        let negative = FunctionPropertyStateResponse {
            object_index: 2,
            property_id: 7,
            return_code: None,
            data: vec![],
        };
        assert_eq!(negative.encode().unwrap(), [0x02, 0xC9, 2, 7]);
        assert_eq!(
            FunctionPropertyStateResponse::decode(&negative.encode().unwrap()).unwrap(),
            negative
        );

        let success = FunctionPropertyStateResponse {
            return_code: Some(0),
            data: vec![0xAA],
            ..negative
        };
        assert_eq!(success.encode().unwrap(), [0x02, 0xC9, 2, 7, 0, 0xAA]);
        assert_eq!(
            FunctionPropertyStateResponse::decode(&success.encode().unwrap()).unwrap(),
            success
        );
        assert!(
            FunctionPropertyStateResponse {
                return_code: None,
                data: vec![1],
                ..success.clone()
            }
            .encode()
            .is_err()
        );
        assert!(FunctionPropertyStateResponse::decode(&[0x02, 0xC9, 1]).is_err());
        assert!(FunctionPropertyStateResponse::decode(&[0x02, 0xC8, 1, 2]).is_err());
    }

    #[test]
    fn function_property_ext_response_preserves_unknown_codes_and_checks_header() {
        let response = FunctionPropertyExtStateResponse {
            interface_object_type: 0x1234,
            object_instance: 0xABC,
            property_id: 0xDEF,
            return_code: 0x42,
            data: vec![0x10, 0x20],
        };
        let encoded = response.encode().unwrap();
        assert_eq!(
            encoded,
            [0x01, 0xD6, 0x12, 0x34, 0xAB, 0xCD, 0xEF, 0x42, 0x10, 0x20]
        );
        assert_eq!(
            FunctionPropertyExtStateResponse::decode(&encoded).unwrap(),
            response
        );
        assert!(FunctionPropertyExtStateResponse::decode(&[0x01, 0xD6, 0, 1, 2, 3, 4]).is_err());
        assert!(
            FunctionPropertyExtStateResponse {
                object_instance: 0x1000,
                ..response
            }
            .encode()
            .is_err()
        );
    }

    #[test]
    fn device_descriptor_read_encodes_to_apci_0x0300() {
        assert_eq!(
            DeviceDescriptorRead { descriptor: 0 }.encode(),
            vec![0x03, 0x00]
        );
        assert_eq!(
            DeviceDescriptorRead { descriptor: 0x3F }.encode(),
            vec![0x03, 0x3F]
        );
    }

    #[test]
    fn device_descriptor_read_masks_descriptor_to_six_bits() {
        assert_eq!(
            DeviceDescriptorRead { descriptor: 0xFF }.encode(),
            vec![0x03, 0x3F]
        );
    }

    #[test]
    fn device_descriptor_response_round_trip() {
        let response = DeviceDescriptorResponse {
            descriptor: 0,
            data: vec![0x07, 0xB0],
        };
        let encoded = response.encode();
        assert_eq!(encoded, vec![0x03, 0x40, 0x07, 0xB0]);
        assert_eq!(
            DeviceDescriptorResponse::decode(&encoded).unwrap(),
            response
        );
    }

    #[test]
    fn device_descriptor_response_decodes_known_bytes() {
        let decoded = DeviceDescriptorResponse::decode(&[0x03, 0x40, 0x07, 0xB0]).unwrap();
        assert_eq!(decoded.descriptor, 0);
        assert_eq!(decoded.data, vec![0x07, 0xB0]);
    }

    #[test]
    fn device_descriptor_response_preserves_descriptor_bits() {
        let decoded = DeviceDescriptorResponse::decode(&[0x03, 0x42]).unwrap();
        assert_eq!(decoded.descriptor, 0x02);
        assert_eq!(decoded.data, [] as [u8; 0]);
    }

    #[test]
    fn device_descriptor_response_rejects_non_0x0340_pattern() {
        assert!(DeviceDescriptorResponse::decode(&[0x03, 0x00]).is_err());
        assert!(DeviceDescriptorResponse::decode(&[0x00, 0x40]).is_err());
    }

    #[test]
    fn device_descriptor_response_rejects_short_input() {
        assert!(DeviceDescriptorResponse::decode(&[]).is_err());
        assert!(DeviceDescriptorResponse::decode(&[0x03]).is_err());
    }

    #[test]
    fn property_description_response_round_trip_and_accessors() {
        let response = PropertyDescriptionResponse {
            object_index: 1,
            property_id: 2,
            property_index: 3,
            type_: 0xB5,
            max_count: 0x123,
            access: 0xA7,
        };
        let encoded = response.encode();
        assert_eq!(encoded, [0x03, 0xD9, 1, 2, 3, 0xB5, 0x01, 0x23, 0xA7]);
        assert_eq!(
            PropertyDescriptionResponse::decode(&encoded).unwrap(),
            response
        );
        assert!(response.writable());
        assert_eq!(response.pdt(), 0x35);
        assert_eq!(response.read_level(), 0x0A);
        assert_eq!(response.write_level(), 0x07);
    }

    #[test]
    fn property_description_response_masks_count_and_preserves_unknown_pdt() {
        let encoded = [0x03, 0xD9, 0, 1, 2, 0xFF, 0xF1, 0x23, 0x4D];
        let decoded = PropertyDescriptionResponse::decode(&encoded).unwrap();
        assert_eq!(decoded.max_count, 0x0123);
        assert_eq!(decoded.pdt(), 0x3F);
        assert_eq!(decoded.type_, 0xFF);
        assert_eq!(
            decoded.encode(),
            [0x03, 0xD9, 0, 1, 2, 0xFF, 0x01, 0x23, 0x4D]
        );
    }

    #[test]
    fn property_description_response_rejects_wrong_service_and_length() {
        assert!(PropertyDescriptionResponse::decode(&[0x03, 0xD8, 0, 1, 2, 3, 0, 1, 0]).is_err());
        assert!(PropertyDescriptionResponse::decode(&[0x03, 0xD9]).is_err());
        assert!(
            PropertyDescriptionResponse::decode(&[0x03, 0xD9, 0, 1, 2, 3, 0, 1, 0, 0]).is_err()
        );
    }

    #[test]
    fn property_description_read_round_trip_and_validation() {
        let read = PropertyDescriptionRead {
            object_index: 1,
            property_id: 2,
            property_index: 3,
        };
        let encoded = read.encode();
        assert_eq!(encoded, [0x03, 0xD8, 1, 2, 3]);
        assert_eq!(PropertyDescriptionRead::decode(&encoded).unwrap(), read);
        assert!(PropertyDescriptionRead::decode(&[0x03, 0xD9, 1, 2, 3]).is_err());
        assert!(PropertyDescriptionRead::decode(&[0x03, 0xD8, 1, 2]).is_err());
        assert!(PropertyDescriptionRead::decode(&[0x03, 0xD8, 1, 2, 3, 4]).is_err());
    }

    #[test]
    fn property_value_read_encodes_count_and_full_12_bit_index() {
        let read = PropertyValueRead {
            object_index: 1,
            property_id: 2,
            count: 15,
            start_index: 0x0FFF,
        };
        let encoded = read.encode().unwrap();
        assert_eq!(encoded, [0x03, 0xD5, 1, 2, 0xFF, 0xFF]);
        assert_eq!(PropertyValueRead::decode(&encoded).unwrap(), read);
        assert!(PropertyValueRead::decode(&[0x03, 0xD5, 1, 2, 0xFF, 0xFF, 0]).is_err());
    }

    #[test]
    fn property_value_response_and_write_round_trip() {
        let response = PropertyValueResponse {
            object_index: 1,
            property_id: 2,
            count: 2,
            start_index: 0x123,
            data: vec![0xAA, 0xBB],
        };
        let encoded_response = response.encode().unwrap();
        assert_eq!(encoded_response, [0x03, 0xD6, 1, 2, 0x21, 0x23, 0xAA, 0xBB]);
        assert_eq!(
            PropertyValueResponse::decode(&encoded_response).unwrap(),
            response
        );

        let write = PropertyValueWrite {
            object_index: 3,
            property_id: 4,
            count: 1,
            start_index: 0xABC,
            data: vec![0x55],
        };
        let encoded_write = write.encode().unwrap();
        assert_eq!(encoded_write, [0x03, 0xD7, 3, 4, 0x1A, 0xBC, 0x55]);
        assert_eq!(PropertyValueWrite::decode(&encoded_write).unwrap(), write);
    }

    #[test]
    fn authorize_apci_codecs_match_wire_vectors_without_debugging_the_key() {
        let request = AuthorizeRequest { key: 0x1234_5678 };
        let encoded = request.encode();
        assert_eq!(encoded, [0x03, 0xD1, 0, 0x12, 0x34, 0x56, 0x78]);
        assert_eq!(AuthorizeRequest::decode(&encoded).unwrap().key, request.key);

        let response = AuthorizeResponse { level: 2 };
        assert_eq!(response.encode(), [0x03, 0xD2, 2]);
        assert_eq!(
            AuthorizeResponse::decode(&response.encode()).unwrap(),
            response
        );
        assert!(AuthorizeRequest::decode(&[0x03, 0xD1]).is_err());
        assert!(AuthorizeResponse::decode(&[0x03, 0xD1, 2]).is_err());
    }

    #[test]
    fn master_reset_apci_codecs_match_wire_vectors() {
        let request = RestartMasterReset {
            erase_code: 1,
            channel_number: 7,
        };
        assert_eq!(request.encode(), [0x03, 0x81, 1, 7]);
        assert_eq!(
            RestartMasterReset::decode(&[0x03, 0x81, 1, 7]).unwrap(),
            request
        );

        let response = RestartMasterResetResponse {
            error_code: 0,
            process_time: 0x1234,
        };
        assert_eq!(response.encode(), [0x03, 0xA1, 0, 0x12, 0x34]);
        assert_eq!(
            RestartMasterResetResponse::decode(&response.encode()).unwrap(),
            response
        );
        assert!(RestartMasterReset::decode(&[0x03, 0x80]).is_err());
        assert!(RestartMasterResetResponse::decode(&[0x03, 0xA1]).is_err());
    }

    #[test]
    fn memory_and_user_memory_apci_codecs_match_wire_vectors() {
        let read = MemoryRead {
            address: 0x1234,
            count: 3,
        };
        assert_eq!(read.encode().unwrap(), [0x02, 0x03, 0x12, 0x34]);
        assert_eq!(MemoryRead::decode(&[0x02, 0x03, 0x12, 0x34]).unwrap(), read);
        let response = MemoryResponse {
            address: 0x1234,
            count: 2,
            data: vec![0xAA, 0xBB],
        };
        let encoded = response.encode().unwrap();
        assert_eq!(encoded, [0x02, 0x42, 0x12, 0x34, 0xAA, 0xBB]);
        assert_eq!(MemoryResponse::decode(&encoded).unwrap(), response);
        let write = MemoryWrite {
            address: 0x1234,
            count: 2,
            data: vec![0xAA, 0xBB],
        };
        let encoded = write.encode().unwrap();
        assert_eq!(encoded, [0x02, 0x82, 0x12, 0x34, 0xAA, 0xBB]);
        assert_eq!(MemoryWrite::decode(&encoded).unwrap(), write);

        let user_read = UserMemoryRead {
            address: 0xABCDE,
            count: 3,
        };
        let encoded = user_read.encode().unwrap();
        assert_eq!(encoded, [0x02, 0xC0, 0xA3, 0xBC, 0xDE]);
        assert_eq!(UserMemoryRead::decode(&encoded).unwrap(), user_read);
        let user_response = UserMemoryResponse {
            address: 0xABCDE,
            count: 2,
            data: vec![0xAA, 0xBB],
        };
        let encoded = user_response.encode().unwrap();
        assert_eq!(encoded, [0x02, 0xC1, 0xA2, 0xBC, 0xDE, 0xAA, 0xBB]);
        assert_eq!(UserMemoryResponse::decode(&encoded).unwrap(), user_response);
        let user_write = UserMemoryWrite {
            address: 0xABCDE,
            count: 2,
            data: vec![0xAA, 0xBB],
        };
        let encoded = user_write.encode().unwrap();
        assert_eq!(encoded, [0x02, 0xC2, 0xA2, 0xBC, 0xDE, 0xAA, 0xBB]);
        assert_eq!(UserMemoryWrite::decode(&encoded).unwrap(), user_write);
    }

    #[test]
    fn memory_codecs_reject_invalid_services_lengths_and_fields() {
        assert!(MemoryRead::decode(&[0x02, 0x03, 0x12]).is_err());
        assert!(MemoryRead::decode(&[0x02, 0x43, 0x12, 0x34]).is_err());
        assert!(MemoryResponse::decode(&[0x02, 0x02, 0x12, 0x34]).is_err());
        assert!(
            MemoryRead {
                address: 0,
                count: 64
            }
            .encode()
            .is_err()
        );
        assert!(UserMemoryRead::decode(&[0x02, 0xC1, 0, 0, 0]).is_err());
        assert!(
            UserMemoryRead {
                address: 0x10_0000,
                count: 1,
            }
            .encode()
            .is_err()
        );
        assert!(
            UserMemoryRead {
                address: 0,
                count: 16,
            }
            .encode()
            .is_err()
        );
    }

    #[test]
    fn property_value_codecs_reject_out_of_range_fields_and_malformed_frames() {
        assert!(
            PropertyValueRead {
                object_index: 0,
                property_id: 1,
                count: 16,
                start_index: 0,
            }
            .encode()
            .is_err()
        );
        assert!(
            PropertyValueRead {
                object_index: 0,
                property_id: 1,
                count: 1,
                start_index: 4096,
            }
            .encode()
            .is_err()
        );
        assert!(PropertyValueResponse::decode(&[0x03, 0xD5, 0, 1, 0x10, 0]).is_err());
        assert!(PropertyValueWrite::decode(&[0x03, 0xD7, 0, 1, 0x10]).is_err());
    }
}
