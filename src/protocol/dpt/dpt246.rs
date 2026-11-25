//! DPT 246.600 DALI emergency battery information.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BatteryInfo {
    pub battery_failure: bool,
    pub battery_duration_failure: bool,
    pub battery_fully_charged: bool,
    pub charge_level: Option<u8>,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "246.600".into(),
        details: "Invalid battery information".into(),
    }
    .into()
}
impl BatteryInfo {
    pub(super) fn encode(self) -> Result<[u8; 2]> {
        if self.charge_level == Some(255) {
            return Err(bad());
        }
        Ok([
            u8::from(self.battery_failure)
                | u8::from(self.battery_duration_failure) << 1
                | u8::from(self.battery_fully_charged) << 2,
            self.charge_level.unwrap_or(255),
        ])
    }
    pub(super) fn decode(raw: [u8; 2]) -> Self {
        Self {
            battery_failure: raw[0] & 1 != 0,
            battery_duration_failure: raw[0] & 2 != 0,
            battery_fully_charged: raw[0] & 4 != 0,
            charge_level: (raw[1] != 255).then_some(raw[1]),
        }
    }
}
impl std::fmt::Display for BatteryInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = self
            .charge_level
            .map_or_else(|| "none".into(), |v| v.to_string());
        write!(
            f,
            "{},{},{},{}",
            self.battery_failure, self.battery_duration_failure, self.battery_fully_charged, level
        )
    }
}
impl std::str::FromStr for BatteryInfo {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let mut p = s.split(',').map(str::trim);
        let battery_failure = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let battery_duration_failure = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let battery_fully_charged = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let level = p.next().ok_or_else(bad)?;
        let charge_level = if level == "none" {
            None
        } else {
            Some(level.parse().map_err(|_| bad())?)
        };
        if p.next().is_some() {
            return Err(bad());
        }
        let value = Self {
            battery_failure,
            battery_duration_failure,
            battery_fully_charged,
            charge_level,
        };
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTBatteryInfo([u8; 2]);
impl DPTBatteryInfo {
    pub fn new(value: BatteryInfo) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> BatteryInfo {
        BatteryInfo::decode(self.0)
    }
}
impl DptValue for DPTBatteryInfo {
    const DPT_NUMBER: &'static str = "246.600";
    const VALUE_TYPE: &'static str = "battery_info";
    const BYTE_LENGTH: usize = 2;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self(bytes.try_into().map_err(|_| bad())?))
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
impl WriteValue<DPTBatteryInfo> for BatteryInfo {
    fn resolve(self, _: DptType) -> Result<DPTBatteryInfo> {
        DPTBatteryInfo::new(self)
    }
}

/// Borrowed DPT 246.600 battery information.
#[derive(Debug, Clone, Copy)]
pub struct BatteryInfoView<'a>(&'a [u8; 2]);

impl<'a> BatteryInfoView<'a> {
    pub(super) fn from_bytes(bytes: &'a [u8]) -> Result<Self> {
        Ok(Self(bytes.try_into().map_err(|_| bad())?))
    }

    #[must_use]
    pub fn value(&self) -> BatteryInfo {
        BatteryInfo::decode(*self.0)
    }

    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
