//! Individual-address claim registry for bus connections.
//!
//! Tracks which [`IndividualAddress`]es are currently claimed by active bus
//! connections (e.g. tunnels) and which are known to be occupied from external
//! sources (keyring/ETS export). This prevents two connections from claiming the
//! same address and lets address-selection logic avoid known-occupied addresses.
//!
//! The registry is cheap to [`Clone`] (it shares state through [`Arc`]) so it can
//! be handed to multiple components.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use crate::error::{Result, TransportError};
use crate::log_transport;
use crate::logging::LogLevel;
use crate::protocol::address::IndividualAddress;

/// Registry of claimed and known-occupied individual addresses.
///
/// Cloning is cheap: clones share the same underlying state.
#[derive(Debug, Clone, Default)]
pub struct AddressRegistry {
    /// Addresses currently claimed by active connections in this process.
    claimed: Arc<RwLock<HashSet<IndividualAddress>>>,
    /// Addresses known to be occupied from external sources (keyring/ETS).
    known_occupied: Arc<RwLock<HashSet<IndividualAddress>>>,
}

impl AddressRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Claim an address for a bus connection.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::InvalidConfiguration`] if the address is
    /// already claimed, preventing two connections from using the same
    /// individual address (collision).
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn claim(&self, addr: IndividualAddress) -> Result<()> {
        let mut claimed = self
            .claimed
            .write()
            .expect("address registry claimed lock poisoned");

        if claimed.contains(&addr) {
            log_transport!(
                LogLevel::Warn,
                "Address {} already claimed, collision prevented",
                addr
            );
            return Err(TransportError::InvalidConfiguration {
                details: format!("Address {addr} already claimed"),
            }
            .into());
        }

        claimed.insert(addr);
        log_transport!(
            LogLevel::Info,
            "Address {} claimed for bus connection",
            addr
        );
        Ok(())
    }

    /// Release a previously claimed address, allowing it to be re-claimed.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn release(&self, addr: IndividualAddress) {
        let mut claimed = self
            .claimed
            .write()
            .expect("address registry claimed lock poisoned");
        if claimed.remove(&addr) {
            log_transport!(LogLevel::Info, "Address {} released", addr);
        }
    }

    /// Returns `true` if the address is currently claimed.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    #[must_use]
    pub fn is_claimed(&self, addr: IndividualAddress) -> bool {
        self.claimed
            .read()
            .expect("address registry claimed lock poisoned")
            .contains(&addr)
    }

    /// Mark an address as known-occupied from an external source (keyring/ETS).
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn add_known_occupied(&self, addr: IndividualAddress) {
        self.known_occupied
            .write()
            .expect("address registry known_occupied lock poisoned")
            .insert(addr);
    }

    /// Returns `true` if the address is available: neither claimed nor known to
    /// be occupied.
    ///
    /// # Panics
    ///
    /// Panics if an internal lock is poisoned.
    #[must_use]
    pub fn is_available(&self, addr: IndividualAddress) -> bool {
        let claimed = self
            .claimed
            .read()
            .expect("address registry claimed lock poisoned");
        if claimed.contains(&addr) {
            return false;
        }
        let known_occupied = self
            .known_occupied
            .read()
            .expect("address registry known_occupied lock poisoned");
        !known_occupied.contains(&addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(area: u8, line: u8, device: u8) -> IndividualAddress {
        IndividualAddress::new(area, line, device)
    }

    #[test]
    fn address_registry_claim_succeeds_then_collision_fails() {
        let registry = AddressRegistry::new();
        let a = addr(1, 1, 10);

        // First claim succeeds.
        assert!(registry.claim(a).is_ok());
        // Second claim of the same address fails (collision prevented + WARN log).
        assert!(registry.claim(a).is_err());
    }

    #[test]
    fn address_registry_release_allows_reclaim() {
        let registry = AddressRegistry::new();
        let a = addr(1, 1, 11);

        assert!(registry.claim(a).is_ok());
        assert!(registry.claim(a).is_err());

        registry.release(a);

        // After release the address can be claimed again.
        assert!(registry.claim(a).is_ok());
    }

    #[test]
    fn address_registry_is_claimed_reflects_state() {
        let registry = AddressRegistry::new();
        let a = addr(1, 1, 12);

        assert!(!registry.is_claimed(a));
        registry.claim(a).unwrap();
        assert!(registry.is_claimed(a));
        registry.release(a);
        assert!(!registry.is_claimed(a));
    }

    #[test]
    fn address_registry_known_occupied_makes_unavailable() {
        let registry = AddressRegistry::new();
        let a = addr(1, 1, 13);

        assert!(registry.is_available(a));
        registry.add_known_occupied(a);
        assert!(!registry.is_available(a));
        // Known-occupied does not count as "claimed".
        assert!(!registry.is_claimed(a));
    }

    #[test]
    fn address_registry_is_available_only_when_free() {
        let registry = AddressRegistry::new();
        let free = addr(1, 1, 14);
        let claimed = addr(1, 1, 15);
        let occupied = addr(1, 1, 16);

        registry.claim(claimed).unwrap();
        registry.add_known_occupied(occupied);

        // Available only when neither claimed nor known-occupied.
        assert!(registry.is_available(free));
        assert!(!registry.is_available(claimed));
        assert!(!registry.is_available(occupied));
    }

    #[test]
    fn address_registry_clone_shares_state() {
        let registry = AddressRegistry::new();
        let clone = registry.clone();
        let a = addr(1, 1, 17);

        // Claiming through one handle is visible through the other (shared Arc).
        registry.claim(a).unwrap();
        assert!(clone.is_claimed(a));
        assert!(clone.claim(a).is_err());
    }
}
