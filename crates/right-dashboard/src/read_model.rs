use std::io;

use crate::api_types::{OverviewResponse, RunDetailResponse};
use chrono::{DateTime, Duration, Utc};
use right_db::Connection;
use thiserror::Error;

#[path = "read_model/activity.rs"]
pub mod activity;
#[path = "read_model/dashboard_overview.rs"]
pub mod dashboard_overview;
#[path = "read_model/learning.rs"]
pub mod learning;
#[path = "read_model/learning_outcomes.rs"]
mod learning_outcomes;
#[path = "read_model/usage.rs"]
pub mod usage;

#[derive(Debug, Error)]
pub enum ReadModelError {
    #[error(transparent)]
    Db(#[from] right_db::DbError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    ParseTimestamp(#[from] chrono::ParseError),
    #[error("invalid start-of-day for timestamp {0}")]
    InvalidStartOfDay(String),
    #[error("invalid lifecycle row: {0}")]
    InvalidLifecycle(String),
}

pub type OverviewInput = activity::ActivityOverviewInput;

pub async fn overview(
    conn: &Connection,
    input: OverviewInput,
) -> Result<OverviewResponse, ReadModelError> {
    activity::activity_overview(conn, input).await
}

pub async fn run_detail(
    conn: &Connection,
    run_id: &str,
    max_lines: usize,
) -> Result<Option<RunDetailResponse>, ReadModelError> {
    activity::activity_run_detail(conn, run_id, max_lines).await
}

pub(crate) fn parse_utc(value: &str) -> Result<DateTime<Utc>, ReadModelError> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

/// Widen `[since, now]` by one day on each side for use as inclusive SQL
/// bounds. Callers MUST re-filter results against the precise `since`/`now`
/// window in Rust; `count_parsed_window_rows` is the canonical consumer.
///
/// Rationale: dashboard timestamps are written as RFC 3339 strings whose
/// representation can shift around DST/leap second boundaries or be produced
/// by different writers with slightly different precision. A 1-day cushion
/// lets the SQL prefilter stay cheap (it just uses lexicographic string
/// comparison on the indexed column) while keeping correctness on the Rust
/// side.
pub(crate) fn coarse_timestamp_bounds(
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> (String, String) {
    (
        (*since - Duration::days(1)).to_rfc3339(),
        (*now + Duration::days(1)).to_rfc3339(),
    )
}

pub(crate) fn count_parsed_window_rows(
    rows: impl Iterator<Item = Result<String, right_db::DbError>>,
    since: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<i64, ReadModelError> {
    let mut count = 0;
    for row in rows {
        let timestamp = parse_utc(&row?)?;
        if timestamp >= *since && timestamp <= *now {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coarse_bounds_widen_window_by_one_day_each_side() {
        let since = DateTime::parse_from_rfc3339("2026-05-10T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let now = DateTime::parse_from_rfc3339("2026-05-12T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let (coarse_since, coarse_until) = coarse_timestamp_bounds(&since, &now);
        assert_eq!(coarse_since, "2026-05-09T12:00:00+00:00");
        assert_eq!(coarse_until, "2026-05-13T12:00:00+00:00");
    }

    #[test]
    fn count_parsed_window_rows_drops_rows_outside_precise_window() {
        let since = DateTime::parse_from_rfc3339("2026-05-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let now = DateTime::parse_from_rfc3339("2026-05-11T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let rows = [
            "2026-05-09T23:00:00Z", // inside coarse, before precise
            "2026-05-10T00:00:00Z", // boundary in
            "2026-05-10T12:00:00Z", // inside both
            "2026-05-11T00:00:00Z", // boundary in
            "2026-05-11T01:00:00Z", // inside coarse, after precise
        ];
        let count =
            count_parsed_window_rows(rows.iter().map(|s| Ok((*s).to_owned())), &since, &now)
                .unwrap();
        assert_eq!(count, 3, "rows outside precise window must be dropped");
    }
}
