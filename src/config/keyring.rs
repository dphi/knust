//! Keyring file parsing for KNX security credentials.
//!
//! This module handles parsing of .knxkeys files which contain security
//! credentials for KNX Data Security and KNX IP Secure communication.

use std::fmt::Write as _;
use std::path::Path;
use tokio::fs;

use crate::config::{escape_xml, unescape_xml};
use crate::error::{ConfigurationError, Result};
use crate::protocol::address::{GroupAddress, IndividualAddress};
use crate::security::SecurityKey;

/// Parsed keyring configuration data.
#[derive(Debug, Clone)]
pub struct KeyringConfig {
    /// Keyring metadata
    pub metadata: KeyringMetadata,
    /// Interfaces (gateways) with their credentials
    pub interfaces: Vec<KeyringInterface>,
    /// Devices with their individual addresses and keys
    pub devices: Vec<KeyringDevice>,
    /// Group addresses with their security keys
    pub group_addresses: Vec<KeyringGroupAddress>,
}

/// Keyring metadata information.
#[derive(Debug, Clone)]
pub struct KeyringMetadata {
    /// Keyring creation timestamp
    pub created: Option<String>,
    /// Keyring creator/tool
    pub creator: Option<String>,
    /// Project name
    pub project: Option<String>,
    /// Signature for integrity verification
    pub signature: Option<Vec<u8>>,
}

/// KNX/IP interface (gateway) configuration.
#[derive(Debug, Clone)]
pub struct KeyringInterface {
    /// Interface individual address
    pub individual_address: IndividualAddress,
    /// Interface type (e.g., "Tunneling", "Routing")
    pub interface_type: String,
    /// Host identifier (IP address or hostname)
    pub host: String,
    /// User ID for authentication
    pub user_id: u8,
    /// User password (derived from management password)
    pub user_password: SecurityKey,
    /// Device authentication code
    pub device_authentication: Option<SecurityKey>,
    /// Backbone key for routing
    pub backbone_key: Option<SecurityKey>,
}

/// KNX device with security information.
#[derive(Debug, Clone)]
pub struct KeyringDevice {
    /// Device individual address
    pub individual_address: IndividualAddress,
    /// Device serial number
    pub serial_number: Option<String>,
    /// Tool key for device management
    pub tool_key: Option<SecurityKey>,
    /// Management password
    pub management_password: Option<SecurityKey>,
    /// Authentication code
    pub authentication_code: Option<SecurityKey>,
}

/// Group address with security key.
#[derive(Debug, Clone)]
pub struct KeyringGroupAddress {
    /// Group address
    pub address: GroupAddress,
    /// Security key for Data Secure communication
    pub key: SecurityKey,
    /// Key description/name
    pub description: Option<String>,
}

/// Parser for KNX keyring files.
pub struct KeyringParser;

impl KeyringParser {
    /// Parse a keyring file from disk.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError::ParseError`] if `path` can't be read, or
    /// the same errors as [`Self::parse_bytes`] for its contents.
    pub async fn parse_file<P: AsRef<Path>>(path: P) -> Result<KeyringConfig> {
        let data = fs::read(path)
            .await
            .map_err(|e| ConfigurationError::ParseError {
                file: "keyring file".to_string(),
                reason: format!("Failed to read keyring file: {e}"),
            })?;

        Self::parse_bytes(&data)
    }

    /// Parse keyring data from bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError::ParseError`] if `data` matches neither
    /// the XML nor the binary `.knxkeys` format (binary parsing is not yet
    /// implemented, so it always falls through to this error).
    pub fn parse_bytes(data: &[u8]) -> Result<KeyringConfig> {
        // Try to parse as XML first (most common format)
        if let Ok(config) = Self::parse_xml_keyring(data) {
            return Ok(config);
        }

        // Try to parse as binary format
        if let Ok(config) = Self::parse_binary_keyring(data) {
            return Ok(config);
        }

        Err(ConfigurationError::ParseError {
            file: "keyring data".to_string(),
            reason: "Unable to parse keyring data as XML or binary format".to_string(),
        }
        .into())
    }

