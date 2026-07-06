//! DPT 5.xxx - Unsigned 8-bit values

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 5.001 - Scaling (0-100%)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Scaling {
    data: [u8; 1],
}

impl Scaling {
    #[must_use]
    pub fn new(value: u8) -> Self {
        Self { data: [value] }
    }

    #[must_use]
    pub fn value(&self) -> u8 {
        self.data[0]
    }
}

impl DptValue for Scaling {
    const DPT_NUMBER: &'static str = "5.001";
    const VALUE_TYPE: &'static str = "scaling";
    const UNIT: Option<&'static str> = Some("%");
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
        (0.0, 255.0)
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for Scaling {
    type InnerType = u8;
    fn new(value: u8) -> Self {
        Self::new(value)
    }
    fn into_inner(self) -> u8 {
        self.value()
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(DPTScaling, 5, 001, Scaling, "percent", Some("%"), None);
dpt_alias!(DPTAngle, 5, 003, Scaling, "angle", Some("°"), None);
dpt_alias!(DPTPercentU8, 5, 004, Scaling, "percentU8", Some("%"), None);
dpt_alias!(
    DPTDecimalFactor,
    5,
    005,
    Scaling,
    "decimal_factor",
    None,
    None
);
dpt_alias!(DPTTariff, 5, 006, Scaling, "tariff", None, None);
dpt_alias!(DPTValue1Ucount, 5, 010, Scaling, "pulse", None, None);
