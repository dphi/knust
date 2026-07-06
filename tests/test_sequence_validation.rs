//! Tests for sequence number validation in tunneling connections

use knust::protocol::knxip::{KnxIpFrame, ServiceType, TunnellingAck, TunnellingRequest};
use knust::transport::{SequenceValidationResult, Tunnel};
use std::net::SocketAddr;

#[tokio::test]
async fn test_sequence_validation_valid_sequence() {
    // Create a tunneling connection
    let gateway_addr: SocketAddr = "127.0.0.1:3671".parse().unwrap();
    let connection = Tunnel::new_udp(gateway_addr);

    // Test valid sequence numbers (0, 1, 2, ...)
    assert_eq!(
        connection.validate_sequence_number(0),
        SequenceValidationResult::Valid
    );
    assert_eq!(connection.expected_sequence(), 1);

    assert_eq!(
        connection.validate_sequence_number(1),
        SequenceValidationResult::Valid
    );
    assert_eq!(connection.expected_sequence(), 2);

    assert_eq!(
        connection.validate_sequence_number(2),
        SequenceValidationResult::Valid
    );
    assert_eq!(connection.expected_sequence(), 3);
}

#[tokio::test]
async fn test_sequence_validation_duplicate_sequence() {
    let gateway_addr: SocketAddr = "127.0.0.1:3671".parse().unwrap();
    let connection = Tunnel::new_udp(gateway_addr);

    // Process first frame (sequence 0)
    assert_eq!(
        connection.validate_sequence_number(0),
        SequenceValidationResult::Valid
    );
    assert_eq!(connection.expected_sequence(), 1);

    // Receive duplicate frame (sequence 0 again) - should be marked as duplicate
    assert_eq!(
        connection.validate_sequence_number(0),
        SequenceValidationResult::Duplicate
    );
    assert_eq!(connection.expected_sequence(), 1); // Expected sequence shouldn't change

    // Process next valid frame (sequence 1)
    assert_eq!(
        connection.validate_sequence_number(1),
        SequenceValidationResult::Valid
    );
    assert_eq!(connection.expected_sequence(), 2);
}

#[tokio::test]
async fn test_sequence_validation_invalid_sequence() {
    let gateway_addr: SocketAddr = "127.0.0.1:3671".parse().unwrap();
    let connection = Tunnel::new_udp(gateway_addr);

    // Process first frame (sequence 0)
    assert_eq!(
        connection.validate_sequence_number(0),
        SequenceValidationResult::Valid
    );
    assert_eq!(connection.expected_sequence(), 1);

    // Receive frame with wrong sequence number (skipping sequence 1)
    match connection.validate_sequence_number(2) {
        SequenceValidationResult::Invalid { expected, received } => {
            assert_eq!(expected, 1);
            assert_eq!(received, 2);
        }
        _ => panic!("Expected Invalid result"),
    }
    assert_eq!(connection.expected_sequence(), 1); // Expected sequence shouldn't change
}

#[tokio::test]
async fn test_sequence_validation_wraparound() {
    let gateway_addr: SocketAddr = "127.0.0.1:3671".parse().unwrap();
    let connection = Tunnel::new_udp(gateway_addr);

    // Set sequence to near wraparound (254, 255, 0)
    // First, advance to sequence 254
    for i in 0..255 {
        assert_eq!(
            connection.validate_sequence_number(i),
            SequenceValidationResult::Valid
        );
    }
    assert_eq!(connection.expected_sequence(), 255);

    // Process sequence 255
    assert_eq!(
        connection.validate_sequence_number(255),
        SequenceValidationResult::Valid
    );
    assert_eq!(connection.expected_sequence(), 0); // Should wrap around to 0

    // Process sequence 0 (after wraparound)
    assert_eq!(
        connection.validate_sequence_number(0),
        SequenceValidationResult::Valid
    );
    assert_eq!(connection.expected_sequence(), 1);
}

