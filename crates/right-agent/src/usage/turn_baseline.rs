//! Per-agent statistical baselines for foreground turn metrics.

use crate::usage::error::UsageError;
use chrono::{DateTime, Utc};
use right_db::{Connection, params};

#[derive(Debug, Clone, PartialEq)]
pub struct TurnBaselines {
    pub sample_size: u32,
    pub elapsed_sample_size: u32,
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

pub async fn compute(
    conn: &Connection,
    window_days: u32,
    min_sample: u32,
) -> Result<TurnBaselines, UsageError> {
    let window_cutoff = (Utc::now() - chrono::Duration::days(window_days as i64)).to_rfc3339();
    let rows = conn
        .query_all(
            "SELECT total_cost_usd, num_turns, wall_elapsed_ms \
         FROM usage_events \
         WHERE source = 'interactive' AND ts >= ?1",
            [&window_cutoff],
            |r| {
                Ok((
                    r.get::<_, f64>(0)?,
                    r.get::<_, i64>(1)? as u32,
                    r.get::<_, Option<i64>>(2)?.map(|v| v as u64),
                ))
            },
        )
        .await?;
    let mut costs: Vec<f64> = Vec::new();
    let mut turns: Vec<u32> = Vec::new();
    let mut elapsed: Vec<u64> = Vec::new();
    for (c, t, e) in rows {
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
        elapsed_sample_size,
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

/// Trigger evidence for `cost_spike` — populated when the gate fires due to
/// today's probe-writer cost exceeding the 14d P50 multiplier.
#[derive(Debug, Clone, PartialEq)]
pub struct CostSpikeEvidence {
    pub today_cost_usd: f64,
    pub baseline_p50_usd: f64,
    pub k: f64,
    pub min_floor_usd: f64,
}

/// Return `Some(evidence)` iff today's probe-writer spend exceeds both `k *
/// baseline_p50` and `min_floor_usd`. Both conditions must hold.
pub async fn check_probe_writer_cost_spike(
    conn: &Connection,
    now: DateTime<Utc>,
    baseline_days: u32,
    k: f64,
    min_floor_usd: f64,
) -> Result<Option<CostSpikeEvidence>, UsageError> {
    let today_start = now.format("%Y-%m-%dT00:00:00Z").to_string();
    let today_cost: f64 = conn
        .query_row(
            "SELECT COALESCE(SUM(total_cost_usd), 0.0) FROM usage_events \
         WHERE source = 'learning_probe_writer' AND ts >= ?1",
            [&today_start],
            |r| r.get(0),
        )
        .await?;
    if today_cost < min_floor_usd {
        return Ok(None);
    }
    // Daily sums over the baseline window — group by date.
    let window_start = (now - chrono::Duration::days(baseline_days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();
    let rows = conn
        .query_all(
            "SELECT SUM(total_cost_usd) FROM usage_events \
         WHERE source = 'learning_probe_writer' AND ts >= ?1 AND ts < ?2 \
         GROUP BY substr(ts, 1, 10)",
            params![window_start, today_start],
            |r| r.get(0),
        )
        .await?;
    let mut daily: Vec<f64> = rows;
    if daily.is_empty() {
        // No probe_writer history in the baseline window — defer to other
        // triggers (skill-change-count or time-fallback) rather than firing
        // CostSpike with a fabricated p50=0.
        return Ok(None);
    }
    daily.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(&daily, 0.50);
    if today_cost >= k * p50 && today_cost >= min_floor_usd {
        Ok(Some(CostSpikeEvidence {
            today_cost_usd: today_cost,
            baseline_p50_usd: p50,
            k,
            min_floor_usd,
        }))
    } else {
        Ok(None)
    }
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

    #[tokio::test]
    async fn compute_returns_insufficient_when_below_min_sample() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        for i in 0..5 {
            insert_interactive(&conn, &sample(0.01, 1, Some(i * 100)), 1, 0)
                .await
                .unwrap();
        }
        let b = compute(&conn, 14, 20).await.unwrap();
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

    #[tokio::test]
    async fn compute_returns_available_when_at_least_min_sample() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        for i in 0..50 {
            let cost = 0.01 * (i + 1) as f64;
            insert_interactive(
                &conn,
                &sample(cost, i as u32 + 1, Some((i + 1) * 100)),
                1,
                0,
            )
            .await
            .unwrap();
        }
        let b = compute(&conn, 14, 20).await.unwrap();
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

    #[tokio::test]
    async fn compute_excludes_non_foreground_sources() {
        use crate::usage::insert::{insert_cron, insert_learning_curator};
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        // 25 foreground + 25 cron + 25 curator; baseline counts only foreground.
        for _ in 0..25 {
            insert_interactive(&conn, &sample(0.10, 5, Some(1000)), 1, 0)
                .await
                .unwrap();
        }
        for _ in 0..25 {
            insert_cron(&conn, &sample(0.99, 99, None), "j")
                .await
                .unwrap();
        }
        for _ in 0..25 {
            insert_learning_curator(&conn, &sample(0.99, 99, None))
                .await
                .unwrap();
        }
        let b = compute(&conn, 14, 20).await.unwrap();
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

    #[tokio::test]
    async fn compute_returns_insufficient_with_zero_sample_on_empty_db() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        let b = compute(&conn, 14, 20).await.unwrap();
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

    #[tokio::test]
    async fn compute_excludes_null_wall_elapsed_from_elapsed_baseline_only() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        // 30 rows total, 10 with NULL elapsed, 20 with elapsed.
        for _ in 0..10 {
            insert_interactive(&conn, &sample(0.05, 2, None), 1, 0)
                .await
                .unwrap();
        }
        for i in 0..20 {
            insert_interactive(&conn, &sample(0.05, 2, Some((i + 1) * 100)), 1, 0)
                .await
                .unwrap();
        }
        let b = compute(&conn, 14, 20).await.unwrap();
        assert_eq!(b.sample_size, 30);
        assert!(matches!(b.cost_usd, BaselineMetric::Available { .. }));
        // Elapsed baseline has 20 samples; passes min_sample=20.
        assert!(matches!(
            b.wall_elapsed_ms,
            BaselineMetric::Available { .. }
        ));
    }
}

#[cfg(test)]
mod cost_spike_tests {
    use super::*;
    use crate::usage::UsageBreakdown;
    use crate::usage::insert::insert_learning_probe_writer;
    use right_db::open_connection;
    use tempfile::tempdir;

    fn b(cost: f64) -> UsageBreakdown {
        UsageBreakdown {
            session_uuid: "s".into(),
            total_cost_usd: cost,
            num_turns: 1,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests: 0,
            web_fetch_requests: 0,
            model_usage_json: "{}".into(),
            api_key_source: "none".into(),
            wall_elapsed_ms: None,
        }
    }

    #[tokio::test]
    async fn returns_none_when_today_below_floor() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_learning_probe_writer(&conn, &b(0.01), 1, 0)
            .await
            .unwrap();
        let now = Utc::now();
        let r = check_probe_writer_cost_spike(&conn, now, 14, 3.0, 0.05)
            .await
            .unwrap();
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn returns_none_when_no_baseline_history() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        insert_learning_probe_writer(&conn, &b(0.20), 1, 0)
            .await
            .unwrap();
        let now = Utc::now();
        let r = check_probe_writer_cost_spike(&conn, now, 14, 3.0, 0.05)
            .await
            .unwrap();
        // No prior probe_writer days in the baseline window — must not fire.
        // Skill-change-count or time-fallback triggers handle the new-agent case.
        assert!(r.is_none());
    }

    /// Regression: p50=0.02/day, floor=0.05, k=3.0, today=0.08.
    /// today >= k*p50 (0.06) AND today >= floor (0.05) → must fire.
    /// Old code: threshold = k * max(p50, floor) = 3 * 0.05 = 0.15 → 0.08 < 0.15 → None (bug).
    ///
    /// The baseline query groups by day and sums; one row per day at 0.02 gives p50=0.02.
    #[tokio::test]
    async fn fires_when_today_exceeds_k_times_p50_and_floor_independently() {
        let dir = tempdir().unwrap();
        let conn = open_connection(dir.path(), true).await.unwrap();
        // One row per day for 7 days in the baseline window, each costing 0.02.
        // p50 of [0.02, 0.02, 0.02, 0.02, 0.02, 0.02, 0.02] = 0.02.
        for day in 1..=7u32 {
            let ts = (Utc::now() - chrono::Duration::days(day as i64))
                .format("%Y-%m-%dT12:00:00Z")
                .to_string();
            conn.execute(
                "INSERT INTO usage_events \
                 (session_uuid, source, ts, total_cost_usd, num_turns, \
                  input_tokens, output_tokens, cache_creation_tokens, \
                  cache_read_tokens, web_search_requests, web_fetch_requests, \
                  model_usage_json, api_key_source) \
                 VALUES ('s', 'learning_probe_writer', ?1, 0.02, 1, 0, 0, 0, 0, 0, 0, '{}', 'none')",
                [&ts],
            )
            .await
            .unwrap();
        }
        // Today's spend: 0.08 >= k*p50=0.06 AND 0.08 >= floor=0.05 → must fire.
        insert_learning_probe_writer(&conn, &b(0.08), 1, 0)
            .await
            .unwrap();
        let now = Utc::now();
        let r = check_probe_writer_cost_spike(&conn, now, 14, 3.0, 0.05)
            .await
            .unwrap();
        assert!(
            r.is_some(),
            "expected Some: today=0.08 >= k*p50=0.06 and >= floor=0.05"
        );
        let ev = r.unwrap();
        assert!((ev.today_cost_usd - 0.08).abs() < 1e-9);
        assert!((ev.baseline_p50_usd - 0.02).abs() < 1e-9);
    }
}
