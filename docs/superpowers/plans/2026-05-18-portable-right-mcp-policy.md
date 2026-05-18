# Portable Right MCP Policy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Right MCP sandbox-to-host access portable by generating bootstrap policy before sandbox create, then resolving every sandbox-visible `host.openshell.internal` IP and hot-applying exact `allowed_ips` before any agent invocation.

**Architecture:** Right codegen emits a bootstrap Right MCP endpoint without broad private fallback ranges. Once a sandbox exists, callers resolve `host.openshell.internal` from inside that sandbox with `getent ahosts`, generate IPv4 `/32` and IPv6 `/128` allowlists, and hot-apply the exact policy. Bot startup, fresh init, agent init, restore, and sandbox migration all follow the same bootstrap -> exact apply lifecycle.

**Tech Stack:** Rust 2024, `right-codegen`, `right-openshell` gRPC exec helpers, OpenShell `policy set --wait`, `serde_saphyr` YAML parsing, Cargo tests through `devenv shell --`.

---

## File Structure

- Modify `crates/right-codegen/src/policy.rs`: add `HostMcpAccess`, generate bootstrap/exact Right MCP endpoint, update policy tests.
- Modify `crates/right-codegen/src/pipeline.rs`: write generated policy to `config.resolve_policy_path()` and use bootstrap mode during codegen.
- Modify `crates/right-openshell/src/openshell.rs`: add `parse_getent_ahosts_ips` and replace `resolve_host_ip` with `resolve_host_ips`.
- Modify `crates/right-openshell/src/openshell_tests.rs`: add parser tests.
- Modify `crates/bot/src/lib.rs`: use multi-IP exact policy at startup and fail if resolution fails.
- Modify `crates/right/src/main.rs`: apply exact policy after sandbox create in `right init`, `right agent init`, restore, and sandbox migration create flows.
- Modify `ARCHITECTURE.md`, `docs/architecture/sandbox.md`, `docs/architecture/lifecycle.md`, `docs/architecture/mcp.md`: document the policy lifecycle.

## Task 1: Policy API And Unit Tests

**Files:**
- Modify: `crates/right-codegen/src/policy.rs`

- [x] **Step 1: Write failing policy tests**

Add tests for bootstrap omission of guessed private ranges, resolved IPv4 `/32`, resolved IPv6 `/128`, and rejection of empty exact IP lists.

- [x] **Step 2: Verify red**

Run:

```bash
devenv shell -- cargo test -p right-codegen right_mcp_policy
```

Expected: FAIL because `HostMcpAccess` does not exist.

- [x] **Step 3: Implement minimal policy API**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostMcpAccess {
    BootstrapUnresolved,
    Resolved(Vec<std::net::IpAddr>),
}
```

Generate no `allowed_ips` block for bootstrap mode and exact CIDRs for resolved mode. IPv4 uses `/32`; IPv6 uses `/128`.

- [x] **Step 4: Verify green**

Run:

```bash
devenv shell -- cargo test -p right-codegen policy
```

## Task 2: Sandbox Host IP Parser And Resolver

**Files:**
- Modify: `crates/right-openshell/src/openshell.rs`
- Modify: `crates/right-openshell/src/openshell_tests.rs`

- [x] **Step 1: Write failing parser tests**

Add tests for duplicate `STREAM/DGRAM/RAW` rows, mixed IPv4/IPv6, malformed first tokens, and empty/no-valid output.

- [x] **Step 2: Verify red**

Run:

```bash
devenv shell -- cargo test -p right-openshell parse_getent_ahosts_ips
```

Expected: FAIL because parser does not exist.

- [x] **Step 3: Implement parser and resolver**

Add `pub(crate) fn parse_getent_ahosts_ips(stdout: &str) -> Vec<IpAddr>`. Replace `resolve_host_ip` with `resolve_host_ips`, executing:

```rust
&["getent", "ahosts", "host.openshell.internal"]
```

If exec exits non-zero or parsed IP list is empty, return a hard error. Log all resolved IPs.

- [x] **Step 4: Verify green**

Run:

```bash
devenv shell -- cargo test -p right-openshell parse_getent_ahosts_ips
```

## Task 3: Codegen Bootstrap Policy And Active Policy Path

**Files:**
- Modify: `crates/right-codegen/src/pipeline.rs`
- Modify: `crates/right-codegen/src/policy.rs`

- [x] **Step 1: Write failing codegen tests**

Add tests that `run_single_agent_codegen` writes a bootstrap Right MCP policy without broad ranges and writes to `sandbox.policy_file` when configured.

- [x] **Step 2: Verify red**

Run:

```bash
devenv shell -- cargo test -p right-codegen run_single_agent_codegen
```

- [x] **Step 3: Implement codegen changes**

Use `config.resolve_policy_path(&agent.path)?` when available. Generate with `HostMcpAccess::BootstrapUnresolved`.

- [x] **Step 4: Verify green**

Run:

```bash
devenv shell -- cargo test -p right-codegen run_single_agent_codegen
```

## Task 4: Runtime Exact Policy Apply

**Files:**
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/right/src/main.rs`

