//! KNX/IP routing connection implementation.

use super::connection::{Connection, ConnectionState, ConnectionStats};
use crate::error::{Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::knxip::{KnxIpFrame, ServiceType};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

/// KNX/IP routing connection using multicast
pub struct RoutingConnection {
    socket: Arc<UdpSocket>,
    multicast_addr: SocketAddr,
    local_addr: SocketAddr,
    state: Arc<std::sync::RwLock<ConnectionState>>,
    stats: Arc<std::sync::RwLock<ConnectionStats>>,
    established_at: Option<Instant>,
    routing_busy_count: Arc<std::sync::RwLock<u32>>,
    lost_message_count: Arc<std::sync::RwLock<u32>>,
}

impl RoutingConnection {
    /// Standard KNX/IP multicast address
    pub const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 23, 12);
    pub const MULTICAST_PORT: u16 = 3671;

    /// Create a new routing connection
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::SocketError`] if binding the UDP socket,
    /// querying its local address, or joining the KNX multicast group fails.
    pub async fn new(local_addr: Option<IpAddr>) -> Result<Self> {
        let bind_addr = match local_addr {
            Some(addr) => SocketAddr::new(addr, Self::MULTICAST_PORT),
            None => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), Self::MULTICAST_PORT),
        };

        let socket = UdpSocket::bind(bind_addr)
            .await
            .map_err(|e| TransportError::SocketError {
                operation: "bind".to_string(),
                source: e,
            })?;

        let actual_local_addr = socket
            .local_addr()
            .map_err(|e| TransportError::SocketError {
                operation: "get_local_addr".to_string(),
                source: e,
            })?;

        let multicast_addr =
            SocketAddr::new(IpAddr::V4(Self::MULTICAST_ADDR), Self::MULTICAST_PORT);

        // Join multicast group
        socket
            .join_multicast_v4(Self::MULTICAST_ADDR, Ipv4Addr::UNSPECIFIED)
            .map_err(|e| TransportError::SocketError {
                operation: "join_multicast".to_string(),
                source: e,
            })?;

        Ok(Self {
            socket: Arc::new(socket),
            multicast_addr,
            local_addr: actual_local_addr,
            state: Arc::new(std::sync::RwLock::new(ConnectionState::Connected)),
            stats: Arc::new(std::sync::RwLock::new(ConnectionStats::default())),
            established_at: Some(Instant::now()),
            routing_busy_count: Arc::new(std::sync::RwLock::new(0)),
            lost_message_count: Arc::new(std::sync::RwLock::new(0)),
        })
    }

    /// Create a new routing connection with specific interface
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::SocketError`] if binding the UDP socket,
    /// querying its local address, or joining the KNX multicast group on
    /// `interface_addr` fails.
    pub async fn new_with_interface(interface_addr: Ipv4Addr) -> Result<Self> {
        let bind_addr = SocketAddr::new(IpAddr::V4(interface_addr), Self::MULTICAST_PORT);

        let socket = UdpSocket::bind(bind_addr)
            .await
            .map_err(|e| TransportError::SocketError {
                operation: "bind".to_string(),
                source: e,
            })?;

        let actual_local_addr = socket
            .local_addr()
            .map_err(|e| TransportError::SocketError {
                operation: "get_local_addr".to_string(),
                source: e,
            })?;

        let multicast_addr =
            SocketAddr::new(IpAddr::V4(Self::MULTICAST_ADDR), Self::MULTICAST_PORT);

        // Join multicast group on specific interface
        socket
            .join_multicast_v4(Self::MULTICAST_ADDR, interface_addr)
            .map_err(|e| TransportError::SocketError {
                operation: "join_multicast_with_interface".to_string(),
                source: e,
            })?;

        Ok(Self {
            socket: Arc::new(socket),
            multicast_addr,
            local_addr: actual_local_addr,
            state: Arc::new(std::sync::RwLock::new(ConnectionState::Connected)),
            stats: Arc::new(std::sync::RwLock::new(ConnectionStats::default())),
            established_at: Some(Instant::now()),
            routing_busy_count: Arc::new(std::sync::RwLock::new(0)),
            lost_message_count: Arc::new(std::sync::RwLock::new(0)),
        })
    }

    /// Process incoming routing messages and handle routing-specific frames
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::InvalidConfiguration`] if `frame_data` isn't
    /// a valid KNX/IP frame.
    pub async fn process_routing_message(&self, frame_data: &[u8]) -> Result<Vec<u8>> {
        // Parse the KNX/IP frame
        let frame =
            KnxIpFrame::parse(frame_data).map_err(|e| TransportError::InvalidConfiguration {
                details: format!("Failed to parse routing frame: {e}"),
            })?;

        match frame.header.service_type {
            ServiceType::RoutingIndication => {
                // This is a normal routing message containing CEMI data
                // Return the CEMI payload for further processing
                Ok(frame.body)
            }
            ServiceType::RoutingLostMessage => {
                // Handle lost message indication
                self.handle_routing_lost_message(&frame.body);
                // Return empty vec as this is a control message
                Ok(Vec::new())
            }
            ServiceType::RoutingBusy => {
                // Handle routing busy indication
                self.handle_routing_busy(&frame.body).await?;
                // Return empty vec as this is a control message
                Ok(Vec::new())
            }
            _ => {
                // For other service types, return the body as-is
                Ok(frame.body)
            }
        }
    }

    /// Handle routing lost message indication
    fn handle_routing_lost_message(&self, _body: &[u8]) {
        // Increment lost message counter
        if let Ok(mut count) = self.lost_message_count.write() {
            *count += 1;
        }

        // Update statistics
        if let Ok(mut stats) = self.stats.write() {
            stats.recv_errors += 1;
            stats.last_error = Some("Routing lost message received".to_string());
        }

        log_transport!(
            LogLevel::Warn,
            "KNX/IP routing lost message indication received"
        );
    }

    /// Handle routing busy indication
    async fn handle_routing_busy(&self, body: &[u8]) -> Result<()> {
        // Parse routing busy parameters if present
        let device_state = if body.len() >= 2 {
            u16::from_be_bytes([body[0], body[1]])
        } else {
            0
        };

        let wait_time = if body.len() >= 4 {
            u16::from_be_bytes([body[2], body[3]])
        } else {
            100 // Default wait time in ms
        };

        // Increment busy counter
        if let Ok(mut count) = self.routing_busy_count.write() {
            *count += 1;
        }

        // Update statistics
        if let Ok(mut stats) = self.stats.write() {
            stats.recv_errors += 1;
            stats.last_error = Some(format!(
                "Routing busy: device_state=0x{device_state:04X}, wait_time={wait_time}ms"
            ));
        }

        log_transport!(
            LogLevel::Warn,
            "KNX/IP routing busy indication received (device_state=0x{device_state:04X}, wait_time={wait_time}ms)"
        );

        // In a real implementation, we might want to implement backoff logic here
        if wait_time > 0 {
            tokio::time::sleep(Duration::from_millis(u64::from(wait_time))).await;
        }

        Ok(())
    }

    /// Send a routing indication message
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::SocketError`] if sending the multicast
    /// datagram fails.
    pub async fn send_routing_indication(&self, cemi_data: &[u8]) -> Result<()> {
        let frame = KnxIpFrame::new(ServiceType::RoutingIndication, cemi_data.to_vec());
        let frame_data = frame.serialize();

        self.socket
            .send_to(&frame_data, self.multicast_addr)
            .await
            .map_err(|e| {
                if let Ok(mut stats) = self.stats.write() {
                    stats.send_errors += 1;
                    stats.last_error = Some(e.to_string());
                }
                TransportError::SocketError {
                    operation: "send_routing_indication".to_string(),
                    source: e,
                }
            })?;

        if let Ok(mut stats) = self.stats.write() {
            stats.frames_sent += 1;
        }

        Ok(())
    }

    /// Get routing-specific statistics
    #[must_use]
    pub fn routing_stats(&self) -> RoutingStats {
        let busy_count = self.routing_busy_count.read().map_or(0, |c| *c);
        let lost_count = self.lost_message_count.read().map_or(0, |c| *c);

        RoutingStats {
            routing_busy_count: busy_count,
            lost_message_count: lost_count,
            uptime: self.uptime(),
        }
    }

    /// Get connection uptime
    #[must_use]
    pub fn uptime(&self) -> Option<Duration> {
        self.established_at.map(|start| start.elapsed())
    }

    /// Check if connection is active
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(self.state(), ConnectionState::Connected)
    }

    /// Get local address
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Get multicast address
    #[must_use]
    pub fn multicast_addr(&self) -> SocketAddr {
        self.multicast_addr
    }
}

