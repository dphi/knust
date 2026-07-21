//! Group address → DPT binding registry.
//!
//! Every group address that's registered here is bound to exactly one DPT,
//! shared by both directions: [`DptType::parse`] (typed writes) and
//! [`DptType::decode_ref`] (decoding incoming telegrams) both consult the
//! same binding, so the two directions can never silently disagree about
//! what a given address means.

use std::collections::HashMap;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use super::DptType;
use crate::error::{ProtocolError, Result};
use crate::protocol::address::GroupAddress;
#[cfg(test)]
use crate::protocol::address::{MainGroup, MiddleGroup};

/// A `Binding`/entry-map is plain bookkeeping data with no invariant that a
/// panic mid-write could leave broken beyond the one field being written —
/// unlike, say, a connection handle. So on a poisoned lock (some unrelated
/// panic occurred while it was held) we recover the last-written state
/// instead of propagating the panic: one bad access shouldn't turn every
/// future registry lookup — including the telegram receive hot path — into
/// a permanent crash.
fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Per-address state: the DPT binding plus everything needed to drive an
/// optional TTL-based refresh (see [`GroupAddressRegistry::next_stale`]).
#[derive(Debug, Clone)]
struct Binding {
    dpt: DptType,
    /// Refresh interval; `None` means never auto-refreshed.
    ttl: Option<Duration>,
    /// When the last real `GroupValueWrite`/`GroupValueResponse` for this
    /// address was observed.
    last_seen: Option<Instant>,
    /// The payload of that last real telegram.
    last_seen_value: Option<Vec<u8>>,
    /// When we last sent a refresh `GroupValueRead` for this address.
    /// Independent of `last_seen` — only a real answer bumps that.
    last_tried: Option<Instant>,
}

/// Snapshot of a registered group address's DPT binding, refresh TTL, and
/// last known value.
#[derive(Debug, Clone)]
pub struct GroupAddressState {
    pub dpt: DptType,
    pub ttl: Option<Duration>,
    pub last_seen: Option<Instant>,
    pub last_seen_value: Option<Vec<u8>>,
    pub last_tried: Option<Instant>,
}

impl From<Binding> for GroupAddressState {
    fn from(binding: Binding) -> Self {
        Self {
            dpt: binding.dpt,
            ttl: binding.ttl,
            last_seen: binding.last_seen,
            last_seen_value: binding.last_seen_value,
            last_tried: binding.last_tried,
        }
    }
}

/// Registry binding group addresses to their DPT.
///
/// Each binding lives behind its own inner lock, so a hot-path update to
/// one address (e.g. [`Self::record_seen`] on every incoming
/// `GroupValueWrite`) only ever takes the outer map lock for a read and
/// contends with other addresses' updates, not with lookups on them.
#[derive(Default)]
pub struct GroupAddressRegistry {
    entries: RwLock<HashMap<GroupAddress, RwLock<Binding>>>,
}

