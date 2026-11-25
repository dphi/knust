//! Main Knx library interface.

use crate::application::callbacks::{
    CallbackHandle, ConnectionCallbackFn, ConnectionState, EventHandler, TelegramCallbackFn,
    TelegramFilter,
};
#[cfg(feature = "secure")]
use crate::error::SecurityError;
use crate::error::{ConfigurationError, DeviceError, KnxError, Result};
use crate::log_application;
use crate::logging::{Component, LogLevel, Timer};
use crate::memory::{MemoryMonitor, PerformanceOptimizer};
#[cfg(feature = "dpt")]
use crate::protocol::Priority;
use crate::protocol::Telegram;
#[cfg(feature = "dpt")]
use crate::protocol::dpt::GroupAddressRegistry;
use crate::protocol::{Address, RawGroupAddress, TelegramType};
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

#[cfg(feature = "dpt")]
pub(super) mod group_address;

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
    state: Arc<RwLock<ConnectionState>>,

    /// Shutdown flag for graceful termination
    shutdown_flag: Arc<AtomicBool>,

    /// Notified when the library shuts down (for `run()` to wake from)
    shutdown_notify: Arc<Notify>,
    shutdown_started: Arc<Notify>,
    receiving_stop: Arc<AtomicBool>,
    lifecycle_lock: Arc<tokio::sync::Mutex<()>>,
    individual_request_lock: Arc<tokio::sync::Mutex<()>>,
    active_p2p_sessions: ActiveP2pSessions,
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
    address: RawGroupAddress,
    tx: mpsc::Sender<Vec<u8>>,
}

