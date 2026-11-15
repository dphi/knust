//! DPT 244.600 DALI emergency converter status.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConverterStatus {
    pub mode: u8,
    pub hardwired_inhibit_active: bool,
    pub hardwired_switch_on: bool,
    pub function_test_pending: u8,
    pub duration_test_pending: u8,
    pub partial_duration_test_pending: u8,
    pub failure: u8,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "244.600".into(),
        details: "Invalid converter status".into(),
    }
    .into()
}
impl ConverterStatus {
    pub(super) fn encode(self) -> Result<[u8; 2]> {
        if self.mode > 9
            || self.function_test_pending > 2
            || self.duration_test_pending > 2
            || self.partial_duration_test_pending > 2
            || self.failure > 2
        {
            return Err(bad());
        }
        Ok([
            self.mode << 4
                | u8::from(self.hardwired_switch_on) << 1
                | u8::from(self.hardwired_inhibit_active),
            self.function_test_pending << 6
                | self.duration_test_pending << 4
                | self.partial_duration_test_pending << 2
                | self.failure,
        ])
    }
    pub(super) fn decode(raw: [u8; 2]) -> Result<Self> {
        let value = Self {
            mode: raw[0] >> 4,
            hardwired_inhibit_active: raw[0] & 1 != 0,
            hardwired_switch_on: raw[0] & 2 != 0,
            function_test_pending: raw[1] >> 6,
            duration_test_pending: (raw[1] >> 4) & 3,
            partial_duration_test_pending: (raw[1] >> 2) & 3,
            failure: raw[1] & 3,
        };
        value.encode()?;
        Ok(value)
    }
}
impl std::fmt::Display for ConverterStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{},{},{},{},{}",
            self.mode,
            self.hardwired_inhibit_active,
            self.hardwired_switch_on,
            self.function_test_pending,
            self.duration_test_pending,
            self.partial_duration_test_pending,
            self.failure
        )
    }
}
impl std::str::FromStr for ConverterStatus {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let mut p = s.split(',').map(str::trim);
        let v = Self {
            mode: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            hardwired_inhibit_active: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            hardwired_switch_on: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            function_test_pending: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            duration_test_pending: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            partial_duration_test_pending: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            failure: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
        };
        if p.next().is_some() {
            return Err(bad());
        }
        v.encode()?;
        Ok(v)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTConverterStatus([u8; 2]);
impl DPTConverterStatus {
    pub fn new(value: ConverterStatus) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> ConverterStatus {
        ConverterStatus::decode(self.0).expect("validated status")
    }
}
impl DptValue for DPTConverterStatus {
    const DPT_NUMBER: &'static str = "244.600";
    const VALUE_TYPE: &'static str = "converter_status";
    const BYTE_LENGTH: usize = 2;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = bytes.try_into().map_err(|_| bad())?;
        ConverterStatus::decode(raw)?;
        Ok(Self(raw))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        ConverterStatus::decode(self.0).map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTConverterStatus> for ConverterStatus {
    fn resolve(self, _: DptType) -> Result<DPTConverterStatus> {
        DPTConverterStatus::new(self)
    }
}
