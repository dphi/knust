//! Main Knx library interface.

use crate::application::callbacks::{
    CallbackHandle, ConnectionCallbackFn, EventHandler, TelegramCallbackFn, TelegramFilter,
};
#[cfg(feature = "secure")]
use crate::error::SecurityError;
use crate::error::{ConfigurationError, DeviceError, KnxError, Result};
use crate::log_application;
use crate::logging::{Component, LogLevel, Timer};
use crate::memory::{MemoryMonitor, PerformanceOptimizer};
use crate::protocol::Telegram;
#[cfg(feature = "dpt")]
use crate::protocol::TelegramPriority;
#[cfg(feature = "dpt")]
use crate::protocol::dpt::{
    DptType, DptValue, DptView, GroupAddressRegistry, GroupAddressState, WriteValue,
};
use crate::protocol::{Address, GroupAddress, TelegramType};
#[cfg(feature = "secure")]
use crate::security::KeyRing;
use crate::transport::{
    Connection, ConnectionConfig, ConnectionType, ReceiveLimitConfig, ReceiveRateLimiter,
    ReceiveStats, RoutingConnection, Tunnel, queue::TelegramQueue,
};
use std::sync::Arc;
#[cfg(feature = "secure")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

type TaskSlot = Arc<RwLock<Option<JoinHandle<()>>>>;

/// Control events for connection management
#[derive(Debug, Clone)]
pub enum ConnectionControlEvent {
    /// Tunnel was lost (e.g., `DisconnectRequest` received from server)
    TunnelLost { channel_id: u8, reason: String },
    /// Request to send a `DisconnectResponse`
    SendDisconnectResponse { channel_id: u8 },
}

struct ConnectionControlMessage {
    event: ConnectionControlEvent,
    connection: Arc<dyn Connection>,
}

// A cancelled read must unregister its callback even though Drop cannot await.
struct ReadCallbackGuard {
    handler: Arc<EventHandler>,
    handle: Option<CallbackHandle>,
}
impl Drop for ReadCallbackGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let handler = self.handler.clone();
            runtime.spawn(async move {
                handler.unregister_callback(handle).await;
            });
        }
    }
}

type ActiveConnection = tokio::sync::watch::Sender<Option<Arc<dyn Connection>>>;

const CEMI_CONFIRMATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Main Knx library interface
///
/// Knx provides a high-level interface for KNX/IP communication.
/// It manages connections and telegram processing with built-in memory
/// management and performance optimization. It has no built-in device
/// abstraction layer — send/receive telegrams directly via `send_telegram`,
/// `read_group_value`, and `register_telegram_callback`, or build your own
/// device-like types on top (see `examples/custom_devices.rs`).
///
/// # Example
///
/// ```rust,no_run
/// use knust::{Knx, ConnectionConfig, ConnectionType};
///
/// #[tokio::main]
/// async fn main() -> Result<(), knust::KnxError> {
///     // Create Knx instance using builder
///     let knx = Knx::builder()
///         .connection_type(ConnectionType::Routing)
///         .memory_limit_mb(64) // Set memory limit
///         .build()
///         .await?;
///     
///     // Connect and start processing
///     knx.connect().await?;
///     knx.start().await?;
///     
///     // ... use the library ...
///     
///     // Cleanup
///     knx.shutdown().await?;
///     Ok(())
/// }
/// ```
// Group address ↔ DPT bindings live in `group_registry` (see
// `group_address`, `group_address_dpt`, `decode_group_value`). Every
// address is bound to exactly one DPT for the lifetime of the registry, so
// a generic bus-monitoring/logging tool built on this crate can auto-decode
// raw telegram payloads once the addresses it cares about are registered.
//
// The registry also tracks last-seen value/time and an optional per-address
// refresh TTL (`set_group_address_refresh`, `group_address_state`) — a
// background task re-reads any address that's gone stale (see
// `start_group_refresh_task`), paced and low-priority so it can't flood the
// bus.
#[derive(Clone)]
pub struct Knx {
    /// Connection configuration
    config: ConnectionConfig,

    #[cfg(feature = "secure")]
    data_secure_keyring: Arc<RwLock<Option<KeyRing>>>,
    #[cfg(feature = "secure")]
    data_secure_sequence: Arc<AtomicU64>,

    /// Active connection
    connection: ActiveConnection,

    /// Library state
    state: Arc<RwLock<KnxState>>,

    /// Shutdown flag for graceful termination
    shutdown_flag: Arc<AtomicBool>,

    /// Notified when the library shuts down (for `run()` to wake from)
    shutdown_notify: Arc<Notify>,
    shutdown_started: Arc<Notify>,
    receiving_stop: Arc<AtomicBool>,
    lifecycle_lock: Arc<tokio::sync::Mutex<()>>,
    individual_request_lock: Arc<tokio::sync::Mutex<()>>,
    active_p2p_sessions:
        Arc<std::sync::Mutex<std::collections::HashSet<crate::protocol::IndividualAddress>>>,
    pending_cemi_confirmation: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,

    /// Handle to the telegram processing task
    processing_task: TaskSlot,

    /// Handle to the telegram receiving task
    receiving_task: TaskSlot,

    /// Outgoing sender and receive-limiter drain tasks.
    sending_task: TaskSlot,
    receive_drain_task: TaskSlot,

    /// Handle to the connection control task
    control_task: TaskSlot,

    /// Handle to the reconnection task (if running)
    reconnect_task: TaskSlot,

    /// Telegram queue for ordered processing
    telegram_queue: Arc<TelegramQueue>,

    /// Memory monitor for tracking resource usage
    memory_monitor: Arc<MemoryMonitor>,

    /// Performance optimizer for hot path optimization
    performance_optimizer: Arc<PerformanceOptimizer>,

    /// Memory cleanup task handle
    cleanup_task: TaskSlot,

    /// Event handler for managing callbacks
    event_handler: Arc<EventHandler>,

    /// Channel for connection control events (tunnel lost, disconnect, etc.)
    control_tx: mpsc::Sender<ConnectionControlMessage>,

    /// Receiver for connection control events (wrapped in Option for taking)
    control_rx: Arc<RwLock<Option<mpsc::Receiver<ConnectionControlMessage>>>>,

    /// Current communication channel ID (for tunneling connections)
    channel_id: Arc<RwLock<Option<u8>>>,

    /// Receive-path rate limiter
    receive_limiter: Option<Arc<ReceiveRateLimiter>>,

    /// Group address → DPT bindings, shared by the typed write path
    /// ([`Knx::group_address`]) and the incoming-telegram decode path
    /// ([`Knx::decode_group_value`]).
    #[cfg(feature = "dpt")]
    group_registry: Arc<GroupAddressRegistry>,

    /// Handle to the group-address refresh task (see
    /// [`Knx::start_group_refresh_task`]).
    #[cfg(feature = "dpt")]
    refresh_task: TaskSlot,
}

/// Maximum number of callbacks that can be registered via builder
const MAX_BUILDER_CALLBACKS: usize = 1000;

/// Stored callback for transfer to Knx instance
enum StoredCallback {
    Telegram {
        callback: Box<dyn TelegramCallbackFn + Send + Sync>,
        filter: TelegramFilter,
        include_outgoing: bool,
    },
    Connection {
        callback: Box<dyn ConnectionCallbackFn + Send + Sync>,
    },
}

struct GroupValueReadCallback {
    address: GroupAddress,
    tx: mpsc::Sender<Vec<u8>>,
}

struct IndividualResponseCallback {
    source: crate::protocol::IndividualAddress,
    destination: crate::protocol::IndividualAddress,
    expected_service: [u8; 2],
    tx: mpsc::Sender<Vec<u8>>,
}

/// A group address bound to DPT `T`, constructible only via
/// [`Knx::group_address`] — holding one is proof that this bus already
/// knows `T` is the DPT for this address.
///
/// [`Self::write`] and [`Self::read`]/[`Self::decode`] all go through `T`'s
/// own [`DptValue::encode`]/[`DptValue::decode`] — the single place that
/// knows how to turn this DPT's bytes into a value and back — so the two
/// directions can't drift apart.
#[cfg(feature = "dpt")]
#[derive(Clone)]
pub struct TypedGroupAddress<T: DptValue> {
    address: GroupAddress,
    dpt: DptType,
    knx: Knx,
    _dpt_value: std::marker::PhantomData<T>,
}

#[cfg(feature = "dpt")]
impl<T: DptValue> TypedGroupAddress<T> {
    #[must_use]
    pub fn address(&self) -> GroupAddress {
        self.address
    }

    #[must_use]
    pub fn dpt(&self) -> DptType {
        self.dpt
    }

    /// Resolve `value` against the bound DPT and send it as a
    /// `GroupValueWrite`, via [`WriteValue`].
    ///
    /// Accepts `T` itself, its plain inner value for DPTs generated by
    /// [`crate::dpt_alias`] (e.g. `bool` for `DPTSwitch`), or a human
    /// string parsed the same way [`DptType::parse`] would (`"on"`,
    /// `"23.5"`, ...) — the string is the only shape that can still fail at
    /// runtime; the others can't resolve to the wrong DPT because their
    /// type already ties them to `T`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`](crate::error::ProtocolError::DptError)
    /// if `value` doesn't resolve to a valid `T` (e.g. an unparseable
    /// string), `T::validate` rejects it, the address was unregistered or
    /// rebound to a different DPT since this handle was created, or the
    /// same errors as [`Knx::send_telegram`] if sending fails.
    pub async fn write(&self, value: impl WriteValue<T>) -> Result<()> {
        self.check_dpt_current()?;
        let telegram = Telegram::group_write_value(self.address, value)?;
        self.knx.send_telegram(&telegram).await
    }

    /// Read the current value and decode it as `T`.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Knx::read_group_value`], or
    /// [`ProtocolError::DptError`](crate::error::ProtocolError::DptError)
    /// if the received payload doesn't decode as `T`, or if the address was
    /// unregistered or rebound to a different DPT since this handle was
    /// created.
    pub async fn read(&self, timeout_duration: std::time::Duration) -> Result<T> {
        self.check_dpt_current()?;
        let bytes = self
            .knx
            .read_group_value(self.address, timeout_duration)
            .await?;
        T::decode(&bytes)
    }

    /// Decode `telegram`'s payload as `T`, if `telegram` is addressed to
    /// this handle's group address.
    ///
    /// Returns `None` if `telegram` targets a different address, or carries
    /// no value (a `GroupValueRead`).
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`](crate::error::ProtocolError::DptError)
    /// if the payload doesn't decode as `T`, or if the address was
    /// unregistered or rebound to a different DPT since this handle was
    /// created.
    #[must_use]
    pub fn decode(&self, telegram: &Telegram) -> Option<Result<T>> {
        if telegram.destination != Address::Group(self.address) {
            return None;
        }
        if telegram.telegram_type == TelegramType::GroupValueRead {
            return None;
        }
        if let Err(e) = self.check_dpt_current() {
            return Some(Err(e));
        }
        Some(T::decode(&telegram.payload))
    }

    /// Re-check that `self.address` is still registered under `self.dpt`,
    /// so a handle whose address was unregistered and rebound to a
    /// different DPT after this handle was created stops reading/writing
    /// under the now-stale DPT.
    fn check_dpt_current(&self) -> Result<()> {
        match self.knx.group_registry.get(self.address) {
            Some(dpt) if dpt == self.dpt => Ok(()),
            Some(dpt) => Err(crate::error::ProtocolError::DptError {
                dpt_type: self.dpt.number_str(),
                details: format!(
                    "group address {} was rebound to DPT {} after this handle was created for DPT {}",
                    self.address,
                    dpt.number_str(),
                    self.dpt.number_str()
                ),
            }
            .into()),
            None => Err(crate::error::ProtocolError::DptError {
                dpt_type: self.dpt.number_str(),
                details: format!(
                    "group address {} is no longer registered (was DPT {})",
                    self.address,
                    self.dpt.number_str()
                ),
            }
            .into()),
        }
    }
}

#[async_trait::async_trait]
impl TelegramCallbackFn for GroupValueReadCallback {
    async fn call(&self, telegram: &Telegram) {
        if telegram.destination == Address::Group(self.address) && !telegram.payload.is_empty() {
            let _ = self.tx.try_send(telegram.payload.clone());
        }
    }
}
#[async_trait::async_trait]
impl TelegramCallbackFn for IndividualResponseCallback {
    async fn call(&self, telegram: &Telegram) {
        if telegram.telegram_type == TelegramType::IndividualData
            && telegram.source == self.source
            && telegram.destination == Address::Individual(self.destination)
            && telegram.payload.starts_with(&self.expected_service)
        {
            let _ = self.tx.try_send(telegram.payload.clone());
        }
    }
}

/// Builder for creating Knx instances with custom configuration
pub struct KnxBuilder {
    config: ConnectionConfig,
    memory_limit_mb: u64,
    max_connections: usize,
    stored_callbacks: Vec<StoredCallback>,
}

#[cfg(feature = "secure")]
fn next_data_secure_sequence(sequence: &AtomicU64) -> Result<u64> {
    let mut current = sequence.load(Ordering::Relaxed);
    loop {
        if current > 0xFFFF_FFFF_FFFF {
            return Err(SecurityError::CryptographicError {
                operation: "group encrypt".to_string(),
                reason: "48-bit sequence number exhausted".to_string(),
            }
            .into());
        }
        match sequence.compare_exchange_weak(
            current,
            current + 1,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ) {
            Ok(_) => return Ok(current),
            Err(actual) => current = actual,
        }
    }
}

impl KnxBuilder {
    /// Create a new builder with default configuration
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ConnectionConfig::default(),
            memory_limit_mb: 64, // Default 64MB memory limit
            max_connections: 10, // Default max 10 connections in pool
            stored_callbacks: Vec::new(),
        }
    }

    /// Set the connection type
    #[must_use]
    pub fn connection_type(mut self, connection_type: ConnectionType) -> Self {
        self.config.connection_type = connection_type;
        self
    }

    /// Set the gateway IP address (required for tunneling)
    #[must_use]
    pub fn gateway_ip(mut self, ip: std::net::IpAddr) -> Self {
        self.config.gateway_ip = Some(ip);
        self
    }

    /// Set the gateway port (default: 3671)
    #[must_use]
    pub fn gateway_port(mut self, port: u16) -> Self {
        self.config.gateway_port = Some(port);
        self
    }

    /// Set the local IP address for binding
    #[must_use]
    pub fn local_ip(mut self, ip: std::net::IpAddr) -> Self {
        self.config.local_ip = Some(ip);
        self
    }

    /// Set the individual address for this client
    #[must_use]
    pub fn individual_address(mut self, address: crate::protocol::IndividualAddress) -> Self {
        self.config.individual_address = address;
        self
    }

    /// Set the connection timeout in milliseconds
    #[must_use]
    pub fn timeout_ms(mut self, timeout: u64) -> Self {
        self.config.timeout_ms = timeout;
        self
    }

    /// Enable or disable automatic reconnection
    #[must_use]
    pub fn auto_reconnect(mut self, enabled: bool) -> Self {
        self.config.auto_reconnect = enabled;
        self
    }

    /// Set memory limit in megabytes (default: 64MB)
    #[must_use]
    pub fn memory_limit_mb(mut self, limit: u64) -> Self {
        self.memory_limit_mb = limit;
        self
    }

    /// Set maximum number of connections in pool (default: 10)
    #[must_use]
    pub fn max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Register a telegram callback
    ///
    /// # Arguments
    /// * `callback` - The callback function to register
    ///
    /// # Returns
    /// The builder instance for method chaining
    ///
    /// # Errors
    /// Returns an error if the maximum number of callbacks has been reached
    pub fn telegram_callback<F>(mut self, callback: F) -> Result<Self>
    where
        F: TelegramCallbackFn + Send + Sync + 'static,
    {
        if self.stored_callbacks.len() >= MAX_BUILDER_CALLBACKS {
            return Err(KnxError::Configuration(
                ConfigurationError::ValidationError {
                    details: format!(
                        "Maximum number of builder callbacks ({MAX_BUILDER_CALLBACKS}) exceeded"
                    ),
                },
            ));
        }

        self.stored_callbacks.push(StoredCallback::Telegram {
            callback: Box::new(callback),
            filter: TelegramFilter::All,
            include_outgoing: false,
        });
        Ok(self)
    }

    /// Register a telegram callback with filtering
    ///
    /// # Arguments
    /// * `callback` - The callback function to register
    /// * `filter` - Filter to limit callback scope
    /// * `include_outgoing` - Whether to include outgoing telegrams
    ///
    /// # Returns
    /// The builder instance for method chaining
    ///
    /// # Errors
    /// Returns an error if the maximum number of callbacks has been reached
    pub fn telegram_callback_filtered<F>(
        mut self,
        callback: F,
        filter: TelegramFilter,
        include_outgoing: bool,
    ) -> Result<Self>
    where
        F: TelegramCallbackFn + Send + Sync + 'static,
    {
        if self.stored_callbacks.len() >= MAX_BUILDER_CALLBACKS {
            return Err(KnxError::Configuration(
                ConfigurationError::ValidationError {
                    details: format!(
                        "Maximum number of builder callbacks ({MAX_BUILDER_CALLBACKS}) exceeded"
                    ),
                },
            ));
        }

        self.stored_callbacks.push(StoredCallback::Telegram {
            callback: Box::new(callback),
            filter,
            include_outgoing,
        });
        Ok(self)
    }

    /// Register a connection state callback
    ///
    /// # Arguments
    /// * `callback` - The callback function to register
    ///
    /// # Returns
    /// The builder instance for method chaining
    ///
    /// # Errors
    /// Returns an error if the maximum number of callbacks has been reached
    pub fn connection_callback<F>(mut self, callback: F) -> Result<Self>
    where
        F: ConnectionCallbackFn + Send + Sync + 'static,
    {
        if self.stored_callbacks.len() >= MAX_BUILDER_CALLBACKS {
            return Err(KnxError::Configuration(
                ConfigurationError::ValidationError {
                    details: format!(
                        "Maximum number of builder callbacks ({MAX_BUILDER_CALLBACKS}) exceeded"
                    ),
                },
            ));
        }

        self.stored_callbacks.push(StoredCallback::Connection {
            callback: Box::new(callback),
        });
        Ok(self)
    }

    /// Build the Knx instance
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Knx::new_with_options`].
    pub async fn build(self) -> Result<Knx> {
        log_application!(
            LogLevel::Info,
            "Building Knx instance with configuration: {:?}",
            self.config.connection_type
        );

        // Log callback statistics
        let telegram_count = self
            .stored_callbacks
            .iter()
            .filter(|cb| matches!(cb, StoredCallback::Telegram { .. }))
            .count();
        let connection_count = self
            .stored_callbacks
            .iter()
            .filter(|cb| matches!(cb, StoredCallback::Connection { .. }))
            .count();

        log_application!(
            LogLevel::Debug,
            "Registering {} callbacks: {} telegram, {} connection",
            self.stored_callbacks.len(),
            telegram_count,
            connection_count
        );

        let knx =
            Knx::new_with_options(self.config, self.memory_limit_mb, self.max_connections).await?;

        // Transfer stored callbacks to the Knx instance
        let mut successful_transfers = 0;
        for stored_callback in self.stored_callbacks {
            match stored_callback {
                StoredCallback::Telegram {
                    callback,
                    filter,
                    include_outgoing,
                } => {
                    knx.event_handler
                        .register_telegram_callback_boxed(callback, filter, include_outgoing)
                        .await;
                    successful_transfers += 1;
                }
                StoredCallback::Connection { callback } => {
                    knx.event_handler
                        .register_connection_callback_boxed(callback)
                        .await;
                    successful_transfers += 1;
                }
            }
        }

        log_application!(
            LogLevel::Info,
            "Successfully transferred {} callbacks to Knx instance",
            successful_transfers
        );

        Ok(knx)
    }
}

impl Default for KnxBuilder {
    fn default() -> Self {
        Self::new()
    }
}

const P2P_CONNECTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);
const P2P_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
const P2P_MAX_RETRANSMISSIONS: usize = 3;

struct P2pTelegramCallback {
    tx: mpsc::Sender<Telegram>,
}

#[async_trait::async_trait]
impl TelegramCallbackFn for P2pTelegramCallback {
    async fn call(&self, telegram: &Telegram) {
        let _ = self.tx.send(telegram.clone()).await;
    }
}

struct P2pCallbackRegistration {
    knx: Knx,
    handle: Option<CallbackHandle>,
    active_sessions:
        Arc<std::sync::Mutex<std::collections::HashSet<crate::protocol::IndividualAddress>>>,
    destination: crate::protocol::IndividualAddress,
}

impl Drop for P2pCallbackRegistration {
    fn drop(&mut self) {
        self.active_sessions
            .lock()
            .unwrap()
            .remove(&self.destination);
        if let Some(handle) = self.handle.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let knx = self.knx.clone();
            runtime.spawn(async move {
                knx.unregister_callback(handle).await;
            });
        }
    }
}

/// One interface object and its discovered property descriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedInterfaceObject {
    pub object_index: u8,
    pub object_type: u16,
    pub properties: Vec<crate::protocol::management::PropertyDescriptionResponse>,
}

/// An established connection-oriented transport link to one individual address.
///
/// Methods require `&mut self`, so requests on one link are serialized and each
/// connected-data sequence has a single owner.
pub struct P2pConnection {
    knx: Knx,
    destination: crate::protocol::IndividualAddress,
    receiver: mpsc::Receiver<Telegram>,
    pending: std::collections::VecDeque<Telegram>,
    callback: Option<P2pCallbackRegistration>,
    send_sequence: u8,
    receive_sequence: u8,
    closed: bool,
}

impl Knx {
    /// Create a new builder for Knx configuration
    #[must_use]
    pub fn builder() -> KnxBuilder {
        KnxBuilder::new()
    }

