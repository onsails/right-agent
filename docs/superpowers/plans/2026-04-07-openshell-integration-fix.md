# Fix OpenShell Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the broken OpenShell integration by replacing non-existent `openshell sandbox exec` with SSH, adding tonic gRPC readiness polling with mTLS, and wiring OpenShell lifecycle into cmd_up/cmd_down/invoke_cc.

**Architecture:** Proto files compiled at build time via tonic-build. New `openshell.rs` module provides gRPC client (mTLS) for readiness polling, SSH for command execution, and thin CLI wrappers for sandbox lifecycle. Bot spawns sandbox at startup, polls readiness via gRPC, generates SSH config, and uses SSH for each claude -p invocation.

**Tech Stack:** tonic 0.14, prost 0.14, tonic-build 0.14 (build-dep), SSH via tokio process

---

### Task 1: Copy proto files and add tonic-build

**Files:**
- Create: `proto/openshell/openshell.proto`
- Create: `proto/openshell/datamodel.proto`
- Create: `proto/openshell/sandbox.proto`
- Create: `crates/rightclaw/build.rs`
- Modify: `Cargo.toml` (workspace deps)
- Modify: `crates/rightclaw/Cargo.toml`

- [ ] **Step 1: Copy proto files**

Copy from /tmp/OpenShell/proto/ to proto/openshell/ in the rightclaw repo. Only copy openshell.proto, datamodel.proto, sandbox.proto (skip inference.proto, test.proto).

- [ ] **Step 2: Add tonic and prost workspace dependencies**

In root Cargo.toml [workspace.dependencies] add:
- tonic = "0.14"
- prost = "0.14"
- prost-types = "0.14"

In crates/rightclaw/Cargo.toml [dependencies] add:
- tonic.workspace = true
- prost.workspace = true  
- prost-types.workspace = true

In crates/rightclaw/Cargo.toml add [build-dependencies]:
- tonic-build = "0.14"

- [ ] **Step 3: Create build.rs for proto compilation**

Create crates/rightclaw/build.rs that uses tonic_build::configure() with build_server(false) to compile the three proto files. Include path should be "../../proto/openshell". Proto paths relative to crate root: "../../proto/openshell/openshell.proto" etc.

Note: sandbox.proto imports google/protobuf/struct.proto. tonic-build handles well-known types via prost-types automatically. If the import fails, download struct.proto into proto/google/protobuf/ and add "../../proto" as an additional include path.

- [ ] **Step 4: Verify proto compilation**

Run: cargo check -p rightclaw
Expected: PASS

- [ ] **Step 5: Commit**

Stage proto/, build.rs, both Cargo.toml files, Cargo.lock.
Message: "chore: add OpenShell proto files + tonic-build for gRPC client generation"

---

### Task 2: Create openshell.rs module with gRPC client

**Files:**
- Create: `crates/rightclaw/src/openshell.rs`
- Modify: `crates/rightclaw/src/lib.rs`

- [ ] **Step 1: Add proto module includes and openshell module to lib.rs**

Add pub mod openshell and an openshell_proto module with tonic::include_proto! calls for the three packages: openshell.v1, openshell.datamodel.v1, openshell.sandbox.v1.

- [ ] **Step 2: Create openshell.rs with naming helpers and tests**

Functions: sandbox_name(agent_name) returning "rightclaw-{agent_name}" and ssh_host(agent_name) returning "openshell-rightclaw-{agent_name}". Tests for both.

- [ ] **Step 3: Add gRPC client functions**

Add connect_grpc(mtls_dir) that creates a tonic Channel with ClientTlsConfig (CA cert + client identity) connecting to https://127.0.0.1:8080. Returns OpenShellClient.

Add is_sandbox_ready(client, name) that calls GetSandbox RPC and checks phase == SANDBOX_PHASE_READY (value 2 as i32).

Add wait_for_ready(client, name, timeout_secs, poll_interval_secs) that polls is_sandbox_ready until true or timeout.

- [ ] **Step 4: Verify compilation**

Run: cargo check -p rightclaw

- [ ] **Step 5: Commit**

Message: "feat: add openshell.rs with gRPC client (mTLS) and readiness polling"

---

