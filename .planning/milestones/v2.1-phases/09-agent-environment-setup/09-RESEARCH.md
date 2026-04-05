# Phase 9: Agent Environment Setup - Research

**Researched:** 2026-03-24
**Domain:** Rust CLI file generation — git init, Telegram channel config, skills propagation, settings.local.json
**Confidence:** HIGH

## Summary

Phase 9 is pure Rust extension work: extend the existing `cmd_up()` per-agent loop in `main.rs` with four new operations, and add three new fields to `AgentConfig` in `types.rs`. There is no new external dependency — every pattern is already present in the codebase.

The most complex piece is the Telegram channel config: `AgentConfig` gains `telegram_token_file`, `telegram_token`, and `telegram_user_id` fields. On `up`, the CLI reads the token (from file or inline), writes `.env` and `access.json` to `$AGENT_DIR/.claude/channels/telegram/`, and creates the `.mcp.json` marker. The exact `.env` format (`TELEGRAM_BOT_TOKEN=<token>\n`) and `access.json` format are already established in `init.rs` lines 119-160 — copy exactly, no invention required.

The git init, skills reinstall, and settings.local.json writes are mechanically simpler. The key constraint is ordering: git init and settings.local.json are idempotent-conditional (skip if exists); Telegram `.env`/`access.json` overwrite on every `up`; skills always overwrite (built-in directories only).

**Primary recommendation:** Extract `install_builtin_skills(agent_path)` from `init.rs` into `codegen/` and add `generate_telegram_channel_config(agent)` as a new codegen function. Keep `git init` inline in `cmd_up()` — it's a one-liner `Command`. Add `git` to `doctor.rs` as a `Warn`-severity check (non-fatal in `cmd_up()`).

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**git init (AENV-01)**
- D-01: Run `git init` (regular, not bare) in each agent directory during `rightclaw up`.
- D-02: Skip git init if `.git/` already exists (idempotent check: `agent.path.join(".git").exists()`).
- D-03: `std::process::Command::new("git").arg("init").current_dir(&agent.path)` — no git2 crate.

**Telegram Channel Config (AENV-02)**
- D-04: Config is per-agent from agent.yaml — no copying from host `~/.claude/channels/telegram/`.
- D-05: `AgentConfig` gains: `telegram_token_file: Option<String>`, `telegram_token: Option<String>`, `telegram_user_id: Option<String>`. All with `#[serde(default)]`. Precedence: file > inline.
- D-06: Default token file convention `.telegram.env` in agent dir. `rightclaw init` extended to write it and set `telegram_token_file: .telegram.env` + gitignore.
- D-07: On `up` per agent: (1) resolve token, (2) write `.env`, (3) write `access.json` if `telegram_user_id` set, (4) ensure `.mcp.json` exists, (5) skip if no Telegram config.
- D-08: Overwrite `.env` and `access.json` on every `up` (not idempotent).

**Skills Propagation (AENV-03)**
- D-09: Reinstall built-in skills (`clawhub/SKILL.md`, `rightcron/SKILL.md`, `installed.json`) into each agent's `.claude/skills/` on every `up`. Always overwrite.
- D-10: Only overwrite named built-in skill directories — never wipe the entire skills dir.

**settings.local.json (AENV-03)**
- D-11: Write empty `{}` only if file does not already exist (agents may write to it at runtime).

**Ordering in cmd_up() per-agent loop (D-12):**
1. Generate combined prompt (existing)
2. Generate shell wrapper (existing)
3. Generate settings.json (existing)
4. Generate .claude.json (Phase 8)
5. Create credential symlink (Phase 8)
6. git init if .git/ missing (Phase 9)
7. Telegram channel config (Phase 9)
8. Reinstall built-in skills (Phase 9)
9. Write settings.local.json if missing (Phase 9)

### Claude's Discretion
- Exact token file format: use `TELEGRAM_BOT_TOKEN=<token>\n` (matches `init.rs` line 134).
- git binary absence severity: `Warn` in doctor, non-fatal `log warning, continue` in `cmd_up`.

