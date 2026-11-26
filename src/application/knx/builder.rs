//! Builder for configuring a Knx client before connecting.

use super::Knx;
use crate::application::callbacks::{ConnectionCallbackFn, TelegramCallbackFn, TelegramFilter};
use crate::error::{ConfigurationError, KnxError, Result};
use crate::log_application;
use crate::logging::LogLevel;
use crate::transport::{ConnectionConfig, ConnectionType};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

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

    /// Set the IP address and port of the tunneling gateway.
    #[must_use]
    pub fn gateway(mut self, address: SocketAddr) -> Self {
        self.config.gateway_ip = Some(address.ip());
        self.config.gateway_port = Some(address.port());
        self
    }

    /// Select UDP tunneling to a gateway with a known endpoint.
    #[must_use]
    pub fn tunnel(mut self, gateway: SocketAddr) -> Self {
        self.config.security = None;
        self.connection_type(ConnectionType::Tunneling)
            .gateway(gateway)
    }

    /// Select TCP tunneling to a gateway with a known endpoint.
    #[must_use]
    pub fn tcp_tunnel(mut self, gateway: SocketAddr) -> Self {
        self.config.security = None;
        self.connection_type(ConnectionType::TcpTunneling)
            .gateway(gateway)
    }

    /// Select authenticated KNX IP Secure tunneling over TCP.
    #[cfg(feature = "secure")]
    #[must_use]
    pub fn secure_tunnel(
        mut self,
        gateway: SocketAddr,
        security: crate::transport::SecurityConfig,
    ) -> Self {
        self.config.security = Some(security);
        self.connection_type(ConnectionType::SecureTunneling)
            .gateway(gateway)
    }

    /// Select multicast routing, optionally on a specific IPv4 interface.
    #[must_use]
    pub fn routing(mut self, interface: Option<Ipv4Addr>) -> Self {
        self.config.security = None;
        self.config.gateway_ip = None;
        self.config.gateway_port = None;
        self.config.local_ip = interface.map(Into::into);
        self.connection_type(ConnectionType::Routing)
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

    /// Set the connection deadline for either tunnel transport.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        let millis = u64::try_from(timeout.as_millis())
            .unwrap_or(u64::MAX)
            .max(u64::from(!timeout.is_zero()));
        self.config.timeout_ms = millis;
        self.config.tcp_config.connect_timeout_ms = millis;
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

    /// Build the client, connect it, and start processing telegrams.
    ///
    /// # Errors
    /// Returns configuration, connection, or startup errors.
    pub async fn connect(self) -> Result<Knx> {
        let knx = self.build().await?;
        knx.connect_and_start().await?;
        Ok(knx)
    }
}

impl Default for KnxBuilder {
    fn default() -> Self {
        Self::new()
    }
}
