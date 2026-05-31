# Base prompt vs OPERATING_INSTRUCTIONS boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the single verbatim duplication between the base prompt and OPERATING_INSTRUCTIONS, and document a boundary invariant so it cannot drift again.

**Architecture:** The composite system prompt = base prompt (`generate_system_prompt`, a parameterized Rust `format!`) + OPERATING_INSTRUCTIONS (static `include_str!` markdown). The base prompt is included in every mode including Bootstrap; OPERATING is included only in Normal/Cron. The "remember → `/right-memory`" rule currently sits in both; it is operating-only, so it is removed from the base prompt. A pinning test that asserts the opposite invariant is refocused.

**Tech Stack:** Rust (edition 2024), `cargo test`, the `right-codegen` crate. Run commands via `devenv shell -- …`.

Spec: `docs/superpowers/specs/2026-06-01-prompt-base-operating-boundary-design.md`

---

## File Structure

- `crates/right-codegen/src/agent_def.rs` — base prompt generator. **Modify:** remove the remember-routing paragraph from the `## Identity Files` block of the `format!` string.
- `crates/right-codegen/src/agent_def_tests.rs` — unit tests. **Modify:** refocus the pinning test to encode the new invariant (base must NOT carry `/right-memory`; identity framing stays).
- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` — **no change**; it already carries the canonical remember rule (`## Your Files`). Verified-only.
- `PROMPT_SYSTEM.md` — **modify:** add the boundary invariant under `## Base Prompt`.
- `AGENTS.md` — **modify:** cross-reference the invariant from the "Prompt-tier brevity" bullet.

---

### Task 1: Refocus the pinning test to encode the invariant (TDD red)

**Files:**
- Modify: `crates/right-codegen/src/agent_def_tests.rs` (the `system_prompt_delegates_remember_routing_to_right_memory_skill` test, ~lines 118-147)

- [ ] **Step 1: Confirm no other test pins the removed paragraph**

Run:
```bash
devenv shell -- rg -n "persistence intent|choose the persistence target|remember.*right-memory|/right-memory" crates/right-codegen/src/agent_def_tests.rs
```
Expected: only the `system_prompt_delegates_remember_routing_to_right_memory_skill` test (~line 130) references `/right-memory` against the **base** prompt, and `operating_instructions_keep_soul_agent_authored_and_delegate_remember_routing` (~line 250) references it against **OPERATING**. No test asserts the literal sentence "persistence intent" / "choose the persistence target" against the base prompt. If any other base-prompt test pins the removed sentence, update it the same way as Step 2.

- [ ] **Step 2: Rewrite the test to assert the new invariant**

Replace the entire `system_prompt_delegates_remember_routing_to_right_memory_skill` function with:

```rust
#[test]
fn system_prompt_keeps_identity_framing_without_remember_routing() {
    let result = generate_system_prompt(
        "test",
        &right_agent_config::SandboxMode::Openshell,
        "/sandbox",
    );

    for needle in [
        "Identity files are always-loaded durable context",
        "`SOUL.md`",
        "agent-authored durable voice",
    ] {
        assert!(
            result.contains(needle),
            "base prompt must preserve identity-file framing: missing {needle:?}"
        );
    }

    for forbidden in [
        concat!("compact ", "operating contract"),
        "\"Remember\" requests are routed by semantic type before storage. Tool/API/env rules go to",
        // remember -> /right-memory routing is operating-only; it must NOT live
        // in the base prompt, because Bootstrap mode omits OPERATING_INSTRUCTIONS.
        "/right-memory",
    ] {
        assert!(
            !result.contains(forbidden),
            "base prompt must not carry operating-only routing or prescribe SOUL defaults: found {forbidden:?}"
        );
    }
}
```

- [ ] **Step 3: Run the test to verify it fails (red)**

Run:
```bash
devenv shell -- cargo test -p right-codegen system_prompt_keeps_identity_framing_without_remember_routing
```
Expected: FAIL — assertion message `base prompt must not carry operating-only routing or prescribe SOUL defaults: found "/right-memory"` (the base prompt still contains the paragraph).

---

### Task 2: Remove the remember-routing paragraph from the base prompt (TDD green)

**Files:**
- Modify: `crates/right-codegen/src/agent_def.rs` (the `## Identity Files` block inside `generate_system_prompt`, ~lines 220-229)

- [ ] **Step 1: Delete the duplicated paragraph**

In the `format!` string, find:

```text
- `TOOLS.md` stores durable tool, API, environment, and workflow constraints.

When the user says \"remember\", \"save this\", or \"don't forget\", treat it as persistence intent. Use the `/right-memory` skill to choose the persistence target before editing identity files or calling memory tools.

## Response Rules
```

Replace with (delete the middle paragraph and its surrounding blank line):

```text
- `TOOLS.md` stores durable tool, API, environment, and workflow constraints.

## Response Rules
```

Leave every other line of the function unchanged.

- [ ] **Step 2: Run the refocused test to verify it passes (green)**

