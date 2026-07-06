//! Telegram queue implementation for ordered processing with priority support.

use crate::error::{Result, TransportError};
use crate::protocol::telegram::Telegram;
use crate::transport::rate_limit::{RateLimitConfig, RateLimiter};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tokio::time::{Duration, Instant};

use crate::log_queue;
use crate::logging::LogLevel;

/// Maximum queue size before backpressure kicks in
const DEFAULT_MAX_QUEUE_SIZE: usize = 1000;

/// Default processing timeout for telegrams
const DEFAULT_PROCESSING_TIMEOUT: Duration = Duration::from_secs(30);

/// Telegram queue with async processing and priority support
pub struct TelegramQueue {
    /// Internal queue state
    state: Arc<Mutex<QueueState>>,

    /// Notification for queue changes
    notify: Arc<Notify>,

    /// Configuration
    config: QueueConfig,

    /// Rate limiter for outgoing telegrams
    rate_limiter: Arc<Mutex<RateLimiter>>,
}

/// Internal queue state
struct QueueState {
    /// Priority queue for outgoing telegrams
    outgoing: BinaryHeap<PriorityTelegram>,

    /// FIFO queue for incoming telegrams (maintain strict ordering)
    incoming: VecDeque<Telegram>,

    /// Queue statistics
    stats: QueueStats,

    /// Whether the queue is closed
    closed: bool,
}

/// Wrapper for telegrams with priority ordering
#[derive(Debug)]
struct PriorityTelegram {
    telegram: Telegram,
    sequence: u64,
    enqueued_at: Instant,
}

impl PartialEq for PriorityTelegram {
    fn eq(&self, other: &Self) -> bool {
        self.telegram.priority == other.telegram.priority && self.sequence == other.sequence
    }
}

impl Eq for PriorityTelegram {}

impl PartialOrd for PriorityTelegram {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PriorityTelegram {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority values should be processed first (reverse order for BinaryHeap)
        // If priorities are equal, use sequence number for FIFO within priority
        match other.telegram.priority.cmp(&self.telegram.priority) {
            Ordering::Equal => other.sequence.cmp(&self.sequence),
            other_order => other_order,
        }
    }
}

/// Queue configuration
#[derive(Debug, Clone)]
pub struct QueueConfig {
    /// Maximum number of telegrams in queue
    pub max_size: usize,

    /// Processing timeout for telegrams
    pub processing_timeout: Duration,

    /// Enable strict ordering for incoming telegrams
    pub strict_incoming_order: bool,

    /// Enable priority processing for outgoing telegrams
    pub priority_processing: bool,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            max_size: DEFAULT_MAX_QUEUE_SIZE,
            processing_timeout: DEFAULT_PROCESSING_TIMEOUT,
            strict_incoming_order: true,
            priority_processing: true,
        }
    }
}

/// Queue statistics
#[derive(Debug, Clone, Default)]
pub struct QueueStats {
    /// Total telegrams enqueued
    pub total_enqueued: u64,

    /// Total telegrams dequeued
    pub total_dequeued: u64,

    /// Current queue size
    pub current_size: usize,

    /// Peak queue size
    pub peak_size: usize,

    /// Number of dropped telegrams due to backpressure
    pub dropped_count: u64,

    /// Average processing time
    pub avg_processing_time: Duration,

    /// Sequence counter for ordering
    pub sequence_counter: u64,
}