#[tokio::test]
async fn test_sequence_validation_duplicate_after_wraparound() {
    let gateway_addr: SocketAddr = "127.0.0.1:3671".parse().unwrap();
    let connection = Tunnel::new_udp(gateway_addr);

    // Advance to sequence 255
    for i in 0..256u16 {
        // Intentional wraparound: 0..256 covers the full u8 range and wraps at 256->0.
        #[allow(clippy::cast_possible_truncation)]
        let seq = i as u8;
        assert_eq!(
            connection.validate_sequence_number(seq),
            SequenceValidationResult::Valid
        );
    }
    assert_eq!(connection.expected_sequence(), 0); // Wrapped around

    // Receive duplicate of sequence 255 (one less than expected 0)
    assert_eq!(
        connection.validate_sequence_number(255),
        SequenceValidationResult::Duplicate
    );
    assert_eq!(connection.expected_sequence(), 0); // Should remain 0
}

#[tokio::test]
async fn test_sequence_reset() {
    let gateway_addr: SocketAddr = "127.0.0.1:3671".parse().unwrap();
    let connection = Tunnel::new_udp(gateway_addr);

    // Process some frames
    for i in 0..5 {
        assert_eq!(
            connection.validate_sequence_number(i),
            SequenceValidationResult::Valid
        );
    }
    assert_eq!(connection.expected_sequence(), 5);

    // Reset sequence counters
    connection.reset_sequence();
    assert_eq!(connection.current_sequence(), 0);
    assert_eq!(connection.expected_sequence(), 0);

    // Should accept sequence 0 again after reset
    assert_eq!(
        connection.validate_sequence_number(0),
        SequenceValidationResult::Valid
    );
    assert_eq!(connection.expected_sequence(), 1);
}

#[tokio::test]
async fn test_tunnelling_request_parsing() {
    // Test TunnellingRequest parsing
    let raw_cemi = vec![0x29, 0x00, 0xBC, 0xE0, 0x11, 0x01, 0x01, 0x00, 0x81];
    let request = TunnellingRequest::new(1, 5, raw_cemi.clone());

    // Serialize and parse back
    let serialized = request.serialize();
    let parsed = TunnellingRequest::parse(&serialized).unwrap();

    assert_eq!(parsed.communication_channel_id, 1);
    assert_eq!(parsed.sequence_counter, 5);
    assert_eq!(parsed.raw_cemi, raw_cemi);
}

#[tokio::test]
async fn test_tunnelling_ack_parsing() {
    // Test TunnellingAck parsing
    let ack = TunnellingAck::new_ok(1, 5);

    // Serialize and parse back
    let serialized = ack.serialize();
    let parsed = TunnellingAck::parse(&serialized).unwrap();

    assert_eq!(parsed.communication_channel_id, 1);
    assert_eq!(parsed.sequence_counter, 5);
    assert_eq!(parsed.status_code, TunnellingAck::STATUS_OK);
    assert!(parsed.is_success());

    // Test error ACK
    let error_ack = TunnellingAck::new_sequence_error(2, 10);
    let error_serialized = error_ack.serialize();
    let error_parsed = TunnellingAck::parse(&error_serialized).unwrap();

    assert_eq!(error_parsed.communication_channel_id, 2);
    assert_eq!(error_parsed.sequence_counter, 10);
    assert_eq!(
        error_parsed.status_code,
        TunnellingAck::STATUS_ERROR_SEQUENCE_NUMBER
    );
    assert!(!error_parsed.is_success());
}

