-- V1 schema for per-agent memory database
-- Source: Phase 16 decision D-02

-- Main memories table
CREATE TABLE IF NOT EXISTS memories (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    content     TEXT    NOT NULL,
    tags        TEXT,
    stored_by   TEXT,
    source_tool TEXT,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    deleted_at  TEXT,
    expires_at  TEXT,
    importance  REAL    NOT NULL DEFAULT 0.5
);

-- Append-only audit log
CREATE TABLE IF NOT EXISTS memory_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id   INTEGER,
    event_type  TEXT    NOT NULL,
    actor       TEXT,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- Block UPDATE and DELETE on memory_events (append-only invariant)
CREATE TRIGGER IF NOT EXISTS memory_events_no_update
BEFORE UPDATE ON memory_events
BEGIN
    SELECT RAISE(ABORT, 'memory_events is append-only: UPDATE not permitted');
END;

CREATE TRIGGER IF NOT EXISTS memory_events_no_delete
BEFORE DELETE ON memory_events
BEGIN
    SELECT RAISE(ABORT, 'memory_events is append-only: DELETE not permitted');
END;

CREATE INDEX IF NOT EXISTS idx_memories_turso_fts
ON memories USING fts(content);
