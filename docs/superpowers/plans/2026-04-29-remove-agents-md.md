# Remove AGENTS.md Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete `AGENTS.md` end-to-end from the Right Agent platform — template, codegen, prompt assembly, reverse sync, types, tests, skills and docs — leaving zero artifacts.

**Architecture:** `AGENTS.md` is removed as a prompt section, as a tracked agent-owned file, and as a field on `AgentDef`. The five-file identity surface (`IDENTITY.md`, `SOUL.md`, `USER.md`, `AGENTS.md`, `TOOLS.md`) shrinks to four. Subagent definitions remain CC-native via `.claude/agents/<name>.md` (description-based dispatch). Existing on-disk `AGENTS.md` files in deployed agents are not auto-deleted (per the project's "AgentOwned files codegen never touches" rule) — they simply stop being loaded into the prompt and stop being reverse-synced. Operators or agents themselves can `rm` the dead file at their leisure.

**Tech Stack:** Rust 2024 edition, Cargo workspace (`right-agent`, `right`, `bot`), `cargo test`, `cargo build --workspace`. Markdown for templates, skills, docs.

**Rationale (from conversation 2026-04-29):**
- The template (`templates/right/agent/AGENTS.md`) is an empty placeholder with three stub sections. Subagent listing duplicates `.claude/agents/*.md` (CC's own discovery). Skill index duplicates `skills/installed.json`. "Task Routing" is prose without a backing mechanism.
- OpenClaw's `AGENTS.md` carries behavioural policy (memory rules, group-chat etiquette, red lines) — content that in Right Agent is already split into `SOUL.md`, `OPERATING_INSTRUCTIONS.md` (compiled-in), and per-skill `SKILL.md`. Our `AGENTS.md` is left with no unique content.
- Hermes has no `AGENTS.md` slot at all and uses an LLM-invoked `delegate_task` tool for delegation — confirming the file-as-routing-table approach is not load-bearing.
- Hindsight memory is **not** the replacement (it's a different jam: stochastic recall vs deterministic prompt injection); the file is removed because its content is empty/duplicated, not because memory absorbs it.

---

## File Structure

Files modified or deleted by this plan, grouped by responsibility.

### Compiled-in content (effective on bot rebuild)

| Path | Action |
|---|---|
| `templates/right/agent/AGENTS.md` | **delete** |
| `templates/right/agent/BOOTSTRAP.md` | edit — drop AGENTS.md row from self-config table |
| `templates/right/prompt/OPERATING_INSTRUCTIONS.md` | edit — drop three AGENTS.md references |
| `skills/rightmemory-file/SKILL.md` | edit — drop AGENTS.md from "where things go" |
| `skills/rightmemory-hindsight/SKILL.md` | edit — drop AGENTS.md from "where things go" |

### `right-agent` crate (core)

| Path | Action |
|---|---|
| `crates/right-agent/src/init.rs` | edit — drop `DEFAULT_AGENTS` const, files-array entry, doc comment, `agents/right/AGENTS.md` print line, `agents_path: None` literal in `trust_agent`, init-test assertion |
| `crates/right-agent/src/agent/types.rs` | edit — drop `agents_path` field from `AgentDef` |
| `crates/right-agent/src/agent/discovery.rs` | edit — drop two `agents_path:` assignments |
| `crates/right-agent/src/agent/discovery_tests.rs` | edit — drop AGENTS.md write + `assert!(a.agents_path.is_some())` |
| `crates/right-agent/src/agent/destroy.rs` | edit — replace fixture write of AGENTS.md with IDENTITY.md |
| `crates/right-agent/src/codegen/process_compose_tests.rs` | edit — drop six `agents_path: None,` literals |
| `crates/right-agent/src/codegen/claude_json.rs` | edit — drop two `agents_path: None,` literals |
| `crates/right-agent/src/codegen/telegram.rs` | edit — drop one `agents_path: None,` literal |
| `crates/right-agent/src/platform_store.rs` | edit — drop AGENTS.md from doc comment |
| `crates/right-agent/src/platform_store_tests.rs` | edit — drop AGENTS.md from `agent_owned` test array |
| `crates/right-agent/src/doctor_tests.rs` | edit — replace AGENTS.md fixture writes with IDENTITY.md or remove (per test) |

### `right` crate (CLI binary)

| Path | Action |
|---|---|
| `crates/right/src/main.rs` | edit — drop `agents_path:` literals at L1494/L1953, drop AGENTS.md tuple entry at L4190 |
| `crates/right/tests/cli_integration.rs` | edit — drop AGENTS.md existence assertions; replace fixture writes |

### `bot` crate (Telegram bot)

| Path | Action |
|---|---|
| `crates/bot/src/telegram/prompt.rs` | edit — drop `PROMPT_SECTIONS` entry; drop `AGENTS.md` test assertion |
| `crates/bot/src/telegram/worker.rs` | edit — delete `script_normal_has_agent_configuration_section` test |
| `crates/bot/src/sync.rs` | edit — drop `"AGENTS.md"` from `REVERSE_SYNC_FILES` |

### Architecture docs

| Path | Action |
|---|---|
| `PROMPT_SYSTEM.md` | edit — drop three AGENTS.md rows |
| `ARCHITECTURE.md` | edit — drop AGENTS.md from lifecycle write list, prompting-architecture file list, config hierarchy table, AgentOwned examples list |

---

## Pre-flight

- [ ] **Step 0: Create a worktree for this plan**

```bash
git -C /Users/molt/dev/rightclaw worktree add -b remove-agents-md .worktrees/remove-agents-md master
cd /Users/molt/dev/rightclaw/.worktrees/remove-agents-md
```

Expected: worktree at `.worktrees/remove-agents-md` on branch `remove-agents-md`. All subsequent steps use this directory.

---

### Task 1: Update compiled-in markdown (templates + skills)

These files are pulled into the binary via `include_str!` / `include_dir!`. They must be updated *before* prompt-assembly code drops the AGENTS.md section, otherwise the agent will be told "AGENTS.md exists" while the runtime no longer reads it. Doing them first keeps the agent-visible state monotonically consistent.

**Files:**
- Modify: `templates/right/prompt/OPERATING_INSTRUCTIONS.md:21,31,51`
- Modify: `templates/right/agent/BOOTSTRAP.md:43`
- Modify: `skills/rightmemory-file/SKILL.md:23,36`
- Modify: `skills/rightmemory-hindsight/SKILL.md:35,49,68`

- [ ] **Step 1.1: Edit `OPERATING_INSTRUCTIONS.md`**

In `templates/right/prompt/OPERATING_INSTRUCTIONS.md`, delete line 21:

```
- `AGENTS.md` — your subagents, task routing, installed skills.
```

Delete the table row at line 31:

```
| "Subagent `reviewer` handles code review" | `AGENTS.md` |
```

Delete the bullet at line 51:

```
- Subagent / task-routing rules → `AGENTS.md`
```

After edits, the "Your Files" list ends at `TOOLS.md`. The "Where things go" table no longer mentions subagent routing. The "Do NOT save to memory" list omits the subagent-routing bullet.

- [ ] **Step 1.2: Edit `BOOTSTRAP.md`**

In `templates/right/agent/BOOTSTRAP.md`, delete the table row at line 43:

```
| Add/remove capabilities, subagents, tools, skills | `AGENTS.md` |
```

The two adjacent rows (`SOUL.md` for tone, `IDENTITY.md` for principles) remain. Subagent management is implicitly via `.claude/agents/<name>.md` files (CC-native).

- [ ] **Step 1.3: Edit `skills/rightmemory-file/SKILL.md`**

Replace the file list at line 23:

```
`AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`):
```

with:

```
`IDENTITY.md`, `SOUL.md`, `USER.md`):
```

Delete the bullet at line 36:

```
- Subagent routing → `AGENTS.md`
```

- [ ] **Step 1.4: Edit `skills/rightmemory-hindsight/SKILL.md`**

Replace the file list at line 35:

```
(`TOOLS.md`, `AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`):
```

with:

```
(`TOOLS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`):
```

Delete the bullet at line 49:

```
- Subagent routing → `AGENTS.md`
```

Delete the bullet at line 68:

```
- You just learned a subagent's responsibility → `AGENTS.md`
```

- [ ] **Step 1.5: Verify no AGENTS.md remaining in compiled-in markdown**

```bash
rg -n "AGENTS\.md" templates/ skills/
```

Expected: no output. If anything matches, fix it before continuing.

- [ ] **Step 1.6: Build to confirm `include_str!` still resolves**

```bash
cargo build --workspace
```

Expected: clean build. (Templates are read at compile time; if any path in `include_str!` is broken, the build fails here.)

- [ ] **Step 1.7: Commit**

```bash
git add templates/right/prompt/OPERATING_INSTRUCTIONS.md templates/right/agent/BOOTSTRAP.md skills/rightmemory-file/SKILL.md skills/rightmemory-hindsight/SKILL.md
git commit -m "$(cat <<'EOF'
docs(prompt): drop AGENTS.md from compiled-in templates and skills

AGENTS.md is being removed from Right Agent's identity surface.
This step removes it from templates and skill docs so the agent-visible
narrative no longer references it.
EOF
)"
```

---

### Task 2: Drop AGENTS.md from prompt assembly

Stop loading `AGENTS.md` into the composite system prompt. Two callsites: the bot worker (`telegram/prompt.rs::PROMPT_SECTIONS`) and the CLI cron-debug command (`right/src/main.rs` prompt assembly tuple). Tests that assert on AGENTS.md presence are updated.

**Files:**
- Modify: `crates/bot/src/telegram/prompt.rs:34-46`
- Modify: `crates/bot/src/telegram/prompt.rs:272-288` (test)
- Modify: `crates/bot/src/telegram/worker.rs:2281-2301` (test)
- Modify: `crates/right/src/main.rs:4186-4198`

- [ ] **Step 2.1: Edit `prompt.rs` — drop AGENTS.md from `PROMPT_SECTIONS`**

In `crates/bot/src/telegram/prompt.rs`, delete the entry at lines 38-41:

```rust
    PromptSection {
        filename: "AGENTS.md",
        header: "## Agent Configuration",
    },
```

The remaining entries are `IDENTITY.md`, `SOUL.md`, `USER.md`, `TOOLS.md` (four).

- [ ] **Step 2.2: Edit `prompt.rs` — drop AGENTS.md test assertion**

In the same file, the test at line 272 (`script_normal_includes_all_identity_files`) contains:

```rust
        assert!(script.contains("AGENTS.md"));
```

Delete this line (line 278). Keep the four remaining `assert!(script.contains(...))` lines for IDENTITY/SOUL/USER/TOOLS.

- [ ] **Step 2.3: Delete `script_normal_has_agent_configuration_section` test**

In `crates/bot/src/telegram/worker.rs`, delete the entire test starting at line 2281 (function `script_normal_has_agent_configuration_section`) through its closing `}`. The test asserts on `"Agent Configuration"` and `"cat /sandbox/AGENTS.md"`, both gone after this plan.

- [ ] **Step 2.4: Edit `main.rs` cron-debug prompt assembly**

In `crates/right/src/main.rs`, the prompt assembly loop at lines 4186-4198:

```rust
    for (file, header) in [
        ("IDENTITY.md", "## Your Identity"),
        ("SOUL.md", "## Your Personality and Values"),
        ("USER.md", "## Your User"),
        ("AGENTS.md", "## Agent Configuration"),
        ("TOOLS.md", "## Environment and Tools"),
    ] {
```

Delete the AGENTS.md tuple entry. The loop becomes:

```rust
    for (file, header) in [
        ("IDENTITY.md", "## Your Identity"),
        ("SOUL.md", "## Your Personality and Values"),
        ("USER.md", "## Your User"),
        ("TOOLS.md", "## Environment and Tools"),
    ] {
```

- [ ] **Step 2.5: Build + test**

```bash
cargo build --workspace
cargo test -p bot --lib telegram::prompt
cargo test -p bot --lib telegram::worker
```

Expected: build clean. Both `script_normal_includes_all_identity_files` and any other prompt tests pass; the deleted `script_normal_has_agent_configuration_section` is gone.

- [ ] **Step 2.6: Commit**

```bash
git add crates/bot/src/telegram/prompt.rs crates/bot/src/telegram/worker.rs crates/right/src/main.rs
git commit -m "$(cat <<'EOF'
feat(prompt): drop AGENTS.md section from composite system prompt

AGENTS.md is no longer assembled into the system prompt by either the
bot worker or the cron-debug CLI path. Tests asserting its presence
are removed.
EOF
)"
```

---

### Task 3: Drop AGENTS.md from reverse sync

Stop downloading `AGENTS.md` from the sandbox after each `claude -p` turn.

**Files:**
- Modify: `crates/bot/src/sync.rs:73-76`

- [ ] **Step 3.1: Edit `REVERSE_SYNC_FILES`**

In `crates/bot/src/sync.rs`, the constant at line 76:

```rust
const REVERSE_SYNC_FILES: &[&str] = &["AGENTS.md", "TOOLS.md", "IDENTITY.md", "SOUL.md", "USER.md"];
```

Becomes:

```rust
const REVERSE_SYNC_FILES: &[&str] = &["TOOLS.md", "IDENTITY.md", "SOUL.md", "USER.md"];
```

- [ ] **Step 3.2: Build + test**

```bash
cargo build --workspace
cargo test -p bot --lib sync
```

Expected: clean build. Sync tests (if any touch AGENTS.md) pass — review failures and update if a test expected the file in `REVERSE_SYNC_FILES`.

- [ ] **Step 3.3: Commit**

```bash
git add crates/bot/src/sync.rs
git commit -m "$(cat <<'EOF'
feat(sync): drop AGENTS.md from reverse-sync allowlist

AGENTS.md is no longer pulled from sandbox to host after CC turns.
Existing on-disk AGENTS.md files (in deployed agents and sandboxes)
are left untouched as dead bytes per the AgentOwned-files convention.
EOF
)"
```

---

### Task 4: Drop AGENTS.md from agent init

Stop creating `AGENTS.md` in fresh agent directories. Delete the template file. Update init-flow tests that assert on its presence.

**Files:**
- Delete: `templates/right/agent/AGENTS.md`
- Modify: `crates/right-agent/src/init.rs:27,34,75,287,664-665`

- [ ] **Step 4.1: Delete the template file**

```bash
git rm templates/right/agent/AGENTS.md
```

Expected: file removed from index and worktree.

- [ ] **Step 4.2: Edit `init.rs` — drop `DEFAULT_AGENTS` const**

In `crates/right-agent/src/init.rs`, delete line 27:

```rust
const DEFAULT_AGENTS: &str = include_str!("../templates/right/agent/AGENTS.md");
```

The constants `DEFAULT_BOOTSTRAP`, `DEFAULT_TOOLS`, `DEFAULT_AGENT_YAML` remain.

- [ ] **Step 4.3: Edit `init.rs` — update doc comment**

The doc comment at line 34 currently reads:

```rust
/// Creates the agent directory with template files (AGENTS.md, BOOTSTRAP.md,
/// agent.yaml), installs built-in skills, generates
```

Update to:

```rust
/// Creates the agent directory with template files (TOOLS.md, BOOTSTRAP.md,
/// agent.yaml), installs built-in skills, generates
```

(`TOOLS.md` is the closest analogue — agent-owned, written at init.)

- [ ] **Step 4.4: Edit `init.rs` — drop AGENTS.md from files array**

The slice at lines 74-79:

```rust
    let files: &[(&str, &str)] = &[
        ("AGENTS.md", DEFAULT_AGENTS),
        ("BOOTSTRAP.md", DEFAULT_BOOTSTRAP),
        ("TOOLS.md", DEFAULT_TOOLS),
        ("agent.yaml", DEFAULT_AGENT_YAML),
    ];
```

Becomes:

```rust
    let files: &[(&str, &str)] = &[
        ("BOOTSTRAP.md", DEFAULT_BOOTSTRAP),
        ("TOOLS.md", DEFAULT_TOOLS),
        ("agent.yaml", DEFAULT_AGENT_YAML),
    ];
```

- [ ] **Step 4.5: Edit `init.rs` — drop AGENTS.md print line**

In `init_right_home`, delete line 287:

```rust
    println!("  agents/right/AGENTS.md");
```

The remaining prints (BOOTSTRAP.md, TOOLS.md, agent.yaml, skills) are unchanged.

- [ ] **Step 4.6: Edit `init.rs` — drop AGENTS.md test assertion**

Find the test that asserts the file was created (around line 664-665):

```rust
        assert!(agents_dir.join("AGENTS.md").exists());
```

Delete this line. Adjacent assertions for BOOTSTRAP.md, TOOLS.md remain.

- [ ] **Step 4.7: Build + test**

```bash
cargo build --workspace
cargo test -p right-agent --lib init
```

Expected: clean build. Init tests pass, no longer asserting on AGENTS.md.

- [ ] **Step 4.8: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(init): stop emitting AGENTS.md template on agent init

Removes the AGENTS.md template, its DEFAULT_AGENTS const, the
init files-array entry, the post-init print line, and the related
test assertion. New agents are no longer initialized with AGENTS.md.
EOF
)"
```

---

### Task 5: Remove `agents_path` from `AgentDef`

The struct field is one read site (a discovery test) and ten write sites (init, codegen tests, CLI). Drop the field; drop all assignments and the one read.

**Files:**
- Modify: `crates/right-agent/src/agent/types.rs:433-434`
- Modify: `crates/right-agent/src/agent/discovery.rs:101,162`
- Modify: `crates/right-agent/src/agent/discovery_tests.rs:189,199`
- Modify: `crates/right-agent/src/init.rs:230`
- Modify: `crates/right/src/main.rs:1494-1498,1953-1957`
- Modify: `crates/right-agent/src/codegen/process_compose_tests.rs:50,86,101,137,430,604`
- Modify: `crates/right-agent/src/codegen/claude_json.rs:142,371`
- Modify: `crates/right-agent/src/codegen/telegram.rs:58`

- [ ] **Step 5.1: Drop the `agents_path` field from `AgentDef`**

In `crates/right-agent/src/agent/types.rs` at lines 433-434:

```rust
    /// Path to AGENTS.md if present.
    pub agents_path: Option<PathBuf>,
```

Delete both lines. Adjacent fields (`user_path`, `tools_path`) remain.

- [ ] **Step 5.2: Drop `agents_path:` assignments in `discovery.rs`**

In `crates/right-agent/src/agent/discovery.rs`, delete line 101:

```rust
        agents_path: optional_file(agent_dir, "AGENTS.md"),
```

And delete line 162:

```rust
            agents_path: optional_file(&path, "AGENTS.md"),
```

- [ ] **Step 5.3: Drop AGENTS.md fixture + assertion in `discovery_tests.rs`**

In `crates/right-agent/src/agent/discovery_tests.rs`:

Delete line 189:

```rust
    fs::write(agent_dir.join("AGENTS.md"), "agents").unwrap();
```

Delete line 199:

```rust
    assert!(a.agents_path.is_some());
```

The test `discover_detects_optional_files` continues to verify SOUL/USER/TOOLS/BOOTSTRAP/HEARTBEAT.

- [ ] **Step 5.4: Drop `agents_path: None,` from `init.rs:230`**

In `crates/right-agent/src/init.rs`, the `trust_agent` literal at lines 222-234:

```rust
    let trust_agent = crate::agent::AgentDef {
        name: name.to_owned(),
        path: agents_dir.clone(),
        identity_path: agents_dir.join("IDENTITY.md"),
        config: None,
        soul_path: None,
        user_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    };
```

Delete the `agents_path: None,` line.

- [ ] **Step 5.5: Drop `agents_path:` literals from `main.rs`**

In `crates/right/src/main.rs`, the literal at lines 1494-1498:

```rust
            agents_path: if agent_dir.join("AGENTS.md").exists() {
                Some(agent_dir.join("AGENTS.md"))
            } else {
                None
            },
```

Delete all five lines. Repeat for the duplicate at lines 1953-1957.

- [ ] **Step 5.6: Drop `agents_path: None,` from `process_compose_tests.rs`**

In `crates/right-agent/src/codegen/process_compose_tests.rs`, delete the `agents_path: None,` line at each of: 50, 86, 101, 137, 430, 604. Use `rg -n "agents_path: None," crates/right-agent/src/codegen/process_compose_tests.rs` to confirm zero matches after editing.

- [ ] **Step 5.7: Drop `agents_path: None,` from `claude_json.rs`**

In `crates/right-agent/src/codegen/claude_json.rs`, delete the `agents_path: None,` line at lines 142 and 371.

- [ ] **Step 5.8: Drop `agents_path: None,` from `telegram.rs`**

In `crates/right-agent/src/codegen/telegram.rs`, delete the `agents_path: None,` line at line 58.

- [ ] **Step 5.9: Verify no `agents_path` remaining**

```bash
rg -n "agents_path" crates/
```

Expected: no output.

- [ ] **Step 5.10: Build + test**

```bash
cargo build --workspace
cargo test --workspace
```

Expected: clean build, all tests pass. Compilation errors here mean a literal `AgentDef { … }` or read of `.agents_path` was missed — `rg` and fix.

- [ ] **Step 5.11: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
refactor(agent-def): drop agents_path field

Removes AgentDef.agents_path and every literal/assignment/read of it
across right-agent, right (CLI), and bot test fixtures.
EOF
)"
```

---

### Task 6: Update doctor + platform_store + destroy fixtures

Several tests use AGENTS.md as a generic "this agent has a markdown file" placeholder. Replace with IDENTITY.md (the canonical present-after-bootstrap file) or drop the write if not load-bearing.

**Files:**
- Modify: `crates/right-agent/src/platform_store.rs:77`
- Modify: `crates/right-agent/src/platform_store_tests.rs:113-119`
- Modify: `crates/right-agent/src/doctor_tests.rs:59,93,111-112,532,732,758,777`
- Modify: `crates/right-agent/src/agent/destroy.rs:383`

- [ ] **Step 6.1: Update `platform_store.rs` doc comment**

In `crates/right-agent/src/platform_store.rs:77`:

```rust
/// Excludes agent-owned files (IDENTITY.md, SOUL.md, USER.md, AGENTS.md, TOOLS.md).
```

Becomes:

```rust
/// Excludes agent-owned files (IDENTITY.md, SOUL.md, USER.md, TOOLS.md).
```

- [ ] **Step 6.2: Drop AGENTS.md from `platform_store_tests.rs:117`**

In `crates/right-agent/src/platform_store_tests.rs`, the `agent_owned` array at lines 113-119:

```rust
    let agent_owned: &[&str] = &[
        "IDENTITY.md",
        "SOUL.md",
        "USER.md",
        "AGENTS.md",
        "TOOLS.md",
    ];
```

Becomes:

```rust
    let agent_owned: &[&str] = &[
        "IDENTITY.md",
        "SOUL.md",
        "USER.md",
        "TOOLS.md",
    ];
```

- [ ] **Step 6.3: Replace AGENTS.md fixture writes in `doctor_tests.rs`**

In `crates/right-agent/src/doctor_tests.rs`, AGENTS.md appears as a fixture file in seven places. Each is a fixture write of the form `std::fs::write(agent_dir.join("AGENTS.md"), "# Agents").unwrap();` whose intent is "this is a non-empty agent dir".

For each occurrence, decide based on the surrounding test:

- **Line 59 (`run_doctor_with_valid_agent_reports_pass`):** AGENTS.md is the only "config" file alongside IDENTITY/SOUL/USER. Drop the AGENTS.md write — the other three are sufficient to make doctor report a valid agent.
- **Line 93 (`run_doctor_reports_bootstrap_pending`):** AGENTS.md is paired with BOOTSTRAP.md. Drop the AGENTS.md write — the test is about BOOTSTRAP.md, AGENTS.md is incidental.
- **Lines 111-112 (`run_doctor_reports_missing_identity`):** Comment "No IDENTITY.md — only AGENTS.md present" intends "agent has *some* file but not IDENTITY". Replace AGENTS.md with TOOLS.md, update the comment.
- **Line 532 (any test):** `git grep` the test name; replace with TOOLS.md if the file just needs to exist as fixture, otherwise drop.
- **Lines 732, 758, 777:** Same pattern — replace with TOOLS.md or drop based on each test's intent.

After edits, confirm `rg -n "AGENTS\.md" crates/right-agent/src/doctor_tests.rs` returns nothing.

- [ ] **Step 6.4: Replace AGENTS.md fixture in `destroy.rs:383`**

In `crates/right-agent/src/agent/destroy.rs:383`:

```rust
        std::fs::write(agents_dir.join("AGENTS.md"), "# Test agent").unwrap();
```

Replace with:

```rust
        std::fs::write(agents_dir.join("IDENTITY.md"), "# Test agent").unwrap();
```

The fixture only needs *some* file in the agent dir for backup-tar to capture.

- [ ] **Step 6.5: Build + test**

```bash
cargo build --workspace
cargo test -p right-agent --lib doctor
cargo test -p right-agent --lib platform_store
cargo test -p right-agent --lib agent::destroy
```

Expected: all pass.

- [ ] **Step 6.6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
test: drop AGENTS.md from doctor/platform_store/destroy fixtures

Replaces AGENTS.md fixture writes with IDENTITY.md or TOOLS.md
where a file was needed; drops the write where it was redundant.
Updates platform_store doc comment.
EOF
)"
```

---

### Task 7: Update CLI integration tests

`crates/right/tests/cli_integration.rs` has four AGENTS.md references: two existence assertions after `right init`, and two backup-fixture writes.

**Files:**
- Modify: `crates/right/tests/cli_integration.rs:56,84-88,807,907`

- [ ] **Step 7.1: Drop AGENTS.md existence assertion at L56**

In the test that runs `right init`, the assertion at line 56:

```rust
    assert!(dir.path().join("agents/right/AGENTS.md").exists());
```

Delete it. The adjacent assertions for BOOTSTRAP.md and the negative assertions for IDENTITY/SOUL remain.

- [ ] **Step 7.2: Drop AGENTS.md block at L84-88**

In `test_init_generates_per_agent_codegen`, the assertion block at lines 84-88:

```rust
    // AGENTS.md and TOOLS.md live at agent root
    assert!(
        dir.path().join("agents/right/AGENTS.md").exists(),
        "missing AGENTS.md at agent root"
    );
```

Delete the comment line and the four-line `assert!`. The TOOLS.md assertion that follows remains; update the leading comment if needed:

```rust
    // TOOLS.md lives at agent root
    assert!(
        dir.path().join("agents/right/TOOLS.md").exists(),
        "missing TOOLS.md at agent root"
    );
```

- [ ] **Step 7.3: Replace AGENTS.md fixtures at L807 and L907**

Both lines write a placeholder file before running `right agent backup`:

```rust
    fs::write(agent_dir.join("AGENTS.md"), "# Agents\n").unwrap();
```

Replace each with:

```rust
    fs::write(agent_dir.join("TOOLS.md"), "# Tools\n").unwrap();
```

(IDENTITY.md is already written separately in both tests; reuse-of-fixture intent is "extra file in agent dir for tar capture".)

- [ ] **Step 7.4: Build + test**

```bash
cargo build --workspace
cargo test -p right --test cli_integration
```

Expected: all pass.

- [ ] **Step 7.5: Commit**

```bash
git add crates/right/tests/cli_integration.rs
git commit -m "$(cat <<'EOF'
test(cli): drop AGENTS.md from cli_integration fixtures and assertions

Removes AGENTS.md existence assertions after `right init` and replaces
fixture writes with TOOLS.md where a placeholder file was needed.
EOF
)"
```

---

### Task 8: Update architecture docs

Two long-form docs (`PROMPT_SYSTEM.md`, `ARCHITECTURE.md`) describe AGENTS.md in tables, lifecycle diagrams, and category lists. Update each to reflect the four-file identity surface.

**Files:**
- Modify: `PROMPT_SYSTEM.md:63,161,175`
- Modify: `ARCHITECTURE.md:55,263,346,496`

- [ ] **Step 8.1: Edit `PROMPT_SYSTEM.md`**

Line 63 — the prompt-structure block:

```
{AGENTS.md — per-agent: subagents, task routing, installed skills}
```

Delete this line entirely (the line before describes USER.md, the line after describes TOOLS.md).

Line 161 — the sandbox file table row:

```
| AGENTS.md | `/sandbox/AGENTS.md` | Agent (editable) |
```

Delete this row.

Line 175 — the host file table row:

```
| AGENTS.md | `agent_dir/AGENTS.md` | reverse_sync |
```

Delete this row.

Also update the section header at line 62 (the prompt-structure code block) to remove the `## Agent Configuration` header line if present — confirm by reading lines 45-80 of `PROMPT_SYSTEM.md`.