#[tokio::test]
async fn test_knxip_frame_with_tunnelling_request() {
    // Test complete KNX/IP frame with TunnellingRequest
    let raw_cemi = vec![0x29, 0x00, 0xBC, 0xE0, 0x11, 0x01, 0x01, 0x00, 0x81];
    let request = TunnellingRequest::new(1, 5, raw_cemi.clone());
    let request_body = request.serialize();
    let frame = KnxIpFrame::new(ServiceType::TunnellingRequest, request_body);

    // Serialize and parse back
    let frame_data = frame.serialize();
    let parsed_frame = KnxIpFrame::parse(&frame_data).unwrap();

    assert_eq!(
        parsed_frame.header.service_type,
        ServiceType::TunnellingRequest
    );

    // Parse the TunnellingRequest from the frame body
    let parsed_request = TunnellingRequest::parse(&parsed_frame.body).unwrap();
    assert_eq!(parsed_request.communication_channel_id, 1);
    assert_eq!(parsed_request.sequence_counter, 5);
    assert_eq!(parsed_request.raw_cemi, raw_cemi);
}

#[tokio::test]
async fn test_knxip_frame_with_tunnelling_ack() {
    // Test complete KNX/IP frame with TunnellingAck
    let ack = TunnellingAck::new_ok(1, 5);
    let ack_body = ack.serialize();
    let frame = KnxIpFrame::new(ServiceType::TunnellingAck, ack_body);

    // Serialize and parse back
    let frame_data = frame.serialize();
    let parsed_frame = KnxIpFrame::parse(&frame_data).unwrap();

    assert_eq!(parsed_frame.header.service_type, ServiceType::TunnellingAck);

    // Parse the TunnellingAck from the frame body
    let parsed_ack = TunnellingAck::parse(&parsed_frame.body).unwrap();
    assert_eq!(parsed_ack.communication_channel_id, 1);
    assert_eq!(parsed_ack.sequence_counter, 5);
    assert_eq!(parsed_ack.status_code, TunnellingAck::STATUS_OK);
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn property_sequence_validation_consistency(
            sequences in prop::collection::vec(any::<u8>(), 1..100)
        ) {
            let _ = tokio_test::block_on(async {
                let gateway_addr: SocketAddr = "127.0.0.1:3671".parse().unwrap();
                let connection = Tunnel::new_udp(gateway_addr);

                let mut expected = 0u8;
                for &seq in &sequences {
                    let result = connection.validate_sequence_number(seq);

                    match result {
                        SequenceValidationResult::Valid => {
                            // Should only be valid if sequence matches expected
                            prop_assert_eq!(seq, expected);
                            expected = expected.wrapping_add(1);
                        }
                        SequenceValidationResult::Duplicate => {
                            // Should only be duplicate if sequence is one less than expected
                            prop_assert_eq!(seq, expected.wrapping_sub(1));
                        }
                        SequenceValidationResult::Invalid { expected: exp, received } => {
                            // Should be invalid if sequence doesn't match expected or expected-1
                            prop_assert_eq!(exp, expected);
                            prop_assert_eq!(received, seq);
                            prop_assert_ne!(seq, expected);
                            prop_assert_ne!(seq, expected.wrapping_sub(1));
                        }
                    }
                }
                Ok(())
            });
        }

        #[test]
        fn property_tunnelling_request_roundtrip(
            channel_id in any::<u8>(),
            sequence in any::<u8>(),
            cemi_data in prop::collection::vec(any::<u8>(), 0..100)
        ) {
            let request = TunnellingRequest::new(channel_id, sequence, cemi_data.clone());
            let serialized = request.serialize();
            let parsed = TunnellingRequest::parse(&serialized).unwrap();

            prop_assert_eq!(parsed.communication_channel_id, channel_id);
            prop_assert_eq!(parsed.sequence_counter, sequence);
            prop_assert_eq!(parsed.raw_cemi, cemi_data);
        }

        #[test]
        fn property_tunnelling_ack_roundtrip(
            channel_id in any::<u8>(),
            sequence in any::<u8>(),
            status in any::<u8>()
        ) {
            let ack = TunnellingAck::new(channel_id, sequence, status);
            let serialized = ack.serialize();
            let parsed = TunnellingAck::parse(&serialized).unwrap();

            prop_assert_eq!(parsed.communication_channel_id, channel_id);
            prop_assert_eq!(parsed.sequence_counter, sequence);
            prop_assert_eq!(parsed.status_code, status);
        }
    }
}
