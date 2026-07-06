//! Tests for configuration parsing functionality.

use super::*;
#[cfg(feature = "secure")]
use crate::config::keyring::{KeyringGroupAddress, KeyringMetadata};
#[cfg(feature = "secure")]
use crate::protocol::address::{GroupAddress, IndividualAddress};
#[cfg(feature = "secure")]
use crate::security::SecurityKey;
#[cfg(feature = "secure")]
use proptest::prelude::*;

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_configuration_new() {
        let config = Configuration::new();
        #[cfg(feature = "secure")]
        assert!(config.keyring.is_none());
        assert!(matches!(config.metadata.format, ConfigFormat::Unknown));
    }

    #[test]
    fn test_config_format_default() {
        let format = ConfigFormat::default();
        assert!(matches!(format, ConfigFormat::Unknown));
    }

    #[cfg(feature = "secure")]
    #[test]
    fn test_keyring_config_creation() {
        let keyring = KeyringConfig {
            metadata: KeyringMetadata {
                created: Some("2024-01-01".to_string()),
                creator: Some("Test".to_string()),
                project: Some("Test Project".to_string()),
                signature: None,
            },
            interfaces: vec![],
            devices: vec![],
            group_addresses: vec![],
        };

        assert_eq!(keyring.metadata.created, Some("2024-01-01".to_string()));
        assert_eq!(keyring.metadata.creator, Some("Test".to_string()));
    }

    #[cfg(feature = "secure")]
    #[test]
    fn test_keyring_xml_escaping_round_trip() {
        let name = r#"Office <A> & "B"'s lamp"#;
        let keyring = KeyringConfig {
            metadata: KeyringMetadata {
                created: None,
                creator: Some(name.to_string()),
                project: None,
                signature: None,
            },
            interfaces: vec![],
            devices: vec![],
            group_addresses: vec![KeyringGroupAddress {
                address: GroupAddress::try_from_raw(1).unwrap(),
                key: SecurityKey::new(vec![0u8; 16]),
                description: Some(name.to_string()),
            }],
        };

        let serialized = KeyringParser::serialize(&keyring);
        let parsed = KeyringParser::parse_bytes(&serialized).unwrap();

        assert_eq!(parsed.metadata.creator.as_deref(), Some(name));
        assert_eq!(parsed.group_addresses[0].description.as_deref(), Some(name));
    }

    #[cfg(feature = "secure")]
    #[test]
    fn test_validation_result() {
        let result = ValidationResult {
            is_valid: true,
            errors: vec![],
            warnings: vec![],
        };

        assert!(result.is_ok());
        assert_eq!(result.error_count(), 0);
        assert_eq!(result.warning_count(), 0);
    }

    #[cfg(feature = "secure")]
    #[test]
    fn test_debug_serialization() {
        // Create a simple keyring with project name
        let keyring = KeyringConfig {
            metadata: KeyringMetadata {
                created: None,
                creator: None,
                project: Some("test".to_string()),
                signature: None,
            },
            interfaces: vec![],
            devices: vec![],
            group_addresses: vec![],
        };

        // Serialize it
        let serialized = KeyringParser::serialize(&keyring);
        let xml_str = String::from_utf8(serialized.clone()).unwrap();
        println!("Serialized XML: {xml_str}");

        // Parse it back
        let parsed = KeyringParser::parse_bytes(&serialized).unwrap();
        println!("Original project: {:?}", keyring.metadata.project);
        println!("Parsed project: {:?}", parsed.metadata.project);

        assert_eq!(parsed.metadata.project, keyring.metadata.project);
    }

    #[cfg(feature = "secure")]
    #[test]
    fn test_config_validator_creation() {
        let _validator = ConfigValidator::new();
    }

    #[cfg(feature = "secure")]
    #[test]
    fn test_empty_configuration_validation() {
        let config = Configuration::new();
        let validator = ConfigValidator::new();
        let result = validator.validate(&config);

        assert!(result.is_valid);
        assert_eq!(result.errors.len(), 0);
    }
}

#[cfg(test)]
#[cfg(feature = "secure")]
mod property_tests {
    use super::*;

