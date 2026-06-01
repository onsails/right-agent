# MCP Local-Server SSRF Policy — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let operator-registered MCP servers on private/Tailscale/LAN addresses connect through the host aggregator (fixing the permanently-`unreachable` obsidian server), while keeping server-supplied metadata/token URLs strict and never supporting OAuth for local servers.

**Architecture:** Introduce a two-tier `NetworkPolicy` (`PublicOnly` | `AllowPrivate`) in `right-mcp::ssrf`. The operator-supplied base URL is connected/validated under `AllowPrivate` (public + RFC1918/CGNAT/ULA, minus an always-denied cloud-metadata list). Server-supplied metadata/token/redirect URLs keep the existing `PublicOnly` behaviour. OAuth auto-detection stays gated on the strict `is_public_url`, so private URLs are recommended Headers and never attempt OAuth. The dashboard warns (with ack) only when the operator adds a plaintext `http://` server.

**Tech Stack:** Rust 2024 (`right-mcp`, `right`, `bot` crates), `reqwest` custom DNS resolver, `thiserror`/`anyhow`; Vue 3 + TypeScript + Vitest for the `right-dashboard` frontend.

**Spec:** `docs/superpowers/specs/2026-06-02-mcp-local-server-ssrf-policy-design.md`

**Git:** Work on a feature branch (or this session's in-place checkout) and land via fast-forward to `origin/master`. Conventional Commits. Do not push until the final workspace test passes.

---

## File Structure

- `crates/right-mcp/src/ssrf.rs` — **core**: `NetworkPolicy` enum, `is_user_private_lan`, `is_cloud_metadata`, `ip_allowed`, policy-parameterised `PublicNetworkResolver`, `hardened_client_builder(policy)`, `is_allowed_http_url(input, policy)`. `is_public_http_url` becomes a thin `PublicOnly` wrapper.
- `crates/right-mcp/src/credentials.rs` — `validate_server_url` relaxed to `AllowPrivate`. `is_public_url`/`is_loopback_url` unchanged (they remain the strict OAuth-recommendation gate).
- `crates/right-mcp/src/detect.rs` — private (non-loopback) base URL now recommends `Headers` with a new `DetectionReason::PrivateNetworkNoOauth` instead of `UrlAsIs`.
- `crates/right/src/main.rs`, `crates/right/src/internal_api.rs`, `crates/bot/src/telegram/dashboard/mcp.rs`, `crates/bot/src/telegram/oauth_callback.rs`, `crates/right-mcp/src/refresh.rs` — pass the correct `NetworkPolicy` to each `hardened_client_builder` / URL-validation call-site.
- `crates/right-dashboard/frontend/src/views/mcpViewModel.ts` — pure `evaluateHttpUrlSubmit(url, alreadyWarned)` helper.
- `crates/right-dashboard/frontend/src/views/McpView.vue` — `addWarn`/`addWarnAck` refs, amber `.notice.warn` markup, submit gate, re-arm watcher.
- `crates/right-dashboard/frontend/src/types.ts` — widen the `DetectionReason` string union if it is a strict literal type.

---

## Phase 1 — SSRF policy core (unblocks obsidian)

### Task 1.1: `NetworkPolicy`, private-LAN + cloud-metadata predicates, `ip_allowed`

**Files:**
- Modify: `crates/right-mcp/src/ssrf.rs`
- Test: `crates/right-mcp/src/ssrf.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests** — append to the `tests` module in `crates/right-mcp/src/ssrf.rs`:

```rust
    #[test]
    fn allow_private_admits_tailscale_cgnat() {
        // 100.85.147.49 is the obsidian repro address; 100.64.0.0/10 CGNAT.
        for ip in ["100.85.147.49", "100.64.0.0", "100.127.255.255"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(ip_allowed(ip, NetworkPolicy::AllowPrivate), "{ip} must be allowed");
            assert!(!ip_allowed(ip, NetworkPolicy::PublicOnly), "{ip} must be public-blocked");
        }
    }

    #[test]
    fn allow_private_admits_rfc1918_and_ula() {
        for ip in ["10.0.0.5", "172.16.9.9", "192.168.1.10", "fc00::1", "fd12:3456::1"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(ip_allowed(ip, NetworkPolicy::AllowPrivate), "{ip} must be allowed");
        }
    }

    #[test]
    fn allow_private_still_blocks_loopback_and_link_local() {
        for ip in ["127.0.0.1", "::1", "169.254.0.5", "fe80::1", "0.0.0.0", "::"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(!ip_allowed(ip, NetworkPolicy::AllowPrivate), "{ip} must stay blocked");
            assert!(!ip_allowed(ip, NetworkPolicy::PublicOnly), "{ip} must stay blocked");
        }
    }

    #[test]
    fn cloud_metadata_blocked_even_inside_allowed_families() {
        // fd00:ec2::254 is inside ULA fc00::/7; 100.100.100.200 is inside CGNAT 100.64/10.
        for ip in ["169.254.169.254", "100.100.100.200", "fd00:ec2::254"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(!ip_allowed(ip, NetworkPolicy::AllowPrivate), "metadata {ip} must be denied");
            assert!(!ip_allowed(ip, NetworkPolicy::PublicOnly), "metadata {ip} must be denied");
        }
    }

    #[test]
    fn allow_private_admits_public() {
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        assert!(ip_allowed(ip, NetworkPolicy::AllowPrivate));
        assert!(ip_allowed(ip, NetworkPolicy::PublicOnly));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-mcp ssrf::tests::allow_private 2>&1 | tail -20`
Expected: FAIL — `cannot find function ip_allowed` / `cannot find type NetworkPolicy`.

- [ ] **Step 3: Implement the policy core** — in `crates/right-mcp/src/ssrf.rs`, after the `PUBLIC_DNS_ERROR_MARKER` const, add:

```rust
/// Outbound-connection trust tier. `PublicOnly` is the historical SSRF-hardened
/// behaviour (globally-routable only). `AllowPrivate` additionally permits the
/// operator's own private LAN / Tailscale / ULA ranges — used only for the
/// operator-supplied base URL, never for server-supplied metadata URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    PublicOnly,
    AllowPrivate,
}

/// RFC1918 + RFC6598 CGNAT (Tailscale) + IPv6 ULA — the operator's own network.
fn is_user_private_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let n = u32::from(v4);
            n >> 24 == 10
                || in_ipv4_cidr(n, Ipv4Addr::new(172, 16, 0, 0), 12)
                || in_ipv4_cidr(n, Ipv4Addr::new(192, 168, 0, 0), 16)
                || in_ipv4_cidr(n, Ipv4Addr::new(100, 64, 0, 0), 10)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_user_private_lan(IpAddr::V4(v4));
            }
            in_ipv6_cidr(u128::from(v6), Ipv6Addr::from_str("fc00::").unwrap(), 7)
        }
    }
}

