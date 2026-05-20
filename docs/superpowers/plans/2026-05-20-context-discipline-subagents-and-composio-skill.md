# Context discipline: subagent rule + composio core skill — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** [`docs/superpowers/specs/2026-05-20-context-discipline-subagents-and-composio-skill-design.md`](../specs/2026-05-20-context-discipline-subagents-and-composio-skill-design.md)

**Goal:** Tighten the subagent-delegation rule in the agent system prompt and ship a bundled `right-composio` core skill that enforces workbench-vs-context discipline on Composio MCP calls.

**Architecture:** Two coupled `Regenerated(BotRestart)` content changes: rewrite `### Subagents` in `OPERATING_INSTRUCTIONS.md`, and add a new built-in skill at `crates/right-codegen/skills/right-composio/SKILL.md` registered through `BUILTIN_SKILL_NAMES`. No runtime, no SQL, no sandbox recreation.

**Tech Stack:** Rust (edition 2024), `include_dir!` for embedded skill assets, `tempfile` + `cargo test` for codegen tests. Single workspace crate touched: `right-codegen`.

**Verification cadence:** Targeted `devenv shell -- cargo test -p right-codegen` after each implementation slice. One final `devenv shell -- cargo test --workspace` in Task 8 (mandated by AGENTS.md).

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `crates/right-codegen/skills/right-composio/SKILL.md` | **create** | Composio MCP playbook (workbench discipline + tool-selection patterns) |
| `crates/right-codegen/src/skills.rs` | modify (~3 lines) | Register `SKILL_RIGHT_COMPOSIO` const, add to `BUILTIN_SKILL_NAMES`, add match arm in `builtin_skill_dir`. Tests live in the same file's `mod tests`. |
| `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` | modify (two non-overlapping edits) | Rewrite `### Subagents` (lines ~107–118) + add `/right-composio` line under `## Core Skills` (line ~265) |
| `PROMPT_SYSTEM.md` | modify (1 line) | Append `right-composio` to the skills-list cell at line 296 (sandbox skills table) |

Nothing else changes. `right-platform-store/src/lib.rs::build_manifest` already iterates `BUILTIN_SKILL_NAMES`, so no edit there — its existing `build_manifest_deploys_all_listed_builtin_skills` test will exercise the new skill automatically.

---

### Task 1: Add failing test for right-composio skill installation

**Files:**
- Modify: `crates/right-codegen/src/skills.rs` (inside the existing `#[cfg(test)] mod tests` block, around line 200)

- [ ] **Step 1: Add the failing test**

In `crates/right-codegen/src/skills.rs`, inside `mod tests`, add (place it next to the other `installs_*` tests, e.g. right after `installs_right_learn_skill`):

```rust
#[test]
fn installs_right_composio_skill() {
    let dir = tempdir().unwrap();
    install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
    assert!(
        dir.path()
            .join(".claude/skills/right-composio/SKILL.md")
            .exists(),
        "right-composio/SKILL.md should exist"
    );
}
```

- [ ] **Step 2: Run the test, verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-codegen installs_right_composio_skill
```

Expected output: test runs, asserts, then prints `assertion failed: dir.path().join(...).exists()` (because `"right-composio"` is not yet in `BUILTIN_SKILL_NAMES`). Final line: `test result: FAILED. 0 passed; 1 failed`.

Do NOT commit yet — red is the start of the cycle, not the end.

---

### Task 2: Create stub SKILL.md and register the skill (test goes green)

**Files:**
- Create: `crates/right-codegen/skills/right-composio/SKILL.md`
- Modify: `crates/right-codegen/src/skills.rs` (three sites: const block ~14–21, `BUILTIN_SKILL_NAMES` ~30–37, `builtin_skill_dir` match ~52–66)

- [ ] **Step 1: Create a one-line stub SKILL.md so the `include_dir!` macro has something to embed**

Create `crates/right-codegen/skills/right-composio/SKILL.md` with exactly this content (stub, will be replaced in Task 4):

```markdown
---
name: right-composio
description: stub — full content lands in a later task
---

