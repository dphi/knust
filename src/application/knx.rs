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

mod builder;
#[cfg(feature = "dpt")]
pub(super) mod group_address;

mod p2p;

pub use builder::KnxBuilder;
use p2p::ActiveP2pSessions;
pub use p2p::{P2pConnection, P2pConnectionless, ScannedInterfaceObject};

#[derive(Clone, Default)]
struct TaskSlot(Arc<std::sync::Mutex<Option<OwnedTask>>>);

struct OwnedTask(JoinHandle<()>);

impl Drop for OwnedTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl TaskSlot {
    fn replace(&self, task: JoinHandle<()>) {
        let previous = self.0.lock().unwrap().replace(OwnedTask(task));
        drop(previous);
    }

    fn take(&self) -> Option<OwnedTask> {
        self.0.lock().unwrap().take()
    }

    fn is_running(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|task| !task.0.is_finished())
    }

    #[cfg(test)]
    fn abort_handle(&self) -> Option<tokio::task::AbortHandle> {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .map(|task| task.0.abort_handle())
    }
}

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

struct PendingCemiConfirmation {
    request: crate::protocol::CemiFrame,
    result: tokio::sync::oneshot::Sender<Result<()>>,
}

type CemiConfirmationSlot = std::sync::Mutex<Option<PendingCemiConfirmation>>;

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
    pending_cemi_confirmation: Arc<CemiConfirmationSlot>,

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

struct GroupValueReadCallback {
    address: RawGroupAddress,
    tx: mpsc::Sender<Vec<u8>>,
}

