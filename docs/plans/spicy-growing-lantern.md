# Recovery Plan: Restore v3.3 MCP Code + Planning Artifacts Destroyed by Commit 9297d83

## Context

Commit `9297d83` ("feat(43-01): add ChromeConfig to config.rs + Chrome/MCP detection helpers") was created by a GSD executor agent running on the main working tree during Phase 43 wave 1 execution. Instead of surgically editing files, it regenerated the repo from its mental model, silently reverting the entire v3.3 MCP Self-Management milestone and deleting all planning artifacts for phases 42, 43, and the v3.3 milestone.

**Damage:** +1,460/-6,776 lines across 48 files. 27 files deleted, 21 files modified (many destructively). The Chrome integration code added by this commit is correct and must be preserved. Everything else it touched needs recovery.

**Recovery source:** All content is recoverable from `git show 9297d83^:<path>` (the parent commit).

**Constraint:** Chrome integration code in HEAD (commits 9297d83 through e84d3fa) must be preserved — we're restoring what was deleted WITHOUT reverting the Chrome work.

---

## Step 1: Restore Deleted Planning Artifacts (straight git restore)

These files were fully deleted and have no conflicts — restore directly from the parent commit.

### Phase 01 (v3.3 MCP Self-Management) — 8 files
```
.planning/phases/01-mcp-management-tools-in-rightmemory-server/.gitkeep
.planning/phases/01-mcp-management-tools-in-rightmemory-server/01-01-PLAN.md
.planning/phases/01-mcp-management-tools-in-rightmemory-server/01-01-SUMMARY.md
.planning/phases/01-mcp-management-tools-in-rightmemory-server/01-02-PLAN.md
.planning/phases/01-mcp-management-tools-in-rightmemory-server/01-02-SUMMARY.md
.planning/phases/01-mcp-management-tools-in-rightmemory-server/01-CONTEXT.md
.planning/phases/01-mcp-management-tools-in-rightmemory-server/01-DISCUSSION-LOG.md
.planning/phases/01-mcp-management-tools-in-rightmemory-server/01-RESEARCH.md
.planning/phases/01-mcp-management-tools-in-rightmemory-server/01-VERIFICATION.md
```

### Phase 42 planning — 9 files
```
.planning/phases/42-chrome-config-infrastructure-mcp-injection/42-01-PLAN.md
.planning/phases/42-chrome-config-infrastructure-mcp-injection/42-01-SUMMARY.md
.planning/phases/42-chrome-config-infrastructure-mcp-injection/42-02-PLAN.md
.planning/phases/42-chrome-config-infrastructure-mcp-injection/42-02-SUMMARY.md
.planning/phases/42-chrome-config-infrastructure-mcp-injection/42-03-PLAN.md
.planning/phases/42-chrome-config-infrastructure-mcp-injection/42-03-SUMMARY.md
.planning/phases/42-chrome-config-infrastructure-mcp-injection/42-CONTEXT.md
.planning/phases/42-chrome-config-infrastructure-mcp-injection/42-DISCUSSION-LOG.md
.planning/phases/42-chrome-config-infrastructure-mcp-injection/42-RESEARCH.md
```
Note: `42-VERIFICATION.md` already exists on disk (untracked) — keep it.

### Phase 43 planning — 4 files (43-02-PLAN.md already restored by 57fe06d)
```
.planning/phases/43-init-detection-up-revalidation/43-01-PLAN.md
.planning/phases/43-init-detection-up-revalidation/43-CONTEXT.md
.planning/phases/43-init-detection-up-revalidation/43-DISCUSSION-LOG.md
.planning/phases/43-init-detection-up-revalidation/43-RESEARCH.md
```

### v3.3 milestone docs — 2 files
```
.planning/milestones/v3.3-REQUIREMENTS.md
.planning/milestones/v3.3-ROADMAP.md
```

### Phase 01 security artifact — 1 file
```
.planning/phases/01-foundation-and-agent-discovery/01-SECURITY.md
```

**Method:** For each file:
```bash
git show 9297d83^:<path> > <path>
```

---

## Step 2: Restore Planning Files That Need Merging

These files exist on disk with modifications from later commits. They need the pre-damage content merged back, not a blind overwrite.

### 2a. `ROADMAP.md` — add back v3.3 shipped + v3.4 section