### Deferred Ideas (OUT OF SCOPE)
- `rightclaw agent init` subcommand (future phase)
- secretspec / env var injection for Telegram token
- Agent-level `env:` section in agent.yaml
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| AENV-01 | `rightclaw up` initializes `.git/` in each agent directory | git init via `std::process::Command`, idempotent check on `.git/` existence |
| AENV-02 | `rightclaw up` copies Telegram channel config to agent HOME `.claude/channels/telegram/` when configured | New `AgentConfig` fields + `generate_telegram_channel_config()` codegen function, exact format from `init.rs:119-160` |
| AENV-03 | Pre-populated `.claude/` includes settings.json, settings.local.json (empty), skills/ | Extract `install_builtin_skills()`, write settings.local.json only-if-missing; settings.json already done per Phase 6 |
| PERM-03 | Telegram channel active as permission relay safety net | AENV-02 delivers this: bot token + access.json written per-agent |
</phase_requirements>

## Standard Stack

### Core (already in Cargo.toml — no new deps)
| Library | Purpose | Notes |
|---------|---------|-------|
| `std::process::Command` | `git init` | Already used in runtime; one-shot invocation |
| `std::fs` | File writes (`.env`, `access.json`, `settings.local.json`) | Standard, no crate needed |
| `which` | Detect `git` binary for doctor check | Already in deps for process-compose/claude checks |
| `serde` + `serde_saphyr` | New `AgentConfig` fields deserialization | Existing pattern, add `#[serde(default)]` fields |

No new crate dependencies required for this phase.

## Architecture Patterns

### Recommended Project Structure Changes

```
crates/rightclaw/src/
├── agent/
│   └── types.rs          # Add telegram_token_file, telegram_token, telegram_user_id to AgentConfig
├── codegen/
│   ├── mod.rs            # Export new: generate_telegram_channel_config, install_builtin_skills
│   ├── telegram.rs       # NEW: generate_telegram_channel_config()
│   └── skills.rs         # NEW (or extend existing): install_builtin_skills()
└── init.rs               # Refactor Telegram + skills blocks to call new shared functions
```

### Pattern 1: Token Resolution (D-05, D-07)

Resolve token from file-or-inline before writing:

```rust
// Source: init.rs lines 119-160 — extended for per-agent with file path support
fn resolve_telegram_token(agent: &AgentDef) -> miette::Result<Option<String>> {
    let config = match agent.config.as_ref() {
        Some(c) => c,
        None => return Ok(None),
    };

    if let Some(ref file_path) = config.telegram_token_file {
        // Resolve relative to agent dir (D-05)
        let abs = agent.path.join(file_path);
        let content = std::fs::read_to_string(&abs)
            .map_err(|e| miette::miette!("failed to read telegram token file {}: {e:#}", abs.display()))?;
        return Ok(Some(content.trim().to_string()));
    }

    Ok(config.telegram_token.clone())
}
```

### Pattern 2: Telegram Channel Config Write (D-07, D-08)

```rust
// Source: init.rs lines 119-160 — adapted for per-agent codegen
pub fn generate_telegram_channel_config(agent: &AgentDef) -> miette::Result<()> {
    let token = match resolve_telegram_token(agent)? {
        Some(t) => t,
        None => return Ok(()), // skip if no Telegram config
    };

    let channel_dir = agent.path.join(".claude").join("channels").join("telegram");
    std::fs::create_dir_all(&channel_dir)
        .map_err(|e| miette::miette!("failed to create telegram channel dir: {e:#}"))?;

    // Always overwrite (D-08)
    std::fs::write(channel_dir.join(".env"), format!("TELEGRAM_BOT_TOKEN={token}\n"))
        .map_err(|e| miette::miette!("failed to write telegram .env: {e:#}"))?;

    // access.json only if user_id set
    if let Some(ref user_id) = agent.config.as_ref().and_then(|c| c.telegram_user_id.clone()) {
        let access_json = format!(
            r#"{{"dmPolicy":"allowlist","allowFrom":["{user_id}"],"pending":{{}},"groups":{{}}}}"#
        );
        std::fs::write(channel_dir.join("access.json"), access_json)
            .map_err(|e| miette::miette!("failed to write access.json: {e:#}"))?;
    }

    // Ensure .mcp.json marker exists (shell wrapper checks this for --channels flag)
    let mcp_json = agent.path.join(".mcp.json");
    if !mcp_json.exists() {
        std::fs::write(&mcp_json, r#"{"telegram": true}"#)
            .map_err(|e| miette::miette!("failed to write .mcp.json: {e:#}"))?;
    }

    tracing::debug!(agent = %agent.name, "wrote telegram channel config");
    Ok(())
}
```

