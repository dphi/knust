//! Comprehensive logging and debugging support for Knx.
//!
//! This module provides structured logging capabilities with configurable
//! log levels per component, protocol event logging, and debugging utilities
//! for troubleshooting KNX/IP communication issues.

use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

/// Log levels for different components — re-exported from the `log` crate.
pub use log::Level as LogLevel;

/// Component categories for targeted logging
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    /// Transport layer (connections, sockets)
    Transport,
    /// Protocol layer (CEMI, KNX/IP frames)
    Protocol,
    /// Device layer (lights, sensors, etc.)
    Device,
    /// Security layer (authentication, encryption)
    Security,
    /// Configuration parsing and validation
    Configuration,
    /// Gateway discovery
    Discovery,
    /// Application layer (main Knx interface)
    Application,
    /// Queue management
    Queue,
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Component::Transport => write!(f, "transport"),
            Component::Protocol => write!(f, "protocol"),
            Component::Device => write!(f, "device"),
            Component::Security => write!(f, "security"),
            Component::Configuration => write!(f, "config"),
            Component::Discovery => write!(f, "discovery"),
            Component::Application => write!(f, "application"),
            Component::Queue => write!(f, "queue"),
        }
    }
}

/// Logging configuration manager
#[derive(Debug)]
pub struct LoggingConfig {
    /// Component-specific log levels
    component_levels: Arc<RwLock<HashMap<Component, LogLevel>>>,
    /// Global default log level
    default_level: LogLevel,
    /// Enable protocol event logging
    protocol_events: bool,
    /// Enable hex dump of raw data
    hex_dump: bool,
    /// Maximum hex dump size
    max_hex_dump_size: usize,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            component_levels: Arc::new(RwLock::new(HashMap::new())),
            default_level: LogLevel::Info,
            protocol_events: false,
            hex_dump: false,
            max_hex_dump_size: 256,
        }
    }
}

impl LoggingConfig {
    /// Create a new logging configuration
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the default log level for all components
    pub fn set_default_level(&mut self, level: LogLevel) {
        self.default_level = level;
    }

    /// Set log level for a specific component
    pub fn set_component_level(&mut self, component: Component, level: LogLevel) {
        if let Ok(mut levels) = self.component_levels.write() {
            levels.insert(component, level);
        }
    }

    /// Get log level for a component
    #[must_use]
    pub fn get_component_level(&self, component: Component) -> LogLevel {
        if let Ok(levels) = self.component_levels.read() {
            levels
                .get(&component)
                .copied()
                .unwrap_or(self.default_level)
        } else {
            self.default_level
        }
    }

    /// Enable or disable protocol event logging
    pub fn set_protocol_events(&mut self, enabled: bool) {
        self.protocol_events = enabled;
    }

    /// Enable or disable hex dump logging
    pub fn set_hex_dump(&mut self, enabled: bool) {
        self.hex_dump = enabled;
    }

    /// Set maximum hex dump size
    pub fn set_max_hex_dump_size(&mut self, size: usize) {
        self.max_hex_dump_size = size;
    }

    /// Check if logging is enabled for a component at a specific level
    #[must_use]
    pub fn is_enabled(&self, component: Component, level: LogLevel) -> bool {
        let component_level = self.get_component_level(component);
        level <= component_level && log::log_enabled!(level)
    }

    /// Check if protocol events are enabled
    #[must_use]
    pub fn protocol_events_enabled(&self) -> bool {
        self.protocol_events
    }

    /// Check if hex dump is enabled
    #[must_use]
    pub fn hex_dump_enabled(&self) -> bool {
        self.hex_dump
    }
}

/// Global logging configuration instance
static LOGGING_CONFIG: std::sync::OnceLock<LoggingConfig> = std::sync::OnceLock::new();

/// Initialize the logging system
pub fn init_logging(config: LoggingConfig) {
    LOGGING_CONFIG.set(config).ok(); // Ignore error if already initialized
}

/// Get the global logging configuration
fn get_config() -> &'static LoggingConfig {
    use std::sync::OnceLock;
    static DEFAULT_CONFIG: OnceLock<LoggingConfig> = OnceLock::new();

    LOGGING_CONFIG
        .get()
        .unwrap_or_else(|| DEFAULT_CONFIG.get_or_init(LoggingConfig::default))
}

