//! Keyring file parsing for KNX security credentials.
//!
//! This module handles parsing of .knxkeys files which contain security
//! credentials for KNX Data Security and KNX IP Secure communication.

use std::fmt::Write as _;
use std::path::Path;
use tokio::fs;
use zeroize::Zeroizing;

use aes::{
    Aes128,
    cipher::{BlockCipherDecrypt, KeyInit},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use pbkdf2::pbkdf2_hmac;
use sha2::Sha256;

use crate::config::{escape_xml, unescape_xml};
use crate::error::{ConfigurationError, Result};
use crate::protocol::address::{GroupAddress, IndividualAddress};
use crate::security::{KeyRing, SecurityKey, sha256_hash};

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

impl KeyringConfig {
    pub fn into_data_secure_keyring(&self) -> KeyRing {
        self.build_data_secure_keyring(None)
    }

    /// Build the Data Secure keyring for a particular receiver interface.
    #[must_use]
    pub fn into_data_secure_keyring_for(&self, receiver: IndividualAddress) -> KeyRing {
        self.build_data_secure_keyring(Some(receiver))
    }

    fn build_data_secure_keyring(&self, receiver: Option<IndividualAddress>) -> KeyRing {
        let mut keyring = KeyRing::new();
        for group in &self.group_addresses {
            if receiver.is_none_or(|receiver| {
                self.interfaces.iter().any(|interface| {
                    interface.individual_address == receiver
                        && interface
                            .group_assignments
                            .iter()
                            .any(|assignment| assignment.address == group.address)
                })
            }) {
                keyring.add_group_key(group.address, group.key.clone());
            }
        }
        for interface in &self.interfaces {
            for assignment in &interface.group_assignments {
                for sender in &assignment.senders {
                    keyring.set_sequence_number(*sender, 0);
                }
            }
        }
        for device in &self.devices {
            keyring.set_sequence_number(device.individual_address, device.sequence_number);
        }
        keyring
    }
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
    /// Group address assignments and permitted senders
    pub group_assignments: Vec<KeyringGroupAssignment>,
}

/// Group address assignment for an interface.
#[derive(Debug, Clone)]
pub struct KeyringGroupAssignment {
    /// Assigned group address
    pub address: GroupAddress,
    /// Individual addresses permitted to send Data Secure group telegrams
    pub senders: Vec<IndividualAddress>,
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
    /// Last known Data Secure sequence number
    pub sequence_number: u64,
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

    /// Parse an ETS keyring file and decrypt its Data Secure group keys.
    ///
    /// # Errors
    ///
    /// Returns a parse error if the password, signature, or encrypted group keys
    /// are invalid.
    pub async fn parse_file_with_password<P: AsRef<Path>>(
        path: P,
        password: &str,
    ) -> Result<KeyringConfig> {
        let data = fs::read(path)
            .await
            .map_err(|e| ConfigurationError::ParseError {
                file: "keyring file".to_string(),
                reason: format!("Failed to read keyring file: {e}"),
            })?;
        Self::parse_bytes_with_password(&data, password)
    }

    /// Parse an ETS keyring and decrypt its Data Secure group keys.
    ///
    /// # Errors
    ///
    /// Returns a parse error if the password, signature, or encrypted group keys
    /// are invalid.
    pub fn parse_bytes_with_password(data: &[u8], password: &str) -> Result<KeyringConfig> {
        let xml = std::str::from_utf8(data).map_err(|e| ConfigurationError::ParseError {
            file: "keyring XML".to_string(),
            reason: format!("Invalid UTF-8 in keyring XML: {e}"),
        })?;
        let config = Self::parse_xml_keyring(data)?;
        let mut password_hash = Zeroizing::new([0u8; 16]);
        pbkdf2_hmac::<Sha256>(
            password.as_bytes(),
            b"1.keyring.ets.knx.org",
            65_536,
            &mut password_hash[..],
        );

        let signature = Self::extract_xml_attribute(xml, "Signature")
            .and_then(|signature| BASE64.decode(signature).ok())
            .ok_or_else(|| ConfigurationError::ParseError {
                file: "keyring XML".to_string(),
                reason: "Missing or invalid keyring signature".to_string(),
            })?;
        let signature_data = Zeroizing::new(Self::keyring_signature_data(xml, &password_hash)?);
        let digest = sha256_hash(&signature_data);
        if signature.as_slice() != &digest[..16] {
            return Err(ConfigurationError::ParseError {
                file: "keyring XML".to_string(),
                reason: "Keyring signature validation failed".to_string(),
            }
            .into());
        }

        let created =
            config
                .metadata
                .created
                .as_deref()
                .ok_or_else(|| ConfigurationError::ParseError {
                    file: "keyring XML".to_string(),
                    reason: "Missing keyring creation timestamp".to_string(),
                })?;
        let digest = sha256_hash(created.as_bytes());
        let iv = Zeroizing::new(<[u8; 16]>::try_from(&digest[..16]).expect("SHA-256 has 32 bytes"));
        let group_addresses = Self::parse_encrypted_group_addresses(xml, &password_hash, &iv)?;

        Ok(KeyringConfig {
            group_addresses,
            ..config
        })
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
            if Self::has_unparsed_group_keys(data, &config) {
                return Err(ConfigurationError::ParseError {
                    file: "keyring XML".to_string(),
                    reason:
                        "Password required for encrypted group keys; use parse_bytes_with_password"
                            .to_string(),
                }
                .into());
            }
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
                signature: Self::extract_xml_attribute(xml_str, "Signature")
                    .and_then(|signature| BASE64.decode(signature).ok()),
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

    fn parse_xml_interfaces(xml: &str) -> Vec<KeyringInterface> {
        let mut interfaces = Vec::new();
        let mut pos = 0;
        while let Some(offset) = xml[pos..].find("<Interface") {
            let start = pos + offset;
            let Ok(tag_end) = Self::find_xml_tag_end(xml, start) else {
                break;
            };
            let is_self_closing = xml[start..tag_end].trim_end().ends_with('/');
            let end = if is_self_closing {
                tag_end + 1
            } else if let Some(close_offset) = xml[tag_end + 1..].find("</Interface>") {
                tag_end + 1 + close_offset + "</Interface>".len()
            } else {
                break;
            };
            if let Ok(interface) = Self::parse_single_interface(&xml[start..end]) {
                interfaces.push(interface);
            }
            pos = end;
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

        let group_assignments = xml
            .split("<Group ")
            .skip(1)
            .filter_map(|group| group.split('>').next())
            .filter_map(|group| {
                let address = Self::extract_xml_attribute(group, "Address").and_then(|value| {
                    value.parse::<GroupAddress>().ok().or_else(|| {
                        value
                            .parse::<u16>()
                            .ok()
                            .and_then(|raw| GroupAddress::try_from_raw(raw).ok())
                    })
                })?;
                let senders = Self::extract_xml_attribute(group, "Senders")
                    .unwrap_or_default()
                    .split_whitespace()
                    .filter_map(|sender| sender.parse().ok())
                    .collect();
                Some(KeyringGroupAssignment { address, senders })
            })
            .collect();

        Ok(KeyringInterface {
            individual_address,
            interface_type,
            host,
            user_id,
            user_password,
            device_authentication,
            backbone_key,
            group_assignments,
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

        let sequence_number = Self::extract_xml_attribute(xml, "SequenceNumber")
            .and_then(|sequence| sequence.parse().ok())
            .unwrap_or(0);

        Ok(KeyringDevice {
            individual_address,
            serial_number,
            tool_key,
            management_password,
            authentication_code,
            sequence_number,
        })
    }

    fn keyring_signature_data(xml: &str, password_hash: &[u8; 16]) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        let mut pos = 0;
        while let Some(offset) = xml[pos..].find('<') {
            let start = pos + offset;
            if xml[start..].starts_with("<!--") {
                pos = start
                    + xml[start..]
                        .find("-->")
                        .ok_or_else(|| Self::xml_error("Unclosed XML comment"))?
                    + 3;
                continue;
            }
            if xml[start..].starts_with("<?") {
                pos = start
                    + xml[start..]
                        .find("?>")
                        .ok_or_else(|| Self::xml_error("Unclosed XML declaration"))?
                    + 2;
                continue;
            }
            if xml[start..].starts_with("<!") {
                return Err(Self::xml_error("Unsupported XML declaration"));
            }

            let end = Self::find_xml_tag_end(xml, start)?;
            let raw_tag = xml[start + 1..end].trim();
            let closing = raw_tag.starts_with('/');
            let self_closing = raw_tag.ends_with('/');
            let tag = raw_tag.trim_start_matches('/').trim_end_matches('/').trim();
            let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
            let name = &tag[..name_end];
            if name.is_empty() {
                return Err(Self::xml_error("Empty XML tag"));
            }

            if closing {
                output.push(2);
            } else {
                output.push(1);
                Self::append_signature_string(&mut output, name)?;
                let mut attributes = Self::parse_xml_attributes(&tag[name_end..])?;
                attributes.retain(|(name, _)| name != "xmlns" && name != "Signature");
                attributes.sort_by(|a, b| a.0.cmp(&b.0));
                for (name, value) in attributes {
                    Self::append_signature_string(&mut output, &name)?;
                    Self::append_signature_string(&mut output, &value)?;
                }
                if self_closing {
                    output.push(2);
                }
            }
            pos = end + 1;
        }
        Self::append_signature_bytes(&mut output, BASE64.encode(password_hash).as_bytes())?;
        Ok(output)
    }

    fn parse_xml_attributes(mut text: &str) -> Result<Vec<(String, String)>> {
        let mut attributes = Vec::new();
        loop {
            text = text.trim_start();
            if text.is_empty() || text == "/" {
                return Ok(attributes);
            }
            let name_end = text
                .find(|c: char| c.is_whitespace() || c == '=' || c == '/')
                .unwrap_or(text.len());
            if name_end == 0 {
                return Err(Self::xml_error("Invalid XML attribute"));
            }
            let name = &text[..name_end];
            text = text[name_end..].trim_start();
            let Some(value_text) = text.strip_prefix('=') else {
                return Err(Self::xml_error("XML attribute has no value"));
            };
            text = value_text.trim_start();
            let quote = text
                .chars()
                .next()
                .filter(|quote| *quote == '"' || *quote == '\'')
                .ok_or_else(|| Self::xml_error("XML attribute value is not quoted"))?;
            text = &text[quote.len_utf8()..];
            let value_end = text
                .find(quote)
                .ok_or_else(|| Self::xml_error("Unclosed XML attribute"))?;
            let value = Self::unescape_xml_attribute(&text[..value_end])?;
            attributes.push((name.to_string(), value));
            text = &text[value_end + quote.len_utf8()..];
        }
    }

    fn unescape_xml_attribute(value: &str) -> Result<String> {
        let mut decoded = String::with_capacity(value.len());
        let mut remaining = value;
        while let Some(offset) = remaining.find('&') {
            decoded.push_str(&remaining[..offset]);
            let entity = &remaining[offset + 1..];
            let end = entity
                .find(';')
                .ok_or_else(|| Self::xml_error("Unclosed XML entity reference"))?;
            let entity_value = &entity[..end];
            let character = match entity_value {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                value if value.starts_with("#x") || value.starts_with("#X") => {
                    u32::from_str_radix(&value[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| Self::xml_error("Invalid hexadecimal XML character"))?
                }
                value if value.starts_with('#') => value[1..]
                    .parse::<u32>()
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| Self::xml_error("Invalid decimal XML character"))?,
                _ => return Err(Self::xml_error("Unknown XML entity reference")),
            };
            decoded.push(character);
            remaining = &entity[end + 1..];
        }
        decoded.push_str(remaining);
        Ok(decoded)
    }

    fn append_signature_string(output: &mut Vec<u8>, value: &str) -> Result<()> {
        Self::append_signature_bytes(output, value.as_bytes())
    }

    fn append_signature_bytes(output: &mut Vec<u8>, value: &[u8]) -> Result<()> {
        let length = u8::try_from(value.len())
            .map_err(|_| Self::xml_error("Signed XML value exceeds 255 bytes"))?;
        output.push(length);
        output.extend_from_slice(value);
        Ok(())
    }

    fn find_xml_tag_end(xml: &str, start: usize) -> Result<usize> {
        let mut quote = None;
        for (offset, character) in xml[start + 1..].char_indices() {
            if Some(character) == quote {
                quote = None;
            } else if quote.is_none() && (character == '"' || character == '\'') {
                quote = Some(character);
            } else if quote.is_none() && character == '>' {
                return Ok(start + 1 + offset);
            }
        }
        Err(Self::xml_error("Unclosed XML tag"))
    }

    fn parse_encrypted_group_addresses(
        xml: &str,
        password_hash: &[u8; 16],
        iv: &[u8; 16],
    ) -> Result<Vec<KeyringGroupAddress>> {
        let section_start = xml
            .find("<GroupAddresses")
            .ok_or_else(|| Self::xml_error("Missing GroupAddresses section"))?;
        let content_start = Self::find_xml_tag_end(xml, section_start)? + 1;
        let section_end = xml[content_start..]
            .find("</GroupAddresses>")
            .map(|offset| content_start + offset)
            .ok_or_else(|| Self::xml_error("Unclosed GroupAddresses section"))?;
        let section = &xml[content_start..section_end];
        let mut groups = Vec::new();
        let mut pos = 0;
        while let Some(offset) = section[pos..].find("<Group ") {
            let start = pos + offset;
            let end = Self::find_xml_tag_end(section, start)?;
            let tag = &section[start..=end];
            let address = Self::extract_xml_attribute(tag, "Address")
                .and_then(|value| {
                    value.parse::<GroupAddress>().ok().or_else(|| {
                        value
                            .parse::<u16>()
                            .ok()
                            .and_then(|raw| GroupAddress::try_from_raw(raw).ok())
                    })
                })
                .ok_or_else(|| Self::xml_error("Missing or invalid group address"))?;
            let encrypted_key = Self::extract_xml_attribute(tag, "Key")
                .ok_or_else(|| Self::xml_error("Missing group key"))?;
            let mut key = BASE64
                .decode(encrypted_key)
                .map_err(|_| Self::xml_error("Invalid base64 group key"))?;
            if key.len() != 16 {
                return Err(Self::xml_error("Encrypted group key must be 16 bytes"));
            }
            let cipher =
                Aes128::new_from_slice(password_hash).expect("PBKDF2 derives an AES-128 key");
            let ciphertext: [u8; 16] = key.as_slice().try_into().expect("length checked");
            let mut block = aes::cipher::Block::<Aes128>::try_from(&ciphertext[..])
                .expect("AES-128 block has 16 bytes");
            cipher.decrypt_block(&mut block);
            for (byte, iv_byte) in block.iter_mut().zip(iv) {
                *byte ^= iv_byte;
            }
            key.copy_from_slice(&block);
            let description = Self::extract_xml_attribute(tag, "Description");
            groups.push(KeyringGroupAddress {
                address,
                key: SecurityKey::new(key),
                description,
            });
            pos = end + 1;
        }
        Ok(groups)
    }

    fn xml_error(reason: &str) -> crate::error::KnxError {
        ConfigurationError::ParseError {
            file: "keyring XML".to_string(),
            reason: reason.to_string(),
        }
        .into()
    }

    fn has_unparsed_group_keys(data: &[u8], config: &KeyringConfig) -> bool {
        let Ok(xml) = std::str::from_utf8(data) else {
            return false;
        };
        let Some(section_start) = xml.find("<GroupAddresses") else {
            return false;
        };
        let Some(section_end) = xml[section_start..].find("</GroupAddresses>") else {
            return false;
        };
        let section = &xml[section_start..section_start + section_end];
        section.matches("<Group ").count() != config.group_addresses.len()
    }

    fn parse_xml_group_addresses(xml: &str) -> Vec<KeyringGroupAddress> {
        let mut group_addresses = Vec::new();

        if let Some(section_start) = xml.find("<GroupAddresses")
            && let Ok(section_tag_end) = Self::find_xml_tag_end(xml, section_start)
            && let Some(section_close) = xml[section_tag_end + 1..].find("</GroupAddresses>")
        {
            let section = &xml[section_tag_end + 1..section_tag_end + 1 + section_close];
            let mut pos = 0;
            while let Some(offset) = section[pos..].find("<Group ") {
                let start = pos + offset;
                let Ok(end) = Self::find_xml_tag_end(section, start) else {
                    break;
                };
                if let Ok(group) = Self::parse_single_group_address(&section[start..=end]) {
                    group_addresses.push(group);
                }
                pos = end + 1;
            }
            return group_addresses;
        }

        let mut pos = 0;
        while let Some(offset) = xml[pos..].find("<GroupAddress ") {
            let start = pos + offset;
            let Ok(end) = Self::find_xml_tag_end(xml, start) else {
                break;
            };
            if let Ok(group) = Self::parse_single_group_address(&xml[start..=end]) {
                group_addresses.push(group);
            }
            pos = end + 1;
        }

        group_addresses
    }

    /// Parse a single group address from XML.
    fn parse_single_group_address(xml: &str) -> Result<KeyringGroupAddress> {
        let address = Self::extract_xml_attribute(xml, "Address")
            .and_then(|value| {
                value.parse::<GroupAddress>().ok().or_else(|| {
                    value
                        .parse::<u16>()
                        .ok()
                        .and_then(|raw| GroupAddress::try_from_raw(raw).ok())
                })
            })
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

            if interface.group_assignments.is_empty() {
                xml.push_str(" />\n");
            } else {
                xml.push_str(">\n");
                for assignment in &interface.group_assignments {
                    let senders = assignment
                        .senders
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" ");
                    let _ = writeln!(
                        xml,
                        "    <Group Address=\"{}\" Senders=\"{}\" />",
                        assignment.address, senders
                    );
                }
                xml.push_str("  </Interface>\n");
            }
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

            let _ = write!(xml, " SequenceNumber=\"{}\"", device.sequence_number);
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
