//! DPT 206 mode and time delay.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModeNext {
    pub mode: Option<u8>,
    pub delay_time: Option<u16>,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "206".into(),
        details: "Invalid mode or delay".into(),
    }
    .into()
}
impl ModeNext {
    pub(super) fn encode(self, sub: u16) -> Result<[u8; 3]> {
        if self.delay_time == Some(0) {
            return Err(bad());
        }
        let mode = match (sub, self.mode) {
            (100 | 102, None) => 0,
            (100 | 102, Some(value @ 1..=4)) | (104 | 105, Some(value @ 0..=2)) => value,
            _ => return Err(bad()),
        };
        let time = self.delay_time.unwrap_or(0).to_be_bytes();
        Ok([time[0], time[1], mode])
    }
    pub(super) fn decode(raw: [u8; 3], sub: u16) -> Result<Self> {
        let time = u16::from_be_bytes([raw[0], raw[1]]);
        let mode = if matches!(sub, 100 | 102) && raw[2] == 0 {
            None
        } else {
            Some(raw[2])
        };
        let value = Self {
            mode,
            delay_time: (time != 0).then_some(time),
        };
        value.encode(sub)?;
        Ok(value)
    }
}
impl std::fmt::Display for ModeNext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn p<T: std::fmt::Display>(v: Option<T>) -> String {
            v.map_or_else(|| "none".into(), |x| x.to_string())
        }
        write!(f, "{},{}", p(self.mode), p(self.delay_time))
    }
}
impl std::str::FromStr for ModeNext {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        fn p<T: std::str::FromStr>(s: &str) -> Result<Option<T>> {
            if s.trim() == "none" {
                Ok(None)
            } else {
                Ok(Some(s.trim().parse().map_err(|_| bad())?))
            }
        }
        let (mode, delay) = s.split_once(',').ok_or_else(bad)?;
        Ok(Self {
            mode: p(mode)?,
            delay_time: p(delay)?,
        })
    }
}
macro_rules! mode {
    ($name:ident,$sub:literal,$number:literal,$kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name([u8; 3]);
        impl $name {
            ///
            /// # Errors
            ///
            /// Returns an error if the value is outside the valid range or cannot be encoded.
            pub fn new(value: ModeNext) -> Result<Self> {
                Ok(Self(value.encode($sub)?))
            }
            #[must_use]
            pub fn value(&self) -> ModeNext {
                ModeNext::decode(self.0, $sub).expect("validated mode")
            }
        }
        impl DptValue for $name {
            const DPT_NUMBER: &'static str = $number;
            const VALUE_TYPE: &'static str = $kind;
            const BYTE_LENGTH: usize = 3;
            fn from_bytes(bytes: &[u8]) -> Result<Self> {
                let raw = bytes.try_into().map_err(|_| bad())?;
                ModeNext::decode(raw, $sub)?;
                Ok(Self(raw))
            }
            fn as_bytes(&self) -> &[u8] {
                &self.0
            }
            fn validate(&self) -> Result<()> {
                ModeNext::decode(self.0, $sub).map(|_| ())
            }
            fn value_range() -> (f64, f64) {
                (0.0, 0.0)
            }
        }
        impl WriteValue<$name> for ModeNext {
            fn resolve(self, _: DptType) -> Result<$name> {
                $name::new(self)
            }
        }
    };
}
mode!(DPTHVACModeNext, 100, "206.100", "hvac_mode_next");
mode!(DPTDHWModeNext, 102, "206.102", "dhw_mode_next");
mode!(DPTOccModeNext, 104, "206.104", "occupancy_mode_next");
mode!(DPTBuildingModeNext, 105, "206.105", "building_mode_next");

#[derive(Debug, Clone, Copy)]
pub struct ModeNextView<'a>(pub(super) &'a [u8], pub(super) u16);
impl<'a> ModeNextView<'a> {
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> ModeNext {
        ModeNext::decode([self.0[0], self.0[1], self.0[2]], self.1).expect("validated mode")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
