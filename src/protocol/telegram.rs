//! KNX telegram representation and handling.

use crate::protocol::address::{Address, GroupAddress, IndividualAddress};
use std::time::Duration;

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

    /// Timestamp when telegram was created/received
    pub timestamp: std::time::SystemTime,
}

impl Telegram {
    /// Build an outgoing `GroupValueRead` for `destination`.
    ///
    /// `source` is left as [`IndividualAddress::default`] (`0.0.0`) —
    /// sending this via
    /// [`Knx::send_telegram`](crate::application::Knx::send_telegram) fills
    /// it in with the bus's own configured individual address, so callers
    /// never need to know or pass it themselves.
    #[must_use]
    pub fn group_read(destination: GroupAddress) -> Self {
        Self {
            source: IndividualAddress::default(),
            destination: Address::Group(destination),
            payload: Vec::new(),
            priority: Priority::Normal,
            direction: Direction::Outgoing,
            telegram_type: TelegramType::GroupValueRead,
            timestamp: std::time::SystemTime::now(),
        }
    }

    /// Build an outgoing `GroupValueWrite` for `destination` carrying
    /// `payload`. `source` is filled in the same way as [`Self::group_read`].
    #[must_use]
    pub fn group_write(destination: GroupAddress, payload: Vec<u8>) -> Self {
        Self {
            source: IndividualAddress::default(),
            destination: Address::Group(destination),
            payload,
            priority: Priority::Normal,
            direction: Direction::Outgoing,
            telegram_type: TelegramType::GroupValueWrite,
            timestamp: std::time::SystemTime::now(),
        }
    }

    /// Build a telegram as if it arrived from `source` on the bus — for
    /// simulating incoming traffic (tests, mocks) or representing an
    /// already-decoded frame. `telegram_type` says which APCI service it is
    /// (`GroupValueRead`/`GroupValueWrite`/`GroupValueResponse`); unlike
    /// [`Self::group_read`]/[`Self::group_write`], it isn't inferred,
    /// because a real incoming telegram's type isn't ours to choose.
    #[must_use]
    pub fn received(
        source: IndividualAddress,
        destination: GroupAddress,
        telegram_type: TelegramType,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            source,
            destination: Address::Group(destination),
            payload,
            priority: Priority::Normal,
            direction: Direction::Incoming,
            telegram_type,
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

    /// Whether `timestamp` is older than `max_age`.
    ///
    /// A clock error (`timestamp` somehow in the future) counts as stale
    /// too — if the age can't be determined, "too old to trust" is the
    /// safer assumption. Useful for e.g. a read-responder that must not
    /// answer a `GroupValueRead` it picked up long after it actually
    /// arrived (queue backlog, a slow callback ahead of it, ...) — the
    /// requester may well have already given up waiting.
    #[must_use]
    pub fn is_older_than(&self, max_age: Duration) -> bool {
        match self.timestamp.elapsed() {
            Ok(elapsed) => elapsed > max_age,
            Err(_) => true,
        }
    }
}

#[cfg(feature = "dpt")]
impl Telegram {
    /// Build an outgoing `GroupValueWrite` for `destination` from a
    /// [`DptValue`](crate::protocol::dpt::DptValue) instead of an
    /// already-encoded byte payload — accepts `T` itself, its plain inner
    /// value (`true` for a
    /// [`dpt_alias`](crate::dpt_alias)-generated switch DPT, etc.), or a
    /// human string (`"on"`); see
    /// [`WriteValue`](crate::protocol::dpt::WriteValue). This is the same
    /// resolve/validate/encode [`Knx::group_address`](crate::application::Knx::group_address)'s
    /// typed `write` uses — reach for this directly when you want that
    /// ergonomics without registering the address first.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`](crate::error::ProtocolError::DptError)
    /// if `T::DPT_NUMBER` has no registered runtime
    /// [`DptType`](crate::protocol::dpt::DptType), `value` doesn't resolve
    /// to a valid `T` (e.g. an unparseable string), or `T::validate` rejects
    /// it.
    pub fn group_write_value<T: crate::protocol::dpt::DptValue>(
        destination: GroupAddress,
        value: impl crate::protocol::dpt::WriteValue<T>,
    ) -> crate::error::Result<Self> {
        let dpt = T::dpt_type()?;
        let value: T = value.resolve(dpt)?;
        value.validate()?;
        Ok(Self::group_write(destination, value.encode()?))
    }