impl GroupAddressRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind `address` to `dpt`.
    ///
    /// Re-registering the same `(address, dpt)` pair is a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `address` is already bound to
    /// a *different* DPT.
    pub fn register(&self, address: GroupAddress, dpt: DptType) -> Result<()> {
        let mut entries = write_lock(&self.entries);
        match entries.get(&address) {
            Some(existing) if read_lock(existing).dpt != dpt => Err(ProtocolError::DptError {
                dpt_type: dpt.number_str(),
                details: format!(
                    "group address {address} is already registered as DPT {} (cannot rebind to DPT {})",
                    read_lock(existing).dpt.number_str(),
                    dpt.number_str()
                ),
            }
            .into()),
            Some(_) => Ok(()),
            None => {
                entries.insert(
                    address,
                    RwLock::new(Binding {
                        dpt,
                        ttl: None,
                        last_seen: None,
                        last_seen_value: None,
                        last_tried: None,
                    }),
                );
                Ok(())
            }
        }
    }

    /// Look up the DPT bound to `address`, if any.
    #[must_use]
    pub fn get(&self, address: GroupAddress) -> Option<DptType> {
        read_lock(&self.entries)
            .get(&address)
            .map(|b| read_lock(b).dpt)
    }

    /// Remove `address`'s binding, if any, returning the DPT it was bound
    /// to. The address is free to be registered under a different DPT
    /// afterward.
    pub fn unregister(&self, address: GroupAddress) -> Option<DptType> {
        write_lock(&self.entries).remove(&address).map(|b| {
            b.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .dpt
        })
    }

    /// Snapshot of every registered `(address, dpt)` binding, for
    /// inspection. Order is unspecified.
    #[must_use]
    pub fn entries(&self) -> Vec<(GroupAddress, DptType)> {
        read_lock(&self.entries)
            .iter()
            .map(|(address, binding)| (*address, read_lock(binding).dpt))
            .collect()
    }

    /// Number of registered addresses.
    #[must_use]
    pub fn len(&self) -> usize {
        read_lock(&self.entries).len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Set (or clear, with `None`) the refresh TTL for `address`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::DptError`] if `address` isn't registered.
    pub fn set_refresh_ttl(&self, address: GroupAddress, ttl: Option<Duration>) -> Result<()> {
        let entries = read_lock(&self.entries);
        match entries.get(&address) {
            Some(binding) => {
                write_lock(binding).ttl = ttl;
                Ok(())
            }
            None => Err(ProtocolError::DptError {
                dpt_type: String::new(),
                details: format!(
                    "group address {address} is not registered; register it before configuring a refresh TTL"
                ),
            }
            .into()),
        }
    }

    /// Record that a real `GroupValueWrite`/`GroupValueResponse` for
    /// `address` was just observed, remembering its payload. No-op if
    /// `address` isn't registered.
    pub fn record_seen(&self, address: GroupAddress, value: &[u8]) {
        if let Some(binding) = read_lock(&self.entries).get(&address) {
            let mut binding = write_lock(binding);
            binding.last_seen = Some(Instant::now());
            binding.last_seen_value = Some(value.to_vec());
        }
    }

    /// Record that a refresh `GroupValueRead` was just sent for `address`.
    /// Does not affect `last_seen`/staleness. No-op if `address` isn't
    /// registered.
    pub fn record_tried(&self, address: GroupAddress) {
        if let Some(binding) = read_lock(&self.entries).get(&address) {
            write_lock(binding).last_tried = Some(Instant::now());
        }
    }

    /// The most overdue stale address, if any: among addresses with a TTL
    /// set whose value hasn't been seen within it, the one that was least
    /// recently (or never) refresh-attempted.
    ///
    /// Round-robining on `last_tried` — rather than just returning the
    /// first stale address found — matters because an address that never
    /// answers never gets `last_seen` bumped, so without this it would
    /// stay at the front forever and starve every other stale address
    /// behind it.
    #[must_use]
    pub fn next_stale(&self, now: Instant) -> Option<GroupAddress> {
        read_lock(&self.entries)
            .iter()
            .filter(|(_, binding)| {
                let binding = read_lock(binding);
                let Some(ttl) = binding.ttl else {
                    return false;
                };
                match binding.last_seen {
                    Some(last_seen) => now.saturating_duration_since(last_seen) >= ttl,
                    None => true,
                }
            })
            .min_by(|(_, a), (_, b)| {
                let a = read_lock(a).last_tried;
                let b = read_lock(b).last_tried;
                match (a, b) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (Some(x), Some(y)) => x.cmp(&y),
                }
            })
            .map(|(address, _)| *address)
    }

    /// Snapshot of `address`'s full state — DPT, TTL, and last-seen/last-tried
    /// bookkeeping — if it's registered.
    #[must_use]
    pub fn state(&self, address: GroupAddress) -> Option<GroupAddressState> {
        read_lock(&self.entries)
            .get(&address)
            .map(|b| GroupAddressState::from(read_lock(b).clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        assert_eq!(registry.get(ga), None);

        registry.register(ga, DptType::Switch).unwrap();
        assert_eq!(registry.get(ga), Some(DptType::Switch));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn reregistering_same_dpt_is_a_noop() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        registry.register(ga, DptType::Switch).unwrap();
        registry.register(ga, DptType::Switch).unwrap();
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn conflicting_dpt_is_rejected() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        registry.register(ga, DptType::Switch).unwrap();
        let err = registry.register(ga, DptType::Temperature).unwrap_err();
        assert!(err.to_string().contains("already registered"));
        // The original binding is untouched.
        assert_eq!(registry.get(ga), Some(DptType::Switch));
    }

    #[test]
    fn unregister_removes_binding_and_frees_the_address() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        registry.register(ga, DptType::Switch).unwrap();

        assert_eq!(registry.unregister(ga), Some(DptType::Switch));
        assert_eq!(registry.get(ga), None);
        assert!(registry.is_empty());

        // Unregistering something not present is a harmless no-op.
        assert_eq!(registry.unregister(ga), None);

        // The address can now be rebound to a different DPT.
        registry.register(ga, DptType::Temperature).unwrap();
        assert_eq!(registry.get(ga), Some(DptType::Temperature));
    }

    #[test]
    fn entries_snapshots_all_bindings() {
        let registry = GroupAddressRegistry::new();
        let switch = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        let temp = GroupAddress::new(MainGroup::new(2), MiddleGroup::new(1), 1);
        registry.register(switch, DptType::Switch).unwrap();
        registry.register(temp, DptType::Temperature).unwrap();

        let mut entries = registry.entries();
        entries.sort_by_key(|(address, _)| address.raw());
        assert_eq!(
            entries,
            vec![(switch, DptType::Switch), (temp, DptType::Temperature)]
        );
    }

    #[test]
    fn set_refresh_ttl_requires_registration() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        let err = registry
            .set_refresh_ttl(ga, Some(Duration::from_secs(60)))
            .unwrap_err();
        assert!(err.to_string().contains("not registered"));
    }

    #[test]
    fn record_seen_sets_value_without_touching_last_tried() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        registry.register(ga, DptType::Switch).unwrap();

        registry.record_seen(ga, &[0x01]);
        let state = registry.state(ga).unwrap();
        assert_eq!(state.last_seen_value, Some(vec![0x01]));
        assert!(state.last_seen.is_some());
        assert!(state.last_tried.is_none());
    }

    #[test]
    fn record_tried_sets_last_tried_without_touching_last_seen() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        registry.register(ga, DptType::Switch).unwrap();

        registry.record_tried(ga);
        let state = registry.state(ga).unwrap();
        assert!(state.last_tried.is_some());
        assert!(state.last_seen.is_none());
        assert!(state.last_seen_value.is_none());
    }

    #[test]
    fn next_stale_ignores_addresses_without_ttl() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        registry.register(ga, DptType::Switch).unwrap();

        assert_eq!(registry.next_stale(Instant::now()), None);
    }

    #[test]
    fn next_stale_treats_never_seen_as_stale() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        registry.register(ga, DptType::Switch).unwrap();
        registry
            .set_refresh_ttl(ga, Some(Duration::from_secs(60)))
            .unwrap();

        assert_eq!(registry.next_stale(Instant::now()), Some(ga));
    }

    #[test]
    fn next_stale_respects_ttl_not_yet_elapsed() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        registry.register(ga, DptType::Switch).unwrap();
        registry
            .set_refresh_ttl(ga, Some(Duration::from_secs(60)))
            .unwrap();
        registry.record_seen(ga, &[0x01]);

        assert_eq!(registry.next_stale(Instant::now()), None);
    }

    #[test]
    fn next_stale_fires_once_ttl_elapses() {
        let registry = GroupAddressRegistry::new();
        let ga = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        registry.register(ga, DptType::Switch).unwrap();
        registry
            .set_refresh_ttl(ga, Some(Duration::from_millis(10)))
            .unwrap();
        registry.record_seen(ga, &[0x01]);

        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(registry.next_stale(Instant::now()), Some(ga));
    }

    #[test]
    fn next_stale_round_robins_by_last_tried() {
        let registry = GroupAddressRegistry::new();
        let first = GroupAddress::new(MainGroup::new(1), MiddleGroup::new(2), 3);
        let second = GroupAddress::new(MainGroup::new(2), MiddleGroup::new(1), 1);
        registry.register(first, DptType::Switch).unwrap();
        registry.register(second, DptType::Switch).unwrap();
        registry
            .set_refresh_ttl(first, Some(Duration::from_secs(60)))
            .unwrap();
        registry
            .set_refresh_ttl(second, Some(Duration::from_secs(60)))
            .unwrap();

        // Both are never-seen and never-tried; either may come back first,
        // but trying it must hand priority to the other one next.
        let picked = registry.next_stale(Instant::now()).unwrap();
        registry.record_tried(picked);
        let other = if picked == first { second } else { first };

        assert_eq!(registry.next_stale(Instant::now()), Some(other));
    }
}
