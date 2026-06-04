# Skill-Writer Multi-Model Delegation Awareness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make RightClaw's skill writer author multi-step `rightx-*` skills that delegate mechanical / disposable-intermediate steps to cheaper-model subagents, and fix the probe-writer prompt drift so the canonical instructions are actually sent at runtime.

**Architecture:** A single canonical model-tier ladder (`haiku`/`sonnet`/default) lives in `OPERATING_INSTRUCTIONS.md`. The automatic probe-writer's runtime prompt is consolidated to compose from the existing-but-dead `right_codegen::PROBE_WRITER_ANCHOR_TEMPLATE` + `PROBE_WRITER_INSTRUCTIONS` constants (instead of an inline duplicate), and a concrete delegation directive is added once to `PROBE_WRITER_INSTRUCTIONS` and to the explicit `right-learn-skill/SKILL.md`. Simple single-procedure recipes get no delegation boilerplate.

**Tech Stack:** Rust (edition 2024), `cargo`/`devenv`, `include_str!`-embedded markdown templates, compile-time string constants.

**Spec:** `docs/superpowers/specs/2026-06-04-skill-writer-multi-model-delegation-design.md`

---

## Important conventions for the worker

- **Verification cadence (project rule):** run the *targeted* command after each task; run the full workspace suite only at the end (Task 6). Do not run `cargo test --workspace` after every task.
- **Run commands via devenv:** prefix with `devenv shell -- ` (e.g. `devenv shell -- cargo test -p right-codegen <filter>`).
- **Commit hook is broken** in this checkout (`prek's Git shim is installed in migration mode`). These are docs/string/template edits with no code-formatting impact; commit with `git commit --no-verify`. (If a later task edits Rust and you want rustfmt, run `devenv shell -- cargo fmt -p <crate>` manually before committing.)
- **Grep caveat in this environment:** the interactive `rg` shim sometimes renders the *matched token* as `n`/`l`. Trust `Read` and editor file state over grep for exact content.
- **No `set_var` in tests; pass config through params** (project rule). None of these tasks need env.
- The probe-writer's `allowed_tools` are `Write, Read, Bash, mcp__right__skill_learning_start, mcp__right__skill_learning_finish` (see `build_invocation` in `crates/bot/src/learning_probe_writer.rs`). There is **no `Edit` tool** — any instruction telling the writer to use `Edit` is a bug.

---

## File Structure

- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` — canonical model-tier ladder (add `haiku` rung). Prompt-tier: keep terse.
- `crates/right-codegen/src/agent_def.rs` — `PROBE_WRITER_INSTRUCTIONS` constant (add delegation directive; fix `Edit/Write` → `Read + Write`).
- `crates/right-codegen/src/agent_def_tests.rs` — extend the two probe-writer / operating-instructions content tests.
- `crates/bot/src/learning_probe_writer.rs` — `build_user_prompt`: compose from the canonical constants instead of inline duplicate; trim redundant hint text. Extend its unit tests.
- `crates/right-codegen/skills/right-learn-skill/SKILL.md` — add a delegation bullet to "Skill Quality".
- `crates/right-codegen/src/skills.rs` — extend the SKILL.md content test.
- `PROMPT_SYSTEM.md` — document the consolidation + delegation behavior + ladder.

---

## Task 1: Add the `haiku` rung to the canonical model-tier ladder

**Files:**
- Test: `crates/right-codegen/src/agent_def_tests.rs` (add a test near the existing `operating_instructions_teach_agent_tool_delegation`, ~line 540)
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md:97`

- [ ] **Step 1: Write the failing test**

Add to `crates/right-codegen/src/agent_def_tests.rs`:

```rust
#[test]
fn operating_instructions_teach_three_tier_model_ladder() {
    let ops = crate::OPERATING_INSTRUCTIONS;
    for needle in [r#"model: "haiku""#, r#"model: "sonnet""#] {
        assert!(
            ops.contains(needle),
            "OPERATING_INSTRUCTIONS must teach the {needle:?} subagent tier"
        );
    }
    assert!(
        ops.contains("judgment calls"),
        "OPERATING_INSTRUCTIONS must keep the default-model judgment-call rung"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-codegen operating_instructions_teach_three_tier_model_ladder`
Expected: FAIL — current text has `model: "sonnet"` but no `model: "haiku"`.

- [ ] **Step 3: Edit the template**

