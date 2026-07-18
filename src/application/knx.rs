//! Main Knx library interface.

use crate::application::callbacks::{
    CallbackHandle, ConnectionCallbackFn, EventHandler, TelegramCallbackFn, TelegramFilter,
};
use crate::error::{ConfigurationError, DeviceError, KnxError, Result};
use crate::log_application;
use crate::logging::{Component, LogLevel, Timer};
use crate::memory::{MemoryMonitor, PerformanceOptimizer};
use crate::protocol::Telegram;
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
// TODO: No StateUpdater — nothing periodically re-reads group addresses that
// haven't been updated recently, so a value that changed while nothing was
// listening — or whose initial state was never read — can silently stay
// stale here indefinitely. Callers only learn of updates from telegrams
// they happen to receive.
//
// TODO: No GroupAddressDPT mapping. There's no central "group address X is
// DPT Y" table — every caller has to already know the encoding for a given
// address. A generic
// bus-monitoring/logging tool built on this crate can't auto-decode raw
// telegram payloads without that kind of project-wide type mapping.
#[derive(Clone)]
pub struct Knx {
    /// Connection configuration
    config: ConnectionConfig,

    /// Active connection
    connection: Arc<RwLock<Option<Arc<dyn Connection>>>>,

    /// Library state
    state: Arc<RwLock<KnxState>>,

    /// Shutdown flag for graceful termination
    shutdown_flag: Arc<AtomicBool>,

    /// Notified when the library shuts down (for `run()` to wake from)
    shutdown_notify: Arc<Notify>,

    /// Handle to the telegram processing task
    processing_task: TaskSlot,

    /// Handle to the telegram receiving task
    receiving_task: TaskSlot,

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
    control_tx: mpsc::Sender<ConnectionControlEvent>,

    /// Receiver for connection control events (wrapped in Option for taking)
    control_rx: Arc<RwLock<Option<mpsc::Receiver<ConnectionControlEvent>>>>,

    /// Current communication channel ID (for tunneling connections)
    channel_id: Arc<RwLock<Option<u8>>>,

    /// Receive-path rate limiter
    receive_limiter: Option<Arc<ReceiveRateLimiter>>,
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
        let (control_tx, control_rx) = mpsc::channel::<ConnectionControlEvent>(32);

        // Create receive rate limiter
        let (receive_limiter, mut receive_limiter_rx) =
            ReceiveRateLimiter::new(ReceiveLimitConfig::default());
        let receive_limiter = Arc::new(receive_limiter);

        // Spawn drain task — telegrams gated through the limiter are forwarded
        // into the telegram queue by the receive path directly; this drain just
        // prevents the internal channel from filling up.
        let tq_for_drain = telegram_queue.clone();
        tokio::spawn(async move {
            while let Some(telegram) = receive_limiter_rx.recv().await {
                let _ = tq_for_drain.enqueue_incoming(telegram).await;
            }
        });