#[async_trait::async_trait]
impl TelegramCallbackFn for GroupValueReadCallback {
    async fn call(&self, telegram: &Telegram) {
        if telegram.telegram_type == TelegramType::GroupValueResponse
            && telegram.destination == Address::Group(self.address)
            && !telegram.payload.is_empty()
        {
            let _ = self.tx.try_send(telegram.payload.clone());
        }
    }
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
            processing_task: TaskSlot::default(),
            receiving_task: TaskSlot::default(),
            sending_task: TaskSlot::default(),
            receive_drain_task: TaskSlot::default(),
            control_task: TaskSlot::default(),
            reconnect_task: TaskSlot::default(),
            memory_monitor,
            performance_optimizer,
            cleanup_task: TaskSlot::default(),
            event_handler,
            telegram_queue,
            control_tx,
            control_rx: Arc::new(RwLock::new(Some(control_rx))),
            channel_id: Arc::new(RwLock::new(None)),
            receive_limiter: Some(receive_limiter),
            #[cfg(feature = "dpt")]
            group_registry: Arc::new(GroupAddressRegistry::new()),
            #[cfg(feature = "dpt")]
            refresh_task: TaskSlot::default(),
        };

        knx.receive_drain_task.replace(receive_drain_handle);

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

        self.cleanup_task.replace(handle);

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

        self.refresh_task.replace(handle);

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
                                    if !reconnect_task.is_running() {
                                        // Start reconnection task (following Python pattern)
                                        let reconnect_handle = Self::start_reconnection_task(
                                            config.clone(),
                                            connection_arc.clone(),
                                            state.clone(),
                                            event_handler.clone(),
                                            channel_id_arc.clone(),
                                            shutdown_flag.clone(),
                                        );

                                        reconnect_task.replace(reconnect_handle);
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
        self.control_task.replace(handle);

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
        if self.telegram_queue.is_closed().await {
            return Err(crate::error::TransportError::QueueClosed.into());
        }
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

        let result = match self.connection.send_replace(None) {
            Some(connection) => connection.close().await,
            None => Ok(()),
        };
        *self.channel_id.write().await = None;

        {
            let mut state = self.state.write().await;
            *state = ConnectionState::Disconnected;
        }

        // Notify connection state change callbacks
        self.notify_connection_state_changed(ConnectionState::Disconnected)
            .await;

        result
    }

    /// Install group Data Secure keys and sender sequence baselines.
    ///
    /// Reloading preserves the highest sequence already accepted for each
    /// sender still present in the new keyring.
    #[cfg(feature = "secure")]
    pub async fn set_data_secure_keyring(&self, mut keyring: KeyRing) {
        let mut current = self.data_secure_keyring.write().await;
        if let Some(previous) = current.as_ref() {
            keyring.merge_sequence_baselines(previous);
        }
        *current = Some(keyring);
    }

    /// Enqueue a telegram for transmission by the processing task.
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
        let job = crate::transport::queue::OutgoingTelegram::new(telegram.clone());
        self.send_tracked_job(job).await
    }

    async fn send_tracked_job(
        &self,
        mut job: crate::transport::queue::OutgoingTelegram,
    ) -> Result<()> {
        job.telegram.source = self.config.individual_address;
        let (_sent, completed) = self.telegram_queue.enqueue_tracked_job(job).await?;
        tokio::time::timeout(std::time::Duration::from_secs(10), completed)
            .await
            .map_err(|_| crate::error::TransportError::QueueTimeout { timeout_ms: 10_000 })?
            .map_err(|_| crate::error::TransportError::QueueClosed)?
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
        if self.telegram_queue.is_closed().await {
            return Err(crate::error::TransportError::QueueClosed.into());
        }
        if self.sending_task.is_running() {
            return Ok(());
        }
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
        self.sending_task
            .replace(self.start_telegram_sending_task());

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
        self.receiving_task.replace(handle);
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
        self.processing_task.replace(handle);

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
                if let Some(mut job) = telegram_queue.dequeue_outgoing_job().await {
                    let telegram = &job.telegram;
                    let mut receipt = job.receipt.take();
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
                        telegram,
                        &connection,
                        &data_secure_keyring,
                        &data_secure_sequence,
                    )
                    .await;
                    #[cfg(not(feature = "secure"))]
                    let frame_result =
                        Self::build_outgoing_frame_for_connection(telegram, &connection);
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
                        && !Self::is_cemi_confirmation_error(error)
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
                                    telegram,
                                    &replacement,
                                    &data_secure_keyring,
                                    &data_secure_sequence,
                                )
                                .await;
                            #[cfg(not(feature = "secure"))]
                            let replacement_frame_result =
                                Self::build_outgoing_frame_for_connection(telegram, &replacement);
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
                                && !Self::is_cemi_confirmation_error(error)
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
                    drop(job);
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
        pending_confirmation: &CemiConfirmationSlot,
    ) -> Result<()> {
        let receiver = if connection.as_any().is::<Tunnel>() {
            use crate::protocol::knxip::{KnxIpFrame, TunnellingRequest};

            let request = TunnellingRequest::parse(&KnxIpFrame::parse(frame)?.body)?;
            let request = crate::protocol::CemiFrame::parse(&request.raw_cemi)?;
            let (result, receiver) = tokio::sync::oneshot::channel();
            *pending_confirmation.lock().unwrap() =
                Some(PendingCemiConfirmation { request, result });
            Some(receiver)
        } else {
            None
        };
        if let Err(error) = Self::send_telegram_frame(connection, frame, attempts, receipt).await {
            if receiver.is_some() {
                pending_confirmation.lock().unwrap().take();
            }
            return Err(error);
        }
        if let Some(receiver) = receiver {
            match tokio::time::timeout(CEMI_CONFIRMATION_TIMEOUT, receiver).await {
                Ok(Ok(result)) => return result,
                Ok(Err(_)) => return Err(crate::error::TransportError::ConnectionClosed.into()),
                Err(_) => {
                    pending_confirmation.lock().unwrap().take();
                    return Err(DeviceError::CommunicationTimeout {
                        device: "L_Data.con".to_string(),
                        timeout_ms: CEMI_CONFIRMATION_TIMEOUT.as_millis() as u64,
                    }
                    .into());
                }
            }
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

    fn is_cemi_confirmation_error(error: &KnxError) -> bool {
        matches!(
            error,
            KnxError::Transport(crate::error::TransportError::NegativeConfirmation)
        ) || matches!(
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
        let handle = task.take();
        if let Some(mut handle) = handle
            && tokio::time::timeout(grace, &mut handle.0).await.is_err()
        {
            handle.0.abort();
            let _ = (&mut handle.0).await;
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
        let stopped = self.shutdown_notify.notified();
        tokio::pin!(stopped);
        stopped.as_mut().enable();
        self.connect_and_start().await?;
        stopped.await;
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
        self.connect_and_start().await?;

        // Wait for shutdown signal
        shutdown_signal.await;

        // Perform clean shutdown
        self.shutdown().await?;

        Ok(())
    }

    async fn connect_and_start(&self) -> Result<()> {
        self.connect().await?;
        if let Err(error) = self.start().await {
            let _ = self.disconnect().await;
            return Err(error);
        }
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
        pending_cemi_confirmation: Option<&CemiConfirmationSlot>,
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
        pending_cemi_confirmation: Option<&CemiConfirmationSlot>,
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

                if let Ok(frame) = crate::protocol::CemiFrame::parse(&tunnelling_request.raw_cemi)
                    && frame.message_code == crate::protocol::cemi::MessageCode::LDataCon
                {
                    if let Some(pending) = pending_cemi_confirmation {
                        let mut pending = pending.lock().unwrap();
                        if pending.as_ref().is_some_and(|pending| {
                            pending.request.dest_addr == frame.dest_addr
                                && pending.request.tpci == frame.tpci
                                && pending.request.apci_data == frame.apci_data
                        }) && let Some(pending) = pending.take()
                        {
                            let result = if frame.control_field.confirm {
                                Err(crate::error::TransportError::NegativeConfirmation.into())
                            } else {
                                Ok(())
                            };
                            let _ = pending.result.send(result);
                        }
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
            let mut keyring = data_secure_keyring.write().await;
            let keyring = keyring
                .as_mut()
                .ok_or_else(|| SecurityError::InvalidCredentials {
                    details: "Data Secure frame received without group keys".to_string(),
                })?;
            frame = crate::security::decrypt_group_frame(keyring, &frame)?;
        } else if let Address::Group(destination) = frame.dest_addr
            && data_secure_keyring
                .read()
                .await
                .as_ref()
                .is_some_and(|keys| keys.is_group_secured(&destination))
        {
            return Err(SecurityError::AuthenticationFailed {
                reason: format!("Plaintext telegram for secured group {destination}"),
            }
            .into());
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
        CemiFrame::from_telegram(telegram, message_code)
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

#[cfg(test)]
mod handover_tests;

#[cfg(test)]
mod shutdown_tests;

#[cfg(test)]
mod channel_tests;

#[cfg(test)]
mod integration_tests;