Restore from `9297d83^` version, then update to reflect current reality:
- v3.3 MCP Self-Management: mark as SHIPPED (was in progress at time of damage)
- v3.4 Chrome Integration: mark Phase 42 as complete, Phase 43 as complete (both verified)
- Phase 44 definition: restore as-is (still TBD/unplanned)
- Progress table: add v3.3 and v3.4 rows with correct completion status

### 2b. `REQUIREMENTS.md` — restore v3.4 Chrome requirements

Currently contains v3.2 MCP OAuth requirements (a downgrade). Restore the v3.4 Chrome Integration requirements from `9297d83^`. Mark completed requirements as checked:
- CHROME-01, CHROME-02, CHROME-03: all complete (Phase 43 verified)
- INJECT-01, INJECT-02, INJECT-03: all complete (Phases 42+43 verified)
- SBOX-01, SBOX-02: complete (Phase 42 verified)
- VALID-01, VALID-02: not yet done (Phase 44 not executed)
- AGENT-01: not yet done (Phase 44 not executed)

### 2c. `MILESTONES.md` — add back v3.3 section

Add the v3.3 MCP Self-Management shipped section back at the top (before v3.2). Content from `9297d83^` version.

### 2d. `RETROSPECTIVE.md` — add back v3.3 retro section

Add the v3.3 retrospective section back (39 lines). Content from `9297d83^` version. Also restore the Cross-Milestone Trends table row.

### 2e. `PROJECT.md` — restore current milestone context

Restore the "Current Milestone: v3.4 Chrome Integration" section. Update to reflect that Phases 42 and 43 are complete, Phase 44 is the remaining work.

### 2f. Research files — restore Chrome research (4 files)

The 4 research files were overwritten from Chrome (v3.4) content back to v3.1 sandbox content. Restore the Chrome versions from `9297d83^`:
```
.planning/research/ARCHITECTURE.md  (Chrome integration architecture)
.planning/research/FEATURES.md      (chrome-devtools-mcp features + tool catalog)
.planning/research/PITFALLS.md      (Chrome sandbox pitfalls)
.planning/research/STACK.md         (chrome-devtools-mcp package details)
```

### 2g. `STATE.md` — update to reflect actual current state

Don't blindly restore — write a corrected version reflecting reality:
- milestone: v3.4 Chrome Integration
- status: executing (Phase 44 not yet planned)
- Phase 42: complete, verified 15/15
- Phase 43: complete, verified 7/7
- Phase 44: not started (plans TBD)
- progress: 2/3 phases complete, 5/5 plans complete (in phases 42+43)

---

## Step 3: Restore v3.3 MCP Code in memory_server.rs

### 3a. Restore `MemoryServer` struct fields

Add back `agent_dir` and `rightclaw_home` fields to the struct:

**File:** `crates/rightclaw-cli/src/memory_server.rs`
```rust
pub struct MemoryServer {
    tool_router: ToolRouter<Self>,
    conn: Arc<Mutex<rusqlite::Connection>>,
    agent_name: String,
    agent_dir: std::path::PathBuf,         // ← RESTORE
    rightclaw_home: std::path::PathBuf,    // ← RESTORE
}
```

### 3b. Update constructor

Update `MemoryServer::new()` to accept 4 params again:
```rust
pub fn new(
    conn: rusqlite::Connection,
    agent_name: String,
    agent_dir: std::path::PathBuf,
    rightclaw_home: std::path::PathBuf,
) -> Self {
    Self {
        tool_router: Self::tool_router(),
        conn: Arc::new(Mutex::new(conn)),
        agent_name,
        agent_dir,
        rightclaw_home,
    }
}
```

### 3c. Restore 4 MCP tool parameter types

Add back after existing param types (after `CronShowRunParams`):
```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpAddParams { pub name: String, pub url: String }

#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpRemoveParams { pub name: String }

#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpListParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpAuthParams { pub server_name: String }
```

### 3d. Restore 4 MCP tool functions

Restore from `git show 9297d83^:crates/rightclaw-cli/src/memory_server.rs`:
- `async fn mcp_add()` — adds HTTP MCP servers to `.claude.json`
- `async fn mcp_remove()` — removes MCP servers (rightmemory protected)
- `async fn mcp_list()` — lists configured servers with auth state
- `async fn mcp_auth()` — OAuth AS discovery endpoint retrieval