In `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`, replace the single paragraph at line 97 (currently beginning `For mechanical subagent work (long reads, summarization, source sweeps, extraction, format conversion), pass \`model: "sonnet"\`.`) with exactly:

```markdown
Match the subagent's model to the work. Pass `model: "haiku"` for purely mechanical steps with easily-verified output (format conversion, field extraction, mechanical file reads); `model: "sonnet"` for mechanical work needing light comprehension (long reads, summarization, source sweeps). Keep the default model for judgment calls — design decisions, ambiguous-spec interpretation, anything you'd want your strongest model on. Downgrades are free savings when your main is opus.
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-codegen operating_instructions_teach_three_tier_model_ladder`
Expected: PASS

- [ ] **Step 5: Run the existing operating-instructions tests (no regression)**

Run: `devenv shell -- cargo test -p right-codegen operating_instructions`
Expected: PASS (including `operating_instructions_teach_sparse_progress_updates` and `operating_instructions_teach_agent_tool_delegation`, which assert tokens unchanged by this edit).

- [ ] **Step 6: Commit**

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md crates/right-codegen/src/agent_def_tests.rs
git commit --no-verify -m "feat(prompt): add haiku rung to subagent model ladder"
```

---

## Task 2: Add the delegation directive to `PROBE_WRITER_INSTRUCTIONS` and fix the `Edit` bug

**Files:**
- Test: `crates/right-codegen/src/agent_def_tests.rs:754-761` (extend `probe_writer_instructions_contain_class_first_guidance`)
- Modify: `crates/right-codegen/src/agent_def.rs:102-137` (the `PROBE_WRITER_INSTRUCTIONS` constant)

- [ ] **Step 1: Extend the test (failing)**

In `crates/right-codegen/src/agent_def_tests.rs`, replace the body of `probe_writer_instructions_contain_class_first_guidance` (lines 754-761) with:

```rust
#[test]
fn probe_writer_instructions_contain_class_first_guidance() {
    assert!(PROBE_WRITER_INSTRUCTIONS.contains("Survey"));
    assert!(PROBE_WRITER_INSTRUCTIONS.to_lowercase().contains("update"));
    assert!(PROBE_WRITER_INSTRUCTIONS.contains("rightx-"));
    assert!(PROBE_WRITER_INSTRUCTIONS.contains("skill_learning_start"));
    assert!(PROBE_WRITER_INSTRUCTIONS.contains("skill_learning_finish"));
    // Delegation-authoring directive (multi-model awareness).
    assert!(
        PROBE_WRITER_INSTRUCTIONS.contains(r#"`haiku`"#)
            && PROBE_WRITER_INSTRUCTIONS.contains(r#"`sonnet`"#),
        "PROBE_WRITER_INSTRUCTIONS must teach delegation model tiers"
    );
    assert!(
        PROBE_WRITER_INSTRUCTIONS.contains("disposable-intermediate"),
        "PROBE_WRITER_INSTRUCTIONS must scope delegation to mechanical/disposable steps"
    );
    // The probe-writer has no Edit tool; instructions must not tell it to use Edit.
    assert!(
        !PROBE_WRITER_INSTRUCTIONS.contains("Edit/Write"),
        "PROBE_WRITER_INSTRUCTIONS must not instruct the writer to use the (unavailable) Edit tool"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-codegen probe_writer_instructions_contain_class_first_guidance`
Expected: FAIL — constant lacks the delegation directive and still contains `Edit/Write`.

- [ ] **Step 3: Edit the constant**

In `crates/right-codegen/src/agent_def.rs`, inside `PROBE_WRITER_INSTRUCTIONS`:

(a) In step 2 of the protocol, change the phrase `patch the skill files via \` / `Edit/Write, then call \` so it reads `patch the skill files via Read + Write, then call \`. (Remove the `Edit/Write`; use `Read + Write`.)

(b) In the "`rightx-*` skill quality:" bullet list, add a new bullet immediately after the `Body:` bullet:

```
- If the procedure is multi-step with mechanical or disposable-intermediate \
  steps, encode concrete subagent-delegation directives in the steps, naming \
  the model tier (`haiku` for purely mechanical, `sonnet` for mechanical work \
  needing light comprehension). Do NOT add delegation directives to simple \
  single-procedure recipes.
```

(Match the existing `\`-continuation line style used throughout the constant so the Rust string compiles.)

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-codegen probe_writer_instructions_contain_class_first_guidance`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/right-codegen/src/agent_def.rs crates/right-codegen/src/agent_def_tests.rs
git commit --no-verify -m "feat(learning): teach probe-writer to author delegation-aware skills"
```

---

## Task 3: Consolidate `build_user_prompt` to compose from the canonical constants

This is the drift fix: the live runtime prompt stops being an inline duplicate and instead composes `PROBE_WRITER_INSTRUCTIONS` + hint + `hint_outcome` + index + `PROBE_WRITER_ANCHOR_TEMPLATE`, so the delegation directive from Task 2 actually reaches the writer.

**Files:**
- Modify: `crates/bot/src/learning_probe_writer.rs` (`build_user_prompt`, lines 51-99; imports at top)
- Test: `crates/bot/src/learning_probe_writer.rs` (existing `#[cfg(test)]` module, extend assertions)

- [ ] **Step 1: Confirm the bot crate depends on `right-codegen`**

Run: `devenv shell -- cargo tree -p bot -i right-codegen --depth 0`
Expected: prints `right-codegen v...` (bot already depends on it for prompt assembly). If it does NOT, add `right-codegen = { path = "../right-codegen" }` to `crates/bot/Cargo.toml` `[dependencies]` before proceeding.

- [ ] **Step 2: Extend the unit test (failing)**

In `crates/bot/src/learning_probe_writer.rs`, update `build_user_prompt_includes_anchor_instructions_and_index` to also assert the composed canonical content and anchor markers:

```rust
#[tokio::test]
async fn build_user_prompt_includes_anchor_instructions_and_index() {
    let p = build_user_prompt(&anchor("hi", "bye"), "- rightx-foo: bar", &default_hint());
    assert!(p.contains("hi"));
    assert!(p.contains("bye"));
    assert!(p.contains("rightx-foo: bar"));
    assert!(p.contains("hint_outcome"));
    // Composed from the canonical codegen constants (drift fixed).
    assert!(p.contains("Survey"), "must include PROBE_WRITER_INSTRUCTIONS body");
    assert!(p.contains("disposable-intermediate"), "must include delegation directive");
    assert!(p.contains("probe_writer_anchor"), "must include the anchor template markers");
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `devenv shell -- cargo test -p bot build_user_prompt_includes_anchor_instructions_and_index`
Expected: FAIL — current inline prompt has neither `Survey`, `disposable-intermediate`, nor `probe_writer_anchor`.

- [ ] **Step 4: Add the imports**

At the top of `crates/bot/src/learning_probe_writer.rs`, add:

```rust
use right_codegen::{PROBE_WRITER_ANCHOR_TEMPLATE, PROBE_WRITER_INSTRUCTIONS};
```

- [ ] **Step 5: Rewrite `build_user_prompt`**

Replace the entire `build_user_prompt` function (lines 51-99) with the composition below. It (a) prepends `PROBE_WRITER_INSTRUCTIONS`, (b) trims each hint branch to the prefilter recommendation + "verify against the protocol above," keeping the pinned tokens, (c) keeps the `hint_outcome` block and index, (d) appends the substituted `PROBE_WRITER_ANCHOR_TEMPLATE` instead of the ad-hoc `USER:/ASSISTANT:` lines:

```rust
/// Compose the first user-message body delivered to the fork.
///
/// Layered prompt: canonical class-first instructions + quality (incl. the
/// delegation directive) from `right_codegen::PROBE_WRITER_INSTRUCTIONS`, then
/// the prefilter's per-turn hint, the `hint_outcome` reporting contract, the
/// agent's `rightx-*` skill index, and finally the anchored turn rendered from
/// `right_codegen::PROBE_WRITER_ANCHOR_TEMPLATE`.
pub(crate) fn build_user_prompt(
    anchor: &ProbeAnchor,
    skill_index: &str,
    hint: &ProbeWriterHint,
) -> String {
    let user: String = anchor.user_msg_text.chars().take(8000).collect();
    let assistant: String = anchor.assistant_reply_text.chars().take(12000).collect();

    let hint_block = match hint {
        ProbeWriterHint::PatchExisting {
            target_skill,
            reason,
        } => format!(
            "PREFILTER HINT: patch_existing\n\
TARGET SKILL: {target_skill}\n\
REASON: {reason}\n\n\
Verify this recommendation against the protocol above and the anchored turn \
below, then apply it or override it (patch a different skill, create instead, \
or exit silently) as the protocol directs.",
        ),
        ProbeWriterHint::CreateNew { topic_hint, reason } => format!(
            "PREFILTER HINT: create_new\n\
TOPIC HINT: {topic_hint}\n\
REASON: {reason}\n\n\
Verify this recommendation against the protocol above and the anchored turn \
below, then apply it or override it (patch an existing skill instead, or exit \
silently) as the protocol directs.",
        ),
    };

    let index = if skill_index.is_empty() {
        "(no existing rightx-* skills)"
    } else {
        skill_index
    };

    let anchor_rendered = PROBE_WRITER_ANCHOR_TEMPLATE
        .replace("{user_msg_text}", &user)
        .replace("{assistant_reply_text}", &assistant);

    format!(
        "{PROBE_WRITER_INSTRUCTIONS}\n\n\
{hint_block}\n\n\
When you call mcp__right__skill_learning_finish, ALWAYS include the field\n\
\"hint_outcome\" with one of:\n\
  - \"applied_as_hinted\" — you patched/created exactly as the hint suggested.\n\
  - \"applied_differently\" — you took action but not as hinted (e.g. patched a\n\
    different skill, created instead of patched).\n\
  - \"refused\" — you exited without writing because the hint was unjustified.\n\n\
EXISTING SKILLS:\n{index}\n\n{anchor_rendered}"
    )
}
```

- [ ] **Step 6: Run the probe-writer unit tests to verify pass + no regression**

Run: `devenv shell -- cargo test -p bot learning_probe_writer`
Expected: PASS. Specifically still-green: `build_user_prompt_empty_index_uses_placeholder` (placeholder retained), `build_user_prompt_includes_patch_block_for_patch_hint` and `build_user_prompt_includes_create_block_for_create_hint` (tokens `PREFILTER HINT: patch_existing` / `create_new`, `TARGET SKILL:`, `TOPIC HINT:`, `REASON`, `hint_outcome` all retained).

- [ ] **Step 7: Build the bot crate (catch unused-import / type errors)**

Run: `devenv shell -- cargo check -p bot`
Expected: clean (no unused-import warning for the new `use`, since both constants are now referenced).

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/learning_probe_writer.rs
git commit --no-verify -m "refactor(learning): compose probe-writer prompt from canonical constants"
```

---

## Task 4: Add the delegation bullet to `right-learn-skill/SKILL.md`

**Files:**
- Test: `crates/right-codegen/src/skills.rs` (extend `right_learn_skill_mentions_protocol_and_boundaries`, ~line 278)
- Modify: `crates/right-codegen/skills/right-learn-skill/SKILL.md` ("Skill Quality" section, lines 99-112)

- [ ] **Step 1: Extend the test (failing)**

In `crates/right-codegen/src/skills.rs`, add two needles to the `for needle in [ ... ]` array inside `right_learn_skill_mentions_protocol_and_boundaries`:

```rust
            "scripts/",
            "references/",
            "assets/",
            // delegation-authoring directive
            "disposable-intermediate",
            "`haiku`",
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-codegen right_learn_skill_mentions_protocol_and_boundaries`
Expected: FAIL — SKILL.md has neither needle yet.

- [ ] **Step 3: Edit SKILL.md**

In `crates/right-codegen/skills/right-learn-skill/SKILL.md`, in the "## Skill Quality" section, add a bullet immediately after the existing `In SKILL.md, include:` list (after the `- when not to use it` / receipt bullets, before the `Do not store secrets.` line):

```markdown
When the procedure is multi-step with mechanical or disposable-intermediate
steps, encode concrete subagent-delegation directives in the steps, naming the
model tier (`haiku` for purely mechanical work like format conversion or field
extraction, `sonnet` for mechanical work needing light comprehension like long
reads or summarization). Keep simple single-procedure recipes free of
delegation directives.
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-codegen right_learn_skill_mentions_protocol_and_boundaries`
Expected: PASS

- [ ] **Step 5: Run the learn-skill prompt tests (no regression)**

Run: `devenv shell -- cargo test -p right-codegen right_learn_skill`
Expected: PASS (including `right_learn_skill_prompt_uses_explicit_intent_framing` and `learned_skill_prompt_text_has_no_old_or_invalid_prefixes` — the new bullet introduces none of the forbidden `rl-` / `_right-` substrings).

- [ ] **Step 6: Commit**

```bash
git add crates/right-codegen/skills/right-learn-skill/SKILL.md crates/right-codegen/src/skills.rs
git commit --no-verify -m "feat(learning): teach explicit skill writer to author delegation-aware skills"
```

---

## Task 5: Sync `PROMPT_SYSTEM.md`

**Files:**
- Modify: `PROMPT_SYSTEM.md` (probe-writer section ~479-491; and wherever the model ladder / delegation is described, if present)

- [ ] **Step 1: Update the probe-writer section**

In `PROMPT_SYSTEM.md`, in the `### PROBE_WRITER_ANCHOR_TEMPLATE + PROBE_WRITER_INSTRUCTIONS (post-turn probe-writer)` section, make these factual corrections:

- State that `bot::learning_probe_writer::build_user_prompt` **composes** the first user message from `right_codegen::PROBE_WRITER_INSTRUCTIONS` (class-first protocol + skill quality, incl. the delegation directive), then the prefilter hint block, the `hint_outcome` contract, the `rightx-*` skill index, and finally `right_codegen::PROBE_WRITER_ANCHOR_TEMPLATE` (anchored turn). Remove any wording implying the constants are unused or that the prompt is built inline/ad-hoc.
- Add one sentence: the writer authors delegation directives into multi-step skills with mechanical / disposable-intermediate steps, naming the model tier (`haiku`/`sonnet`), per the model ladder in `OPERATING_INSTRUCTIONS.md`; simple recipes stay delegation-free.

- [ ] **Step 2: Update / confirm the model-ladder description**

Search `PROMPT_SYSTEM.md` for an existing description of the subagent model tier (it previously documented only `sonnet`). If present, update it to the three-tier ladder (`haiku` mechanical / `sonnet` light-comprehension / default judgment). If absent, no addition is required — `OPERATING_INSTRUCTIONS.md` is the source of truth.

Run: `rg -n "sonnet|haiku|model ladder|subagent" PROMPT_SYSTEM.md`
(Use the result to decide whether an edit is needed. Remember the grep token-mangle caveat — confirm with `Read` if output looks odd.)

- [ ] **Step 3: Commit**

```bash
git add PROMPT_SYSTEM.md
git commit --no-verify -m "docs(prompt): document probe-writer consolidation and delegation ladder"
```

---

## Task 6: Final full-workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full workspace test suite (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. If pre-existing flakes appear (per project notes, a `cc/invocation` pid race and a dashboard warn-count test can flake under parallel load), re-run those isolated before attributing failure to this change:
`devenv shell -- cargo test -p bot <flaky_name> -- --exact`

- [ ] **Step 2: Build the workspace (debug)**

Run: `devenv shell -- cargo build --workspace`
Expected: clean build.

- [ ] **Step 3: Final commit (only if Step 1/2 produced fmt or incidental changes)**

```bash
git status --short
# If nothing to commit, skip. Otherwise:
git add -A && git commit --no-verify -m "chore(learning): finalize delegation-aware skill writer"
```

---

## Self-Review (completed by plan author)

**Spec coverage:**
- Haiku rung in canonical ladder → Task 1. ✔
- Consolidate `build_user_prompt` from canonical constants (Option 2 drift fix) → Task 3. ✔
- Delegation directive in `PROBE_WRITER_INSTRUCTIONS` → Task 2. ✔
- `Edit/Write` → `Read + Write` latent-bug fix → Task 2. ✔
- Delegation bullet in `right-learn-skill/SKILL.md` → Task 4. ✔
- Hard gate against bloat (simple recipes get no directive) → wording in Tasks 2 & 4. ✔
- `PROMPT_SYSTEM.md` sync → Task 5. ✔
- Tests for all four content surfaces → Tasks 1-4. ✔
- Targeted-then-final verification cadence → per-task targeted + Task 6 full workspace. ✔

**Placeholder scan:** no TBD/TODO; every code/edit step shows exact content. ✔

**Type/identifier consistency:** `PROBE_WRITER_ANCHOR_TEMPLATE` / `PROBE_WRITER_INSTRUCTIONS` (codegen constants), `build_user_prompt` signature unchanged `(&ProbeAnchor, &str, &ProbeWriterHint) -> String`, hint tokens preserved verbatim across Task 3 and the existing pinned tests. ✔
