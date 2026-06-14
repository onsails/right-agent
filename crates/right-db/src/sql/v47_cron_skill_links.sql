-- Explicit many-to-many link between a cron job and the rightx-* skills it
-- should deterministically pull. origin distinguishes platform auto-links from
-- agent-authored links. Per-agent data.db; job_name is unique within an agent.
CREATE TABLE IF NOT EXISTS cron_skill_links (
  job_name   TEXT NOT NULL,
  skill_name TEXT NOT NULL,
  origin     TEXT NOT NULL CHECK (origin IN ('auto', 'agent')),
  created_at TEXT NOT NULL,
  PRIMARY KEY (job_name, skill_name)
);

CREATE INDEX IF NOT EXISTS idx_cron_skill_links_skill
  ON cron_skill_links(skill_name);
