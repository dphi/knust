//! DPT 272.600 DALI emergency converter information.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConverterInfo {
    pub info_valid: u8,
    pub lamp_emergency_time: u16,
    pub lamp_total_operation_time: u16,
    pub emergency_level: Option<u8>,
    pub short_address: u8,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "272.600".into(),
        details: "Invalid converter information".into(),
    }
    .into()
}
impl ConverterInfo {
    pub(super) fn encode(self) -> Result<[u8; 7]> {
        if self.info_valid > 2 || self.short_address > 63 || self.emergency_level == Some(255) {
            return Err(bad());
        }
        let emergency = self.lamp_emergency_time.to_be_bytes();
        let total = self.lamp_total_operation_time.to_be_bytes();
        Ok([
            self.info_valid,
            emergency[0],
            emergency[1],
            total[0],
            total[1],
            self.emergency_level.unwrap_or(255),
            self.short_address,
        ])
    }
    pub(super) fn decode(raw: [u8; 7]) -> Result<Self> {
        let value = Self {
            info_valid: raw[0],
            lamp_emergency_time: u16::from_be_bytes([raw[1], raw[2]]),
            lamp_total_operation_time: u16::from_be_bytes([raw[3], raw[4]]),
            emergency_level: (raw[5] != 255).then_some(raw[5]),
            short_address: raw[6],
        };
        value.encode()?;
        Ok(value)
    }
}
impl std::fmt::Display for ConverterInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = self
            .emergency_level
            .map_or_else(|| "none".into(), |v| v.to_string());
        write!(
            f,
            "{},{},{},{},{}",
            self.info_valid,
            self.lamp_emergency_time,
            self.lamp_total_operation_time,
            level,
            self.short_address
        )
    }
}
impl std::str::FromStr for ConverterInfo {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let mut p = s.split(',').map(str::trim);
        let info_valid = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let lamp_emergency_time = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let lamp_total_operation_time = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let level = p.next().ok_or_else(bad)?;
        let emergency_level = if level == "none" {
            None
        } else {
            Some(level.parse().map_err(|_| bad())?)
        };
        let short_address = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        if p.next().is_some() {
            return Err(bad());
        }
        let value = Self {
            info_valid,
            lamp_emergency_time,
            lamp_total_operation_time,
            emergency_level,
            short_address,
        };
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTConverterInfo([u8; 7]);
impl DPTConverterInfo {
    pub fn new(value: ConverterInfo) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> ConverterInfo {
        ConverterInfo::decode(self.0).expect("validated information")
    }
}
impl DptValue for DPTConverterInfo {
    const DPT_NUMBER: &'static str = "272.600";
    const VALUE_TYPE: &'static str = "converter_info";
    const BYTE_LENGTH: usize = 7;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = bytes.try_into().map_err(|_| bad())?;
        ConverterInfo::decode(raw)?;
        Ok(Self(raw))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        ConverterInfo::decode(self.0).map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTConverterInfo> for ConverterInfo {
    fn resolve(self, _: DptType) -> Result<DPTConverterInfo> {
        DPTConverterInfo::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConverterInfoView<'a>(pub(super) &'a [u8]);
impl<'a> ConverterInfoView<'a> {
    #[must_use]
    pub fn value(&self) -> ConverterInfo {
        ConverterInfo::decode(self.0.try_into().expect("validated length"))
            .expect("validated information")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