        let knx = Self {
            config,
            connection: Arc::new(RwLock::new(None)),
            state: Arc::new(RwLock::new(KnxState::Disconnected)),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(Notify::new()),
            processing_task: Arc::new(RwLock::new(None)),
            receiving_task: Arc::new(RwLock::new(None)),
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

    /// Start the connection control task that handles disconnect/reconnect events
    /// Following Python implementation pattern for `tunnel_lost` handling
    async fn start_connection_control_task(&self, connection: Arc<dyn Connection>) -> Result<()> {
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
                    Some(event) = control_rx.recv() => {
                        match event {
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

                                // Clear the channel ID since we're disconnected
                                *channel_id_arc.write().await = None;
                            }
                            ConnectionControlEvent::TunnelLost { channel_id, reason } => {
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

                                // Clear the connection
                                {
                                    let mut conn = connection_arc.write().await;
                                    if let Some(c) = conn.take()
                                        && let Err(e) = c.close().await {
                                        log_application!(LogLevel::Warn, "Failed to close connection: {}", e);
                                    }
                                }

                                if auto_reconnect {
                                    log_application!(LogLevel::Info, "Auto-reconnect enabled, starting reconnection task");

                                    // Check if reconnection is already in progress
                                    let mut reconnect_task_guard = reconnect_task.write().await;
                                    if reconnect_task_guard.is_none() {
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
                                    break;
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

    /// Start reconnection task with exponential backoff (following Python implementation)
    fn start_reconnection_task(
        config: ConnectionConfig,
        connection_arc: Arc<RwLock<Option<Arc<dyn Connection>>>>,
        state: Arc<RwLock<KnxState>>,
        event_handler: Arc<EventHandler>,
        channel_id_arc: Arc<RwLock<Option<u8>>>,
        shutdown_flag: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            log_application!(LogLevel::Info, "Reconnection task started");

            let backoff = &config.reconnect_backoff;
            let mut attempt = 1;
            let mut delay_ms = backoff.initial_delay_ms;

            loop {
                // Check shutdown flag
                if shutdown_flag.load(Ordering::SeqCst) {
                    log_application!(
                        LogLevel::Info,
                        "Shutdown flag set, stopping reconnection task"
                    );
                    break;
                }

                // Check if we've exceeded max attempts
                if attempt > backoff.max_attempts {
                    log_application!(
                        LogLevel::Error,
                        "Maximum reconnection attempts ({}) exceeded, giving up",
                        backoff.max_attempts
                    );
                    break;
                }

                log_application!(
                    LogLevel::Debug,
                    "Reconnecting to KNX bus... (attempt {} of {})",
                    attempt,
                    backoff.max_attempts
                );

                // Attempt to create and establish new connection
                match Self::create_connection(&config).await {
                    Ok(new_connection) => {
                        // Get channel ID from the connection
                        {
                            let channel_id = Self::establish_connection(&new_connection, &config);
                            {
                                log_application!(
                                    LogLevel::Info,
                                    "Successfully reconnected to KNX bus"
                                );

                                // Update connection and state
                                {
                                    let mut conn = connection_arc.write().await;
                                    *conn = Some(new_connection);
                                }

                                {
                                    let mut state_guard = state.write().await;
                                    *state_guard = KnxState::Connected;
                                }

                                // Update channel ID, keeping the previous value if this
                                // connection type doesn't have one (Routing) or the
                                // downcast to get it failed.
                                if let Some(channel_id) = channel_id {
                                    let mut channel_guard = channel_id_arc.write().await;
                                    *channel_guard = Some(channel_id);
                                }

                                // Notify callbacks
                                event_handler
                                    .notify_connection_state_changed(
                                        crate::application::callbacks::ConnectionState::Connected,
                                    )
                                    .await;

                                log_application!(
                                    LogLevel::Info,
                                    "Reconnection successful, resuming operations"
                                );
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        log_application!(
                            LogLevel::Warn,
                            "Failed to create connection on attempt {}: {}",
                            attempt,
                            e
                        );
                    }
                }

                // Wait before next attempt with exponential backoff
                log_application!(
                    LogLevel::Debug,
                    "Reconnection attempt {} failed. Waiting {}ms before next attempt",
                    attempt,
                    delay_ms
                );

                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

                // Update for next attempt
                attempt += 1;
                delay_ms = std::cmp::min(
                    (delay_ms as f64 * backoff.multiplier) as u64,
                    backoff.max_delay_ms,
                );
            }

            log_application!(LogLevel::Info, "Reconnection task stopped");
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

    /// Connect to the KNX/IP network
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError::MissingParameter`] if `self.config`
    /// selects a tunneling connection type without a `gateway_ip`, or the
    /// underlying [`Tunnel::connect`]/[`RoutingConnection::new`] error for
    /// the selected connection type.
    pub async fn connect(&self) -> Result<()> {
        let timer = Timer::start(Component::Application, "knx_connect");
        log_application!(
            LogLevel::Info,
            "Connecting to KNX/IP network using {:?}",
            self.config.connection_type
        );

        {
            let mut state = self.state.write().await;
            *state = KnxState::Connecting;
        }

        // Notify connection state change callbacks
        self.notify_connection_state_changed(KnxState::Connecting.into())
            .await;

        log::info!(target: "transport", "Connection state change: {:?} disconnected -> connecting (connect() called)", self.config.connection_type);

        let connection_result = match self.config.connection_type {
            ConnectionType::Tunneling | ConnectionType::SecureTunneling => {
                let gateway_ip = self.config.gateway_ip.ok_or_else(|| {
                    log_application!(
                        LogLevel::Error,
                        "Gateway IP not configured for tunneling connection"
                    );
                    ConfigurationError::MissingParameter {
                        parameter: "gateway_ip".to_string(),
                    }
                })?;

                let gateway_port = self.config.gateway_port.unwrap_or(3671);
                let gateway_addr = std::net::SocketAddr::new(gateway_ip, gateway_port);

                log_application!(
                    LogLevel::Info,
                    "Creating UDP tunneling connection to {}:{}",
                    gateway_ip,
                    gateway_port
                );

                let mut tunneling_conn = Tunnel::new_udp(gateway_addr);

                if self.config.connection_type == ConnectionType::SecureTunneling {
                    #[cfg(feature = "secure")]
                    {
                        let security = self.config.security.as_ref().ok_or_else(|| {
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
                let channel_id = tunneling_conn.channel_id();
                *self.channel_id.write().await = Some(channel_id);

                Ok(Arc::new(tunneling_conn) as Arc<dyn Connection>)
            }
            ConnectionType::TcpTunneling => {
                let gateway_ip = self.config.gateway_ip.ok_or_else(|| {
                    log_application!(
                        LogLevel::Error,
                        "Gateway IP not configured for TCP tunneling connection"
                    );
                    ConfigurationError::MissingParameter {
                        parameter: "gateway_ip".to_string(),
                    }
                })?;

                let gateway_port = self.config.gateway_port.unwrap_or(3671);
                let gateway_addr = std::net::SocketAddr::new(gateway_ip, gateway_port);

                log_application!(
                    LogLevel::Info,
                    "Creating TCP tunneling connection to {}:{}",
                    gateway_ip,
                    gateway_port
                );

                let mut tcp_conn = Tunnel::new_tcp_with_timeout(
                    gateway_addr,
                    std::time::Duration::from_millis(self.config.tcp_config.connect_timeout_ms),
                );

                // Establish the TCP connection
                tcp_conn.connect().await?;

                Ok(Arc::new(tcp_conn) as Arc<dyn Connection>)
            }
            ConnectionType::Routing => {
                log_application!(
                    LogLevel::Info,
                    "Creating routing connection (local_ip: {:?})",
                    self.config.local_ip
                );

                RoutingConnection::new(self.config.local_ip)
                    .await
                    .map(|conn| Arc::new(conn) as Arc<dyn Connection>)
            }
            ConnectionType::SecureRouting => Err(ConfigurationError::ValidationError {
                details: "SecureRouting (KNX IP Secure over multicast) is not yet implemented"
                    .to_string(),
            }
            .into()),
        };

        match connection_result {
            Ok(connection) => {
                {
                    let mut conn = self.connection.write().await;
                    *conn = Some(connection);
                }

                {
                    let mut state = self.state.write().await;
                    *state = KnxState::Connected;
                }

                // Notify connection state change callbacks
                self.notify_connection_state_changed(KnxState::Connected.into())
                    .await;

                log::info!(target: "transport", "Connection state change: {:?} connecting -> connected", self.config.connection_type);

                log_application!(LogLevel::Info, "Successfully connected to KNX/IP network");
                timer.finish_with_message("KNX/IP connection established");

                Ok(())
            }
            Err(e) => {
                {
                    let mut state = self.state.write().await;
                    *state = KnxState::Error;
                }

                // Notify connection state change callbacks
                self.notify_connection_state_changed(KnxState::Error.into())
                    .await;

                log::info!(target: "transport", "Connection state change: {:?} connecting -> error ({})", self.config.connection_type, e);
                log::error!(target: "application", "Failed to connect to KNX/IP network: {} (category: {})", e, e.category());
                timer.finish_with_message(&format!("KNX/IP connection failed: {e}"));

                Err(e)
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

        if let Some(connection) = self.connection.write().await.take() {
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

    /// Send a telegram to the KNX network
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::QueueClosed`](crate::error::TransportError::QueueClosed)
    /// if the outgoing queue has been closed, or
    /// [`TransportError::QueueFull`](crate::error::TransportError::QueueFull)
    /// if it is at capacity.
    pub async fn send_telegram(&self, telegram: &Telegram) -> Result<()> {
        // Enqueue the telegram for sending with priority handling
        self.telegram_queue
            .enqueue_outgoing(telegram.clone())
            .await?;
        Ok(())
    }

    /// Read a group value and wait for the matching `GroupValueResponse` payload.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::send_telegram`] if sending the
    /// `GroupValueRead` fails, or
    /// [`DeviceError::CommunicationTimeout`] if no response arrives within
    /// `timeout_duration`.
    pub async fn read_group_value(
        &self,
        address: GroupAddress,
        timeout_duration: std::time::Duration,
    ) -> Result<Vec<u8>> {
        let (tx, mut rx) = mpsc::channel(1);
        let handle = self
            .register_telegram_callback(GroupValueReadCallback { address, tx })
            .await;

        let read_telegram = Telegram::new_outgoing(
            self.config.individual_address,
            Address::Group(address),
            Vec::new(),
        );

        if let Err(err) = self.send_telegram(&read_telegram).await {
            self.unregister_callback(handle).await;
            return Err(err);
        }

        let result = tokio::time::timeout(timeout_duration, rx.recv()).await;
        self.unregister_callback(handle).await;

        match result {
            Ok(Some(payload)) => Ok(payload),
            Ok(None) | Err(_) => Err(DeviceError::CommunicationTimeout {
                device: format!("group address {address}"),
                timeout_ms: timeout_duration.as_millis() as u64,
            }
            .into()),
        }
    }

    /// Start the main event loop for processing incoming telegrams
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationError::ValidationError`] if [`Self::connect`]
    /// hasn't been called yet.
    pub async fn start(&self) -> Result<()> {
        let timer = Timer::start(Component::Application, "knx_start");
        log_application!(
            LogLevel::Info,
            "Starting Knx telegram processing with queue"
        );

        let connection = self.connection.read().await;
        let connection = connection
            .as_ref()
            .ok_or_else(|| {
                log_application!(
                    LogLevel::Error,
                    "Cannot start: not connected to KNX network"
                );
                KnxError::Configuration(ConfigurationError::ValidationError {
                    details: "Not connected to KNX network".to_string(),
                })
            })?
            .clone();

        // Start the connection control task (handles disconnect/reconnect events)
        self.start_connection_control_task(connection.clone())
            .await?;

        // Start the telegram receiving task
        self.start_telegram_receiving_task(connection.clone())
            .await?;

        // Start the telegram processing task
        self.start_telegram_processing_task().await?;

        // Start the outgoing telegram sending task
        self.start_telegram_sending_task(connection);

        log_application!(
            LogLevel::Info,
            "Knx telegram processing started successfully with queue"
        );
        timer.finish_with_message("Knx telegram processing started");

        Ok(())
    }

    /// Start the telegram receiving task that feeds the queue
    async fn start_telegram_receiving_task(&self, connection: Arc<dyn Connection>) -> Result<()> {
        let shutdown_flag = self.shutdown_flag.clone();
        let memory_monitor = self.memory_monitor.clone();
        let performance_optimizer = self.performance_optimizer.clone();
        let telegram_queue = self.telegram_queue.clone();
        let control_tx = self.control_tx.clone();
        let channel_id = self.channel_id.clone();
        let receive_limiter = self.receive_limiter.clone();

        log_application!(LogLevel::Debug, "Starting telegram receiving task");

        let handle = tokio::spawn(async move {
            log_application!(LogLevel::Info, "Telegram receiving task started");
            let mut receive_count = 0u64;

            loop {
                // Check shutdown flag
                if shutdown_flag.load(Ordering::SeqCst) {
                    log_application!(
                        LogLevel::Info,
                        "Shutdown flag set, stopping telegram receiving (received {} telegrams)",
                        receive_count
                    );
                    break;
                }

                // Use select to allow checking shutdown flag periodically
                tokio::select! {
                    result = connection.recv() => {
                        let recv_timer = std::time::Instant::now();

                        match result {
                            Ok(frame_data) => {
                                receive_count += 1;
                                log_application!(LogLevel::Trace, "Received frame data ({} bytes) - telegram #{}", frame_data.len(), receive_count);

                                // Track memory usage for frame data
                                let frame_size = frame_data.len() as u64;
                                if let Err(e) = memory_monitor.allocate(Component::Protocol, frame_size).await {
                                    log_application!(LogLevel::Warn, "Memory allocation failed for frame data: {}", e);
                                    continue;
                                }

                                // Handle incoming frame based on type
                                let parse_timer = std::time::Instant::now();
                                let current_channel_id = *channel_id.read().await;
                                match Self::handle_incoming_frame(&frame_data, &telegram_queue, &control_tx, current_channel_id, &connection, receive_limiter.as_deref()).await {
                                    Ok(frame_handled) => {
                                        let parse_duration = parse_timer.elapsed();
                                        performance_optimizer.record_hot_path("frame_handle", parse_duration).await;

                                        if frame_handled {
                                            log_application!(LogLevel::Trace, "Successfully handled frame #{}", receive_count);
                                        }
                                    }
                                    Err(e) => {
                                        log_application!(LogLevel::Warn, "Failed to handle frame #{}: {}", receive_count, e);
                                    }
                                }

                                // Deallocate frame memory
                                memory_monitor.deallocate(Component::Protocol, frame_size).await;

                                // Record overall receive processing time
                                let recv_duration = recv_timer.elapsed();
                                performance_optimizer.record_hot_path("telegram_recv", recv_duration).await;
                            }
                            Err(e) => {
                                log_application!(LogLevel::Error, "Failed to receive telegram: {}", e);
                                break;
                            }
                        }
                    }
                    () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                        // Periodic check for shutdown flag
                        if shutdown_flag.load(Ordering::SeqCst) {
                            break;
                        }
                    }
                }
            }

            // Close the queue when receiving stops
            telegram_queue.close().await;

            log_application!(
                LogLevel::Info,
                "Telegram receiving task stopped (total received: {})",
                receive_count
            );
        });

        // Store the task handle
        {
            let mut task = self.receiving_task.write().await;
            *task = Some(handle);
        }

        Ok(())
    }

    /// Start the telegram processing task that processes from the queue
    async fn start_telegram_processing_task(&self) -> Result<()> {
        let shutdown_flag = self.shutdown_flag.clone();
        let performance_optimizer = self.performance_optimizer.clone();
        let event_handler = self.event_handler.clone();
        let telegram_queue = self.telegram_queue.clone();

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
    fn start_telegram_sending_task(&self, connection: Arc<dyn Connection>) {
        let shutdown_flag = self.shutdown_flag.clone();
        let telegram_queue = self.telegram_queue.clone();
        let performance_optimizer = self.performance_optimizer.clone();

        log_application!(LogLevel::Debug, "Starting telegram sending task");

        tokio::spawn(async move {
            log_application!(LogLevel::Info, "Telegram sending task started");
            let mut send_count = 0u64;

            loop {
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
                if let Some(telegram) = telegram_queue.dequeue_outgoing().await {
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

                    let frame_data =
                        Self::build_outgoing_frame_for_connection(&telegram, &connection);

                    match connection.send(&frame_data).await {
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
        });
    }

    /// Stop the telegram processing loop
    pub async fn stop(&self) {
        log_application!(LogLevel::Info, "Stopping Knx telegram processing");

        // Set shutdown flag
        self.shutdown_flag.store(true, Ordering::SeqCst);

        // Close the telegram queue to signal tasks to stop
        self.telegram_queue.close().await;

        // Wait for receiving task to complete
        if let Some(handle) = self.receiving_task.write().await.take() {
            log_application!(LogLevel::Debug, "Waiting for receiving task to complete");
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }

        // Wait for processing task to complete
        if let Some(handle) = self.processing_task.write().await.take() {
            log_application!(LogLevel::Debug, "Waiting for processing task to complete");
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }

        // Wait for control task to complete
        if let Some(handle) = self.control_task.write().await.take() {
            log_application!(LogLevel::Debug, "Waiting for control task to complete");
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }

        // Wait for reconnect task to complete (if running)
        if let Some(handle) = self.reconnect_task.write().await.take() {
            log_application!(LogLevel::Debug, "Waiting for reconnect task to complete");
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }

        // Wait for cleanup task to complete
        if let Some(handle) = self.cleanup_task.write().await.take() {
            log_application!(LogLevel::Debug, "Waiting for cleanup task to complete");
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        }

        // Reset shutdown flag for potential restart
        self.shutdown_flag.store(false, Ordering::SeqCst);

        // Wake any run() waiter
        self.shutdown_notify.notify_waiters();

        log_application!(LogLevel::Info, "Knx telegram processing stopped");
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
        let connection = self.connection.read().await;
        connection.as_ref().map(|c| c.stats())
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
        control_tx: &mpsc::Sender<ConnectionControlEvent>,
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
                Self::handle_disconnect_request(&knx_frame, control_tx, current_channel_id).await?;
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
    /// Following Python implementation: send `DisconnectResponse`, then trigger `tunnel_lost`
    async fn handle_disconnect_request(
        knx_frame: &crate::protocol::knxip::KnxIpFrame,
        control_tx: &mpsc::Sender<ConnectionControlEvent>,
        current_channel_id: Option<u8>,
    ) -> Result<()> {
        use crate::protocol::knxip::DisconnectRequest;

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

        // Check if this disconnect is for our channel (following Python pattern)
        let is_our_channel =
            current_channel_id.is_none_or(|id| id == disconnect_request.communication_channel_id); // If we don't know our channel, assume it's ours

        if is_our_channel {
            // Send event to send DisconnectResponse
            if let Err(e) = control_tx
                .send(ConnectionControlEvent::SendDisconnectResponse {
                    channel_id: disconnect_request.communication_channel_id,
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
                .send(ConnectionControlEvent::TunnelLost {
                    channel_id: disconnect_request.communication_channel_id,
                    reason: "DisconnectRequest received from server".to_string(),
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
            gateway_id: None,
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

        let group_service = if telegram.payload.is_empty() {
            crate::protocol::GroupValueService::Read
        } else {
            crate::protocol::GroupValueService::Write(telegram.payload.clone())
        };
        let apci_data = group_service.encode();
        let cemi = CemiFrame::new(
            MessageCode::LDataReq,
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
        address::{Address, GroupAddress, IndividualAddress},
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
        service: GroupValueService,
        expected_type: TelegramType,
        expected_payload: &[u8],
    ) -> Telegram {
        let source = IndividualAddress::new(1, 1, 5);
        let destination = Address::Group(GroupAddress::new(1, 2, 3));
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
            GroupValueService::Read,
            TelegramType::GroupValueRead,
            &[0x00],
        );
    }

    #[test]
    fn parse_cemi_to_telegram_classifies_group_value_response() {
        let telegram = assert_parsed_group_service(
            GroupValueService::Response(vec![0x0c, 0x3f]),
            TelegramType::GroupValueResponse,
            &[0x0c, 0x3f],
        );
        assert_ne!(telegram.telegram_type, TelegramType::GroupValueWrite);
    }

    #[test]
    fn parse_cemi_to_telegram_classifies_group_value_write() {
        let telegram = assert_parsed_group_service(
            GroupValueService::Write(vec![0x0c, 0x3f]),
            TelegramType::GroupValueWrite,
            &[0x0c, 0x3f],
        );
        assert_ne!(telegram.telegram_type, TelegramType::GroupValueResponse);
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
            destination: Address::Group(GroupAddress::new(0, 1, 1)),
            payload: vec![0x01, 0x02],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            gateway_id: None,
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
            destination: Address::Group(GroupAddress::new(0, 1, 1)),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            gateway_id: None,
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
        let target_address = GroupAddress::new(0, 1, 1);

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
            gateway_id: None,
            timestamp: std::time::SystemTime::now(),
        };

        let non_matching_telegram = Telegram {
            source: IndividualAddress::new(1, 1, 1),
            destination: Address::Group(GroupAddress::new(0, 2, 2)),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            gateway_id: None,
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
            destination: Address::Group(GroupAddress::new(0, 1, 1)),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            gateway_id: None,
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
            destination: Address::Group(GroupAddress::new(0, 1, 1)),
            payload: vec![0x01],
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            gateway_id: None,
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
