//! DPT 7.xxx - Unsigned 16-bit values

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 7.001 - Value 2 Byte Unsigned
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Value2ByteUnsigned {
    data: [u8; 2],
}

impl Value2ByteUnsigned {
    #[must_use]
    pub fn new(value: u16) -> Self {
        Self {
            data: value.to_be_bytes(),
        }
    }

    #[must_use]
    pub fn value(&self) -> u16 {
        u16::from_be_bytes(self.data)
    }
}

impl DptValue for Value2ByteUnsigned {
    const DPT_NUMBER: &'static str = "7.001";
    const VALUE_TYPE: &'static str = "value_2_byte_unsigned";
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
        (0.0, 65535.0)
    }
}

/// DPT 7.013 - Brightness (lux)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Brightness {
    data: [u8; 2],
}

impl Brightness {
    #[must_use]
    pub fn new(value: u16) -> Self {
        Self {
            data: value.to_be_bytes(),
        }
    }

    #[must_use]
    pub fn value(&self) -> u16 {
        u16::from_be_bytes(self.data)
    }
}

impl DptValue for Brightness {
    const DPT_NUMBER: &'static str = "7.013";
    const VALUE_TYPE: &'static str = "brightness";
    const UNIT: Option<&'static str> = Some("lux");
    const HA_DEVICE_CLASS: Option<&'static str> = Some("illuminance");
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
        (0.0, 65535.0)
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for Value2ByteUnsigned {
    type InnerType = u16;
    fn new(value: u16) -> Self {
        Self::new(value)
    }
    fn into_inner(self) -> u16 {
        self.value()
    }
}

impl DptInnerType for Brightness {
    type InnerType = u16;
    fn new(value: u16) -> Self {
        Self::new(value)
    }
    fn into_inner(self) -> u16 {
        self.value()
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(
    DPT2Ucount,
    7,
    001,
    Value2ByteUnsigned,
    "pulse_2byte",
    None,
    None
);
dpt_alias!(
    DPTTimePeriodMsec,
    7,
    002,
    Value2ByteUnsigned,
    "time_period_msec",
    Some("ms"),
    None
);
dpt_alias!(
    DPTTimePeriod10Msec,
    7,
    003,
    Value2ByteUnsigned,
    "time_period_10msec",
    Some("ms"),
    None
);
dpt_alias!(
    DPTTimePeriod100Msec,
    7,
    004,
    Value2ByteUnsigned,
    "time_period_100msec",
    Some("ms"),
    None
);
dpt_alias!(
    DPTTimePeriodSec,
    7,
    005,
    Value2ByteUnsigned,
    "time_period_sec",
    Some("s"),
    None
);
dpt_alias!(
    DPTTimePeriodMin,
    7,
    006,
    Value2ByteUnsigned,
    "time_period_min",
    Some("min"),
    None
);
dpt_alias!(
    DPTTimePeriodHrs,
    7,
    007,
    Value2ByteUnsigned,
    "time_period_hrs",
    Some("h"),
    None
);
dpt_alias!(
    DPTPropDataType,
    7,
    010,
    Value2ByteUnsigned,
    "prop_data_type",
    None,
    None
);
dpt_alias!(
    DPTLengthMm,
    7,
    011,
    Value2ByteUnsigned,
    "length_mm",
    Some("mm"),
    None
);
dpt_alias!(
    DPTUElCurrentmA,
    7,
    012,
    Value2ByteUnsigned,
    "current",
    Some("mA"),
    None
);
dpt_alias!(
    DPTBrightness,
    7,
    013,
    Brightness,
    "brightness",
    Some("lux"),
    Some("illuminance")
);
dpt_alias!(
    DPTColorTemperature,
    7,
    600,
    Value2ByteUnsigned,
    "color_temperature",
    Some("K"),
    None
);
