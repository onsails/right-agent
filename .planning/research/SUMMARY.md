# Project Research Summary

**Project:** RightClaw v3.0 — Teloxide Bot Runtime
**Domain:** Per-agent Rust Telegram bot + claude -p session management + Rust cron runtime
**Researched:** 2026-03-31
**Confidence:** HIGH

## Executive Summary

RightClaw v3.0 replaces broken Claude Code Telegram channels (SEED-011, iv6/M6 gap) with a per-agent Rust teloxide binary that owns the full Telegram conversation loop. The recommended approach: one `rightclaw bot --agent <name>` subcommand (same binary, new subcommand) launched as a process-compose entry per agent alongside the existing CC session process. This bot handles long polling, maps `(chat_id, thread_id)` to Claude session UUIDs in `memory.db`, and invokes `claude -p --resume` as subprocesses. The architecture is a clean replacement, not a parallel system — the old `--channels` flag and shell wrapper generation are removed entirely in the same `rightclaw up` release.

The core risks are concentrated in two areas: CC session continuity bugs and the telegram_sessions schema design. Three confirmed CC upstream bugs (issue #16103, #8069, #1967) interact: `--resume` ignores `CLAUDE_CONFIG_DIR` (use HOME isolation instead), resumed sessions return a new session_id in JSON output (store only the root ID, never update on resume), and `--resume` has known regression in some CC versions (have a fallback strategy). All three must be designed around from day one — they are not fixable upstream. The second major risk area is message concurrency: concurrent `claude -p` invocations on the same session corrupt the JSONL file, requiring a per-session message queue (mpsc channel) as a fundamental architectural choice, not an afterthought.

The cron runtime (also v3.0) is lower risk: it replaces CC-native CronCreate tools with a tokio task loop reading `crons/*.yaml`. This eliminates the v2.5 BOOT-01/BOOT-02/CRITICAL guard complexity entirely, since all of that was a workaround for CC's main-thread restriction on CronCreate. The main pitfalls are using `notify-debouncer-full` (not mini) with crossbeam disabled, and making the reconciler fully idempotent.

## Key Findings

### Recommended Stack

The v3.0 delta adds exactly four new dependencies to the workspace. All are well-validated and the versions are confirmed stable as of 2026-03-31. `teloxide 0.17` (macros feature only) is the only serious Rust Telegram framework; its dptree dispatcher maps cleanly to the "message → dispatch to agent session" pattern. File watching uses `notify 8.2` + `notify-debouncer-full 0.7` — the full debouncer is required for rename-stitching (editors write via temp+rename). Schedule parsing uses `cron 0.16` as a pure computation primitive; the heavier `tokio-cron-scheduler` is explicitly rejected because RightClaw owns the scheduler loop. No new HTTP server, no webhook infrastructure, no teloxide storage backends.

**Core new technologies:**
- `teloxide 0.17` (macros only): Telegram bot framework — dominant in Rust, tokio-native, long polling built-in, dptree filter chains
- `notify 8.2` + `notify-debouncer-full 0.7`: Cross-platform file watching for `crons/` directory — debouncer required for editor write patterns (rename-into-place)
- `cron 0.16`: Pure cron expression parsing → `chrono::DateTime<Utc>` — no scheduler framework, just the primitive for next-run computation
- No new HTTP client: existing `reqwest` + `tokio::process::Command` from stdlib handle everything else

**Existing stack confirmed unchanged:** tokio, serde, rusqlite, minijinja, reqwest, thiserror, miette, tracing — not re-evaluated.

**What NOT to add:** `tokio-cron-scheduler` (embeds competing scheduler loop), `notify-debouncer-mini` (older, less event detail), teloxide `sqlite-storage`/`redis-storage`/`webhooks-axum` features (all unnecessary), `axum`/`tower` (no HTTP server needed — long polling).

### Expected Features

**Must have (table stakes) — v3.0:**
- Per-agent teloxide process entry in PC config (conditional on `telegram_token` in agent.yaml)
- `telegram_sessions` table: `(chat_id, thread_id NOT NULL DEFAULT 0)` → root session UUID
- Session continuity via `claude -p --resume <root_id>` or `--session-id <root_id>` fallback
- Thread routing: `message_thread_id` passed back to `SendMessage`; General topic (thread_id=1) normalized to 0
- System prompt composed from SOUL.md + USER.md + AGENTS.md → `system-prompt.txt` on `rightclaw up`
- Per-session mpsc message queue — serialize concurrent messages to same thread (architectural requirement)
- `/reset` command to clear session row for current thread
- Error forwarding: subprocess failure → informative Telegram message (not silence)
- Response splitting: Telegram 4096-char limit
- Cron runtime: tokio tasks + `crons/*.yaml` reading + lock file check + `claude -p` execution

**Should have (differentiators) — v3.1:**
- Real-time streaming response (edit-in-place via `--output-format stream-json` + `editMessageText`)
- `allowed_chat_ids` enforcement in agent.yaml
- `/fast` prefix for `--bare` mode (faster, no MCP tools)
- `notify` file watcher hot-reload for cron specs (60s polling acceptable for v3.0)

**Defer (v3.x+):**
- Webhook support (zero ops value for developer-machine deployments)
- File/image handling from Telegram (significant attack surface, text-only for v3.0)

**Anti-features to avoid:**
- In-process Anthropic API calls — no tools, no memory, no sandbox; CC subprocess IS the agent
- Stateful in-process CC — CC doesn't support stdin piping for external control
- Teloxide dialogue FSM — CC session is the state machine; don't duplicate
- Broadcasting/scheduled messages from bot — cron runtime handles scheduled tasks separately

### Architecture Approach

The teloxide bot runs as `rightclaw bot --agent <name>` — a new subcommand in the existing binary using `current_exe()` as the binary path in PC config (same pattern as `memory-server`). No separate crate required unless the existing binary grows too large. The bot process runs two concurrent tokio tasks: (1) teloxide long-polling dispatcher for Telegram messages, and (2) cron runtime loop watching `crons/` and evaluating schedules. CC subprocesses are invoked via `tokio::process::Command` with `HOME=$AGENT_DIR`, `cwd=$AGENT_DIR`, `wait_with_output()` (never `wait()`), `kill_on_drop(true)`, and separate stdout/stderr pipes.

The process-compose template changes significantly: shell wrapper generation is removed, `is_interactive` is removed (bots don't need TTY), and an `environment:` block with `RC_AGENT_DIR`/`RC_AGENT_NAME`/`RC_TELEGRAM_TOKEN` is added. The cutover is atomic — CC channels flag and shell wrapper codegen are removed in the same `rightclaw up` that starts teloxide processes.

**Major components and changes:**
1. `rightclaw bot` subcommand (NEW) — teloxide dispatcher + CC subprocess invocation + cron runtime as concurrent tokio tasks
2. `memory/` V2 migration — adds `telegram_sessions(chat_id, thread_id NOT NULL DEFAULT 0, root_session_id, ...)`
3. `codegen/process_compose.rs` — PC YAML with bot binary + env block; `wrapper_path` replaced by `bot_binary_path`
4. `codegen/system_prompt.rs` — compose SOUL+USER+AGENTS → `system-prompt.txt` (new step in `rightclaw up`)
5. `codegen/shell_wrapper.rs` — REMOVED (shell wrappers gone entirely)
6. `cronsync SKILL.md` — reduced to file management only; execution logic moves to Rust runtime

**Data flow:** Telegram update → teloxide dispatcher → per-session mpsc queue → lookup `(chat_id, thread_id)` in memory.db → spawn `claude -p [--resume <root_id>]` with `HOME=$AGENT_DIR` → `wait_with_output()` → `bot.send_message(chat_id, response)`.

### Critical Pitfalls

1. **`--resume` ignores CLAUDE_CONFIG_DIR (CC bug #16103, closed "not planned")** — Use `HOME=$AGENT_DIR` isolation for per-agent CC invocations, never `CLAUDE_CONFIG_DIR`. Sessions stored under `$HOME/.claude/projects/` are correctly found by `--resume` with HOME override.

2. **Resume returns a new session_id in JSON output (CC bug #8069, unfixed)** — Store ONLY the root session_id from the first `claude -p` call for a thread. Never update `telegram_sessions` with the session_id returned from a `--resume` call. Schema needs explicit `root_session_id` column semantics.

3. **Concurrent messages on same thread corrupt session JSONL** — `claude -p` has no file-level locking. Implement a per-`(chat_id, thread_id)` mpsc channel; process messages serially per thread. This is the fundamental dispatch architecture — not an optimization added later.

4. **stdout pipe deadlock at 64KB** — `claude -p` output easily exceeds Linux's default 64KB pipe buffer on code generation tasks. Always use `child.wait_with_output()` (tokio), never `child.wait()`. Set `stdin(Stdio::null())`.

5. **Telegram General topic (thread_id=1) rejects `message_thread_id` in Bot API** — Normalize `thread_id = Some(1)` to `None`/0 for both session keying and reply routing. Use an `effective_thread_id()` helper. Cannot be caught in unit tests — requires a real forum supergroup.

6. **teloxide `Throttle` adaptor deadlock (issue #516, confirmed unfixed)** — Use `CacheMe<Throttle<Bot>>` ordering (Throttle innermost). Add process-compose `restart: on-failure` with backoff. Per-process isolation means one deadlock doesn't cascade to other agents.

## Implications for Roadmap

The ARCHITECTURE.md build order (A→B→C→D→E→F→G) is the correct sequencing. Session schema design must happen first because CC bugs dictate schema semantics. Telegram handler dispatch architecture (per-session queue) must be decided before writing a single message handler. Suggested phases:

### Phase 1: DB Schema + Session Design
**Rationale:** The `telegram_sessions` schema is the foundational constraint. CC bugs (#16103, #8069) dictate schema semantics before any handler code exists. Getting column semantics wrong (nullable thread_id, updating session_id on resume) is a correctness bug that compounds across all later phases.
**Delivers:** V2 rusqlite_migration; `telegram_sessions(chat_id INT, thread_id INT NOT NULL DEFAULT 0, root_session_id TEXT, created_at, last_used_at, UNIQUE(chat_id, thread_id))`; documented session continuity strategy (HOME isolation, root-ID-only storage policy)
**Addresses:** Session continuity (table stakes)
**Avoids:** Pitfalls 1+2 (schema design bugs that are architectural mistakes if discovered late)

### Phase 2: `rightclaw bot` Subcommand Skeleton + Agent Dir Loading
**Rationale:** Establish the binary entry point, env var reading, DB connection, and system-prompt.txt loading before writing any dispatch logic. Gives a runnable binary other phases can extend.
**Delivers:** `rightclaw bot --agent <name>` subcommand; reads RC_AGENT_DIR/RC_AGENT_NAME/RC_TELEGRAM_TOKEN[_FILE]; opens memory.db; loads system-prompt.txt; compiles and starts teloxide dispatcher (no-op handlers)
**Uses:** teloxide 0.17 (bot construction only), existing rusqlite, existing tracing setup
**Implements:** Bot process architecture component

### Phase 3: System Prompt Composition in `rightclaw up`
**Rationale:** Parallel with Phase 2. `rightclaw up` must generate `system-prompt.txt` before the bot can pass useful context to CC. Also removes shell wrapper generation — this is the right phase since both changes are in `cmd_up` codegen.
**Delivers:** `codegen/system_prompt.rs` generates `agent_dir/.claude/system-prompt.txt` from present SOUL.md + USER.md + AGENTS.md; `codegen/shell_wrapper.rs` removed; PC template updated with env block and `is_interactive` removed

### Phase 4: Telegram Message Handler + CC Subprocess Invocation
**Rationale:** Core of the milestone. With schema, binary skeleton, and system prompt in place, implement the full message dispatch loop. Concurrency architecture (per-session mpsc queue) must be the design, not a retrofit.
**Delivers:** Functional bot receiving messages, mapping to sessions via `root_session_id`, invoking `claude -p [--resume]` serially per thread, replying to correct thread; `/reset` command; error forwarding; response splitting at 4096 chars; graceful shutdown with `kill_on_drop(true)`
**Uses:** teloxide dptree handler chain, `distribution_function` on `(chat_id, thread_id)`, `tokio::sync::mpsc` per session, `tokio::process::Command` with `wait_with_output()`
**Avoids:** Pitfalls 3 (concurrent messages), 4 (pipe deadlock), 5 (General topic normalization), 9 (zombie processes)

### Phase 5: process-compose Template + `rightclaw up` Wiring
**Rationale:** Wire the bot into the full `rightclaw up` lifecycle. This is the atomic cutover: CC channels flag removed, teloxide started, doctor check added.
**Delivers:** PC config entry for `<agent>-bot` conditional on telegram_token; CC channels flag removed from all code paths; `deleteWebhook` call on startup to clear any prior webhook; doctor check for active webhooks on configured tokens; `restart: on-failure` + backoff in PC entry
**Avoids:** Pitfall 12 (polling + CC channels conflict — hard cutover required)

### Phase 6: Cron Runtime
**Rationale:** Independent of Telegram functionality. Depends only on Phase 2 skeleton. Lower risk than bot dispatch. Eliminates v2.5 CC-native cron complexity entirely.
**Delivers:** tokio task loop reading `crons/*.yaml`; `cron 0.16` schedule parsing → `tokio::time::sleep_until`; lock file check before execution; `claude -p --system-prompt-file ... -- "<job prompt>"` subprocess; deterministic UUID per job (uuid5 of agent+job name); 60s polling fallback (notify watcher deferred to v3.1)
**Uses:** cron 0.16, notify 8.2, notify-debouncer-full 0.7 (or 60s poll for v3.0)
**Avoids:** Pitfalls 10 (crossbeam disabled), 11 (idempotent reconciler, 500ms debounce)

### Phase 7: Cronsync SKILL.md Rewrite
**Rationale:** File-management-only reduction. All CC-native CronCreate tools and BOOT-01/BOOT-02/CRITICAL guard logic are removed. Must ship after Phase 6 runtime is stable and validated.
**Delivers:** Simplified cronsync SKILL.md that only creates/edits/deletes YAML spec files in `crons/`; removes all inline bootstrap and CHECK/RECONCILE/CRITICAL guard logic
**Implements:** Clean skill-runtime separation; reduces SKILL.md complexity by ~60%

### Phase Ordering Rationale

- Schema must precede handler code because CC session bugs dictate semantics — wrong schema discovered after v3.0 ships means migration under live session data.
- Phase 2 (binary skeleton) and Phase 3 (system prompt codegen) can develop in parallel — no interdependency, both must complete before Phase 4.
- The cutover (Phase 5) must be atomic — no intermediate state where CC channels and teloxide both poll the same token. Define and test the cutover procedure before writing any bot handler code.
- Cron runtime (Phase 6) is independent of Telegram dispatch and can develop in parallel with Phases 4-5, but wiring into `rightclaw up` happens after Phase 5 is stable.
- Cronsync SKILL.md rewrite (Phase 7) is last — simplifies existing code, cannot regress if the runtime is not yet fully stable.

### Research Flags

Phases needing deeper investigation during planning:
- **Phase 4 (Telegram handler):** Verify `--resume` behavior on the deployed CC version before building session continuity. Bug #1967 regression status is MEDIUM confidence (single community source). Test `claude -p --resume <uuid> "prompt"` and verify it (a) finds the session and (b) returns the same session_id. Have the `--session-id` fallback (stateless per call) tested and ready before committing to `--resume`.
- **Phase 4 (Throttle deadlock):** Teloxide issue #516 has no upstream fix. Validate `CacheMe<Throttle<Bot>>` ordering mitigation experimentally with message bursts before shipping. Do not assume the workaround fully prevents the deadlock.

Standard patterns (skip research-phase):
- **Phase 1:** SQLite migration via rusqlite_migration is an established project pattern — already used in v2.3 memory system.
- **Phase 2:** `current_exe()` subcommand pattern already used for `memory-server`.
- **Phase 3:** File concatenation with graceful missing-file handling — no research needed.
- **Phase 5:** `deleteWebhook` API call is one-liner; doctor check patterns established.
- **Phase 6:** `cron` crate + tokio sleep loop is well-documented; pitfalls are fully addressed in research.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | All 4 new crate versions confirmed on crates.io 2026-03-31; rejections rationale sourced from official docs and crate comparisons |
| Features | HIGH | Feature set derived from existing architecture constraints + official teloxide 0.17 / CC CLI docs; anti-features grounded in confirmed bugs |
| Architecture | HIGH | Build order validated against existing codebase; component boundaries follow established project patterns (current_exe, memory-server subcommand) |
| Pitfalls | HIGH (CC bugs, Throttle, General topic) / MEDIUM (notify patterns) | CC session bugs are confirmed GitHub issues; Throttle deadlock confirmed; notify crossbeam pattern from forum posts |

**Overall confidence:** HIGH

### Gaps to Address

- **`--resume` regression status:** CC issue #1967 closed but regression reported in v1.0.51+. Validate against the exact CC binary deployed before Phase 4 implementation. If broken, fallback to `--session-id <deterministic-uuid>` with stateless prompts — viable but loses conversation history. This fallback must be implemented regardless.
- **Throttle deadlock mitigation:** Issue #516 is confirmed unfixed. The `CacheMe<Throttle<Bot>>` ordering fix is from a GitHub issue comment (#649), not official docs. Validate experimentally before shipping Phase 4.
- **notify crate version discrepancy:** ARCHITECTURE.md references notify v7.x while STACK.md specifies 8.2. Use 8.2 — current stable. The sync→async bridge pattern (`blocking_send` into tokio mpsc) is version-independent.
- **`--bare` vs MCP tools tradeoff:** Explicitly decide in Phase 4 whether bot invocations use `--bare` (faster, no rightmemory MCP tool) or load `.mcp.json` (slower, memory tools available). FEATURES.md defers `/fast` prefix to v3.1 but the default for all bot invocations must be chosen at Phase 4 and documented.

## Sources

### Primary (HIGH confidence)
- [teloxide 0.17.0 on crates.io / docs.rs](https://docs.rs/teloxide/latest/teloxide/) — dispatcher, dptree, distribution_function, message types, Throttle adaptor
- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference) — `-p`, `--resume`, `--session-id`, `--system-prompt-file`, `--output-format`, `--bare` flags
- [tokio::process::Child docs](https://docs.rs/tokio/latest/tokio/process/struct.Child.html) — `wait_with_output()`, `kill_on_drop()`
- [cron 0.16.0 on crates.io](https://crates.io/crates/cron) — schedule parsing API, chrono dependency
- [notify 8.2.0 on crates.io / docs.rs](https://crates.io/crates/notify) — `recommended_watcher`, tokio integration pattern
- [notify-debouncer-full 0.7.0 on crates.io](https://crates.io/crates/notify-debouncer-full) — rename stitching, crossbeam feature flag
- [Telegram Bot API Reference](https://core.telegram.org/bots/api) — message_thread_id, forum_topic, General topic behavior

### Secondary (MEDIUM confidence)
- [CC issue #16103: --resume ignores CLAUDE_CONFIG_DIR](https://github.com/anthropics/claude-code/issues/16103) — closed "not planned" Feb 2026
- [CC issue #8069: resume returns different session_id](https://github.com/anthropics/claude-code/issues/8069) — confirmed unfixed
- [teloxide issue #516: Throttle deadlock](https://github.com/teloxide/teloxide/issues/516) — confirmed, no fix
- [teloxide issue #649: Throttle+CacheMe ordering](https://github.com/teloxide/teloxide/issues/649) — ordering requirement documented in issue only
- [tdlib/telegram-bot-api issue #447: General topic spurious thread_id](https://github.com/tdlib/telegram-bot-api/issues/447) — confirmed Bot API bug
- [grammY Flood Control Guide](https://grammy.dev/advanced/flood) — rate limits (30 msg/sec global, 20 msg/min group)
- [tokio issue #2685: zombie processes when Child dropped](https://github.com/tokio-rs/tokio/issues/2685) — confirmed behavior

### Tertiary (LOW confidence)
- [CC issue #1967: --resume broken in print mode](https://github.com/anthropics/claude-code/issues/1967) — fix merged but regression reported; needs live validation
- [CC subprocess guard (CC 2.1.39)](https://x.com/dani_avila7/status/2021786412861862036) — single source; relevant only for subprocess-of-subprocess (not v3.0 pattern)
- [notify forum: duplicate events under load](https://users.rust-lang.org/t/problem-with-notify-crate-v6-1/99877) — crossbeam pattern validation

---
*Research completed: 2026-03-31*
*Ready for roadmap: yes*
