//! DPT 26.001 scene information.
use super::{DptType, DptValue, WriteValue};
use crate::error::{ProtocolError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneInfo {
    pub scene_number: u8,
    pub active: bool,
}
impl SceneInfo {
    pub(super) fn encode(self) -> Result<u8> {
        if !(1..=64).contains(&self.scene_number) {
            return Err(ProtocolError::DptError {
                dpt_type: "26.001".into(),
                details: "Scene number must be in 1..=64".into(),
            }
            .into());
        }
        Ok((self.scene_number - 1) | if self.active { 0 } else { 0x40 })
    }
    pub(super) fn decode(raw: u8) -> Self {
        Self {
            scene_number: (raw & 0x3f) + 1,
            active: raw & 0x40 == 0,
        }
    }
}
impl std::fmt::Display for SceneInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{}", self.scene_number, self.active)
    }
}
impl std::str::FromStr for SceneInfo {
    type Err = crate::error::KnxError;
    fn from_str(s: &str) -> Result<Self> {
        let bad = || ProtocolError::DptError {
            dpt_type: "26.001".into(),
            details: "Expected scene_number,active".into(),
        };
        let (number, active) = s.split_once(',').ok_or_else(bad)?;
        let value = Self {
            scene_number: number.trim().parse().map_err(|_| bad())?,
            active: active.trim().parse().map_err(|_| bad())?,
        };
        value.encode()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DPTSceneInfo([u8; 1]);
impl DPTSceneInfo {
    pub fn new(value: SceneInfo) -> Result<Self> {
        Ok(Self([value.encode()?]))
    }
    #[must_use]
    pub fn value(&self) -> SceneInfo {
        SceneInfo::decode(self.0[0])
    }
}
impl DptValue for DPTSceneInfo {
    const DPT_NUMBER: &'static str = "26.001";
    const VALUE_TYPE: &'static str = "scene_info";
    const BYTE_LENGTH: usize = 1;
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let [raw] = bytes else {
            return Err(ProtocolError::DptError {
                dpt_type: "26.001".into(),
                details: "Expected one byte".into(),
            }
            .into());
        };
        Ok(Self([*raw]))
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
impl WriteValue<DPTSceneInfo> for SceneInfo {
    fn resolve(self, _: DptType) -> Result<DPTSceneInfo> {
        DPTSceneInfo::new(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SceneInfoView<'a>(pub(super) &'a [u8]);
impl<'a> SceneInfoView<'a> {
    #[must_use]
    pub fn value(&self) -> SceneInfo {
        SceneInfo::decode(self.0[0])
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