- [ ] **Step 8.2: Edit `ARCHITECTURE.md`**

Line 55 — agent lifecycle:

```
  ├─ Write AGENTS.md, BOOTSTRAP.md, agent.yaml
```

Becomes:

```
  ├─ Write BOOTSTRAP.md, TOOLS.md, agent.yaml
```

(TOOLS.md is also written at init; reflect actual behavior post-Task 4.)

Line 263 — prompting architecture file list:

```
(IDENTITY.md, SOUL.md, USER.md, AGENTS.md, TOOLS.md, MCP instructions, composite-memory).
```

Becomes:

```
(IDENTITY.md, SOUL.md, USER.md, TOOLS.md, MCP instructions, composite-memory).
```

Line 346 — config hierarchy table row:

```
| Per-agent | `agents/<name>/AGENTS.md` | Per-agent config (subagents, routing, skills) | `AgentOwned` |
```

Delete this row.

Line 496 — codegen-categories examples list:

```
| `AgentOwned` | Created by init. Never touched again. | TOOLS.md, AGENTS.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md, settings.local.json |
```

Becomes:

```
| `AgentOwned` | Created by init. Never touched again. | TOOLS.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md, settings.local.json |
```

- [ ] **Step 8.3: Verify no AGENTS.md remaining in docs**