### Pattern 3: Built-in Skills Install (D-09, D-10)

Extract from `init.rs` lines 57-76 to a standalone function:

```rust
// Source: init.rs lines 57-76 — extract as shared function
pub fn install_builtin_skills(agent_path: &Path) -> miette::Result<()> {
    let built_in_skills: &[(&str, &str)] = &[
        ("clawhub/SKILL.md", SKILL_CLAWHUB),
        ("rightcron/SKILL.md", SKILL_RIGHTCRON),
    ];
    let claude_skills_dir = agent_path.join(".claude").join("skills");
    for (skill_path, content) in built_in_skills {
        let path = claude_skills_dir.join(skill_path);
        std::fs::create_dir_all(path.parent().unwrap())
            .map_err(|e| miette::miette!("failed to create skill dir: {e:#}"))?;
        std::fs::write(&path, content)
            .map_err(|e| miette::miette!("failed to write {}: {e:#}", path.display()))?;
    }
    std::fs::write(claude_skills_dir.join("installed.json"), "{}")
        .map_err(|e| miette::miette!("failed to write installed.json: {e:#}"))?;
    Ok(())
}
```

Note: `init.rs` calls this after extraction. The constants `SKILL_CLAWHUB`/`SKILL_RIGHTCRON` live in `init.rs` — move them to the new `skills.rs` module or make the function take a path and constants parameter. Simplest: move constants to `codegen/skills.rs`.

### Pattern 4: git init (D-01, D-02, D-03)

```rust
// Inline in cmd_up() per-agent loop — no codegen function needed
if !agent.path.join(".git").exists() {
    let status = std::process::Command::new("git")
        .arg("init")
        .current_dir(&agent.path)
        .status()
        .map_err(|e| {
            tracing::warn!(agent = %agent.name, "git not found, skipping git init: {e}");
            // return Ok — non-fatal (D-03 discretion: Warn severity)
        });
    match status {
        Ok(s) if s.success() => {
            tracing::debug!(agent = %agent.name, "git init done");
        }
        Ok(s) => {
            tracing::warn!(agent = %agent.name, "git init exited {}", s);
        }
        Err(_) => {
            tracing::warn!(agent = %agent.name, "git binary not found, skipping git init");
        }
    }
}
```

Non-fatal: log warning and continue if git is absent. Doctor check added at `Warn` severity.

### Pattern 5: settings.local.json (D-11)

```rust
// Conditional write — only if file absent
let settings_local = claude_dir.join("settings.local.json");
if !settings_local.exists() {
    std::fs::write(&settings_local, "{}")
        .map_err(|e| miette::miette!("failed to write settings.local.json: {e:#}"))?;
}
```

### AgentConfig Extension (D-05)

Add to `crates/rightclaw/src/agent/types.rs`, inside `AgentConfig`:

```rust
/// Path to file containing Telegram bot token (relative to agent dir).
/// Takes precedence over `telegram_token` if both set.
#[serde(default)]
pub telegram_token_file: Option<String>,

/// Inline Telegram bot token (fallback if no token file).
#[serde(default)]
pub telegram_token: Option<String>,

/// Numeric Telegram user ID for access.json pre-pairing.
/// If absent, access.json is not written.
#[serde(default)]
pub telegram_user_id: Option<String>,
```

Note: `AgentConfig` uses `#[serde(deny_unknown_fields)]` — new fields MUST be added before any agent.yaml files exist with those keys, or deserialization fails. Adding fields with `#[serde(default)]` is backward-compatible (existing yaml without the keys still works).

### Anti-Patterns to Avoid

- **Wiping `.claude/skills/`**: Never `remove_dir_all` then recreate. Only overwrite named built-in paths (D-10).
- **Overwriting settings.local.json on every up**: CC and agents write runtime state there. Only write if absent (D-11).
- **Reading token from inline agent.yaml and logging it**: Token appears in struct at deserialization time — do not include it in any `tracing::debug!` formatting.
- **Using bare repo for git init**: Bare repos have no `.git/` directory — CC workspace trust check looks for `.git/`. Use regular `git init` (D-01).
- **Hardcoding `~/.claude/channels/telegram/`**: Under HOME override the agent's HOME is `$AGENT_DIR`. Write to `$AGENT_DIR/.claude/channels/telegram/` (D-04).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Git repository initialization | Custom file writes to `.git/` | `git init` via `Command` | Git internal format is complex and versioned |
| Binary PATH detection | Manual `std::env::var("PATH")` scanning | `which` crate (already in deps) | Handles PATH edge cases, already used in doctor |

