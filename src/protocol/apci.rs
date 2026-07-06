//! APCI helpers for KNX application-layer group communication.
//!
// TODO: Only A_GroupValue_* services are implemented. Management APCI
// services (individual address read/write, device descriptor read,
// property/memory read-write, restart, etc.) don't exist — this crate
// cannot act as an ETS-style commissioning/management client, only a
// group-communication one.

use crate::error::{ProtocolError, Result};
use crate::log_protocol;
use crate::logging::LogLevel;

/// KNX group value APCI service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupValueService {
    /// `A_GroupValue_Read` with no payload.
    Read,
    /// `A_GroupValue_Response` carrying a DPT payload.
    Response(Vec<u8>),
    /// `A_GroupValue_Write` carrying a DPT payload.
    Write(Vec<u8>),
}

impl GroupValueService {
    /// Encode this group service into CEMI APCI/data bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let result = match self {
            Self::Read => vec![0x00],
            Self::Response(payload) => encode_payload(0x40, payload),
            Self::Write(payload) => encode_payload(0x80, payload),
        };
        log_protocol!(
            LogLevel::Trace,
            "APCI encode: {:?} \u{2192} {} bytes",
            self,
            result.len()
        );
        result
    }

    /// Decode a group service from CEMI TPCI and APCI/data bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `apci_data` is empty or encodes
    /// a service other than `GroupValueRead`/`Response`/`Write`.
    pub fn decode(tpci: u8, apci_data: &[u8]) -> Result<Self> {
        let first_apci = apci_data
            .first()
            .copied()
            .ok_or_else(|| ProtocolError::DptError {
                dpt_type: "APCI".to_string(),
                details: "Missing APCI data".to_string(),
            })?;
        let service = (u16::from(tpci & 0x03) << 8) | u16::from(first_apci & 0xC0);
        log_protocol!(
            LogLevel::Trace,
            "APCI decode: service=0x{:04X} data_len={}",
            service,
            apci_data.len()
        );

        match service {
            0x0000 => Ok(Self::Read),
            0x0040 => Ok(Self::Response(decode_payload(first_apci, apci_data))),
            0x0080 => Ok(Self::Write(decode_payload(first_apci, apci_data))),
            _ => {
                log_protocol!(
                    LogLevel::Warn,
                    "APCI: unsupported service 0x{:04X}",
                    service
                );
                Err(ProtocolError::DptError {
                    dpt_type: "APCI".to_string(),
                    details: format!("Unsupported group value APCI service: 0x{service:04X}"),
                }
                .into())
            }
        }
    }

    /// Borrow the DPT payload for write/response services.
    #[must_use]
    pub fn payload(&self) -> Option<&[u8]> {
        match self {
            Self::Read => None,
            Self::Response(payload) | Self::Write(payload) => Some(payload),
        }
    }
}

fn encode_payload(service_bits: u8, payload: &[u8]) -> Vec<u8> {
    if payload.len() <= 1 {
        vec![service_bits | (payload.first().copied().unwrap_or(0) & 0x3F)]
    } else {
        let mut apci_data = Vec::with_capacity(payload.len() + 1);
        apci_data.push(service_bits);
        apci_data.extend_from_slice(payload);
        apci_data
    }
}

fn decode_payload(first_apci: u8, apci_data: &[u8]) -> Vec<u8> {
    if apci_data.len() == 1 {
        vec![first_apci & 0x3F]
    } else {
        apci_data[1..].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::GroupValueService;

    #[test]
    fn group_value_read_encodes_to_single_zero_apci() {
        assert_eq!(GroupValueService::Read.encode(), vec![0x00]);
    }

    #[test]
    fn group_value_write_binary_round_trip() {
        let encoded = GroupValueService::Write(vec![1]).encode();
        assert_eq!(encoded, vec![0x81]);
        assert_eq!(
            GroupValueService::decode(0x00, &encoded).unwrap(),
            GroupValueService::Write(vec![1])
        );
    }

    #[test]
    fn group_value_response_array_round_trip() {
        let encoded = GroupValueService::Response(vec![0x0c, 0x3f]).encode();
        assert_eq!(encoded, vec![0x40, 0x0c, 0x3f]);
        assert_eq!(
            GroupValueService::decode(0x00, &encoded).unwrap(),
            GroupValueService::Response(vec![0x0c, 0x3f])
        );
    }
}
