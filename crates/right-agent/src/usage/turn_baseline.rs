//! Per-agent statistical baselines for foreground turn metrics.

use crate::usage::error::UsageError;
use chrono::Utc;
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq)]
pub struct TurnBaselines {
    pub sample_size: u32,
    pub window_days: u32,
    pub cost_usd: BaselineMetric<f64>,
    pub num_turns: BaselineMetric<u32>,
    pub wall_elapsed_ms: BaselineMetric<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BaselineMetric<T> {
    Insufficient { sample_size: u32 },
    Available { p50: T, p90: T, p99: T },
}

pub fn compute(
    conn: &Connection,
    window_days: u32,
    min_sample: u32,
) -> Result<TurnBaselines, UsageError> {
    let window_cutoff = (Utc::now() - chrono::Duration::days(window_days as i64)).to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT total_cost_usd, num_turns, wall_elapsed_ms \
         FROM usage_events \
         WHERE source = 'interactive' AND ts >= ?1",
    )?;
    let rows = stmt.query_map([&window_cutoff], |r| {
        Ok((
            r.get::<_, f64>(0)?,
            r.get::<_, i64>(1)? as u32,
            r.get::<_, Option<i64>>(2)?.map(|v| v as u64),
        ))
    })?;
    let mut costs: Vec<f64> = Vec::new();
    let mut turns: Vec<u32> = Vec::new();
    let mut elapsed: Vec<u64> = Vec::new();
    for row in rows {
        let (c, t, e) = row?;
        costs.push(c);
        turns.push(t);
        if let Some(v) = e {
            elapsed.push(v);
        }
    }
    let sample_size = costs.len() as u32;
    let elapsed_sample_size = elapsed.len() as u32;
    Ok(TurnBaselines {
        sample_size,
        window_days,
        cost_usd: percentile_metric(&mut costs, sample_size, min_sample),
        num_turns: percentile_metric(&mut turns, sample_size, min_sample),
        wall_elapsed_ms: percentile_metric(&mut elapsed, elapsed_sample_size, min_sample),
    })
}

/// Types that can be linearly interpolated during percentile computation.
trait Lerp: Copy + PartialOrd {
    fn lerp(lo: Self, hi: Self, frac: f64) -> Self;
}

impl Lerp for f64 {
    fn lerp(lo: Self, hi: Self, frac: f64) -> Self {
        lo + frac * (hi - lo)
    }
}

impl Lerp for u32 {
    fn lerp(lo: Self, hi: Self, frac: f64) -> Self {
        (lo as f64 + frac * (hi as f64 - lo as f64)).round() as u32
    }
}

impl Lerp for u64 {
    fn lerp(lo: Self, hi: Self, frac: f64) -> Self {
        (lo as f64 + frac * (hi as f64 - lo as f64)).round() as u64
    }
}

