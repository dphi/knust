//! DPT 2.xxx - 2-bit control values (control bit + value bit)

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 2.001 - Switch Control (base type; other DPT 2.xxx numbers alias this)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BinaryControl([u8; 1]);

impl BinaryControl {
    #[must_use]
    pub fn new(control: bool, value: bool) -> Self {
        Self([(u8::from(control) << 1) | u8::from(value)])
    }

    #[must_use]
    pub fn control(&self) -> bool {
        (self.0[0] & 0x02) != 0
    }

    #[must_use]
    pub fn value(&self) -> bool {
        (self.0[0] & 0x01) != 0
    }
}

impl DptValue for BinaryControl {
    const DPT_NUMBER: &'static str = "2.001";
    const VALUE_TYPE: &'static str = "switch_control";
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
        let value = Self([bytes[0]]);
        value.validate()?;
        Ok(value)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn validate(&self) -> Result<()> {
        if self.0[0] > 0x03 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!(
                    "Invalid 2-bit control value: expected 0-3, got {}",
                    self.0[0]
                ),
            }
            .into());
        }
        Ok(())
    }

    fn value_range() -> (f64, f64) {
        (0.0, 3.0)
    }
}

use super::DptInnerType;

impl DptInnerType for BinaryControl {
    type InnerType = (bool, bool);
    fn new(value: (bool, bool)) -> Self {
        BinaryControl::new(value.0, value.1)
    }
    fn into_inner(self) -> (bool, bool) {
        (self.control(), self.value())
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(
    DPTSwitchControl,
    2,
    001,
    BinaryControl,
    "switch_control",
    None,
    None
);
dpt_alias!(
    DPTBoolControl,
    2,
    002,
    BinaryControl,
    "bool_control",
    None,
    None
);
dpt_alias!(
    DPTEnableControl,
    2,
    003,
    BinaryControl,
    "enable_control",
    None,
    None
);
dpt_alias!(
    DPTRampControl,
    2,
    004,
    BinaryControl,
    "ramp_control",
    None,
    None
);
dpt_alias!(
    DPTAlarmControl,
    2,
    005,
    BinaryControl,
    "alarm_control",
    None,
    None
);
dpt_alias!(
    DPTBinaryValueControl,
    2,
    006,
    BinaryControl,
    "binary_value_control",
    None,
    None
);
dpt_alias!(
    DPTStepControl,
    2,
    007,
    BinaryControl,
    "step_control",
    None,
    None
);
dpt_alias!(
    DPTDirection1Control,
    2,
    008,
    BinaryControl,
    "direction1_control",
    None,
    None
);
dpt_alias!(
    DPTDirection2Control,
    2,
    009,
    BinaryControl,
    "direction2_control",
    None,
    None
);
dpt_alias!(
    DPTStartControl,
    2,
    010,
    BinaryControl,
    "start_control",
    None,
    None
);
dpt_alias!(
    DPTStateControl,
    2,
    011,
    BinaryControl,
    "state_control",
    None,
    None
);
dpt_alias!(
    DPTInvertControl,
    2,
    012,
    BinaryControl,
    "invert_control",
    None,
    None
);
