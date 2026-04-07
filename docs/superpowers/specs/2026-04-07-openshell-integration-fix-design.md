# Fix OpenShell Integration — Real API

**Date:** 2026-04-07
**Status:** Draft
**Scope:** Fix broken OpenShell integration — sandbox.rs, worker.rs, readiness polling, error propagation

## Problem

The OpenShell integration was built around `openshell sandbox exec` which doesn't exist. The real CLI has no exec subcommand. Command execution inside sandboxes requires SSH. Readiness polling requires gRPC (no JSON CLI output, no reflection).

Errors from openshell commands are silently lost — not propagated to rightclaw agent logs in process-compose.

## Real OpenShell API

| Command | Purpose |
|---|---|
| `sandbox create --name X --policy Y --no-tty [--upload PATH]` | Create sandbox, blocks (babysitter process) |
| `sandbox ssh-config NAME` | Print SSH Host config block for `openshell-NAME` |
| `sandbox upload NAME LOCAL [DEST]` | Upload files (dest treated as **directory**) |
| `sandbox delete NAME` | Delete sandbox |
| `sandbox list` | List sandboxes (table, no JSON) |
| `sandbox get NAME` | Get sandbox details (colored text, no JSON) |

**No exec.** Commands run via SSH: `ssh -F <config> openshell-NAME -- command`.

**No structured output.** CLI prints colored text only. Status must be queried via gRPC.

**gRPC API** on `127.0.0.1:8080` with mTLS:
- `GetSandbox(name)` returns `Sandbox` with `phase: SandboxPhase` where `READY = 2`
- mTLS certs at `~/.config/openshell/gateways/openshell/mtls/{ca.crt, tls.crt, tls.key}`
- Server does NOT support gRPC reflection — proto files required at compile time

**Upload gotcha:** `sandbox upload NAME file.txt /sandbox/dest` treats dest as **directory**, creates `/sandbox/dest/file.txt`. Use `--upload` flag on `create` for correct initial file placement.

## Solution

### 1. Proto files in repo + tonic gRPC client

Copy OpenShell proto files to `proto/openshell/`:
- `openshell.proto` — service definition (GetSandbox, ListSandboxes, Health)
- `datamodel.proto` — Sandbox, SandboxPhase enum
- `sandbox.proto` — SandboxPolicy, FilesystemPolicy, etc.

Add `build.rs` to the `rightclaw` crate to compile protos via `tonic-build`. Generate client-only code (no server). Note: `sandbox.proto` imports `google/protobuf/struct.proto` — tonic-build handles this via `prost-types` (well-known types compiled automatically).

New dependencies in rightclaw crate:
```toml
tonic = "0.14"
prost = "0.14"
# build-dependencies:
tonic-build = "0.14"
```

### 2. New module: openshell.rs

Replace `codegen/sandbox.rs` with a proper OpenShell client module that owns:

**gRPC client:**
- `OpenShellGrpcClient` — wraps tonic `OpenShellClient<Channel>` with mTLS
- `connect(mtls_dir)` — creates mTLS channel to `127.0.0.1:8080`
- `get_sandbox(name)` — returns `Option<Sandbox>` with phase
- `wait_for_ready(name, timeout)` — polls `get_sandbox` until phase is READY

**CLI wrappers (thin, no output parsing):**
- `spawn_sandbox(name, policy, upload_dir)` — returns `tokio::process::Child`, spawns `create --no-tty`
- `generate_ssh_config(name, config_dir)` — writes SSH config file, returns path
- `delete_sandbox(name)` — runs `sandbox delete`

**SSH command execution:**
- `ssh_exec(ssh_host, ssh_config, cmd, timeout)` — returns `Output`, runs command via SSH
- `ssh_host(agent_name)` — returns `"openshell-rightclaw-{agent_name}"`

### 3. Fix worker.rs — SSH instead of exec

Replace `openshell sandbox exec` with:
```
ssh -F <config> openshell-rightclaw-{agent} -- claude -p ...
```

Add `ssh_config_path: PathBuf` to `WorkerContext`.

### 4. Fix cmd_up — sandbox lifecycle

Current broken flow:
```
create_sandbox -> upload_file -> (exec per message)
```

Fixed flow:
```
1. spawn_sandbox(name, policy, staging_dir)    # background child, --upload for initial files
2. grpc_client.wait_for_ready(name, 60s)       # poll via tonic gRPC
3. generate_ssh_config(name, run_dir)           # write SSH config for bot
4. (bot uses SSH for claude -p per message)
```

On shutdown (cmd_down):
```
1. delete_sandbox(name)
2. process-compose shutdown
```

### 5. Error propagation

**Problem:** openshell errors (stderr) are captured by piped stdio but never logged.

**Fix:** In `ssh_exec` and `spawn_sandbox`, always log stderr on failure via tracing::error. In `invoke_cc` (worker.rs), SSH stderr must be captured and included in error replies to Telegram.

### 6. Delete legacy code

| Code | Action |
|---|---|
| `codegen/sandbox.rs` | Delete — replaced by `openshell.rs` |
| `codegen/mod.rs` `pub mod sandbox` | Remove |
| All `exec_in_sandbox` references | Replace with `ssh_exec` |
| `build_exec_args` | Delete |

### 7. Doctor checks update

Replace grpcurl check with:
- openshell binary check (already done)
- Docker daemon check (already done)
- OpenShell mTLS certs existence check
- OpenShell gateway health check via gRPC Health RPC

## File changes summary

| File | Action |
|---|---|
| `proto/openshell/openshell.proto` | **New:** copied from OpenShell repo |
| `proto/openshell/datamodel.proto` | **New:** copied from OpenShell repo |
| `proto/openshell/sandbox.proto` | **New:** copied from OpenShell repo |
| `crates/rightclaw/build.rs` | **New:** tonic-build proto compilation |
| `crates/rightclaw/Cargo.toml` | Add tonic, prost deps + build-dependencies |
| `crates/rightclaw/src/openshell.rs` | **New:** gRPC client, CLI wrappers, SSH exec |
| `crates/rightclaw/src/lib.rs` | Add `pub mod openshell;` |
| `crates/rightclaw/src/codegen/sandbox.rs` | Delete |
| `crates/rightclaw/src/codegen/mod.rs` | Remove `pub mod sandbox` |
| `crates/bot/src/telegram/worker.rs` | SSH instead of openshell exec |
| `crates/bot/src/lib.rs` | Pass ssh_config_path to WorkerContext |
| `crates/rightclaw-cli/src/main.rs` | Fix cmd_up (gRPC wait_for_ready), cmd_down |
| `crates/rightclaw/src/doctor.rs` | Add mTLS + gRPC health checks |

## Testing

- **openshell.rs gRPC:** Unit test mTLS channel creation (mock or skip if no gateway). Integration test wait_for_ready with real OpenShell (ignored by default).
- **openshell.rs SSH:** Unit test command construction. Integration test SSH exec (ignored by default).
- **worker.rs:** Verify SSH command args construction.
- **cmd_up/cmd_down:** Integration test full lifecycle (ignored by default).

## Risks

- **Proto file drift:** OpenShell updates proto, our copy gets stale. Mitigated: proto is stable (v1), doctor can check gateway version.
- **mTLS cert location:** Hardcoded ~/.config/openshell/. Mitigated: standard OpenShell convention, doctor validates.
- **SSH ProxyCommand:** SSH uses openshell ssh-proxy as ProxyCommand. If openshell binary not in PATH for SSH subprocess, it fails. Mitigated: doctor checks openshell in PATH.
