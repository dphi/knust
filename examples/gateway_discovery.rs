//! Gateway discovery example using Knx.

use knust::transport::GatewayScanner;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), knust::KnxError> {
    // Initialize logging
    env_logger::init();

    println!("Starting KNX/IP gateway discovery...");

    // Create gateway scanner
    let scanner = GatewayScanner::new().await?;

    // Discover gateways with 10 second timeout
    let gateways = scanner.discover(Duration::from_secs(10)).await?;

    if gateways.is_empty() {
        println!("No KNX/IP gateways found on the network.");
        return Ok(());
    }

    println!("Found {} gateway(s):", gateways.len());
    println!();

    for (i, gateway) in gateways.iter().enumerate() {
        println!("Gateway {}:", i + 1);
        println!("  Name: {}", gateway.name);
        println!("  Address: {}", gateway.addr);
        println!("  Serial: {}", gateway.device_serial);

        if let Some(mac) = &gateway.mac_address {
            println!("  MAC Address: {mac}");
        }

        println!("  Capabilities:");
        println!("    Tunneling: {}", gateway.capabilities.supports_tunneling);
        println!("    Routing: {}", gateway.capabilities.supports_routing);
        println!(
            "    Device Management: {}",
            gateway.capabilities.supports_device_management
        );
        println!(
            "    Max Tunneling Connections: {}",
            gateway.capabilities.max_tunneling_connections
        );

        println!("  Supported Services:");
        for service in &gateway.supported_services {
            println!("    {service:?}");
        }

        if let Some(multicast) = &gateway.multicast_addr {
            println!("  Multicast Address: {multicast}");
        }

        println!();
    }

    // Show connection recommendations
    println!("Connection Recommendations:");

    let tunneling_gateways: Vec<_> = gateways
        .iter()
        .filter(|g| g.capabilities.supports_tunneling)
        .collect();

    let routing_gateways: Vec<_> = gateways
        .iter()
        .filter(|g| g.capabilities.supports_routing)
        .collect();

    if !tunneling_gateways.is_empty() {
        println!("  For Tunneling connections, use:");
        for gateway in tunneling_gateways {
            println!("    {} ({})", gateway.name, gateway.addr);
        }
    }

    if !routing_gateways.is_empty() {
        println!("  For Routing connections, use:");
        for gateway in routing_gateways {
            println!("    {} ({})", gateway.name, gateway.addr);
        }
    }

    println!("\nDiscovery completed successfully!");
    Ok(())
}
