//! Thread safety tests for the callback system
//!
//! These tests verify that the callback system works correctly under concurrent access
//! from multiple threads, which is a critical requirement for the Knx library.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::time::sleep;

use crate::application::callbacks::{ConnectionState, EventHandler, TelegramFilter};
use crate::protocol::{
    address::{Address, GroupAddress, IndividualAddress, MainGroup, MiddleGroup},
    telegram::{Direction, Priority, Telegram, TelegramType},
};

#[tokio::test]
async fn test_concurrent_callback_registration() {
    let handler = Arc::new(EventHandler::new());
    let registration_count = Arc::new(AtomicUsize::new(0));

    // Spawn multiple tasks that register callbacks concurrently
    let mut task_handles = Vec::new();

    for _i in 0..10 {
        let handler_clone = handler.clone();
        let count_clone = registration_count.clone();

        let handle = tokio::spawn(async move {
            // Register telegram callback
            let _telegram_handle = handler_clone
                .register_telegram_callback_sync(move |_| {
                    // Callback body
                })
                .await;

            // Register connection callback
            let _connection_handle = handler_clone
                .register_connection_callback_sync(move |_| {
                    // Callback body
                })
                .await;

            count_clone.fetch_add(2, Ordering::SeqCst); // 2 callbacks registered
        });

        task_handles.push(handle);
    }

    // Wait for all registration tasks to complete
    for handle in task_handles {
        handle.await.unwrap();
    }

    // Verify all callbacks were registered
    assert_eq!(registration_count.load(Ordering::SeqCst), 20); // 10 tasks * 2 callbacks each
    assert_eq!(handler.total_callback_count().await, 20);
    assert_eq!(handler.telegram_callback_count().await, 10);
    assert_eq!(handler.connection_callback_count().await, 10);
}