    /// Parse XML keyring format.
    fn parse_xml_keyring(data: &[u8]) -> Result<KeyringConfig> {
        let xml_str = std::str::from_utf8(data).map_err(|e| ConfigurationError::ParseError {
            file: "keyring XML".to_string(),
            reason: format!("Invalid UTF-8 in keyring XML: {e}"),
        })?;

        let mut config = KeyringConfig {
            metadata: KeyringMetadata {
                created: None,
                creator: None,
                project: None,
                signature: None,
            },
            interfaces: Vec::new(),
            devices: Vec::new(),
            group_addresses: Vec::new(),
        };

        // Extract basic metadata from Keyring element
        if let Some(keyring_start) = xml_str.find("<Keyring")
            && let Some(keyring_end) = xml_str[keyring_start..].find('>')
        {
            let keyring_tag = &xml_str[keyring_start..=(keyring_start + keyring_end)];

            if let Some(created) = Self::extract_xml_attribute(keyring_tag, "Created") {
                config.metadata.created = Some(created);
            }

            if let Some(creator) = Self::extract_xml_attribute(keyring_tag, "CreatedBy") {
                config.metadata.creator = Some(creator);
            }

            if let Some(project) = Self::extract_xml_attribute(keyring_tag, "Project") {
                config.metadata.project = Some(project);
            }
        }

        // Parse interfaces
        config.interfaces = Self::parse_xml_interfaces(xml_str);

        // Parse devices
        config.devices = Self::parse_xml_devices(xml_str);

        // Parse group addresses
        config.group_addresses = Self::parse_xml_group_addresses(xml_str);

        Ok(config)
    }

    /// Parse binary keyring format.
    fn parse_binary_keyring(data: &[u8]) -> Result<KeyringConfig> {
        // Binary keyring parsing would be implemented here
        // This is a simplified placeholder implementation

        if data.len() < 16 {
            return Err(ConfigurationError::ParseError {
                file: "binary keyring".to_string(),
                reason: "Binary keyring too short".to_string(),
            }
            .into());
        }

        // Check for binary keyring magic bytes (example)
        if &data[0..4] != b"KNXK" {
            return Err(ConfigurationError::ParseError {
                file: "binary keyring".to_string(),
                reason: "Invalid binary keyring magic bytes".to_string(),
            }
            .into());
        }

        Err(ConfigurationError::ParseError {
            file: "binary keyring".to_string(),
            reason: "binary .knxkeys parsing is not implemented; export the keyring as XML"
                .to_string(),
        }
        .into())
    }

    /// Extract XML attribute value (simplified).
    fn extract_xml_attribute(xml: &str, attr_name: &str) -> Option<String> {
        let pattern = format!("{attr_name}=\"");
        if let Some(start) = xml.find(&pattern) {
            let start = start + pattern.len();
            if let Some(end) = xml[start..].find('"') {
                return Some(unescape_xml(&xml[start..start + end]).into_owned());
            }
        }
        None
    }

    /// Parse interfaces from XML (simplified).
    fn parse_xml_interfaces(xml: &str) -> Vec<KeyringInterface> {
        let mut interfaces = Vec::new();

        let mut pos = 0;
        while let Some(start) = xml[pos..].find("<Interface") {
            let start = pos + start;
            // Look for self-closing tag first
            if let Some(end) = xml[start..].find(" />") {
                let end = start + end + 3;
                let interface_xml = &xml[start..end];

                if let Ok(interface) = Self::parse_single_interface(interface_xml) {
                    interfaces.push(interface);
                }

                pos = end;
            } else if let Some(end) = xml[start..].find("</Interface>") {
                let end = start + end + "</Interface>".len();
                let interface_xml = &xml[start..end];

                if let Ok(interface) = Self::parse_single_interface(interface_xml) {
                    interfaces.push(interface);
                }

                pos = end;
            } else {
                break;
            }
        }

        interfaces
    }

