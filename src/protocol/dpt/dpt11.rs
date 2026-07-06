//! DPT 11.xxx - Date values

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 11.001 - Date
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Date {
    pub day: u8,   // 1-31
    pub month: u8, // 1-12
    pub year: u8,  // 0-99 (represents 1990-2089)
    data: [u8; 3],
}

impl Date {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `day` is 0 or > 31, `month` is
    /// 0 or > 12, or `year > 99`.
    pub fn new(day: u8, month: u8, year: u8) -> Result<Self> {
        let value = Self {
            day,
            month,
            year,
            data: [day, month, year],
        };
        value.validate()?;
        Ok(value)
    }
}

impl DptValue for Date {
    const DPT_NUMBER: &'static str = "11.001";
    const VALUE_TYPE: &'static str = "date";
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

        let day = bytes[0] & 0x1F;
        let month = bytes[1] & 0x0F;
        let year = bytes[2] & 0x7F;

        Self::new(day, month, year)
    }

    fn validate(&self) -> Result<()> {
        if self.day == 0 || self.day > 31 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Day {} out of range [1-31]", self.day),
            }
            .into());
        }
        if self.month == 0 || self.month > 12 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Month {} out of range [1-12]", self.month),
            }
            .into());
        }
        if self.year > 99 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Year {} out of range [0-99]", self.year),
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
