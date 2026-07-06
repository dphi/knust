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

#[cfg(test)]
mod tests {
    use super::{DeviceDescriptorRead, DeviceDescriptorResponse};

    #[test]
    fn device_descriptor_read_encodes_to_apci_0x0300() {
        // Descriptor 0 -> [0x03, 0x00] (APCI 0x0300).
        assert_eq!(
            DeviceDescriptorRead { descriptor: 0 }.encode(),
            vec![0x03, 0x00]
        );
        // Descriptor bits live in the low six bits of the second byte.
        assert_eq!(
            DeviceDescriptorRead { descriptor: 0x3F }.encode(),
            vec![0x03, 0x3F]
        );
    }

    #[test]
    fn device_descriptor_read_masks_descriptor_to_six_bits() {
        // High bits of the descriptor must not bleed into the APCI service bits.
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
        // [0x03, 0x40, 0x07, 0xB0] is a descriptor-0 response carrying 0x07B0.
        let decoded = DeviceDescriptorResponse::decode(&[0x03, 0x40, 0x07, 0xB0]).unwrap();
        assert_eq!(decoded.descriptor, 0);
        assert_eq!(decoded.data, vec![0x07, 0xB0]);
    }

    #[test]
    fn device_descriptor_response_preserves_descriptor_bits() {
        // Second byte 0x42 -> response bits 0x40 + descriptor 0x02.
        let decoded = DeviceDescriptorResponse::decode(&[0x03, 0x42]).unwrap();
        assert_eq!(decoded.descriptor, 0x02);
        assert!(decoded.data.is_empty());
    }

    #[test]
    fn device_descriptor_response_rejects_non_0x0340_pattern() {
        // A read pattern (APCI 0x0300) must not decode as a response.
        assert!(DeviceDescriptorResponse::decode(&[0x03, 0x00]).is_err());
        // An unrelated APCI high-bit pattern must also be rejected.
        assert!(DeviceDescriptorResponse::decode(&[0x00, 0x40]).is_err());
    }

    #[test]
    fn device_descriptor_response_rejects_short_input() {
        assert!(DeviceDescriptorResponse::decode(&[]).is_err());
        assert!(DeviceDescriptorResponse::decode(&[0x03]).is_err());
    }
}
