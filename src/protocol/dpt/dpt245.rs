//! DPT 245.600 DALI emergency converter test result.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConverterTestResult {
    pub function_test_result: u8,
    pub duration_test_result: u8,
    pub partial_duration_test_result: u8,
    pub function_test_start_method: u8,
    pub duration_test_start_method: u8,
    pub partial_duration_test_start_method: u8,
    pub duration_test_discharge_time: u16,
    pub partial_duration_test_charge_level: Option<u8>,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "245.600".into(),
        details: "Invalid converter test result".into(),
    }
    .into()
}
impl ConverterTestResult {
    pub(super) fn encode(self) -> Result<[u8; 6]> {
        if self.function_test_result > 5
            || self.duration_test_result > 5
            || self.partial_duration_test_result > 5
            || self.function_test_start_method > 2
            || self.duration_test_start_method > 2
            || self.partial_duration_test_start_method > 2
            || self.duration_test_discharge_time > 510
            || self.partial_duration_test_charge_level == Some(255)
        {
            return Err(bad());
        }
        let time = self.duration_test_discharge_time.to_be_bytes();
        Ok([
            self.function_test_result << 4 | self.duration_test_result,
            self.partial_duration_test_result << 4,
            self.function_test_start_method << 6
                | self.duration_test_start_method << 4
                | self.partial_duration_test_start_method << 2,
            time[0],
            time[1],
            self.partial_duration_test_charge_level.unwrap_or(255),
        ])
    }
    pub(super) fn decode(raw: [u8; 6]) -> Result<Self> {
        let value = Self {
            function_test_result: raw[0] >> 4,
            duration_test_result: raw[0] & 15,
            partial_duration_test_result: raw[1] >> 4,
            function_test_start_method: raw[2] >> 6,
            duration_test_start_method: (raw[2] >> 4) & 3,
            partial_duration_test_start_method: (raw[2] >> 2) & 3,
            duration_test_discharge_time: u16::from_be_bytes([raw[3], raw[4]]),
            partial_duration_test_charge_level: (raw[5] != 255).then_some(raw[5]),
        };
        value.encode()?;
        Ok(value)
    }
}
impl std::fmt::Display for ConverterTestResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = self
            .partial_duration_test_charge_level
            .map_or_else(|| "none".into(), |v| v.to_string());
        write!(
            f,
            "{},{},{},{},{},{},{},{}",
            self.function_test_result,
            self.duration_test_result,
            self.partial_duration_test_result,
            self.function_test_start_method,
            self.duration_test_start_method,
            self.partial_duration_test_start_method,
            self.duration_test_discharge_time,
            level
        )
    }
}
impl std::str::FromStr for ConverterTestResult {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let mut p = s.split(',').map(str::trim);
        let function_test_result = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let duration_test_result = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let partial_duration_test_result = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let function_test_start_method = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let duration_test_start_method = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let partial_duration_test_start_method =
            p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let duration_test_discharge_time = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let level = p.next().ok_or_else(bad)?;
        let partial_duration_test_charge_level = if level == "none" {
            None
        } else {
            Some(level.parse().map_err(|_| bad())?)
        };
        if p.next().is_some() {
            return Err(bad());
        }
        let value = Self {
            function_test_result,
            duration_test_result,
            partial_duration_test_result,
            function_test_start_method,
            duration_test_start_method,
            partial_duration_test_start_method,
            duration_test_discharge_time,
            partial_duration_test_charge_level,
        };
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTConverterTestResult([u8; 6]);
impl DPTConverterTestResult {
    ///
    /// # Errors
    ///
    /// Returns an error if the value is outside the valid range or cannot be encoded.
    pub fn new(value: ConverterTestResult) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> ConverterTestResult {
        ConverterTestResult::decode(self.0).expect("validated result")
    }
}
impl DptValue for DPTConverterTestResult {
    const DPT_NUMBER: &'static str = "245.600";
    const VALUE_TYPE: &'static str = "converter_test_result";
    const BYTE_LENGTH: usize = 6;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = bytes.try_into().map_err(|_| bad())?;
        ConverterTestResult::decode(raw)?;
        Ok(Self(raw))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        ConverterTestResult::decode(self.0).map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTConverterTestResult> for ConverterTestResult {
    fn resolve(self, _: DptType) -> Result<DPTConverterTestResult> {
        DPTConverterTestResult::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConverterTestResultView<'a>(pub(super) &'a [u8]);
impl<'a> ConverterTestResultView<'a> {
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> ConverterTestResult {
        ConverterTestResult::decode(self.0.try_into().expect("validated length"))
            .expect("validated result")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
