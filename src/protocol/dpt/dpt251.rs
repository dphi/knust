//! DPT 251.xxx - RGBW Color

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 251.600 - RGBW Color
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColorRGBW {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub white: u8,
    pub red_valid: bool,
    pub green_valid: bool,
    pub blue_valid: bool,
    pub white_valid: bool,
    data: [u8; 6],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ColorRGBWValidity {
    pub red: bool,
    pub green: bool,
    pub blue: bool,
    pub white: bool,
}

impl ColorRGBW {
    #[must_use]
    pub fn new(red: u8, green: u8, blue: u8, white: u8) -> Self {
        Self::new_with_validity(
            red,
            green,
            blue,
            white,
            ColorRGBWValidity {
                red: true,
                green: true,
                blue: true,
                white: true,
            },
        )
    }

    #[must_use]
    pub fn new_with_validity(
        red: u8,
        green: u8,
        blue: u8,
        white: u8,
        validity: ColorRGBWValidity,
    ) -> Self {
        let data = [
            if validity.red { red } else { 0 },
            if validity.green { green } else { 0 },
            if validity.blue { blue } else { 0 },
            if validity.white { white } else { 0 },
            0,
            (u8::from(validity.red) << 3)
                | (u8::from(validity.green) << 2)
                | (u8::from(validity.blue) << 1)
                | u8::from(validity.white),
        ];

        Self {
            red,
            green,
            blue,
            white,
            red_valid: validity.red,
            green_valid: validity.green,
            blue_valid: validity.blue,
            white_valid: validity.white,
            data,
        }
    }
}

impl DptValue for ColorRGBW {
    const DPT_NUMBER: &'static str = "251.600";
    const VALUE_TYPE: &'static str = "color_rgbw";
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
        Ok(Self::new_with_validity(
            bytes[0],
            bytes[1],
            bytes[2],
            bytes[3],
            ColorRGBWValidity {
                red: ((bytes[5] >> 3) & 0x01) != 0,
                green: ((bytes[5] >> 2) & 0x01) != 0,
                blue: ((bytes[5] >> 1) & 0x01) != 0,
                white: (bytes[5] & 0x01) != 0,
            },
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
        (0.0, 4_294_967_295.0) // 32-bit RGBW
    }
}
