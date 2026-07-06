//! DPT 6.xxx - Signed 8-bit values

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 6.001 - Percent V8 (-128% to 127%)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PercentV8 {
    data: [u8; 1],
}

impl PercentV8 {
    #[must_use]
    pub fn new(value: i8) -> Self {
        Self {
            data: [value as u8],
        }
    }

    #[must_use]
    pub fn value(&self) -> i8 {
        self.data[0] as i8
    }
}

impl DptValue for PercentV8 {
    const DPT_NUMBER: &'static str = "6.001";
    const VALUE_TYPE: &'static str = "percentV8";
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
        (-128.0, 127.0)
    }
}

/// DPT 6.010 - Value 1 Count
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Value1Count {
    data: [u8; 1],
}

impl Value1Count {
    #[must_use]
    pub fn new(value: i8) -> Self {
        Self {
            data: [value as u8],
        }
    }

    #[must_use]
    pub fn value(&self) -> i8 {
        self.data[0] as i8
    }
}

impl DptValue for Value1Count {
    const DPT_NUMBER: &'static str = "6.010";
    const VALUE_TYPE: &'static str = "counter_pulses";
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
        (-128.0, 127.0)
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for PercentV8 {
    type InnerType = i8;
    fn new(value: i8) -> Self {
        Self::new(value)
    }
    fn into_inner(self) -> i8 {
        self.value()
    }
}

impl DptInnerType for Value1Count {
    type InnerType = i8;
    fn new(value: i8) -> Self {
        Self::new(value)
    }
    fn into_inner(self) -> i8 {
        self.value()
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(
    DPTPercentV8,
    6,
    001,
    PercentV8,
    "percentV8",
    Some("%"),
    None
);
dpt_alias!(
    DPTValue1Count,
    6,
    010,
    Value1Count,
    "counter_pulses",
    Some("counter pulses"),
    None
);
