-- V17: Per-cron delivery target. Columns are nullable for back-compat with
-- rows that existed before this migration; the MCP layer enforces presence
-- on new inserts. NULL rows are surfaced by `doctor::check_cron_targets`.
ALTER TABLE cron_specs ADD COLUMN target_chat_id   INTEGER;
ALTER TABLE cron_specs ADD COLUMN target_thread_id INTEGER;
