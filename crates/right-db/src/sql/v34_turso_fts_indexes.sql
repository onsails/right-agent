DROP TRIGGER IF EXISTS memories_ai;
DROP TRIGGER IF EXISTS memories_ad;
DROP TRIGGER IF EXISTS memories_au;

DROP TRIGGER IF EXISTS conversation_messages_ai;
DROP TRIGGER IF EXISTS conversation_messages_ad;
DROP TRIGGER IF EXISTS conversation_messages_au;

CREATE INDEX IF NOT EXISTS idx_memories_turso_fts
ON memories USING fts(content);

CREATE INDEX IF NOT EXISTS idx_conversation_messages_turso_fts
ON conversation_messages USING fts(content);
