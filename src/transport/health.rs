//! Connection health tracking and scoring.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// State of a gateway connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayConnectionState {
    Connecting,
    Connected,
    Degraded,
    Disconnected,
    Failed,
}

/// Health metrics for a single gateway connection.
#[derive(Debug, Clone)]
pub struct ConnectionHealth {
    pub gateway: SocketAddr,
    pub state: GatewayConnectionState,
    pub latency_ms: Option<u64>,
    pub connected_since: Option<Instant>,
    pub total_failures: u64,
    pub consecutive_failures: u32,
    pub last_heartbeat: Option<Instant>,
    pub telegrams_received: u64,
    pub telegrams_sent: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
}

impl ConnectionHealth {
    #[must_use]
    pub fn new(gateway: SocketAddr) -> Self {
        Self {
            gateway,
            state: GatewayConnectionState::Disconnected,
            latency_ms: None,
            connected_since: None,
            total_failures: 0,
            consecutive_failures: 0,
            last_heartbeat: None,
            telegrams_received: 0,
            telegrams_sent: 0,
            rx_errors: 0,
            tx_errors: 0,
        }
    }

    pub fn record_connected(&mut self) {
        self.state = GatewayConnectionState::Connected;
        self.connected_since = Some(Instant::now());
        self.consecutive_failures = 0;
    }

    pub fn record_disconnected(&mut self) {
        self.state = GatewayConnectionState::Disconnected;
        self.connected_since = None;
    }

    pub fn record_heartbeat_success(&mut self, latency_ms: u64) {
        self.latency_ms = Some(latency_ms);
        self.last_heartbeat = Some(Instant::now());
        self.consecutive_failures = 0;
        if self.state == GatewayConnectionState::Degraded {
            self.state = GatewayConnectionState::Connected;
        }
    }

    pub fn record_heartbeat_failure(&mut self) {
        self.consecutive_failures += 1;
        self.total_failures += 1;
        if self.consecutive_failures >= 2 {
            self.state = GatewayConnectionState::Degraded;
        }
    }

    pub fn record_telegram_received(&mut self) {
        self.telegrams_received += 1;
    }

    pub fn record_telegram_sent(&mut self) {
        self.telegrams_sent += 1;
    }

    pub fn record_telegram_receive_error(&mut self) {
        self.rx_errors += 1;
    }

    pub fn record_telegram_send_error(&mut self) {
        self.tx_errors += 1;
    }

    /// Uptime since connection was established.
    #[must_use]
    pub fn uptime(&self) -> Option<Duration> {
        self.connected_since.map(|t| t.elapsed())
    }

    /// Health score from 0.0 (dead) to 1.0 (perfect).
    /// Factors: connection state, latency, consecutive failures, uptime.
    #[must_use]
    pub fn score(&self) -> f64 {
        match self.state {
            GatewayConnectionState::Failed | GatewayConnectionState::Disconnected => 0.0,
            GatewayConnectionState::Connecting => 0.1,
            GatewayConnectionState::Degraded => 0.3,
            GatewayConnectionState::Connected => {
                let mut score: f64 = 1.0;
                // Penalize high latency
                if let Some(lat) = self.latency_ms {
                    if lat > 1000 {
                        score -= 0.3;
                    } else if lat > 500 {
                        score -= 0.2;
                    } else if lat > 200 {
                        score -= 0.1;
                    }
                }
                // Penalize failures
                if self.total_failures > 10 {
                    score -= 0.1;
                }
                score.max(0.1)
            }
        }
    }
}