    /// Create a new Knx instance with the given configuration
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::new_with_options`].
    pub async fn new(config: ConnectionConfig) -> Result<Self> {
        Self::new_with_options(config, 64, 10).await // Default 64MB, 10 connections
    }

    /// Create a new Knx instance with custom memory and connection limits
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError::MissingParameter`] if `config` selects a
    /// tunneling connection type without a `gateway_ip`, or
    /// [`ConfigurationError::InvalidValue`] if `config.timeout_ms` is 0.
    pub async fn new_with_options(
        config: ConnectionConfig,
        memory_limit_mb: u64,
        max_connections: usize,
    ) -> Result<Self> {
        let timer = Timer::start(Component::Application, "knx_new");
        log_application!(LogLevel::Info, "Creating new Knx instance");
        log_application!(
            LogLevel::Debug,
            "Configuration: connection_type={:?}, timeout={}ms, auto_reconnect={}, memory_limit={}MB, max_connections={}",
            config.connection_type,
            config.timeout_ms,
            config.auto_reconnect,
            memory_limit_mb,
            max_connections
        );

        // Validate configuration
        Self::validate_config(&config)?;
        log_application!(LogLevel::Debug, "Configuration validation passed");

        // Initialize memory monitor
        let memory_monitor = Arc::new(MemoryMonitor::new(memory_limit_mb));

        // Initialize performance optimizer
        let performance_optimizer = Arc::new(PerformanceOptimizer::new(memory_monitor.clone()));

        // Initialize event handler
        let event_handler = Arc::new(EventHandler::new());

        // Initialize telegram queue
        let telegram_queue = Arc::new(TelegramQueue::new());

        // Create control channel for connection events
        let (control_tx, control_rx) = mpsc::channel::<ConnectionControlMessage>(32);

        // Create receive rate limiter
        let (receive_limiter, mut receive_limiter_rx) =
            ReceiveRateLimiter::new(ReceiveLimitConfig::default());
        let receive_limiter = Arc::new(receive_limiter);

        // Spawn drain task — telegrams gated through the limiter are forwarded
        // into the telegram queue by the receive path directly; this drain just
        // prevents the internal channel from filling up.
        let tq_for_drain = telegram_queue.clone();
        let receive_drain_handle = tokio::spawn(async move {
            while let Some(telegram) = receive_limiter_rx.recv().await {
                let _ = tq_for_drain.enqueue_incoming(telegram).await;
            }
        });

        #[cfg(feature = "secure")]
        let data_secure_sequence = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let knx = Self {
            config,
            #[cfg(feature = "secure")]
            data_secure_keyring: Arc::new(RwLock::new(None)),
            #[cfg(feature = "secure")]
            data_secure_sequence: Arc::new(AtomicU64::new(data_secure_sequence)),
            connection: tokio::sync::watch::channel(None).0,
            state: Arc::new(RwLock::new(KnxState::Disconnected)),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(Notify::new()),
            shutdown_started: Arc::new(Notify::new()),
            receiving_stop: Arc::new(AtomicBool::new(false)),
            lifecycle_lock: Arc::new(tokio::sync::Mutex::new(())),
            individual_request_lock: Arc::new(tokio::sync::Mutex::new(())),
            active_p2p_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            pending_cemi_confirmation: Arc::new(std::sync::Mutex::new(None)),
            processing_task: Arc::new(RwLock::new(None)),
            receiving_task: Arc::new(RwLock::new(None)),
            sending_task: Arc::new(RwLock::new(None)),
            receive_drain_task: Arc::new(RwLock::new(Some(receive_drain_handle))),
            control_task: Arc::new(RwLock::new(None)),
            reconnect_task: Arc::new(RwLock::new(None)),
            memory_monitor,
            performance_optimizer,
            cleanup_task: Arc::new(RwLock::new(None)),
            event_handler,
            telegram_queue,
            control_tx,
            control_rx: Arc::new(RwLock::new(Some(control_rx))),
            channel_id: Arc::new(RwLock::new(None)),
            receive_limiter: Some(receive_limiter),
            #[cfg(feature = "dpt")]
            group_registry: Arc::new(GroupAddressRegistry::new()),
            #[cfg(feature = "dpt")]
            refresh_task: Arc::new(RwLock::new(None)),
        };

        // Start memory cleanup task
        knx.start_memory_cleanup_task().await?;

        log_application!(LogLevel::Info, "Knx instance created successfully");
        timer.finish_with_message("Knx instance created");

        Ok(knx)
    }

    /// Start the memory cleanup task
    async fn start_memory_cleanup_task(&self) -> Result<()> {
        let memory_monitor = self.memory_monitor.clone();
        let performance_optimizer = self.performance_optimizer.clone();
        let shutdown_flag = self.shutdown_flag.clone();

        let handle = tokio::spawn(async move {
            log_application!(LogLevel::Info, "Memory cleanup task started");

            loop {
                // Check shutdown flag
                if shutdown_flag.load(Ordering::SeqCst) {
                    log_application!(
                        LogLevel::Info,
                        "Shutdown flag set, stopping memory cleanup task"
                    );
                    break;
                }

                // Wait for cleanup interval (5 minutes)
                tokio::time::sleep(std::time::Duration::from_secs(5 * 60)).await;

                // Check if cleanup is needed
                if memory_monitor.should_cleanup().await {
                    log_application!(LogLevel::Debug, "Performing scheduled memory cleanup");
                    memory_monitor.cleanup().await;
                }

                // Perform performance optimization
                performance_optimizer.optimize().await;
            }

            log_application!(LogLevel::Info, "Memory cleanup task stopped");
        });

        {
            let mut task = self.cleanup_task.write().await;
            *task = Some(handle);
        }

        Ok(())
    }

    /// Start the group-address refresh task.
    ///
    /// Each tick, sends at most one `GroupValueRead` — for whichever
    /// registered address has gone longest without a refresh attempt among
    /// those past their configured TTL (see [`Self::set_group_address_refresh`]).
    /// Paced at one per second and tagged [`TelegramPriority::Low`] so a
    /// large stale backlog (e.g. right after startup) trickles out instead
    /// of bursting onto the bus or crowding out real application traffic —
    /// the existing priority queue and rate limiter in
    /// [`crate::transport::queue::TelegramQueue`] take care of the rest.
    #[cfg(feature = "dpt")]
    async fn start_group_refresh_task(&self) -> Result<()> {
        let shutdown_flag = self.shutdown_flag.clone();
        let group_registry = self.group_registry.clone();
        let knx = self.clone();

        let handle = tokio::spawn(async move {
            log_application!(LogLevel::Info, "Group address refresh task started");

            loop {
                if shutdown_flag.load(Ordering::SeqCst) {
                    break;
                }

                tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                if shutdown_flag.load(Ordering::SeqCst) {
                    break;
                }

                if let Some(address) = group_registry.next_stale(std::time::Instant::now()) {
                    let mut read_telegram = Telegram::group_read(address);
                    read_telegram.priority = TelegramPriority::Low;

                    match knx.send_telegram(&read_telegram).await {
                        Ok(()) => group_registry.record_tried(address),
                        Err(e) => {
                            log_application!(
                                LogLevel::Warn,
                                "Failed to send refresh GroupValueRead for {}: {}",
                                address,
                                e
                            );
                        }
                    }
                }
            }

            log_application!(LogLevel::Info, "Group address refresh task stopped");
        });

        {
            let mut task = self.refresh_task.write().await;
            *task = Some(handle);
        }

        Ok(())
    }

    /// Start the connection control task that handles disconnect/reconnect events
    /// Following Python implementation pattern for `tunnel_lost` handling
    async fn start_connection_control_task(&self) -> Result<()> {
        use crate::protocol::knxip::{DisconnectResponse, KnxIpFrame, ServiceType};

        let shutdown_flag = self.shutdown_flag.clone();
        let auto_reconnect = self.config.auto_reconnect;
        let config = self.config.clone();
        let connection_arc = self.connection.clone();
        let state = self.state.clone();
        let event_handler = self.event_handler.clone();
        let channel_id_arc = self.channel_id.clone();
        let reconnect_task = self.reconnect_task.clone();

        // Take the receiver from the stored field
        let mut control_rx = self.control_rx.write().await.take().ok_or_else(|| {
            KnxError::Configuration(ConfigurationError::ValidationError {
                details: "Control receiver already taken".to_string(),
            })
        })?;

        log_application!(LogLevel::Debug, "Starting connection control task");

        let handle = tokio::spawn(async move {
            log_application!(
                LogLevel::Info,
                "Connection control task started (auto_reconnect={})",
                auto_reconnect
            );

            loop {
                tokio::select! {
                    // Check for control events
                    Some(message) = control_rx.recv() => {
                        let connection = message.connection;
                        let is_current = connection_arc.borrow().as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &connection));
                        if !is_current { continue; }
                        match message.event {
                            ConnectionControlEvent::SendDisconnectResponse { channel_id } => {
                                log_application!(LogLevel::Debug, "Sending DisconnectResponse for channel {}", channel_id);

                                // Create and send DisconnectResponse
                                let response = DisconnectResponse::new(channel_id, DisconnectResponse::STATUS_OK);
                                let response_body = response.serialize();
                                let response_frame = KnxIpFrame::new(ServiceType::DisconnectResponse, response_body);
                                let response_data = response_frame.serialize();

                                if let Err(e) = connection.send(&response_data).await {
                                    log_application!(LogLevel::Warn, "Failed to send DisconnectResponse: {}", e);
                                } else {
                                    log_application!(LogLevel::Info, "DisconnectResponse sent for channel {}", channel_id);
                                }


                            }
                            ConnectionControlEvent::TunnelLost { channel_id, reason } => {
                                let removed = connection_arc.send_if_modified(|active| {
                                    if active.as_ref().is_some_and(|current| Arc::ptr_eq(current, &connection)) {
                                        *active = None;
                                        true
                                    } else { false }
                                });
                                if !removed { continue; }
                                log_application!(LogLevel::Warn, "Tunnel lost for channel {}: {}", channel_id, reason);

                                // Update state to disconnected
                                {
                                    let mut s = state.write().await;
                                    *s = KnxState::Disconnected;
                                }

                                // Notify callbacks
                                event_handler.notify_connection_state_changed(
                                    crate::application::callbacks::ConnectionState::Disconnected
                                ).await;

                                *channel_id_arc.write().await = None;
                                if let Err(e) = connection.close().await {
                                    log_application!(LogLevel::Warn, "Failed to close connection: {}", e);
                                }

                                if auto_reconnect && connection_arc.borrow().is_none() {
                                    log_application!(LogLevel::Info, "Auto-reconnect enabled, starting reconnection task");

                                    // Check if reconnection is already in progress
                                    let mut reconnect_task_guard = reconnect_task.write().await;
                                    if reconnect_task_guard.as_ref().is_none_or(|task| task.is_finished()) {
                                        // Start reconnection task (following Python pattern)
                                        let reconnect_handle = Self::start_reconnection_task(
                                            config.clone(),
                                            connection_arc.clone(),
                                            state.clone(),
                                            event_handler.clone(),
                                            channel_id_arc.clone(),
                                            shutdown_flag.clone(),
                                        );

                                        *reconnect_task_guard = Some(reconnect_handle);
                                        log_application!(LogLevel::Debug, "Reconnection task started");
                                    } else {
                                        log_application!(LogLevel::Debug, "Reconnection already in progress");
                                    }
                                } else {
                                    log_application!(LogLevel::Warn, "Auto-reconnect disabled, tunnel connection closed");
                                }
                            }
                        }
                    }
                    // Check shutdown flag periodically
                    () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                        if shutdown_flag.load(Ordering::SeqCst) {
                            log_application!(LogLevel::Info, "Shutdown flag set, stopping connection control task");
                            break;
                        }
                    }
                }
            }

            log_application!(LogLevel::Info, "Connection control task stopped");
        });

        // Store the task handle
        {
            let mut task = self.control_task.write().await;
            *task = Some(handle);
        }

        Ok(())
    }

    fn start_reconnection_task(
        config: ConnectionConfig,
        connection_arc: ActiveConnection,
        state: Arc<RwLock<KnxState>>,
        event_handler: Arc<EventHandler>,
        channel_id_arc: Arc<RwLock<Option<u8>>>,
        shutdown_flag: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let backoff = &config.reconnect_backoff;
            let mut delay_ms = backoff.initial_delay_ms;
            for attempt in 1..=backoff.max_attempts {
                if shutdown_flag.load(Ordering::SeqCst) {
                    break;
                }
                match Self::create_connection(&config).await {
                    Ok(connection) => {
                        if shutdown_flag.load(Ordering::SeqCst) {
                            break;
                        }
                        *channel_id_arc.write().await =
                            Self::establish_connection(&connection, &config);
                        *state.write().await = KnxState::Connected;
                        connection_arc.send_replace(Some(connection));
                        event_handler
                            .notify_connection_state_changed(
                                crate::application::callbacks::ConnectionState::Connected,
                            )
                            .await;
                        return;
                    }
                    Err(error) => {
                        log_application!(
                            LogLevel::Warn,
                            "Reconnect attempt {} failed: {}",
                            attempt,
                            error
                        );
                    }
                }
                if attempt < backoff.max_attempts {
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    delay_ms =
                        ((delay_ms as f64 * backoff.multiplier) as u64).min(backoff.max_delay_ms);
                }
            }
        })
    }

    /// Create a new connection based on configuration (helper for reconnection)
    async fn create_connection(config: &ConnectionConfig) -> Result<Arc<dyn Connection>> {
        match config.connection_type {
            ConnectionType::Tunneling | ConnectionType::SecureTunneling => {
                let gateway_ip =
                    config
                        .gateway_ip
                        .ok_or_else(|| ConfigurationError::MissingParameter {
                            parameter: "gateway_ip".to_string(),
                        })?;

                let gateway_port = config.gateway_port.unwrap_or(3671);
                let gateway_addr = std::net::SocketAddr::new(gateway_ip, gateway_port);

                let mut tunneling_conn = Tunnel::new_udp_with_timeout(
                    gateway_addr,
                    std::time::Duration::from_millis(config.timeout_ms),
                );

                if config.connection_type == ConnectionType::SecureTunneling {
                    #[cfg(feature = "secure")]
                    {
                        let security = config.security.as_ref().ok_or_else(|| {
                            ConfigurationError::MissingParameter {
                                parameter: "security".to_string(),
                            }
                        })?;
                        tunneling_conn.connect_secure(security).await?;
                    }
                    #[cfg(not(feature = "secure"))]
                    {
                        return Err(ConfigurationError::ValidationError {
                            details: "SecureTunneling requires the `secure` feature".to_string(),
                        }
                        .into());
                    }
                } else {
                    // Establish the tunneling connection with KNX/IP protocol handshake
                    tunneling_conn.connect().await?;
                }

                Ok(Arc::new(tunneling_conn))
            }
            ConnectionType::TcpTunneling => {
                let gateway_ip =
                    config
                        .gateway_ip
                        .ok_or_else(|| ConfigurationError::MissingParameter {
                            parameter: "gateway_ip".to_string(),
                        })?;

                let gateway_port = config.gateway_port.unwrap_or(3671);
                let gateway_addr = std::net::SocketAddr::new(gateway_ip, gateway_port);

                let mut tcp_conn = Tunnel::new_tcp_with_timeout(
                    gateway_addr,
                    std::time::Duration::from_millis(config.tcp_config.connect_timeout_ms),
                );

                // Establish the TCP connection
                tcp_conn.connect().await?;

                Ok(Arc::new(tcp_conn))
            }
            ConnectionType::Routing => {
                let routing_conn = RoutingConnection::new(config.local_ip).await?;
                Ok(Arc::new(routing_conn))
            }
            ConnectionType::SecureRouting => Err(ConfigurationError::ValidationError {
                details: "SecureRouting (KNX IP Secure over multicast) is not yet implemented"
                    .to_string(),
            }
            .into()),
        }
    }

    /// Get the channel ID for a newly established connection (helper for reconnection).
    ///
    /// Returns `None` for connection types with no channel ID (Routing), or if
    /// downcasting to the concrete connection type fails — callers should
    /// keep the previous channel ID in that case rather than treat this as authoritative.
    fn establish_connection(
        connection: &Arc<dyn Connection>,
        config: &ConnectionConfig,
    ) -> Option<u8> {
        match config.connection_type {
            ConnectionType::Tunneling | ConnectionType::SecureTunneling => {
                // For tunneling connections, try to get channel ID via downcasting
                if let Some(tunneling_conn) = connection.as_any().downcast_ref::<Tunnel>() {
                    let channel_id = tunneling_conn.channel_id();
                    log_application!(
                        LogLevel::Debug,
                        "Tunneling connection established with channel ID: {}",
                        channel_id
                    );
                    Some(channel_id)
                } else {
                    log_application!(LogLevel::Warn, "Failed to downcast to Tunnel");
                    None
                }
            }
            ConnectionType::TcpTunneling => {
                // Similar for TCP tunneling
                if let Some(tcp_conn) = connection.as_any().downcast_ref::<Tunnel>() {
                    let channel_id = tcp_conn.channel_id();
                    log_application!(
                        LogLevel::Debug,
                        "TCP tunneling connection established with channel ID: {}",
                        channel_id
                    );
                    Some(channel_id)
                } else {
                    log_application!(LogLevel::Warn, "Failed to downcast to Tunnel");
                    None
                }
            }
            ConnectionType::Routing | ConnectionType::SecureRouting => {
                // Routing connections don't have channel IDs
                log_application!(LogLevel::Debug, "Routing connection established");
                None
            }
        }
    }

    /// Connect to the configured KNX/IP network.
    ///
    /// # Errors
    ///
    /// Returns configuration, handshake or transport errors for the selected connection.
    pub async fn connect(&self) -> Result<()> {
        *self.state.write().await = KnxState::Connecting;
        self.notify_connection_state_changed(KnxState::Connecting.into())
            .await;
        match Self::create_connection(&self.config).await {
            Ok(connection) => {
                *self.channel_id.write().await =
                    Self::establish_connection(&connection, &self.config);
                *self.state.write().await = KnxState::Connected;
                self.connection.send_replace(Some(connection));
                self.notify_connection_state_changed(KnxState::Connected.into())
                    .await;
                Ok(())
            }
            Err(error) => {
                *self.state.write().await = KnxState::Error;
                self.notify_connection_state_changed(KnxState::Error.into())
                    .await;
                Err(error)
            }
        }
    }

    /// Disconnect from the KNX/IP network
    ///
    /// # Errors
    ///
    /// Returns the underlying connection's close error, if any.
    pub async fn disconnect(&self) -> Result<()> {
        {
            let mut state = self.state.write().await;
            *state = KnxState::Disconnecting;
        }

        // Notify connection state change callbacks
        self.notify_connection_state_changed(KnxState::Disconnecting.into())
            .await;

        if let Some(connection) = self.connection.send_replace(None) {
            connection.close().await?;
        }

        {
            let mut state = self.state.write().await;
            *state = KnxState::Disconnected;
        }

        // Notify connection state change callbacks
        self.notify_connection_state_changed(KnxState::Disconnected.into())
            .await;

        Ok(())
    }

    /// Enable group Data Secure with group keys and sender sequence baselines.
    #[cfg(feature = "secure")]
    pub async fn set_data_secure_keyring(&self, keyring: KeyRing) {
        *self.data_secure_keyring.write().await = Some(keyring);
    }

    /// Send a telegram to the KNX network.
    ///
    /// `telegram.source` is overwritten with this bus's configured
    /// individual address before sending — callers building a telegram
    /// (e.g. via [`Telegram::group_write`]) never need to know or set it
    /// themselves, and can't accidentally send under the wrong address.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::QueueClosed`](crate::error::TransportError::QueueClosed)
    /// if the outgoing queue has been closed, or
    /// [`TransportError::QueueFull`](crate::error::TransportError::QueueFull)
    /// if it is at capacity.
    pub async fn send_telegram(&self, telegram: &Telegram) -> Result<()> {
        let mut telegram = telegram.clone();
        telegram.source = self.config.individual_address;
        self.telegram_queue.enqueue_outgoing(telegram).await?;
        Ok(())
    }

    async fn send_telegram_acknowledged(&self, telegram: &Telegram) -> Result<()> {
        let mut telegram = telegram.clone();
        telegram.source = self.config.individual_address;
        let (sent, completed) = self
            .telegram_queue
            .enqueue_outgoing_tracked(telegram)
            .await?;
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            sent.await
                .map_err(|_| crate::error::TransportError::QueueClosed)?;
            completed
                .await
                .map_err(|_| crate::error::TransportError::QueueClosed)??;
            Ok::<_, KnxError>(())
        })
        .await
        .map_err(|_| crate::error::TransportError::QueueTimeout { timeout_ms: 10_000 })??;
        Ok(())
    }

    /// Send a connectionless individual-address request and await its matching response.
    ///
    /// The two-byte APCI service prefix identifies the expected response. Requests are
    /// serialized because connectionless responses do not carry a request sequence.
    pub async fn request_individual(
        &self,
        destination: crate::protocol::IndividualAddress,
        request_apdu: Vec<u8>,
        expected_service: [u8; 2],
        timeout_duration: std::time::Duration,
    ) -> Result<Vec<u8>> {
        if request_apdu.len() < 2 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Individual-address APDU must include an APCI service".to_owned(),
            }
            .into());
        }
        let _request_guard = self.individual_request_lock.lock().await;
        let stopping = self.shutdown_started.notified();
        tokio::pin!(stopping);
        stopping.as_mut().enable();
        if self.shutdown_flag.load(Ordering::SeqCst) {
            return Err(crate::error::TransportError::QueueClosed.into());
        }

        let local_address = self.config.individual_address;
        let (tx, mut rx) = mpsc::channel(1);
        let callback = IndividualResponseCallback {
            source: destination,
            destination: local_address,
            expected_service,
            tx,
        };
        let filter = TelegramFilter::Custom(Arc::new(move |telegram| {
            telegram.telegram_type == TelegramType::IndividualData
                && telegram.source == destination
                && telegram.destination == Address::Individual(local_address)
                && telegram.payload.starts_with(&expected_service)
        }));
        let handle = self
            .register_telegram_callback_filtered(callback, filter, false)
            .await;
        let mut callback_guard = ReadCallbackGuard {
            handler: self.event_handler.clone(),
            handle: Some(handle),
        };

        let operation = async {
            let telegram = Telegram::individual_data(destination, request_apdu);
            let (sent, completed) = self
                .telegram_queue
                .enqueue_outgoing_tracked(telegram)
                .await?;
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                tokio::select! {
                    sent = sent => sent.map_err(|_| crate::error::TransportError::QueueClosed.into()),
                    result = completed => result.map_err(|_| crate::error::TransportError::QueueClosed)?,

                }
            })
            .await
            .map_err(|_| crate::error::TransportError::QueueTimeout { timeout_ms: 10_000 })??;

            match tokio::time::timeout(timeout_duration, rx.recv()).await {
                Ok(Some(response)) => Ok(response),
                _ => Err(DeviceError::CommunicationTimeout {
                    device: format!("individual address {destination}"),
                    timeout_ms: timeout_duration.as_millis() as u64,
                }
                .into()),
            }
        };
        let result = tokio::select! {
            result = operation => result,
            _ = &mut stopping => Err(crate::error::TransportError::QueueClosed.into()),
        };
        self.unregister_callback(handle).await;
        callback_guard.handle = None;
        result
    }

    /// Open a connection-oriented transport link to an individual address.
    ///
    /// Only one connected session per peer may be active on a Knx instance.
    /// Connectionless requests remain available through
    /// [`Self::request_individual`].
    pub async fn connect_p2p(
        &self,
        destination: crate::protocol::IndividualAddress,
    ) -> Result<P2pConnection> {
        let active_sessions = self.active_p2p_sessions.clone();
        if !active_sessions.lock().unwrap().insert(destination) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("A P2P session to {destination} is already active"),
            }
            .into());
        }
        let mut callback = P2pCallbackRegistration {
            knx: self.clone(),
            handle: None,
            active_sessions,
            destination,
        };
        let local_address = self.config.individual_address;
        let (tx, receiver) = mpsc::channel(16);
        let filter = TelegramFilter::Custom(Arc::new(move |telegram| {
            telegram.source == destination
                && telegram.destination == Address::Individual(local_address)
                && matches!(
                    telegram.telegram_type,
                    TelegramType::IndividualData | TelegramType::IndividualControl
                )
        }));
        callback.handle = Some(
            self.register_telegram_callback_filtered(P2pTelegramCallback { tx }, filter, false)
                .await,
        );
        let mut connection = P2pConnection {
            knx: self.clone(),
            destination,
            receiver,
            pending: std::collections::VecDeque::new(),
            callback: Some(callback),
            send_sequence: 0,
            receive_sequence: 0,
            closed: false,
        };
        connection
            .send_control(crate::protocol::TpciFrame::Connect)
            .await?;
        connection.wait_until_connected().await?;
        Ok(connection)
    }

    /// Read a group value, starting the response deadline after transmission.
    ///
    /// # Errors
    ///
    /// Returns queue/transport errors, a send timeout if the read cannot be sent
    /// within ten seconds, or a response timeout after `timeout_duration`.
    pub async fn read_group_value(
        &self,
        address: GroupAddress,
        timeout_duration: std::time::Duration,
    ) -> Result<Vec<u8>> {
        let stopping = self.shutdown_started.notified();
        tokio::pin!(stopping);
        stopping.as_mut().enable();
        if self.shutdown_flag.load(Ordering::SeqCst) {
            return Err(crate::error::TransportError::QueueClosed.into());
        }
        let (tx, mut rx) = mpsc::channel(1);
        let handle = self
            .register_telegram_callback(GroupValueReadCallback { address, tx })
            .await;
        let mut guard = ReadCallbackGuard {
            handler: self.event_handler.clone(),
            handle: Some(handle),
        };
        let operation = async {
            let mut read = Telegram::group_read(address);
            read.source = self.config.individual_address;
            let (sent, completed) = self.telegram_queue.enqueue_outgoing_tracked(read).await?;
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                tokio::select! {
                    sent = sent => sent.map_err(|_| crate::error::TransportError::QueueClosed.into()),
                    result = completed => result.map_err(|_| crate::error::TransportError::QueueClosed)?,
                }
            }).await.map_err(|_| crate::error::TransportError::QueueTimeout { timeout_ms: 10_000 })??;
            match tokio::time::timeout(timeout_duration, rx.recv()).await {
                Ok(Some(payload)) => Ok(payload),
                _ => Err(DeviceError::CommunicationTimeout {
                    device: format!("group address {address}"),
                    timeout_ms: timeout_duration.as_millis() as u64,
                }
                .into()),
            }
        };
        let result = tokio::select! {
            result = operation => result,
            _ = &mut stopping => Err(crate::error::TransportError::QueueClosed.into()),
        };
        self.unregister_callback(handle).await;
        guard.handle = None;
        result
    }

    /// Bind `address` to DPT `T` and return a handle for typed reads/writes.
    ///
    /// This is the only way to obtain a [`GroupAddress`](TypedGroupAddress)
    /// — it can't exist without a bus that has already agreed `address` is
    /// DPT `T`. The binding is shared by [`Self::decode_group_value`] and
    /// every other `GroupAddress` handle for the same address; registering
    /// the same `(address, T)` pair again just returns a fresh handle to
    /// the existing binding.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`](crate::error::ProtocolError::DptError)
    /// if `address` is already bound to a different DPT.
    #[cfg(feature = "dpt")]
    pub fn group_address<T: DptValue>(
        &self,
        address: GroupAddress,
    ) -> Result<TypedGroupAddress<T>> {
        let dpt = T::dpt_type()?;
        self.register_group_address_dyn(address, dpt)?;
        Ok(TypedGroupAddress {
            address,
            dpt,
            knx: self.clone(),
            _dpt_value: std::marker::PhantomData,
        })
    }

    /// Bind `address` to `dpt` without a compile-time `T`.
    ///
    /// Prefer [`Self::group_address`] when the DPT is known in code; this
    /// is for callers where it's only known at runtime — bulk-loading
    /// bindings from an ETS export (see [`crate::config::parse_ets_csv`]),
    /// or a bus monitor that wants [`Self::decode_group_value`] to work for
    /// addresses it discovers dynamically.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`](crate::error::ProtocolError::DptError)
    /// if `address` is already bound to a *different* DPT.
    #[cfg(feature = "dpt")]
    pub fn register_group_address_dyn(&self, address: GroupAddress, dpt: DptType) -> Result<()> {
        self.group_registry.register(address, dpt)
    }

    /// Register every group address from a parsed ETS CSV export
    /// (`crate::config::parse_ets_csv`) into this bus's registry in one
    /// call.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`](crate::error::ProtocolError::DptError)
    /// on the first address already bound to a *different* DPT (by an
    /// earlier import, or a [`Self::group_address`] call). Entries already
    /// processed stay registered — this doesn't roll back a partial import.
    #[cfg(feature = "ets")]
    pub fn register_group_addresses_from_ets(
        &self,
        configs: &std::collections::HashMap<GroupAddress, crate::config::GroupAddressConfig>,
    ) -> Result<()> {
        for (address, config) in configs {
            self.register_group_address_dyn(*address, config.dpt)?;
        }
        Ok(())
    }

    /// The DPT `address` is bound to, if it was registered via
    /// [`Self::group_address`] or [`Self::register_group_address_dyn`].
    #[cfg(feature = "dpt")]
    #[must_use]
    pub fn group_address_dpt(&self, address: GroupAddress) -> Option<DptType> {
        self.group_registry.get(address)
    }

    /// Snapshot every group address currently bound to a DPT. Order is
    /// unspecified.
    #[cfg(feature = "dpt")]
    #[must_use]
    pub fn registered_group_addresses(&self) -> Vec<(GroupAddress, DptType)> {
        self.group_registry.entries()
    }

    /// Configure a refresh TTL for `address`: if no real value is observed
    /// for it within `ttl`, the background refresh task
    /// ([`Self::start_group_refresh_task`]) sends a `GroupValueRead` for it.
    ///
    /// The refresh task only runs once [`Self::start`] has been called; if
    /// `start()` hasn't run yet, this still records the TTL (it'll take
    /// effect once `start()` runs) but logs a warning, since it's easy to
    /// otherwise assume the refresh is already active.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`](crate::error::ProtocolError::DptError)
    /// if `address` isn't registered — see [`Self::group_address`] or
    /// [`Self::register_group_address_dyn`].
    #[cfg(feature = "dpt")]
    pub fn set_group_address_refresh(
        &self,
        address: GroupAddress,
        ttl: std::time::Duration,
    ) -> Result<()> {
        self.group_registry.set_refresh_ttl(address, Some(ttl))?;
        if let Ok(guard) = self.refresh_task.try_read() {
            if guard.is_none() {
                log_application!(
                    LogLevel::Warn,
                    "set_group_address_refresh({}) configured before Knx::start() — refresh won't run until start() is called",
                    address
                );
            }
        }
        Ok(())
    }

    /// Stop refreshing `address` — undoes [`Self::set_group_address_refresh`].
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`](crate::error::ProtocolError::DptError)
    /// if `address` isn't registered.
    #[cfg(feature = "dpt")]
    pub fn clear_group_address_refresh(&self, address: GroupAddress) -> Result<()> {
        self.group_registry.set_refresh_ttl(address, None)
    }

    /// Snapshot of `address`'s DPT, refresh TTL, and last known value — the
    /// last real `GroupValueWrite`/`GroupValueResponse` observed for it, if
    /// any. This is a local, synchronous read; it never touches the bus.
    /// Compare [`Self::read_group_value`], which always round-trips.
    #[cfg(feature = "dpt")]
    #[must_use]
    pub fn group_address_state(&self, address: GroupAddress) -> Option<GroupAddressState> {
        self.group_registry.state(address)
    }

    /// Remove `address`'s DPT binding, if any, returning the DPT it was
    /// bound to.
    ///
    /// `address` becomes free to register under a *different* DPT.
    /// [`Self::group_address_dpt`]/[`Self::decode_group_value`] stop
    /// resolving it until it's registered again, and existing
    /// [`TypedGroupAddress`] handles for `address` re-check the registry on
    /// every use — they start erroring once unregistered, and error rather
    /// than silently using their original DPT if `address` is rebound to a
    /// different one.
    #[cfg(feature = "dpt")]
    #[must_use]
    pub fn unregister_group_address(&self, address: GroupAddress) -> Option<DptType> {
        self.group_registry.unregister(address)
    }

    /// Decode an incoming telegram's payload using the DPT bound to its
    /// destination address.
    ///
    /// Returns `None` if `telegram` isn't a group telegram, carries no value
    /// (a `GroupValueRead`), or its address hasn't been registered. This is
    /// the generic counterpart to [`GroupAddress::decode`](TypedGroupAddress::decode)
    /// for callers (e.g. a bus monitor) that see telegrams for addresses
    /// they don't hold a specific handle for.
    #[cfg(feature = "dpt")]
    #[must_use]
    pub fn decode_group_value<'a>(&self, telegram: &'a Telegram) -> Option<Result<DptView<'a>>> {
        let Address::Group(address) = telegram.destination else {
            return None;
        };
        if telegram.telegram_type == TelegramType::GroupValueRead {
            return None;
        }
        let dpt = self.group_registry.get(address)?;
        Some(dpt.decode_ref(&telegram.payload))
    }

    /// Start the main event loop for processing incoming telegrams
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError::ValidationError`] if [`Self::connect`]
    /// hasn't been called yet.
    pub async fn start(&self) -> Result<()> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.receiving_stop.store(false, Ordering::SeqCst);
        let timer = Timer::start(Component::Application, "knx_start");
        log_application!(
            LogLevel::Info,
            "Starting Knx telegram processing with queue"
        );

        if self.connection.borrow().is_none() {
            return Err(ConfigurationError::ValidationError {
                details: "Not connected to KNX network".to_string(),
            }
            .into());
        }

        // Start the connection control task (handles disconnect/reconnect events)
        self.start_connection_control_task().await?;

        // Start the telegram receiving task
        self.start_telegram_receiving_task().await?;

        // Start the telegram processing task
        self.start_telegram_processing_task().await?;

        // Start the outgoing telegram sending task
        *self.sending_task.write().await = Some(self.start_telegram_sending_task());

        // Start the group-address refresh task
        #[cfg(feature = "dpt")]
        self.start_group_refresh_task().await?;

        log_application!(
            LogLevel::Info,
            "Knx telegram processing started successfully with queue"
        );
        timer.finish_with_message("Knx telegram processing started");

        Ok(())
    }

    async fn start_telegram_receiving_task(&self) -> Result<()> {
        let mut active = self.connection.subscribe();
        let shutdown_flag = self.receiving_stop.clone();
        let telegram_queue = self.telegram_queue.clone();
        let control_tx = self.control_tx.clone();
        let receive_limiter = self.receive_limiter.clone();
        let pending_cemi_confirmation = self.pending_cemi_confirmation.clone();
        #[cfg(feature = "secure")]
        let data_secure_keyring = self.data_secure_keyring.clone();
        let memory_monitor = self.memory_monitor.clone();
        let performance_optimizer = self.performance_optimizer.clone();
        let handle = tokio::spawn(async move {
            let mut heartbeat: Option<(
                Arc<dyn Connection>,
                crate::transport::tunnel::HeartbeatHandle,
            )> = None;
            while !shutdown_flag.load(Ordering::SeqCst) {
                let connection = active.borrow_and_update().clone();
                let Some(connection) = connection else {
                    heartbeat = None;
                    tokio::select! {
                        _ = active.changed() => {}
                        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                    }
                    continue;
                };
                if heartbeat
                    .as_ref()
                    .is_none_or(|(current, _)| !Arc::ptr_eq(current, &connection))
                {
                    heartbeat = connection
                        .clone()
                        .start_heartbeat("knust".to_string())
                        .map(|handle| (connection.clone(), handle));
                }
                let heartbeat_lost = heartbeat.as_ref().map(|(_, handle)| handle.lost());
                let channel_id = connection
                    .as_any()
                    .downcast_ref::<Tunnel>()
                    .map(Tunnel::channel_id);

                tokio::select! {
                    _ = async {
                        match heartbeat_lost.as_ref() {
                            Some(lost) => lost.notified().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        let _ = control_tx.send(ConnectionControlMessage {
                            event: ConnectionControlEvent::TunnelLost {
                                channel_id: channel_id.unwrap_or(0),
                                reason: "ConnectionState heartbeat failed".to_string(),
                            },
                            connection: connection.clone(),
                        }).await;
                    }
                    _ = active.changed() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                    received = connection.recv() => {
                        let is_current = active.borrow().as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &connection));
                        if !is_current { continue; }
                        match received {
                            Ok(frame_data) => {
                                if let Some(tunnel) = connection.as_any().downcast_ref::<Tunnel>()
                                    && let Ok(frame) = crate::protocol::knxip::KnxIpFrame::parse(&frame_data)
                                    && tunnel.router().dispatch(frame).is_none()
                                { continue; }
                                let frame_size = frame_data.len() as u64;
                                if let Err(error) = memory_monitor.allocate(Component::Protocol, frame_size).await {
                                    log_application!(LogLevel::Warn, "Receive allocation failed: {}", error);
                                    continue;
                                }
                                let timer = std::time::Instant::now();
                                if let Err(error) = Self::handle_incoming_frame(
                                    &frame_data, &telegram_queue, &control_tx, channel_id,
                                    &connection, receive_limiter.as_deref(),
                                    Some(&pending_cemi_confirmation),
                                    #[cfg(feature = "secure")] &data_secure_keyring,
                                ).await {
                                    log_application!(LogLevel::Warn, "Failed to handle incoming frame: {}", error);
                                }
                                memory_monitor.deallocate(Component::Protocol, frame_size).await;
                                performance_optimizer.record_hot_path("frame_handle", timer.elapsed()).await;
                                performance_optimizer.record_hot_path("telegram_recv", timer.elapsed()).await;
                            }
                            Err(error) => {
                                let _ = control_tx.send(ConnectionControlMessage {
                                    event: ConnectionControlEvent::TunnelLost {
                                        channel_id: channel_id.unwrap_or(0), reason: error.to_string(),
                                    },
                                    connection: connection.clone(),
                                }).await;
                                // Avoid spinning on EOF while control handles this generation.
                                tokio::select! {
                                    _ = active.changed() => {}
                                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                                }
                            }
                        }
                    }
                }
            }
        });
        *self.receiving_task.write().await = Some(handle);
        Ok(())
    }

    /// If `telegram` is a real `GroupValueWrite`/`GroupValueResponse` for a
    /// group address, record it as this bus's last known value for that
    /// address (see [`Self::group_address_state`]). No-op for anything else
    /// — a `GroupValueRead` carries no value, and an unregistered address
    /// has nothing to record it in.
    #[cfg(feature = "dpt")]
    fn record_group_value_if_present(group_registry: &GroupAddressRegistry, telegram: &Telegram) {
        if let Address::Group(addr) = telegram.destination
            && matches!(
                telegram.telegram_type,
                TelegramType::GroupValueWrite | TelegramType::GroupValueResponse
            )
        {
            group_registry.record_seen(addr, &telegram.payload);
        }
    }

    /// Start the telegram processing task that processes from the queue
    async fn start_telegram_processing_task(&self) -> Result<()> {
        let shutdown_flag = self.shutdown_flag.clone();
        let performance_optimizer = self.performance_optimizer.clone();
        let event_handler = self.event_handler.clone();
        let telegram_queue = self.telegram_queue.clone();
        #[cfg(feature = "dpt")]
        let group_registry = self.group_registry.clone();

        log_application!(LogLevel::Debug, "Starting telegram processing task");

        let handle = tokio::spawn(async move {
            log_application!(LogLevel::Info, "Telegram processing task started");
            let mut process_count = 0u64;

            loop {
                // Check shutdown flag
                if shutdown_flag.load(Ordering::SeqCst) && telegram_queue.is_empty().await {
                    log_application!(
                        LogLevel::Info,
                        "Shutdown flag set and queue empty, stopping telegram processing (processed {} telegrams)",
                        process_count
                    );
                    break;
                }

                // Dequeue incoming telegram for processing
                if let Some(telegram) = telegram_queue.dequeue_incoming().await {
                    process_count += 1;
                    let process_timer = std::time::Instant::now();

                    log_application!(
                        LogLevel::Debug,
                        "Processing telegram #{}: {} -> {}",
                        process_count,
                        telegram.source,
                        match &telegram.destination {
                            crate::protocol::Address::Group(addr) => addr.to_string(),
                            crate::protocol::Address::Individual(addr) => addr.to_string(),
                        }
                    );

                    #[cfg(feature = "dpt")]
                    Self::record_group_value_if_present(&group_registry, &telegram);

                    // Notify telegram callbacks
                    event_handler.notify_telegram_received(&telegram).await;

                    let process_duration = process_timer.elapsed();
                    performance_optimizer
                        .record_hot_path("telegram_process", process_duration)
                        .await;
                    log_application!(
                        LogLevel::Trace,
                        "Successfully processed telegram #{}",
                        process_count
                    );
                } else {
                    // Queue is closed and empty, exit
                    log_application!(
                        LogLevel::Debug,
                        "Telegram queue closed, stopping processing task"
                    );
                    break;
                }
            }

            log_application!(
                LogLevel::Info,
                "Telegram processing task stopped (total processed: {})",
                process_count
            );
        });

        // Store the task handle
        {
            let mut task = self.processing_task.write().await;
            *task = Some(handle);
        }

        Ok(())
    }

    /// Start the telegram sending task that sends outgoing telegrams
    fn start_telegram_sending_task(&self) -> JoinHandle<()> {
        let mut active = self.connection.subscribe();
        let control_tx = self.control_tx.clone();
        let auto_reconnect = self.config.auto_reconnect;
        let shutdown_flag = self.shutdown_flag.clone();
        let telegram_queue = self.telegram_queue.clone();
        let pending_cemi_confirmation = self.pending_cemi_confirmation.clone();
        #[cfg(feature = "secure")]
        let data_secure_keyring = self.data_secure_keyring.clone();
        #[cfg(feature = "secure")]
        let data_secure_sequence = self.data_secure_sequence.clone();
        let performance_optimizer = self.performance_optimizer.clone();

        log_application!(LogLevel::Debug, "Starting telegram sending task");

        tokio::spawn(async move {
            log_application!(LogLevel::Info, "Telegram sending task started");
            let mut send_count = 0u64;

            'sending: loop {
                // Check shutdown flag
                if shutdown_flag.load(Ordering::SeqCst) && telegram_queue.is_empty().await {
                    log_application!(
                        LogLevel::Info,
                        "Shutdown flag set and queue empty, stopping telegram sending (sent {} telegrams)",
                        send_count
                    );
                    break;
                }

                // Dequeue outgoing telegram for sending
                if let Some((telegram, mut receipt)) = telegram_queue.dequeue_outgoing_job().await {
                    if receipt.as_ref().is_some_and(|receipt| receipt.is_closed()) {
                        continue;
                    }
                    // Acquire rate limiter token before sending
                    telegram_queue.acquire_send_token().await;

                    send_count += 1;
                    let send_timer = std::time::Instant::now();

                    log_application!(
                        LogLevel::Debug,
                        "Sending telegram #{}: {} -> {} (priority: {:?})",
                        send_count,
                        telegram.source,
                        match &telegram.destination {
                            crate::protocol::Address::Group(addr) => addr.to_string(),
                            crate::protocol::Address::Individual(addr) => addr.to_string(),
                        },
                        telegram.priority
                    );

                    let connection = loop {
                        let connection = active.borrow_and_update().clone();
                        if let Some(connection) = connection {
                            break connection;
                        }
                        if shutdown_flag.load(Ordering::SeqCst) {
                            return;
                        }
                        tokio::select! {
                            _ = active.changed() => {}
                            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                        }
                    };
                    if receipt.as_ref().is_some_and(|receipt| receipt.is_closed()) {
                        continue;
                    }
                    #[cfg(feature = "secure")]
                    let frame_result = Self::build_outgoing_frame_with_security(
                        &telegram,
                        &connection,
                        &data_secure_keyring,
                        &data_secure_sequence,
                    )
                    .await;
                    #[cfg(not(feature = "secure"))]
                    let frame_result =
                        Self::build_outgoing_frame_for_connection(&telegram, &connection);
                    let mut result = match frame_result {
                        Ok(frame_data) => {
                            Self::send_telegram_frame_with_confirmation(
                                &connection,
                                &frame_data,
                                2,
                                &mut receipt,
                                &pending_cemi_confirmation,
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    };
                    if let Err(error) = &result
                        && !Self::is_cemi_confirmation_timeout(error)
                    {
                        if receipt.as_ref().is_some_and(|receipt| receipt.is_closed()) {
                            continue;
                        }
                        Self::report_connection_failure(
                            &control_tx,
                            connection.clone(),
                            error.to_string(),
                        )
                        .await;
                        if auto_reconnect && !shutdown_flag.load(Ordering::SeqCst) {
                            let replacement = loop {
                                let current = active.borrow_and_update().clone();
                                if let Some(current) = current
                                    && !Arc::ptr_eq(&current, &connection)
                                {
                                    break current;
                                }
                                if shutdown_flag.load(Ordering::SeqCst) {
                                    return;
                                }
                                if receipt.as_ref().is_some_and(|receipt| receipt.is_closed()) {
                                    continue 'sending;
                                }
                                tokio::select! {
                                    _ = active.changed() => {}
                                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                                }
                            };
                            #[cfg(feature = "secure")]
                            let replacement_frame_result =
                                Self::build_outgoing_frame_with_security(
                                    &telegram,
                                    &replacement,
                                    &data_secure_keyring,
                                    &data_secure_sequence,
                                )
                                .await;
                            #[cfg(not(feature = "secure"))]
                            let replacement_frame_result =
                                Self::build_outgoing_frame_for_connection(&telegram, &replacement);
                            result = match replacement_frame_result {
                                Ok(frame) => {
                                    Self::send_telegram_frame_with_confirmation(
                                        &replacement,
                                        &frame,
                                        1,
                                        &mut receipt,
                                        &pending_cemi_confirmation,
                                    )
                                    .await
                                }
                                Err(error) => Err(error),
                            };
                            if let Err(error) = &result
                                && !Self::is_cemi_confirmation_timeout(error)
                            {
                                Self::report_connection_failure(
                                    &control_tx,
                                    replacement,
                                    error.to_string(),
                                )
                                .await;
                            }
                        }
                    }
                    match &result {
                        Ok(()) => {
                            let send_duration = send_timer.elapsed();
                            performance_optimizer
                                .record_hot_path("telegram_send", send_duration)
                                .await;
                            log_application!(
                                LogLevel::Trace,
                                "Successfully sent telegram #{}",
                                send_count
                            );
                        }
                        Err(e) => {
                            log_application!(
                                LogLevel::Error,
                                "Failed to send telegram #{}: {}",
                                send_count,
                                e
                            );
                        }
                    }
                    if let Some(receipt) = receipt {
                        let _ = receipt.completed.send(result);
                    }
                } else {
                    // Queue is closed and empty, exit
                    log_application!(
                        LogLevel::Debug,
                        "Outgoing telegram queue closed, stopping sending task"
                    );
                    break;
                }
            }

            log_application!(
                LogLevel::Info,
                "Telegram sending task stopped (total sent: {})",
                send_count
            );
        })
    }

    async fn send_telegram_frame(
        connection: &Arc<dyn Connection>,
        frame: &[u8],
        attempts: u8,
        receipt: &mut Option<crate::transport::queue::SendReceipt>,
    ) -> Result<()> {
        let mut untracked = None;
        let signal = receipt
            .as_mut()
            .map(|receipt| &mut receipt.transmitted)
            .unwrap_or(&mut untracked);
        if let Some(tunnel) = connection.as_any().downcast_ref::<Tunnel>() {
            tunnel.send_acknowledged(frame, attempts, signal).await
        } else {
            if let Some(tx) = signal.as_mut() {
                tokio::select! {
                    result = connection.send(frame) => result?,
                    _ = tx.closed() => return Err(crate::error::TransportError::QueueClosed.into()),
                }
            } else {
                connection.send(frame).await?;
            }
            if let Some(tx) = signal.take() {
                let _ = tx.send(());
            }
            Ok(())
        }
    }

    async fn send_telegram_frame_with_confirmation(
        connection: &Arc<dyn Connection>,
        frame: &[u8],
        attempts: u8,
        receipt: &mut Option<crate::transport::queue::SendReceipt>,
        pending_confirmation: &std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    ) -> Result<()> {
        let receiver = if connection.as_any().is::<Tunnel>() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            *pending_confirmation.lock().unwrap() = Some(tx);
            Some(rx)
        } else {
            None
        };
        if let Err(error) = Self::send_telegram_frame(connection, frame, attempts, receipt).await {
            if receiver.is_some() {
                pending_confirmation.lock().unwrap().take();
            }
            return Err(error);
        }
        if let Some(receiver) = receiver
            && tokio::time::timeout(CEMI_CONFIRMATION_TIMEOUT, receiver)
                .await
                .is_err()
        {
            pending_confirmation.lock().unwrap().take();
            return Err(DeviceError::CommunicationTimeout {
                device: "L_Data.con".to_string(),
                timeout_ms: CEMI_CONFIRMATION_TIMEOUT.as_millis() as u64,
            }
            .into());
        }
        Ok(())
    }

    async fn report_connection_failure(
        control: &mpsc::Sender<ConnectionControlMessage>,
        connection: Arc<dyn Connection>,
        reason: String,
    ) {
        let channel_id = connection
            .as_any()
            .downcast_ref::<Tunnel>()
            .map_or(0, Tunnel::channel_id);
        let _ = control
            .send(ConnectionControlMessage {
                event: ConnectionControlEvent::TunnelLost { channel_id, reason },
                connection,
            })
            .await;
    }

    fn is_cemi_confirmation_timeout(error: &KnxError) -> bool {
        matches!(
            error,
            KnxError::Device(DeviceError::CommunicationTimeout { device, .. })
                if device == "L_Data.con"
        )
    }

    /// Stop processing, allowing queued work to finish within bounded deadlines.
    ///
    /// Tasks still blocked at their deadline are cancelled and joined before returning.
    pub async fn stop(&self) {
        use std::time::Duration;
        let _lifecycle = self.lifecycle_lock.lock().await;
        log_application!(LogLevel::Info, "Stopping Knx telegram processing");
        self.shutdown_flag.store(true, Ordering::SeqCst);
        self.shutdown_started.notify_waiters();

        // Control must stop first so it cannot create another reconnect task.
        Self::finish_task(&self.control_task, Duration::ZERO).await;
        Self::finish_task(&self.reconnect_task, Duration::ZERO).await;
        self.telegram_queue.close().await;

        Self::finish_task(&self.sending_task, Duration::from_secs(5)).await;
        self.receiving_stop.store(true, Ordering::SeqCst);
        Self::finish_task(&self.receiving_task, Duration::from_secs(5)).await;
        Self::finish_task(&self.processing_task, Duration::from_secs(5)).await;
        Self::finish_task(&self.receive_drain_task, Duration::ZERO).await;
        Self::finish_task(&self.cleanup_task, Duration::from_secs(2)).await;
        #[cfg(feature = "dpt")]
        Self::finish_task(&self.refresh_task, Duration::from_secs(2)).await;

        // Release queued read receipts after the sender has finished or been cancelled.
        self.telegram_queue.clear().await;
        self.shutdown_flag.store(false, Ordering::SeqCst);
        self.shutdown_notify.notify_waiters();
        log_application!(LogLevel::Info, "Knx telegram processing stopped");
    }

    async fn finish_task(task: &TaskSlot, grace: std::time::Duration) {
        let handle = task.write().await.take();
        if let Some(mut handle) = handle
            && tokio::time::timeout(grace, &mut handle).await.is_err()
        {
            handle.abort();
            let _ = handle.await;
        }
    }

    /// Shutdown the library completely (disconnect and cleanup)
    ///
    /// This method provides a clean shutdown sequence:
    /// 1. Stop telegram processing
    /// 2. Disconnect from the network
    /// 3. Clean up resources
    ///
    /// # Errors
    ///
    /// Returns the underlying connection's close error, if any (see
    /// [`Self::disconnect`]).
    pub async fn shutdown(&self) -> Result<()> {
        let timer = Timer::start(Component::Application, "knx_shutdown");
        log_application!(LogLevel::Info, "Shutting down Knx library");

        // Stop processing first
        self.stop().await;

        // Then disconnect
        self.disconnect().await?;

        log_application!(LogLevel::Info, "Knx shutdown complete");
        timer.finish_with_message("Knx shutdown complete");
        Ok(())
    }

    /// Run the library (connect, start, and block until shutdown)
    ///
    /// This is a convenience method that combines `connect()` and `start()`
    /// and blocks until the library is shut down.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::connect`] and [`Self::start`].
    pub async fn run(&self) -> Result<()> {
        self.connect().await?;
        self.start().await?;
        self.shutdown_notify.notified().await;
        Ok(())
    }

    /// Run with a shutdown signal
    ///
    /// This method runs the library until the provided future completes,
    /// then performs a clean shutdown.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::connect`], [`Self::start`], and
    /// [`Self::shutdown`].
    pub async fn run_until<F>(&self, shutdown_signal: F) -> Result<()>
    where
        F: std::future::Future<Output = ()>,
    {
        self.connect().await?;
        self.start().await?;

        // Wait for shutdown signal
        shutdown_signal.await;

        // Perform clean shutdown
        self.shutdown().await?;

        Ok(())
    }

    /// Get the current library state
    pub async fn state(&self) -> KnxState {
        *self.state.read().await
    }

    /// Check if connected to the KNX network
    pub async fn is_connected(&self) -> bool {
        matches!(self.state().await, KnxState::Connected)
    }

    /// Get connection statistics
    pub async fn connection_stats(&self) -> Option<crate::transport::connection::ConnectionStats> {
        let connection = self.connection.borrow().clone();
        connection.as_ref().map(|c| c.stats())
    }

    /// Capture health for this instance's current connection.
    pub async fn connection_health(&self) -> Option<crate::transport::ConnectionHealth> {
        let connection = self.connection.borrow().clone();
        connection.as_ref().map(|connection| connection.health())
    }

    /// Get memory usage statistics
    pub async fn memory_stats(&self) -> crate::memory::MemoryStats {
        self.memory_monitor.get_stats().await
    }

    /// Get performance statistics
    pub async fn performance_stats(&self) -> crate::memory::HotPathStats {
        self.performance_optimizer.get_hot_path_stats().await
    }

    /// Get telegram queue statistics
    pub async fn telegram_queue_stats(&self) -> crate::transport::queue::QueueStats {
        self.telegram_queue.stats().await
    }

    /// Get receive rate limiter statistics
    #[must_use]
    pub fn receive_stats(&self) -> Option<ReceiveStats> {
        self.receive_limiter.as_ref().map(|l| l.stats())
    }

    /// Get telegram queue size
    pub async fn telegram_queue_size(&self) -> usize {
        self.telegram_queue.len().await
    }

    /// Check if telegram queue is empty
    pub async fn telegram_queue_is_empty(&self) -> bool {
        self.telegram_queue.is_empty().await
    }

    /// Register a telegram callback
    ///
    /// # Arguments
    /// * `callback` - The callback function to register
    ///
    /// # Returns
    /// A unique handle that can be used to unregister the callback
    pub async fn register_telegram_callback<F>(&self, callback: F) -> CallbackHandle
    where
        F: TelegramCallbackFn + Send + Sync + 'static,
    {
        self.event_handler
            .register_telegram_callback(callback)
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
        self.event_handler
            .register_telegram_callback_filtered(callback, filter, include_outgoing)
            .await
    }

    /// Register a connection state callback
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
        self.event_handler
            .register_connection_callback(callback)
            .await
    }

    /// Unregister any callback by its handle
    ///
    /// # Arguments
    /// * `handle` - The handle returned when the callback was registered
    ///
    /// # Returns
    /// `true` if the callback was found and removed, `false` otherwise
    pub async fn unregister_callback(&self, handle: CallbackHandle) -> bool {
        self.event_handler.unregister_callback(handle).await
    }

    /// Get the total number of registered callbacks (for testing)
    #[cfg(test)]
    pub async fn total_callback_count(&self) -> usize {
        self.event_handler.total_callback_count().await
    }

    /// Get the number of registered telegram callbacks (for testing)
    #[cfg(test)]
    pub async fn telegram_callback_count(&self) -> usize {
        self.event_handler.telegram_callback_count().await
    }

    /// Get the number of registered connection callbacks (for testing)
    #[cfg(test)]
    pub async fn connection_callback_count(&self) -> usize {
        self.event_handler.connection_callback_count().await
    }

    /// Clear all registered callbacks (for testing)
    #[cfg(test)]
    pub async fn clear_all_callbacks(&self) {
        self.event_handler.clear_all_callbacks().await;
    }

    /// Notify connection state changed (internal use)
    pub(crate) async fn notify_connection_state_changed(
        &self,
        state: crate::application::callbacks::ConnectionState,
    ) {
        self.event_handler
            .notify_connection_state_changed(state)
            .await;
    }

    /// Notify telegram received (for testing)
    #[cfg(test)]
    pub async fn test_notify_telegram_received(&self, telegram: &Telegram) {
        self.event_handler.notify_telegram_received(telegram).await;
    }

    /// Notify connection state changed (for testing)
    #[cfg(test)]
    pub async fn test_notify_connection_state_changed(
        &self,
        state: crate::application::callbacks::ConnectionState,
    ) {
        self.notify_connection_state_changed(state).await;
    }

    /// Force memory cleanup
    pub async fn force_cleanup(&self) -> u64 {
        self.memory_monitor.cleanup().await
    }

    /// Check if memory usage is within bounds
    #[must_use]
    pub fn memory_within_bounds(&self) -> bool {
        self.memory_monitor.is_within_bounds()
    }

    /// Get memory usage percentage
    #[must_use]
    pub fn memory_usage_percentage(&self) -> f64 {
        self.memory_monitor.usage_percentage()
    }

    /// Get the current configuration
    #[must_use]
    pub fn config(&self) -> &ConnectionConfig {
        &self.config
    }

    /// Check if the library is shutting down
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown_flag.load(Ordering::SeqCst)
    }

    /// Validate the configuration
    fn validate_config(config: &ConnectionConfig) -> Result<()> {
        match config.connection_type {
            ConnectionType::Tunneling
            | ConnectionType::TcpTunneling
            | ConnectionType::SecureTunneling => {
                if config.gateway_ip.is_none() {
                    return Err(ConfigurationError::MissingParameter {
                        parameter: "gateway_ip".to_string(),
                    }
                    .into());
                }
            }
            ConnectionType::Routing | ConnectionType::SecureRouting => {
                // Routing doesn't require gateway IP
            }
        }

        if config.timeout_ms == 0 {
            return Err(ConfigurationError::InvalidValue {
                parameter: "timeout_ms".to_string(),
                value: config.timeout_ms.to_string(),
                reason: "Timeout must be greater than 0".to_string(),
            }
            .into());
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    /// Handle incoming KNX/IP frame based on service type (following Python pattern)
    async fn handle_incoming_frame(
        frame_data: &[u8],
        telegram_queue: &crate::transport::queue::TelegramQueue,
        control_tx: &mpsc::Sender<ConnectionControlMessage>,
        current_channel_id: Option<u8>,
        connection: &Arc<dyn Connection>,
        receive_limiter: Option<&ReceiveRateLimiter>,
        pending_cemi_confirmation: Option<
            &std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        >,
        #[cfg(feature = "secure")] data_secure_keyring: &RwLock<Option<KeyRing>>,
    ) -> Result<bool> {
        use crate::protocol::knxip::{KnxIpFrame, ServiceType};

        // Parse KNX/IP frame first
        let knx_frame = KnxIpFrame::parse(frame_data)?;

        log_application!(
            LogLevel::Trace,
            "Handling frame with service type: {:?}",
            knx_frame.header.service_type
        );

        // Dispatch based on service type (following Python pattern)
        match knx_frame.header.service_type {
            // Data frames containing CEMI telegrams
            ServiceType::TunnellingRequest => {
                Self::handle_tunnelling_request(
                    &knx_frame,
                    telegram_queue,
                    connection,
                    receive_limiter,
                    pending_cemi_confirmation,
                    #[cfg(feature = "secure")]
                    data_secure_keyring,
                )
                .await?;
                Ok(true)
            }
            ServiceType::RoutingIndication => {
                Self::handle_routing_indication(
                    &knx_frame,
                    telegram_queue,
                    receive_limiter,
                    #[cfg(feature = "secure")]
                    data_secure_keyring,
                )
                .await?;
                Ok(true)
            }

            // Control frames (no CEMI data)
            ServiceType::DisconnectRequest => {
                Self::handle_disconnect_request(
                    &knx_frame,
                    control_tx,
                    current_channel_id,
                    connection,
                )
                .await?;
                Ok(true)
            }
            ServiceType::DisconnectResponse => {
                Self::handle_disconnect_response(&knx_frame)?;
                Ok(true)
            }
            ServiceType::ConnectionstateRequest => {
                Self::handle_connectionstate_request(&knx_frame)?;
                Ok(true)
            }
            ServiceType::ConnectionstateResponse => {
                Self::handle_connectionstate_response(&knx_frame)?;
                Ok(true)
            }
            ServiceType::ConnectRequest => {
                Self::handle_connect_request(&knx_frame);
                Ok(true)
            }
            ServiceType::ConnectResponse => {
                Self::handle_connect_response(&knx_frame);
                Ok(true)
            }

            // Unsupported service types
            _ => {
                log_application!(
                    LogLevel::Debug,
                    "Service not implemented: {:?}",
                    knx_frame.header.service_type
                );
                Ok(false)
            }
        }
    }

    /// Handle `TunnellingRequest` frame (contains CEMI data)
    async fn handle_tunnelling_request(
        knx_frame: &crate::protocol::knxip::KnxIpFrame,
        telegram_queue: &crate::transport::queue::TelegramQueue,
        connection: &Arc<dyn Connection>,
        receive_limiter: Option<&ReceiveRateLimiter>,
        pending_cemi_confirmation: Option<
            &std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        >,
        #[cfg(feature = "secure")] data_secure_keyring: &RwLock<Option<KeyRing>>,
    ) -> Result<()> {
        use crate::protocol::knxip::{TunnellingAck, TunnellingRequest};
        use crate::transport::{SequenceValidationResult, Tunnel};

        // Parse TunnellingRequest
        let tunnelling_request = TunnellingRequest::parse(&knx_frame.body)?;

        log_application!(
            LogLevel::Debug,
            "Received TunnellingRequest: channel={}, sequence={}, cemi_len={}",
            tunnelling_request.communication_channel_id,
            tunnelling_request.sequence_counter,
            tunnelling_request.raw_cemi.len()
        );

        // Foreign-channel traffic must not ACK, consume sequence numbers or deliver.
        if let Some(tunnel) = connection.as_any().downcast_ref::<Tunnel>()
            && (tunnel.channel_id() == 0
                || tunnelling_request.communication_channel_id != tunnel.channel_id())
        {
            return Ok(());
        }

        // Validate sequence number if this is a tunneling connection
        let sequence_result =
            if let Some(tunneling_conn) = connection.as_any().downcast_ref::<Tunnel>() {
                tunneling_conn.validate_sequence_number(tunnelling_request.sequence_counter)
            } else {
                // For non-tunneling connections (shouldn't happen), just process
                SequenceValidationResult::Valid
            };

        // Send appropriate acknowledgment based on sequence validation
        match sequence_result {
            SequenceValidationResult::Valid => {
                // Send positive ACK
                if let Some(tunneling_conn) = connection.as_any().downcast_ref::<Tunnel>()
                    && let Err(e) = tunneling_conn
                        .send_tunnelling_ack(
                            tunnelling_request.communication_channel_id,
                            tunnelling_request.sequence_counter,
                            TunnellingAck::STATUS_OK,
                        )
                        .await
                {
                    log_application!(LogLevel::Warn, "Failed to send TunnellingAck: {}", e);
                }

                if crate::protocol::cemi::CemiFrame::parse(&tunnelling_request.raw_cemi).is_ok_and(
                    |frame| frame.message_code == crate::protocol::cemi::MessageCode::LDataCon,
                ) {
                    if let Some(pending) = pending_cemi_confirmation
                        && let Some(sender) = pending.lock().unwrap().take()
                    {
                        let _ = sender.send(());
                    }
                    return Ok(());
                }

                // Parse CEMI and enqueue telegram for processing
                #[cfg(feature = "secure")]
                let telegram = Self::parse_cemi_to_telegram_with_security(
                    &tunnelling_request.raw_cemi,
                    data_secure_keyring,
                )
                .await?;
                #[cfg(not(feature = "secure"))]
                let telegram = Self::parse_cemi_to_telegram(&tunnelling_request.raw_cemi)?;

                log_application!(
                    LogLevel::Debug,
                    "Parsed valid tunnelling telegram: {} -> {}",
                    telegram.source,
                    match &telegram.destination {
                        crate::protocol::Address::Group(addr) => addr.to_string(),
                        crate::protocol::Address::Individual(addr) => addr.to_string(),
                    }
                );

                // Enqueue incoming telegram for processing
                if let Some(limiter) = receive_limiter {
                    use crate::transport::ReceiveResult;
                    match limiter.try_send(telegram) {
                        ReceiveResult::Sent => {}
                        ReceiveResult::DroppedQueueFull | ReceiveResult::DroppedThrottled(_) => {
                            log_application!(
                                LogLevel::Debug,
                                "Incoming telegram dropped by receive limiter"
                            );
                        }
                    }
                } else if let Err(e) = telegram_queue.enqueue_incoming(telegram).await {
                    log_application!(
                        LogLevel::Warn,
                        "Failed to enqueue tunnelling telegram: {}",
                        e
                    );
                }
            }
            SequenceValidationResult::Duplicate => {
                // Send positive ACK but don't process (duplicate frame)
                if let Some(tunneling_conn) = connection.as_any().downcast_ref::<Tunnel>()
                    && let Err(e) = tunneling_conn
                        .send_tunnelling_ack(
                            tunnelling_request.communication_channel_id,
                            tunnelling_request.sequence_counter,
                            TunnellingAck::STATUS_OK,
                        )
                        .await
                {
                    log_application!(
                        LogLevel::Warn,
                        "Failed to send TunnellingAck for duplicate: {}",
                        e
                    );
                }

                log_application!(
                    LogLevel::Debug,
                    "Received duplicate TunnellingRequest (sequence one less than expected). Acknowledging but discarding."
                );
            }
            SequenceValidationResult::Invalid { expected, received } => {
                // Send sequence error ACK and don't process
                if let Some(tunneling_conn) = connection.as_any().downcast_ref::<Tunnel>()
                    && let Err(e) = tunneling_conn
                        .send_tunnelling_ack(
                            tunnelling_request.communication_channel_id,
                            tunnelling_request.sequence_counter,
                            TunnellingAck::STATUS_ERROR_SEQUENCE_NUMBER,
                        )
                        .await
                {
                    log_application!(
                        LogLevel::Warn,
                        "Failed to send sequence error TunnellingAck: {}",
                        e
                    );
                }

                log_application!(
                    LogLevel::Warn,
                    "Received TunnellingRequest with invalid sequence number: expected {}, received {}. Discarding frame.",
                    expected,
                    received
                );

                // According to KNX specification, we should drop the frame
                // The gateway should repeat the frame or disconnect if ACK is not received
            }
        }

        Ok(())
    }

    /// Handle `RoutingIndication` frame (contains CEMI data)
    async fn handle_routing_indication(
        knx_frame: &crate::protocol::knxip::KnxIpFrame,
        telegram_queue: &crate::transport::queue::TelegramQueue,
        receive_limiter: Option<&ReceiveRateLimiter>,
        #[cfg(feature = "secure")] data_secure_keyring: &RwLock<Option<KeyRing>>,
    ) -> Result<()> {
        // RoutingIndication format: [cemi_data...]
        let cemi_data = &knx_frame.body;
        if crate::protocol::cemi::CemiFrame::parse(cemi_data)
            .is_ok_and(|frame| frame.message_code == crate::protocol::cemi::MessageCode::LDataCon)
        {
            return Ok(());
        }
        #[cfg(feature = "secure")]
        let telegram =
            Self::parse_cemi_to_telegram_with_security(cemi_data, data_secure_keyring).await?;
        #[cfg(not(feature = "secure"))]
        let telegram = Self::parse_cemi_to_telegram(cemi_data)?;

        log_application!(
            LogLevel::Debug,
            "Parsed routing telegram: {} -> {}",
            telegram.source,
            match &telegram.destination {
                crate::protocol::Address::Group(addr) => addr.to_string(),
                crate::protocol::Address::Individual(addr) => addr.to_string(),
            }
        );

        // Enqueue incoming telegram for processing (through limiter if available)
        if let Some(limiter) = receive_limiter {
            use crate::transport::ReceiveResult;
            match limiter.try_send(telegram) {
                ReceiveResult::Sent => {}
                ReceiveResult::DroppedQueueFull | ReceiveResult::DroppedThrottled(_) => {
                    log_application!(
                        LogLevel::Debug,
                        "Incoming telegram dropped by receive limiter"
                    );
                }
            }
        } else if let Err(e) = telegram_queue.enqueue_incoming(telegram).await {
            log_application!(LogLevel::Warn, "Failed to enqueue routing telegram: {}", e);
        }

        Ok(())
    }

    /// Handle `DisconnectRequest` frame (control frame)
    /// Reply with a channel error for foreign requests; disconnect only our channel.
    async fn handle_disconnect_request(
        knx_frame: &crate::protocol::knxip::KnxIpFrame,
        control_tx: &mpsc::Sender<ConnectionControlMessage>,
        current_channel_id: Option<u8>,
        connection: &Arc<dyn Connection>,
    ) -> Result<()> {
        use crate::protocol::knxip::{
            DisconnectRequest, DisconnectResponse, KnxIpFrame, ServiceType,
        };

        log_application!(
            LogLevel::Warn,
            "Received DisconnectRequest from tunnelling server"
        );

        // Parse DisconnectRequest body
        if knx_frame.body.len() < DisconnectRequest::LENGTH {
            log_application!(
                LogLevel::Error,
                "DisconnectRequest body too short: {} bytes",
                knx_frame.body.len()
            );
            return Ok(());
        }

        let disconnect_request = DisconnectRequest::parse(&knx_frame.body)?;

        log_application!(
            LogLevel::Debug,
            "DisconnectRequest: channel_id={}, control_endpoint={:?}",
            disconnect_request.communication_channel_id,
            disconnect_request.control_endpoint
        );

        let is_our_channel =
            current_channel_id == Some(disconnect_request.communication_channel_id);

        if is_our_channel {
            // Send event to send DisconnectResponse
            if let Err(e) = control_tx
                .send(ConnectionControlMessage {
                    event: ConnectionControlEvent::SendDisconnectResponse {
                        channel_id: disconnect_request.communication_channel_id,
                    },
                    connection: connection.clone(),
                })
                .await
            {
                log_application!(
                    LogLevel::Warn,
                    "Failed to send DisconnectResponse event: {}",
                    e
                );
            }

            // Send tunnel lost event to trigger reconnection
            if let Err(e) = control_tx
                .send(ConnectionControlMessage {
                    event: ConnectionControlEvent::TunnelLost {
                        channel_id: disconnect_request.communication_channel_id,
                        reason: "DisconnectRequest received from server".to_string(),
                    },
                    connection: connection.clone(),
                })
                .await
            {
                log_application!(LogLevel::Warn, "Failed to send TunnelLost event: {}", e);
            }
        } else {
            log_application!(
                LogLevel::Warn,
                "Received DisconnectRequest for different channel {} (our channel: {:?})",
                disconnect_request.communication_channel_id,
                current_channel_id
            );
            let response = DisconnectResponse::new(
                disconnect_request.communication_channel_id,
                DisconnectResponse::STATUS_ERROR_CONNECTION_ID,
            );
            connection
                .send(
                    &KnxIpFrame::new(ServiceType::DisconnectResponse, response.serialize())
                        .serialize(),
                )
                .await?;
        }

        Ok(())
    }

    /// Handle `DisconnectResponse` frame (control frame)
    /// Handle `DisconnectResponse` frame (control frame)
    fn handle_disconnect_response(knx_frame: &crate::protocol::knxip::KnxIpFrame) -> Result<()> {
        use crate::protocol::knxip::DisconnectResponse;

        log_application!(LogLevel::Debug, "Received DisconnectResponse from server");

        // Parse DisconnectResponse body
        if knx_frame.body.len() < DisconnectResponse::LENGTH {
            log_application!(
                LogLevel::Error,
                "DisconnectResponse body too short: {} bytes",
                knx_frame.body.len()
            );
            return Ok(());
        }

        let disconnect_response = DisconnectResponse::parse(&knx_frame.body)?;

        log_application!(
            LogLevel::Debug,
            "DisconnectResponse: channel_id={}, status=0x{:02X}",
            disconnect_response.communication_channel_id,
            disconnect_response.status
        );

        // State lifecycle managed by caller
        if disconnect_response.is_success() {
            log_application!(LogLevel::Info, "Disconnect acknowledged by server");
        } else {
            log_application!(
                LogLevel::Warn,
                "Disconnect failed with status: 0x{:02X}",
                disconnect_response.status
            );
        }

        Ok(())
    }

    /// Handle `ConnectionstateRequest` frame (control frame)
    fn handle_connectionstate_request(
        knx_frame: &crate::protocol::knxip::KnxIpFrame,
    ) -> Result<()> {
        log_application!(
            LogLevel::Debug,
            "Received ConnectionstateRequest from server"
        );

        let request = crate::protocol::knxip::ConnectionstateRequest::parse(&knx_frame.body)?;
        log_application!(
            LogLevel::Debug,
            "ConnectionstateRequest: channel_id={}, control_endpoint={:?}",
            request.communication_channel_id,
            request.control_endpoint
        );
        // Response handled by transport layer heartbeat monitor

        Ok(())
    }

    /// Handle `ConnectionstateResponse` frame (control frame)
    fn handle_connectionstate_response(
        knx_frame: &crate::protocol::knxip::KnxIpFrame,
    ) -> Result<()> {
        log_application!(
            LogLevel::Debug,
            "Received ConnectionstateResponse from server"
        );

        let response = crate::protocol::knxip::ConnectionstateResponse::parse(&knx_frame.body)?;
        log_application!(
            LogLevel::Debug,
            "ConnectionstateResponse: channel_id={}, status=0x{:02X}",
            response.communication_channel_id,
            response.status
        );
        // State tracking handled by heartbeat monitor

        Ok(())
    }

    /// Handle `ConnectRequest` frame (control frame)
    fn handle_connect_request(knx_frame: &crate::protocol::knxip::KnxIpFrame) {
        log_application!(LogLevel::Debug, "Received ConnectRequest from client");

        // As a client, we typically don't receive ConnectRequest (that's for servers)
        // Just log it for debugging purposes
        log_application!(
            LogLevel::Trace,
            "ConnectRequest body: {} bytes",
            knx_frame.body.len()
        );
    }

    /// Handle `ConnectResponse` frame (control frame)
    fn handle_connect_response(knx_frame: &crate::protocol::knxip::KnxIpFrame) {
        log_application!(LogLevel::Debug, "Received ConnectResponse from server");

        // ConnectResponse is typically handled during the connect() handshake
        // If we receive it here, it's likely a duplicate or unexpected response
        log_application!(
            LogLevel::Trace,
            "ConnectResponse body: {} bytes",
            knx_frame.body.len()
        );
    }

    #[cfg(feature = "secure")]
    async fn parse_cemi_to_telegram_with_security(
        cemi_data: &[u8],
        data_secure_keyring: &RwLock<Option<KeyRing>>,
    ) -> Result<Telegram> {
        let mut frame = crate::protocol::cemi::CemiFrame::parse(cemi_data)?;
        let is_data_secure = frame.tpci & 0x03 == 0x03 && frame.apci_data.first() == Some(&0xF1);
        if is_data_secure {
            if frame.apci_data.len() < 8 {
                return Err(SecurityError::CryptographicError {
                    operation: "group decrypt".to_string(),
                    reason: "Data Secure APDU is too short for its sequence number".to_string(),
                }
                .into());
            }
            let sequence_number = u64::from_be_bytes([
                0,
                0,
                frame.apci_data[2],
                frame.apci_data[3],
                frame.apci_data[4],
                frame.apci_data[5],
                frame.apci_data[6],
                frame.apci_data[7],
            ]);
            let mut keyring = data_secure_keyring.write().await;
            let keyring = keyring
                .as_mut()
                .ok_or_else(|| SecurityError::InvalidCredentials {
                    details: "Data Secure frame received without group keys".to_string(),
                })?;
            if keyring.get_sequence_number(&frame.source_addr).is_none() {
                return Err(SecurityError::InvalidCredentials {
                    details: format!("No Data Secure sender entry for {}", frame.source_addr),
                }
                .into());
            }
            frame = crate::security::decrypt_group_frame(keyring, &frame)?;
            keyring.validate_sequence(frame.source_addr, sequence_number)?;
        }
        Telegram::from_cemi_frame(frame)
    }

    #[cfg(any(test, not(feature = "secure")))]
    fn parse_cemi_to_telegram(cemi_data: &[u8]) -> Result<Telegram> {
        crate::protocol::telegram::Telegram::from_cemi_frame(
            crate::protocol::cemi::CemiFrame::parse(cemi_data)?,
        )
    }

    fn cemi_frame_for_telegram(
        telegram: &Telegram,
        connection: &Arc<dyn Connection>,
    ) -> crate::protocol::cemi::CemiFrame {
        use crate::protocol::cemi::{CemiFrame, MessageCode};

        let message_code = if connection.as_any().is::<Tunnel>() {
            MessageCode::LDataReq
        } else {
            MessageCode::LDataInd
        };
        let mut frame = CemiFrame::new(
            message_code,
            telegram.source,
            telegram.destination,
            telegram.encode_apdu(),
        );
        if matches!(
            telegram.telegram_type,
            TelegramType::IndividualData | TelegramType::IndividualControl
        ) {
            frame.tpci = telegram.tpci;
        }
        frame
    }

    fn serialize_outgoing_cemi(
        cemi: crate::protocol::cemi::CemiFrame,
        connection: &Arc<dyn Connection>,
    ) -> Result<Vec<u8>> {
        use crate::protocol::knxip::{KnxIpFrame, ServiceType, TunnellingRequest};

        let raw_cemi = cemi.serialize()?;
        if let Some(tunneling_conn) = connection.as_any().downcast_ref::<Tunnel>() {
            let tunnelling_request = TunnellingRequest::new(
                tunneling_conn.channel_id(),
                tunneling_conn.next_sequence(),
                raw_cemi,
            );
            return Ok(KnxIpFrame::new(
                ServiceType::TunnellingRequest,
                tunnelling_request.serialize(),
            )
            .serialize());
        }
        Ok(KnxIpFrame::new(ServiceType::RoutingIndication, raw_cemi).serialize())
    }

    #[cfg(feature = "secure")]
    async fn build_outgoing_frame_with_security(
        telegram: &Telegram,
        connection: &Arc<dyn Connection>,
        keyring: &RwLock<Option<KeyRing>>,
        sequence: &AtomicU64,
    ) -> Result<Vec<u8>> {
        let mut cemi = Self::cemi_frame_for_telegram(telegram, connection);
        if let Address::Group(destination) = telegram.destination {
            let keyring = keyring.read().await;
            if keyring
                .as_ref()
                .is_some_and(|keys| keys.get_group_key(&destination).is_some())
            {
                let sequence_number = next_data_secure_sequence(sequence)?;
                cemi = crate::security::encrypt_group_frame(
                    keyring.as_ref().expect("checked above"),
                    &cemi,
                    sequence_number,
                )?;
            }
        }
        Self::serialize_outgoing_cemi(cemi, connection)
    }

    #[cfg(any(test, not(feature = "secure")))]
    fn build_outgoing_frame_for_connection(
        telegram: &Telegram,
        connection: &Arc<dyn Connection>,
    ) -> Result<Vec<u8>> {
        Self::serialize_outgoing_cemi(
            Self::cemi_frame_for_telegram(telegram, connection),
            connection,
        )
    }
}

/// Knx library state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnxState {
    /// Library is disconnected
    Disconnected,

    /// Library is connecting
    Connecting,

    /// Library is connected and operational
    Connected,

    /// Library is disconnecting
    Disconnecting,

    /// Library is in error state
    Error,
}