```bash
rg -n "AGENTS\.md" PROMPT_SYSTEM.md ARCHITECTURE.md
```

Expected: no output.

- [ ] **Step 8.4: Commit**

```bash
git add PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "$(cat <<'EOF'
docs(architecture): drop AGENTS.md from PROMPT_SYSTEM and ARCHITECTURE

Reflects the removal of AGENTS.md from the identity surface.
EOF
)"
```

---

### Task 9: Final verification

A single-pass grep across the whole repo (excluding historical specs/plans/CHANGELOG and `/tmp/`) confirms no leftover references. A full build + test guards against missed call sites.

- [ ] **Step 9.1: Repo-wide grep for `AGENTS.md`**

```bash
rg -n "AGENTS\.md" \
  --glob '!docs/superpowers/specs/**' \
  --glob '!docs/superpowers/plans/**' \
  --glob '!docs/plans/**' \
  --glob '!CHANGELOG.md' \
  --glob '!.worktrees/**' \
  --glob '!target/**'
```

Expected: no output. Historical docs (specs, plans, CHANGELOG) intentionally retain references — they're a record of what was true at write-time.

- [ ] **Step 9.2: Repo-wide grep for `agents_path`**

```bash
rg -n "agents_path" --glob '!target/**' --glob '!.worktrees/**'
```