### Task 3: Add SSH and CLI wrappers to openshell.rs

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs`

- [ ] **Step 1: Add spawn_sandbox function**

Takes name, policy_path, optional upload_dir. Spawns "openshell sandbox create --name NAME --policy PATH --no-tty [--upload DIR]" as a tokio child process. Returns Child handle. kill_on_drop(false) so sandbox survives if parent dies.

- [ ] **Step 2: Add generate_ssh_config function**

Runs "openshell sandbox ssh-config NAME", writes stdout to config_dir/{name}.ssh-config. Returns path.

- [ ] **Step 3: Add ssh_exec function**

Runs "ssh -F CONFIG HOST -- CMD..." with timeout. Captures stdout+stderr. Logs stderr on failure via tracing::error.

- [ ] **Step 4: Add delete_sandbox function**

Runs "openshell sandbox delete NAME". Best-effort, logs warning on failure.

- [ ] **Step 5: Verify compilation and commit**

Run: cargo check -p rightclaw
Message: "feat: add SSH exec, spawn_sandbox, ssh-config, delete to openshell.rs"

---

### Task 4: Delete old codegen/sandbox.rs

**Files:**
- Delete: `crates/rightclaw/src/codegen/sandbox.rs`
- Modify: `crates/rightclaw/src/codegen/mod.rs`

- [ ] **Step 1: Remove pub mod sandbox from codegen/mod.rs**

- [ ] **Step 2: Delete the file**

rm crates/rightclaw/src/codegen/sandbox.rs

- [ ] **Step 3: Fix any references to codegen::sandbox**

Search for codegen::sandbox in the workspace. Replace with openshell:: equivalents. Also check the refresh scheduler (refresh.rs) which may call codegen::sandbox::upload_file.

- [ ] **Step 4: Verify compilation and commit**

Run: cargo check --workspace
Message: "refactor: delete old codegen/sandbox.rs, replaced by openshell.rs"

---

### Task 5: Fix worker.rs — SSH instead of openshell exec

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add ssh_config_path to WorkerContext**

Find WorkerContext struct or equivalent. Add ssh_config_path: std::path::PathBuf field.

- [ ] **Step 2: Replace openshell exec with SSH in invoke_cc**

In invoke_cc() around line 400-411, replace:
- rightclaw::codegen::sandbox::sandbox_name with rightclaw::openshell::ssh_host
- Command::new("openshell").args(["sandbox", "exec", ...]) with Command::new("ssh").args(["-F", config, host, "--"])

Keep all claude_args construction unchanged. Keep stdin(null), stdout(piped), stderr(piped), kill_on_drop(true).

- [ ] **Step 3: Verify compilation and commit**

Run: cargo check --workspace
Message: "fix: invoke_cc uses SSH instead of non-existent openshell exec"

---

### Task 6: Wire OpenShell lifecycle into cmd_up

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`

- [ ] **Step 1: Add OpenShell sandbox creation after per-agent file generation**

After the existing per-agent loop (around line 878), add:
1. Connect gRPC client (connect_grpc with mTLS certs from ~/.config/openshell/...)
2. Create ssh config directory under run_dir
3. For each agent: delete stale sandbox, generate policy, spawn_sandbox with --upload, wait_for_ready via gRPC, generate SSH config

- [ ] **Step 2: Add RIGHTMEMORY_PORT constant**

const RIGHTMEMORY_PORT: u16 = 8100;

- [ ] **Step 3: Verify compilation and commit**

Run: cargo check --workspace
Message: "feat: wire OpenShell sandbox lifecycle into cmd_up"

---

### Task 7: Wire OpenShell cleanup into cmd_down

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`

- [ ] **Step 1: Add sandbox deletion before process-compose shutdown**

In cmd_down, after reading state, iterate agents and call openshell::delete_sandbox for each. Best-effort, log warnings on failure.

- [ ] **Step 2: Verify compilation and commit**

Run: cargo check --workspace
Message: "feat: cmd_down deletes OpenShell sandboxes"

---

### Task 8: Update doctor checks

**Files:**
- Modify: `crates/rightclaw/src/doctor.rs`

- [ ] **Step 1: Add mTLS certs check**

Check existence of ca.crt, tls.crt, tls.key in ~/.config/openshell/gateways/openshell/mtls/. Severity: Fail.

- [ ] **Step 2: Add gateway health check via gRPC**

Call connect_grpc then Health RPC. If run_doctor is sync, wrap in tokio runtime::block_on or make run_doctor async.

- [ ] **Step 3: Verify compilation and commit**

Run: cargo check --workspace
Message: "feat: add OpenShell mTLS + gateway health doctor checks"

---

### Task 9: Pass SSH config path to bot

**Files:**
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Resolve SSH config path in bot entry**

In bot lib.rs during initialization, compute ssh_config_path from rightclaw_home/run/ssh/{sandbox}.ssh-config. Verify it exists (error if not — rightclaw up not run).

- [ ] **Step 2: Thread ssh_config_path to WorkerContext**

Follow how agent_dir and agent_name reach invoke_cc. Add ssh_config_path alongside them.

- [ ] **Step 3: Verify compilation, run tests, commit**

Run: cargo check --workspace
Run: cargo test --workspace -- --skip test_status
Message: "feat: pass SSH config path from bot init to invoke_cc"

---

### Task 10: Full workspace build + clippy + test

- [ ] **Step 1: Build**

Run: cargo build --workspace

- [ ] **Step 2: Clippy**

Run: cargo clippy --workspace -- -D warnings

- [ ] **Step 3: Test**

Run: cargo test --workspace -- --skip test_status_no_running

- [ ] **Step 4: Commit any fixes**

Message: "chore: fix clippy warnings from OpenShell integration fix"
