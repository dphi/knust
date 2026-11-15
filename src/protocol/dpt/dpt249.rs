//! DPT 249.600 colour temperature transition.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTemperatureTransition {
    pub fade_time: Option<f64>,
    pub color_temperature: Option<u16>,
    pub brightness: Option<u8>,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "249.600".into(),
        details: "Invalid colour temperature transition".into(),
    }
    .into()
}
impl ColorTemperatureTransition {
    pub(super) fn encode(self) -> Result<[u8; 6]> {
        let fade = match self.fade_time {
            None => 0,
            Some(v) if (0.0..=6553.5).contains(&v) => (v * 10.0).round_ties_even() as u16,
            _ => return Err(bad()),
        }
        .to_be_bytes();
        let color = self.color_temperature.unwrap_or(0).to_be_bytes();
        Ok([
            fade[0],
            fade[1],
            color[0],
            color[1],
            self.brightness.unwrap_or(0),
            u8::from(self.fade_time.is_some()) << 2
                | u8::from(self.color_temperature.is_some()) << 1
                | u8::from(self.brightness.is_some()),
        ])
    }
    pub(super) fn decode(raw: [u8; 6]) -> Self {
        let flags = raw[5];
        Self {
            fade_time: (flags & 4 != 0)
                .then(|| f64::from(u16::from_be_bytes([raw[0], raw[1]])) / 10.0),
            color_temperature: (flags & 2 != 0).then(|| u16::from_be_bytes([raw[2], raw[3]])),
            brightness: (flags & 1 != 0).then_some(raw[4]),
        }
    }
}
impl std::fmt::Display for ColorTemperatureTransition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn p<T: std::fmt::Display>(v: Option<T>) -> String {
            v.map_or_else(|| "none".into(), |v| v.to_string())
        }
        write!(
            f,
            "{},{},{}",
            p(self.fade_time),
            p(self.color_temperature),
            p(self.brightness)
        )
    }
}
impl std::str::FromStr for ColorTemperatureTransition {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        fn p<T: std::str::FromStr>(v: &str) -> Result<Option<T>> {
            if v.trim() == "none" {
                Ok(None)
            } else {
                Ok(Some(v.trim().parse().map_err(|_| bad())?))
            }
        }
        let mut values = s.split(',');
        let value = Self {
            fade_time: p(values.next().ok_or_else(bad)?)?,
            color_temperature: p(values.next().ok_or_else(bad)?)?,
            brightness: p(values.next().ok_or_else(bad)?)?,
        };
        if values.next().is_some() {
            return Err(bad());
        }
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct DPTColorTemperatureTransition([u8; 6]);
impl DPTColorTemperatureTransition {
    pub fn new(value: ColorTemperatureTransition) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> ColorTemperatureTransition {
        ColorTemperatureTransition::decode(self.0)
    }
}
impl DptValue for DPTColorTemperatureTransition {
    const DPT_NUMBER: &'static str = "249.600";
    const VALUE_TYPE: &'static str = "color_temperature_transition";
    const BYTE_LENGTH: usize = 6;
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
impl WriteValue<DPTColorTemperatureTransition> for ColorTemperatureTransition {
    fn resolve(self, _: DptType) -> Result<DPTColorTemperatureTransition> {
        DPTColorTemperatureTransition::new(self)
    }
}
