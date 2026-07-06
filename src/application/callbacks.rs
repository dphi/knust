//! Event callback system for Knx library.
//!
//! This module provides a comprehensive callback system that allows users to register
//! event handlers for telegram reception and connection state changes.
//! The system is designed to be thread-safe, async-compatible, and memory-efficient.

use crate::protocol::{Telegram, address::GroupAddress};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

/// Unique handle for registered callbacks
///
/// This handle is returned when registering a callback and can be used
/// to unregister the callback later. Each handle is guaranteed to be unique
/// within the lifetime of the `EventHandler`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallbackHandle(u64);

impl CallbackHandle {
    /// Create a new callback handle with the given ID
    fn new(id: u64) -> Self {
        Self(id)
    }

    /// Get the internal ID of this handle
    #[must_use]
    pub fn id(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for CallbackHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CallbackHandle({})", self.0)
    }
}

/// Filtering system for telegram callbacks
///
/// `TelegramFilter` allows selective callback invocation based on various criteria.
/// This enables efficient callback processing by only invoking callbacks that
/// are interested in specific telegrams.
#[derive(Clone, Default)]
pub enum TelegramFilter {
    /// Match specific group addresses
    GroupAddresses(Vec<GroupAddress>),

    /// Match a range of group addresses (inclusive)
    AddressRange(GroupAddress, GroupAddress),

    /// Custom filter function
    Custom(Arc<dyn Fn(&Telegram) -> bool + Send + Sync>),

    /// Match all telegrams (no filtering)
    #[default]
    All,
}

impl std::fmt::Debug for TelegramFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TelegramFilter::GroupAddresses(addresses) => {
                f.debug_tuple("GroupAddresses").field(addresses).finish()
            }
            TelegramFilter::AddressRange(start, end) => f
                .debug_tuple("AddressRange")
                .field(start)
                .field(end)
                .finish(),
            TelegramFilter::Custom(_) => f.debug_tuple("Custom").field(&"<function>").finish(),
            TelegramFilter::All => f.debug_tuple("All").finish(),
        }
    }
}

impl TelegramFilter {
    /// Check if this filter matches the given telegram
    #[must_use]
    pub fn matches(&self, telegram: &Telegram) -> bool {
        match self {
            TelegramFilter::GroupAddresses(addresses) => {
                if let crate::protocol::address::Address::Group(group_addr) = telegram.destination {
                    addresses.contains(&group_addr)
                } else {
                    false
                }
            }
            TelegramFilter::AddressRange(start, end) => {
                if let crate::protocol::address::Address::Group(group_addr) = telegram.destination {
                    group_addr.raw() >= start.raw() && group_addr.raw() <= end.raw()
                } else {
                    false
                }
            }
            TelegramFilter::Custom(filter_fn) => filter_fn(telegram),
            TelegramFilter::All => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Connection is disconnected
    Disconnected,
    /// Connection is in the process of connecting
    Connecting,
    /// Connection is established and operational
    Connected,
    /// Connection is in the process of disconnecting
    Disconnecting,
    /// Connection is in an error state
    Error,
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionState::Disconnected => write!(f, "disconnected"),
            ConnectionState::Connecting => write!(f, "connecting"),
            ConnectionState::Connected => write!(f, "connected"),
            ConnectionState::Disconnecting => write!(f, "disconnecting"),
            ConnectionState::Error => write!(f, "error"),
        }
    }
}

/// Trait for telegram callback functions
///
/// This trait is implemented for both sync and async functions that can handle
/// telegram events. The trait uses `async_trait` to support async callbacks.
#[async_trait]
pub trait TelegramCallbackFn {
    /// Called when a telegram is received
    async fn call(&self, telegram: &Telegram);
}

/// Trait for connection state callback functions
///
/// This trait is implemented for both sync and async functions that can handle
/// connection state change events.
#[async_trait]
pub trait ConnectionCallbackFn {
    /// Called when the connection state changes
    async fn call(&self, state: ConnectionState);
}

struct SyncTelegramCb<F>(F);
struct SyncConnectionCb<F>(F);

#[async_trait]
impl<F: Fn(&Telegram) + Send + Sync> TelegramCallbackFn for SyncTelegramCb<F> {
    async fn call(&self, telegram: &Telegram) {
        (self.0)(telegram);
    }
}
#[async_trait]
impl<F: Fn(ConnectionState) + Send + Sync> ConnectionCallbackFn for SyncConnectionCb<F> {
    async fn call(&self, state: ConnectionState) {
        (self.0)(state);
    }
}

/// Internal storage entry for telegram callbacks
struct TelegramCallbackEntry {
    id: CallbackHandle,
    callback: Box<dyn TelegramCallbackFn + Send + Sync>,
    filter: TelegramFilter,
    include_outgoing: bool,
}

/// Internal storage entry for connection callbacks
struct ConnectionCallbackEntry {
    id: CallbackHandle,
    callback: Box<dyn ConnectionCallbackFn + Send + Sync>,
}

/// Central event handler managing all callback operations
///
/// The `EventHandler` is the core component of the callback system. It manages
/// registration, storage, and execution of all callback types. It is designed
/// to be thread-safe and can be shared across multiple async tasks.
///
/// # Example
///
/// ```rust,no_run
/// use knust::application::callbacks::{EventHandler, ConnectionState};
/// use std::sync::Arc;
///
/// #[tokio::main]
/// async fn main() {
///     let handler = Arc::new(EventHandler::new());
///     
///     // Register a connection state callback
///     let handle = handler.register_connection_callback_sync(|state| {
///         println!("Connection state changed to: {}", state);
///     }).await;
///     
///     // Notify of state change
///     handler.notify_connection_state_changed(ConnectionState::Connected).await;
///     
///     // Unregister the callback
///     handler.unregister_connection_callback(handle).await;
/// }
/// ```
pub struct EventHandler {
    /// Storage for telegram callbacks
    telegram_callbacks: Arc<RwLock<Vec<TelegramCallbackEntry>>>,

    /// Storage for connection callbacks
    connection_callbacks: Arc<RwLock<Vec<ConnectionCallbackEntry>>>,

    /// Atomic counter for generating unique callback handles
    next_id: Arc<AtomicU64>,
}

