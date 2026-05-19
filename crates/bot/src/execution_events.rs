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
    let mut first_id = None;
    for (block_index, event) in events.into_iter().enumerate() {
        let id = insert_stream_event(conn, scope, seq, block_index, event)?;
        first_id.get_or_insert(id);
    }
    Ok(first_id)
}

fn insert_stream_event(
    conn: &rusqlite::Connection,
    scope: &ExecutionEventScope<'_>,
    seq: i64,
    block_index: usize,
    event: crate::cc::stream::PersistedStreamEvent,
) -> Result<i64, rusqlite::Error> {
    let event_kind = to_domain_kind(event.kind);
    let trust_label = if matches!(
        event.kind,
        crate::cc::stream::PersistedStreamEventKind::Thinking
    ) {
        right_agent::learning_episodes::TrustLabel::Secondary
    } else {
        right_agent::learning_episodes::TrustLabel::Primary
    };
    let content_json = bound_content_json(redact_sensitive_json(event.content_json.clone()));
    let content_text = truncate_to_chars(
        &redact_sensitive_text(&content_text_from_redacted_event(&event, &content_json)),
        MAX_CONTENT_TEXT_CHARS,
    );
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
            tool_name: event.tool_name,
            content_json,
            content_text,
            trust_label,
        },
    )
}

fn content_text_from_redacted_event(
    event: &crate::cc::stream::PersistedStreamEvent,
    content_json: &serde_json::Value,
) -> String {
    match event.kind {
        crate::cc::stream::PersistedStreamEventKind::AssistantText => content_json
            .get("text")
            .and_then(|text| text.as_str())
            .unwrap_or(&event.content_text)
            .to_owned(),
        crate::cc::stream::PersistedStreamEventKind::Thinking => content_json
            .get("thinking")
            .and_then(|thinking| thinking.as_str())
            .unwrap_or(&event.content_text)
            .to_owned(),
        crate::cc::stream::PersistedStreamEventKind::ToolCall => {
            let tool = event.tool_name.as_deref().unwrap_or("?");
            let input = content_json
                .get("input")
                .unwrap_or(&serde_json::Value::Null);
            summarize_tool_input(tool, input)
        }
        crate::cc::stream::PersistedStreamEventKind::ToolResult
        | crate::cc::stream::PersistedStreamEventKind::ToolError => value_text(
            content_json
                .get("content")
                .unwrap_or(&serde_json::Value::Null),
        ),
        crate::cc::stream::PersistedStreamEventKind::InvocationResult => content_json
            .get("result")
            .map(value_text)
            .unwrap_or_else(|| event.content_text.clone()),
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
    let normalized = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
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

fn redact_sensitive_text(value: &str) -> String {
    let mut redact_next = false;
    value
        .split_whitespace()
        .map(|part| {
            if redact_next {
                redact_next = false;
                return "[redacted]".to_string();
            }

            let separator = part.find(['=', ':']);
            if let Some(separator) = separator {
                let key = &part[..separator];
                if is_sensitive_key(key) {
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

fn value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(|item| {
                item.as_str().map(str::to_owned).or_else(|| {
                    item.get("text")
                        .and_then(|text| text.as_str())
                        .map(str::to_owned)
                })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
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
    serde_json::json!({
        "truncated": true,
        "preview": truncate_to_chars(&serialized, MAX_CONTENT_JSON_CHARS),
    })
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
