-- v27: Track the origin of each accepted learning signal.
--
-- Source values:
--   'reply_field' — agent emitted the signal in its structured reply.
--   'fork_probe'  — post-turn fork-classifier identified the signal.
--
-- Implementation lives in Rust hook `v27_skill_nudge_signals_source`
-- because SQLite lacks `ADD COLUMN IF NOT EXISTS`. This file is doc-only.

ALTER TABLE skill_nudge_signals
  ADD COLUMN source TEXT NOT NULL DEFAULT 'reply_field';

CREATE INDEX IF NOT EXISTS idx_skill_nudge_signals_source
  ON skill_nudge_signals(source);
