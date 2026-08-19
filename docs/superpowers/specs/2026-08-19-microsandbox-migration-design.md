# microsandbox migration — design

Status: approved, not started. Supersedes the OpenShell policy/providers work
described in the 2026-08-19 handoff.

Related: `docs/adr/0001-microsandbox-replaces-openshell.md`,
`docs/adr/0002-right-owned-provider-credential-store.md`,
`docs/adr/0003-tls-interception-scoped-to-provider-hosts.md`, `CONTEXT.md`.

## Why

OpenShell ≥0.0.97 rejects any policy where a hostless catch-all endpoint overlaps a
composed provider endpoint on the same port, because `connection_conflicts` compares
`tls` and `allowed_ips`. Right's permissive floor is exactly that shape, so every
permissive Agent with Providers fails policy load and the supervisor retries forever.
An 11-shape spike on a live sandbox found no combination giving both whole-internet
egress and credential injection, and no upstream flag or issue acknowledges the gap —
OpenShell's model is per-host by design. Combined with repeated alpha breakage, the
dependency is replaced rather than worked around.

microsandbox (v0.6.10, Apache-2.0, Rust) supplies every primitive Right uses: microVM
isolation via libkrun (KVM on Linux, Hypervisor.framework on Apple Silicon), a
user-space egress engine with deny-by-default domain/CIDR/port rules, TLS interception,
and host-side credential substitution gated per secret on SNI + DNS pin + TLS identity +
authority. Substitution is independent of egress rules, which is precisely the
conflict that blocks OpenShell.

## Decisions

| Area | Decision |
|---|---|
| Cutover | No backend abstraction. New concrete `right-sandbox` crate; `right-openshell` deleted whole in the final stage. Migration is an explicit one-time command, so no runtime dispatch between backends is needed. |
| Sandboxless mode | `sandbox: mode: none` is removed. Every Agent has an Agent Sandbox. A config carrying `mode: none` is a hard error; `mode: openshell` is accepted and ignored for one release, then the field is dropped. |
| Process ownership | The Agent's bot spawns its sandbox detached and re-attaches by name on restart. No new daemon, no process-compose entry. Sandboxes do not auto-start on host reboot; the supervisor starts them on attach. |
| Command transport | SSH is retained via `ProxyCommand msb ssh serve <name> --stdio`. `resolved_sandbox` + `ssh_config_path` collapse into one `SandboxHandle`. |
| Egress | Permissive = `public` + `host`. Restrictive = `host` + explicit allowlist, shipped but documented experimental until exercised. Egress is a typed value applied through the SDK; `policy.yaml` codegen and the `SandboxPolicyApply` codegen category are deleted. |
| Filesystem | The microVM boundary replaces landlock. No host bind mounts; all transfer goes through the fs API/SFTP. The `SandboxRecreate` codegen category disappears. |
| Guest user | An unprivileged `sandbox` user runs `claude`; provisioning runs as root. This preserves the `chmod a-w` integrity guarantee on `/sandbox/.platform`, which root would ignore. |
| Resources | 2 vCPU, 8 GiB memory, 16 GiB writable layer, overridable per Agent under `sandbox.resources`. Memory is a limit, not a reservation. |
| Base image | Stock OCI image plus imperative bootstrap. No Right-maintained image, no base snapshot. |
| Providers | Credentials live in `~/.right/providers.db` (SQLite, 0600) and reach the sandbox as microsandbox source-ref secrets, so the runtime persists no secret. Ownership is a column; `shared_from` is dropped from `agent.yaml`. Built-in provider types become Rust consts; `managed_profiles.rs` is deleted. |
| Injection | Headers by default; `query_params` opt-in per catalog entry; body injection never. Violations use `BlockAndLog` plus an operator alert over Telegram. |
| Provider status | `ready` / `needs-value` / `error`. Composition confirmation, `wait_for_provider_composed*`, and `ensure_v2_enabled` are deleted. |
| Upstream risk | Exact-pin the `microsandbox` crate, assert the version at startup, and keep a small real-VM contract suite. |
| Diagnosis | `GatewayCause` is replaced by `SandboxCause { MsbMissing, MsbVersionMismatch, HypervisorUnavailable, SandboxNotFound, SandboxNotRunning, Unreachable }`. `SandboxHealth` and `sandbox_gate` are unchanged. |
| Testing | No fake backend. Sandbox-touching tests become real-VM `ci_msb_*` tests that do not run on GitHub-hosted runners. |
| Migration | Explicit `right agent migrate-sandbox`; the bot refuses to start against an unmigrated Agent with an actionable message. |

