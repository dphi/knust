//! Configuration validation for KNX configuration data.
//!
//! This module provides validation logic for parsed configuration data
//! to ensure consistency and correctness.

use std::collections::HashSet;

use crate::config::{Configuration, KeyringConfig};

/// Result of configuration validation.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether validation passed
    pub is_valid: bool,
    /// Validation errors found
    pub errors: Vec<ValidationError>,
    /// Validation warnings
    pub warnings: Vec<ValidationWarning>,
}

/// Configuration validation error.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Error category
    pub category: ValidationCategory,
    /// Error message
    pub message: String,
    /// Context information
    pub context: Option<String>,
}

/// Configuration validation warning.
#[derive(Debug, Clone)]
pub struct ValidationWarning {
    /// Warning category
    pub category: ValidationCategory,
    /// Warning message
    pub message: String,
    /// Context information
    pub context: Option<String>,
}

/// Validation error/warning categories.
#[derive(Debug, Clone)]
pub enum ValidationCategory {
    /// Address conflicts or invalid addresses
    Addressing,
    /// Security key issues
    Security,
    /// Group address issues
    GroupAddress,
}

/// Configuration validator.
pub struct ConfigValidator;

impl ConfigValidator {
    /// Create a new validator.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Validate a configuration.
    #[must_use]
    pub fn validate(&self, config: &Configuration) -> ValidationResult {
        let mut result = ValidationResult {
            is_valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
        };

        // Validate keyring if present
        if let Some(ref keyring) = config.keyring {
            Self::validate_keyring(keyring, &mut result);
        }

        // Update overall validity
        result.is_valid = result.errors.is_empty();

        result
    }

    /// Validate keyring configuration.
    fn validate_keyring(keyring: &KeyringConfig, result: &mut ValidationResult) {
        // Check for duplicate individual addresses
        let mut individual_addresses = HashSet::new();

        // Check interfaces
        for interface in &keyring.interfaces {
            if !individual_addresses.insert(interface.individual_address) {
                result.errors.push(ValidationError {
                    category: ValidationCategory::Addressing,
                    message: format!(
                        "Duplicate individual address {} in interfaces",
                        interface.individual_address
                    ),
                    context: Some(format!("Interface: {}", interface.host)),
                });
            }

            // Validate interface-specific fields
            if interface.user_id == 0 || interface.user_id > 127 {
                result.errors.push(ValidationError {
                    category: ValidationCategory::Security,
                    message: format!("Invalid user ID {} (must be 1-127)", interface.user_id),
                    context: Some(format!("Interface: {}", interface.host)),
                });
            }

            if interface.user_password.is_empty() {
                result.errors.push(ValidationError {
                    category: ValidationCategory::Security,
                    message: "Empty user password".to_string(),
                    context: Some(format!("Interface: {}", interface.host)),
                });
            }
        }

        // Check devices
        for device in &keyring.devices {
            if !individual_addresses.insert(device.individual_address) {
                result.errors.push(ValidationError {
                    category: ValidationCategory::Addressing,
                    message: format!(
                        "Duplicate individual address {} in devices",
                        device.individual_address
                    ),
                    context: device.serial_number.clone(),
                });
            }
        }

        // Check group addresses
        let mut group_addresses = HashSet::new();
        for ga in &keyring.group_addresses {
            if !group_addresses.insert(ga.address) {
                result.warnings.push(ValidationWarning {
                    category: ValidationCategory::GroupAddress,
                    message: format!("Duplicate group address {}", ga.address),
                    context: ga.description.clone(),
                });
            }

            if ga.key.is_empty() {
                result.errors.push(ValidationError {
                    category: ValidationCategory::Security,
                    message: format!("Empty security key for group address {}", ga.address),
                    context: ga.description.clone(),
                });
            }
        }
    }
}

impl Default for ConfigValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationResult {
    /// Check if validation passed without errors.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.is_valid
    }

    /// Get the number of errors.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// Get the number of warnings.
    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.warnings.len()
    }

    /// Get all error messages.
    #[must_use]
    pub fn error_messages(&self) -> Vec<String> {
        self.errors.iter().map(|e| e.message.clone()).collect()
    }

    /// Get all warning messages.
    #[must_use]
    pub fn warning_messages(&self) -> Vec<String> {
        self.warnings.iter().map(|w| w.message.clone()).collect()
    }
}
