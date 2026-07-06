//! Example demonstrating KNX/IP tunneling connection functionality.

use knust::transport::connection::Connection;
use knust::transport::tunnel::Tunnel;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("KNX/IP Tunneling Connection Example");
    println!("===================================");

    // Configure gateway address (replace with your actual KNX/IP gateway)
    let gateway_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 3671);

    println!("Creating tunneling connection to {gateway_addr}...");

    // Create tunneling connection
    let mut connection = Tunnel::new_udp(gateway_addr);

    println!("Connection created successfully!");
    println!("Initial state: {:?}", connection.state());
    println!("Channel ID: {}", connection.channel_id());
    println!("Sequence counter: {}", connection.current_sequence());
    println!("Is connected: {}", connection.is_connected());

    // Display connection statistics
    let stats = connection.stats();
    println!("\nConnection Statistics:");
    println!("  Frames sent: {}", stats.frames_sent);
    println!("  Frames received: {}", stats.frames_received);
    println!("  Send errors: {}", stats.send_errors);
    println!("  Receive errors: {}", stats.recv_errors);
    println!("  Uptime: {} seconds", stats.uptime_seconds);

    // Attempt to connect (will likely fail without actual gateway)
    println!("\nAttempting to connect to gateway...");
    match connection.connect().await {
        Ok(()) => {
            println!("Connected successfully!");
            println!("Channel ID: {}", connection.channel_id());
            println!("Connection state: {:?}", connection.state());

            if let Some(uptime) = connection.uptime() {
                println!("Connection uptime: {uptime:?}");
            }

            // Demonstrate sequence counter management
            println!("\nSequence counter management:");
            println!("Current sequence: {}", connection.current_sequence());
            connection.reset_sequence();
            println!("After reset: {}", connection.current_sequence());

            // Close connection
            println!("\nClosing connection...");
            connection.close().await?;
            println!("Connection closed. Final state: {:?}", connection.state());
        }
        Err(e) => {
            println!("Connection failed (expected in demo environment): {e}");
            println!("Final state: {:?}", connection.state());
        }
    }

    println!("\nExample completed!");
    Ok(())
}
