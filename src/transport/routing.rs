//! KNX/IP routing connection implementation.

use super::connection::{Connection, ConnectionState, ConnectionStats};
use crate::error::{Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::knxip::{KnxIpFrame, ServiceType};
use rand::RngExt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

#[path = "routing_flow.rs"]
mod flow;
use flow::FlowControl;

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
    flow: std::sync::Mutex<FlowControl>,
    send_lock: tokio::sync::Mutex<()>,
    flow_changed: tokio::sync::Notify,
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
            flow: std::sync::Mutex::new(FlowControl::default()),
            send_lock: tokio::sync::Mutex::new(()),
            flow_changed: tokio::sync::Notify::new(),
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
            flow: std::sync::Mutex::new(FlowControl::default()),
            send_lock: tokio::sync::Mutex::new(()),
            flow_changed: tokio::sync::Notify::new(),
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
                if frame.body.len() != 4 || frame.body[0] != 4 {
                    return Err(TransportError::InvalidConfiguration {
                        details: "Invalid routing lost message body".to_string(),
                    }
                    .into());
                }
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
    fn handle_routing_lost_message(&self, body: &[u8]) {
        // Increment lost message counter
        if let Ok(mut count) = self.lost_message_count.write() {
            *count += 1;
        }

        // Update statistics
        if let Ok(mut stats) = self.stats.write() {
            stats.record_receive_error(&"Routing lost message received");
        }

        log_transport!(
            LogLevel::Warn,
            "KNX/IP routing lost message indication: {} messages lost (device state {})",
            u16::from_be_bytes([body[2], body[3]]),
            body[1]
        );
    }

    async fn handle_routing_busy(&self, body: &[u8]) -> Result<()> {
        if body.len() != 6 || body[0] != 6 {
            return Err(TransportError::InvalidConfiguration {
                details: "RoutingBusy must contain a six-byte structure".to_string(),
            }
            .into());
        }
        let wait = u16::from_be_bytes([body[2], body[3]]);
        let random = rand::rng().random::<f64>();
        self.flow.lock().unwrap().busy(
            tokio::time::Instant::now(),
            Duration::from_millis(u64::from(wait)),
            random,
        );
        self.flow_changed.notify_waiters();
        if let Ok(mut count) = self.routing_busy_count.write() {
            *count += 1;
        }
        if let Ok(mut stats) = self.stats.write() {
            stats.record_receive_error(&format!(
                "Routing busy: device_state=0x{:02X}, wait_time={wait}ms",
                body[1]
            ));
        }
        Ok(())
    }

    /// Send cEMI data in one KNX/IP routing indication.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is closed or sending the datagram fails.
    pub async fn send_routing_indication(&self, cemi_data: &[u8]) -> Result<()> {
        let frame = KnxIpFrame::new(ServiceType::RoutingIndication, cemi_data.to_vec());
        Connection::send(self, &frame.serialize()).await
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

        let indication =
            KnxIpFrame::parse(frame)?.header.service_type == ServiceType::RoutingIndication;
        let _send_guard = if indication {
            Some(self.send_lock.lock().await)
        } else {
            None
        };
        if indication {
            loop {
                let changed = self.flow_changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if !self.is_connected() {
                    return Err(TransportError::ConnectionClosed.into());
                }
                let now = tokio::time::Instant::now();
                let until = self.flow.lock().unwrap().deadline(now);
                if until <= now {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep_until(until) => {}
                    _ = &mut changed => {}
                }
            }
        }
        self.socket
            .send_to(frame, self.multicast_addr)
            .await
            .map_err(|e| {
                if let Ok(mut stats) = self.stats.write() {
                    stats.record_send_error(&e);
                }
                TransportError::SocketError {
                    operation: "send_to".to_string(),
                    source: e,
                }
            })?;

        if indication {
            self.flow.lock().unwrap().sent(tokio::time::Instant::now());
        }
        if let Ok(mut stats) = self.stats.write() {
            stats.frames_sent += 1;
        }
        Ok(())
    }

    async fn recv(&self) -> Result<Vec<u8>> {
        if !self.is_connected() {
            return Err(TransportError::ConnectionClosed.into());
        }

        loop {
            let mut buf = vec![0u8; 1024];
            let (len, _addr) = self.socket.recv_from(&mut buf).await.map_err(|e| {
                if let Ok(mut stats) = self.stats.write() {
                    stats.record_receive_error(&e);
                }
                TransportError::SocketError {
                    operation: "recv_from".to_string(),
                    source: e,
                }
            })?;
            buf.truncate(len);

            // Consume routing control messages; the application expects full KNX/IP frames.
            if self.process_routing_message(&buf).await?.is_empty() {
                continue;
            }
            if let Ok(mut stats) = self.stats.write() {
                stats.frames_received += 1;
            }
            return Ok(buf);
        }
    }

    async fn close(&self) -> Result<()> {
        if let Ok(mut state) = self.state.write() {
            *state = ConnectionState::Disconnecting;
        }

        self.flow_changed.notify_waiters();

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

#[cfg(test)]
mod health_tests {
    use super::*;

    async fn test_connection(destination: SocketAddr) -> RoutingConnection {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let local_addr = socket.local_addr().unwrap();
        RoutingConnection {
            socket: Arc::new(socket),
            multicast_addr: destination,
            local_addr,
            state: Arc::new(std::sync::RwLock::new(ConnectionState::Connected)),
            stats: Arc::new(std::sync::RwLock::new(ConnectionStats::default())),
            established_at: Some(Instant::now()),
            routing_busy_count: Arc::new(std::sync::RwLock::new(0)),
            lost_message_count: Arc::new(std::sync::RwLock::new(0)),
            flow: std::sync::Mutex::new(FlowControl::default()),
            send_lock: tokio::sync::Mutex::new(()),
            flow_changed: tokio::sync::Notify::new(),
        }
    }

    #[tokio::test]
    async fn routing_send_preserves_frames_and_wraps_cemi_only_once() {
        use crate::protocol::{
            CemiFrame, GroupValueService,
            address::{Address, GroupAddress, IndividualAddress},
            cemi::MessageCode,
        };
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let connection = test_connection(receiver.local_addr().unwrap()).await;
        let cemi = CemiFrame::new(
            MessageCode::LDataReq,
            IndividualAddress::new(1, 1, 1),
            Address::Group(GroupAddress::try_from_raw(1).unwrap()),
            GroupValueService::Write(vec![1]).encode(),
        )
        .serialize();
        let expected = KnxIpFrame::new(ServiceType::RoutingIndication, cemi.clone()).serialize();
        let mut bytes = [0; 1024];

        Connection::send(&connection, &expected).await.unwrap();
        let len = tokio::time::timeout(Duration::from_secs(1), receiver.recv(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes[..len], expected);
        assert_eq!(KnxIpFrame::parse(&bytes[..len]).unwrap().body, cemi);

        connection.send_routing_indication(&cemi).await.unwrap();
        let len = tokio::time::timeout(Duration::from_secs(1), receiver.recv(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes[..len], expected);
        assert_eq!(connection.stats().frames_sent, 2);

        *connection.state.write().unwrap() = ConnectionState::Disconnected;
        assert!(Connection::send(&connection, &expected).await.is_err());
        assert!(connection.send_routing_indication(&cemi).await.is_err());
        assert_eq!(connection.stats().frames_sent, 2);
    }

    #[tokio::test]
    async fn routing_receive_preserves_header_and_consumes_control_messages() {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let connection = test_connection(peer.local_addr().unwrap()).await;
        let lost = KnxIpFrame::new(ServiceType::RoutingLostMessage, vec![4, 0, 0, 1]).serialize();
        let indication =
            KnxIpFrame::new(ServiceType::RoutingIndication, vec![0x33, 0x44]).serialize();
        peer.send_to(&lost, connection.local_addr()).await.unwrap();
        peer.send_to(&indication, connection.local_addr())
            .await
            .unwrap();
        let received = tokio::time::timeout(Duration::from_secs(1), Connection::recv(&connection))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, indication);
        assert_eq!(KnxIpFrame::parse(&received).unwrap().body, [0x33, 0x44]);
        assert_eq!(connection.stats().frames_received, 1);
        assert_eq!(connection.routing_stats().lost_message_count, 1);
    }

    #[tokio::test]
    async fn concurrent_routing_sends_are_spaced_and_cancelled_waiters_do_not_block() {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let connection = Arc::new(test_connection(peer.local_addr().unwrap()).await);
        let frame = KnxIpFrame::new(ServiceType::RoutingIndication, vec![1]).serialize();
        let mut jobs = Vec::new();
        for _ in 0..3 {
            let connection = connection.clone();
            let frame = frame.clone();
            jobs.push(tokio::spawn(async move {
                connection.send(&frame).await.unwrap();
                tokio::time::Instant::now()
            }));
        }
        let mut bytes = [0; 64];
        for _ in 0..3 {
            tokio::time::timeout(Duration::from_secs(1), peer.recv(&mut bytes))
                .await
                .unwrap()
                .unwrap();
        }
        let mut sent_at = Vec::new();
        for job in jobs {
            sent_at.push(job.await.unwrap());
        }
        sent_at.sort();
        for pair in sent_at.windows(2) {
            assert!(pair[1].duration_since(pair[0]) >= Duration::from_millis(20));
        }
        connection
            .handle_routing_busy(&[6, 0, 0x03, 0xe8, 0, 0])
            .await
            .unwrap();
        let blocked = tokio::spawn({
            let connection = connection.clone();
            let frame = frame.clone();
            async move { connection.send(&frame).await }
        });
        tokio::task::yield_now().await;
        blocked.abort();
        let _ = blocked.await;
        // Close wakes the following waiter even though the busy delay has not elapsed.
        let next = tokio::spawn({
            let connection = connection.clone();
            async move { connection.send(&frame).await }
        });
        tokio::task::yield_now().await;
        let _ = connection.close().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), next)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn lost_message_validates_wire_structure_before_counting() {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let connection = test_connection(peer.local_addr().unwrap()).await;
        for body in [
            &[][..],
            &[4, 0, 0][..],
            &[3, 0, 0, 1][..],
            &[4, 0, 0, 1, 0][..],
        ] {
            let frame = KnxIpFrame::new(ServiceType::RoutingLostMessage, body.to_vec());
            assert!(
                connection
                    .process_routing_message(&frame.serialize())
                    .await
                    .is_err()
            );
        }
        assert_eq!(connection.routing_stats().lost_message_count, 0);
        assert_eq!(connection.health().recv_errors, 0);
        let frame = KnxIpFrame::new(ServiceType::RoutingLostMessage, vec![4, 2, 1, 2]);
        assert!(
            connection
                .process_routing_message(&frame.serialize())
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(connection.routing_stats().lost_message_count, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn busy_handling_is_nonblocking_and_validates_the_entire_body() {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let connection = test_connection(peer.local_addr().unwrap()).await;
        for body in [
            &[][..],
            &[6, 0, 0, 1][..],
            &[5, 0, 0, 1, 0, 0][..],
            &[6, 0, 0, 1, 0, 0, 0][..],
        ] {
            assert!(connection.handle_routing_busy(body).await.is_err());
        }
        let now = tokio::time::Instant::now();
        connection
            .handle_routing_busy(&[6, 2, 0, 100, 0, 0])
            .await
            .unwrap();
        assert_eq!(
            tokio::time::Instant::now(),
            now,
            "busy handler blocked receive processing"
        );
        assert!(connection.flow.lock().unwrap().deadline(now) >= now + Duration::from_millis(100));
        assert_eq!(connection.routing_stats().routing_busy_count, 1);
    }

    #[tokio::test]
    // `score()` returns literal 0.0/1.0 constants for the connected-without-
    // errors and disconnected states asserted here, not a computed ratio.
    #[allow(clippy::float_cmp)]
    async fn connection_health_tracks_routing_lifecycle_counters_and_errors() {
        let outbound_receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let connection = test_connection(outbound_receiver.local_addr().unwrap()).await;

        let initial = connection.health();
        assert_eq!(initial.state, ConnectionState::Connected);
        assert_eq!(initial.score(), 1.0);
        assert_eq!(initial.total_errors(), 0);
        assert!(initial.connected_since.is_some());

        connection
            .send_routing_indication(&[0x11, 0x22])
            .await
            .unwrap();
        let mut outbound = [0_u8; 64];
        outbound_receiver.recv_from(&mut outbound).await.unwrap();

        let inbound_sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let inbound = KnxIpFrame::new(ServiceType::RoutingIndication, vec![0x33, 0x44]);
        inbound_sender
            .send_to(&inbound.serialize(), connection.local_addr())
            .await
            .unwrap();
        assert_eq!(
            Connection::recv(&connection).await.unwrap(),
            inbound.serialize()
        );

        let error_window_start = Instant::now();
        let lost = KnxIpFrame::new(ServiceType::RoutingLostMessage, vec![4, 0, 0, 1]);
        assert_eq!(
            connection
                .process_routing_message(&lost.serialize())
                .await
                .unwrap(),
            []
        );

        let active = connection.health();
        assert_eq!(active.frames_sent, 1);
        assert_eq!(active.frames_received, 1);
        assert_eq!(active.recv_errors, 1);
        assert_eq!(active.total_errors(), 1);
        assert_eq!(
            active.last_error.as_deref(),
            Some("Routing lost message received")
        );
        assert!(
            active
                .last_error_at
                .is_some_and(|at| at >= error_window_start)
        );
        assert!(active.score() < 1.0);
        assert!(active.score() > 0.1);

        let _ = Connection::close(&connection).await;
        let stopped = connection.health();
        assert_eq!(stopped.state, ConnectionState::Disconnected);
        assert_eq!(stopped.score(), 0.0);
        assert!(stopped.connected_since.is_none());
        assert_eq!(stopped.frames_sent, 1);
        assert_eq!(stopped.frames_received, 1);
        assert_eq!(stopped.recv_errors, 1);
    }

    #[tokio::test]
    // `score()` returns the literal 1.0 constant for an error-free connected
    // connection, not a computed ratio.
    #[allow(clippy::float_cmp)]
    async fn connection_health_is_independent_between_routing_connections() {
        let sink_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sink_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let connection_a = test_connection(sink_a.local_addr().unwrap()).await;
        let connection_b = test_connection(sink_b.local_addr().unwrap()).await;

        let lost = KnxIpFrame::new(ServiceType::RoutingLostMessage, vec![4, 0, 0, 1]);
        connection_a
            .process_routing_message(&lost.serialize())
            .await
            .unwrap();
        connection_a.send_routing_indication(&[0x01]).await.unwrap();

        let health_a = connection_a.health();
        let health_b = connection_b.health();
        assert_eq!(health_a.frames_sent, 1);
        assert_eq!(health_a.recv_errors, 1);
        assert!(health_a.last_error.is_some());
        assert_eq!(health_b.frames_sent, 0);
        assert_eq!(health_b.recv_errors, 0);
        assert!(health_b.last_error.is_none());
        assert_eq!(health_b.score(), 1.0);
    }
}
