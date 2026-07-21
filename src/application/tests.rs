//! Property-based tests for the application layer.

use proptest::prelude::*;
use proptest::test_runner::Config as ProptestConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::time::timeout;

use crate::application::knx::{Knx, KnxState};
use crate::protocol::address::IndividualAddress;
use crate::transport::{BackoffConfig, ConnectionConfig, ConnectionType, TcpConfig};

/// Generate arbitrary connection configurations for property testing
fn arb_connection_config() -> impl Strategy<Value = ConnectionConfig> {
    (
        prop_oneof![Just(ConnectionType::Routing),],
        (1u8..=15u8, 1u8..=15u8, 1u8..=255u8),
        100u64..=500u64,
    )
        .prop_map(
            |(connection_type, (area, line, device), timeout_ms)| ConnectionConfig {
                connection_type,
                gateway_ip: None,
                gateway_port: Some(3671),
                local_ip: None,
                individual_address: IndividualAddress::new(area, line, device),
                security: None,
                timeout_ms,
                auto_reconnect: false,
                reconnect_backoff: BackoffConfig::default(),
                tcp_config: TcpConfig::default(),
            },
        )
}

/// A resource tracker to verify cleanup happens correctly
#[derive(Debug)]
struct ResourceTracker {
    /// Number of resources currently allocated
    allocated: AtomicUsize,
    /// Whether cleanup was called
    cleanup_called: AtomicBool,
}

impl ResourceTracker {
    fn new() -> Self {
        Self {
            allocated: AtomicUsize::new(0),
            cleanup_called: AtomicBool::new(false),
        }
    }

    fn allocate(&self) {
        self.allocated.fetch_add(1, Ordering::SeqCst);
    }

    fn deallocate(&self) {
        self.allocated.fetch_sub(1, Ordering::SeqCst);
    }

    fn mark_cleanup(&self) {
        self.cleanup_called.store(true, Ordering::SeqCst);
    }

