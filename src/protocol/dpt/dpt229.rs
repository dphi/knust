//! DPT 229.001 metering value.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MeteringValueUnit {
    EnergyWh = 0,
    EnergyJ = 8,
    VolumeM3 = 16,
    MassKg = 24,
    PowerW = 40,
    PowerJPerHour = 48,
    VolumeFlowM3PerHour = 56,
    VolumeFlowM3PerMin = 64,
    VolumeFlowM3PerSec = 72,
    MassFlowKgPerHour = 80,
    HcaUnits = 110,
    DimensionlessCounter = 186,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "229.001".into(),
        details: "Invalid metering value or unit exponent".into(),
    }
    .into()
}
fn unit(code: u8) -> Result<MeteringValueUnit> {
    use MeteringValueUnit::*;
    Ok(match code {
        0 => EnergyWh,
        8 => EnergyJ,
        16 => VolumeM3,
        24 => MassKg,
        40 => PowerW,
        48 => PowerJPerHour,
        56 => VolumeFlowM3PerHour,
        64 => VolumeFlowM3PerMin,
        72 => VolumeFlowM3PerSec,
        80 => MassFlowKgPerHour,
        110 => HcaUnits,
        186 => DimensionlessCounter,
        _ => return Err(bad()),
    })
}
fn classify(code: u8) -> Result<(MeteringValueUnit, i8)> {
    use MeteringValueUnit::*;
    let (first, exponent, kind) = match code {
        0x00..=0x07 => (0x00, -3, EnergyWh),
        0x08..=0x0f => (0x08, 0, EnergyJ),
        0x10..=0x17 => (0x10, -6, VolumeM3),
        0x18..=0x1f => (0x18, -3, MassKg),
        0x28..=0x2f => (0x28, -3, PowerW),
        0x30..=0x37 => (0x30, 0, PowerJPerHour),
        0x38..=0x3f => (0x38, -6, VolumeFlowM3PerHour),
        0x40..=0x47 => (0x40, -7, VolumeFlowM3PerMin),
        0x48..=0x4f => (0x48, -9, VolumeFlowM3PerSec),
        0x50..=0x57 => (0x50, -3, MassFlowKgPerHour),
        0x80..=0x81 => (0x80, 5, EnergyWh),
        0x88..=0x89 => (0x88, 8, EnergyJ),
        0xa8..=0xa9 => (0xa8, 5, PowerW),
        0xb0..=0xb1 => (0xb0, 8, PowerJPerHour),
        0x6e => (0x6e, 0, HcaUnits),
        0xba => (0xba, 0, DimensionlessCounter),
        _ => return Err(bad()),
    };
    Ok((kind, exponent + (code - first) as i8))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeteringValue {
    pub counter_value: i32,
    pub unit: MeteringValueUnit,
    pub exponent: i8,
    pub out_of_service: bool,
    pub fault: bool,
    pub overridden: bool,
    pub in_alarm: bool,
    pub alarm_un_ack: bool,
}
impl MeteringValue {
    pub(super) fn encode(self) -> Result<[u8; 6]> {
        let code = (0..=u8::MAX)
            .find(|v| classify(*v).is_ok_and(|pair| pair == (self.unit, self.exponent)))
            .ok_or_else(bad)?;
        let b = self.counter_value.to_be_bytes();
        let flags = u8::from(self.out_of_service)
            | u8::from(self.fault) << 1
            | u8::from(self.overridden) << 2
            | u8::from(self.in_alarm) << 3
            | u8::from(self.alarm_un_ack) << 4;
        Ok([b[0], b[1], b[2], b[3], code, flags])
    }
    pub(super) fn decode(raw: [u8; 6]) -> Result<Self> {
        let (unit, exponent) = classify(raw[4])?;
        let flags = raw[5];
        Ok(Self {
            counter_value: i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]),
            unit,
            exponent,
            out_of_service: flags & 1 != 0,
            fault: flags & 2 != 0,
            overridden: flags & 4 != 0,
            in_alarm: flags & 8 != 0,
            alarm_un_ack: flags & 16 != 0,
        })
    }
}
impl std::fmt::Display for MeteringValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{},{},{},{},{},{}",
            self.counter_value,
            self.unit as u8,
            self.exponent,
            self.out_of_service,
            self.fault,
            self.overridden,
            self.in_alarm,
            self.alarm_un_ack
        )
    }
}
impl std::str::FromStr for MeteringValue {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let mut p = s.split(',').map(str::trim);
        let value = Self {
            counter_value: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            unit: unit(p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?)?,
            exponent: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            out_of_service: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            fault: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            overridden: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            in_alarm: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            alarm_un_ack: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
        };
        if p.next().is_some() {
            return Err(bad());
        }
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTMeteringValue([u8; 6]);
impl DPTMeteringValue {
    pub fn new(value: MeteringValue) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> MeteringValue {
        MeteringValue::decode(self.0).expect("validated unit")
    }
}
impl DptValue for DPTMeteringValue {
    const DPT_NUMBER: &'static str = "229.001";
    const VALUE_TYPE: &'static str = "metering_value";
    const BYTE_LENGTH: usize = 6;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = bytes.try_into().map_err(|_| bad())?;
        MeteringValue::decode(raw)?;
        Ok(Self(raw))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        MeteringValue::decode(self.0).map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTMeteringValue> for MeteringValue {
    fn resolve(self, _: DptType) -> Result<DPTMeteringValue> {
        DPTMeteringValue::new(self)
    }
}
