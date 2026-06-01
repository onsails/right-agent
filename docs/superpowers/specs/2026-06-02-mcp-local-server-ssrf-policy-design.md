# MCP Local-Server SSRF Policy — Design

**Date:** 2026-06-02
**Status:** Approved (brainstorm)
**Area:** `right-mcp` SSRF hardening, dashboard MCP add flow

## Problem

External MCP servers hosted on the operator's own private network (Tailscale,
LAN) cannot connect through the host aggregator. The `obsidian` server at
`http://openclaw.owl-skate.ts.net:27123/mcp` (resolves to `100.85.147.49`, in
the Tailscale CGNAT range `100.64.0.0/10`) sits permanently `unreachable`.

### Root cause

Commit `26f134c9 fix(mcp): harden dashboard control surface` (2026-05-26) wired
`ssrf::hardened_client_builder()` into the runtime aggregator/reconnect/health
HTTP client (`crates/right/src/main.rs:1037`). Its `PublicNetworkResolver`
(`crates/right-mcp/src/ssrf.rs`) strips private/loopback/link-local addresses
from DNS results, treating `100.64.0.0/10` as non-public. The host resolves the
obsidian hostname to a single CGNAT address, so after filtering no address
remains, DNS resolution fails, and reqwest surfaces it as the generic
`error sending request for url … when send initialize request` (the
`PUBLIC_DNS_ERROR_MARKER` cause is buried in the source chain). The health
reconciler retries every 20s and fails identically each time. A plain `curl`
from the same host succeeds (HTTP 200, valid token), confirming the box is up
and the only blocker is the SSRF DNS resolver.

The same block also applies at registration (`validate_server_url`,
`crates/right-mcp/src/credentials.rs:285`), so a fresh `mcp_add` of any
private/LAN URL is rejected too. `obsidian` predates the check (registered
2026-05-18), which is why its row exists at all.

### Why the original rationale is too coarse

The SSRF guard was specified as defense-in-depth on the host aggregator
(`docs/superpowers/specs/2026-04-12-mcp-aggregator-design.md`): block RFC1918 /
link-local / loopback and, originally, non-HTTPS. But:

- MCP servers are registered **only by the operator** via the dashboard; agents
  cannot register servers. The base URL is operator-supplied and trusted.
