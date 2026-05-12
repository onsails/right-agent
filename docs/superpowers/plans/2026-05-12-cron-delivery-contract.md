# Cron Delivery Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Teach cron agents that their structured output is the Telegram delivery channel, so they stop reaching for external messaging tools to "send" cron notifications.

**Architecture:** A new compiled-in `CRON_INSTRUCTIONS` template is injected into the system prompt only for cron runs, gated by a new local `PromptMode` enum (`Normal | Bootstrap | Cron`) in `crates/bot/src/cc/prompt.rs`. The `rightcron` SKILL.md is updated with authoring guidance that rewrites imperative messaging verbs into output-oriented phrasing so the same problem doesn't reach the cron at execution time.

**Tech Stack:** Rust (workspace, edition 2024), `include_str!` for compiled-in templates, existing `build_prompt_assembly_script` shell-emission machinery in `crates/bot/src/cc/prompt.rs`.

**Spec:** `docs/superpowers/specs/2026-05-12-cron-delivery-contract-design.md`.

---

## File Structure

| Action | Path | Responsibility |
|---|---|---|
| Create | `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md` | Static body of the "Cron Delivery Contract" section. |
| Modify | `crates/right-codegen/src/agent_def.rs` | Add `pub const CRON_INSTRUCTIONS` next to existing template consts. |
| Modify | `crates/right-codegen/src/lib.rs` | Re-export `CRON_INSTRUCTIONS` from `agent_def`. |
| Modify | `crates/right-codegen/src/agent_def_tests.rs` | Tests for the new const (non-empty + marker substrings). |
| Modify | `crates/bot/src/cc/prompt.rs` | Define `PromptMode` enum, switch `build_prompt_assembly_script` signature, emit `CRON_INSTRUCTIONS` for `PromptMode::Cron`, migrate existing tests + add new ones. |
| Modify | `crates/bot/src/telegram/worker.rs` (lines 1787, 1827) | Replace `bootstrap_mode: bool` arg with `PromptMode::Bootstrap`/`PromptMode::Normal`. |
| Modify | `crates/bot/src/cron.rs` (lines 480, 516) | Use `PromptMode::Cron` at both call sites. |
| Modify | `crates/bot/src/cron_delivery.rs` (lines 541, 567) | Use `PromptMode::Normal`. |
| Modify | `crates/bot/src/reflection.rs` (lines 245, 273) | Use `PromptMode::Normal` (reflection produces user-facing `REPLY_SCHEMA_JSON` output, not cron output). |
| Modify | `crates/right-codegen/skills/rightcron/SKILL.md` | Add "Writing Cron Prompts" section; bump `version: 3.2.0` → `3.3.0`. |
| Modify | `PROMPT_SYSTEM.md` | Update Callers table (mode column) and add Cron mode subsection. |

Total non-test callsites of `build_prompt_assembly_script`: 8 (worker:2, cron:2, cron_delivery:2, reflection:2). Test callsites in `crates/bot/src/cc/prompt.rs`: 12.

---

## Commit Convention

Every commit message in this plan ends with the trailer:

```
Closes #48
```

This is the GitHub close-on-merge keyword. GitHub auto-closes the issue when any commit message referencing this trailer lands on the default branch via PR merge. Multiple `Closes #48` trailers across commits are harmless.

Commit subjects follow Conventional Commits (e.g. `feat(prompt): ...`, `docs(spec): ...`) — match the style of recent `git log --oneline` entries.

---

### Task 1: Add `CRON_INSTRUCTIONS` template and const

**Files:**
- Create: `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`
- Modify: `crates/right-codegen/src/agent_def.rs` (add const just after `BOOTSTRAP_INSTRUCTIONS` at line 14)
- Modify: `crates/right-codegen/src/lib.rs` (extend the `pub use agent_def::{...}` block at lines 15–18)
- Test: `crates/right-codegen/src/agent_def_tests.rs`

- [ ] **Step 1: Write the failing test**

Append the following test to `crates/right-codegen/src/agent_def_tests.rs` (immediately after the existing `bootstrap_instructions_*` tests around line 147):

