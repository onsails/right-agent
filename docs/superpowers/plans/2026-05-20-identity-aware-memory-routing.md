# Identity-Aware Memory Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep identity-aware persistence routing while removing all platform-authored `SOUL.md` content.

**Architecture:** `/right-memory` is the single detailed router for explicit persistence requests. System prompt surfaces explain identity-file ownership and direct agents to `/right-memory`, while memory tool/MCP descriptions only say `mcp__right__memory_retain` is residual fallback after routing.

**Tech Stack:** Rust 2024, generated prompt templates, Markdown docs, MCP tool metadata, `devenv shell -- cargo`.

---

## File Structure

- Modify `crates/right-codegen/src/agent_def_tests.rs`: replace operating-contract tests with ownership and `/right-memory` delegation tests.
- Modify `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`: explain identity files without duplicating the detailed router.
- Modify `crates/right-codegen/templates/right/agent/BOOTSTRAP.md`: remove required operating-contract content from `SOUL.md`.
- Modify `crates/right-agent/src/identity_mirror.rs`: remove managed `SOUL.md` migration helper and its tests.
- Modify `crates/right/src/aggregator.rs`: make `memory_retain` schema residual and `/right-memory`-aware without listing target files.
- Modify `crates/right/src/memory_server.rs`: make MCP instructions delegate routing to `/right-memory`.
- Modify `crates/right/src/memory_server_mcp_tests.rs`: update MCP instruction tests for delegation.
- Modify `PROMPT_SYSTEM.md`: keep docs synchronized with generated prompt behavior.
- Modify `docs/architecture/memory.md` and `docs/architecture/lifecycle.md`: remove platform-authored SOUL contract language and document ownership.
- Update PR #71 body after code lands.

---

### Task 1: Add Red Tests For Prompt Ownership And Delegation

**Files:**
- Modify: `crates/right-codegen/src/agent_def_tests.rs`

- [ ] **Step 1: Replace the operating-instructions SOUL test**

In `crates/right-codegen/src/agent_def_tests.rs`, replace `operating_instructions_describe_soul_as_operating_contract` with:

```rust
#[test]
fn operating_instructions_keep_soul_agent_authored_and_delegate_remember_routing() {
    let ops = crate::OPERATING_INSTRUCTIONS;
    for needle in [
        "`SOUL.md`",
        "agent-authored durable voice",
        "Do not invent platform-default content for this file",
        "Use the `/right-memory` skill to classify the correct persistence target",
        "smallest accurate edit",
    ] {
        assert!(
            ops.contains(needle),
            "OPERATING_INSTRUCTIONS must describe ownership-safe SOUL routing: missing {needle:?}"
        );
    }

    for forbidden in [
        concat!("compact ", "operating contract"),
        "Edit the always-loaded file when the fact belongs in one",
        "`TOOLS.md` for tool/API/environment rules",
        "`USER.md` for user profile",
        "`SOUL.md` for your voice",
    ] {
        assert!(
            !ops.contains(forbidden),
            "OPERATING_INSTRUCTIONS must not duplicate detailed routing or platform SOUL defaults: found {forbidden:?}"
        );
    }
}
```

- [ ] **Step 2: Replace the bootstrap operating-contract test**

In the same file, replace `bootstrap_instructions_generate_soul_operating_contract` with:

```rust
#[test]
fn bootstrap_instructions_do_not_invent_platform_soul_contract() {
    let bootstrap = crate::BOOTSTRAP_INSTRUCTIONS;
    for needle in [
        "based on the user's bootstrap choices",
        "If the user gave no signal, keep it minimal",
        "do not invent a platform-default operating contract",
    ] {
        assert!(
            bootstrap.contains(needle),
            "BOOTSTRAP_INSTRUCTIONS must keep SOUL user/agent-authored: missing {needle:?}"
        );
    }

    for forbidden in [
        "**Operating Contract**",
        "act on reversible low-risk work",
        "credential/security, or private-data actions",
        "usable outcomes over polished artifacts",
    ] {
        assert!(
            !bootstrap.contains(forbidden),
            "BOOTSTRAP_INSTRUCTIONS must not prescribe platform SOUL content: found {forbidden:?}"
        );
    }
}
```

- [ ] **Step 3: Add a generated system prompt regression test**

Add this test after `system_prompt_mentions_right_mcp`:

