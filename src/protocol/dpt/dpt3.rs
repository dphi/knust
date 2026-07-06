//! DPT 3.xxx - Control values (4-bit)

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 3.007 - Control Dimming
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ControlDimming {
    data: [u8; 1],
}

impl ControlDimming {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `step_code > 7`.
    pub fn new(control: bool, step_code: u8) -> Result<Self> {
        if step_code > 7 {
            return Err(ProtocolError::DptError {
                dpt_type: "3.007".to_string(),
                details: format!("Step code {step_code} out of range [0-7]"),
            }
            .into());
        }
        let control_bit = if control { 0x08 } else { 0x00 };
        let step_code = step_code & 0x07;
        Ok(Self {
            data: [control_bit | step_code],
        })
    }

    #[must_use]
    pub fn control(&self) -> bool {
        (self.data[0] & 0x08) != 0
    }

    #[must_use]
    pub fn step_code(&self) -> u8 {
        self.data[0] & 0x07
    }
}

impl DptValue for ControlDimming {
    const DPT_NUMBER: &'static str = "3.007";
    const VALUE_TYPE: &'static str = "control_dimming";
    const BYTE_LENGTH: usize = 1;

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
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
        Ok(Self { data: [bytes[0]] })
    }

    fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        Self::from_bytes(bytes)
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }

    fn value_range() -> (f64, f64) {
        (0.0, 15.0)
    }
}

/// DPT 3.008 - Control Blinds
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ControlBlinds {
    data: [u8; 1],
}

impl ControlBlinds {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `step_code > 7`.
    pub fn new(control: bool, step_code: u8) -> Result<Self> {
        if step_code > 7 {
            return Err(ProtocolError::DptError {
                dpt_type: "3.008".to_string(),
                details: format!("Step code {step_code} out of range [0-7]"),
            }
            .into());
        }
        let control_bit = if control { 0x08 } else { 0x00 };
        let step_code = step_code & 0x07;
        Ok(Self {
            data: [control_bit | step_code],
        })
    }

    #[must_use]
    pub fn control(&self) -> bool {
        (self.data[0] & 0x08) != 0
    }

    #[must_use]
    pub fn step_code(&self) -> u8 {
        self.data[0] & 0x07
    }
}

impl DptValue for ControlBlinds {
    const DPT_NUMBER: &'static str = "3.008";
    const VALUE_TYPE: &'static str = "control_blinds";
    const BYTE_LENGTH: usize = 1;

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
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
        Ok(Self { data: [bytes[0]] })
    }

    fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        Self::from_bytes(bytes)
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }

    fn value_range() -> (f64, f64) {
        (0.0, 15.0)
    }
}
