//! KNX Gateway Packet Listener Example
//!
//! This example connects to a KNX gateway and listens for incoming packets,
//! printing them to the console for up to 3 minutes (or until Ctrl+C).
//!
//! Usage:
//! ```bash
//! # With default settings (routing connection)
//! cargo run --example packet_listener
//!
//! # With specific gateway (tunneling connection)
//! KNX_GATEWAY_IP=192.168.1.100 cargo run --example packet_listener
//!
//! # With custom settings
//! KNX_GATEWAY_IP=192.168.1.100 \
//! KNX_CONNECTION_TYPE=tunneling \
//! KNX_INDIVIDUAL_ADDRESS=1.1.240 \
//! KNX_LISTEN_DURATION=300 \
//! cargo run --example packet_listener
//! ```

use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use tokio::signal;
use tokio::time::{sleep, timeout};

use knust::application::callbacks::TelegramCallbackFn;
use knust::protocol::address::IndividualAddress;
use knust::protocol::telegram::Telegram;
use knust::{ConnectionConfig, ConnectionType, Knx};

/// Configuration loaded from environment variables
struct Config {
    gateway_ip: Option<std::net::IpAddr>,
    individual_address: IndividualAddress,
    connection_type: ConnectionType,
    duration: Duration,
}

impl Config {
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
            Ok("tunneling") => ConnectionType::Tunneling,
            _ => {
                if gateway_ip.is_some() {
                    ConnectionType::Tunneling // Default to tunneling if gateway specified
                } else {
                    ConnectionType::Routing // Default to routing if no gateway
                }
            }
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

/// Packet printer that implements the `TelegramCallbackFn` trait
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

/// Print usage information
fn print_usage() {
    println!("KNX Gateway Packet Listener");
    println!("===========================");
    println!();
    println!("This example connects to a KNX gateway and listens for packets.");
    println!();
    println!("Environment Variables:");
    println!("  KNX_GATEWAY_IP         Gateway IP address (e.g., 192.168.1.100)");
    println!("                         Default: 192.168.33.49 (from .env.test)");
    println!("                         Not required for routing connections");
    println!("  KNX_CONNECTION_TYPE    Connection type: routing, tunneling, tcp_tunneling");
    println!("                         Default: tunneling (if gateway), routing (if no gateway)");
    println!("  KNX_INDIVIDUAL_ADDRESS Individual address (e.g., 1.1.240)");
    println!("  KNX_LISTEN_DURATION    Listen duration in seconds (default: 180)");
    println!();
    println!("Connection Types:");
    println!("  routing       - Multicast connection (no gateway IP needed)");
    println!("  tunneling     - UDP point-to-point (requires gateway IP)");
    println!("  tcp_tunneling - TCP point-to-point (requires gateway IP)");
    println!();
    println!("Examples:");
    println!("  # RECOMMENDED: Use routing connection (most stable)");
    println!("  KNX_CONNECTION_TYPE=routing cargo run --example packet_listener");
    println!();
    println!("  # Listen with default gateway from .env.test (192.168.33.49) - may disconnect");
    println!("  cargo run --example packet_listener");
    println!();
    println!("  # Listen for 2 minutes with routing connection");
    println!(
        "  KNX_CONNECTION_TYPE=routing KNX_LISTEN_DURATION=120 cargo run --example packet_listener"
    );
    println!();
    println!("  # Try tunneling with different gateway (may have connection issues)");
    println!("  KNX_GATEWAY_IP=192.168.1.100 cargo run --example packet_listener");
    println!();
    println!("  # Custom configuration with tunneling");
    println!("  KNX_GATEWAY_IP=192.168.1.100 \\");
    println!("  KNX_CONNECTION_TYPE=tunneling \\");
    println!("  KNX_INDIVIDUAL_ADDRESS=1.1.240 \\");
    println!("  KNX_LISTEN_DURATION=300 \\");
    println!("  cargo run --example packet_listener");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::init();

    // Check for help flag
    if env::args().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(());
    }

    // Load configuration
    let config = Config::from_env();

    println!("KNX Gateway Packet Listener");
    println!("===========================");
    println!("Configuration:");

    if config.connection_type == ConnectionType::Routing {
        println!("  Connection Type: Routing (multicast)");
        println!("  Gateway IP: Not required for routing");
    } else {
        println!("  Connection Type: {:?}", config.connection_type);
        println!("  Gateway IP: {:?}", config.gateway_ip);
    }

    println!("  Individual Address: {}", config.individual_address);
    println!("  Listen Duration: {:?}", config.duration);
    println!();

    // Validate configuration
    if matches!(
        config.connection_type,
        ConnectionType::Tunneling | ConnectionType::TcpTunneling
    ) && config.gateway_ip.is_none()
    {
        eprintln!("Error: Gateway IP is required for tunneling connections");
        eprintln!("Set KNX_GATEWAY_IP environment variable or use routing connection");
        return Ok(());
    }

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

    // Create Knx instance with proper configuration
    let mut builder = Knx::builder()
        .connection_type(config.connection_type)
        .individual_address(config.individual_address)
        .timeout_ms(connection_config.timeout_ms)
        .auto_reconnect(connection_config.auto_reconnect);

    // Add gateway IP if provided
    if let Some(gateway_ip) = config.gateway_ip {
        builder = builder.gateway_ip(gateway_ip);
    }

    let knx = builder.build().await?;

    // Use the configured instance
    run_listener(knx, config, packet_counter).await
}

