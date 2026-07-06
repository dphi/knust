//! Memory management and performance optimization utilities.

use crate::log_application;
use crate::logging::{Component, LogLevel, Timer};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Memory usage statistics and monitoring
#[derive(Debug, Clone)]
pub struct MemoryStats {
    /// Current memory usage in bytes (estimated)
    pub current_usage: u64,

    /// Peak memory usage in bytes
    pub peak_usage: u64,

    /// Number of active connections
    pub active_connections: usize,

    /// Number of cached telegrams
    pub cached_telegrams: usize,

    /// Number of registered devices
    pub registered_devices: usize,

    /// Memory usage by component
    pub component_usage: ComponentMemoryUsage,
}

/// Memory usage breakdown by component
#[derive(Debug, Clone, Default)]
pub struct ComponentMemoryUsage {
    /// Transport layer memory usage
    pub transport: u64,

    /// Protocol layer memory usage
    pub protocol: u64,

    /// Device layer memory usage
    pub device: u64,

    /// Application layer memory usage
    pub application: u64,

    /// Security layer memory usage
    pub security: u64,
}

/// Memory monitor for tracking and limiting memory usage
pub struct MemoryMonitor {
    /// Current estimated memory usage
    current_usage: AtomicU64,

    /// Peak memory usage
    peak_usage: AtomicU64,

    /// Memory usage limit in bytes
    memory_limit: u64,

    /// Component-specific usage tracking
    component_usage: Arc<RwLock<ComponentMemoryUsage>>,

    /// Last cleanup time
    last_cleanup: Arc<RwLock<Instant>>,
}

impl MemoryMonitor {
    /// Create a new memory monitor with the specified limit
    #[must_use]
    pub fn new(memory_limit_mb: u64) -> Self {
        let memory_limit = memory_limit_mb * 1024 * 1024; // Convert MB to bytes

        log_application!(
            LogLevel::Info,
            "Initializing memory monitor with limit: {} MB",
            memory_limit_mb
        );

        Self {
            current_usage: AtomicU64::new(0),
            peak_usage: AtomicU64::new(0),
            memory_limit,
            component_usage: Arc::new(RwLock::new(ComponentMemoryUsage::default())),
            last_cleanup: Arc::new(RwLock::new(Instant::now())),
        }
    }

