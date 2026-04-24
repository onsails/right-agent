# Handoff: MCP from sandbox returns 403 + OpenShell `tls: terminate` deprecation

**Date:** 2026-04-24
**Status:** Root cause confirmed, fix not implemented yet
**Branch:** master (clean)

## TL;DR

Two independent problems surfaced together and looked like one:

1. **Blocker:** MCP aggregator returns `403 "Forbidden: Host header is not allowed"` to CC inside the sandbox. Root cause — `rmcp` crate bumped 1.3.0 → 1.5.0 (dependabot, commit `49ccb54c`), 1.4.0 added DNS-rebinding check on Host header, default allowlist is only `localhost / 127.0.0.1 / ::1`. Sandbox sends `Host: host.openshell.internal:8100` → rejected.
2. **Nag:** OpenShell v0.0.34+ deprecated `tls: terminate` / `tls: passthrough` in policy YAML. Auto-termination by peeking ClientHello replaced them. Our `codegen/policy.rs` still emits `tls: terminate`. Logs spam `WARN` per request. Not broken, but needs cleanup before OpenShell removes the field.

The original symptom — "MCP отвалился" — is #1. The agent (`right` / wbsbrain) correctly read `"status":"failed"` from CC init, then **fabricated** a fake "dословно" quote of a notification when the user pressed. This is a reflection issue for another day, not part of the fix.

## What was confirmed

### Problem 1 — rmcp DNS-rebinding check

**Symptom:**
```
$ curl -H "Authorization: Bearer <valid>" http://host.openshell.internal:8100/mcp  (from sandbox)
HTTP/1.1 403 Forbidden
Forbidden: Host header is not allowed

$ curl -H "Authorization: Bearer <invalid>" ... (from sandbox)
HTTP/1.1 401 Unauthorized
Invalid Bearer token
```

The differentiation between valid/invalid bearer at the proxy was a red herring. Both requests traverse OpenShell proxy fine. The 403 comes from **rmcp's own middleware**, after the aggregator's own `bearer_auth_middleware` lets the request through. With an invalid bearer, `bearer_auth_middleware` rejects at axum layer with 401 before the request reaches rmcp's `StreamableHttpService`.

**Source of the error string:**

File: `~/.cargo/registry/src/index.crates.io-.../rmcp-1.5.0/src/transport/streamable_http_server/tower.rs`

```rust
// line 240
if !host_is_allowed(&host, &config.allowed_hosts) {
    return Err(forbidden_response("Forbidden: Host header is not allowed"));
}

// line 72 — default config
allowed_hosts: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
```

**Upstream history:**
- `rmcp-v1.4.0` (2026-04-10) — release note: `*(http)* add host check ([#764])`
- PR: https://github.com/modelcontextprotocol/rust-sdk/pull/764

**How the regression reached us:**
- `49ccb54c chore(deps): bump the prod-deps group with 17 updates` — dependabot 2026-04-23 14:02 UTC. Among 17 bumps: `rmcp 1.3.0 → 1.5.0`
- `96e98b1f revert(deps): roll back breaking dependabot bumps` — Andrey reverted `rand`, `hmac`, `sha2`, `fs4`, `which`, `notify-debouncer-mini` — but NOT rmcp. Cargo.lock still has `rmcp = 1.5.0`.
- Aggregator process restarted ~23:30 UTC (after OpenShell restart at 22:34 UTC, but that was coincidence). From then on: all new CC sessions inside sandbox see `mcp_servers:[{"name":"right","status":"failed"}]` in init, because the streamable-http init POST gets 403.

**Where the aggregator builds config** (`crates/rightclaw-cli/src/aggregator.rs:564`):

```rust
let config = StreamableHttpServerConfig::default()
    .with_stateful_mode(false)
    .with_json_response(true)
    .with_sse_keep_alive(None)
    .with_cancellation_token(ct.clone());
```