# /right-composio — stub
```

`include_dir!` runs at compile time and requires the directory to exist with at least one file. Writing the full content here would require re-editing in Task 4 to add the content test first; a stub keeps the TDD cycle clean.

- [ ] **Step 2: Add the include_dir! constant in `crates/right-codegen/src/skills.rs`**

In the const block (currently lines 14–21), add the new line after `SKILL_RIGHT_LEARN_SKILL`:

```rust
const SKILL_RIGHT_COMPOSIO: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/right-composio");
```

- [ ] **Step 3: Add `"right-composio"` to `BUILTIN_SKILL_NAMES`**

In `BUILTIN_SKILL_NAMES` (currently lines 30–37), append `"right-composio"` as the last entry:

```rust
pub const BUILTIN_SKILL_NAMES: &[&str] = &[
    "right-skills",
    "right-cron",
    "right-mcp",
    "right-learn-skill",
    "right-memory",
    "right-reflect",
    "right-composio",
];
```

- [ ] **Step 4: Add the match arm in `builtin_skill_dir`**

In `builtin_skill_dir` (currently lines 48–66), add the arm after `"right-reflect"`:

```rust
"right-reflect" => Ok(&SKILL_RIGHT_REFLECT),
"right-composio" => Ok(&SKILL_RIGHT_COMPOSIO),
```

- [ ] **Step 5: Run the test, verify it now passes**

Run:

```bash
devenv shell -- cargo test -p right-codegen installs_right_composio_skill
```

Expected output: `test result: ok. 1 passed; 0 failed`.

- [ ] **Step 6: Run the package suite to confirm no regressions in other skill tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen
```

Expected: all tests pass (including `build_manifest_deploys_all_listed_builtin_skills` if it lives here — otherwise see Task 8 workspace check). No new failures.

- [ ] **Step 7: Commit**

```bash
git add crates/right-codegen/skills/right-composio/SKILL.md crates/right-codegen/src/skills.rs
git commit -m "feat(codegen): register right-composio built-in skill (stub)"
```

---

### Task 3: Add failing content-assertion test

**Files:**
- Modify: `crates/right-codegen/src/skills.rs` (same `mod tests` block, next to the `installs_right_composio_skill` test from Task 1)

- [ ] **Step 1: Add a test that asserts on the SKILL.md content**

Place this test directly under `installs_right_composio_skill` in `mod tests`:

```rust
#[test]
fn right_composio_skill_documents_workbench_discipline() {
    let dir = tempdir().unwrap();
    install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
    let content = std::fs::read_to_string(
        dir.path().join(".claude/skills/right-composio/SKILL.md"),
    )
    .unwrap();
    // Frontmatter must declare the skill name CC's selector matches on.
    assert!(
        content.contains("name: right-composio"),
        "SKILL.md must declare name: right-composio in frontmatter"
    );
    // Workbench discipline is the load-bearing reason the skill exists.
    assert!(
        content.contains("sync_response_to_workbench"),
        "SKILL.md must document sync_response_to_workbench"
    );
    assert!(
        content.contains("COMPOSIO_MULTI_EXECUTE_TOOL"),
        "SKILL.md must reference the MULTI_EXECUTE tool by name"
    );
    // Auth pitfall must defer to the main MCP Error Diagnosis section,
    // not duplicate /mcp auth advice (per the 2026-05-06 spec).
    assert!(
        content.contains("Do NOT suggest `/mcp auth composio`"),
        "SKILL.md must steer the agent away from suggesting /mcp auth composio"
    );
}
```

- [ ] **Step 2: Run the test, verify it fails on every assertion**

Run:

```bash
devenv shell -- cargo test -p right-codegen right_composio_skill_documents_workbench_discipline
```

