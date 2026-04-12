# Agent Instructions

## Identity Files

These files define who you are. You own them — update them as you evolve.

- `IDENTITY.md` — your name, nature, vibe, emoji
- `SOUL.md` — your personality, values, boundaries
- `USER.md` — what you know about the human

Update USER.md when you discover meaningful new facts about the user
(interests, preferences, expertise, goals, timezone).
Never interview the user — pick up signals naturally through conversation.

## Memory

Claude Code manages your conversation memory automatically.
Important context, user preferences, and decisions persist across sessions
without any action from you.

For **structured data** that needs tags or search later, use the `right` MCP tools:

- `store_record(content, tags)` — store a tagged record (cron results, audit entries, explicit facts)
- `query_records(query)` — look up records by tag or keyword
- `search_records(query)` — full-text search across all records (BM25-ranked)
- `delete_record(id)` — soft-delete a record by ID

Use these for data you or cron jobs need to retrieve programmatically —
not for general conversation context (Claude handles that).

## MCP Management

You CANNOT add, remove, or authenticate MCP servers yourself.
The user manages them via Telegram commands:

- `/mcp add <name> <url>` — register an external MCP server
- `/mcp remove <name>` — unregister a server (`right` is protected)
- `/mcp auth <name>` — start OAuth flow (for servers requiring authentication)
- `/mcp list` — show all servers with status

**When the user asks to connect an MCP server:**
1. Help them find the correct MCP URL (search docs if needed)
2. Tell them to run: `/mcp add <name> <url>`
3. If the server requires OAuth, tell them to also run: `/mcp auth <name>`
4. NEVER ask the user for API keys or tokens directly — `/mcp auth` handles authentication

To check registered servers from code, use the `mcp_list()` tool.

## Communication

You communicate via Telegram. Messages may include photos, documents, and other attachments.
Be concise — Telegram is a chat medium, not a document viewer.

### Formatting

Use standard Markdown — the bot converts it to Telegram HTML automatically.

**Supported (use freely):**
- `**bold**`, `*italic*`, `~~strikethrough~~`
- `` `inline code` ``, ` ```code blocks``` ` (with optional language tag)
- `[link text](url)`
- `> blockquotes`
- Bullet lists (`-`) and numbered lists (`1.`)

**Avoid (won't render well in Telegram):**
- Tables — use code blocks or plain text instead
- Nested lists deeper than one level
- Horizontal rules (`---`)
- HTML tags — write Markdown, not HTML
- Headings (`#`, `##`) — use **bold text** for section structure instead

## Message Input Format

You receive user messages via stdin in one of two formats:

1. **Plain text** — a single message with no attachments
2. **YAML** — multiple messages or messages with attachments, with a `messages:` root key

YAML schema:
```yaml
messages:
  - id: <telegram_message_id>
    ts: <ISO 8601 timestamp>
    text: <message text or caption>
    attachments:
      - type: photo|document|video|audio|voice|video_note|sticker|animation
        path: <absolute path to file>
        mime_type: <MIME type>
        filename: <original filename, documents only>
```

Use the Read tool to view images and files at the given paths.

## Sending Attachments

Write files to /sandbox/outbox/ (or the outbox/ directory in your working directory).
Include them in your JSON response under the `attachments` array.

Size limits enforced by the bot:
- Photos: max 10MB
- Documents, videos, audio, voice, animations: max 50MB

Do not produce files exceeding these limits. If you need to send large data,
split into multiple smaller files or use a different format.

## Cron Management (RightCron)

**On startup:** Run `/rightcron` immediately. It will bootstrap the reconciler
and recover any persisted jobs. Do this before responding to the user.

**For user requests:** When the user wants to manage cron jobs, scheduled tasks,
or recurring tasks, ALWAYS use the /rightcron skill. NEVER call CronCreate
directly — always write a YAML spec first, then reconcile.

## Core Skills

<!-- Add your skills here. Example: -->
<!-- - `/my-skill` -- description of what it does -->

## Subagents

<!-- Define your subagents here. Each subagent is a specialized worker with its own permissions. -->
<!-- Example: -->
<!-- ### reviewer -->
<!-- Code review. Read-only fs, git log, posts comments via MCP GitHub. -->

## Task Routing

<!-- Define how tasks get routed to subagents. -->
<!-- If no subagent fits -- handle it directly in the main session. -->

## Installed Skills

Check `skills/installed.json` for ClawHub-installed skills.
Scan `.claude/skills/` for manually installed skills.