These functions call modules that still exist with compatible APIs:
- `rightclaw::mcp::credentials::add_http_server_to_claude_json()` (exists at credentials.rs:49)
- `rightclaw::mcp::credentials::remove_http_server_from_claude_json()` (exists at credentials.rs:91)
- `rightclaw::mcp::credentials::list_http_servers_from_claude_json()` (exists at credentials.rs:115)
- `rightclaw::mcp::detect::mcp_auth_status()` (exists at detect.rs:71)
- `rightclaw::mcp::oauth::discover_as()` (exists at oauth.rs:184)
- `rightclaw::mcp::PROTECTED_MCP_SERVER` (exists at mcp/mod.rs:7)

### 3e. Update tool description string

Change:
```rust
"RightClaw tools: store, recall, search, forget, cron_list_runs, cron_show_run"
```
To:
```rust
"RightClaw tools: store, recall, search, forget, cron_list_runs, cron_show_run, mcp_add, mcp_remove, mcp_list, mcp_auth"
```

### 3f. Update production `serve_mcp()` call site

Add back the `agent_dir` and `rightclaw_home` env reading and pass to constructor:
```rust
let agent_dir = home.clone();

let rightclaw_home = match std::env::var("RC_RIGHTCLAW_HOME") {
    Ok(p) if !p.is_empty() => std::path::PathBuf::from(p),
    _ => {
        tracing::warn!("RC_RIGHTCLAW_HOME not set — mcp_auth tunnel commands will be unavailable");
        std::path::PathBuf::from(".")
    }
};

let server = MemoryServer::new(conn, agent_name, agent_dir, rightclaw_home);
```

### 3g. Update inline test `setup_server()`

```rust
fn setup_server() -> (MemoryServer, tempfile::TempDir) {
    let dir = tempdir().expect("tempdir");
    let conn = rightclaw::memory::open_connection(dir.path()).expect("open_connection");
    let server = MemoryServer::new(
        conn,
        "test-agent".to_string(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    (server, dir)
}
```

---

## Step 4: Restore memory_server_tests.rs

Restore the extracted test file:

**File:** `crates/rightclaw-cli/src/memory_server_tests.rs` (328 lines)

Restore from `git show 9297d83^:crates/rightclaw-cli/src/memory_server_tests.rs`.

Then update the inline test module in `memory_server.rs` to reference the external file. The old version used:
```rust
#[cfg(test)]
#[path = "memory_server_tests.rs"]
mod tests;
```

Currently it has inline tests. Decision: **keep inline tests AND restore the external file** — the inline tests cover core memory/cron functions; the external file covers MCP tool functions.

Approach: rename the external test module to avoid name collision:
- Rename restored file to `memory_server_mcp_tests.rs`
- Add at end of memory_server.rs:
  ```rust
  #[cfg(test)]
  #[path = "memory_server_mcp_tests.rs"]
  mod mcp_tests;
  ```
- Update `use super::*;` in the test file — should work since all MCP types are in the parent module

The test file's `setup_server_with_dir()` helper needs updating to pass 4 args to `MemoryServer::new()`.

---

## Step 5: Restore reqwest dependency

**File:** `crates/rightclaw-cli/Cargo.toml`

Add back:
```toml
reqwest = { workspace = true }
```

This is needed by `mcp_auth` which uses `reqwest::Client` for OAuth AS discovery HTTP calls.

---

## Step 6: Restore Missing Tests in mcp_config.rs and config.rs

### 6a. `crates/rightclaw/src/codegen/mcp_config.rs` — 2 deleted test functions + 5 assertions

Restore from `git show 9297d83^:crates/rightclaw/src/codegen/mcp_config.rs`:

1. `fn chrome_devtools_not_injected_when_none()` — validates chrome-devtools absent when config is None
2. `fn chrome_devtools_uses_absolute_binary_path_not_npx()` — validates no npx in command

Also restore 5 arg assertions in `chrome_devtools_injected_when_chrome_config_some()`:
```rust
assert!(args_strs.contains(&"--executablePath"));
assert!(args_strs.contains(&"--headless"));
assert!(args_strs.contains(&"--isolated"));
assert!(args_strs.contains(&"--no-sandbox"));
assert!(args_strs.contains(&"--userDataDir"));
```

### 6b. `crates/rightclaw/src/config.rs` — 7 deleted Chrome config test functions

