CREATE TABLE IF NOT EXISTS forum_topics (
  chat_id              INTEGER NOT NULL,
  message_thread_id    INTEGER NOT NULL,
  name                 TEXT,
  icon_color           INTEGER,
  icon_custom_emoji_id TEXT,
  state                TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'closed')),
  updated_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  PRIMARY KEY (chat_id, message_thread_id)
);

CREATE INDEX IF NOT EXISTS idx_forum_topics_chat
  ON forum_topics (chat_id, updated_at);
