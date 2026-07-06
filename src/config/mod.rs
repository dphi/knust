//! Configuration parsing for KNX/IP systems.
//!
//! This module provides parsers for various KNX configuration formats including:
//! - Keyring files (.knxkeys) for security credentials
//! - ETS CSV group-address exports
//! - XML and binary configuration formats

#[cfg(feature = "ets")]
pub mod ets_csv;
#[cfg(feature = "secure")]
pub mod keyring;
#[cfg(feature = "secure")]
pub mod validation;

#[cfg(test)]
mod tests;

#[cfg(feature = "ets")]
pub use ets_csv::{GroupAddressConfig, parse_ets_csv};
#[cfg(feature = "secure")]
pub use keyring::{KeyringConfig, KeyringDevice, KeyringInterface, KeyringParser};
#[cfg(feature = "secure")]
pub use validation::{ConfigValidator, ValidationError, ValidationResult};

#[cfg(feature = "secure")]
use crate::error::{ConfigurationError, Result};
#[cfg(feature = "secure")]
use std::path::Path;

/// Escape `& < > " '` for safe interpolation into an XML attribute value.
#[cfg(feature = "secure")]
pub(crate) fn escape_xml(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains(['&', '<', '>', '"', '\'']) {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                _ => out.push(c),
            }
        }
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// Unescape the five predefined XML entities produced by [`escape_xml`].
#[cfg(feature = "secure")]
pub(crate) fn unescape_xml(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('&') {
        return std::borrow::Cow::Borrowed(s);
    }
    let out = s
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    std::borrow::Cow::Owned(out)
}

/// Configuration data that can be parsed from various sources.
#[derive(Debug, Clone)]
pub struct Configuration {
    /// Security keyring data
    #[cfg(feature = "secure")]
    pub keyring: Option<KeyringConfig>,
    /// Additional metadata
    pub metadata: ConfigMetadata,
}

/// Metadata about the configuration source.
#[derive(Debug, Clone, Default)]
pub struct ConfigMetadata {
    /// Source file path
    pub source_path: Option<String>,
    /// Configuration format
    pub format: ConfigFormat,
    /// Parse timestamp
    pub parsed_at: Option<std::time::SystemTime>,
}

/// Supported configuration formats.
#[derive(Debug, Clone, Copy, Default)]
pub enum ConfigFormat {
    #[default]
    Unknown,
    /// KNX keyring file (.knxkeys)
    Keyring,
}

impl Configuration {
    /// Create a new empty configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "secure")]
            keyring: None,
            metadata: ConfigMetadata::default(),
        }
    }

    /// Parse configuration from a file path.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError::ParseError`] if the file extension is
    /// unrecognized (`.knxkeys`), or the same errors as [`KeyringParser`]
    /// for its contents.
    #[cfg(feature = "secure")]
    pub async fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

        let mut config = Self::new();
        config.metadata.source_path = Some(path.to_string_lossy().to_string());
        config.metadata.parsed_at = Some(std::time::SystemTime::now());

        match extension.to_lowercase().as_str() {
            "knxkeys" => {
                config.metadata.format = ConfigFormat::Keyring;
                let keyring = KeyringParser::parse_file(path).await?;
                config.keyring = Some(keyring);
            }
            _ => {
                return Err(ConfigurationError::ParseError {
                    file: path.to_string_lossy().to_string(),
                    reason: format!("Unsupported configuration file format: {extension}"),
                }
                .into());
            }
        }

        Ok(config)
    }

    /// Parse configuration from raw bytes with format hint.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError::ValidationError`] if `format` is
    /// [`ConfigFormat::Unknown`], or the same errors as [`KeyringParser`]
    /// for `data`.
    #[cfg(feature = "secure")]
    pub fn from_bytes(data: &[u8], format: ConfigFormat) -> Result<Self> {
        let mut config = Self::new();
        config.metadata.format = format;
        config.metadata.parsed_at = Some(std::time::SystemTime::now());

        match format {
            ConfigFormat::Keyring => {
                let keyring = KeyringParser::parse_bytes(data)?;
                config.keyring = Some(keyring);
            }
            ConfigFormat::Unknown => {
                return Err(ConfigurationError::ValidationError {
                    details: "Cannot parse configuration with unknown format".to_string(),
                }
                .into());
            }
        }

        Ok(config)
    }

    /// Validate the configuration.
    #[cfg(feature = "secure")]
    #[must_use]
    pub fn validate(&self) -> ValidationResult {
        let validator = ConfigValidator::new();
        validator.validate(self)
    }

    /// Serialize the configuration to bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError::ValidationError`] if `format` requires
    /// data this configuration doesn't have (no keyring loaded).
    #[cfg(feature = "secure")]
    pub fn to_bytes(&self, format: ConfigFormat) -> Result<Vec<u8>> {
        match format {
            ConfigFormat::Keyring => {
                if let Some(ref keyring) = self.keyring {
                    Ok(KeyringParser::serialize(keyring))
                } else {
                    Err(ConfigurationError::ValidationError {
                        details: "No keyring data to serialize".to_string(),
                    }
                    .into())
                }
            }
            ConfigFormat::Unknown => Err(ConfigurationError::ValidationError {
                details: "Cannot serialize configuration with unknown format".to_string(),
            }
            .into()),
        }
    }
}

impl Default for Configuration {
    fn default() -> Self {
        Self::new()
    }
}
