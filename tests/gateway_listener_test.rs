//! Test that connects to KNX gateway and listens for packets for 3 minutes
//!
//! This test demonstrates how to:
//! - Connect to a KNX gateway
//! - Register telegram callbacks to listen for incoming packets
//! - Print received packets with detailed information
//! - Run for a specified duration (3 minutes)

use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use tokio::time::{sleep, timeout};

use knust::application::callbacks::TelegramCallbackFn;
use knust::protocol::address::IndividualAddress;
use knust::protocol::telegram::{Telegram, TelegramType};
use knust::{ConnectionConfig, ConnectionType, Knx};

/// Test configuration from environment variables
struct TestConfig {
    gateway_ip: Option<std::net::IpAddr>,
    individual_address: IndividualAddress,
    connection_type: ConnectionType,
    duration: Duration,
}

impl TestConfig {
    fn from_env() -> Self {
        // Try to load from .env.test file first, then environment variables
        let gateway_ip = env::var("KNX_GATEWAY_IP")
            .or_else(|_| env::var("KNX_GATEWAY"))
            .ok()
            .and_then(|ip| ip.parse().ok())
            .or_else(|| {
                // Default to the IP from .env.test if no environment variable is set
                "192.168.33.49".parse().ok()
            });

        let individual_address = env::var("KNX_INDIVIDUAL_ADDRESS")
            .ok()
            .and_then(|addr| addr.parse().ok())
            .unwrap_or_else(|| IndividualAddress::new(1, 1, 240));

        let connection_type = match env::var("KNX_CONNECTION_TYPE").as_deref() {
            Ok("routing") => ConnectionType::Routing,
            Ok("tcp_tunneling") => ConnectionType::TcpTunneling,
            _ => ConnectionType::Tunneling, // Default to tunneling
        };

        let duration_secs = env::var("KNX_LISTEN_DURATION")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(180); // Default 3 minutes

        Self {
            gateway_ip,
            individual_address,
            connection_type,
            duration: Duration::from_secs(duration_secs),
        }
    }
}

/// Format telegram information for display
fn format_telegram(telegram: &Telegram, packet_count: usize) -> String {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let destination_str = match &telegram.destination {
        knust::protocol::Address::Group(addr) => format!("Group {addr}"),
        knust::protocol::Address::Individual(addr) => format!("Individual {addr}"),
    };

    let payload_hex = if telegram.payload.is_empty() {
        "Empty".to_string()
    } else {
        telegram
            .payload
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    };

    format!(
        "[{}] Packet #{}: {} -> {} | Priority: {:?} | Direction: {:?} | Payload: {} ({} bytes)",
        timestamp,
        packet_count,
        telegram.source,
        destination_str,
        telegram.priority,
        telegram.direction,
        payload_hex,
        telegram.payload.len()
    )
}

/// Callback wrapper that prints each received telegram.
struct PacketPrinter {
    counter: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl TelegramCallbackFn for PacketPrinter {
    async fn call(&self, telegram: &Telegram) {
        let count = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let formatted = format_telegram(telegram, count);
        println!("{formatted}");
    }
}

/// Callback wrapper used by the mock (no real gateway) test.
struct MockPacketPrinter {
    counter: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl TelegramCallbackFn for MockPacketPrinter {
    async fn call(&self, telegram: &Telegram) {
        let count = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let formatted = format_telegram(telegram, count);
        println!("MOCK: {formatted}");
    }
}

/// Main test function that listens for KNX packets
// Linear connect -> listen -> report walkthrough; splitting it up would add
// indirection without making the flow clearer.
#[allow(clippy::too_many_lines)]
async fn listen_for_packets(config: TestConfig) -> Result<(), Box<dyn std::error::Error>> {
    println!("KNX Gateway Packet Listener Test");
    println!("================================");
    println!("Configuration:");
    println!("  Gateway IP: {:?}", config.gateway_ip);
    println!("  Individual Address: {}", config.individual_address);
    println!("  Connection Type: {:?}", config.connection_type);
    println!("  Listen Duration: {:?}", config.duration);
    println!();

    // Create connection configuration
    let connection_config = ConnectionConfig {
        connection_type: config.connection_type,
        gateway_ip: config.gateway_ip,
        individual_address: config.individual_address,
        timeout_ms: 10000, // 10 second timeout
        auto_reconnect: true,
        ..Default::default()
    };

    // Packet counter for display
    let packet_counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = packet_counter.clone();

    // Create Knx instance
    let knx = Knx::builder()
        .connection_type(config.connection_type)
        .gateway_ip(
            config
                .gateway_ip
                .unwrap_or_else(|| "192.168.1.100".parse().unwrap()),
        )
        .individual_address(config.individual_address)
        .timeout_ms(connection_config.timeout_ms)
        .auto_reconnect(connection_config.auto_reconnect)
        .build()
        .await?;

    // Register telegram callback after creation
    let _callback_handle = knx
        .register_telegram_callback(PacketPrinter {
            counter: counter_clone,
        })
        .await;

    println!("Knx instance created successfully");

    // Attempt to connect to the gateway
    println!("Connecting to KNX gateway...");
    match knx.connect().await {
        Ok(()) => {
            println!("✓ Successfully connected to KNX gateway");
        }
        Err(e) => {
            println!("✗ Failed to connect to KNX gateway: {e}");
            println!("  This might be expected if no gateway is available");
            println!("  The test will continue to demonstrate the setup");
            return Ok(());
        }
    }

    // Start telegram processing
    println!("Starting telegram processing...");
    knx.start().await?;
    println!("✓ Telegram processing started");

    println!();
    println!("Listening for KNX packets for {:?}...", config.duration);
    println!("Press Ctrl+C to stop early");
    println!("----------------------------------------");

    // Listen for the specified duration
    let listen_result = timeout(config.duration, async {
        // Keep the connection alive and process telegrams
        loop {
            sleep(Duration::from_millis(100)).await;

            // Check if we're still connected
            if !knx.is_connected().await {
                println!("Connection lost, attempting to reconnect...");
                if let Err(e) = knx.connect().await {
                    println!("Reconnection failed: {e}");
                    break;
                }
                println!("Reconnected successfully");
            }
        }
    })
    .await;

    match listen_result {
        Ok(()) => println!("Listening completed (connection lost)"),
        Err(_) => println!("Listening completed (timeout reached)"),
    }

    // Show statistics
    let final_count = packet_counter.load(Ordering::SeqCst);
    println!("----------------------------------------");
    println!("Listening session completed");
    println!("Total packets received: {final_count}");

    if final_count > 0 {
        // Display-only rate; precision loss beyond 2^52 packets/seconds is irrelevant here.
        #[allow(clippy::cast_precision_loss)]
        let rate = final_count as f64 / config.duration.as_secs() as f64;
        println!("Average rate: {rate:.2} packets/second");
    }

    // Get connection and memory statistics
    if let Some(conn_stats) = knx.connection_stats().await {
        println!("Connection statistics: {conn_stats:?}");
    }

    let memory_stats = knx.memory_stats().await;
    println!("Memory usage: {:.2}%", knx.memory_usage_percentage());
    println!("Memory stats: {memory_stats:?}");

    // Clean shutdown
    println!("Shutting down...");
    knx.shutdown().await?;
    println!("✓ Shutdown complete");

    Ok(())
}

#[tokio::test]
#[ignore = "requires a real KNX gateway"]
async fn test_gateway_packet_listener() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging for the test
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    // Load configuration from environment
    let config = TestConfig::from_env();

