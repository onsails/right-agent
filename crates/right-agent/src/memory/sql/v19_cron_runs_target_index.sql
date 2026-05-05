-- v19: Index supporting `build_bg_marker_for_chat` and the delivery loop.
--
-- Both query `cron_runs` by `target_chat_id` filtered on `status` and
-- `delivered_at`. `cron_runs` is monotonically appended by the cron engine and
-- never pruned, so the table grows with deployment age — without an index the
-- per-message marker lookup degrades to a full scan.
CREATE INDEX IF NOT EXISTS idx_cron_runs_target_status
    ON cron_runs(target_chat_id, status, delivered_at);
