use right_mcp::internal_db as ipc;

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: i64,
    pub chat_id: i64,
    pub thread_id: i64,
    pub root_session_id: String,
    pub label: Option<String>,
    pub is_active: bool,
    pub created_at: String,
    pub last_used_at: String,
}

impl From<ipc::SessionRowDto> for SessionRow {
    fn from(row: ipc::SessionRowDto) -> Self {
        Self {
            id: row.id,
            chat_id: row.chat_id,
            thread_id: row.thread_id,
            root_session_id: row.root_session_id,
            label: row.label,
            is_active: row.is_active,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
        }
    }
}

pub fn effective_thread_id(msg: &frankenstein::types::Message) -> i64 {
    msg.message_thread_id.unwrap_or(0) as i64
}

pub fn truncate_label(s: &str) -> &str {
    const MAX: usize = 50;
    if s.len() <= MAX {
        return s;
    }
    let mut end = MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

pub async fn get_active_session(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<SessionRow>, ipc::InternalDbError> {
    client
        .get_active_session(&ipc::GetActiveSessionRequest {
            agent: agent.to_owned(),
            chat_id,
            thread_id,
        })
        .await
        .map(|response| response.session.map(Into::into))
}

pub async fn create_session(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    chat_id: i64,
    thread_id: i64,
    session_uuid: &str,
    label: Option<&str>,
) -> Result<i64, ipc::InternalDbError> {
    client
        .create_session(&ipc::CreateSessionRequest {
            agent: agent.to_owned(),
            chat_id,
            thread_id,
            session_uuid: session_uuid.to_owned(),
            label: label.map(str::to_owned),
        })
        .await
        .map(|response| response.session_id)
}

pub async fn deactivate_current(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    chat_id: i64,
    thread_id: i64,
) -> Result<Option<String>, ipc::InternalDbError> {
    client
        .deactivate_current_session(&ipc::DeactivateCurrentSessionRequest {
            agent: agent.to_owned(),
            chat_id,
            thread_id,
        })
        .await
        .map(|response| response.previous_root_session_id)
}

pub async fn activate_session(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    session_id: i64,
) -> Result<(), ipc::InternalDbError> {
    client
        .activate_session(&ipc::ActivateSessionRequest {
            agent: agent.to_owned(),
            session_id,
        })
        .await
        .map(drop)
}

pub async fn touch_session(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    session_id: i64,
) -> Result<(), ipc::InternalDbError> {
    client
        .touch_session(&ipc::TouchSessionRequest {
            agent: agent.to_owned(),
            session_id,
        })
        .await
        .map(drop)
}

pub async fn list_sessions(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    chat_id: i64,
    thread_id: i64,
) -> Result<Vec<SessionRow>, ipc::InternalDbError> {
    client
        .list_sessions(&ipc::ListSessionsRequest {
            agent: agent.to_owned(),
            chat_id,
            thread_id,
        })
        .await
        .map(|response| response.sessions.into_iter().map(Into::into).collect())
}

pub async fn find_sessions_by_uuid(
    client: &right_mcp::internal_client::InternalClient,
    agent: &str,
    chat_id: i64,
    thread_id: i64,
    partial: &str,
) -> Result<Vec<SessionRow>, ipc::InternalDbError> {
    client
        .find_sessions_by_uuid(&ipc::FindSessionsByUuidRequest {
            agent: agent.to_owned(),
            chat_id,
            thread_id,
            uuid_prefix: partial.to_owned(),
        })
        .await
        .map(|response| response.sessions.into_iter().map(Into::into).collect())
}
