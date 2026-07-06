//! Demonstration of sequence number validation in KNX/IP tunneling connections
//!
//! This example shows how the Knx library validates sequence numbers in tunneling
//! requests to ensure proper packet ordering and detect lost or duplicate frames.

use knust::protocol::knxip::{KnxIpFrame, ServiceType, TunnellingAck, TunnellingRequest};
use knust::transport::{SequenceValidationResult, Tunnel};
use std::net::SocketAddr;

// Linear connect -> demonstrate -> report walkthrough; splitting it up would
// add indirection without making the flow clearer.
#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("🔢 KNX/IP Sequence Number Validation Demo");
    println!("==========================================\n");

    // Create a tunneling connection (not actually connected for this demo)
    let gateway_addr: SocketAddr = "192.168.1.100:3671".parse()?;
    let connection = Tunnel::new_udp(gateway_addr);

    println!("📡 Created tunneling connection to {gateway_addr}");
    println!(
        "Initial expected sequence number: {}\n",
        connection.expected_sequence()
    );

    // Demonstrate valid sequence progression
    println!("✅ Testing valid sequence progression:");
    for i in 0..5 {
        let result = connection.validate_sequence_number(i);
        match result {
            SequenceValidationResult::Valid => {
                println!(
                    "  Sequence {}: ✓ Valid (expected: {})",
                    i,
                    connection.expected_sequence() - 1
                );
            }
            _ => {
                println!("  Sequence {i}: ❌ Unexpected result: {result:?}");
            }
        }
    }

    println!("\n🔄 Testing duplicate frame detection:");
    // Try to send sequence 4 again (duplicate)
    let result = connection.validate_sequence_number(4);
    match result {
        SequenceValidationResult::Duplicate => {
            println!("  Sequence 4 (duplicate): ✓ Correctly identified as duplicate");
        }
        _ => {
            println!("  Sequence 4 (duplicate): ❌ Unexpected result: {result:?}");
        }
    }

    println!("\n❌ Testing invalid sequence detection:");
    // Try to send sequence 7 (skipping 5 and 6)
    let result = connection.validate_sequence_number(7);
    match result {
        SequenceValidationResult::Invalid { expected, received } => {
            println!("  Sequence 7: ✓ Correctly identified as invalid");
            println!("    Expected: {expected}, Received: {received}");
        }
        _ => {
            println!("  Sequence 7: ❌ Unexpected result: {result:?}");
        }
    }

    println!("\n🔄 Continuing with correct sequence:");
    // Send the correct sequence numbers
    for i in 5..7 {
        let result = connection.validate_sequence_number(i);
        match result {
            SequenceValidationResult::Valid => {
                println!("  Sequence {i}: ✓ Valid");
            }
            _ => {
                println!("  Sequence {i}: ❌ Unexpected result: {result:?}");
            }
        }
    }

    println!("\n🔄 Testing sequence wraparound (255 -> 0):");
    // Reset and test wraparound
    connection.reset_sequence();

    // Advance to near wraparound
    for i in 0..256u16 {
        // Intentional wraparound: 0..256 covers the full u8 range and wraps at 256->0.
        #[allow(clippy::cast_possible_truncation)]
        let seq = i as u8;
        let result = connection.validate_sequence_number(seq);
        if i >= 253 || i <= 2 {
            match result {
                SequenceValidationResult::Valid => {
                    println!(
                        "  Sequence {}: ✓ Valid (expected next: {})",
                        seq,
                        connection.expected_sequence()
                    );
                }
                _ => {
                    println!("  Sequence {seq}: ❌ Unexpected result: {result:?}");
                }
            }
        }
    }

    println!("\n📦 Testing TunnellingRequest and TunnellingAck structures:");

    // Create a sample CEMI frame (simplified)
    let sample_cemi = vec![
        0x29, 0x00, 0xBC, 0xE0, // CEMI header
        0x11, 0x01, // Source address (1.1.1)
        0x01, 0x01, // Destination group address (1/0/1)
        0x00, 0x81, // APCI + data
    ];

    // Create TunnellingRequest
    let request = TunnellingRequest::new(1, 42, sample_cemi.clone());
    println!("  Created TunnellingRequest:");
    println!("    Channel ID: {}", request.communication_channel_id);
    println!("    Sequence: {}", request.sequence_counter);
    println!("    CEMI length: {} bytes", request.raw_cemi.len());

    // Serialize and create KNX/IP frame
    let request_body = request.serialize();
    let knx_frame = KnxIpFrame::new(ServiceType::TunnellingRequest, request_body);
    let frame_data = knx_frame.serialize();
    println!("    Total frame size: {} bytes", frame_data.len());

    // Create TunnellingAck responses
    let ack_ok = TunnellingAck::new_ok(1, 42);
    let ack_error = TunnellingAck::new_sequence_error(1, 42);

    println!("\n  TunnellingAck (OK):");
    println!("    Channel ID: {}", ack_ok.communication_channel_id);
    println!("    Sequence: {}", ack_ok.sequence_counter);
    println!(
        "    Status: 0x{:02X} ({})",
        ack_ok.status_code,
        ack_ok.status_description()
    );
    println!("    Success: {}", ack_ok.is_success());

    println!("\n  TunnellingAck (Sequence Error):");
    println!("    Channel ID: {}", ack_error.communication_channel_id);
    println!("    Sequence: {}", ack_error.sequence_counter);
    println!(
        "    Status: 0x{:02X} ({})",
        ack_error.status_code,
        ack_error.status_description()
    );
    println!("    Success: {}", ack_error.is_success());

    println!("\n🎯 Sequence validation ensures:");
    println!("  • Frames are processed in correct order");
    println!("  • Duplicate frames are acknowledged but not processed twice");
    println!("  • Lost frames are detected and appropriate errors are sent");
    println!("  • Sequence numbers wrap around correctly (255 -> 0)");
    println!("  • Protocol compliance with KNX/IP specification");

    println!("\n✨ Demo completed successfully!");

    Ok(())
}
