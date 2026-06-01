# MCP Detection Resolve-Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the MCP detection DNS-rebind hole — resolve a domain base URL inside `detect` so private/loopback hostnames short-circuit (no OAuth discovery), and make the dashboard detection probe client `PublicOnly` so server-supplied discovered URLs honor `PublicOnly` at the DNS layer, not just a string check.

**Architecture:** `detect_mcp_auth` keeps its literal short-circuits, then for a *domain* host resolves the name via an injectable resolver and classifies the addresses against the canonical `ssrf` predicates (loopback → `UrlAsIs`, private-LAN → `Headers`, public/unknown → discovery). The probe client (`detection_http_client`) flips `AllowPrivate → PublicOnly`; private base URLs never reach discovery, and rebinding discovered domains are stripped by `PublicNetworkResolver`. The runtime proxy and registration tiers stay `AllowPrivate` (operator-chosen targets — unchanged).

**Tech Stack:** Rust 2024, `reqwest` (custom `dns_resolver`), `tokio::net::lookup_host`, `url`, `wiremock` (tests). Crates: `right-mcp` (detect/ssrf/credentials), `right-bot` (dashboard probe client).

**Spec:** `docs/superpowers/specs/2026-06-02-mcp-detection-resolve-gate-design.md`

**Verification cadence (per AGENTS.md):** run the new/regression test first and confirm it fails, implement, rerun the targeted test. Use the narrowest command during the loop (`devenv shell -- cargo test -p right-mcp <filter>`, `-p right-bot` for the dashboard change). Run `devenv shell -- cargo test --workspace` **once at the end** (Task 6) — not after every task.

