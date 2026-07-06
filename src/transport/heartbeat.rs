//! KNX/IP connection heartbeat monitor (KNX spec 03.08.02 §5.4).
//!
//! The monitor holds heartbeat state (consecutive failures, tunnel-lost flag,
//! last success). The loop that drives it — periodically sending
//! `ConnectionState_Request` and correlating the response through the
//! connection's `FrameRouter` — is owned by [`super::tunnel::Tunnel::start_heartbeat`],
//! which calls [`HeartbeatMonitor::record_success`] / [`HeartbeatMonitor::record_failure`]
//! and reports outcomes as [`HeartbeatEvent`]s.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use tokio::time::{Duration, Instant};

use crate::log_transport;
use crate::logging::LogLevel;

/// Configuration for heartbeat monitoring.
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// Interval between heartbeat requests (default: 60s per KNX spec)
    pub interval: Duration,
    /// Timeout waiting for response (default: 10s)
    pub timeout: Duration,
    /// Max consecutive failures before declaring tunnel lost (default: 3)
    pub max_failures: u8,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            timeout: Duration::from_secs(10),
            max_failures: 3,
        }
    }
}

/// Outcome of a single heartbeat cycle, reported by [`super::tunnel::HeartbeatHandle::recv_event`].
#[derive(Debug, Clone, Copy)]
pub struct HeartbeatEvent {
    /// Whether the `ConnectionState_Response` was received and OK within the timeout.
    pub ok: bool,
    /// Round-trip latency in milliseconds, if the heartbeat succeeded.
    pub latency_ms: Option<u64>,
}

/// Heartbeat monitor state for a single tunnel connection.
pub struct HeartbeatMonitor {
    config: HeartbeatConfig,
    channel_id: u8,
    label: String,
    consecutive_failures: AtomicU8,
    tunnel_lost: AtomicBool,
    last_success: std::sync::RwLock<Option<Instant>>,
}

impl HeartbeatMonitor {
    #[must_use]
    pub fn new(config: HeartbeatConfig, channel_id: u8, label: String) -> Self {
        Self {
            config,
            channel_id,
            label,
            consecutive_failures: AtomicU8::new(0),
            tunnel_lost: AtomicBool::new(false),
            last_success: std::sync::RwLock::new(None),
        }
    }

    /// Heartbeat configuration (interval, timeout, max failures).
    pub fn config(&self) -> &HeartbeatConfig {
        &self.config
    }

    /// Whether the tunnel has been declared lost.
    pub fn is_tunnel_lost(&self) -> bool {
        self.tunnel_lost.load(Ordering::SeqCst)
    }

    /// Get consecutive failure count.
    pub fn consecutive_failures(&self) -> u8 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }

    /// Get last successful heartbeat time.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn last_success(&self) -> Option<Instant> {
        *self.last_success.read().unwrap()
    }

    /// Record a successful heartbeat response.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        *self.last_success.write().unwrap() = Some(Instant::now());
        log_transport!(
            LogLevel::Debug,
            "[{}] Heartbeat OK (channel {})",
            self.label,
            self.channel_id
        );
    }

    /// Record a failed heartbeat. Returns true if the tunnel is now declared lost.
    pub fn record_failure(&self) -> bool {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        log_transport!(
            LogLevel::Warn,
            "[{}] Heartbeat failed (channel {}, attempt {}/{})",
            self.label,
            self.channel_id,
            failures,
            self.config.max_failures
        );
        if failures >= self.config.max_failures {
            self.tunnel_lost.store(true, Ordering::SeqCst);
            log_transport!(
                LogLevel::Error,
                "[{}] Tunnel lost: {} consecutive heartbeat failures (channel {})",
                self.label,
                failures,
                self.channel_id
            );
            return true;
        }
        false
    }
}
