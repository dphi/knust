//! Memory optimization and performance monitoring demonstration.
//!
//! This example shows how to use Knx's memory management and performance
//! optimization features to monitor resource usage and optimize performance.

use knust::{Component, ConnectionType, Knx, LogLevel, LoggingConfig};
use std::time::Duration;
use tokio::time::sleep;

// Linear connect -> demonstrate -> report walkthrough; splitting it up would
// add indirection without making the flow clearer.
#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::init();

    println!("🚀 Knx Memory Optimization Demo");
    println!("==================================");

    // Configure logging for memory and performance monitoring
    let mut logging_config = LoggingConfig::new();
    logging_config.set_component_level(Component::Application, LogLevel::Info);
    logging_config.set_component_level(Component::Transport, LogLevel::Debug);
    logging_config.set_component_level(Component::Protocol, LogLevel::Debug);
    logging_config.set_protocol_events(true);

    // Create Knx instance with memory management
    let knx = Knx::builder()
        .connection_type(ConnectionType::Routing)
        .memory_limit_mb(32) // Set 32MB memory limit
        .max_connections(5) // Limit connection pool to 5
        .build()
        .await?;

    println!("✅ Knx instance created with memory management");

    // Display initial memory statistics
    let initial_stats = knx.memory_stats().await;
    println!("\n📊 Initial Memory Statistics:");
    println!("   Current usage: {} bytes", initial_stats.current_usage);
    println!("   Peak usage: {} bytes", initial_stats.peak_usage);
    println!("   Memory limit: {}%", knx.memory_usage_percentage());
    println!("   Within bounds: {}", knx.memory_within_bounds());

    // Simulate some operations to generate performance data
    println!("\n⚡ Simulating operations for performance monitoring...");

    // Try to connect (this will likely fail without a real KNX gateway, but will generate metrics)
    match knx.connect().await {
        Ok(()) => {
            println!("✅ Connected to KNX network");

            // Start telegram processing
            if let Err(e) = knx.start().await {
                println!("⚠️  Failed to start telegram processing: {e}");
            }

            // Let it run for a bit to collect performance data
            sleep(Duration::from_secs(2)).await;
        }
        Err(e) => {
            println!("⚠️  Connection failed (expected without real gateway): {e}");
        }
    }

    // Force memory cleanup to demonstrate the feature
    println!("\n🧹 Performing memory cleanup...");
    let freed_bytes = knx.force_cleanup().await;
    println!("   Freed {freed_bytes} bytes");

    // Display updated memory statistics
    let updated_stats = knx.memory_stats().await;
    println!("\n📊 Updated Memory Statistics:");
    println!("   Current usage: {} bytes", updated_stats.current_usage);
    println!("   Peak usage: {} bytes", updated_stats.peak_usage);
    println!("   Memory limit: {}%", knx.memory_usage_percentage());
    println!("   Within bounds: {}", knx.memory_within_bounds());

    // Display component-specific memory usage
    println!("\n🔍 Memory Usage by Component:");
    println!(
        "   Transport: {} bytes",
        updated_stats.component_usage.transport
    );
    println!(
        "   Protocol: {} bytes",
        updated_stats.component_usage.protocol
    );
    println!("   Device: {} bytes", updated_stats.component_usage.device);
    println!(
        "   Application: {} bytes",
        updated_stats.component_usage.application
    );
    println!(
        "   Security: {} bytes",
        updated_stats.component_usage.security
    );

    // Display performance statistics
    let perf_stats = knx.performance_stats().await;
    println!("\n⚡ Performance Statistics:");
    if perf_stats.paths.is_empty() {
        println!("   No performance data collected yet");
    } else {
        for (path, entry) in &perf_stats.paths {
            println!(
                "   {}: {} calls, avg {:?}, min {:?}, max {:?}",
                path, entry.call_count, entry.avg_duration, entry.min_duration, entry.max_duration
            );
        }
    }

    // Demonstrate memory monitoring with artificial allocations
    println!("\n🧪 Testing memory allocation limits...");

    // This would normally be done internally by the library
    // but we'll demonstrate the monitoring capabilities
    let memory_monitor = knx.memory_stats().await;
    println!(
        "   Current memory usage: {} bytes",
        memory_monitor.current_usage
    );

    // Test memory bounds checking
    if knx.memory_within_bounds() {
        println!("   ✅ Memory usage is within acceptable bounds");
    } else {
        println!("   ⚠️  Memory usage exceeds bounds!");
    }

    // Clean shutdown
    println!("\n🛑 Shutting down Knx...");
    knx.shutdown().await?;

    // Display final memory statistics after shutdown
    let final_stats = knx.memory_stats().await;
    println!("\n📊 Final Memory Statistics (after shutdown):");
    println!("   Current usage: {} bytes", final_stats.current_usage);
    println!("   Peak usage: {} bytes", final_stats.peak_usage);
    println!("   Memory usage: {}%", knx.memory_usage_percentage());

    println!("\n✅ Memory optimization demo completed successfully!");
    println!("\n💡 Key Features Demonstrated:");
    println!("   • Memory usage monitoring and limits");
    println!("   • Connection pool management");
    println!("   • Performance hot path tracking");
    println!("   • Automatic memory cleanup");
    println!("   • Component-specific resource tracking");
    println!("   • Graceful resource cleanup on shutdown");

    Ok(())
}