    /// Parse a single interface from XML.
    fn parse_single_interface(xml: &str) -> Result<KeyringInterface> {
        let individual_address = Self::extract_xml_attribute(xml, "IndividualAddress")
            .and_then(|addr| addr.parse().ok())
            .ok_or_else(|| ConfigurationError::ParseError {
                file: "interface XML".to_string(),
                reason: "Missing or invalid IndividualAddress in interface".to_string(),
            })?;

        let interface_type =
            Self::extract_xml_attribute(xml, "Type").unwrap_or_else(|| "Unknown".to_string());

        let host =
            Self::extract_xml_attribute(xml, "Host").unwrap_or_else(|| "0.0.0.0".to_string());

        let user_id = Self::extract_xml_attribute(xml, "UserID")
            .and_then(|id| id.parse().ok())
            .unwrap_or(1);

        let user_password = Self::extract_xml_attribute(xml, "UserPassword")
            .and_then(|pwd| SecurityKey::from_hex(&pwd).ok())
            .unwrap_or_else(|| SecurityKey::new(vec![0; 16]));

        let device_authentication = Self::extract_xml_attribute(xml, "DeviceAuthentication")
            .and_then(|auth| SecurityKey::from_hex(&auth).ok());

        let backbone_key = Self::extract_xml_attribute(xml, "BackboneKey")
            .and_then(|key| SecurityKey::from_hex(&key).ok());

        Ok(KeyringInterface {
            individual_address,
            interface_type,
            host,
            user_id,
            user_password,
            device_authentication,
            backbone_key,
        })
    }

    /// Parse devices from XML (simplified).
    fn parse_xml_devices(xml: &str) -> Vec<KeyringDevice> {
        let mut devices = Vec::new();

        let mut pos = 0;
        while let Some(start) = xml[pos..].find("<Device") {
            let start = pos + start;
            // Look for self-closing tag first
            if let Some(end) = xml[start..].find(" />") {
                let end = start + end + 3;
                let device_xml = &xml[start..end];

                if let Ok(device) = Self::parse_single_device(device_xml) {
                    devices.push(device);
                }

                pos = end;
            } else if let Some(end) = xml[start..].find("</Device>") {
                let end = start + end + "</Device>".len();
                let device_xml = &xml[start..end];

                if let Ok(device) = Self::parse_single_device(device_xml) {
                    devices.push(device);
                }

                pos = end;
            } else {
                break;
            }
        }

        devices
    }

    /// Parse a single device from XML.
    fn parse_single_device(xml: &str) -> Result<KeyringDevice> {
        let individual_address = Self::extract_xml_attribute(xml, "IndividualAddress")
            .and_then(|addr| addr.parse().ok())
            .ok_or_else(|| ConfigurationError::ParseError {
                file: "device XML".to_string(),
                reason: "Missing or invalid IndividualAddress in device".to_string(),
            })?;

        let serial_number = Self::extract_xml_attribute(xml, "SerialNumber");

        let tool_key = Self::extract_xml_attribute(xml, "ToolKey")
            .and_then(|key| SecurityKey::from_hex(&key).ok());

        let management_password = Self::extract_xml_attribute(xml, "ManagementPassword")
            .and_then(|pwd| SecurityKey::from_hex(&pwd).ok());

        let authentication_code = Self::extract_xml_attribute(xml, "AuthenticationCode")
            .and_then(|auth| SecurityKey::from_hex(&auth).ok());

        Ok(KeyringDevice {
            individual_address,
            serial_number,
            tool_key,
            management_password,
            authentication_code,
        })
    }

    /// Parse group addresses from XML (simplified).
    fn parse_xml_group_addresses(xml: &str) -> Vec<KeyringGroupAddress> {
        let mut group_addresses = Vec::new();

        let mut pos = 0;
        while let Some(start) = xml[pos..].find("<GroupAddress") {
            let start = pos + start;
            // Look for self-closing tag first
            if let Some(end) = xml[start..].find(" />") {
                let end = start + end + 3;
                let ga_xml = &xml[start..end];

                if let Ok(ga) = Self::parse_single_group_address(ga_xml) {
                    group_addresses.push(ga);
                }

                pos = end;
            } else if let Some(end) = xml[start..].find("</GroupAddress>") {
                let end = start + end + "</GroupAddress>".len();
                let ga_xml = &xml[start..end];

                if let Ok(ga) = Self::parse_single_group_address(ga_xml) {
                    group_addresses.push(ga);
                }

                pos = end;
            } else {
                break;
            }
        }

        group_addresses
    }

