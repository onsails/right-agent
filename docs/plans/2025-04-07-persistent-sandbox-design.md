# Persistent Sandbox Design

## Problem

Current sandbox lifecycle is ephemeral: deleted and recreated on every bot restart. Since OpenShell sandboxes are k3s containers (not bind-mounted host dirs), all agent work inside the sandbox — code, git commits, modified SOUL/IDENTITY/AGENTS files — is lost on restart. Additionally, host `.credentials.json` is copied into sandbox, which is both a security leak and causes `ERR_BAD_REQUEST` when the token expires.

## Design Decisions

### Sandbox Lifecycle

- Sandboxes are **persistent** — never deleted automatically.
- On bot startup: check if sandbox exists and is READY. If yes, reuse. If not, create.
- Sandbox deletion only via explicit user action (`/sandbox reset` or `rightclaw sandbox delete`).
- Policy updates applied via `openshell policy set <sandbox> --policy <file> --wait` — no sandbox recreation needed.

### Credential Isolation

- Host `.credentials.json` is **never** uploaded to sandbox.
- Sandbox obtains its own credentials via the login flow (already implemented): auth error detected → `login-{agent}` process started → OAuth URL sent to Telegram → user authenticates → credentials written inside sandbox.

### File Sync Strategy

| # | File | Strategy | When |
|---|------|----------|------|
| 1 | `.credentials.json` | **Never upload** — sandbox gets own via login flow | — |
| 2 | `settings.json` | **Periodic sync** — upload from host every 5 min | Background timer |
| 3 | `.claude.json` | **Periodic verify+fix** — download from sandbox, check rightclaw-managed keys (`hasTrustDialogAccepted`, `hasCompletedOnboarding`), fix if CC overwrote them, re-upload | Same 5 min timer |
| 4 | `agents/<name>.md` | **One-time** — upload at sandbox creation only | Sandbox create |
| 5 | `reply-schema.json` | **Periodic sync** — upload from host every 5 min | Same 5 min timer |
| 6 | `.mcp.json` | **Host = source of truth** — upload after `/mcp add/remove` and on token refresh | On change + refresh timer |
| 7 | `plugins/` | **Removed** — no longer upload plugins, Telegram CC plugin not used | — |
| 8 | `skills/` | **Only rightclaw builtins** — upload only rightclaw/rightmemory skill files, not entire skills/ tree | Sandbox create + periodic sync |
| 9 | `shell-snapshots/` | **Removed** — CC creates this directory itself inside sandbox | — |

### Policy Hot-Reload

- `rightclaw up` generates policy.yaml as before.
- If sandbox already exists: `openshell policy set <sandbox> --policy <file> --wait` instead of delete+recreate.
- If sandbox doesn't exist: create with `--upload` as before (minus excluded files).

### Sync Routine

Background tokio task in the bot process, runs every 5 minutes:

1. Upload `settings.json` from host to `/sandbox/.claude/settings.json`
2. Download `/sandbox/.claude.json` via `openshell sandbox download`, verify rightclaw keys, fix if needed, re-upload
3. Upload `reply-schema.json` from host to `/sandbox/.claude/reply-schema.json`
4. Upload rightclaw builtin skills from host to `/sandbox/.claude/skills/`

### MCP Flow (unchanged)

- `/mcp add/remove` writes `.mcp.json` on host → needs upload to sandbox after write (currently missing — must add)
- Token refresh updates Bearer in `.mcp.json` on host → re-uploads to sandbox (already implemented)
- `oauth-state.json` stays on host only

### Initial Sandbox Creation (revised)

When sandbox doesn't exist, `--upload` staging dir contains:
- `settings.json`
- `.claude.json` (generated, no host credentials)
- `agents/<name>.md`
- `reply-schema.json`
- `.mcp.json`
- Rightclaw builtin skills only

**Excluded** from staging:
- `.credentials.json` (any symlink or file)
- `plugins/` directory
- `shell-snapshots/` directory
- Non-rightclaw skills

### Login Flow (already implemented)

When `claude -p` returns 403/401 inside sandbox:
1. `is_auth_error()` detects auth failure in CC JSON output
2. Bot starts `login-{agent}` process in process-compose
3. Auth watcher scrapes OAuth URL from PC logs, sends to Telegram
4. User clicks URL, authenticates
5. Watcher probes `claude -p "say ok"` every 10s
6. On success: stops login process, notifies user

### Open Questions

- Should `/mcp add/remove` immediately upload `.mcp.json` to sandbox, or wait for next sync cycle?
- Does `openshell sandbox download` exist as a CLI command? Need to verify for `.claude.json` verify+fix flow.
- How to detect if sandbox already exists at bot startup — gRPC `GetSandbox` (already implemented in `is_sandbox_ready`)?
