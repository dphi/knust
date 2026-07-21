//! KNX address types and parsing with type-safe newtype patterns.
//!
//! This module provides type-safe wrappers around KNX addresses with compile-time
//! validation and comprehensive trait implementations following Rust 2024 best practices.

use crate::error::{ProtocolError, Result};
use std::fmt;
use std::str::FromStr;

/// KNX address enumeration with type-safe variants
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Address {
    /// Group address for logical communication
    Group(GroupAddress),

    /// Individual address for physical devices
    Individual(IndividualAddress),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Group(addr) => write!(f, "{addr}"),
            Address::Individual(addr) => write!(f, "{addr}"),
        }
    }
}

impl From<GroupAddress> for Address {
    fn from(addr: GroupAddress) -> Self {
        Address::Group(addr)
    }
}

impl From<IndividualAddress> for Address {
    fn from(addr: IndividualAddress) -> Self {
        Address::Individual(addr)
    }
}

impl TryFrom<&str> for Address {
    type Error = crate::error::KnxError;

    fn try_from(s: &str) -> Result<Self> {
        // Try parsing as group address first (contains '/')
        if s.contains('/') {
            Ok(Address::Group(GroupAddress::from_str(s)?))
        } else if s.contains('.') {
            Ok(Address::Individual(IndividualAddress::from_str(s)?))
        } else {
            Err(ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: "Address must contain either '/' (group) or '.' (individual)".to_string(),
            }
            .into())
        }
    }
}

/// The main-group component of a [`GroupAddress`] (4 bits: 0-15).
///
/// Holding a `MainGroup` at all is proof the value is in range — there's no
/// way to construct one otherwise. Use [`Self::new`] for a value you
/// already trust (a literal, e.g. — invalid input there is a bug and
/// panics, or fails to compile if used in a `const`). For a genuinely
/// dynamic value where you want a recoverable [`AddressError`] instead of a
/// panic, build the whole [`GroupAddress`] via [`GroupAddress::try_new`]
/// (which takes raw `u8`s) rather than constructing a `MainGroup` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MainGroup(u8);

impl MainGroup {
    /// Maximum valid value (4 bits)
    pub const MAX: u8 = 15;

    /// # Panics
    ///
    /// Panics if `value > Self::MAX`. In a `const` context this is a
    /// compile error instead of a runtime panic.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        assert!(value <= Self::MAX, "main group out of range (0-15)");
        Self(value)
    }

    /// The validated value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl From<u8> for MainGroup {
    /// # Panics
    ///
    /// Panics if `value > Self::MAX` — same guarantee as [`Self::new`].
    fn from(value: u8) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for MainGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The middle-group component of a [`GroupAddress`] (3 bits: 0-7). See
/// [`MainGroup`] — same guarantee, same two ways to construct one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MiddleGroup(u8);

impl MiddleGroup {
    /// Maximum valid value (3 bits)
    pub const MAX: u8 = 7;

    /// # Panics
    ///
    /// Panics if `value > Self::MAX`. In a `const` context this is a
    /// compile error instead of a runtime panic.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        assert!(value <= Self::MAX, "middle group out of range (0-7)");
        Self(value)
    }

    /// The validated value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl From<u8> for MiddleGroup {
    /// # Panics
    ///
    /// Panics if `value > Self::MAX` — same guarantee as [`Self::new`].
    fn from(value: u8) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for MiddleGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// KNX group address (logical address) with compile-time validation
///
/// Group addresses are used for logical communication in KNX networks.
/// They follow the format main/middle/sub where:
/// - main: 0-15 (4 bits)
/// - middle: 0-7 (3 bits)
/// - sub: 0-255 (8 bits)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupAddress(u16);

impl GroupAddress {
    /// Maximum valid main group value (4 bits)
    pub const MAX_MAIN: u8 = 15;

    /// Maximum valid middle group value (3 bits)
    pub const MAX_MIDDLE: u8 = 7;

