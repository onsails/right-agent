---
phase: 02-cli-runtime-and-sandboxing
verified: 2026-03-22T17:30:00Z
status: passed
score: 5/5 must-haves verified
re_verification: false
---

# Phase 2: CLI Runtime and Sandboxing Verification Report

**Phase Goal:** Users can launch, monitor, and stop sandboxed agents with a single CLI command
**Verified:** 2026-03-22T17:30:00Z
**Status:** passed
**Re-verification:** No -- initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | `rightclaw up` generates shell wrappers and process-compose.yaml, then launches agents inside OpenShell sandboxes | VERIFIED | `cmd_up` in main.rs calls `generate_wrapper` per agent (with openshell invocation in template), `generate_process_compose`, writes files to run/, spawns `process-compose up` |
| 2 | `rightclaw up --agents a,b` launches only named agents; `rightclaw up -d` launches in background | VERIFIED | `--agents` with `value_delimiter = ','` filters discovered agents; `--detach`/`-d` adds `--detached-with-tui` flag and health-checks after spawn |
| 3 | `rightclaw status` shows agent states; `rightclaw restart <agent>` restarts single agent | VERIFIED | `cmd_status` calls `PcClient::list_processes()` and prints table; `cmd_restart` calls `PcClient::restart_process()` |
| 4 | `rightclaw attach` connects to running process-compose TUI | VERIFIED | Uses `std::os::unix::process::CommandExt` to replace current process with `process-compose attach --unix-socket` |
| 5 | `rightclaw down` stops all agents and explicitly destroys OpenShell sandboxes | VERIFIED | `cmd_down` reads state, calls `PcClient::shutdown()`, then `destroy_sandboxes()` which runs `openshell sandbox delete` per agent; skips if `no_sandbox` was used |

