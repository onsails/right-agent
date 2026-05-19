# Agent Tool Subagents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Teach the main Right Agent Claude Code session that it may use the built-in `Agent` tool for bounded subagent delegation, without creating first-class subagent definitions or reviving per-agent `AGENTS.md`.

**Architecture:** This is a prompt/documentation-only behavior change plus one regression test. The compiled `OPERATING_INSTRUCTIONS.md` is the runtime source shown to agents; codegen tests pin the required guidance. Runtime invocation logic already allows `Agent` in the relevant callsites, so no CLI, schema, config, or sandbox code changes are needed.

**Tech Stack:** Rust 2024, `right-codegen` prompt constants via `include_str!`, markdown prompt templates, `cargo test`, `devenv shell --`.

---

## File Structure

- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
  - Responsibility: agent-facing operating rules. Add the only runtime instruction about using the built-in Claude Code `Agent` tool.
- Modify: `crates/right-codegen/src/agent_def_tests.rs`
  - Responsibility: tests for compiled prompt constants. Add one narrow regression test for the subagent delegation guidance.
- Modify: `PROMPT_SYSTEM.md`
  - Responsibility: prescriptive description of prompt assembly. Keep it synced with the new operating-instructions section without mentioning old per-agent `AGENTS.md`.
- Modify: `docs/architecture/sessions.md`
  - Responsibility: callsite/tool-allowance architecture. Clarify that allowed subagents are spawned through the built-in `Agent` tool, not generated specialist definitions.
- Do not modify: `crates/right-agent/src/init.rs`
  - Reason: this change must not create `AGENTS.md` or `.claude/agents/*.md`.
- Do not modify: `crates/bot/src/cc/invocation.rs`
  - Reason: `Agent` is already excluded from the baseline denied tools and is callsite-controlled.

## Starting-State Cleanup

There may be draft edits from an interrupted previous attempt. Keep only the hunks that match this plan. In particular, remove any new prose that says per-agent `AGENTS.md` was "uprazdnen"/"uprazднен"/"deprecated"/"does not generate per-agent `AGENTS.md`" from runtime docs; old internal history should not become a user-visible concept.

### Task 1: Add Regression Test

**Files:**
- Modify: `crates/right-codegen/src/agent_def_tests.rs`

- [ ] **Step 1: Add the failing test**

In `crates/right-codegen/src/agent_def_tests.rs`, insert this test immediately after `operating_instructions_teach_sparse_progress_updates`:

```rust
#[test]
fn operating_instructions_teach_agent_tool_delegation() {
    let ops = crate::OPERATING_INSTRUCTIONS;

    for needle in [
        "`Agent` tool",
        "independent workstream",
        "main session remains accountable",
        "synthesize the result",
    ] {
        assert!(
            ops.contains(needle),
            "OPERATING_INSTRUCTIONS must teach Agent-tool delegation: missing {needle:?}"
        );
    }
}
```

- [ ] **Step 2: Run the targeted test and verify RED**

Run:

```bash
devenv shell -- cargo test -p right-codegen operating_instructions_teach_agent_tool_delegation
```

Expected: FAIL with a message like:

```text
OPERATING_INSTRUCTIONS must teach Agent-tool delegation: missing "`Agent` tool"
```

If it passes before editing `OPERATING_INSTRUCTIONS.md`, the test is not proving the missing behavior; inspect current draft edits before continuing.

### Task 2: Teach Agent Tool Delegation

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`

- [ ] **Step 1: Add the Subagents section**

In `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`, insert this section after the "Communication" opening paragraph and before `### Progress Updates`:

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

- [ ] **Step 2: Run the targeted test and verify GREEN**

Run:

```bash
devenv shell -- cargo test -p right-codegen operating_instructions_teach_agent_tool_delegation
```

Expected: PASS:

```text
test agent_def::tests::operating_instructions_teach_agent_tool_delegation ... ok
```

### Task 3: Sync Prompt Documentation Without Reviving AGENTS.md