```rust
#[test]
fn system_prompt_delegates_remember_routing_to_right_memory_skill() {
    let result = generate_system_prompt(
        "test",
        &right_agent_config::SandboxMode::Openshell,
        "/sandbox",
    );

    for needle in [
        "Identity files are always-loaded durable context",
        "`SOUL.md`",
        "agent-authored durable voice",
        "/right-memory",
    ] {
        assert!(
            result.contains(needle),
            "system prompt must preserve identity ownership and right-memory delegation: missing {needle:?}"
        );
    }

    for forbidden in [
        concat!("compact ", "operating contract"),
        "\"Remember\" requests are routed by semantic type before storage. Tool/API/env rules go to",
    ] {
        assert!(
            !result.contains(forbidden),
            "system prompt must not duplicate detailed routing or prescribe SOUL defaults: found {forbidden:?}"
        );
    }
}
```

- [ ] **Step 4: Run red tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen operating_instructions_keep_soul_agent_authored_and_delegate_remember_routing
devenv shell -- cargo test -p right-codegen bootstrap_instructions_do_not_invent_platform_soul_contract
devenv shell -- cargo test -p right-codegen system_prompt_delegates_remember_routing_to_right_memory_skill
```

Expected: all three tests fail before prompt/template changes because current text still prescribes a platform-authored SOUL contract, includes `**Operating Contract**`, and duplicates the detailed routing table.

- [ ] **Step 5: Commit red tests**

Do not commit if the tests unexpectedly pass. Otherwise commit:

```bash
git add crates/right-codegen/src/agent_def_tests.rs
git commit -m "test(memory): require identity ownership routing"
```

---

### Task 2: Rewrite Prompt Templates To Delegate Detailed Routing

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
- Modify: `crates/right-codegen/templates/right/agent/BOOTSTRAP.md`

- [ ] **Step 1: Replace the Your Files section in operating instructions**

In `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`, keep the opening paragraph, then replace the `IDENTITY.md` / `SOUL.md` / `USER.md` / `TOOLS.md` bullets and `Where things go` table through the explicit remember routing paragraph with:

```markdown
Identity files are always-loaded durable context:

- `IDENTITY.md` - your identity and rarely-changing core facts.
- `SOUL.md` - agent-authored durable voice, values, interaction style, and
  behavioral boundaries established by bootstrap or user intent. Do not invent
  platform-default content for this file.
- `USER.md` - stable facts about the user (name, preferences, timezone,
  expertise, recurring interests). Update when you discover something durable;
  never interview - pick up signals naturally through conversation.
- `TOOLS.md` - durable tool, API, environment, and workflow constraints:
  tool-selection rules, integration quirks and gotchas, credentials/setup
  notes, environment paths, and API-shape corrections after validation errors.

Edit identity files only when the user asks to persist something, bootstrap
establishes it, or the existing conversation makes the durable update explicit.
Preserve existing user/agent-authored content and make the smallest accurate
edit.

When the user says "remember", "save this", or "don't forget", treat it as an
intent to persist. Use the `/right-memory` skill to classify the correct
persistence target before editing files or calling memory tools.
```

- [ ] **Step 2: Replace memory section guidance**

In the same file, replace the first paragraph under `## Memory` with:

```markdown
Your memory skill (`/right-memory`) defines how memory works in your setup and
is the detailed router for persistence requests. Consult it before storing
explicit "remember", "save this", or "don't forget" requests.
```

Do not keep the `Use memory for facts that don't have a home in the files above:`
or `Do NOT save to memory:` lists in operating instructions. `/right-memory`
owns that detailed routing. Keep only the Hindsight fallback phrasing and the
`/right-learn-skill` paragraph; do not mention `MEMORY.md` in always-loaded
operating instructions because no-memory prompts must not include it.

- [ ] **Step 3: Replace the bootstrap SOUL.md section**

In `crates/right-codegen/templates/right/agent/BOOTSTRAP.md`, replace the `### SOUL.md` subsection with:

```markdown
### SOUL.md

Personality based only on chosen vibe and explicit bootstrap signals. Suggested headings when there is evidence:

- **Tone & Style**: concrete tone, verbosity, formality, emoji, or language preferences the user chose or clearly implied
- **Personality**: bullet list of behavioral traits that follow from the chosen vibe and user signals
- **Boundaries**: only durable behavioral boundaries the user explicitly requested or clearly established during bootstrap

If the user gave no signal for a section, omit it or keep it minimal. Do not invent a platform-default operating contract.
```