    /// Maximum valid sub group value (8 bits)
    pub const MAX_SUB: u8 = 255;

    /// Maximum valid raw address value
    pub const MAX_RAW: u16 = 0x7FFF;

    /// Create a group address from a validated main/middle component pair
    /// and a sub-group byte.
    ///
    /// `main`/`middle` are [`MainGroup`]/[`MiddleGroup`] — types that can
    /// only hold an in-range value — so unlike a raw `u8`-based
    /// constructor, this can never silently corrupt the packed
    /// representation on out-of-range input, whether that input came from
    /// a literal or a dynamic source: `MainGroup`/`MiddleGroup` already
    /// forced that check when they were constructed.
    #[must_use]
    pub const fn new(main: MainGroup, middle: MiddleGroup, sub: u8) -> Self {
        let raw = ((main.0 as u16) << 11) | ((middle.0 as u16) << 8) | (sub as u16);
        Self(raw)
    }

    /// Create a group address from main/middle/sub components with validation
    ///
    /// # Errors
    ///
    /// Returns [`AddressError::InvalidRange`] if `main > `[`Self::MAX_MAIN`]
    /// or `middle > `[`Self::MAX_MIDDLE`].
    pub const fn try_new(main: u8, middle: u8, sub: u8) -> std::result::Result<Self, AddressError> {
        if main > Self::MAX_MAIN {
            return Err(AddressError::InvalidRange {
                component: "main",
                value: main as u32,
                max: Self::MAX_MAIN as u32,
            });
        }

        if middle > Self::MAX_MIDDLE {
            return Err(AddressError::InvalidRange {
                component: "middle",
                value: middle as u32,
                max: Self::MAX_MIDDLE as u32,
            });
        }

        let raw = ((main as u16) << 11) | ((middle as u16) << 8) | (sub as u16);
        Ok(Self(raw))
    }

    /// Create a group address from raw value with validation
    ///
    /// # Errors
    ///
    /// Returns [`AddressError::InvalidRange`] if `raw > `[`Self::MAX_RAW`].
    pub const fn try_from_raw(raw: u16) -> std::result::Result<Self, AddressError> {
        if raw > Self::MAX_RAW {
            return Err(AddressError::InvalidRange {
                component: "raw",
                value: raw as u32,
                max: Self::MAX_RAW as u32,
            });
        }
        Ok(Self(raw))
    }

    /// Create a group address from raw value without validation
    ///
    /// # Safety
    /// The caller must ensure that `raw` is a valid group address value (≤ 0x7FFF)
    #[must_use]
    pub const fn from_raw_unchecked(raw: u16) -> Self {
        Self(raw)
    }