Expected: test runs, fails on the first content assertion (the stub doesn't contain `sync_response_to_workbench` etc.). Final line: `test result: FAILED. 0 passed; 1 failed`.

Do NOT commit yet — paired with Task 4 below.

---

### Task 4: Replace stub SKILL.md with full content (test goes green)

**Files:**
- Modify: `crates/right-codegen/skills/right-composio/SKILL.md` (full rewrite)

- [ ] **Step 1: Replace the stub SKILL.md with the full playbook**

Overwrite `crates/right-codegen/skills/right-composio/SKILL.md` with this exact content:

````markdown
---
name: right-composio
description: >-
  Use when the user's request maps to a Composio-fronted service
  (Notion, Gmail, Calendar, Slack, GitHub, etc.) and you're about to
  call mcp__right__composio__*. Covers workbench-vs-context discipline,
  MULTI_EXECUTE batching, and search_tools discovery. Activate ONLY
  when composio is in your MCP list.
---

# /right-composio — Composio MCP playbook

Composio is a gateway: one MCP server fronts 250+ external services
(Notion, Gmail, Calendar, Slack, GitHub, ...). Tool surface is narrow
(~7 meta-tools) but responses can be huge. Two biggest context risks:
dumping list/search/fetch payloads into context, and looping single
tool calls when one MULTI_EXECUTE would do.

## When to Activate

- The user's request maps to a Composio-fronted service.
- You're about to invoke `mcp__right__composio__*` and need to decide:
  workbench yes/no, MULTI_EXECUTE vs single, search_tools first?
- If composio is not in `mcp__right__mcp_list`, this skill does not
  apply — ask the user to `/mcp add composio <url>`.

## Workbench discipline

`mcp__right__composio__COMPOSIO_MULTI_EXECUTE_TOOL` has a
`sync_response_to_workbench` field. `true` → response stored in
Composio's remote workbench, you get a reference. `false` (default)
→ full payload lands in your context.

**`sync_response_to_workbench: true` when:**
- Tool slug contains `_LIST_`, `_SEARCH_`, `_FETCH_`, `_GET_ALL`,
  `_PAGES`, `_THREADS` (collections).
- Batching 2+ tools in one MULTI_EXECUTE call.
- Expecting prose bodies (email content, Notion page text).
- Follow-up MULTI_EXECUTE will act on the result — pass the
  workbench reference via `session_id`.

**`sync_response_to_workbench: false` (or omit) when:**
- Single write/update returning only an id or status
  (`NOTION_INSERT_ROW_DATABASE`, `GMAIL_SEND_EMAIL`,
  `CALENDAR_CREATE_EVENT`).
- Single read of one known record where the body IS the user's
  answer (`NOTION_FETCH_PAGE` by id when the user asked "what's on
  that page").
- Next step in this turn branches on the result AND the result
  is small.

When in doubt: workbench on. Pull with
`mcp__right__composio__COMPOSIO_REMOTE_WORKBENCH` later.

## Tool-selection patterns

- **Unknown toolkit slug?** Always
  `mcp__right__composio__COMPOSIO_SEARCH_TOOLS` first. Don't guess —
  slugs change.
- **Multiple ops on same toolkit?** One MULTI_EXECUTE with a `tools`
  array beats N separate calls.
- **Non-trivial query/transform on a result?**
  `mcp__right__composio__COMPOSIO_REMOTE_BASH_TOOL` on workbench
  data beats pulling-and-parsing in context.

## Pitfalls

- **`input` vs `arguments`:** per-tool args go under `arguments`, not
  `input`. "Required at" / "missing fields" errors = your fault.
- **Connection errors:** `has_active_connection: false` is a
  toolkit-level Composio↔external auth, not MCP-transport auth.
  Call `mcp__right__composio__COMPOSIO_MANAGE_CONNECTIONS` as the
  upstream tells you. Do NOT suggest `/mcp auth composio`. (See
  "MCP Error Diagnosis → Trust upstream diagnostics" in your main
  prompt.)
````

- [ ] **Step 2: Run both right-composio tests, verify green**

Run:

```bash
devenv shell -- cargo test -p right-codegen right_composio
```

Expected: both `installs_right_composio_skill` and `right_composio_skill_documents_workbench_discipline` pass. Final line: `test result: ok. 2 passed; 0 failed`.

- [ ] **Step 3: Commit**

```bash
git add crates/right-codegen/skills/right-composio/SKILL.md crates/right-codegen/src/skills.rs
git commit -m "feat(codegen): right-composio playbook content"
```

---

### Task 5: Rewrite `### Subagents` in OPERATING_INSTRUCTIONS.md

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md:107-118`

- [ ] **Step 1: Replace the existing `### Subagents` block**

The current block is exactly these 12 lines (107–118):

```markdown
### Subagents

For complex work, you may use the built-in Claude Code `Agent` tool to spawn a
subagent for a narrow, independent workstream. Use subagents when isolation,
parallel investigation, or fresh review will reduce main-session context load or
improve quality. Do not use subagents for quick edits, simple command output, or
work that depends tightly on the next step in the main session.

The main session remains accountable: give the subagent a bounded task, keep
sensitive decisions in the main session, review its output, resolve conflicts,
and synthesize the result for the user. Do not paste raw subagent output as the
final answer.
```

Replace those 12 lines with this block (verbatim, including the trailing blank line before `### Progress Updates`):

```markdown
### Subagents

Use the built-in Claude Code `Agent` tool when you can offload work
whose intermediate results don't need to live in your main context.
Two canonical triggers:

1. **Multi-step workflows where only the final outcome matters.**
   Researching across several sources, building a candidate list and
   picking from it, comparing options — dispatch the whole loop and
   take back only the conclusion.

2. **File or tool reads where only the verdict matters.**
   "Does this JSONL contain a specific decision?", "Find the endpoint
   URL on this docs page", "Summarize what this long Composio response
   says about X" — read in a subagent, take back the answer.

Do NOT delegate when:
- You need to see the intermediate output to decide the next step in
  the same turn.
- The task is one cheap tool call with a small response (e.g.
  `mcp__right__mcp_list`, a single `mcp__right__cron_trigger`, a
  `mcp__right__send_progress` update).
- The work is a short edit, single command, or quick verification
  whose entire output you'd read anyway.

For independent subtasks (e.g. "research these three options"),
dispatch multiple subagents in one message via parallel `Agent`
tool calls — sequential dispatch wastes time.

The main session is accountable: give the subagent a bounded prompt,
review its output, resolve conflicts with what you already know, and
synthesize for the user. Do not paste raw subagent output as the
final answer.
```

- [ ] **Step 2: Verify the file still renders (no leftover tokens, no broken Markdown)**

Run:

```bash
devenv shell -- cargo test -p right-codegen
```

Expected: no test regressions. The template is not snapshot-tested in this crate, so this is a sanity build pass, not a behavioral assert. The skill tests from Task 4 must still pass.

- [ ] **Step 3: Commit**

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "feat(prompt): tighten subagent rule around intermediate-result relevance"
```

---

### Task 6: Add `/right-composio` to `## Core Skills` in OPERATING_INSTRUCTIONS.md

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` (the `## Core Skills` section, around lines 265–268)

- [ ] **Step 1: Insert the new bullet after the `/right-reflect` line**

Current Core Skills block (around lines 265–268):

```markdown
## Core Skills

- `/right-reflect` — read your own past sessions when the user asks "why did you ...?". Reads CC's project JSONL inside the sandbox. No MCP calls, no DB.

<!-- Add additional skills here. Example: -->
<!-- - `/my-skill` -- description of what it does -->
```

Replace with:

```markdown
## Core Skills

- `/right-reflect` — read your own past sessions when the user asks "why did you ...?". Reads CC's project JSONL inside the sandbox. No MCP calls, no DB.
- `/right-composio` — playbook for Composio MCP. Use when calling `mcp__right__composio__*` and composio is in your MCP list.

<!-- Add additional skills here. Example: -->
<!-- - `/my-skill` -- description of what it does -->
```

- [ ] **Step 2: Commit**

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "feat(prompt): advertise /right-composio under Core Skills"
```

---

### Task 7: Update PROMPT_SYSTEM.md skills inventory

**Files:**
- Modify: `PROMPT_SYSTEM.md:296`

- [ ] **Step 1: Append `right-composio` to the skills-list cell**

Current line 296:

```markdown
| skills/ | `/sandbox/.claude/skills/{right-skills,right-cron,right-mcp,right-learn-skill,right-memory,right-reflect}` → `/platform/skills/<name>.<hash>` | Platform (symlink) |
```

Replace with:

```markdown
| skills/ | `/sandbox/.claude/skills/{right-skills,right-cron,right-mcp,right-learn-skill,right-memory,right-reflect,right-composio}` → `/platform/skills/<name>.<hash>` | Platform (symlink) |
```

- [ ] **Step 2: Commit**

```bash
git add PROMPT_SYSTEM.md
git commit -m "docs(prompt): list right-composio in the sandbox skills table"
```

---

### Task 8: Final workspace test

**Files:** none modified — verification only.

- [ ] **Step 1: Run the full workspace test suite**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: all tests pass, including:
- `right-codegen::skills::tests::installs_right_composio_skill`
- `right-codegen::skills::tests::right_composio_skill_documents_workbench_discipline`
- `right_platform_store::platform_store_tests::build_manifest_deploys_all_listed_builtin_skills` (already iterates `BUILTIN_SKILL_NAMES`; this verifies the sandbox deployer picks up `right-composio` automatically with no code change)

If any test fails that pre-existed on the branch (per AGENTS.md: "Record any pre-existing failures"), document but do not fix in this plan — surface to the user for triage.

- [ ] **Step 2: Confirm the post-merge upgrade story locally (optional, operator-side)**

This step is operator verification, not part of the automated suite. On a machine with running agents:

```bash
right restart agent-b && right restart right
ls ~/.right/agents/agent-b/.claude/skills/right-composio/
ls ~/.right/agents/right/.claude/skills/right-composio/
```

Expected: each lists `SKILL.md`. Stops here — full behavioral verification (workbench parameter rate, subagent over-/under-delegation) is the post-merge manual probe described in the spec under "Manual verification", not part of this plan.

- [ ] **Step 3: Final commit gate**

There is nothing to commit in this task. The branch is ready for `right restart` rollout once Tasks 1–7 are merged.

---

## Self-Review

**1. Spec coverage:**

| Spec section | Task |
|---|---|
| A. Subagent rule rewrite | Task 5 (full replacement text verbatim from spec) |
| B. `right-composio` SKILL.md content | Task 4 (full content verbatim from spec) |
| B. `skills.rs` registration (const + name + match arm) | Task 2 |
| B. `OPERATING_INSTRUCTIONS.md` Core Skills bullet | Task 6 |
| B. `PROMPT_SYSTEM.md` skill list update | Task 7 |
| Testing: `right_composio_in_builtin_skill_names` / `right_composio_resolves_to_dir` (spec names) | Task 1 + Task 3 (renamed to `installs_right_composio_skill` and `right_composio_skill_documents_workbench_discipline` — same coverage via the existing `installs_*` test pattern in this file; spec test names were illustrative) |
| `build_manifest_deploys_all_listed_builtin_skills` auto-coverage | Task 8 (verifies it passes) |
| Final workspace test (AGENTS.md mandate) | Task 8 |

No spec section is unaddressed.

**2. Placeholder scan:** No `TBD` / `TODO` / "implement later" / "add appropriate" left. All edits show the exact code or text to write.

**3. Type / name consistency:**
- `SKILL_RIGHT_COMPOSIO` const name used identically across Task 2 Steps 2 and 4.
- `right-composio` string identical across `BUILTIN_SKILL_NAMES`, match arm, SKILL.md frontmatter, test paths, OPERATING_INSTRUCTIONS Core Skills bullet, PROMPT_SYSTEM.md table.
- Test function names referenced by name in Task 4 Step 2 (`installs_right_composio_skill`, `right_composio_skill_documents_workbench_discipline`) match the definitions in Task 1 Step 1 and Task 3 Step 1.
- All MCP tool references use the full `mcp__right__composio__<TOOL>` form per AGENTS.md.

**4. Scope check:** Plan stays inside the spec's Non-goals (no custom subagent types, no IDENTITY/SOUL edits, no token thresholds, no general payload-hygiene skill, no conditional rendering).
