//! Telegram HTML message rendering for `/usage`.

use crate::usage::{ModelTotals, WindowSummary, pricing};

/// All windows × sources, as produced by the handler before rendering.
pub struct AllWindows {
    pub today_interactive: WindowSummary,
    pub today_cron: WindowSummary,
    pub week_interactive: WindowSummary,
    pub week_cron: WindowSummary,
    pub month_interactive: WindowSummary,
    pub month_cron: WindowSummary,
    pub all_interactive: WindowSummary,
    pub all_cron: WindowSummary,
}

/// Render the complete `/usage` summary as Telegram HTML. When `detail` is
/// true each source block also renders a raw-tokens line.
pub fn format_summary_message(w: &AllWindows, detail: bool) -> String {
    let total_invocations = w.all_interactive.invocations + w.all_cron.invocations;
    if total_invocations == 0 {
        return "No usage recorded yet.".to_string();
    }

    let total_cost = w.all_interactive.cost_usd + w.all_cron.cost_usd;
    let total_sub = w.all_interactive.subscription_cost_usd + w.all_cron.subscription_cost_usd;
    let total_api = w.all_interactive.api_cost_usd + w.all_cron.api_cost_usd;

    let mut out = String::new();
    out.push_str("\u{1f4ca} <b>Usage Summary</b> (UTC)\n\n");
    out.push_str(&render_window("Today", &w.today_interactive, &w.today_cron, detail));
    out.push_str(&render_window("Last 7 days", &w.week_interactive, &w.week_cron, detail));
    out.push_str(&render_window("Last 30 days", &w.month_interactive, &w.month_cron, detail));
    out.push_str(&render_window("All time", &w.all_interactive, &w.all_cron, detail));

    // Total footer: plain when single mode, split when both present.
    if total_sub > 0.0 && total_api > 0.0 {
        out.push_str(&format!(
            "\n<b>Total retail:</b> {} · subscription: {} · API-billed: {}\n",
            format_cost(total_cost),
            format_cost(total_sub),
            format_cost(total_api),
        ));
    } else {
        out.push_str(&format!("\n<b>Total retail:</b> {}\n", format_cost(total_cost)));
    }
    out
}

fn render_window(title: &str, interactive: &WindowSummary, cron: &WindowSummary, detail: bool) -> String {
    let mut s = format!("\u{2501}\u{2501} <b>{}</b> \u{2501}\u{2501}\n", html_escape(title));
    if interactive.invocations == 0 && cron.invocations == 0 {
        s.push_str("(no activity)\n\n");
        return s;
    }
    if interactive.invocations > 0 {
        s.push_str(&render_source("\u{1f4ac} Interactive", interactive, "sessions", detail));
    }
    if cron.invocations > 0 {
        s.push_str(&render_source("\u{23f0} Cron", cron, "runs", detail));
    }
    let web_s = interactive.web_search_requests + cron.web_search_requests;
    let web_f = interactive.web_fetch_requests + cron.web_fetch_requests;
    if web_s > 0 || web_f > 0 {
        s.push_str(&format!("\u{1f50e} Web: {web_s} searches, {web_f} fetches\n"));
    }

    // Footer per window.
    let sub = interactive.subscription_cost_usd + cron.subscription_cost_usd;
    let api = interactive.api_cost_usd + cron.api_cost_usd;
    if sub > 0.0 && api > 0.0 {
        s.push_str(&format!(
            "Subscription: {} · API-billed: {}\n",
            format_cost(sub),
            format_cost(api),
        ));
    } else if api == 0.0 && sub > 0.0 {
        s.push_str("Subscription covers this (Claude subscription via setup-token)\n");
    } else if sub == 0.0 && api > 0.0 {
        s.push_str("Billed via API key\n");
    }
    s.push('\n');
    s
}

fn render_source(label: &str, w: &WindowSummary, unit: &str, detail: bool) -> String {
    let billing_tag = match (w.subscription_cost_usd > 0.0, w.api_cost_usd > 0.0) {
        (true, true) => " (Mixed)",
        (false, true) => " (API-billed)",
        _ => "",
    };
    let mut s = format!(
        "{label}{billing_tag}: {cost} retail · {turns} turns · {count} {unit}\n",
        cost = format_cost(w.cost_usd),
        turns = w.turns,
        count = w.invocations,
    );

    // Per-model lines sorted by cost desc for readability.
    let mut models: Vec<_> = w.per_model.iter().collect();
    models.sort_by(|a, b| b.1.cost_usd.partial_cmp(&a.1.cost_usd).unwrap_or(std::cmp::Ordering::Equal));
    for (name, totals) in &models {
        s.push_str(&format!(
            "  {} \u{2014} {}\n",
            html_escape(name),
            format_cost(totals.cost_usd),
        ));
    }

    // Cache effectiveness line (omitted when no cache reads in this window).
    if let Some(line) = format_cache_line(&w.per_model) {
        s.push_str("  ");
        s.push_str(&line);
        s.push('\n');
    }

    // Detail mode: raw-tokens line.
    if detail {
        s.push_str(&format!(
            "  Tokens: {} new in, {} out, {} cache-created, {} cache-read\n",
            format_count(w.input_tokens),
            format_count(w.output_tokens),
            format_count(w.cache_creation_tokens),
            format_count(w.cache_read_tokens),
        ));
    }
    s
}

