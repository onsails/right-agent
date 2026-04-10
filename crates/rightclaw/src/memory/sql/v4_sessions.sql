-- V4 schema: multi-session support
-- Replaces telegram_sessions (single session per chat+thread)
-- with sessions (multiple sessions per chat+thread, one active at a time).

DROP TABLE IF EXISTS telegram_sessions;

CREATE TABLE sessions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id         INTEGER NOT NULL,
    thread_id       INTEGER NOT NULL DEFAULT 0,
    root_session_id TEXT    NOT NULL,
    label           TEXT,
    is_active       INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
    last_used_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);

CREATE UNIQUE INDEX idx_sessions_active
    ON sessions(chat_id, thread_id) WHERE is_active = 1;
