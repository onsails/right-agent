-- v16: Add api_key_source to usage_events.
-- 'none' = OAuth/setup-token (subscription), other values = API key.
-- Column is added via a Rust hook with a pragma_table_info guard for
-- idempotency (SQLite lacks ADD COLUMN IF NOT EXISTS).
-- The DEFAULT also backfills existing rows to 'none' — a safe assumption
-- since current RightClaw deployments all use setup-token auth.
ALTER TABLE usage_events ADD COLUMN api_key_source TEXT NOT NULL DEFAULT 'none';