- Connecting to private infrastructure (the operator's own Tailscale/LAN box) is
  a first-class use case for this platform, not an attack.
- The current policy is backwards relative to real risk: it **blocks** private
  IPs (not a real risk for an operator-chosen target) while **silently
  allowing** plaintext HTTP (the actual confidentiality risk — credentials in
  the clear).

The real SSRF vector is not "private IP" but **pivot**: a server-supplied URL
(`.well-known`, `token_endpoint`, OAuth `resource`, HTTP redirect) jumping to an
internal target — e.g. a public base URL `evil.com` whose discovered
`token_endpoint` points at `169.254.169.254` (cloud metadata) or
`10.0.0.5/admin`. Danger lives in URLs the **server** controls, not in the base
URL the operator typed.

## Design

### 1. Two-tier network policy (`crates/right-mcp/src/ssrf.rs`)

Introduce a policy enum threaded into the DNS resolver and the URL validator:

```rust
pub enum NetworkPolicy {
    /// Current behaviour: only globally-routable public addresses.
    PublicOnly,
    /// Public addresses plus the operator's private LAN families.
    AllowPrivate,
}
```

`AllowPrivate` is defined conservatively in terms of the existing public check
plus exactly three private LAN families, with a cloud-metadata deny-list layered
on top:

```
AllowPrivate(ip) = (is_public_ip(ip) || is_user_private_lan(ip))
                   && !is_cloud_metadata(ip)

is_user_private_lan(ip):
    IPv4: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16,   (RFC1918)
          100.64.0.0/10                                 (RFC6598 CGNAT / Tailscale)
    IPv6: fc00::/7                                       (ULA)

is_cloud_metadata(ip):   # explicit deny-list, applied in BOTH tiers
    169.254.169.254      (AWS / GCP / Azure / OpenStack IMDS — IPv4)
    100.100.100.200      (Alibaba Cloud IMDS — INSIDE 100.64/10 CGNAT)
    fd00:ec2::254        (AWS IMDS — IPv6, INSIDE fc00::/7 ULA)
```

Loopback (`127.0.0.0/8`, `::1`), link-local (`169.254.0.0/16`, `fe80::/10`),
unspecified (`0.0.0.0`, `::`), multicast, and reserved/TEST-NET ranges are not
members of the three LAN families, so they remain blocked under `AllowPrivate`
automatically.

**The subtle case the deny-list exists for:** two cloud metadata addresses fall
*inside* allowed families — `fd00:ec2::254` is a valid ULA address, and
`100.100.100.200` is a valid CGNAT address Tailscale could nominally assign.
Without `is_cloud_metadata`, "allow ULA/CGNAT" would re-open IPv6/Alibaba
metadata to a DNS-rebinding pivot at runtime (the `AllowPrivate` base-URL client
re-resolves the hostname on every reconnect). The deny-list is checked in both
`PublicOnly` and `AllowPrivate` so the block holds regardless of tier. Cost of
the carve-out is three `/32`/`/128` constants; no legitimate MCP server runs on
a cloud metadata IP.

`PublicNetworkResolver` carries a `NetworkPolicy` and filters resolved addresses
accordingly. `is_public_http_url` gains a policy parameter (the existing public
behaviour stays the `PublicOnly` arm). `hardened_client_builder(policy)` selects
the resolver. IPv4-mapped IPv6 continues to fold through the IPv4 check.

### 2. Call-site → policy assignment

`AllowPrivate` (connecting to the operator-supplied base URL):

| Call-site | Purpose |
|---|---|
| `crates/right/src/main.rs:1037` | runtime proxy / reconnect / health client |
| `crates/right/src/internal_api.rs:515` | `mcp-add` upstream connect |
| `crates/right/src/internal_api.rs:662` | set-headers background reconnect |
| `crates/bot/src/telegram/dashboard/mcp.rs:461` | detection probe client |
| `crates/bot/src/telegram/dashboard/mcp.rs:120,191` | add/detection URL validate |
| `crates/right-mcp/src/credentials.rs:285` (`validate_server_url`) | registration validate |

`PublicOnly` (server-supplied / metadata-derived URLs — unchanged behaviour):

| Call-site | Purpose |
|---|---|
| `crates/right-mcp/src/refresh.rs:190` | OAuth token refresh (`token_endpoint`) |
| `crates/bot/src/telegram/oauth_callback.rs:233` | OAuth token exchange |
| `crates/right/src/internal_api.rs:325` | OAuth reconnect client |
| `crates/right/src/internal_api.rs:804` | `token_endpoint` validate |
| `crates/right-mcp/src/oauth.rs:2076` | discovered metadata URL validate |
| `crates/bot/src/telegram/oauth_callback.rs:322`, `dashboard/mcp.rs:529` | callback/token URL validate |

HTTP redirects remain disabled everywhere (`redirect::none`), so a server cannot
302 the host into the internal network regardless of tier.

### 3. OAuth not supported for local servers

Local OAuth would require a private `token_endpoint`, which the `PublicOnly`
tier rejects by design. We make this explicit rather than a silent failure:

- Detection: when the base URL resolves to (or is) a private/LAN address, the
  dashboard does **not** recommend OAuth and surfaces "OAuth is not supported for
  local servers — use Headers."
- If the operator manually picks OAuth for a private base URL, token exchange
  fails at the `PublicOnly` check with a clear, non-generic error message.

Local servers use header / bearer / no-auth (e.g. obsidian uses a static
`Authorization` header), which is the common case.

### 4. Dashboard warn-ack on plaintext HTTP only

Reuse the existing warn-ack primitive from the providers form (amber border,
per-flow ack ref). When the operator adds an `http://` (non-TLS) server, require
an acknowledgement:

> Plaintext HTTP — credentials are sent without TLS encryption. Safe only if the
> transport is otherwise encrypted (e.g. Tailscale / WireGuard) or a trusted LAN.

A private IP alone produces **no** warning — per the agreed risk model, the
unsafe property is plaintext transport, not address privacy.

### 5. Upgrade / self-healing

Pure runtime + code-logic change; no codegen, sandbox, or on-disk state changes.
The existing `obsidian` row self-heals: after the bot restarts with the fix, the
health reconciler's next tick (≤20s) connects it through the `AllowPrivate`
client. No re-add, no sandbox recreation, no manual edits. Conforms to the
self-healing-platform convention.

## Threat model summary

| Target | Base URL (operator) | Metadata/token (server) |
|---|---|---|
| Public IP | allow | allow |
| RFC1918 / CGNAT / ULA | **allow** (was block) | block (pivot defense) |
| Loopback | block | block |
| Link-local / cloud metadata | block | block |
| Plaintext HTTP | allow + warn-ack | allow (token endpoints rarely HTTP; covered by HTTPS-typical) |

Residual accepted risk: an operator who deliberately registers a malicious base
URL on their own LAN. This requires operator action to add an attacker-controlled
server and is out of scope for an SSRF DNS guard.

## Testing (TDD)

1. **Regression first (must fail before the fix):** `AllowPrivate` resolver
   admits `100.85.147.49` and the `100.64.0.0/10` range; a runtime connect to a
   CGNAT-resolving host does not fail at DNS. Direct obsidian repro.
2. `PublicOnly` still strips private addresses — pivot `public base →
   token_endpoint 10.x` stays blocked.
3. Hard block holds in both tiers: loopback, `169.254.169.254`, `0.0.0.0`,
   `fe80::` rejected under `AllowPrivate` and `PublicOnly`. **Metadata-in-family
   carve-out:** `fd00:ec2::254` (inside allowed ULA) and `100.100.100.200`
   (inside allowed CGNAT) MUST be rejected under `AllowPrivate` — this is the
   test that fails if `is_cloud_metadata` is forgotten.
4. `validate_server_url`: Tailscale / RFC1918 / ULA pass; loopback / link-local
   rejected; non-HTTP(S) scheme rejected.
5. Dashboard SSR test: `http://` add renders the ack warning; `https://` does
   not; private `https://` renders no privacy warning.
6. Verification: `devenv shell -- cargo test --workspace` (final, mandatory);
   targeted `-p right-mcp` during the loop.

## Out of scope

- Per-server policy overrides in `agent.yaml`.
- mDNS / `.local` special handling (works if the host OS resolver returns an
  address; not specially resolved).
- Changing OpenShell sandbox network policy (unrelated to the host-side guard).