    /// Parse a single group address from XML.
    fn parse_single_group_address(xml: &str) -> Result<KeyringGroupAddress> {
        let address = Self::extract_xml_attribute(xml, "Address")
            .and_then(|addr| addr.parse().ok())
            .ok_or_else(|| ConfigurationError::ParseError {
                file: "group address XML".to_string(),
                reason: "Missing or invalid Address in group address".to_string(),
            })?;

        let key = Self::extract_xml_attribute(xml, "Key")
            .and_then(|key| SecurityKey::from_hex(&key).ok())
            .ok_or_else(|| ConfigurationError::ParseError {
                file: "group address XML".to_string(),
                reason: "Missing or invalid Key in group address".to_string(),
            })?;

        let description = Self::extract_xml_attribute(xml, "Description");

        Ok(KeyringGroupAddress {
            address,
            key,
            description,
        })
    }

    /// Serialize keyring configuration to bytes.
    #[must_use]
    pub fn serialize(config: &KeyringConfig) -> Vec<u8> {
        // Generate XML representation
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
        xml.push_str("<Keyring");

        if let Some(ref created) = config.metadata.created {
            let _ = write!(xml, " Created=\"{}\"", escape_xml(created));
        }

        if let Some(ref creator) = config.metadata.creator {
            let _ = write!(xml, " CreatedBy=\"{}\"", escape_xml(creator));
        }

        if let Some(ref project) = config.metadata.project {
            let _ = write!(xml, " Project=\"{}\"", escape_xml(project));
        }

        xml.push_str(">\n");

        // Serialize interfaces
        for interface in &config.interfaces {
            xml.push_str("  <Interface");
            let _ = write!(
                xml,
                " IndividualAddress=\"{}\"",
                interface.individual_address
            );
            let _ = write!(xml, " Type=\"{}\"", interface.interface_type);
            let _ = write!(xml, " Host=\"{}\"", escape_xml(&interface.host));
            let _ = write!(xml, " UserID=\"{}\"", interface.user_id);
            let _ = write!(
                xml,
                " UserPassword=\"{}\"",
                hex::encode(interface.user_password.as_bytes())
            );

            if let Some(ref device_auth) = interface.device_authentication {
                let _ = write!(
                    xml,
                    " DeviceAuthentication=\"{}\"",
                    hex::encode(device_auth.as_bytes())
                );
            }

            if let Some(ref backbone_key) = interface.backbone_key {
                let _ = write!(
                    xml,
                    " BackboneKey=\"{}\"",
                    hex::encode(backbone_key.as_bytes())
                );
            }

            xml.push_str(" />\n");
        }

        // Serialize devices
        for device in &config.devices {
            xml.push_str("  <Device");
            let _ = write!(xml, " IndividualAddress=\"{}\"", device.individual_address);

            if let Some(ref serial) = device.serial_number {
                let _ = write!(xml, " SerialNumber=\"{}\"", escape_xml(serial));
            }

            if let Some(ref tool_key) = device.tool_key {
                let _ = write!(xml, " ToolKey=\"{}\"", hex::encode(tool_key.as_bytes()));
            }

            if let Some(ref mgmt_pwd) = device.management_password {
                let _ = write!(
                    xml,
                    " ManagementPassword=\"{}\"",
                    hex::encode(mgmt_pwd.as_bytes())
                );
            }

            if let Some(ref auth_code) = device.authentication_code {
                let _ = write!(
                    xml,
                    " AuthenticationCode=\"{}\"",
                    hex::encode(auth_code.as_bytes())
                );
            }

            xml.push_str(" />\n");
        }

        // Serialize group addresses
        for ga in &config.group_addresses {
            xml.push_str("  <GroupAddress");
            let _ = write!(xml, " Address=\"{}\"", ga.address);
            let _ = write!(xml, " Key=\"{}\"", hex::encode(ga.key.as_bytes()));

            if let Some(ref desc) = ga.description {
                let _ = write!(xml, " Description=\"{}\"", escape_xml(desc));
            }

            xml.push_str(" />\n");
        }

        xml.push_str("</Keyring>\n");

        xml.into_bytes()
    }
}
