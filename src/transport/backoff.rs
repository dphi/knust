//! Jittered exponential backoff for reconnection.

use std::time::Duration;

use rand::RngExt;

use super::BackoffConfig;

use crate::log_transport;
use crate::logging::LogLevel;

/// Exponential backoff with jitter for reconnection delays.
pub struct JitteredBackoff {
    config: BackoffConfig,
    attempt: u32,
}

impl JitteredBackoff {
    #[must_use]
    pub fn new(config: BackoffConfig) -> Self {
        Self { config, attempt: 0 }
    }

    /// Get the next delay with jitter applied (±25% by default).
    pub fn next_delay(&mut self) -> Duration {
        self.attempt += 1;
        let base_ms = (self.config.initial_delay_ms as f64)
            * self.config.multiplier.powi((self.attempt - 1) as i32);
        let capped_ms = base_ms.min(self.config.max_delay_ms as f64);

        // Apply jitter: ±25%
        let jitter_factor = 0.25;
        let mut rng = rand::rng();
        let jitter = rng.random_range((1.0 - jitter_factor)..=(1.0 + jitter_factor));
        let final_ms = (capped_ms * jitter) as u64;

        log_transport!(
            LogLevel::Debug,
            "Backoff: attempt={} delay={}ms",
            self.attempt,
            final_ms
        );

        Duration::from_millis(final_ms)
    }

    /// Reset backoff state (call after successful connection).
    pub fn reset(&mut self) {
        if self.attempt > 0 {
            log_transport!(
                LogLevel::Debug,
                "Backoff: reset after {} attempts",
                self.attempt
            );
        }
        self.attempt = 0;
    }

    /// Check if max attempts have been exhausted.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        let exhausted = self.attempt >= self.config.max_attempts;
        if exhausted {
            log_transport!(
                LogLevel::Warn,
                "Backoff: exhausted after {} attempts (max={})",
                self.attempt,
                self.config.max_attempts
            );
        }
        exhausted
    }

    /// Get current attempt number.
    #[must_use]
    pub fn attempt(&self) -> u32 {
        self.attempt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_increases() {
        let config = BackoffConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            multiplier: 2.0,
            max_attempts: 10,
        };
        let mut backoff = JitteredBackoff::new(config);
        let d1 = backoff.next_delay();
        let d2 = backoff.next_delay();
        // d2 should be roughly 2x d1 (with jitter)
        assert!(d2.as_millis() > d1.as_millis());
    }

    #[test]
    fn test_backoff_caps_at_max() {
        let config = BackoffConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 5000,
            multiplier: 10.0,
            max_attempts: 10,
        };
        let mut backoff = JitteredBackoff::new(config);
        for _ in 0..5 {
            let d = backoff.next_delay();
            assert!(d.as_millis() <= 6250); // 5000 * 1.25 (max jitter)
        }
    }

    #[test]
    fn test_reset() {
        let config = BackoffConfig::default();
        let mut backoff = JitteredBackoff::new(config);
        backoff.next_delay();
        backoff.next_delay();
        assert_eq!(backoff.attempt(), 2);
        backoff.reset();
        assert_eq!(backoff.attempt(), 0);
    }

    #[test]
    fn test_exhausted() {
        let config = BackoffConfig {
            initial_delay_ms: 100,
            max_delay_ms: 1000,
            multiplier: 2.0,
            max_attempts: 3,
        };
        let mut backoff = JitteredBackoff::new(config);
        assert!(!backoff.is_exhausted());
        backoff.next_delay();
        backoff.next_delay();
        backoff.next_delay();
        assert!(backoff.is_exhausted());
    }
}