- [x] **Step 1: Update startup call sites**

Use `resolve_host_ips` and `HostMcpAccess::Resolved(host_ips)` before bot dispatch and before any Claude invocation. Resolution/apply failure must fail startup instead of silently retaining stale policy.

- [x] **Step 2: Factor CLI helper**

In `crates/right/src/main.rs`, add an async helper that receives `policy_path`, `sandbox_name`, and `network_policy`, resolves the sandbox ID/IPs, generates exact policy, and calls `write_and_apply_sandbox_policy`.

- [x] **Step 3: Call helper after create**

Use the helper after sandbox readiness in `right init`, `right agent init`, sandboxed restore, and sandbox migration create flow.

- [x] **Step 4: Verify targeted compile/tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen -p right-openshell -p right --lib
```

## Task 5: Restore/Stale Policy Regression Tests

**Files:**
- Modify: `crates/right/src/main.rs`

- [x] **Step 1: Write narrow regression test**

Add a pure helper if needed so restore can be tested without live OpenShell. Test that a copied stale custom policy path is overwritten with bootstrap generated policy before create.

- [x] **Step 2: Verify red then green**

Run:

```bash
devenv shell -- cargo test -p right restore
```

Expected assertions: stale fake old-host IP removed, broad fallback ranges absent, Right MCP bootstrap endpoint has no `allowed_ips`.

## Task 6: Documentation

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/sandbox.md`
- Modify: `docs/architecture/lifecycle.md`
- Modify: `docs/architecture/mcp.md`

- [x] **Step 1: Update docs**

Document bootstrap unresolved policy, exact sandbox-visible multi-IP hot-apply, IPv4 `/32`, IPv6 `/128`, bot startup self-healing, backup/restore/new-host stale policy regeneration, and why `openshell forward/service` are not the MCP route.

- [x] **Step 2: Verify docs mention invariant**

Run:

```bash
devenv shell -- rg -n "bootstrap unresolved|host.openshell.internal|/128|self-heal|policy set" ARCHITECTURE.md docs/architecture
```

## Task 7: Live OpenShell Test And CLI/Bot Flow

**Files:**
- No production files unless live testing exposes a bug.

- [x] **Step 1: Inspect CLI help**

Run:

```bash
devenv shell -- cargo run -p right -- --help
devenv shell -- cargo run -p right -- agent --help
devenv shell -- cargo run -p right -- init --help
devenv shell -- cargo run -p right -- up --help
```

- [x] **Step 2: Create isolated test Right home and test agent**

Use `/private/tmp/right-test-policy-<short-id>` and only `right-test-policy-*` OpenShell resources. Do not use production `~/.right`.

- [x] **Step 3: Verify sandbox/MCP/policy**

Record evidence that sandbox reaches `Ready`, the staged `mcp.json` points to `http://host.openshell.internal:<port>/mcp`, `getent ahosts host.openshell.internal` returns observed IPs, final policy contains every observed IP as `/32` or `/128`, and sandbox HTTP reaches Right MCP backend rather than OpenShell `ssrf_denied`.

- [x] **Step 4: Test bot process**

If `RIGHT_TEST_TELEGRAM_BOT_TOKEN` and `RIGHT_TEST_TELEGRAM_CHAT_ID` exist, start the actual test bot and send a harmless command. If absent, start the nearest non-secret local bot/agent path and report the missing env vars.

- [x] **Step 5: Migration/stale policy live test**

For the same test agent only, write or restore stale Right MCP IP data, restart through the real path, and verify exact policy is regenerated without deleting/recreating the existing sandbox.

- [x] **Step 6: Cleanup**

Stop test processes, delete only `right-test-policy-*` sandboxes, remove test forwards/services created by this goal, remove `/private/tmp/right-test-policy-*`, leave the worktree.

## Task 8: Final Verification And Audit

**Files:**
- Inspect all changed files and command outputs.

- [x] **Step 1: Format**

Run:

```bash
devenv shell -- cargo fmt --check
```

- [x] **Step 2: Build**

Run:

```bash
devenv shell -- cargo build --workspace
```

- [x] **Step 3: Full tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

- [x] **Step 4: Completion audit**

Map every requirement in `/private/tmp/right-agent-policy-implementation-goal-prompt.txt` to code, docs, tests, live test output, and cleanup status. Only mark the goal complete if every requirement is covered or a documented environmental blocker is explicit.
