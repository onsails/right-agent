-- v12: Add delivery_status and no_notify_reason to cron_runs for self-diagnostics.
-- delivery_status tracks lifecycle: silent, pending, delivered, superseded, failed.
-- no_notify_reason stores CC's explanation when notify is null.
ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT;
ALTER TABLE cron_runs ADD COLUMN no_notify_reason TEXT;

-- Backfill existing rows based on current state.
UPDATE cron_runs SET delivery_status = 'delivered'
  WHERE notify_json IS NOT NULL AND delivered_at IS NOT NULL;
UPDATE cron_runs SET delivery_status = 'pending'
  WHERE notify_json IS NOT NULL AND delivered_at IS NULL;
UPDATE cron_runs SET delivery_status = 'silent'
  WHERE notify_json IS NULL;

-- Trigger: auto-set delivery_status on INSERT when not explicitly provided.
CREATE TRIGGER IF NOT EXISTS cron_runs_delivery_status_insert
AFTER INSERT ON cron_runs
WHEN NEW.delivery_status IS NULL
BEGIN
  UPDATE cron_runs SET delivery_status =
    CASE
      WHEN NEW.notify_json IS NOT NULL AND NEW.delivered_at IS NOT NULL THEN 'delivered'
      WHEN NEW.notify_json IS NOT NULL AND NEW.delivered_at IS NULL     THEN 'pending'
      ELSE 'silent'
    END
  WHERE id = NEW.id;
END;
