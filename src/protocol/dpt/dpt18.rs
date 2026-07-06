//! DPT 18.xxx - Scene Control

use super::{DptValue, Result};
use crate::error::ProtocolError;

/// DPT 18.001 - Scene Control
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneControl {
    pub scene_number: u8,
    pub learn: bool,
    data: [u8; 1],
}

impl SceneControl {
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `scene_number` is outside `1..=64`.
    pub fn new(scene_number: u8, learn: bool) -> Result<Self> {
        if !(1..=64).contains(&scene_number) {
            return Err(ProtocolError::DptError {
                dpt_type: "18.001".to_string(),
                details: format!("Scene number {scene_number} out of range [1-64]"),
            }
            .into());
        }

        let mut byte = scene_number - 1;
        if learn {
            byte |= 0x80;
        }

        Ok(Self {
            scene_number,
            learn,
            data: [byte],
        })
    }
}

impl DptValue for SceneControl {
    const DPT_NUMBER: &'static str = "18.001";
    const VALUE_TYPE: &'static str = "scene_control";
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

        let byte = bytes[0];
        let learn = (byte & 0x80) != 0;
        let scene_number = (byte & 0x3F) + 1;

        Ok(SceneControl {
            scene_number,
            learn,
            data: [byte],
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
