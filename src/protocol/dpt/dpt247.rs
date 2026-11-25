//! DPT 247.600 DALI converter test information.
use super::{DateTime, DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConverterTestInfo {
    pub test_start: DateTime,
    pub automated_test_interval: u16,
    pub test_execution_time: u16,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "247.600".into(),
        details: "Invalid converter test information".into(),
    }
    .into()
}
impl ConverterTestInfo {
    pub(super) fn encode(&self) -> Result<[u8; 12]> {
        self.test_start.validate()?;
        let date = self.test_start.encode_data();
        let interval = self.automated_test_interval.to_be_bytes();
        let time = self.test_execution_time.to_be_bytes();
        Ok([
            date[0],
            date[1],
            date[2],
            date[3],
            date[4],
            date[5],
            date[6],
            date[7],
            interval[0],
            interval[1],
            time[0],
            time[1],
        ])
    }
    pub(super) fn decode(raw: &[u8]) -> Result<Self> {
        Ok(Self {
            test_start: DateTime::decode(&raw[..8])?,
            automated_test_interval: u16::from_be_bytes([raw[8], raw[9]]),
            test_execution_time: u16::from_be_bytes([raw[10], raw[11]]),
        })
    }
}
impl std::fmt::Display for ConverterTestInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{}",
            hex::encode(self.test_start.encode_data()),
            self.automated_test_interval,
            self.test_execution_time
        )
    }
}
impl std::str::FromStr for ConverterTestInfo {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let mut p = s.split(',').map(str::trim);
        let date = hex::decode(p.next().ok_or_else(bad)?).map_err(|_| bad())?;
        let value = Self {
            test_start: DateTime::decode(&date)?,
            automated_test_interval: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            test_execution_time: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
        };
        if p.next().is_some() {
            return Err(bad());
        }
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DPTConverterTestInfo([u8; 12]);
impl DPTConverterTestInfo {
    pub fn new(value: ConverterTestInfo) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> ConverterTestInfo {
        ConverterTestInfo::decode(&self.0).expect("validated date/time")
    }
}
impl DptValue for DPTConverterTestInfo {
    const DPT_NUMBER: &'static str = "247.600";
    const VALUE_TYPE: &'static str = "converter_test_info";
    const BYTE_LENGTH: usize = 12;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw: [u8; 12] = bytes.try_into().map_err(|_| bad())?;
        ConverterTestInfo::decode(&raw)?;
        Ok(Self(raw))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        ConverterTestInfo::decode(&self.0).map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTConverterTestInfo> for ConverterTestInfo {
    fn resolve(self, _: DptType) -> Result<DPTConverterTestInfo> {
        DPTConverterTestInfo::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConverterTestInfoView<'a>(pub(super) &'a [u8]);
impl<'a> ConverterTestInfoView<'a> {
    #[must_use]
    pub fn value(&self) -> ConverterTestInfo {
        ConverterTestInfo::decode(self.0).expect("validated date/time")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
