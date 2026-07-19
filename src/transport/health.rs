//! Health snapshots for one KNX/IP connection.

use std::time::{Duration, Instant};

use super::connection::{ConnectionState, ConnectionStats};

/// Current health of one concrete transport connection.
///
/// A snapshot contains only state and statistics from the connection that
/// produced it. Creating a snapshot never consults or combines other
/// connections.
#[derive(Debug, Clone)]
pub struct ConnectionHealth {
    /// Lifecycle state observed for the connection.
    pub state: ConnectionState,
    /// Number of frames sent successfully by the connection.
    pub frames_sent: u64,
    /// Number of frames received successfully by the connection.
    pub frames_received: u64,
    /// Number of failed connection-establishment attempts.
    pub connection_errors: u64,
    /// Number of failed frame sends.
    pub send_errors: u64,
    /// Number of failed frame receives or receive-side protocol errors.
    pub recv_errors: u64,
    /// Time at which this snapshot was captured.
    pub observed_at: Instant,
    /// Approximate start of the current connected period.
    pub connected_since: Option<Instant>,
    /// Most recent connection or I/O error message.
    pub last_error: Option<String>,
    /// Time at which `last_error` was recorded.
    pub last_error_at: Option<Instant>,
}

impl ConnectionHealth {
    /// Build a health snapshot from one connection's state and statistics.
    #[must_use]
    pub fn from_state_and_stats(state: ConnectionState, connection_stats: ConnectionStats) -> Self {
        let observed_at = Instant::now();
        let connected_since = matches!(state, ConnectionState::Connected)
            .then(|| observed_at.checked_sub(Duration::from_secs(connection_stats.uptime_seconds)))
            .flatten();

        Self {
            state,
            frames_sent: connection_stats.frames_sent,
            frames_received: connection_stats.frames_received,
            connection_errors: connection_stats.connection_errors,
            send_errors: connection_stats.send_errors,
            recv_errors: connection_stats.recv_errors,
            observed_at,
            connected_since,
            last_error: connection_stats.last_error,
            last_error_at: connection_stats.last_error_at,
        }
    }

    /// Uptime of the current connected period at snapshot time.
    #[must_use]
    pub fn uptime(&self) -> Option<Duration> {
        self.connected_since
            .map(|connected_since| self.observed_at.duration_since(connected_since))
    }

    /// Total number of connection, send, and receive errors.
    #[must_use]
    pub fn total_errors(&self) -> u64 {
        self.connection_errors
            .saturating_add(self.send_errors)
            .saturating_add(self.recv_errors)
    }

    /// Health score from 0.0 (not usable) to 1.0 (connected without errors).
    ///
    /// A connected connection is penalized by its observed error ratio while
    /// retaining a non-zero score. Transitional and inactive states have fixed
    /// scores because they cannot currently carry application traffic.
    #[must_use]
    pub fn score(&self) -> f64 {
        match self.state {
            ConnectionState::Failed
            | ConnectionState::Disconnecting
            | ConnectionState::Disconnected => 0.0,
            ConnectionState::Connecting => 0.1,
            ConnectionState::Connected => {
                let errors = self.total_errors();
                let operations = self
                    .frames_sent
                    .saturating_add(self.frames_received)
                    .saturating_add(errors);
                if operations == 0 {
                    return 1.0;
                }

                let error_ratio = errors as f64 / operations as f64;
                (1.0 - error_ratio * 0.9).max(0.1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // `score()` returns literal 0.0/1.0 constants for these states, not a
    // computed ratio, so exact comparison is correct here.
    #[allow(clippy::float_cmp)]
    fn score_reflects_lifecycle_and_connection_local_errors() {
        let disconnected = ConnectionHealth::from_state_and_stats(
            ConnectionState::Disconnected,
            ConnectionStats::default(),
        );
        assert_eq!(disconnected.score(), 0.0);
        assert!(disconnected.uptime().is_none());

        let connected = ConnectionHealth::from_state_and_stats(
            ConnectionState::Connected,
            ConnectionStats {
                frames_sent: 8,
                frames_received: 1,
                send_errors: 1,
                uptime_seconds: 2,
                ..ConnectionStats::default()
            },
        );
        assert!(connected.score() > 0.8);
        assert!(connected.score() < 1.0);
        assert_eq!(connected.total_errors(), 1);
        assert_eq!(connected.uptime(), Some(Duration::from_secs(2)));
        assert!(connected.connected_since.is_some());
    }
}
