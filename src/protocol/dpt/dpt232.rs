//! DPT 232.xxx - RGB Color

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 232.600 - RGB Color
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColorRGB {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    data: [u8; 3],
}

impl ColorRGB {
    #[must_use]
    pub fn new(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red,
            green,
            blue,
            data: [red, green, blue],
        }
    }
}

impl DptValue for ColorRGB {
    const DPT_NUMBER: &'static str = "232.600";
    const VALUE_TYPE: &'static str = "color_rgb";
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
        Ok(Self::new(bytes[0], bytes[1], bytes[2]))
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::decode(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    fn value_range() -> (f64, f64) {
        (0.0, 16_777_215.0) // 24-bit RGB
    }
}
