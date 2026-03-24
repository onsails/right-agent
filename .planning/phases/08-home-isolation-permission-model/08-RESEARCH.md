# Phase 8: HOME Isolation & Permission Model - Research

**Researched:** 2026-03-24
**Domain:** Claude Code HOME override, per-agent `.claude.json` generation, credential symlinks, sandbox path resolution, shell wrapper env forwarding
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Shell Wrapper**
- D-01: Set `export HOME="$WORKING_DIR"` early in template — all subsequent shell expansion uses agent HOME.
- D-02: Forward `GIT_CONFIG_GLOBAL`, `SSH_AUTH_SOCK`, `GIT_SSH_COMMAND`, `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL` via explicit exports before HOME is overridden.
- D-03: Forward `ANTHROPIC_API_KEY` via `export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"` — no-op if unset.
- D-04: `--dangerously-skip-permissions` stays. Already implemented.

**Per-Agent .claude.json**
- D-05: `rightclaw up` generates per-agent `.claude.json` with `projects[agent_abs_path].hasTrustDialogAccepted: true` and bypass-accepted field.
- D-06: `rightclaw up` STOPS writing to host `~/.claude.json`. Keep host writes from `init` only as fallback.

**Credential Symlink**
- D-07: Create `$AGENT_DIR/.claude/.credentials.json` → `[absolute_real_host_home]/.claude/.credentials.json`. Use `dirs::home_dir()` captured BEFORE HOME override. Symlink is MANDATORY.
- D-08: If host credentials file does not exist: warn and skip. Do NOT fail fast.

**Sandbox Path Resolution**
- D-09: All sandbox paths use absolute paths resolved at `rightclaw up` time. Switch `denyRead` from `~/`-relative to `[resolved_host_home]/.ssh`, `[resolved_host_home]/.aws`, `[resolved_host_home]/.gnupg`.
- D-09b: Add `allowRead` support. Default: `allowRead: [absolute_agent_path]`, `denyRead: [absolute_real_host_home/]`. New `allow_read: Vec<String>` field in `SandboxOverrides`. Use absolute paths for `allowRead`.

**OAuth Validation Gate**
- D-10: VALIDATED 2026-03-24. Symlink approach works. No-symlink fails. Proceed.

**Integration Tests**
- D-11: Integration tests covering: OAuth without symlink (baseline fail), OAuth with symlink (pass), git env forwarding, `denyRead` enforcement, `allowRead` enforcement.

### Claude's Discretion

- Exact `.claude.json` field name for bypassing the bypass-warning dialog (verify empirically or from CC source).
- Whether to retain or remove `pre_trust_directory()` host-level writes from `init.rs` entirely.
- `allowRead`/`denyRead` conflict resolution semantics — test and implement accordingly.

### Deferred Ideas (OUT OF SCOPE)

