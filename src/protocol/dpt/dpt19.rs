//! DPT 19.xxx - Date and Time

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 19.001 - Date and Time
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DateTime {
    pub year: u8,
    pub month: u8,       // 1-12
    pub day: u8,         // 1-31
    pub day_of_week: u8, // 0-7 (0=no day, 1=Monday, ..., 7=Sunday)
    pub hour: u8,
    pub minute: u8, // 0-59
    pub second: u8, // 0-59
    pub fault: bool,
    pub working_day: bool,
    pub no_wd: bool,
    pub no_year: bool,
    pub no_date: bool,
    pub no_dow: bool,
    pub no_time: bool,
    pub suti: bool,
    pub external_sync: bool,
    pub source_reliable: bool,
    data: [u8; 8],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DateTimeParts {
    pub year: u8,
    pub month: u8,
    pub day: u8,
    pub day_of_week: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DateTimeFlags {
    pub fault: bool,
    pub working_day: bool,
    pub no_wd: bool,
    pub no_year: bool,
    pub no_date: bool,
    pub no_dow: bool,
    pub no_time: bool,
    pub suti: bool,
    pub external_sync: bool,
    pub source_reliable: bool,
}

impl DateTime {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `month`, `day`, `day_of_week`,
    /// `hour`, `minute`, or `second` is out of its valid range (see
    /// [`Self::new_with_flags`], which this delegates to with default flags).
    pub fn new(
        year: u8,
        month: u8,
        day: u8,
        day_of_week: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Result<Self> {
        Self::new_with_flags(
            DateTimeParts {
                year,
                month,
                day,
                day_of_week,
                hour,
                minute,
                second,
            },
            DateTimeFlags::default(),
        )
    }

    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if, for fields not marked absent by
    /// the corresponding `no_*` flag: `month` is 0 or > 12, `day` is 0 or > 31,
    /// `day_of_week > 7`, `hour > 24`, `minute > 59`, or `second > 59`.
    pub fn new_with_flags(parts: DateTimeParts, flags: DateTimeFlags) -> Result<Self> {
        let mut value = Self {
            year: parts.year,
            month: parts.month,
            day: parts.day,
            day_of_week: parts.day_of_week,
            hour: parts.hour,
            minute: parts.minute,
            second: parts.second,
            fault: flags.fault,
            working_day: flags.working_day,
            no_wd: flags.no_wd,
            no_year: flags.no_year,
            no_date: flags.no_date,
            no_dow: flags.no_dow,
            no_time: flags.no_time,
            suti: flags.suti,
            external_sync: flags.external_sync,
            source_reliable: flags.source_reliable,
            data: [0; 8],
        };
        value.validate()?;
        value.data = value.encode_data();
        Ok(value)
    }

    fn encode_data(&self) -> [u8; 8] {
        let year = if self.no_year { 0 } else { self.year };
        let month = if self.no_date { 0 } else { self.month };
        let day = if self.no_date { 0 } else { self.day };
        let day_of_week = if self.no_dow { 0 } else { self.day_of_week };
        let hour = if self.no_time { 0 } else { self.hour };
        let minute = if self.no_time { 0 } else { self.minute };
        let second = if self.no_time { 0 } else { self.second };

        [
            year,
            month,
            day,
            (day_of_week << 5) | hour,
            minute,
            second,
            (u8::from(self.fault) << 7)
                | (u8::from(self.working_day) << 6)
                | (u8::from(self.no_wd) << 5)
                | (u8::from(self.no_year) << 4)
                | (u8::from(self.no_date) << 3)
                | (u8::from(self.no_dow) << 2)
                | (u8::from(self.no_time) << 1)
                | u8::from(self.suti),
            (u8::from(self.external_sync) << 7) | (u8::from(self.source_reliable) << 6),
        ]
    }
}

impl DptValue for DateTime {
    const DPT_NUMBER: &'static str = "19.001";
    const VALUE_TYPE: &'static str = "datetime";
    const BYTE_LENGTH: usize = 8;

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

        let year = bytes[0];
        let month = bytes[1] & 0x0F;
        let day = bytes[2] & 0x1F;
        let day_of_week = (bytes[3] >> 5) & 0x07;
        let hour = bytes[3] & 0x1F;
        let minute = bytes[4] & 0x3F;
        let second = bytes[5] & 0x3F;

        let flags = bytes[6];
        let fault = (flags & 0x80) != 0;
        let working_day = (flags & 0x40) != 0;
        let no_wd = (flags & 0x20) != 0;
        let no_year = (flags & 0x10) != 0;
        let no_date = (flags & 0x08) != 0;
        let no_dow = (flags & 0x04) != 0;
        let no_time = (flags & 0x02) != 0;
        let suti = (flags & 0x01) != 0;
        let external_sync = (bytes[7] & 0x80) != 0;
        let source_reliable = (bytes[7] & 0x40) != 0;

        let mut value = DateTime {
            year,
            month,
            day,
            day_of_week,
            hour,
            minute,
            second,
            fault,
            working_day,
            no_wd,
            no_year,
            no_date,
            no_dow,
            no_time,
            suti,
            external_sync,
            source_reliable,
            data: [0; 8],
        };
        value.validate()?;
        value.data = value.encode_data();
        Ok(value)
    }

    fn validate(&self) -> Result<()> {
        if !self.no_date && (self.month == 0 || self.month > 12) {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Month {} out of range [1-12]", self.month),
            }
            .into());
        }
        if !self.no_date && (self.day == 0 || self.day > 31) {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Day {} out of range [1-31]", self.day),
            }
            .into());
        }
        if !self.no_dow && self.day_of_week > 7 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Day of week {} out of range [0-7]", self.day_of_week),
            }
            .into());
        }
        if !self.no_time && self.hour > 24 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Hour {} out of range [0-24]", self.hour),
            }
            .into());
        }
        if !self.no_time && self.minute > 59 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Minute {} out of range [0-59]", self.minute),
            }
            .into());
        }
        if !self.no_time && self.second > 59 {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Second {} out of range [0-59]", self.second),
            }
            .into());
        }
        if !self.no_time && self.hour == 24 && (self.minute != 0 || self.second != 0) {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: "When hour is 24, minute and second must be 0".to_string(),
            }
            .into());
        }
        Ok(())
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::decode(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