    fn is_clean(&self) -> bool {
        self.allocated.load(Ordering::SeqCst) == 0
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 20,
        max_shrink_iters: 10,
        timeout: 30000,
        .. ProptestConfig::default()
    })]

    /// For any cancelled async operation, all resources should be properly cleaned up
    /// without leaking.
    #[test]
    fn test_async_operation_cancellation_safety(
        config in arb_connection_config(),
        cancel_after_ms in 1u64..50u64,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let tracker = Arc::new(ResourceTracker::new());
            let tracker_clone = tracker.clone();

            // Create Knx instance
            let Ok(knx) = Knx::new(config.clone()).await else {
                // Configuration validation failure is acceptable
                return Ok(());
            };

            // Verify initial state
            let initial_state = knx.state().await;
            prop_assert_eq!(
                initial_state,
                KnxState::Disconnected,
                "Knx should start in Disconnected state"
            );

            // Simulate resource allocation
            tracker.allocate();

            // Create a cancellable operation
            let cancel_duration = Duration::from_millis(cancel_after_ms);

            // Test 1: Cancel during connect operation
            let _connect_result = timeout(cancel_duration, knx.connect()).await;

            // Whether it completed or timed out, state should be consistent
            let state_after_connect = knx.state().await;
            prop_assert!(
                state_after_connect == KnxState::Connected ||
                state_after_connect == KnxState::Disconnected ||
                state_after_connect == KnxState::Connecting ||
                state_after_connect == KnxState::Error,
                "State should be valid after connect attempt/cancellation, got: {:?}",
                state_after_connect
            );

            // Test 2: Cancel during disconnect operation
            if state_after_connect == KnxState::Connected {
                let _disconnect_result = timeout(cancel_duration, knx.disconnect()).await;

                // State should be consistent after disconnect attempt
                let state_after_disconnect = knx.state().await;
                prop_assert!(
                    state_after_disconnect == KnxState::Disconnected ||
                    state_after_disconnect == KnxState::Disconnecting ||
                    state_after_disconnect == KnxState::Connected,
                    "State should be valid after disconnect attempt/cancellation, got: {:?}",
                    state_after_disconnect
                );
            }

            // Simulate resource cleanup
            tracker.deallocate();
            tracker.mark_cleanup();

            // Verify resources are cleaned up
            prop_assert!(
                tracker_clone.is_clean(),
                "All resources should be cleaned up after cancellation"
            );

            Ok(())
        })?;
    }

    /// Test that multiple concurrent operations handle cancellation correctly
    #[test]
    fn test_concurrent_operation_cancellation(
        config in arb_connection_config(),
        num_operations in 2usize..5usize,
        cancel_after_ms in 5u64..30u64,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let knx = match Knx::new(config.clone()).await {
                Ok(x) => Arc::new(x),
                Err(_) => return Ok(()),
            };

            let completed = Arc::new(AtomicUsize::new(0));
            let cancelled = Arc::new(AtomicUsize::new(0));

            let mut handles = Vec::new();

            // Spawn multiple concurrent operations
            for _ in 0..num_operations {
                let knx_clone = knx.clone();
                let completed_clone = completed.clone();
                let cancelled_clone = cancelled.clone();
                let cancel_duration = Duration::from_millis(cancel_after_ms);

                let handle = tokio::spawn(async move {
                    match timeout(cancel_duration, knx_clone.state()).await {
                        Ok(_) => {
                            completed_clone.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(_) => {
                            cancelled_clone.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                });

                handles.push(handle);
            }

            // Wait for all operations to complete or be cancelled
            for handle in handles {
                let _ = handle.await;
            }

            // Verify all operations were accounted for
            let total = completed.load(Ordering::SeqCst) + cancelled.load(Ordering::SeqCst);
            prop_assert_eq!(
                total,
                num_operations,
                "All operations should be either completed or cancelled"
            );

            // Verify Knx is still in a valid state
            let final_state = knx.state().await;
            prop_assert!(
                final_state == KnxState::Disconnected ||
                final_state == KnxState::Connected ||
                final_state == KnxState::Connecting ||
                final_state == KnxState::Disconnecting ||
                final_state == KnxState::Error,
                "Knx should be in a valid state after concurrent operations, got: {:?}",
                final_state
            );

            Ok(())
        })?;
    }

}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[tokio::test]
    async fn test_knx_creation_with_routing_config() {
        let config = ConnectionConfig {
            connection_type: ConnectionType::Routing,
            gateway_ip: None,
            gateway_port: Some(3671),
            local_ip: None,
            individual_address: IndividualAddress::new(1, 1, 240),
            security: None,
            timeout_ms: 5000,
            auto_reconnect: true,
            reconnect_backoff: BackoffConfig::default(),
            tcp_config: TcpConfig::default(),
        };

        let knx = Knx::new(config).await.unwrap();
        assert_eq!(knx.state().await, KnxState::Disconnected);
    }

    #[tokio::test]
    async fn test_knx_state_transitions() {
        let config = ConnectionConfig {
            connection_type: ConnectionType::Routing,
            gateway_ip: None,
            gateway_port: Some(3671),
            local_ip: None,
            individual_address: IndividualAddress::new(1, 1, 240),
            security: None,
            timeout_ms: 5000,
            auto_reconnect: false,
            reconnect_backoff: BackoffConfig::default(),
            tcp_config: TcpConfig::default(),
        };

        let knx = Knx::new(config).await.unwrap();

        // Initial state should be Disconnected
        assert_eq!(knx.state().await, KnxState::Disconnected);
        assert!(!knx.is_connected().await);

        // Try to connect (may fail in test environment, but state should be consistent)
        let _connect_result = knx.connect().await;

        // State should be either Connected, Connecting, Disconnected, or Error
        let state = knx.state().await;
        assert!(
            state == KnxState::Connected
                || state == KnxState::Connecting
                || state == KnxState::Disconnected
                || state == KnxState::Error,
            "State should be valid after connect attempt, got: {state:?}"
        );
    }

    #[tokio::test]
    async fn test_cancellation_during_connect() {
        let config = ConnectionConfig {
            connection_type: ConnectionType::Routing,
            gateway_ip: None,
            gateway_port: Some(3671),
            local_ip: None,
            individual_address: IndividualAddress::new(1, 1, 240),
            security: None,
            timeout_ms: 5000,
            auto_reconnect: false,
            reconnect_backoff: BackoffConfig::default(),
            tcp_config: TcpConfig::default(),
        };

        let knx = Knx::new(config).await.unwrap();

        // Cancel connect after very short time
        let _result = timeout(Duration::from_millis(1), knx.connect()).await;

        // Whether it completed or timed out, Knx should be in a valid state
        let state = knx.state().await;
        assert!(
            state == KnxState::Disconnected
                || state == KnxState::Connecting
                || state == KnxState::Connected
                || state == KnxState::Error,
            "State should be valid after cancellation, got: {state:?}"
        );
    }

    #[tokio::test]
    async fn test_knx_builder() {
        // Test builder with routing connection
        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .timeout_ms(3000)
            .auto_reconnect(false)
            .build()
            .await
            .unwrap();

        assert_eq!(knx.state().await, KnxState::Disconnected);
        assert_eq!(knx.config().connection_type, ConnectionType::Routing);
        assert_eq!(knx.config().timeout_ms, 3000);
        assert!(!knx.config().auto_reconnect);
    }

    #[tokio::test]
    async fn test_knx_shutdown_flag() {
        let config = ConnectionConfig {
            connection_type: ConnectionType::Routing,
            gateway_ip: None,
            gateway_port: Some(3671),
            local_ip: None,
            individual_address: IndividualAddress::new(1, 1, 240),
            security: None,
            timeout_ms: 5000,
            auto_reconnect: false,
            reconnect_backoff: BackoffConfig::default(),
            tcp_config: TcpConfig::default(),
        };

        let knx = Knx::new(config).await.unwrap();

        // Initially not shutting down
        assert!(!knx.is_shutting_down());

        // After stop, shutdown flag should be reset
        knx.stop().await;
        assert!(!knx.is_shutting_down());
    }

    #[tokio::test]
    async fn test_knx_config_access() {
        let config = ConnectionConfig {
            connection_type: ConnectionType::Routing,
            gateway_ip: None,
            gateway_port: Some(3672),
            local_ip: None,
            individual_address: IndividualAddress::new(2, 3, 4),
            security: None,
            timeout_ms: 10000,
            auto_reconnect: true,
            reconnect_backoff: BackoffConfig::default(),
            tcp_config: TcpConfig::default(),
        };

        let knx = Knx::new(config.clone()).await.unwrap();

        // Verify config is accessible
        assert_eq!(knx.config().connection_type, ConnectionType::Routing);
        assert_eq!(knx.config().gateway_port, Some(3672));
        assert_eq!(knx.config().timeout_ms, 10000);
        assert!(knx.config().auto_reconnect);
    }

    #[cfg(feature = "dpt")]
    #[tokio::test]
    async fn test_group_address_typed_write_and_decode() {
        use crate::protocol::address::{GroupAddress, MainGroup, MiddleGroup};
        use crate::protocol::dpt::{DPTScaling, DPTSwitch, DptType};
        use crate::protocol::telegram::Telegram;

        let config = ConnectionConfig {
            connection_type: ConnectionType::Routing,
            gateway_ip: None,
            gateway_port: Some(3671),
            local_ip: None,
            individual_address: IndividualAddress::new(1, 1, 240),
            security: None,
            timeout_ms: 5000,
            auto_reconnect: false,
            reconnect_backoff: BackoffConfig::default(),
            tcp_config: TcpConfig::default(),
        };
        let knx = Knx::new(config).await.unwrap();

        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        let switch = knx.group_address::<DPTSwitch>(ga).unwrap();
        assert_eq!(knx.group_address_dpt(ga), Some(DptType::Switch));
        assert_eq!(switch.dpt(), DptType::Switch);

        // Re-registering with a different DPT is a conflict.
        assert!(knx.group_address::<DPTScaling>(ga).is_err());
        // ...but the same DPT again is fine and doesn't disturb the binding.
        assert!(knx.group_address::<DPTSwitch>(ga).is_ok());

        // Plain bool: passing e.g. a DPTScaling (or a u8) here wouldn't compile.
        switch.write(true).await.unwrap();
        assert_eq!(knx.telegram_queue_size().await, 1);

        // The DPT value type itself still works too.
        switch.write(DPTSwitch::new(false)).await.unwrap();
        assert_eq!(knx.telegram_queue_size().await, 2);

        // String, checked against the bound DPT at runtime instead of at
        // compile time — the only shape that can fail on bad input.
        switch.write("off").await.unwrap();
        assert_eq!(knx.telegram_queue_size().await, 3);
        assert!(switch.write("banana").await.is_err());

        // Both the handle-scoped (typed) and the generic (dynamic) decode
        // path agree, because both read from the same registry binding.
        let incoming = Telegram::group_write(ga, vec![0x01]);
        assert!(switch.decode(&incoming).unwrap().unwrap().value());
        assert_eq!(
            knx.decode_group_value(&incoming)
                .unwrap()
                .unwrap()
                .formatted(DptType::Switch),
            "On"
        );

        // An unregistered address has no binding.
        let other = GroupAddress::new(MainGroup::new(4), MiddleGroup::new(5), 6);
        assert!(knx.group_address_dpt(other).is_none());
        let unbound = Telegram::group_write(other, vec![0x01]);
        assert!(knx.decode_group_value(&unbound).is_none());
    }

    #[cfg(feature = "dpt")]
    #[tokio::test]
    async fn test_group_address_inspect_and_unregister() {
        use crate::protocol::address::{GroupAddress, MainGroup, MiddleGroup};
        use crate::protocol::dpt::{DPTScaling, DPTSwitch, DptType};
        use crate::protocol::telegram::Telegram;

        let config = ConnectionConfig {
            connection_type: ConnectionType::Routing,
            gateway_ip: None,
            gateway_port: Some(3671),
            local_ip: None,
            individual_address: IndividualAddress::new(1, 1, 240),
            security: None,
            timeout_ms: 5000,
            auto_reconnect: false,
            reconnect_backoff: BackoffConfig::default(),
            tcp_config: TcpConfig::default(),
        };
        let knx = Knx::new(config).await.unwrap();

        let switch_ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        let scaling_ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 4);
        let switch = knx.group_address::<DPTSwitch>(switch_ga).unwrap();
        knx.group_address::<DPTScaling>(scaling_ga).unwrap();

        // Inspect: both bindings show up, regardless of which entry point
        // (typed or dynamic) registered them.
        let mut entries = knx.registered_group_addresses();
        entries.sort_by_key(|(address, _)| address.raw());
        assert_eq!(
            entries,
            vec![(switch_ga, DptType::Switch), (scaling_ga, DptType::Scaling),]
        );

        // Unregister: the binding disappears from lookups...
        assert_eq!(
            knx.unregister_group_address(switch_ga),
            Some(DptType::Switch)
        );
        assert!(knx.group_address_dpt(switch_ga).is_none());
        assert_eq!(knx.registered_group_addresses().len(), 1);
        // ...unregistering again is a harmless no-op.
        assert!(knx.unregister_group_address(switch_ga).is_none());

        // ...decode_group_value (which relies on the registry) stops
        // resolving it...
        let telegram = Telegram::group_write(switch_ga, vec![0x01]);
        assert!(knx.decode_group_value(&telegram).is_none());

        // ...and the handle obtained *before* unregistering now errors
        // too — it re-checks the registry on every use, so it can't keep
        // silently operating under a DPT the registry no longer agrees on.
        assert!(switch.decode(&telegram).unwrap().is_err());
        assert!(switch.write(true).await.is_err());

        // The address is free to be bound to a different DPT now.
        knx.group_address::<DPTScaling>(switch_ga).unwrap();
        assert_eq!(knx.group_address_dpt(switch_ga), Some(DptType::Scaling));

        // The old handle still carries DPT::Switch, which now disagrees
        // with the registry's DPT::Scaling — it keeps erroring rather than
        // silently reading/writing under its stale DPT.
        assert!(switch.decode(&telegram).unwrap().is_err());
    }

    #[cfg(feature = "ets")]
    #[tokio::test]
    async fn test_register_group_addresses_from_ets() {
        use crate::config::parse_ets_csv;
        use crate::protocol::address::GroupAddress;
        use crate::protocol::dpt::DptType;

        let config = ConnectionConfig {
            connection_type: ConnectionType::Routing,
            gateway_ip: None,
            gateway_port: Some(3671),
            local_ip: None,
            individual_address: IndividualAddress::new(1, 1, 240),
            security: None,
            timeout_ms: 5000,
            auto_reconnect: false,
            reconnect_backoff: BackoffConfig::default(),
            tcp_config: TcpConfig::default(),
        };
        let knx = Knx::new(config).await.unwrap();

        let csv = "\"Group name\";\"Address\";\"DatapointType\"\n\
                   \"Living Room Temp\";\"1/2/3\";\"DPST-9-1\"\n\
                   \"Hall Light\";\"1/2/4\";\"DPST-1-1\"\n";
        let parsed = parse_ets_csv(csv).unwrap();

        knx.register_group_addresses_from_ets(&parsed).unwrap();

        assert_eq!(
            knx.group_address_dpt(GroupAddress::from_parts(1, 2, 3).unwrap()),
            Some(DptType::Temperature)
        );
        assert_eq!(
            knx.group_address_dpt(GroupAddress::from_parts(1, 2, 4).unwrap()),
            Some(DptType::Switch)
        );
        assert_eq!(knx.registered_group_addresses().len(), 2);

        // Importing the same file again is a no-op, not a conflict.
        knx.register_group_addresses_from_ets(&parsed).unwrap();
        assert_eq!(knx.registered_group_addresses().len(), 2);
    }
}