- [ ] **Step 4: Run Task 1 tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen operating_instructions_keep_soul_agent_authored_and_delegate_remember_routing
devenv shell -- cargo test -p right-codegen bootstrap_instructions_do_not_invent_platform_soul_contract
devenv shell -- cargo test -p right-codegen system_prompt_delegates_remember_routing_to_right_memory_skill
```

Expected: PASS.

- [ ] **Step 5: Commit prompt template changes**

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md crates/right-codegen/templates/right/agent/BOOTSTRAP.md
git commit -m "fix(prompt): delegate remember routing to right-memory"
```

---

### Task 3: Remove Platform-Written SOUL Migration Helper

**Files:**
- Modify: `crates/right-agent/src/identity_mirror.rs`

- [ ] **Step 1: Remove managed constants and helper functions**

In `crates/right-agent/src/identity_mirror.rs`, delete:

- `SOUL_OPERATING_CONTRACT_MARKER`
- `SOUL_OPERATING_CONTRACT_BLOCK`
- the full `with_soul_operating_contract` function
- the full `migrate_host_soul_operating_contract` function

Remove the full function bodies, not just the signatures. After this edit,
the first item after `IDENTITY_MIRROR_FILES` should be
`host_identity_mirror_complete`.

- [ ] **Step 2: Remove helper tests**

Delete these tests from the same file:

```rust
fn soul_operating_contract_migration_appends_managed_block()
fn soul_operating_contract_migration_is_idempotent()
fn host_soul_operating_contract_migration_skips_missing_soul()
fn host_soul_operating_contract_migration_updates_existing_soul_once()
```

Keep the existing mirror tests such as `host_identity_mirror_requires_only_identity_files`, `host_identity_mirror_complete_requires_all_identity_files`, and sandbox sync tests.

- [ ] **Step 3: Run identity mirror tests**

Run:

```bash
devenv shell -- cargo test -p right-agent identity_mirror
```

Expected: PASS. There should be no references to `SOUL_OPERATING_CONTRACT`, `with_soul_operating_contract`, or `migrate_host_soul_operating_contract`.

- [ ] **Step 4: Commit helper removal**

```bash
git add crates/right-agent/src/identity_mirror.rs
git commit -m "fix(agent): remove managed soul migration helper"
```

---

### Task 4: Make MCP Memory Surfaces Delegate Instead Of Duplicating The Router

**Files:**
- Modify: `crates/right/src/aggregator.rs`
- Modify: `crates/right/src/memory_server.rs`
- Modify: `crates/right/src/memory_server_mcp_tests.rs`

- [ ] **Step 1: Update `memory_retain` tool description**

In `crates/right/src/aggregator.rs`, replace the `memory_retain` description with:

```rust
"Store residual durable context to long-term memory after /right-memory \
 routing. Do not use as the default handler for remember/save/don't-forget \
 requests. Hindsight automatically extracts structured facts, resolves \
 entities, and indexes for retrieval.",
```

Replace the `content` property description with:

```rust
"Residual durable context to store after /right-memory routing when memory is the correct fallback target."
```

Keep the `context` property description unchanged.

- [ ] **Step 2: Update aggregator instructions**

In `crates/right/src/aggregator.rs`, replace the memory tools list inside `with_instructions()` with:

```rust
memory (`mcp__right__memory_retain`, `mcp__right__memory_recall`, and
`mcp__right__memory_reflect` — Hindsight mode only;
`mcp__right__memory_retain` is residual storage after `/right-memory` routing
chooses memory as the fallback target),
```

- [ ] **Step 3: Update aggregator test**

In `crates/right/src/aggregator.rs`, update `memory_retain_schema_marks_memory_as_residual_storage` so it asserts:

```rust
for needle in [
    "residual",
    "/right-memory",
    "Do not use as the default handler",
] {
    assert!(
        description.contains(needle),
        "memory_retain description must include {needle:?}: {description}"
    );
}

for forbidden in ["TOOLS.md", "USER.md", "SOUL.md", "IDENTITY.md"] {
    assert!(
        !description.contains(forbidden),
        "memory_retain description must not duplicate detailed routing: found {forbidden:?}"
    );
}
```

Keep the `content_desc.contains("/right-memory routing")` assertion.

- [ ] **Step 4: Update right MCP server instructions**

In `crates/right/src/memory_server.rs`, replace the `## Memory Routing` paragraph with:

```rust
                 ## Memory Routing\n\
                 When the user says \"remember\", \"save this\", or \"don't forget\", treat it as persistence intent and use the /right-memory skill to classify the correct target before calling mcp__right__memory_retain or editing files. mcp__right__memory_retain is only for residual durable context after /right-memory routing chooses memory as the fallback target.\n\n\
```