- Agent-level `env:` section in agent.yaml for arbitrary env var forwarding (Phase 9 or later).
- `allowRead` for project repos — common-patterns list UX improvement.
- Strict mode requiring ANTHROPIC_API_KEY.
- macOS Keychain credential strategy (if symlink doesn't help on macOS).
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| HOME-01 | Shell wrapper sets `HOME=$AGENT_DIR` before launching claude | D-01: early `export HOME` in template; see "Shell Wrapper Template Changes" |
| HOME-02 | `rightclaw up` generates per-agent `.claude.json` with trust + bypass state | D-05/D-06: generate in per-agent loop; see "Per-Agent .claude.json Generation" |
| HOME-03 | `rightclaw up` symlinks host OAuth credentials to agent `.claude/.credentials.json` | D-07/D-08: `std::os::unix::fs::symlink`; see "Credential Symlink" |
| HOME-04 | Shell wrapper forwards git/SSH identity env vars | D-02/D-03: explicit exports before HOME override; see "Shell Wrapper Template Changes" |
| HOME-05 | Generated sandbox `allowWrite`/`denyRead` paths use absolute paths | D-09: resolve via `dirs::home_dir()` at generation time; see "Sandbox Path Resolution" |
| PERM-01 | Shell wrapper keeps `--dangerously-skip-permissions` | Already implemented. No change. |
| PERM-02 | Pre-populate `.claude.json` with bypass-accepted state | D-05: write bypass field; see "Bypass Dialog Suppression" section |
</phase_requirements>

---

## Summary

Phase 8 adds per-agent HOME isolation: each agent's shell wrapper overrides `HOME` to `$AGENT_DIR`, and `rightclaw up` generates the supporting per-agent files that CC needs to find there (`.claude.json` for trust + bypass state, `.claude/.credentials.json` symlink for OAuth). Without the symlink, agents cannot authenticate at all — this was validated manually on 2026-03-24.

The implementation touches four areas: (1) shell wrapper template to add HOME export and env var forwarding, (2) per-agent `.claude.json` generation in `cmd_up()` loop, (3) credential symlink creation in `cmd_up()` loop, and (4) `generate_settings()` + `SandboxOverrides` to switch to absolute paths and add `allowRead` support.

All changes are additive extensions to existing patterns. The per-agent generation loop in `cmd_up()` (lines 297–341 of `main.rs`) already does settings.json on every `up` — `.claude.json` and the credential symlink follow the same pattern.

**Primary recommendation:** Implement in two task groups — (A) shell wrapper + credential symlink + `.claude.json` generation, then (B) sandbox path hardening + `allowRead`. Write integration tests (D-11) as part of group A to validate security assumptions before proceeding.

---

## Standard Stack

All required libraries are already in `Cargo.toml`. No new dependencies needed.

### Core (already present)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `dirs` | workspace | `dirs::home_dir()` — resolve real host HOME before override | Already used in `init.rs` |
| `std::os::unix::fs::symlink` | stdlib | Create `$AGENT_DIR/.claude/.credentials.json` symlink | No crate needed; OS-native |
| `serde_json` | workspace | Generate `.claude.json` content | Already used everywhere |
| `assert_cmd` + `predicates` | dev | CLI integration tests | Already in `rightclaw-cli` dev-deps |
| `tempfile` | dev | Temporary directories in tests | Already in both crates |

### No New Dependencies

The entire phase is implementable with the existing dependency graph.

---

## Architecture Patterns

### Shell Wrapper Template Changes

Current template (`templates/agent-wrapper.sh.j2`) sets no HOME. Required additions:

```bash
# Capture identity env vars BEFORE HOME override (so they aren't lost)
export GIT_CONFIG_GLOBAL="${GIT_CONFIG_GLOBAL:-}"
export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-}"
export GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-}"
export SSH_AUTH_SOCK="${SSH_AUTH_SOCK:-}"
export GIT_SSH_COMMAND="${GIT_SSH_COMMAND:-}"
export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"

# Override HOME to agent directory — CC reads .claude/, .claude.json from here
export HOME="{{ working_dir }}"
```

Place this block AFTER `set -euo pipefail` and BEFORE the `exec "$CLAUDE_BIN"` call. The `:-` fallback makes each export a no-op when the var is unset in the environment — critical for `set -u` compatibility.

Key ordering constraint: all `export VAR="${VAR:-}"` lines must appear BEFORE `export HOME=...` to capture the real values. After HOME is overridden, any shell that uses `~` in expansion would resolve to the agent dir.

The template renderer (`shell_wrapper.rs`) passes `working_dir` already — no Rust code changes needed, only template changes.

### Per-Agent `.claude.json` Generation

CC reads `$HOME/.claude.json` when `HOME` is overridden. This file must exist at `$AGENT_DIR/.claude.json` with:

```json
{
  "projects": {
    "/absolute/path/to/agent/dir": {
      "hasTrustDialogAccepted": true
    }
  }
}
```

The pattern exactly mirrors `pre_trust_directory()` in `init.rs` (lines 187–229) — read existing if present, merge, write back. The difference: write to `$AGENT_DIR/.claude.json` rather than `$HOST_HOME/.claude.json`.

**Location in codebase:** Add a `generate_claude_json(agent: &AgentDef) -> miette::Result<()>` function to `init.rs` or a new `crates/rightclaw/src/codegen/claude_json.rs`. Call it from `cmd_up()` in the per-agent loop, after the settings.json write (line 339 of `main.rs`).

**Key subtlety:** The path key in `projects` must be the agent's absolute path, same key CC uses when it writes trust state. Use `agent.path.canonicalize().unwrap_or(agent.path.clone())` consistent with how `pre_trust_directory()` does it.

### Bypass Dialog Suppression (PERM-02)

The existing `skipDangerousModePermissionPrompt: true` in `settings.json` already suppresses the bypass warning dialog. This lives in project-level `.claude/settings.json`, which CC reads from `$HOME/.claude/settings.json` under HOME override — and `rightclaw up` regenerates it on every launch.

The CONTEXT.md mentions also writing a bypass-accepted field to `.claude.json`. Empirical investigation of the CC source (minified cli.js, v2.1.81) found no `bypassPermissionsAccepted` field referenced in `.claude.json` — the suppression is controlled exclusively via `skipDangerousModePermissionPrompt` in `settings.json`. The `.claude.json` file's project entry already has `hasTrustDialogAccepted` (trust dialog), but the bypass warning is a separate dialog suppressed via settings.

**Recommendation for planner:** Write only `hasTrustDialogAccepted: true` to the per-agent `.claude.json`. Trust the already-generated `settings.json` (`skipDangerousModePermissionPrompt: true`) to suppress the bypass dialog. If the bypass dialog appears in testing, the field name to try is `bypassPermissionsAccepted` at the top level of `.claude.json` (not nested under `projects`). Flag as an empirical test item.

### Credential Symlink

```rust
use std::os::unix::fs;

let host_home = dirs::home_dir()
    .ok_or_else(|| miette::miette!("cannot determine home directory"))?;
// NOTE: host_home must be captured BEFORE any HOME env var manipulation.
// In cmd_up(), HOME is never changed in the Rust process itself -- only in
// the generated shell script. So dirs::home_dir() always returns real host home.

let host_creds = host_home.join(".claude").join(".credentials.json");
let agent_claude_dir = agent.path.join(".claude");
std::fs::create_dir_all(&agent_claude_dir)?;
let agent_creds = agent_claude_dir.join(".credentials.json");

if host_creds.exists() {
    // Remove stale symlink if present (idempotent on re-runs).
    let _ = std::fs::remove_file(&agent_creds);
    fs::symlink(&host_creds, &agent_creds)
        .map_err(|e| miette::miette!("failed to create credentials symlink for '{}': {e:#}", agent.name))?;
    tracing::debug!(agent = %agent.name, "credentials symlink created");
} else {
    tracing::warn!(
        "no OAuth credentials found at {} -- agents will need ANTHROPIC_API_KEY to authenticate",
        host_creds.display()
    );
    // Warn to stdout as well (user-facing).
    eprintln!(
        "warning: no OAuth credentials at {} -- agent '{}' needs ANTHROPIC_API_KEY",
        host_creds.display(), agent.name
    );
}
```

**Critical:** `dirs::home_dir()` in Rust uses the `HOME` env var on Linux. Since `cmd_up()` never modifies its own process's `HOME` env var (it only writes a shell script that does so), `dirs::home_dir()` safely returns the real user home throughout the Rust process lifetime. This is confirmed by the process boundary — shell wrapper runs in a child process.

**Idempotency:** Use `remove_file` before `symlink` so repeated `rightclaw up` runs don't fail with "file exists".

### Sandbox Path Resolution (HOME-05, D-09, D-09b)

Current state in `settings.rs`:
- `allowWrite`: already absolute (`agent.path.display()`) — no change.
- `denyRead`: currently `~/.ssh`, `~/.aws`, `~/.gnupg` — these tilde-paths resolve to agent HOME under override. Must switch to absolute.

Required change: `generate_settings()` needs the real host home path as a parameter.

New signature:
```rust
pub fn generate_settings(
    agent: &AgentDef,
    no_sandbox: bool,
    host_home: &std::path::Path,  // real host HOME, resolved by caller
) -> miette::Result<serde_json::Value>
```

Caller (`cmd_up()`) passes `dirs::home_dir()?` before the per-agent loop. `init.rs` also calls `generate_settings()` — update that call too.

`DEFAULT_DENY_READ` becomes:
```rust
// Built dynamically from host_home parameter:
let deny_read = vec![
    host_home.join(".ssh").display().to_string(),
    host_home.join(".aws").display().to_string(),
    host_home.join(".gnupg").display().to_string(),
    host_home.display().to_string() + "/",  // belt: deny entire host HOME
];
```

**`allowRead` addition (D-09b):**

`SandboxOverrides` gets a new field:
```rust
#[serde(default)]
pub allow_read: Vec<String>,
```

`generate_settings()` adds `allowRead` to the filesystem section:
```rust
let mut allow_read = vec![agent.path.display().to_string()];
// Merge user overrides
if let Some(ref overrides) = config.sandbox {
    allow_read.extend(overrides.allow_read.iter().cloned());
}
// In the JSON:
"filesystem": {
    "allowWrite": allow_write,
    "allowRead": allow_read,
    "denyRead": deny_read,
}
```

**`allowRead`/`denyRead` precedence open question:** Does CC's sandbox evaluate `allowRead` as an exception to `denyRead` (allowList wins over denyList), or is it purely additive? The CONTEXT.md notes this needs empirical testing. If `allowRead: [/home/user/.rightclaw/agents/right]` does NOT override `denyRead: [/home/user/]`, the approach breaks (agent dir is inside denied host home on standard installs).

If specificity wins (allow beats deny when more specific) — implementation is correct as planned.
If deny wins over allow — `denyRead` for host home must exclude the agent dir subtree, or omit the host-home deny and rely on the specific `.ssh`/`.aws`/`.gnupg` denies only.

Plan for this uncertainty: implement as designed, include a test that validates `allowRead` beats `denyRead`, and if it fails adjust to deny-specific-paths-only approach.

### Integration Tests (D-11)

Pattern from existing `crates/rightclaw-cli/tests/cli_integration.rs` using `assert_cmd` + `Command::cargo_bin("rightclaw")`.

D-11 tests require `claude -p` with actual OAuth — these cannot be unit tests. They are `#[ignore]` integration tests runnable manually in CI with live credentials:

```rust
#[test]
#[ignore = "requires live claude credentials"]
fn test_oauth_with_symlink_succeeds() { ... }

#[test]
#[ignore = "requires live claude credentials"]
fn test_oauth_without_symlink_fails() { ... }
```

Non-credential tests (file structure, settings content, symlink presence) can be non-ignored.

Recommended test coverage:
1. `rightclaw up` creates `$AGENT_DIR/.claude.json` with `hasTrustDialogAccepted: true` (assert file contents)
2. `rightclaw up` creates `$AGENT_DIR/.claude/.credentials.json` symlink pointing to host creds (assert symlink target)
3. `rightclaw up` when host creds absent: exits successfully (not error), emits warning text
4. Generated settings.json `denyRead` uses absolute paths (not `~/`) (parse JSON, assert)
5. Generated settings.json includes `allowRead` with absolute agent path (parse JSON, assert)
6. Shell wrapper script contains `export HOME=` and git env vars (assert script content)
7. [Live] OAuth with symlink works (run `claude -p "hi" --output-format json`, check no error)
8. [Live] `denyRead` enforcement blocks reading host `.ssh` (run `claude -p "cat ~/.ssh/config"`, expect sandbox refusal)

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Symlink creation | Custom symlink abstraction | `std::os::unix::fs::symlink` | stdlib, one line |
| Detecting stale symlinks | Complex stat checks | `std::fs::remove_file` + `symlink` | remove-then-create is idempotent and handles symlink, broken symlink, and file all at once |
| Home directory resolution | Env var parsing | `dirs::home_dir()` | Handles edge cases (no HOME set, XDG overrides) |
| JSON merge for `.claude.json` | String templating | `serde_json::Value` read-modify-write | Preserves existing fields CC has written |

---

## Common Pitfalls

### Pitfall 1: Tilde in denyRead Resolves to Agent HOME Under Override

**What goes wrong:** `denyRead: ["~/.ssh"]` resolves at CC runtime relative to the current HOME. Under HOME override, `~` = agent dir. The deny is ineffective — it denies `$AGENT_DIR/.ssh` (which doesn't exist) rather than the real host `.ssh`.

**Why it happens:** Shell tilde expansion is not happening here — CC expands `~/` internally relative to the `HOME` env var it sees. Under HOME override, that's the agent dir.

**How to avoid:** Generate absolute paths using `dirs::home_dir()` in Rust before writing the settings file. Current code uses `~/.ssh` literals — must change.

**Warning signs:** Agent can read host `.ssh/config` during sandbox testing.

### Pitfall 2: Self-Referential Credential Symlink If `~/` Used

**What goes wrong:** `fs::symlink("~/.claude/.credentials.json", target)` or `dirs::home_dir()` called AFTER HOME env var is changed in the current process would create `$AGENT_DIR/.claude/.credentials.json` → `$AGENT_DIR/.claude/.credentials.json` (symlink to itself).

**Why it happens:** `dirs::home_dir()` on Linux reads `HOME` env var. If the process's own `HOME` is modified (e.g., `std::env::set_var("HOME", agent_dir)`), it returns the wrong value.

**How to avoid:** `cmd_up()` never modifies its own process's `HOME` env var — it only writes the shell script. So `dirs::home_dir()` is always safe in the Rust code. Document this assumption explicitly. Never call `std::env::set_var("HOME", ...)` in Rust code.

**Warning signs:** `ls -la $AGENT_DIR/.claude/.credentials.json` shows a symlink pointing to itself.

### Pitfall 3: Non-Idempotent Symlink Creation on Re-Run

**What goes wrong:** `std::os::unix::fs::symlink(src, dst)` fails with `ErrorKind::AlreadyExists` if the target path already exists (even if it's already the correct symlink).

**Why it happens:** `rightclaw up` regenerates all files on every launch. Symlinks don't support "create or replace" atomically.

**How to avoid:** `let _ = std::fs::remove_file(&agent_creds);` before calling `symlink`. The `_ =` suppresses the error if the file doesn't exist yet. This handles: no file, existing symlink, stale broken symlink, and regular file all in one pattern.

### Pitfall 4: `SandboxOverrides` `deny_unknown_fields` Breaks `allow_read` Deserialization

**What goes wrong:** `SandboxOverrides` has `#[serde(deny_unknown_fields)]`. Adding `allow_read` to the struct but forgetting to also update the test YAML will cause test failures. Users with existing `agent.yaml` files that don't have `allow_read` won't be affected (default empty vec), but the struct's unknown-field rejection means the field name in YAML must match exactly.

**How to avoid:** Add `allow_read` with `#[serde(default)]`. Update test at line 163 of `types.rs` that tests "rejects unknown fields" — verify `allow_read` IS accepted (it should be, since it's now a known field). Add a new test that deserializes `allow_read` paths.

### Pitfall 5: `init.rs` `generate_settings()` Call Signature Change

**What goes wrong:** `generate_settings()` currently takes `(agent, no_sandbox)`. Adding `host_home` parameter breaks all callers: `init.rs` line 102 and `cmd_up()` line 330.

**How to avoid:** Update both call sites simultaneously. `init.rs` can use `dirs::home_dir()` directly. `cmd_up()` resolves home_dir once before the per-agent loop. Update `settings_tests.rs` — the existing test `includes_deny_read_security_defaults` asserts the tilde-style paths (line 187) and must be updated to assert absolute paths.

---

## Code Examples

### Symlink creation (idempotent)

```rust
// Source: stdlib std::os::unix::fs
use std::os::unix::fs as unix_fs;

let _ = std::fs::remove_file(&agent_creds); // no-op if absent
unix_fs::symlink(&host_creds, &agent_creds)
    .map_err(|e| miette::miette!("failed to create credentials symlink for '{}': {e:#}", agent.name))?;
```

### Per-agent `.claude.json` generation

```rust
// Source: pattern from pre_trust_directory() in init.rs
fn generate_agent_claude_json(agent: &AgentDef) -> miette::Result<()> {
    let claude_json_path = agent.path.join(".claude.json");

    let mut config: serde_json::Value = if claude_json_path.exists() {
        let content = std::fs::read_to_string(&claude_json_path)
            .map_err(|e| miette::miette!("failed to read {}: {e:#}", claude_json_path.display()))?;
        serde_json::from_str(&content)
            .map_err(|e| miette::miette!("failed to parse {}: {e:#}", claude_json_path.display()))?
    } else {
        serde_json::json!({})
    };

    let path_key = agent.path.canonicalize()
        .unwrap_or_else(|_| agent.path.clone())
        .display()
        .to_string();

    let projects = config
        .as_object_mut()
        .ok_or_else(|| miette::miette!(".claude.json is not a JSON object"))?
        .entry("projects")
        .or_insert_with(|| serde_json::json!({}));

    let project = projects
        .as_object_mut()
        .ok_or_else(|| miette::miette!("projects is not a JSON object"))?
        .entry(&path_key)
        .or_insert_with(|| serde_json::json!({}));

    project
        .as_object_mut()
        .ok_or_else(|| miette::miette!("project entry is not a JSON object"))?
        .insert("hasTrustDialogAccepted".to_owned(), serde_json::Value::Bool(true));

    std::fs::write(
        &claude_json_path,
        serde_json::to_string_pretty(&config)
            .map_err(|e| miette::miette!("failed to serialize: {e:#}"))?,
    )
    .map_err(|e| miette::miette!("failed to write {}: {e:#}", claude_json_path.display()))
}
```

### Shell wrapper template additions

```jinja
#!/usr/bin/env bash
# Generated by rightclaw -- do not edit
# Agent: {{ agent_name }}
set -euo pipefail

# Capture identity env vars from real environment BEFORE HOME override.
export GIT_CONFIG_GLOBAL="${GIT_CONFIG_GLOBAL:-}"
export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-}"
export GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-}"
export SSH_AUTH_SOCK="${SSH_AUTH_SOCK:-}"
export GIT_SSH_COMMAND="${GIT_SSH_COMMAND:-}"
export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"

# Override HOME so CC reads agent-local .claude/, .claude.json, sessions, memory.
export HOME="{{ working_dir }}"

# ... (existing claude binary detection) ...
```

### Absolute denyRead in generate_settings

```rust
// Source: settings.rs — change required
pub fn generate_settings(
    agent: &AgentDef,
    no_sandbox: bool,
    host_home: &Path,
) -> miette::Result<serde_json::Value> {
    // ...
    let deny_read = vec![
        host_home.join(".ssh").display().to_string(),
        host_home.join(".aws").display().to_string(),
        host_home.join(".gnupg").display().to_string(),
        // Belt: deny entire host home. allowRead[agent_path] must override this.
        host_home.display().to_string(),
    ];

    let allow_read = vec![agent.path.display().to_string()];
    // ...
}
```

---

## Open Questions

1. **`allowRead` beats `denyRead` precedence**
   - What we know: CC sandbox docs show both fields exist. User confirmed `allowRead: ["."]` usage.
   - What's unclear: Does `allowRead: [/home/user/.rightclaw/agents/right]` override `denyRead: [/home/user/]` (more-specific wins), or does deny always win?
   - Recommendation: Implement as "allow beats deny (specificity)" — test empirically in D-11 tests. Fallback if deny wins: remove broad host-home denyRead, rely only on explicit `.ssh`/`.aws`/`.gnupg` denies.

2. **`.claude.json` bypass field name for PERM-02**
   - What we know: `skipDangerousModePermissionPrompt: true` in `settings.json` already suppresses the bypass dialog. The minified CC cli.js (v2.1.81) has no reference to `bypassPermissionsAccepted` in `.claude.json`.
   - What's unclear: Whether there is a separate `.claude.json` field for the bypass-accepted state (vs settings.json field).
   - Recommendation: Write only `hasTrustDialogAccepted: true` to `.claude.json`. The bypass dialog suppression is already handled by `skipDangerousModePermissionPrompt` in `settings.json` (already generated by `rightclaw up`). If bypass dialog appears in testing, try `bypassPermissionsAccepted: true` at top level of `.claude.json` as a fallback.

3. **`init.rs` `pre_trust_directory()` host-file writes**
   - What we know: Under HOME override, CC reads `$AGENT_DIR/.claude.json`, not `$HOST_HOME/.claude.json`. The host-file writes are irrelevant for agents using HOME override.
   - What's unclear: Whether to keep them as a non-HOME-override fallback (in case feature is reverted) or remove to reduce confusion.
   - Recommendation: Keep the host-file writes in `pre_trust_directory()` during Phase 8 (conservative). Log a comment that they're a fallback. Revisit removal in Phase 9 once HOME override is battle-tested.

---

## Sources

### Primary (HIGH confidence)
- Direct code inspection: `crates/rightclaw/src/codegen/settings.rs` — current denyRead uses tilde literals (confirmed pitfall)
- Direct code inspection: `crates/rightclaw/src/init.rs` — `pre_trust_directory()` pattern (lines 187–229) is the template for `.claude.json` generation
- Direct code inspection: `templates/agent-wrapper.sh.j2` — no HOME export present (confirmed gap)
- Direct code inspection: `crates/rightclaw-cli/src/main.rs` lines 297–341 — per-agent generation loop pattern
- Direct inspection: host `~/.claude.json` project entries confirm `hasTrustDialogAccepted` key exists and is used by CC
- Validated fact (08-CONTEXT.md D-10): Symlink approach confirmed working; no-symlink fails with "Not logged in"
- stdlib docs: `std::os::unix::fs::symlink` — standard approach, no crate needed

### Secondary (MEDIUM confidence)
- CC binary inspection (`cli.js` v2.1.81): No `bypassPermissionsAccepted` field found in `.claude.json` structure — bypass suppression is via `settings.json` `skipDangerousModePermissionPrompt`
- `~/.claude.json` live inspection: project entries contain `hasTrustDialogAccepted` but no bypass-accepted field at project or top level

### Tertiary (LOW confidence)
- `allowRead`/`denyRead` specificity semantics — referenced in CONTEXT.md from CC docs but not independently verified by code inspection. Treat as unvalidated.

---

## Metadata

**Confidence breakdown:**
- Shell wrapper changes: HIGH — template and codegen patterns are clear, implementation is straightforward
- Per-agent `.claude.json` generation: HIGH — direct code pattern from `pre_trust_directory()` to follow
- Credential symlink: HIGH — stdlib API, design validated empirically
- Sandbox path resolution: HIGH — code change is mechanical, known bug to fix
- `allowRead` support: MEDIUM — struct/codegen change is clear, but `allowRead`/`denyRead` precedence semantics need empirical validation
- Bypass dialog field in `.claude.json`: MEDIUM — evidence suggests it's not needed (settings.json already handles it), but definitive verification requires a live test

**Research date:** 2026-03-24
**Valid until:** 2026-04-14 (30 days — stable CC version, stable patterns)
