//! DPT 236.001 prioritised mode control.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrioritisedModeControl {
    pub priority_level: u8,
    pub mode_level: u8,
    pub deactivate_priority: bool,
}
impl PrioritisedModeControl {
    pub(super) fn encode(self) -> Result<u8> {
        if self.priority_level > 7 || self.mode_level > 15 {
            return Err(ProtocolError::DptError {
                dpt_type: "236.001".into(),
                details: "Priority must be 0..=7 and mode 0..=15".into(),
            }
            .into());
        }
        Ok(
            (u8::from(self.deactivate_priority) << 7)
                | (self.priority_level << 4)
                | self.mode_level,
        )
    }
    pub(super) fn decode(raw: u8) -> Self {
        Self {
            priority_level: (raw >> 4) & 7,
            mode_level: raw & 15,
            deactivate_priority: raw & 0x80 != 0,
        }
    }
}
impl std::fmt::Display for PrioritisedModeControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{}",
            self.priority_level, self.mode_level, self.deactivate_priority
        )
    }
}
impl std::str::FromStr for PrioritisedModeControl {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let bad = || ProtocolError::DptError {
            dpt_type: "236.001".into(),
            details: "Expected priority,mode,deactivate".into(),
        };
        let mut p = s.split(',').map(str::trim);
        let value = Self {
            priority_level: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            mode_level: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            deactivate_priority: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
        };
        if p.next().is_some() {
            return Err(bad().into());
        }
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTPrioritisedModeControl([u8; 1]);
impl DPTPrioritisedModeControl {
    pub fn new(value: PrioritisedModeControl) -> Result<Self> {
        Ok(Self([value.encode()?]))
    }
    #[must_use]
    pub fn value(&self) -> PrioritisedModeControl {
        PrioritisedModeControl::decode(self.0[0])
    }
}
impl DptValue for DPTPrioritisedModeControl {
    const DPT_NUMBER: &'static str = "236.001";
    const VALUE_TYPE: &'static str = "prioritised_mode_control";
    const BYTE_LENGTH: usize = 1;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let [raw] = bytes else {
            return Err(ProtocolError::DptError {
                dpt_type: "236.001".into(),
                details: "Expected one byte".into(),
            }
            .into());
        };
        Ok(Self([*raw]))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        Ok(())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTPrioritisedModeControl> for PrioritisedModeControl {
    fn resolve(self, _: DptType) -> Result<DPTPrioritisedModeControl> {
        DPTPrioritisedModeControl::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PrioritisedModeControlView<'a>(pub(super) &'a [u8]);
impl<'a> PrioritisedModeControlView<'a> {
    #[must_use]
    pub fn value(&self) -> PrioritisedModeControl {
        PrioritisedModeControl::decode(self.0[0])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