/// Build the "Cache: H% hit rate, saved ~$C.CC" line for a window's per-model
/// map. Returns `None` when every model has zero cache reads (nothing to say).
/// When some models are priced and others aren't, the dollar savings still
/// represents only the priced portion — accepted as "estimate", not audit.
fn format_cache_line(per_model: &std::collections::BTreeMap<String, ModelTotals>) -> Option<String> {
    let mut total_cache_read: u64 = 0;
    let mut total_input_bearing: u64 = 0; // input + cache_creation + cache_read
    let mut total_savings_usd: f64 = 0.0;
    let mut any_priced = false;

    for (model, t) in per_model {
        total_cache_read = total_cache_read.saturating_add(t.cache_read_tokens);
        total_input_bearing = total_input_bearing
            .saturating_add(t.input_tokens)
            .saturating_add(t.cache_creation_tokens)
            .saturating_add(t.cache_read_tokens);
        if let Some(p) = pricing::lookup(model) {
            any_priced = true;
            // Cached reads cost 10% of regular input, so 90% of the fresh-input
            // rate is saved per cached token.
            total_savings_usd += t.cache_read_tokens as f64 * p.input_per_mtok * 0.9 / 1_000_000.0;
        }
    }

    if total_cache_read == 0 {
        return None;
    }

    let hit_rate = if total_input_bearing == 0 {
        0.0
    } else {
        total_cache_read as f64 / total_input_bearing as f64
    };
    let pct = (hit_rate * 100.0).round() as u32;

    if any_priced {
        Some(format!("Cache: {pct}% hit rate, saved ~{}", format_cost(total_savings_usd)))
    } else {
        Some(format!("Cache: {pct}% hit rate"))
    }
}

fn format_cost(v: f64) -> String {
    if v > 0.0 && v < 0.01 {
        "&lt;$0.01".to_string()
    } else {
        format!("${v:.2}")
    }
}

fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{ModelTotals, WindowSummary};
    use std::collections::BTreeMap;

    fn empty(source: &str) -> WindowSummary {
        WindowSummary { source: source.into(), ..Default::default() }
    }

    fn sub_only(source: &str, cost: f64, model: &str) -> WindowSummary {
        let mut per_model = BTreeMap::new();
        per_model.insert(model.to_string(), ModelTotals {
            cost_usd: cost,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 50,
            cache_read_tokens: 300,
        });
        WindowSummary {
            source: source.into(),
            cost_usd: cost,
            subscription_cost_usd: cost,
            api_cost_usd: 0.0,
            turns: 3,
            invocations: 1,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 50,
            cache_read_tokens: 300,
            web_search_requests: 0,
            web_fetch_requests: 0,
            per_model,
        }
    }

    fn api_only(source: &str, cost: f64, model: &str) -> WindowSummary {
        let mut w = sub_only(source, cost, model);
        w.subscription_cost_usd = 0.0;
        w.api_cost_usd = cost;
        w
    }

    fn all_empty() -> AllWindows {
        AllWindows {
            today_interactive: empty("interactive"),
            today_cron: empty("cron"),
            week_interactive: empty("interactive"),
            week_cron: empty("cron"),
            month_interactive: empty("interactive"),
            month_cron: empty("cron"),
            all_interactive: empty("interactive"),
            all_cron: empty("cron"),
        }
    }

    #[test]
    fn empty_db_returns_no_usage_line() {
        assert_eq!(format_summary_message(&all_empty(), false), "No usage recorded yet.");
    }

    #[test]
    fn empty_window_shows_no_activity() {
        let mut w = all_empty();
        w.week_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.month_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("Today"));
        assert!(msg.contains("(no activity)"));
        assert!(msg.contains("Last 7 days"));
    }

    #[test]
    fn default_mode_omits_raw_tokens_line() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        assert!(!msg.contains("Tokens:"), "default mode must not include raw tokens");
    }

    #[test]
    fn detail_mode_includes_raw_tokens_line() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, true);
        assert!(msg.contains("Tokens:"));
        assert!(msg.contains("new in"));
        assert!(msg.contains("cache-created"));
        assert!(msg.contains("cache-read"));
    }

    #[test]
    fn cache_line_renders_hit_rate_and_dollar_savings_for_known_model() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        // cache_read=300, total_input=10+50+300=360 → 83%. At $3/Mtok × 0.9 × 300 = $0.00081 → "<$0.01".
        assert!(msg.contains("Cache:"));
        assert!(msg.contains("83%"));
        assert!(msg.contains("saved"));
    }

    #[test]
    fn cache_line_without_dollar_for_unknown_model() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-fake-unknown");
        w.all_interactive = sub_only("interactive", 0.1, "claude-fake-unknown");
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("83%"));
        assert!(!msg.contains("saved"), "unknown model must not render 'saved' clause");
    }

    #[test]
    fn cache_line_omitted_when_no_cache_reads() {
        let mut ws = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        // Zero out cache reads on both the window totals and per-model.
        ws.cache_read_tokens = 0;
        ws.per_model.get_mut("claude-sonnet-4-6").unwrap().cache_read_tokens = 0;
        let mut w = all_empty();
        w.today_interactive = ws.clone();
        w.all_interactive = ws;
        let msg = format_summary_message(&w, false);
        assert!(!msg.contains("Cache:"), "cache line must be omitted when cache_read=0");
    }

    #[test]
    fn subscription_only_window_has_subscription_footnote() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("Subscription covers this"));
    }

    #[test]
    fn api_only_window_labels_block_and_has_api_footnote() {
        let mut w = all_empty();
        w.today_interactive = api_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = api_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("(API-billed)"));
        assert!(msg.contains("Billed via API key"));
        assert!(!msg.contains("Subscription covers this"));
    }

    #[test]
    fn mixed_billing_window_shows_split_footer() {
        let mut ws = sub_only("interactive", 0.30, "claude-sonnet-4-6");
        ws.subscription_cost_usd = 0.20;
        ws.api_cost_usd = 0.10;
        let mut w = all_empty();
        w.today_interactive = ws.clone();
        w.all_interactive = ws;
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("(Mixed)"));
        assert!(msg.contains("Subscription: $0.20"));
        assert!(msg.contains("API-billed: $0.10"));
    }

    #[test]
    fn total_footer_plain_when_single_mode() {
        let mut w = all_empty();
        w.today_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        w.all_interactive = sub_only("interactive", 0.1, "claude-sonnet-4-6");
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("<b>Total retail:</b> $0.10"));
        assert!(!msg.contains("subscription:"), "single-mode footer must not show split");
    }

    #[test]
    fn total_footer_splits_when_both_modes_present() {
        let ws_sub = sub_only("interactive", 0.10, "claude-sonnet-4-6");
        let ws_api = api_only("cron", 0.05, "claude-sonnet-4-6");
        // All-time window needs combined totals for the footer line.
        let mut ws_all = ws_sub.clone();
        ws_all.source = "interactive".into();
        ws_all.cost_usd = 0.10;
        ws_all.subscription_cost_usd = 0.10;
        ws_all.api_cost_usd = 0.0;
        let mut ws_all_cron = ws_api.clone();
        ws_all_cron.cost_usd = 0.05;
        ws_all_cron.subscription_cost_usd = 0.0;
        ws_all_cron.api_cost_usd = 0.05;

        let w = AllWindows {
            today_interactive: ws_sub.clone(),
            today_cron: ws_api.clone(),
            week_interactive: ws_sub.clone(),
            week_cron: ws_api.clone(),
            month_interactive: ws_sub,
            month_cron: ws_api,
            all_interactive: ws_all,
            all_cron: ws_all_cron,
        };
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("<b>Total retail:</b> $0.15"));
        assert!(msg.contains("subscription: $0.10"));
        assert!(msg.contains("API-billed: $0.05"));
    }

    #[test]
    fn cost_below_one_cent_shown_as_less_than() {
        assert_eq!(format_cost(0.003), "&lt;$0.01");
        assert_eq!(format_cost(0.0), "$0.00");
        assert_eq!(format_cost(1.234), "$1.23");
    }

    #[test]
    fn counts_use_k_and_m_suffix() {
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(1_234), "1.2k");
        assert_eq!(format_count(1_234_567), "1.2M");
    }

    #[test]
    fn html_escape_applied_to_model_names() {
        let mut ws = sub_only("interactive", 0.1, "foo<script>");
        // Need cache_read=0 to skip pricing.lookup on the nonsense name.
        ws.cache_read_tokens = 0;
        ws.per_model.get_mut("foo<script>").unwrap().cache_read_tokens = 0;
        let mut w = all_empty();
        w.today_interactive = ws.clone();
        w.all_interactive = ws;
        let msg = format_summary_message(&w, false);
        assert!(msg.contains("foo&lt;script&gt;"));
        assert!(!msg.contains("<script>"));
    }
}