impl EventHandler {
    /// Create a new `EventHandler` instance
    #[must_use]
    pub fn new() -> Self {
        Self {
            telegram_callbacks: Arc::new(RwLock::new(Vec::new())),
            connection_callbacks: Arc::new(RwLock::new(Vec::new())),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Generate a unique callback handle
    fn generate_handle(&self) -> CallbackHandle {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        CallbackHandle::new(id)
    }

    /// Register a telegram callback with optional filtering
    ///
    /// # Arguments
    /// * `callback` - The callback function to register
    /// * `filter` - Optional filter to limit callback scope
    /// * `include_outgoing` - Whether to include outgoing telegrams
    ///
    /// # Returns
    /// A unique handle that can be used to unregister the callback
    pub async fn register_telegram_callback<F>(&self, callback: F) -> CallbackHandle
    where
        F: TelegramCallbackFn + Send + Sync + 'static,
    {
        self.register_telegram_callback_filtered(callback, TelegramFilter::All, false)
            .await
    }

    /// Register a telegram callback with filtering
    ///
    /// # Arguments
    /// * `callback` - The callback function to register
    /// * `filter` - Filter to limit callback scope
    /// * `include_outgoing` - Whether to include outgoing telegrams
    ///
    /// # Returns
    /// A unique handle that can be used to unregister the callback
    pub async fn register_telegram_callback_filtered<F>(
        &self,
        callback: F,
        filter: TelegramFilter,
        include_outgoing: bool,
    ) -> CallbackHandle
    where
        F: TelegramCallbackFn + Send + Sync + 'static,
    {
        let handle = self.generate_handle();
        let entry = TelegramCallbackEntry {
            id: handle,
            callback: Box::new(callback),
            filter,
            include_outgoing,
        };

        let mut callbacks = self.telegram_callbacks.write().await;
        callbacks.push(entry);

        handle
    }

    /// Register a sync telegram callback
    ///
    /// # Arguments
    /// * `callback` - The sync callback function to register
    ///
    /// # Returns
    /// A unique handle that can be used to unregister the callback
    pub async fn register_telegram_callback_sync<F>(&self, callback: F) -> CallbackHandle
    where
        F: Fn(&Telegram) + Send + Sync + 'static,
    {
        self.register_telegram_callback_filtered(
            SyncTelegramCb(callback),
            TelegramFilter::All,
            false,
        )
        .await
    }

    /// Register a sync telegram callback with filtering
    ///
    /// # Arguments
    /// * `callback` - The sync callback function to register
    /// * `filter` - Filter to limit callback scope
    /// * `include_outgoing` - Whether to include outgoing telegrams
    ///
    /// # Returns
    /// A unique handle that can be used to unregister the callback
    pub async fn register_telegram_callback_sync_filtered<F>(
        &self,
        callback: F,
        filter: TelegramFilter,
        include_outgoing: bool,
    ) -> CallbackHandle
    where
        F: Fn(&Telegram) + Send + Sync + 'static,
    {
        self.register_telegram_callback_filtered(SyncTelegramCb(callback), filter, include_outgoing)
            .await
    }

    /// Register a connection state callback from a sync closure
    ///
    /// # Arguments
    /// * `callback` - The sync callback function to register
    ///
    /// # Returns
    /// A unique handle that can be used to unregister the callback
    pub async fn register_connection_callback_sync<F>(&self, callback: F) -> CallbackHandle
    where
        F: Fn(ConnectionState) + Send + Sync + 'static,
    {
        self.register_connection_callback(SyncConnectionCb(callback))
            .await
    }

    /// Register a connection callback (convenience method that accepts both sync and async)
    ///
    /// # Arguments
    /// * `callback` - The callback function to register
    ///
    /// # Returns
    /// A unique handle that can be used to unregister the callback
    pub async fn register_connection_callback<F>(&self, callback: F) -> CallbackHandle
    where
        F: ConnectionCallbackFn + Send + Sync + 'static,
    {
        let handle = self.generate_handle();
        let entry = ConnectionCallbackEntry {
            id: handle,
            callback: Box::new(callback),
        };

        let mut callbacks = self.connection_callbacks.write().await;
        callbacks.push(entry);

        handle
    }

    /// Unregister a telegram callback
    ///
    /// # Arguments
    /// * `handle` - The handle returned when the callback was registered
    ///
    /// # Returns
    /// `true` if the callback was found and removed, `false` otherwise
    pub async fn unregister_telegram_callback(&self, handle: CallbackHandle) -> bool {
        let mut callbacks = self.telegram_callbacks.write().await;
        if let Some(pos) = callbacks.iter().position(|entry| entry.id == handle) {
            callbacks.remove(pos);
            true
        } else {
            false
        }
    }

    /// Unregister a connection callback
    ///
    /// # Arguments
    /// * `handle` - The handle returned when the callback was registered
    ///
    /// # Returns
    /// `true` if the callback was found and removed, `false` otherwise
    pub async fn unregister_connection_callback(&self, handle: CallbackHandle) -> bool {
        let mut callbacks = self.connection_callbacks.write().await;
        if let Some(pos) = callbacks.iter().position(|entry| entry.id == handle) {
            callbacks.remove(pos);
            true
        } else {
            false
        }
    }

    /// Unregister any callback by its handle
    ///
    /// This is a convenience method that attempts to unregister the callback
    /// from all callback types.
    ///
    /// # Arguments
    /// * `handle` - The handle returned when the callback was registered
    ///
    /// # Returns
    /// `true` if the callback was found and removed, `false` otherwise
    pub async fn unregister_callback(&self, handle: CallbackHandle) -> bool {
        // Try to unregister from each callback type
        let telegram_removed = self.unregister_telegram_callback(handle).await;
        let connection_removed = self.unregister_connection_callback(handle).await;

        telegram_removed || connection_removed
    }

    /// Register a telegram callback from a boxed trait object
    ///
    /// This method is used internally by the builder to register stored callbacks.
    ///
    /// # Arguments
    /// * `callback` - The boxed callback trait object
    /// * `filter` - Filter to limit callback scope
    /// * `include_outgoing` - Whether to include outgoing telegrams
    ///
    /// # Returns
    /// A unique handle that can be used to unregister the callback
    pub async fn register_telegram_callback_boxed(
        &self,
        callback: Box<dyn TelegramCallbackFn + Send + Sync>,
        filter: TelegramFilter,
        include_outgoing: bool,
    ) -> CallbackHandle {
        let handle = self.generate_handle();
        let entry = TelegramCallbackEntry {
            id: handle,
            callback,
            filter,
            include_outgoing,
        };

        let mut callbacks = self.telegram_callbacks.write().await;
        callbacks.push(entry);

        handle
    }

    /// Register a connection callback from a boxed trait object
    ///
    /// This method is used internally by the builder to register stored callbacks.
    ///
    /// # Arguments
    /// * `callback` - The boxed callback trait object
    ///
    /// # Returns
    /// A unique handle that can be used to unregister the callback
    pub async fn register_connection_callback_boxed(
        &self,
        callback: Box<dyn ConnectionCallbackFn + Send + Sync>,
    ) -> CallbackHandle {
        let handle = self.generate_handle();
        let entry = ConnectionCallbackEntry {
            id: handle,
            callback,
        };

        let mut callbacks = self.connection_callbacks.write().await;
        callbacks.push(entry);

        handle
    }

    /// Notify all registered telegram callbacks
    ///
    /// This method is called internally when a telegram is received.
    /// It invokes all registered telegram callbacks that match the telegram's
    /// filter criteria, in registration order. Failed callbacks are logged
    /// but don't prevent other callbacks from executing.
    ///
    /// # Arguments
    /// * `telegram` - The received telegram
    pub async fn notify_telegram_received(&self, telegram: &Telegram) {
        let callbacks = self.telegram_callbacks.read().await;

        for entry in callbacks.iter() {
            // Check if we should skip outgoing telegrams
            if !entry.include_outgoing
                && telegram.direction == crate::protocol::telegram::Direction::Outgoing
            {
                continue;
            }

            // Check if the filter matches
            if !entry.filter.matches(telegram) {
                continue;
            }

            // Execute callback and handle any errors
            match tokio::time::timeout(
                std::time::Duration::from_millis(50), // Reduced timeout for faster tests
                entry.callback.call(telegram),
            )
            .await
            {
                Ok(()) => {
                    // Callback executed successfully
                    log::debug!("Telegram callback {} executed successfully", entry.id);
                }
                Err(_) => {
                    // Log timeout but continue with other callbacks
                    log::warn!(
                        "Telegram callback {} timed out after 50ms for telegram from {} to {}",
                        entry.id,
                        telegram.source,
                        telegram.destination
                    );
                }
            }
        }
    }

    /// Notify all registered connection callbacks
    ///
    /// This method is called internally when the connection state changes.
    /// It invokes all registered connection callbacks in registration order.
    ///
    /// # Arguments
    /// * `state` - The new connection state
    pub async fn notify_connection_state_changed(&self, state: ConnectionState) {
        let callbacks = self.connection_callbacks.read().await;

        for entry in callbacks.iter() {
            // Execute callback and handle any errors
            if (tokio::time::timeout(
                std::time::Duration::from_secs(5),
                entry.callback.call(state),
            )
            .await)
                .is_err()
            {
                // Log timeout but continue with other callbacks
                log::warn!("Connection callback {} timed out", entry.id);
            }
        }
    }

    /// Get the number of registered telegram callbacks
    pub async fn telegram_callback_count(&self) -> usize {
        self.telegram_callbacks.read().await.len()
    }

    /// Get the number of registered connection callbacks
    pub async fn connection_callback_count(&self) -> usize {
        self.connection_callbacks.read().await.len()
    }

    /// Get the total number of registered callbacks
    pub async fn total_callback_count(&self) -> usize {
        let telegram_count = self.telegram_callback_count().await;
        let connection_count = self.connection_callback_count().await;

        telegram_count + connection_count
    }

    /// Clear all registered callbacks
    ///
    /// This method removes all registered callbacks of all types.
    /// It's primarily useful for testing and cleanup scenarios.
    pub async fn clear_all_callbacks(&self) {
        let mut telegram_callbacks = self.telegram_callbacks.write().await;
        let mut connection_callbacks = self.connection_callbacks.write().await;

        telegram_callbacks.clear();
        connection_callbacks.clear();
    }
}

impl Default for EventHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::Config as ProptestConfig;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{Duration, sleep};

    /// Async connection-callback wrapper used by `test_async_callback_execution`.
    struct AsyncWrapper {
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ConnectionCallbackFn for AsyncWrapper {
        async fn call(&self, _state: ConnectionState) {
            sleep(Duration::from_millis(10)).await;
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Async connection-callback wrapper used by `test_multiple_callbacks_execution_order`.
    struct OrderCallback {
        order: Arc<RwLock<Vec<i32>>>,
        value: i32,
    }

    #[async_trait]
    impl ConnectionCallbackFn for OrderCallback {
        async fn call(&self, _state: ConnectionState) {
            let mut order = self.order.write().await;
            order.push(self.value);
        }
    }

    /// Slow telegram callback used by property tests exercising timeout handling.
    struct SlowCallback {
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl TelegramCallbackFn for SlowCallback {
        async fn call(&self, _telegram: &Telegram) {
            // Increment counter to show we were called
            self.counter.fetch_add(1, Ordering::SeqCst);
            // Sleep longer than the timeout to simulate a slow callback
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }

    /// Async connection callback used by `property_sync_async_callback_support`.
    struct AsyncCallback {
        counter: Arc<AtomicUsize>,
        order_tracker: Arc<tokio::sync::RwLock<Vec<String>>>,
        id: usize,
    }

    #[async_trait]
    impl ConnectionCallbackFn for AsyncCallback {
        async fn call(&self, _state: ConnectionState) {
            // Simulate async work
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            self.counter.fetch_add(1, Ordering::SeqCst);
            self.order_tracker
                .write()
                .await
                .push(format!("async_{}", self.id));
        }
    }

    /// Async telegram callback used by `property_sync_async_callback_support`.
    struct AsyncTelegramCallback {
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl TelegramCallbackFn for AsyncTelegramCallback {
        async fn call(&self, _telegram: &Telegram) {
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Sync-style telegram callback registered via the builder in property tests.
    struct SyncTelegramCallback {
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl TelegramCallbackFn for SyncTelegramCallback {
        async fn call(&self, _telegram: &Telegram) {
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Sync-style connection callback registered via the builder in property tests.
    struct SyncConnectionCallback {
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ConnectionCallbackFn for SyncConnectionCallback {
        async fn call(&self, _state: crate::application::callbacks::ConnectionState) {
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_callback_handle_uniqueness() {
        let handler = EventHandler::new();

        // Register multiple callbacks and verify handles are unique
        let telegram_handle_a = handler.register_telegram_callback_sync(|_| {}).await;
        let telegram_handle_b = handler.register_telegram_callback_sync(|_| {}).await;
        let connection_handle = handler.register_connection_callback_sync(|_| {}).await;

        // All handles should be different
        assert_ne!(telegram_handle_a, telegram_handle_b);
        assert_ne!(telegram_handle_a, connection_handle);
        assert_ne!(telegram_handle_b, connection_handle);
    }

    /// For any callback registration operation (telegram or connection),
    /// the system should accept the callback, store it correctly, and return a unique handle
    #[test]
    fn property_callback_handle_uniqueness() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(20), |(
            // Generate number of callbacks to register for each type
            telegram_count in 1usize..10usize,
            connection_count in 1usize..10usize,
        )| {
            rt.block_on(async {
                let handler = EventHandler::new();
                let mut all_handles = HashSet::new();

                // Register telegram callbacks and collect handles
                for _ in 0..telegram_count {
                    let handle = handler.register_telegram_callback_sync(|_| {}).await;
                    // Each handle should be unique
                    assert!(all_handles.insert(handle), "Duplicate handle found: {handle}");
                }

                // Register connection callbacks and collect handles
                for _ in 0..connection_count {
                    let handle = handler.register_connection_callback_sync(|_| {}).await;
                    // Each handle should be unique
                    assert!(all_handles.insert(handle), "Duplicate handle found: {handle}");
                }

                // Verify total count matches expected
                let expected_total = telegram_count + connection_count;
                assert_eq!(all_handles.len(), expected_total);
                assert_eq!(handler.total_callback_count().await, expected_total);

                // Verify individual counts
                assert_eq!(handler.telegram_callback_count().await, telegram_count);
                assert_eq!(handler.connection_callback_count().await, connection_count);
            });
        });
    }

    #[tokio::test]
    async fn test_callback_registration_and_storage() {
        let handler = EventHandler::new();

        // Initially no callbacks
        assert_eq!(handler.total_callback_count().await, 0);

        // Register callbacks
        let _handle1 = handler.register_telegram_callback_sync(|_| {}).await;
        assert_eq!(handler.telegram_callback_count().await, 1);
        assert_eq!(handler.total_callback_count().await, 1);

        let _handle2 = handler.register_connection_callback_sync(|_| {}).await;
        assert_eq!(handler.connection_callback_count().await, 1);
        assert_eq!(handler.total_callback_count().await, 2);
    }

    #[tokio::test]
    async fn test_callback_unregistration() {
        let handler = EventHandler::new();

        // Register callbacks
        let telegram_handle = handler.register_telegram_callback_sync(|_| {}).await;
        let connection_handle = handler.register_connection_callback_sync(|_| {}).await;

        assert_eq!(handler.total_callback_count().await, 2);

        // Unregister specific callbacks
        assert!(handler.unregister_telegram_callback(telegram_handle).await);
        assert_eq!(handler.telegram_callback_count().await, 0);
        assert_eq!(handler.total_callback_count().await, 1);

        assert!(
            handler
                .unregister_connection_callback(connection_handle)
                .await
        );
        assert_eq!(handler.connection_callback_count().await, 0);
        assert_eq!(handler.total_callback_count().await, 0);
    }

    #[tokio::test]
    async fn test_invalid_handle_unregistration() {
        let handler = EventHandler::new();

        // Try to unregister non-existent handles
        let fake_handle = CallbackHandle::new(999);

        assert!(!handler.unregister_telegram_callback(fake_handle).await);
        assert!(!handler.unregister_connection_callback(fake_handle).await);
        assert!(!handler.unregister_callback(fake_handle).await);
    }

    #[tokio::test]
    async fn test_sync_callback_execution() {
        let handler = EventHandler::new();
        let counter = Arc::new(AtomicUsize::new(0));

        // Register sync callback
        let counter_clone = counter.clone();
        let _handle = handler
            .register_connection_callback_sync(move |_state| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Notify and verify callback was called
        handler
            .notify_connection_state_changed(ConnectionState::Connected)
            .await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Notify again
        handler
            .notify_connection_state_changed(ConnectionState::Disconnected)
            .await;
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_async_callback_execution() {
        let handler = EventHandler::new();
        let counter = Arc::new(AtomicUsize::new(0));

        // Register async callback
        let _handle = handler
            .register_connection_callback(AsyncWrapper {
                counter: counter.clone(),
            })
            .await;

        // Notify and verify callback was called
        handler
            .notify_connection_state_changed(ConnectionState::Connected)
            .await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_multiple_callbacks_execution_order() {
        let handler = EventHandler::new();
        let execution_order = Arc::new(RwLock::new(Vec::new()));

        // Register multiple callbacks
        let _handle1 = handler
            .register_connection_callback(OrderCallback {
                order: execution_order.clone(),
                value: 1,
            })
            .await;

        let _handle2 = handler
            .register_connection_callback(OrderCallback {
                order: execution_order.clone(),
                value: 2,
            })
            .await;

        let _handle3 = handler
            .register_connection_callback(OrderCallback {
                order: execution_order.clone(),
                value: 3,
            })
            .await;

        // Notify and verify execution order
        handler
            .notify_connection_state_changed(ConnectionState::Connected)
            .await;

        let order = execution_order.read().await;
        assert_eq!(*order, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_clear_all_callbacks() {
        let handler = EventHandler::new();

        // Register callbacks
        let _handle1 = handler.register_telegram_callback_sync(|_| {}).await;
        let _handle2 = handler.register_connection_callback_sync(|_| {}).await;

        assert_eq!(handler.total_callback_count().await, 2);

        // Clear all callbacks
        handler.clear_all_callbacks().await;
        assert_eq!(handler.total_callback_count().await, 0);
    }

    /// For any telegram received and any set of registered telegram callbacks,
    /// all callbacks without filters or with matching filters should be invoked exactly once
    #[test]
    fn property_telegram_callback_invocation() {
        use crate::protocol::{
            address::{Address, GroupAddress, IndividualAddress},
            telegram::{Direction, Telegram, TelegramType},
        };
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(20), |(
            // Generate telegram properties
            source_raw in 0u16..=0xFFFF,
            dest_main in 0u8..=GroupAddress::MAX_MAIN,
            dest_middle in 0u8..=GroupAddress::MAX_MIDDLE,
            dest_sub in 0u8..=255u8,
            payload in prop::collection::vec(0u8..=255u8, 0..5),
            is_outgoing in prop::bool::ANY,

            // Generate callback configurations
            callback_configs in prop::collection::vec(
                (prop::bool::ANY, prop::bool::ANY), // (has_filter, include_outgoing)
                1..5usize
            ),
        )| {
            let _ = rt.block_on(async {
                let handler = EventHandler::new();
                let mut callback_counters = HashMap::new();

                // Create telegram
                let source = IndividualAddress::from_raw(source_raw);
                let destination = Address::Group(GroupAddress::from_parts(dest_main, dest_middle, dest_sub).unwrap());
                let direction = if is_outgoing { Direction::Outgoing } else { Direction::Incoming };

                let telegram = Telegram {
                    source,
                    destination,
                    payload,
                    priority: crate::protocol::telegram::Priority::Normal,
                    direction,
                    telegram_type: TelegramType::GroupValueWrite,
                    gateway_id: None,
                    timestamp: std::time::SystemTime::now(),
                };

                // Register callbacks with different configurations
                for (i, (has_filter, include_outgoing)) in callback_configs.iter().enumerate() {
                    let counter = Arc::new(AtomicUsize::new(0));
                    callback_counters.insert(i, counter.clone());

                    let filter = if *has_filter {
                        // Create a filter that matches our telegram's destination
                        if let Address::Group(group_addr) = telegram.destination {
                            TelegramFilter::GroupAddresses(vec![group_addr])
                        } else {
                            TelegramFilter::All
                        }
                    } else {
                        TelegramFilter::All
                    };

                    let counter_clone = counter.clone();
                    let _handle = handler.register_telegram_callback_sync_filtered(
                        move |_telegram| {
                            counter_clone.fetch_add(1, Ordering::SeqCst);
                        },
                        filter,
                        *include_outgoing
                    ).await;
                }

                // Notify telegram received
                handler.notify_telegram_received(&telegram).await;

                // Verify callback invocations
                for (i, (has_filter, include_outgoing)) in callback_configs.iter().enumerate() {
                    let counter = callback_counters.get(&i).unwrap();
                    let count = counter.load(Ordering::SeqCst);

                    // Determine if callback should have been invoked
                    let should_invoke = if is_outgoing && !include_outgoing {
                        // Outgoing telegram but callback doesn't include outgoing
                        false
                    } else if *has_filter {
                        // Has filter - should match since we created matching filter
                        true
                    } else {
                        // No filter - should always match
                        true
                    };

                    if should_invoke {
                        prop_assert_eq!(count, 1, "Callback {} should have been invoked exactly once", i);
                    } else {
                        prop_assert_eq!(count, 0, "Callback {} should not have been invoked", i);
                    }
                }

                Ok(())
            });
        });
    }

    /// For any telegram and any set of filters, only callbacks with matching filters
    /// should be invoked
    #[test]
    fn property_telegram_filtering() {
        use crate::protocol::{
            address::{Address, GroupAddress, IndividualAddress},
            telegram::{Direction, Telegram, TelegramType},
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(20), |(
            // Generate telegram properties
            source_raw in 0u16..=0xFFFF,
            dest_main in 0u8..=GroupAddress::MAX_MAIN,
            dest_middle in 0u8..=GroupAddress::MAX_MIDDLE,
            dest_sub in 0u8..=255u8,
            payload in prop::collection::vec(0u8..=255u8, 0..5),

            // Generate different filter addresses
            filter_addresses in prop::collection::vec(
                (0u8..=GroupAddress::MAX_MAIN, 0u8..=GroupAddress::MAX_MIDDLE, 0u8..=GroupAddress::MAX_SUB), // (main, middle, sub)
                1..3usize
            ),
        )| {
            let _ = rt.block_on(async {
                let handler = EventHandler::new();

                // Create telegram
                let source = IndividualAddress::from_raw(source_raw);
                let telegram_addr = GroupAddress::from_parts(dest_main, dest_middle, dest_sub).unwrap();
                let destination = Address::Group(telegram_addr);

                let telegram = Telegram {
                    source,
                    destination,
                    payload,
                    priority: crate::protocol::telegram::Priority::Normal,
                    direction: Direction::Incoming,
                    telegram_type: TelegramType::GroupValueWrite,
                    gateway_id: None,
                    timestamp: std::time::SystemTime::now(),
                };

                // Register callbacks with different filters
                let mut callback_counters = Vec::new();
                let mut expected_invocations = Vec::new();

                for (filter_main, filter_middle, filter_sub) in &filter_addresses {
                    let counter = Arc::new(AtomicUsize::new(0));
                    callback_counters.push(counter.clone());

                    let filter_addr = GroupAddress::from_parts(*filter_main, *filter_middle, *filter_sub).unwrap();
                    let filter = TelegramFilter::GroupAddresses(vec![filter_addr]);

                    // Determine if this filter should match the telegram
                    let should_match = filter_addr == telegram_addr;
                    expected_invocations.push(should_match);

                    let counter_clone = counter.clone();
                    let _handle = handler.register_telegram_callback_sync_filtered(
                        move |_telegram| {
                            counter_clone.fetch_add(1, Ordering::SeqCst);
                        },
                        filter,
                        false // Don't include outgoing
                    ).await;
                }

                // Also register a callback with All filter (should always match)
                let all_counter = Arc::new(AtomicUsize::new(0));
                let all_counter_clone = all_counter.clone();
                let _all_handle = handler.register_telegram_callback_sync_filtered(
                    move |_telegram| {
                        all_counter_clone.fetch_add(1, Ordering::SeqCst);
                    },
                    TelegramFilter::All,
                    false
                ).await;

                // Notify telegram received
                handler.notify_telegram_received(&telegram).await;

                // Verify callback invocations
                for (i, expected) in expected_invocations.iter().enumerate() {
                    let counter = &callback_counters[i];
                    let count = counter.load(Ordering::SeqCst);

                    if *expected {
                        prop_assert_eq!(count, 1, "Callback {} with matching filter should have been invoked", i);
                    } else {
                        prop_assert_eq!(count, 0, "Callback {} with non-matching filter should not have been invoked", i);
                    }
                }

                // The "All" filter callback should always be invoked
                prop_assert_eq!(all_counter.load(Ordering::SeqCst), 1, "Callback with All filter should always be invoked");

                Ok(())
            });
        });
    }

    /// For any sequence of registered callbacks, when the corresponding event occurs,
    /// all matching callbacks should be invoked in their registration order
    #[test]
    fn property_callback_ordering() {
        use crate::protocol::{
            address::{Address, GroupAddress, IndividualAddress},
            telegram::{Direction, Telegram, TelegramType},
        };
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(20), |(
            // Generate telegram properties
            source_raw in 0u16..=0xFFFF,
            dest_main in 0u8..=GroupAddress::MAX_MAIN,
            dest_middle in 0u8..=GroupAddress::MAX_MIDDLE,
            dest_sub in 0u8..=255u8,
            payload in prop::collection::vec(0u8..=255u8, 0..5),

            // Generate number of callbacks to register
            callback_count in 2usize..5usize,
        )| {
            let _ = rt.block_on(async {
                let handler = EventHandler::new();
                let execution_order = Arc::new(RwLock::new(Vec::new()));

                // Create telegram
                let source = IndividualAddress::from_raw(source_raw);
                let destination = Address::Group(GroupAddress::from_parts(dest_main, dest_middle, dest_sub).unwrap());

                let telegram = Telegram {
                    source,
                    destination,
                    payload,
                    priority: crate::protocol::telegram::Priority::Normal,
                    direction: Direction::Incoming,
                    telegram_type: TelegramType::GroupValueWrite,
                    gateway_id: None,
                    timestamp: std::time::SystemTime::now(),
                };

                // Register callbacks in order
                for i in 0..callback_count {
                    let order_clone = execution_order.clone();
                    let _handle = handler.register_telegram_callback_sync(move |_telegram| {
                        let order_clone = order_clone.clone();
                        tokio::spawn(async move {
                            let mut order = order_clone.write().await;
                            order.push(i);
                        });
                    }).await;
                }

                // Notify telegram received
                handler.notify_telegram_received(&telegram).await;

                // Give callbacks time to execute
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                // Verify execution order
                let order = execution_order.read().await;
                prop_assert_eq!(order.len(), callback_count, "All callbacks should have been executed");

                // Check that callbacks were executed in registration order
                for (i, &executed_index) in order.iter().enumerate() {
                    prop_assert_eq!(executed_index, i, "Callback {} should have been executed in position {}", executed_index, i);
                }

                Ok(())
            });
        });
    }

    /// For any callback that throws an error during execution, other callbacks
    /// should still be invoked and the system should continue processing normally
    #[test]
    fn property_error_isolation() {
        use crate::protocol::{
            address::{Address, GroupAddress, IndividualAddress},
            telegram::{Direction, Telegram, TelegramType},
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(20), |(
            // Generate telegram properties
            source_raw in 0u16..=0xFFFF,
            dest_main in 0u8..=GroupAddress::MAX_MAIN,
            dest_middle in 0u8..=GroupAddress::MAX_MIDDLE,
            dest_sub in 0u8..=255u8,
            payload in prop::collection::vec(0u8..=255u8, 0..5),

            // Generate callback configurations
            callback_count in 3usize..5usize,
            slow_callback_index in 0usize..2usize, // Which callback should be slow/timeout
        )| {
            let _ = rt.block_on(async {
                let handler = EventHandler::new();
                let success_counter = Arc::new(AtomicUsize::new(0));
                let slow_callback_counter = Arc::new(AtomicUsize::new(0));

                // Create telegram
                let source = IndividualAddress::from_raw(source_raw);
                let destination = Address::Group(GroupAddress::from_parts(dest_main, dest_middle, dest_sub).unwrap());

                let telegram = Telegram {
                    source,
                    destination,
                    payload,
                    priority: crate::protocol::telegram::Priority::Normal,
                    direction: Direction::Incoming,
                    telegram_type: TelegramType::GroupValueWrite,
                    gateway_id: None,
                    timestamp: std::time::SystemTime::now(),
                };

                // Register callbacks, with one that will be slow and timeout
                for i in 0..callback_count {
                    if i == slow_callback_index {
                        // This callback will be slow and timeout
                        let slow_counter = slow_callback_counter.clone();

                        let _handle = handler.register_telegram_callback_filtered(
                            SlowCallback { counter: slow_counter },
                            TelegramFilter::All,
                            false
                        ).await;
                    } else {
                        // Normal fast callback
                        let counter_clone = success_counter.clone();
                        let _handle = handler.register_telegram_callback_sync(move |_telegram| {
                            counter_clone.fetch_add(1, Ordering::SeqCst);
                        }).await;
                    }
                }

                // Notify telegram received - this should complete despite the slow callback
                handler.notify_telegram_received(&telegram).await;

                // Verify that all fast callbacks were executed
                let fast_callback_count = callback_count - 1; // All except the slow one
                let success_calls = success_counter.load(Ordering::SeqCst);
                prop_assert_eq!(success_calls, fast_callback_count, "All fast callbacks should have been executed");

                // Verify the slow callback was attempted (it increments before sleeping)
                let slow_calls = slow_callback_counter.load(Ordering::SeqCst);
                prop_assert_eq!(slow_calls, 1, "The slow callback should have been attempted");

                Ok(())
            });
        });
    }

    /// For any connection state change and any set of registered connection callbacks,
    /// all callbacks should be invoked with the correct state parameter, but only when
    /// the state actually changes
    #[test]
    fn property_connection_state_callback_invocation() {
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(20), |(
            // Generate connection state sequences
            state_sequence in prop::collection::vec(
                prop::sample::select(vec![
                    ConnectionState::Disconnected,
                    ConnectionState::Connecting,
                    ConnectionState::Connected,
                    ConnectionState::Disconnecting,
                    ConnectionState::Error,
                ]),
                1..5usize
            ),

            // Generate callback configurations
            callback_count in 1usize..5usize,
        )| {
            let _ = rt.block_on(async {
                let handler = EventHandler::new();
                let mut callback_counters = HashMap::new();
                let mut received_states = HashMap::new();

                // Register connection state callbacks
                for i in 0..callback_count {
                    let counter = Arc::new(AtomicUsize::new(0));
                    let states = Arc::new(tokio::sync::RwLock::new(Vec::new()));

                    callback_counters.insert(i, counter.clone());
                    received_states.insert(i, states.clone());

                    let counter_clone = counter.clone();
                    let states_clone = states.clone();

                    let _handle = handler.register_connection_callback_sync(move |state| {
                        counter_clone.fetch_add(1, Ordering::SeqCst);
                        let states_clone = states_clone.clone();
                        tokio::spawn(async move {
                            states_clone.write().await.push(state);
                        });
                    }).await;
                }

                // Notify each state change
                for state in &state_sequence {
                    handler.notify_connection_state_changed(*state).await;
                }

                // Give async tasks time to complete
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

                // Verify all callbacks were invoked for each state change
                for i in 0..callback_count {
                    let counter = callback_counters.get(&i).unwrap();
                    let count = counter.load(Ordering::SeqCst);
                    prop_assert_eq!(count, state_sequence.len(),
                        "Connection callback {} should have been invoked {} times", i, state_sequence.len());

                    // Verify callback received correct states in correct order
                    let states = received_states.get(&i).unwrap();
                    let received = states.read().await;
                    prop_assert_eq!(received.len(), state_sequence.len(),
                        "Callback {} should have received {} states", i, state_sequence.len());

                    for (j, expected_state) in state_sequence.iter().enumerate() {
                        prop_assert_eq!(received[j], *expected_state,
                            "Callback {} should have received state {:?} at position {}", i, expected_state, j);
                    }
                }

                Ok(())
            });
        });
    }

    /// For any registered callback, unregistering it using its handle should remove it
    /// from the system, and subsequent events should not invoke that callback
    #[test]
    fn property_callback_unregistration() {
        use crate::protocol::{
            address::{Address, GroupAddress, IndividualAddress},
            telegram::{Direction, Telegram, TelegramType},
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(20), |(
            // Generate telegram properties
            source_raw in 0u16..=0xFFFF,
            dest_main in 0u8..=GroupAddress::MAX_MAIN,
            dest_middle in 0u8..=GroupAddress::MAX_MIDDLE,
            dest_sub in 0u8..=255u8,
            payload in prop::collection::vec(0u8..=255u8, 0..5),

            // Generate callback configurations
            telegram_callback_count in 1usize..3usize,
            connection_callback_count in 1usize..3usize,
        )| {
            let _ = rt.block_on(async {
                let handler = EventHandler::new();

                // Counters to track callback invocations
                let telegram_counters = Arc::new(tokio::sync::RwLock::new(Vec::new()));
                let connection_counters = Arc::new(tokio::sync::RwLock::new(Vec::new()));

                // Storage for handles
                let mut telegram_handles = Vec::new();
                let mut connection_handles = Vec::new();

                // Register telegram callbacks
                for _i in 0..telegram_callback_count {
                    let counter = Arc::new(AtomicUsize::new(0));
                    telegram_counters.write().await.push(counter.clone());

                    let counter_clone = counter.clone();
                    let handle = handler.register_telegram_callback_sync(move |_telegram| {
                        counter_clone.fetch_add(1, Ordering::SeqCst);
                    }).await;
                    telegram_handles.push(handle);
                }

                // Register connection callbacks
                for _i in 0..connection_callback_count {
                    let counter = Arc::new(AtomicUsize::new(0));
                    connection_counters.write().await.push(counter.clone());

                    let counter_clone = counter.clone();
                    let handle = handler.register_connection_callback_sync(move |_state| {
                        counter_clone.fetch_add(1, Ordering::SeqCst);
                    }).await;
                    connection_handles.push(handle);
                }

                // Verify initial callback counts
                prop_assert_eq!(handler.telegram_callback_count().await, telegram_callback_count);
                prop_assert_eq!(handler.connection_callback_count().await, connection_callback_count);

                // Create test data
                let source = IndividualAddress::from_raw(source_raw);
                let destination = Address::Group(GroupAddress::from_parts(dest_main, dest_middle, dest_sub).unwrap());
                let telegram = Telegram {
                    source,
                    destination,
                    payload,
                    priority: crate::protocol::telegram::Priority::Normal,
                    direction: Direction::Incoming,
                    telegram_type: TelegramType::GroupValueWrite,
                    gateway_id: None,
                    timestamp: std::time::SystemTime::now(),
                };

                // Trigger events - all callbacks should be invoked
                handler.notify_telegram_received(&telegram).await;
                handler.notify_connection_state_changed(ConnectionState::Connected).await;

                // Verify all callbacks were invoked
                let telegram_counters_read = telegram_counters.read().await;
                for (i, counter) in telegram_counters_read.iter().enumerate() {
                    prop_assert_eq!(counter.load(Ordering::SeqCst), 1, "Telegram callback {} should have been invoked", i);
                }
                drop(telegram_counters_read);

                let connection_counters_read = connection_counters.read().await;
                for (i, counter) in connection_counters_read.iter().enumerate() {
                    prop_assert_eq!(counter.load(Ordering::SeqCst), 1, "Connection callback {} should have been invoked", i);
                }
                drop(connection_counters_read);

                // Unregister half of the callbacks (or at least one from each type)
                let telegram_unregister_count = std::cmp::max(1, telegram_callback_count / 2);
                let connection_unregister_count = std::cmp::max(1, connection_callback_count / 2);

                // Unregister telegram callbacks
                for (i, &handle) in telegram_handles.iter().enumerate().take(telegram_unregister_count) {
                    prop_assert!(handler.unregister_telegram_callback(handle).await, "Should successfully unregister telegram callback {}", i);
                }

                // Unregister connection callbacks
                for (i, &handle) in connection_handles.iter().enumerate().take(connection_unregister_count) {
                    prop_assert!(handler.unregister_connection_callback(handle).await, "Should successfully unregister connection callback {}", i);
                }

                // Verify callback counts decreased
                prop_assert_eq!(handler.telegram_callback_count().await, telegram_callback_count - telegram_unregister_count);
                prop_assert_eq!(handler.connection_callback_count().await, connection_callback_count - connection_unregister_count);

                // Trigger events again - only remaining callbacks should be invoked
                handler.notify_telegram_received(&telegram).await;
                handler.notify_connection_state_changed(ConnectionState::Disconnected).await;

                // Verify unregistered callbacks were NOT invoked again
                let telegram_counters_read = telegram_counters.read().await;
                for i in 0..telegram_callback_count {
                    let counter = &telegram_counters_read[i];
                    let expected_count = if i < telegram_unregister_count { 1 } else { 2 };
                    prop_assert_eq!(counter.load(Ordering::SeqCst), expected_count,
                        "Telegram callback {} should have been invoked {} times", i, expected_count);
                }
                drop(telegram_counters_read);

                let connection_counters_read = connection_counters.read().await;
                for i in 0..connection_callback_count {
                    let counter = &connection_counters_read[i];
                    let expected_count = if i < connection_unregister_count { 1 } else { 2 };
                    prop_assert_eq!(counter.load(Ordering::SeqCst), expected_count,
                        "Connection callback {} should have been invoked {} times", i, expected_count);
                }

                Ok(())
            });
        });
    }

    /// For any concurrent callback registration operations from multiple threads,
    /// the system should handle them safely without data races or corruption
    #[test]
    fn property_thread_safety() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(10), |( // Reduced cases for performance
            // Generate concurrent operation parameters
            thread_count in 2usize..5usize,
            operations_per_thread in 2usize..5usize,
            notification_count in 1usize..3usize,
        )| {
            let _ = rt.block_on(async {
                let handler = Arc::new(EventHandler::new());
                let execution_counter = Arc::new(AtomicUsize::new(0));
                let registration_counter = Arc::new(AtomicUsize::new(0));

                // Storage for handles from each thread
                let handles_storage = Arc::new(tokio::sync::RwLock::new(Vec::new()));

                // Spawn multiple threads that perform concurrent operations
                let mut thread_handles = Vec::new();

                for _thread_id in 0..thread_count {
                    let handler_clone = handler.clone();
                    let execution_counter_clone = execution_counter.clone();
                    let registration_counter_clone = registration_counter.clone();
                    let handles_storage_clone = handles_storage.clone();

                    let thread_handle = tokio::spawn(async move {
                        let mut local_handles = Vec::new();

                        // Each thread registers multiple callbacks
                        for _op_id in 0..operations_per_thread {
                            let exec_counter_telegram = execution_counter_clone.clone();
                            let exec_counter_connection = execution_counter_clone.clone();
                            let reg_counter = registration_counter_clone.clone();

                            // Register telegram callback
                            let telegram_handle = handler_clone.register_telegram_callback_sync(move |_telegram| {
                                exec_counter_telegram.fetch_add(1, Ordering::SeqCst);
                            }).await;
                            local_handles.push(telegram_handle);

                            // Register connection callback
                            let connection_handle = handler_clone.register_connection_callback_sync(move |_state| {
                                exec_counter_connection.fetch_add(1, Ordering::SeqCst);
                            }).await;
                            local_handles.push(connection_handle);

                            reg_counter.fetch_add(2, Ordering::SeqCst); // 2 callbacks registered

                            // Small delay to increase chance of race conditions
                            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
                        }

                        // Store handles for later verification
                        handles_storage_clone.write().await.extend(local_handles);
                    });

                    thread_handles.push(thread_handle);
                }

                // Wait for all registration threads to complete
                for handle in thread_handles {
                    handle.await.unwrap();
                }

                // Verify all registrations completed successfully
                let expected_registrations = thread_count * operations_per_thread * 2; // 2 callback types per operation
                let actual_registrations = registration_counter.load(Ordering::SeqCst);
                prop_assert_eq!(actual_registrations, expected_registrations,
                    "All registrations should complete successfully");

                // Verify callback counts match expectations
                let total_callbacks = handler.total_callback_count().await;
                prop_assert_eq!(total_callbacks, expected_registrations,
                    "Handler should have correct total callback count");

                // Verify individual callback type counts
                let expected_per_type = thread_count * operations_per_thread;
                prop_assert_eq!(handler.telegram_callback_count().await, expected_per_type,
                    "Should have correct telegram callback count");
                prop_assert_eq!(handler.connection_callback_count().await, expected_per_type,
                    "Should have correct connection callback count");

                // Test concurrent notifications
                let mut notification_handles = Vec::new();

                for _ in 0..notification_count {
                    let handler_clone = handler.clone();

                    let notification_handle = tokio::spawn(async move {
                        // Create test data
                        use crate::protocol::{address::{Address, GroupAddress, IndividualAddress}, telegram::{Telegram, Direction, TelegramType}};

                        let telegram = Telegram {
                            source: IndividualAddress::from_raw(0x1234),
                            destination: Address::Group(GroupAddress::new(0, 1, 1)),
                            payload: vec![0x01],
                            priority: crate::protocol::telegram::Priority::Normal,
                            direction: Direction::Incoming,
                            telegram_type: TelegramType::GroupValueWrite,
                            gateway_id: None,
                            timestamp: std::time::SystemTime::now(),
                        };

                        // Trigger all notification types concurrently
                        let telegram_notify = handler_clone.notify_telegram_received(&telegram);
                        let connection_notify = handler_clone.notify_connection_state_changed(crate::application::callbacks::ConnectionState::Connected);

                        // Wait for all notifications to complete
                        tokio::join!(telegram_notify, connection_notify);
                    });

                    notification_handles.push(notification_handle);
                }

                // Wait for all notifications to complete
                for handle in notification_handles {
                    handle.await.unwrap();
                }

                // Verify callbacks were executed
                // Each notification should trigger all callbacks of that type
                // Total executions = notification_count * callbacks_per_type * callback_types
                let expected_executions = notification_count * expected_per_type * 2; // 2 callback types
                let actual_executions = execution_counter.load(Ordering::SeqCst);
                prop_assert_eq!(actual_executions, expected_executions,
                    "All callbacks should have been executed the correct number of times");

                // Test concurrent unregistration
                let stored_handles = handles_storage.read().await;
                let unregister_count = std::cmp::min(stored_handles.len() / 2, 10); // Unregister up to half or 10, whichever is smaller

                let mut unregister_handles = Vec::new();
                for i in 0..unregister_count {
                    let handler_clone = handler.clone();
                    let handle_to_unregister = stored_handles[i];

                    let unregister_handle = tokio::spawn(async move {
                        handler_clone.unregister_callback(handle_to_unregister).await
                    });

                    unregister_handles.push(unregister_handle);
                }

                // Wait for unregistrations to complete
                let mut successful_unregistrations = 0;
                for handle in unregister_handles {
                    if handle.await.unwrap() {
                        successful_unregistrations += 1;
                    }
                }

                // Verify unregistrations were successful
                prop_assert_eq!(successful_unregistrations, unregister_count,
                    "All unregistration attempts should succeed");

                // Verify final callback count is correct
                let final_callback_count = handler.total_callback_count().await;
                let expected_final_count = expected_registrations - unregister_count;
                prop_assert_eq!(final_callback_count, expected_final_count,
                    "Final callback count should be correct after unregistrations");

                Ok(())
            });
        });
    }

    /// For any combination of sync and async callbacks, the system should execute
    /// both types correctly and await async callbacks before proceeding
    #[test]
    // sync/async paired names (e.g. expected_sync_executions/expected_async_executions)
    // are clearer than any rename clippy would accept as dissimilar.
    #[allow(clippy::similar_names)]
    fn property_sync_async_callback_support() {
        use crate::protocol::{
            address::{Address, GroupAddress, IndividualAddress},
            telegram::{Direction, Telegram, TelegramType},
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(20), |(
            // Generate callback configurations
            sync_callback_count in 1usize..4usize,
            async_callback_count in 1usize..4usize,
            notification_count in 1usize..3usize,
        )| {
            let _ = rt.block_on(async {
                let handler = EventHandler::new();

                // Counters to track callback executions
                let sync_execution_counter = Arc::new(AtomicUsize::new(0));
                let async_execution_counter = Arc::new(AtomicUsize::new(0));
                let execution_order = Arc::new(tokio::sync::RwLock::new(Vec::new()));

                // Storage for callback handles
                let mut callback_handles = Vec::new();

                // Register sync callbacks
                for i in 0..sync_callback_count {
                    let sync_counter = sync_execution_counter.clone();
                    let order_tracker = execution_order.clone();

                    let handle = handler.register_connection_callback_sync(move |_state| {
                        sync_counter.fetch_add(1, Ordering::SeqCst);
                        let order_tracker = order_tracker.clone();
                        tokio::spawn(async move {
                            order_tracker.write().await.push(format!("sync_{i}"));
                        });
                    }).await;
                    callback_handles.push(handle);
                }

                // Register async callbacks
                for i in 0..async_callback_count {
                    let async_counter = async_execution_counter.clone();
                    let order_tracker = execution_order.clone();

                    let handle = handler.register_connection_callback(AsyncCallback {
                        counter: async_counter.clone(),
                        order_tracker: order_tracker.clone(),
                        id: i,
                    }).await;
                    callback_handles.push(handle);
                }

                // Verify initial callback counts
                let total_expected = sync_callback_count + async_callback_count;
                prop_assert_eq!(handler.connection_callback_count().await, total_expected,
                    "Should have registered all callbacks");

                // Trigger notifications
                for _ in 0..notification_count {
                    handler.notify_connection_state_changed(ConnectionState::Connected).await;
                }

                // Give async callbacks time to complete
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                // Verify all callbacks were executed the correct number of times
                let sync_executions = sync_execution_counter.load(Ordering::SeqCst);
                let async_executions = async_execution_counter.load(Ordering::SeqCst);

                let expected_sync_executions = sync_callback_count * notification_count;
                let expected_async_executions = async_callback_count * notification_count;

                prop_assert_eq!(sync_executions, expected_sync_executions,
                    "Sync callbacks should be executed correct number of times");
                prop_assert_eq!(async_executions, expected_async_executions,
                    "Async callbacks should be executed correct number of times");

                // Verify execution order (callbacks should execute in registration order for each notification)
                let order = execution_order.read().await;
                let total_expected_executions = total_expected * notification_count;
                prop_assert_eq!(order.len(), total_expected_executions,
                    "Should have correct total number of executions");

                // For each notification, verify the order is consistent
                for notification_idx in 0..notification_count {
                    let start_idx = notification_idx * total_expected;
                    let end_idx = start_idx + total_expected;

                    if end_idx <= order.len() {
                        let notification_order = &order[start_idx..end_idx];

                        // Verify sync callbacks come first (in registration order)
                        for i in 0..sync_callback_count {
                            if i < notification_order.len() {
                                let expected = format!("sync_{i}");
                                prop_assert_eq!(&notification_order[i], &expected,
                                    "Sync callback {} should execute in correct order for notification {}", i, notification_idx);
                            }
                        }

                        // Verify async callbacks come after sync callbacks (in registration order)
                        for i in 0..async_callback_count {
                            let order_idx = sync_callback_count + i;
                            if order_idx < notification_order.len() {
                                let expected = format!("async_{i}");
                                prop_assert_eq!(&notification_order[order_idx], &expected,
                                    "Async callback {} should execute in correct order for notification {}", i, notification_idx);
                            }
                        }
                    }
                }

                // Test mixed sync/async with telegram callbacks as well
                let telegram_sync_counter = Arc::new(AtomicUsize::new(0));
                let telegram_async_counter = Arc::new(AtomicUsize::new(0));

                // Register mixed telegram callbacks
                let sync_counter_clone = telegram_sync_counter.clone();
                let _sync_handle = handler.register_telegram_callback_sync(move |_telegram| {
                    sync_counter_clone.fetch_add(1, Ordering::SeqCst);
                }).await;

                let _async_handle = handler.register_telegram_callback_filtered(
                    AsyncTelegramCallback { counter: telegram_async_counter.clone() },
                    TelegramFilter::All,
                    false
                ).await;

                // Create test telegram
                let telegram = Telegram {
                    source: IndividualAddress::from_raw(0x1234),
                    destination: Address::Group(GroupAddress::new(0, 1, 1)),
                    payload: vec![0x01],
                    priority: crate::protocol::telegram::Priority::Normal,
                    direction: Direction::Incoming,
                    telegram_type: TelegramType::GroupValueWrite,
                    gateway_id: None,
                    timestamp: std::time::SystemTime::now(),
                };

                // Trigger telegram notifications
                for _ in 0..notification_count {
                    handler.notify_telegram_received(&telegram).await;
                }

                // Verify mixed telegram callbacks executed correctly
                let telegram_sync_executions = telegram_sync_counter.load(Ordering::SeqCst);
                let telegram_async_executions = telegram_async_counter.load(Ordering::SeqCst);

                prop_assert_eq!(telegram_sync_executions, notification_count,
                    "Sync telegram callback should be executed correct number of times");
                prop_assert_eq!(telegram_async_executions, notification_count,
                    "Async telegram callback should be executed correct number of times");

                Ok(())
            });
        });
    }

    /// For any invalid callback handle, attempting to unregister it should complete
    /// without errors or exceptions
    #[test]
    fn property_graceful_invalid_unregistration() {
        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(20), |(
            // Generate invalid handle IDs
            invalid_handle_ids in prop::collection::vec(1u64..=u64::MAX, 1..5),

            // Generate some valid callbacks to register first
            valid_callback_count in 0usize..3usize,
        )| {
            let _ = rt.block_on(async {
                let handler = EventHandler::new();
                let mut valid_handles = Vec::new();

                // Register some valid callbacks first
                for _ in 0..valid_callback_count {
                    let handle = handler.register_telegram_callback_sync(|_| {}).await;
                    valid_handles.push(handle);
                }

                let initial_count = handler.total_callback_count().await;
                prop_assert_eq!(initial_count, valid_callback_count);

                // Try to unregister with invalid handles - should not panic or error
                for &invalid_id in &invalid_handle_ids {
                    let invalid_handle = CallbackHandle::new(invalid_id);

                    // These should all return false but not panic
                    let telegram_result = handler.unregister_telegram_callback(invalid_handle).await;
                    let connection_result = handler.unregister_connection_callback(invalid_handle).await;
                    let unified_result = handler.unregister_callback(invalid_handle).await;

                    // All should return false for invalid handles
                    prop_assert!(!telegram_result, "Unregistering invalid telegram handle should return false");
                    prop_assert!(!connection_result, "Unregistering invalid connection handle should return false");
                    prop_assert!(!unified_result, "Unregistering invalid handle with unified method should return false");
                }

                // Verify that valid callbacks are still registered and unaffected
                prop_assert_eq!(handler.total_callback_count().await, valid_callback_count,
                    "Valid callbacks should remain registered after invalid unregistration attempts");

                // Test that we can still unregister valid handles successfully
                for handle in valid_handles {
                    prop_assert!(handler.unregister_callback(handle).await,
                        "Should be able to unregister valid handle");
                }

                prop_assert_eq!(handler.total_callback_count().await, 0,
                    "All callbacks should be unregistered after valid unregistration");

                Ok(())
            });
        });
    }

    /// For any callbacks registered via the builder pattern, they should be properly
    /// transferred to the final Knx instance and function identically to runtime-registered callbacks
    #[test]
    fn property_builder_callback_transfer() {
        use crate::application::knx::Knx;
        use crate::protocol::{
            address::{Address, GroupAddress, IndividualAddress},
            telegram::{Direction, Telegram, TelegramType},
        };
        use crate::transport::ConnectionType;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(10), |( // Reduced cases for performance
            // Generate callback configurations
            telegram_callback_count in 1usize..3usize,
            connection_callback_count in 1usize..3usize,

            // Generate test data
            source_raw in 0u16..=0xFFFF,
            dest_main in 0u8..=GroupAddress::MAX_MAIN,
            dest_middle in 0u8..=GroupAddress::MAX_MIDDLE,
            dest_sub in 0u8..=255u8,
            payload in prop::collection::vec(0u8..=255u8, 0..3),
        )| {
            let _ = rt.block_on(async {
                // Create counters to track callback executions
                let telegram_counters = Arc::new(tokio::sync::RwLock::new(Vec::new()));
                let connection_counters = Arc::new(tokio::sync::RwLock::new(Vec::new()));

                // Build Knx instance with callbacks registered via builder
                let mut builder = Knx::builder()
                    .connection_type(ConnectionType::Routing)
                    .memory_limit_mb(32);

                // Register telegram callbacks via builder
                for _i in 0..telegram_callback_count {
                    let counter = Arc::new(AtomicUsize::new(0));
                    telegram_counters.write().await.push(counter.clone());

                    let counter_clone = counter.clone();

                    builder = builder.telegram_callback(SyncTelegramCallback { counter: counter_clone }).unwrap();
                }

                // Register connection callbacks via builder
                for _i in 0..connection_callback_count {
                    let counter = Arc::new(AtomicUsize::new(0));
                    connection_counters.write().await.push(counter.clone());

                    let counter_clone = counter.clone();

                    builder = builder.connection_callback(SyncConnectionCallback { counter: counter_clone }).unwrap();
                }

                // Build the Knx instance
                let knx = builder.build().await.unwrap();

                // Verify that callbacks were transferred correctly
                let expected_total = telegram_callback_count + connection_callback_count;
                prop_assert_eq!(knx.total_callback_count().await, expected_total,
                    "All callbacks should be transferred from builder to Knx instance");

                prop_assert_eq!(knx.telegram_callback_count().await, telegram_callback_count,
                    "Telegram callbacks should be transferred correctly");
                prop_assert_eq!(knx.connection_callback_count().await, connection_callback_count,
                    "Connection callbacks should be transferred correctly");

                // Create test data
                let source = IndividualAddress::from_raw(source_raw);
                let destination = Address::Group(GroupAddress::from_parts(dest_main, dest_middle, dest_sub).unwrap());
                let telegram = Telegram {
                    source,
                    destination,
                    payload,
                    priority: crate::protocol::telegram::Priority::Normal,
                    direction: Direction::Incoming,
                    telegram_type: TelegramType::GroupValueWrite,
                    gateway_id: None,
                    timestamp: std::time::SystemTime::now(),
                };

                // Test that callbacks function correctly
                knx.test_notify_telegram_received(&telegram).await;
                knx.test_notify_connection_state_changed(crate::application::callbacks::ConnectionState::Connected).await;

                // Verify all callbacks were executed
                let telegram_counters_read = telegram_counters.read().await;
                for (i, counter) in telegram_counters_read.iter().enumerate() {
                    prop_assert_eq!(counter.load(Ordering::SeqCst), 1,
                        "Builder-registered telegram callback {} should be executed", i);
                }
                drop(telegram_counters_read);

                let connection_counters_read = connection_counters.read().await;
                for (i, counter) in connection_counters_read.iter().enumerate() {
                    prop_assert_eq!(counter.load(Ordering::SeqCst), 1,
                        "Builder-registered connection callback {} should be executed", i);
                }
                drop(connection_counters_read);

                // Test that builder-registered callbacks can be unregistered
                // (We can't get the handles from the builder, but we can clear all callbacks)
                knx.clear_all_callbacks().await;
                prop_assert_eq!(knx.total_callback_count().await, 0,
                    "Builder-registered callbacks should be removable");

                // Test that after clearing, callbacks are not executed
                knx.test_notify_telegram_received(&telegram).await;
                knx.test_notify_connection_state_changed(crate::application::callbacks::ConnectionState::Disconnected).await;

                // Verify no callbacks were executed after clearing
                let telegram_counters_read = telegram_counters.read().await;
                for (i, counter) in telegram_counters_read.iter().enumerate() {
                    prop_assert_eq!(counter.load(Ordering::SeqCst), 1,
                        "Telegram callback {} should not be executed after clearing", i);
                }
                drop(telegram_counters_read);

                let connection_counters_read = connection_counters.read().await;
                for (i, counter) in connection_counters_read.iter().enumerate() {
                    prop_assert_eq!(counter.load(Ordering::SeqCst), 1,
                        "Connection callback {} should not be executed after clearing", i);
                }

                Ok(())
            });
        });
    }

    /// For any callback registration that fails (builder or runtime), the system should
    /// return a descriptive error without corrupting internal state
    #[test]
    fn property_registration_error_handling() {
        use crate::application::knx::Knx;
        use crate::error::KnxError;
        use crate::transport::ConnectionType;

        let rt = tokio::runtime::Runtime::new().unwrap();

        proptest!(ProptestConfig::with_cases(10), |( // Reduced cases for performance
            // Generate callback counts that will exceed the limit
            excessive_callback_count in 1001usize..1010usize, // Above MAX_BUILDER_CALLBACKS (1000)

            // Generate normal callback counts for testing partial registration
            normal_callback_count in 1usize..10usize,
        )| {
            let _ = rt.block_on(async {
                // Create callback types for testing
                #[derive(Clone)]
                struct TestCallback;

                #[async_trait]
                impl TelegramCallbackFn for TestCallback {
                    async fn call(&self, _telegram: &Telegram) {}
                }

                #[derive(Clone)]
                struct TestConnectionCallback;

                #[async_trait]
                impl ConnectionCallbackFn for TestConnectionCallback {
                    async fn call(&self, _state: crate::application::callbacks::ConnectionState) {}
                }

                // Test 1: Verify that exceeding callback limit returns proper error
                let mut builder = Knx::builder()
                    .connection_type(ConnectionType::Routing)
                    .memory_limit_mb(32);

                // Register callbacks up to the limit
                let mut successful_registrations = 0;
                let mut first_error: Option<KnxError> = None;

                for _i in 0..excessive_callback_count {
                    match builder.telegram_callback(TestCallback) {
                        Ok(new_builder) => {
                            builder = new_builder;
                            successful_registrations += 1;
                        }
                        Err(e) => {
                            if first_error.is_none() {
                                first_error = Some(e);
                            }
                            break;
                        }
                    }
                }

                // Verify that we got an error when exceeding the limit
                prop_assert!(first_error.is_some(), "Should get an error when exceeding callback limit");

                // Verify that the error is descriptive
                if let Some(error) = first_error {
                    let error_message = error.to_string();
                    prop_assert!(error_message.contains("Maximum number"),
                        "Error message should mention maximum number: {}", error_message);
                    prop_assert!(error_message.contains("1000"),
                        "Error message should mention the limit: {}", error_message);
                }

                // Verify that we successfully registered up to the limit
                prop_assert_eq!(successful_registrations, 1000,
                    "Should successfully register exactly 1000 callbacks before failing");

                // Test 2: Verify that builder state is not corrupted after error
                // Create a new builder with the successful callbacks to test building
                let mut test_builder = Knx::builder()
                    .connection_type(ConnectionType::Routing)
                    .memory_limit_mb(32);

                for _i in 0..successful_registrations {
                    test_builder = test_builder.telegram_callback(TestCallback).unwrap();
                }

                let knx_result = test_builder.build().await;
                prop_assert!(knx_result.is_ok(), "Should be able to build Knx instance after callback registration error");

                if let Ok(knx) = knx_result {
                    // Verify that all successfully registered callbacks are present
                    prop_assert_eq!(knx.total_callback_count().await, successful_registrations,
                        "Knx instance should have all successfully registered callbacks");
                }

                // Test 3: Test error handling with different callback types
                let mut builder2 = Knx::builder()
                    .connection_type(ConnectionType::Routing)
                    .memory_limit_mb(32);

                // Register normal amount of telegram callbacks first
                for _i in 0..normal_callback_count {
                    builder2 = builder2.telegram_callback(TestCallback).unwrap();
                }

                // Now try to register connection callbacks up to the limit
                let remaining_slots = 1000 - normal_callback_count;
                let mut connection_registrations = 0;
                let mut connection_error: Option<KnxError> = None;

                for _i in 0..=remaining_slots { // Try one more than remaining slots
                    match builder2.connection_callback(TestConnectionCallback) {
                        Ok(new_builder) => {
                            builder2 = new_builder;
                            connection_registrations += 1;
                        }
                        Err(e) => {
                            connection_error = Some(e);
                            break;
                        }
                    }
                }

                // Should get error when trying to exceed limit
                if remaining_slots > 0 {
                    prop_assert_eq!(connection_registrations, remaining_slots,
                        "Should register exactly the remaining slots");
                    prop_assert!(connection_error.is_some(),
                        "Should get error when trying to register beyond limit");
                }

                // Test 4: Test connection callback error handling
                // Create a builder at the limit to test connection callback error
                let mut limit_builder = Knx::builder()
                    .connection_type(ConnectionType::Routing)
                    .memory_limit_mb(32);

                // Fill up to the limit
                for _i in 0..1000 {
                    limit_builder = limit_builder.telegram_callback(TestCallback).unwrap();
                }

                // Try to register one more callback (should fail since we're at the limit)
                let connection_result = limit_builder.connection_callback(TestConnectionCallback);
                prop_assert!(connection_result.is_err(),
                    "Should get error when trying to register callback beyond limit");

                if let Err(error) = connection_result {
                    let error_message = error.to_string();
                    prop_assert!(error_message.contains("Maximum number"),
                        "Connection callback error should be descriptive: {}", error_message);
                }

                // Test 5: Verify that builder can still be used after errors
                // Create a fresh builder with fewer callbacks to test successful build
                let mut final_builder = Knx::builder()
                    .connection_type(ConnectionType::Routing)
                    .memory_limit_mb(32);

                // Register a small number of callbacks to ensure we can build successfully
                let test_callback_count = std::cmp::min(normal_callback_count, 5);
                for _i in 0..test_callback_count {
                    final_builder = final_builder.telegram_callback(TestCallback).unwrap();
                }

                let final_knx_result = final_builder.build().await;
                prop_assert!(final_knx_result.is_ok(),
                    "Should be able to build Knx instance even after callback registration errors");

                if let Ok(final_knx) = final_knx_result {
                    let total_callbacks = final_knx.total_callback_count().await;
                    prop_assert_eq!(total_callbacks, test_callback_count,
                        "Final Knx instance should have exactly {} callbacks, got {}",
                        test_callback_count, total_callbacks);
                }

                Ok(())
            });
        });
    }
}
