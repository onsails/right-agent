//! Per-model Anthropic pricing table used for cache-savings estimation.
//!
//! Rates come from the `llm-prices` crate (dated, source-cited list prices).

/// Per-million-token input, output and cache-read rates for a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
}

/// Look up published rates for a model. Returns `None` for unknown models —
/// callers must render gracefully (e.g. omit the dollar portion of a
/// cache-savings line).
pub fn lookup(model: &str) -> Option<ModelPricing> {
    // Context-window suffixes such as `[1m]` do not change the model id.
    let id = model.split('[').next().unwrap_or(model);
    let p = llm_prices::lookup(id)?;
    let input = p.input?;
    Some(ModelPricing {
        input_per_mtok: input,
        output_per_mtok: p.output?,
        cache_read_per_mtok: p.cache_read.unwrap_or(input),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rates(model: &str, input: f64, output: f64, cache_read: f64) {
        let p = lookup(model).unwrap_or_else(|| panic!("{model} must be known"));
        assert!((p.input_per_mtok - input).abs() < 1e-9, "{model} input");
        assert!((p.output_per_mtok - output).abs() < 1e-9, "{model} output");
        assert!(
            (p.cache_read_per_mtok - cache_read).abs() < 1e-9,
            "{model} cache read"
        );
    }

    #[test]
    fn sonnet_4_6_known() {
        assert_rates("claude-sonnet-4-6", 3.0, 15.0, 0.3);
    }

    #[test]
    fn opus_4_7_known() {
        assert_rates("claude-opus-4-7", 5.0, 25.0, 0.5);
    }

    #[test]
    fn opus_4_8_known() {
        assert_rates("claude-opus-4-8", 5.0, 25.0, 0.5);
    }

    #[test]
    fn opus_4_8_one_m_variant_matches() {
        assert_rates("claude-opus-4-8[1m]", 5.0, 25.0, 0.5);
    }

    #[test]
    fn haiku_dated_variant_matches() {
        assert_rates("claude-haiku-4-5-20251001", 1.0, 5.0, 0.1);
    }

    #[test]
    fn haiku_undated_variant_matches() {
        assert_rates("claude-haiku-4-5", 1.0, 5.0, 0.1);
    }

    #[test]
    fn models_outside_the_old_table_are_priced() {
        assert_rates("claude-opus-4-5", 5.0, 25.0, 0.5);
        assert_rates("claude-sonnet-4-5-20250929", 3.0, 15.0, 0.3);
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(lookup("claude-fake-9-0").is_none());
        assert!(lookup("").is_none());
    }
}