**Score:** 5/5 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `templates/agent-wrapper.sh.j2` | Shell wrapper template with openshell | VERIFIED | 21 lines, contains `openshell sandbox create`, `--dangerously-skip-permissions`, `--append-system-prompt-file`, sandbox naming `rightclaw-{{ agent_name }}`, no-sandbox branch |
| `templates/process-compose.yaml.j2` | process-compose config template | VERIFIED | 18 lines, contains `version: "0.5"`, `is_strict: true`, `processes:` loop with restart/backoff/shutdown config |
| `crates/rightclaw/src/codegen/shell_wrapper.rs` | Shell wrapper generation | VERIFIED | Exports `generate_wrapper`, uses `include_str!` to embed template, renders with minijinja, handles no_sandbox and default start_prompt |
| `crates/rightclaw/src/codegen/process_compose.rs` | process-compose.yaml generation | VERIFIED | Exports `generate_process_compose`, maps `RestartPolicy` to strings, uses `include_str!` for template, `ProcessAgent` context struct |
| `crates/rightclaw/src/codegen/mod.rs` | Codegen module re-exports | VERIFIED | Re-exports `generate_wrapper` and `generate_process_compose` |
| `crates/rightclaw/src/runtime/pc_client.rs` | process-compose REST API client | VERIFIED | `PcClient` struct with `unix_socket` transport, async methods: `health_check`, `list_processes`, `restart_process`, `stop_process`, `shutdown` |
| `crates/rightclaw/src/runtime/sandbox.rs` | Sandbox tracking and cleanup | VERIFIED | `RuntimeState`/`AgentState` with serde, `write_state`/`read_state` JSON persistence, `destroy_sandboxes` calls `openshell sandbox delete`, `sandbox_name_for` returns `rightclaw-{name}` |
| `crates/rightclaw/src/runtime/deps.rs` | Dependency verification | VERIFIED | Checks `process-compose`, `claude`, `openshell` (skipped if `no_sandbox`) via `which::which()` with actionable error messages |
| `crates/rightclaw/src/runtime/mod.rs` | Runtime module re-exports | VERIFIED | Re-exports PcClient, ProcessInfo, RuntimeState, AgentState, destroy_sandboxes, write_state, read_state, sandbox_name_for, verify_dependencies |
| `crates/rightclaw-cli/src/main.rs` | All CLI subcommands wired | VERIFIED | 389 lines, `#[tokio::main]`, Commands enum with Up/Down/Status/Restart/Attach, dedicated `cmd_*` handler functions |
| `crates/rightclaw-cli/tests/cli_integration.rs` | Integration tests | VERIFIED | 14 tests total, 8 new for Phase 2 covering help text, error paths (no socket, no state file) |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `codegen/shell_wrapper.rs` | `templates/agent-wrapper.sh.j2` | `include_str!` | WIRED | `include_str!("../../../../templates/agent-wrapper.sh.j2")` |
| `codegen/process_compose.rs` | `templates/process-compose.yaml.j2` | `include_str!` | WIRED | `include_str!("../../../../templates/process-compose.yaml.j2")` |
| `codegen/shell_wrapper.rs` | `agent/types.rs` | uses AgentDef | WIRED | `use crate::agent::AgentDef` |
| `runtime/pc_client.rs` | reqwest | unix_socket | WIRED | `reqwest::Client::builder().unix_socket(socket_path)` |
| `runtime/sandbox.rs` | openshell CLI | Command::new | WIRED | `Command::new("openshell").args(["sandbox", "delete", ...])` |
| `runtime/deps.rs` | which crate | which::which() | WIRED | Checks `process-compose`, `claude`, `openshell` |
| `main.rs` | codegen module | generate_wrapper, generate_process_compose | WIRED | `rightclaw::codegen::generate_wrapper(agent, no_sandbox)` and `rightclaw::codegen::generate_process_compose(&agents, &run_dir)` |
| `main.rs` | runtime module | PcClient, verify_dependencies, destroy_sandboxes | WIRED | `rightclaw::runtime::verify_dependencies()`, `PcClient::new()`, `destroy_sandboxes()`, `write_state()`, `read_state()` |
| `main.rs` | agent module | discover_agents | WIRED | `rightclaw::agent::discover_agents(&agents_dir)` |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| CLI-01 | 02-01, 02-03 | `rightclaw up <project-path>` scans agents, generates config, launches | SATISFIED | `cmd_up` discovers agents, generates wrappers + PC config, spawns process-compose |
| CLI-02 | 02-03 | `rightclaw up --agents watchdog,reviewer` launches specific agents | SATISFIED | `--agents` with `value_delimiter = ','` filters agent list |
| CLI-03 | 02-03 | `rightclaw up -d` launches in background with TUI server | SATISFIED | `-d`/`--detach` flag adds `--detached-with-tui` |
| CLI-04 | 02-03 | `rightclaw attach` connects to running TUI | SATISFIED | `cmd_attach` uses `CommandExt` with `process-compose attach` |
| CLI-05 | 02-02 | `rightclaw status` shows agent states | SATISFIED | `cmd_status` calls `PcClient::list_processes()`, prints NAME/STATUS/PID/UPTIME table |
| CLI-06 | 02-02 | `rightclaw restart <agent>` restarts single agent | SATISFIED | `cmd_restart` calls `PcClient::restart_process(agent)` |
| CLI-07 | 02-03 | `rightclaw down` stops agents and destroys sandboxes | SATISFIED | `cmd_down` calls `shutdown()` then `destroy_sandboxes()` per agent |
| SAND-01 | 02-01 | Each agent launches inside OpenShell sandbox with YAML policy | SATISFIED | Template generates `openshell sandbox create --policy <path>` per agent |
| SAND-02 | 02-01 | Shell wrapper reads policy and invokes openshell | SATISFIED | `agent-wrapper.sh.j2` contains full openshell invocation with policy path and sandbox name |
| SAND-03 | 02-02 | `rightclaw down` explicitly destroys sandboxes | SATISFIED | `destroy_sandboxes()` iterates agents, calls `openshell sandbox delete {sandbox_name}` |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| None | - | - | - | No anti-patterns found in Phase 2 code |

### Human Verification Required

### 1. End-to-end agent launch

**Test:** Install process-compose, openshell, and claude CLI. Run `rightclaw init` then `rightclaw up -d --no-sandbox` (or with sandbox if openshell available). Verify agents appear in process-compose TUI.
**Expected:** Agents start, `rightclaw status` shows them as Running, `rightclaw attach` connects to TUI, `rightclaw down` stops everything.
**Why human:** Requires external tools (process-compose, openshell, claude) not available in CI.

### 2. OpenShell sandbox enforcement

**Test:** With openshell installed, run `rightclaw up` and verify agents are inside sandboxes. Run `openshell sandbox list` to confirm `rightclaw-<agent>` sandboxes exist. Run `rightclaw down` and verify sandboxes are destroyed.
**Expected:** Sandboxes created on up, destroyed on down.
**Why human:** Requires openshell alpha software installed and running.

### 3. Detached mode stability

**Test:** Run `rightclaw up -d`, wait, then `rightclaw attach`. Verify TUI is responsive. Detach, then `rightclaw down`.
**Expected:** Background process survives shell detach, TUI attach works.
**Why human:** Requires live process-compose daemon and terminal interaction.

### Gaps Summary

No gaps found. All 5 success criteria verified through code inspection. All 10 requirement IDs (CLI-01 through CLI-07, SAND-01 through SAND-03) are satisfied by substantive, wired implementations. All 77 workspace tests pass (14 integration tests, rest unit tests). No stubs, no TODOs, no placeholder implementations in Phase 2 code.

---

_Verified: 2026-03-22T17:30:00Z_
_Verifier: Claude (gsd-verifier)_
