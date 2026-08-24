# Agent Sandbox

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

An Agent Sandbox is a microsandbox microVM: hardware-isolated, booted from a
stock OCI image, driven entirely through the microsandbox SDK. There is no
SSH, no gateway daemon, and no host execution path. `crates/right-sandbox` is
the only crate that depends on the SDK; nothing in its public API exposes a
raw SDK type.

## Migrating out of OpenShell

`right agent migrate-sandbox <agent>` moves an agent whose `agent.yaml` still
carries `sandbox.mode: openshell` into a microsandbox VM. The order is the
safety property, so it is worth stating: archive the OpenShell home (kept under
`~/.right/backups/<agent>/migrate-<stamp>/`), create the new sandbox from the
same spec builder the bot uses, extract with `--no-same-owner`, hand every
top-level entry except `.platform` to the guest user, **verify**, only then
rewrite `agent.yaml`, and only then delete the OpenShell sandbox. Any failure
before the verification deletes the new sandbox and leaves both the OpenShell
sandbox and the original `agent.yaml` untouched, so the command can just be run
again.

The old sandbox's deletion is the one step that may fail without failing the
command: by then `agent.yaml` is rewritten and the restore is verified, so the
migration is durable. The recap warns and prints the manual
`openshell sandbox delete <name>` to run, and only claims the sandbox was
deleted when deletion was confirmed.

Provider credentials do not come along: OpenShell redacted them on read, so
migration writes definition-only `NeedsValue` provider records. Bot bring-up
skips those records, keeping the dashboard available. When the operator enters
the first real value in `/providers`, the dashboard adds the missing binding to
the existing sandbox through SDK-managed restart without deleting it.

Everything that still touches OpenShell lives in
`crates/right/src/migrate_sandbox/legacy_openshell.rs`, a frozen CLI-only
reader owned by this command and deleted once the migration window closes. The
`right-openshell` crate, its vendored protos, and its gRPC/mTLS transport are
gone.

Code: `crates/right/src/migrate_sandbox.rs` (legacy read side, `agent.yaml`
rewrite) and `right_agent::sandbox_migrate` (guest side: extract, ownership,
verification).

## Sandbox architecture

Sandboxes are **persistent** — never deleted automatically. They live as long
as the agent lives, run detached (they survive the bot process exiting), and
dropping a handle does not stop them. Lifecycle is explicit: `stop`/`destroy`.

