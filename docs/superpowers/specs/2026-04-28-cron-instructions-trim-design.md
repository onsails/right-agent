# Cron Management Instructions Trim — Design

## Goal

Replace the 28-line `## Cron Management (RightCron)` block in
`templates/right/prompt/OPERATING_INSTRUCTIONS.md` (lines 175–202) with a
4-line block. Eliminate obsolete instructions that waste tokens and prime the
agent toward dead workflows, while preserving the one fact that cannot live
elsewhere.

**Primary motivation:** `OPERATING_INSTRUCTIONS.md` is compiled into the system
prompt of every agent session. Two of its current cron-block instructions are
defunct:

1. `On startup: Run /rightcron immediately. It will bootstrap the reconciler
   and recover any persisted jobs.` — The cron reconciler runs autonomously
   inside the bot process (`crates/bot/src/cron.rs:782` `run_cron_task`), polling
   the SQLite store every 5 seconds. The agent has no role in bootstrapping.
   The instruction nonetheless coerces the agent to invoke `/rightcron` on the
   first turn of every session, loading ~250 lines of skill content with no
   effect.
2. `NEVER call CronCreate directly — always write a YAML spec first, then
   reconcile.` — The CC-native `CronCreate`/`CronList`/`CronDelete` tools are
   already in `--disallowedTools` (`crates/bot/src/telegram/worker.rs:1163`,
   `crates/bot/src/cron.rs:289`); CC will not load them. The "YAML spec +
   reconcile" workflow does not exist — the current `skills/rightcron/SKILL.md`
   exposes only MCP tools (`cron_create`/`cron_update`/`cron_delete`).

The remaining content of the block (`mcp__right__cron_list_runs`,
`cron_show_run`, `target_chat_id`, supergroup-thread guidance) duplicates
material already present in `skills/rightcron/SKILL.md` ("Checking Run History"
at line 160, "Delivery Target" at lines 130–146). The skill is loaded on demand
when the agent calls `/rightcron`, which is the only context where this
guidance applies.

**Non-goal:** changes to `skills/rightcron/SKILL.md`. Its existing coverage of
delivery target and run history is sufficient.

## Scope

**In scope:**

- Single edit to `templates/right/prompt/OPERATING_INSTRUCTIONS.md`, lines
  175–202.
- Verification that no other file references the deleted text in a way that
  would be left dangling.

**Out of scope:**

- `skills/rightcron/SKILL.md`.
- Code changes (no Rust touched).
- Other sections of `OPERATING_INSTRUCTIONS.md`.

## Replacement Text

The current block (lines 175–202) is replaced verbatim by:

```markdown
## Cron Management

When the user wants to schedule, create, list, or remove cron jobs, use the
`/rightcron` skill. Cron results are auto-delivered to Telegram after 3 minutes
of chat inactivity — do NOT relay them manually; the delivery loop will surface
them when the user becomes idle.
```

## What Survives Where

| Topic | Current home | Post-edit home | Rationale |
|---|---|---|---|
| Skill discovery (`/rightcron`) | `OPERATING_INSTRUCTIONS.md` | `OPERATING_INSTRUCTIONS.md` | Always-on pointer so the agent knows when to load the skill. |
| 3-min idle delivery + "do not relay manually" | `OPERATING_INSTRUCTIONS.md` (verbose) | `OPERATING_INSTRUCTIONS.md` (compact) | Required when the agent *receives* a cron result via the delivery loop. The skill is not loaded in that path, so the rule must live in the always-on prompt. |
| `target_chat_id`, `target_thread_id`, allowlist semantics | both | `skills/rightcron/SKILL.md` only ("Delivery Target") | Only relevant during create/update — agent has the skill loaded then. |
| `cron_list_runs` / `cron_show_run` usage | both | `skills/rightcron/SKILL.md` only ("Checking Run History") | Only relevant when the agent is debugging history — agent has the skill loaded then. |
| `On startup: Run /rightcron` | `OPERATING_INSTRUCTIONS.md` | deleted | Reconciler is autonomous; instruction is dead. |
| `NEVER call CronCreate ... write a YAML spec first` | `OPERATING_INSTRUCTIONS.md` | deleted | Tools are already in `--disallowedTools`; YAML workflow does not exist. |

## Verification

After the edit, run the following checks:

1. **Markdown structure intact.** Read the file and confirm neighbouring H2
   sections (`## Message Input Format`, `## Sending Attachments`,
   `## MCP Error Diagnosis`, `## Core Skills`, `## System Notices`) are
   unmoved and unchanged.
2. **No orphan references.** Run
   `rg "rightcron|reconciler|CronCreate" templates docs PROMPT_SYSTEM.md
   crates/right-agent/src/codegen` and confirm only live references remain
   (skill loader, `--disallowedTools` list, the skill file itself, the new
   compact pointer).
3. **PROMPT_SYSTEM.md sync.** Verified at design time:
   `PROMPT_SYSTEM.md` does not currently mention the cron block, so no sync
   is required. If a future change adds cron-related copy there, the project
   convention applies — keep `PROMPT_SYSTEM.md` aligned with the actual
   prompting system.

## Risks and Rollback

- **Risk:** the agent stops invoking `/rightcron` because the imperative
  startup instruction is gone. Acceptable — invoking the skill on startup with
  no user request was the bug we are fixing.
- **Risk:** the agent forgets to pass `target_chat_id` because the duplicate
  reminder is gone. Mitigated — the skill itself ("Delivery Target") states
  this requirement, and the agent is required to consult the skill before any
  cron operation.
- **Risk:** the agent attempts to relay cron results manually after the trim.
  Mitigated — the surviving 4-line block retains the "do NOT relay them
  manually" directive in always-on context.

Rollback is `git revert` of the single commit.

## Out of Scope, Possibly Worth Filing Separately

- Stale doc-comment in `crates/bot/src/cron.rs:774`:
  `Polls 'crons/*.yaml' every 60s` — the loop polls the SQLite store, not YAML
  files; interval is 5s, not 60s. Not part of this design but flagged for a
  future cleanup pass.