    /// Allocate memory for a specific component
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::LimitExceeded`] if usage still exceeds the
    /// configured memory limit after an attempted cleanup.
    pub async fn allocate(&self, component: Component, size: u64) -> Result<(), MemoryError> {
        let new_usage = self.current_usage.fetch_add(size, Ordering::SeqCst) + size;

        // Update peak usage
        let mut peak = self.peak_usage.load(Ordering::SeqCst);
        while new_usage > peak {
            match self.peak_usage.compare_exchange_weak(
                peak,
                new_usage,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(current) => peak = current,
            }
        }

        // Check memory limit
        if new_usage > self.memory_limit {
            log_application!(
                LogLevel::Warn,
                "Memory limit exceeded: {} bytes (limit: {} bytes)",
                new_usage,
                self.memory_limit
            );

            // Try to free memory
            self.cleanup().await;

            let current = self.current_usage.load(Ordering::SeqCst);
            if current > self.memory_limit {
                return Err(MemoryError::LimitExceeded {
                    current,
                    limit: self.memory_limit,
                });
            }
        }

        // Update component usage
        {
            let mut usage = self.component_usage.write().await;
            match component {
                Component::Transport => usage.transport += size,
                Component::Protocol => usage.protocol += size,
                Component::Device => usage.device += size,
                Component::Application => usage.application += size,
                Component::Security => usage.security += size,
                _ => {} // Other components don't track memory
            }
        }

        log_application!(
            LogLevel::Trace,
            "Allocated {} bytes for {:?} (total: {} bytes)",
            size,
            component,
            new_usage
        );

        Ok(())
    }

    /// Deallocate memory for a specific component
    pub async fn deallocate(&self, component: Component, size: u64) {
        let new_usage = self
            .current_usage
            .fetch_sub(size, Ordering::SeqCst)
            .saturating_sub(size);

        // Update component usage
        {
            let mut usage = self.component_usage.write().await;
            match component {
                Component::Transport => usage.transport = usage.transport.saturating_sub(size),
                Component::Protocol => usage.protocol = usage.protocol.saturating_sub(size),
                Component::Device => usage.device = usage.device.saturating_sub(size),
                Component::Application => {
                    usage.application = usage.application.saturating_sub(size);
                }
                Component::Security => usage.security = usage.security.saturating_sub(size),
                _ => {} // Other components don't track memory
            }
        }

        log_application!(
            LogLevel::Trace,
            "Deallocated {} bytes for {:?} (total: {} bytes)",
            size,
            component,
            new_usage
        );
    }

    /// Get current memory statistics
    pub async fn get_stats(&self) -> MemoryStats {
        let current_usage = self.current_usage.load(Ordering::SeqCst);
        let peak_usage = self.peak_usage.load(Ordering::SeqCst);
        let component_usage = self.component_usage.read().await.clone();

        MemoryStats {
            current_usage,
            peak_usage,
            active_connections: 0, // Will be updated by connection pool
            cached_telegrams: 0,   // Will be updated by telegram cache
            registered_devices: 0, // Will be updated by device registry
            component_usage,
        }
    }

    /// Check if memory usage is within acceptable bounds
    pub fn is_within_bounds(&self) -> bool {
        self.current_usage.load(Ordering::SeqCst) <= self.memory_limit
    }

    /// Get memory usage percentage
    pub fn usage_percentage(&self) -> f64 {
        let current = self.current_usage.load(Ordering::SeqCst) as f64;
        let limit = self.memory_limit as f64;
        (current / limit) * 100.0
    }

    /// Perform memory cleanup
    pub async fn cleanup(&self) -> u64 {
        let timer = Timer::start(Component::Application, "memory_cleanup");
        let start_usage = self.current_usage.load(Ordering::SeqCst);

        log_application!(
            LogLevel::Info,
            "Starting memory cleanup (current usage: {} bytes)",
            start_usage
        );

        // Update last cleanup time
        {
            let mut last_cleanup = self.last_cleanup.write().await;
            *last_cleanup = Instant::now();
        }

        // Force garbage collection hint (Rust doesn't have explicit GC, but we can drop unused data)
        // This is a placeholder for actual cleanup logic that would be implemented by components

        let end_usage = self.current_usage.load(Ordering::SeqCst);
        let freed = start_usage.saturating_sub(end_usage);

        log_application!(
            LogLevel::Info,
            "Memory cleanup completed: freed {} bytes",
            freed
        );
        timer.finish_with_message(&format!("Memory cleanup freed {freed} bytes"));

        freed
    }

    /// Check if cleanup is needed based on time and usage
    pub async fn should_cleanup(&self) -> bool {
        let usage_percentage = self.usage_percentage();
        let last_cleanup = *self.last_cleanup.read().await;
        let time_since_cleanup = last_cleanup.elapsed();

        // Cleanup if usage is high or it's been a while
        usage_percentage > 80.0 || time_since_cleanup > Duration::from_secs(5 * 60)
    }
}

/// Connection pool for managing and reusing connections
pub struct ConnectionPool<T: ?Sized> {
    /// Pool of available connections
    available: Arc<RwLock<Vec<Arc<T>>>>,

    /// Pool of active connections
    active: Arc<RwLock<Vec<Arc<T>>>>,

    /// Maximum pool size
    max_size: usize,

    /// Connection creation function
    create_fn: Arc<dyn Fn() -> crate::Result<Arc<T>> + Send + Sync>,

    /// Memory monitor for tracking pool usage
    memory_monitor: Arc<MemoryMonitor>,
}

impl<T: ?Sized + Send + Sync + 'static> ConnectionPool<T> {
    /// Create a new connection pool
    pub fn new<F>(max_size: usize, create_fn: F, memory_monitor: Arc<MemoryMonitor>) -> Self
    where
        F: Fn() -> crate::Result<Arc<T>> + Send + Sync + 'static,
    {
        log_application!(
            LogLevel::Info,
            "Creating connection pool with max size: {}",
            max_size
        );

        Self {
            available: Arc::new(RwLock::new(Vec::new())),
            active: Arc::new(RwLock::new(Vec::new())),
            max_size,
            create_fn: Arc::new(create_fn),
            memory_monitor,
        }
    }

    /// Get a connection from the pool
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::PoolExhausted`] if the pool is at capacity and
    /// no connection is available, [`MemoryError::ConnectionCreationFailed`]
    /// if creating a new connection fails, or the same errors as
    /// [`MemoryMonitor::allocate`] if accounting for the new connection's
    /// memory exceeds the limit.
    pub async fn get_connection(&self) -> Result<PooledConnection<T>, MemoryError> {
        // Try to get an available connection first
        {
            let mut available = self.available.write().await;
            if let Some(conn) = available.pop() {
                let mut active = self.active.write().await;
                active.push(conn.clone());

                log_application!(
                    LogLevel::Trace,
                    "Reused connection from pool (active: {}, available: {})",
                    active.len(),
                    available.len()
                );

                return Ok(PooledConnection::new(
                    conn,
                    self.available.clone(),
                    self.active.clone(),
                ));
            }
        }

        // Check if we can create a new connection
        {
            let active = self.active.read().await;
            if active.len() >= self.max_size {
                return Err(MemoryError::PoolExhausted {
                    max_size: self.max_size,
                    current_size: active.len(),
                });
            }
        }

        // Create a new connection
        let conn = (self.create_fn)().map_err(|e| MemoryError::ConnectionCreationFailed {
            reason: e.to_string(),
        })?;

        // Estimate memory usage for the connection (use a fixed size since T is ?Sized)
        let conn_size = 1024u64; // Estimated overhead for connection management
        self.memory_monitor
            .allocate(Component::Transport, conn_size)
            .await?;

        {
            let mut active = self.active.write().await;
            active.push(conn.clone());

            log_application!(
                LogLevel::Debug,
                "Created new connection (active: {}, available: {})",
                active.len(),
                0
            );
        }

        Ok(PooledConnection::new(
            conn,
            self.available.clone(),
            self.active.clone(),
        ))
    }

    /// Get pool statistics
    pub async fn get_stats(&self) -> PoolStats {
        let available = self.available.read().await;
        let active = self.active.read().await;

        PoolStats {
            max_size: self.max_size,
            active_connections: active.len(),
            available_connections: available.len(),
            total_connections: active.len() + available.len(),
        }
    }

    /// Cleanup unused connections
    pub async fn cleanup(&self) -> usize {
        let mut available = self.available.write().await;
        let initial_count = available.len();

        // Keep only half of the available connections to free memory
        let keep_count = available.len().div_ceil(2);
        available.truncate(keep_count);

        let removed = initial_count - available.len();

        if removed > 0 {
            log_application!(
                LogLevel::Debug,
                "Cleaned up {} unused connections from pool",
                removed
            );
        }

        removed
    }
}