```
Bot startup (bring_up_sandbox, crates/bot/src/sandbox_supervisor.rs):
  ├─ ensure_runtime_installed — the SDK installs its own runtime on first
  │   use; no PATH dependency and no operator install step
  ├─ diagnose_host — /dev/kvm and hypervisor preflight; a hypervisor-less
  │   host degrades with a cause-specific SandboxDiagnosis, it does not crash
  ├─ agent_sandbox_spec — image, resources, egress, secret bindings, workdir,
  │   guest user (the one spec builder shared by the bot and the CLI)
  ├─ SandboxHandle::create_or_attach — attach wins over create; a sandbox in
  │   a non-running phase is started from its persisted config
  ├─ wait_ready (DEFAULT_READY_TIMEOUT, 120s) — covers only the attach race
  │   (Created/Starting); create/start already block until the guest agent
  │   is serving
  ├─ hot_reconcile_providers — after the guest is ready, re-resolve every
  │   credential-bearing provider from `providers.db` and apply it to the live
  │   sandbox through a scoped in-process resolver; existing bindings rotate
  │   live, missing bindings restart-add, `NeedsValue` remains skipped, and any
  │   apply error fails bring-up
  ├─ fs_mkdir /sandbox/inbox and /sandbox/outbox — recreated every bring-up
  │   because a recreated sandbox starts from the stock image
  ├─ initial_sync (blocking — before the Telegram bot starts)
  │   ├─ Create the guest user and its user-local CLI/npm directories
  │   ├─ Write the managed env and patch `.bashrc`
  │   ├─ Deploy platform files to /sandbox/.platform/ (content-addressed +
  │   │   symlinks)
  │   ├─ Write .claude.json and verify trust keys
  │   └─ Host-download and verify pinned Claude Code, upload it to a unique
  │       /sandbox/.platform/claude/ target, then atomically activate
  │       /sandbox/.platform/bin/claude
  └─ reverse_sync_md — startup identity mirror (advisory: logged, not fatal)

Create-time state:
  ├─ egress policy — the SDK cannot change network policy on a running VM
  └─ initial resources (cpus, memory, writable layer)

Provider state:
  ├─ existing bindings diff complete allowed-host sets: shrink removals while
  │   the old credential is live, rotate, then widen additions only after the
  │   rotation succeeds; a failed rotation therefore cannot expose an old
  │   credential to a new destination
  ├─ obsolete Right-managed bindings are explicitly revoked live through
  │   modify().remove_secret(...).apply(); unrelated sandbox secrets are not
  │   candidates, and removing the final binding persists TLS-off for next
  │   start while the current VM remains TLS-on with no usable credentials
  ├─ a missing non-query binding is added through
  │   modify().secret(...).restart().apply(); this stops and starts the same
  │   sandbox, preserving its writable filesystem
  └─ a missing query-injected binding fails clearly because the pinned SDK's
      modify API cannot express that injection policy

Sandbox network:
  ├─ Egress is a typed value applied at create: Permissive, or Restrictive
  │   with a domain-suffix allow list (anthropic.com, claude.com, claude.ai,
  │   storage.googleapis.com). Suffixes, not globs.
  ├─ Startup does not download guest packages or installers. Claude Code is
  │   downloaded by the host, verified before guest mutation, and uploaded
  │   through the SDK fs control plane, so existing restrictive sandboxes
  │   adopt it on restart without recreation or public guest egress.
  ├─ The host destination group is always open on top of the allow list —
  │   that is how the guest reaches the MCP aggregator on the host.
  └─ Guest → host loopback services resolve through
      `host.microsandbox.internal`.

Provider secrets (ADR-0003):
  ├─ A SecretBinding carries a private `SecretString` credential plus durable
  │   references: the guest-visible env var, an owner-and-record-scoped source
  │   identity, and the hosts allowed to receive the real value.
  ├─ Right installs the credential in the vendored SDK's scoped, zeroizing
  │   in-process resolver only across create/start/apply. No process environment
  │   mutation is used.
  ├─ The SDK persists the placeholder and source identity only — no secret
  │   material at rest or in Debug output, and the guest sees only a placeholder.
  └─ TLS interception is a **bypass deny-list**: adding any secret enables
      interception for every destination on the intercepted ports except
      TLS_BYPASS_HOSTS, which always carries the Anthropic hosts so Claude's
      own path is never intercepted and needs no guest CA configuration.

Staging (minimal bootstrap — platform files deployed to /sandbox/.platform/
during initial_sync):
  ├─ .claude/settings.json     — CC behavioral flags
  ├─ .claude/reply-schema.json — structured output schema
  ├─ .claude.json              — trust + onboarding
  ├─ mcp.json                  — MCP server entries
  └─ TOOLS.md                  — agent-editable tool notes seeded for turn 1
  EXCLUDED: skills (deployed to /sandbox/.platform/), credentials, plugins

Platform store (/sandbox/.platform/ inside the guest):
  ├─ Content-addressed files: settings.json.<hash>, reply-schema.json.<hash>
  ├─ Content-addressed skill dirs (one per `right_codegen::BUILTIN_SKILL_NAMES`)
  ├─ Pinned Claude runtime under `/sandbox/.platform/claude/`, atomically
  │   selected through `/sandbox/.platform/bin/claude`
  ├─ Symlinked from agent-visible locations into `/sandbox/.platform/`
  ├─ Root-owned and read-only to the unprivileged `sandbox` user
  └─ Manifest GC preserves the separately managed `claude/` and `bin/` trees
```

Sandbox names come from `right_sandbox::resolve_sandbox_name`: the explicit
`sandbox.name` in `agent.yaml` when set, otherwise `right-{agent}`. Both go
through `fit_sandbox_name`, which maps any string into the SDK's name space
(1..=128 bytes over `[A-Za-z0-9._-]`, alphanumeric first byte) by collapsing
invalid characters and, when the result is too long, truncating to
`{prefix}-{hash8}` where `hash8` is the first 8 hex chars of the SHA-256 of
the full candidate. It is total: every output passes the SDK validator. Every
lifecycle path — bot bring-up, `right agent destroy`, `right agent
rebootstrap` — MUST resolve through this function; two call sites disagreeing
about an agent's sandbox name is how a destroy orphans a microVM.

Learned skill packages are agent-owned directories under
`/sandbox/.claude/skills/rightx-*`. The learning MCP tools do not patch
non-`rightx-*` skill directories and do not copy skill files from sandbox to
host.

The dashboard may run explicit, read-only guest probes when the user opens
Mini App health, identity, or knowledge-skill views. These probes reuse the
bot-owned sandbox handle, are bounded by short per-request timeouts, and read
only the requested dashboard material: sandbox stats, identity files, or skill
inventory/details. Overview rendering does not run these probes.

### Graceful degrade

