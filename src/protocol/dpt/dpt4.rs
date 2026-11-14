//! DPT 4.xxx - single ASCII or Latin-1 characters.

use super::{DptValue, Result};
use crate::error::ProtocolError;

pub(super) fn encode_character(value: &str, latin1: bool) -> Result<[u8; 1]> {
    let mut characters = value.chars();
    let character = characters.next().unwrap_or('\0');
    if characters.next().is_some() {
        return Err(ProtocolError::DptError {
            dpt_type: if latin1 { "4.002" } else { "4.001" }.to_string(),
            details: "Character value must contain at most one character".to_string(),
        }
        .into());
    }
    Ok([if character as u32 <= if latin1 { 0xff } else { 0x7f } {
        character as u8
    } else {
        b'?'
    }])
}

macro_rules! character_dpt {
    ($name:ident, $number:literal, $value_type:literal, $latin1:literal) => {
        #[doc = concat!("DPT ", $number, " - a single character, stored as one wire octet.")]
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name([u8; 1]);

        impl $name {
            /// Empty input encodes as NUL; unrepresentable characters become `?`.
            ///
            /// # Errors
            ///
            /// Returns an error if `value` contains more than one character.
            pub fn new(value: impl AsRef<str>) -> Result<Self> {
                Ok(Self(encode_character(value.as_ref(), $latin1)?))
            }

            /// Decode the character, omitting NUL and replacing invalid ASCII.
            #[must_use]
            pub fn value(&self) -> String {
                super::view::CharacterView(&self.0, $latin1).as_string()
            }
        }

        impl DptValue for $name {
            const DPT_NUMBER: &'static str = $number;
            const VALUE_TYPE: &'static str = $value_type;
            const BYTE_LENGTH: usize = 1;

            fn from_bytes(bytes: &[u8]) -> Result<Self> {
                let payload = bytes.try_into().map_err(|_| ProtocolError::DptError {
                    dpt_type: Self::DPT_NUMBER.to_string(),
                    details: format!("Invalid length: expected 1, got {}", bytes.len()),
                })?;
                Ok(Self(payload))
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
    };
}

character_dpt!(CharacterAscii, "4.001", "character", false);
character_dpt!(CharacterLatin1, "4.002", "character_latin_1", true);

/// XKNX-compatible name for DPT 4.001.
pub type DPTCharacter = CharacterAscii;
/// XKNX-compatible name for DPT 4.002.
pub type DPTCharacterLatin1 = CharacterLatin1;
