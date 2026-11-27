//! DPT 240.800 combined blind position.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CombinedPosition {
    pub height_position: Option<u8>,
    pub slats_position: Option<u8>,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "240.800".into(),
        details: "Expected two percentages in 0..=100 or none".into(),
    }
    .into()
}
impl CombinedPosition {
    pub(super) fn encode(self) -> Result<[u8; 3]> {
        fn scale(v: Option<u8>) -> Result<u8> {
            match v {
                None => Ok(0),
                Some(v) if v <= 100 => Ok((f64::from(v) / 100.0 * 255.0).round_ties_even() as u8),
                _ => Err(bad()),
            }
        }
        Ok([
            scale(self.height_position)?,
            scale(self.slats_position)?,
            u8::from(self.height_position.is_some()) | u8::from(self.slats_position.is_some()) << 1,
        ])
    }
    pub(super) fn decode(raw: [u8; 3]) -> Self {
        let scale = |v: u8| (f64::from(v) / 255.0 * 100.0).round_ties_even() as u8;
        Self {
            height_position: (raw[2] & 1 != 0).then(|| scale(raw[0])),
            slats_position: (raw[2] & 2 != 0).then(|| scale(raw[1])),
        }
    }
}
impl std::fmt::Display for CombinedPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = |v: Option<u8>| v.map_or_else(|| "none".into(), |x| x.to_string());
        write!(f, "{},{}", p(self.height_position), p(self.slats_position))
    }
}
impl std::str::FromStr for CombinedPosition {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let parse = |s: &str| -> Result<Option<u8>> {
            if s.trim() == "none" {
                Ok(None)
            } else {
                Ok(Some(s.trim().parse().map_err(|_| bad())?))
            }
        };
        let (height, slats) = s.split_once(',').ok_or_else(bad)?;
        let value = Self {
            height_position: parse(height)?,
            slats_position: parse(slats)?,
        };
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTCombinedPosition([u8; 3]);
impl DPTCombinedPosition {
    ///
    /// # Errors
    ///
    /// Returns an error if the value is outside the valid range or cannot be encoded.
    pub fn new(value: CombinedPosition) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> CombinedPosition {
        CombinedPosition::decode(self.0)
    }
}
impl DptValue for DPTCombinedPosition {
    const DPT_NUMBER: &'static str = "240.800";
    const VALUE_TYPE: &'static str = "combined_position";
    const BYTE_LENGTH: usize = 3;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self(bytes.try_into().map_err(|_| bad())?))
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        Ok(())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTCombinedPosition> for CombinedPosition {
    fn resolve(self, _: DptType) -> Result<DPTCombinedPosition> {
        DPTCombinedPosition::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CombinedPositionView<'a>(pub(super) &'a [u8]);
impl<'a> CombinedPositionView<'a> {
    #[must_use]
    pub fn value(&self) -> CombinedPosition {
        CombinedPosition::decode([self.0[0], self.0[1], self.0[2]])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
