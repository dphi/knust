//! DPT 242.xxx - XYY Color

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 242.600 - XYY Color
#[derive(Debug, Clone, PartialEq)]
pub struct ColorXYY {
    pub x: u16,
    pub y: u16,
    pub brightness: u8,
    pub color_valid: bool,
    pub brightness_valid: bool,
    data: [u8; 6],
}

impl ColorXYY {
    #[must_use]
    pub fn new(x: u16, y: u16, brightness: u8) -> Self {
        Self::new_with_validity(x, y, brightness, true, true)
    }

    #[must_use]
    pub fn new_with_validity(
        x: u16,
        y: u16,
        brightness: u8,
        color_valid: bool,
        brightness_valid: bool,
    ) -> Self {
        let x_bytes = if color_valid { x.to_be_bytes() } else { [0, 0] };
        let y_bytes = if color_valid { y.to_be_bytes() } else { [0, 0] };
        let encoded_brightness = if brightness_valid { brightness } else { 0 };
        let data = [
            x_bytes[0],
            x_bytes[1],
            y_bytes[0],
            y_bytes[1],
            encoded_brightness,
            (u8::from(color_valid) << 1) | u8::from(brightness_valid),
        ];

        Self {
            x,
            y,
            brightness,
            color_valid,
            brightness_valid,
            data,
        }
    }
}

impl DptValue for ColorXYY {
    const DPT_NUMBER: &'static str = "242.600";
    const VALUE_TYPE: &'static str = "color_xyy";
    const BYTE_LENGTH: usize = 6;

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
        let x = u16::from_be_bytes([bytes[0], bytes[1]]);
        let y = u16::from_be_bytes([bytes[2], bytes[3]]);
        let brightness = bytes[4];
        Ok(Self::new_with_validity(
            x,
            y,
            brightness,
            ((bytes[5] >> 1) & 0x01) != 0,
            (bytes[5] & 0x01) != 0,
        ))
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
        (0.0, 16_777_215.0)
    }
}