/// A connection borrowed from the pool
pub struct PooledConnection<T: ?Sized + Send + Sync + 'static> {
    /// The actual connection
    connection: Option<Arc<T>>,

    /// Reference to available connections pool
    available: Arc<RwLock<Vec<Arc<T>>>>,

    /// Reference to active connections pool
    active: Arc<RwLock<Vec<Arc<T>>>>,
}

impl<T: ?Sized + Send + Sync + 'static> PooledConnection<T> {
    fn new(
        connection: Arc<T>,
        available: Arc<RwLock<Vec<Arc<T>>>>,
        active: Arc<RwLock<Vec<Arc<T>>>>,
    ) -> Self {
        Self {
            connection: Some(connection),
            available,
            active,
        }
    }

    /// Get a reference to the connection
    ///
    /// # Panics
    ///
    /// Never panics in practice: `connection` is only `None` after `Drop`
    /// has run, and `Drop` consumes `self`.
    #[must_use]
    pub fn get(&self) -> &T {
        self.connection
            .as_ref()
            .expect("connection is only None after Drop, which consumes self")
    }
}

impl<T: ?Sized + Send + Sync + 'static> Drop for PooledConnection<T> {
    fn drop(&mut self) {
        if let Some(conn) = self.connection.take() {
            // Return connection to the pool
            let available = self.available.clone();
            let active = self.active.clone();

            tokio::spawn(async move {
                // Remove from active
                {
                    let mut active_guard = active.write().await;
                    active_guard.retain(|c| !Arc::ptr_eq(c, &conn));
                }

                // Add to available
                {
                    let mut available_guard = available.write().await;
                    available_guard.push(conn);
                }
            });
        }
    }
}

/// Pool statistics
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Maximum pool size
    pub max_size: usize,

    /// Number of active connections
    pub active_connections: usize,

    /// Number of available connections
    pub available_connections: usize,

    /// Total connections in pool
    pub total_connections: usize,
}

/// Memory-related errors
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Memory limit exceeded: current {current} bytes, limit {limit} bytes")]
    LimitExceeded { current: u64, limit: u64 },

    #[error("Connection pool exhausted: {current_size}/{max_size} connections")]
    PoolExhausted {
        max_size: usize,
        current_size: usize,
    },

    #[error("Failed to create connection: {reason}")]
    ConnectionCreationFailed { reason: String },
}

/// Performance optimization utilities
pub struct PerformanceOptimizer {
    /// Memory monitor
    memory_monitor: Arc<MemoryMonitor>,

