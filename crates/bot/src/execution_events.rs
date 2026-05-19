const MAX_CONTENT_TEXT_CHARS: usize = 2_000;
const MAX_JSON_STRING_CHARS: usize = 2_000;
const MAX_CONTENT_JSON_CHARS: usize = 8_000;

pub(crate) struct ExecutionEventScope<'a> {
    pub(crate) agent_name: &'a str,
    pub(crate) root_session_id: Option<&'a str>,
    pub(crate) invocation_id: Option<&'a str>,
    pub(crate) turn_id: Option<i64>,
    pub(crate) async_run_id: Option<&'a str>,
    pub(crate) cron_job_name: Option<&'a str>,
    pub(crate) cron_run_id: Option<&'a str>,
}

pub(crate) fn persist_stream_line(
    conn: &rusqlite::Connection,
    scope: &ExecutionEventScope<'_>,
    seq: i64,
    line: &str,
) -> Result<Option<i64>, rusqlite::Error> {
    let events = crate::cc::stream::parse_persisted_stream_events(line);
    if events.is_empty() {
        return Ok(None);
    };
    let tx = conn.unchecked_transaction()?;
    let mut first_id = None;
    for (block_index, event) in events.into_iter().enumerate() {
        let id = insert_stream_event(&tx, scope, seq, block_index, event)?;
        first_id.get_or_insert(id);
    }
    tx.commit()?;
    Ok(first_id)
}

fn insert_stream_event(
    conn: &rusqlite::Connection,
    scope: &ExecutionEventScope<'_>,
    seq: i64,
    block_index: usize,
    event: crate::cc::stream::PersistedStreamEvent,
) -> Result<i64, rusqlite::Error> {
    let crate::cc::stream::PersistedStreamEvent {
        kind: raw_kind,
        tool_name,
        content_json,
        content_text: fallback_content_text,
    } = event;
    let event_kind = to_domain_kind(raw_kind);
    let trust_label = if matches!(
        raw_kind,
        crate::cc::stream::PersistedStreamEventKind::Thinking
    ) {
        right_agent::learning_episodes::TrustLabel::Secondary
    } else {
        right_agent::learning_episodes::TrustLabel::Primary
    };
    let redacted_content_json = redact_sensitive_json(content_json);
    let content_text = truncate_to_chars(
        &redact_sensitive_text(&content_text_from_redacted_event(
            raw_kind,
            tool_name.as_deref(),
            &fallback_content_text,
            &redacted_content_json,
        )),
        MAX_CONTENT_TEXT_CHARS,
    );
    let content_json = bound_content_json(redacted_content_json);
    let seq = seq
        .saturating_mul(1_000)
        .saturating_add(i64::try_from(block_index).unwrap_or(i64::MAX));

    right_agent::learning_episodes::insert_execution_event(
        conn,
        &right_agent::learning_episodes::NewExecutionEvent {
            agent_name: scope.agent_name.to_owned(),
            root_session_id: scope.root_session_id.map(str::to_owned),
            invocation_id: scope.invocation_id.map(str::to_owned),
            turn_id: scope.turn_id,
            async_run_id: scope.async_run_id.map(str::to_owned),
            cron_job_name: scope.cron_job_name.map(str::to_owned),
            cron_run_id: scope.cron_run_id.map(str::to_owned),
            seq,
            event_kind,
            tool_name,
            content_json,
            content_text,
            trust_label,
        },
    )
}

fn content_text_from_redacted_event(
    kind: crate::cc::stream::PersistedStreamEventKind,
    tool_name: Option<&str>,
    fallback_content_text: &str,
    content_json: &serde_json::Value,
) -> String {
    match kind {
        crate::cc::stream::PersistedStreamEventKind::AssistantText => content_json
            .get("text")
            .and_then(|text| text.as_str())
            .unwrap_or(fallback_content_text)
            .to_owned(),
        crate::cc::stream::PersistedStreamEventKind::Thinking => content_json
            .get("thinking")
            .and_then(|thinking| thinking.as_str())
            .unwrap_or(fallback_content_text)
            .to_owned(),
        crate::cc::stream::PersistedStreamEventKind::ToolCall => {
            let tool = tool_name.unwrap_or("?");
            let input = content_json
                .get("input")
                .unwrap_or(&serde_json::Value::Null);
            summarize_tool_input(tool, input)
        }
        crate::cc::stream::PersistedStreamEventKind::ToolResult
        | crate::cc::stream::PersistedStreamEventKind::ToolError => crate::cc::stream::value_text(
            content_json
                .get("content")
                .unwrap_or(&serde_json::Value::Null),
        ),
        crate::cc::stream::PersistedStreamEventKind::InvocationResult => content_json
            .get("result")
            .map(crate::cc::stream::value_text)
            .unwrap_or_else(|| fallback_content_text.to_owned()),
    }
}

