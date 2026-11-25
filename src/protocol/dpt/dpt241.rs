//! DPT 241.800 shutter and sunblind actuator status.
use super::{CombinedPosition, DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatusSAB {
    pub height_position: Option<u8>,
    pub slats_position: Option<u8>,
    pub upper_end_position: bool,
    pub lower_end_position: bool,
    pub lower_predefined_position: bool,
    pub target_position_reached: bool,
    pub target_height_position_restricted: bool,
    pub target_slats_position_restricted: bool,
    pub weather_alarm: bool,
    pub forced: bool,
    pub locked: bool,
    pub local_override: bool,
    pub failure: bool,
}
fn bad() -> crate::error::KnxError {
    ProtocolError::DptError {
        dpt_type: "241.800".into(),
        details: "Expected height,slats and eleven status bits".into(),
    }
    .into()
}
impl StatusSAB {
    pub(super) fn encode(self) -> Result<[u8; 4]> {
        let position = CombinedPosition {
            height_position: self.height_position,
            slats_position: self.slats_position,
        }
        .encode()?;
        let flags = [
            self.upper_end_position,
            self.lower_end_position,
            self.lower_predefined_position,
            self.target_position_reached,
            self.target_height_position_restricted,
            self.target_slats_position_restricted,
            self.weather_alarm,
            self.forced,
            self.locked,
            self.local_override,
            self.failure,
        ];
        let mut attributes = u16::from(position[2]) << 14;
        for (bit, enabled) in flags.into_iter().enumerate() {
            if enabled {
                attributes |= 1 << bit;
            }
        }
        let b = attributes.to_be_bytes();
        Ok([position[0], position[1], b[0], b[1]])
    }
    pub(super) fn decode(raw: [u8; 4]) -> Self {
        let bits = u16::from_be_bytes([raw[2], raw[3]]);
        let pos = CombinedPosition::decode([raw[0], raw[1], (bits >> 14) as u8]);
        let flag = |bit: u32| bits & (1u16 << bit) != 0u16;
        Self {
            height_position: pos.height_position,
            slats_position: pos.slats_position,
            upper_end_position: flag(0),
            lower_end_position: flag(1),
            lower_predefined_position: flag(2),
            target_position_reached: flag(3),
            target_height_position_restricted: flag(4),
            target_slats_position_restricted: flag(5),
            weather_alarm: flag(6),
            forced: flag(7),
            locked: flag(8),
            local_override: flag(9),
            failure: flag(10),
        }
    }
}
impl std::fmt::Display for StatusSAB {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let position = CombinedPosition {
            height_position: self.height_position,
            slats_position: self.slats_position,
        };
        let flags = [
            self.upper_end_position,
            self.lower_end_position,
            self.lower_predefined_position,
            self.target_position_reached,
            self.target_height_position_restricted,
            self.target_slats_position_restricted,
            self.weather_alarm,
            self.forced,
            self.locked,
            self.local_override,
            self.failure,
        ];
        write!(f, "{position},")?;
        for flag in flags {
            write!(f, "{}", u8::from(flag))?;
        }
        Ok(())
    }
}
impl std::str::FromStr for StatusSAB {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let (position, flags) = s.rsplit_once(',').ok_or_else(bad)?;
        let position: CombinedPosition = position.parse()?;
        if flags.len() != 11 || !flags.bytes().all(|b| b == b'0' || b == b'1') {
            return Err(bad());
        }
        let mut raw = [0; 4];
        let p = position.encode()?;
        raw[0] = p[0];
        raw[1] = p[1];
        let mut attributes = u16::from(p[2]) << 14;
        for (bit, flag) in flags.bytes().enumerate() {
            if flag == b'1' {
                attributes |= 1 << bit;
            }
        }
        let a = attributes.to_be_bytes();
        raw[2] = a[0];
        raw[3] = a[1];
        Ok(Self::decode(raw))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTStatusSAB([u8; 4]);
impl DPTStatusSAB {
    pub fn new(value: StatusSAB) -> Result<Self> {
        Ok(Self(value.encode()?))
    }
    #[must_use]
    pub fn value(&self) -> StatusSAB {
        StatusSAB::decode(self.0)
    }
}
impl DptValue for DPTStatusSAB {
    const DPT_NUMBER: &'static str = "241.800";
    const VALUE_TYPE: &'static str = "status_sab";
    const BYTE_LENGTH: usize = 4;
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
impl WriteValue<DPTStatusSAB> for StatusSAB {
    fn resolve(self, _: DptType) -> Result<DPTStatusSAB> {
        DPTStatusSAB::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StatusSABView<'a>(pub(super) &'a [u8]);
impl<'a> StatusSABView<'a> {
    #[must_use]
    pub fn value(&self) -> StatusSAB {
        StatusSAB::decode([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
