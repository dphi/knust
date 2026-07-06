//! DPT 10.xxx - Time values

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 10.001 - Time of Day
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TimeOfDay {
    pub day: u8,    // 0-7 (0=no day, 1=Monday, ..., 7=Sunday)
    pub hour: u8,   // 0-23
    pub minute: u8, // 0-59
    pub second: u8, // 0-59
    data: [u8; 3],
}

impl TimeOfDay {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `day > 7`, `hour > 23`,
    /// `minute > 59`, or `second > 59`.
    pub fn new(day: u8, hour: u8, minute: u8, second: u8) -> Result<Self> {
        let value = Self {
            day,
            hour,
            minute,
            second,
            data: [(day << 5) | hour, minute, second],
        };
        value.validate()?;
        Ok(value)
    }
}

impl DptValue for TimeOfDay {
    const DPT_NUMBER: &'static str = "10.001";
    const VALUE_TYPE: &'static str = "time";
    const BYTE_LENGTH: usize = 3;

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

        let day = (bytes[0] >> 5) & 0x07;
        let hour = bytes[0] & 0x1F;
        let minute = bytes[1] & 0x3F;
        let second = bytes[2] & 0x3F;

        Self::new(day, hour, minute, second)
    }

    fn validate(&self) -> Result<()> {
        if self.day > 7 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Day {} out of range [0-7]", self.day),
            }
            .into());
        }
        if self.hour > 23 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Hour {} out of range [0-23]", self.hour),
            }
            .into());
        }
        if self.minute > 59 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Minute {} out of range [0-59]", self.minute),
            }
            .into());
        }
        if self.second > 59 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Second {} out of range [0-59]", self.second),
            }
            .into());
        }
        Ok(())
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::decode(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    fn value_range() -> (f64, f64) {
        (0.0, 16_777_215.0) // 24-bit value
    }
}
