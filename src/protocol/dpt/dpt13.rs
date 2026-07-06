//! DPT 13.xxx - Signed 32-bit values

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 13.001 - Value 4 Byte Signed
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Value4ByteSigned {
    data: [u8; 4],
}

impl Value4ByteSigned {
    #[must_use]
    pub fn new(value: i32) -> Self {
        Self {
            data: value.to_be_bytes(),
        }
    }

    #[must_use]
    pub fn value(&self) -> i32 {
        i32::from_be_bytes(self.data)
    }
}

impl DptValue for Value4ByteSigned {
    const DPT_NUMBER: &'static str = "13.001";
    const VALUE_TYPE: &'static str = "value_4_byte_signed";
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
        (-2_147_483_648.0, 2_147_483_647.0)
    }
}

/// DPT 13.010 - Active Energy (Wh)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActiveEnergy {
    data: [u8; 4],
}

impl ActiveEnergy {
    #[must_use]
    pub fn new(value: i32) -> Self {
        Self {
            data: value.to_be_bytes(),
        }
    }

    #[must_use]
    pub fn value(&self) -> i32 {
        i32::from_be_bytes(self.data)
    }
}

impl DptValue for ActiveEnergy {
    const DPT_NUMBER: &'static str = "13.010";
    const VALUE_TYPE: &'static str = "active_energy";
    const UNIT: Option<&'static str> = Some("Wh");
    const HA_DEVICE_CLASS: Option<&'static str> = Some("energy");
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
        (-2_147_483_648.0, 2_147_483_647.0)
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for Value4ByteSigned {
    type InnerType = i32;
    fn new(value: i32) -> Self {
        Self::new(value)
    }
    fn into_inner(self) -> i32 {
        self.value()
    }
}

impl DptInnerType for ActiveEnergy {
    type InnerType = i32;
    fn new(value: i32) -> Self {
        Self::new(value)
    }
    fn into_inner(self) -> i32 {
        self.value()
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(
    DPTValue4Count,
    13,
    001,
    Value4ByteSigned,
    "pulse_4byte",
    Some("counter pulses"),
    None
);
dpt_alias!(
    DPTFlowRateM3H,
    13,
    002,
    Value4ByteSigned,
    "flow_rate_m3h",
    Some("m³/h"),
    None
);
dpt_alias!(
    DPTActiveEnergy,
    13,
    010,
    ActiveEnergy,
    "active_energy",
    Some("Wh"),
    Some("energy")
);
dpt_alias!(
    DPTApparantEnergy,
    13,
    011,
    Value4ByteSigned,
    "apparant_energy",
    Some("VAh"),
    None
);
dpt_alias!(
    DPTReactiveEnergy,
    13,
    012,
    Value4ByteSigned,
    "reactive_energy",
    Some("VARh"),
    None
);
dpt_alias!(
    DPTActiveEnergykWh,
    13,
    013,
    Value4ByteSigned,
    "active_energy_kwh",
    Some("kWh"),
    Some("energy")
);
dpt_alias!(
    DPTApparantEnergykVAh,
    13,
    014,
    Value4ByteSigned,
    "apparant_energy_kvah",
    Some("kVAh"),
    None
);
dpt_alias!(
    DPTReactiveEnergykVARh,
    13,
    015,
    Value4ByteSigned,
    "reactive_energy_kvarh",
    Some("kVARh"),
    None
);
dpt_alias!(
    DPTActiveEnergyMWh,
    13,
    016,
    Value4ByteSigned,
    "active_energy_mwh",
    Some("MWh"),
    Some("energy")
);
dpt_alias!(
    DPTLongDeltaTimeSec,
    13,
    100,
    Value4ByteSigned,
    "long_delta_timesec",
    Some("s"),
    None
);
dpt_alias!(
    DPTDeltaVolumeLiquidLitre,
    13,
    1200,
    Value4ByteSigned,
    "delta_volume_liquid_litre",
    Some("L"),
    None
);
dpt_alias!(
    DPTDeltaVolumeM3,
    13,
    1201,
    Value4ByteSigned,
    "delta_volume_m3",
    Some("m³"),
    None
);