struct ConnectionlessResponseCallback {
    tx: tokio::sync::broadcast::Sender<Telegram>,
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
impl TelegramCallbackFn for ConnectionlessResponseCallback {
    async fn call(&self, telegram: &Telegram) {
        let _ = self.tx.send(telegram.clone());
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

const P2P_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum P2pMode {
    Connected,
    Connectionless,
}

type ActiveP2pSessions =
    Arc<std::sync::Mutex<std::collections::HashSet<(crate::protocol::IndividualAddress, P2pMode)>>>;

struct P2pCallbackRegistration {
    knx: Knx,
    handle: Option<CallbackHandle>,
    active_sessions: ActiveP2pSessions,
    key: (crate::protocol::IndividualAddress, P2pMode),
}

impl Drop for P2pCallbackRegistration {
    fn drop(&mut self) {
        self.active_sessions.lock().unwrap().remove(&self.key);
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

/// An address-scoped receive context for unnumbered individual telegrams.
///
/// Dropping the context releases its address reservation. Use [Self::close] to
/// also wait for callback removal.
pub struct P2pConnectionless {
    registration: P2pCallbackRegistration,
    receiver: tokio::sync::broadcast::Receiver<Telegram>,
}

impl P2pConnectionless {
    /// Send an unnumbered APDU after discarding previously buffered replies.
    pub async fn send(&mut self, apdu: Vec<u8>) -> Result<()> {
        if !(2..=u8::MAX as usize).contains(&apdu.len()) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("P2P APDU length must be in 2..=255, got {}", apdu.len()),
            }
            .into());
        }
        let telegram = Telegram::individual_data(self.registration.key.0, apdu);
        self.receiver = self.receiver.resubscribe();
        let knx = &self.registration.knx;
        tokio::select! {
            biased;
            _ = Self::wait_for_shutdown(knx) => Err(crate::error::TransportError::QueueClosed.into()),
            result = knx.send_telegram_acknowledged(&telegram) => result,
        }
    }

    /// Receive the next matching telegram, or `None` when the deadline passes.
    ///
    /// Reuse the same deadline to receive several replies within one window.
    /// Returns a queue error if more than 16 telegrams accumulate unread.
    pub async fn receive(
        &mut self,
        expected_service: Option<[u8; 2]>,
        deadline: tokio::time::Instant,
    ) -> Result<Option<Telegram>> {
        use tokio::sync::broadcast::error::RecvError;

        let knx = &self.registration.knx;
        let received = async {
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return Ok(None);
                }
                match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
                    Ok(Ok(telegram))
                        if expected_service
                            .is_none_or(|service| telegram.payload.starts_with(&service)) =>
                    {
                        return Ok(Some(telegram));
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(RecvError::Lagged(_))) => {
                        return Err(crate::error::TransportError::QueueFull.into());
                    }
                    Ok(Err(RecvError::Closed)) => {
                        return Err(crate::error::TransportError::QueueClosed.into());
                    }
                    Err(_) => return Ok(None),
                }
            }
        };
        tokio::select! {
            biased;
            _ = Self::wait_for_shutdown(knx) => Err(crate::error::TransportError::QueueClosed.into()),
            result = received => result,
        }
    }

    /// Send an APDU and return the first matching reply.
    pub async fn request(
        &mut self,
        apdu: Vec<u8>,
        expected_service: Option<[u8; 2]>,
        timeout: std::time::Duration,
    ) -> Result<Telegram> {
        self.send(apdu).await?;
        self.receive(expected_service, tokio::time::Instant::now() + timeout)
            .await?
            .ok_or_else(|| {
                DeviceError::CommunicationTimeout {
                    device: format!("individual address {}", self.registration.key.0),
                    timeout_ms: timeout.as_millis() as u64,
                }
                .into()
            })
    }

    /// Close the context and unregister its receive callback.
    pub async fn close(mut self) {
        if let Some(handle) = self.registration.handle {
            self.registration.knx.unregister_callback(handle).await;
            self.registration.handle = None;
        }
    }

    async fn wait_for_shutdown(knx: &Knx) {
        let stopping = knx.shutdown_started.notified();
        tokio::pin!(stopping);
        stopping.as_mut().enable();
        if !knx.shutdown_flag.load(Ordering::SeqCst) {
            stopping.await;
        }
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
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
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
    /// Paced at one per second and tagged [`Priority::Low`] so a
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
                    read_telegram.priority = Priority::Low;

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
                                    *s = ConnectionState::Disconnected;
                                }

                                // Notify callbacks
                                event_handler.notify_connection_state_changed(
                                    ConnectionState::Disconnected
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
        state: Arc<RwLock<ConnectionState>>,
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
                        *state.write().await = ConnectionState::Connected;
                        connection_arc.send_replace(Some(connection));
                        event_handler
                            .notify_connection_state_changed(ConnectionState::Connected)
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
        *self.state.write().await = ConnectionState::Connecting;
        self.notify_connection_state_changed(ConnectionState::Connecting)
            .await;
        match Self::create_connection(&self.config).await {
            Ok(connection) => {
                *self.channel_id.write().await =
                    Self::establish_connection(&connection, &self.config);
                *self.state.write().await = ConnectionState::Connected;
                self.connection.send_replace(Some(connection));
                self.notify_connection_state_changed(ConnectionState::Connected)
                    .await;
                Ok(())
            }
            Err(error) => {
                *self.state.write().await = ConnectionState::Error;
                self.notify_connection_state_changed(ConnectionState::Error)
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
            *state = ConnectionState::Disconnecting;
        }

        // Notify connection state change callbacks
        self.notify_connection_state_changed(ConnectionState::Disconnecting)
            .await;

        if let Some(connection) = self.connection.send_replace(None) {
            connection.close().await?;
        }

        {
            let mut state = self.state.write().await;
            *state = ConnectionState::Disconnected;
        }

        // Notify connection state change callbacks
        self.notify_connection_state_changed(ConnectionState::Disconnected)
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
        let _request_guard = self.individual_request_lock.lock().await;
        let mut session = self.connectionless_p2p(destination).await?;
        let result = session
            .request(request_apdu, Some(expected_service), timeout_duration)
            .await;
        session.close().await;
        result.map(|telegram| telegram.payload)
    }

    /// Check whether an individual address is occupied by a device.
    ///
    /// A descriptor response or an explicit P2P refusal indicates an occupied
    /// address. A timeout indicates no device answered the probe.
    pub async fn check_individual_address(
        &self,
        destination: crate::protocol::IndividualAddress,
        timeout: std::time::Duration,
    ) -> Result<bool> {
        let mut connection = self.connect_p2p(destination).await?;
        let response = connection.read_device_descriptor(0, timeout).await;
        let _ = connection.close().await;
        match response {
            Ok(_)
            | Err(crate::error::KnxError::Device(
                crate::error::DeviceError::CommunicationRefused { .. },
            )) => Ok(true),
            Err(crate::error::KnxError::Device(
                crate::error::DeviceError::CommunicationTimeout { .. },
            )) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Read addresses of devices currently in programming mode.
    ///
    /// Returns all responders before the timeout elapses. Set `raise_if_multiple`
    /// to return an error when a second device responds.
    pub async fn read_individual_addresses_in_programming_mode(
        &self,
        timeout_duration: std::time::Duration,
        raise_if_multiple: bool,
    ) -> Result<Vec<crate::protocol::IndividualAddress>> {
        let responses = self
            .collect_broadcast_responses(
                vec![0x01, 0x00],
                vec![0x01, 0x40],
                timeout_duration,
                raise_if_multiple.then_some(2),
            )
            .await?;
        let addresses = responses
            .iter()
            .map(|response| response.source)
            .collect::<Vec<_>>();
        if raise_if_multiple && addresses.len() > 1 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "More than one KNX device is in programming mode".to_owned(),
            }
            .into());
        }
        Ok(addresses)
    }

    /// Assign an address to the single device in programming mode, then restart it.
    ///
    /// Rejects an occupied address unless it belongs to the programming-mode device.
    /// A device that does not answer an address check is considered absent.
    pub async fn write_individual_address_in_programming_mode(
        &self,
        address: crate::protocol::IndividualAddress,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let occupied = self.check_individual_address(address, timeout).await?;
        let devices = self
            .read_individual_addresses_in_programming_mode(timeout, true)
            .await?;
        let current =
            devices
                .first()
                .copied()
                .ok_or_else(|| crate::error::ProtocolError::InvalidFrame {
                    details: "No KNX device is in programming mode".to_owned(),
                })?;
        if occupied && current != address {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("Individual address {address} is already occupied"),
            }
            .into());
        }
        if !occupied {
            let mut apdu = vec![0x00, 0xc0];
            apdu.extend_from_slice(&u16::from(address).to_be_bytes());
            let broadcast =
                RawGroupAddress::try_new(0, 0, 0).expect("0/0/0 is a valid group address");
            let mut telegram = Telegram::group_write(broadcast, apdu);
            telegram.telegram_type = TelegramType::IndividualData;
            telegram.compact_payload = false;
            self.send_telegram_acknowledged(&telegram).await?;
        }
        let mut connection = self.connect_p2p(address).await?;
        let result = async {
            connection.read_device_descriptor(0, timeout).await?;
            connection.restart().await
        }
        .await;
        let close_result = connection.close().await;
        result.and(close_result)
    }

    /// Read the address assigned to a device with the given serial number.
    pub async fn read_individual_address_by_serial_number(
        &self,
        serial: [u8; 6],
        timeout_duration: std::time::Duration,
    ) -> Result<Option<crate::protocol::IndividualAddress>> {
        let request = crate::protocol::IndividualAddressSerialRead { serial }.encode();
        let mut response_prefix = vec![0x03, 0xDD];
        response_prefix.extend_from_slice(&serial);
        let response = self
            .collect_broadcast_responses(request, response_prefix, timeout_duration, Some(1))
            .await?
            .into_iter()
            .next();
        let Some(response) = response else {
            return Ok(None);
        };
        let decoded = crate::protocol::IndividualAddressSerialResponse::decode(&response.payload)?;
        if decoded.serial != serial || decoded.address != response.source {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "IndividualAddressSerialResponse does not match its sender or request"
                    .to_owned(),
            }
            .into());
        }
        Ok(Some(response.source))
    }

    /// Assign an individual address to the device with the given serial number
    /// and verify the address by reading it back.
    pub async fn write_individual_address_by_serial_number(
        &self,
        serial: [u8; 6],
        address: crate::protocol::IndividualAddress,
        timeout_duration: std::time::Duration,
    ) -> Result<()> {
        let broadcast = RawGroupAddress::try_new(0, 0, 0).expect("0/0/0 is a valid group address");
        let mut telegram = Telegram::group_write(
            broadcast,
            crate::protocol::IndividualAddressSerialWrite { serial, address }.encode(),
        );
        telegram.telegram_type = TelegramType::IndividualData;
        telegram.compact_payload = false;
        self.send_telegram(&telegram).await?;

        let actual = self
            .read_individual_address_by_serial_number(serial, timeout_duration)
            .await?
            .ok_or_else(|| crate::error::DeviceError::CommunicationTimeout {
                device: format!("serial number {serial:02X?}"),
                timeout_ms: timeout_duration.as_millis() as u64,
            })?;
        if actual != address {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Individual address assignment verification failed: expected {address}, got {actual}"
                ),
            }
            .into());
        }
        Ok(())
    }

    async fn collect_broadcast_responses(
        &self,
        request_apdu: Vec<u8>,
        response_prefix: Vec<u8>,
        timeout_duration: std::time::Duration,
        max_responses: Option<usize>,
    ) -> Result<Vec<Telegram>> {
        let _request_guard = self.individual_request_lock.lock().await;
        let stopping = self.shutdown_started.notified();
        tokio::pin!(stopping);
        stopping.as_mut().enable();
        if self.shutdown_flag.load(Ordering::SeqCst) {
            return Err(crate::error::TransportError::QueueClosed.into());
        }

        let broadcast = RawGroupAddress::try_new(0, 0, 0).expect("0/0/0 is a valid group address");
        let (tx, mut rx) = mpsc::channel(16);
        let filter = TelegramFilter::Custom(Arc::new(move |telegram| {
            telegram.destination == Address::Group(broadcast)
                && telegram.payload.starts_with(&response_prefix)
        }));
        let handle = self
            .register_telegram_callback_filtered(P2pTelegramCallback { tx }, filter, false)
            .await;
        let mut callback_guard = ReadCallbackGuard {
            handler: self.event_handler.clone(),
            handle: Some(handle),
        };

        let operation = async {
            let mut telegram = Telegram::group_write(broadcast, request_apdu);
            telegram.telegram_type = TelegramType::IndividualData;
            telegram.compact_payload = false;
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

            let mut responses = Vec::new();
            let deadline = tokio::time::sleep(timeout_duration);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => return Ok(responses),
                    response = rx.recv() => match response {
                        Some(response) => {
                            responses.push(response);
                            if max_responses.is_some_and(|limit| responses.len() >= limit) {
                                return Ok(responses);
                            }
                        }
                        None => return Ok(responses),
                    }
                }
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
        let (tx, receiver) = mpsc::channel(16);
        let callback = self
            .register_p2p_peer(destination, P2pMode::Connected, P2pTelegramCallback { tx })
            .await?;
        let connection = P2pConnection {
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
        Ok(connection)
    }

    /// Open a receive context for connectionless data from one individual address.
    ///
    /// Only one context per peer can be open. A connected P2P link to the same
    /// device can be active at the same time.
    pub async fn connectionless_p2p(
        &self,
        destination: crate::protocol::IndividualAddress,
    ) -> Result<P2pConnectionless> {
        let (tx, receiver) = tokio::sync::broadcast::channel(16);
        let registration = self
            .register_p2p_peer(
                destination,
                P2pMode::Connectionless,
                ConnectionlessResponseCallback { tx },
            )
            .await?;
        Ok(P2pConnectionless {
            registration,
            receiver,
        })
    }

    async fn register_p2p_peer(
        &self,
        destination: crate::protocol::IndividualAddress,
        mode: P2pMode,
        callback: impl TelegramCallbackFn + Send + Sync + 'static,
    ) -> Result<P2pCallbackRegistration> {
        let key = (destination, mode);
        let active_sessions = self.active_p2p_sessions.clone();
        if !active_sessions.lock().unwrap().insert(key) {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!("A P2P session of this type to {destination} is already active"),
            }
            .into());
        }
        let mut registration = P2pCallbackRegistration {
            knx: self.clone(),
            handle: None,
            active_sessions,
            key,
        };
        let local_address = self.config.individual_address;
        let filter = TelegramFilter::Custom(Arc::new(move |telegram| {
            if telegram.source != destination
                || telegram.destination != Address::Individual(local_address)
            {
                return false;
            }
            match mode {
                P2pMode::Connected => {
                    telegram.telegram_type == TelegramType::IndividualControl
                        || (telegram.telegram_type == TelegramType::IndividualData
                            && telegram.tpci & 0xc0 == 0x40)
                }
                P2pMode::Connectionless => {
                    telegram.telegram_type == TelegramType::IndividualData
                        && telegram.tpci & 0xfc == 0
                }
            }
        }));
        registration.handle = Some(
            self.register_telegram_callback_filtered(callback, filter, false)
                .await,
        );
        Ok(registration)
    }

    /// Open a P2P connection and switch the device's programming mode.
    ///
    /// The mask version determines whether the device uses memory or a property.
    pub async fn switch_programming_mode(
        &self,
        destination: crate::protocol::IndividualAddress,
        enabled: bool,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let mut connection = self.connect_p2p(destination).await?;
        let result = connection
            .switch_programming_mode(enabled, verify, timeout)
            .await;
        let close_result = connection.close().await;
        result.and(close_result)
    }

    /// Restart a device using a temporary P2P connection.
    ///
    /// A basic restart is unconfirmed; success means the request was sent.
    pub async fn restart_device(
        &self,
        destination: crate::protocol::IndividualAddress,
    ) -> Result<()> {
        let mut connection = self.connect_p2p(destination).await?;
        let result = connection.restart().await;
        let close_result = connection.close().await;
        result.and(close_result)
    }

    /// Read a group value, starting the response deadline after transmission.
    ///
    /// # Errors
    ///
    /// Returns queue/transport errors, a send timeout if the read cannot be sent
    /// within ten seconds, or a response timeout after `timeout_duration`.
    pub async fn read_group_value(
        &self,
        address: RawGroupAddress,
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
    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    /// Check if connected to the KNX network
    pub async fn is_connected(&self) -> bool {
        matches!(self.state().await, ConnectionState::Connected)
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
    pub(crate) async fn notify_connection_state_changed(&self, state: ConnectionState) {
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
    pub async fn test_notify_connection_state_changed(&self, state: ConnectionState) {
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
        telegram.source = self.knx.config.individual_address;
        telegram.tpci = crate::protocol::TpciFrame::DataConnected { sequence }.encode();
        for attempt in 0..=P2P_MAX_RETRANSMISSIONS {
            let (sent, completed) = self
                .knx
                .telegram_queue
                .enqueue_outgoing_tracked(telegram.clone())
                .await?;
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                tokio::select! {
                    sent = sent => sent.map_err(|_| crate::error::TransportError::QueueClosed.into()),
                    result = completed => result.map_err(|_| crate::error::TransportError::QueueClosed)?,
                }
            }).await.map_err(|_| crate::error::TransportError::QueueTimeout { timeout_ms: 10_000 })??;
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
                            self.closed = true;
                            return Err(DeviceError::CommunicationRefused {
                                device: format!("individual address {}", self.destination),
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
                    return Err(DeviceError::CommunicationRefused {
                        device: format!("individual address {}", self.destination),
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

    /// Switch programming mode through Device Object property PID_PROGMODE (54).
    ///
    /// Use this property-based procedure for Realisation Type 1 devices.
    /// With `verify`, the write response must echo the requested value.
    pub async fn switch_programming_mode_property(
        &mut self,
        enabled: bool,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let value = u8::from(enabled);
        let response = self
            .write_property_value(
                crate::protocol::management::PropertyValueWrite {
                    object_index: 0,
                    property_id: 54,
                    count: 1,
                    start_index: 1,
                    data: vec![value],
                },
                timeout,
            )
            .await?;
        if response.count != 1 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Programming-mode property write returned count {}",
                    response.count
                ),
            }
            .into());
        }
        if verify && response.data != [value] {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Programming-mode write response did not echo {value}: {:?}",
                    response.data
                ),
            }
            .into());
        }
        Ok(())
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
    ///
    /// Memory writes have a transport acknowledgement but no application response.
    pub async fn write_memory(
        &mut self,
        request: crate::protocol::management::MemoryWrite,
    ) -> Result<()> {
        if request.count == 0
            || request.data.len() != usize::from(request.count)
            || request.data.len() > 0x1_0000 - usize::from(request.address)
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Memory write data, count, or address range is invalid".to_owned(),
            }
            .into());
        }
        self.send_data(request.encode()?, true).await
    }

    /// Switch programming mode through memory address 0x60 (Realisation Type 2).
    ///
    /// Preserves the other bits and toggles the parity bit when the mode changes.
    /// `verify` reads the byte back after the transport-acknowledged write.
    pub async fn switch_programming_mode_memory(
        &mut self,
        enabled: bool,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let response = self
            .read_memory(
                crate::protocol::management::MemoryRead {
                    address: 0x0060,
                    count: 1,
                },
                timeout,
            )
            .await?;
        if response.count != 1 || response.data.len() != 1 {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "Programming-mode memory read must return one byte".to_owned(),
            }
            .into());
        }
        let current = response.data[0];
        if (current & 1 != 0) == enabled {
            return Ok(());
        }
        let value = current ^ 0x81;
        self.write_memory(crate::protocol::management::MemoryWrite {
            address: 0x0060,
            count: 1,
            data: vec![value],
        })
        .await?;
        if verify {
            let response = self
                .read_memory(
                    crate::protocol::management::MemoryRead {
                        address: 0x0060,
                        count: 1,
                    },
                    timeout,
                )
                .await?;
            if response.count != 1 || response.data != [value] {
                return Err(crate::error::ProtocolError::InvalidFrame {
                    details: format!("Programming-mode read-back returned {:?}", response.data),
                }
                .into());
            }
        }
        Ok(())
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
    ///
    /// Memory writes have a transport acknowledgement but no application response.
    pub async fn write_user_memory(
        &mut self,
        request: crate::protocol::management::UserMemoryWrite,
    ) -> Result<()> {
        if request.count == 0
            || request.data.len() != usize::from(request.count)
            || request.address > 0x0F_FFFF
            || request.data.len() > 0x10_0000 - request.address as usize
        {
            return Err(crate::error::ProtocolError::InvalidFrame {
                details: "User memory write data, count, or address range is invalid".to_owned(),
            }
            .into());
        }
        self.send_data(request.encode()?, true).await
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
            self.write_memory(crate::protocol::management::MemoryWrite {
                address: current_address as u16,
                count: chunk.len() as u8,
                data: chunk.to_vec(),
            })
            .await?;
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
            self.write_user_memory(crate::protocol::management::UserMemoryWrite {
                address: current_address,
                count: chunk.len() as u8,
                data: chunk.to_vec(),
            })
            .await?;
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

    /// Switch programming mode using the mechanism associated with the mask version.
    ///
    /// Unknown mask versions return an error without writing to the device.
    pub async fn switch_programming_mode(
        &mut self,
        enabled: bool,
        verify: bool,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let mask = self.read_mask_version(timeout).await?;
        match mask {
            0x0010..=0x0013
            | 0x0020
            | 0x0021
            | 0x0025
            | 0x0700
            | 0x0701
            | 0x0705
            | 0x0900
            | 0x0910..=0x0912
            | 0x091a
            | 0x1011..=0x1013
            | 0x1900
            | 0x5705 => {
                self.switch_programming_mode_memory(enabled, verify, timeout)
                    .await
            }
            0x0300 | 0x07b0 | 0x17b0 | 0x2705 | 0x27b0 | 0x2920 | 0x57b0 => {
                self.switch_programming_mode_property(enabled, verify, timeout)
                    .await
            }
            _ => Err(crate::error::ProtocolError::InvalidFrame {
                details: format!(
                    "Programming-mode location is unknown for mask version {mask:04X}"
                ),
            }
            .into()),
        }
    }

    /// Send an unconfirmed basic restart to the connected device.
    ///
    /// The device can restart before acknowledging the telegram and the P2P
    /// link may stop working. Close the connection after this operation.
    pub async fn restart(&mut self) -> Result<()> {
        self.send_data(vec![0x03, 0x80], false).await
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

#[cfg(test)]
mod handover_tests;

#[cfg(test)]
mod shutdown_tests;

#[cfg(test)]
mod channel_tests;

#[cfg(test)]
mod integration_tests;