#[tokio::test]
async fn test_concurrent_callback_execution() {
    let handler = Arc::new(EventHandler::new());
    let execution_count = Arc::new(AtomicUsize::new(0));

    // Register multiple callbacks that increment a counter
    for _ in 0..5 {
        let count_clone = execution_count.clone();
        let _handle = handler
            .register_connection_callback_sync(move |_state| {
                count_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;
    }

    // Spawn multiple tasks that trigger notifications concurrently
    let mut task_handles = Vec::new();

    for _ in 0..10 {
        let handler_clone = handler.clone();

        let handle = tokio::spawn(async move {
            handler_clone
                .notify_connection_state_changed(ConnectionState::Connected)
                .await;
        });

        task_handles.push(handle);
    }

    // Wait for all notification tasks to complete
    for handle in task_handles {
        handle.await.unwrap();
    }

    // Each notification should trigger all 5 callbacks, so 10 notifications * 5 callbacks = 50
    assert_eq!(execution_count.load(Ordering::SeqCst), 50);
}

#[tokio::test]
async fn test_concurrent_registration_and_unregistration() {
    let handler = Arc::new(EventHandler::new());
    let mut callback_handles = Vec::new();

    // Register some initial callbacks
    for _ in 0..5 {
        let handle = handler.register_telegram_callback_sync(|_| {}).await;
        callback_handles.push(handle);
    }

    assert_eq!(handler.telegram_callback_count().await, 5);

    // Spawn concurrent registration and unregistration tasks
    let mut task_handles = Vec::new();

    // Registration tasks
    for _ in 0..3 {
        let handler_clone = handler.clone();
        let handle = tokio::spawn(async move {
            let _callback_handle = handler_clone.register_telegram_callback_sync(|_| {}).await;
            // Let the callback exist for a bit
            sleep(Duration::from_millis(10)).await;
        });
        task_handles.push(handle);
    }

    // Unregistration tasks
    for callback_handle in callback_handles.into_iter().take(3) {
        let handler_clone = handler.clone();
        let handle = tokio::spawn(async move {
            sleep(Duration::from_millis(5)).await; // Small delay
            handler_clone
                .unregister_telegram_callback(callback_handle)
                .await;
        });
        task_handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in task_handles {
        handle.await.unwrap();
    }

    // Final count should be consistent (2 original + 3 new - 3 removed = 2, but timing may vary)
    let final_count = handler.telegram_callback_count().await;
    assert!(
        (2..=5).contains(&final_count),
        "Final count should be between 2 and 5, got {final_count}"
    );
}

#[tokio::test]
async fn test_concurrent_mixed_operations() {
    let handler = Arc::new(EventHandler::new());
    let telegram_execution_count = Arc::new(AtomicUsize::new(0));
    let connection_execution_count = Arc::new(AtomicUsize::new(0));

    // Create test data
    let telegram = Telegram {
        source: IndividualAddress::from_raw(0x1234),
        destination: Address::Group(GroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1)),
        payload: vec![0x01],
        priority: Priority::Normal,
        direction: Direction::Incoming,
        telegram_type: TelegramType::GroupValueWrite,
        timestamp: std::time::SystemTime::now(),
    };

    // Spawn mixed concurrent operations
    let mut task_handles = Vec::new();

    // Registration tasks
    for i in 0..5 {
        let handler_clone = handler.clone();
        let telegram_count = telegram_execution_count.clone();
        let connection_count = connection_execution_count.clone();

        let handle = tokio::spawn(async move {
            // Register callbacks
            let _telegram_handle = handler_clone
                .register_telegram_callback_sync(move |_| {
                    telegram_count.fetch_add(1, Ordering::SeqCst);
                })
                .await;

            let _connection_handle = handler_clone
                .register_connection_callback_sync(move |_| {
                    connection_count.fetch_add(1, Ordering::SeqCst);
                })
                .await;

            // Small delay to let other tasks register
            sleep(Duration::from_millis(i * 2)).await;
        });

        task_handles.push(handle);
    }

    // Notification tasks
    for i in 0..3 {
        let handler_clone = handler.clone();
        let telegram_clone = telegram.clone();

        let handle = tokio::spawn(async move {
            sleep(Duration::from_millis(i * 5 + 10)).await; // Stagger notifications

            // Trigger notifications
            handler_clone
                .notify_telegram_received(&telegram_clone)
                .await;
            handler_clone
                .notify_connection_state_changed(ConnectionState::Connected)
                .await;
        });

        task_handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in task_handles {
        handle.await.unwrap();
    }

    // Give callbacks time to execute
    sleep(Duration::from_millis(100)).await;

    // Verify callbacks were executed
    // Each notification should trigger all registered callbacks at that time
    // The exact counts depend on timing, but should be > 0
    let telegram_executions = telegram_execution_count.load(Ordering::SeqCst);
    let connection_executions = connection_execution_count.load(Ordering::SeqCst);

    assert!(
        telegram_executions > 0,
        "Telegram callbacks should have been executed"
    );
    assert!(
        connection_executions > 0,
        "Connection callbacks should have been executed"
    );

    // Both should have similar counts since we registered the same number of each type
    // and triggered the same number of notifications for each type
    println!(
        "Execution counts - Telegram: {telegram_executions}, Connection: {connection_executions}"
    );
}

#[tokio::test]
async fn test_concurrent_filter_operations() {
    let handler = Arc::new(EventHandler::new());
    let matching_count = Arc::new(AtomicUsize::new(0));
    let non_matching_count = Arc::new(AtomicUsize::new(0));

    // Register callbacks with different filters concurrently
    let mut task_handles = Vec::new();

    for i in 0..5 {
        let handler_clone = handler.clone();
        let matching_clone = matching_count.clone();
        let non_matching_clone = non_matching_count.clone();

        let handle = tokio::spawn(async move {
            if i % 2 == 0 {
                // Register callback that matches group address 0x0101
                let _handle = handler_clone
                    .register_telegram_callback_sync_filtered(
                        move |_| {
                            matching_clone.fetch_add(1, Ordering::SeqCst);
                        },
                        TelegramFilter::GroupAddresses(vec![GroupAddress::new(
                            MainGroup::new(0),
                            MiddleGroup::new(1),
                            1,
                        )]),
                        false,
                    )
                    .await;
            } else {
                // Register callback that matches group address 0x0202 (won't match our test telegram)
                let _handle = handler_clone
                    .register_telegram_callback_sync_filtered(
                        move |_| {
                            non_matching_clone.fetch_add(1, Ordering::SeqCst);
                        },
                        TelegramFilter::GroupAddresses(vec![GroupAddress::new(
                            MainGroup::new(0),
                            MiddleGroup::new(2),
                            2,
                        )]),
                        false,
                    )
                    .await;
            }
        });

        task_handles.push(handle);
    }

    // Wait for registration to complete
    for handle in task_handles {
        handle.await.unwrap();
    }

    // Create telegram that matches 0x0101
    let telegram = Telegram {
        source: IndividualAddress::from_raw(0x1234),
        destination: Address::Group(GroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1)),
        payload: vec![0x01],
        priority: Priority::Normal,
        direction: Direction::Incoming,
        telegram_type: TelegramType::GroupValueWrite,
        timestamp: std::time::SystemTime::now(),
    };

    // Notify concurrently from multiple tasks
    let mut notification_handles = Vec::new();

    for _ in 0..3 {
        let handler_clone = handler.clone();
        let telegram_clone = telegram.clone();

        let handle = tokio::spawn(async move {
            handler_clone
                .notify_telegram_received(&telegram_clone)
                .await;
        });

        notification_handles.push(handle);
    }

    // Wait for notifications to complete
    for handle in notification_handles {
        handle.await.unwrap();
    }

    // Verify filtering worked correctly
    let matching_executions = matching_count.load(Ordering::SeqCst);
    let non_matching_executions = non_matching_count.load(Ordering::SeqCst);

    // We registered 3 matching callbacks (i=0,2,4) and 2 non-matching (i=1,3)
    // With 3 notifications, matching callbacks should be executed 3*3=9 times
    assert_eq!(
        matching_executions, 9,
        "Matching callbacks should be executed 9 times"
    );
    assert_eq!(
        non_matching_executions, 0,
        "Non-matching callbacks should not be executed"
    );
}
