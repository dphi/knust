//! Main Knx library interface.

use crate::application::callbacks::{
    CallbackHandle, ConnectionCallbackFn, EventHandler, TelegramCallbackFn, TelegramFilter,
};
use crate::error::{ConfigurationError, DeviceError, KnxError, Result};
use crate::log_application;
use crate::logging::{Component, LogLevel, Timer};
use crate::memory::{MemoryMonitor, PerformanceOptimizer};
use crate::protocol::Telegram;
#[cfg(feature = "dpt")]
use crate::protocol::TelegramPriority;
#[cfg(feature = "dpt")]
use crate::protocol::TelegramType;
#[cfg(feature = "dpt")]
use crate::protocol::dpt::{
    DptType, DptValue, DptView, GroupAddressRegistry, GroupAddressState, WriteValue,
};
use crate::protocol::{Address, GroupAddress};
use crate::transport::{
    Connection, ConnectionConfig, ConnectionType, ReceiveLimitConfig, ReceiveRateLimiter,
    ReceiveStats, RoutingConnection, Tunnel, queue::TelegramQueue,
};
use std::sync::Arc;
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

/// Builder for creating Knx instances with custom configuration
pub struct KnxBuilder {
    config: ConnectionConfig,
    memory_limit_mb: u64,
    max_connections: usize,
    stored_callbacks: Vec<StoredCallback>,
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

        let knx = Self {
            config,
            connection: tokio::sync::watch::channel(None).0,
            state: Arc::new(RwLock::new(KnxState::Disconnected)),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(Notify::new()),
            shutdown_started: Arc::new(Notify::new()),
            receiving_stop: Arc::new(AtomicBool::new(false)),
            lifecycle_lock: Arc::new(tokio::sync::Mutex::new(())),
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

                let mut tunneling_conn = Tunnel::new_udp(gateway_addr);

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
        let memory_monitor = self.memory_monitor.clone();
        let performance_optimizer = self.performance_optimizer.clone();
        let handle = tokio::spawn(async move {
            while !shutdown_flag.load(Ordering::SeqCst) {
                let connection = active.borrow_and_update().clone();
                let Some(connection) = connection else {
                    tokio::select! {
                        _ = active.changed() => {}
                        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                    }
                    continue;
                };
                tokio::select! {
                    _ = active.changed() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                    received = connection.recv() => {
                        let is_current = active.borrow().as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &connection));
                        if !is_current { continue; }
                        let channel_id = connection.as_any().downcast_ref::<Tunnel>().map(Tunnel::channel_id);
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
                    let frame_data =
                        Self::build_outgoing_frame_for_connection(&telegram, &connection);

                    let mut result =
                        Self::send_telegram_frame(&connection, &frame_data, 2, &mut receipt).await;
                    if let Err(error) = &result {
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
                            let frame =
                                Self::build_outgoing_frame_for_connection(&telegram, &replacement);
                            result =
                                Self::send_telegram_frame(&replacement, &frame, 1, &mut receipt)
                                    .await;
                            if let Err(error) = &result {
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

    /// Handle incoming KNX/IP frame based on service type (following Python pattern)
    async fn handle_incoming_frame(
        frame_data: &[u8],
        telegram_queue: &crate::transport::queue::TelegramQueue,
        control_tx: &mpsc::Sender<ConnectionControlMessage>,
        current_channel_id: Option<u8>,
        connection: &Arc<dyn Connection>,
        receive_limiter: Option<&ReceiveRateLimiter>,
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
                )
                .await?;
                Ok(true)
            }
            ServiceType::RoutingIndication => {
                Self::handle_routing_indication(&knx_frame, telegram_queue, receive_limiter)
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

                // Parse CEMI and enqueue telegram for processing
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
    ) -> Result<()> {
        // RoutingIndication format: [cemi_data...]
        let cemi_data = &knx_frame.body;
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

    /// Parse CEMI data to telegram (extracted from `parse_telegram`)
    fn parse_cemi_to_telegram(cemi_data: &[u8]) -> Result<Telegram> {
        use crate::protocol::cemi::CemiFrame;
        use crate::protocol::telegram::{Direction, Priority, TelegramType};

        // Parse CEMI frame
        let cemi_frame = CemiFrame::parse(cemi_data)?;

        let service =
            crate::protocol::GroupValueService::decode(cemi_frame.tpci, &cemi_frame.apci_data).ok();
        let payload = service
            .as_ref()
            .and_then(|service| service.payload().map(<[u8]>::to_vec))
            .unwrap_or_else(|| cemi_frame.apci_data.clone());
        let telegram_type = match &service {
            Some(crate::protocol::GroupValueService::Read) => TelegramType::GroupValueRead,
            Some(crate::protocol::GroupValueService::Response(_)) => {
                TelegramType::GroupValueResponse
            }
            Some(crate::protocol::GroupValueService::Write(_)) | None => {
                TelegramType::GroupValueWrite
            }
        };

        // Convert CEMI frame to Telegram
        let telegram = Telegram {
            source: cemi_frame.source_addr,
            destination: cemi_frame.dest_addr,
            payload,
            priority: match cemi_frame.control_field.priority {
                crate::protocol::cemi::Priority::System => Priority::System,
                crate::protocol::cemi::Priority::Normal => Priority::Normal,
                crate::protocol::cemi::Priority::Urgent => Priority::Urgent,
                crate::protocol::cemi::Priority::Low => Priority::Low,
            },
            direction: Direction::Incoming,
            telegram_type,
            compact_payload: cemi_frame.apci_data.len() == 1,
            timestamp: std::time::SystemTime::now(),
        };

        Ok(telegram)
    }

    fn build_outgoing_frame_for_connection(
        telegram: &Telegram,
        connection: &Arc<dyn Connection>,
    ) -> Vec<u8> {
        use crate::protocol::cemi::{CemiFrame, MessageCode};
        use crate::protocol::knxip::{KnxIpFrame, ServiceType, TunnellingRequest};
        use crate::transport::Tunnel;

        let apci_data = telegram.encode_group_service();
        let message_code = if connection.as_any().is::<Tunnel>() {
            MessageCode::LDataReq
        } else {
            MessageCode::LDataInd
        };
        let cemi = CemiFrame::new(
            message_code,
            telegram.source,
            telegram.destination,
            apci_data,
        );
        let raw_cemi = cemi.serialize();

        if let Some(tunneling_conn) = connection.as_any().downcast_ref::<Tunnel>() {
            let tunnelling_request = TunnellingRequest::new(
                tunneling_conn.channel_id(),
                tunneling_conn.next_sequence(),
                raw_cemi,
            );
            return KnxIpFrame::new(
                ServiceType::TunnellingRequest,
                tunnelling_request.serialize(),
            )
            .serialize();
        }

        KnxIpFrame::new(ServiceType::RoutingIndication, raw_cemi).serialize()
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

        let telegram = Knx::parse_cemi_to_telegram(&frame.serialize()).unwrap();

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
                let wire = Knx::build_outgoing_frame_for_connection(&telegram, &connection);
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
        assert_eq!(switch.encode_group_service(), [0x81]);
        assert_eq!(Telegram::group_read(address).encode_group_service(), [0]);
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
