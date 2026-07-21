//! Receive-path rate limiting: bounded channel backpressure + per-address throttle.

use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::address::{Address, GroupAddress};
use crate::protocol::telegram::Telegram;

/// Configuration for receive rate limiting.
#[derive(Debug, Clone)]
pub struct ReceiveLimitConfig {
    /// Bounded channel capacity (backpressure threshold)
    pub queue_size: usize,
    /// Max telegrams per second for a single group address
    pub per_address_max_per_sec: u32,
}

impl Default for ReceiveLimitConfig {
    fn default() -> Self {
        Self {
            queue_size: 1000,
            per_address_max_per_sec: 10,
        }
    }
}

/// Result of attempting to send a telegram through the limiter.
#[derive(Debug, PartialEq)]
pub enum ReceiveResult {
    Sent,
    DroppedQueueFull,
    DroppedThrottled(GroupAddress),
}

/// Receive-path rate limiter.
pub struct ReceiveRateLimiter {
    config: ReceiveLimitConfig,
    tx: mpsc::Sender<Telegram>,
    per_address: std::sync::Mutex<HashMap<GroupAddress, AddressState>>,
    stats: std::sync::Mutex<ReceiveStats>,
}

struct AddressState {
    count: u32,
    window_start: Instant,
}

/// Stats for monitoring.
#[derive(Debug, Clone, Default)]
pub struct ReceiveStats {
    pub total_received: u64,
    pub total_sent: u64,
    pub dropped_queue_full: u64,
    pub dropped_throttled: u64,
}

impl ReceiveRateLimiter {
    /// Create a new limiter. Returns (limiter, receiver).
    #[must_use]
    pub fn new(config: ReceiveLimitConfig) -> (Self, mpsc::Receiver<Telegram>) {
        let (tx, rx) = mpsc::channel(config.queue_size);
        let limiter = Self {
            config,
            tx,
            per_address: std::sync::Mutex::new(HashMap::new()),
            stats: std::sync::Mutex::new(ReceiveStats::default()),
        };
        (limiter, rx)
    }

    /// Try to forward a telegram. Returns the result.
    ///
    /// # Panics
    ///
    /// Panics if an internal lock is poisoned.
    pub fn try_send(&self, telegram: Telegram) -> ReceiveResult {
        {
            let mut stats = self.stats.lock().unwrap();
            stats.total_received += 1;
        }

        // Per-address throttle check
        if let Address::Group(ga) = telegram.destination
            && self.is_throttled(ga)
        {
            let mut stats = self.stats.lock().unwrap();
            stats.dropped_throttled += 1;
            log_transport!(
                LogLevel::Trace,
                "Receive throttled: {} exceeds {}/s",
                ga,
                self.config.per_address_max_per_sec
            );
            return ReceiveResult::DroppedThrottled(ga);
        }

        // Bounded channel send
        match self.tx.try_send(telegram) {
            Ok(()) => {
                let mut stats = self.stats.lock().unwrap();
                stats.total_sent += 1;
                ReceiveResult::Sent
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                let mut stats = self.stats.lock().unwrap();
                stats.dropped_queue_full += 1;
                log_transport!(
                    LogLevel::Warn,
                    "Receive queue full ({} capacity), dropping telegram",
                    self.config.queue_size
                );
                ReceiveResult::DroppedQueueFull
            }
            Err(mpsc::error::TrySendError::Closed(_)) => ReceiveResult::DroppedQueueFull,
        }
    }

    /// Get current stats.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn stats(&self) -> ReceiveStats {
        self.stats.lock().unwrap().clone()
    }

    fn is_throttled(&self, ga: GroupAddress) -> bool {
        let mut map = self.per_address.lock().unwrap();
        let now = Instant::now();

        let state = map.entry(ga).or_insert(AddressState {
            count: 0,
            window_start: now,
        });

        // Reset window if >1 second has passed
        if now.duration_since(state.window_start).as_secs() >= 1 {
            state.count = 1;
            state.window_start = now;
            return false;
        }

        state.count += 1;
        state.count > self.config.per_address_max_per_sec
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::address::IndividualAddress;
    use crate::protocol::telegram::TelegramType;

    #[test]
    fn test_bounded_channel_backpressure() {
        let config = ReceiveLimitConfig {
            queue_size: 2,
            per_address_max_per_sec: 100,
        };
        let (limiter, _rx) = ReceiveRateLimiter::new(config);
        let ga = GroupAddress::from_parts(1, 2, 3).unwrap();
        let t = || {
            Telegram::received(
                IndividualAddress::new(1, 1, 5),
                ga,
                TelegramType::GroupValueWrite,
                vec![1],
            )
        };

        assert_eq!(limiter.try_send(t()), ReceiveResult::Sent);
        assert_eq!(limiter.try_send(t()), ReceiveResult::Sent);
        assert_eq!(limiter.try_send(t()), ReceiveResult::DroppedQueueFull);
    }

    #[test]
    fn test_per_address_throttle() {
        let config = ReceiveLimitConfig {
            queue_size: 1000,
            per_address_max_per_sec: 2,
        };
        let (limiter, _rx) = ReceiveRateLimiter::new(config);
        let ga = GroupAddress::from_parts(1, 2, 3).unwrap();
        let t = || {
            Telegram::received(
                IndividualAddress::new(1, 1, 5),
                ga,
                TelegramType::GroupValueWrite,
                vec![1],
            )
        };

        assert_eq!(limiter.try_send(t()), ReceiveResult::Sent);
        assert_eq!(limiter.try_send(t()), ReceiveResult::Sent);
        assert_eq!(limiter.try_send(t()), ReceiveResult::DroppedThrottled(ga));
    }
}
