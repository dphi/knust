//! DPT 20.xxx - HVAC values
//!
// TODO: DPTHVACStatus (the DPT 20 status type, a bitfield of
// fault/alarm/warning/etc. flags) is not implemented here — only HVACMode
// (20.102) and HVACControllerMode (20.105) are. A device/config reading a
// group address typed as this DPT will fail to decode.

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 20.102 - HVAC Mode
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HVACMode {
    value: u8,
    data: [u8; 1],
}

impl HVACMode {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `value > 4`.
    pub fn new(value: u8) -> Result<Self> {
        if value > 4 {
            return Err(ProtocolError::DptError {
                dpt_type: "20.102".to_string(),
                details: format!("HVAC mode {value} out of range [0-4]"),
            }
            .into());
        }

        Ok(Self {
            value,
            data: [value],
        })
    }

    #[must_use]
    pub fn value(&self) -> u8 {
        self.value
    }
}

impl DptValue for HVACMode {
    const DPT_NUMBER: &'static str = "20.102";
    const VALUE_TYPE: &'static str = "hvac_mode";
    const BYTE_LENGTH: usize = 1;

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
        Self::new(bytes[0])
    }

    fn validate(&self) -> Result<()> {
        // HVAC modes: 0=Auto, 1=Comfort, 2=Standby, 3=Economy, 4=Building Protection
        if self.value > 4 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("HVAC mode {} out of range [0-4]", self.value),
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
        (0.0, 4.0)
    }
}

/// DPT 20.105 - HVAC Controller Mode
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HVACControllerMode {
    value: u8,
    data: [u8; 1],
}

impl HVACControllerMode {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `value > 17` and `value != 20`.
    pub fn new(value: u8) -> Result<Self> {
        if value > 17 && value != 20 {
            return Err(ProtocolError::DptError {
                dpt_type: "20.105".to_string(),
                details: format!(
                    "HVAC controller mode {value} out of Python-compatible range [0-17, 20]"
                ),
            }
            .into());
        }

        Ok(Self {
            value,
            data: [value],
        })
    }

    #[must_use]
    pub fn value(&self) -> u8 {
        self.value
    }
}

impl DptValue for HVACControllerMode {
    const DPT_NUMBER: &'static str = "20.105";
    const VALUE_TYPE: &'static str = "hvac_controller_mode";
    const BYTE_LENGTH: usize = 1;

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
        Self::new(bytes[0])
    }

    fn validate(&self) -> Result<()> {
        if self.value > 17 && self.value != 20 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!(
                    "HVAC controller mode {} out of Python-compatible range [0-17, 20]",
                    self.value
                ),
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
        (0.0, 20.0)
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for HVACMode {
    type InnerType = u8;
    fn new(value: u8) -> Self {
        HVACMode::new(value).unwrap()
    }
    fn into_inner(self) -> u8 {
        self.value
    }
}

impl DptInnerType for HVACControllerMode {
    type InnerType = u8;
    fn new(value: u8) -> Self {
        HVACControllerMode::new(value).unwrap()
    }
    fn into_inner(self) -> u8 {
        self.value
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(DPTHVACMode, 20, 102, HVACMode, "hvac_mode", None, None);
dpt_alias!(
    DPTHVACContrMode,
    20,
    105,
    HVACControllerMode,
    "hvac_controller_mode",
    None,
    None
);