**Branch/landing:** Work on the current in-place branch (`worktree-mcp-ssrf-policy`); do **not** create a new branch. Land via fast-forward to `origin/master` after the final workspace test passes (same flow as the parent change). Do not push until Task 6 is green.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/right-mcp/src/ssrf.rs` | Canonical IP-policy predicates | Make `is_loopback_addr` `pub(crate)`; add hermetic resolver test |
| `crates/right-mcp/src/detect.rs` | MCP auth-mode detection | Add `ResolvedHostClass` + `classify_resolved_host`; thread an injectable resolver through a new `detect_mcp_auth_inner`; add the domain resolve-gate; `system_resolve` production resolver |
| `crates/right-mcp/src/credentials.rs` | URL validation + literal classification | #3 consolidation: `is_private_or_link_local_ipv6` ULA check delegates to `ssrf::is_user_private_lan` |
| `crates/bot/src/telegram/dashboard/mcp.rs` | Dashboard detection/OAuth-start handlers | `detection_http_client()` → `PublicOnly` |
| `docs/architecture/mcp.md` | Descriptive MCP doc | Cite-on-touch: update detection-client description if drifted |

No new files. No public API removed. `detect_mcp_auth` / `detect_mcp_auth_with_url_policy` signatures are **unchanged** (the resolver is injected only into a private inner fn for tests).

---

## Task 1: Expose `is_loopback_addr` and add resolved-host classification

**Files:**
- Modify: `crates/right-mcp/src/ssrf.rs:91` (visibility only)
- Modify: `crates/right-mcp/src/detect.rs` (add enum + helper + test)

- [ ] **Step 1: Make `is_loopback_addr` crate-visible**

In `crates/right-mcp/src/ssrf.rs`, change the function at line 91 from private to `pub(crate)` (it is already used inside `ip_allowed`; `detect` now needs it too):

```rust
/// Loopback check that folds IPv4-mapped IPv6 (`::ffff:127.0.0.1`) so it matches
/// the bare IPv4 form. `Ipv6Addr::is_loopback` alone only catches `::1`.
pub(crate) fn is_loopback_addr(ip: IpAddr) -> bool {
```

- [ ] **Step 2: Write the failing classification test**

In `crates/right-mcp/src/detect.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn classify_resolved_host_maps_addresses() {
    use std::net::IpAddr;
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();

    // loopback takes precedence
    assert_eq!(classify_resolved_host(&[ip("127.0.0.1")]), ResolvedHostClass::Loopback);
    assert_eq!(classify_resolved_host(&[ip("::1")]), ResolvedHostClass::Loopback);
    // private-LAN families
    assert_eq!(classify_resolved_host(&[ip("10.0.0.5")]), ResolvedHostClass::PrivateLan);
    assert_eq!(classify_resolved_host(&[ip("100.85.147.49")]), ResolvedHostClass::PrivateLan);
    assert_eq!(classify_resolved_host(&[ip("fc00::1")]), ResolvedHostClass::PrivateLan);
    // public, empty, and unknown all fall through
    assert_eq!(classify_resolved_host(&[ip("8.8.8.8")]), ResolvedHostClass::PublicOrUnknown);
    assert_eq!(classify_resolved_host(&[]), ResolvedHostClass::PublicOrUnknown);
    // mixed: any loopback wins; else any private-LAN
    assert_eq!(
        classify_resolved_host(&[ip("8.8.8.8"), ip("127.0.0.1")]),
        ResolvedHostClass::Loopback
    );
    assert_eq!(
        classify_resolved_host(&[ip("8.8.8.8"), ip("10.0.0.5")]),
        ResolvedHostClass::PrivateLan
    );
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `devenv shell -- cargo test -p right-mcp classify_resolved_host`
Expected: FAIL to compile — `cannot find type ResolvedHostClass` / `function classify_resolved_host`.

- [ ] **Step 4: Implement the enum and helper**

In `crates/right-mcp/src/detect.rs`, add near the top of the file (after the imports, before `detect_mcp_auth`). First extend the imports:

```rust
use std::net::IpAddr;
```

Then add:

```rust
/// Privacy class of a resolved base-URL host. Drives the detection
/// recommendation so private/loopback hostnames never reach OAuth discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedHostClass {
    Loopback,
    PrivateLan,
    PublicOrUnknown,
}

/// Classify resolved addresses using the canonical `ssrf` predicates (the same
/// ones the connect tier enforces). Loopback wins over private-LAN; an empty
/// list (resolution failed) is `PublicOrUnknown` so the caller falls through to
/// discovery, which surfaces the real connect error.
fn classify_resolved_host(addrs: &[IpAddr]) -> ResolvedHostClass {
    if addrs.iter().copied().any(crate::ssrf::is_loopback_addr) {
        ResolvedHostClass::Loopback
    } else if addrs.iter().copied().any(crate::ssrf::is_user_private_lan) {
        ResolvedHostClass::PrivateLan
    } else {
        ResolvedHostClass::PublicOrUnknown
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `devenv shell -- cargo test -p right-mcp classify_resolved_host`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-mcp/src/ssrf.rs crates/right-mcp/src/detect.rs
git commit -m "feat(mcp): resolved-host classification helper for detection gate"
```

---

## Task 2: Thread an injectable resolver through `detect` and add the resolve-gate

**Files:**
- Modify: `crates/right-mcp/src/detect.rs:58-125` (restructure into an inner fn + add the gate)
- Test: `crates/right-mcp/src/detect.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing detect tests (stub resolver)**

In `crates/right-mcp/src/detect.rs` tests module, add three tests. They call the new `detect_mcp_auth_inner` with a stub resolver so the resolution is hermetic. The stub is a closure `Fn(String) -> impl Future<Output = Vec<IpAddr>>`.

```rust
#[tokio::test]
async fn detect_private_resolving_hostname_recommends_headers_without_probe() {
    let server = wiremock::MockServer::start().await;
    let host = "private-magicdns.test";
    let addr = *server.address();
    let client = client_resolving(host, addr);
    let url = format!("http://{host}:{}/mcp", addr.port());

    let resolve = |_h: String| async { vec!["10.0.0.5".parse::<std::net::IpAddr>().unwrap()] };
    let result = detect_mcp_auth_inner(&client, &url, |_| Ok(()), resolve)
        .await
        .expect("private hostname should classify without error");

    assert_eq!(result.recommended_mode, McpAuthMode::Headers);
    assert_eq!(result.reason, DetectionReason::PrivateNetworkNoOauth);
    assert!(!result.oauth_discovered);
    assert_eq!(request_count(&server).await, 0); // no discovery probe fired
}

#[tokio::test]
async fn detect_loopback_resolving_hostname_recommends_url_as_is() {
    let server = wiremock::MockServer::start().await;
    let host = "loopback-name.test";
    let addr = *server.address();
    let client = client_resolving(host, addr);
    let url = format!("http://{host}:{}/mcp", addr.port());

    let resolve = |_h: String| async { vec!["127.0.0.1".parse::<std::net::IpAddr>().unwrap()] };
    let result = detect_mcp_auth_inner(&client, &url, |_| Ok(()), resolve)
        .await
        .expect("loopback hostname should classify without error");

    assert_eq!(result.recommended_mode, McpAuthMode::UrlAsIs);
    assert_eq!(result.reason, DetectionReason::LoopbackOrPrivate);
    assert_eq!(request_count(&server).await, 0);
}

#[tokio::test]
async fn detect_public_resolving_hostname_runs_discovery() {
    use wiremock::matchers::method;
    use wiremock::{Mock, ResponseTemplate};

    let server = wiremock::MockServer::start().await;
    let host = "public-name.test";
    let addr = *server.address();
    let client = client_resolving(host, addr);
    let url = format!("http://{host}:{}/mcp", addr.port());
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let resolve = |_h: String| async { vec!["8.8.8.8".parse::<std::net::IpAddr>().unwrap()] };
    let result = detect_mcp_auth_inner(&client, &url, |_| Ok(()), resolve)
        .await
        .expect("public hostname should reach discovery");

    assert_eq!(result.recommended_mode, McpAuthMode::Headers);
    assert_eq!(result.reason, DetectionReason::NoOAuthMetadata);
    assert!(request_count(&server).await > 0); // discovery probed
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devenv shell -- cargo test -p right-mcp detect_private_resolving_hostname detect_loopback_resolving_hostname detect_public_resolving_hostname`
Expected: FAIL to compile — `cannot find function detect_mcp_auth_inner`.

- [ ] **Step 3: Restructure `detect` into a resolver-injectable inner fn + add the gate**

In `crates/right-mcp/src/detect.rs`, replace the bodies of `detect_mcp_auth` (lines ~58-63) and `detect_mcp_auth_with_url_policy` (lines ~66-125) with thin wrappers, and add `detect_mcp_auth_inner` + `system_resolve`. The literal short-circuits are preserved verbatim; the only new logic is the domain resolve-gate block.

```rust
/// Detect the safest MCP authentication mode for a server URL.
///
/// Security contract: the supplied `reqwest::Client` is used for probing
/// untrusted URLs. Callers must configure redirect handling, DNS resolution,
/// and private-address guards appropriate to their trust boundary before
/// passing the client here.
pub async fn detect_mcp_auth(
    client: &reqwest::Client,
    original_url: &str,
) -> Result<McpAuthDetection, OAuthError> {
    detect_mcp_auth_with_url_policy(client, original_url, |_| Ok(())).await
}

/// Detect MCP authentication mode, applying `url_policy` before every OAuth discovery fetch.
pub async fn detect_mcp_auth_with_url_policy<F>(
    client: &reqwest::Client,
    original_url: &str,
    url_policy: F,
) -> Result<McpAuthDetection, OAuthError>
where
    F: Fn(&str) -> Result<(), OAuthError>,
{
    detect_mcp_auth_inner(client, original_url, url_policy, system_resolve).await
}

/// Resolve a host to its addresses for the detection gate, mirroring the
/// resolver `PublicNetworkResolver` uses. A resolution failure is non-fatal:
/// it yields an empty list so the caller falls through to discovery, which
/// surfaces the real connect error with URL context (the connection itself is
/// always made through the hardened client, never here).
async fn system_resolve(host: String) -> Vec<IpAddr> {
    match tokio::net::lookup_host((host.as_str(), 0)).await {
        Ok(addrs) => addrs.map(|addr| addr.ip()).collect(),
        Err(error) => {
            tracing::debug!("MCP detect host resolution failed for {host}: {error}");
            Vec::new()
        }
    }
}

async fn detect_mcp_auth_inner<F, R, Fut>(
    client: &reqwest::Client,
    original_url: &str,
    url_policy: F,
    resolve_host: R,
) -> Result<McpAuthDetection, OAuthError>
where
    F: Fn(&str) -> Result<(), OAuthError>,
    R: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Vec<IpAddr>>,
{
    let parsed = reqwest::Url::parse(original_url).map_err(|_| invalid_server_url())?;
    validate_detection_url(&parsed)?;
    let has_query = parsed.query().is_some();
    let mut bare = parsed.clone();
    bare.set_query(None);
    let bare_url = bare.to_string();

    if has_query {
        return Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::UrlAsIs,
            reason: DetectionReason::QueryStringPresent,
            oauth: None,
        });
    }

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
        // Private / LAN / link-local base URL given as an IP literal: OAuth is not
        // supported for local servers. Recommend Headers and skip OAuth discovery.
        return Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::Headers,
            reason: DetectionReason::PrivateNetworkNoOauth,
            oauth: None,
        });
    }

    // Domain host: resolve before deciding. A hostname that resolves to a private
    // or loopback address (e.g. Tailscale MagicDNS) must short-circuit exactly like
    // the equivalent IP literal, so detection never probes private space and the
    // PublicOnly discovery client below only ever sees public targets.
    let domain_host = match bare.host() {
        Some(url::Host::Domain(host)) => Some(host.to_string()),
        _ => None,
    };
    if let Some(host) = domain_host {
        match classify_resolved_host(&resolve_host(host).await) {
            ResolvedHostClass::Loopback => {
                return Ok(McpAuthDetection {
                    bare_url,
                    oauth_discovered: false,
                    recommended_mode: McpAuthMode::UrlAsIs,
                    reason: DetectionReason::LoopbackOrPrivate,
                    oauth: None,
                });
            }
            ResolvedHostClass::PrivateLan => {
                return Ok(McpAuthDetection {
                    bare_url,
                    oauth_discovered: false,
                    recommended_mode: McpAuthMode::Headers,
                    reason: DetectionReason::PrivateNetworkNoOauth,
                    oauth: None,
                });
            }
            ResolvedHostClass::PublicOrUnknown => {}
        }
    }

    match discover_oauth_with_url_policy(client, &bare_url, url_policy).await {
        Ok(discovery) => Ok(oauth_detected(bare_url, discovery)),
        Err(error) if error.is_no_as_metadata() => Ok(McpAuthDetection {
            bare_url,
            oauth_discovered: false,
            recommended_mode: McpAuthMode::Headers,
            reason: DetectionReason::NoOAuthMetadata,
            oauth: None,
        }),
        Err(error) => Err(error),
    }
}
```

- [ ] **Step 4: Run the new tests + the full detect suite to verify pass + no regression**

Run: `devenv shell -- cargo test -p right-mcp detect_`
Expected: PASS — the three new tests plus every pre-existing `detect_*` test (the existing tests use unresolvable `*.test` hosts, so `system_resolve` returns empty → `PublicOrUnknown` → unchanged fall-through to discovery).

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/detect.rs
git commit -m "feat(mcp): resolve domain base URLs in detect; private/loopback hostnames skip OAuth discovery"
```