No `.with_allowed_hosts(...)` and no `.disable_allowed_hosts()` — it gets the default 3-host allowlist.

### Problem 2 — `tls: terminate` deprecated

**Symptom in sandbox logs** (`kubectl logs rightclaw-right -c agent`):
```
WARN openshell_sandbox::l7: 'tls: terminate' is deprecated; TLS termination is
now automatic. Use 'tls: skip' to explicitly disable. This field will be
removed in a future version.
```
Fires per-request. Only a warning, no functional impact today.

**Upstream change:** OpenShell PR [#544](https://github.com/NVIDIA/OpenShell/pull/544) (merged 2026-03-24, shipped in v0.0.28+), closes issue [#533](https://github.com/NVIDIA/OpenShell/issues/533). Sandbox proxy now peeks ClientHello bytes and terminates TLS unconditionally. Valid `tls:` values:

| Value | Behavior |
|---|---|
| missing / `auto` | **Default.** Peek + terminate if TLS |
| `skip` | **New.** Raw tunnel, no MITM, no credential injection — for mTLS client-cert, unusual protocols |
| `terminate`, `passthrough` | Deprecated; treated as `auto`; WARN per request |

Source: `crates/openshell-sandbox/src/l7/mod.rs:37` (`TlsMode`), `:97` (`parse_l7_config` with deprecation warnings), `:144` (`parse_tls_mode`).

**Where we emit it** (`crates/rightclaw/src/codegen/policy.rs`):
- line 25 — restrictive endpoints helper, each domain gets `tls: terminate`
- line 55 — permissive `**.*:443` wildcard
- line 139 — test assertion

HTTP port-80 endpoints don't have `tls:`, already correct.

## What was NOT the problem

Spent time on these before ruling out:

- **OpenShell upgrade/gateway restart.** User mentioned this, and the docker container DID restart at 22:34 UTC with 70min uptime when I checked. But tests against the sandbox proxy showed invalid-bearer requests pass through fine — proxy is not the gatekeeper here.
- **PR #878 (path canonicalization in v0.0.34).** Unrelated, request-target canonicalization wouldn't reject based on Host header.
- **PR #912 (hostAliases SSRF fix in v0.0.36).** Also unrelated. `host.openshell.internal` resolves correctly from sandbox to 192.168.65.254 which matches our `allowed_ips: 192.168.65.254/32`.
- **`allowed_ips` stale after gateway IP change.** Policy's `192.168.65.254/32` matches. Verified `getent hosts host.openshell.internal` inside sandbox.
- **Aggregator binding to wrong interface.** `lsof -iTCP:8100` showed `*:8100` (0.0.0.0). Fine.

## The fix (to be decided)

User asked a sharp question that's unresolved:

> имеет смысл в итоге явно перечислять учитывая наличие токена? хост то подменить всегда можно не?

**The design question:** should we `.disable_allowed_hosts()` or `.with_allowed_hosts([...])`?

**Case for `disable`:**
- Every agent has a unique 32-byte Bearer token. Axum middleware authenticates before the request reaches rmcp.
- An attacker on the same network who can make the sandbox talk to the aggregator must first steal the bearer — if they have it, they already have full tool access regardless of the Host header.
- DNS rebinding protection in rmcp is aimed at browser-based attacks where a malicious JS page tricks the browser into hitting `http://localhost:8100`. That threat model doesn't apply here — CC client is not a browser.
- Hardcoding an allowlist couples us to infrastructure names (`host.openshell.internal`, `host.docker.internal`, `host.orb.internal`, VM bridge names on Linux). One more thing to maintain.

**Case for explicit allowlist:**
- Defense in depth. The Bearer token is long-lived and per-agent, could leak. Host-header check is a cheap extra barrier against stolen-token-on-wrong-network scenarios.
- But: an attacker with the token can just set Host themselves — it's a client-controlled header.

**Proposal:** go with `.disable_allowed_hosts()`. The token is the only real boundary; adding a Host allowlist adds maintenance burden without raising the bar for a motivated attacker. Document the decision in a comment at the call site so the next person who reads rmcp release notes doesn't re-add it.

**Tests to write:**
- Aggregator integration test: POST `/mcp` with various Host headers (`host.openshell.internal:8100`, `localhost`, `example.evil`, empty) should all pass with valid bearer, fail with invalid bearer.
- Regression test: if we ever accidentally go back to `StreamableHttpServerConfig::default()` without `.disable_allowed_hosts()`, test should catch it.

### Problem 2 fix (independent, low priority)

`crates/rightclaw/src/codegen/policy.rs`:
- Remove `tls: terminate` from restrictive endpoint template (line 25) — `auto` is the default, drop the line.
- Remove from permissive `**.*:443` (line 55).
- Update test assertion (line 139) — assert the policy does NOT contain `tls: terminate`.

This is a one-way fix. Existing deployed agents generate fresh policy on bot startup, no migration needed.

## How to verify the fix

```bash
# 1. Apply aggregator fix, rebuild
cargo build --workspace

# 2. Restart bot for right agent (triggers PC restart of mcp-aggregator)
rightclaw restart right   # or rightclaw down && rightclaw up

# 3. From sandbox — should now be 405 (upstream says GET not allowed, meaning request reached aggregator)
ssh -F ~/.rightclaw/run/ssh/rightclaw-right.ssh-config openshell-rightclaw-right \
    "curl -s -o /dev/null -w '%{http_code}\n' \
     -H 'Authorization: Bearer $(jq -r '.mcpServers.right.headers.Authorization' ~/.rightclaw/agents/right/mcp.json | cut -d' ' -f2)' \
     http://host.openshell.internal:8100/mcp"
# expect: 405 (not 403)

# 4. Send /hi to bot in Telegram, check stream log — mcp_servers status should be "connected"
tail -f ~/.rightclaw/logs/streams/*.ndjson | grep mcp_servers

# 5. Clean logs — WARN about tls: terminate should disappear after policy.rs fix + bot restart
docker exec openshell-cluster-openshell \
    kubectl -n openshell logs rightclaw-right -c agent --tail=20 | grep -i 'tls.*terminate'
# expect: no matches
```

## Useful reference paths

- Aggregator service config: `crates/rightclaw-cli/src/aggregator.rs:564`
- Bearer middleware: `crates/rightclaw-cli/src/aggregator.rs` (search `bearer_auth_middleware`)
- Policy codegen: `crates/rightclaw/src/codegen/policy.rs`
- Agent policies on disk: `~/.rightclaw/agents/<agent>/policy.yaml`
- CC client config: `~/.rightclaw/agents/<agent>/mcp.json`
- Aggregator log: `~/.rightclaw/logs/mcp-aggregator.log.<date>`
- Stream logs: `~/.rightclaw/logs/streams/<session-uuid>.ndjson` — look for `"mcp_servers":[{"name":"right","status":...}]` in init events
- Sandbox pod log (TLS deprecation WARN): `docker exec openshell-cluster-openshell kubectl -n openshell logs rightclaw-<agent> -c agent`

## External refs

- rmcp 1.4 release: https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v1.4.0
- rmcp host-check PR: https://github.com/modelcontextprotocol/rust-sdk/pull/764
- rmcp 1.5 source (current): `~/.cargo/registry/src/index.crates.io-.../rmcp-1.5.0/src/transport/streamable_http_server/tower.rs`
- OpenShell TLS auto-terminate PR: https://github.com/NVIDIA/OpenShell/pull/544
- OpenShell TLS auto-terminate issue: https://github.com/NVIDIA/OpenShell/issues/533
- Dependabot bump commit: `49ccb54c chore(deps): bump the prod-deps group with 17 updates`
- Partial revert commit: `96e98b1f revert(deps): roll back breaking dependabot bumps`
