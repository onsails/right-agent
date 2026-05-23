use std::io;

use crate::api_types::{OverviewResponse, RunDetailResponse};
use rusqlite::Connection;
use thiserror::Error;

#[path = "read_model/activity.rs"]
pub mod activity;
#[path = "read_model/dashboard_overview.rs"]
pub mod dashboard_overview;
#[path = "read_model/learning.rs"]
pub mod learning;
#[path = "read_model/learning_episodes.rs"]
pub mod learning_episodes;
#[path = "read_model/learning_outcomes.rs"]
mod learning_outcomes;
#[path = "read_model/usage.rs"]
pub mod usage;

#[derive(Debug, Error)]
pub enum ReadModelError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    ParseTimestamp(#[from] chrono::ParseError),
    #[error("invalid start-of-day for timestamp {0}")]
    InvalidStartOfDay(String),
}

pub type OverviewInput = activity::ActivityOverviewInput;

pub fn overview(
    conn: &Connection,
    input: OverviewInput,
) -> Result<OverviewResponse, ReadModelError> {
    activity::activity_overview(conn, input)
}

pub fn run_detail(
    conn: &Connection,
    run_id: &str,
    max_lines: usize,
) -> Result<Option<RunDetailResponse>, ReadModelError> {
    activity::activity_run_detail(conn, run_id, max_lines)
}