fn percentile_metric<T: Lerp>(
    values: &mut [T],
    sample_size: u32,
    min_sample: u32,
) -> BaselineMetric<T> {
    if values.is_empty() {
        return BaselineMetric::Insufficient { sample_size };
    }
    if sample_size < min_sample {
        return BaselineMetric::Insufficient { sample_size };
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(values, 0.50);
    let p90 = percentile(values, 0.90);
    let p99 = percentile(values, 0.99);
    BaselineMetric::Available { p50, p90, p99 }
}

/// Linear-interpolation percentile (Hazen / Type 5 formula).
///
/// Uses `pos = q * n - 0.5` as the fractional 0-based index, clamped to
/// `[0, n-1]`. This places p50 exactly at the midpoint of an even-length
/// sequence and matches the expected values in the test suite.
fn percentile<T: Lerp>(sorted: &[T], q: f64) -> T {
    debug_assert!(!sorted.is_empty(), "percentile requires non-empty slice");
    let n = sorted.len();
    let pos = (q * n as f64 - 0.5).max(0.0).min((n - 1) as f64);
    let lo = (pos.floor() as usize).min(n - 1);
    let hi = (lo + 1).min(n - 1);
    let frac = pos - pos.floor();
    T::lerp(sorted[lo], sorted[hi], frac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::UsageBreakdown;
    use crate::usage::insert::insert_interactive;
    use right_db::open_connection;
    use tempfile::tempdir;

    fn sample(cost: f64, turns: u32, elapsed: Option<u64>) -> UsageBreakdown {
        UsageBreakdown {
            session_uuid: "s".into(),
            total_cost_usd: cost,
            num_turns: turns,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests: 0,
            web_fetch_requests: 0,
            model_usage_json: "{}".into(),
            api_key_source: "none".into(),
            wall_elapsed_ms: elapsed,
        }
    }

    #[test]
    fn compute_returns_insufficient_when_below_min_sample() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        for i in 0..5 {
            insert_interactive(&conn, &sample(0.01, 1, Some(i * 100)), 1, 0).unwrap();
        }
        let b = compute(&conn, 14, 20).unwrap();
        assert_eq!(b.sample_size, 5);
        assert_eq!(b.window_days, 14);
        assert!(matches!(
            b.cost_usd,
            BaselineMetric::Insufficient { sample_size: 5 }
        ));
        assert!(matches!(b.num_turns, BaselineMetric::Insufficient { .. }));
        assert!(matches!(
            b.wall_elapsed_ms,
            BaselineMetric::Insufficient { .. }
        ));
    }

    #[test]
    fn compute_returns_available_when_at_least_min_sample() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        for i in 0..50 {
            let cost = 0.01 * (i + 1) as f64;
            insert_interactive(
                &conn,
                &sample(cost, i as u32 + 1, Some((i + 1) * 100)),
                1,
                0,
            )
            .unwrap();
        }
        let b = compute(&conn, 14, 20).unwrap();
        assert_eq!(b.sample_size, 50);
        let cost_available = matches!(b.cost_usd, BaselineMetric::Available { .. });
        assert!(cost_available);
        if let BaselineMetric::Available { p50, p90, p99 } = b.cost_usd {
            assert!((p50 - 0.255).abs() < 1e-3, "p50: {p50}");
            assert!((p90 - 0.455).abs() < 1e-3, "p90: {p90}");
            assert!(
                (p99 - 0.5).abs() < 1e-3 || (p99 - 0.495).abs() < 1e-3,
                "p99: {p99}"
            );
        }
    }

    #[test]
    fn compute_excludes_non_foreground_sources() {
        use crate::usage::insert::{insert_cron, insert_learning_curator};
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        // 25 foreground + 25 cron + 25 curator; baseline counts only foreground.
        for _ in 0..25 {
            insert_interactive(&conn, &sample(0.10, 5, Some(1000)), 1, 0).unwrap();
        }
        for _ in 0..25 {
            insert_cron(&conn, &sample(0.99, 99, None), "j").unwrap();
        }
        for _ in 0..25 {
            insert_learning_curator(&conn, &sample(0.99, 99, None)).unwrap();
        }
        let b = compute(&conn, 14, 20).unwrap();
        assert_eq!(b.sample_size, 25);
        if let BaselineMetric::Available { p50, .. } = b.cost_usd {
            assert!(
                (p50 - 0.10).abs() < 1e-9,
                "foreground only; p50 must be 0.10, got {p50}"
            );
        } else {
            panic!("expected Available");
        }
    }

    #[test]
    fn compute_returns_insufficient_with_zero_sample_on_empty_db() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        let b = compute(&conn, 14, 20).unwrap();
        assert_eq!(b.sample_size, 0);
        assert!(matches!(
            b.cost_usd,
            BaselineMetric::Insufficient { sample_size: 0 }
        ));
        assert!(matches!(
            b.num_turns,
            BaselineMetric::Insufficient { sample_size: 0 }
        ));
        assert!(matches!(
            b.wall_elapsed_ms,
            BaselineMetric::Insufficient { sample_size: 0 }
        ));
    }

    #[test]
    fn compute_excludes_null_wall_elapsed_from_elapsed_baseline_only() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).unwrap();
        // 30 rows total, 10 with NULL elapsed, 20 with elapsed.
        for _ in 0..10 {
            insert_interactive(&conn, &sample(0.05, 2, None), 1, 0).unwrap();
        }
        for i in 0..20 {
            insert_interactive(&conn, &sample(0.05, 2, Some((i + 1) * 100)), 1, 0).unwrap();
        }
        let b = compute(&conn, 14, 20).unwrap();
        assert_eq!(b.sample_size, 30);
        assert!(matches!(b.cost_usd, BaselineMetric::Available { .. }));
        // Elapsed baseline has 20 samples; passes min_sample=20.
        assert!(matches!(
            b.wall_elapsed_ms,
            BaselineMetric::Available { .. }
        ));
    }
}