impl P2pConnection {
    fn timeout_error(&self, timeout: std::time::Duration) -> KnxError {
        DeviceError::CommunicationTimeout {
            device: format!("individual address {}", self.destination),
            timeout_ms: timeout.as_millis() as u64,
        }
        .into()
    }

    async fn next_event_until(
        &mut self,
        deadline: tokio::time::Instant,
        timeout: std::time::Duration,
    ) -> Result<Telegram> {
        if let Some(telegram) = self.pending.pop_front() {
            return Ok(telegram);
        }
        match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
            Ok(Some(telegram)) => Ok(telegram),
            Ok(None) => Err(crate::error::TransportError::QueueClosed.into()),
            Err(_) => Err(self.timeout_error(timeout)),
        }
    }

    async fn next_channel_event_until(
        &mut self,
        deadline: tokio::time::Instant,
        timeout: std::time::Duration,
    ) -> Result<Telegram> {
        match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
            Ok(Some(telegram)) => Ok(telegram),
            Ok(None) => Err(crate::error::TransportError::QueueClosed.into()),
            Err(_) => Err(self.timeout_error(timeout)),
        }
    }

    fn control_telegram(&self, tpci: crate::protocol::TpciFrame) -> Telegram {
        let mut telegram = Telegram::individual_data(self.destination, Vec::new());
        telegram.telegram_type = TelegramType::IndividualControl;
        telegram.tpci = tpci.encode();
        telegram
    }

    async fn send_control(&self, tpci: crate::protocol::TpciFrame) -> Result<()> {
        self.knx
            .send_telegram_acknowledged(&self.control_telegram(tpci))
            .await
    }

    async fn wait_until_connected(&mut self) -> Result<()> {
        let deadline = tokio::time::Instant::now() + P2P_CONNECTION_TIMEOUT;
        loop {
            let telegram = self
                .next_channel_event_until(deadline, P2P_CONNECTION_TIMEOUT)
                .await?;
            if telegram.telegram_type == TelegramType::IndividualControl {
                match crate::protocol::TpciFrame::decode(telegram.tpci) {
                    crate::protocol::TpciFrame::Connect => return Ok(()),
                    crate::protocol::TpciFrame::Disconnect => {
                        return Err(crate::error::ProtocolError::InvalidFrame {
                            details: format!("{} rejected the P2P connection", self.destination),
                        }
                        .into());
                    }
                    _ => {}
                }
            }
            self.pending.push_back(telegram);
        }
    }

    /// Send one connected-data APDU, repeating it up to three times after the
    /// initial transmission if its matching T_ACK is lost or rejected. Success
    /// does not wait for a CEMI L_Data.con bus confirmation.
    pub async fn send_data(&mut self, apdu: Vec<u8>, wait_for_ack: bool) -> Result<()> {
        if self.closed {
            return Err(crate::error::TransportError::QueueClosed.into());
        }
        if !(2..=u8::MAX as usize).contains(&apdu.len()) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("P2P APDU length must be in 2..=255, got {}", apdu.len()),
            }
            .into());
        }
        let sequence = self.send_sequence;
        let mut telegram = Telegram::individual_data(self.destination, apdu);
        telegram.tpci = crate::protocol::TpciFrame::DataConnected { sequence }.encode();
        for attempt in 0..=P2P_MAX_RETRANSMISSIONS {
            self.knx.send_telegram(&telegram).await?;
            if !wait_for_ack {
                self.send_sequence = (sequence + 1) & 0x0f;
                return Ok(());
            }
            let deadline = tokio::time::Instant::now() + P2P_ACK_TIMEOUT;
            loop {
                let event = match self
                    .next_channel_event_until(deadline, P2P_ACK_TIMEOUT)
                    .await
                {
                    Ok(event) => event,
                    Err(crate::error::KnxError::Device(DeviceError::CommunicationTimeout {
                        ..
                    })) => {
                        break;
                    }
                    Err(error) => return Err(error),
                };
                if event.telegram_type == TelegramType::IndividualControl {
                    match crate::protocol::TpciFrame::decode(event.tpci) {
                        crate::protocol::TpciFrame::Ack { sequence: ack } if ack == sequence => {
                            self.send_sequence = (sequence + 1) & 0x0f;
                            return Ok(());
                        }
                        crate::protocol::TpciFrame::Nak { sequence: nak } if nak == sequence => {
                            break;
                        }
                        crate::protocol::TpciFrame::Disconnect => {
                            return Err(crate::error::ProtocolError::InvalidFrame {
                                details: format!(
                                    "{} disconnected the P2P session",
                                    self.destination
                                ),
                            }
                            .into());
                        }
                        _ => {}
                    }
                }
                self.pending.push_back(event);
            }
            if attempt == P2P_MAX_RETRANSMISSIONS {
                return Err(
                    self.timeout_error(P2P_ACK_TIMEOUT * (P2P_MAX_RETRANSMISSIONS as u32 + 1))
                );
            }
        }
        unreachable!("the retry loop either succeeds or returns")
    }

    /// Receive one APDU from the connected peer, optionally filtering by its
    /// two-byte response service. Connected data is acknowledged before return.
    pub async fn receive(
        &mut self,
        expected_service: Option<[u8; 2]>,
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>> {
        if self.closed {
            return Err(crate::error::TransportError::QueueClosed.into());
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let event = self.next_event_until(deadline, timeout).await?;
            if event.telegram_type == TelegramType::IndividualControl {
                if crate::protocol::TpciFrame::decode(event.tpci)
                    == crate::protocol::TpciFrame::Disconnect
                {
                    self.closed = true;
                    return Err(crate::error::ProtocolError::InvalidFrame {
                        details: format!("{} disconnected the P2P session", self.destination),
                    }
                    .into());
                }
                continue;
            }
            if event.telegram_type != TelegramType::IndividualData {
                continue;
            }
            if let crate::protocol::TpciFrame::DataConnected { sequence } =
                crate::protocol::TpciFrame::decode(event.tpci)
            {
                let new_data = if sequence == self.receive_sequence {
                    self.receive_sequence = (sequence + 1) & 0x0f;
                    true
                } else if sequence == (self.receive_sequence.wrapping_sub(1) & 0x0f) {
                    false
                } else {
                    self.send_control(crate::protocol::TpciFrame::Nak {
                        sequence: self.receive_sequence,
                    })
                    .await?;
                    continue;
                };
                self.send_control(crate::protocol::TpciFrame::Ack { sequence })
                    .await?;
                if !new_data {
                    continue;
                }
            }
            let matches_service = expected_service.is_none_or(|service| {
                // Memory and descriptor responses carry payload bits in the service octet.
                if service[1] & 0xC0 == 0x40 && service[1] & 0x3F == 0 {
                    event.payload.first() == Some(&service[0])
                        && event.payload.get(1).is_some_and(|byte| byte & 0xC0 == 0x40)
                } else {
                    event.payload.starts_with(&service)
                }
            });
            if matches_service {
                return Ok(event.payload);
            }
        }
    }

    /// Send a connected request and return the first response with the expected
    /// APCI service within `timeout`.
    pub async fn request(
        &mut self,
        apdu: Vec<u8>,
        expected_response_service: [u8; 2],
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>> {
        self.send_data(apdu, true).await?;
        self.receive(Some(expected_response_service), timeout).await
    }

    /// Read a single property-value range over this P2P session.
    pub async fn read_property_value(
        &mut self,
        request: crate::protocol::management::PropertyValueRead,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::PropertyValueResponse> {
        let response = self
            .request(request.encode()?, [0x03, 0xD6], timeout)
            .await?;
        let response = crate::protocol::management::PropertyValueResponse::decode(&response)?;
        if response.object_index != request.object_index
            || response.property_id != request.property_id
            || response.start_index != request.start_index
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "PropertyValueResponse does not match the requested object, property, and start index".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Read a property value range, splitting it into APDUs that fit the negotiated limit.
    pub async fn read_property_values(
        &mut self,
        request: crate::protocol::management::PropertyValueRead,
        max_apdu_length: usize,
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>> {
        if max_apdu_length == 0 || request.count == 0 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Property read requires a positive APDU length and element count"
                    .to_owned(),
            }
            .into());
        }
        if request.start_index == 0 && request.count != 1 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Property element-count query at index 0 requires count 1".to_owned(),
            }
            .into());
        }
        if u32::from(request.start_index) + u32::from(request.count) - 1 > 0x0FFF {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Property read range exceeds the 12-bit start index".to_owned(),
            }
            .into());
        }

        let mut data = Vec::new();
        let mut current_index = request.start_index;
        let mut remaining = request.count;
        let mut max_elements_per_chunk = 1;
        let mut element_size = None;

        while remaining > 0 {
            let chunk_count = remaining.min(max_elements_per_chunk);
            let response = self
                .read_property_value(
                    crate::protocol::management::PropertyValueRead {
                        object_index: request.object_index,
                        property_id: request.property_id,
                        count: chunk_count,
                        start_index: current_index,
                    },
                    timeout,
                )
                .await?;
            if response.count != chunk_count {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Property read requested {chunk_count} elements at index {current_index}, got {}",
                        response.count
                    ),
                }
                .into());
            }
            let response_size = response.data.len();
            if let Some(element_size) = element_size {
                if response_size != usize::from(chunk_count) * element_size {
                    return Err(crate::error::ProtocolError::InvalidFrame {
                        details:
                            "Property read response data length does not match its element count"
                                .to_owned(),
                    }
                    .into());
                }
            } else {
                if response_size == 0 {
                    return Err(crate::error::ProtocolError::InvalidFrame {
                        details: "Property read returned empty data for one element".to_owned(),
                    }
                    .into());
                }
                element_size = Some(response_size);
                max_elements_per_chunk =
                    (max_apdu_length.saturating_sub(6) / response_size).clamp(1, 15) as u8;
            }
            data.extend(response.data);
            remaining -= chunk_count;
            current_index += u16::from(chunk_count);
        }

        Ok(data)
    }

    /// Write a single property-value range over this P2P session.
    pub async fn write_property_value(
        &mut self,
        request: crate::protocol::management::PropertyValueWrite,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::PropertyValueResponse> {
        let response = self
            .request(request.encode()?, [0x03, 0xD6], timeout)
            .await?;
        let response = crate::protocol::management::PropertyValueResponse::decode(&response)?;
        if response.object_index != request.object_index
            || response.property_id != request.property_id
            || response.start_index != request.start_index
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "PropertyValueResponse does not match the requested object, property, and start index".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Verify a property value range by reading it back and comparing its bytes.
    pub async fn verify_property_values(
        &mut self,
        request: crate::protocol::management::PropertyValueRead,
        expected: &[u8],
        max_apdu_length: usize,
        timeout: std::time::Duration,
    ) -> Result<()> {
        if self
            .read_property_values(request, max_apdu_length, timeout)
            .await?
            != expected
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Property verification failed for object {} property {} at index {}",
                    request.object_index, request.property_id, request.start_index
                ),
            }
            .into());
        }
        Ok(())
    }

    /// Write a property value range, splitting it into APDUs that fit the negotiated limit.
    pub async fn write_property_values(
        &mut self,
        request: crate::protocol::management::PropertyValueWrite,
        max_apdu_length: usize,
        timeout: std::time::Duration,
    ) -> Result<()> {
        if max_apdu_length == 0 || request.count == 0 || request.start_index == 0 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Property write requires a positive APDU length, count, and start index"
                    .to_owned(),
            }
            .into());
        }
        if u32::from(request.start_index) + u32::from(request.count) - 1 > 0x0FFF {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Property write range exceeds the 12-bit start index".to_owned(),
            }
            .into());
        }
        let element_count = usize::from(request.count);
        if request.data.is_empty() || request.data.len() % element_count != 0 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details:
                    "Property write data must contain an equal, nonzero byte count per element"
                        .to_owned(),
            }
            .into());
        }
        let element_size = request.data.len() / element_count;
        let max_elements_per_chunk = (max_apdu_length.saturating_sub(6) / element_size).min(15);
        if max_elements_per_chunk == 0 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "One property element does not fit within the APDU length limit"
                    .to_owned(),
            }
            .into());
        }

        let mut current_index = request.start_index;
        let mut remaining = request.count;
        let mut data_offset = 0;
        while remaining > 0 {
            let chunk_count = remaining.min(max_elements_per_chunk as u8);
            let chunk_size = usize::from(chunk_count) * element_size;
            let response = self
                .write_property_value(
                    crate::protocol::management::PropertyValueWrite {
                        object_index: request.object_index,
                        property_id: request.property_id,
                        count: chunk_count,
                        start_index: current_index,
                        data: request.data[data_offset..data_offset + chunk_size].to_vec(),
                    },
                    timeout,
                )
                .await?;
            if response.count != chunk_count {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Property write requested {chunk_count} elements at index {current_index}, got {}",
                        response.count
                    ),
                }
                .into());
            }
            current_index += u16::from(chunk_count);
            remaining -= chunk_count;
            data_offset += chunk_size;
        }

        Ok(())
    }

    /// Read one range from the standard 64 KiB memory address space.
    pub async fn read_memory(
        &mut self,
        request: crate::protocol::management::MemoryRead,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::MemoryResponse> {
        let response = self
            .request(request.encode()?, [0x02, 0x40], timeout)
            .await?;
        let response = crate::protocol::management::MemoryResponse::decode(&response)?;
        if response.address != request.address {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "MemoryResponse address does not match the request".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Write one range to the standard 64 KiB memory address space.
    pub async fn write_memory(
        &mut self,
        request: crate::protocol::management::MemoryWrite,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::MemoryResponse> {
        let response = self
            .request(request.encode()?, [0x02, 0x40], timeout)
            .await?;
        let response = crate::protocol::management::MemoryResponse::decode(&response)?;
        if response.address != request.address {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "MemoryResponse address does not match the request".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Read one range from the 1 MiB user-memory address space.
    pub async fn read_user_memory(
        &mut self,
        request: crate::protocol::management::UserMemoryRead,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::UserMemoryResponse> {
        let response = self
            .request(request.encode()?, [0x02, 0xC1], timeout)
            .await?;
        let response = crate::protocol::management::UserMemoryResponse::decode(&response)?;
        if response.address != request.address {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "UserMemoryResponse address does not match the request".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Write one range to the 1 MiB user-memory address space.
    pub async fn write_user_memory(
        &mut self,
        request: crate::protocol::management::UserMemoryWrite,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::UserMemoryResponse> {
        let response = self
            .request(request.encode()?, [0x02, 0xC1], timeout)
            .await?;
        let response = crate::protocol::management::UserMemoryResponse::decode(&response)?;
        if response.address != request.address {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "UserMemoryResponse address does not match the request".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Read a standard-memory range in response-sized chunks.
    pub async fn read_memory_range(
        &mut self,
        address: u16,
        count: usize,
        max_apdu_length: usize,
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>> {
        if count == 0 || max_apdu_length <= 4 || count > 0x1_0000 - usize::from(address) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Memory read count, APDU length, or address range is invalid".to_owned(),
            }
            .into());
        }
        let chunk_limit = (max_apdu_length - 4).min(63);
        let mut data = Vec::with_capacity(count);
        let mut current_address = usize::from(address);
        while data.len() < count {
            let chunk_count = (count - data.len()).min(chunk_limit);
            let response = self
                .read_memory(
                    crate::protocol::management::MemoryRead {
                        address: current_address as u16,
                        count: chunk_count as u8,
                    },
                    timeout,
                )
                .await?;
            if usize::from(response.count) != chunk_count || response.data.len() != chunk_count {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Memory read at 0x{current_address:04X} requested {chunk_count} bytes, got {}",
                        response.count
                    ),
                }
                .into());
            }
            data.extend(response.data);
            current_address += chunk_count;
        }
        Ok(data)
    }

    /// Read a user-memory range in response-sized chunks.
    pub async fn read_user_memory_range(
        &mut self,
        address: u32,
        count: usize,
        max_apdu_length: usize,
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>> {
        if count == 0
            || max_apdu_length <= 5
            || address > 0x10_0000
            || count > 0x10_0000 - address as usize
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "User memory read count, APDU length, or address range is invalid"
                    .to_owned(),
            }
            .into());
        }
        let chunk_limit = (max_apdu_length - 5).min(15);
        let mut data = Vec::with_capacity(count);
        let mut current_address = address;
        while data.len() < count {
            let chunk_count = (count - data.len()).min(chunk_limit);
            let response = self
                .read_user_memory(
                    crate::protocol::management::UserMemoryRead {
                        address: current_address,
                        count: chunk_count as u8,
                    },
                    timeout,
                )
                .await?;
            if usize::from(response.count) != chunk_count || response.data.len() != chunk_count {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "User memory read at 0x{current_address:05X} requested {chunk_count} bytes, got {}",
                        response.count
                    ),
                }
                .into());
            }
            data.extend(response.data);
            current_address += chunk_count as u32;
        }
        Ok(data)
    }

    /// Write a standard-memory range in bounded chunks, optionally reading each back.
    pub async fn write_memory_range(
        &mut self,
        address: u16,
        data: &[u8],
        max_apdu_length: usize,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        if data.is_empty() || max_apdu_length <= 4 || data.len() > 0x1_0000 - usize::from(address) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Memory write data, APDU length, or address range is invalid".to_owned(),
            }
            .into());
        }
        let chunk_limit = (max_apdu_length - 4).min(63);
        let mut offset = 0;
        while offset < data.len() {
            let chunk = &data[offset..(offset + chunk_limit).min(data.len())];
            let current_address = usize::from(address) + offset;
            let response = self
                .write_memory(
                    crate::protocol::management::MemoryWrite {
                        address: current_address as u16,
                        count: chunk.len() as u8,
                        data: chunk.to_vec(),
                    },
                    timeout,
                )
                .await?;
            if usize::from(response.count) != chunk.len() {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Memory write at 0x{current_address:04X} requested {} bytes, got {}",
                        chunk.len(),
                        response.count
                    ),
                }
                .into());
            }
            if verify
                && self
                    .read_memory_range(
                        current_address as u16,
                        chunk.len(),
                        max_apdu_length,
                        timeout,
                    )
                    .await?
                    != chunk
            {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!("Memory verification failed at 0x{current_address:04X}"),
                }
                .into());
            }
            offset += chunk.len();
        }
        Ok(())
    }

    /// Write a user-memory range in bounded chunks, optionally reading each back.
    pub async fn write_user_memory_range(
        &mut self,
        address: u32,
        data: &[u8],
        max_apdu_length: usize,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        if data.is_empty()
            || max_apdu_length <= 5
            || address > 0x10_0000
            || data.len() > 0x10_0000 - address as usize
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "User memory write data, APDU length, or address range is invalid"
                    .to_owned(),
            }
            .into());
        }
        let chunk_limit = (max_apdu_length - 5).min(15);
        let mut offset = 0;
        while offset < data.len() {
            let chunk = &data[offset..(offset + chunk_limit).min(data.len())];
            let current_address = address + offset as u32;
            let response = self
                .write_user_memory(
                    crate::protocol::management::UserMemoryWrite {
                        address: current_address,
                        count: chunk.len() as u8,
                        data: chunk.to_vec(),
                    },
                    timeout,
                )
                .await?;
            if usize::from(response.count) != chunk.len() {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "User memory write at 0x{current_address:05X} requested {} bytes, got {}",
                        chunk.len(),
                        response.count
                    ),
                }
                .into());
            }
            if verify
                && self
                    .read_user_memory_range(current_address, chunk.len(), max_apdu_length, timeout)
                    .await?
                    != chunk
            {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!("User memory verification failed at 0x{current_address:05X}"),
                }
                .into());
            }
            offset += chunk.len();
        }
        Ok(())
    }

    /// Read metadata for one interface-object property over this P2P session.
    pub async fn read_property_description(
        &mut self,
        request: crate::protocol::management::PropertyDescriptionRead,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::PropertyDescriptionResponse> {
        let response = self
            .request(request.encode(), [0x03, 0xD9], timeout)
            .await?;
        let response = crate::protocol::management::PropertyDescriptionResponse::decode(&response)?;
        if response.object_index != request.object_index
            || (request.property_id != 0 && response.property_id != request.property_id)
            || response.property_index != request.property_index
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "PropertyDescriptionResponse does not match the requested object, property, and index".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Authorize this P2P session with a device access key.
    pub async fn authorize(
        &mut self,
        key: u32,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::AuthorizeResponse> {
        let response = self
            .request(
                crate::protocol::management::AuthorizeRequest { key }.encode(),
                [0x03, 0xD2],
                timeout,
            )
            .await?;
        crate::protocol::management::AuthorizeResponse::decode(&response)
    }

    /// Try free access and the configured key, returning the better access level.
    pub async fn authorize_best(
        &mut self,
        client_key: u32,
        timeout: std::time::Duration,
    ) -> Result<u8> {
        let free_level = self
            .authorize(crate::protocol::management::FREE_ACCESS_KEY, timeout)
            .await?
            .level;
        let client_level = self.authorize(client_key, timeout).await?.level;
        Ok(free_level.min(client_level))
    }

    /// Read one device descriptor over this P2P session.
    pub async fn read_device_descriptor(
        &mut self,
        descriptor: u8,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::DeviceDescriptorResponse> {
        if descriptor > 0x3F {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("Device descriptor type {descriptor} exceeds six bits"),
            }
            .into());
        }
        let response = self
            .request(
                crate::protocol::management::DeviceDescriptorRead { descriptor }.encode(),
                [0x03, 0x40],
                timeout,
            )
            .await?;
        let response = crate::protocol::management::DeviceDescriptorResponse::decode(&response)?;
        if response.descriptor != descriptor {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "DeviceDescriptorResponse type does not match the request".to_owned(),
            }
            .into());
        }
        Ok(response)
    }

    /// Confirm device management access and return the device mask version.
    pub async fn read_mask_version(&mut self, timeout: std::time::Duration) -> Result<u16> {
        let response = self.read_device_descriptor(0, timeout).await?;
        if response.data.len() != 2 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Mask version descriptor must contain two bytes".to_owned(),
            }
            .into());
        }
        Ok(u16::from_be_bytes([response.data[0], response.data[1]]))
    }

    /// Perform a confirmed Master Reset on the connected device.
    ///
    /// Wait at least the returned process_time before sending another request.
    pub async fn master_reset(
        &mut self,
        erase_code: u8,
        channel_number: u8,
        timeout: std::time::Duration,
    ) -> Result<crate::protocol::management::RestartMasterResetResponse> {
        if !(1..=8).contains(&erase_code) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("Master Reset erase code {erase_code} is not defined"),
            }
            .into());
        }
        let response = self
            .request(
                crate::protocol::management::RestartMasterReset {
                    erase_code,
                    channel_number,
                }
                .encode(),
                [0x03, 0xA1],
                timeout,
            )
            .await?;
        let response = crate::protocol::management::RestartMasterResetResponse::decode(&response)?;
        if response.error_code != 0 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Master Reset failed with error code 0x{:02X}",
                    response.error_code
                ),
            }
            .into());
        }
        Ok(response)
    }

    /// Discover interface objects and their property descriptions on this device.
    pub async fn scan_interface_objects(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Vec<ScannedInterfaceObject>> {
        let mut objects = Vec::new();
        for object_index in 0..=u8::MAX {
            let first_property = self
                .read_property_description(
                    crate::protocol::management::PropertyDescriptionRead {
                        object_index,
                        property_id: 0,
                        property_index: 0,
                    },
                    timeout,
                )
                .await?;
            if first_property.property_id == 0 {
                return Ok(objects);
            }

            let object_type = self
                .read_property_values(
                    crate::protocol::management::PropertyValueRead {
                        object_index,
                        property_id: 1,
                        count: 1,
                        start_index: 1,
                    },
                    15,
                    timeout,
                )
                .await?;
            if object_type.len() != 2 {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Object type for interface object {object_index} must be two bytes"
                    ),
                }
                .into());
            }

            let mut properties = Vec::new();
            let mut found_property_end = false;
            for property_index in 0..=u8::MAX {
                let property = self
                    .read_property_description(
                        crate::protocol::management::PropertyDescriptionRead {
                            object_index,
                            property_id: 0,
                            property_index,
                        },
                        timeout,
                    )
                    .await?;
                if property.property_id == 0 {
                    found_property_end = true;
                    break;
                }
                properties.push(property);
            }
            if !found_property_end {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!(
                        "Property list for interface object {object_index} has no terminator"
                    ),
                }
                .into());
            }
            objects.push(ScannedInterfaceObject {
                object_index,
                object_type: u16::from_be_bytes([object_type[0], object_type[1]]),
                properties,
            });
            if object_index == u8::MAX {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: "Interface object list has no terminator".to_owned(),
                }
                .into());
            }
        }
        Err(crate::error::ProtocolError::InvalidFrame {
            details: "Interface object scan ended without a terminator".to_owned(),
        }
        .into())
    }

    /// Tear down the transport link and unregister its receive callback.
    pub async fn close(&mut self) -> Result<()> {
        let result = if self.closed {
            Ok(())
        } else {
            self.closed = true;
            self.send_control(crate::protocol::TpciFrame::Disconnect)
                .await
        };
        if let Some(mut callback) = self.callback.take()
            && let Some(handle) = callback.handle.take()
        {
            callback.knx.unregister_callback(handle).await;
        }
        result
    }
}