#[async_trait::async_trait]
impl Connection for RoutingConnection {
    async fn send(&self, frame: &[u8]) -> Result<()> {
        if !self.is_connected() {
            return Err(TransportError::ConnectionClosed.into());
        }

        // For routing connections, we need to wrap the CEMI data in a RoutingIndication frame
        self.send_routing_indication(frame).await
    }

    async fn recv(&self) -> Result<Vec<u8>> {
        if !self.is_connected() {
            return Err(TransportError::ConnectionClosed.into());
        }

        let mut buf = vec![0u8; 1024];
        let (len, _addr) = self.socket.recv_from(&mut buf).await.map_err(|e| {
            if let Ok(mut stats) = self.stats.write() {
                stats.recv_errors += 1;
                stats.last_error = Some(e.to_string());
            }
            TransportError::SocketError {
                operation: "recv_from".to_string(),
                source: e,
            }
        })?;

        buf.truncate(len);

        // Process the routing message and extract CEMI data
        let cemi_data = self.process_routing_message(&buf).await?;

        // Only update stats and return data if we got actual CEMI data
        if !cemi_data.is_empty()
            && let Ok(mut stats) = self.stats.write()
        {
            stats.frames_received += 1;
        }

        Ok(cemi_data)
    }

    async fn close(&self) -> Result<()> {
        if let Ok(mut state) = self.state.write() {
            *state = ConnectionState::Disconnecting;
        }

        // Leave multicast group
        let result = self
            .socket
            .leave_multicast_v4(Self::MULTICAST_ADDR, Ipv4Addr::UNSPECIFIED);

        if let Ok(mut state) = self.state.write() {
            *state = ConnectionState::Disconnected;
        }

        result.map_err(|e| TransportError::SocketError {
            operation: "leave_multicast".to_string(),
            source: e,
        })?;

        Ok(())
    }

    fn state(&self) -> ConnectionState {
        *self.state.read().unwrap()
    }

    fn stats(&self) -> ConnectionStats {
        let mut stats = self.stats.read().unwrap().clone();

        // Update uptime if connected
        if let Some(uptime) = self.uptime() {
            stats.uptime_seconds = uptime.as_secs();
        }

        stats
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Routing-specific statistics
#[derive(Debug, Clone, Default)]
pub struct RoutingStats {
    /// Number of routing busy messages received
    pub routing_busy_count: u32,

    /// Number of lost message indications received
    pub lost_message_count: u32,

    /// Connection uptime
    pub uptime: Option<Duration>,
}
