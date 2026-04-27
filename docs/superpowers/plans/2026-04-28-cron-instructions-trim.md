# Cron Management Instructions Trim — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 28-line `## Cron Management (RightCron)` block (lines 175–202) in `templates/right/prompt/OPERATING_INSTRUCTIONS.md` with a 4-line block that drops obsolete instructions and keeps only the always-on rule about auto-delivery.

**Architecture:** Single-file template edit. The template is baked into the `OPERATING_INSTRUCTIONS` Rust constant via `include_str!` and injected into every agent's system prompt at session start. No Rust source changes; existing codegen tests assert on `## Your Files` and `## MCP Management` headings only, so the renamed cron heading and shrunken body do not break tests.

**Tech Stack:** Markdown template + Rust `include_str!` build-time include.

**Spec:** `docs/superpowers/specs/2026-04-28-cron-instructions-trim-design.md`

---

## File Structure

| File | Change |
|---|---|
| `templates/right/prompt/OPERATING_INSTRUCTIONS.md` | Modify: replace lines 175–202 with the 4-line block (Task 1) |

**Untouched (intentional):**
- `skills/rightcron/SKILL.md` — already covers `target_chat_id`, `cron_list_runs`, `cron_show_run`.
- `templates/right/agent/TOOLS.md:14` — single sentence pointing to the skill, still correct.
- `crates/bot/src/telegram/worker.rs:1163`, `crates/bot/src/cron.rs:289` — `--disallowedTools` list (this is what makes the deleted `NEVER call CronCreate` warning redundant).
- `crates/right-agent/src/codegen/agent_def.rs` — `include_str!` site, no edit needed.
- `ARCHITECTURE.md`, `PROMPT_SYSTEM.md` — no references to the deleted text.
- `docs/superpowers/specs/*` and `docs/superpowers/plans/2026-04-13-system-prompt-restructure.md` — historical artifacts, frozen.

---

## Task 1: Trim the Cron Management block in OPERATING_INSTRUCTIONS.md

**Files:**
- Modify: `templates/right/prompt/OPERATING_INSTRUCTIONS.md` (lines 175–202)

- [ ] **Step 1: Read the target file to confirm line numbers**

Run: `Read templates/right/prompt/OPERATING_INSTRUCTIONS.md` (full file)
Expected: line 175 starts with `## Cron Management (RightCron)`, line 202 is the last line of that block (the `target_thread_id` parenthetical sentence). The next H2 heading after the block is `## MCP Error Diagnosis`.

If the line numbers have drifted (the file may have been edited since the spec was written), update Step 2's `old_string` to match the current contents. The semantic content of the block to be replaced is unchanged.

- [ ] **Step 2: Replace the block via Edit**

Use the Edit tool on `templates/right/prompt/OPERATING_INSTRUCTIONS.md`.

`old_string` (current block, exactly as it stands in the file — verify against the Read in Step 1 before submitting):

```markdown
## Cron Management (RightCron)

**On startup:** Run `/rightcron` immediately. It will bootstrap the reconciler
and recover any persisted jobs. Do this before responding to the user.

**For user requests:** When the user wants to manage cron jobs, scheduled tasks,
or recurring tasks, ALWAYS use the /rightcron skill. NEVER call CronCreate
directly — always write a YAML spec first, then reconcile.

**Viewing results:** Use `mcp__right__cron_list_runs` and `mcp__right__cron_show_run`
to see cron run results. They return the summary and notify content directly —
no need to read log files.

**Automatic delivery:** When a cron job produces a notification (`notify` in its
output), the platform automatically delivers it to Telegram after 3 minutes of
chat inactivity. You do not need to relay cron results manually — the delivery
system handles it. If the user asks about a cron result before delivery, use the
MCP tools above to show them the data.

**Always pass `target_chat_id`** when creating or updating a cron job. Set it to
the `chat.id` value from the incoming message YAML — this ensures the cron delivers
back to the same chat where the user made the request. The MCP tool rejects any
chat ID that is not in the agent's allowlist. For supergroup topics, also pass
`target_thread_id` from the message's `chat.topic_id` if the user wants the
notification to land in that specific topic. To change a cron's destination later,
call `mcp__right__cron_update` with the new `target_chat_id` (and optionally
`target_thread_id`). Only use a different chat ID if the user explicitly asks to
deliver to a different chat.
```

`new_string`:

```markdown
## Cron Management

When the user wants to schedule, create, list, or remove cron jobs, use the
`/rightcron` skill. Cron results are auto-delivered to Telegram after 3 minutes
of chat inactivity — do NOT relay them manually; the delivery loop will surface
them when the user becomes idle.
```