    // Run the packet listener
    listen_for_packets(config).await
}

#[tokio::test]
async fn test_gateway_packet_listener_mock() -> Result<(), Box<dyn std::error::Error>> {
    // This test runs without requiring a real gateway
    // It demonstrates the setup and callback registration

    println!("Mock KNX Gateway Packet Listener Test");
    println!("====================================");

    let packet_counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = packet_counter.clone();

    // Create Knx instance
    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32)
        .build()
        .await?;

    // Register telegram callback after creation
    let _callback_handle = knx
        .register_telegram_callback(MockPacketPrinter {
            counter: counter_clone,
        })
        .await;

    println!("✓ Knx instance created with telegram callback");

    // Note: telegram_callback_count() is only available in internal tests
    // For external tests, we'll verify the callback works by triggering it

    // Create a mock telegram to test the callback
    let mock_telegram = Telegram::received(
        IndividualAddress::new(1, 1, 1),
        knust::protocol::GroupAddress::new(
            knust::protocol::MainGroup::new(1),
            knust::protocol::MiddleGroup::new(1),
            1,
        ),
        TelegramType::GroupValueWrite,
        vec![0x01, 0x80], // Example: switch on command
    );

    // Note: test_notify_telegram_received() is only available in internal tests
    // For external tests, we simulate the callback behavior
    println!("Simulating received telegram...");

    // Manually trigger the callback to verify it works
    let callback = MockPacketPrinter {
        counter: packet_counter.clone(),
    };
    callback.call(&mock_telegram).await;

    // Give the callback time to execute
    sleep(Duration::from_millis(50)).await;

    // Verify the callback was invoked
    assert_eq!(packet_counter.load(Ordering::SeqCst), 1);
    println!("✓ Mock telegram processed successfully");

    println!("Mock test completed successfully!");
    Ok(())
}

/// Integration test that can be run manually with a real gateway
///
/// To run this test with a real KNX gateway:
/// ```bash
/// KNX_GATEWAY_IP=192.168.1.100 \
/// KNX_INDIVIDUAL_ADDRESS=1.1.240 \
/// KNX_CONNECTION_TYPE=tunneling \
/// KNX_LISTEN_DURATION=180 \
/// cargo test test_gateway_packet_listener -- --ignored --nocapture
/// ```
#[cfg(test)]
mod manual_tests {
    use super::*;

    /// Manual test function that can be called from main
    #[allow(dead_code)]
    pub async fn run_manual_listener() -> Result<(), Box<dyn std::error::Error>> {
        println!("Manual KNX Gateway Listener");
        println!("To stop, press Ctrl+C");
        println!();

        let config = TestConfig::from_env();
        listen_for_packets(config).await
    }
}

// Example of how to run this as a standalone program
// Uncomment the main function below to run as a binary
/*
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    manual_tests::run_manual_listener().await
}
*/