/// Log a message for a specific component
pub fn log_message(component: Component, level: LogLevel, message: &str) {
    let config = get_config();
    if config.is_enabled(component, level) {
        match level {
            log::Level::Error => error!(target: &component.to_string(), "{message}"),
            log::Level::Warn => warn!(target: &component.to_string(), "{message}"),
            log::Level::Info => info!(target: &component.to_string(), "{message}"),
            log::Level::Debug => debug!(target: &component.to_string(), "{message}"),
            log::Level::Trace => trace!(target: &component.to_string(), "{message}"),
        }
    }
}

/// Log a formatted message for a specific component
#[macro_export]
macro_rules! log_component {
    ($component:expr, $level:expr, $($arg:tt)*) => {
        $crate::logging::log_message($component, $level, &format!($($arg)*))
    };
}

/// Log hex dump of binary data
pub fn log_hex_dump(target: &str, description: &str, data: &[u8], max_size: usize) {
    let data_to_dump = if data.len() > max_size {
        &data[..max_size]
    } else {
        data
    };

    let hex_string = data_to_dump
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");

    let truncated = if data.len() > max_size {
        format!(" (truncated from {} bytes)", data.len())
    } else {
        String::new()
    };

    debug!(target: target, "{description}{truncated}: {hex_string}");
}

/// Performance timing helper
pub struct Timer {
    start: std::time::Instant,
    component: Component,
    operation: String,
}

impl Timer {
    /// Start timing an operation
    #[must_use]
    pub fn start(component: Component, operation: &str) -> Self {
        Self {
            start: std::time::Instant::now(),
            component,
            operation: operation.to_string(),
        }
    }

    /// Finish timing and log the duration
    pub fn finish(self) {
        let duration = self.start.elapsed();
        log_message(
            self.component,
            LogLevel::Debug,
            &format!("Operation '{}' completed in {:?}", self.operation, duration),
        );
    }

    /// Finish timing with a custom message
    pub fn finish_with_message(self, message: &str) {
        let duration = self.start.elapsed();
        log_message(
            self.component,
            LogLevel::Debug,
            &format!("{message} (took {duration:?})"),
        );
    }
}

/// Convenience macros for component-specific logging
#[macro_export]
macro_rules! log_transport {
    ($level:expr, $($arg:tt)*) => {
        $crate::logging::log_message($crate::logging::Component::Transport, $level, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_protocol {
    ($level:expr, $($arg:tt)*) => {
        $crate::logging::log_message($crate::logging::Component::Protocol, $level, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_device {
    ($level:expr, $($arg:tt)*) => {
        $crate::logging::log_message($crate::logging::Component::Device, $level, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_security {
    ($level:expr, $($arg:tt)*) => {
        $crate::logging::log_message($crate::logging::Component::Security, $level, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_config {
    ($level:expr, $($arg:tt)*) => {
        $crate::logging::log_message($crate::logging::Component::Configuration, $level, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_discovery {
    ($level:expr, $($arg:tt)*) => {
        $crate::logging::log_message($crate::logging::Component::Discovery, $level, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_application {
    ($level:expr, $($arg:tt)*) => {
        $crate::logging::log_message($crate::logging::Component::Application, $level, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_queue {
    ($level:expr, $($arg:tt)*) => {
        $crate::logging::log_message($crate::logging::Component::Queue, $level, &format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Error < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Trace);
    }

    #[test]
    fn test_component_display() {
        assert_eq!(Component::Transport.to_string(), "transport");
        assert_eq!(Component::Protocol.to_string(), "protocol");
        assert_eq!(Component::Device.to_string(), "device");
    }

    #[test]
    fn test_logging_config() {
        let mut config = LoggingConfig::new();

        // Test default level
        assert_eq!(
            config.get_component_level(Component::Transport),
            LogLevel::Info
        );

        // Test setting component level
        config.set_component_level(Component::Transport, LogLevel::Debug);
        assert_eq!(
            config.get_component_level(Component::Transport),
            LogLevel::Debug
        );

        // Test other components still use default
        assert_eq!(
            config.get_component_level(Component::Protocol),
            LogLevel::Info
        );
    }

    #[test]
    fn test_timer() {
        let timer = Timer::start(Component::Transport, "test_operation");
        std::thread::sleep(std::time::Duration::from_millis(1));
        timer.finish();
    }
}