## Common Pitfalls

### Pitfall 1: `deny_unknown_fields` on AgentConfig

**What goes wrong:** `AgentConfig` has `#[serde(deny_unknown_fields)]`. Any agent.yaml that has keys not in the struct will fail to parse — including keys added to the struct that the parser hasn't loaded yet during a test.

**Why it happens:** Adding the new Telegram fields means existing test agent.yaml fixtures with those keys will fail on older code. In reverse, new tests using Telegram fields work correctly because the struct now knows them.

**How to avoid:** Always add new `agent.yaml` keys to `AgentConfig` AND `SandboxOverrides` (if applicable) before writing tests that use them. Update the template `agent.yaml` to document the new fields.

### Pitfall 2: Token File Path Resolution

**What goes wrong:** `telegram_token_file: .telegram.env` in agent.yaml is relative. If resolved against `cwd` instead of `agent.path`, it finds the wrong file (or none).

**Why it happens:** `agent.path` is the absolute agent directory. Relative paths in agent.yaml must be joined against `agent.path`, not the process cwd.

**How to avoid:** Always `agent.path.join(telegram_token_file)` when resolving the token file path. Add test coverage for this with a tempdir agent.

### Pitfall 3: `.env` Format Must Match Plugin Expectation

**What goes wrong:** Writing just the token string (without `TELEGRAM_BOT_TOKEN=` prefix) causes the Telegram plugin to fail silently — it reads the `.env` file expecting `KEY=VALUE` format.

**Why it happens:** The CC Telegram plugin expects dotenv format. `init.rs` line 134 already establishes the correct format: `format!("TELEGRAM_BOT_TOKEN={token}\n")`.

**How to avoid:** Copy the exact format from `init.rs` line 134. The trailing newline is included. Do not strip it.

### Pitfall 4: `git init` in CI/Test Environments

**What goes wrong:** Unit tests that exercise the git init path may fail in CI if git is not installed, or create `.git/` directories inside the project's own git repo (confusing git operations).

**Why it happens:** `tempdir()` creates dirs outside the git repo on most systems, but CI may differ. Also, `git init` failure should be non-fatal in production but tests may need to assert on it.

**How to avoid:** In tests for git init: (1) use `tempfile::tempdir()` which creates outside `/tmp`; (2) test the "already exists" skip path with a pre-created `.git/` dir; (3) test the non-fatal behavior when git is absent using a PATH override or by mocking.

### Pitfall 5: Settings.local.json Clobber

**What goes wrong:** Writing `{}` unconditionally on every `up` destroys runtime state CC or the agent wrote to `settings.local.json`.

**Why it happens:** CC may persist per-session settings or plugin state to `settings.local.json` between launches.

**How to avoid:** D-11 is explicit — check `!settings_local.exists()` before writing. This is a non-negotiable constraint.

### Pitfall 6: init.rs Token Write Path Conflict

**What goes wrong:** `init.rs` currently writes the Telegram token to `~/.claude/channels/telegram/.env` (host-level). After Phase 9, `cmd_up()` writes to `$AGENT_DIR/.claude/channels/telegram/.env` (agent-level). These are different paths under HOME override.

**Why it happens:** `init.rs` was written before HOME override existed. The default "right" agent init now needs to write to the agent dir, not host HOME. However, `init.rs` also calls `generate_agent_claude_json` which was adapted for this. The Telegram path in init.rs was NOT updated.

**How to avoid:** After extracting `generate_telegram_channel_config()` into codegen, update `init.rs` to call it (passing agent path) instead of its current inline logic. This unifies the write path and makes init consistent with `cmd_up`.

## Code Examples

### AgentConfig with new Telegram fields (types.rs)

```rust
// Source: crates/rightclaw/src/agent/types.rs — extend existing struct
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    // ... existing fields ...
    pub telegram_token_file: Option<String>,  // with #[serde(default)]
    pub telegram_token: Option<String>,       // with #[serde(default)]
    pub telegram_user_id: Option<String>,     // with #[serde(default)]
}
```

### Existing .env format (from init.rs line 134)

```rust
// Source: crates/rightclaw/src/init.rs line 134
format!("TELEGRAM_BOT_TOKEN={token}\n")
```

### Existing access.json format (from init.rs line 154-156)

