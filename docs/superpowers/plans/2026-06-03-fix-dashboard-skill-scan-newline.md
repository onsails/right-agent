# Fix Dashboard Skill-Scan Newline Rejection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the dashboard "Skills" tab work for sandboxed agents by rewriting two multi-line shell-script constants as single-line scripts, so OpenShell's `ExecSandbox` no longer rejects them.

**Architecture:** OpenShell's gRPC `ExecSandbox` server rejects any command argument that contains a real newline (`\n`, 0x0A) or carriage-return (`\r`, 0x0D) byte. The dashboard passes each script as a single `sh -c <script>` argument. Two constants in `crates/bot/src/telegram/dashboard/skills.rs` are `r#"..."#` raw strings with real newline bytes between statements, so every scan fails with `command argument 2 contains newline or carriage return characters`. The fix joins the statements with `;` into a single physical line (no real newline bytes), mirroring the already-working `SANDBOX_SKILL_DUMP_CMD` in `crates/bot/src/learning_prefilter.rs:330`. Pure shell-string change — no transport, OpenShell, or control-flow change. The `printf '%s\n'` newline stays as the two-char escaped `\n` it already is (a backslash + `n` handed to `printf` at runtime), never a real newline byte.

**Tech Stack:** Rust (edition 2024), `cargo`/`devenv`, OpenShell gRPC (`right-openshell`), POSIX `sh`.

---

## Background / current state

- Bug root-caused already. The two offending constants:
  - `SANDBOX_LIST_SKILLS_SCRIPT` — `crates/bot/src/telegram/dashboard/skills.rs:18-28` (used by `scan_sandbox_skills`, the path that produces the reported error).
  - `SANDBOX_READ_SKILL_SCRIPT` — `crates/bot/src/telegram/dashboard/skills.rs:29-34` (used by `read_sandbox_skill_detail`).
- An **ignored live repro test already exists** (RED): `ci_openshell_dashboard_skill_scripts_have_no_newline_args` in the `ci_sandbox_tests` module at the bottom of `skills.rs`. It creates a live sandbox and runs both constants through `SandboxExec::exec`, asserting the list script exits 0 and the read script exits 3. It currently fails with the newline rejection and will go GREEN after this fix. Do not modify it.
- `probe_sandbox_skill_package` (pin path) uses an inline single-line `test -f "$1"` and is **not** affected — leave it unchanged.
- Package name is `right-bot` (directory `crates/bot/`). Prefix all commands with `devenv shell --`. Never invoke bare `right`.

## Scope / non-goals

