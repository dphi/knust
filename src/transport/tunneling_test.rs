//! Unit tests for the unified tunnel (UDP transport).

#[cfg(test)]
mod tests {
    use crate::transport::connection::{Connection, ConnectionState};
    use crate::transport::tunnel::Tunnel;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn gateway() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 3671)
    }

    #[tokio::test]
    async fn test_tunnel_creation() {
        let conn = Tunnel::new_udp(gateway());

        assert_eq!(Connection::state(&conn), ConnectionState::Disconnected);
        assert_eq!(conn.channel_id(), 0);
        assert_eq!(conn.current_sequence(), 0);
        assert!(!conn.is_connected());
        assert!(conn.uptime().is_none());

        let stats = Connection::stats(&conn);
        assert_eq!(stats.frames_sent, 0);
        assert_eq!(stats.frames_received, 0);
    }

    #[tokio::test]
    async fn test_tunnel_sequence_management() {
        let conn = Tunnel::new_udp(gateway());
        assert_eq!(conn.current_sequence(), 0);
        conn.reset_sequence();
        assert_eq!(conn.current_sequence(), 0);
    }

    #[tokio::test]
    async fn test_tunnel_state_management() {
        let mut conn = Tunnel::new_udp(gateway());
        assert_eq!(Connection::state(&conn), ConnectionState::Disconnected);
        assert!(!conn.is_connected());

        // Will fail/timeout in the test environment; state must end Failed/Disconnected.
        if conn.connect().await.is_err() {
            let state = Connection::state(&conn);
            assert!(
                state == ConnectionState::Failed || state == ConnectionState::Disconnected,
                "unexpected state after failed connect: {state:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_tunnel_send_recv_without_connection() {
        let conn = Tunnel::new_udp(gateway());
        // The Connection contract refuses I/O while not connected.
        assert!(Connection::send(&conn, &[0x01, 0x02, 0x03]).await.is_err());
        assert!(Connection::recv(&conn).await.is_err());

        let health = Connection::health(&conn);
        assert_eq!(health.send_errors, 1);
        assert_eq!(health.recv_errors, 1);
        assert_eq!(health.total_errors(), 2);
        assert!(health.last_error.is_some());
        assert!(health.last_error_at.is_some());
    }

    #[tokio::test]
    async fn test_failed_connect_updates_connection_health() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);

        let mut conn = Tunnel::new_tcp_with_timeout(address, std::time::Duration::from_secs(1));
        let error_window_start = std::time::Instant::now();
        assert!(conn.connect().await.is_err());

        let health = Connection::health(&conn);
        assert_eq!(health.state, ConnectionState::Failed);
        assert_eq!(health.connection_errors, 1);
        assert!(health.last_error.is_some());
        assert!(
            health
                .last_error_at
                .is_some_and(|at| at >= error_window_start)
        );
        assert_eq!(health.score(), 0.0);
    }
}
