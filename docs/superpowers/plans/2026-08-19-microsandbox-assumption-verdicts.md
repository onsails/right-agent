# microsandbox migration — stage 1 assumption verdicts

Gate for the stack described in
`docs/superpowers/plans/2026-08-19-microsandbox-stacked-delivery.md`. Every
assumption in [issue #172](https://github.com/onsails/right-agent/issues/172)
was exercised against a real microVM on an Apple Silicon host with
microsandbox `=0.6.10`. Probes live in `crates/right-sandbox/tests/`
(`ci_msb_*`, marker `ci-msb`).

**Result: all seven assumptions hold. The stack proceeds.** One design
document is wrong and is corrected here: ADR-0003's interception-scoping model.

## Verdicts

| # | Assumption | Verdict |
| --- | --- | --- |
| 1 | Turn runs under streaming exec with piped stdin, surviving idle timeout | **Verified** |
| 2 | A bound destination accepts a substituted credential | **Verified** |
| 3 | Claude traffic works with interception scoped away and no certificate configuration | **Verified**, with a corrected scoping model |
| 4 | A source-reference secret rotates live | **Verified** |
| 5 | Guest reaches a loopback-bound host service through the host alias | **Verified** |
| 6 | Default resources sustain a real turn | **Verified** |
| 7 | Unprivileged guest user runs turns with platform files non-writable | **Verified**, with a provisioning constraint |

### 1 — Streaming execution

1.2 MB pushed through the stdin sink in 64 KiB chunks while reading
concurrently. Stdout and stderr each arrived complete, in order, and
unmixed; 1111 events per stream, first output at 851 ms, so output streams
rather than buffering. Exit status maps faithfully (0 → success, 7 → failure).

A 60-second exec on a sandbox built with a 10-second idle timeout completed
untouched, and the sandbox stayed running. The same probe proved idle
detection was armed: the sandbox stopped once the exec ended. **A long turn
needs no keepalive.**

### 2 — Credential substitution

The guest environment carried only the placeholder; the canary value appeared
nowhere in a full environment dump. The bound destination received the real
value in an `Authorization` header. Toward an unbound destination the request
was blocked, the destination received neither the value nor the placeholder,
and the sandbox stayed running.

### 3 — Claude traffic and interception scope

With a secret configured, a bypassed Anthropic host completed its handshake
against the genuine upstream certificate (issuer `O=Google Trust Services`)
with no certificate configuration. A non-bypassed host was served the
interception CA instead.

**ADR-0003 is wrong about how scoping works.** Interception is not an
allowlist of provider-bound hosts. It is a bypass **deny-list**: a single
secret turns interception on for every destination on the intercepted ports
(default 443) except those explicitly bypassed. A secret's allowed-host list
governs *substitution eligibility only*, never interception scope.

### 4 — Live secret rotation

A source-reference secret replanned as a live rotation with no warnings, and
the new value reached the destination while the guest boot id was unchanged —
no restart. The pinned runtime advertises the live-secrets capability; the
planner degrades to a restart when it does not.

Constraint: the placeholder must stay stable across rotations. Changing it
forces a restart, and applying a restart-required plan really does stop and
start the sandbox.

### 5 — Host alias

The guest reached a host service bound to loopback through the host alias,
provided the host destination group was granted. Granting only public egress
left the listener with zero connections, which confirms the upstream default
denies host access. **The MCP aggregator can stop binding all interfaces.**

The guest may route the alias over the IPv6 gateway, so host fixtures bind
both loopback families.

### 6 — Default resources

2 vCPU, 8 GiB memory, 16 GiB writable layer ran a real Claude turn: 9.8 s
boot, 3.1 s provisioning, 6.1 s turn. Available guest memory never dropped
below 7.98 GB and the writable layer used 411 MB of 16 GiB. Memory was never
the constraint.

The credential used for this probe was read into memory only and passed as a
per-exec environment value, never as persisted sandbox configuration.

### 7 — Unprivileged guest user

The per-exec user override really switches user. Against a root-owned
read-only platform file the agent was refused write, create, and unlink,
while reads and ordinary writes succeeded.

**Constraint:** file permissions protect contents, not the directory entry. An
agent-owned parent directory let the agent rename the whole platform tree
away. Stage 2 must keep the platform tree's parent root-owned and give the
agent an owned subdirectory instead of all of the working root.

## Corrections to the design

1. **ADR-0003 must be rewritten** around the deny-list model above. The
   bypass list becomes a maintained, security-relevant list of uninspected
   destinations. Destinations on non-intercepted ports are never inspected, so
   a placeholder aimed at one is forwarded unsubstituted.
2. **Guest CA trust is a provisioning requirement, not a free property.** The
   guest agent installs the interception CA into the guest trust directories
   and points the usual environment variables at a bundle. On an image that
   already ships real root certificates this composes correctly. On an image
   without them the guest agent creates a bundle containing *only* the
   interception CA, so certificate-verifying clients then trust the
   interception CA and nothing else — which breaks precisely the **bypassed**
   hosts, including Claude. Provisioning must therefore install the guest's
   certificate package, and must never inherit or set a client pinned to a
   private bundle.
3. **Default resources are Right's, not the SDK's.** Upstream defaults are 1
   vCPU, 512 MiB, and a 4 GiB writable layer. Right sets its own explicitly.
4. **Violation alerts read the sandbox log stream, not a file.** The runtime
   emits a structured warning naming the environment variable, placeholder,
   protocol, server name, method, and path, and never the credential. The SDK
   exposes it as a system log source, which is the supported hook for the
   Telegram operator alert.
5. **The stdin sink does not chunk.** One write becomes one protocol frame,
   capped at 4 MiB; an oversized write fails and tears down that execution
   session. The turn's stdin writer chunks below the cap.
6. **Do not gate health on reported CPU usage.** It read zero on every sample
   even under load. Memory, disk, and writable-layer figures looked correct.
7. **Measure boot time against writable-layer size in stage 2.** One image
   booted in about 10 seconds with an explicit 16 GiB layer and far slower on
   the default layer. The cause is unconfirmed and worth one measurement
   before the default is fixed.

## Coverage note

These probes require a hypervisor and therefore cannot run on GitHub-hosted
runners. They are ignored by default with the `ci-msb` marker and the
`ci_msb_` name prefix, registered in the workspace ignored-test contract, so
they can be selected as one group the moment suitable runners exist.
