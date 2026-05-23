-- v31: Index supporting the dashboard's per-agent/phase/created_at filter
-- on skill_learning_events. Covers learning_overview hot-path queries.
CREATE INDEX IF NOT EXISTS idx_skill_learning_events_agent_phase_created
  ON skill_learning_events(agent_name, phase, created_at);
