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

const APCI_FUNCTION_PROPERTY_STATE_RESPONSE: [u8; 2] = [0x02, 0xC9];
const APCI_FUNCTION_PROPERTY_EXT_STATE_RESPONSE: [u8; 2] = [0x01, 0xD6];

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
        if self.object_instance > 0x0FFF || self.property_id > 0x0FFF {
            return Err(ProtocolError::InvalidFrame {
                details: "Function-property object instance or property ID exceeds 12 bits"
                    .to_owned(),
            }
            .into());
        }
        let mut result = vec![
            APCI_FUNCTION_PROPERTY_EXT_STATE_RESPONSE[0],
            APCI_FUNCTION_PROPERTY_EXT_STATE_RESPONSE[1],
            (self.interface_object_type >> 8) as u8,
            self.interface_object_type as u8,
            (self.object_instance >> 4) as u8,
            ((self.object_instance << 4) as u8) | (self.property_id >> 8) as u8,
            self.property_id as u8,
            self.return_code,
        ];
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
        Ok(Self {
            interface_object_type: u16::from_be_bytes([apci_bytes[2], apci_bytes[3]]),
            object_instance: (u16::from(apci_bytes[4]) << 4) | u16::from(apci_bytes[5] >> 4),
            property_id: (u16::from(apci_bytes[5] & 0x0F) << 8) | u16::from(apci_bytes[6]),
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
        DeviceDescriptorRead, DeviceDescriptorResponse, FunctionPropertyExtStateResponse,
        FunctionPropertyStateResponse, PropertyDescriptionRead, PropertyDescriptionResponse,
        PropertyValueRead, PropertyValueResponse, PropertyValueWrite,
    };

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
