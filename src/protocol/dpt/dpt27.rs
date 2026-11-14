//! DPT 27.001 combined on/off information.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CombinedInfoOnOff {
    pub outputs: [Option<bool>; 16],
}
impl CombinedInfoOnOff {
    pub(super) fn encode(self) -> [u8; 4] {
        let mut raw = 0u32;
        for (bit, value) in self.outputs.iter().enumerate() {
            if let Some(on) = value {
                raw |= 1 << (bit + 16);
                if *on {
                    raw |= 1 << bit;
                }
            }
        }
        raw.to_be_bytes()
    }
    pub(super) fn decode(bytes: [u8; 4]) -> Self {
        let raw = u32::from_be_bytes(bytes);
        Self {
            outputs: std::array::from_fn(|bit| {
                (raw & (1 << (bit + 16)) != 0).then_some(raw & (1 << bit) != 0)
            }),
        }
    }
}
impl std::fmt::Display for CombinedInfoOnOff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for value in self.outputs {
            write!(
                f,
                "{}",
                match value {
                    None => '-',
                    Some(false) => '0',
                    Some(true) => '1',
                }
            )?;
        }
        Ok(())
    }
}
impl std::str::FromStr for CombinedInfoOnOff {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let bad = || ProtocolError::DptError {
            dpt_type: "27.001".into(),
            details: "Expected 16 output states using -, 0 or 1".into(),
        };
        if s.len() != 16 {
            return Err(bad().into());
        }
        let mut outputs = [None; 16];
        for (index, byte) in s.bytes().enumerate() {
            outputs[index] = match byte {
                b'-' => None,
                b'0' => Some(false),
                b'1' => Some(true),
                _ => return Err(bad().into()),
            };
        }
        Ok(Self { outputs })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTCombinedInfoOnOff([u8; 4]);
impl DPTCombinedInfoOnOff {
    #[must_use]
    pub fn new(value: CombinedInfoOnOff) -> Self {
        Self(value.encode())
    }
    #[must_use]
    pub fn value(&self) -> CombinedInfoOnOff {
        CombinedInfoOnOff::decode(self.0)
    }
}
impl DptValue for DPTCombinedInfoOnOff {
    const DPT_NUMBER: &'static str = "27.001";
    const VALUE_TYPE: &'static str = "combined_info_on_off";
    const BYTE_LENGTH: usize = 4;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = bytes.try_into().map_err(|_| ProtocolError::DptError {
            dpt_type: "27.001".into(),
            details: "Expected four bytes".into(),
        })?;
        Ok(Self(raw))
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
impl WriteValue<DPTCombinedInfoOnOff> for CombinedInfoOnOff {
    fn resolve(self, _: DptType) -> Result<DPTCombinedInfoOnOff> {
        Ok(DPTCombinedInfoOnOff::new(self))
    }
}