```rust
// Source: crates/rightclaw/src/init.rs lines 154-156
format!(r#"{{"dmPolicy":"allowlist","allowFrom":["{user_id}"],"pending":{{}},"groups":{{}}}}"#)
```

### cmd_up() loop structure after Phase 9 (main.rs)

```rust
// Source: crates/rightclaw-cli/src/main.rs — per-agent loop at line 301
for agent in &agents {
    // ... existing steps 1-5 ...

    // 6. git init if .git/ missing (Phase 9, AENV-01)
    // non-fatal: Warn if git absent

    // 7. Telegram channel config (Phase 9, AENV-02)
    rightclaw::codegen::generate_telegram_channel_config(agent)?;

    // 8. Reinstall built-in skills (Phase 9, AENV-03)
    rightclaw::codegen::install_builtin_skills(&agent.path)?;

    // 9. Write settings.local.json if missing (Phase 9, AENV-03)
    // conditional: only if !exists
}
```

## State of the Art

No external library changes in this phase. All patterns are established within the existing codebase.

| Operation | Current State | Phase 9 Change |
|-----------|--------------|----------------|
| Skills install | Only in `init.rs` (one-shot init) | Extracted to shared function, called from `cmd_up()` too |
| Telegram config write | Only in `init.rs`, writes to host HOME | Extracted to codegen, writes to agent HOME under override |
| git init | Not done | Added to `cmd_up()` per-agent loop |
| settings.local.json | Not pre-created | Added to `cmd_up()` with conditional write |
| `AgentConfig` fields | No Telegram fields | +3 optional Telegram fields with `#[serde(default)]` |

## Open Questions

1. **init.rs Telegram write path after refactor**
   - What we know: `init.rs` currently writes token to host `~/.claude/channels/telegram/` (before HOME isolation was a concern). Phase 9 adds per-agent writes under agent HOME.
   - What's unclear: Should `init.rs` be updated in this phase to use the new `generate_telegram_channel_config()`, or left as-is and updated in a follow-up?
   - Recommendation: Update `init.rs` to call the new codegen function in this phase (step 7 of any plan touching Telegram). Keeps the two paths consistent and removes the inline duplication. The function signature takes `agent: &AgentDef` which `init.rs` already constructs.

2. **git doctor check severity**
   - What we know: D-03 says non-fatal in `cmd_up`. The discretion note says `Warn` in doctor.
   - What's unclear: Should doctor add a `git` check by name, or just rely on the Warn log in `cmd_up`?
   - Recommendation: Add `git` to `doctor.rs` as a `Warn` severity check (using `check_binary("git", Some(...))`). This gives users visibility. The `cmd_up` path silently continues on absence — log at `warn!` level.

## Sources

### Primary (HIGH confidence)
- `/home/wb/dev/rightclaw/crates/rightclaw/src/init.rs` — Telegram write pattern (lines 119-160), skills install pattern (lines 57-76)
- `/home/wb/dev/rightclaw/crates/rightclaw-cli/src/main.rs` — `cmd_up()` per-agent loop (lines 301-351)
- `/home/wb/dev/rightclaw/crates/rightclaw/src/agent/types.rs` — `AgentConfig` struct
- `/home/wb/dev/rightclaw/crates/rightclaw/src/codegen/mod.rs` — codegen module exports
- `/home/wb/dev/rightclaw/crates/rightclaw/src/codegen/claude_json.rs` — established codegen function pattern
- `/home/wb/dev/rightclaw/crates/rightclaw/src/runtime/deps.rs` — `verify_dependencies()` pattern for doctor extension
- `/home/wb/dev/rightclaw/.planning/phases/09-agent-environment-setup/09-CONTEXT.md` — locked decisions D-01 through D-12

### Secondary (MEDIUM confidence)
- Project MEMORY.md — Claude Code gotchas (Telegram plugin reads `.env`, `access.json`)
- `/home/wb/dev/rightclaw/.planning/REQUIREMENTS.md` — requirement details for AENV-01/02/03, PERM-03

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new deps, all patterns in existing codebase
- Architecture: HIGH — decisions locked in CONTEXT.md, patterns directly copied from init.rs
- Pitfalls: HIGH — surfaced from code inspection and established CC behavior (deny_unknown_fields, .env format, HOME override)

**Research date:** 2026-03-24
**Valid until:** Stable — no external APIs or libraries involved
