//! DPT 17.xxx - Scene Number

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 17.001 - Scene Number
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneNumber {
    scene_number: u8,
    data: [u8; 1],
}

impl SceneNumber {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `scene_number` is outside `1..=64`.
    pub fn new(scene_number: u8) -> Result<Self> {
        if !(1..=64).contains(&scene_number) {
            return Err(ProtocolError::DptError {
                dpt_type: "17.001".to_string(),
                details: format!("Scene number {scene_number} out of range [1-64]"),
            }
            .into());
        }

        Ok(Self {
            scene_number,
            data: [scene_number - 1],
        })
    }

    #[must_use]
    pub fn value(&self) -> u8 {
        self.scene_number
    }
}

impl DptValue for SceneNumber {
    const DPT_NUMBER: &'static str = "17.001";
    const VALUE_TYPE: &'static str = "scene_number";
    const BYTE_LENGTH: usize = 1;

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::BYTE_LENGTH {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!(
                    "Invalid length: expected {}, got {}",
                    Self::BYTE_LENGTH,
                    bytes.len()
                ),
            }
            .into());
        }
        let scene_number = (bytes[0] & 0x3F) + 1;
        Ok(Self {
            scene_number,
            data: [scene_number - 1],
        })
    }

    fn validate(&self) -> Result<()> {
        if !(1..=64).contains(&self.scene_number) {
            return Err(ProtocolError::DptError {
                dpt_type: Self::DPT_NUMBER.to_string(),
                details: format!("Scene number {} out of range [1-64]", self.scene_number),
            }
            .into());
        }
        Ok(())
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::decode(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    fn value_range() -> (f64, f64) {
        (1.0, 64.0)
    }
}

// Helper trait for aliases
use super::DptInnerType;

impl DptInnerType for SceneNumber {
    type InnerType = u8;
    fn new(value: u8) -> Self {
        SceneNumber::new(value).unwrap()
    }
    fn into_inner(self) -> u8 {
        self.scene_number
    }
}

// Python-style aliases
use crate::dpt_alias;

dpt_alias!(
    DPTSceneNumber,
    17,
    001,
    SceneNumber,
    "scene_number",
    None,
    None
);