When the sandbox cannot be brought up, `bring_up_sandbox` (in
`crates/bot/src/sandbox_supervisor.rs`) classifies the `SandboxError` into a
cause-specific `SandboxDiagnosis` (summary + ordered fixes) and returns
`Ok(Err(diagnosis))` instead of crashing. Recoverable availability failures —
runtime install failure, no hypervisor, boot failure, a sandbox still
starting, an unreachable guest agent — all take this path. Errors that say
nothing about backend health (invalid spec, a guest command that failed) fall
back to `Unreachable`. Genuine non-self-healing config errors still propagate
as hard failures.

On a degraded start, `lib.rs` logs the diagnosis at ERROR, constructs the
`SandboxRuntimeHandle` with health `Unavailable`, and continues booting
Telegram — so incoming messages are handled immediately rather than cycling
through process-compose restarts.

The `SandboxSupervisor` task (spawned once per sandboxed agent, outlasting any
single bring-up attempt) is the sole writer of `SandboxRuntimeHandle` after
startup:

- **Recovery loop:** retries `bring_up_sandbox` with a fixed backoff schedule
  of 5 → 10 → 15 → 15 → 30 s (last value repeats). On success it calls
  `handle.set_ready(sandbox)`, spawns the background sync task, and sends a
  "✅ Sandbox back online" notice to every chat that received an
  unavailability message during the outage.
- **Monitor mode:** when `Ready`, the supervisor waits for a failure report or
  shutdown. Worker, keepalive, and periodic sync failures all call
  `SandboxRuntimeHandle::report_suspected_failure()`; the supervisor verifies
  each coalesced wake by reading the real sandbox phase, degrades on a
  terminal phase, and ignores transient non-ready phases.
- **Sync task ownership:** the supervisor seeds the startup sync task (when
  bring-up succeeded) or skips it (degraded). On degrade-from-ready it aborts
  the sync task; on recovery it re-spawns it.

Recovery publishes a *new* handle addressing a newly created VM, which is why
consumers hold the `SandboxRuntimeHandle` and call `current_sandbox()` per
unit of work (turn, cron job, delivery, dashboard request) rather than caching
a `Sandbox` for the process lifetime.

### User-Local CLI Environment

Right Agent treats `/sandbox/.local/bin` as the canonical user-installed
executable directory. Startup sync creates `/sandbox/.local/bin` and
`/sandbox/.npm`, assigns them to the guest user, atomically writes
`/sandbox/.right/env.sh`, and ensures `/sandbox/.bashrc` sources it. The Claude
invocation wrapper sources the same file with an inline fallback. Managed PATH
order is `/sandbox/.local/bin` first and `/sandbox/.platform/bin` second: a
user-local Claude installed by the unprivileged upgrade task wins, while a
fresh stock image uses the host-staged platform runtime. This provisioning does
not install git, Python, or other baseline tools.

Guest process stdin is buffered through `SandboxStdin`, whose explicit async
`close` drains every queued chunk through the SDK and awaits the SDK EOF frame.
Turn, delivery, and reflection paths race the full write-plus-close operation
against their transport deadline (and foreground stop cancellation) before
reading stdout. Dropping `SandboxStdin` or cancelling `close` aborts the stdin
forwarder, so it cannot remain detached in an SDK write/close; completed closes
still propagate the forwarder's write/close error.

Claude stream-json's top-level `result` event is the authoritative completion
signal for session-bearing invocations. The microsandbox SDK can deliver that
terminal stdout record without subsequently emitting `Exited` or closing the
stream, so consumers finish processing the result, explicitly kill the guest
exec handle, and use bounded wait/drop cleanup rather than waiting for EOF.
The result's `is_error` value defines semantic success or failure even when
transport cleanup reports no usable process exit code.

The managed environment sets `NPM_CONFIG_PREFIX=/sandbox/.local` and
`NPM_CONFIG_CACHE=/sandbox/.npm`. Agents should not use `sudo` or `~/bin` for
sandbox tool installs.

### Test coverage

Live-microVM coverage is CI-explicit: tests that boot a real sandbox use
`#[ignore = "ci-msb: ..."]` with a `ci_msb_` test-name prefix, enforced by
`crates/right/tests/ci_ignored_contract.rs` and selected by the `sandbox` job
in `.github/workflows/tests.yml`. That job is opt-in (repository variable
`SANDBOX_RUNNER`) because it needs a runner exposing `/dev/kvm`. Everything
that does not need a live VM stays in the default workspace test path.

`ci_msb_right_apply_secret_activates_tls_and_preserves_provider_contracts`
starts a no-secret sandbox with TLS interception disabled, then exercises
Right's production apply/remove paths for first and second provider additions,
live value rotation, host confinement, placeholder opacity, writable-layer
persistence, removing one binding without disturbing its survivor, restart
persistence of revocation, and last-binding removal with desired TLS disabled
(the active VM keeps TLS-on until restart). It uses local TLS fixtures and
canary values.