- [ ] **Step 5: Update memory server MCP test**

In `crates/right/src/memory_server_mcp_tests.rs`, replace the old
detailed-routing MCP info test with:

```rust
#[test]
fn test_get_info_delegates_memory_routing_to_right_memory() {
    let (server, _dir) = setup_server_with_dir();
    let info = server.get_info();
    let instructions = info.instructions.unwrap_or_default();
    for needle in [
        "remember",
        "save this",
        "don't forget",
        "/right-memory",
        "mcp__right__memory_retain",
        "residual durable context",
        "fallback target",
    ] {
        assert!(
            instructions.contains(needle),
            "instructions should delegate memory routing to /right-memory: missing {needle:?}; {instructions}"
        );
    }

    for forbidden in [
        concat!("Route tool/API/environment ", "rules"),
        concat!("stable user facts/preferences to ", "USER.md"),
        concat!("agent voice or escalation boundaries to ", "SOUL.md"),
        concat!("core identity/security posture to ", "IDENTITY.md"),
    ] {
        assert!(
            !instructions.contains(forbidden),
            "instructions must not duplicate detailed /right-memory routing: found {forbidden:?}"
        );
    }
}
```

- [ ] **Step 6: Run targeted MCP tests**

Run:

```bash
devenv shell -- cargo test -p right memory_retain_schema_marks_memory_as_residual_storage
devenv shell -- cargo test -p right test_get_info_delegates_memory_routing_to_right_memory
devenv shell -- cargo test -p right get_info_memory_tools_use_prefixed_agent_names
```

Expected: PASS.

- [ ] **Step 7: Commit MCP surface changes**

```bash
git add crates/right/src/aggregator.rs crates/right/src/memory_server.rs crates/right/src/memory_server_mcp_tests.rs
git commit -m "fix(memory): keep retain as residual fallback"
```

---

### Task 5: Update Prompt Documentation And Architecture Docs

**Files:**
- Modify: `PROMPT_SYSTEM.md`
- Modify: `docs/architecture/memory.md`
- Modify: `docs/architecture/lifecycle.md`

- [ ] **Step 1: Update `PROMPT_SYSTEM.md` prompt structure**

In `PROMPT_SYSTEM.md`, replace the stale `SOUL.md` prompt-structure line
that described a platform-authored contract for values, style, autonomy,
pushback, and escalation boundaries with:

```markdown
{SOUL.md - agent-authored durable voice, values, interaction style, and
 behavioral boundaries established by bootstrap or user intent}
```

Use the exact dash style already present in the file if the surrounding block uses Unicode punctuation.

- [ ] **Step 2: Replace detailed remember-routing paragraph**

In `PROMPT_SYSTEM.md`, replace the paragraph beginning with the old detailed
remember-routing sentence with:

```markdown
Identity files are always-loaded durable context. Right Agent explains their
purpose but does not own or prescribe their contents. `SOUL.md` is
agent-authored and changes only from bootstrap/user intent or explicit
conversation evidence.

For explicit "remember", "save this", or "don't forget" requests, the agent
must use the `/right-memory` skill to choose the persistence target before
editing identity files or calling memory tools. Operating instructions do not
embed the detailed target table; `mcp__right__memory_retain` is residual storage
after `/right-memory` selects memory as the target.
```

- [ ] **Step 3: Update MCP Server Instructions docs**

In `PROMPT_SYSTEM.md`, document the agent-facing retain tool name as:

```markdown
`mcp__right__memory_retain` is residual storage after `/right-memory` routing
chooses memory as the fallback target
```

- [ ] **Step 4: Update memory architecture doc**

In `docs/architecture/memory.md`, replace the detailed retain-routing paragraph with:

```markdown
Explicit retain is residual storage, not the default destination for every
"remember" request. Agent-facing prompt text directs explicit persistence
requests to the `/right-memory` skill, which owns the detailed routing between
identity files, tool notes, learned skills, and memory fallback.
```

- [ ] **Step 5: Update lifecycle architecture doc**

In `docs/architecture/lifecycle.md`, replace the bootstrap note that says `SOUL.md` is created with a platform-authored SOUL contract with:

```text
SOUL.md is created later by the bootstrap CC session from user choices)
```

Keep the diagram formatting coherent after the replacement.

- [ ] **Step 6: Check for forbidden stale wording**

Run:

```bash
devenv shell -- rg -n "compact[ ]operating[ ]contract|RIGHT_AGENT:SOUL[_]OPERATING[_]CONTRACT|managed operating[-]contract|platform-default[ ]operating[ ]contract" PROMPT_SYSTEM.md docs crates/right-codegen crates/right-agent crates/right/src
```

Expected: no matches except the approved phrase `do not invent a platform-default operating contract` in bootstrap/spec/plan files. If `rg` prints only approved docs/spec/plan references, verify no production code or generated prompt template contains stale platform-authored SOUL contract language.

- [ ] **Step 7: Commit docs**

```bash
git add PROMPT_SYSTEM.md docs/architecture/memory.md docs/architecture/lifecycle.md
git commit -m "docs(prompt): clarify identity file ownership"
```

---

### Task 6: Full Verification And PR Metadata

**Files:**
- No repository file changes expected unless verification reveals a test adjustment is needed.
- Update GitHub PR #71 body.

- [ ] **Step 1: Run formatting**

```bash
devenv shell -- cargo fmt --all
```

Expected: exit 0.

- [ ] **Step 2: Run package checks**

```bash
devenv shell -- cargo test -p right-codegen
devenv shell -- cargo test -p right-agent identity_mirror
devenv shell -- cargo test -p right memory_retain_schema_marks_memory_as_residual_storage
devenv shell -- cargo test -p right test_get_info_delegates_memory_routing_to_right_memory
devenv shell -- cargo test -p right get_info_memory_tools_use_prefixed_agent_names
```

Expected: each command exits 0.

- [ ] **Step 3: Run final workspace tests**

```bash
devenv shell -- cargo test --workspace
```

Expected: exit 0. Record ignored CI/OpenShell tests as ignored, not failures.

- [ ] **Step 4: Run final workspace build**

```bash
devenv shell -- cargo build --workspace
```

Expected: exit 0.

- [ ] **Step 5: Check worktree**

```bash
devenv shell -- git status -sb
```

Expected: either clean or only formatting/test-result changes that must be reviewed and committed. Do not leave untracked `docs/superpowers/` files.

- [ ] **Step 6: Commit any final formatting changes**

If `cargo fmt --all` changed files, commit them:

```bash
git add crates/right-codegen/src/agent_def_tests.rs crates/right-agent/src/identity_mirror.rs crates/right/src/aggregator.rs crates/right/src/memory_server.rs crates/right/src/memory_server_mcp_tests.rs
git commit -m "chore: format identity routing changes"
```

If there are no formatting changes, skip this step.

- [ ] **Step 7: Push branch**

```bash
devenv shell -- git push
```

Expected: branch `codex/identity-aware-memory-routing` updates on `origin`.

- [ ] **Step 8: Update PR #71 body**

Replace the current PR body with:

```markdown
## What changed

- Route explicit `remember` / `save this` requests through `/right-memory` before choosing a persistence target.
- Keep the detailed identity-aware routing table in the `/right-memory` skill.
- Mark `mcp__right__memory_retain` as residual fallback storage instead of the default destination for remember requests.
- Clarify that `SOUL.md` is user/agent-authored durable identity context and Right Agent does not inject platform-default content into it.
- Update prompt and architecture docs to match the ownership boundary.

## Why

Tool rules, user profile facts, identity/voice rules, and reusable procedures are always-loaded agent context. Storing them only in long-term memory makes the agent less likely to apply them reliably. At the same time, Right Agent must not decide the contents of `SOUL.md`; it can only explain the file's purpose and route user intent.

## Validation

- `devenv shell -- cargo fmt --all`
- `devenv shell -- cargo test -p right-codegen`
- `devenv shell -- cargo test -p right-agent identity_mirror`
- `devenv shell -- cargo test -p right memory_retain_schema_marks_memory_as_residual_storage`
- `devenv shell -- cargo test -p right test_get_info_delegates_memory_routing_to_right_memory`
- `devenv shell -- cargo test -p right get_info_memory_tools_use_prefixed_agent_names`
- `devenv shell -- cargo test --workspace`
- `devenv shell -- cargo build --workspace`
```

Use the GitHub connector `_update_pull_request` for `onsails/right-agent` PR `71`.

- [ ] **Step 9: Final status**

```bash
devenv shell -- git status -sb
devenv shell -- git log --oneline --decorate -5
```

Expected: clean worktree, branch tracking `origin/codex/identity-aware-memory-routing`, latest commits include the implementation and docs commits.
