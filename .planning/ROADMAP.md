# Roadmap: RightClaw

## Milestones

- ✅ **v1.0 Core Runtime** - Phases 1-4 (shipped 2026-03-23)
- ✅ **v2.0 Native Sandbox** - Phases 5-7 (shipped 2026-03-24)
- ✅ **v2.1 Headless Agent Isolation** - Phases 8-10 (shipped 2026-03-25)
- ✅ **v2.2 Skills Registry** - Phases 11-15 (shipped 2026-03-26)
- ✅ **v2.3 Memory System** - Phases 16-19 (shipped 2026-03-27)
- ✅ **v2.4 Sandbox Telegram Fix** - Phase 20 (shipped 2026-03-28)
- ✅ **v2.5 RightCron Reliability** - Phase 21 (shipped 2026-03-31)
- 🚧 **v3.0 Teloxide Bot Runtime** - Phases 22-28 (in progress)

## Phases

<details>
<summary>✅ v1.0 Core Runtime (Phases 1-4) - SHIPPED 2026-03-23</summary>

See [milestones/v1.0-ROADMAP.md](milestones/v1.0-ROADMAP.md)

</details>

<details>
<summary>✅ v2.0 Native Sandbox (Phases 5-7) - SHIPPED 2026-03-24</summary>

See [milestones/v2.0-ROADMAP.md](milestones/v2.0-ROADMAP.md)

</details>

<details>
<summary>✅ v2.1 Headless Agent Isolation (Phases 8-10) - SHIPPED 2026-03-25</summary>

See [milestones/v2.1-ROADMAP.md](milestones/v2.1-ROADMAP.md)

</details>

<details>
<summary>✅ v2.2 Skills Registry (Phases 11-15) - SHIPPED 2026-03-26</summary>

See [milestones/v2.2-ROADMAP.md](milestones/v2.2-ROADMAP.md)

</details>

<details>
<summary>✅ v2.3 Memory System (Phases 16-19) — SHIPPED 2026-03-27</summary>

See [milestones/v2.3-ROADMAP.md](milestones/v2.3-ROADMAP.md)

</details>

<details>
<summary>✅ v2.4 Sandbox Telegram Fix (Phase 20) — SHIPPED 2026-03-28</summary>

See [milestones/v2.4-ROADMAP.md](milestones/v2.4-ROADMAP.md)

</details>

<details>
<summary>✅ v2.5 RightCron Reliability (Phase 21) — SHIPPED 2026-03-31</summary>

See [milestones/v2.5-ROADMAP.md](milestones/v2.5-ROADMAP.md)

</details>

### 🚧 v3.0 Teloxide Bot Runtime (In Progress)

**Milestone Goal:** Replace Claude Code Telegram channels with a per-agent Rust teloxide bot, move cron execution into a Rust runtime, and give each agent full control over its system prompt.