impl TelegramQueue {
    /// Create a new telegram queue with default configuration
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(QueueConfig::default())
    }

    /// Create a new telegram queue with custom configuration
    #[must_use]
    pub fn with_config(config: QueueConfig) -> Self {
        let state = QueueState {
            outgoing: BinaryHeap::new(),
            incoming: VecDeque::new(),
            stats: QueueStats::default(),
            closed: false,
        };

        Self {
            state: Arc::new(Mutex::new(state)),
            notify: Arc::new(Notify::new()),
            config,
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new(RateLimitConfig::default()))),
        }
    }

    /// Enqueue an outgoing telegram (with priority handling)
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::QueueClosed`] if the queue has been closed,
    /// or [`TransportError::QueueFull`] if it is at capacity.
    pub async fn enqueue_outgoing(&self, telegram: Telegram) -> Result<()> {
        let mut state = self.state.lock().await;

        if state.closed {
            return Err(TransportError::QueueClosed.into());
        }

        // Check for backpressure
        if state.outgoing.len() >= self.config.max_size {
            log_queue!(
                LogLevel::Warn,
                "Outgoing dropped: queue full ({}/{})",
                state.outgoing.len(),
                self.config.max_size
            );
            state.stats.dropped_count += 1;
            return Err(TransportError::QueueFull.into());
        }

        let log_dest = format!("{}", telegram.destination);
        let log_prio = telegram.priority;

        // Create priority telegram with sequence number
        let priority_telegram = PriorityTelegram {
            telegram,
            sequence: state.stats.sequence_counter,
            enqueued_at: Instant::now(),
        };

        state.stats.sequence_counter += 1;
        state.stats.total_enqueued += 1;

        if self.config.priority_processing {
            state.outgoing.push(priority_telegram);
        } else {
            // If priority processing is disabled, treat as FIFO
            state.outgoing.push(priority_telegram);
        }

        state.stats.current_size = state.outgoing.len() + state.incoming.len();
        if state.stats.current_size > state.stats.peak_size {
            state.stats.peak_size = state.stats.current_size;
        }

        log_queue!(
            LogLevel::Debug,
            "Outgoing enqueued: priority={:?} dest={} queue_size={}",
            log_prio,
            log_dest,
            state.outgoing.len()
        );

        drop(state);
        self.notify.notify_waiters();

        Ok(())
    }

    /// Enqueue an incoming telegram (strict FIFO ordering)
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::QueueClosed`] if the queue has been closed,
    /// or [`TransportError::QueueFull`] if it is at capacity.
    pub async fn enqueue_incoming(&self, telegram: Telegram) -> Result<()> {
        let mut state = self.state.lock().await;

        if state.closed {
            return Err(TransportError::QueueClosed.into());
        }

        // Check for backpressure
        if state.incoming.len() >= self.config.max_size {
            log_queue!(
                LogLevel::Warn,
                "Incoming dropped: queue full ({}/{})",
                state.incoming.len(),
                self.config.max_size
            );
            state.stats.dropped_count += 1;
            return Err(TransportError::QueueFull.into());
        }

        let log_src = format!("{}", telegram.source);
        let log_dest = format!("{}", telegram.destination);

        state.incoming.push_back(telegram);
        state.stats.total_enqueued += 1;
        state.stats.current_size = state.outgoing.len() + state.incoming.len();

        if state.stats.current_size > state.stats.peak_size {
            state.stats.peak_size = state.stats.current_size;
        }

        log_queue!(
            LogLevel::Debug,
            "Incoming enqueued: src={} dest={} queue_size={}",
            log_src,
            log_dest,
            state.incoming.len()
        );

        drop(state);
        self.notify.notify_waiters();

        Ok(())
    }

    /// Dequeue the next outgoing telegram (priority-based).
    ///
    /// Returns `None` once the queue is closed and drained.
    pub async fn dequeue_outgoing(&self) -> Option<Telegram> {
        loop {
            let mut state = self.state.lock().await;
            if let Some(priority_telegram) = state.outgoing.pop() {
                state.stats.total_dequeued += 1;
                state.stats.current_size = state.outgoing.len() + state.incoming.len();

                let processing_time = priority_telegram.enqueued_at.elapsed();
                Self::update_avg_processing_time(&mut state.stats, processing_time);

                log_queue!(
                    LogLevel::Debug,
                    "Outgoing dequeued: dest={} wait={:?} remaining={}",
                    priority_telegram.telegram.destination,
                    processing_time,
                    state.outgoing.len()
                );
                return Some(priority_telegram.telegram);
            }

            if state.closed {
                return None;
            }

            drop(state);
            self.notify.notified().await;
        }
    }

    /// Dequeue the next incoming telegram (FIFO order).
    ///
    /// Returns `None` once the queue is closed and drained.
    pub async fn dequeue_incoming(&self) -> Option<Telegram> {
        loop {
            let mut state = self.state.lock().await;
            if let Some(telegram) = state.incoming.pop_front() {
                state.stats.total_dequeued += 1;
                state.stats.current_size = state.outgoing.len() + state.incoming.len();
                log_queue!(
                    LogLevel::Debug,
                    "Incoming dequeued: src={} dest={} remaining={}",
                    telegram.source,
                    telegram.destination,
                    state.incoming.len()
                );
                return Some(telegram);
            }

            if state.closed {
                return None;
            }

            drop(state);
            self.notify.notified().await;
        }
    }

    /// Get current queue statistics
    pub async fn stats(&self) -> QueueStats {
        let state = self.state.lock().await;
        state.stats.clone()
    }

    /// Check if the queue is empty
    pub async fn is_empty(&self) -> bool {
        let state = self.state.lock().await;
        state.outgoing.is_empty() && state.incoming.is_empty()
    }

    /// Get current queue size
    pub async fn len(&self) -> usize {
        let state = self.state.lock().await;
        state.outgoing.len() + state.incoming.len()
    }

    /// Close the queue (no more telegrams can be enqueued)
    pub async fn close(&self) {
        let mut state = self.state.lock().await;
        state.closed = true;
        drop(state);
        self.notify.notify_waiters();
    }

    /// Check if the queue is closed
    pub async fn is_closed(&self) -> bool {
        let state = self.state.lock().await;
        state.closed
    }

    /// Clear all telegrams from the queue
    pub async fn clear(&self) {
        let mut state = self.state.lock().await;
        state.outgoing.clear();
        state.incoming.clear();
        state.stats.current_size = 0;
    }

    /// Acquire a send token from the rate limiter, waiting if necessary.
    pub async fn acquire_send_token(&self) {
        self.rate_limiter.lock().await.acquire().await;
    }

    /// Update average processing time
    fn update_avg_processing_time(stats: &mut QueueStats, new_time: Duration) {
        if stats.total_dequeued == 1 {
            stats.avg_processing_time = new_time;
        } else {
            // Simple moving average
            let alpha = 0.1; // Weight for new sample
            let new_millis = new_time.as_millis() as f64;
            let current_millis = stats.avg_processing_time.as_millis() as f64;
            let updated_millis = (alpha * new_millis) + ((1.0 - alpha) * current_millis);
            stats.avg_processing_time = Duration::from_millis(updated_millis as u64);
        }
    }
}