- **In scope:** rewrite the two constants single-line; add one fast (non-sandbox) unit guard so the regression can't return without a live OpenShell run; confirm both the new guard and the existing ignored live repro pass.
- **Out of scope (YAGNI):** no defensive newline check inside `exec_in_sandbox_once` (shared low-level fn — not this bug's home); no behavior change to the scripts' logic; no ARCHITECTURE.md change (these constants are an implementation detail, not a contract). `PROMPT_SYSTEM.md` is unaffected (no prompt/tool surface changes).

## File Structure

- Modify: `crates/bot/src/telegram/dashboard/skills.rs`
  - Lines 18-34: the two constant definitions (rewrite single-line).
  - Append one new `#[cfg(test)] mod script_constants_tests` (fast, no sandbox) for the regression guard.
- No other files. No new dependencies (`right-openshell` `test-support` dev-dep already present; the new guard needs nothing beyond `super::`).

---

### Task 1: Fast regression guard (no sandbox) — RED first

A pure-Rust unit test that asserts neither script constant contains a real newline/CR byte. It runs in the normal `cargo test` suite (no OpenShell needed), so CI catches a reintroduced multi-line script without a live sandbox. Currently RED because the constants still contain real newlines.

**Files:**
- Modify (append at end of file): `crates/bot/src/telegram/dashboard/skills.rs`

- [ ] **Step 1: Write the failing guard test**

Append to the end of `crates/bot/src/telegram/dashboard/skills.rs` (after the existing `ci_sandbox_tests` module):

```rust
#[cfg(test)]
mod script_constants_tests {
    use super::{SANDBOX_LIST_SKILLS_SCRIPT, SANDBOX_READ_SKILL_SCRIPT};

    /// OpenShell's gRPC `ExecSandbox` rejects any command argument that
    /// contains a real newline or carriage-return byte. These scripts are
    /// passed as a single `sh -c <script>` argument, so they MUST stay
    /// single-line. The only `\n` they may carry is the two-char escaped
    /// `\n` handed to `printf` (a backslash followed by `n`), never a real
    /// 0x0A byte. This fast guard fails without needing a live sandbox, so a
    /// reintroduced multi-line script is caught in the normal test suite.
    #[test]
    fn dashboard_skill_scripts_have_no_real_newline_bytes() {
        for (name, script) in [
            ("SANDBOX_LIST_SKILLS_SCRIPT", SANDBOX_LIST_SKILLS_SCRIPT),
            ("SANDBOX_READ_SKILL_SCRIPT", SANDBOX_READ_SKILL_SCRIPT),
        ] {
            assert!(
                !script.contains('\n') && !script.contains('\r'),
                "{name} contains a real newline/CR byte; OpenShell ExecSandbox \
                 rejects command arguments with newlines — keep it single-line \
                 (`;`-joined statements; use \\\\n for printf)"
            );
        }
    }
}
```

- [ ] **Step 2: Run the guard to verify it FAILS**

Run: `devenv shell -- cargo test -p right-bot script_constants_tests::dashboard_skill_scripts_have_no_real_newline_bytes`
Expected: FAIL — assertion panics, e.g. `SANDBOX_LIST_SKILLS_SCRIPT contains a real newline/CR byte; ...`. (If it passes here, the constants were already changed — stop and reconcile before continuing.)

---

### Task 2: Rewrite both constants single-line — make Task 1 GREEN

**Files:**
- Modify: `crates/bot/src/telegram/dashboard/skills.rs:18-34`

- [ ] **Step 1: Replace `SANDBOX_LIST_SKILLS_SCRIPT`**

Replace the existing definition (currently the `r#"..."#` raw multi-line block at `skills.rs:18-28`):

```rust
const SANDBOX_LIST_SKILLS_SCRIPT: &str = r#"cd "$1" 2>/dev/null || exit 0
limit="$2"
count=0
for d in *; do
  [ -d "$d" ] || continue
  [ -L "$d/SKILL.md" ] && continue
  [ -f "$d/SKILL.md" ] || continue
  printf '%s\n' "$d"
  count=$((count + 1))
  [ "$count" -ge "$limit" ] && break
done"#;
```

with this single-line form (Rust `\`-continuations strip the source newlines + indentation; the space before each `\` preserves the statement separator):

```rust
const SANDBOX_LIST_SKILLS_SCRIPT: &str = "cd \"$1\" 2>/dev/null || exit 0; \
     limit=\"$2\"; \
     count=0; \
     for d in *; do \
     [ -d \"$d\" ] || continue; \
     [ -L \"$d/SKILL.md\" ] && continue; \
     [ -f \"$d/SKILL.md\" ] || continue; \
     printf '%s\\n' \"$d\"; \
     count=$((count + 1)); \
     [ \"$count\" -ge \"$limit\" ] && break; \
     done";
```

The resulting string value (no real newline bytes) is:
`cd "$1" 2>/dev/null || exit 0; limit="$2"; count=0; for d in *; do [ -d "$d" ] || continue; [ -L "$d/SKILL.md" ] && continue; [ -f "$d/SKILL.md" ] || continue; printf '%s\n' "$d"; count=$((count + 1)); [ "$count" -ge "$limit" ] && break; done`

- [ ] **Step 2: Replace `SANDBOX_READ_SKILL_SCRIPT`**

Replace the existing definition (`skills.rs:29-34`):

```rust
const SANDBOX_READ_SKILL_SCRIPT: &str = r#"cd "$1" 2>/dev/null || exit 3
file="$2/SKILL.md"
[ -e "$file" ] || exit 3
[ -L "$file" ] && exit 3
[ -f "$file" ] || exit 3
head -c "$3" "$file""#;
```

with:

```rust
const SANDBOX_READ_SKILL_SCRIPT: &str = "cd \"$1\" 2>/dev/null || exit 3; \
     file=\"$2/SKILL.md\"; \
     [ -e \"$file\" ] || exit 3; \
     [ -L \"$file\" ] && exit 3; \
     [ -f \"$file\" ] || exit 3; \
     head -c \"$3\" \"$file\"";
```

The resulting string value is:
`cd "$1" 2>/dev/null || exit 3; file="$2/SKILL.md"; [ -e "$file" ] || exit 3; [ -L "$file" ] && exit 3; [ -f "$file" ] || exit 3; head -c "$3" "$file"`

- [ ] **Step 3: Run the fast guard to verify it now PASSES**

Run: `devenv shell -- cargo test -p right-bot script_constants_tests::dashboard_skill_scripts_have_no_real_newline_bytes`
Expected: PASS (`test result: ok. 1 passed`).

- [ ] **Step 4: Compile-check the crate (no behavior surprises)**

Run: `devenv shell -- cargo test -p right-bot --no-run`
Expected: builds cleanly (the escaped string literals compile; no warnings about the changed constants).

- [ ] **Step 5: Commit the test + fix together**

```bash
git add crates/bot/src/telegram/dashboard/skills.rs
git commit -m "fix(dashboard): single-line sandbox skill-scan scripts so OpenShell ExecSandbox accepts them

OpenShell's ExecSandbox rejects command arguments containing newline/CR
bytes. SANDBOX_LIST_SKILLS_SCRIPT and SANDBOX_READ_SKILL_SCRIPT were
multi-line raw strings passed as a single sh -c argument, so the
dashboard Skills tab failed on sandboxed agents with 'command argument 2
contains newline or carriage return characters'. Join the statements
single-line (mirroring learning_prefilter::SANDBOX_SKILL_DUMP_CMD); add a
fast unit guard against reintroduction."
```

---

### Task 3: Confirm the live repro is GREEN and run the final gate

**Files:** none (verification only).

- [ ] **Step 1: Run the existing ignored live repro (dev machine has OpenShell)**

Run: `devenv shell -- cargo test -p right-bot ci_openshell_dashboard_skill_scripts -- --ignored --nocapture`
Expected: PASS. The list script returns exit 0, the read script returns exit 3, and neither exec is rejected. (Before this fix it failed with `command argument 2 contains newline or carriage return characters`.)

- [ ] **Step 2: Run the ignored-test contract gate (unchanged, must still pass)**

Run: `devenv shell -- cargo test -p right --test ci_ignored_contract`
Expected: PASS (`ci_ignored_tests_have_workspace_filterable_names ... ok`).

- [ ] **Step 3: Final full workspace gate (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Note: the ignored live repro does not run here (no `--ignored`), so the workspace run is fully green. If pre-existing flaky tests trip (see project memory: cc/invocation pid race, dashboard warn-count), re-run those isolated before blaming this change.

- [ ] **Step 4: Commit the plan doc**

```bash
git add docs/superpowers/plans/2026-06-03-fix-dashboard-skill-scan-newline.md
git commit -m "docs(plan): dashboard skill-scan newline fix plan"
```

---

## Self-Review

- **Spec coverage:** The bug is exactly the two multi-line constants → Task 2 rewrites both. The "can't silently regress without a live sandbox" requirement → Task 1 fast guard. The "prove end-to-end behavior" requirement → existing ignored live repro, confirmed GREEN in Task 3. ✓
- **Placeholder scan:** No TBD/TODO; every code step shows the full replacement text and the resulting string value. ✓
- **Type/name consistency:** Constant names (`SANDBOX_LIST_SKILLS_SCRIPT`, `SANDBOX_READ_SKILL_SCRIPT`), module names (`script_constants_tests`, existing `ci_sandbox_tests`), test names, and the package name (`right-bot`) are used consistently across tasks. ✓
- **Shell correctness:** Both rewritten scripts are POSIX `sh`; `for ... do ... done` and the `[ ... ] && continue` / `|| exit N` guards keep the original semantics. Empty/missing skills dir → list exits 0; absent `SKILL.md` → read exits 3, matching the existing live repro's assertions. ✓
