CREATE TABLE IF NOT EXISTS conversation_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    platform TEXT NOT NULL DEFAULT 'telegram',
    chat_id INTEGER NOT NULL,
    thread_id INTEGER NOT NULL DEFAULT 0,
    message_id INTEGER,
    sender_user_id INTEGER,
    sender_name TEXT,
    addressed_to_bot INTEGER NOT NULL DEFAULT 0 CHECK (addressed_to_bot IN (0, 1)),
    routed_to_agent INTEGER NOT NULL DEFAULT 0 CHECK (routed_to_agent IN (0, 1)),
    root_session_id TEXT,
    turn_id INTEGER,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_conversation_messages_inbound_unique
ON conversation_messages (platform, chat_id, message_id, role);

CREATE INDEX IF NOT EXISTS idx_conversation_messages_thread_created
ON conversation_messages (platform, chat_id, thread_id, created_at);

CREATE INDEX IF NOT EXISTS idx_conversation_messages_chat_created
ON conversation_messages (platform, chat_id, created_at);

CREATE INDEX IF NOT EXISTS idx_conversation_messages_session_turn
ON conversation_messages (root_session_id, turn_id)
WHERE root_session_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_conversation_messages_turso_fts
ON conversation_messages USING fts(content);
