//! DPT 217.001 - KNX version information (U5U5U6).

use super::{DptValue, Result, WriteValue};
use crate::error::ProtocolError;

/// KNX version components, displayed as magic.version.revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KnxVersion {
    /// Incompatible-change counter, 0..=31.
    pub magic_number: u8,
    /// Feature counter, 0..=31.
    pub version_number: u8,
    /// Minor-change counter, 0..=63.
    pub revision_number: u8,
}

impl KnxVersion {
    /// The invalid, void or undefined version-information sentinel.
    #[must_use]
    pub const fn invalid() -> Self {
        Self {
            magic_number: 31,
            version_number: 31,
            revision_number: 63,
        }
    }

    #[must_use]
    pub fn is_invalid(&self) -> bool {
        *self == Self::invalid()
    }

    pub(super) fn from_raw(bytes: [u8; 2]) -> Self {
        let raw = u16::from_be_bytes(bytes);
        Self {
            magic_number: (raw >> 11) as u8,
            version_number: ((raw >> 6) & 0x1f) as u8,
            revision_number: (raw & 0x3f) as u8,
        }
    }

    pub(super) fn encode(self) -> Result<[u8; 2]> {
        if self.magic_number > 31 || self.version_number > 31 || self.revision_number > 63 {
            return Err(ProtocolError::DptError {
                dpt_type: "217.001".to_string(),
                details: format!(
                    "Version {self} out of range (magic/version 0..=31, revision 0..=63)"
                ),
            }
            .into());
        }
        Ok(((u16::from(self.magic_number) << 11)
            | (u16::from(self.version_number) << 6)
            | u16::from(self.revision_number))
        .to_be_bytes())
    }
}

impl std::fmt::Display for KnxVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}.{}.{}",
            self.magic_number, self.version_number, self.revision_number
        )
    }
}

impl std::str::FromStr for KnxVersion {
    type Err = crate::error::KnxError;

    fn from_str(value: &str) -> Result<Self> {
        let mut parts = value.split('.');
        let mut next = || parts.next().and_then(|part| part.parse::<u8>().ok());
        let components = (next(), next(), next());
        if let (Some(magic_number), Some(version_number), Some(revision_number)) = components
            && parts.next().is_none()
        {
            let version = Self {
                magic_number,
                version_number,
                revision_number,
            };
            version.encode()?;
            return Ok(version);
        }
        Err(ProtocolError::DptError {
            dpt_type: "217.001".to_string(),
            details: format!("Invalid version {value:?}: expected magic.version.revision"),
        }
        .into())
    }
}

/// DPT 217.001, stored as its two wire octets.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version([u8; 2]);

impl Version {
    /// # Errors
    ///
    /// Returns an error for components outside their 5/5/6-bit ranges.
    pub fn new(value: KnxVersion) -> Result<Self> {
        Ok(Self(value.encode()?))
    }

    #[must_use]
    pub fn value(&self) -> KnxVersion {
        KnxVersion::from_raw(self.0)
    }
}

impl DptValue for Version {
    const DPT_NUMBER: &'static str = "217.001";
    const VALUE_TYPE: &'static str = "version";
    const BYTE_LENGTH: usize = 2;

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let bytes = bytes.try_into().map_err(|_| ProtocolError::DptError {
            dpt_type: Self::DPT_NUMBER.to_string(),
            details: format!("Invalid length: expected 2, got {}", bytes.len()),
        })?;
        Ok(Self(bytes))
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

impl WriteValue<Version> for KnxVersion {
    fn resolve(self, _dpt: super::DptType) -> Result<Version> {
        Version::new(self)
    }
}

/// XKNX-compatible name for DPT 217.001.
pub type DPTVersion = Version;

/// Borrowed DPT 217.001 version payload.
#[derive(Debug, Clone, Copy)]
pub struct VersionView<'a>(pub(super) &'a [u8]);

impl<'a> VersionView<'a> {
    #[must_use]
    pub fn value(&self) -> KnxVersion {
        KnxVersion::from_raw([self.0[0], self.0[1]])
    }

    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.0
    }
}
