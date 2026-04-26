//! Per-model Anthropic pricing table used for cache-savings estimation.
//!
//! Rates source: https://www.anthropic.com/pricing
//! Update when Anthropic changes published per-token rates.

/// Per-million-token input and output rates for a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

/// Look up published rates for a model. Returns `None` for unknown models —
/// callers must render gracefully (e.g. omit the dollar portion of a
/// cache-savings line).
pub fn lookup(model: &str) -> Option<ModelPricing> {
    if model.starts_with("claude-sonnet-4-6") {
        return Some(ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        });
    }
    if model.starts_with("claude-opus-4-7") {
        return Some(ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
        });
    }
    if model.starts_with("claude-haiku-4-5") {
        return Some(ModelPricing {
            input_per_mtok: 0.80,
            output_per_mtok: 4.0,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sonnet_4_6_known() {
        let p = lookup("claude-sonnet-4-6").expect("must be known");
        assert!((p.input_per_mtok - 3.0).abs() < 1e-9);
        assert!((p.output_per_mtok - 15.0).abs() < 1e-9);
    }

    #[test]
    fn opus_4_7_known() {
        let p = lookup("claude-opus-4-7").expect("must be known");
        assert!((p.input_per_mtok - 15.0).abs() < 1e-9);
        assert!((p.output_per_mtok - 75.0).abs() < 1e-9);
    }

    #[test]
    fn haiku_dated_variant_matches() {
        let p = lookup("claude-haiku-4-5-20251001").expect("dated haiku must match");
        assert!((p.input_per_mtok - 0.80).abs() < 1e-9);
        assert!((p.output_per_mtok - 4.0).abs() < 1e-9);
    }

    #[test]
    fn haiku_undated_variant_matches() {
        let p = lookup("claude-haiku-4-5").expect("undated haiku must match");
        assert!((p.input_per_mtok - 0.80).abs() < 1e-9);
        assert!((p.output_per_mtok - 4.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(lookup("claude-fake-9-0").is_none());
        assert!(lookup("").is_none());
    }
}
