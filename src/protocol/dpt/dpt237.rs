//! DPT 237.600 DALI control gear diagnostics.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DALIControlGearDiagnostic {
    pub address: u8,
    pub group: bool,
    pub read_request: bool,
    pub lamp_failure: bool,
    pub ballast_failure: bool,
    pub converter_error: bool,
}
fn bad(details: &str) -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "237.600".into(),
        details: details.into(),
    }
    .into()
}
impl DALIControlGearDiagnostic {
    pub(super) fn encode(self) -> Result<[u8; 2]> {
        if self.address > if self.group { 15 } else { 63 } {
            return Err(bad("DALI address out of range"));
        }
        Ok([
            u8::from(self.lamp_failure)
                | u8::from(self.ballast_failure) << 1
                | u8::from(self.converter_error) << 2,
            self.address | u8::from(self.group) << 6 | u8::from(self.read_request) << 7,
        ])
    }
    pub(super) fn decode(raw: [u8; 2]) -> Result<Self> {
        let value = Self {
            address: raw[1] & 63,
            group: raw[1] & 64 != 0,
            read_request: raw[1] & 128 != 0,
            lamp_failure: raw[0] & 1 != 0,
            ballast_failure: raw[0] & 2 != 0,
            converter_error: raw[0] & 4 != 0,
        };
        value.encode()?;
        Ok(value)
    }
}
impl std::fmt::Display for DALIControlGearDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{},{},{},{}",
            self.address,
            self.group,
            self.read_request,
            self.lamp_failure,
            self.ballast_failure,
            self.converter_error
        )
    }
}
impl std::str::FromStr for DALIControlGearDiagnostic {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let err = || bad("Expected address,group,read,lamp,ballast,converter");
        let mut p = s.split(',').map(str::trim);
        let v = Self {
            address: p.next().ok_or_else(err)?.parse().map_err(|_| err())?,
            group: p.next().ok_or_else(err)?.parse().map_err(|_| err())?,
            read_request: p.next().ok_or_else(err)?.parse().map_err(|_| err())?,
            lamp_failure: p.next().ok_or_else(err)?.parse().map_err(|_| err())?,
            ballast_failure: p.next().ok_or_else(err)?.parse().map_err(|_| err())?,
            converter_error: p.next().ok_or_else(err)?.parse().map_err(|_| err())?,
        };
        if p.next().is_some() {
            return Err(err());
        }
        v.encode()?;
        Ok(v)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTDALIControlGearDiagnostic([u8; 2]);
impl DPTDALIControlGearDiagnostic {
    ///
    /// # Errors
    ///
    /// Returns an error if the value is outside the valid range or cannot be encoded.
    pub fn new(value: DALIControlGearDiagnostic) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> DALIControlGearDiagnostic {
        DALIControlGearDiagnostic::decode(self.0).expect("validated address")
    }
}
impl DptValue for DPTDALIControlGearDiagnostic {
    const DPT_NUMBER: &'static str = "237.600";
    const VALUE_TYPE: &'static str = "dali_control_gear_diagnostic";
    const BYTE_LENGTH: usize = 2;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = bytes.try_into().map_err(|_| bad("Expected two bytes"))?;
        DALIControlGearDiagnostic::decode(raw)?;
        Ok(Self(raw))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        DALIControlGearDiagnostic::decode(self.0).map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTDALIControlGearDiagnostic> for DALIControlGearDiagnostic {
    fn resolve(self, _: DptType) -> Result<DPTDALIControlGearDiagnostic> {
        DPTDALIControlGearDiagnostic::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DALIControlGearDiagnosticView<'a>(pub(super) &'a [u8]);
impl<'a> DALIControlGearDiagnosticView<'a> {
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> DALIControlGearDiagnostic {
        DALIControlGearDiagnostic::decode([self.0[0], self.0[1]]).expect("validated address")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
