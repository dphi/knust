//! DPT 8.xxx - Signed 16-bit values

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 8.001 - Value 2 Byte Signed
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Value2ByteSigned {
    data: [u8; 2],
}

impl Value2ByteSigned {
    #[must_use]
    pub fn new(value: i16) -> Self {
        Self {
            data: value.to_be_bytes(),
        }
    }

    #[must_use]
    pub fn value(&self) -> i16 {
        i16::from_be_bytes(self.data)
    }
}

impl DptValue for Value2ByteSigned {
    const DPT_NUMBER: &'static str = "8.001";
    const VALUE_TYPE: &'static str = "value_2_byte_signed";
    const BYTE_LENGTH: usize = 2;

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
            data: [bytes[0], bytes[1]],
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
        (-32768.0, 32767.0)
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for Value2ByteSigned {
    type InnerType = i16;
    fn new(value: i16) -> Self {
        Self::new(value)
    }
    fn into_inner(self) -> i16 {
        self.value()
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(
    DPTValue2Count,
    8,
    001,
    Value2ByteSigned,
    "pulse_2byte_signed",
    None,
    None
);
dpt_alias!(
    DPTDeltaTimeMsec,
    8,
    002,
    Value2ByteSigned,
    "delta_time_ms",
    Some("ms"),
    None
);
dpt_alias!(
    DPTDeltaTime10Msec,
    8,
    003,
    Value2ByteSigned,
    "delta_time_10ms",
    Some("ms"),
    None
);
dpt_alias!(
    DPTDeltaTime100Msec,
    8,
    004,
    Value2ByteSigned,
    "delta_time_100ms",
    Some("ms"),
    None
);
dpt_alias!(
    DPTDeltaTimeSec,
    8,
    005,
    Value2ByteSigned,
    "delta_time_sec",
    Some("s"),
    None
);
dpt_alias!(
    DPTDeltaTimeMin,
    8,
    006,
    Value2ByteSigned,
    "delta_time_min",
    Some("min"),
    None
);
dpt_alias!(
    DPTDeltaTimeHrs,
    8,
    007,
    Value2ByteSigned,
    "delta_time_hrs",
    Some("h"),
    None
);
/// DPT 8.010 - Percent V16 with 0.01% resolution
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTPercentV16 {
    data: [u8; 2],
}

impl DPTPercentV16 {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `value` is not finite (NaN or infinite).
    pub fn new(value: f64) -> Result<Self> {
        if !value.is_finite() {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: "Value must be finite".to_string(),
            }
            .into());
        }

        let raw = (value / 0.01) as i32;
        if !(i32::from(i16::MIN)..=i32::from(i16::MAX)).contains(&raw) {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Percent V16 {value} out of range [-327.68, 327.67]"),
            }
            .into());
        }

        Ok(Self {
            data: (raw as i16).to_be_bytes(),
        })
    }

    #[must_use]
    pub fn value(&self) -> f64 {
        f64::from(i16::from_be_bytes(self.data)) * 0.01
    }
}

impl DptValue for DPTPercentV16 {
    const DPT_NUMBER: &'static str = "8.010";
    const VALUE_TYPE: &'static str = "percentV16";
    const UNIT: Option<&'static str> = Some("%");
    const BYTE_LENGTH: usize = 2;

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
            data: [bytes[0], bytes[1]],
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
        (-327.68, 327.67)
    }
}
dpt_alias!(
    DPTRotationAngle,
    8,
    011,
    Value2ByteSigned,
    "rotation_angle",
    Some("°"),
    None
);
dpt_alias!(
    DPTLengthM,
    8,
    012,
    Value2ByteSigned,
    "length_m",
    Some("m"),
    None
);