Restore from `git show 9297d83^:crates/rightclaw/src/config.rs`:

1. `fn chrome_config_roundtrip()`
2. `fn write_global_config_emits_chrome_section()`
3. `fn read_config_no_chrome_section_is_none()`
4. `fn read_config_with_chrome_section_parses()`
5. `fn read_config_chrome_empty_fields_errors()`
6. `fn write_then_read_with_tunnel_and_chrome()`
7. `fn write_global_config_no_chrome_omits_section()`

Note: The ChromeConfig struct was moved/reordered in the current file. Test functions reference the struct by name, not position, so they should compile. The error handling behavior changed slightly (`.filter().map()` vs `.map().transpose()?`) — verify test 5 (`empty_fields_errors`) still matches current behavior.

---

## Step 7: Restore AGENTS.md MCP Management Sections

### 7a. `identity/AGENTS.md`

Add back the "MCP Management" section (from `9297d83^` version) — should go before "Core Skills":

```markdown
## MCP Management

To install, remove, or authorize MCP servers at runtime, use the `rightmemory` MCP tools:

- `mcp_add(name, url)` -- add an HTTP MCP server to `.claude.json`
- `mcp_remove(name)` -- remove an MCP server (rightmemory itself is protected)
- `mcp_list()` -- list all configured MCP servers (no tokens exposed)
- `mcp_auth(server_name)` -- get the OAuth authorization URL for a server; send the link to the user via Telegram to complete auth

Never edit `.claude.json` directly -- always use these tools.
```

### 7b. `templates/right/AGENTS.md`

Add back the same "MCP Management" section (from `9297d83^` version).

---

## Step 8: Build and Test

```bash
# 1. Build entire workspace
cargo build --workspace

# 2. Run all tests
cargo test --workspace

# 3. Specifically verify restored MCP tests compile and pass
cargo test -p rightclaw-cli -- mcp

# 4. Verify Chrome tests still pass
cargo test -p rightclaw -- chrome
cargo test -p rightclaw -- config

# 5. Verify no regressions
cargo clippy --workspace
```

---

## Step 9: Commit

Single commit with all restored files:
```
fix: restore v3.3 MCP tools + planning artifacts deleted by 9297d83

Wave 1 executor agent for Phase 43 ran on main working tree and
committed mass deletions alongside the Chrome feature code. This
restores:

- v3.3 MCP Self-Management code: mcp_add/remove/list/auth in
  rightmemory server (4 functions, 328 lines of tests)
- Phase 42 planning: 9 files (PLAN, SUMMARY, CONTEXT, RESEARCH)
- Phase 43 planning: 4 files (43-01-PLAN, CONTEXT, RESEARCH)
- v3.3 milestone: REQUIREMENTS.md, ROADMAP.md
- v3.3 phase: 01-mcp-management-tools-in-rightmemory-server (8 files)
- MILESTONES.md, RETROSPECTIVE.md, ROADMAP.md v3.3+v3.4 sections
- REQUIREMENTS.md (v3.4 Chrome requirements)
- Chrome research files (ARCHITECTURE, FEATURES, PITFALLS, STACK)
- config.rs Chrome tests (7 functions)
- mcp_config.rs Chrome tests (2 functions + 5 assertions)
- identity/AGENTS.md + templates/right/AGENTS.md MCP Management section
- reqwest dependency in rightclaw-cli

Chrome integration code (phases 42+43) is preserved as-is.
```

---

## Files Modified (summary)

| Category | Files | Action |
|----------|-------|--------|
| Planning restore (git show) | 24 files | Create from `9297d83^` |
| Planning merge | 7 files (ROADMAP, REQUIREMENTS, MILESTONES, RETROSPECTIVE, PROJECT, STATE, research x4) | Edit to add back deleted sections |
| Code restore | `memory_server.rs` | Edit: add fields, constructor args, 4 functions, 4 param types |
| Code restore | `memory_server_mcp_tests.rs` | Create from `9297d83^` (renamed) |
| Code restore | `mcp_config.rs` | Edit: add 2 test functions + 5 assertions |
| Code restore | `config.rs` | Edit: add 7 test functions |
| Dependency | `Cargo.toml` (rightclaw-cli) | Edit: add `reqwest` |
| Docs | `identity/AGENTS.md`, `templates/right/AGENTS.md` | Edit: add MCP Management section |
