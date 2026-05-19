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
    let Some(event) = crate::cc::stream::parse_persisted_stream_event(line) else {
        return Ok(None);
    };
    let event_kind = to_domain_kind(event.kind);
    let trust_label = if matches!(
        event.kind,
        crate::cc::stream::PersistedStreamEventKind::Thinking
    ) {
        right_agent::learning_episodes::TrustLabel::Secondary
    } else {
        right_agent::learning_episodes::TrustLabel::Primary
    };
    let content_json = bound_content_json(redact_sensitive_json(event.content_json));
    let content_text = truncate_to_chars(&event.content_text, MAX_CONTENT_TEXT_CHARS);

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
    .map(Some)
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
        serde_json::Value::String(value) => {
            serde_json::Value::String(truncate_to_chars(&value, MAX_JSON_STRING_CHARS))
        }
        other => other,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "apikey"
            | "accesstoken"
            | "refreshtoken"
            | "authtoken"
            | "authorization"
            | "password"
            | "secret"
            | "clientsecret"
            | "privatekey"
            | "bearertoken"
    )
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
        let input = serde_json::json!({"api_key":"abc","nested":{"refresh_token":"secret"},"safe":"visible"});
        let redacted = redact_sensitive_json(input);
        assert_eq!(redacted["api_key"], "[redacted]");
        assert_eq!(redacted["nested"]["refresh_token"], "[redacted]");
        assert_eq!(redacted["safe"], "visible");
    }
}
