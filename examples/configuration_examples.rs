//! Configuration examples for Knx.
//!
//! This example demonstrates various ways to configure Knx for different
//! use cases, including connection types, device setup, and advanced options.

use knust::protocol::{GroupAddress, IndividualAddress};
use knust::transport::{BackoffConfig, SecurityConfig, TcpConfig};
use knust::{Component, ConnectionConfig, ConnectionType, Knx, LogLevel, LoggingConfig};

// Linear connect -> demonstrate -> report walkthrough; splitting it up would
// add indirection without making the flow clearer.
#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("⚙️  Knx Configuration Examples");
    println!("===============================");

    // Example 1: Basic Tunneling Configuration
    println!("\n🔌 Example 1: Basic Tunneling Configuration");
    println!("--------------------------------------------");

    let basic_config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: Some("192.168.1.100".parse().unwrap()),
        gateway_port: Some(3671), // Standard KNX/IP port
        local_ip: None,           // Auto-detect
        individual_address: IndividualAddress::new(1, 1, 240),
        security: None,
        timeout_ms: 5000,
        auto_reconnect: true,
        reconnect_backoff: BackoffConfig::default(),
        tcp_config: TcpConfig::default(),
    };

    println!("📋 Basic Tunneling Configuration:");
    println!("   Connection type: {:?}", basic_config.connection_type);
    println!("   Gateway IP: {:?}", basic_config.gateway_ip);
    println!("   Gateway port: {:?}", basic_config.gateway_port);
    println!("   Individual address: {}", basic_config.individual_address);
    println!("   Timeout: {}ms", basic_config.timeout_ms);
    println!("   Auto-reconnect: {}", basic_config.auto_reconnect);

    let _basic_knx = Knx::new(basic_config).await?;
    println!("✅ Basic Knx instance created");

    // Example 2: Routing Configuration
    println!("\n📡 Example 2: Multicast Routing Configuration");
    println!("----------------------------------------------");

    let routing_config = ConnectionConfig {
        connection_type: ConnectionType::Routing,
        gateway_ip: None, // Not needed for routing
        gateway_port: Some(3671),
        local_ip: Some("192.168.1.50".parse().unwrap()), // Specify interface
        individual_address: IndividualAddress::new(1, 1, 241),
        security: None,
        timeout_ms: 3000,
        auto_reconnect: false, // Routing doesn't need reconnection
        reconnect_backoff: BackoffConfig::default(),
        tcp_config: TcpConfig::default(),
    };

    println!("📋 Routing Configuration:");
    println!("   Connection type: {:?}", routing_config.connection_type);
    println!("   Local IP: {:?}", routing_config.local_ip);
    println!(
        "   Individual address: {}",
        routing_config.individual_address
    );
    println!("   Auto-reconnect: {}", routing_config.auto_reconnect);

    let _routing_knx = Knx::new(routing_config).await?;
    println!("✅ Routing Knx instance created");

    // Example 3: Advanced Configuration with Custom Backoff
    println!("\n⚡ Example 3: Advanced Configuration with Custom Backoff");
    println!("--------------------------------------------------------");

    let custom_backoff = BackoffConfig {
        initial_delay_ms: 500, // Start with 500ms delay
        max_delay_ms: 60000,   // Max 60 seconds
        multiplier: 1.5,       // Increase by 50% each time
        max_attempts: 15,      // Try up to 15 times
    };

    let advanced_config = ConnectionConfig {
        connection_type: ConnectionType::Tunneling,
        gateway_ip: Some("192.168.1.100".parse().unwrap()),
        gateway_port: Some(3671),
        local_ip: Some("192.168.1.50".parse().unwrap()),
        individual_address: IndividualAddress::new(1, 1, 242),
        security: None,
        timeout_ms: 8000,
        auto_reconnect: true,
        reconnect_backoff: custom_backoff,
        tcp_config: TcpConfig::default(),
    };

    println!("📋 Advanced Configuration:");
    println!("   Timeout: {}ms", advanced_config.timeout_ms);
    println!(
        "   Backoff initial delay: {}ms",
        advanced_config.reconnect_backoff.initial_delay_ms
    );
    println!(
        "   Backoff max delay: {}ms",
        advanced_config.reconnect_backoff.max_delay_ms
    );
    println!(
        "   Backoff multiplier: {}",
        advanced_config.reconnect_backoff.multiplier
    );
    println!(
        "   Max reconnect attempts: {}",
        advanced_config.reconnect_backoff.max_attempts
    );

    let _advanced_knx = Knx::new(advanced_config).await?;
    println!("✅ Advanced Knx instance created");

    // Example 4: Secure Configuration
    println!("\n🔐 Example 4: Secure Tunneling Configuration");
    println!("---------------------------------------------");

    let security_config = SecurityConfig {
        device_auth_password: "DeviceAuthPassword123!".to_string(),
        user_password: Some("SecurePassword123!".to_string()),
        keyring_path: Some("/path/to/keyring.knxkeys".to_string()),
        session_timeout: 600, // 10 minutes
    };

    let secure_config = ConnectionConfig {
        connection_type: ConnectionType::SecureTunneling,
        gateway_ip: Some("192.168.1.100".parse().unwrap()),
        gateway_port: Some(3671),
        local_ip: None,
        individual_address: IndividualAddress::new(1, 1, 243),
        security: Some(security_config),
        timeout_ms: 15000, // Longer timeout for secure handshake
        auto_reconnect: true,
        reconnect_backoff: BackoffConfig {
            initial_delay_ms: 2000, // Longer initial delay for secure connections
            max_delay_ms: 120_000,  // Max 2 minutes
            multiplier: 2.0,
            max_attempts: 10,
        },
        tcp_config: TcpConfig::default(),
    };

    println!("📋 Secure Configuration:");
    println!("   Connection type: {:?}", secure_config.connection_type);
    println!("   Security: Enabled");
    println!(
        "   Session timeout: {}s",
        secure_config.security.as_ref().unwrap().session_timeout
    );
    println!(
        "   Keyring path: {:?}",
        secure_config.security.as_ref().unwrap().keyring_path
    );
    println!("   Extended timeout: {}ms", secure_config.timeout_ms);

    let _secure_knx = Knx::new(secure_config).await?;
    println!("✅ Secure Knx instance created");

    // Example 5: Group Address Layout
    println!("\n💡 Example 5: Group Address Layout Examples");
    println!("--------------------------------------------");
    println!("There's no built-in device layer - see examples/custom_devices.rs");
    println!("for the recommended pattern to build one on send_telegram/read_group_value.");

    println!("💡 Living Room Main Light:");
    println!("   Switch: {}", GroupAddress::from_parts(1, 2, 1)?);
    println!("   Brightness: {}", GroupAddress::from_parts(1, 2, 2)?);
    println!("   Color: {}", GroupAddress::from_parts(1, 2, 3)?);

    println!("💡 Hallway Light (switch only):");
    println!("   Switch: {}", GroupAddress::from_parts(1, 3, 1)?);

    println!("🌡️  Living Room Temperature (DPT 9.001):");
    println!("   Address: {}", GroupAddress::from_parts(2, 1, 1)?);

    println!("💧 Living Room Humidity (DPT 9.007):");
    println!("   Address: {}", GroupAddress::from_parts(2, 1, 2)?);

    // Example 6: Logging Configuration
    println!("\n📝 Example 6: Logging Configuration");
    println!("-----------------------------------");

    let mut logging_config = LoggingConfig::new();

    // Set component-specific log levels
    logging_config.set_default_level(LogLevel::Info);
    logging_config.set_component_level(Component::Transport, LogLevel::Debug);
    logging_config.set_component_level(Component::Protocol, LogLevel::Trace);
    logging_config.set_component_level(Component::Device, LogLevel::Info);
    logging_config.set_component_level(Component::Security, LogLevel::Debug);
    logging_config.set_component_level(Component::Application, LogLevel::Info);

    // Enable advanced logging features
    logging_config.set_protocol_events(true);
    logging_config.set_hex_dump(true);
    logging_config.set_max_hex_dump_size(128);

    println!("📋 Logging Configuration:");
    println!("   Default level: {:?}", LogLevel::Info);
    println!("   Transport: {:?}", LogLevel::Debug);
    println!("   Protocol: {:?} (with hex dumps)", LogLevel::Trace);
    println!("   Device: {:?}", LogLevel::Info);
    println!("   Security: {:?}", LogLevel::Debug);
    println!("   Protocol events: Enabled");
    println!("   Hex dump size: 128 bytes");

    // Example 7: Builder Pattern Configuration
    println!("\n🏗️  Example 7: Builder Pattern Configuration");
    println!("---------------------------------------------");

    let _builder_knx = Knx::builder()
        .connection_type(ConnectionType::Tunneling)
        .gateway_ip("192.168.1.100".parse().unwrap())
        .gateway_port(3671)
        .individual_address(IndividualAddress::new(1, 1, 244))
        .timeout_ms(7000)
        .auto_reconnect(true)
        .build()
        .await?;

    println!("📋 Builder Pattern Configuration:");
    println!("   ✅ Fluent API for easy configuration");
    println!("   ✅ Type-safe parameter validation");
    println!("   ✅ Default values for optional parameters");
    println!("   ✅ Compile-time configuration validation");

    // Example 8: Configuration File Formats
    println!("\n📄 Example 8: Configuration File Formats");
    println!("-----------------------------------------");

    println!("TOML Configuration Example:");
    println!("```toml");
    println!("[connection]");
    println!("type = \"Tunneling\"");
    println!("gateway_ip = \"192.168.1.100\"");
    println!("gateway_port = 3671");
    println!("individual_address = \"1.1.240\"");
    println!("timeout_ms = 5000");
    println!("auto_reconnect = true");
    println!();
    println!("[reconnect]");
    println!("initial_delay_ms = 1000");
    println!("max_delay_ms = 30000");
    println!("multiplier = 2.0");
    println!("max_attempts = 10");
    println!();
    println!("[[devices]]");
    println!("type = \"Light\"");
    println!("name = \"Living Room Light\"");
    println!("switch_address = \"1/2/1\"");
    println!("brightness_address = \"1/2/2\"");
    println!("color_address = \"1/2/3\"");
    println!();
    println!("[[devices]]");
    println!("type = \"Sensor\"");
    println!("name = \"Temperature Sensor\"");
    println!("address = \"2/1/1\"");
    println!("dpt = \"9.001\"");
    println!("```");

    println!("\nJSON Configuration Example:");
    println!("```json");
    println!("{{");
    println!("  \"connection\": {{");
    println!("    \"type\": \"Tunneling\",");
    println!("    \"gateway_ip\": \"192.168.1.100\",");
    println!("    \"gateway_port\": 3671,");
    println!("    \"individual_address\": \"1.1.240\",");
    println!("    \"timeout_ms\": 5000,");
    println!("    \"auto_reconnect\": true");
    println!("  }},");
    println!("  \"devices\": [");
    println!("    {{");
    println!("      \"type\": \"Light\",");
    println!("      \"name\": \"Living Room Light\",");
    println!("      \"switch_address\": \"1/2/1\",");
    println!("      \"brightness_address\": \"1/2/2\"");
    println!("    }}");
    println!("  ]");
    println!("}}");
    println!("```");

    println!("\n✅ Configuration examples completed!");
    println!("\n📚 Configuration Summary:");
    println!("   • Basic tunneling for point-to-point connections");
    println!("   • Multicast routing for broadcast communication");
    println!("   • Advanced reconnection strategies");
    println!("   • Secure connections with authentication");
    println!("   • Device configuration for lights and sensors");
    println!("   • Comprehensive logging setup");
    println!("   • Builder pattern for fluent configuration");
    println!("   • Multiple configuration file formats");
    println!();
    println!("💡 Best Practices:");
    println!("   • Use tunneling for reliable point-to-point connections");
    println!("   • Use routing for monitoring multiple devices");
    println!("   • Configure appropriate timeouts for your network");
    println!("   • Enable auto-reconnect for production deployments");
    println!("   • Use secure connections for sensitive installations");
    println!("   • Set component-specific log levels for debugging");

    Ok(())
}
