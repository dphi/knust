//! DPT 255.001 geographical location.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeographicalLocation {
    pub longitude: f32,
    pub latitude: f32,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "255.001".into(),
        details: "Expected longitude -180..=180 and latitude -90..=90".into(),
    }
    .into()
}
impl GeographicalLocation {
    pub(super) fn encode(self) -> Result<[u8; 8]> {
        if !(-180.0..=180.0).contains(&self.longitude) || !(-90.0..=90.0).contains(&self.latitude) {
            return Err(bad());
        }
        let x = self.longitude.to_be_bytes();
        let y = self.latitude.to_be_bytes();
        Ok([x[0], x[1], x[2], x[3], y[0], y[1], y[2], y[3]])
    }
    pub(super) fn decode(b: [u8; 8]) -> Result<Self> {
        let value = Self {
            longitude: f32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            latitude: f32::from_be_bytes([b[4], b[5], b[6], b[7]]),
        };
        value.encode()?;
        Ok(value)
    }
}
impl std::fmt::Display for GeographicalLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{}", self.longitude, self.latitude)
    }
}
impl std::str::FromStr for GeographicalLocation {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let (lon, lat) = s.split_once(',').ok_or_else(bad)?;
        let value = Self {
            longitude: lon.trim().parse().map_err(|_| bad())?,
            latitude: lat.trim().parse().map_err(|_| bad())?,
        };
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct DPTGeographicalLocation([u8; 8]);
impl DPTGeographicalLocation {
    pub fn new(value: GeographicalLocation) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> GeographicalLocation {
        GeographicalLocation::decode(self.0).expect("validated coordinates")
    }
}
impl DptValue for DPTGeographicalLocation {
    const DPT_NUMBER: &'static str = "255.001";
    const VALUE_TYPE: &'static str = "geographical_location";
    const BYTE_LENGTH: usize = 8;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = bytes.try_into().map_err(|_| bad())?;
        GeographicalLocation::decode(raw)?;
        Ok(Self(raw))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        GeographicalLocation::decode(self.0).map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTGeographicalLocation> for GeographicalLocation {
    fn resolve(self, _: DptType) -> Result<DPTGeographicalLocation> {
        DPTGeographicalLocation::new(self)
    }
}
