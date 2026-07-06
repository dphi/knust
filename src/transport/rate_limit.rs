//! Token-bucket rate limiter for outgoing KNX telegrams.

use std::time::Instant;
use tokio::time::{Duration, sleep};

use crate::log_transport;
use crate::logging::LogLevel;

/// Configuration for rate limiting.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum telegrams per second
    pub max_per_second: u32,
    /// Burst allowance (tokens above steady-state)
    pub burst: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_per_second: 50, // KNX TP1 bus limit
            burst: 10,
        }
    }
}

/// Token-bucket rate limiter.
pub struct RateLimiter {
    config: RateLimitConfig,
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        let tokens = f64::from(config.burst);
        Self {
            config,
            tokens,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens based on elapsed time.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens += elapsed * f64::from(self.config.max_per_second);
        let max_tokens = f64::from(self.config.max_per_second + self.config.burst);
        if self.tokens > max_tokens {
            self.tokens = max_tokens;
        }
        self.last_refill = now;
    }

    /// Try to acquire a token. Returns true if available, false if rate-limited.
    pub fn try_acquire(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            log_transport!(
                LogLevel::Trace,
                "Rate limiter: token acquired, remaining={:.1}",
                self.tokens
            );
            true
        } else {
            log_transport!(
                LogLevel::Warn,
                "Rate limiter: telegram throttled, tokens={:.1}/{}",
                self.tokens,
                self.config.max_per_second + self.config.burst
            );
            false
        }
    }

    /// Acquire a token, waiting if necessary.
    pub async fn acquire(&mut self) {
        loop {
            self.refill();
            if self.tokens >= 1.0 {
                self.tokens -= 1.0;
                log_transport!(
                    LogLevel::Trace,
                    "Rate limiter: token acquired (async), remaining={:.1}",
                    self.tokens
                );
                return;
            }
            let wait_ms =
                ((1.0 - self.tokens) / f64::from(self.config.max_per_second) * 1000.0) as u64;
            log_transport!(
                LogLevel::Debug,
                "Rate limiter: waiting {}ms for token",
                wait_ms.max(1)
            );
            sleep(Duration::from_millis(wait_ms.max(1))).await;
        }
    }

    /// Current token count.
    #[must_use]
    pub fn available_tokens(&self) -> f64 {
        self.tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_burst_tokens_available() {
        let mut limiter = RateLimiter::new(RateLimitConfig {
            max_per_second: 10,
            burst: 5,
        });
        // Should be able to acquire burst amount immediately
        for _ in 0..5 {
            assert!(limiter.try_acquire());
        }
    }

    #[test]
    fn test_rate_limit_kicks_in() {
        let mut limiter = RateLimiter::new(RateLimitConfig {
            max_per_second: 10,
            burst: 2,
        });
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        // Third immediate acquire should fail (burst exhausted, no time elapsed)
        assert!(!limiter.try_acquire());
    }
}
