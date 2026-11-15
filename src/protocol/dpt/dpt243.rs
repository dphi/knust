//! DPT 243.600 xyY colour transition.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XYYColorTransition {
    pub fade_time: f64,
    pub color: Option<(f64, f64)>,
    pub brightness: Option<u8>,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "243.600".into(),
        details: "Invalid xyY transition".into(),
    }
    .into()
}
impl XYYColorTransition {
    pub(super) fn encode(self) -> Result<[u8; 8]> {
        if !(0.0..=6553.5).contains(&self.fade_time) {
            return Err(bad());
        }
        let fade = (self.fade_time * 10.0).round_ties_even() as u16;
        let (x, y) = match self.color {
            None => (0, 0),
            Some((x, y)) if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) => (
                (x * 65535.0).round_ties_even() as u16,
                (y * 65535.0).round_ties_even() as u16,
            ),
            _ => return Err(bad()),
        };
        let f = fade.to_be_bytes();
        let x = x.to_be_bytes();
        let y = y.to_be_bytes();
        Ok([
            f[0],
            f[1],
            x[0],
            x[1],
            y[0],
            y[1],
            self.brightness.unwrap_or(0),
            u8::from(self.color.is_some()) << 1 | u8::from(self.brightness.is_some()),
        ])
    }
    pub(super) fn decode(raw: [u8; 8]) -> Self {
        let axis = |a, b| {
            (f64::from(u16::from_be_bytes([a, b])) / 65535.0 * 100_000.0).round_ties_even()
                / 100_000.0
        };
        Self {
            fade_time: f64::from(u16::from_be_bytes([raw[0], raw[1]])) / 10.0,
            color: (raw[7] & 2 != 0).then(|| (axis(raw[2], raw[3]), axis(raw[4], raw[5]))),
            brightness: (raw[7] & 1 != 0).then_some(raw[6]),
        }
    }
}
impl std::fmt::Display for XYYColorTransition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (x, y) = self.color.map_or_else(
            || ("none".to_string(), "none".to_string()),
            |(x, y)| (x.to_string(), y.to_string()),
        );
        let b = self
            .brightness
            .map_or_else(|| "none".into(), |v| v.to_string());
        write!(f, "{},{},{},{}", self.fade_time, x, y, b)
    }
}
impl std::str::FromStr for XYYColorTransition {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        fn p<T: std::str::FromStr>(s: &str) -> Result<Option<T>> {
            if s.trim() == "none" {
                Ok(None)
            } else {
                Ok(Some(s.trim().parse().map_err(|_| bad())?))
            }
        }
        let mut fields = s.split(',');
        let fade_time = fields
            .next()
            .ok_or_else(bad)?
            .trim()
            .parse()
            .map_err(|_| bad())?;
        let x = p(fields.next().ok_or_else(bad)?)?;
        let y = p(fields.next().ok_or_else(bad)?)?;
        let brightness = p(fields.next().ok_or_else(bad)?)?;
        if fields.next().is_some() {
            return Err(bad());
        }
        let color = match (x, y) {
            (None, None) => None,
            (Some(x), Some(y)) => Some((x, y)),
            _ => return Err(bad()),
        };
        let value = Self {
            fade_time,
            color,
            brightness,
        };
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct DPTColorXYYTransition([u8; 8]);
impl DPTColorXYYTransition {
    pub fn new(value: XYYColorTransition) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> XYYColorTransition {
        XYYColorTransition::decode(self.0)
    }
}
impl DptValue for DPTColorXYYTransition {
    const DPT_NUMBER: &'static str = "243.600";
    const VALUE_TYPE: &'static str = "color_xyy_transition";
    const BYTE_LENGTH: usize = 8;
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
impl WriteValue<DPTColorXYYTransition> for XYYColorTransition {
    fn resolve(self, _: DptType) -> Result<DPTColorXYYTransition> {
        DPTColorXYYTransition::new(self)
    }
}