    /// Alias for `try_new` for backward compatibility
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::try_new`].
    pub const fn from_parts(
        main: u8,
        middle: u8,
        sub: u8,
    ) -> std::result::Result<Self, AddressError> {
        Self::try_new(main, middle, sub)
    }

    /// Get the raw address value
    #[must_use]
    pub const fn raw(&self) -> u16 {
        self.0
    }

    /// Get the main group (4 bits: 0-15)
    #[must_use]
    pub const fn main(&self) -> u8 {
        ((self.0 >> 11) & 0x0F) as u8
    }

    /// Get the middle group (3 bits: 0-7)
    #[must_use]
    pub const fn middle(&self) -> u8 {
        ((self.0 >> 8) & 0x07) as u8
    }

    /// Get the sub group (8 bits: 0-255)
    #[must_use]
    pub const fn sub(&self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    /// Get all components as a tuple (main, middle, sub)
    #[must_use]
    pub const fn parts(&self) -> (u8, u8, u8) {
        (self.main(), self.middle(), self.sub())
    }

    /// Check if this is a broadcast address (0/0/0)
    #[must_use]
    pub const fn is_broadcast(&self) -> bool {
        self.0 == 0
    }

    /// Create a broadcast group address (0/0/0)
    #[must_use]
    pub const fn broadcast() -> Self {
        Self(0)
    }
}

impl fmt::Display for GroupAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.main(), self.middle(), self.sub())
    }
}

impl FromStr for GroupAddress {
    type Err = crate::error::KnxError;

    fn from_str(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() != 3 {
            return Err(ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: "Group address must have format main/middle/sub".to_string(),
            }
            .into());
        }

        let main = parts[0]
            .parse::<u8>()
            .map_err(|_| ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: "Invalid main group number".to_string(),
            })?;

        let middle = parts[1]
            .parse::<u8>()
            .map_err(|_| ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: "Invalid middle group number".to_string(),
            })?;

        let sub = parts[2]
            .parse::<u8>()
            .map_err(|_| ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: "Invalid sub group number".to_string(),
            })?;

        Self::try_new(main, middle, sub).map_err(|e| {
            ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: e.to_string(),
            }
            .into()
        })
    }
}

impl TryFrom<u16> for GroupAddress {
    type Error = AddressError;

    fn try_from(raw: u16) -> std::result::Result<Self, Self::Error> {
        Self::try_from_raw(raw)
    }
}

impl From<GroupAddress> for u16 {
    fn from(addr: GroupAddress) -> Self {
        addr.raw()
    }
}

impl TryFrom<(u8, u8, u8)> for GroupAddress {
    type Error = AddressError;

    fn try_from((main, middle, sub): (u8, u8, u8)) -> std::result::Result<Self, Self::Error> {
        Self::try_new(main, middle, sub)
    }
}

impl From<GroupAddress> for (u8, u8, u8) {
    fn from(addr: GroupAddress) -> Self {
        addr.parts()
    }
}

/// KNX individual address (physical address) with compile-time validation
///
/// Individual addresses are used for physical device identification in KNX networks.
/// They follow the format area.line.device where:
/// - area: 0-15 (4 bits)
/// - line: 0-15 (4 bits)
/// - device: 0-255 (8 bits)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IndividualAddress(u16);

impl IndividualAddress {
    /// Maximum valid area value (4 bits)
    pub const MAX_AREA: u8 = 15;

    /// Maximum valid line value (4 bits)
    pub const MAX_LINE: u8 = 15;

    /// Maximum valid device value (8 bits)
    pub const MAX_DEVICE: u8 = 255;

    /// Maximum valid raw address value
    pub const MAX_RAW: u16 = 0xFFFF;

    /// Create an individual address from area/line/device components
    ///
    /// # Panics
    /// Panics if any component is out of valid range in debug builds.
    /// In release builds, invalid values are silently wrapped.
    #[must_use]
    pub const fn new(area: u8, line: u8, device: u8) -> Self {
        debug_assert!(area <= Self::MAX_AREA, "area out of range");
        debug_assert!(line <= Self::MAX_LINE, "line out of range");

        let raw = ((area as u16) << 12) | ((line as u16) << 8) | (device as u16);
        Self(raw)
    }

    /// Create an individual address from area/line/device components with validation
    ///
    /// # Errors
    ///
    /// Returns [`AddressError::InvalidRange`] if `area > `[`Self::MAX_AREA`]
    /// or `line > `[`Self::MAX_LINE`].
    pub const fn try_new(
        area: u8,
        line: u8,
        device: u8,
    ) -> std::result::Result<Self, AddressError> {
        if area > Self::MAX_AREA {
            return Err(AddressError::InvalidRange {
                component: "area",
                value: area as u32,
                max: Self::MAX_AREA as u32,
            });
        }

        if line > Self::MAX_LINE {
            return Err(AddressError::InvalidRange {
                component: "line",
                value: line as u32,
                max: Self::MAX_LINE as u32,
            });
        }

        let raw = ((area as u16) << 12) | ((line as u16) << 8) | (device as u16);
        Ok(Self(raw))
    }

    /// Create an individual address from raw value
    #[must_use]
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    /// Create an individual address from area/line/device components without validation
    #[must_use]
    pub const fn from_parts_unchecked(area: u8, line: u8, device: u8) -> Self {
        let raw = ((area as u16) << 12) | ((line as u16) << 8) | (device as u16);
        Self(raw)
    }

    /// Get the raw address value
    #[must_use]
    pub const fn raw(&self) -> u16 {
        self.0
    }

    /// Get the area (4 bits: 0-15)
    #[must_use]
    pub const fn area(&self) -> u8 {
        ((self.0 >> 12) & 0x0F) as u8
    }

    /// Get the line (4 bits: 0-15)
    #[must_use]
    pub const fn line(&self) -> u8 {
        ((self.0 >> 8) & 0x0F) as u8
    }

    /// Get the device (8 bits: 0-255)
    #[must_use]
    pub const fn device(&self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    /// Get all components as a tuple (area, line, device)
    #[must_use]
    pub const fn parts(&self) -> (u8, u8, u8) {
        (self.area(), self.line(), self.device())
    }

    /// Check if this is a broadcast address (0.0.0)
    #[must_use]
    pub const fn is_broadcast(&self) -> bool {
        self.0 == 0
    }

    /// Create a broadcast individual address (0.0.0)
    #[must_use]
    pub const fn broadcast() -> Self {
        Self(0)
    }
}

impl fmt::Display for IndividualAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.area(), self.line(), self.device())
    }
}

impl Default for IndividualAddress {
    /// `0.0.0` (see [`Self::broadcast`]) — the placeholder
    /// [`Telegram::group_read`](crate::protocol::Telegram::group_read) and
    /// [`Telegram::group_write`](crate::protocol::Telegram::group_write)
    /// use for `source`, since `Knx::send_telegram` overwrites it with the
    /// bus's own configured address before sending.
    fn default() -> Self {
        Self::broadcast()
    }
}

impl FromStr for IndividualAddress {
    type Err = crate::error::KnxError;

    fn from_str(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: "Individual address must have format area.line.device".to_string(),
            }
            .into());
        }

        let area = parts[0]
            .parse::<u8>()
            .map_err(|_| ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: "Invalid area number".to_string(),
            })?;

        let line = parts[1]
            .parse::<u8>()
            .map_err(|_| ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: "Invalid line number".to_string(),
            })?;

        let device = parts[2]
            .parse::<u8>()
            .map_err(|_| ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: "Invalid device number".to_string(),
            })?;

        Self::try_new(area, line, device).map_err(|e| {
            ProtocolError::InvalidAddress {
                address: s.to_string(),
                reason: e.to_string(),
            }
            .into()
        })
    }
}

impl TryFrom<u16> for IndividualAddress {
    type Error = AddressError;

    fn try_from(raw: u16) -> std::result::Result<Self, Self::Error> {
        // Individual addresses can use the full u16 range
        Ok(Self::from_raw(raw))
    }
}

impl From<IndividualAddress> for u16 {
    fn from(addr: IndividualAddress) -> Self {
        addr.raw()
    }
}

impl TryFrom<(u8, u8, u8)> for IndividualAddress {
    type Error = AddressError;

    fn try_from((area, line, device): (u8, u8, u8)) -> std::result::Result<Self, Self::Error> {
        Self::try_new(area, line, device)
    }
}

impl From<IndividualAddress> for (u8, u8, u8) {
    fn from(addr: IndividualAddress) -> Self {
        addr.parts()
    }
}

/// Structured error type for address validation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    /// Address component value is out of valid range
    InvalidRange {
        component: &'static str,
        value: u32,
        max: u32,
    },
    /// Invalid address format during parsing
    InvalidFormat {
        input: String,
        expected: &'static str,
    },
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressError::InvalidRange {
                component,
                value,
                max,
            } => {
                write!(f, "Invalid {component} value: {value} (max: {max})")
            }
            AddressError::InvalidFormat { input, expected } => {
                write!(
                    f,
                    "Invalid address format: '{input}' (expected: {expected})"
                )
            }
        }
    }
}

impl std::error::Error for AddressError {}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::str::FromStr;

    /// For any KNX address input, invalid addresses should be rejected with appropriate
    /// error messages and valid addresses should parse correctly.
    #[test]
    fn property_address_validation_completeness() {
        proptest!(|(
            // Generate valid group address components
            main in 0u8..=GroupAddress::MAX_MAIN,
            middle in 0u8..=GroupAddress::MAX_MIDDLE,
            sub in 0u8..=GroupAddress::MAX_SUB,

            // Generate valid individual address components
            area in 0u8..=IndividualAddress::MAX_AREA,
            line in 0u8..=IndividualAddress::MAX_LINE,
            device in 0u8..=IndividualAddress::MAX_DEVICE,

            // Generate invalid components for testing validation
            invalid_main in (GroupAddress::MAX_MAIN + 1)..=255u8,
            invalid_middle in (GroupAddress::MAX_MIDDLE + 1)..=255u8,
            invalid_area in (IndividualAddress::MAX_AREA + 1)..=255u8,
            invalid_line in (IndividualAddress::MAX_LINE + 1)..=255u8,
        )| {
            // Test valid GroupAddress creation and parsing
            let group_addr = GroupAddress::new(MainGroup::new(main), MiddleGroup::new(middle), sub);
            prop_assert_eq!(group_addr.main(), main);
            prop_assert_eq!(group_addr.middle(), middle);
            prop_assert_eq!(group_addr.sub(), sub);

            // Test GroupAddress string round trip
            let group_str = group_addr.to_string();
            let parsed_group = GroupAddress::from_str(&group_str)?;
            prop_assert_eq!(group_addr, parsed_group);

            // Test valid IndividualAddress creation and parsing
            let individual_addr = IndividualAddress::new(area, line, device);
            prop_assert_eq!(individual_addr.area(), area);
            prop_assert_eq!(individual_addr.line(), line);
            prop_assert_eq!(individual_addr.device(), device);

            // Test IndividualAddress string round trip
            let individual_str = individual_addr.to_string();
            let parsed_individual = IndividualAddress::from_str(&individual_str)?;
            prop_assert_eq!(individual_addr, parsed_individual);

            // Test Address enum conversions
            let group_enum: Address = group_addr.into();
            let individual_enum: Address = individual_addr.into();
            prop_assert_eq!(group_enum.to_string(), group_str);
            prop_assert_eq!(individual_enum.to_string(), individual_str);

            // Test invalid GroupAddress components are rejected
            prop_assert!(GroupAddress::try_new(invalid_main, middle, sub).is_err());
            prop_assert!(GroupAddress::try_new(main, invalid_middle, sub).is_err());

            // Test invalid IndividualAddress components are rejected
            prop_assert!(IndividualAddress::try_new(invalid_area, line, device).is_err());
            prop_assert!(IndividualAddress::try_new(area, invalid_line, device).is_err());

            // Test conversion traits
            let group_tuple: (u8, u8, u8) = group_addr.into();
            prop_assert_eq!(group_tuple, (main, middle, sub));

            let individual_tuple: (u8, u8, u8) = individual_addr.into();
            prop_assert_eq!(individual_tuple, (area, line, device));

            let group_raw: u16 = group_addr.into();
            let individual_raw: u16 = individual_addr.into();
            prop_assert_eq!(GroupAddress::try_from(group_raw)?, group_addr);
            prop_assert_eq!(IndividualAddress::try_from(individual_raw)?, individual_addr);
        });
    }

    #[test]
    fn test_group_address_validation() {
        // Test valid group addresses
        let addr = GroupAddress::new(MainGroup::new(15), MiddleGroup::new(7), 255);
        assert_eq!(addr.main(), 15);
        assert_eq!(addr.middle(), 7);
        assert_eq!(addr.sub(), 255);

        // Test invalid main group (> 15)
        let result = GroupAddress::try_new(16, 0, 0);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("main"));
        }

        // Test invalid middle group (> 7)
        let result = GroupAddress::try_new(0, 8, 0);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("middle"));
        }

        // Test parsing valid string
        let addr = GroupAddress::from_str("1/2/3").unwrap();
        assert_eq!(addr.main(), 1);
        assert_eq!(addr.middle(), 2);
        assert_eq!(addr.sub(), 3);

        // Test parsing invalid strings
        assert!(GroupAddress::from_str("1/2").is_err());
        assert!(GroupAddress::from_str("1/2/3/4").is_err());
        assert!(GroupAddress::from_str("16/0/0").is_err());
        assert!(GroupAddress::from_str("0/8/0").is_err());
        assert!(GroupAddress::from_str("a/b/c").is_err());
    }

    #[test]
    fn test_individual_address_validation() {
        // Test valid individual addresses
        let addr = IndividualAddress::new(15, 15, 255);
        assert_eq!(addr.area(), 15);
        assert_eq!(addr.line(), 15);
        assert_eq!(addr.device(), 255);

        // Test invalid area (> 15)
        let result = IndividualAddress::try_new(16, 0, 0);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("area"));
        }

        // Test invalid line (> 15)
        let result = IndividualAddress::try_new(0, 16, 0);
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("line"));
        }

        // Test parsing valid string
        let addr = IndividualAddress::from_str("1.2.3").unwrap();
        assert_eq!(addr.area(), 1);
        assert_eq!(addr.line(), 2);
        assert_eq!(addr.device(), 3);

        // Test parsing invalid strings
        assert!(IndividualAddress::from_str("1.2").is_err());
        assert!(IndividualAddress::from_str("1.2.3.4").is_err());
        assert!(IndividualAddress::from_str("16.0.0").is_err());
        assert!(IndividualAddress::from_str("0.16.0").is_err());
        assert!(IndividualAddress::from_str("a.b.c").is_err());
    }

    #[test]
    fn test_address_display() {
        let group_addr = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        assert_eq!(group_addr.to_string(), "1/2/3");

        let individual_addr = IndividualAddress::new(1, 2, 3);
        assert_eq!(individual_addr.to_string(), "1.2.3");

        let group_enum = Address::Group(group_addr);
        assert_eq!(group_enum.to_string(), "1/2/3");

        let individual_enum = Address::Individual(individual_addr);
        assert_eq!(individual_enum.to_string(), "1.2.3");
    }

    #[test]
    fn test_address_raw_values() {
        // Test GroupAddress raw value calculation
        let addr = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        let expected_raw = (1u16 << 11) | (2u16 << 8) | 3u16;
        assert_eq!(addr.raw(), expected_raw);

        let addr_from_raw = GroupAddress::try_from_raw(expected_raw).unwrap();
        assert_eq!(addr, addr_from_raw);

        // Test IndividualAddress raw value calculation
        let addr = IndividualAddress::new(1, 2, 3);
        let expected_raw = (1u16 << 12) | (2u16 << 8) | 3u16;
        assert_eq!(addr.raw(), expected_raw);

        let addr_from_raw = IndividualAddress::from_raw(expected_raw);
        assert_eq!(addr, addr_from_raw);
    }

    #[test]
    fn test_broadcast_addresses() {
        // Test group broadcast
        let group_broadcast = GroupAddress::broadcast();
        assert!(group_broadcast.is_broadcast());
        assert_eq!(group_broadcast.parts(), (0, 0, 0));

        // Test individual broadcast
        let individual_broadcast = IndividualAddress::broadcast();
        assert!(individual_broadcast.is_broadcast());
        assert_eq!(individual_broadcast.parts(), (0, 0, 0));
    }

    #[test]
    fn test_address_ordering() {
        let addr1 = GroupAddress::from_parts(1, 0, 0).unwrap();
        let addr2 = GroupAddress::from_parts(1, 0, 1).unwrap();
        let addr3 = GroupAddress::from_parts(1, 1, 0).unwrap();

        assert!(addr1 < addr2);
        assert!(addr2 < addr3);
        assert!(addr1 < addr3);

        let iaddr1 = IndividualAddress::new(1, 0, 0);
        let iaddr2 = IndividualAddress::new(1, 0, 1);
        let iaddr3 = IndividualAddress::new(1, 1, 0);

        assert!(iaddr1 < iaddr2);
        assert!(iaddr2 < iaddr3);
        assert!(iaddr1 < iaddr3);
    }

    #[test]
    fn test_conversion_traits() {
        let group_addr = GroupAddress::new(MainGroup::new(5), MiddleGroup::new(3), 100);

        // Test tuple conversion
        let tuple: (u8, u8, u8) = group_addr.into();
        assert_eq!(tuple, (5, 3, 100));

        let from_tuple = GroupAddress::try_from(tuple).unwrap();
        assert_eq!(from_tuple, group_addr);

        // Test raw conversion
        let raw: u16 = group_addr.into();
        let from_raw = GroupAddress::try_from(raw).unwrap();
        assert_eq!(from_raw, group_addr);

        let individual_addr = IndividualAddress::new(2, 5, 50);

        // Test tuple conversion
        let tuple: (u8, u8, u8) = individual_addr.into();
        assert_eq!(tuple, (2, 5, 50));

        let from_tuple = IndividualAddress::try_from(tuple).unwrap();
        assert_eq!(from_tuple, individual_addr);

        // Test raw conversion
        let raw: u16 = individual_addr.into();
        let from_raw = IndividualAddress::try_from(raw).unwrap();
        assert_eq!(from_raw, individual_addr);
    }

    #[test]
    fn test_address_enum_parsing() {
        // Test parsing group address through Address enum
        let group_str = "1/2/3";
        let addr = Address::try_from(group_str).unwrap();
        if let Address::Group(group_addr) = addr {
            assert_eq!(group_addr.parts(), (1, 2, 3));
        } else {
            panic!("Expected Group address");
        }

        // Test parsing individual address through Address enum
        let individual_str = "1.2.3";
        let addr = Address::try_from(individual_str).unwrap();
        if let Address::Individual(individual_addr) = addr {
            assert_eq!(individual_addr.parts(), (1, 2, 3));
        } else {
            panic!("Expected Individual address");
        }

        // Test invalid format
        assert!(Address::try_from("invalid").is_err());
    }

    #[test]
    fn test_const_functions() {
        // Test const functions work at compile time
        const GROUP_ADDR: GroupAddress =
            GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        const INDIVIDUAL_ADDR: IndividualAddress = IndividualAddress::new(1, 2, 3);
        // Test const validation with try_new
        const VALID_GROUP: std::result::Result<GroupAddress, AddressError> =
            GroupAddress::try_new(1, 2, 3);
        const VALID_INDIVIDUAL: std::result::Result<IndividualAddress, AddressError> =
            IndividualAddress::try_new(1, 2, 3);
        // Test const validation
        const INVALID_GROUP: std::result::Result<GroupAddress, AddressError> =
            GroupAddress::try_new(32, 0, 0);
        const INVALID_INDIVIDUAL: std::result::Result<IndividualAddress, AddressError> =
            IndividualAddress::try_new(16, 0, 0);

        assert_eq!(GROUP_ADDR.parts(), (1, 2, 3));
        assert_eq!(INDIVIDUAL_ADDR.parts(), (1, 2, 3));

        assert!(VALID_GROUP.is_ok());
        assert!(VALID_INDIVIDUAL.is_ok());

        assert!(INVALID_GROUP.is_err());
        assert!(INVALID_INDIVIDUAL.is_err());
    }
}