Expected: no output.

- [ ] **Step 9.3: Full workspace build**

```bash
cargo build --workspace
```

Expected: clean build with no warnings about unused fields/imports tied to AGENTS.md.

- [ ] **Step 9.4: Full workspace test**

```bash
cargo test --workspace
```

Expected: all pass.

- [ ] **Step 9.5: Run review-rust-code subagent**

Per project convention (`CLAUDE.md`: *"After significant changes: run review-rust-code subagent, issues → TODOs, fix one by one"*). Use the rust-dev:review-rust-code agent on the diff between this branch and `master`. Address any flagged issues before opening a PR.

- [ ] **Step 9.6: Final commit (if any review-driven fixes)**

If Step 9.5 produced changes, commit them as `chore(review): address review-rust-code feedback for AGENTS.md removal`. Otherwise skip.

---

## Self-Review

**Spec coverage** — every requirement maps to a task:

- "Удалить AGENTS.md" (delete the template) → Task 4.1.
- "Не оставить лишних артефактов" (no leftover artifacts) → Tasks 1-8 cover every reference found by `rg AGENTS.md`. Task 9 verifies via final grep.
- Compiled-in templates / skills updated → Task 1.
- Prompt assembly stops loading the file → Task 2.
- Reverse sync stops downloading the file → Task 3.
- Init stops creating the file → Task 4.
- Type system loses the field → Task 5.
- Test fixtures stop using the file → Tasks 6, 7.
- Long-form docs reflect the new state → Task 8.
- Build + test pass clean → Task 9.

**Placeholder scan** — no "TBD", "implement later", "handle edge cases", or vague "tests for the above" instructions. Every step has the exact diff or the exact command.

**Type consistency** — `AgentDef` field name `agents_path` is consistently named across all listed sites; verified by `rg agents_path`. `REVERSE_SYNC_FILES` and `PROMPT_SECTIONS` are referenced by their actual identifiers. Test names (`script_normal_includes_all_identity_files`, `script_normal_has_agent_configuration_section`) are quoted from source.

**Migration consideration** — existing deployed agents have `AGENTS.md` on disk in both `agent_dir/` and `/sandbox/`. After this plan:
- The host file is no longer reverse-synced (Task 3) but remains on disk.
- The sandbox file is no longer loaded into the prompt (Task 2) but remains in the sandbox.
- Per the project's "AgentOwned files codegen never touches" rule (`ARCHITECTURE.md:496`), we do **not** auto-delete. The file is dead bytes; the operator or agent can `rm` at leisure.
- No sandbox migration is required. No `right agent config` re-run is required. The change is effective on `right restart <agent>`.

This is consistent with `CLAUDE.md`'s "Upgrade-friendly design" rule: every change deployable to running agents without recreation.
