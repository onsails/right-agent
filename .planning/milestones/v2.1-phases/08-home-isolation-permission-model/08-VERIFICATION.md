---
phase: 08-home-isolation-permission-model
verified: 2026-03-24T22:30:00Z
status: passed
score: 16/16 must-haves verified
gaps: []
human_verification:
  - test: "Launch rightclaw up with a real agent, then run the generated wrapper script"
    expected: "claude binary sees HOME=$AGENT_DIR — verify via `echo $HOME` inside a claude tool invocation"
    why_human: "Can only verify HOME override effect when CC actually launches; static analysis confirms the export is present and ordered correctly"
  - test: "Launch rightclaw up, then confirm .claude/.credentials.json symlink resolves to live token"
    expected: "Symlink exists, is not dangling, and claude can authenticate without ANTHROPIC_API_KEY"
    why_human: "Requires live Claude credentials and a running agent session"
---

# Phase 8: Home Isolation and Permission Model — Verification Report

**Phase Goal:** Implement per-agent HOME directory isolation so each Claude Code agent reads agent-local config instead of host config, with credential symlinks for OAuth, and sandbox path hardening to use absolute paths.
**Verified:** 2026-03-24T22:30:00Z
**Status:** PASSED
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Shell wrapper sets HOME to agent directory before launching claude | VERIFIED | `templates/agent-wrapper.sh.j2` line 15: `export HOME="{{ working_dir }}"` |
| 2 | Shell wrapper forwards git/SSH identity env vars from real environment | VERIFIED | Lines 7-11 in wrapper: GIT_CONFIG_GLOBAL, GIT_AUTHOR_NAME, GIT_AUTHOR_EMAIL, SSH_AUTH_SOCK, GIT_SSH_COMMAND — all with `:-` fallback |
| 3 | Shell wrapper forwards ANTHROPIC_API_KEY when set | VERIFIED | `agent-wrapper.sh.j2` line 12: `export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"` |
| 4 | rightclaw up generates per-agent .claude.json with hasTrustDialogAccepted | VERIFIED | `main.rs` line 347: `rightclaw::codegen::generate_agent_claude_json(agent)?` in per-agent loop |
| 5 | rightclaw up creates credential symlink from agent .claude/.credentials.json to host credentials | VERIFIED | `main.rs` line 350: `rightclaw::codegen::create_credential_symlink(agent, &host_home)?` |
| 6 | rightclaw up warns (not errors) when host credentials file is missing | VERIFIED | `claude_json.rs` lines 92-101: returns `Ok(())` after `eprintln!` warning; test `test_credential_symlink_warns_when_no_host_creds` passes |
| 7 | --dangerously-skip-permissions remains in shell wrapper | VERIFIED | `agent-wrapper.sh.j2` line 32: `--dangerously-skip-permissions`; regression test `wrapper_retains_dangerously_skip_permissions` passes |
| 8 | rightclaw init writes per-agent .claude.json instead of host ~/.claude.json (D-06) | VERIFIED | `init.rs` lines 166-180: builds `trust_agent` AgentDef and calls `crate::codegen::generate_agent_claude_json(&trust_agent)?`; `pre_trust_directory()` completely absent from codebase |
| 9 | Generated denyRead paths use absolute host HOME paths, not tilde-relative | VERIFIED | `settings.rs` lines 59-65: `host_home.join(".ssh")` etc., no tilde literals; test `deny_read_uses_absolute_paths_not_tilde` passes |
| 10 | Generated settings include allowRead with absolute agent path | VERIFIED | `settings.rs` line 45: `let mut allow_read = vec![agent.path.display().to_string()]`; test `includes_allow_read_with_agent_path` passes |
| 11 | SandboxOverrides supports user-defined allow_read paths in agent.yaml | VERIFIED | `types.rs` lines 34-36: `pub allow_read: Vec<String>` with `#[serde(default)]`; test `sandbox_overrides_deserializes_allow_read` passes |
| 12 | User-defined allow_read paths merge with default agent path | VERIFIED | `settings.rs` line 53: `allow_read.extend(overrides.allow_read.iter().cloned())`; test `merges_user_allow_read_overrides` passes |
| 13 | generate_settings() callers in init.rs and main.rs pass host_home parameter | VERIFIED | `init.rs` line 106: `generate_settings(&agent_def, false, &host_home)`; `main.rs` line 334: `generate_settings(agent, no_sandbox, &host_home)` |
| 14 | Integration tests verify per-agent .claude.json contains hasTrustDialogAccepted | VERIFIED | `home_isolation.rs`: `init_agent_claude_json_has_trust` — passes |
| 15 | Integration tests verify credential symlink exists and points to host creds | VERIFIED | `home_isolation.rs`: `init_agent_credentials_is_symlink` — passes |
| 16 | Integration tests verify missing host creds produces warning, not error | VERIFIED | `home_isolation.rs`: `init_warns_when_host_creds_missing` with fake HOME — passes |

