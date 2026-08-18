CREATE TABLE IF NOT EXISTS bootstrap_answers (
    chat_id INTEGER NOT NULL,
    thread_id INTEGER NOT NULL,
    stage TEXT NOT NULL,
    answer TEXT NOT NULL,
    source_message_id INTEGER NOT NULL,
    recorded_at TEXT NOT NULL,
    PRIMARY KEY (chat_id, thread_id, stage),
    UNIQUE (chat_id, thread_id, source_message_id)
);

CREATE TABLE IF NOT EXISTS bootstrap_questions (
    chat_id INTEGER NOT NULL,
    thread_id INTEGER NOT NULL,
    stage TEXT NOT NULL,
    assistant_message_id INTEGER NOT NULL,
    issued_at TEXT NOT NULL,
    PRIMARY KEY (chat_id, thread_id, stage)
);

CREATE TABLE IF NOT EXISTS bootstrap_interview (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    chat_id INTEGER NOT NULL,
    thread_id INTEGER NOT NULL
);
