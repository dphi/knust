//! DPT 1.xxx - Boolean values (true zero-copy)

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 1.001 - Switch (Boolean) - stores raw byte
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Switch([u8; 1]);

impl Switch {
    #[must_use]
    pub fn new(value: bool) -> Self {
        Self([u8::from(value)])
    }

    #[must_use]
    pub fn value(&self) -> bool {
        (self.0[0] & 0x01) != 0
    }
}

impl DptValue for Switch {
    const DPT_NUMBER: &'static str = "1.001";
    const VALUE_TYPE: &'static str = "switch";
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
        let value = Switch([bytes[0]]);
        value.validate()?;
        Ok(value)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn validate(&self) -> Result<()> {
        if self.0[0] > 1 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Invalid binary value: expected 0 or 1, got {}", self.0[0]),
            }
            .into());
        }
        Ok(())
    }

    fn value_range() -> (f64, f64) {
        (0.0, 1.0)
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for Switch {
    type InnerType = bool;
    fn new(value: bool) -> Self {
        Switch::new(value)
    }
    fn into_inner(self) -> bool {
        self.value()
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(DPTSwitch, 1, 001, Switch, "switch", None, None);
dpt_alias!(DPTBool, 1, 002, Switch, "bool", None, None);
dpt_alias!(DPTEnable, 1, 003, Switch, "enable", None, None);
dpt_alias!(DPTRamp, 1, 004, Switch, "ramp", None, None);
dpt_alias!(DPTAlarm, 1, 005, Switch, "alarm", None, None);
dpt_alias!(DPTBinaryValue, 1, 006, Switch, "binary_value", None, None);
dpt_alias!(DPTStep, 1, 007, Switch, "step", None, None);
dpt_alias!(DPTUpDown, 1, 008, Switch, "up_down", None, None);
dpt_alias!(DPTOpenClose, 1, 009, Switch, "open_close", None, None);
dpt_alias!(DPTStart, 1, 010, Switch, "start", None, None);
dpt_alias!(DPTState, 1, 011, Switch, "state", None, None);
dpt_alias!(DPTInvert, 1, 012, Switch, "invert", None, None);
dpt_alias!(
    DPTDimSendStyle,
    1,
    013,
    Switch,
    "dim_send_style",
    None,
    None
);
dpt_alias!(DPTInputSource, 1, 014, Switch, "input_source", None, None);
dpt_alias!(DPTReset, 1, 015, Switch, "reset", None, None);
dpt_alias!(DPTAck, 1, 016, Switch, "ack", None, None);
dpt_alias!(DPTTrigger, 1, 017, Switch, "trigger", None, None);
dpt_alias!(DPTOccupancy, 1, 018, Switch, "occupancy", None, None);
dpt_alias!(DPTWindowDoor, 1, 019, Switch, "window_door", None, None);
dpt_alias!(
    DPTLogicalFunction,
    1,
    021,
    Switch,
    "logical_function",
    None,
    None
);
dpt_alias!(DPTSceneAB, 1, 022, Switch, "scene_ab", None, None);
dpt_alias!(
    DPTShutterBlindsMode,
    1,
    023,
    Switch,
    "shutter_blinds_mode",
    None,
    None
);
dpt_alias!(DPTDayNight, 1, 024, Switch, "day_night", None, None);
dpt_alias!(DPTHeatCool, 1, 100, Switch, "heat_cool", None, None);
dpt_alias!(
    DPTConsumerProducer,
    1,
    1200,
    Switch,
    "consumer_producer",
    None,
    None
);
dpt_alias!(
    DPTEnergyDirection,
    1,
    1201,
    Switch,
    "energy_direction",
    None,
    None
);
