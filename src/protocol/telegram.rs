//! KNX telegram representation and handling.

use crate::protocol::address::{Address, IndividualAddress};

/// APCI service type — what kind of group message this is
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TelegramType {
    /// `A_GroupValue_Read` — requesting current value (no payload)
    GroupValueRead,
    /// `A_GroupValue_Response` — response to a read request
    GroupValueResponse,
    /// `A_GroupValue_Write` — writing/commanding a new value
    GroupValueWrite,
}

impl std::fmt::Display for TelegramType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GroupValueRead => write!(f, "GroupValueRead"),
            Self::GroupValueResponse => write!(f, "GroupValueResponse"),
            Self::GroupValueWrite => write!(f, "GroupValueWrite"),
        }
    }
}

/// KNX telegram structure
#[derive(Debug, Clone)]
pub struct Telegram {
    /// Source individual address
    pub source: IndividualAddress,

    /// Destination address (group or individual)
    pub destination: Address,

    /// Telegram payload data
    pub payload: Vec<u8>,

    /// Message priority
    pub priority: Priority,

    /// Telegram direction (incoming/outgoing)
    pub direction: Direction,

    /// APCI service type
    pub telegram_type: TelegramType,

    /// Gateway identifier (populated by `MultiConnection` coordinator)
    pub gateway_id: Option<u16>,

    /// Timestamp when telegram was created/received
    pub timestamp: std::time::SystemTime,
}

impl Telegram {
    /// Create a new outgoing telegram
    #[must_use]
    pub fn new_outgoing(source: IndividualAddress, destination: Address, payload: Vec<u8>) -> Self {
        let telegram_type = if payload.is_empty() {
            TelegramType::GroupValueRead
        } else {
            TelegramType::GroupValueWrite
        };
        Self {
            source,
            destination,
            payload,
            priority: Priority::Normal,
            direction: Direction::Outgoing,
            telegram_type,
            gateway_id: None,
            timestamp: std::time::SystemTime::now(),
        }
    }

    /// Create a new incoming telegram
    #[must_use]
    pub fn new_incoming(source: IndividualAddress, destination: Address, payload: Vec<u8>) -> Self {
        Self {
            source,
            destination,
            payload,
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type: TelegramType::GroupValueWrite,
            gateway_id: None,
            timestamp: std::time::SystemTime::now(),
        }
    }

    /// Check if this is a group telegram
    #[must_use]
    pub fn is_group_telegram(&self) -> bool {
        matches!(self.destination, Address::Group(_))
    }

    /// Check if this is an individual telegram
    #[must_use]
    pub fn is_individual_telegram(&self) -> bool {
        matches!(self.destination, Address::Individual(_))
    }

    /// Get the payload length
    #[must_use]
    pub fn payload_len(&self) -> usize {
        self.payload.len()
    }

    /// Check if the telegram is empty (no payload)
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.payload.is_empty()
    }
}

/// Telegram priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    /// System priority (highest)
    System = 0,

    /// Normal priority
    #[default]
    Normal = 1,

    /// Urgent priority
    Urgent = 2,

    /// Low priority (lowest)
    Low = 3,
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Lower numeric values have higher priority
        // System (0) > Urgent (2) > Normal (1) > Low (3)
        let self_priority = match self {
            Priority::System => 0,
            Priority::Urgent => 1,
            Priority::Normal => 2,
            Priority::Low => 3,
        };

        let other_priority = match other {
            Priority::System => 0,
            Priority::Urgent => 1,
            Priority::Normal => 2,
            Priority::Low => 3,
        };

        self_priority.cmp(&other_priority)
    }
}

impl Priority {
    /// Convert from u8 value
    #[must_use]
    pub fn from_u8(value: u8) -> Self {
        match value & 0x03 {
            0 => Priority::System,
            2 => Priority::Urgent,
            3 => Priority::Low,
            _ => Priority::Normal,
        }
    }

    /// Convert to u8 value
    #[must_use]
    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

/// Telegram direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Incoming telegram (received from network)
    Incoming,

    /// Outgoing telegram (sent to network)
    Outgoing,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Incoming => write!(f, "incoming"),
            Direction::Outgoing => write!(f, "outgoing"),
        }
    }
}
