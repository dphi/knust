use std::time::Duration;
use tokio::time::Instant;

/// KNX/IP routing pause and moving busy window; evaluated lazily without timer tasks.
#[derive(Default)]
pub(super) struct FlowControl {
    last_send: Option<Instant>,
    last_busy: Option<Instant>,
    base_until: Option<Instant>,
    busy_until: Option<Instant>,
    decay_at: Option<Instant>,
    busy_count: u32,
}

impl FlowControl {
    fn decay(&mut self, now: Instant) {
        if let Some(at) = self.decay_at
            && let Some(elapsed) = now.checked_duration_since(at)
        {
            let steps = (elapsed.as_millis() / 5 + 1).min(u128::from(self.busy_count)) as u32;
            self.busy_count -= steps;
            self.decay_at =
                (self.busy_count > 0).then(|| at + Duration::from_millis(u64::from(steps) * 5));
        }
    }

    pub(super) fn busy(&mut self, now: Instant, wait: Duration, random: f64) {
        self.decay(now);
        if self
            .last_busy
            .is_none_or(|previous| now.duration_since(previous) > Duration::from_millis(10))
        {
            self.busy_count = self.busy_count.saturating_add(1);
        }
        self.last_busy = Some(now);
        if self.busy_until.is_some_and(|until| until > now)
            && self
                .base_until
                .and_then(|until| until.checked_duration_since(now))
                .is_some_and(|remaining| remaining >= wait)
        {
            return;
        }
        let base = now + wait;
        let until = base + Duration::from_secs_f64(random * f64::from(self.busy_count) * 0.05);
        self.base_until = Some(base);
        self.busy_until = Some(until);
        self.decay_at = Some(until + Duration::from_millis(u64::from(self.busy_count) * 100 + 5));
    }

    pub(super) fn deadline(&mut self, now: Instant) -> Instant {
        self.decay(now);
        let spacing = self
            .last_send
            .map_or(now, |sent| sent + Duration::from_millis(20));
        spacing.max(self.busy_until.unwrap_or(now)).max(now)
    }

    pub(super) fn sent(&mut self, now: Instant) {
        self.last_send = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn busy_count_cooldown_extension_and_decay_match_wire_rules() {
        let start = Instant::now();
        let mut flow = FlowControl::default();
        flow.busy(start, Duration::from_millis(100), 1.0);
        assert_eq!(flow.busy_count, 1);
        assert_eq!(flow.deadline(start), start + Duration::from_millis(150));
        // A shorter repeated wait cannot shorten the original pause; exactly 10ms does not increment N.
        flow.busy(
            start + Duration::from_millis(10),
            Duration::from_millis(20),
            0.0,
        );
        assert_eq!(flow.busy_count, 1);
        assert_eq!(flow.deadline(start), start + Duration::from_millis(150));
        flow.busy(
            start + Duration::from_millis(21),
            Duration::from_millis(200),
            1.0,
        );
        assert_eq!(flow.busy_count, 2);
        assert_eq!(flow.deadline(start), start + Duration::from_millis(321));
        flow.deadline(start + Duration::from_millis(525));
        assert_eq!(flow.busy_count, 2);
        flow.deadline(start + Duration::from_millis(526));
        assert_eq!(flow.busy_count, 1);
        flow.deadline(start + Duration::from_millis(531));
        assert_eq!(flow.busy_count, 0);
        flow.busy(start + Duration::from_millis(600), Duration::ZERO, 0.0);
        assert_eq!(flow.busy_count, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn first_send_is_immediate_and_later_sends_respect_spacing() {
        let now = Instant::now();
        let mut flow = FlowControl::default();
        assert_eq!(flow.deadline(now), now);
        flow.sent(now);
        assert_eq!(flow.deadline(now), now + Duration::from_millis(20));
        assert_eq!(
            flow.deadline(now + Duration::from_secs(1)),
            now + Duration::from_secs(1)
        );
    }
}