    /// Build an outgoing `GroupValueResponse` for `destination` from a
    /// [`DptValue`](crate::protocol::dpt::DptValue) — same value-shape
    /// ergonomics as [`Self::group_write_value`], for answering a
    /// `GroupValueRead` with the address's current value.
    ///
    /// Only ever call this with a value you actually have — a response
    /// carrying a stale default (e.g. `0.0` because no real reading was
    /// taken yet) tells the bus something false. Silently not answering is
    /// always safer than answering wrong.
    ///
    /// # Errors
    ///
    /// Same as [`Self::group_write_value`].
    pub fn group_response_value<T: crate::protocol::dpt::DptValue>(
        destination: GroupAddress,
        value: impl crate::protocol::dpt::WriteValue<T>,
    ) -> crate::error::Result<Self> {
        let mut telegram = Self::group_write_value(destination, value)?;
        telegram.telegram_type = TelegramType::GroupValueResponse;
        Ok(telegram)
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

#[cfg(test)]
mod staleness_tests {
    use super::*;
    use crate::protocol::address::{GroupAddress, MainGroup, MiddleGroup};

    fn ga() -> GroupAddress {
        GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3)
    }

    #[test]
    fn is_older_than_false_for_a_fresh_telegram() {
        let telegram = Telegram::group_read(ga());
        assert!(!telegram.is_older_than(Duration::from_secs(60)));
    }

    #[test]
    fn is_older_than_true_once_max_age_elapses() {
        let mut telegram = Telegram::group_read(ga());
        telegram.timestamp -= Duration::from_millis(50);
        assert!(telegram.is_older_than(Duration::from_millis(10)));
        assert!(!telegram.is_older_than(Duration::from_secs(60)));
    }

    #[test]
    fn is_older_than_true_on_clock_error() {
        let mut telegram = Telegram::group_read(ga());
        // A timestamp in the future (clock skew, VM pause, ...) can't have
        // its age computed — must be treated as stale, not fresh.
        telegram.timestamp += Duration::from_secs(60);
        assert!(telegram.is_older_than(Duration::from_secs(1)));
    }
}

#[cfg(all(test, feature = "dpt"))]
mod value_tests {
    use super::*;
    use crate::protocol::address::{GroupAddress, MainGroup, MiddleGroup};
    use crate::protocol::dpt::DPTSwitch;

    fn ga() -> GroupAddress {
        GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3)
    }

    #[test]
    fn group_write_value_accepts_bool_and_string_and_matches_typed_encode() {
        let from_bool = Telegram::group_write_value::<DPTSwitch>(ga(), true).unwrap();
        assert_eq!(from_bool.telegram_type, TelegramType::GroupValueWrite);
        assert_eq!(from_bool.direction, Direction::Outgoing);
        assert_eq!(from_bool.payload, vec![0x01]);

        let from_str = Telegram::group_write_value::<DPTSwitch>(ga(), "on").unwrap();
        assert_eq!(from_str.payload, vec![0x01]);

        let off = Telegram::group_write_value::<DPTSwitch>(ga(), false).unwrap();
        assert_eq!(off.payload, vec![0x00]);
    }

    #[test]
    fn group_write_value_rejects_unparseable_string() {
        assert!(Telegram::group_write_value::<DPTSwitch>(ga(), "banana").is_err());
    }

    #[test]
    fn group_response_value_is_a_response_not_a_write() {
        let response = Telegram::group_response_value::<DPTSwitch>(ga(), true).unwrap();
        assert_eq!(response.telegram_type, TelegramType::GroupValueResponse);
        assert_eq!(response.direction, Direction::Outgoing);
        assert_eq!(response.payload, vec![0x01]);
        assert_eq!(response.destination, Address::Group(ga()));
    }
}