Run:
```bash
devenv shell -- cargo test -p right-codegen system_prompt_keeps_identity_framing_without_remember_routing
```
Expected: PASS.

- [ ] **Step 3: Run the OPERATING carrier test to confirm the rule still has a home**

Run:
```bash
devenv shell -- cargo test -p right-codegen operating_instructions_keep_soul_agent_authored_and_delegate_remember_routing
```
Expected: PASS — OPERATING_INSTRUCTIONS remains the sole carrier of the remember→`/right-memory` routing.

- [ ] **Step 4: Run the full crate test suite**

Run:
```bash
devenv shell -- cargo test -p right-codegen
```
Expected: PASS — no other test pinned the removed paragraph.

- [ ] **Step 5: Commit**

```bash
git add crates/right-codegen/src/agent_def.rs crates/right-codegen/src/agent_def_tests.rs
git commit -m "refactor(prompt): drop duplicated remember-routing from base prompt

The remember -> /right-memory rule lived in both the base prompt and
OPERATING_INSTRUCTIONS. It is operating-only (Bootstrap omits OPERATING),
so it now lives only in OPERATING_INSTRUCTIONS. Refocus the pinning test
to assert the base prompt keeps identity framing but not the routing."
```

---

### Task 3: Document the boundary invariant

**Files:**
- Modify: `PROMPT_SYSTEM.md` (under the `## Base Prompt` heading)
- Modify: `AGENTS.md` (the "Prompt-tier brevity" bullet under "## Conventions")

- [ ] **Step 1: Add the invariant to PROMPT_SYSTEM.md**

Locate the `## Base Prompt` section (the line `## Base Prompt`, followed by `Generated by \`generate_system_prompt()\`…`). Immediately after that section's existing paragraph (the "Content: agent name, …" line), insert:

```markdown

### Boundary invariant: base prompt vs OPERATING_INSTRUCTIONS

The base prompt carries exactly two kinds of content: (1) values it
interpolates or branches on (`agent_name`, `sandbox_mode`, `home_dir`); and
(2) the universal minimum every mode needs — **including Bootstrap, which omits
OPERATING_INSTRUCTIONS** — i.e. the platform description, MCP reference, Response
Rules, and the *purpose* list of the identity files. All static operating
procedure for Normal/Cron turns (identity-file edit discipline, the
remember→`/right-memory` routing, MCP management, cron, attachments, formatting,
etc.) lives only in OPERATING_INSTRUCTIONS. No rule appears in both sections.

Tie-breaker when allocating a new rule: *does Bootstrap mode need it?* Yes → base
prompt. No → OPERATING_INSTRUCTIONS.
```

- [ ] **Step 2: Cross-reference from AGENTS.md**

In `AGENTS.md`, under `## Conventions`, find the bullet beginning `- **Prompt-tier brevity**:`. Append this sentence to the end of that bullet's text (before the next bullet):

```markdown
 The base prompt (`generate_system_prompt`) and `OPERATING_INSTRUCTIONS.md` must not duplicate rules; their split follows the boundary invariant in `PROMPT_SYSTEM.md` (base = parameterized values + the Bootstrap-universal minimum; OPERATING = operating-only procedure).
```

- [ ] **Step 3: Sanity-check the docs render**

Run:
```bash
devenv shell -- rg -n "Boundary invariant: base prompt vs OPERATING_INSTRUCTIONS" PROMPT_SYSTEM.md
devenv shell -- rg -n "boundary invariant in .PROMPT_SYSTEM.md." AGENTS.md
```
Expected: one match each.

- [ ] **Step 4: Commit**

```bash
git add PROMPT_SYSTEM.md AGENTS.md
git commit -m "docs(prompt): record base-vs-OPERATING boundary invariant"
```

---

### Task 4: Final workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full workspace test suite**

Run:
```bash
devenv shell -- cargo test --workspace
```
Expected: PASS. Record any pre-existing unrelated failures; the change touches only `right-codegen` prompt text and its tests.

- [ ] **Step 2: Confirm the tree is clean and committed**

Run:
```bash
git status --short
```
Expected: empty (all changes committed in Tasks 2 and 3).

---

## Self-Review

- **Spec coverage:**
  - Invariant documented → Task 3 (PROMPT_SYSTEM.md + AGENTS.md). ✓
  - Remove remember-routing paragraph from base → Task 2. ✓
  - Keep identity-file purpose list in base → preserved (Task 2 deletes only the one paragraph; test Task 1 asserts the framing needles remain). ✓
  - Refocus the pinning test → Task 1. ✓
  - OPERATING remains sole carrier → Task 2 Step 3 verifies the existing OPERATING test. ✓
  - Non-goals (don't merge mechanisms, don't move Response Rules/MCP ref, don't touch other OPERATING sections) → respected; no task does these. ✓
- **Placeholder scan:** no TBD/TODO; every code/doc step shows the exact content. ✓
- **Type consistency:** the renamed test `system_prompt_keeps_identity_framing_without_remember_routing` is self-contained; `generate_system_prompt` signature unchanged. ✓
