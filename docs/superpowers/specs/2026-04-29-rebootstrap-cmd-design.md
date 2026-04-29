# `right agent rebootstrap` — Design

**Date:** 2026-04-29
**Status:** Approved (awaiting implementation plan)

## Summary

Add a `right agent rebootstrap <name> [-y]` subcommand that puts an existing
agent back into bootstrap mode for debugging, without destroying its sandbox,
credentials, memory bank, or `data.db`. The command:

1. Backs up the host and sandbox copies of `IDENTITY.md`, `SOUL.md`, `USER.md`
   to `~/.right/backups/<agent>/rebootstrap-<YYYYMMDD-HHMM>/`.
2. Deletes those files from both host and sandbox.
3. Recreates `BOOTSTRAP.md` on host (the bootstrap-mode flag).
4. Deactivates all active CC sessions for the agent so the next message starts
   a new one (rows preserved, `is_active = 0`).
5. Bounces the agent's process-compose worker (`<name>-bot`) so a fresh
   codegen cycle picks up the new state.

The sandbox stays alive throughout. The agent's Hindsight memory bank, file
attachments, cron specs, and OAuth credentials are not touched.

## Motivation

Debugging the bootstrap conversation today requires destroying the agent
(`right agent destroy` + `right agent init`). That throws away the sandbox,
credentials, and any installed state — far more than necessary just to
re-trigger the onboarding flow.

This subcommand is the minimal "rewind to bootstrap" primitive: it inverts
exactly the state that bootstrap completion mutates (`BOOTSTRAP.md` removed +
identity files written + session continuity), and nothing else.

## Non-goals

- A formal "restore from rebootstrap backup" subcommand. The backup dir is the
  manual recovery path; YAGNI until we observe actual demand.
- Wiping the Hindsight memory bank or file-mode `MEMORY.md`. That is "killing
  the agent" territory and out of scope here.
- Touching `AGENTS.md` or `TOOLS.md`. Those are agent-owned but not part of the
  bootstrap loop — wiping them would leave the agent in a different broken
  state, not a fresh one.
- Resetting cron specs, attachments inbox/outbox, MCP server registrations,
  Telegram allowlist, or any other persistent agent state.

## Surface

```
right agent rebootstrap <name> [-y]
```

- `<name>` — agent dir under `$RIGHT_HOME/agents/`. Error if missing.
- `-y` / `--yes` — skip the typed-name confirmation prompt.

The confirmation prompt mirrors `agent destroy`: prints what will happen + the
backup destination, then asks the operator to type the agent's name verbatim.
Wrong name or Ctrl+C aborts before any side effect.

Help text:

> Re-enter bootstrap mode for an agent. Backs up identity files, deletes them
> from host and sandbox, recreates BOOTSTRAP.md, and deactivates active
> sessions so the next message starts fresh. Sandbox, credentials, memory
> bank, and data.db are preserved.

## Step Sequence

Order matters; each step assumes the previous succeeded. The whole operation
is best-effort idempotent — re-running after a partial failure converges to
the desired state.

1. **Resolve and validate.** Load agent dir from `$RIGHT_HOME/agents/<name>`,
   error if missing. Read `agent.yaml` to determine `sandbox.mode` and
   `sandbox.name`. Compute backup dir
   `$RIGHT_HOME/backups/<name>/rebootstrap-<YYYYMMDD-HHMM>/`.

2. **Confirm.** Skipped if `-y`. Otherwise typed-name prompt. Abort cleanly on
   mismatch.

3. **Stop the bot via process-compose.** `PcClient::from_home(home)`:
   - `Some(pc)` → `pc.stop_process("<name>-bot")`, poll until stopped (~5s
     timeout). Mark `bot_was_stopped = true`. If stop fails, abort the whole
     operation — running it with the bot up courts a reverse-sync race.
   - `None` (PC not running) → INFO log "process-compose not running, skipping
     bot stop", continue with `bot_was_stopped = false`.

4. **Backup identity files.** Create `<backup_dir>/`. For each of
   `IDENTITY.md`, `SOUL.md`, `USER.md`:
   - If host copy exists → copy to `<backup_dir>/<file>`.
   - If `sandbox.mode != none` AND sandbox is reachable AND copy exists in
     `/sandbox/` → download to `<backup_dir>/sandbox/<file>` via
     `openshell::download_file`.

   Missing files are logged at DEBUG and not treated as errors. Record what
   was actually backed up so the final report can show it.