- [ ] **Step 3: Read the file again to verify the structure**

Run: `Read templates/right/prompt/OPERATING_INSTRUCTIONS.md` (offset around former line 175, limit ~30)
Expected:
- The new `## Cron Management` heading is followed by exactly four lines of body content (one paragraph, no blank-line gaps inside) — wait, the new block is a single multi-line paragraph. Verify it appears as one paragraph followed by a blank line and then `## MCP Error Diagnosis`.
- No leftover `(RightCron)` parenthetical, no `**On startup:**`, no `CronCreate`, no `**Always pass ...**` paragraph.
- The H2 sections immediately before (`## Sending Attachments` and its subsection `### Media Groups (Albums)`) and immediately after (`## MCP Error Diagnosis`) are unchanged.

- [ ] **Step 4: Verify no orphan references in live code/templates/active docs**

Run:
```bash
rg -n "On startup.*Run.*rightcron|bootstrap the reconciler|YAML spec first|NEVER call CronCreate" \
  templates crates ARCHITECTURE.md PROMPT_SYSTEM.md skills
```
Expected: zero results.

Then:
```bash
rg -n "rightcron|/rightcron" templates crates ARCHITECTURE.md PROMPT_SYSTEM.md skills
```
Expected results (live references only):
- `templates/right/prompt/OPERATING_INSTRUCTIONS.md` — the new compact pointer
- `templates/right/agent/TOOLS.md:14` — `For cron / scheduled tasks use the /rightcron skill.`
- `skills/rightcron/SKILL.md` — the skill file itself
- `skills/rightskills/SKILL.md` — table row listing `rightcron` as an installable skill
- `ARCHITECTURE.md` — contextual references to the cron flow (no edit needed)
- `crates/right-agent/src/codegen/...` — skill installer paths (no edit needed)

If any unexpected reference appears in `templates/`, `crates/`, `ARCHITECTURE.md`, or `PROMPT_SYSTEM.md` (i.e. a place that *should* have been updated and wasn't), stop and investigate before continuing.

- [ ] **Step 5: Build the workspace**

Run: `cargo build --workspace`
Expected: clean build. The constant `OPERATING_INSTRUCTIONS` is `include_str!`-ed at compile time; the smaller body changes the constant's value but not its type.

- [ ] **Step 6: Run the codegen tests**

Run: `cargo test -p right-agent --lib codegen::agent_def_tests`
Expected: all tests pass. The relevant test is `operating_instructions_constant_is_non_empty` at `crates/right-agent/src/codegen/agent_def_tests.rs:115`, which asserts the constant is non-empty and contains the headings `## Your Files` and `## MCP Management`. Both headings are still present after the edit.

- [ ] **Step 7: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: all tests pass. (Project convention: integration tests against live OpenShell sandboxes are not `#[ignore]`'d; if you do not have OpenShell running locally, sandbox tests fail and that is unrelated to this change. In that case, document the failures and proceed.)

- [ ] **Step 8: Commit**

```bash
git add templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "$(cat <<'EOF'
docs(prompt): trim cron management block in operating instructions

Remove the obsolete startup directive (the cron reconciler runs autonomously
inside the bot at crates/bot/src/cron.rs:782 and does not need agent
bootstrap) and the dead CronCreate warning (the CC-native cron tools are
already in --disallowedTools at crates/bot/src/telegram/worker.rs:1163 and
crates/bot/src/cron.rs:289). The deleted target_chat_id and run-history
guidance is fully covered in skills/rightcron/SKILL.md, which the agent
loads on demand. Keep the auto-delivery rule in always-on context because
the agent receives cron results outside the skill-loaded path.

Spec: docs/superpowers/specs/2026-04-28-cron-instructions-trim-design.md
EOF
)"
```

Expected: one commit on the current branch (`prompt-cleanup`), one file changed.

---

## Self-Review Notes (already applied)

- **Spec coverage:** every spec section (replacement text, what survives where, verification, risks) is covered by Steps 2 (edit), 3 (structure), 4 (orphan refs), 5–7 (build/tests). The "PROMPT_SYSTEM.md sync" verification was resolved at design time — confirmed not needed — and is not repeated here.
- **Placeholder scan:** no TBD/TODO; all code blocks contain exact strings.
- **Type consistency:** N/A (no Rust types touched).
- **Out-of-scope flag from spec** (stale doc-comment in `crates/bot/src/cron.rs:774` claiming `Polls 'crons/*.yaml' every 60s` when the loop polls SQLite every 5s) is intentionally NOT addressed in this plan; file separately.
