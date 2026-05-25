pub(crate) fn success(html: &str) -> String {
    format!("✅ {html}")
}

pub(crate) fn warning(html: &str) -> String {
    format!("⚠️ {html}")
}

pub(crate) fn error(html: &str) -> String {
    format!("❌ {html}")
}

pub(crate) fn action(html: &str) -> String {
    format!("➡️ {html}")
}

pub(crate) fn blocks<I, S>(blocks: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    blocks
        .into_iter()
        .map(|block| block.as_ref().to_string())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn severity_lines_have_telegram_status_icons() {
        assert_eq!(success("Added MCP server."), "✅ Added MCP server.");
        assert_eq!(
            warning("Plain HTTP: trusted/encrypted networks only."),
            "⚠️ Plain HTTP: trusted/encrypted networks only."
        );
        assert_eq!(error("Request failed."), "❌ Request failed.");
        assert_eq!(
            action("Run <code>/mcp auth obsidian</code>."),
            "➡️ Run <code>/mcp auth obsidian</code>."
        );
    }

    #[tokio::test]
    async fn message_blocks_are_separated_for_scanability() {
        assert_eq!(
            blocks([
                success("Added MCP server. 15 tools available."),
                warning("Plain HTTP: trusted/encrypted networks only."),
            ]),
            "✅ Added MCP server. 15 tools available.\n\n⚠️ Plain HTTP: trusted/encrypted networks only."
        );
    }
}
