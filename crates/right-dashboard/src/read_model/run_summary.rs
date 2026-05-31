//! Shared SQL column list, `FROM` clause, and row-mapper for
//! `async_runs` → `RunSummary` queries, reused across read models to keep
//! column alignment consistent.

use crate::api_types::RunSummary;

pub(crate) const RUN_SUMMARY_COLUMNS: &str =
    "ar.id, ar.kind, ar.producer_ref, ar.status, ar.started_at, ar.finished_at,
        ar.exit_code, ar.delivery_status, ar.delivery_required, ar.delivery_json,
        ar.run_note, costs.cost_usd";

pub(crate) const RUN_SUMMARY_FROM: &str = "FROM async_runs ar
 LEFT JOIN (
    SELECT session_uuid, SUM(total_cost_usd) AS cost_usd
    FROM usage_events
    GROUP BY session_uuid
 ) costs ON costs.session_uuid = ar.run_session_id";

pub(crate) fn run_summary_from_row(
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
