//! DPT 235.xxx - Tariff Active Energy

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 235.001 - Tariff Active Energy
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TariffActiveEnergy {
    pub tariff: u8,
    pub energy: i32,
    pub tariff_valid: bool,
    pub energy_valid: bool,
    data: [u8; 6],
}

impl TariffActiveEnergy {
    #[must_use]
    pub fn new(energy: i32, tariff: u8) -> Self {
        Self::new_with_validity(energy, tariff, true, true)
    }

    #[must_use]
    pub fn new_with_validity(
        energy: i32,
        tariff: u8,
        energy_valid: bool,
        tariff_valid: bool,
    ) -> Self {
        let energy_bytes = if energy_valid {
            energy.to_be_bytes()
        } else {
            [0, 0, 0, 0]
        };
        let encoded_tariff = if tariff_valid { tariff } else { 0 };
        let data = [
            energy_bytes[0],
            energy_bytes[1],
            energy_bytes[2],
            energy_bytes[3],
            encoded_tariff,
            (u8::from(!energy_valid) << 1) | u8::from(!tariff_valid),
        ];

        Self {
            tariff,
            energy,
            tariff_valid,
            energy_valid,
            data,
        }
    }
}

impl DptValue for TariffActiveEnergy {
    const DPT_NUMBER: &'static str = "235.001";
    const VALUE_TYPE: &'static str = "tariff_active_energy";
    const UNIT: Option<&'static str> = Some("Wh");
    const HA_DEVICE_CLASS: Option<&'static str> = Some("energy");
    const BYTE_LENGTH: usize = 6;

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
        let energy = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let tariff = bytes[4];
        let energy_valid = ((bytes[5] >> 1) & 0x01) == 0;
        let tariff_valid = (bytes[5] & 0x01) == 0;
        Ok(Self::new_with_validity(
            energy,
            tariff,
            energy_valid,
            tariff_valid,
        ))
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
        (f64::from(i32::MIN), f64::from(i32::MAX))
    }
}
