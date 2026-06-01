# MCP Detection Resolve-Gate — Design

**Date:** 2026-06-02
**Status:** Approved (brainstorm)
**Area:** `right-mcp` MCP auth detection, dashboard detection probe client
**Follows:** `docs/superpowers/specs/2026-06-02-mcp-local-server-ssrf-policy-design.md`
(the two-tier `NetworkPolicy` change this hardens)

## Problem

The MCP local-server SSRF policy change (landed as commit `4c551403`) switched the
dashboard detection probe client to `NetworkPolicy::AllowPrivate` so that the
operator's private/Tailscale base URLs are reachable. A `max`-effort code review
found that this widened the detection trust boundary in a way the original spec
explicitly said it would not. Two faces of one root cause:

### Root cause

`is_public_url` / `is_public_http_url` classify a host by **literal inspection
only** — `is_private_or_link_local_host` returns `false` for every
`url::Host::Domain(_)` (`crates/right-mcp/src/credentials.rs:277`). A hostname is
never resolved before the public/private decision. Consequences:

1. **Private base URLs given as hostnames bypass the short-circuit.** A Tailscale
   MagicDNS URL (`http://openclaw.owl-skate.ts.net:27123/mcp`, the obsidian repro
   host) is classed public, so `detect_mcp_auth` (`crates/right-mcp/src/detect.rs:101`)
   skips the `PrivateNetworkNoOauth` arm and runs OAuth discovery — six doomed
   probe fetches that end at `Headers` via `NoOAuthMetadata`. The spec said
   "when the base URL resolves to a private address" the dashboard does not run
   OAuth; the implementation only honored that for IP literals.

2. **Server-supplied discovered URLs can reach the operator's LAN.** Discovery
   applies `mcp_detection_url_policy` (`PublicOnly`, via `is_public_http_url`)
   before each fetch — but that is a **string** check: it rejects private IP
   literals and `localhost`, and passes any other domain. A discovered URL (e.g.
   `WWW-Authenticate: resource_metadata="https://rebind.evil.com/x"`) that is a
   domain resolving to a private IP passes the string check, and the
   `AllowPrivate` detection client then resolves and connects to the private
   address. The original spec asserted "server-supplied / metadata-derived URLs
   stay `PublicOnly`"; that invariant is false at the DNS layer for rebinding
   domains.

### Severity (why this is a correctness/invariant fix, not an incident)

Low and bounded: detection is pre-auth (no credentials sent), redirects are
disabled (`redirect::none`), cloud-metadata is denied in **both** tiers
regardless (`is_cloud_metadata`), the response is a blind oracle (not returned to
the attacker), and it requires the operator to paste an attacker-controlled URL.
The compelling reasons to fix are (a) the branch contradicts its own documented
invariant, and (b) face #1 is a real UX gap for the exact case the branch exists
to support.

A string check cannot catch a rebinding domain — only a `PublicOnly` **resolver**
can. So the fix must move server-supplied fetches behind a `PublicOnly` resolver,
which in turn requires that private base URLs never reach discovery.

## Design

### 1. Resolve the base host in `detect`, then classify

`detect_mcp_auth_with_url_policy` gains a host-classification step that resolves a
**domain** host before the public/private branch. IP-literal handling is
unchanged (the existing `is_loopback_url` / `is_public_url` literal checks still
run first and preserve every current literal behavior). For a domain host, resolve
via `tokio::net::lookup_host((host, 0))` — the same call `PublicNetworkResolver`
uses — and map the resolved addresses to a recommendation that **mirrors the
existing literal behavior exactly**:

| Resolved addresses contain… | Recommendation | Reason |
|---|---|---|
| a loopback address (`127.0.0.0/8`, `::1`) | `UrlAsIs` | `LoopbackOrPrivate` |
| a private-LAN address (RFC1918 / CGNAT `100.64/10` / ULA `fc00::/7`) | `Headers` | `PrivateNetworkNoOauth` |
| only public addresses | run OAuth discovery | (`OAuthDiscovered` / `NoOAuthMetadata`) |

The resolved-address path classifies with the canonical `ssrf::is_loopback_addr`
and `ssrf::is_user_private_lan` predicates directly (single source of truth with
the connect tier). The existing literal checks (`is_loopback_url`, `is_public_url`)
still run first for IP-literal hosts and are unchanged. #3's consolidation rides
along opportunistically: align `is_private_or_link_local_ipv6`'s ULA check to
`ssrf::is_user_private_lan` so the range definitions live in one place rather than
a parallel stdlib `is_unique_local()` call. `validate_server_url` keeps its literal
`ip_allowed(AllowPrivate)` check.

**Edge-case rules:**

- **Mixed results** (host resolves to both public and private/loopback addresses):
  *any* loopback → `UrlAsIs`; else *any* private-LAN → `Headers`. Conservative:
  `Headers`/`UrlAsIs` always work; OAuth is the only risky recommendation, and
  detection is advisory.
