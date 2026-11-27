//! DPT 234 two-letter language and region codes.
use super::DptValue;
use crate::error::{ProtocolError, Result};

pub(super) fn encode_code(value: &str, uppercase: bool) -> Result<[u8; 2]> {
    let bytes = value.as_bytes();
    if bytes.len() != 2 || !bytes.iter().all(u8::is_ascii_alphabetic) {
        return Err(ProtocolError::DptError {
            dpt_type: if uppercase { "234.002" } else { "234.001" }.into(),
            details: "Expected two ASCII letters".into(),
        }
        .into());
    }
    let convert = |b: u8| {
        if uppercase {
            b.to_ascii_uppercase()
        } else {
            b.to_ascii_lowercase()
        }
    };
    Ok([convert(bytes[0]), convert(bytes[1])])
}
pub(super) fn decode_code(bytes: &[u8], uppercase: bool) -> Result<String> {
    let text = std::str::from_utf8(bytes).map_err(|_| ProtocolError::DptError {
        dpt_type: "234".into(),
        details: "Expected ASCII letters".into(),
    })?;
    Ok(String::from_utf8(encode_code(text, uppercase)?.to_vec()).expect("ASCII code"))
}
macro_rules! code {
    ($name:ident, $number:literal, $kind:literal, $upper:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name([u8; 2]);
        impl $name {
            ///
            /// # Errors
            ///
            /// Returns an error if the value is outside the valid range or cannot be encoded.
            pub fn new(value: &str) -> Result<Self> {
                Ok(Self(encode_code(value, $upper)?))
            }
            #[must_use]
            pub fn value(&self) -> String {
                decode_code(&self.0, $upper).expect("validated code")
            }
        }
        impl DptValue for $name {
            const DPT_NUMBER: &'static str = $number;
            const VALUE_TYPE: &'static str = $kind;
            const BYTE_LENGTH: usize = 2;
            fn from_bytes(bytes: &[u8]) -> Result<Self> {
                decode_code(bytes, $upper)?;
                Ok(Self([bytes[0], bytes[1]]))
            }
            fn as_bytes(&self) -> &[u8] {
                &self.0
            }
            fn validate(&self) -> Result<()> {
                decode_code(&self.0, $upper).map(|_| ())
            }
            fn value_range() -> (f64, f64) {
                (0.0, 0.0)
            }
        }
    };
}
code!(
    DPTLanguageCodeAlpha2ASCII,
    "234.001",
    "language_code_alpha2",
    false
);
code!(
    DPTRegionCodeAlpha2ASCII,
    "234.002",
    "region_code_alpha2",
    true
);

#[derive(Debug, Clone, Copy)]
pub struct CodeView<'a>(pub(super) &'a [u8], pub(super) bool);
impl<'a> CodeView<'a> {
    #[must_use]
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes violate the validation invariant established at construction.
    pub fn value(&self) -> String {
        decode_code(self.0, self.1).expect("validated code")
    }
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