---

## Task 3: Make the dashboard detection probe client `PublicOnly`

**Files:**
- Modify: `crates/right-mcp/src/ssrf.rs` (`#[cfg(test)] mod tests`) — hermetic resolver regression
- Modify: `crates/bot/src/telegram/dashboard/mcp.rs:466-471` (`detection_http_client`)

- [ ] **Step 1: Write the failing resolver regression (locks the guarantee the flip relies on)**

In `crates/right-mcp/src/ssrf.rs` tests module, add a test proving the `PublicOnly` resolver strips a hostname that resolves to loopback while `AllowPrivate` admits it. (`localhost` is resolved by the OS to `127.0.0.1`/`::1` on dev and CI hosts.)

```rust
#[tokio::test]
async fn public_only_resolver_strips_private_resolving_hostname() {
    use reqwest::dns::Resolve as _;
    use std::str::FromStr as _;

    let public_only = PublicNetworkResolver::new(NetworkPolicy::PublicOnly);
    let stripped = public_only
        .resolve(reqwest::dns::Name::from_str("localhost").unwrap())
        .await;
    assert!(
        stripped.is_err(),
        "localhost resolves to loopback; PublicOnly must strip it and fail resolution"
    );

    let allow_private = PublicNetworkResolver::new(NetworkPolicy::AllowPrivate);
    let admitted = allow_private
        .resolve(reqwest::dns::Name::from_str("localhost").unwrap())
        .await;
    assert!(
        admitted.is_ok(),
        "AllowPrivate must admit a loopback-resolving hostname"
    );
}
```