- **Resolution failure / empty / NXDOMAIN:** fall through to the discovery path
  (today's behavior). The `PublicOnly` discovery client then fails to connect if
  the host is truly unreachable; `detect` returns the same error class it does now.
- **Resolves only to link-local / cloud-metadata:** not loopback and not
  private-LAN → discovery path → `PublicOnly` resolver blocks → `detect` errors.
  Acceptable (such a host is broken or hostile). Link-local / cloud-metadata IP
  *literals* still short-circuit unchanged via `is_public_url`.
- Resolve-in-`detect` makes **no connection** — it only classifies. The single
  connection still goes through the hardened client. A rebind between `detect`'s
  lookup and the client's lookup affects only the advisory recommendation, never
  enforcement.

### 2. Detection probe client becomes `PublicOnly`

`detection_http_client()` (`crates/bot/src/telegram/dashboard/mcp.rs:466`) changes
from `AllowPrivate` to `PublicOnly`. Because private base URLs now short-circuit
before discovery, the probe client never needs private reach, and any
server-supplied discovered URL — literal or rebinding domain — is filtered at the
resolver. This removes the AllowPrivate-detection-client special case and makes
the detection policy uniform (`PublicOnly`). `mcp_detection_url_policy` stays as
cheap early defense-in-depth (rejects private literals before the network round
trip); the resolver is now the authoritative backstop for rebinding domains.

### 3. Deliberately unchanged (preserves the obsidian fix)

- **Runtime proxy / reconnect / health client (`crates/right/src/main.rs:1037`)
  stays `AllowPrivate`** — it connects to the operator's *registered* base URL,
  the accepted-risk feature this whole effort delivers.
- **`validate_server_url` and the dashboard add-path pre-checks
  (`is_allowed_http_url(_, AllowPrivate)`) stay `AllowPrivate`** — registering a
  private/Tailscale server must still succeed.
- **`mcp-add` connect (`internal_api.rs:515`) and set-headers reconnect
  (`internal_api.rs:662`) stay `AllowPrivate`** — they connect to the registered
  operator base URL, like the runtime proxy.

Only the **detection probe** path tightens. The obsidian flow is unaffected:
registration validates under `AllowPrivate`, the runtime proxy connects under
`AllowPrivate`, and detection now recommends `Headers` for the MagicDNS host
*without* probing.

## Testability

Host classification extracts to a unit with an **injectable resolver** so the
resolve→private→`Headers` and resolve→loopback→`UrlAsIs` paths are covered by
tests with a stub resolver, not left to implicit wiring. Production passes a thin
adapter over `tokio::net::lookup_host`; tests pass a stub returning canned
addresses. Existing `detect` integration tests are unaffected: their fake
`*.test` hosts fail real resolution and fall through to discovery exactly as
before.

## Threat-model delta

| Target | Before this fix | After this fix |
|---|---|---|
| Operator base URL = private IP literal | Headers (short-circuit) | Headers (unchanged) |
| Operator base URL = private hostname (MagicDNS) | runs OAuth discovery via AllowPrivate client | **Headers, no probe** (resolve-gate) |
| Server-supplied discovered URL = private IP literal | blocked by `url_policy` string check | blocked (unchanged) |
| Server-supplied discovered URL = domain → private IP | **fetched** (AllowPrivate client) | **blocked at resolver** (PublicOnly client) |
| Runtime proxy → operator's registered private server | allowed (AllowPrivate) | allowed (unchanged — accepted risk) |

Residual accepted risk (unchanged from the parent spec): an operator who
deliberately registers a malicious base URL on their own LAN. The runtime proxy
re-resolves that operator-chosen host on each connection; that is the operator's
own target and out of scope for an SSRF DNS guard.

## Testing (TDD)

1. **Classification unit (injected resolver):** loopback addrs → `UrlAsIs`;
   private-LAN/CGNAT/ULA → `Headers`; only-public → discovery signal; mixed
   (public + private) → `Headers`; mixed (public + loopback) → `UrlAsIs`; empty →
   fall-through signal; resolver error → fall-through signal.
2. **`detect` with stub resolver:** a private-resolving hostname recommends
   `Headers` / `PrivateNetworkNoOauth` and issues **zero** probe requests; a
   public-resolving hostname proceeds to discovery.
3. **PublicOnly detection client (regression for face #2):** with the production
   `PublicOnly` client, a discovered URL that is a domain resolving to a private
   address is rejected at the resolver (surfaces `PUBLIC_DNS_ERROR_MARKER`), not
   fetched. Retain the existing
   `guarded_detection_rejects_private_resource_metadata_before_fetch` literal test.
4. **Unchanged-path regression:** registering and runtime-connecting a private
   base URL still succeed under `AllowPrivate` (existing
   `validate_server_url_allows_private_loopback_and_tailscale` and the
   AllowPrivate resolver tests stay green).
5. **Verification:** targeted `-p right-mcp` and `-p right-bot` during the loop;
   `devenv shell -- cargo test --workspace` final, mandatory.

## Out of scope

- Tightening the runtime proxy / registration tiers (operator-chosen targets,
  accepted risk).
- Per-server policy overrides.
- A full rewrite of `credentials.rs` IP classification — it is aligned to the
  `ssrf` predicates (the IPv6 ULA check), not rewritten; the literal checks
  (`is_public_url`, `is_loopback_url`) and `validate_server_url` stay.
- mDNS / `.local` special handling beyond whatever the host OS resolver returns.