5. **Delete identity files from sandbox.** Skipped if `sandbox.mode = none` or
   sandbox unreachable. Single `openshell::exec_in_sandbox` invocation:
   `rm -f /sandbox/IDENTITY.md /sandbox/SOUL.md /sandbox/USER.md`. Failure
   aborts (otherwise reverse-sync would re-populate the host copies on the
   next message and we'd silently undo our own work).

6. **Delete identity files from host.** `fs::remove_file` for each path;
   "not found" is fine.

7. **Recreate `BOOTSTRAP.md` on host.** Write
   `right_agent::codegen::BOOTSTRAP_INSTRUCTIONS` to
   `<agent_dir>/BOOTSTRAP.md`. The constant doubles as the on-disk content
   and the system-prompt injection, so they stay in lockstep.

8. **Deactivate active sessions.** Skipped if `data.db` is missing. Open via
   `memory::open_connection(agent_dir, false)`. The `data.db` is already
   scoped to a single agent, so a single statement covers all active
   sessions:

   ```sql
   UPDATE sessions SET is_active = 0 WHERE is_active = 1
   ```

   Use `Connection::execute` and return the affected row count for the
   report. No transaction needed (single statement).

   We deliberately do **not** call `right-bot`'s
   `telegram::session::deactivate_current` — `right-agent` doesn't depend on
   `right-bot` (and shouldn't, that would be a cycle), and the per-
   `(chat_id, thread_id)` scoping there is the wrong shape for rebootstrap
   anyway.

9. **Restart the bot.** Only if step 3 set `bot_was_stopped = true`:
   `pc.start_process("<name>-bot")`. Don't wait for full readiness — confirm
   the start request was accepted and move on.

10. **Print report.** Backup path, what was backed up (host vs sandbox), how
    many sessions deactivated, restart status, hint to run `right up` if PC
    was down.

### Asymmetry: stop vs start

If step 3 found PC down, step 9 doesn't try to start anything. The operator
runs `right up` themselves. This is deliberate — the command shouldn't
accidentally launch process-compose as a side effect.

### No cross-step atomicity

If step 5 succeeds but step 7 fails, the agent is briefly in a degraded state
(no identity, no `BOOTSTRAP.md`). Restarting via `right restart` re-runs
codegen; another `rebootstrap` invocation converges. Acceptable for a debug
tool — formal atomicity isn't worth the complexity.

## Module Layout

```
crates/right-agent/src/rebootstrap.rs    (new)
crates/right-agent/src/lib.rs            (add: pub mod rebootstrap;)
crates/right/src/main.rs                 (add AgentCommands::Rebootstrap variant
                                          + cmd_agent_rebootstrap)
```

`BOOTSTRAP.md` and identity files are `AgentOwned` per ARCHITECTURE.md, so
they bypass the `codegen::contract` writer registry. No registry changes
needed.

### Public surface — `right_agent::rebootstrap`

```rust
pub struct RebootstrapPlan {
    pub agent_name: String,
    pub agent_dir: PathBuf,
    pub backup_dir: PathBuf,
    pub sandbox_mode: SandboxMode,
    pub sandbox_name: Option<String>,
}

pub struct RebootstrapReport {
    pub backup_dir: PathBuf,
    pub host_backed_up: Vec<&'static str>,
    pub sandbox_backed_up: Vec<&'static str>,
    pub sessions_deactivated: usize,
    pub bot_was_stopped: bool,
}

pub fn plan(home: &Path, agent_name: &str) -> miette::Result<RebootstrapPlan>;
pub async fn execute(plan: &RebootstrapPlan) -> miette::Result<RebootstrapReport>;
```

Internal step helpers (private, callable from in-file tests):

- `backup_identity_files(plan) -> miette::Result<(Vec<&'static str>, Vec<&'static str>)>`
- `delete_identity_from_sandbox(plan) -> miette::Result<()>`
- `delete_identity_from_host(plan)` — infallible (best-effort)
- `write_bootstrap_md(plan) -> miette::Result<()>`
- `deactivate_active_sessions(plan) -> miette::Result<usize>`

Errors are `miette::Result` throughout — consistent with `init::run` and the
rest of the module's neighbors. No discriminated `thiserror` enum; callers
don't need to branch on failure kind.

### CLI shim — `cmd_agent_rebootstrap`

The shim handles PC orchestration (consistent with `cmd_restart` /
`cmd_up` patterns); the library module stays focused on state mutations.

```text
1. let plan = right_agent::rebootstrap::plan(&home, &name)?;
2. confirmation prompt unless -y
3. stop bot via PcClient (record bot_was_stopped)
4. let report = right_agent::rebootstrap::execute(&plan).await?;
5. if report.bot_was_stopped: pc.start_process(...)
6. print report
```

## Edge Cases

| Case | Behavior |
|---|---|
| Agent has never bootstrapped (no IDENTITY/SOUL/USER on host or sandbox) | Backup dir contains nothing; INFO log "no identity files to back up"; `BOOTSTRAP.md` still recreated; sessions still deactivated. Useful for resetting a stuck agent. |
| `BOOTSTRAP.md` already exists | Overwritten with the canonical constant. Idempotent. |
| `sandbox.mode = none` | Skip steps 4 (sandbox download) and 5 (sandbox delete). Host-side steps run normally. |
| Sandbox unreachable (never created or down) | Skip steps 4 (sandbox download) and 5 (sandbox delete) with INFO log "sandbox unreachable, skipped sandbox-side cleanup". Host steps run normally. |
| `process-compose` not running | Skip stop+start; INFO log "PC not running — run `right up` to relaunch the bot". Other steps proceed. |
| `data.db` missing | Skip step 8. |
| Wrong agent name typed at confirm prompt | Abort before any side effect. |
| Concurrent `rebootstrap` for the same agent | Not handled — the user is responsible for not running two debug operations against the same agent simultaneously. The bot stop in step 3 makes this very unlikely to cause real damage. |

## Testing

### 1. Unit tests in `rebootstrap.rs` (in-file `#[cfg(test)]`)

- `backup_identity_files` — temp agent dir with subset of files; backup dir
  contains exactly those files; missing files are not errors.
- `delete_identity_from_host` — temp dir → all removed; second invocation is
  a no-op (idempotent).
- `write_bootstrap_md` — content equals `BOOTSTRAP_INSTRUCTIONS`; overwrites
  existing file.
- `deactivate_active_sessions` — open in-memory `data.db` via
  `memory::open_connection` with migrations, seed two active sessions for
  two `(chat_id, thread_id)` pairs, call helper, assert both rows have
  `is_active = 0` and the function returned `2`.
- `plan` — happy path resolves dirs from a temp `$RIGHT_HOME`; error path
  on missing agent dir.

### 2. Integration test (`crates/right-agent/tests/rebootstrap_sandbox.rs`)

One end-to-end test against a live OpenShell sandbox via
`right_agent::test_support::TestSandbox::create("rebootstrap")`. No
`#[ignore]` — dev machines have OpenShell, per project convention.

Setup:

- Create test sandbox.
- Upload `IDENTITY.md` / `SOUL.md` / `USER.md` to `/sandbox/` and write
  matching files to a temp host agent dir.
- Build a `RebootstrapPlan` pointing at the temp dir + sandbox.

Action:

- Call `execute(&plan).await`.

Assertions:

- All three identity files absent from `/sandbox/` (verify via
  `TestSandbox::exec` with `ls -la`).
- All three identity files absent from host agent dir.
- `BOOTSTRAP.md` present on host with content equal to
  `BOOTSTRAP_INSTRUCTIONS`.
- Backup dir contains both `<file>` and `sandbox/<file>` for each.

### 3. CLI integration test (`crates/right/tests/`, `assert_cmd`)

- `right agent rebootstrap missing-agent` — non-zero exit; stderr mentions
  agent name.
- `right agent rebootstrap <name>` with no `-y` and stdin closed — aborts
  cleanly without mutating the agent dir (verified by reading the dir
  before/after).

The full happy path is **not** re-tested through `assert_cmd` because that
would require both a running PC and a live sandbox; the layer-2 sandbox test
already covers it.

### Out of scope

- PC stop/start orchestration in isolation (already exercised by
  `cmd_restart` and `cmd_up`).
- Bot post-rebootstrap behavior (covered by existing `should_accept_bootstrap_*`
  worker tests and the bootstrap mode prompt-assembly path).

## Future Work (Not Now)

- A `--restore <backup-dir>` flag or sister subcommand if manual restoration
  proves common.
- An optional `--reset-memory` flag that also wipes the agent's Hindsight
  bank. Probably better as a separate command (`right agent forget` or
  similar) since it's a different destruction class.

## Cross-references

- `crates/right-agent/src/init.rs` — agent init flow that creates
  `BOOTSTRAP.md` and the rest of the agent dir.
- `crates/right-agent/src/codegen/agent_def.rs` — `BOOTSTRAP_INSTRUCTIONS`
  const (single source of truth).
- `crates/bot/src/telegram/worker.rs` — bootstrap mode detection,
  `should_accept_bootstrap`, completion path that mirrors what we're
  inverting.
- `crates/bot/src/telegram/session.rs` — `deactivate_current`,
  `create_session`.
- `crates/bot/src/sync.rs` — `reverse_sync_md` (the function that would
  re-populate identity files if we forgot to clean the sandbox copy).
- `ARCHITECTURE.md` — `Upgrade & Migration Model` (codegen categories,
  AgentOwned definition).
