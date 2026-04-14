# Design: bootstrap_done sandbox-aware file verification

## Date
2026-04-14

## Problem

`call_bootstrap_done()` in `right_backend.rs` checks for IDENTITY.md, SOUL.md, and USER.md on the **host filesystem** (`self.agents_dir.join(agent_name)`). But sandboxed agents create these files inside the OpenShell sandbox at `/sandbox/`. The files only reach the host after `reverse_sync_md` runs post-session. Result: `bootstrap_done` always fails mid-session for sandboxed agents.

The agent retries multiple times, wastes turns, and eventually gives up. The worker's post-session safety net (blocking `reverse_sync_md` + host file check) eventually completes bootstrap, but the agent never gets a success signal during the session.

## Design

### RightBackend gets optional mTLS dir

`RightBackend` currently holds only `agents_dir: PathBuf`. Add `mtls_dir: Option<PathBuf>`:

- `Some(path)` — agent runs in OpenShell sandbox; `bootstrap_done` checks files via gRPC exec.
- `None` — agent runs on host (`sandbox: mode: none`); `bootstrap_done` checks files on host (current behavior).

### Initialization (main.rs aggregator setup)

At agent registration time (main.rs ~line 434-466):

1. Parse `agent.yaml` via `parse_agent_config(&agent_dir)`.
2. Check `config.sandbox_mode()`.
3. If `SandboxMode::Openshell` — pass `Some(mtls_dir.clone())` to `RightBackend::new()`.
4. If `SandboxMode::None` — pass `None`.

The `mtls_dir` is already available from `openshell::preflight_check()` at aggregator startup. If preflight fails (OpenShell not running), `mtls_dir` is `None` — fall back to host check.

### call_bootstrap_done logic

```
fn call_bootstrap_done(agent_name):
    if self.mtls_dir is Some(mtls_dir):
        sandbox_name = format!("rightclaw-{agent_name}")
        client = connect_grpc(mtls_dir)
        sandbox_id = resolve_sandbox_id(client, sandbox_name)
        for file in [IDENTITY.md, SOUL.md, USER.md]:
            exec(client, sandbox_id, ["test", "-f", "/sandbox/{file}"])
            if exit_code != 0: mark as missing
        if all present:
            remove BOOTSTRAP.md from host
            return success
        else:
            return error listing missing files
    else:
        // current host-side logic unchanged
```

### Key decisions

- **sandbox_id resolved on the fly** — `bootstrap_done` is called once per agent lifetime, no need to cache. Resolving on each call handles sandbox recreation.
- **BOOTSTRAP.md deleted on host** — this is a signal for the bot worker (host-side process), stays on host.
- **gRPC connect per call** — acceptable overhead for a one-time operation.
- **Preflight failure = host fallback** — if OpenShell gRPC is unreachable, fall back to host check rather than hard-failing. This keeps `--no-sandbox` and degraded environments working.

## Files to change

| File | Change |
|------|--------|
| `crates/rightclaw-cli/src/right_backend.rs` | Add `mtls_dir: Option<PathBuf>` field, update `new()`, make `call_bootstrap_done` async, add sandbox file check via gRPC exec |
| `crates/rightclaw-cli/src/main.rs` | Parse `agent.yaml` at aggregator init, pass `mtls_dir` based on sandbox mode |
| `crates/rightclaw-cli/src/aggregator.rs` | No change — dispatch already passes `agent_name` to `tools_call` |
| `crates/rightclaw-cli/src/right_backend_tests.rs` | Add test for sandbox-aware bootstrap_done (mock or integration) |

## Testing

- Unit test: `RightBackend::new(agents_dir, None)` — host-side check works as before.
- Integration test: create ephemeral sandbox, write files inside it, call `bootstrap_done` — should succeed.
- Integration test: ephemeral sandbox without files — should return error listing missing files.
