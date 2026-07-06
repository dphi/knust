//! DPT 16.xxx - String values

use super::{DptValue, Result};
use crate::error::ProtocolError;

fn validate_string_length(value: &str, dpt_type: &'static str) -> Result<()> {
    if value.chars().count() > 14 {
        return Err(ProtocolError::DptError {
            dpt_type: dpt_type.to_string(),
            details: format!("String too long: {} > {}", value.chars().count(), 14),
        }
        .into());
    }

    Ok(())
}

fn encode_ascii_payload(value: &str) -> Result<[u8; 14]> {
    validate_string_length(value, "16.000")?;

    let mut payload = [0u8; 14];
    for (index, character) in value.chars().enumerate() {
        payload[index] = if character.is_ascii() {
            character as u8
        } else {
            b'?'
        };
    }

    Ok(payload)
}

fn encode_latin1_payload(value: &str) -> Result<[u8; 14]> {
    validate_string_length(value, "16.001")?;

    let mut payload = [0u8; 14];
    for (index, character) in value.chars().enumerate() {
        payload[index] = if (character as u32) <= 0xFF {
            character as u8
        } else {
            b'?'
        };
    }

    Ok(payload)
}

fn decode_ascii_value(bytes: &[u8]) -> String {
    bytes
        .iter()
        .copied()
        .filter(|byte| *byte != 0)
        .map(|byte| if byte.is_ascii() { byte as char } else { '�' })
        .collect()
}

fn decode_latin1_value(bytes: &[u8]) -> String {
    bytes
        .iter()
        .copied()
        .filter(|byte| *byte != 0)
        .map(char::from)
        .collect()
}

/// DPT 16.000 - String (ASCII)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StringAscii {
    value: String,
    payload: [u8; 14],
}

impl StringAscii {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `value` is longer than 14 characters.
    pub fn new(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref();
        let payload = encode_ascii_payload(value)?;
        Ok(Self {
            value: value.to_string(),
            payload,
        })
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl DptValue for StringAscii {
    const DPT_NUMBER: &'static str = "16.000";
    const VALUE_TYPE: &'static str = "string";
    const BYTE_LENGTH: usize = 14;

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::BYTE_LENGTH {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!(
                    "Invalid length: expected {}, got {}",
                    Self::BYTE_LENGTH,
                    bytes.len()
                ),
            }
            .into());
        }

        Ok(Self {
            value: decode_ascii_value(bytes),
            payload: bytes.try_into().expect("validated 14-byte DPT16 payload"),
        })
    }

    fn validate(&self) -> Result<()> {
        validate_string_length(&self.value, Self::DPT_NUMBER)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::decode(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.payload
    }

    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}

/// DPT 16.001 - String (Latin-1)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StringLatin1 {
    value: String,
    payload: [u8; 14],
}

impl StringLatin1 {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `value` is longer than 14 characters.
    pub fn new(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref();
        let payload = encode_latin1_payload(value)?;
        Ok(Self {
            value: value.to_string(),
            payload,
        })
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl DptValue for StringLatin1 {
    const DPT_NUMBER: &'static str = "16.001";
    const VALUE_TYPE: &'static str = "latin_1";
    const BYTE_LENGTH: usize = 14;

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::BYTE_LENGTH {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!(
                    "Invalid length: expected {}, got {}",
                    Self::BYTE_LENGTH,
                    bytes.len()
                ),
            }
            .into());
        }

        Ok(Self {
            value: decode_latin1_value(bytes),
            payload: bytes.try_into().expect("validated 14-byte DPT16 payload"),
        })
    }

    fn validate(&self) -> Result<()> {
        validate_string_length(&self.value, Self::DPT_NUMBER)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::decode(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.payload
    }

    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
