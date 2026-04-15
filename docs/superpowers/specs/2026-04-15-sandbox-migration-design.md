# Sandbox Migration, Backup & Restore

## Problem

OpenShell Landlock filesystem policies are set at sandbox creation time and cannot be hot-reloaded. When a user changes filesystem policy (e.g. adding new writable paths), the only way to apply it is to create a new sandbox. The current system has no mechanism for this — sandbox names are deterministic (`rightclaw-{agent_name}`), there's no way to create a second sandbox for the same agent, and no way to transfer files between sandboxes.

Additionally, there is no backup/restore capability for agent sandboxes, which means no disaster recovery and no way to clone agents.

## Design

### Data Model

**agent.yaml** gains an optional `name` field under `sandbox:`:

```yaml
sandbox:
  mode: openshell
  policy_file: policy.yaml
  name: "rightclaw-myagent-20260415-1430"  # optional, overrides deterministic default
```

**SandboxConfig** in `agent/types.rs`:

```rust
pub struct SandboxConfig {
    pub mode: SandboxMode,
    pub policy_file: Option<PathBuf>,
    pub name: Option<String>,  // explicit sandbox name
}
```

**Name resolution:** new function `resolve_sandbox_name(agent_name: &str, config: &AgentConfig) -> String`. Returns `config.sandbox.name` if set, otherwise falls back to `rightclaw-{agent_name}`. Replaces all calls to `sandbox_name()` across the codebase.

**Naming scheme for new sandboxes:** `rightclaw-{agent_name}-{YYYYMMDD-HHMM}` (timestamp-based, human-readable in `openshell sandbox list`).

**Backward compatibility:** agents without `sandbox.name` in agent.yaml work exactly as before via the deterministic fallback.

### Backup

**CLI:** `rightclaw agent backup <name> [--sandbox-only]`

**Storage:** `~/.rightclaw/backups/<agent>/<YYYYMMDD-HHMM>/`

| Mode | Contents |
|------|----------|
| full (default) | `sandbox.tar.gz`, `agent.yaml`, `data.db`, `policy.yaml` |
| sandbox-only | `sandbox.tar.gz` |

**Hot backup — no agent downtime required.**

**Flow (sandbox mode):**

1. Discover agent, resolve sandbox name from config.
2. Verify sandbox exists and READY via gRPC `GetSandbox`.
3. Create backup directory `~/.rightclaw/backups/<agent>/<YYYYMMDD-HHMM>/`.
4. SSH tar: `ssh <sandbox-ssh-host> tar czpf - /sandbox/ > sandbox.tar.gz`. The `-p` flag preserves file permissions.
5. Full mode (default): additionally copy `agent.yaml`, `policy.yaml` from agent dir. Copy `data.db` via SQLite `VACUUM INTO` for a consistent snapshot under concurrent writes.

**Flow (no-sandbox mode):**

1. Create backup directory.
2. `tar czpf sandbox.tar.gz` of the agent directory, excluding `data.db`.
3. Full mode: `VACUUM INTO` for `data.db`, copy `agent.yaml`, `policy.yaml`.

**No automatic retention/pruning.** User manages backups manually.

### Restore (via agent init)

**CLI:** `rightclaw agent init <name> --from-backup <path>`

Also available interactively when running `rightclaw agent init <name>` without flags — the wizard offers "Restore from backup" as an option alongside "Create fresh". ("Copy from existing agent" is a future enhancement, out of scope for this spec.)

**Preconditions:**

- Agent `<name>` must NOT exist in `~/.rightclaw/agents/`. If it does, fail with: "Agent '<name>' already exists. Delete it first with `rightclaw agent delete <name>`."
- Backup directory must contain `sandbox.tar.gz` and `agent.yaml`.

**Flow:**

1. Validate preconditions.
2. Create agent directory `~/.rightclaw/agents/<name>/`.
3. Restore `agent.yaml`, `data.db`, `policy.yaml` from backup.
4. Create new sandbox `rightclaw-<name>-<YYYYMMDD-HHMM>` with the restored policy.
5. Wait for READY + SSH ready.
6. Restore files: `ssh <new-sandbox-host> tar xzpf - < sandbox.tar.gz` — unpacks into `/sandbox/`.
7. Write new `sandbox.name` into agent.yaml.
8. Run standard codegen (settings.json, mcp.json, etc.).
9. Initial sync (platform store) deploys to `/platform/`.

**Restoring into a different agent name is allowed.** Internal references to the old name (in IDENTITY.md, memory, git history) become stale — the agent adapts on its own.

**No-sandbox restore:** same flow but skip sandbox creation (steps 4-6). Unpack `sandbox.tar.gz` directly into the agent directory on the host.

### Migration (filesystem policy change)

**Trigger:** `rightclaw agent config <name>` when the user changes a setting that affects `filesystem_policy` or `landlock` sections of policy.yaml.

**Detection via gRPC:**

1. Generate new policy.yaml from updated config.
2. Fetch active policy from sandbox: `GetSandboxPolicyStatus(name, version: 0)` — returns `SandboxPolicyRevision` with optional full `policy` field.
3. Parse `filesystem_policy` + `landlock` sections from both the active (gRPC) and new (local) policies.
4. Compare: if filesystem/landlock differ → migration required. If only network differs → hot-reload sufficient.
5. Fallback: if gRPC does not return the full policy field, treat any policy change as requiring migration (safe default).

**Migration flow:**

1. Backup sandbox-only of current sandbox.
2. Create new sandbox `rightclaw-<agent>-<YYYYMMDD-HHMM>` with new policy.
3. Wait for READY + SSH ready.
4. Restore: `ssh <new-sandbox-host> tar xzpf - < sandbox.tar.gz`.
5. Initial sync (platform store) into new sandbox.
6. Write new `sandbox.name` into agent.yaml.
7. Delete old sandbox (best-effort via `delete_sandbox`, then `wait_for_deleted`).
8. config_watcher detects agent.yaml change → bot restarts → picks up new sandbox.

**Rollback on failure:** if restore into the new sandbox fails, delete the new sandbox (best-effort), leave old sandbox and agent.yaml untouched, report error.

**Network-only change:** no migration needed. `rightclaw agent config` writes agent.yaml + policy.yaml. config_watcher triggers bot restart. Bot regenerates policy and calls `apply_policy()` (hot-reload via `openshell policy set --wait`). This is the existing behavior, unchanged.

### Bot Changes

**Single change:** replace `sandbox_name(&args.agent)` with `resolve_sandbox_name(&args.agent, &config)` at startup. The bot does not perform migration, backup, or restore. All sandbox lifecycle management is in the CLI.

Everything else in the bot remains unchanged: config_watcher, exit code 2, codegen, initial_sync, apply_policy for hot-reload.

### What Moves, What Doesn't

| Path | Migrated? | Why |
|------|-----------|-----|
| `/sandbox/` (entire tree) | Yes | Agent-owned data: .claude/, crons/, inbox/, outbox/, git repos, agent-created files |
| `/platform/` | No | Regenerated by `initial_sync` on every bot startup (content-addressed, symlinked) |
| `/tmp/` | No | Ephemeral by definition |

### Backup Directory Layout

```
~/.rightclaw/backups/<agent>/<YYYYMMDD-HHMM>/
  sandbox.tar.gz     # tar czpf with -p (preserve permissions) of /sandbox/
  agent.yaml         # (full mode only)
  data.db            # (full mode only, via VACUUM INTO)
  policy.yaml        # (full mode only)
```
