//! Usage telemetry — per-invocation CC token/cost tracking.
//!
//! One `UsageEvent` row per `claude -p` invocation, written when the
//! stream-json `result` event is received. Read by the `/usage` Telegram
//! command via `aggregate`.

pub mod aggregate;
pub mod error;
pub mod format;
pub mod insert;

use std::collections::BTreeMap;

pub use error::UsageError;

/// Parsed `result` event payload used to write one row.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageBreakdown {
    pub session_uuid: String,
    pub total_cost_usd: f64,
    pub num_turns: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    /// Raw `modelUsage` sub-object as JSON string (preserves per-model fields).
    pub model_usage_json: String,
}

/// Per-model totals, aggregated across rows in a window.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ModelTotals {
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

/// Aggregated summary for one window + source.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct WindowSummary {
    pub source: String,
    pub cost_usd: f64,
    pub turns: u64,
    pub invocations: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub per_model: BTreeMap<String, ModelTotals>,
}
