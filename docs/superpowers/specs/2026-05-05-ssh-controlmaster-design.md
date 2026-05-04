# SSH ControlMaster for Bot Hot Path

## Summary

Add OpenSSH connection multiplexing (`ControlMaster`) to the bot's per-message
`ssh` invocations so each message reuses an established TCP+SSH session
instead of paying a fresh handshake. Goal: shave the ~200–500ms SSH
handshake off user-perceived "first token" latency on every Telegram
message.

## Motivation

Each Telegram message currently triggers a fresh `ssh -F <config> <host>
-- <claude -p ...>` subprocess in `crates/bot/src/telegram/worker.rs`.
That subprocess does a full TCP+key handshake before stdin can flow into
`claude`. The same pattern repeats for cron, cron_delivery, reflection,
keepalive, attachments, and several auxiliary calls. The handshake is
pure latency overhead — the bot is long-running, the sandbox is
long-running, and the SSH host is the same across calls.

The chosen target is **latency**, not subprocess churn or architectural
cleanliness. Both fall out as free side effects, but they don't drive
the design.

## Approach Considered and Rejected: russh in-process

Holding a `russh::client::Handle` in the bot and opening a fresh channel
per call would also eliminate the handshake. It offers programmatic
session lifecycle, in-process tracing, and the keepalive case becomes a
config knob rather than a subprocess.

Rejected as the first step because:

- It's a non-trivial dependency (~10–15 transitive crates) and a new
  failure surface we'd own (host-key verification, key parsing, channel
  multiplex limits, reconnect/backoff logic).
- The OpenSSH binary is already a hard project dependency. Reaching
  the same latency target via `ControlMaster` is configuration-only.
- We can revisit russh later if the bot's needs outgrow what
  multiplexing gives us. This design is forward-compatible: nothing
  about ControlMaster prevents a future swap.

## Approach Considered and Rejected: gRPC `exec_in_sandbox`

The OpenShell gRPC server already exposes `ExecSandbox` and the bot
already uses it for short admin commands. We considered routing
`claude -p` through it.

Rejected because the existing implementation
(`crates/right-agent/src/openshell.rs:1074`) is one-shot: it accumulates
stdout into a buffer and returns only at process exit. `claude -p`
requires piped stdin (the assembled prompt) and streaming JSON stdout
(stream-json output format). Making this viable would require an
OpenShell server-side change to support streaming exec, which is out of
scope.

## Design

### Directives in the SSH Config

`generate_ssh_config` in `crates/right-agent/src/openshell.rs` shells
out to `openshell sandbox ssh-config <name>` and writes the result to
`<RIGHT_HOME>/run/ssh/<sandbox-name>.ssh-config` (where `RIGHT_HOME`
is the runtime root, default `~/.right`, overridable via `--home`).
After writing, append three lines:

```
ControlMaster auto
ControlPath  <RIGHT_HOME>/run/ssh/<sandbox-name>.cm
ControlPersist yes
```

The `ControlPath` value must be the resolved absolute path (no `~`
expansion ambiguity) so the directive behaves identically regardless
of the bot's `HOME` env var.

Every existing `ssh -F <config>` call site automatically inherits
multiplexing. No per-call-site Rust changes — worker, cron,
cron_delivery, reflection, keepalive, attachments, prompt, and
invocation all benefit transparently.

### Why `ControlPath` Is Keyed on Sandbox Name

OpenSSH ControlMaster matches multiplex candidates on `(user, host,
port)`. The SSH host alias produced by `ssh_host_for_sandbox` (in
`crates/right-agent/src/openshell.rs:49`) is `openshell-<sandbox-name>`
— it changes whenever the sandbox is recreated (filesystem-policy
migration creates a new timestamped sandbox name). A per-agent
`ControlPath` would bind a master to `openshell-OLD`, then the next
ssh's config would point at `openshell-NEW` — OpenSSH detects the
mismatch and either falls back to a fresh connection (no benefit) or
refuses to reuse (brittle).

Per-sandbox naming matches the granularity OpenSSH already enforces.
Within a sandbox's lifetime — which includes the network-policy
hot-reload path and the common case overall — the master is stable.
Across sandbox migrations the master is invalidated, which is what
would happen anyway.

### Why `ControlPersist yes` (No Idle Timeout)

The bot is the long-running owner. We want the master alive as long as
the bot is alive. A numeric idle timeout creates a "first message after
a quiet hour pays full handshake again" failure mode that defeats the
goal.