    // Property test for configuration parsing round trip
    proptest! {
        #[test]
        fn test_keyring_serialization_round_trip(
            created in prop::option::of("[0-9]{4}-[0-9]{2}-[0-9]{2}"),
            creator in prop::option::of("[A-Za-z ]{1,50}"),
            project_name in prop::option::of("[A-Za-z0-9 ]{1,50}"),
            interface_count in 0usize..5,
            device_count in 0usize..5,
            ga_count in 0usize..5,
        ) {
            // Create a keyring configuration
            let mut keyring = KeyringConfig {
                metadata: KeyringMetadata {
                    created,
                    creator,
                    project: project_name,
                    signature: None,
                },
                interfaces: vec![],
                devices: vec![],
                group_addresses: vec![],
            };

            // Add interfaces
            for i in 0..interface_count {
                let interface = KeyringInterface {
                    individual_address: IndividualAddress::new(1, 1, i as u8 + 1),
                    interface_type: "Tunneling".to_string(),
                    host: format!("192.168.1.{}", i + 100),
                    user_id: (i as u8 + 1).min(127),
                    user_password: SecurityKey::new(vec![i as u8; 16]),
                    device_authentication: None,
                    backbone_key: None,
                };
                keyring.interfaces.push(interface);
            }

            // Add devices
            for i in 0..device_count {
                let device = KeyringDevice {
                    individual_address: IndividualAddress::new(2, 1, i as u8 + 1),
                    serial_number: Some(format!("SN{i:06}")),
                    tool_key: Some(SecurityKey::new(vec![i as u8; 16])),
                    management_password: None,
                    authentication_code: None,
                };
                keyring.devices.push(device);
            }

            // Add group addresses
            for i in 0..ga_count {
                let ga = KeyringGroupAddress {
                    address: GroupAddress::try_from_raw(i as u16 + 1).unwrap(),
                    key: SecurityKey::new(vec![i as u8; 16]),
                    description: Some(format!("Group {i}")),
                };
                keyring.group_addresses.push(ga);
            }

            // Serialize the keyring
            let serialized = KeyringParser::serialize(&keyring);

            // Parse it back
            let parsed = KeyringParser::parse_bytes(&serialized).unwrap();

            // Verify round trip integrity
            assert_eq!(parsed.metadata.created, keyring.metadata.created);
            assert_eq!(parsed.metadata.creator, keyring.metadata.creator);
            assert_eq!(parsed.metadata.project, keyring.metadata.project);
            assert_eq!(parsed.interfaces.len(), keyring.interfaces.len());
            assert_eq!(parsed.devices.len(), keyring.devices.len());
            assert_eq!(parsed.group_addresses.len(), keyring.group_addresses.len());

            // Verify interface data
            for (original, parsed) in keyring.interfaces.iter().zip(parsed.interfaces.iter()) {
                assert_eq!(original.individual_address, parsed.individual_address);
                assert_eq!(original.interface_type, parsed.interface_type);
                assert_eq!(original.host, parsed.host);
                assert_eq!(original.user_id, parsed.user_id);
                assert_eq!(original.user_password.as_bytes(), parsed.user_password.as_bytes());
            }

            // Verify device data
            for (original, parsed) in keyring.devices.iter().zip(parsed.devices.iter()) {
                assert_eq!(original.individual_address, parsed.individual_address);
                assert_eq!(original.serial_number, parsed.serial_number);
            }

            // Verify group address data
            for (original, parsed) in keyring.group_addresses.iter().zip(parsed.group_addresses.iter()) {
                assert_eq!(original.address, parsed.address);
                assert_eq!(original.key.as_bytes(), parsed.key.as_bytes());
                assert_eq!(original.description, parsed.description);
            }
        }
    }

    proptest! {
        #[test]
        fn test_configuration_validation_consistency(
            interface_count in 0usize..10,
            device_count in 0usize..10,
            duplicate_addresses in any::<bool>(),
        ) {
            // Create a configuration with potential validation issues
            let mut keyring = KeyringConfig {
                metadata: KeyringMetadata {
                    created: Some("2024-01-01".to_string()),
                    creator: Some("Test".to_string()),
                    project: Some("Test Project".to_string()),
                    signature: None,
                },
                interfaces: vec![],
                devices: vec![],
                group_addresses: vec![],
            };

            // Add interfaces
            for i in 0..interface_count {
                let addr_offset = if duplicate_addresses && i > 0 { 0 } else { i };
                let interface = KeyringInterface {
                    individual_address: IndividualAddress::new(1, 1, addr_offset as u8 + 1),
                    interface_type: "Tunneling".to_string(),
                    host: format!("192.168.1.{}", i + 100),
                    user_id: (i as u8 + 1).min(127),
                    user_password: SecurityKey::new(vec![1; 16]),
                    device_authentication: None,
                    backbone_key: None,
                };
                keyring.interfaces.push(interface);
            }

            // Add devices
            for i in 0..device_count {
                let addr_offset = if duplicate_addresses && i > 0 { 0 } else { i };
                let device = KeyringDevice {
                    individual_address: IndividualAddress::new(2, 1, addr_offset as u8 + 1),
                    serial_number: Some(format!("SN{i:06}")),
                    tool_key: Some(SecurityKey::new(vec![1; 16])),
                    management_password: None,
                    authentication_code: None,
                };
                keyring.devices.push(device);
            }

            let config = Configuration {
                keyring: Some(keyring),
                metadata: ConfigMetadata::default(),
            };

            let validator = ConfigValidator::new();
            let result = validator.validate(&config);

            // If we have duplicate addresses, there should be validation errors
            if duplicate_addresses && (interface_count > 1 || device_count > 1) {
                assert!(!result.is_valid);
                assert!(result.error_count() > 0);
            } else {
                // Otherwise, validation should pass
                assert!(result.is_valid);
            }
        }
    }
}
