//! DPT 29.xxx - Signed 64-bit values

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 29.010 - Active Energy (8-byte signed)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActiveEnergy8Byte {
    value: i64,
    data: [u8; 8],
}

impl ActiveEnergy8Byte {
    #[must_use]
    pub fn new(value: i64) -> Self {
        Self {
            value,
            data: value.to_be_bytes(),
        }
    }

    #[must_use]
    pub fn value(&self) -> i64 {
        self.value
    }
}

impl DptValue for ActiveEnergy8Byte {
    const DPT_NUMBER: &'static str = "29.010";
    const VALUE_TYPE: &'static str = "active_energy_8byte";
    const UNIT: Option<&'static str> = Some("Wh");
    const HA_DEVICE_CLASS: Option<&'static str> = Some("energy");
    const BYTE_LENGTH: usize = 8;

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
        let value = i64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        Ok(Self::new(value))
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
        (i64::MIN as f64, i64::MAX as f64)
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for ActiveEnergy8Byte {
    type InnerType = i64;
    fn new(value: i64) -> Self {
        ActiveEnergy8Byte::new(value)
    }
    fn into_inner(self) -> i64 {
        self.value
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(
    DPTActiveEnergy8Byte,
    29,
    010,
    ActiveEnergy8Byte,
    "active_energy_8byte",
    Some("Wh"),
    Some("energy")
);
dpt_alias!(
    DPTApparantEnergy8Byte,
    29,
    011,
    ActiveEnergy8Byte,
    "apparant_energy_8byte",
    Some("VAh"),
    None
);
dpt_alias!(
    DPTReactiveEnergy8Byte,
    29,
    012,
    ActiveEnergy8Byte,
    "reactive_energy_8byte",
    Some("VARh"),
    None
);
