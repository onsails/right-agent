# Claude Code Auto-Upgrade in Sandboxes

## Problem

Claude Code binary in OpenShell sandboxes is baked into the image at `/usr/local/bin/claude` (read-only). The image is immutable after sandbox creation — no in-place image updates. Agents run stale CC versions until sandbox is recreated.

## Solution

A background tokio task in the bot that runs `claude upgrade` inside the sandbox every 8 hours via SSH.

## How `claude upgrade` Works in Sandboxes

Verified experimentally on `rightclaw-rightclaw-test-lifecycle`:

1. `claude upgrade` downloads a native binary to `/sandbox/.local/share/claude/versions/<ver>`
2. Creates symlink `/sandbox/.local/bin/claude` → the new version
3. Original `/usr/local/bin/claude` (from image) stays untouched (read-only `/usr`)
4. RightClaw's `sync.rs` already adds `/sandbox/.local/bin` to PATH in `.bashrc` — placed before `/usr/local/bin`, so upgraded binary takes precedence
5. Next `claude -p` invocation picks up the new binary automatically — no bot restart needed

## Requirements

### Network Policy

`storage.googleapis.com:443` must be reachable from sandbox with `tls: terminate`. This is the distribution CDN for Claude Code native binaries.

Add to `codegen/policy.rs` `RESTRICTIVE_DOMAINS`:
```
storage.googleapis.com
```

Permissive mode already covers all domains via wildcard.

### Binary Permission

The `claude_code` network policy needs `binaries: path: "**"` or at minimum `/usr/local/bin/claude` + `/usr/bin/node` + `/sandbox/.local/bin/claude`. RightClaw's generated policies already use `path: "**"`.

## Implementation

### New File: `crates/bot/src/upgrade.rs`

Single public function:

```rust
pub async fn spawn_upgrade_task(
    ssh_config_path: PathBuf,
    agent_name: String,
    shutdown: CancellationToken,
)
```

Behavior:
- `tokio::time::interval(Duration::from_secs(8 * 3600))` — first tick fires immediately on bot start
- Each tick: run `ssh -F <config> <host> -- claude upgrade` with 120s timeout
- Parse stdout for version info
- Log via tracing: `info!` on success/no-update, `error!` on failure
- Never panic or propagate errors — log and wait for next tick
- Respects `CancellationToken` for graceful shutdown

### Policy Change: `crates/rightclaw/src/codegen/policy.rs`

Add `storage.googleapis.com` to `RESTRICTIVE_DOMAINS`.

### Integration: `crates/bot/src/lib.rs`

Spawn alongside existing background tasks (cron, sync). Only when `ssh_config_path.is_some()` (sandbox mode).

## Testing

### Integration Test: `claude upgrade` in sandbox

New test in the existing sandbox integration test suite (likely `tests/` or alongside lifecycle tests).

Prerequisites: a running sandbox with `storage.googleapis.com` in policy.

1. **Test upgrade runs without error**: exec `claude upgrade` via SSH, assert exit code 0
2. **Test binary appears in `.local/bin`**: after upgrade, verify `/sandbox/.local/bin/claude` exists and is a symlink
3. **Test upgraded binary is usable**: run `/sandbox/.local/bin/claude --version`, assert it returns a valid semver
4. **Test PATH precedence**: run `bash -lc 'which claude'` and verify it resolves to `/sandbox/.local/bin/claude` (not `/usr/local/bin/claude`) — requires `.bashrc` PATH setup from sync

These tests use the real sandbox (not mocked) since they verify network policy + filesystem + binary installation end-to-end.

## Not Included

- No Telegram notification on upgrade (tracing only)
- No configurable interval in agent.yaml (hardcoded 8h)
- No no-sandbox support (user manages host binary)
- No bot restart after upgrade