pub(crate) fn redact_sensitive_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let value = if is_sensitive_key(&key) {
                        serde_json::Value::String("[redacted]".to_string())
                    } else {
                        redact_sensitive_json(value)
                    };
                    (key, value)
                })
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(redact_sensitive_json).collect())
        }
        serde_json::Value::String(value) => serde_json::Value::String(truncate_to_chars(
            &redact_sensitive_text(&value),
            MAX_JSON_STRING_CHARS,
        )),
        other => other,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = normalize_sensitive_key(key);
    normalized.contains("token")
        || normalized.contains("secret")
        || normalized.contains("password")
        || normalized.contains("apikey")
        || normalized.contains("privatekey")
        || normalized.contains("authorization")
        || normalized.contains("bearer")
        || normalized.contains("cookie")
        || normalized.contains("credential")
}

fn normalize_sensitive_key(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn redact_sensitive_text(value: &str) -> String {
    let mut redact_next = false;
    let mut redact_cookie_pairs = false;
    value
        .split_whitespace()
        .map(|part| {
            if redact_cookie_pairs {
                if part.contains('=') {
                    return "[redacted]".to_string();
                }
                redact_cookie_pairs = false;
            }

            if redact_next {
                redact_next = is_auth_scheme(part);
                return "[redacted]".to_string();
            }

            let separator = part.find(['=', ':']);
            if let Some(separator) = separator {
                let key = &part[..separator];
                if is_sensitive_key(key) {
                    let value = &part[separator + 1..];
                    if value.is_empty() {
                        if is_cookie_key(key) {
                            redact_cookie_pairs = true;
                        } else {
                            redact_next = true;
                        }
                    } else if is_auth_scheme(value) {
                        redact_next = true;
                    }
                    return format!("{}[redacted]", &part[..=separator]);
                }
            }

            if is_sensitive_key(part) {
                redact_next = true;
            }
            part.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_cookie_key(key: &str) -> bool {
    normalize_sensitive_key(key).contains("cookie")
}

fn is_auth_scheme(value: &str) -> bool {
    let normalized = normalize_sensitive_key(value);
    matches!(normalized.as_str(), "bearer" | "basic")
}

fn summarize_tool_input(tool: &str, input: &serde_json::Value) -> String {
    match tool {
        "Bash" => input
            .get("command")
            .and_then(|command| command.as_str())
            .unwrap_or("")
            .to_string(),
        "Read" => input
            .get("file_path")
            .and_then(|path| path.as_str())
            .unwrap_or("?")
            .to_string(),
        "Write" | "Edit" => input
            .get("file_path")
            .and_then(|path| path.as_str())
            .unwrap_or("?")
            .to_string(),
        "Grep" | "Glob" => input
            .get("pattern")
            .and_then(|pattern| pattern.as_str())
            .unwrap_or("")
            .to_string(),
        "Skill" => input
            .get("skill")
            .and_then(|skill| skill.as_str())
            .map(|skill| format!("/{skill}"))
            .unwrap_or_default(),
        "Agent" => input
            .get("description")
            .and_then(|description| description.as_str())
            .unwrap_or("...")
            .to_string(),
        _ => input.to_string(),
    }
}

fn to_domain_kind(
    kind: crate::cc::stream::PersistedStreamEventKind,
) -> right_agent::learning_episodes::ExecutionEventKind {
    match kind {
        crate::cc::stream::PersistedStreamEventKind::AssistantText => {
            right_agent::learning_episodes::ExecutionEventKind::AssistantText
        }
        crate::cc::stream::PersistedStreamEventKind::Thinking => {
            right_agent::learning_episodes::ExecutionEventKind::Thinking
        }
        crate::cc::stream::PersistedStreamEventKind::ToolCall => {
            right_agent::learning_episodes::ExecutionEventKind::ToolCall
        }
        crate::cc::stream::PersistedStreamEventKind::ToolResult => {
            right_agent::learning_episodes::ExecutionEventKind::ToolResult
        }
        crate::cc::stream::PersistedStreamEventKind::ToolError => {
            right_agent::learning_episodes::ExecutionEventKind::ToolError
        }
        crate::cc::stream::PersistedStreamEventKind::InvocationResult => {
            right_agent::learning_episodes::ExecutionEventKind::InvocationResult
        }
    }
}

fn bound_content_json(value: serde_json::Value) -> serde_json::Value {
    let serialized = match serde_json::to_string(&value) {
        Ok(serialized) => serialized,
        Err(_) => return serde_json::json!({"serialization_error": true}),
    };
    if serialized.chars().count() <= MAX_CONTENT_JSON_CHARS {
        return value;
    }
    let mut preview_chars = MAX_CONTENT_JSON_CHARS;
    loop {
        let bounded = serde_json::json!({
            "truncated": true,
            "preview": truncate_to_chars(&serialized, preview_chars),
        });
        let Ok(bounded_serialized) = serde_json::to_string(&bounded) else {
            return serde_json::json!({"serialization_error": true});
        };
        let bounded_chars = bounded_serialized.chars().count();
        if bounded_chars <= MAX_CONTENT_JSON_CHARS {
            return bounded;
        }
        if preview_chars == 0 {
            return serde_json::json!({"truncated": true});
        }
        preview_chars =
            preview_chars.saturating_sub((bounded_chars - MAX_CONTENT_JSON_CHARS).max(1));
    }
}

fn truncate_to_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_json_keys_are_redacted() {
        let input = serde_json::json!({
            "api_key": "abc",
            "nested": {
                "refresh_token": "secret",
                "token": "bare",
                "secret_key": "key",
                "github_token": "ghp_secret",
                "anthropic_api_key": "sk-ant-secret",
                "cookie": "session=abc",
                "set_cookie": "session=def",
                "credential": "cred",
                "credentials": "creds"
            },
            "safe": "visible"
        });
        let redacted = redact_sensitive_json(input);
        assert_eq!(redacted["api_key"], "[redacted]");
        assert_eq!(redacted["nested"]["refresh_token"], "[redacted]");
        assert_eq!(redacted["nested"]["token"], "[redacted]");
        assert_eq!(redacted["nested"]["secret_key"], "[redacted]");
        assert_eq!(redacted["nested"]["github_token"], "[redacted]");
        assert_eq!(redacted["nested"]["anthropic_api_key"], "[redacted]");
        assert_eq!(redacted["nested"]["cookie"], "[redacted]");
        assert_eq!(redacted["nested"]["set_cookie"], "[redacted]");
        assert_eq!(redacted["nested"]["credential"], "[redacted]");
        assert_eq!(redacted["nested"]["credentials"], "[redacted]");
        assert_eq!(redacted["safe"], "visible");
    }

    fn conn() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        right_db::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    fn scope<'a>() -> ExecutionEventScope<'a> {
        ExecutionEventScope {
            agent_name: "right",
            root_session_id: Some("session-1"),
            invocation_id: Some("inv-1"),
            turn_id: Some(42),
            async_run_id: Some("async-1"),
            cron_job_name: Some("daily"),
            cron_run_id: Some("cron-1"),
        }
    }

    #[test]
    fn persist_stream_line_stores_scope_and_thinking_secondary() {
        let conn = conn();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Need check first"}]}}"#;

        let id = persist_stream_line(&conn, &scope(), 7, line)
            .unwrap()
            .unwrap();

        let row: (
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT agent_name, root_session_id, invocation_id, turn_id, async_run_id, \
                        cron_job_name, cron_run_id, seq, event_kind, trust_label, content_text \
                 FROM execution_events WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0, "right");
        assert_eq!(row.1.as_deref(), Some("session-1"));
        assert_eq!(row.2.as_deref(), Some("inv-1"));
        assert_eq!(row.3, Some(42));
        assert_eq!(row.4.as_deref(), Some("async-1"));
        assert_eq!(row.5.as_deref(), Some("daily"));
        assert_eq!(row.6.as_deref(), Some("cron-1"));
        assert_eq!(row.7, 7_000);
        assert_eq!(row.8, "thinking");
        assert_eq!(row.9, "secondary");
        assert_eq!(row.10, "Need check first");
    }

    #[test]
    fn persist_stream_line_redacts_content_json_and_text() {
        let conn = conn();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ANTHROPIC_API_KEY=sk-text-secret curl https://example.com","api_key":"sk-json-secret"}}]}}"#;

        let id = persist_stream_line(&conn, &scope(), 3, line)
            .unwrap()
            .unwrap();

        let (content_json, content_text): (String, String) = conn
            .query_row(
                "SELECT content_json, content_text FROM execution_events WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(!content_json.contains("sk-json-secret"), "{content_json}");
        assert!(!content_json.contains("sk-text-secret"), "{content_json}");
        assert!(!content_text.contains("sk-json-secret"), "{content_text}");
        assert!(!content_text.contains("sk-text-secret"), "{content_text}");
        assert!(content_json.contains("[redacted]"), "{content_json}");
        assert!(content_text.contains("[redacted]"), "{content_text}");
    }

    #[test]
    fn persist_stream_line_redacts_header_style_secrets() {
        let conn = conn();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"curl -H Cookie: session=abc -H X-Api-Key: sk-live -H Authorization:Bearer sk-auth https://example.com"}}]}}"#;

        let id = persist_stream_line(&conn, &scope(), 4, line)
            .unwrap()
            .unwrap();

        let (content_json, content_text): (String, String) = conn
            .query_row(
                "SELECT content_json, content_text FROM execution_events WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        for secret in ["session=abc", "sk-live", "sk-auth"] {
            assert!(!content_json.contains(secret), "{content_json}");
            assert!(!content_text.contains(secret), "{content_text}");
        }
        assert!(content_json.contains("[redacted]"), "{content_json}");
        assert!(content_text.contains("[redacted]"), "{content_text}");
    }

    #[test]
    fn persist_stream_line_caps_content_text_and_json() {
        let conn = conn();
        let mut input = serde_json::Map::new();
        for i in 0..6 {
            input.insert(
                format!("safe_{i}"),
                serde_json::Value::String("x".repeat(2_100)),
            );
        }
        let line = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "name": "Unknown",
                    "input": input,
                }]
            }
        })
        .to_string();

        let id = persist_stream_line(&conn, &scope(), 5, &line)
            .unwrap()
            .unwrap();

        let (content_json, content_text): (String, String) = conn
            .query_row(
                "SELECT content_json, content_text FROM execution_events WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(content_json.chars().count() <= 8_000, "{content_json}");
        assert_eq!(content_text.chars().count(), 2_000);
    }

    #[test]
    fn persist_stream_line_rolls_back_on_partial_failure() {
        let conn = conn();
        // Force the second block (block_index=1, seq=9_001) to fail by creating
        // a UNIQUE index on seq and pre-inserting a row at that seq. The first
        // block (seq=9_000) would succeed if not transactional; with the fix it
        // must roll back so no rows from this line remain.
        conn.execute(
            "CREATE UNIQUE INDEX test_unique_seq ON execution_events(seq)",
            [],
        )
        .unwrap();
        right_agent::learning_episodes::insert_execution_event(
            &conn,
            &right_agent::learning_episodes::NewExecutionEvent {
                agent_name: "other".to_owned(),
                root_session_id: None,
                invocation_id: None,
                turn_id: None,
                async_run_id: None,
                cron_job_name: None,
                cron_run_id: None,
                seq: 9_001,
                event_kind: right_agent::learning_episodes::ExecutionEventKind::AssistantText,
                tool_name: None,
                content_json: serde_json::json!({}),
                content_text: "sentinel".to_owned(),
                trust_label: right_agent::learning_episodes::TrustLabel::Primary,
            },
        )
        .unwrap();

        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"First"},{"type":"thinking","thinking":"Second"},{"type":"tool_use","name":"Read","input":{"file_path":"/tmp/file"}}]}}"#;

        let err = persist_stream_line(&conn, &scope(), 9, line).unwrap_err();
        assert!(
            matches!(err, rusqlite::Error::SqliteFailure(_, _)),
            "expected sqlite constraint failure, got {err:?}"
        );

        // Only the pre-existing sentinel remains; first-block insert (seq=9_000)
        // must have been rolled back.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM execution_events WHERE agent_name='right'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "first block must roll back on partial failure");
        let sentinel_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM execution_events WHERE agent_name='other'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sentinel_count, 1, "sentinel row must remain");
    }

    #[test]
    fn persist_stream_line_inserts_all_blocks_in_order() {
        let conn = conn();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"First"},{"type":"thinking","thinking":"Second"},{"type":"tool_use","name":"Read","input":{"file_path":"/tmp/file"}}]}}"#;

        let first_id = persist_stream_line(&conn, &scope(), 9, line)
            .unwrap()
            .unwrap();

        let rows: Vec<(i64, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT seq, event_kind, content_text \
                     FROM execution_events ORDER BY seq ASC",
                )
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            rows,
            vec![
                (9_000, "assistant_text".to_string(), "First".to_string()),
                (9_001, "thinking".to_string(), "Second".to_string()),
                (9_002, "tool_call".to_string(), "/tmp/file".to_string()),
            ]
        );
        let first_seq: i64 = conn
            .query_row(
                "SELECT seq FROM execution_events WHERE id=?1",
                [first_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(first_seq, 9_000);
    }
}
