CREATE TABLE IF NOT EXISTS skill_lifecycle (
  skill_name       TEXT PRIMARY KEY,
  state            TEXT NOT NULL DEFAULT 'active'
                   CHECK (state IN ('active', 'stale', 'archived')),
  pinned           INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
  created_by       TEXT NOT NULL DEFAULT 'foreground'
                   CHECK (created_by IN ('foreground', 'probe_writer', 'curator', 'bundled')),
  use_count        INTEGER NOT NULL DEFAULT 0 CHECK (use_count >= 0),
  patch_count      INTEGER NOT NULL DEFAULT 0 CHECK (patch_count >= 0),
  created_at       TEXT,
  last_used_at     TEXT,
  last_patched_at  TEXT,
  archived_at      TEXT,
  absorbed_into    TEXT
);

CREATE INDEX IF NOT EXISTS idx_skill_lifecycle_state
  ON skill_lifecycle(state);

CREATE INDEX IF NOT EXISTS idx_skill_lifecycle_created_by_state
  ON skill_lifecycle(created_by, state);

CREATE INDEX IF NOT EXISTS idx_skill_lifecycle_pinned
  ON skill_lifecycle(pinned);
