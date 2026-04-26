//! Agent-facing memory status and its watch channel.
//!
//! Severity ordering: Healthy < Degraded < AuthFailed. Worker merges the
//! wrapper-owned status with per-turn local status via `.max()`.

use std::time::Instant;

#[derive(Debug, Clone, Copy)]
pub enum MemoryStatus {
    Healthy,
    Degraded { since: Instant },
    AuthFailed { since: Instant },
}

impl MemoryStatus {
    fn severity(&self) -> u8 {
        match self {
            MemoryStatus::Healthy => 0,
            MemoryStatus::Degraded { .. } => 1,
            MemoryStatus::AuthFailed { .. } => 2,
        }
    }
}

impl PartialEq for MemoryStatus {
    fn eq(&self, other: &Self) -> bool {
        self.severity() == other.severity()
    }
}
impl Eq for MemoryStatus {}
impl PartialOrd for MemoryStatus {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MemoryStatus {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.severity().cmp(&other.severity())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_ordering() {
        let h = MemoryStatus::Healthy;
        let d = MemoryStatus::Degraded {
            since: Instant::now(),
        };
        let a = MemoryStatus::AuthFailed {
            since: Instant::now(),
        };
        assert!(h < d);
        assert!(d < a);
        assert!(h < a);
    }

    #[test]
    fn max_merges_by_severity() {
        let h = MemoryStatus::Healthy;
        let d = MemoryStatus::Degraded {
            since: Instant::now(),
        };
        let a = MemoryStatus::AuthFailed {
            since: Instant::now(),
        };
        assert_eq!(h.max(d).severity(), d.severity());
        assert_eq!(d.max(a).severity(), a.severity());
        assert_eq!(h.max(a).severity(), a.severity());
    }

    #[test]
    fn equal_severity_eq() {
        let d1 = MemoryStatus::Degraded {
            since: Instant::now(),
        };
        let d2 = MemoryStatus::Degraded {
            since: Instant::now() + std::time::Duration::from_secs(5),
        };
        assert_eq!(d1, d2);
    }
}
