//! Shared SQL column list, `FROM` clause, and row-mapper for
//! `async_runs` → `RunSummary` queries, reused across read models to keep
//! column alignment consistent.

use crate::api_types::RunSummary;
use chrono::{DateTime, Utc};
use right_db::{Connection, params};

use super::{ReadModelError, coarse_timestamp_bounds, parse_utc};

pub(super) const RUN_SUMMARY_COLUMNS: &str =
    "ar.id, ar.kind, ar.producer_ref, ar.status, ar.started_at, ar.finished_at,
        ar.exit_code, ar.delivery_status, ar.delivery_required, ar.delivery_json,
        ar.run_note, costs.cost_usd";

pub(super) const RUN_SUMMARY_FROM: &str = "FROM async_runs ar
 LEFT JOIN (
    SELECT session_uuid, SUM(total_cost_usd) AS cost_usd
    FROM usage_events
    GROUP BY session_uuid
 ) costs ON costs.session_uuid = ar.run_session_id";

pub(super) fn run_summary_from_row(
    row: &right_db::row::Row<'_>,
) -> Result<RunSummary, right_db::DbError> {
    Ok(RunSummary {
        id: row.get(0)?,
        kind: row.get(1)?,
        producer_ref: row.get(2)?,
        status: row.get(3)?,
        started_at: row.get(4)?,
        finished_at: row.get(5)?,
        exit_code: row.get(6)?,
        delivery_status: row.get(7)?,
        delivery_required: row.get::<_, i64>(8)? != 0,
        delivery_kind: delivery_kind_from_json(row.get::<_, Option<String>>(9)?.as_deref()),
        run_note: row.get(10)?,
        cost_usd: row.get(11)?,
    })
}

fn delivery_kind_from_json(json: Option<&str>) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(json?).ok()?;
    value.get("kind")?.as_str().map(ToOwned::to_owned)
}

/// Failed `async_runs` whose window timestamp falls in `[since, now]`, newest
/// first. `kind` optionally restricts to a single run kind (e.g. `"cron"`);
/// `None` matches all kinds. Returns the exact count of matching rows and the
/// newest `FAILURE_SAMPLE_LIMIT` of them — the count may exceed the list
/// length. Shared by the dashboard overview (24h, all kinds) and activity
/// overview (7d, cron-only) so both stay aligned with `RUN_SUMMARY_COLUMNS`.
pub(super) async fn failed_runs_in_window(
    conn: &Connection,
    now: &DateTime<Utc>,
    since: &DateTime<Utc>,
    kind: Option<&str>,
) -> Result<(usize, Vec<RunSummary>), ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since, now);
    // win_ts is appended immediately after the RUN_SUMMARY_COLUMNS list.
    let sql = format!(
        "SELECT {RUN_SUMMARY_COLUMNS},
                COALESCE(ar.finished_at, ar.updated_at, ar.created_at) AS win_ts
         {RUN_SUMMARY_FROM}
         WHERE ar.status = 'failed'
           AND (?3 IS NULL OR ar.kind = ?3)
           AND COALESCE(ar.finished_at, ar.updated_at, ar.created_at) >= ?1
           AND COALESCE(ar.finished_at, ar.updated_at, ar.created_at) <= ?2
         ORDER BY COALESCE(ar.finished_at, ar.updated_at, ar.created_at) DESC, ar.created_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![coarse_since, coarse_until, kind], |row| {
            Ok((run_summary_from_row(row)?, row.get::<_, String>(12)?))
        })
        .await?;
    let mut out = Vec::new();
    for row in rows {
        let (run, win_ts) = row?;
        let ts = parse_utc(&win_ts)?;
        if ts >= *since && ts <= *now {
            out.push(run);
        }
    }
    let total = out.len();
    out.truncate(super::FAILURE_SAMPLE_LIMIT);
    Ok((total, out))
}