## Proof-of-concept gate

No refactoring starts until all seven hold on an Apple Silicon workstation. Each is
currently unverified; failure of 1, 2, or 5 changes the design.

1. `claude -p` runs over `msb ssh serve --stdio` with piped stdin and `stream-json`
   stdout.
2. A real provider API returns success with a substituted credential. Upstream
   configures no ALPN on either TLS side, so an h2-capable guest is expected to fall
   back to HTTP/1.1 at the interceptor — inferred from rustls semantics, never tested
   upstream.
3. `claude` reaches Anthropic with interception scoped away from it, needing no CA
   trust configuration.
4. Whether `modify()` rotates a source-ref secret live or forces a restart.
5. Guest reaches the MCP aggregator through `host.microsandbox.internal` with the
   `host` group allowed, and the aggregator binds loopback instead of `0.0.0.0`.
6. 2 vCPU / 8 GiB sustains a real turn.
7. The unprivileged `sandbox` user runs `claude` with `chmod a-w` platform files intact.

## Stages

1. **PoC.** The gate above. Throwaway scripts, no production code.
2. **`right-sandbox` crate.** Concrete msb-backed sandbox lifecycle, exec, file
   transfer, egress, secrets, and the unified `SandboxHandle`. Version pin and
   preflight.
3. **Provider store.** `~/.right/providers.db`, built-in catalog as consts, ownership
   and borrowing, dashboard and internal-API routes repointed. Dashboard keeps its
   current flows; `composed` becomes the tri-state status.
4. **Rewiring.** Supervisor, invocation, keepalive, sync, cron, attachments,
   background, reflection, learning, dashboard skills, codegen. `mode: none` removal
   lands here, deleting the host-mode branches across ~20 files.
5. **`right agent migrate-sandbox`.** Modeled on the existing `perform_migration`
   shape: tar agent-owned content, create the msb sandbox, restore, verify, update
   config, then delete the OpenShell sandbox. Rollback keeps the old sandbox on any
   failure. Carries provider metadata; credentials are re-entered once.
6. **Delete OpenShell.** Remove `right-openshell`, the vendored protos, the proto
   compat workflow, and the OpenShell CI job. Update `ARCHITECTURE.md` and the
   `docs/architecture/` satellites.

## Migration contents

Carried: agent-created files, `.claude/projects` transcripts, `.claude/skills/rightx-*`,
`.claude/settings.local.json`, `IDENTITY.md`, `SOUL.md`, `USER.md`, `TOOLS.md`,
`inbox/`, `outbox/`, `crons/logs/`.

Excluded: `.platform` and platform symlinks (regenerated), `.local/bin` and `.npm`
(ABI-bound to the old image, reinstalled), and the existing rebuildable set
(`.cache`, `.venv`, `.npm`, `.uv`).

Restore must remap ownership rather than preserve numeric uids: the archive is written
by OpenShell's `sandbox` uid, and the guest's uid may differ.

## Known gaps

- Live sandbox tests lose CI coverage. GitHub-hosted runners expose no usable
  hypervisor: macOS arm64 runners report `kern.hv_support = 0`, and Linux runners need
  an unofficial udev workaround GitHub explicitly disclaims. Upstream microsandbox runs
  its own VM lanes on self-hosted runners. Coverage returns when Right moves runners.
- Credential substitution does not cover HTTP/2 DATA frames, compressed bodies, bodies
  over 16 MiB, or headers over 64 KiB — those requests are blocked, not silently sent.
  Certificate-pinning clients cannot be intercepted at all.
- An approved destination receives the real credential. Keep `allow_hosts` narrow.

## Out of scope

Base-image snapshots, cloud backend, restrictive-mode rollout for existing Agents, and
any change to Telegram, memory, learning, or MCP semantics.
