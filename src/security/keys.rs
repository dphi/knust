//! Key management for KNX Secure communication.
//!
//! This module provides types and utilities for managing security keys,
//! including keyring parsing and credential storage.

use std::collections::HashMap;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{Result, SecurityError};
use crate::protocol::address::{GroupAddress, IndividualAddress};

/// A security key with automatic zeroization on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecurityKey {
    /// The raw key bytes
    key: Vec<u8>,
}

impl SecurityKey {
    /// Create a new security key from raw bytes.
    #[must_use]
    pub fn new(key: Vec<u8>) -> Self {
        Self { key }
    }

    /// Create a security key from a hex string.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::InvalidCredentials`] if `hex` (after
    /// stripping spaces/colons) is not valid hex.
    pub fn from_hex(hex: &str) -> Result<Self> {
        let key = hex::decode(hex.replace([' ', ':'], "")).map_err(|e| {
            SecurityError::InvalidCredentials {
                details: format!("Invalid hex key: {e}"),
            }
        })?;
        Ok(Self::new(key))
    }

    /// Get the key bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.key
    }

    /// Get the key length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.key.len()
    }

    /// Check if the key is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.key.is_empty()
    }
}

impl std::fmt::Debug for SecurityKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't expose key contents in debug output
        write!(f, "SecurityKey([{} bytes])", self.key.len())
    }
}

/// Security credentials for a KNX/IP connection.
#[derive(Debug, Clone)]
pub struct SecurityCredentials {
    /// User ID (1-127)
    pub user_id: u8,
    /// User password (derived)
    pub user_password: SecurityKey,
    /// Device authentication code (derived, optional)
    pub device_auth: Option<SecurityKey>,
    /// Backbone key for routing (optional)
    pub backbone_key: Option<SecurityKey>,
}

impl SecurityCredentials {
    /// Create new security credentials.
    #[must_use]
    pub fn new(user_id: u8, user_password: SecurityKey) -> Self {
        Self {
            user_id,
            user_password,
            device_auth: None,
            backbone_key: None,
        }
    }

    /// Set the device authentication code.
    #[must_use]
    pub fn with_device_auth(mut self, device_auth: SecurityKey) -> Self {
        self.device_auth = Some(device_auth);
        self
    }

    /// Set the backbone key for routing.
    #[must_use]
    pub fn with_backbone_key(mut self, backbone_key: SecurityKey) -> Self {
        self.backbone_key = Some(backbone_key);
        self
    }
}

/// Keyring for storing and managing KNX security keys.
///
/// The keyring stores group keys for Data Secure communication
/// and individual address sequence numbers for replay protection.
#[derive(Debug, Default)]
pub struct KeyRing {
    /// Group address to key mapping for Data Secure
    group_keys: HashMap<GroupAddress, SecurityKey>,
    /// Individual address to last sequence number mapping
    sequence_numbers: HashMap<IndividualAddress, u64>,
    /// Tunnel credentials by host address
    tunnel_credentials: HashMap<String, SecurityCredentials>,
}

impl KeyRing {
    /// Create a new empty keyring.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a group key for Data Secure communication.
    pub fn add_group_key(&mut self, address: GroupAddress, key: SecurityKey) {
        self.group_keys.insert(address, key);
    }

    /// Get the key for a group address.
    #[must_use]
    pub fn get_group_key(&self, address: &GroupAddress) -> Option<&SecurityKey> {
        self.group_keys.get(address)
    }

    /// Check if a group address has a key (is secured).
    #[must_use]
    pub fn is_group_secured(&self, address: &GroupAddress) -> bool {
        self.group_keys.contains_key(address)
    }

    /// Get all secured group addresses.
    pub fn secured_groups(&self) -> impl Iterator<Item = &GroupAddress> {
        self.group_keys.keys()
    }

    /// Set the last valid sequence number for an individual address.
    pub fn set_sequence_number(&mut self, address: IndividualAddress, seq: u64) {
        self.sequence_numbers.insert(address, seq);
    }

    /// Get the last valid sequence number for an individual address.
    #[must_use]
    pub fn get_sequence_number(&self, address: &IndividualAddress) -> Option<u64> {
        self.sequence_numbers.get(address).copied()
    }

    /// Validate and update sequence number for replay protection.
    ///
    /// Returns Ok(()) if the sequence number is valid (greater than last seen),
    /// and updates the stored value.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::CryptographicError`] if `seq` is not greater
    /// than the last valid sequence number seen for `address`.
    pub fn validate_sequence(&mut self, address: IndividualAddress, seq: u64) -> Result<()> {
        if let Some(&last_seq) = self.sequence_numbers.get(&address)
            && seq <= last_seq
        {
            return Err(SecurityError::CryptographicError {
                operation: "sequence validation".to_string(),
                reason: format!("Sequence number {seq} not greater than last valid {last_seq}"),
            }
            .into());
        }
        self.sequence_numbers.insert(address, seq);
        Ok(())
    }

    /// Add tunnel credentials for a host.
    pub fn add_tunnel_credentials(&mut self, host: String, credentials: SecurityCredentials) {
        self.tunnel_credentials.insert(host, credentials);
    }

    /// Get tunnel credentials for a host.
    #[must_use]
    pub fn get_tunnel_credentials(&self, host: &str) -> Option<&SecurityCredentials> {
        self.tunnel_credentials.get(host)
    }

    /// Get the number of group keys stored.
    #[must_use]
    pub fn group_key_count(&self) -> usize {
        self.group_keys.len()
    }

    /// Get the number of tracked senders.
    #[must_use]
    pub fn sender_count(&self) -> usize {
        self.sequence_numbers.len()
    }
}