- [x] **Phase 22: DB Schema** - Add telegram_sessions V2 migration to memory.db (completed 2026-03-31)
- [x] **Phase 23: Bot Skeleton** - rightclaw bot subcommand with env loading, DB open, and no-op teloxide dispatcher (completed 2026-03-31)
- [x] **Phase 24: System Prompt Codegen** - Compose SOUL.md + USER.md + AGENTS.md into system-prompt.txt on rightclaw up; remove shell wrapper codegen (completed 2026-03-31)
- [x] **Phase 25: Telegram Handler + CC Dispatch** - Full message dispatch loop with session continuity, per-thread mpsc queue, and CC subprocess invocation (completed 2026-04-01)
- [x] **Phase 25.5: Agent Definition Codegen** - Generate .claude/agents/<name>.md per agent; migrate bot to --agent + --json-schema structured output (completed 2026-04-01)
- [x] **Phase 26: PC Cutover** - Wire bot into rightclaw up lifecycle; atomic cutover removes CC channels flag and starts teloxide (completed 2026-04-01)
- [ ] **Phase 27: Cron Runtime** - tokio cron task loop reading crons/*.yaml and executing claude -p subprocesses
- [ ] **Phase 28: Cronsync SKILL Rewrite** - Reduce cronsync SKILL.md to file management only; remove all execution logic

## Phase Details

### Phase 22: DB Schema
**Goal**: telegram_sessions table exists in memory.db with semantics correct for CC session continuity bugs
**Depends on**: Phase 21 (existing memory.db with rusqlite_migration)
**Requirements**: SES-01
**Success Criteria** (what must be TRUE):
  1. `rightclaw up` creates memory.db with telegram_sessions table when absent; existing DBs get migration applied
  2. telegram_sessions schema has `thread_id INT NOT NULL DEFAULT 0` (non-nullable — guards against thread_id=1 normalization bug)
  3. `UNIQUE(chat_id, thread_id)` constraint prevents duplicate session rows for the same conversation thread
  4. root_session_id column is present with semantics documented: stores only the first-call session UUID, never updated on resume
**Plans**: 1 plan

Plans:
- [x] 22-01-PLAN.md — V2 migration: SQL file + migrations.rs registration + tests (TDD)

### Phase 23: Bot Skeleton
**Goal**: rightclaw bot --agent runs as a process, reads agent config, and starts a teloxide dispatcher (even with no-op handlers)
**Depends on**: Phase 22
**Requirements**: BOT-01, BOT-03, BOT-04, BOT-05
**Success Criteria** (what must be TRUE):
  1. `rightclaw bot --agent <name>` starts without error when RC_AGENT_DIR, RC_AGENT_NAME, RC_TELEGRAM_TOKEN env vars are set
  2. Bot uses CacheMe<Throttle<Bot>> adaptor ordering (not Throttle<CacheMe<Bot>>) — prevents issue #516 deadlock
  3. SIGTERM causes bot to shut down cleanly: all in-flight claude -p subprocesses receive kill signal before exit
  4. Messages from chat IDs not in allowed_chat_ids are silently dropped (no reply, no log entry)
**Plans**: 3 plans
**UI hint**: no

Plans:
- [x] 23-01-PLAN.md — Add allowed_chat_ids: Vec<i64> to AgentConfig (TDD)
- [x] 23-02-PLAN.md — Create crates/bot workspace crate with teloxide skeleton, adaptor ordering, signal shutdown
- [x] 23-03-PLAN.md — Wire Commands::Bot into rightclaw-cli; smoke test CLI

### Phase 24: System Prompt Codegen
**Goal**: rightclaw up produces a system-prompt.txt per agent from present identity files; shell wrapper generation is removed
**Depends on**: Phase 21 (existing rightclaw up codegen pipeline)
**Requirements**: PROMPT-01, PROMPT-02, PROMPT-03
**Success Criteria** (what must be TRUE):
  1. `rightclaw up` writes `agent_dir/.claude/system-prompt.txt` by concatenating whichever of SOUL.md, USER.md, AGENTS.md exist; absent files are skipped without error
  2. Shell wrapper files are no longer written to disk on `rightclaw up`; codegen/shell_wrapper.rs is removed from the codebase
  3. claude -p invocations pass `--system-prompt-file agent_dir/.claude/system-prompt.txt` on first-message calls
**Plans**: 3 plans

Plans:
- [x] 24-01-PLAN.md — Rewrite generate_system_prompt, delete shell_wrapper, remove start_prompt field (Wave 1)
- [x] 24-02-PLAN.md — Update cmd_up loop and cmd_replay call site in main.rs (Wave 2)
- [x] 24-03-PLAN.md — Create USER.md template, update AGENTS.md, update init.rs (Wave 1, parallel with 24-01)

### Phase 25: Telegram Handler + CC Dispatch
**Goal**: Bot receives Telegram messages, maps them to Claude sessions, invokes claude -p serially per thread, and replies correctly
**Depends on**: Phase 22, Phase 23, Phase 24
**Requirements**: BOT-02, BOT-06, SES-02, SES-03, SES-04, SES-05, SES-06, DIS-01, DIS-02, DIS-03, DIS-04, DIS-05, DIS-06
**Success Criteria** (what must be TRUE):
  1. First message in a thread triggers `claude -p --session-id <uuid>` with `--output-format json`; the session UUID is stored in telegram_sessions; subsequent messages use `--resume <root_session_id>`
  2. Concurrent messages to the same (chat_id, thread_id) are serialised — no concurrent claude -p calls on the same session; second message waits for first to complete
  3. Bot replies to the correct Telegram thread; General topic messages (thread_id=1) reply without message_thread_id (effective_thread_id=0 normalisation)
  4. `/reset` command deletes the telegram_sessions row for the current thread; next message starts a fresh session
  5. claude -p non-zero exit or stderr output produces an error reply in Telegram; responses over 4096 chars are split into multiple messages
  6. Bot shows ChatAction::Typing indicator while claude -p subprocess is running
**Plans**: 3 plans
**UI hint**: no

Plans:
- [x] 25-01-PLAN.md — Add Cargo deps + session.rs DB CRUD with TDD (Wave 1)
- [x] 25-02-PLAN.md — worker.rs: debounce loop, CC subprocess, reply tool parsing, typing indicator (Wave 2)
- [x] 25-03-PLAN.md — handler.rs + dispatch.rs rewrite: DashMap worker map, BotCommand schema, lib.rs wiring (Wave 3)

### Phase 25.5: Agent Definition Codegen
**Goal**: rightclaw up generates a CC-native agent definition file per agent; bot migrates from --system-prompt-file to --agent + --json-schema structured output
**Depends on**: Phase 25
**Requirements**: AGDEF-01, AGDEF-02, AGDEF-03, AGDEF-04, AGDEF-05
**Success Criteria** (what must be TRUE):
  1. `rightclaw up` generates `agent_dir/.claude/agents/<name>.md` with YAML frontmatter (name, model, description, tools whitelist) + body concatenated from present IDENTITY.md → SOUL.md → USER.md → AGENTS.md
  2. worker.rs first call uses `--agent <name> --json-schema <reply_schema> --output-format json --session-id <uuid>`; `--system-prompt-file` and `--append-system-prompt` flags are gone
  3. worker.rs resume call uses `--resume <root_session_id> --json-schema <reply_schema> --output-format json`; no `--agent` (system prompt stored in session)
  4. Output parsing reads direct structured JSON (reply schema: content, reply_to_message_id, media_paths); tool_use block parsing removed
  5. `system-prompt.txt` no longer written by `rightclaw up`; `codegen/system_prompt.rs` removed
**Plans**: 2 plans

Plans:
- [x] 25.5-01-PLAN.md — agent_def.rs codegen module + cmd_up/cmd_pair wiring + system_prompt.rs deletion
- [x] 25.5-02-PLAN.md — worker.rs invoke_cc rewrite + parse_reply_output + test updates

### Phase 26: PC Cutover
**Goal**: rightclaw up starts teloxide bot processes and removes all CC channels infrastructure atomically
**Depends on**: Phase 25.5
**Requirements**: PC-01, PC-02, PC-03, PC-04, PC-05
**Success Criteria** (what must be TRUE):
  1. `rightclaw up` generates a `<agent>-bot` process-compose entry for every agent with telegram_token set; entry includes RC_AGENT_DIR, RC_AGENT_NAME, RC_TELEGRAM_TOKEN environment block
  2. CC channels flag (`--channels plugin:telegram@...`) is absent from all agent launch paths after cutover
  3. is_interactive is removed from the PC template — bot processes do not request TTY
  4. Bot process calls deleteWebhook on startup; `rightclaw doctor` warns when a configured token has an active webhook
**Plans**: 2 plans

Plans:
- [x] 26-01-PLAN.md — process_compose.rs BotProcessAgent + template rewrite + cmd_up channels cleanup
- [x] 26-02-PLAN.md — deleteWebhook in bot/src/lib.rs + doctor webhook check

### Phase 27: Cron Runtime
**Goal**: Cron jobs defined in crons/*.yaml are scheduled and executed by a Rust tokio task inside the bot process
**Depends on**: Phase 23 (bot binary structure)
**Requirements**: CRON-01, CRON-02, CRON-03, CRON-04, CRON-05, CRON-06
**Success Criteria** (what must be TRUE):
  1. A cron job defined in `agent_dir/crons/<name>.yaml` fires within one schedule interval of its next due time after `rightclaw bot` starts
  2. Cron runtime re-reads `crons/` every 60 seconds; adding or removing a YAML file takes effect without restarting the bot
  3. A job with a fresh lock file is skipped — no duplicate execution when a previous run is still in progress
  4. Removing a cron spec and waiting one polling interval results in the job no longer being scheduled; re-adding it schedules it again (idempotent reconciler)
**Plans**: 2 plans

Plans:
- [ ] 25.5-01-PLAN.md — agent_def.rs codegen module + cmd_up/cmd_pair wiring + system_prompt.rs deletion
- [ ] 25.5-02-PLAN.md — worker.rs invoke_cc rewrite + parse_reply_output + test updates

### Phase 28: Cronsync SKILL Rewrite
**Goal**: cronsync SKILL.md manages only cron spec files in crons/ directory; all execution logic is handled by the Rust runtime
**Depends on**: Phase 27
**Requirements**: SKILL-01, SKILL-02, SKILL-03
**Success Criteria** (what must be TRUE):
  1. cronsync SKILL.md contains only file-management instructions: create, edit, delete YAML spec files in `crons/`
  2. All CHECK/RECONCILE/CRITICAL guard logic is absent from SKILL.md — no reference to CronCreate, CronList, CronDelete, or Agent tool invocation
  3. BOOT-01/BOOT-02 startup bootstrap references are absent from SKILL.md
**Plans**: 2 plans

Plans:
- [ ] 25.5-01-PLAN.md — agent_def.rs codegen module + cmd_up/cmd_pair wiring + system_prompt.rs deletion
- [ ] 25.5-02-PLAN.md — worker.rs invoke_cc rewrite + parse_reply_output + test updates

## Progress

**Execution Order:**
Phases execute in order: 22 → 23 (parallel with 24) → 25 → 26, 27 (parallel) → 28

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 22. DB Schema | v3.0 | 1/1 | Complete   | 2026-03-31 |
| 23. Bot Skeleton | v3.0 | 3/3 | Complete    | 2026-03-31 |
| 24. System Prompt Codegen | v3.0 | 3/3 | Complete    | 2026-03-31 |
| 25. Telegram Handler + CC Dispatch | v3.0 | 3/3 | Complete    | 2026-04-01 |
| 26. PC Cutover | v3.0 | 2/2 | Complete   | 2026-04-01 |
| 27. Cron Runtime | v3.0 | 0/? | Not started | - |
| 28. Cronsync SKILL Rewrite | v3.0 | 0/? | Not started | - |
