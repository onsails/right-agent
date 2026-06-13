-- V44: Widen skill_lifecycle.created_by CHECK to include 'cron'.
-- SQLite does not support ALTER COLUMN; recreate the table preserving all data.
PRAGMA foreign_keys = OFF;

CREATE TABLE skill_lifecycle_new (
  skill_name       TEXT PRIMARY KEY,
  state            TEXT NOT NULL DEFAULT 'active'
                   CHECK (state IN ('active', 'stale', 'archived')),
  pinned           INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
  created_by       TEXT NOT NULL DEFAULT 'foreground'
                   CHECK (created_by IN ('foreground', 'probe_writer', 'curator', 'bundled', 'cron')),
  use_count        INTEGER NOT NULL DEFAULT 0 CHECK (use_count >= 0),
  patch_count      INTEGER NOT NULL DEFAULT 0 CHECK (patch_count >= 0),
  created_at       TEXT,
  last_used_at     TEXT,
  last_patched_at  TEXT,
  archived_at      TEXT,
  absorbed_into    TEXT
);

INSERT INTO skill_lifecycle_new SELECT * FROM skill_lifecycle;

DROP TABLE skill_lifecycle;

ALTER TABLE skill_lifecycle_new RENAME TO skill_lifecycle;

CREATE INDEX IF NOT EXISTS idx_skill_lifecycle_state
  ON skill_lifecycle(state);

CREATE INDEX IF NOT EXISTS idx_skill_lifecycle_created_by_state
  ON skill_lifecycle(created_by, state);

CREATE INDEX IF NOT EXISTS idx_skill_lifecycle_pinned
  ON skill_lifecycle(pinned);

PRAGMA foreign_keys = ON;