impl Default for TelegramQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::address::{Address, GroupAddress, IndividualAddress};
    use crate::protocol::telegram::{Direction, Priority, TelegramType};
    use proptest::prelude::*;

    fn create_test_telegram(priority: Priority) -> Telegram {
        Telegram {
            source: IndividualAddress::new(1, 1, 1),
            destination: Address::Group(
                GroupAddress::try_from_raw(0x0101).expect("Valid test address"),
            ),
            payload: vec![0x01, 0x02, 0x03],
            priority,
            direction: Direction::Outgoing,
            telegram_type: TelegramType::GroupValueWrite,
            gateway_id: None,
            timestamp: std::time::SystemTime::now(),
        }
    }

    // Property-based test generators
    prop_compose! {
        fn arb_priority()(priority in 0u8..4) -> Priority {
            Priority::from_u8(priority)
        }
    }

    prop_compose! {
        fn arb_telegram()(
            priority in arb_priority(),
            source_area in 0u8..16,
            source_line in 0u8..16,
            source_device in 0u8..255,
            dest_addr in 0u16..=GroupAddress::MAX_RAW,
            payload_len in 0usize..20,
            payload_byte in 0u8..255,
        ) -> Telegram {
            let payload = vec![payload_byte; payload_len];
            Telegram {
                source: IndividualAddress::new(source_area, source_line, source_device),
                destination: Address::Group(GroupAddress::try_from_raw(dest_addr).expect("Valid test address")),
                payload,
                priority,
                direction: Direction::Outgoing,
                telegram_type: TelegramType::GroupValueWrite,
                gateway_id: None,
                timestamp: std::time::SystemTime::now(),
            }
        }
    }

    proptest! {
        #[test]
        fn prop_telegram_queue_ordering(telegrams in prop::collection::vec(arb_telegram(), 1..50)) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let queue = TelegramQueue::new();

                // Enqueue all telegrams in order
                for telegram in &telegrams {
                    queue.enqueue_incoming(telegram.clone()).await.unwrap();
                }

                // Dequeue all telegrams and verify they come out in the same order
                let mut dequeued = Vec::new();
                for _ in 0..telegrams.len() {
                    if let Some(telegram) = queue.dequeue_incoming().await {
                        dequeued.push(telegram);
                    }
                }

                // Verify ordering is preserved for incoming queue (FIFO)
                prop_assert_eq!(dequeued.len(), telegrams.len());
                for (original, dequeued_telegram) in telegrams.iter().zip(dequeued.iter()) {
                    prop_assert_eq!(original.source, dequeued_telegram.source);
                    prop_assert_eq!(original.destination, dequeued_telegram.destination);
                    prop_assert_eq!(&original.payload, &dequeued_telegram.payload);
                    prop_assert_eq!(original.priority, dequeued_telegram.priority);
                }

                Ok(())
            })?;
        }

        #[test]
        fn prop_outgoing_priority_ordering(telegrams in prop::collection::vec(arb_telegram(), 1..50)) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let queue = TelegramQueue::new();

                // Enqueue all telegrams
                for telegram in &telegrams {
                    queue.enqueue_outgoing(telegram.clone()).await.unwrap();
                }

                // Dequeue all telegrams
                let mut dequeued = Vec::new();
                for _ in 0..telegrams.len() {
                    if let Some(telegram) = queue.dequeue_outgoing().await {
                        dequeued.push(telegram);
                    }
                }

                // Verify priority ordering: each telegram should have priority >= next telegram
                prop_assert_eq!(dequeued.len(), telegrams.len());
                for window in dequeued.windows(2) {
                    let current_priority = window[0].priority;
                    let next_priority = window[1].priority;
                    prop_assert!(current_priority <= next_priority,
                        "Priority ordering violated: {:?} should come before or equal to {:?}",
                        current_priority, next_priority);
                }

                Ok(())
            })?;
        }

        #[test]
        fn prop_queue_backpressure_handling(telegrams in prop::collection::vec(arb_telegram(), 1..20)) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let config = QueueConfig {
                    max_size: 5, // Small queue to trigger backpressure
                    ..Default::default()
                };
                let queue = TelegramQueue::with_config(config);

                let mut enqueued_count = 0;
                let mut dropped_count = 0;

                // Try to enqueue all telegrams
                for telegram in &telegrams {
                    match queue.enqueue_outgoing(telegram.clone()).await {
                        Ok(()) => enqueued_count += 1,
                        Err(_) => dropped_count += 1,
                    }
                }

                // Verify backpressure behavior
                prop_assert!(enqueued_count <= 5, "Should not enqueue more than max_size");
                prop_assert_eq!(enqueued_count + dropped_count, telegrams.len());

                let stats = queue.stats().await;
                prop_assert_eq!(stats.dropped_count, dropped_count as u64);
                prop_assert_eq!(stats.total_enqueued, enqueued_count as u64);

                Ok(())
            })?;
        }
    }

    #[tokio::test]
    async fn test_basic_enqueue_dequeue() {
        let queue = TelegramQueue::new();
        let telegram = create_test_telegram(Priority::Normal);

        // Test outgoing queue
        queue.enqueue_outgoing(telegram.clone()).await.unwrap();
        let dequeued = queue.dequeue_outgoing().await.unwrap();
        assert_eq!(dequeued.priority, telegram.priority);

        // Test incoming queue
        queue.enqueue_incoming(telegram.clone()).await.unwrap();
        let dequeued = queue.dequeue_incoming().await.unwrap();
        assert_eq!(dequeued.priority, telegram.priority);
    }

    #[tokio::test]
    async fn test_incoming_notification_does_not_stop_outgoing_waiter() {
        let queue = Arc::new(TelegramQueue::new());
        let incoming = create_test_telegram(Priority::Normal);
        let outgoing = create_test_telegram(Priority::Urgent);

        let outgoing_queue = queue.clone();
        let outgoing_waiter = tokio::spawn(async move { outgoing_queue.dequeue_outgoing().await });

        let incoming_queue = queue.clone();
        let incoming_waiter = tokio::spawn(async move { incoming_queue.dequeue_incoming().await });

        tokio::time::sleep(Duration::from_millis(10)).await;
        queue.enqueue_incoming(incoming.clone()).await.unwrap();

        let received_incoming = tokio::time::timeout(Duration::from_secs(1), incoming_waiter)
            .await
            .expect("incoming waiter should be notified")
            .expect("incoming waiter should not panic")
            .expect("incoming telegram should be present");
        assert_eq!(received_incoming.payload, incoming.payload);

        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            !outgoing_waiter.is_finished(),
            "wrong notification must not make outgoing waiter return None"
        );

        queue.enqueue_outgoing(outgoing.clone()).await.unwrap();
        let received_outgoing = tokio::time::timeout(Duration::from_secs(1), outgoing_waiter)
            .await
            .expect("outgoing waiter should be notified")
            .expect("outgoing waiter should not panic")
            .expect("outgoing telegram should be present");
        assert_eq!(received_outgoing.priority, outgoing.priority);
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        let queue = TelegramQueue::new();

        // Enqueue telegrams in reverse priority order
        let low = create_test_telegram(Priority::Low);
        let normal = create_test_telegram(Priority::Normal);
        let urgent = create_test_telegram(Priority::Urgent);
        let system = create_test_telegram(Priority::System);

        queue.enqueue_outgoing(low).await.unwrap();
        queue.enqueue_outgoing(normal).await.unwrap();
        queue.enqueue_outgoing(urgent).await.unwrap();
        queue.enqueue_outgoing(system).await.unwrap();

        // Should dequeue in priority order: System, Urgent, Normal, Low
        assert_eq!(
            queue.dequeue_outgoing().await.unwrap().priority,
            Priority::System
        );
        assert_eq!(
            queue.dequeue_outgoing().await.unwrap().priority,
            Priority::Urgent
        );
        assert_eq!(
            queue.dequeue_outgoing().await.unwrap().priority,
            Priority::Normal
        );
        assert_eq!(
            queue.dequeue_outgoing().await.unwrap().priority,
            Priority::Low
        );
    }

    #[tokio::test]
    async fn test_fifo_ordering_within_priority() {
        let queue = TelegramQueue::new();

        // Enqueue multiple telegrams with same priority
        let telegram1 = create_test_telegram(Priority::Normal);
        let telegram2 = create_test_telegram(Priority::Normal);
        let telegram3 = create_test_telegram(Priority::Normal);

        queue.enqueue_outgoing(telegram1).await.unwrap();
        queue.enqueue_outgoing(telegram2).await.unwrap();
        queue.enqueue_outgoing(telegram3).await.unwrap();

        // Should maintain FIFO order within same priority
        let stats_before = queue.stats().await;
        assert_eq!(stats_before.total_enqueued, 3);

        queue.dequeue_outgoing().await.unwrap();
        queue.dequeue_outgoing().await.unwrap();
        queue.dequeue_outgoing().await.unwrap();

        let stats_after = queue.stats().await;
        assert_eq!(stats_after.total_dequeued, 3);
    }

    #[tokio::test]
    async fn test_backpressure() {
        let config = QueueConfig {
            max_size: 2,
            ..Default::default()
        };
        let queue = TelegramQueue::with_config(config);

        let telegram = create_test_telegram(Priority::Normal);

        // Fill queue to capacity
        queue.enqueue_outgoing(telegram.clone()).await.unwrap();
        queue.enqueue_outgoing(telegram.clone()).await.unwrap();

        // Next enqueue should fail due to backpressure
        let result = queue.enqueue_outgoing(telegram).await;
        assert!(result.is_err());

        let stats = queue.stats().await;
        assert_eq!(stats.dropped_count, 1);
    }

    #[tokio::test]
    async fn test_queue_closure() {
        let queue = TelegramQueue::new();
        let telegram = create_test_telegram(Priority::Normal);

        queue.enqueue_outgoing(telegram.clone()).await.unwrap();
        queue.close().await;

        // Should not be able to enqueue after closure
        let result = queue.enqueue_outgoing(telegram).await;
        assert!(result.is_err());

        // Should still be able to dequeue existing telegrams
        let dequeued = queue.dequeue_outgoing().await;
        assert!(dequeued.is_some());

        // Next dequeue should return None (queue closed and empty)
        let dequeued = queue.dequeue_outgoing().await;
        assert!(dequeued.is_none());
    }

    #[tokio::test]
    async fn test_incoming_fifo_order() {
        let queue = TelegramQueue::new();

        let telegram1 = create_test_telegram(Priority::System);
        let telegram2 = create_test_telegram(Priority::Low);
        let telegram3 = create_test_telegram(Priority::Urgent);

        // Enqueue in mixed priority order
        queue.enqueue_incoming(telegram1).await.unwrap();
        queue.enqueue_incoming(telegram2).await.unwrap();
        queue.enqueue_incoming(telegram3).await.unwrap();

        // Should dequeue in FIFO order regardless of priority
        assert_eq!(
            queue.dequeue_incoming().await.unwrap().priority,
            Priority::System
        );
        assert_eq!(
            queue.dequeue_incoming().await.unwrap().priority,
            Priority::Low
        );
        assert_eq!(
            queue.dequeue_incoming().await.unwrap().priority,
            Priority::Urgent
        );
    }
}
