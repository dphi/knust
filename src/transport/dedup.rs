//! Hash + TTL window deduplication for telegrams.
//!
//! Extracted from `MultiConnection`'s original inline duplicate-frame
//! detection so the same logic can be reused for other, differently-scoped
//! dedup needs (e.g. cross-bus forwarding loop prevention).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::protocol::telegram::Telegram;

/// Tracks recently-seen telegrams within a sliding time window.
pub struct TelegramDedup {
    recent: RwLock<HashMap<u64, Instant>>,
    window: Duration,
}

impl TelegramDedup {
    /// Create a dedup cache with the given time window.
    #[must_use]
    pub fn new(window: Duration) -> Self {
        Self {
            recent: RwLock::new(HashMap::new()),
            window,
        }
    }

    /// Hash a telegram on (source, destination, payload).
    fn hash(telegram: &Telegram) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        telegram.source.hash(&mut hasher);
        telegram.destination.hash(&mut hasher);
        telegram.payload.hash(&mut hasher);
        hasher.finish()
    }

    /// Returns `true` if this telegram was already seen within the window
    /// (the caller should treat it as a duplicate); otherwise records it and
    /// returns `false`.
    pub async fn check_and_record(&self, telegram: &Telegram) -> bool {
        let hash = Self::hash(telegram);
        let now = Instant::now();

        let mut recent = self.recent.write().await;
        if let Some(prev_time) = recent.get(&hash)
            && now.duration_since(*prev_time) < self.window
        {
            return true;
        }
        recent.insert(hash, now);
        recent.retain(|_, t| now.duration_since(*t) < self.window);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::address::{Address, GroupAddress, IndividualAddress};

    fn telegram() -> Telegram {
        Telegram::new_incoming(
            IndividualAddress::new(1, 1, 5),
            Address::Group(GroupAddress::from_parts(1, 2, 3).unwrap()),
            vec![0x01],
        )
    }

    #[tokio::test]
    async fn duplicate_within_window_is_detected() {
        let dedup = TelegramDedup::new(Duration::from_secs(2));
        let t = telegram();
        assert!(!dedup.check_and_record(&t).await);
        assert!(dedup.check_and_record(&t).await);
    }

    #[tokio::test]
    async fn distinct_telegrams_are_not_deduplicated() {
        let dedup = TelegramDedup::new(Duration::from_secs(2));
        let t1 = telegram();
        let mut t2 = telegram();
        t2.payload = vec![0x02];
        assert!(!dedup.check_and_record(&t1).await);
        assert!(!dedup.check_and_record(&t2).await);
    }
}
