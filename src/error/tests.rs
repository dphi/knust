//! Property-based tests for error handling.

#[cfg(test)]
mod error_tests {
    use crate::error::*;
    use proptest::prelude::*;

    /// For any operation that fails with invalid input, the error should contain
    /// sufficient context to identify the failure cause and input validation details.
    #[test]
    fn property_error_context_completeness() {
        proptest!(|(
            timeout_ms in 0u64..1000u64,
        )| {
            // Test timeout errors contain context
            let transport_error = TransportError::Timeout { timeout_ms };
            let knx_error = KnxError::Transport(transport_error);
            let context = knx_error.context();
            prop_assert!(context.contains("timeout") || context.contains("Timeout"));
            prop_assert!(context.contains(&timeout_ms.to_string()));

            // Test error categorization
            prop_assert_eq!(knx_error.category(), "transport");

            // Test error recoverability
            let is_recoverable = knx_error.is_recoverable();
            prop_assert_eq!(is_recoverable, true); // Timeout errors should be recoverable
        });
    }

    #[test]
    fn test_error_hierarchy() {
        // Test that all error types can be converted to KnxError
        let transport_error = TransportError::ConnectionClosed;
        let knx_error: KnxError = transport_error.into();
        assert_eq!(knx_error.category(), "transport");

        let protocol_error = ProtocolError::InvalidFrame {
            details: "Test frame".to_string(),
        };
        let knx_error: KnxError = protocol_error.into();
        assert_eq!(knx_error.category(), "protocol");

        let device_error = DeviceError::CommunicationTimeout {
            device: "group address 1/2/3".to_string(),
            timeout_ms: 1000,
        };
        let knx_error: KnxError = device_error.into();
        assert_eq!(knx_error.category(), "device");
    }

    #[test]
    fn test_error_display() {
        let error = KnxError::Transport(TransportError::ConnectionFailed {
            address: "192.168.1.100:3671".to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "Connection refused",
            ),
        });

        let error_string = error.to_string();
        assert!(error_string.contains("Transport error"));
        assert!(error_string.contains("192.168.1.100:3671"));
    }

    #[test]
    fn test_recoverable_errors() {
        // Timeout errors should be recoverable
        let timeout_error = KnxError::Transport(TransportError::Timeout { timeout_ms: 5000 });
        assert!(timeout_error.is_recoverable());

        // Connection closed should be recoverable
        let closed_error = KnxError::Transport(TransportError::ConnectionClosed);
        assert!(closed_error.is_recoverable());

        // Session expired should be recoverable
        let session_error = KnxError::Security(SecurityError::SessionExpired);
        assert!(session_error.is_recoverable());

        // Parse errors should not be recoverable
        let parse_error = KnxError::Protocol(ProtocolError::ParseError {
            offset: 0,
            reason: "Invalid data".to_string(),
        });
        assert!(!parse_error.is_recoverable());
    }
}