**Score:** 16/16 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `templates/agent-wrapper.sh.j2` | HOME override and env var forwarding | VERIFIED | Contains `export HOME="{{ working_dir }}"` and 6 forwarded env vars with `:-` fallback |
| `crates/rightclaw/src/codegen/claude_json.rs` | Per-agent .claude.json generation | VERIFIED | Exports `generate_agent_claude_json` and `create_credential_symlink`; 8 unit tests; 331 lines |
| `crates/rightclaw/src/codegen/mod.rs` | claude_json module exported | VERIFIED | `pub mod claude_json` + re-exports both functions |
| `crates/rightclaw/src/codegen/settings.rs` | Absolute denyRead + allowRead + host_home param | VERIFIED | `host_home: &Path` parameter; dynamic denyRead build; allowRead array |
| `crates/rightclaw/src/agent/types.rs` | SandboxOverrides with allow_read | VERIFIED | `pub allow_read: Vec<String>` field present with serde default |
| `crates/rightclaw-cli/tests/home_isolation.rs` | Integration tests for HOME isolation | VERIFIED | 6 non-ignored tests, 2 ignored scaffold tests; all 6 pass |
| `crates/rightclaw/src/init.rs` | Per-agent .claude.json via generate_agent_claude_json; no pre_trust_directory | VERIFIED | Lines 166-184 call both codegen functions; `pre_trust_directory` absent from entire codebase |
| `crates/rightclaw-cli/src/main.rs` | host_home + generate_agent_claude_json + create_credential_symlink in cmd_up | VERIFIED | Lines 297-350: host_home before loop, both calls inside loop |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `main.rs` | `codegen/claude_json.rs` | `generate_agent_claude_json(agent)` in cmd_up per-agent loop | WIRED | Line 347 |
| `main.rs` | `std::os::unix::fs::symlink` | `create_credential_symlink(agent, &host_home)` in cmd_up | WIRED | Line 350; underlying symlink in claude_json.rs line 85 |
| `init.rs` | `codegen/claude_json.rs` | `generate_agent_claude_json(&trust_agent)` in init_rightclaw_home | WIRED | Line 180 |
| `init.rs` | `codegen/claude_json.rs` | `create_credential_symlink(&trust_agent, &host_home)` in init | WIRED | Line 184 — added by Plan 02 deviation fix |
| `main.rs` | `codegen/settings.rs` | `generate_settings(agent, no_sandbox, &host_home)` | WIRED | Line 334 |
| `init.rs` | `codegen/settings.rs` | `generate_settings(&agent_def, false, &host_home)` | WIRED | Line 106 |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| HOME-01 | 08-01 | Shell wrapper sets HOME=$AGENT_DIR | SATISFIED | `agent-wrapper.sh.j2` line 15; test `wrapper_contains_home_override` |
| HOME-02 | 08-01 | rightclaw up generates per-agent .claude.json with hasTrustDialogAccepted | SATISFIED | `main.rs` + `claude_json.rs`; integration test `init_agent_claude_json_has_trust` |
| HOME-03 | 08-01 | rightclaw up symlinks host OAuth credentials to each agent | SATISFIED | `main.rs` + `claude_json.rs::create_credential_symlink`; integration tests |
| HOME-04 | 08-01 | Shell wrapper forwards git/SSH identity env vars | SATISFIED | `agent-wrapper.sh.j2` lines 7-11; test `wrapper_contains_git_env_forwarding` |
| HOME-05 | 08-02 | Generated sandbox paths use absolute paths (not ~/ relative) | SATISFIED | `settings.rs` dynamic denyRead + allowRead; unit and integration tests confirm no tilde |
| PERM-01 | 08-01 | Shell wrapper keeps --dangerously-skip-permissions | SATISFIED | `agent-wrapper.sh.j2` line 32; regression test `wrapper_retains_dangerously_skip_permissions` |
| PERM-02 | 08-01 | Pre-populate .claude.json with bypass-accepted state | SATISFIED | `generate_agent_claude_json` writes `hasTrustDialogAccepted: true`; agent-local settings.json has `skipDangerousModePermissionPrompt: true` |

**Orphaned requirements check:** No phase-8 requirements in REQUIREMENTS.md lack a plan claim. All 7 IDs (HOME-01..05, PERM-01, PERM-02) are claimed by 08-01 or 08-02 and verified above.

**Note on HOME-05 wording:** REQUIREMENTS.md says "allowWrite paths use absolute paths" but the implementation (correctly) also fixes denyRead paths and adds allowRead. The requirement text is narrower than what was delivered. Implementation exceeds the requirement — not a gap.

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| — | — | — | — | — |

No stubs, TODOs, placeholders, or empty implementations found in phase-8 files. The `#[ignore]` tests in `home_isolation.rs` are intentional scaffolds for live credential testing — not stubs blocking the goal.

### Human Verification Required

#### 1. HOME Override Runtime Effect

**Test:** Run `rightclaw up`, then from within a claude tool invocation, execute `echo $HOME`
**Expected:** Output matches agent directory path (e.g. `~/.rightclaw/agents/right`), not `/home/username`
**Why human:** Static analysis confirms the export is correctly placed before `exec`, but only a live process can confirm the env var is visible inside CC's tool execution context

#### 2. OAuth Authentication Under HOME Override

**Test:** Run `rightclaw up` on a machine with host `~/.claude/.credentials.json`; verify credential symlink exists at `$AGENT_DIR/.claude/.credentials.json`; then run a claude command without ANTHROPIC_API_KEY set
**Expected:** Claude authenticates successfully via the symlinked credentials
**Why human:** Requires live credentials; the symlink creation is verified programmatically but the OAuth handshake is not

### Test Results

```
rightclaw library tests:    127 passed, 0 failed
shell_wrapper tests:         13 passed, 0 failed  (includes 6 Phase 8 HOME tests)
claude_json unit tests:       8 passed, 0 failed
home_isolation integration:   6 passed, 0 failed, 2 ignored (live credential scaffolds)
workspace build:              clean
```

---

_Verified: 2026-03-24T22:30:00Z_
_Verifier: Claude (gsd-verifier)_
