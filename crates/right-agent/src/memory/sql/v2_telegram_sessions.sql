-- V2 schema: telegram_sessions
-- Source: Phase 22 decisions D-01..D-06

CREATE TABLE IF NOT EXISTS telegram_sessions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id         INT     NOT NULL,
    thread_id       INT     NOT NULL DEFAULT 0,
    root_session_id TEXT    NOT NULL,
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    last_used_at    TEXT,
    UNIQUE(chat_id, thread_id)
);