impl Drop for P2pConnection {
    fn drop(&mut self) {
        if !self.closed
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let knx = self.knx.clone();
            let destination = self.destination;
            runtime.spawn(async move {
                let mut telegram = Telegram::individual_data(destination, Vec::new());
                telegram.telegram_type = TelegramType::IndividualControl;
                telegram.tpci = crate::protocol::TpciFrame::Disconnect.encode();
                let _ = knx.send_telegram_acknowledged(&telegram).await;
            });
        }
    }
}

impl std::fmt::Display for KnxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KnxState::Disconnected => write!(f, "disconnected"),
            KnxState::Connecting => write!(f, "connecting"),
            KnxState::Connected => write!(f, "connected"),
            KnxState::Disconnecting => write!(f, "disconnecting"),
            KnxState::Error => write!(f, "error"),
        }
    }
}

#[cfg(test)]
#[path = "handover_tests.rs"]
mod handover_tests;

#[cfg(test)]
#[path = "shutdown_tests.rs"]
mod shutdown_tests;

#[cfg(test)]
#[path = "channel_tests.rs"]
mod channel_tests;

impl From<KnxState> for crate::application::callbacks::ConnectionState {
    fn from(s: KnxState) -> Self {
        match s {
            KnxState::Disconnected => Self::Disconnected,
            KnxState::Connecting => Self::Connecting,
            KnxState::Connected => Self::Connected,
            KnxState::Disconnecting => Self::Disconnecting,
            KnxState::Error => Self::Error,
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::application::callbacks::{ConnectionState, TelegramFilter};
    use crate::protocol::{
        GroupValueService,
        address::{Address, GroupAddress, IndividualAddress, MainGroup, MiddleGroup},
        cemi::{CemiFrame, MessageCode},
        telegram::{Direction, Priority, Telegram, TelegramType},
    };
    use crate::transport::ConnectionType;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{Duration, sleep};

    /// Telegram callback that sleeps past the callback timeout, used to test
    /// error handling in `test_knx_callback_error_handling_integration`.
    struct SlowCallback {
        counter: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::application::callbacks::TelegramCallbackFn for SlowCallback {
        async fn call(&self, _telegram: &Telegram) {
            self.counter.fetch_add(1, Ordering::SeqCst);
            // Sleep longer than the callback timeout to test error handling
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    struct LoopbackCallback(tokio::sync::mpsc::Sender<Telegram>);

    #[async_trait::async_trait]
    impl crate::application::callbacks::TelegramCallbackFn for LoopbackCallback {
        async fn call(&self, telegram: &Telegram) {
            let _ = self.0.send(telegram.clone()).await;
        }
    }

    fn assert_parsed_group_service(
        service: &GroupValueService,
        expected_type: TelegramType,
        expected_payload: &[u8],
    ) -> Telegram {
        let source = IndividualAddress::new(1, 1, 5);
        let destination =
            Address::Group(GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3));
        let frame = CemiFrame::new(MessageCode::LDataInd, source, destination, service.encode());
        let before_parse = std::time::SystemTime::now();

        let telegram = Knx::parse_cemi_to_telegram(&frame.serialize().unwrap()).unwrap();

        let after_parse = std::time::SystemTime::now();
        assert_eq!(telegram.source, source);
        assert_eq!(telegram.destination, destination);
        assert_eq!(telegram.priority, Priority::Normal);
        assert_eq!(telegram.direction, Direction::Incoming);
        assert_eq!(telegram.telegram_type, expected_type);
        assert_eq!(telegram.payload, expected_payload);
        assert!(telegram.timestamp >= before_parse);
        assert!(telegram.timestamp <= after_parse);
        telegram
    }

    #[test]
    fn parse_cemi_to_telegram_classifies_group_value_read() {
        assert_parsed_group_service(
            &GroupValueService::Read,
            TelegramType::GroupValueRead,
            &[0x00],
        );
    }

    #[test]
    fn parse_cemi_to_telegram_classifies_group_value_response() {
        let telegram = assert_parsed_group_service(
            &GroupValueService::Response(vec![0x0c, 0x3f]),
            TelegramType::GroupValueResponse,
            &[0x0c, 0x3f],
        );
        assert_ne!(telegram.telegram_type, TelegramType::GroupValueWrite);
    }

    #[test]
    fn parse_cemi_to_telegram_classifies_group_value_write() {
        let telegram = assert_parsed_group_service(
            &GroupValueService::Write(vec![0x0c, 0x3f]),
            TelegramType::GroupValueWrite,
            &[0x0c, 0x3f],
        );
        assert_ne!(telegram.telegram_type, TelegramType::GroupValueResponse);
    }

    #[cfg(feature = "server")]
    #[tokio::test]
    async fn tunnel_server_loopback_round_trips_group_telegrams() {
        use crate::transport::TunnelServer;

        let client_address = IndividualAddress::new(1, 1, 8);
        let device_address = IndividualAddress::new(1, 1, 9);
        let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
            .await
            .unwrap();
        let knx = Knx::builder()
            .connection_type(ConnectionType::Tunneling)
            .gateway_ip(server.local_addr().ip())
            .gateway_port(server.local_addr().port())
            .individual_address(client_address)
            .build()
            .await
            .unwrap();
        knx.connect().await.unwrap();
        knx.start().await.unwrap();
        let mut server_traffic = server.subscribe();
        let group = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);

        knx.send_telegram(&Telegram::group_write(group, vec![7]))
            .await
            .unwrap();
        let sent = tokio::time::timeout(Duration::from_secs(2), server_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sent.destination, Address::Group(group));
        assert_eq!(sent.payload, [7]);

        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let callback = knx
            .register_telegram_callback_filtered(
                LoopbackCallback(tx),
                TelegramFilter::default(),
                false,
            )
            .await;
        let mut received = Telegram::group_write(group, vec![9]);
        received.source = device_address;
        server.send(received).await.unwrap();
        let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.source, device_address);
        assert_eq!(received.destination, Address::Group(group));
        assert_eq!(received.payload, [9]);

        knx.unregister_callback(callback).await;
        knx.shutdown().await.unwrap();
        server.shutdown();
    }

    #[cfg(feature = "server")]
    #[tokio::test]
    async fn individual_request_matches_device_and_response_service() {
        use crate::transport::TunnelServer;

        let client_address = IndividualAddress::new(1, 1, 10);
        let device_address = IndividualAddress::new(1, 1, 11);
        let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
            .await
            .unwrap();
        let server_addr = server.local_addr();
        let knx = Knx::builder()
            .connection_type(ConnectionType::Tunneling)
            .gateway_ip(server_addr.ip())
            .gateway_port(server_addr.port())
            .individual_address(client_address)
            .build()
            .await
            .unwrap();
        knx.connect().await.unwrap();
        knx.start().await.unwrap();
        let mut received = server.subscribe();

        let knx_for_request = knx.clone();
        let request = tokio::spawn(async move {
            knx_for_request
                .request_individual(
                    device_address,
                    vec![0x02, 0xC8, 0xAA],
                    [0x02, 0xC9],
                    Duration::from_secs(2),
                )
                .await
        });
        let outgoing = tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outgoing.telegram_type, TelegramType::IndividualData);
        assert_eq!(outgoing.destination, Address::Individual(device_address));
        assert_eq!(outgoing.payload, [0x02, 0xC8, 0xAA]);

        let mut wrong_service = Telegram::individual_data(client_address, vec![0x02, 0xC8, 0xBB]);
        wrong_service.source = device_address;
        server.send(wrong_service).await.unwrap();

        let mut wrong_sender = Telegram::individual_data(client_address, vec![0x02, 0xC9, 0x11]);
        wrong_sender.source = IndividualAddress::new(1, 1, 12);
        server.send(wrong_sender).await.unwrap();

        let mut response = Telegram::individual_data(client_address, vec![0x02, 0xC9, 0x42, 0x55]);
        response.source = device_address;
        server.send(response).await.unwrap();

        assert_eq!(request.await.unwrap().unwrap(), [0x02, 0xC9, 0x42, 0x55]);
        knx.shutdown().await.unwrap();
        server.shutdown();
    }

    #[cfg(feature = "server")]
    #[tokio::test]
    async fn p2p_session_connects_sends_acknowledged_data_and_closes() {
        use crate::protocol::TpciFrame;
        use crate::transport::TunnelServer;

        let client_address = IndividualAddress::new(1, 1, 20);
        let device_address = IndividualAddress::new(1, 1, 21);
        let server = TunnelServer::bind("127.0.0.1:0".parse().unwrap(), client_address)
            .await
            .unwrap();
        let knx = Knx::builder()
            .connection_type(ConnectionType::Tunneling)
            .gateway_ip(server.local_addr().ip())
            .gateway_port(server.local_addr().port())
            .individual_address(client_address)
            .build()
            .await
            .unwrap();
        knx.connect().await.unwrap();
        knx.start().await.unwrap();
        let mut peer_traffic = server.subscribe();

        let knx_for_connect = knx.clone();
        let connect =
            tokio::spawn(async move { knx_for_connect.connect_p2p(device_address).await });
        let connect_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            connect_request.telegram_type,
            TelegramType::IndividualControl
        );
        assert_eq!(connect_request.tpci, TpciFrame::Connect.encode());
        assert_eq!(
            connect_request.destination,
            Address::Individual(device_address)
        );

        let mut connect_response = Telegram::individual_data(client_address, Vec::new());
        connect_response.source = device_address;
        connect_response.telegram_type = TelegramType::IndividualControl;
        connect_response.tpci = TpciFrame::Connect.encode();
        server.send(connect_response).await.unwrap();
        let mut p2p = connect.await.unwrap().unwrap();
        assert!(knx.connect_p2p(device_address).await.is_err());

        let request = tokio::spawn(async move {
            let response = p2p
                .request(vec![0x02, 0xC8, 0x10], [0x02, 0xC9], Duration::from_secs(2))
                .await?;
            Ok::<_, KnxError>((p2p, response))
        });
        let data = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(data.telegram_type, TelegramType::IndividualData);
        assert_eq!(data.tpci, TpciFrame::DataConnected { sequence: 0 }.encode());
        assert_eq!(data.payload, [0x02, 0xC8, 0x10]);

        let mut stale_ack = Telegram::individual_data(client_address, Vec::new());
        stale_ack.source = device_address;
        stale_ack.telegram_type = TelegramType::IndividualControl;
        stale_ack.tpci = TpciFrame::Ack { sequence: 1 }.encode();
        server.send(stale_ack).await.unwrap();

        let mut data_nak = Telegram::individual_data(client_address, Vec::new());
        data_nak.source = device_address;
        data_nak.telegram_type = TelegramType::IndividualControl;
        data_nak.tpci = TpciFrame::Nak { sequence: 0 }.encode();
        server.send(data_nak).await.unwrap();
        let retry = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            retry.tpci,
            TpciFrame::DataConnected { sequence: 0 }.encode()
        );
        assert_eq!(retry.payload, [0x02, 0xC8, 0x10]);
        let timeout_retry = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            timeout_retry.tpci,
            TpciFrame::DataConnected { sequence: 0 }.encode()
        );
        assert_eq!(timeout_retry.payload, [0x02, 0xC8, 0x10]);

        let mut data_ack = Telegram::individual_data(client_address, Vec::new());
        data_ack.source = device_address;
        data_ack.telegram_type = TelegramType::IndividualControl;
        data_ack.tpci = TpciFrame::Ack { sequence: 0 }.encode();
        server.send(data_ack).await.unwrap();
        let mut response = Telegram::individual_data(client_address, vec![0x02, 0xC9, 0x42]);
        response.source = device_address;
        response.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
        server.send(response).await.unwrap();
        let (mut p2p, response_apdu) = request.await.unwrap().unwrap();
        assert_eq!(response_apdu, [0x02, 0xC9, 0x42]);
        let response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response_ack.telegram_type, TelegramType::IndividualControl);
        assert_eq!(response_ack.tpci, TpciFrame::Ack { sequence: 0 }.encode());

        let mut duplicate = Telegram::individual_data(client_address, vec![0x02, 0xC9, 0x42]);
        duplicate.source = device_address;
        duplicate.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
        server.send(duplicate).await.unwrap();
        assert!(p2p.receive(None, Duration::from_millis(50)).await.is_err());
        let duplicate_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(duplicate_ack.tpci, TpciFrame::Ack { sequence: 0 }.encode());

        let send_next = tokio::spawn(async move {
            p2p.send_data(vec![0x02, 0xC8, 0x20], true).await?;
            Ok::<_, KnxError>(p2p)
        });
        let next_data = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            next_data.tpci,
            TpciFrame::DataConnected { sequence: 1 }.encode()
        );
        let mut next_ack = Telegram::individual_data(client_address, Vec::new());
        next_ack.source = device_address;
        next_ack.telegram_type = TelegramType::IndividualControl;
        next_ack.tpci = TpciFrame::Ack { sequence: 1 }.encode();
        server.send(next_ack).await.unwrap();
        let mut p2p = send_next.await.unwrap().unwrap();

        let property_read = crate::protocol::management::PropertyValueRead {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 5,
        };
        let property_apdu = property_read.encode().unwrap();
        let reading = tokio::spawn(async move {
            let response = p2p
                .read_property_value(property_read, Duration::from_secs(2))
                .await?;
            Ok::<_, KnxError>((p2p, response))
        });
        let property_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            property_request.tpci,
            TpciFrame::DataConnected { sequence: 2 }.encode()
        );
        assert_eq!(property_request.payload, property_apdu);
        let mut property_ack = Telegram::individual_data(client_address, Vec::new());
        property_ack.source = device_address;
        property_ack.telegram_type = TelegramType::IndividualControl;
        property_ack.tpci = TpciFrame::Ack { sequence: 2 }.encode();
        server.send(property_ack).await.unwrap();
        let mut property_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyValueResponse {
                object_index: 1,
                property_id: 2,
                count: 1,
                start_index: 5,
                data: vec![0xAA, 0xBB],
            }
            .encode()
            .unwrap(),
        );
        property_response.source = device_address;
        property_response.tpci = TpciFrame::DataConnected { sequence: 1 }.encode();
        server.send(property_response).await.unwrap();
        let (mut p2p, property_response) = reading.await.unwrap().unwrap();
        assert_eq!(property_response.data, [0xAA, 0xBB]);
        let property_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            property_response_ack.tpci,
            TpciFrame::Ack { sequence: 1 }.encode()
        );

        let mismatched_read = crate::protocol::management::PropertyValueRead {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 6,
        };
        let reading = tokio::spawn(async move {
            let result = p2p
                .read_property_value(mismatched_read, Duration::from_secs(2))
                .await;
            Ok::<_, KnxError>((p2p, result))
        });
        let mismatched_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            mismatched_request.tpci,
            TpciFrame::DataConnected { sequence: 3 }.encode()
        );
        let mut mismatched_request_ack = Telegram::individual_data(client_address, Vec::new());
        mismatched_request_ack.source = device_address;
        mismatched_request_ack.telegram_type = TelegramType::IndividualControl;
        mismatched_request_ack.tpci = TpciFrame::Ack { sequence: 3 }.encode();
        server.send(mismatched_request_ack).await.unwrap();
        let mut mismatched_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyValueResponse {
                object_index: 1,
                property_id: 2,
                count: 1,
                start_index: 7,
                data: vec![0xCC],
            }
            .encode()
            .unwrap(),
        );
        mismatched_response.source = device_address;
        mismatched_response.tpci = TpciFrame::DataConnected { sequence: 2 }.encode();
        server.send(mismatched_response).await.unwrap();
        let (mut p2p, result) = reading.await.unwrap().unwrap();
        assert!(result.is_err());
        let mismatched_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            mismatched_response_ack.tpci,
            TpciFrame::Ack { sequence: 2 }.encode()
        );

        let property_write = crate::protocol::management::PropertyValueWrite {
            object_index: 1,
            property_id: 2,
            count: 1,
            start_index: 8,
            data: vec![0x12, 0x34],
        };
        let property_apdu = property_write.encode().unwrap();
        let writing = tokio::spawn(async move {
            let response = p2p
                .write_property_value(property_write, Duration::from_secs(2))
                .await?;
            Ok::<_, KnxError>((p2p, response))
        });
        let property_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            property_request.tpci,
            TpciFrame::DataConnected { sequence: 4 }.encode()
        );
        assert_eq!(property_request.payload, property_apdu);
        let mut property_ack = Telegram::individual_data(client_address, Vec::new());
        property_ack.source = device_address;
        property_ack.telegram_type = TelegramType::IndividualControl;
        property_ack.tpci = TpciFrame::Ack { sequence: 4 }.encode();
        server.send(property_ack).await.unwrap();
        let mut property_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyValueResponse {
                object_index: 1,
                property_id: 2,
                count: 1,
                start_index: 8,
                data: vec![0x12, 0x34],
            }
            .encode()
            .unwrap(),
        );
        property_response.source = device_address;
        property_response.tpci = TpciFrame::DataConnected { sequence: 3 }.encode();
        server.send(property_response).await.unwrap();
        let (mut p2p, property_response) = writing.await.unwrap().unwrap();
        assert_eq!(property_response.data, [0x12, 0x34]);
        let property_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            property_response_ack.tpci,
            TpciFrame::Ack { sequence: 3 }.encode()
        );

        let description_read = crate::protocol::management::PropertyDescriptionRead {
            object_index: 1,
            property_id: 2,
            property_index: 1,
        };
        let description_apdu = description_read.encode();
        let reading = tokio::spawn(async move {
            let response = p2p
                .read_property_description(description_read, Duration::from_secs(2))
                .await?;
            Ok::<_, KnxError>((p2p, response))
        });
        let description_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            description_request.tpci,
            TpciFrame::DataConnected { sequence: 5 }.encode()
        );
        assert_eq!(description_request.payload, description_apdu);
        let mut description_ack = Telegram::individual_data(client_address, Vec::new());
        description_ack.source = device_address;
        description_ack.telegram_type = TelegramType::IndividualControl;
        description_ack.tpci = TpciFrame::Ack { sequence: 5 }.encode();
        server.send(description_ack).await.unwrap();
        let mut description_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyDescriptionResponse {
                object_index: 1,
                property_id: 2,
                property_index: 1,
                type_: 2,
                max_count: 10,
                access: 0,
            }
            .encode(),
        );
        description_response.source = device_address;
        description_response.tpci = TpciFrame::DataConnected { sequence: 4 }.encode();
        server.send(description_response).await.unwrap();
        let (mut p2p, description) = reading.await.unwrap().unwrap();
        assert_eq!(description.max_count, 10);
        assert_eq!(description.pdt(), 2);
        let description_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            description_response_ack.tpci,
            TpciFrame::Ack { sequence: 4 }.encode()
        );

        assert!(
            p2p.read_property_values(
                crate::protocol::management::PropertyValueRead {
                    object_index: 1,
                    property_id: 2,
                    count: 3,
                    start_index: 4094,
                },
                15,
                Duration::from_secs(2),
            )
            .await
            .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
                .await
                .is_err()
        );

        let values = tokio::spawn(async move {
            let data = p2p
                .read_property_values(
                    crate::protocol::management::PropertyValueRead {
                        object_index: 1,
                        property_id: 2,
                        count: 3,
                        start_index: 10,
                    },
                    15,
                    Duration::from_secs(2),
                )
                .await?;
            Ok::<_, KnxError>((p2p, data))
        });
        let first_chunk = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::PropertyValueRead::decode(&first_chunk.payload).unwrap(),
            crate::protocol::management::PropertyValueRead {
                object_index: 1,
                property_id: 2,
                count: 1,
                start_index: 10,
            }
        );
        let mut first_chunk_ack = Telegram::individual_data(client_address, Vec::new());
        first_chunk_ack.source = device_address;
        first_chunk_ack.telegram_type = TelegramType::IndividualControl;
        first_chunk_ack.tpci = TpciFrame::Ack { sequence: 6 }.encode();
        server.send(first_chunk_ack).await.unwrap();
        let mut first_chunk_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyValueResponse {
                object_index: 1,
                property_id: 2,
                count: 1,
                start_index: 10,
                data: vec![0xAA, 0xBB],
            }
            .encode()
            .unwrap(),
        );
        first_chunk_response.source = device_address;
        first_chunk_response.tpci = TpciFrame::DataConnected { sequence: 5 }.encode();
        server.send(first_chunk_response).await.unwrap();
        let first_chunk_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            first_chunk_response_ack.tpci,
            TpciFrame::Ack { sequence: 5 }.encode()
        );
        let second_chunk = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::PropertyValueRead::decode(&second_chunk.payload).unwrap(),
            crate::protocol::management::PropertyValueRead {
                object_index: 1,
                property_id: 2,
                count: 2,
                start_index: 11,
            }
        );
        let mut second_chunk_ack = Telegram::individual_data(client_address, Vec::new());
        second_chunk_ack.source = device_address;
        second_chunk_ack.telegram_type = TelegramType::IndividualControl;
        second_chunk_ack.tpci = TpciFrame::Ack { sequence: 7 }.encode();
        server.send(second_chunk_ack).await.unwrap();
        let mut second_chunk_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyValueResponse {
                object_index: 1,
                property_id: 2,
                count: 2,
                start_index: 11,
                data: vec![0xCC, 0xDD, 0xEE, 0xFF],
            }
            .encode()
            .unwrap(),
        );
        second_chunk_response.source = device_address;
        second_chunk_response.tpci = TpciFrame::DataConnected { sequence: 6 }.encode();
        server.send(second_chunk_response).await.unwrap();
        let (mut p2p, values) = values.await.unwrap().unwrap();
        assert_eq!(values, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        let second_chunk_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            second_chunk_response_ack.tpci,
            TpciFrame::Ack { sequence: 6 }.encode()
        );

        assert!(
            p2p.write_property_values(
                crate::protocol::management::PropertyValueWrite {
                    object_index: 1,
                    property_id: 2,
                    count: 2,
                    start_index: 30,
                    data: vec![1, 2, 3],
                },
                10,
                Duration::from_secs(2),
            )
            .await
            .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
                .await
                .is_err()
        );

        let writes = tokio::spawn(async move {
            p2p.write_property_values(
                crate::protocol::management::PropertyValueWrite {
                    object_index: 1,
                    property_id: 2,
                    count: 3,
                    start_index: 20,
                    data: vec![1, 2, 3, 4, 5, 6],
                },
                10,
                Duration::from_secs(2),
            )
            .await?;
            Ok::<_, KnxError>(p2p)
        });
        let first_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::PropertyValueWrite::decode(&first_write.payload).unwrap(),
            crate::protocol::management::PropertyValueWrite {
                object_index: 1,
                property_id: 2,
                count: 2,
                start_index: 20,
                data: vec![1, 2, 3, 4],
            }
        );
        let mut first_write_ack = Telegram::individual_data(client_address, Vec::new());
        first_write_ack.source = device_address;
        first_write_ack.telegram_type = TelegramType::IndividualControl;
        first_write_ack.tpci = TpciFrame::Ack { sequence: 8 }.encode();
        server.send(first_write_ack).await.unwrap();
        let mut first_write_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyValueResponse {
                object_index: 1,
                property_id: 2,
                count: 2,
                start_index: 20,
                data: Vec::new(),
            }
            .encode()
            .unwrap(),
        );
        first_write_response.source = device_address;
        first_write_response.tpci = TpciFrame::DataConnected { sequence: 7 }.encode();
        server.send(first_write_response).await.unwrap();
        let first_write_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            first_write_response_ack.tpci,
            TpciFrame::Ack { sequence: 7 }.encode()
        );
        let second_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::PropertyValueWrite::decode(&second_write.payload).unwrap(),
            crate::protocol::management::PropertyValueWrite {
                object_index: 1,
                property_id: 2,
                count: 1,
                start_index: 22,
                data: vec![5, 6],
            }
        );
        let mut second_write_ack = Telegram::individual_data(client_address, Vec::new());
        second_write_ack.source = device_address;
        second_write_ack.telegram_type = TelegramType::IndividualControl;
        second_write_ack.tpci = TpciFrame::Ack { sequence: 9 }.encode();
        server.send(second_write_ack).await.unwrap();
        let mut second_write_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyValueResponse {
                object_index: 1,
                property_id: 2,
                count: 1,
                start_index: 22,
                data: Vec::new(),
            }
            .encode()
            .unwrap(),
        );
        second_write_response.source = device_address;
        second_write_response.tpci = TpciFrame::DataConnected { sequence: 8 }.encode();
        server.send(second_write_response).await.unwrap();
        let mut p2p = writes.await.unwrap().unwrap();
        let second_write_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            second_write_response_ack.tpci,
            TpciFrame::Ack { sequence: 8 }.encode()
        );

        let reading_memory = tokio::spawn(async move {
            let response = p2p
                .read_memory(
                    crate::protocol::management::MemoryRead {
                        address: 0x1234,
                        count: 2,
                    },
                    Duration::from_secs(2),
                )
                .await?;
            Ok::<_, KnxError>((p2p, response))
        });
        let memory_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::MemoryRead::decode(&memory_read.payload).unwrap(),
            crate::protocol::management::MemoryRead {
                address: 0x1234,
                count: 2,
            }
        );
        let mut memory_read_ack = Telegram::individual_data(client_address, Vec::new());
        memory_read_ack.source = device_address;
        memory_read_ack.telegram_type = TelegramType::IndividualControl;
        memory_read_ack.tpci = TpciFrame::Ack { sequence: 10 }.encode();
        server.send(memory_read_ack).await.unwrap();
        let mut memory_read_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::MemoryResponse {
                address: 0x1234,
                count: 2,
                data: vec![0xAA, 0xBB],
            }
            .encode()
            .unwrap(),
        );
        memory_read_response.source = device_address;
        memory_read_response.tpci = TpciFrame::DataConnected { sequence: 9 }.encode();
        server.send(memory_read_response).await.unwrap();
        let (mut p2p, memory_response) = reading_memory.await.unwrap().unwrap();
        assert_eq!(memory_response.data, [0xAA, 0xBB]);
        let memory_read_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            memory_read_response_ack.tpci,
            TpciFrame::Ack { sequence: 9 }.encode()
        );

        let reading_user_memory = tokio::spawn(async move {
            let response = p2p
                .read_user_memory(
                    crate::protocol::management::UserMemoryRead {
                        address: 0xABCDE,
                        count: 2,
                    },
                    Duration::from_secs(2),
                )
                .await?;
            Ok::<_, KnxError>((p2p, response))
        });
        let user_memory_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::UserMemoryRead::decode(&user_memory_read.payload)
                .unwrap()
                .address,
            0xABCDE
        );
        let mut user_memory_read_ack = Telegram::individual_data(client_address, Vec::new());
        user_memory_read_ack.source = device_address;
        user_memory_read_ack.telegram_type = TelegramType::IndividualControl;
        user_memory_read_ack.tpci = TpciFrame::Ack { sequence: 11 }.encode();
        server.send(user_memory_read_ack).await.unwrap();
        let mut user_memory_read_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::UserMemoryResponse {
                address: 0xABCDE,
                count: 2,
                data: vec![0xCC, 0xDD],
            }
            .encode()
            .unwrap(),
        );
        user_memory_read_response.source = device_address;
        user_memory_read_response.tpci = TpciFrame::DataConnected { sequence: 10 }.encode();
        server.send(user_memory_read_response).await.unwrap();
        let (mut p2p, user_memory_response) = reading_user_memory.await.unwrap().unwrap();
        assert_eq!(user_memory_response.data, [0xCC, 0xDD]);
        let user_memory_read_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            user_memory_read_response_ack.tpci,
            TpciFrame::Ack { sequence: 10 }.encode()
        );

        let writing_memory = tokio::spawn(async move {
            let response = p2p
                .write_memory(
                    crate::protocol::management::MemoryWrite {
                        address: 0x2345,
                        count: 2,
                        data: vec![0x11, 0x22],
                    },
                    Duration::from_secs(2),
                )
                .await?;
            Ok::<_, KnxError>((p2p, response))
        });
        let memory_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::MemoryWrite::decode(&memory_write.payload)
                .unwrap()
                .address,
            0x2345
        );
        let mut memory_write_ack = Telegram::individual_data(client_address, Vec::new());
        memory_write_ack.source = device_address;
        memory_write_ack.telegram_type = TelegramType::IndividualControl;
        memory_write_ack.tpci = TpciFrame::Ack { sequence: 12 }.encode();
        server.send(memory_write_ack).await.unwrap();
        let mut memory_write_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::MemoryResponse {
                address: 0x2345,
                count: 2,
                data: Vec::new(),
            }
            .encode()
            .unwrap(),
        );
        memory_write_response.source = device_address;
        memory_write_response.tpci = TpciFrame::DataConnected { sequence: 11 }.encode();
        server.send(memory_write_response).await.unwrap();
        let (mut p2p, _) = writing_memory.await.unwrap().unwrap();
        let memory_write_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            memory_write_response_ack.tpci,
            TpciFrame::Ack { sequence: 11 }.encode()
        );

        let writing_user_memory = tokio::spawn(async move {
            let response = p2p
                .write_user_memory(
                    crate::protocol::management::UserMemoryWrite {
                        address: 0xFEDCB,
                        count: 1,
                        data: vec![0x33],
                    },
                    Duration::from_secs(2),
                )
                .await?;
            Ok::<_, KnxError>((p2p, response))
        });
        let user_memory_write = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::UserMemoryWrite::decode(&user_memory_write.payload)
                .unwrap()
                .address,
            0xFEDCB
        );
        let mut user_memory_write_ack = Telegram::individual_data(client_address, Vec::new());
        user_memory_write_ack.source = device_address;
        user_memory_write_ack.telegram_type = TelegramType::IndividualControl;
        user_memory_write_ack.tpci = TpciFrame::Ack { sequence: 13 }.encode();
        server.send(user_memory_write_ack).await.unwrap();
        let mut user_memory_write_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::UserMemoryResponse {
                address: 0xFEDCB,
                count: 1,
                data: Vec::new(),
            }
            .encode()
            .unwrap(),
        );
        user_memory_write_response.source = device_address;
        user_memory_write_response.tpci = TpciFrame::DataConnected { sequence: 12 }.encode();
        server.send(user_memory_write_response).await.unwrap();
        let (mut p2p, _) = writing_user_memory.await.unwrap().unwrap();
        let user_memory_write_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            user_memory_write_response_ack.tpci,
            TpciFrame::Ack { sequence: 12 }.encode()
        );

        assert!(
            p2p.read_memory_range(0, usize::MAX, 15, Duration::from_secs(2))
                .await
                .is_err()
        );
        assert!(
            p2p.read_user_memory_range(0, usize::MAX, 15, Duration::from_secs(2))
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
                .await
                .is_err()
        );

        let reading_memory_range = tokio::spawn(async move {
            let data = p2p
                .read_memory_range(0x1000, 15, 15, Duration::from_secs(2))
                .await?;
            Ok::<_, KnxError>((p2p, data))
        });
        let first_memory_chunk = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::MemoryRead::decode(&first_memory_chunk.payload)
                .unwrap()
                .count,
            11
        );
        let mut first_memory_chunk_ack = Telegram::individual_data(client_address, Vec::new());
        first_memory_chunk_ack.source = device_address;
        first_memory_chunk_ack.telegram_type = TelegramType::IndividualControl;
        first_memory_chunk_ack.tpci = TpciFrame::Ack { sequence: 14 }.encode();
        server.send(first_memory_chunk_ack).await.unwrap();
        let mut first_memory_chunk_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::MemoryResponse {
                address: 0x1000,
                count: 11,
                data: vec![0x11; 11],
            }
            .encode()
            .unwrap(),
        );
        first_memory_chunk_response.source = device_address;
        first_memory_chunk_response.tpci = TpciFrame::DataConnected { sequence: 13 }.encode();
        server.send(first_memory_chunk_response).await.unwrap();
        let first_memory_chunk_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            first_memory_chunk_response_ack.tpci,
            TpciFrame::Ack { sequence: 13 }.encode()
        );
        let second_memory_chunk = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::MemoryRead::decode(&second_memory_chunk.payload)
                .unwrap()
                .count,
            4
        );
        let mut second_memory_chunk_ack = Telegram::individual_data(client_address, Vec::new());
        second_memory_chunk_ack.source = device_address;
        second_memory_chunk_ack.telegram_type = TelegramType::IndividualControl;
        second_memory_chunk_ack.tpci = TpciFrame::Ack { sequence: 15 }.encode();
        server.send(second_memory_chunk_ack).await.unwrap();
        let mut second_memory_chunk_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::MemoryResponse {
                address: 0x100B,
                count: 4,
                data: vec![0x22; 4],
            }
            .encode()
            .unwrap(),
        );
        second_memory_chunk_response.source = device_address;
        second_memory_chunk_response.tpci = TpciFrame::DataConnected { sequence: 14 }.encode();
        server.send(second_memory_chunk_response).await.unwrap();
        let (mut p2p, memory_data) = reading_memory_range.await.unwrap().unwrap();
        assert_eq!(&memory_data[..11], &[0x11; 11]);
        assert_eq!(&memory_data[11..], &[0x22; 4]);
        let second_memory_chunk_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            second_memory_chunk_response_ack.tpci,
            TpciFrame::Ack { sequence: 14 }.encode()
        );

        let reading_user_memory_range = tokio::spawn(async move {
            let data = p2p
                .read_user_memory_range(0xABCDE, 16, 15, Duration::from_secs(2))
                .await?;
            Ok::<_, KnxError>((p2p, data))
        });
        let first_user_memory_chunk =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            crate::protocol::management::UserMemoryRead::decode(&first_user_memory_chunk.payload)
                .unwrap()
                .count,
            10
        );
        let mut first_user_memory_chunk_ack = Telegram::individual_data(client_address, Vec::new());
        first_user_memory_chunk_ack.source = device_address;
        first_user_memory_chunk_ack.telegram_type = TelegramType::IndividualControl;
        first_user_memory_chunk_ack.tpci = TpciFrame::Ack { sequence: 0 }.encode();
        server.send(first_user_memory_chunk_ack).await.unwrap();
        let mut first_user_memory_chunk_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::UserMemoryResponse {
                address: 0xABCDE,
                count: 10,
                data: vec![0x33; 10],
            }
            .encode()
            .unwrap(),
        );
        first_user_memory_chunk_response.source = device_address;
        first_user_memory_chunk_response.tpci = TpciFrame::DataConnected { sequence: 15 }.encode();
        server.send(first_user_memory_chunk_response).await.unwrap();
        let first_user_memory_chunk_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            first_user_memory_chunk_response_ack.tpci,
            TpciFrame::Ack { sequence: 15 }.encode()
        );
        let second_user_memory_chunk =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            crate::protocol::management::UserMemoryRead::decode(&second_user_memory_chunk.payload)
                .unwrap()
                .count,
            6
        );
        let mut second_user_memory_chunk_ack =
            Telegram::individual_data(client_address, Vec::new());
        second_user_memory_chunk_ack.source = device_address;
        second_user_memory_chunk_ack.telegram_type = TelegramType::IndividualControl;
        second_user_memory_chunk_ack.tpci = TpciFrame::Ack { sequence: 1 }.encode();
        server.send(second_user_memory_chunk_ack).await.unwrap();
        let mut second_user_memory_chunk_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::UserMemoryResponse {
                address: 0xABCE8,
                count: 6,
                data: vec![0x44; 6],
            }
            .encode()
            .unwrap(),
        );
        second_user_memory_chunk_response.source = device_address;
        second_user_memory_chunk_response.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
        server
            .send(second_user_memory_chunk_response)
            .await
            .unwrap();
        let (mut p2p, user_memory_data) = reading_user_memory_range.await.unwrap().unwrap();
        assert_eq!(&user_memory_data[..10], &[0x33; 10]);
        assert_eq!(&user_memory_data[10..], &[0x44; 6]);
        let second_user_memory_chunk_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            second_user_memory_chunk_response_ack.tpci,
            TpciFrame::Ack { sequence: 0 }.encode()
        );

        assert!(
            p2p.write_memory_range(0xFFFF, &[1, 2], 15, false, Duration::from_secs(2),)
                .await
                .is_err()
        );
        assert!(
            p2p.write_user_memory_range(0x0F_FFFF, &[1, 2], 15, false, Duration::from_secs(2),)
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
                .await
                .is_err()
        );

        let writing_user_memory_range = tokio::spawn(async move {
            p2p.write_user_memory_range(
                0xABC00,
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
                15,
                false,
                Duration::from_secs(2),
            )
            .await?;
            Ok::<_, KnxError>(p2p)
        });
        let first_user_memory_write =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            crate::protocol::management::UserMemoryWrite::decode(&first_user_memory_write.payload)
                .unwrap(),
            crate::protocol::management::UserMemoryWrite {
                address: 0xABC00,
                count: 10,
                data: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            }
        );
        let mut first_user_memory_write_ack = Telegram::individual_data(client_address, Vec::new());
        first_user_memory_write_ack.source = device_address;
        first_user_memory_write_ack.telegram_type = TelegramType::IndividualControl;
        first_user_memory_write_ack.tpci = TpciFrame::Ack { sequence: 2 }.encode();
        server.send(first_user_memory_write_ack).await.unwrap();
        let mut first_user_memory_write_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::UserMemoryResponse {
                address: 0xABC00,
                count: 10,
                data: Vec::new(),
            }
            .encode()
            .unwrap(),
        );
        first_user_memory_write_response.source = device_address;
        first_user_memory_write_response.tpci = TpciFrame::DataConnected { sequence: 1 }.encode();
        server.send(first_user_memory_write_response).await.unwrap();
        let first_user_memory_write_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            first_user_memory_write_response_ack.tpci,
            TpciFrame::Ack { sequence: 1 }.encode()
        );
        let second_user_memory_write =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            crate::protocol::management::UserMemoryWrite::decode(&second_user_memory_write.payload)
                .unwrap(),
            crate::protocol::management::UserMemoryWrite {
                address: 0xABC0A,
                count: 6,
                data: vec![11, 12, 13, 14, 15, 16],
            }
        );
        let mut second_user_memory_write_ack =
            Telegram::individual_data(client_address, Vec::new());
        second_user_memory_write_ack.source = device_address;
        second_user_memory_write_ack.telegram_type = TelegramType::IndividualControl;
        second_user_memory_write_ack.tpci = TpciFrame::Ack { sequence: 3 }.encode();
        server.send(second_user_memory_write_ack).await.unwrap();
        let mut second_user_memory_write_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::UserMemoryResponse {
                address: 0xABC0A,
                count: 6,
                data: Vec::new(),
            }
            .encode()
            .unwrap(),
        );
        second_user_memory_write_response.source = device_address;
        second_user_memory_write_response.tpci = TpciFrame::DataConnected { sequence: 2 }.encode();
        server
            .send(second_user_memory_write_response)
            .await
            .unwrap();
        let mut p2p = writing_user_memory_range.await.unwrap().unwrap();
        let second_user_memory_write_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            second_user_memory_write_response_ack.tpci,
            TpciFrame::Ack { sequence: 2 }.encode()
        );

        let writing_verified_memory = tokio::spawn(async move {
            let result = p2p
                .write_memory_range(
                    0x2000,
                    &(1..=18).collect::<Vec<u8>>(),
                    10,
                    true,
                    Duration::from_secs(2),
                )
                .await;
            Ok::<_, KnxError>((p2p, result))
        });
        let first_verified_write =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            crate::protocol::management::MemoryWrite::decode(&first_verified_write.payload)
                .unwrap()
                .count,
            6
        );
        let mut first_verified_write_ack = Telegram::individual_data(client_address, Vec::new());
        first_verified_write_ack.source = device_address;
        first_verified_write_ack.telegram_type = TelegramType::IndividualControl;
        first_verified_write_ack.tpci = TpciFrame::Ack { sequence: 4 }.encode();
        server.send(first_verified_write_ack).await.unwrap();
        let mut first_verified_write_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::MemoryResponse {
                address: 0x2000,
                count: 6,
                data: Vec::new(),
            }
            .encode()
            .unwrap(),
        );
        first_verified_write_response.source = device_address;
        first_verified_write_response.tpci = TpciFrame::DataConnected { sequence: 3 }.encode();
        server.send(first_verified_write_response).await.unwrap();
        let first_verified_write_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            first_verified_write_response_ack.tpci,
            TpciFrame::Ack { sequence: 3 }.encode()
        );
        let first_verification_read =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            crate::protocol::management::MemoryRead::decode(&first_verification_read.payload)
                .unwrap()
                .address,
            0x2000
        );
        let mut first_verification_read_ack = Telegram::individual_data(client_address, Vec::new());
        first_verification_read_ack.source = device_address;
        first_verification_read_ack.telegram_type = TelegramType::IndividualControl;
        first_verification_read_ack.tpci = TpciFrame::Ack { sequence: 5 }.encode();
        server.send(first_verification_read_ack).await.unwrap();
        let mut first_verification_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::MemoryResponse {
                address: 0x2000,
                count: 6,
                data: vec![1, 2, 3, 4, 5, 6],
            }
            .encode()
            .unwrap(),
        );
        first_verification_response.source = device_address;
        first_verification_response.tpci = TpciFrame::DataConnected { sequence: 4 }.encode();
        server.send(first_verification_response).await.unwrap();
        let first_verification_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            first_verification_response_ack.tpci,
            TpciFrame::Ack { sequence: 4 }.encode()
        );
        let second_verified_write =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            crate::protocol::management::MemoryWrite::decode(&second_verified_write.payload)
                .unwrap()
                .address,
            0x2006
        );
        let mut second_verified_write_ack = Telegram::individual_data(client_address, Vec::new());
        second_verified_write_ack.source = device_address;
        second_verified_write_ack.telegram_type = TelegramType::IndividualControl;
        second_verified_write_ack.tpci = TpciFrame::Ack { sequence: 6 }.encode();
        server.send(second_verified_write_ack).await.unwrap();
        let mut second_verified_write_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::MemoryResponse {
                address: 0x2006,
                count: 6,
                data: Vec::new(),
            }
            .encode()
            .unwrap(),
        );
        second_verified_write_response.source = device_address;
        second_verified_write_response.tpci = TpciFrame::DataConnected { sequence: 5 }.encode();
        server.send(second_verified_write_response).await.unwrap();
        let second_verified_write_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            second_verified_write_response_ack.tpci,
            TpciFrame::Ack { sequence: 5 }.encode()
        );
        let second_verification_read =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            crate::protocol::management::MemoryRead::decode(&second_verification_read.payload)
                .unwrap()
                .address,
            0x2006
        );
        let mut second_verification_read_ack =
            Telegram::individual_data(client_address, Vec::new());
        second_verification_read_ack.source = device_address;
        second_verification_read_ack.telegram_type = TelegramType::IndividualControl;
        second_verification_read_ack.tpci = TpciFrame::Ack { sequence: 7 }.encode();
        server.send(second_verification_read_ack).await.unwrap();
        let mut mismatched_verification_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::MemoryResponse {
                address: 0x2006,
                count: 6,
                data: vec![0xFF; 6],
            }
            .encode()
            .unwrap(),
        );
        mismatched_verification_response.source = device_address;
        mismatched_verification_response.tpci = TpciFrame::DataConnected { sequence: 6 }.encode();
        server.send(mismatched_verification_response).await.unwrap();
        let (mut p2p, result) = writing_verified_memory.await.unwrap().unwrap();
        assert!(result.is_err());
        let mismatched_verification_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            mismatched_verification_ack.tpci,
            TpciFrame::Ack { sequence: 6 }.encode()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
                .await
                .is_err()
        );

        let verifying_property = tokio::spawn(async move {
            let result = p2p
                .verify_property_values(
                    crate::protocol::management::PropertyValueRead {
                        object_index: 1,
                        property_id: 2,
                        count: 1,
                        start_index: 30,
                    },
                    &[0x5A],
                    15,
                    Duration::from_secs(2),
                )
                .await;
            Ok::<_, KnxError>((p2p, result))
        });
        let property_verification_read =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            crate::protocol::management::PropertyValueRead::decode(
                &property_verification_read.payload
            )
            .unwrap()
            .start_index,
            30
        );
        let mut property_verification_ack = Telegram::individual_data(client_address, Vec::new());
        property_verification_ack.source = device_address;
        property_verification_ack.telegram_type = TelegramType::IndividualControl;
        property_verification_ack.tpci = TpciFrame::Ack { sequence: 8 }.encode();
        server.send(property_verification_ack).await.unwrap();
        let mut property_verification_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyValueResponse {
                object_index: 1,
                property_id: 2,
                count: 1,
                start_index: 30,
                data: vec![0x5A],
            }
            .encode()
            .unwrap(),
        );
        property_verification_response.source = device_address;
        property_verification_response.tpci = TpciFrame::DataConnected { sequence: 7 }.encode();
        server.send(property_verification_response).await.unwrap();
        let (mut p2p, result) = verifying_property.await.unwrap().unwrap();
        result.unwrap();
        let property_verification_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            property_verification_response_ack.tpci,
            TpciFrame::Ack { sequence: 7 }.encode()
        );

        let verifying_mismatched_property = tokio::spawn(async move {
            let result = p2p
                .verify_property_values(
                    crate::protocol::management::PropertyValueRead {
                        object_index: 1,
                        property_id: 2,
                        count: 1,
                        start_index: 31,
                    },
                    &[0x00],
                    15,
                    Duration::from_secs(2),
                )
                .await;
            Ok::<_, KnxError>((p2p, result))
        });
        let mismatched_property_read =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            crate::protocol::management::PropertyValueRead::decode(
                &mismatched_property_read.payload
            )
            .unwrap()
            .start_index,
            31
        );
        let mut mismatched_property_ack = Telegram::individual_data(client_address, Vec::new());
        mismatched_property_ack.source = device_address;
        mismatched_property_ack.telegram_type = TelegramType::IndividualControl;
        mismatched_property_ack.tpci = TpciFrame::Ack { sequence: 9 }.encode();
        server.send(mismatched_property_ack).await.unwrap();
        let mut mismatched_property_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyValueResponse {
                object_index: 1,
                property_id: 2,
                count: 1,
                start_index: 31,
                data: vec![0xFE],
            }
            .encode()
            .unwrap(),
        );
        mismatched_property_response.source = device_address;
        mismatched_property_response.tpci = TpciFrame::DataConnected { sequence: 8 }.encode();
        server.send(mismatched_property_response).await.unwrap();
        let (mut p2p, result) = verifying_mismatched_property.await.unwrap().unwrap();
        assert!(result.is_err());
        let mismatched_property_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            mismatched_property_response_ack.tpci,
            TpciFrame::Ack { sequence: 8 }.encode()
        );

        let scanning = tokio::spawn(async move {
            let objects = p2p.scan_interface_objects(Duration::from_secs(2)).await?;
            Ok::<_, KnxError>((p2p, objects))
        });
        let object_probe = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::PropertyDescriptionRead::decode(&object_probe.payload)
                .unwrap(),
            crate::protocol::management::PropertyDescriptionRead {
                object_index: 0,
                property_id: 0,
                property_index: 0,
            }
        );
        let mut object_probe_ack = Telegram::individual_data(client_address, Vec::new());
        object_probe_ack.source = device_address;
        object_probe_ack.telegram_type = TelegramType::IndividualControl;
        object_probe_ack.tpci = TpciFrame::Ack { sequence: 10 }.encode();
        server.send(object_probe_ack).await.unwrap();
        let mut object_probe_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyDescriptionResponse {
                object_index: 0,
                property_id: 1,
                property_index: 0,
                type_: 0x82,
                max_count: 1,
                access: 0,
            }
            .encode(),
        );
        object_probe_response.source = device_address;
        object_probe_response.tpci = TpciFrame::DataConnected { sequence: 9 }.encode();
        server.send(object_probe_response).await.unwrap();
        let object_probe_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            object_probe_response_ack.tpci,
            TpciFrame::Ack { sequence: 9 }.encode()
        );

        let object_type_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::PropertyValueRead::decode(&object_type_read.payload)
                .unwrap(),
            crate::protocol::management::PropertyValueRead {
                object_index: 0,
                property_id: 1,
                count: 1,
                start_index: 1,
            }
        );
        let mut object_type_ack = Telegram::individual_data(client_address, Vec::new());
        object_type_ack.source = device_address;
        object_type_ack.telegram_type = TelegramType::IndividualControl;
        object_type_ack.tpci = TpciFrame::Ack { sequence: 11 }.encode();
        server.send(object_type_ack).await.unwrap();
        let mut object_type_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::PropertyValueResponse {
                object_index: 0,
                property_id: 1,
                count: 1,
                start_index: 1,
                data: vec![0, 0x11],
            }
            .encode()
            .unwrap(),
        );
        object_type_response.source = device_address;
        object_type_response.tpci = TpciFrame::DataConnected { sequence: 10 }.encode();
        server.send(object_type_response).await.unwrap();
        let object_type_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            object_type_response_ack.tpci,
            TpciFrame::Ack { sequence: 10 }.encode()
        );

        for (request_sequence, response_sequence, property_index, property_id) in [
            (12, 11, 0, 2),
            (13, 12, 1, 3),
            (14, 13, 2, 0),
            (15, 14, 0, 0),
        ] {
            let property_probe = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
            let request = crate::protocol::management::PropertyDescriptionRead::decode(
                &property_probe.payload,
            )
            .unwrap();
            assert_eq!(
                request.object_index,
                if request_sequence == 15 { 1 } else { 0 }
            );
            assert_eq!(request.property_id, 0);
            assert_eq!(request.property_index, property_index);
            let mut property_probe_ack = Telegram::individual_data(client_address, Vec::new());
            property_probe_ack.source = device_address;
            property_probe_ack.telegram_type = TelegramType::IndividualControl;
            property_probe_ack.tpci = TpciFrame::Ack {
                sequence: request_sequence,
            }
            .encode();
            server.send(property_probe_ack).await.unwrap();
            let mut property_probe_response = Telegram::individual_data(
                client_address,
                crate::protocol::management::PropertyDescriptionResponse {
                    object_index: request.object_index,
                    property_id,
                    property_index,
                    type_: 0x82,
                    max_count: 4,
                    access: 0,
                }
                .encode(),
            );
            property_probe_response.source = device_address;
            property_probe_response.tpci = TpciFrame::DataConnected {
                sequence: response_sequence,
            }
            .encode();
            server.send(property_probe_response).await.unwrap();
            let property_probe_response_ack =
                tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(
                property_probe_response_ack.tpci,
                TpciFrame::Ack {
                    sequence: response_sequence,
                }
                .encode()
            );
        }
        let (mut p2p, objects) = scanning.await.unwrap().unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].object_index, 0);
        assert_eq!(objects[0].object_type, 0x0011);
        assert_eq!(objects[0].properties.len(), 2);
        assert_eq!(objects[0].properties[0].property_id, 2);
        assert_eq!(objects[0].properties[1].property_id, 3);

        assert!(
            p2p.master_reset(0, 2, Duration::from_secs(2))
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
                .await
                .is_err()
        );

        let resetting = tokio::spawn(async move {
            let response = p2p.master_reset(1, 2, Duration::from_secs(2)).await?;
            Ok::<_, KnxError>((p2p, response))
        });
        let reset_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reset_request.payload, [0x03, 0x81, 1, 2]);
        assert_eq!(
            reset_request.tpci,
            TpciFrame::DataConnected { sequence: 0 }.encode()
        );
        let mut reset_ack = Telegram::individual_data(client_address, Vec::new());
        reset_ack.source = device_address;
        reset_ack.telegram_type = TelegramType::IndividualControl;
        reset_ack.tpci = TpciFrame::Ack { sequence: 0 }.encode();
        server.send(reset_ack).await.unwrap();
        let mut reset_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::RestartMasterResetResponse {
                error_code: 0,
                process_time: 10,
            }
            .encode(),
        );
        reset_response.source = device_address;
        reset_response.tpci = TpciFrame::DataConnected { sequence: 15 }.encode();
        server.send(reset_response).await.unwrap();
        let (mut p2p, reset_response) = resetting.await.unwrap().unwrap();
        assert_eq!(reset_response.process_time, 10);
        let reset_response_ack = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reset_response_ack.tpci,
            TpciFrame::Ack { sequence: 15 }.encode()
        );

        let resetting_with_error = tokio::spawn(async move {
            let result = p2p.master_reset(2, 0, Duration::from_secs(2)).await;
            Ok::<_, KnxError>((p2p, result))
        });
        let reset_error_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reset_error_request.payload, [0x03, 0x81, 2, 0]);
        let mut reset_error_ack = Telegram::individual_data(client_address, Vec::new());
        reset_error_ack.source = device_address;
        reset_error_ack.telegram_type = TelegramType::IndividualControl;
        reset_error_ack.tpci = TpciFrame::Ack { sequence: 1 }.encode();
        server.send(reset_error_ack).await.unwrap();
        let mut reset_error_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::RestartMasterResetResponse {
                error_code: 1,
                process_time: 0,
            }
            .encode(),
        );
        reset_error_response.source = device_address;
        reset_error_response.tpci = TpciFrame::DataConnected { sequence: 0 }.encode();
        server.send(reset_error_response).await.unwrap();
        let (mut p2p, result) = resetting_with_error.await.unwrap().unwrap();
        assert!(result.is_err());
        let reset_error_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            reset_error_response_ack.tpci,
            TpciFrame::Ack { sequence: 0 }.encode()
        );

        assert!(
            p2p.read_device_descriptor(0x40, Duration::from_secs(2))
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), peer_traffic.recv())
                .await
                .is_err()
        );

        let reading_mask = tokio::spawn(async move {
            let version = p2p.read_mask_version(Duration::from_secs(2)).await?;
            Ok::<_, KnxError>((p2p, version))
        });
        let descriptor_read = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(descriptor_read.payload, [0x03, 0x00]);
        let mut descriptor_ack = Telegram::individual_data(client_address, Vec::new());
        descriptor_ack.source = device_address;
        descriptor_ack.telegram_type = TelegramType::IndividualControl;
        descriptor_ack.tpci = TpciFrame::Ack { sequence: 2 }.encode();
        server.send(descriptor_ack).await.unwrap();
        let mut descriptor_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::DeviceDescriptorResponse {
                descriptor: 0,
                data: vec![0x06, 0x10],
            }
            .encode(),
        );
        descriptor_response.source = device_address;
        descriptor_response.tpci = TpciFrame::DataConnected { sequence: 1 }.encode();
        server.send(descriptor_response).await.unwrap();
        let (mut p2p, mask_version) = reading_mask.await.unwrap().unwrap();
        assert_eq!(mask_version, 0x0610);
        let descriptor_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            descriptor_response_ack.tpci,
            TpciFrame::Ack { sequence: 1 }.encode()
        );

        let authorizing = tokio::spawn(async move {
            let response = p2p.authorize(0x1234_5678, Duration::from_secs(2)).await?;
            Ok::<_, KnxError>((p2p, response))
        });
        let authorize_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::AuthorizeRequest::decode(&authorize_request.payload)
                .unwrap()
                .key,
            0x1234_5678
        );
        let mut authorize_ack = Telegram::individual_data(client_address, Vec::new());
        authorize_ack.source = device_address;
        authorize_ack.telegram_type = TelegramType::IndividualControl;
        authorize_ack.tpci = TpciFrame::Ack { sequence: 3 }.encode();
        server.send(authorize_ack).await.unwrap();
        let mut authorize_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::AuthorizeResponse { level: 2 }.encode(),
        );
        authorize_response.source = device_address;
        authorize_response.tpci = TpciFrame::DataConnected { sequence: 2 }.encode();
        server.send(authorize_response).await.unwrap();
        let (mut p2p, authorize_response) = authorizing.await.unwrap().unwrap();
        assert_eq!(authorize_response.level, 2);
        let authorize_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            authorize_response_ack.tpci,
            TpciFrame::Ack { sequence: 2 }.encode()
        );

        let authorizing_best = tokio::spawn(async move {
            let level = p2p
                .authorize_best(0x8765_4321, Duration::from_secs(2))
                .await?;
            Ok::<_, KnxError>((p2p, level))
        });
        let free_key_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::AuthorizeRequest::decode(&free_key_request.payload)
                .unwrap()
                .key,
            crate::protocol::management::FREE_ACCESS_KEY
        );
        let mut free_key_ack = Telegram::individual_data(client_address, Vec::new());
        free_key_ack.source = device_address;
        free_key_ack.telegram_type = TelegramType::IndividualControl;
        free_key_ack.tpci = TpciFrame::Ack { sequence: 4 }.encode();
        server.send(free_key_ack).await.unwrap();
        let mut free_key_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::AuthorizeResponse { level: 3 }.encode(),
        );
        free_key_response.source = device_address;
        free_key_response.tpci = TpciFrame::DataConnected { sequence: 3 }.encode();
        server.send(free_key_response).await.unwrap();
        let free_key_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            free_key_response_ack.tpci,
            TpciFrame::Ack { sequence: 3 }.encode()
        );

        let client_key_request = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::protocol::management::AuthorizeRequest::decode(&client_key_request.payload)
                .unwrap()
                .key,
            0x8765_4321
        );
        let mut client_key_ack = Telegram::individual_data(client_address, Vec::new());
        client_key_ack.source = device_address;
        client_key_ack.telegram_type = TelegramType::IndividualControl;
        client_key_ack.tpci = TpciFrame::Ack { sequence: 5 }.encode();
        server.send(client_key_ack).await.unwrap();
        let mut client_key_response = Telegram::individual_data(
            client_address,
            crate::protocol::management::AuthorizeResponse { level: 1 }.encode(),
        );
        client_key_response.source = device_address;
        client_key_response.tpci = TpciFrame::DataConnected { sequence: 4 }.encode();
        server.send(client_key_response).await.unwrap();
        let (mut p2p, best_level) = authorizing_best.await.unwrap().unwrap();
        assert_eq!(best_level, 1);
        let client_key_response_ack =
            tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            client_key_response_ack.tpci,
            TpciFrame::Ack { sequence: 4 }.encode()
        );

        p2p.close().await.unwrap();
        let disconnect = tokio::time::timeout(Duration::from_secs(2), peer_traffic.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(disconnect.tpci, TpciFrame::Disconnect.encode());
        knx.shutdown().await.unwrap();
        server.shutdown();
    }

    #[test]
    fn individual_apdus_preserve_connectionless_and_connected_tpci() {
        use crate::protocol::knxip::{KnxIpFrame, TunnellingRequest};
        let destination = crate::protocol::IndividualAddress::new(1, 2, 3);
        let source = crate::protocol::IndividualAddress::new(1, 2, 4);
        let apdu = vec![0x02, 0xC8, 1, 2, 0x42];
        let telegram = Telegram::individual_data(destination, apdu.clone());
        let connection: Arc<dyn Connection> =
            Arc::new(Tunnel::new_udp("127.0.0.1:3671".parse().unwrap()));
        let wire = Knx::build_outgoing_frame_for_connection(&telegram, &connection).unwrap();
        let frame = KnxIpFrame::parse(&wire).unwrap();
        let request = TunnellingRequest::parse(&frame.body).unwrap();
        let cemi = CemiFrame::parse(&request.raw_cemi).unwrap();
        assert_eq!(cemi.dest_addr, Address::Individual(destination));
        assert_eq!(cemi.tpci, 0);
        assert_eq!(cemi.apci_data, apdu);
        let received = Knx::parse_cemi_to_telegram(&request.raw_cemi).unwrap();
        assert_eq!(received.telegram_type, TelegramType::IndividualData);
        assert_eq!(received.tpci, 0);
        assert_eq!(received.payload, apdu);

        let mut connected = cemi;
        connected.tpci = 0x44;
        let received = Telegram::from_cemi_frame(connected).unwrap();
        assert_eq!(received.tpci, 0x44);
        assert_eq!(received.payload, apdu);
        let wire = Knx::build_outgoing_frame_for_connection(&received, &connection).unwrap();
        let frame = KnxIpFrame::parse(&wire).unwrap();
        let request = TunnellingRequest::parse(&frame.body).unwrap();
        let cemi = CemiFrame::parse(&request.raw_cemi).unwrap();
        assert_eq!(cemi.tpci, 0x44);
        assert_eq!(cemi.apci_data, apdu);

        for tpci in [0x80, 0x81, 0xC2, 0xC3] {
            let mut control = CemiFrame::new(
                crate::protocol::cemi::MessageCode::LDataInd,
                source,
                Address::Individual(destination),
                Vec::new(),
            );
            control.tpci = tpci;
            let received = Telegram::from_cemi_frame(control).unwrap();
            assert_eq!(received.telegram_type, TelegramType::IndividualControl);
            assert_eq!(received.tpci, tpci);
            let wire = Knx::build_outgoing_frame_for_connection(&received, &connection).unwrap();
            let frame = KnxIpFrame::parse(&wire).unwrap();
            let request = TunnellingRequest::parse(&frame.body).unwrap();
            let cemi = CemiFrame::parse(&request.raw_cemi).unwrap();
            assert_eq!(cemi.tpci, tpci);
            assert!(cemi.apci_data.is_empty());
        }
    }

    #[cfg(feature = "dpt")]
    #[test]
    fn octet_telegrams_preserve_full_octets_and_service_on_wire() {
        use crate::protocol::dpt::{DPTSwitch, DptValue, Scaling};
        use crate::protocol::knxip::{KnxIpFrame, TunnellingRequest};
        let address = GroupAddress::try_from_raw(0x1234).unwrap();
        let connection: Arc<dyn Connection> =
            Arc::new(Tunnel::new_udp("127.0.0.1:3671".parse().unwrap()));
        for byte in 0..=u8::MAX {
            let value = Scaling::from_bytes(&[byte]).unwrap();
            for (telegram, service_bits) in [
                (
                    Telegram::group_write_value(address, value.clone()).unwrap(),
                    0x80,
                ),
                (
                    Telegram::group_response_value(address, value).unwrap(),
                    0x40,
                ),
            ] {
                let wire =
                    Knx::build_outgoing_frame_for_connection(&telegram, &connection).unwrap();
                let frame = KnxIpFrame::parse(&wire).unwrap();
                let request = TunnellingRequest::parse(&frame.body).unwrap();
                let cemi = CemiFrame::parse(&request.raw_cemi).unwrap();
                assert_eq!(cemi.message_code, MessageCode::LDataReq);
                assert_eq!(cemi.apci_data, [service_bits, byte]);
                let received = Knx::parse_cemi_to_telegram(&request.raw_cemi).unwrap();
                assert_eq!(received.payload, [byte]);
                assert!(!received.compact_payload);
                assert_eq!(received.telegram_type, telegram.telegram_type);
            }
        }
        let switch = Telegram::group_write_value::<DPTSwitch>(address, true).unwrap();
        assert!(switch.compact_payload);
        assert_eq!(switch.encode_apdu(), [0x81]);
        assert_eq!(Telegram::group_read(address).encode_apdu(), [0]);
    }

    #[cfg(feature = "secure")]
    #[tokio::test]
    async fn data_secure_group_telegrams_are_wrapped_and_replay_checked() {
        use crate::protocol::knxip::{KnxIpFrame, TunnellingRequest};
        use crate::security::{KeyRing, SecurityKey};
        use std::sync::atomic::AtomicU64;

        let destination = GroupAddress::try_from_raw(0x0400).unwrap();
        let source = crate::protocol::IndividualAddress::new(5, 0, 1);
        let connection: Arc<dyn Connection> =
            Arc::new(Tunnel::new_udp("127.0.0.1:3671".parse().unwrap()));
        let mut keyring = KeyRing::new();
        keyring.add_group_key(
            destination,
            SecurityKey::from_hex("dfdf23a59fbb40404091d1c162087e8b").unwrap(),
        );
        keyring.set_sequence_number(source, 160170101606);
        let keyring = RwLock::new(Some(keyring));
        let sequence = AtomicU64::new(160170101607);
        let mut telegram = Telegram::group_read(destination);
        telegram.source = source;

        let wire =
            Knx::build_outgoing_frame_with_security(&telegram, &connection, &keyring, &sequence)
                .await
                .unwrap();
        let knxip = KnxIpFrame::parse(&wire).unwrap();
        let request = TunnellingRequest::parse(&knxip.body).unwrap();
        let secure = CemiFrame::parse(&request.raw_cemi).unwrap();
        assert_eq!(secure.tpci, 0x03);
        assert_eq!(sequence.load(Ordering::SeqCst), 160170101608);
        assert_eq!(
            secure.apci_data,
            [
                0xF1, 0x10, 0x00, 0x25, 0x4A, 0xE1, 0xCB, 0x67, 0xCD, 0x18, 0x4A, 0xFE, 0x57, 0x44
            ]
        );

        let mut unknown_sender_keys = KeyRing::new();
        unknown_sender_keys.add_group_key(
            destination,
            SecurityKey::from_hex("dfdf23a59fbb40404091d1c162087e8b").unwrap(),
        );
        assert!(
            Knx::parse_cemi_to_telegram_with_security(
                &request.raw_cemi,
                &RwLock::new(Some(unknown_sender_keys)),
            )
            .await
            .is_err()
        );

        let telegram = Knx::parse_cemi_to_telegram_with_security(&request.raw_cemi, &keyring)
            .await
            .unwrap();
        assert_eq!(telegram.source, source);
        assert_eq!(telegram.destination, Address::Group(destination));
        assert_eq!(telegram.telegram_type, TelegramType::GroupValueRead);

        assert!(
            Knx::parse_cemi_to_telegram_with_security(&request.raw_cemi, &keyring)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_knx_telegram_callback_integration() {
        // Create Knx instance with telegram callback
        let callback_counter = Arc::new(AtomicUsize::new(0));
        let received_telegrams = Arc::new(tokio::sync::RwLock::new(Vec::new()));

        let counter_clone = callback_counter.clone();
        let telegrams_clone = received_telegrams.clone();

        // Create Knx instance first, then register callback at runtime
        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .memory_limit_mb(32)
            .build()
            .await
            .unwrap();

        // Register telegram callback using sync method
        let _handle = knx
            .event_handler
            .register_telegram_callback_sync(move |telegram: &Telegram| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                let telegrams_clone = telegrams_clone.clone();
                let owned_telegram = telegram.clone();
                tokio::spawn(async move {
                    telegrams_clone.write().await.push(owned_telegram);
                });
            })
            .await;

        // Verify callback was registered
        assert_eq!(knx.telegram_callback_count().await, 1);

        // Create test telegram
        let telegram = Telegram {
            source: IndividualAddress::new(1, 1, 1),
            destination: Address::Group(GroupAddress::new(
                MainGroup::new(0),
                MiddleGroup::new(1),
                1,
            )),
            payload: vec![0x01, 0x02],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            compact_payload: true,
            tpci: 0,
            timestamp: std::time::SystemTime::now(),
        };

        // Trigger telegram notification
        knx.test_notify_telegram_received(&telegram).await;

        // Give async tasks time to complete
        sleep(Duration::from_millis(50)).await;

        // Verify callback was invoked
        assert_eq!(callback_counter.load(Ordering::SeqCst), 1);

        let telegrams = received_telegrams.read().await;
        assert_eq!(telegrams.len(), 1);
        assert_eq!(telegrams[0].source, telegram.source);
        assert_eq!(telegrams[0].destination, telegram.destination);
        assert_eq!(telegrams[0].payload, telegram.payload);
    }

    /// Incoming telegrams for group addresses nobody registered a DPT for
    /// must still flow through normally — the registry is opt-in and must
    /// never gate delivery — and every decode entry point must fail soft
    /// (`None`/`Err`, never a panic) rather than assume the sender agrees
    /// with the registry.
    #[cfg(feature = "dpt")]
    #[tokio::test]
    async fn test_incoming_telegram_for_unregistered_address_is_safe() {
        use crate::protocol::dpt::DPTSwitch;

        let callback_counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = callback_counter.clone();

        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .memory_limit_mb(32)
            .build()
            .await
            .unwrap();

        // Only address 0/1/1 gets a DPT binding; 0/2/2 never does.
        let registered = GroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1);
        let unregistered = GroupAddress::new(MainGroup::new(0), MiddleGroup::new(2), 2);
        let switch = knx.group_address::<DPTSwitch>(registered).unwrap();

        let _handle = knx
            .event_handler
            .register_telegram_callback_sync(move |_telegram: &Telegram| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        let telegram_for_unregistered = Telegram {
            source: IndividualAddress::new(1, 1, 1),
            destination: Address::Group(unregistered),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            compact_payload: true,
            tpci: 0,
            timestamp: std::time::SystemTime::now(),
        };

        // Delivery to callbacks doesn't consult the registry at all, so an
        // unregistered destination changes nothing about receiving it.
        knx.test_notify_telegram_received(&telegram_for_unregistered)
            .await;
        sleep(Duration::from_millis(50)).await;
        assert_eq!(callback_counter.load(Ordering::SeqCst), 1);

        // The generic decode path fails soft instead of panicking.
        assert!(knx.group_address_dpt(unregistered).is_none());
        assert!(knx.decode_group_value(&telegram_for_unregistered).is_none());

        // A handle for a *different, registered* address also just returns
        // `None` for a telegram it doesn't recognize — it doesn't panic
        // trying to decode bytes that were never meant for its DPT.
        assert!(switch.decode(&telegram_for_unregistered).is_none());

        // Meanwhile the registered address keeps working normally.
        let telegram_for_registered = Telegram {
            source: IndividualAddress::new(1, 1, 1),
            destination: Address::Group(registered),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            compact_payload: true,
            tpci: 0,
            timestamp: std::time::SystemTime::now(),
        };
        assert!(
            switch
                .decode(&telegram_for_registered)
                .unwrap()
                .unwrap()
                .value()
        );
        assert!(
            knx.decode_group_value(&telegram_for_registered)
                .unwrap()
                .unwrap()
                .as_bool()
                .unwrap()
        );
    }

    /// A real `GroupValueWrite`/`GroupValueResponse` for a registered
    /// address updates its cached state and clears its staleness; a
    /// `GroupValueRead` (no value) does neither.
    #[cfg(feature = "dpt")]
    #[tokio::test]
    async fn incoming_group_value_clears_staleness_and_caches_value() {
        use crate::protocol::dpt::DPTSwitch;

        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .memory_limit_mb(32)
            .build()
            .await
            .unwrap();

        let address = GroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1);
        knx.group_address::<DPTSwitch>(address).unwrap();
        knx.set_group_address_refresh(address, Duration::from_millis(10))
            .unwrap();

        // Never seen yet — stale as soon as the TTL is set.
        assert!(
            knx.group_registry
                .next_stale(std::time::Instant::now())
                .is_some()
        );
        assert!(
            knx.group_address_state(address)
                .unwrap()
                .last_seen
                .is_none()
        );

        // A GroupValueRead carries no value and must not clear staleness.
        let read_telegram = Telegram::received(
            knx.config.individual_address,
            address,
            TelegramType::GroupValueRead,
            Vec::new(),
        );
        Knx::record_group_value_if_present(&knx.group_registry, &read_telegram);
        assert!(
            knx.group_address_state(address)
                .unwrap()
                .last_seen
                .is_none()
        );

        // A real write does clear it and caches the payload.
        let write_telegram = Telegram::received(
            knx.config.individual_address,
            address,
            TelegramType::GroupValueWrite,
            vec![0x01],
        );
        Knx::record_group_value_if_present(&knx.group_registry, &write_telegram);

        let state = knx.group_address_state(address).unwrap();
        assert_eq!(state.last_seen_value, Some(vec![0x01]));
        assert!(state.last_seen.is_some());
        assert!(
            knx.group_registry
                .next_stale(std::time::Instant::now())
                .is_none()
        );

        // Once the TTL elapses again, it's back to stale.
        sleep(Duration::from_millis(30)).await;
        assert!(
            knx.group_registry
                .next_stale(std::time::Instant::now())
                .is_some()
        );
    }

    /// A responder callback (the `read_responder.rs` example's pattern):
    /// answer a `GroupValueRead` for a known address with a
    /// `GroupValueResponse`, but only once it actually has a value — never
    /// a stale default — and never react to a `GroupValueWrite` or its own
    /// outgoing response.
    #[cfg(feature = "dpt")]
    #[tokio::test]
    async fn read_responder_answers_reads_only_once_it_has_a_value() {
        use crate::protocol::dpt::DPTSwitch;

        struct Responder {
            address: GroupAddress,
            knx: Knx,
            value: bool,
        }

        #[async_trait::async_trait]
        impl TelegramCallbackFn for Responder {
            async fn call(&self, telegram: &Telegram) {
                if telegram.telegram_type != TelegramType::GroupValueRead {
                    return;
                }
                let response =
                    Telegram::group_response_value::<DPTSwitch>(self.address, self.value).unwrap();
                self.knx.send_telegram(&response).await.unwrap();
            }
        }

        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .memory_limit_mb(32)
            .build()
            .await
            .unwrap();

        let address = GroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1);
        knx.register_telegram_callback_filtered(
            Responder {
                address,
                knx: knx.clone(),
                value: true,
            },
            TelegramFilter::GroupAddresses(vec![address]),
            false,
        )
        .await;

        // A write must not trigger a response.
        let write = Telegram::received(
            knx.config.individual_address,
            address,
            TelegramType::GroupValueWrite,
            vec![0x01],
        );
        knx.test_notify_telegram_received(&write).await;
        assert_eq!(knx.telegram_queue_size().await, 0);

        // A read does — exactly one response, carrying the DPT-encoded value.
        let read = Telegram::received(
            knx.config.individual_address,
            address,
            TelegramType::GroupValueRead,
            Vec::new(),
        );
        knx.test_notify_telegram_received(&read).await;
        assert_eq!(knx.telegram_queue_size().await, 1);
    }

    /// A `GroupValueRead` that arrived too long before the responder
    /// actually processes it (queue backlog, a slow callback ahead of it,
    /// ...) must not be answered — `Telegram::timestamp` marks when it
    /// arrived, not when the callback happens to run, and a response to a
    /// requester that's long since given up (or that overwrites a fresher
    /// answer from another device on the same address) is worse than no
    /// response at all.
    #[cfg(feature = "dpt")]
    #[tokio::test]
    async fn read_responder_ignores_reads_that_arrived_too_late() {
        use crate::protocol::dpt::DPTSwitch;

        const MAX_RESPONSE_DELAY: Duration = Duration::from_millis(500);

        struct Responder {
            address: GroupAddress,
            knx: Knx,
        }

        #[async_trait::async_trait]
        impl TelegramCallbackFn for Responder {
            async fn call(&self, telegram: &Telegram) {
                if telegram.telegram_type != TelegramType::GroupValueRead {
                    return;
                }
                if telegram.is_older_than(MAX_RESPONSE_DELAY) {
                    return;
                }
                let response =
                    Telegram::group_response_value::<DPTSwitch>(self.address, true).unwrap();
                self.knx.send_telegram(&response).await.unwrap();
            }
        }

        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .memory_limit_mb(32)
            .build()
            .await
            .unwrap();

        let address = GroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1);
        knx.register_telegram_callback_filtered(
            Responder {
                address,
                knx: knx.clone(),
            },
            TelegramFilter::GroupAddresses(vec![address]),
            false,
        )
        .await;

        // A stale read (arrived well past the delay budget) is ignored.
        let mut stale_read = Telegram::received(
            knx.config.individual_address,
            address,
            TelegramType::GroupValueRead,
            Vec::new(),
        );
        stale_read.timestamp -= MAX_RESPONSE_DELAY * 2;
        knx.test_notify_telegram_received(&stale_read).await;
        assert_eq!(knx.telegram_queue_size().await, 0);

        // A fresh read for the same address still gets answered.
        let fresh_read = Telegram::received(
            knx.config.individual_address,
            address,
            TelegramType::GroupValueRead,
            Vec::new(),
        );
        knx.test_notify_telegram_received(&fresh_read).await;
        assert_eq!(knx.telegram_queue_size().await, 1);
    }

    #[tokio::test]
    async fn test_knx_connection_callback_integration() {
        // Create Knx instance with connection callback
        let callback_counter = Arc::new(AtomicUsize::new(0));
        let received_states = Arc::new(tokio::sync::RwLock::new(Vec::new()));

        let counter_clone = callback_counter.clone();
        let states_clone = received_states.clone();

        // Create Knx instance first, then register callback at runtime
        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .memory_limit_mb(32)
            .build()
            .await
            .unwrap();

        // Register connection callback using sync method
        let _handle = knx
            .event_handler
            .register_connection_callback_sync(move |state: ConnectionState| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                let states_clone = states_clone.clone();
                tokio::spawn(async move {
                    states_clone.write().await.push(state);
                });
            })
            .await;

        // Verify callback was registered
        assert_eq!(knx.connection_callback_count().await, 1);

        // Trigger connection state notifications
        knx.test_notify_connection_state_changed(ConnectionState::Connecting)
            .await;
        knx.test_notify_connection_state_changed(ConnectionState::Connected)
            .await;
        knx.test_notify_connection_state_changed(ConnectionState::Disconnected)
            .await;

        // Give async tasks time to complete
        sleep(Duration::from_millis(50)).await;

        // Verify callbacks were invoked
        assert_eq!(callback_counter.load(Ordering::SeqCst), 3);

        let states = received_states.read().await;
        assert_eq!(states.len(), 3);
        assert_eq!(states[0], ConnectionState::Connecting);
        assert_eq!(states[1], ConnectionState::Connected);
        assert_eq!(states[2], ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_knx_mixed_callbacks_integration() {
        // Create Knx instance with multiple callback types
        let telegram_counter = Arc::new(AtomicUsize::new(0));
        let connection_counter = Arc::new(AtomicUsize::new(0));

        let telegram_counter_clone = telegram_counter.clone();
        let connection_counter_clone = connection_counter.clone();

        // Create Knx instance first, then register callbacks at runtime
        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .memory_limit_mb(32)
            .build()
            .await
            .unwrap();

        // Register callbacks using sync methods
        let _telegram_handle = knx
            .event_handler
            .register_telegram_callback_sync(move |_telegram: &Telegram| {
                telegram_counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        let _connection_handle = knx
            .event_handler
            .register_connection_callback_sync(move |_state: ConnectionState| {
                connection_counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Verify all callbacks were registered
        assert_eq!(knx.total_callback_count().await, 2);
        assert_eq!(knx.telegram_callback_count().await, 1);
        assert_eq!(knx.connection_callback_count().await, 1);

        // Create test data
        let telegram = Telegram {
            source: IndividualAddress::new(1, 1, 1),
            destination: Address::Group(GroupAddress::new(
                MainGroup::new(0),
                MiddleGroup::new(1),
                1,
            )),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            compact_payload: true,
            tpci: 0,
            timestamp: std::time::SystemTime::now(),
        };

        // Trigger all notifications
        knx.test_notify_telegram_received(&telegram).await;
        knx.test_notify_connection_state_changed(ConnectionState::Connected)
            .await;

        // Verify all callbacks were invoked
        assert_eq!(telegram_counter.load(Ordering::SeqCst), 1);
        assert_eq!(connection_counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_knx_telegram_filtering_integration() {
        // Create Knx instance with filtered telegram callback
        let callback_counter = Arc::new(AtomicUsize::new(0));
        let target_address = GroupAddress::new(MainGroup::new(0), MiddleGroup::new(1), 1);

        let counter_clone = callback_counter.clone();

        // Create Knx instance first, then register callback at runtime
        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .memory_limit_mb(32)
            .build()
            .await
            .unwrap();

        // Register filtered telegram callback using sync method
        let _handle = knx
            .event_handler
            .register_telegram_callback_sync_filtered(
                move |_telegram: &Telegram| {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                },
                TelegramFilter::GroupAddresses(vec![target_address]),
                false,
            )
            .await;

        // Create telegrams - one matching, one not matching
        let matching_telegram = Telegram {
            source: IndividualAddress::new(1, 1, 1),
            destination: Address::Group(target_address),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            compact_payload: true,
            tpci: 0,
            timestamp: std::time::SystemTime::now(),
        };

        let non_matching_telegram = Telegram {
            source: IndividualAddress::new(1, 1, 1),
            destination: Address::Group(GroupAddress::new(
                MainGroup::new(0),
                MiddleGroup::new(2),
                2,
            )),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            compact_payload: true,
            tpci: 0,
            timestamp: std::time::SystemTime::now(),
        };

        // Trigger notifications
        knx.test_notify_telegram_received(&matching_telegram).await;
        knx.test_notify_telegram_received(&non_matching_telegram)
            .await;

        // Only the matching telegram should trigger the callback
        assert_eq!(callback_counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_knx_callback_unregistration_integration() {
        // Create Knx instance
        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .memory_limit_mb(32)
            .build()
            .await
            .unwrap();

        // Register callbacks at runtime
        let callback_counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = callback_counter.clone();

        let handle = knx
            .event_handler
            .register_telegram_callback_sync(move |_telegram: &Telegram| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        assert_eq!(knx.telegram_callback_count().await, 1);

        // Create test telegram
        let telegram = Telegram {
            source: IndividualAddress::new(1, 1, 1),
            destination: Address::Group(GroupAddress::new(
                MainGroup::new(0),
                MiddleGroup::new(1),
                1,
            )),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            compact_payload: true,
            tpci: 0,
            timestamp: std::time::SystemTime::now(),
        };

        // Trigger notification - should invoke callback
        knx.test_notify_telegram_received(&telegram).await;
        assert_eq!(callback_counter.load(Ordering::SeqCst), 1);

        // Unregister callback
        assert!(knx.event_handler.unregister_callback(handle).await);
        assert_eq!(knx.telegram_callback_count().await, 0);

        // Trigger notification again - should not invoke callback
        knx.test_notify_telegram_received(&telegram).await;
        assert_eq!(callback_counter.load(Ordering::SeqCst), 1); // Still 1, not incremented
    }

    #[tokio::test]
    async fn test_knx_callback_error_handling_integration() {
        // Create Knx instance with callbacks that might have issues
        let successful_counter = Arc::new(AtomicUsize::new(0));
        let slow_counter = Arc::new(AtomicUsize::new(0));

        let successful_counter_clone = successful_counter.clone();
        let slow_counter_clone = slow_counter.clone();

        // Create Knx instance first, then register callbacks at runtime
        let knx = Knx::builder()
            .connection_type(ConnectionType::Routing)
            .memory_limit_mb(32)
            .build()
            .await
            .unwrap();

        // Register successful callback using sync method
        let _successful_handle = knx
            .event_handler
            .register_telegram_callback_sync(move |_telegram: &Telegram| {
                successful_counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Register a slow async callback at runtime
        let _slow_handle = knx
            .event_handler
            .register_telegram_callback(SlowCallback {
                counter: slow_counter_clone,
            })
            .await;

        assert_eq!(knx.telegram_callback_count().await, 2);

        // Create test telegram
        let telegram = Telegram {
            source: IndividualAddress::new(1, 1, 1),
            destination: Address::Group(GroupAddress::new(
                MainGroup::new(0),
                MiddleGroup::new(1),
                1,
            )),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            compact_payload: true,
            tpci: 0,
            timestamp: std::time::SystemTime::now(),
        };

        // Trigger notification
        knx.test_notify_telegram_received(&telegram).await;

        // The fast callback should complete successfully
        assert_eq!(successful_counter.load(Ordering::SeqCst), 1);

        // The slow callback should be attempted (it increments before sleeping)
        assert_eq!(slow_counter.load(Ordering::SeqCst), 1);
    }
}
