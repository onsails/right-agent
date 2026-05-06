-- V5: Extend cron_runs with feedback columns for delivery-through-main-session.
-- summary: always written on successful cron completion.
-- notify_json: serialized notify object (content + attachments) or NULL if silent.
-- delivered_at: set when result is delivered through main CC session.
ALTER TABLE cron_runs ADD COLUMN summary TEXT;
ALTER TABLE cron_runs ADD COLUMN notify_json TEXT;
ALTER TABLE cron_runs ADD COLUMN delivered_at TEXT;
