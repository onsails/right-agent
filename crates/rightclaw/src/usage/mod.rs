//! Usage telemetry — per-invocation CC token/cost tracking.
//!
//! One `UsageEvent` row per `claude -p` invocation, written when the
//! stream-json `result` event is received. Read by the `/usage` Telegram
//! command via `aggregate`.

pub mod aggregate;
pub mod error;
pub mod format;
pub mod insert;
pub mod pricing;

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
    /// `apiKeySource` captured from the CC `system/init` event.
    /// 'none' = OAuth/setup-token (subscription). Other values = API key mode.
    pub api_key_source: String,
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
    pub subscription_cost_usd: f64,
    pub api_cost_usd: f64,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_breakdown_has_api_key_source_field() {
        let b = UsageBreakdown {
            session_uuid: "s".into(),
            total_cost_usd: 0.0,
            num_turns: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests: 0,
            web_fetch_requests: 0,
            model_usage_json: "{}".into(),
            api_key_source: "none".into(),
        };
        assert_eq!(b.api_key_source, "none");
    }

    #[test]
    fn window_summary_has_billing_split_fields() {
        let w = WindowSummary {
            source: "interactive".into(),
            cost_usd: 1.0,
            subscription_cost_usd: 0.6,
            api_cost_usd: 0.4,
            turns: 5,
            invocations: 3,
            input_tokens: 100,
            output_tokens: 200,
            cache_creation_tokens: 50,
            cache_read_tokens: 400,
            web_search_requests: 0,
            web_fetch_requests: 0,
            per_model: BTreeMap::new(),
        };
        assert!((w.subscription_cost_usd - 0.6).abs() < 1e-9);
        assert!((w.api_cost_usd - 0.4).abs() < 1e-9);
    }
}
