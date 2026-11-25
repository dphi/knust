//! DPT 248.600 fixed DALI emergency converter information.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConverterInfoFix {
    pub info_valid: u8,
    pub emergency_min_level: Option<u8>,
    pub emergency_max_level: Option<u8>,
    pub rated_duration: u8,
    pub integral_emergency_control_gear: bool,
    pub maintained_control_gear: bool,
    pub switched_maintained_control_gear: bool,
    pub auto_test_capability: bool,
    pub adjustable_emergency_level: bool,
    pub hardwired_inhibit_supported: bool,
    pub physical_selection_supported: bool,
    pub relight_in_rest_mode_supported: bool,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "248.600".into(),
        details: "Invalid converter information".into(),
    }
    .into()
}
impl ConverterInfoFix {
    pub(super) fn encode(self) -> Result<[u8; 5]> {
        if self.info_valid > 2
            || self.emergency_min_level == Some(255)
            || self.emergency_max_level == Some(255)
        {
            return Err(bad());
        }
        let features = [
            self.integral_emergency_control_gear,
            self.maintained_control_gear,
            self.switched_maintained_control_gear,
            self.auto_test_capability,
            self.adjustable_emergency_level,
            self.hardwired_inhibit_supported,
            self.physical_selection_supported,
            self.relight_in_rest_mode_supported,
        ];
        let mut bits = 0u8;
        for (bit, flag) in features.into_iter().enumerate() {
            if flag {
                bits |= 1 << bit;
            }
        }
        Ok([
            self.info_valid,
            self.emergency_min_level.unwrap_or(255),
            self.emergency_max_level.unwrap_or(255),
            self.rated_duration,
            bits,
        ])
    }
    pub(super) fn decode(raw: [u8; 5]) -> Result<Self> {
        if raw[0] > 2 {
            return Err(bad());
        }
        let b = raw[4];
        Ok(Self {
            info_valid: raw[0],
            emergency_min_level: (raw[1] != 255).then_some(raw[1]),
            emergency_max_level: (raw[2] != 255).then_some(raw[2]),
            rated_duration: raw[3],
            integral_emergency_control_gear: b & 1 != 0,
            maintained_control_gear: b & 2 != 0,
            switched_maintained_control_gear: b & 4 != 0,
            auto_test_capability: b & 8 != 0,
            adjustable_emergency_level: b & 16 != 0,
            hardwired_inhibit_supported: b & 32 != 0,
            physical_selection_supported: b & 64 != 0,
            relight_in_rest_mode_supported: b & 128 != 0,
        })
    }
}
impl std::fmt::Display for ConverterInfoFix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn p(v: Option<u8>) -> String {
            v.map_or_else(|| "none".into(), |v| v.to_string())
        }
        write!(
            f,
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            self.info_valid,
            p(self.emergency_min_level),
            p(self.emergency_max_level),
            self.rated_duration,
            self.integral_emergency_control_gear,
            self.maintained_control_gear,
            self.switched_maintained_control_gear,
            self.auto_test_capability,
            self.adjustable_emergency_level,
            self.hardwired_inhibit_supported,
            self.physical_selection_supported,
            self.relight_in_rest_mode_supported
        )
    }
}
impl std::str::FromStr for ConverterInfoFix {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        fn level(s: &str) -> Result<Option<u8>> {
            if s == "none" {
                Ok(None)
            } else {
                Ok(Some(s.parse().map_err(|_| bad())?))
            }
        }
        let mut p = s.split(',').map(str::trim);
        let v = Self {
            info_valid: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            emergency_min_level: level(p.next().ok_or_else(bad)?)?,
            emergency_max_level: level(p.next().ok_or_else(bad)?)?,
            rated_duration: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            integral_emergency_control_gear: p
                .next()
                .ok_or_else(bad)?
                .parse()
                .map_err(|_| bad())?,
            maintained_control_gear: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            switched_maintained_control_gear: p
                .next()
                .ok_or_else(bad)?
                .parse()
                .map_err(|_| bad())?,
            auto_test_capability: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            adjustable_emergency_level: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            hardwired_inhibit_supported: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            physical_selection_supported: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
            relight_in_rest_mode_supported: p.next().ok_or_else(bad)?.parse().map_err(|_| bad())?,
        };
        if p.next().is_some() {
            return Err(bad());
        }
        v.encode()?;
        Ok(v)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTConverterInfoFix([u8; 5]);
impl DPTConverterInfoFix {
    pub fn new(value: ConverterInfoFix) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> ConverterInfoFix {
        ConverterInfoFix::decode(self.0).expect("validated information")
    }
}
impl DptValue for DPTConverterInfoFix {
    const DPT_NUMBER: &'static str = "248.600";
    const VALUE_TYPE: &'static str = "converter_info_fix";
    const BYTE_LENGTH: usize = 5;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = bytes.try_into().map_err(|_| bad())?;
        ConverterInfoFix::decode(raw)?;
        Ok(Self(raw))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        ConverterInfoFix::decode(self.0).map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTConverterInfoFix> for ConverterInfoFix {
    fn resolve(self, _: DptType) -> Result<DPTConverterInfoFix> {
        DPTConverterInfoFix::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConverterInfoFixView<'a>(pub(super) &'a [u8]);
impl<'a> ConverterInfoFixView<'a> {
    #[must_use]
    pub fn value(&self) -> ConverterInfoFix {
        ConverterInfoFix::decode(self.0.try_into().expect("validated length"))
            .expect("validated information")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
