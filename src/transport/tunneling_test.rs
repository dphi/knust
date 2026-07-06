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
    }
}
