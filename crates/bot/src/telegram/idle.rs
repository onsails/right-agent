// Task 1 lands the tracker before runtime wiring; remove this allow when the tracker is wired.
#![allow(dead_code)]

use dashmap::DashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct IdleKey {
    pub chat_id: i64,
    pub thread_id: i64,
}

#[derive(Debug)]
pub(crate) struct IdleTracker {
    start_ts: i64,
    last_seen: DashMap<IdleKey, i64>,
}

impl IdleTracker {
    /// `start_ts` doubles as the fallback "last seen" for keys that have
    /// never been touched, so a thread the bot has never observed is
    /// considered idle since `start_ts`.
    pub(crate) fn new(start_ts: i64) -> Self {
        Self {
            start_ts,
            last_seen: DashMap::new(),
        }
    }

    pub(crate) fn touch(&self, key: IdleKey, now: i64) {
        self.last_seen.insert(key, now);
    }

    pub(crate) fn idle_for_secs(&self, key: IdleKey, now: i64) -> i64 {
        let last = self
            .last_seen
            .get(&key)
            .map(|entry| *entry.value())
            .unwrap_or(self.start_ts);
        now.saturating_sub(last)
    }

    pub(crate) fn prune_older_than(&self, cutoff_ts: i64) {
        self.last_seen
            .retain(|_, last_seen| *last_seen >= cutoff_ts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn touch_is_isolated_by_thread() {
        let tracker = IdleTracker::new(1_000);
        let thread_a = IdleKey {
            chat_id: -100,
            thread_id: 111,
        };
        let thread_b = IdleKey {
            chat_id: -100,
            thread_id: 222,
        };

        tracker.touch(thread_a, 1_050);

        assert_eq!(tracker.idle_for_secs(thread_a, 1_080), 30);
        assert_eq!(tracker.idle_for_secs(thread_b, 1_080), 80);
    }

    #[tokio::test]
    async fn thread_zero_is_root_chat_key() {
        let tracker = IdleTracker::new(2_000);
        let root = IdleKey {
            chat_id: -200,
            thread_id: 0,
        };

        tracker.touch(root, 2_010);

        assert_eq!(tracker.idle_for_secs(root, 2_050), 40);
    }

    #[tokio::test]
    async fn unknown_key_uses_tracker_start_time() {
        let tracker = IdleTracker::new(3_000);
        let key = IdleKey {
            chat_id: -300,
            thread_id: 9,
        };

        assert_eq!(tracker.idle_for_secs(key, 3_090), 90);
    }

    #[tokio::test]
    async fn prune_removes_old_keys_only() {
        let tracker = IdleTracker::new(4_000);
        let old_key = IdleKey {
            chat_id: -400,
            thread_id: 1,
        };
        let fresh_key = IdleKey {
            chat_id: -400,
            thread_id: 2,
        };

        tracker.touch(old_key, 4_010);
        tracker.touch(fresh_key, 4_100);
        tracker.prune_older_than(4_050);

        assert_eq!(tracker.idle_for_secs(old_key, 4_120), 120);
        assert_eq!(tracker.idle_for_secs(fresh_key, 4_120), 20);
    }

    #[tokio::test]
    async fn prune_keeps_keys_at_cutoff() {
        let tracker = IdleTracker::new(4_500);
        let key = IdleKey {
            chat_id: -450,
            thread_id: 1,
        };

        tracker.touch(key, 4_550);
        tracker.prune_older_than(4_550);

        assert_eq!(tracker.idle_for_secs(key, 4_560), 10);
    }

    #[tokio::test]
    async fn tracker_is_shareable() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<IdleTracker>();

        let tracker = Arc::new(IdleTracker::new(5_000));
        tracker.touch(
            IdleKey {
                chat_id: 5,
                thread_id: 0,
            },
            5_001,
        );
        assert_eq!(
            tracker.idle_for_secs(
                IdleKey {
                    chat_id: 5,
                    thread_id: 0,
                },
                5_011,
            ),
            10,
        );
    }
}
