//! DPT 30.1010 channel activation.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelActivation24 {
    pub channels: [bool; 24],
}
impl ChannelActivation24 {
    pub(super) fn encode(self) -> [u8; 3] {
        let mut raw = 0u32;
        for (bit, active) in self.channels.iter().enumerate() {
            if *active {
                raw |= 1 << bit;
            }
        }
        let bytes = raw.to_be_bytes();
        [bytes[1], bytes[2], bytes[3]]
    }
    pub(super) fn decode(bytes: [u8; 3]) -> Self {
        let raw = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
        Self {
            channels: std::array::from_fn(|bit| raw & (1 << bit) != 0),
        }
    }
}
impl std::fmt::Display for ChannelActivation24 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for value in self.channels {
            write!(f, "{}", u8::from(value))?;
        }
        Ok(())
    }
}
impl std::str::FromStr for ChannelActivation24 {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let bad = || ProtocolError::DptError {
            dpt_type: "30.1010".into(),
            details: "Expected 24 channel states using 0 or 1".into(),
        };
        if s.len() != 24 {
            return Err(bad().into());
        }
        let mut channels = [false; 24];
        for (index, byte) in s.bytes().enumerate() {
            channels[index] = match byte {
                b'0' => false,
                b'1' => true,
                _ => return Err(bad().into()),
            };
        }
        Ok(Self { channels })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTChannelActivation24([u8; 3]);
impl DPTChannelActivation24 {
    #[must_use]
    pub fn new(value: ChannelActivation24) -> Self {
        Self(value.encode())
    }
    #[must_use]
    pub fn value(&self) -> ChannelActivation24 {
        ChannelActivation24::decode(self.0)
    }
}
impl DptValue for DPTChannelActivation24 {
    const DPT_NUMBER: &'static str = "30.1010";
    const VALUE_TYPE: &'static str = "channel_activation_24";
    const BYTE_LENGTH: usize = 3;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self(bytes.try_into().map_err(|_| {
            ProtocolError::DptError {
                dpt_type: "30.1010".into(),
                details: "Expected three bytes".into(),
            }
        })?))
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
impl WriteValue<DPTChannelActivation24> for ChannelActivation24 {
    fn resolve(self, _: DptType) -> Result<DPTChannelActivation24> {
        Ok(DPTChannelActivation24::new(self))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChannelActivation24View<'a>(pub(super) &'a [u8]);
impl<'a> ChannelActivation24View<'a> {
    #[must_use]
    pub fn value(&self) -> ChannelActivation24 {
        ChannelActivation24::decode([self.0[0], self.0[1], self.0[2]])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
