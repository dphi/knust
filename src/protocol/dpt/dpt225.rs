//! DPT 225 scaling speed and step time.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScalingTime {
    pub time_period: u32,
    pub percent: u8,
}
pub type ScalingSpeed = ScalingTime;
pub type ScalingStepTime = ScalingTime;
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "225".into(),
        details: "Invalid time period or percentage".into(),
    }
    .into()
}
impl ScalingTime {
    pub(super) fn encode(self, sub: u16) -> Result<[u8; 3]> {
        let resolution = if sub == 1 { 100 } else { 1 };
        let ticks = self.time_period / resolution;
        if ticks == 0 || ticks > u16::MAX.into() || !(1..=100).contains(&self.percent) {
            return Err(bad());
        }
        let b = (ticks as u16).to_be_bytes();
        let percent = (f64::from(self.percent) / 100.0 * 255.0).round_ties_even() as u8;
        Ok([b[0], b[1], percent])
    }
    pub(super) fn decode(raw: [u8; 3], sub: u16) -> Self {
        let resolution = if sub == 1 { 100 } else { 1 };
        Self {
            time_period: u32::from(u16::from_be_bytes([raw[0], raw[1]])) * resolution,
            percent: (f64::from(raw[2]) / 255.0 * 100.0).round_ties_even() as u8,
        }
    }
}
impl std::fmt::Display for ScalingTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{}", self.time_period, self.percent)
    }
}
impl std::str::FromStr for ScalingTime {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let (time, percent) = s.split_once(',').ok_or_else(bad)?;
        Ok(Self {
            time_period: time.trim().parse().map_err(|_| bad())?,
            percent: percent.trim().parse().map_err(|_| bad())?,
        })
    }
}
macro_rules! scaling_time {
    ($name:ident,$sub:literal,$number:literal,$kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name([u8; 3]);
        impl $name {
            pub fn new(value: ScalingTime) -> Result<Self> {
                Ok(Self(value.encode($sub)?))
            }
            #[must_use]
            pub fn value(&self) -> ScalingTime {
                ScalingTime::decode(self.0, $sub)
            }
        }
        impl DptValue for $name {
            const DPT_NUMBER: &'static str = $number;
            const VALUE_TYPE: &'static str = $kind;
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
        impl WriteValue<$name> for ScalingTime {
            fn resolve(self, _: DptType) -> Result<$name> {
                $name::new(self)
            }
        }
    };
}
scaling_time!(DPTScalingSpeed, 1, "225.001", "scaling_speed");
scaling_time!(DPTScalingStepTime, 2, "225.002", "scaling_step_time");

#[derive(Debug, Clone, Copy)]
pub struct ScalingTimeView<'a>(pub(super) &'a [u8], pub(super) u16);
impl<'a> ScalingTimeView<'a> {
    #[must_use]
    pub fn value(&self) -> ScalingTime {
        ScalingTime::decode([self.0[0], self.0[1], self.0[2]], self.1)
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