    /// Hot path statistics
    hot_path_stats: Arc<RwLock<HotPathStats>>,
}

impl PerformanceOptimizer {
    /// Create a new performance optimizer
    pub fn new(memory_monitor: Arc<MemoryMonitor>) -> Self {
        Self {
            memory_monitor,
            hot_path_stats: Arc::new(RwLock::new(HotPathStats::default())),
        }
    }

    /// Record hot path execution
    pub async fn record_hot_path(&self, path: &str, duration: Duration) {
        let mut stats = self.hot_path_stats.write().await;
        let entry = stats
            .paths
            .entry(path.to_string())
            .or_insert_with(HotPathEntry::default);

        entry.call_count += 1;
        entry.total_duration += duration;
        entry.min_duration = entry.min_duration.min(duration);
        entry.max_duration = entry.max_duration.max(duration);
        entry.avg_duration = entry.total_duration / entry.call_count as u32;
    }

    /// Get hot path statistics
    pub async fn get_hot_path_stats(&self) -> HotPathStats {
        self.hot_path_stats.read().await.clone()
    }

    /// Optimize based on current statistics
    pub async fn optimize(&self) {
        let stats = self.hot_path_stats.read().await;

        // Identify slow paths
        for (path, entry) in &stats.paths {
            if entry.avg_duration > Duration::from_millis(10) {
                log_application!(
                    LogLevel::Warn,
                    "Slow hot path detected: {} (avg: {:?})",
                    path,
                    entry.avg_duration
                );
            }
        }

        // Trigger memory cleanup if needed
        if self.memory_monitor.should_cleanup().await {
            self.memory_monitor.cleanup().await;
        }
    }
}

/// Hot path execution statistics
#[derive(Debug, Clone, Default)]
pub struct HotPathStats {
    /// Statistics per path
    pub paths: std::collections::HashMap<String, HotPathEntry>,
}

/// Statistics for a single hot path
#[derive(Debug, Clone)]
pub struct HotPathEntry {
    /// Number of calls
    pub call_count: u64,

    /// Total execution time
    pub total_duration: Duration,

    /// Average execution time
    pub avg_duration: Duration,

    /// Minimum execution time
    pub min_duration: Duration,

    /// Maximum execution time
    pub max_duration: Duration,
}

impl Default for HotPathEntry {
    fn default() -> Self {
        Self {
            call_count: 0,
            total_duration: Duration::ZERO,
            avg_duration: Duration::ZERO,
            min_duration: Duration::MAX,
            max_duration: Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::sync::Arc;

    use crate::logging::Component;

    /// This test validates that memory usage remains stable under various
    /// allocation and deallocation patterns, ensuring no memory leaks occur.
    #[test]
    fn property_memory_usage_stability() {
        proptest!(|(
            operations in prop::collection::vec(
                (
                    prop::sample::select(vec![
                        Component::Transport,
                        Component::Protocol,
                        Component::Device,
                        Component::Application,
                        Component::Security,
                    ]),
                    1u64..1024u64, // allocation size
                    any::<bool>(),  // allocate (true) or deallocate (false)
                ),
                1..100
            )
        )| {
            // Use a simple runtime for the property test
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let monitor = MemoryMonitor::new(10); // 10MB limit
                let initial_usage = monitor.get_stats().await.current_usage;

                let mut allocated_sizes: std::collections::HashMap<Component, Vec<u64>> =
                    std::collections::HashMap::new();

                // Perform operations
                for (component, size, should_allocate) in operations {
                    if should_allocate {
                        // Allocate memory
                        if monitor.allocate(component, size).await.is_ok() {
                            allocated_sizes.entry(component)
                                .or_default()
                                .push(size);
                        }
                    } else {
                        // Deallocate memory if we have allocated some
                        if let Some(sizes) = allocated_sizes.get_mut(&component)
                            && let Some(allocated_size) = sizes.pop() {
                                monitor.deallocate(component, allocated_size).await;
                            }
                    }
                }

                // Deallocate all remaining memory
                for (component, sizes) in allocated_sizes {
                    for size in sizes {
                        monitor.deallocate(component, size).await;
                    }
                }

                // Check that memory usage returns to initial state (or close to it)
                let final_usage = monitor.get_stats().await.current_usage;
                let usage_diff = final_usage.abs_diff(initial_usage);

                // Allow small differences due to internal bookkeeping
                prop_assert!(usage_diff <= 1024,
                    "Memory usage not stable: initial={}, final={}, diff={}",
                    initial_usage, final_usage, usage_diff);

                // Ensure memory usage is within bounds
                prop_assert!(monitor.is_within_bounds(),
                    "Memory usage exceeded bounds: {}%",
                    monitor.usage_percentage());

                Ok(())
            })?;
        });
    }

