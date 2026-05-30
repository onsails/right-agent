//! Telegram-facing copy for sandbox-backend outages. Consequence-first,
//! HTML-escaped for `ParseMode::Html`, no raw CLI prefixes.

use crate::cc::markdown_utils::html_escape;
use right_openshell::diagnosis::GatewayDiagnosis;

/// Message shown when a sandboxed turn is blocked by an unavailable backend.
pub(crate) fn unavailable_message(d: &GatewayDiagnosis) -> String {
    let summary = html_escape(&d.summary);
    let fix = d.fixes.first().map(|f| html_escape(f)).unwrap_or_default();
    format!(
        "⚠️ I can't run right now — my secure sandbox backend is offline.\n\
         Likely cause: {summary}.\n\
         Fix: {fix}."
    )
}

/// Sent once per affected chat when the backend recovers.
pub(crate) fn back_online_message() -> String {
    "✅ Sandbox back online — I'm ready.".to_owned()
}

#[cfg(test)]
#[path = "sandbox_copy_tests.rs"]
mod tests;