// Linear connect -> listen -> report walkthrough; splitting it up would add
// indirection without making the flow clearer.
#[allow(clippy::too_many_lines)]
async fn run_listener(
    knx: Knx,
    config: Config,
    packet_counter: Arc<AtomicUsize>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Register telegram callback
    let _callback_handle = knx
        .register_telegram_callback(PacketPrinter {
            counter: packet_counter.clone(),
        })
        .await;

    println!("Knx instance created successfully");

    // Set up Ctrl+C handler
    let shutdown_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_flag_clone = shutdown_flag.clone();

    tokio::spawn(async move {
        signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        println!("\nReceived Ctrl+C, shutting down...");
        shutdown_flag_clone.store(true, Ordering::SeqCst);
    });

    // Attempt to connect to the gateway
    println!("Connecting to KNX gateway...");
    match knx.connect().await {
        Ok(()) => {
            println!("✓ Successfully connected to KNX gateway");
        }
        Err(e) => {
            println!("✗ Failed to connect to KNX gateway: {e}");
            println!("  This might be expected if no gateway is available");

            match config.connection_type {
                ConnectionType::Routing => {
                    println!(
                        "  For routing connections, ensure multicast is enabled on your network"
                    );
                    println!("  Routing uses multicast address 224.0.23.12:3671");
                }
                ConnectionType::Tunneling | ConnectionType::TcpTunneling => {
                    println!(
                        "  For tunneling connections, ensure the gateway IP is correct and reachable"
                    );
                    if let Some(ip) = config.gateway_ip {
                        println!("  Trying to connect to: {ip}");
                    }
                    println!("  Common issues:");
                    println!("    - Gateway may limit concurrent tunneling connections");
                    println!("    - Firewall blocking UDP port 3671");
                    println!("    - Gateway may require authentication");
                    println!("    - Gateway may not support tunneling");
                    println!("  Try using routing connection instead:");
                    println!("    KNX_CONNECTION_TYPE=routing cargo run --example packet_listener");
                }
                _ => {}
            }

            return Ok(());
        }
    }

    // Verify connection is still active before starting telegram processing
    if !knx.is_connected().await {
        println!("✗ Connection lost immediately after establishment");
        println!("  This suggests the gateway rejected the connection");
        match config.connection_type {
            ConnectionType::Tunneling | ConnectionType::TcpTunneling => {
                println!("  Try using routing connection instead:");
                println!("    KNX_CONNECTION_TYPE=routing cargo run --example packet_listener");
            }
            _ => {}
        }
        return Ok(());
    }

    // Start telegram processing
    println!("Starting telegram processing...");

    // Add a small delay to ensure connection is fully established
    sleep(Duration::from_millis(200)).await;

    // Check connection again before starting processing
    if !knx.is_connected().await {
        println!("✗ Connection lost before starting telegram processing");
        println!("  The gateway closed the connection immediately");
        match config.connection_type {
            ConnectionType::Tunneling | ConnectionType::TcpTunneling => {
                println!("  This is common with KNX tunneling connections");
                println!("  Try using routing connection instead:");
                println!("    KNX_CONNECTION_TYPE=routing cargo run --example packet_listener");
            }
            _ => {}
        }
        return Ok(());
    }

    match knx.start().await {
        Ok(()) => {
            println!("✓ Telegram processing started");
        }
        Err(e) => {
            println!("✗ Failed to start telegram processing: {e}");
            println!("  This might indicate a connection issue");
            return Ok(());
        }
    }

    println!();
    println!("Listening for KNX packets for {:?}...", config.duration);
    println!("Press Ctrl+C to stop early");
    println!("----------------------------------------");

    // Listen for the specified duration or until Ctrl+C
    let listen_result = timeout(config.duration, async {
        const MAX_RECONNECT_ATTEMPTS: u32 = 5;
        let mut reconnect_attempts = 0;

        loop {
            // Check for shutdown signal
            if shutdown_flag.load(Ordering::SeqCst) {
                println!("Shutdown requested by user");
                break;
            }

            sleep(Duration::from_millis(100)).await;

            // Check if we're still connected
            if !knx.is_connected().await {
                reconnect_attempts += 1;
                println!(
                    "Connection lost (attempt {reconnect_attempts}/{MAX_RECONNECT_ATTEMPTS}), attempting to reconnect..."
                );

                if reconnect_attempts > MAX_RECONNECT_ATTEMPTS {
                    println!("Maximum reconnection attempts reached, giving up");
                    break;
                }

                match knx.connect().await {
                    Ok(()) => {
                        println!("✓ Reconnected successfully");
                        reconnect_attempts = 0; // Reset counter on successful reconnection

                        // Restart telegram processing after reconnection
                        if let Err(e) = knx.start().await {
                            println!("Failed to restart telegram processing: {e}");
                            break;
                        }
                    }
                    Err(e) => {
                        println!("Reconnection failed: {e}");
                        // Wait a bit before next attempt
                        sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
    })
    .await;

    match listen_result {
        Ok(()) => println!("Listening completed (shutdown requested or connection lost)"),
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
    } else {
        println!("No packets received. This could mean:");
        println!("  - No KNX traffic on the network during the listening period");
        println!("  - Connection issues (check logs above for errors)");
        println!("  - Network configuration problems");

        match config.connection_type {
            ConnectionType::Tunneling | ConnectionType::TcpTunneling => {
                println!("  - KNX gateway may have closed the tunneling connection");
                println!("    (this is common - gateways often limit connection duration)");
                println!("  - Try using routing connection for longer monitoring:");
                println!("    KNX_CONNECTION_TYPE=routing cargo run --example packet_listener");
            }
            _ => {}
        }
    }

    // Get connection and memory statistics if available
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