```rust
#[test]
fn cron_instructions_const_is_nonempty() {
    assert!(
        !crate::CRON_INSTRUCTIONS.is_empty(),
        "CRON_INSTRUCTIONS must not be empty"
    );
}

#[test]
fn cron_instructions_contains_delivery_contract_header() {
    assert!(
        crate::CRON_INSTRUCTIONS.contains("## Cron Delivery Contract"),
        "CRON_INSTRUCTIONS must contain Cron Delivery Contract header"
    );
}

#[test]
fn cron_instructions_contains_delivery_rule_marker() {
    assert!(
        crate::CRON_INSTRUCTIONS.contains("structured output IS the Telegram message"),
        "CRON_INSTRUCTIONS must explain that structured output IS the Telegram message"
    );
}

#[test]
fn cron_instructions_contains_no_clarifying_questions_rule() {
    assert!(
        crate::CRON_INSTRUCTIONS.contains("No clarifying questions"),
        "CRON_INSTRUCTIONS must contain No clarifying questions section"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-codegen cron_instructions_`
Expected: All four tests fail with a compile error: `cannot find value 'CRON_INSTRUCTIONS' in crate root`.

- [ ] **Step 3: Create the template file**

Create `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md` with the exact body from `docs/superpowers/specs/2026-05-12-cron-delivery-contract-design.md` § 1:

```markdown
## Cron Delivery Contract

You are executing as a scheduled task — there is no live user at the
other end of this turn. Two rules differ from a normal chat turn:

### 1. Your structured output IS the Telegram message

Delivery happens automatically: the runtime reads your output (per the
attached JSON schema) and sends `notify.content` to Telegram. You don't
call a tool to deliver — you produce the text.

- Non-null `notify` with non-empty `content` → message delivered.
- Null `notify` (only valid when the schema permits it) → silent run.
  Put a short factual reason in `no_notify_reason` (e.g. "no changes
  since last run"). Silent runs are visible to the user via
  `mcp__right__cron_list_runs`.

Do not use external messaging tools or a browser to send Telegram
messages — the runtime is the only delivery path. Every such attempt
wastes budget and never reaches the user.

`@username` inside `notify.content` is plain text. The runtime sends
the message; the Telegram client renders the mention.

### 2. No clarifying questions

There is no live user to answer questions during this turn. If the
task is ambiguous:

- Pick a sensible default, do the work, and explain what you chose in
  `notify.content` so the user can correct it next turn.
- Or, if your schema permits, set `notify: null` with
  `no_notify_reason` describing what blocked you.

Don't end `notify.content` with a question expecting a reply — the
user receives a one-off cron message, not a chat.
```

- [ ] **Step 4: Add the const in `agent_def.rs`**

Insert immediately after line 14 of `crates/right-codegen/src/agent_def.rs`:

```rust
/// Cron delivery contract, compiled into the binary.
///
/// Injected into the system prompt for `PromptMode::Cron` runs
/// (cron::execute_job — both regular cron jobs and background
/// continuation). Tells the agent that its structured output IS
/// the Telegram delivery channel and that the turn has no live user.
/// Source: `templates/right/prompt/CRON_INSTRUCTIONS.md`
pub const CRON_INSTRUCTIONS: &str =
    include_str!("../templates/right/prompt/CRON_INSTRUCTIONS.md");
```

- [ ] **Step 5: Re-export from `lib.rs`**

In `crates/right-codegen/src/lib.rs`, change lines 15–18 from:

```rust
pub use agent_def::{
    BG_CONTINUATION_SCHEMA_JSON, BOOTSTRAP_INSTRUCTIONS, BOOTSTRAP_SCHEMA_JSON, CRON_SCHEMA_JSON,
    OPERATING_INSTRUCTIONS, REPLY_SCHEMA_JSON, generate_system_prompt,
};
```

to:

```rust
pub use agent_def::{
    BG_CONTINUATION_SCHEMA_JSON, BOOTSTRAP_INSTRUCTIONS, BOOTSTRAP_SCHEMA_JSON, CRON_INSTRUCTIONS,
    CRON_SCHEMA_JSON, OPERATING_INSTRUCTIONS, REPLY_SCHEMA_JSON, generate_system_prompt,
};
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-codegen cron_instructions_`
Expected: All four tests PASS.

Also run the existing operating/bootstrap tests as a sanity check:
Run: `devenv shell -- cargo test -p right-codegen --lib`
Expected: All pre-existing tests still PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md \
        crates/right-codegen/src/agent_def.rs \
        crates/right-codegen/src/lib.rs \
        crates/right-codegen/src/agent_def_tests.rs
