//! KNX heating/cooling mode datapoints.

use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

macro_rules! mode {
    ($name:ident, $number:literal, $kind:literal, $max:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name([u8; 1]);

        impl $name {
            /// Construct a mode from its KNX value.
            pub fn new(value: u8) -> Result<Self> {
                if value > $max {
                    return Err(ProtocolError::DptError {
                        dpt_type: $number.to_string(),
                        details: format!("Reserved mode value {value}"),
                    }
                    .into());
                }
                Ok(Self([value]))
            }

            #[must_use]
            pub fn value(&self) -> u8 {
                self.0[0]
            }
        }

        impl DptValue for $name {
            const DPT_NUMBER: &'static str = $number;
            const VALUE_TYPE: &'static str = $kind;
            const BYTE_LENGTH: usize = 1;

            fn from_bytes(bytes: &[u8]) -> Result<Self> {
                let [value] = bytes else {
                    return Err(ProtocolError::DptError {
                        dpt_type: $number.to_string(),
                        details: format!("Expected one byte, got {}", bytes.len()),
                    }
                    .into());
                };
                Self::new(*value)
            }
            fn as_bytes(&self) -> &[u8] {
                &self.0
            }
            fn validate(&self) -> Result<()> {
                Self::new(self.0[0]).map(|_| ())
            }
            fn value_range() -> (f64, f64) {
                (0.0, f64::from($max))
            }
        }

        impl WriteValue<$name> for u8 {
            fn resolve(self, _dpt: DptType) -> Result<$name> {
                $name::new(self)
            }
        }
    };
}

mode!(DPTChangeoverMode, "20.107", "changeover_mode", 2);
mode!(DPTEnableHCStage, "20.116", "enable_hc_stage", 3);
