//! Telegram HTML message rendering for `/usage`.

use crate::usage::WindowSummary;

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

pub fn format_summary_message(w: &AllWindows) -> String {
    let total_invocations = w.all_interactive.invocations + w.all_cron.invocations;
    if total_invocations == 0 {
        return "No usage recorded yet.".to_string();
    }

    let total_cost = w.all_interactive.cost_usd + w.all_cron.cost_usd;

    let mut out = String::new();
    out.push_str("\u{1f4ca} <b>Usage Summary</b> (UTC)\n\n");
    out.push_str(&render_window("Today", &w.today_interactive, &w.today_cron));
    out.push_str(&render_window("Last 7 days", &w.week_interactive, &w.week_cron));
    out.push_str(&render_window("Last 30 days", &w.month_interactive, &w.month_cron));
    out.push_str(&render_window("All time", &w.all_interactive, &w.all_cron));
    out.push_str(&format!("\n<b>Total all time:</b> {}\n", format_cost(total_cost)));
    out
}

fn render_window(title: &str, interactive: &WindowSummary, cron: &WindowSummary) -> String {
    let mut s = format!("\u{2501}\u{2501} <b>{}</b> \u{2501}\u{2501}\n", html_escape(title));
    if interactive.invocations == 0 && cron.invocations == 0 {
        s.push_str("(no activity)\n\n");
        return s;
    }
    if interactive.invocations > 0 {
        s.push_str(&render_source("\u{1f4ac} Interactive", interactive, "invocations"));
    }
    if cron.invocations > 0 {
        s.push_str(&render_source("\u{23f0} Cron", cron, "runs"));
    }
    let web_s = interactive.web_search_requests + cron.web_search_requests;
    let web_f = interactive.web_fetch_requests + cron.web_fetch_requests;
    if web_s > 0 || web_f > 0 {
        s.push_str(&format!("\u{1f50e} Web tools: {web_s} search, {web_f} fetch\n"));
    }
    s.push('\n');
    s
}

fn render_source(label: &str, w: &WindowSummary, unit: &str) -> String {
    let mut s = format!(
        "{label}: {cost} · {turns} turns · {count} {unit}\n   Tokens: in {inp}, out {out}, cache_c {cc}, cache_r {cr}\n",
        cost = format_cost(w.cost_usd),
        turns = w.turns,
        count = w.invocations,
        unit = unit,
        inp = format_count(w.input_tokens),
        out = format_count(w.output_tokens),
        cc = format_count(w.cache_creation_tokens),
        cr = format_count(w.cache_read_tokens),
    );
    // Per-model lines (sorted by cost desc for readability).
    let mut models: Vec<_> = w.per_model.iter().collect();
    models.sort_by(|a, b| b.1.cost_usd.partial_cmp(&a.1.cost_usd).unwrap_or(std::cmp::Ordering::Equal));
    for (name, totals) in models {
        s.push_str(&format!(
            "   \u{2022} {} \u{2014} {}\n",
            html_escape(name),
            format_cost(totals.cost_usd),
        ));
    }
    s
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

    fn with(source: &str, cost: f64, invocations: u64, model: &str, model_cost: f64) -> WindowSummary {
        let mut per_model = BTreeMap::new();
        per_model.insert(model.to_string(), ModelTotals {
            cost_usd: model_cost,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        });
        WindowSummary {
            source: source.into(),
            cost_usd: cost,
            turns: 3,
            invocations,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests: 0,
            web_fetch_requests: 0,
            per_model,
        }
    }

    #[test]
    fn empty_db_returns_no_usage_line() {
        let w = AllWindows {
            today_interactive: empty("interactive"),
            today_cron: empty("cron"),
            week_interactive: empty("interactive"),
            week_cron: empty("cron"),
            month_interactive: empty("interactive"),
            month_cron: empty("cron"),
            all_interactive: empty("interactive"),
            all_cron: empty("cron"),
        };
        assert_eq!(format_summary_message(&w), "No usage recorded yet.");
    }

    #[test]
    fn empty_window_shows_no_activity() {
        let w = AllWindows {
            today_interactive: empty("interactive"),
            today_cron: empty("cron"),
            week_interactive: with("interactive", 0.1, 1, "sonnet", 0.1),
            week_cron: empty("cron"),
            month_interactive: with("interactive", 0.1, 1, "sonnet", 0.1),
            month_cron: empty("cron"),
            all_interactive: with("interactive", 0.1, 1, "sonnet", 0.1),
            all_cron: empty("cron"),
        };
        let msg = format_summary_message(&w);
        assert!(msg.contains("Today"));
        assert!(msg.contains("(no activity)"));
        assert!(msg.contains("Last 7 days"));
        assert!(msg.contains("Interactive"));
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
        let w = AllWindows {
            today_interactive: with("interactive", 0.1, 1, "foo<script>", 0.1),
            today_cron: empty("cron"),
            week_interactive: empty("interactive"),
            week_cron: empty("cron"),
            month_interactive: empty("interactive"),
            month_cron: empty("cron"),
            all_interactive: with("interactive", 0.1, 1, "foo<script>", 0.1),
            all_cron: empty("cron"),
        };
        let msg = format_summary_message(&w);
        assert!(msg.contains("foo&lt;script&gt;"));
        assert!(!msg.contains("<script>"));
    }

    #[test]
    fn total_line_sums_interactive_and_cron() {
        let w = AllWindows {
            today_interactive: with("interactive", 0.5, 2, "sonnet", 0.5),
            today_cron: with("cron", 0.3, 1, "sonnet", 0.3),
            week_interactive: with("interactive", 0.5, 2, "sonnet", 0.5),
            week_cron: with("cron", 0.3, 1, "sonnet", 0.3),
            month_interactive: with("interactive", 0.5, 2, "sonnet", 0.5),
            month_cron: with("cron", 0.3, 1, "sonnet", 0.3),
            all_interactive: with("interactive", 0.5, 2, "sonnet", 0.5),
            all_cron: with("cron", 0.3, 1, "sonnet", 0.3),
        };
        let msg = format_summary_message(&w);
        assert!(msg.contains("Total all time:</b> $0.80"));
    }
}