git commit -m "$(cat <<'EOF'
feat(codegen): add CRON_INSTRUCTIONS template

Compiled-in template for the cron delivery contract that will be
injected into the system prompt for cron runs in the next commit.

Closes #48
EOF
)"
```

---

### Task 2: Define `PromptMode` enum and migrate signature (mechanical refactor, no behaviour change)

**Files:**
- Modify: `crates/bot/src/cc/prompt.rs` (add enum near the top of the file; replace `bootstrap_mode: bool` in `build_prompt_assembly_script` signature; update internal `if bootstrap_mode` branch to `match mode`)
- Modify: `crates/bot/src/cc/prompt.rs` (12 test callsites — see Step 4)
- Modify: `crates/bot/src/telegram/worker.rs` (lines 1787, 1827)
- Modify: `crates/bot/src/cron.rs` (lines 480, 516)
- Modify: `crates/bot/src/cron_delivery.rs` (lines 541, 567)
- Modify: `crates/bot/src/reflection.rs` (lines 245, 273)

This task is a pure refactor: every callsite maps directly to either `PromptMode::Bootstrap` (where the old arg was `true`) or `PromptMode::Normal` (where the old arg was `false`). The cron callsites stay on `PromptMode::Normal` for now — Task 3 flips them to `PromptMode::Cron` once the Cron-mode emission is implemented.

- [ ] **Step 1: Add the `PromptMode` enum**

In `crates/bot/src/cc/prompt.rs`, immediately after the `MemoryMode` enum (which ends near line 9), add:

```rust
/// Which composite prompt body to assemble.
///
/// `Bootstrap` swaps Operating Instructions for Bootstrap Instructions
/// and skips identity files (they're being created this turn).
/// `Cron` keeps Operating Instructions and adds the Cron Delivery
/// Contract; identity files are still emitted. `Normal` is the
/// everyday worker/delivery/reflection path.
pub(crate) enum PromptMode {
    Normal,
    Bootstrap,
    Cron,
}
```

- [ ] **Step 2: Change `build_prompt_assembly_script` signature**

In `crates/bot/src/cc/prompt.rs:54`, change:

```rust
pub(crate) fn build_prompt_assembly_script(
    base_prompt: &str,
    bootstrap_mode: bool,
    root_path: &str,
    prompt_file: &str,
    workdir: &str,
    claude_args: &[String],
    mcp_instructions: Option<&str>,
    memory_mode: Option<&MemoryMode>,
) -> String {
```

to:

```rust
pub(crate) fn build_prompt_assembly_script(
    base_prompt: &str,
    mode: PromptMode,
    root_path: &str,
    prompt_file: &str,
    workdir: &str,
    claude_args: &[String],
    mcp_instructions: Option<&str>,
    memory_mode: Option<&MemoryMode>,
) -> String {
```

And update the inner branch (around line 68) from:

```rust
let file_sections = if bootstrap_mode {
```

to:

```rust
let file_sections = if matches!(mode, PromptMode::Bootstrap) {
```

Also update the memory-section gate at line 98 from:

```rust
let memory_section = if bootstrap_mode {
```

to:

```rust
let memory_section = if matches!(mode, PromptMode::Bootstrap) {
```

These two `matches!` calls preserve current behaviour exactly — `PromptMode::Cron` and `PromptMode::Normal` take the same path as `bootstrap_mode = false` did.

- [ ] **Step 3: Update the test helper `test_script`**

In `crates/bot/src/cc/prompt.rs:255`, change:

```rust
fn test_script(base: &str, bootstrap: bool, args: &[String], mcp: Option<&str>) -> String {
    build_prompt_assembly_script(
        base,
        bootstrap,
        "/sandbox",
        ...
```

to:

```rust
fn test_script(base: &str, mode: PromptMode, args: &[String], mcp: Option<&str>) -> String {
    build_prompt_assembly_script(
        base,
        mode,
        "/sandbox",
        ...
```

- [ ] **Step 4: Migrate the 12 test callsites in `crates/bot/src/cc/prompt.rs`**

Mechanical substitution across the file's `#[cfg(test)] mod tests` block:

| Old arg | New arg |
|---|---|
| `true` (passed as `bootstrap`/`bootstrap_mode`) | `PromptMode::Bootstrap` |
| `false` (passed as `bootstrap`/`bootstrap_mode`) | `PromptMode::Normal` |

Specifically:
- `test_script(..., true, ...)` → `test_script(..., PromptMode::Bootstrap, ...)` (lines 270, 313, 433)
- `test_script(..., false, ...)` → `test_script(..., PromptMode::Normal, ...)` (lines 296, 321, 339, 396, 410, 451)
- Direct `build_prompt_assembly_script(..., true, ...)` → `..., PromptMode::Bootstrap, ...` (lines 372, 644)
- Direct `build_prompt_assembly_script(..., false, ...)` → `..., PromptMode::Normal, ...` (lines 346, 416, 464, 490, 509, 525, 545, 590, 707)

Use search-and-replace carefully — the second positional argument to `test_script` / `build_prompt_assembly_script` is the one to change. Verify each substitution by reading the surrounding context (the file paths immediately after disambiguate identity from custom paths).

- [ ] **Step 5: Update worker.rs callsites**

In `crates/bot/src/telegram/worker.rs`, find the existing variable that holds the bootstrap boolean (typically a local `bootstrap` or `bootstrap_mode` near line 1787). At both call sites (1787 and 1827), replace the bare boolean argument with:

```rust
if bootstrap { crate::cc::prompt::PromptMode::Bootstrap } else { crate::cc::prompt::PromptMode::Normal }
```

The exact local variable name is determined by reading the surrounding context — search for the variable that was being passed as the second argument.

- [ ] **Step 6: Update cron.rs callsites (temporary `Normal`, flipped to `Cron` in Task 3)**

In `crates/bot/src/cron.rs:480` and `crates/bot/src/cron.rs:516`, replace the second positional argument `false` with `crate::cc::prompt::PromptMode::Normal`.

Rationale for `Normal` at this step: we want this refactor to be a pure no-op behaviour change. Task 3 flips both call sites to `PromptMode::Cron` together with the test that proves the Cron-mode behaviour. Keeping them on `Normal` here keeps the bisect surface tiny if anything breaks.

- [ ] **Step 7: Update cron_delivery.rs callsites**

In `crates/bot/src/cron_delivery.rs:541` and `crates/bot/src/cron_delivery.rs:567`, replace the second positional argument `false` with `crate::cc::prompt::PromptMode::Normal`.

- [ ] **Step 8: Update reflection.rs callsites**

In `crates/bot/src/reflection.rs:245` and `crates/bot/src/reflection.rs:273`, replace the second positional argument `false` with `crate::cc::prompt::PromptMode::Normal`.

Reflection produces user-facing `REPLY_SCHEMA_JSON` output (verified at `reflection.rs:205`) — even when triggered by a failed cron, the reply is a chat-style summary, so Normal is the correct mode.

- [ ] **Step 9: Build the workspace**

Run: `devenv shell -- cargo build --workspace`
Expected: clean build, no errors. If the compiler reports a missed callsite, add it to this step and re-run.

- [ ] **Step 10: Run the prompt-module tests**

Run: `devenv shell -- cargo test -p right-bot --lib cc::prompt::`
Expected: all 12 pre-existing tests PASS (the refactor preserves behaviour exactly).

Also run the full workspace test suite to verify nothing else regressed:
Run: `devenv shell -- cargo test --workspace`
Expected: all tests PASS.

- [ ] **Step 11: Commit**

```bash
git add crates/bot/src/cc/prompt.rs \
        crates/bot/src/telegram/worker.rs \
        crates/bot/src/cron.rs \
        crates/bot/src/cron_delivery.rs \
        crates/bot/src/reflection.rs
git commit -m "$(cat <<'EOF'
refactor(prompt): introduce PromptMode enum, replace bootstrap_mode bool

Local enum with three exhaustive variants (Normal / Bootstrap / Cron)
replaces the bootstrap_mode bool argument to build_prompt_assembly_script.
This commit is a pure refactor — every callsite maps to Normal (where
the bool was false) or Bootstrap (where it was true). Cron-mode emission
follows in the next commit.

Closes #48
EOF
)"
```

---

### Task 3: Implement `PromptMode::Cron` emission and flip cron callsites

**Files:**
- Modify: `crates/bot/src/cc/prompt.rs` (emit `CRON_INSTRUCTIONS` after Operating Instructions when mode is `Cron`)
- Modify: `crates/bot/src/cron.rs:480, 516` (switch from `Normal` to `Cron`)
- Test: `crates/bot/src/cc/prompt.rs` (new tests at the end of the `#[cfg(test)] mod tests` block)

- [ ] **Step 1: Write the failing tests**

Append at the end of `#[cfg(test)] mod tests` in `crates/bot/src/cc/prompt.rs` (just before the closing `}` of `mod tests` near line 742):

```rust
#[test]
fn cron_mode_includes_cron_delivery_contract() {
    let script = test_script(
        "Base prompt",
        PromptMode::Cron,
        &["claude".into(), "-p".into()],
        None,
    );
    assert!(
        script.contains("## Cron Delivery Contract"),
        "Cron mode must include the Cron Delivery Contract header"
    );
    assert!(
        script.contains("structured output IS the Telegram message"),
        "Cron mode must include the delivery-rule marker phrase"
    );
}

#[test]
fn cron_mode_includes_operating_instructions_before_contract() {
    let script = test_script(
        "Base prompt",
        PromptMode::Cron,
        &["claude".into(), "-p".into()],
        None,
    );
    let ops_pos = script
        .find("## Operating Instructions")
        .expect("must include Operating Instructions");
    let contract_pos = script
        .find("## Cron Delivery Contract")
        .expect("must include Cron Delivery Contract");
    assert!(
        ops_pos < contract_pos,
        "Operating Instructions must appear before Cron Delivery Contract"
    );
}

#[test]
fn cron_mode_contract_appears_before_identity_files() {
    let script = test_script(
        "Base prompt",
        PromptMode::Cron,
        &["claude".into(), "-p".into()],
        None,
    );
    let contract_pos = script
        .find("## Cron Delivery Contract")
        .expect("must include Cron Delivery Contract");
    let identity_pos = script
        .find("IDENTITY.md")
        .expect("Cron mode must still emit IDENTITY.md");
    assert!(
        contract_pos < identity_pos,
        "Cron Delivery Contract must appear before identity files"
    );
}

#[test]
fn normal_mode_omits_cron_delivery_contract() {
    let script = test_script(
        "Base prompt",
        PromptMode::Normal,
        &["claude".into(), "-p".into()],
        None,
    );
    assert!(
        !script.contains("Cron Delivery Contract"),
        "Normal mode must not leak the cron contract into worker/delivery turns"
    );
}

#[test]
fn bootstrap_mode_omits_cron_delivery_contract() {
    let script = test_script(
        "Base prompt",
        PromptMode::Bootstrap,
        &["claude".into(), "-p".into()],
        None,
    );
    assert!(
        !script.contains("Cron Delivery Contract"),
        "Bootstrap mode must not include the cron contract"
    );
}

#[test]
fn cron_mode_does_not_emit_memory_section_when_memory_mode_none() {
    let script = build_prompt_assembly_script(
        "Base",
        PromptMode::Cron,
        "/sandbox",
        "/tmp/right-system-prompt.md",
        "/sandbox",
        &["claude".into()],
        None,
        None, // cron callsites always pass None today
    );
    assert!(
        !script.contains("MEMORY.md"),
        "Cron mode with memory_mode=None must not emit MEMORY.md"
    );
    assert!(
        !script.contains("composite-memory"),
        "Cron mode with memory_mode=None must not emit composite-memory"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot --lib cc::prompt::cron_mode_ cc::prompt::normal_mode_omits cc::prompt::bootstrap_mode_omits`
Expected:
- `cron_mode_includes_cron_delivery_contract` → FAIL (no `Cron Delivery Contract` substring).
- `cron_mode_includes_operating_instructions_before_contract` → FAIL (contract not present).
- `cron_mode_contract_appears_before_identity_files` → FAIL (contract not present).
- `normal_mode_omits_cron_delivery_contract` → PASS (vacuously — contract isn't in any mode yet).
- `bootstrap_mode_omits_cron_delivery_contract` → PASS (vacuously).
- `cron_mode_does_not_emit_memory_section_when_memory_mode_none` → PASS (vacuously — Cron currently behaves like Normal, and Normal-with-None emits no memory).

The `cron_mode_*` failures are what we expect; the others are documenting future-proofing.

- [ ] **Step 3: Emit `CRON_INSTRUCTIONS` for `PromptMode::Cron`**

In `crates/bot/src/cc/prompt.rs`, locate the `file_sections` assembly (currently around lines 68–88). Replace the `else` branch (the non-bootstrap path) with:

```rust
} else {
    let escaped_ops = right_codegen::OPERATING_INSTRUCTIONS.replace('\'', "'\\''");
    let mut sections =
        format!("\nprintf '\\n## Operating Instructions\\n'\nprintf '%s\\n' '{escaped_ops}'");

    if matches!(mode, PromptMode::Cron) {
        let escaped_cron = right_codegen::CRON_INSTRUCTIONS.replace('\'', "'\\''");
        sections.push_str(&format!(
            "\nprintf '\\n'\nprintf '%s\\n' '{escaped_cron}'"
        ));
    }

    for s in PROMPT_SECTIONS {
        let filename = s.filename;
        let header = s.header;
        sections.push_str(&format!(
            r#"
if [ -f {root_path}/{filename} ]; then
  printf '\n{header}\n'
  cat {root_path}/{filename}
  printf '\n'
fi"#
        ));
    }
    sections
};
```

The new block:
1. Emits Operating Instructions (unchanged).
2. If `mode == Cron`, emits `CRON_INSTRUCTIONS` next. The `CRON_INSTRUCTIONS` template itself starts with `## Cron Delivery Contract`, so no extra header is needed — `printf '%s\\n'` writes the template verbatim.
3. Emits identity files via the existing `PROMPT_SECTIONS` loop (unchanged).

The single-quote escape (`'\''`) is identical to how `OPERATING_INSTRUCTIONS` is handled — required because the entire `printf '...'` is wrapped in shell single quotes.

- [ ] **Step 4: Flip the cron.rs callsites from `Normal` to `Cron`**

In `crates/bot/src/cron.rs:480` and `crates/bot/src/cron.rs:516`, change:

```rust
crate::cc::prompt::PromptMode::Normal
```

to:

```rust
crate::cc::prompt::PromptMode::Cron
```

Both sandbox and no-sandbox branches of `run_cron_task` flip together. `select_schema_and_fork` ensures both `CRON_SCHEMA_JSON` and `BG_CONTINUATION_SCHEMA_JSON` runs route through `run_cron_task`, so a single mode flag covers both.

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot --lib cc::prompt::`
Expected: all tests PASS (including the six new ones from Step 1).

Run the full workspace test suite:
Run: `devenv shell -- cargo test --workspace`
Expected: all tests PASS.

- [ ] **Step 6: Run clippy**

Run: `devenv shell -- cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings, no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/cc/prompt.rs crates/bot/src/cron.rs
git commit -m "$(cat <<'EOF'
feat(prompt): emit Cron Delivery Contract for cron sessions

PromptMode::Cron now injects CRON_INSTRUCTIONS after Operating
Instructions and before identity files. cron::execute_job (both
sandbox and no-sandbox branches) switches to PromptMode::Cron so
every cron run — including background continuation — gets the
delivery contract in its system prompt.

The contract tells the agent that its structured output IS the
Telegram delivery channel, that there is no live user to answer
clarifying questions, and that @mentions are plain text rendered
by Telegram on receipt.

Closes #48
EOF
)"
```

---

### Task 4: Add "Writing Cron Prompts" guidance to `rightcron` SKILL.md

**Files:**
- Modify: `crates/right-codegen/skills/rightcron/SKILL.md`

This task reduces the chance that imperative messaging verbs ("tag X", "send to Y") reach a cron at execution time in the first place. The runtime contract from Task 3 catches the rest.

- [ ] **Step 1: Bump the skill version**

In `crates/right-codegen/skills/rightcron/SKILL.md:8`, change:

```yaml
version: 3.2.0
```

to:

```yaml
version: 3.3.0
```

Minor bump — additive content change, no breaking semantics.

- [ ] **Step 2: Insert the new section**

In `crates/right-codegen/skills/rightcron/SKILL.md`, add the following section immediately after the "Creating a Cron Job" section (which ends around line 60, just before "## Editing a Cron Job"):

```markdown
## Writing Cron Prompts

The cron runs as a separate, non-interactive session — its only
delivery channel is its structured output. Phrase the `prompt:` as a
**task that produces text**, not as an imperative messaging action.
Imperative verbs like "send", "tag", "notify", "ping" prime the cron
agent to look for an external messaging tool.

| User said                            | Store as                                                  |
|--------------------------------------|-----------------------------------------------------------|
| "Tag @bob with a reminder about X"   | "Output a reminder about X, mentioning @bob"              |
| "Send a message to @alice at 9am"    | "Output a heads-up about <topic>, addressed to @alice"   |
| "Ping me when Y happens"             | "Check Y. If it happened, output a notification about it" |
| "Notify the channel about Z"         | "Output a notification about Z"                           |

`@username` is fine as plain text — it ends up in the delivered
message and Telegram renders it as a mention. Don't strip the user's
content or schedule; only rephrase the delivery-imperative verbs.
```

- [ ] **Step 3: Verify the skill still parses correctly**

The skill is bundled into the binary via `right-codegen/src/skills.rs::install_builtin_skills`. The frontmatter and body are not validated at compile time, but a smoke test of the codegen pipeline confirms the file is present:

Run: `devenv shell -- cargo test -p right-codegen skills::`
Expected: all tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/right-codegen/skills/rightcron/SKILL.md
git commit -m "$(cat <<'EOF'
docs(rightcron): guide agents to write output-oriented cron prompts

New "Writing Cron Prompts" section in the rightcron skill instructs
the agent to rewrite imperative messaging verbs ("tag", "send",
"notify", "ping") into output-oriented phrasing when storing the
cron prompt. Combined with the runtime delivery contract, this
reduces the chance that the cron agent reaches for an external
messaging tool at execution time.

Skill version bumped 3.2.0 → 3.3.0.

Closes #48
EOF
)"
```

---

### Task 5: Update `PROMPT_SYSTEM.md`

**Files:**
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Update the Callers table**

In `PROMPT_SYSTEM.md`, locate the Callers table (currently around lines 37–42) which reads:

```markdown
| Caller | Module | bootstrap_mode | Schema | Model |
|--------|--------|---------------|--------|-------|
| Worker (Telegram messages) | `telegram/worker.rs` | true/false | reply-schema.json | agent config |
| Cron (scheduled jobs) | `cron.rs` | false | CRON_SCHEMA_JSON | agent config |
| Cron (background continuation) | `cron.rs` (`ScheduleKind::BackgroundContinuation`) | false | BG_CONTINUATION_SCHEMA_JSON | agent config |
| Delivery (cron result relay) | `cron_delivery.rs` | false | reply-schema.json | claude-haiku-4-5-20251001 |
```

Replace with:

```markdown
| Caller | Module | mode | Schema | Model |
|--------|--------|------|--------|-------|
| Worker (Telegram messages) | `telegram/worker.rs` | `Normal` or `Bootstrap` | reply-schema.json / bootstrap-schema.json | agent config |
| Cron (scheduled jobs) | `cron.rs` | `Cron` | CRON_SCHEMA_JSON | agent config |
| Cron (background continuation) | `cron.rs` (`ScheduleKind::BackgroundContinuation`) | `Cron` | BG_CONTINUATION_SCHEMA_JSON | agent config |
| Delivery (cron result relay) | `cron_delivery.rs` | `Normal` | reply-schema.json | claude-haiku-4-5-20251001 |
| Reflection (post-failure summary) | `reflection.rs` | `Normal` | reply-schema.json | agent config |
```

Reflection is added because it's a `build_prompt_assembly_script` caller and the table should be complete.

- [ ] **Step 2: Add a "Cron mode" subsection under "Prompt Structure"**

In `PROMPT_SYSTEM.md`, locate the "Bootstrap mode" subsection (currently around lines 120–127) and insert a new "Cron mode" subsection immediately after it, before the "Compiled-in Content" heading:

````markdown
### Cron mode

```
[Base: Right Agent agent description, sandbox info, MCP reference]

## Operating Instructions
{compiled-in from templates/right/prompt/OPERATING_INSTRUCTIONS.md}

## Cron Delivery Contract
{compiled-in from templates/right/prompt/CRON_INSTRUCTIONS.md}

## Your Identity
{IDENTITY.md}

## Your Personality and Values
{SOUL.md}

## Your User
{USER.md}

## Environment and Tools
{TOOLS.md}

## MCP Server Instructions  (if any external MCP servers have instructions)
{fetched from aggregator via POST /mcp-instructions}
```

Cron mode is selected by `cron::execute_job` for both regular cron
runs (`CRON_SCHEMA_JSON`) and background-continuation runs
(`BG_CONTINUATION_SCHEMA_JSON`). The memory section is intentionally
omitted — cron jobs are static instructions, not user queries; agents
that need memory call `memory_recall` explicitly from the cron prompt.

The `## Cron Delivery Contract` block tells the agent that its
structured output is the Telegram delivery channel and that the turn
has no live user. See [issue #48](https://github.com/onsails/right-agent/issues/48)
for the production incidents that motivated this section.
````

- [ ] **Step 3: Update the "Compiled-in Content" section to mention `CRON_INSTRUCTIONS`**

In `PROMPT_SYSTEM.md` (around line 131 in the current file), the existing paragraph reads:

```markdown
Operating instructions and bootstrap content are compiled into the binary via
`include_str!()` from `templates/right/prompt/` and `templates/right/agent/`.
```

Change it to:

```markdown
Operating instructions, cron-delivery contract, and bootstrap content
are compiled into the binary via `include_str!()` from
`templates/right/prompt/` and `templates/right/agent/`.
```

- [ ] **Step 4: Smoke-test the docs**

There are no automated tests for `PROMPT_SYSTEM.md`. Verify by reading the diff for typos and broken Markdown:

Run: `git diff PROMPT_SYSTEM.md`
Expected: only the three sections above are modified; no other edits.

- [ ] **Step 5: Final workspace build (sanity)**

Run: `devenv shell -- cargo build --workspace`
Expected: clean build.

- [ ] **Step 6: Commit**

```bash
git add PROMPT_SYSTEM.md
git commit -m "$(cat <<'EOF'
docs(prompt-system): document PromptMode and Cron mode prompt layout

Callers table now uses the PromptMode enum (Normal/Bootstrap/Cron),
including reflection.rs as a caller. New "Cron mode" subsection
under "Prompt Structure" shows the assembled layout for cron runs
(Operating Instructions → Cron Delivery Contract → identity files
→ MCP instructions, with memory section intentionally omitted).
Compiled-in content paragraph mentions CRON_INSTRUCTIONS.

Closes #48
EOF
)"
```

---

## Self-Review Checklist

After implementing all tasks, run this checklist before opening the PR:

- [ ] **Spec coverage:** Every numbered section of the spec maps to a task above.
  - Spec § 1 (New compiled-in template) → Task 1.
  - Spec § 2 (Prompt mode selector) → Task 2 Step 1.
  - Spec § 3 (Assembly behaviour per mode) → Task 3 Steps 1–3.
  - Spec § 4 (Callsite changes) → Task 2 Steps 4–8 + Task 3 Step 4.
  - Spec § 5 (`rightcron` SKILL.md edit) → Task 4.
  - Spec § 6 (`PROMPT_SYSTEM.md` updates) → Task 5.
  - Spec § 7 (Test coverage) → Task 1 Steps 1 + 6, Task 3 Steps 1–2 + 5.
- [ ] **No placeholders:** Search the diff for `TODO`, `TBD`, `FIXME` — should be zero new occurrences.
- [ ] **Type consistency:** `PromptMode` variants used identically everywhere — no `PromptMode::CronMode` typos, no `PromptMode::Reply` invented variants.
- [ ] **Closes trailer present:** Every commit message ends with `Closes #48`. Verify with `git log --grep="Closes #48" origin/master..HEAD` — should return five commits.
- [ ] **Existing tests untouched in semantics:** The 12 migrated test callsites in `crates/bot/src/cc/prompt.rs` only changed their second-argument literal (`true`/`false` → `PromptMode::*`); no assertion changes.
- [ ] **No file > 900 LoC introduced:** None of the modified files cross the 900-LoC threshold from CLAUDE.md.
- [ ] **No haiku subagent used:** Per CLAUDE.md, subagents are sonnet at minimum. If subagent-driven execution is chosen, configure the subagent dispatcher accordingly.

---

## Out of Scope (do not implement)

- JSON schema changes (`CRON_SCHEMA_JSON`, `BG_CONTINUATION_SCHEMA_JSON`).
- Runtime tool blocking (`--disallowedTools` for Composio/browser tools).
- Migration of existing `cron_specs.prompt` rows.
- Composio-specific language anywhere in templates or skills.
- New MCP tools, new schemas, new on-disk state, sandbox recreation.