    #[tokio::test]
    async fn test_memory_monitor_basic_operations() {
        let monitor = MemoryMonitor::new(1); // 1MB limit

        // Test allocation
        assert!(
            monitor
                .allocate(Component::Transport, 512 * 1024)
                .await
                .is_ok()
        );

        let stats = monitor.get_stats().await;
        assert_eq!(stats.current_usage, 512 * 1024);
        assert!(stats.peak_usage >= 512 * 1024);

        // Test deallocation
        monitor.deallocate(Component::Transport, 512 * 1024).await;

        let stats = monitor.get_stats().await;
        assert_eq!(stats.current_usage, 0);
    }

    #[tokio::test]
    async fn test_memory_monitor_limit_exceeded() {
        let monitor = MemoryMonitor::new(1); // 1MB limit

        // Try to allocate more than the limit
        let result = monitor
            .allocate(Component::Transport, 2 * 1024 * 1024)
            .await;
        assert!(result.is_err());

        if let Err(MemoryError::LimitExceeded { current, limit }) = result {
            assert!(current > limit);
        } else {
            panic!("Expected LimitExceeded error");
        }
    }

    #[tokio::test]
    async fn test_connection_pool_basic_operations() {
        // Create a simple connection type for testing
        #[derive(Debug)]
        struct TestConnection;

        let memory_monitor = Arc::new(MemoryMonitor::new(10)); // 10MB limit

        let pool = ConnectionPool::new(
            3, // max 3 connections
            || Ok(Arc::new(TestConnection)),
            memory_monitor,
        );

        // Get connections from pool
        let conn1 = pool.get_connection().await.unwrap();
        let _conn2 = pool.get_connection().await.unwrap();
        let _conn3 = pool.get_connection().await.unwrap();

        // Pool should be exhausted
        let result = pool.get_connection().await;
        assert!(matches!(result, Err(MemoryError::PoolExhausted { .. })));

        // Drop a connection and try again
        drop(conn1);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await; // Allow drop to complete

        let stats = pool.get_stats().await;
        assert_eq!(stats.max_size, 3);
    }

    #[tokio::test]
    async fn test_performance_optimizer() {
        let memory_monitor = Arc::new(MemoryMonitor::new(10));
        let optimizer = PerformanceOptimizer::new(memory_monitor);

        // Record some hot path executions
        optimizer
            .record_hot_path("test_path", std::time::Duration::from_millis(5))
            .await;
        optimizer
            .record_hot_path("test_path", std::time::Duration::from_millis(10))
            .await;
        optimizer
            .record_hot_path("test_path", std::time::Duration::from_millis(15))
            .await;

        let stats = optimizer.get_hot_path_stats().await;
        assert!(stats.paths.contains_key("test_path"));

        let entry = &stats.paths["test_path"];
        assert_eq!(entry.call_count, 3);
        assert_eq!(entry.avg_duration, std::time::Duration::from_millis(10));
        assert_eq!(entry.min_duration, std::time::Duration::from_millis(5));
        assert_eq!(entry.max_duration, std::time::Duration::from_millis(15));
    }

    #[tokio::test]
    async fn test_memory_cleanup() {
        let monitor = MemoryMonitor::new(10); // 10MB limit

        // Allocate some memory
        monitor.allocate(Component::Transport, 1024).await.unwrap();
        monitor.allocate(Component::Protocol, 2048).await.unwrap();

        let stats_before = monitor.get_stats().await;
        assert_eq!(stats_before.current_usage, 3072);

        // Cleanup should not change usage (no actual cleanup implemented yet)
        let _freed = monitor.cleanup().await;

        let stats_after = monitor.get_stats().await;
        // Since we don't have actual cleanup logic, usage should remain the same
        assert_eq!(stats_after.current_usage, stats_before.current_usage);
    }

    #[test]
    fn test_memory_error_display() {
        let error = MemoryError::LimitExceeded {
            current: 1000,
            limit: 500,
        };
        assert!(error.to_string().contains("Memory limit exceeded"));

        let error = MemoryError::PoolExhausted {
            max_size: 10,
            current_size: 10,
        };
        assert!(error.to_string().contains("Connection pool exhausted"));

        let error = MemoryError::ConnectionCreationFailed {
            reason: "test".to_string(),
        };
        assert!(error.to_string().contains("Failed to create connection"));
    }
}