### Master Lifecycle

Four lifecycle events, each mapped to a concrete code site.

**1. Eager establish at bot startup.** Right after the existing call
to `generate_ssh_config` in `crates/bot/src/lib.rs` (in the
sandbox-startup region that today writes the ssh-config and runs
initial sync), do one extra call: `ssh -F <new-config> <host> --
/bin/true`. Forces the master into existence so
the first user message reuses it instead of paying the handshake.
Failures here are non-fatal — log a `warn!`, let the next call
lazy-establish via `ControlMaster=auto`. Reasoning: bot startup is
already the heavy phase (initial sync blocks teloxide). Paying ~300ms
there is invisible. Paying it on the user's first message is exactly
what we're avoiding.

**2. Stale socket recovery on startup.** If the `ControlPath` socket
file already exists at startup (previous bot died ungracefully), don't
blindly overwrite. Run `ssh -F <config> -O check <host>` first. If it
returns 0, the previous master is alive and reusable; skip eager
establish. If it returns nonzero, `rm -f <socket>` and proceed to eager
establish. Without this, eager establish fails on `bind: Address
already in use` and we silently fall back to lazy mode.

**3. Shutdown teardown.** In the bot's existing graceful-shutdown path
(the cleanup task wired up alongside `cleanup_ssh_config` in
`crates/bot/src/lib.rs`), add `ssh -F <config> -O exit <host>` as a
best-effort step. Avoids leaking master processes across bot
restarts. Logged but not awaited; we don't block shutdown for it.

**4. Sandbox-migration teardown.** In `maybe_migrate_sandbox`, after
the new sandbox is created and the new ssh-config written, send `ssh
-F <OLD-config> -O exit <OLD-host>` against the old config (best-effort,
non-fatal — the old sandbox might already be gone, in which case the
master died with it) and `rm -f <OLD-controlpath>` to clean up the
orphaned socket file. The new sandbox's master is established by
event 1 on the next bot startup, which migration triggers.

## Out of Scope

- **Active health-check ticker.** Skipped. The existing per-call retry
  layer in `worker.rs` already handles a dead master by failing the
  call; on the next message we lazy-rebuild via `ControlMaster=auto`.
  A separate liveness ticker is overengineering for a minimum-viable
  change.
- **Master sharing across in-place bot upgrades.** Skipped.
  process-compose restarts spawn a new bot process, and we'd need
  pidfile-style coordination we don't have today. Stale-socket
  recovery on startup handles the SIGKILL'd-previous-bot case
  adequately.
- **`right` CLI `attach` command** (`crates/right/src/main.rs:3541`).
  Interactive TTY, not the bot's hot path. Leave on direct `ssh`.
- **gRPC `exec_in_sandbox`** for admin commands. Already a separate
  path, not affected.

## Verification

- **Manual smoke test.** Start bot, observe `tracing::info!` for
  "established control master in {duration}". Send a Telegram message,
  observe per-message timing in worker. Compare against pre-change
  baseline (current code spawns fresh ssh per message).
- **Integration test addition.** In an existing `TestSandbox`-based
  test, run two `ssh_exec` calls back-to-back and assert the second
  completes faster than the first. Or simpler: assert `ssh -O check`
  against the `ControlPath` returns 0 after the first call.
- **Sandbox-migration test.** Existing migration test should still
  pass. Add an assertion that the old `ControlPath` socket file is
  gone after migration completes.

## Files Touched (Estimate)

- `crates/right-agent/src/openshell.rs` — `generate_ssh_config`
  appends the three directives; new helpers `establish_control_master`,
  `check_control_master`, `tear_down_control_master`.
- `crates/bot/src/lib.rs` — call eager establish after
  `wait_for_ssh`; call shutdown teardown alongside the existing
  ssh-config cleanup task.
- Sandbox-migration code (`maybe_migrate_sandbox`) — call old-master
  teardown before discarding the old config.
- No changes at the call sites (worker, cron, cron_delivery,
  reflection, keepalive, attachments, prompt, invocation): they all
  use `ssh -F <config>` and inherit multiplexing for free.

## Forward Compatibility

If a future need (e.g. structured per-channel telemetry, in-process
session ownership) outgrows what multiplexing offers, swapping to a
russh-based session is straightforward: replace
`establish_control_master`/`tear_down_control_master` with russh
equivalents, and gate the per-site `ssh` subprocess calls behind a
helper that either spawns `ssh` (current) or opens a russh channel
(future). Nothing in this design forecloses that path.