- [ ] **Step 2: Run the test to verify current behavior**

Run: `devenv shell -- cargo test -p right-mcp public_only_resolver_strips_private_resolving_hostname`
Expected: PASS immediately — this characterizes the resolver guarantee that justifies the client flip in Step 3 (the resolver already filters by policy; this locks it against regression). If it fails, the host has no `localhost` resolution — investigate before proceeding.

- [ ] **Step 3: Flip the detection client to `PublicOnly`**

In `crates/bot/src/telegram/dashboard/mcp.rs`, change `detection_http_client` (line 466):

```rust
fn detection_http_client() -> Result<reqwest::Client, reqwest::Error> {
    // PublicOnly: detection probes untrusted URLs. Private/loopback BASE URLs are
    // short-circuited to Headers/UrlAsIs in `detect` before discovery, so the probe
    // never needs private reach; this ensures server-supplied discovered URLs
    // (incl. domains that rebind to a private IP) are stripped at the resolver.
    right_mcp::ssrf::hardened_client_builder(right_mcp::ssrf::NetworkPolicy::PublicOnly)
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(15))
        .build()
}
```

This serves both callers correctly: the detect handler (`mcp.rs:142`) and the OAuth-start discovery/registration flow (`mcp.rs:350`, `:369`) — both fetch probe or server-supplied URLs that must honor `PublicOnly`. An operator who manually picks OAuth for a private base now fails fast at discovery with a clear error (the design's "no OAuth for local servers" enforcement), and the detect handler already maps `PUBLIC_DNS_ERROR_MARKER` to a clean 400 (`mcp.rs:164-172`).

- [ ] **Step 4: Check for any test asserting the detection client tier, then build the bot**

Run: `rg -n 'detection_http_client|AllowPrivate' crates/bot/src/telegram/dashboard/mcp.rs`
If any test (e.g. around the resolver test at `mcp.rs:~558`) asserts the detection client uses `AllowPrivate`, update it to `PublicOnly`. Then:

Run: `devenv shell -- cargo test -p right-bot --no-run && devenv shell -- cargo test -p right-bot mcp`
Expected: compiles; MCP dashboard tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/ssrf.rs crates/bot/src/telegram/dashboard/mcp.rs
git commit -m "fix(mcp): detection probe client is PublicOnly (close discovered-URL rebind)"
```

---

## Task 4: Consolidate IPv6 private classification onto the `ssrf` predicate (#3)

**Files:**
- Modify: `crates/right-mcp/src/credentials.rs:267-273` (`is_private_or_link_local_ipv6`)
- Test: `crates/right-mcp/src/credentials.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the characterization test (behavior must not change)**

In `crates/right-mcp/src/credentials.rs` tests module, add:

```rust
#[test]
fn is_private_or_link_local_ipv6_matches_ula_and_link_local() {
    let v6 = |s: &str| s.parse::<std::net::Ipv6Addr>().unwrap();
    // ULA fc00::/7
    assert!(is_private_or_link_local_ipv6(v6("fc00::1")));
    assert!(is_private_or_link_local_ipv6(v6("fdff:ffff::1")));
    // link-local fe80::/10
    assert!(is_private_or_link_local_ipv6(v6("fe80::1")));
    // ipv4-mapped private folds through
    assert!(is_private_or_link_local_ipv6(v6("::ffff:10.0.0.1")));
    // public stays public
    assert!(!is_private_or_link_local_ipv6(v6("2001:4860:4860::8888")));
}
```

- [ ] **Step 2: Run the test to verify it passes against current code**

Run: `devenv shell -- cargo test -p right-mcp is_private_or_link_local_ipv6_matches`
Expected: PASS (locks current behavior before the refactor).

- [ ] **Step 3: Delegate the ULA check to `ssrf::is_user_private_lan`**

In `crates/right-mcp/src/credentials.rs`, replace `is_private_or_link_local_ipv6` (lines 267-273) so the ULA range comes from the single canonical predicate (the ipv4-mapped branch and link-local handling are unchanged):

```rust
fn is_private_or_link_local_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_or_link_local_ipv4(v4);
    }
    // ULA (fc00::/7) classification shares ssrf's canonical predicate; link-local
    // (fe80::/10) stays on the stdlib check.
    crate::ssrf::is_user_private_lan(IpAddr::V6(ip)) || ip.is_unicast_link_local()
}
```

- [ ] **Step 4: Run the test to verify it still passes**

Run: `devenv shell -- cargo test -p right-mcp is_private_or_link_local_ipv6_matches`
Expected: PASS (behavior identical; the range now has one source of truth).

- [ ] **Step 5: Commit**

```bash
git add crates/right-mcp/src/credentials.rs
git commit -m "refactor(mcp): IPv6 ULA classification delegates to ssrf::is_user_private_lan"
```

---

## Task 5: Cite-on-touch the architecture doc

**Files:**
- Modify (if drifted): `docs/architecture/mcp.md`

- [ ] **Step 1: Re-read the detection description**

Run: `rg -n -i 'detection|AllowPrivate|PublicOnly|probe' docs/architecture/mcp.md`
Read the surrounding paragraphs.

- [ ] **Step 2: Update if it describes the detection client tier**

If `docs/architecture/mcp.md` states the detection/probe client policy or the detection gate, update it to: detection probes under `PublicOnly`; `detect` resolves a domain base URL and short-circuits private/loopback hostnames to Headers/UrlAsIs before discovery; runtime proxy + registration stay `AllowPrivate`. If the doc does not mention this (detection client tier is an implementation detail), make no change — the parent spec's "server-supplied URLs stay PublicOnly" invariant now holds without edit. State explicitly in the commit which case applied.

- [ ] **Step 3: Commit (only if the doc changed)**

```bash
git add docs/architecture/mcp.md
git commit -m "docs(architecture): detection probes under PublicOnly with resolve-gate"
```

---

## Task 6: Final workspace verification

- [ ] **Step 1: Run the full workspace test suite (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS, zero failures. (If a known-flaky test trips — `cc/invocation` pid race or the dashboard warn-count test — re-run it isolated before attributing it to this change.)

- [ ] **Step 2: Confirm the branch is clean and ready to land**

Run: `git status --porcelain` (expect empty) and `git log --oneline master..HEAD`
Hand back for the fast-forward land to `origin/master`.

---

## Self-Review

**Spec coverage:**
- Design §1 (resolve base host, classify, branch) → Tasks 1 + 2. Loopback→UrlAsIs, private→Headers, public→discovery mapping and the mixed/empty/failure edge rules are in `classify_resolved_host` + `system_resolve` + their tests. ✓
- Design §2 (detection client → PublicOnly, both callers) → Task 3. ✓
- Design §3 / "deliberately unchanged" (runtime proxy, validate, add-path stay AllowPrivate) → no task touches them; explicitly out of scope. ✓
- #3 consolidation (IPv6 ULA → ssrf predicate) → Task 4. ✓
- Testability (injectable resolver) → Task 2 `detect_mcp_auth_inner` + stub-resolver tests. ✓
- Threat-model delta row "discovered domain → private IP: blocked at resolver" → Task 3 + the ssrf resolver regression (Task 3 Step 1). ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code; every test step shows the assertion and the exact `cargo` command + expected result. ✓

**Type consistency:** `ResolvedHostClass` {`Loopback`,`PrivateLan`,`PublicOrUnknown`}, `classify_resolved_host(&[IpAddr]) -> ResolvedHostClass`, and `detect_mcp_auth_inner<F, R, Fut>(client, original_url, url_policy, resolve_host)` are used identically in Tasks 1, 2, and their tests. `system_resolve(String) -> Vec<IpAddr>` matches the `R: Fn(String) -> Fut` bound. `ssrf::is_loopback_addr` made `pub(crate)` in Task 1 before use. ✓
