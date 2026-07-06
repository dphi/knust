//! Secure KNX/IP connection example demonstrating KNX Data Security.
//!
//! This example shows how to establish secure connections using KNX IP Secure
//! with proper authentication, encryption, and key management.

use knust::protocol::address::Address;
use knust::protocol::telegram::{Direction, Priority, Telegram, TelegramType};
use knust::protocol::{GroupAddress, IndividualAddress};
use knust::security::{KeyRing, SecurityCredentials, SecurityKey};
use knust::transport::{BackoffConfig, SecurityConfig};
use knust::{Component, ConnectionType, Knx, LogLevel, LoggingConfig};
use std::time::Duration;

// Linear connect -> demonstrate -> report walkthrough; splitting it up would
// add indirection without making the flow clearer.
#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging with security focus
    env_logger::init();

    println!("🔐 Knx Secure Connection Example");
    println!("==================================");

    // Configure logging to show security operations
    let mut logging_config = LoggingConfig::new();
    logging_config.set_component_level(Component::Security, LogLevel::Debug);
    logging_config.set_component_level(Component::Transport, LogLevel::Info);
    logging_config.set_component_level(Component::Application, LogLevel::Info);
    logging_config.set_protocol_events(true);

    // Example 1: Basic secure tunneling connection
    println!("\n📡 Example 1: Secure Tunneling Connection");
    println!("------------------------------------------");

    // Create security configuration for tunneling
    let security_config = SecurityConfig {
        device_auth_password: "DeviceAuthPassword123!".to_string(),
        user_password: Some("MySecurePassword123".to_string()),
        keyring_path: None,
        session_timeout: 300, // 5 minutes
    };

    let secure_config = knust::ConnectionConfig {
        connection_type: ConnectionType::SecureTunneling,
        gateway_ip: Some("192.168.1.100".parse().unwrap()),
        gateway_port: Some(3671),
        individual_address: IndividualAddress::new(1, 1, 240),
        security: Some(security_config),
        timeout_ms: 10000,
        auto_reconnect: true,
        reconnect_backoff: BackoffConfig::default(),
        ..Default::default()
    };

    println!("🔧 Configuration:");
    println!("   Connection type: Secure Tunneling");
    println!("   Gateway: 192.168.1.100:3671");
    println!("   Individual address: 1.1.240");
    println!("   Session timeout: 5 minutes");
    println!("   Auto-reconnect: Enabled");

    // Create Knx instance with secure configuration
    let secure_knx = Knx::new(secure_config).await?;

    // Attempt secure connection
    println!("\n🔐 Attempting secure connection...");
    match secure_knx.connect().await {
        Ok(()) => {
            println!("✅ Secure connection established!");

            // Send a telegram over the secure tunnel directly.
            let source_addr = IndividualAddress::new(1, 1, 240);
            let switch_telegram = Telegram {
                source: source_addr,
                destination: Address::Group(GroupAddress::from_parts(1, 2, 1)?),
                payload: vec![0x01],
                priority: Priority::Normal,
                direction: Direction::Outgoing,
                telegram_type: TelegramType::GroupValueWrite,
                gateway_id: None,
                timestamp: std::time::SystemTime::now(),
            };

            println!("\n🔒 Testing secure telegram send...");
            // Note: This will likely fail without a real secure gateway
            // but demonstrates the API usage
            match secure_knx.send_telegram(&switch_telegram).await {
                Ok(()) => println!("✅ Telegram sent over the secure tunnel"),
                Err(e) => println!("❌ Send failed (expected without real gateway): {e}"),
            }

            tokio::time::sleep(Duration::from_secs(2)).await;

            secure_knx.disconnect().await?;
            println!("🔌 Disconnected from secure gateway");
        }
        Err(e) => {
            println!("❌ Secure connection failed (expected without real gateway): {e}");
            println!("   This demonstrates the security handshake process");
        }
    }

    // Example 2: Using keyring for group address security
    println!("\n🗝️  Example 2: KNX Data Security with Keyring");
    println!("----------------------------------------------");

    // Create a keyring with group keys
    let mut keyring = KeyRing::new();

    // Add group keys for Data Secure communication
    let group_key_1 = SecurityKey::from_hex("0123456789ABCDEF0123456789ABCDEF")?;
    let group_key_2 = SecurityKey::from_hex("FEDCBA9876543210FEDCBA9876543210")?;

    keyring.add_group_key(GroupAddress::from_parts(1, 2, 1)?, group_key_1);
    keyring.add_group_key(GroupAddress::from_parts(1, 2, 2)?, group_key_2);

    println!(
        "🔑 Created keyring with {} group keys",
        keyring.group_key_count()
    );
    println!("   Secured groups: 1/2/1, 1/2/2");

    // Check which groups are secured
    let test_addr_1 = GroupAddress::from_parts(1, 2, 1)?;
    let test_addr_2 = GroupAddress::from_parts(1, 3, 1)?;

    println!(
        "   Group 1/2/1 secured: {}",
        keyring.is_group_secured(&test_addr_1)
    );
    println!(
        "   Group 1/3/1 secured: {}",
        keyring.is_group_secured(&test_addr_2)
    );

    // Example 3: Security credentials management
    println!("\n👤 Example 3: Security Credentials Management");
    println!("----------------------------------------------");

    // Create user credentials
    let user_password = SecurityKey::from_hex("ABCDEF0123456789ABCDEF0123456789")?;
    let device_auth = SecurityKey::from_hex("9876543210FEDCBA9876543210FEDCBA")?;
    let backbone_key = SecurityKey::from_hex("11112222333344445555666677778888")?;

    let credentials = SecurityCredentials::new(1, user_password)
        .with_device_auth(device_auth)
        .with_backbone_key(backbone_key);

    println!("👤 Created security credentials:");
    println!("   User ID: {}", credentials.user_id);
    println!("   User password: [32 bytes]");
    println!("   Device auth: [32 bytes]");
    println!("   Backbone key: [32 bytes]");

    // Add credentials to keyring
    let mut enhanced_keyring = KeyRing::new();
    enhanced_keyring.add_tunnel_credentials("192.168.1.100".to_string(), credentials);

    println!("🔐 Added tunnel credentials for gateway 192.168.1.100");

    // Example 4: Sequence number management for replay protection
    println!("\n🔄 Example 4: Sequence Number Management");
    println!("----------------------------------------");

    let sender_addr = IndividualAddress::new(1, 2, 3);

    // Simulate receiving telegrams with sequence numbers
    let test_sequences = vec![100, 101, 102, 105, 104]; // Note: 104 < 105, should be rejected

    for seq in test_sequences {
        match enhanced_keyring.validate_sequence(sender_addr, seq) {
            Ok(()) => {
                println!("✅ Sequence {seq} from {sender_addr} accepted");
            }
            Err(e) => {
                println!("❌ Sequence {seq} from {sender_addr} rejected: {e}");
            }
        }
    }

    println!("📊 Keyring statistics:");
    println!("   Group keys: {}", enhanced_keyring.group_key_count());
    println!("   Tracked senders: {}", enhanced_keyring.sender_count());

    // Example 5: Configuration file example
    println!("\n📄 Example 5: Configuration File Format");
    println!("----------------------------------------");

    println!("Example secure connection configuration (TOML format):");
    println!();
    println!("[connection]");
    println!("type = \"SecureTunneling\"");
    println!("gateway_ip = \"192.168.1.100\"");
    println!("gateway_port = 3671");
    println!("individual_address = \"1.1.240\"");
    println!("timeout_ms = 10000");
    println!("auto_reconnect = true");
    println!();
    println!("[security]");
    println!("user_password = \"MySecurePassword123\"");
    println!("session_timeout = 300");
    println!("device_auth = [");
    println!("    0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0,");
    println!("    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88");
    println!("]");
    println!();
    println!("[[security.group_keys]]");
    println!("address = \"1/2/1\"");
    println!("key = \"0123456789ABCDEF0123456789ABCDEF\"");
    println!();
    println!("[[security.group_keys]]");
    println!("address = \"1/2/2\"");
    println!("key = \"FEDCBA9876543210FEDCBA9876543210\"");

    println!("\n✅ Secure connection examples completed!");
    println!("\n🔒 Security Features Demonstrated:");
    println!("   • KNX IP Secure tunneling with authentication");
    println!("   • Group address encryption (KNX Data Security)");
    println!("   • Security credential management");
    println!("   • Keyring-based key storage");
    println!("   • Sequence number validation for replay protection");
    println!("   • Configuration file format for security settings");
    println!();
    println!("💡 Security Best Practices:");
    println!("   • Use strong, unique passwords for each installation");
    println!("   • Regularly rotate security keys");
    println!("   • Monitor for replay attacks via sequence numbers");
    println!("   • Store keyrings securely with proper file permissions");
    println!("   • Use secure tunneling for remote access");

    Ok(())
}
