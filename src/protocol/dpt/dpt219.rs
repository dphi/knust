//! DPT 219.001 alarm information.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AlarmInfo {
    pub log_number: u8,
    pub alarm_priority: Option<u8>,
    pub application_area: u8,
    pub error_class: u8,
    pub ack_sup: bool,
    pub time_stamp_sup: bool,
    pub alarm_text_sup: bool,
    pub error_code_sup: bool,
    pub in_alarm: bool,
    pub alarm_un_ack: bool,
    pub locked: bool,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "219.001".into(),
        details: "Invalid alarm priority, application area or fields".into(),
    }
    .into()
}
impl AlarmInfo {
    pub(super) fn encode(self) -> Result<[u8; 6]> {
        let priority = match self.alarm_priority {
            None => 3,
            Some(v @ 0..=2) => v,
            _ => return Err(bad()),
        };
        if !matches!(self.application_area, 0 | 1 | 10..=14 | 20 | 30 | 40 | 50) {
            return Err(bad());
        }
        let attributes = u8::from(self.ack_sup)
            | u8::from(self.time_stamp_sup) << 1
            | u8::from(self.alarm_text_sup) << 2
            | u8::from(self.error_code_sup) << 3;
        let status =
            u8::from(self.in_alarm) | u8::from(self.alarm_un_ack) << 1 | u8::from(self.locked) << 2;
        Ok([
            self.log_number,
            priority,
            self.application_area,
            self.error_class,
            attributes,
            status,
        ])
    }
    pub(super) fn decode(raw: [u8; 6]) -> Result<Self> {
        let value = Self {
            log_number: raw[0],
            alarm_priority: if raw[1] == 3 { None } else { Some(raw[1]) },
            application_area: raw[2],
            error_class: raw[3],
            ack_sup: raw[4] & 1 != 0,
            time_stamp_sup: raw[4] & 2 != 0,
            alarm_text_sup: raw[4] & 4 != 0,
            error_code_sup: raw[4] & 8 != 0,
            in_alarm: raw[5] & 1 != 0,
            alarm_un_ack: raw[5] & 2 != 0,
            locked: raw[5] & 4 != 0,
        };
        value.encode()?;
        Ok(value)
    }
}
impl std::fmt::Display for AlarmInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = self
            .alarm_priority
            .map_or_else(|| "none".into(), |v| v.to_string());
        write!(
            f,
            "{},{},{},{},{},{},{},{},{},{},{}",
            self.log_number,
            p,
            self.application_area,
            self.error_class,
            self.ack_sup,
            self.time_stamp_sup,
            self.alarm_text_sup,
            self.error_code_sup,
            self.in_alarm,
            self.alarm_un_ack,
            self.locked
        )
    }
}
impl std::str::FromStr for AlarmInfo {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let mut p = s.split(',').map(str::trim);
        let log_number = p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        let priority = p.next().ok_or_else(bad)?;
        let alarm_priority = if priority == "none" {
            None
        } else {
            Some(priority.parse().map_err(|_| bad())?)
        };
        let value = Self {
            log_number,
            alarm_priority,
            application_area: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            error_class: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            ack_sup: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            time_stamp_sup: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            alarm_text_sup: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            error_code_sup: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            in_alarm: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            alarm_un_ack: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            locked: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
        };
        if p.next().is_some() {
            return Err(bad());
        }
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTAlarmInfo([u8; 6]);
impl DPTAlarmInfo {
    ///
    /// # Errors
    ///
    /// Returns an error if the value is outside the valid range or cannot be encoded.
    pub fn new(value: AlarmInfo) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> AlarmInfo {
        AlarmInfo::decode(self.0).expect("validated alarm")
    }
}
impl DptValue for DPTAlarmInfo {
    const DPT_NUMBER: &'static str = "219.001";
    const VALUE_TYPE: &'static str = "alarm_info";
    const BYTE_LENGTH: usize = 6;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = bytes.try_into().map_err(|_| bad())?;
        AlarmInfo::decode(raw)?;
        Ok(Self(raw))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        AlarmInfo::decode(self.0).map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTAlarmInfo> for AlarmInfo {
    fn resolve(self, _: DptType) -> Result<DPTAlarmInfo> {
        DPTAlarmInfo::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AlarmInfoView<'a>(pub(super) &'a [u8]);
impl<'a> AlarmInfoView<'a> {
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> AlarmInfo {
        AlarmInfo::decode(self.0.try_into().expect("validated length")).expect("validated alarm")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