**Files:**
- Modify: `PROMPT_SYSTEM.md`
- Modify: `docs/architecture/sessions.md`

- [ ] **Step 1: Update prompt-system description**

In `PROMPT_SYSTEM.md`, after the paragraph that starts with `Missing agent-owned files are silently skipped`, add only this text:

```markdown
Operating instructions include a `### Subagents` section that teaches use of the
built-in Claude Code `Agent` tool for bounded independent workstreams. This is
prompt guidance only; Right Agent does not create separate subagent definition
files.
```

Do not add any text about per-agent `AGENTS.md`. It is gone from this runtime path and should stay invisible.

- [ ] **Step 2: Update session architecture**

In `docs/architecture/sessions.md`, immediately after:

```markdown
The baseline lives in `crates/bot/src/cc/invocation.rs::BASELINE_DISALLOWED_TOOLS`
and explicitly excludes `Agent`.
```

add:

```markdown
Right Agent does not add custom subagent definition files; when allowed,
subagents are spawned by the main Claude Code session through the built-in
`Agent` tool.
```

- [ ] **Step 3: Remove stale per-agent AGENTS.md prose from the touched files**

Run:

```bash
devenv shell -- rg -n 'per-agent `AGENTS\.md`|agents/<name>/AGENTS\.md|Agent Configuration' PROMPT_SYSTEM.md crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md docs/architecture/sessions.md
```

Expected: no output.

If there is output, remove only the stale per-agent `AGENTS.md` text. Do not remove the repository-root `AGENTS.md` cite-on-touch notes in `docs/architecture/*.md`; those are development instructions, not agent runtime config.

### Task 4: Focused Prompt Test Pass

**Files:**
- Test only

- [ ] **Step 1: Run focused right-codegen prompt tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen operating_instructions
```

Expected: PASS. This filter should include the new delegation test and existing operating-instruction tests.

- [ ] **Step 2: Inspect changed files**

Run:

```bash
devenv shell -- git diff -- PROMPT_SYSTEM.md crates/right-codegen/src/agent_def_tests.rs crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md docs/architecture/sessions.md
```

Expected:
- `OPERATING_INSTRUCTIONS.md` adds only the `### Subagents` section.
- `agent_def_tests.rs` adds only `operating_instructions_teach_agent_tool_delegation`.
- `PROMPT_SYSTEM.md` mentions the new `### Subagents` section and does not mention per-agent `AGENTS.md`.
- `docs/architecture/sessions.md` clarifies `Agent` tool spawning and does not introduce generated subagent definitions.

### Task 5: Final Verification

**Files:**
- Test only

- [ ] **Step 1: Run the full workspace test suite**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. Existing ignored tests may remain ignored.

- [ ] **Step 2: Run the final debug build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: PASS.

### Task 6: Commit Implementation

**Files:**
- Commit: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
- Commit: `crates/right-codegen/src/agent_def_tests.rs`
- Commit: `PROMPT_SYSTEM.md`
- Commit: `docs/architecture/sessions.md`

- [ ] **Step 1: Review status**

Run:

```bash
devenv shell -- git status --short
```

Expected: only the four implementation files above are modified, plus this plan file if it has not already been committed separately.

- [ ] **Step 2: Commit implementation**

Run:

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md \
        crates/right-codegen/src/agent_def_tests.rs \
        PROMPT_SYSTEM.md \
        docs/architecture/sessions.md
git commit -m "feat(prompt): teach agent tool delegation"
```

Expected: commit succeeds with only the implementation files staged.

## Self-Review

- Spec coverage: Covers the requested behavior: teach the main agent it can spawn subagents, without creating first-class agents, and remove/avoid stale per-agent `AGENTS.md` concepts.
- Placeholder scan: No placeholder markers, "similar to", or unspecified test steps remain.
- Type/name consistency: The test needles match the exact prompt text in Task 2. File paths match the current repository layout.
