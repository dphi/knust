//! DPT 25.1000 busy and NAK repetitions.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DoubleNibble {
    pub busy: u8,
    pub nak: u8,
}

impl DoubleNibble {
    pub(super) fn encode(self) -> Result<u8> {
        if self.busy > 3 || self.nak > 3 {
            return Err(ProtocolError::DptError {
                dpt_type: "25.1000".into(),
                details: "Repetitions must be in 0..=3".into(),
            }
            .into());
        }
        Ok(self.busy << 4 | self.nak)
    }
    pub(super) fn decode(raw: u8) -> Result<Self> {
        let value = Self {
            busy: raw >> 4,
            nak: raw & 15,
        };
        value.encode()?;
        Ok(value)
    }
}

impl std::fmt::Display for DoubleNibble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{}", self.busy, self.nak)
    }
}

impl std::str::FromStr for DoubleNibble {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let bad = || ProtocolError::DptError {
            dpt_type: "25.1000".into(),
            details: "Expected busy,nak".into(),
        };
        let (busy, nak) = s.split_once(',').ok_or_else(bad)?;
        let value = Self {
            busy: busy.trim().parse().map_err(|_| bad())?,
            nak: nak.trim().parse().map_err(|_| bad())?,
        };
        value.encode()?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTDoubleNibble([u8; 1]);
impl DPTDoubleNibble {
    ///
    /// # Errors
    ///
    /// Returns an error if the value is outside the valid range or cannot be encoded.
    pub fn new(value: DoubleNibble) -> Result<Self> {
        Ok(Self([value.encode()?]))
    }
    #[must_use]
    pub fn value(&self) -> DoubleNibble {
        DoubleNibble {
            busy: self.0[0] >> 4,
            nak: self.0[0] & 15,
        }
    }
}
impl DptValue for DPTDoubleNibble {
    const DPT_NUMBER: &'static str = "25.1000";
    const VALUE_TYPE: &'static str = "double_nibble";
    const BYTE_LENGTH: usize = 1;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let [raw] = bytes else {
            return Err(ProtocolError::DptError {
                dpt_type: "25.1000".into(),
                details: "Expected one byte".into(),
            }
            .into());
        };
        Self::new(DoubleNibble::decode(*raw)?)
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        self.value().encode().map(|_| ())
    }
    fn value_range() -> (f64, f64) {
        (0.0, 0.0)
    }
}
impl WriteValue<DPTDoubleNibble> for DoubleNibble {
    fn resolve(self, _: DptType) -> Result<DPTDoubleNibble> {
        DPTDoubleNibble::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DoubleNibbleView<'a>(pub(super) &'a [u8]);
impl<'a> DoubleNibbleView<'a> {
    #[must_use]
    pub fn value(&self) -> DoubleNibble {
        DoubleNibble {
            busy: self.0[0] >> 4,
            nak: self.0[0] & 15,
        }
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
