//! DPT 12.xxx - Unsigned 32-bit values

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 12.001 - Value 4 Byte Unsigned
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Value4ByteUnsigned {
    data: [u8; 4],
}

impl Value4ByteUnsigned {
    #[must_use]
    pub fn new(value: u32) -> Self {
        Self {
            data: value.to_be_bytes(),
        }
    }

    #[must_use]
    pub fn value(&self) -> u32 {
        u32::from_be_bytes(self.data)
    }
}

impl DptValue for Value4ByteUnsigned {
    const DPT_NUMBER: &'static str = "12.001";
    const VALUE_TYPE: &'static str = "value_4_byte_unsigned";
    const BYTE_LENGTH: usize = 4;

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
        Ok(Self {
            data: [bytes[0], bytes[1], bytes[2], bytes[3]],
        })
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
        (0.0, 4_294_967_295.0)
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for Value4ByteUnsigned {
    type InnerType = u32;
    fn new(value: u32) -> Self {
        Self::new(value)
    }
    fn into_inner(self) -> u32 {
        self.value()
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(
    DPTValue4Ucount,
    12,
    001,
    Value4ByteUnsigned,
    "pulse_4_ucount",
    Some("counter pulses"),
    None
);
dpt_alias!(
    DPTLongTimePeriodSec,
    12,
    100,
    Value4ByteUnsigned,
    "long_time_period_sec",
    Some("s"),
    None
);
dpt_alias!(
    DPTLongTimePeriodMin,
    12,
    101,
    Value4ByteUnsigned,
    "long_time_period_min",
    Some("min"),
    None
);
dpt_alias!(
    DPTLongTimePeriodHrs,
    12,
    102,
    Value4ByteUnsigned,
    "long_time_period_hrs",
    Some("h"),
    None
);
dpt_alias!(
    DPTVolumeLiquidLitre,
    12,
    1200,
    Value4ByteUnsigned,
    "volume_liquid_litre",
    Some("L"),
    None
);
dpt_alias!(
    DPTVolumeM3,
    12,
    1201,
    Value4ByteUnsigned,
    "volume_m3",
    Some("m³"),
    None
);