/// Cloud instance-metadata addresses. Denied in EVERY tier. Two of these fall
/// inside otherwise-allowed families (`100.100.100.200` in CGNAT, `fd00:ec2::254`
/// in ULA), so the deny-list is essential — range membership alone would re-open
/// them to a DNS-rebinding pivot on the AllowPrivate base-URL client.
fn is_cloud_metadata(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4 == Ipv4Addr::new(169, 254, 169, 254) || v4 == Ipv4Addr::new(100, 100, 100, 200)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_cloud_metadata(IpAddr::V4(v4));
            }
            v6 == Ipv6Addr::from_str("fd00:ec2::254").unwrap()
        }
    }
}

/// Is `ip` permitted as a connection target under `policy`?
pub fn ip_allowed(ip: IpAddr, policy: NetworkPolicy) -> bool {
    if is_cloud_metadata(ip) {
        return false;
    }
    match policy {
        NetworkPolicy::PublicOnly => is_public_ip(ip),
        NetworkPolicy::AllowPrivate => is_public_ip(ip) || is_user_private_lan(ip),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-mcp ssrf::tests 2>&1 | tail -20`
Expected: PASS (all new tests + pre-existing ssrf tests green).

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/ssrf.rs
git commit -m "feat(mcp): NetworkPolicy + private-LAN/cloud-metadata IP predicates"
```

---

### Task 1.2: Parameterise resolver, client builder, and URL validator by policy

**Files:**
- Modify: `crates/right-mcp/src/ssrf.rs`
- Modify (compile fix): `crates/bot/src/telegram/dashboard/mcp.rs:585` (resolver test construction), all `hardened_client_builder(...)` and `is_public_http_url` callers (default to `PublicOnly` here; specific flips happen in Task 1.3)
- Test: `crates/right-mcp/src/ssrf.rs`

- [ ] **Step 1: Write the failing test** — append to ssrf `tests`:

```rust
    #[tokio::test]
    async fn allow_private_resolver_keeps_cgnat_address() {
        use reqwest::dns::Resolve;
        // 100.x is unroutable on the public internet but must survive AllowPrivate
        // filtering. We resolve a literal-like host via the OS resolver indirectly;
        // instead assert the filter predicate the resolver uses.
        let ip: IpAddr = "100.85.147.49".parse().unwrap();
        assert!(ip_allowed(ip, NetworkPolicy::AllowPrivate));
        // Resolver construction must accept a policy.
        let _ = PublicNetworkResolver::new(NetworkPolicy::AllowPrivate);
        let r = PublicNetworkResolver::new(NetworkPolicy::PublicOnly);
        // Public host resolves to >=1 public addr under PublicOnly.
        let addrs = r.resolve(reqwest::dns::Name::from_str("example.com").unwrap()).await;
        assert!(addrs.is_ok(), "public host must resolve under PublicOnly");
    }
```

> Note: add `use std::str::FromStr as _;` if not already imported in the test module (the file already imports it at top scope).

- [ ] **Step 2: Run to verify it fails**

Run: `devenv shell -- cargo test -p right-mcp ssrf::tests::allow_private_resolver 2>&1 | tail -20`
Expected: FAIL — `PublicNetworkResolver::new` not found / `PublicNetworkResolver` is a unit struct.

- [ ] **Step 3: Implement** — replace the `PublicNetworkResolver` struct, its impl, `hardened_client_builder`, and `is_public_http_url` in `crates/right-mcp/src/ssrf.rs` with:

```rust
/// `reqwest` DNS resolver that strips addresses disallowed by `policy`. If
/// nothing remains, resolution fails with [`PUBLIC_DNS_ERROR_MARKER`].
#[derive(Debug, Clone, Copy)]
pub struct PublicNetworkResolver {
    policy: NetworkPolicy,
}

impl PublicNetworkResolver {
    pub fn new(policy: NetworkPolicy) -> Self {
        Self { policy }
    }
}

impl reqwest::dns::Resolve for PublicNetworkResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        let policy = self.policy;
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            let allowed = addrs
                .filter(|addr| ip_allowed(addr.ip(), policy))
                .collect::<Vec<_>>();
            if allowed.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("{PUBLIC_DNS_ERROR_MARKER}: {host}"),
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }

            Ok(Box::new(allowed.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Build a `reqwest::ClientBuilder` pre-hardened against SSRF under `policy`.
/// Callers add their own `connect_timeout` / `timeout` and then call `.build()`.
pub fn hardened_client_builder(policy: NetworkPolicy) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .dns_resolver(Arc::new(PublicNetworkResolver::new(policy)))
}

/// Validate a user-/metadata-supplied HTTP URL under `policy` before outbound
/// I/O. IP literals are checked directly; domains are filtered at connect time
/// by [`PublicNetworkResolver`].
pub fn is_allowed_http_url(input: &str, policy: NetworkPolicy) -> bool {
    let Ok(parsed) = reqwest::Url::parse(input) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none()
        && parsed.host().is_some_and(|host| match host {
            url::Host::Domain(domain) => !is_localhost_domain(domain),
            url::Host::Ipv4(ip) => ip_allowed(IpAddr::V4(ip), policy),
            url::Host::Ipv6(ip) => ip_allowed(IpAddr::V6(ip), policy),
        })
}

/// Public-only URL check (historical behaviour). Thin wrapper over
/// [`is_allowed_http_url`] with [`NetworkPolicy::PublicOnly`].
pub fn is_public_http_url(input: &str) -> bool {
    is_allowed_http_url(input, NetworkPolicy::PublicOnly)
}
```

- [ ] **Step 4: Fix the only non-default construction site** — in `crates/bot/src/telegram/dashboard/mcp.rs` around line 585 (the resolver unit test), change `let resolver = PublicNetworkResolver;` to:

```rust
        let resolver = PublicNetworkResolver::new(right_mcp::ssrf::NetworkPolicy::PublicOnly);
```

(Also update its `use` line at ~559 to keep importing `PublicNetworkResolver`, `is_public_ip`, `PUBLIC_DNS_ERROR_MARKER` — unchanged names.)

- [ ] **Step 5: Update every `hardened_client_builder()` caller to pass a policy** — for THIS task pass `right_mcp::ssrf::NetworkPolicy::PublicOnly` at all of them to preserve current behaviour (Task 1.3 flips the base-URL ones):
  - `crates/right/src/main.rs:1037`
  - `crates/right/src/internal_api.rs:325, 515, 662`
  - `crates/bot/src/telegram/dashboard/mcp.rs:461`
  - `crates/bot/src/telegram/oauth_callback.rs:233`
  - `crates/right-mcp/src/refresh.rs:190`

Each becomes `right_mcp::ssrf::hardened_client_builder(right_mcp::ssrf::NetworkPolicy::PublicOnly)` (use `crate::ssrf::` form inside `right-mcp`).

- [ ] **Step 6: Verify it compiles and tests pass**

Run: `devenv shell -- cargo test -p right-mcp ssrf:: 2>&1 | tail -20 && devenv shell -- cargo check -p right -p bot 2>&1 | tail -15`
Expected: PASS / clean check.

- [ ] **Step 7: Commit**

```bash
git add crates/right-mcp/src/ssrf.rs crates/bot/src/telegram/dashboard/mcp.rs crates/right/src/main.rs crates/right/src/internal_api.rs crates/bot/src/telegram/oauth_callback.rs crates/right-mcp/src/refresh.rs
git commit -m "refactor(mcp): thread NetworkPolicy through resolver/client/url-validator"
```

---

### Task 1.3: Flip base-URL call-sites to `AllowPrivate`

**Files:**
- Modify: `crates/right/src/main.rs:1037`
- Modify: `crates/right/src/internal_api.rs:515, 662`
- Modify: `crates/bot/src/telegram/dashboard/mcp.rs:461, 120, 191`

The remaining `PublicOnly` call-sites (`refresh.rs:190`, `internal_api.rs:325`, `oauth_callback.rs:233`, token-endpoint validators, `mcp_detection_url_policy`) are **left unchanged** — they handle server-supplied URLs.

- [ ] **Step 1: Flip the runtime aggregator/reconnect/health client** — `crates/right/src/main.rs:1037`:

```rust
            let http_client = match right_mcp::ssrf::hardened_client_builder(
                right_mcp::ssrf::NetworkPolicy::AllowPrivate,
            )
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
            {
```

- [ ] **Step 2: Flip the dashboard add/connect clients** — in `crates/right/src/internal_api.rs`, the two base-URL connect clients at lines ~515 and ~662 change their argument to `right_mcp::ssrf::NetworkPolicy::AllowPrivate`. Leave `oauth_reconnect_http_client()` (line ~325) as `PublicOnly`.

- [ ] **Step 3: Flip detection probe client + add pre-checks** — in `crates/bot/src/telegram/dashboard/mcp.rs`:
  - `detection_http_client()` (line ~461): pass `right_mcp::ssrf::NetworkPolicy::AllowPrivate`.
  - The two pre-checks at lines ~120 and ~191: replace `!right_mcp::ssrf::is_public_http_url(&request.url)` with `!right_mcp::ssrf::is_allowed_http_url(&request.url, right_mcp::ssrf::NetworkPolicy::AllowPrivate)`.
  - Leave `mcp_detection_url_policy` (line ~528) calling `is_public_http_url` — it guards server-supplied OAuth-discovery URLs.

- [ ] **Step 4: Verify it compiles**

Run: `devenv shell -- cargo check -p right -p bot 2>&1 | tail -15`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/main.rs crates/right/src/internal_api.rs crates/bot/src/telegram/dashboard/mcp.rs
git commit -m "feat(mcp): connect to operator base URL under AllowPrivate (unblocks Tailscale/LAN)"
```

---

### Task 1.4: Relax `validate_server_url` to `AllowPrivate`

**Files:**
- Modify: `crates/right-mcp/src/credentials.rs:285` (`validate_server_url`)
- Test: `crates/right-mcp/src/credentials.rs` (`#[cfg(test)] mod tests`)

`is_public_url` / `is_loopback_url` / `is_private_or_link_local_*` are **unchanged** — they stay strict for the OAuth-recommendation gate in `detect.rs`.

- [ ] **Step 1: Write failing tests** — add to the credentials `tests` module:

```rust
    #[tokio::test]
    async fn validate_server_url_allows_tailscale_and_rfc1918() {
        validate_server_url("http://openclaw.owl-skate.ts.net:27123/mcp").unwrap();
        validate_server_url("http://100.85.147.49:27123/mcp").unwrap();
        validate_server_url("http://192.168.1.10:8080/mcp").unwrap();
        validate_server_url("http://10.0.0.5/mcp").unwrap();
    }

    #[tokio::test]
    async fn validate_server_url_rejects_loopback_link_local_and_metadata() {
        for url in [
            "http://127.0.0.1:8080/mcp",
            "http://localhost:8080/mcp",
            "http://169.254.169.254/latest/meta-data",
            "http://100.100.100.200/latest/meta-data",
            "http://[fd00:ec2::254]/mcp",
            "ftp://example.com/mcp",
        ] {
            assert!(validate_server_url(url).is_err(), "{url} must be rejected");
        }
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `devenv shell -- cargo test -p right-mcp validate_server_url 2>&1 | tail -20`
Expected: FAIL — Tailscale/RFC1918 URLs currently rejected (`private/link-local address ... not allowed`).

- [ ] **Step 3: Implement** — replace the body of `validate_server_url` (lines ~285-307) with:

```rust
pub fn validate_server_url(url_str: &str) -> Result<(), CredentialError> {
    let parsed = parse_url(url_str)?;

    let url_host = parsed
        .host()
        .ok_or_else(|| CredentialError::InvalidServerUrl("URL has no host".to_string()))?;

    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err(CredentialError::InvalidServerUrl(format!(
            "only HTTP/HTTPS URLs are allowed, got '{}'",
            parsed.scheme()
        )));
    }

    // Operator-supplied base URL: allow public + private LAN/Tailscale/ULA, but
    // never loopback, link-local, or cloud-metadata (AllowPrivate tier).
    let allowed = match url_host {
        url::Host::Domain(domain) => !crate::ssrf::is_localhost_domain(domain),
        url::Host::Ipv4(v4) => {
            crate::ssrf::ip_allowed(IpAddr::V4(v4), crate::ssrf::NetworkPolicy::AllowPrivate)
        }
        url::Host::Ipv6(v6) => {
            crate::ssrf::ip_allowed(IpAddr::V6(v6), crate::ssrf::NetworkPolicy::AllowPrivate)
        }
    };
    if !allowed {
        return Err(CredentialError::InvalidServerUrl(format!(
            "address '{}' is not allowed (loopback, link-local, or cloud-metadata)",
            parsed.host_str().unwrap_or("<unknown>")
        )));
    }

    Ok(())
}
```

> Ensure `use std::net::IpAddr;` is present in `credentials.rs` (it already imports `Ipv4Addr`, `Ipv6Addr`; add `IpAddr` to that `use`).

- [ ] **Step 4: Run to verify they pass**

Run: `devenv shell -- cargo test -p right-mcp validate_server_url 2>&1 | tail -20`
Expected: PASS. (Pre-existing `validate_server_url_https_ok` / `_plain_http_ok` still pass.)

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/credentials.rs
git commit -m "feat(mcp): validate_server_url allows operator private/LAN base URLs"
```

---

### Task 1.5: Phase-1 targeted verification

- [ ] **Step 1: Run the affected crates' tests**

Run: `devenv shell -- cargo test -p right-mcp -p right -p bot 2>&1 | tail -30`
Expected: PASS (note any pre-existing flaky failures per `flaky_tests_parallel_load`; re-run isolated if a cc/invocation or dashboard warn-count test flakes).

> Phase 1 is the complete obsidian fix. Live acceptance is in Phase 4.

---

## Phase 2 — OAuth not supported for local servers

### Task 2.1: Private base URL recommends Headers, never OAuth

**Files:**
- Modify: `crates/right-mcp/src/detect.rs:17-27` (add `DetectionReason::PrivateNetworkNoOauth`), `:89-97` (split loopback vs private)
- Test: `crates/right-mcp/src/detect.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write failing tests** — add to the detect `tests` module:

```rust
    #[tokio::test]
    async fn private_base_url_recommends_headers_not_oauth() {
        let client = reqwest::Client::new();
        let d = detect_mcp_auth(&client, "http://100.85.147.49:27123/mcp")
            .await
            .unwrap();
        assert_eq!(d.recommended_mode, McpAuthMode::Headers);
        assert_eq!(d.reason, DetectionReason::PrivateNetworkNoOauth);
        assert!(!d.oauth_discovered);
        assert!(d.oauth.is_none());
    }

    #[tokio::test]
    async fn loopback_base_url_still_recommends_url_as_is() {
        let client = reqwest::Client::new();
        let d = detect_mcp_auth(&client, "http://127.0.0.1:8080/mcp")
            .await
            .unwrap();
        assert_eq!(d.recommended_mode, McpAuthMode::UrlAsIs);
        assert_eq!(d.reason, DetectionReason::LoopbackOrPrivate);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `devenv shell -- cargo test -p right-mcp detect 2>&1 | tail -20`
Expected: FAIL — `PrivateNetworkNoOauth` variant missing; private URL currently returns `UrlAsIs`/`LoopbackOrPrivate`.

- [ ] **Step 3: Add the reason variant** — in `crates/right-mcp/src/detect.rs`, inside `enum DetectionReason`, add after `LoopbackOrPrivate`:

```rust
    #[serde(rename = "private_network_no_oauth")]
    PrivateNetworkNoOauth,
```

- [ ] **Step 4: Split the loopback/private branch** — replace lines ~89-97 (`if is_loopback_url(...) || !is_public_url(...) { ... }`) with:

```rust
    if is_loopback_url(original_url) {
        return Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::UrlAsIs,
            reason: DetectionReason::LoopbackOrPrivate,
            oauth: None,
        });
    }

    if !is_public_url(&bare_url) {
        // Private / LAN / Tailscale base URL: OAuth is not supported for local
        // servers (its token_endpoint would be private and rejected by the
        // strict metadata policy). Recommend Headers and skip OAuth discovery.
        return Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::Headers,
            reason: DetectionReason::PrivateNetworkNoOauth,
            oauth: None,
        });
    }
```

- [ ] **Step 5: Run to verify they pass**

Run: `devenv shell -- cargo test -p right-mcp detect 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-mcp/src/detect.rs
git commit -m "feat(mcp): private base URL recommends Headers (no OAuth for local servers)"
```

---

### Task 2.2: Regression guard — forced OAuth with a private token endpoint is rejected

**Files:**
- Test: `crates/right-mcp/src/oauth.rs` (`#[cfg(test)] mod tests`)

This locks in that the `PublicOnly` policy still refuses a private `token_endpoint` even if a caller forces OAuth, so OAuth-local cannot silently succeed.

- [ ] **Step 1: Write the test** — add to the oauth `tests` module:

```rust
    #[tokio::test]
    async fn token_exchange_rejects_private_token_endpoint() {
        let client = reqwest::Client::new();
        let policy = |u: &str| -> Result<(), OAuthError> {
            if crate::ssrf::is_public_http_url(u) {
                Ok(())
            } else {
                Err(OAuthError::TokenExchangeFailed("private".into()))
            }
        };
        let err = exchange_token_with_url_policy(
            &client,
            "http://192.168.1.10/token",
            "code",
            "https://example.com/cb",
            "client-id",
            None,
            "verifier",
            "https://example.com/mcp",
            policy,
        )
        .await
        .expect_err("private token_endpoint must be rejected");
        assert!(matches!(err, OAuthError::TokenExchangeFailed(_)));
    }
```

> Adjust the import path/`use super::*;` and the `OAuthError` variant to match the module; `exchange_token_with_url_policy` is defined in this file (line ~910).

- [ ] **Step 2: Run to verify it passes** (behaviour already exists; this is a guard)

Run: `devenv shell -- cargo test -p right-mcp token_exchange_rejects_private 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/right-mcp/src/oauth.rs
git commit -m "test(mcp): guard that private token_endpoint stays rejected under PublicOnly"
```

---

## Phase 3 — Dashboard plaintext-HTTP warn-ack

### Task 3.1: Pure `evaluateHttpUrlSubmit` helper + unit test

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/mcpViewModel.ts`
- Test: `crates/right-dashboard/frontend/src/views/McpView.test.ts`

- [ ] **Step 1: Write failing tests** — add to `McpView.test.ts` (import `evaluateHttpUrlSubmit` from `./mcpViewModel`):

```typescript
import { evaluateHttpUrlSubmit } from './mcpViewModel'

describe('evaluateHttpUrlSubmit', () => {
  it('blocks the first submit of a plaintext http:// url', () => {
    const r = evaluateHttpUrlSubmit('http://openclaw.owl-skate.ts.net:27123/mcp', false)
    expect(r.proceed).toBe(false)
    expect(r.warning).toContain('without TLS')
  })

  it('proceeds on the second submit (already warned)', () => {
    expect(evaluateHttpUrlSubmit('http://box.local/mcp', true)).toEqual({ proceed: true, warning: null })
  })

  it('never warns for https:// urls', () => {
    expect(evaluateHttpUrlSubmit('https://mcp.example.com/mcp', false)).toEqual({ proceed: true, warning: null })
  })
})
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd crates/right-dashboard/frontend && npx vitest run src/views/McpView.test.ts 2>&1 | tail -20`
Expected: FAIL — `evaluateHttpUrlSubmit` is not exported.

- [ ] **Step 3: Implement** — append to `crates/right-dashboard/frontend/src/views/mcpViewModel.ts`:

```typescript
export const PLAINTEXT_HTTP_WARNING =
  'This server uses plaintext http:// — credentials are sent without TLS encryption. Safe only if the transport is otherwise encrypted (e.g. Tailscale/WireGuard) or a trusted LAN. Press Save again to proceed.'

export function evaluateHttpUrlSubmit(
  url: string,
  alreadyWarned: boolean,
): { proceed: boolean; warning: string | null } {
  const isPlaintextHttp = /^http:\/\//i.test(url.trim())
  if (isPlaintextHttp && !alreadyWarned) {
    return { proceed: false, warning: PLAINTEXT_HTTP_WARNING }
  }
  return { proceed: true, warning: null }
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cd crates/right-dashboard/frontend && npx vitest run src/views/McpView.test.ts 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/mcpViewModel.ts crates/right-dashboard/frontend/src/views/McpView.test.ts
git commit -m "feat(dashboard): evaluateHttpUrlSubmit helper for MCP http warn-ack"
```

---

### Task 3.2: Wire warn-ack into `McpView.vue`

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/McpView.vue`
- Test: `crates/right-dashboard/frontend/src/views/McpView.test.ts` (SSR smoke, if not present)

- [ ] **Step 1: Add refs + import** — in the `<script setup>` of `McpView.vue`, near the other add-flow refs (~line 40-49), add:

```typescript
import { evaluateHttpUrlSubmit } from './mcpViewModel'

const addWarn = ref<string | null>(null)
// Set once a plaintext-http URL has been flagged; a second Save then proceeds.
const addWarnAck = ref(false)
```

- [ ] **Step 2: Gate `saveServer`** — at the top of the `saveServer()` body (before `busyAction.value = 'add'`), insert:

```typescript
  const httpCheck = evaluateHttpUrlSubmit(url.value, addWarnAck.value)
  if (!httpCheck.proceed) {
    addWarn.value = httpCheck.warning
    addWarnAck.value = true
    return
  }
  addWarn.value = null
```

- [ ] **Step 3: Re-arm on URL edit** — add a watcher near the other watchers:

```typescript
watch(url, () => {
  addWarnAck.value = false
  addWarn.value = null
})
```

(Ensure `watch` is imported from `vue`.)

- [ ] **Step 4: Reset in `resetAdd`** — inside `resetAdd()` (alongside `url.value = ''`), add:

```typescript
  addWarn.value = null
  addWarnAck.value = false
```

- [ ] **Step 5: Render the amber warning** — near the URL field / add error (~line 423-436), add after the error line:

```vue
        <p v-if="addWarn" class="notice inline warn">{{ addWarn }}</p>
```

And add the amber style to the component `<style scoped>` (mirroring ProvidersView):

```css
.notice.warn {
  color: var(--tg-theme-text-color, #17212b);
  background: rgba(214, 165, 26, 0.14);
  border: 1px solid rgba(214, 165, 26, 0.4);
  border-radius: 7px;
  padding: 6px 8px;
}
```

- [ ] **Step 6: SSR smoke test** — ensure `McpView.test.ts` has a render test (add if absent):

```typescript
import { renderToString } from '@vue/server-renderer'
import { createApp } from 'vue'
import McpView from './McpView.vue'

it('renders without throwing', async () => {
  const html = await renderToString(createApp(McpView))
  expect(typeof html).toBe('string')
})
```

> If the view calls API functions on mount, stub them with `vi.mock('../api', ...)` mirroring `ProvidersView.test.ts`.

- [ ] **Step 7: Run frontend tests**

Run: `cd crates/right-dashboard/frontend && npx vitest run src/views/McpView.test.ts 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/McpView.vue crates/right-dashboard/frontend/src/views/McpView.test.ts
git commit -m "feat(dashboard): plaintext-http warn-ack on MCP server add"
```

---

### Task 3.3: Widen `DetectionReason` TS type (if strict)

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts`

- [ ] **Step 1: Check the type** — `grep -n "loopback_or_private\|DetectionReason\|reason" crates/right-dashboard/frontend/src/types.ts`. If `reason` is a string-literal union, add `'private_network_no_oauth'`:

```typescript
// e.g. reason: 'query_string_present' | 'loopback_or_private' | 'private_network_no_oauth' | 'oauth_discovered' | 'no_oauth_metadata'
```

If `reason` is typed as plain `string`, no change is needed — skip this task.

- [ ] **Step 2: Type-check the frontend**

Run: `cd crates/right-dashboard/frontend && npx vue-tsc --noEmit 2>&1 | tail -20` (or the project's configured type-check script)
Expected: clean.

- [ ] **Step 3: Commit (only if changed)**

```bash
git add crates/right-dashboard/frontend/src/types.ts
git commit -m "chore(dashboard): add private_network_no_oauth to DetectionReason type"
```

---

## Phase 4 — Final verification & live acceptance

### Task 4.1: Full workspace test + frontend suite

- [ ] **Step 1: Frontend full suite**

Run: `cd crates/right-dashboard/frontend && npx vitest run 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 2: Mandatory full workspace test**

Run: `devenv shell -- cargo test --workspace 2>&1 | tail -40`
Expected: PASS. Record any pre-existing flaky failures (`flaky_tests_parallel_load`) and re-run the named tests isolated to confirm they are unrelated.

- [ ] **Step 3: Debug build**

Run: `devenv shell -- cargo build --workspace 2>&1 | tail -10`
Expected: success.

### Task 4.2: Live acceptance — obsidian recovers

> Operator-run; the bot must be rebuilt and restarted to pick up the change.

- [ ] **Step 1: Restart the `right` agent's bot with the new binary** (never bare `right`):

Run: `devenv shell -- cargo run --release --bin right -- restart right`

- [ ] **Step 2: Within ~30s, confirm obsidian connects** via the internal socket:

```bash
curl -s --unix-socket ~/.right/run/internal.sock -X POST http://localhost/mcp-list \
  -H "Content-Type: application/json" -d '{"agent":"right"}' | python3 -m json.tool
```
Expected: the `obsidian` entry shows `"status": "connected"` with `tool_count > 0` and a recent `last_success_at`. (The health reconciler reconnects within one `UNREACHABLE_CADENCE` ≈ 20s tick.)

- [ ] **Step 3: Land** — fast-forward to `origin/master` per project workflow once all checks are green.

---

## Self-Review

**Spec coverage:**
- Two-tier `NetworkPolicy` core → Task 1.1/1.2. ✓
- `is_cloud_metadata` carve-out (fd00:ec2::254, 100.100.100.200) → Task 1.1 (test `cloud_metadata_blocked_even_inside_allowed_families`). ✓
- Call-site → policy table → Task 1.2 (default PublicOnly) + 1.3 (flip base-URL). ✓
- `validate_server_url` relaxation → Task 1.4. ✓
- OAuth-local unsupported (recommend Headers, never OAuth) → Task 2.1; strict token endpoint guard → Task 2.2. ✓
- Plaintext-HTTP warn-ack, private IP silent → Task 3.1/3.2. ✓
- Self-healing upgrade (no re-add) → Task 4.2 live acceptance. ✓
- Testing (regression first, final workspace) → Tasks 1.1, 1.4, 2.1, 4.1. ✓

**Placeholder scan:** Task 3.3 is conditional (depends on whether `types.ts` uses a strict union) — it includes the concrete check command and both branches, so it is not an open-ended placeholder. No "TBD"/"handle edge cases".

**Type consistency:** `NetworkPolicy`, `ip_allowed`, `is_allowed_http_url`, `is_public_http_url`, `PublicNetworkResolver::new`, `hardened_client_builder(policy)`, `DetectionReason::PrivateNetworkNoOauth`, `evaluateHttpUrlSubmit` are used identically across all tasks that reference them.
