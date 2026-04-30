# OAuth Discovery Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `right_agent::mcp::oauth::discover_as` correctly detect OAuth on real-world MCP servers (Linear, and any server that follows the MCP Authorization spec), instead of falling through to bearer-token detection.

**Architecture:** Single-file change in `crates/right-agent/src/mcp/oauth.rs`. Three fixes layered on top of the existing 3-step discovery chain:

1. **Tolerant probe loop** — any non-2xx response (or connection error) on a speculative well-known URL is logged and skipped, instead of aborting the chain. Only "no probe succeeded" returns `DiscoveryFailed`.
2. **Origin-level fallback for path-bearing MCP URLs** — `as_metadata_urls` already enumerates path-aware variants (RFC 8414 §3.1) for `https://host/mcp`; we extend it to also include the origin-only well-known URLs as the final fallback, since many MCP servers (Linear, …) serve AS metadata at the origin.
3. **`WWW-Authenticate` probe (MCP spec / RFC 9728 §5.1)** — as the new Step 0 of `discover_as`, send an unauthenticated GET to the MCP server URL and parse `WWW-Authenticate: Bearer resource_metadata="<url>"` from a 401 response. If present, fetch RFC 9728 metadata directly from that URL.

Tests are wiremock-driven for new code (already a dev-dependency, used elsewhere in the workspace) and pin Linear's exact response pattern as a regression test.

**Tech Stack:** Rust 2024, `reqwest`, `wiremock = "0.6"`, `tokio::test`, `thiserror`.

---

## File Map

- **Modify:** `crates/right-agent/src/mcp/oauth.rs`
  - `as_metadata_urls` (lines 150–178) — extend path-bearing branch with origin-only fallback URLs
  - `discover_as` (lines 188–291) — tolerate non-2xx in both probe steps; add Step 0 (WWW-Authenticate probe)
  - new helper `parse_www_authenticate_resource_metadata` (private fn)
  - new helper `probe_resource_metadata_via_www_authenticate` (private async fn)
  - existing test `discover_as_5xx_aborts_immediately` (lines 659–682) — semantics change; rename and rewrite
  - new tests appended in the existing `mod tests` block at the bottom of the file

No other files change. No new modules, no new crates, no public API changes.

---

## Background (for the implementing agent)

The bug observed in production (`~/.right/logs/test.log.2026-04-30`):

```
mcp add: starting OAuth AS discovery url=https://mcp.linear.app/mcp
mcp add: OAuth AS discovery complete url=https://mcp.linear.app/mcp
  oauth_discovered=false
  err=Some(DiscoveryFailed("AS metadata server error 401 Unauthorized
       at https://mcp.linear.app/mcp/.well-known/openid-configuration"))
```

What happened:
1. RFC 9728 probe at `https://mcp.linear.app/.well-known/oauth-protected-resource/mcp` → 404 → fell through (`as_url = None`).
2. `as_metadata_urls(server_url)` returned, in order:
   - `https://mcp.linear.app/.well-known/oauth-authorization-server/mcp` — 404 (continued)
   - `https://mcp.linear.app/.well-known/openid-configuration/mcp` — 404 (continued)
   - `https://mcp.linear.app/mcp/.well-known/openid-configuration` — **401** → `DiscoveryFailed` returned, loop aborted.
3. Linear's actual AS metadata, served at `https://mcp.linear.app/.well-known/oauth-authorization-server` (origin-only, no path suffix), was **never probed** because the path-bearing branch of `as_metadata_urls` does not include origin-only URLs.
4. Linear ALSO advertises RFC 9728 metadata via `WWW-Authenticate: Bearer resource_metadata="…"` on a 401 from `/mcp` itself, but `discover_as` never sends an unauthenticated request to the MCP URL to read that header.

After the failure, the `/mcp add` handler fell through to a 5-turn Haiku heuristic that incorrectly classified Linear as `bearer`. That fallback path itself is fine; the proximate bug is in `discover_as`.

---

## Task 1: Regression test pinning Linear's exact pattern

**Files:**
- Modify: `crates/right-agent/src/mcp/oauth.rs` (append to `mod tests`)

This test is what would have caught the production bug. Without any of the three fixes it MUST fail.

The mock server replays Linear's exact behavior for each well-known URL the chain probes, plus serves valid AS metadata at the origin-only RFC 8414 location. After Tasks 2+3 it passes.

- [ ] **Step 1: Append the regression test to `mod tests` in `oauth.rs`**

```rust
    #[tokio::test]
    async fn discover_as_linear_pattern_uses_origin_well_known() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // Path-suffixed RFC 9728 → 404 (Linear does not host metadata here)
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        // Path-suffixed RFC 8414 → 404
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        // Path-suffixed OIDC → 404
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration/mcp"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        // Resource-path-prefixed OIDC → 401 (this is the URL that aborted
        // the loop in production; mock Linear's actual response).
        Mock::given(method("GET"))
            .and(path("/mcp/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        // ORIGIN-ONLY RFC 8414 → 200 with valid AS metadata. Linear hosts
        // metadata here. Current code never probes this URL when the MCP
        // server URL has a non-empty path; Task 2 makes it probe this URL,
        // and Task 3 makes the chain reach this URL despite the 401 above.
        let as_meta = serde_json::json!({
            "authorization_endpoint": "https://auth.linear.example/authorize",
            "token_endpoint":         "https://auth.linear.example/token"
        });
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(as_meta))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let server_url = format!("{}/mcp", server.uri());
        let result = discover_as(&client, &server_url).await;

        let meta = result.expect("discover_as must succeed for Linear-style server");
        assert_eq!(meta.authorization_endpoint, "https://auth.linear.example/authorize");
        assert_eq!(meta.token_endpoint, "https://auth.linear.example/token");
    }
```

- [ ] **Step 2: Verify the test fails on master**

Run: `cargo test -p right-agent discover_as_linear_pattern -- --nocapture`

Expected: FAIL. The error string is the same shape as the production log line: `DiscoveryFailed("AS metadata server error 401 Unauthorized at .../mcp/.well-known/openid-configuration")`. Capture the failure output — it confirms the bug is reproduced.

- [ ] **Step 3: Commit the failing test**

```bash
git add crates/right-agent/src/mcp/oauth.rs
git commit -m "test(oauth): regression for Linear-pattern AS discovery

Pins the exact response pattern observed from mcp.linear.app:
- path-suffixed well-known URLs all 404
- /mcp/.well-known/openid-configuration returns 401
- AS metadata is at the origin-only RFC 8414 URL

Currently fails because (a) the probe loop aborts on 401 and
(b) origin-only URLs are not probed when the MCP URL has a path."
```

---

## Task 2: Fix #2 — origin-only fallback in `as_metadata_urls`

**Files:**
- Modify: `crates/right-agent/src/mcp/oauth.rs:150-178`

The path-bearing branch must also try the origin-only well-known URLs after the path-aware variants. The path-aware variants stay first (RFC 8414 §3.1 mandates them); origin-only comes after as a real-world fallback.

- [ ] **Step 1: Write a unit test for the new URL ordering**

Append to `mod tests`:

```rust
    #[test]
    fn as_metadata_urls_path_bearing_includes_origin_only_fallback() {
        let urls = as_metadata_urls("https://mcp.linear.app/mcp");
        // Path-aware variants come first (RFC 8414 §3.1).
        assert_eq!(
            urls[0],
            "https://mcp.linear.app/.well-known/oauth-authorization-server/mcp"
        );
        assert_eq!(
            urls[1],
            "https://mcp.linear.app/.well-known/openid-configuration/mcp"
        );
        assert_eq!(
            urls[2],
            "https://mcp.linear.app/mcp/.well-known/openid-configuration"
        );
        // Origin-only fallbacks come after (real-world: Linear, …).
        assert!(
            urls.contains(
                &"https://mcp.linear.app/.well-known/oauth-authorization-server".to_string()
            ),
            "origin-only RFC 8414 URL must be in fallback list, got {urls:?}"
        );
        assert!(
            urls.contains(
                &"https://mcp.linear.app/.well-known/openid-configuration".to_string()
            ),
            "origin-only OIDC URL must be in fallback list, got {urls:?}"
        );
    }

    #[test]
    fn as_metadata_urls_no_path_unchanged() {
        // Origin-only AS URL should still produce the same two URLs as before.
        let urls = as_metadata_urls("https://auth.example.com");
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://auth.example.com/.well-known/oauth-authorization-server");
        assert_eq!(urls[1], "https://auth.example.com/.well-known/openid-configuration");
    }
```

- [ ] **Step 2: Run the new tests; verify they fail**

Run: `cargo test -p right-agent as_metadata_urls -- --nocapture`

Expected: `as_metadata_urls_path_bearing_includes_origin_only_fallback` FAILS (the origin-only URLs are not present). `as_metadata_urls_no_path_unchanged` PASSES.

- [ ] **Step 3: Implement the fix**

Replace the body of `as_metadata_urls` in `crates/right-agent/src/mcp/oauth.rs` (currently lines 150–178) with:

```rust
fn as_metadata_urls(as_url: &str) -> Vec<String> {
    let parsed = match reqwest::Url::parse(as_url) {
        Ok(u) => u,
        Err(_) => return vec![],
    };
    let origin = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""));
    let port_str = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    let origin = format!("{origin}{port_str}");

    let raw_path = parsed.path();
    let path = raw_path.trim_end_matches('/');

    let mut urls = Vec::new();

    if path.is_empty() || path == "/" {
        // No-path AS URL: try RFC 8414, then OIDC.
        urls.push(format!("{origin}/.well-known/oauth-authorization-server"));
        urls.push(format!("{origin}/.well-known/openid-configuration"));
    } else {
        // Path-bearing AS URL: try RFC 8414 §3.1 path-aware variants first…
        urls.push(format!(
            "{origin}/.well-known/oauth-authorization-server{path}"
        ));
        urls.push(format!("{origin}/.well-known/openid-configuration{path}"));
        urls.push(format!("{origin}{path}/.well-known/openid-configuration"));
        // …then origin-only fallbacks. Many real MCP servers (Linear, …) host
        // metadata at the origin-only well-known location even when the
        // server URL itself has a path component.
        urls.push(format!("{origin}/.well-known/oauth-authorization-server"));
        urls.push(format!("{origin}/.well-known/openid-configuration"));
    }

    urls
}
```

- [ ] **Step 4: Run unit tests; verify they pass**

Run: `cargo test -p right-agent as_metadata_urls -- --nocapture`

Expected: both `as_metadata_urls_*` tests PASS.

- [ ] **Step 5: Run the regression test from Task 1; verify it still fails**

Run: `cargo test -p right-agent discover_as_linear_pattern -- --nocapture`

Expected: still FAILS, but with a different error — the loop now reaches the origin-only URL after first hitting the 401, and aborts there. After Task 3 it will pass.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/mcp/oauth.rs
git commit -m "fix(oauth): try origin-only well-known URLs for path-bearing MCP

RFC 8414 §3.1 mandates path-aware well-known URLs for path-bearing
issuers, but real MCP servers (Linear, …) host AS metadata at the
origin-only location. Probe both, with path-aware variants first."
```

---

## Task 3: Fix #1 — tolerate non-2xx in the discovery chain

**Files:**
- Modify: `crates/right-agent/src/mcp/oauth.rs:188-291`
- Modify: existing test `discover_as_5xx_aborts_immediately` (lines 659–682) — semantics change

The chain currently treats only 404 as "skip". Any other non-success (401, 403, 500, …) aborts. That is wrong for speculative probes against synthesized URLs: a 401 from an MCP path that is not actually a well-known URL is not "discovery failed", it's "the server doesn't host metadata here". Reqwest connection errors (connection refused, DNS failure on a per-URL basis — rare for the same host but possible with redirects) should also just skip.

Only "every URL in the chain failed" is `DiscoveryFailed`.

- [ ] **Step 1: Rewrite the existing 5xx test to match the new contract**

Replace the existing `discover_as_5xx_aborts_immediately` test (lines 659–682) with:

```rust
    #[tokio::test]
    async fn discover_as_5xx_skips_and_continues() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // RFC 9728 path-suffixed → 500 (must NOT abort the chain).
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        // Origin-only AS metadata → 200 (must be reached).
        let as_meta = serde_json::json!({
            "authorization_endpoint": "https://auth.example.com/authorize",
            "token_endpoint":         "https://auth.example.com/token"
        });
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(as_meta))
            .mount(&server)
            .await;

        // Anything else 404s implicitly via wiremock's default.

        let client = reqwest::Client::new();
        let server_url = format!("{}/mcp", server.uri());
        let result = discover_as(&client, &server_url).await;

        let meta = result.expect("5xx on a speculative probe must not abort the chain");
        assert_eq!(meta.token_endpoint, "https://auth.example.com/token");
    }

    #[tokio::test]
    async fn discover_as_all_non_2xx_returns_discovery_failed() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Every probe gets 401 — chain has no successful URL.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let server_url = format!("{}/mcp", server.uri());
        let result = discover_as(&client, &server_url).await;

        assert!(
            matches!(result, Err(OAuthError::DiscoveryFailed(_))),
            "all-401 chain must end in DiscoveryFailed: {result:?}"
        );
    }
```

- [ ] **Step 2: Run the rewritten tests; verify they fail (or one passes)**

Run: `cargo test -p right-agent 'discover_as_5xx_skips_and_continues|discover_as_all_non_2xx_returns_discovery_failed' -- --nocapture`

Expected: `discover_as_5xx_skips_and_continues` FAILS (current code aborts on 5xx). `discover_as_all_non_2xx_returns_discovery_failed` may PASS already (current code returns `DiscoveryFailed` on the first 401 too, just earlier).

- [ ] **Step 3: Implement the fix in `discover_as`**

Replace `discover_as` in `crates/right-agent/src/mcp/oauth.rs` (currently lines 188–291) with:

```rust
pub async fn discover_as(
    client: &reqwest::Client,
    server_url: &str,
) -> Result<AsMetadata, OAuthError> {
    let parsed = reqwest::Url::parse(server_url).map_err(|e| {
        OAuthError::DiscoveryFailed(format!("invalid server URL {server_url}: {e}"))
    })?;
    let origin = {
        let scheme = parsed.scheme();
        let host = parsed.host_str().unwrap_or("");
        let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
        format!("{scheme}://{host}{port}")
    };

    let raw_path = parsed.path();
    let path = raw_path.trim_end_matches('/');

    // --- Step 1: RFC 9728 resource metadata ---
    let rfc9728_url = if path.is_empty() || path == "/" {
        format!("{origin}/.well-known/oauth-protected-resource")
    } else {
        format!("{origin}/.well-known/oauth-protected-resource{path}")
    };

    debug!("discover_as: trying RFC 9728 resource metadata at {rfc9728_url}");
    let as_url: Option<String> = match client.get(&rfc9728_url).send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                let meta: ResourceMetadata = resp.json().await.map_err(|e| {
                    OAuthError::DiscoveryFailed(format!(
                        "failed to parse RFC 9728 response from {rfc9728_url}: {e}"
                    ))
                })?;
                debug!(
                    "discover_as: RFC 9728 succeeded, AS URL = {:?}",
                    meta.authorization_servers.first()
                );
                Some(
                    meta.authorization_servers
                        .into_iter()
                        .next()
                        .ok_or_else(|| {
                            OAuthError::DiscoveryFailed(
                                "RFC 9728 authorization_servers array is empty".to_string(),
                            )
                        })?,
                )
            } else {
                // Any non-2xx (404, 401, 5xx, …) → speculative URL didn't
                // hit metadata. Skip and try the AS metadata fallback chain.
                debug!(
                    "discover_as: RFC 9728 returned {status}, falling back to AS metadata"
                );
                None
            }
        }
        Err(e) => {
            // Connection error on a speculative URL → skip, try fallback.
            debug!("discover_as: RFC 9728 request failed for {rfc9728_url}: {e}, falling back");
            None
        }
    };

    // --- Steps 2 & 3: AS metadata + OIDC ---
    let as_base = as_url.as_deref().unwrap_or(server_url);
    let as_meta_urls = as_metadata_urls(as_base);

    let mut last_err: Option<String> = None;
    for url in &as_meta_urls {
        debug!("discover_as: trying AS metadata at {url}");
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    let meta: AsMetadata = resp.json().await.map_err(|e| {
                        OAuthError::DiscoveryFailed(format!(
                            "failed to parse AS metadata from {url}: {e}"
                        ))
                    })?;
                    debug!("discover_as: succeeded via {url}");
                    return Ok(meta);
                }
                debug!("discover_as: {url} returned {status}, trying next");
                last_err = Some(format!("{status} at {url}"));
            }
            Err(e) => {
                debug!("discover_as: AS metadata request failed for {url}: {e}, trying next");
                last_err = Some(format!("request failed for {url}: {e}"));
            }
        }
    }

    Err(OAuthError::DiscoveryFailed(format!(
        "no AS metadata found for {server_url} (last probe: {})",
        last_err.unwrap_or_else(|| "no probes executed".to_string())
    )))
}
```

- [ ] **Step 4: Run all `discover_as_*` tests; verify they pass**

Run: `cargo test -p right-agent discover_as -- --nocapture`

Expected: ALL pass, including:
- `discover_as_linear_pattern_uses_origin_well_known` (Task 1 regression)
- `discover_as_5xx_skips_and_continues`
- `discover_as_all_non_2xx_returns_discovery_failed`
- existing `discover_as_rfc9728_success_returns_as_metadata`
- existing `discover_as_all_404_returns_discovery_failed`

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/mcp/oauth.rs
git commit -m "fix(oauth): skip speculative probes on any non-2xx, not just 404

The discovery chain probes a list of synthesized well-known URLs.
A 401/403/5xx from one of those URLs means 'this URL does not host
metadata' — the same as 404 — not 'discovery failed'. Only when
every URL in the chain fails do we return DiscoveryFailed.

Closes the production bug where mcp.linear.app/mcp returned 401 on
a speculative OIDC URL and aborted the chain before the real
metadata at the origin-only RFC 8414 URL was probed."
```

---

## Task 4: WWW-Authenticate header parser (helper + unit tests)

**Files:**
- Modify: `crates/right-agent/src/mcp/oauth.rs` (add private helper near `discover_as`)

Per RFC 9728 §5.1 / MCP Authorization spec, an unauthenticated request to a protected MCP resource returns 401 with:

```
WWW-Authenticate: Bearer realm="...", resource_metadata="https://host/.well-known/oauth-protected-resource"
```

We parse the `resource_metadata` parameter only. Quoted (RFC-compliant) and unquoted forms must both work; everything else returns `None`.

- [ ] **Step 1: Add the failing unit tests**

Append to `mod tests`:

```rust
    #[test]
    fn parse_www_authenticate_quoted_resource_metadata() {
        let h = r#"Bearer realm="Linear", resource_metadata="https://mcp.linear.app/.well-known/oauth-protected-resource""#;
        assert_eq!(
            parse_www_authenticate_resource_metadata(h),
            Some("https://mcp.linear.app/.well-known/oauth-protected-resource".to_string())
        );
    }

    #[test]
    fn parse_www_authenticate_unquoted_resource_metadata() {
        let h = "Bearer resource_metadata=https://api.example.com/.well-known/x";
        assert_eq!(
            parse_www_authenticate_resource_metadata(h),
            Some("https://api.example.com/.well-known/x".to_string())
        );
    }

    #[test]
    fn parse_www_authenticate_unquoted_with_trailing_param() {
        let h = "Bearer resource_metadata=https://api.example.com/x, error=invalid_token";
        assert_eq!(
            parse_www_authenticate_resource_metadata(h),
            Some("https://api.example.com/x".to_string())
        );
    }

    #[test]
    fn parse_www_authenticate_no_resource_metadata_returns_none() {
        assert_eq!(
            parse_www_authenticate_resource_metadata(r#"Bearer realm="x""#),
            None
        );
        assert_eq!(parse_www_authenticate_resource_metadata("Basic realm=x"), None);
        assert_eq!(parse_www_authenticate_resource_metadata(""), None);
    }
```

- [ ] **Step 2: Run; verify compile failure (function does not exist)**

Run: `cargo test -p right-agent parse_www_authenticate -- --nocapture`

Expected: compile error — `parse_www_authenticate_resource_metadata` is undefined.

- [ ] **Step 3: Implement the parser**

Add this private function to `crates/right-agent/src/mcp/oauth.rs`, immediately above `pub async fn discover_as`:

```rust
/// Extract `resource_metadata="<url>"` (RFC 9728 §5.1) from a `WWW-Authenticate`
/// header value. Returns `None` if the parameter is absent or malformed.
///
/// Supports both the quoted RFC-compliant form (`resource_metadata="https://…"`)
/// and the unquoted form some servers emit. We deliberately do not parse the
/// full Bearer challenge grammar (RFC 6750 §3) — only the one parameter we need.
fn parse_www_authenticate_resource_metadata(header: &str) -> Option<String> {
    let needle = "resource_metadata=";
    let start = header.find(needle)? + needle.len();
    let rest = &header[start..];
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        let end = rest.find(',').unwrap_or(rest.len());
        let value = rest[..end].trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    }
}
```

- [ ] **Step 4: Run; verify tests pass**

Run: `cargo test -p right-agent parse_www_authenticate -- --nocapture`

Expected: all four `parse_www_authenticate_*` tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/mcp/oauth.rs
git commit -m "feat(oauth): parse resource_metadata from WWW-Authenticate header

Helper for the upcoming WWW-Authenticate-driven discovery probe
(RFC 9728 §5.1, MCP Authorization spec). Handles both quoted and
unquoted parameter forms; intentionally narrow — only extracts
resource_metadata, not the whole challenge grammar."
```

---

## Task 5: Fix #3 — WWW-Authenticate probe as Step 0 of `discover_as`

**Files:**
- Modify: `crates/right-agent/src/mcp/oauth.rs` (add helper + insert Step 0 in `discover_as`)

The MCP Authorization spec defines this as the canonical discovery path. We do an unauthenticated `GET` to the MCP server URL itself; on a 401 with a parseable `resource_metadata` URL, we fetch RFC 9728 metadata directly from there. On anything else, we fall through to the existing well-known chain (now hardened by Tasks 2+3).

- [ ] **Step 1: Add the failing integration test**

Append to `mod tests`:

```rust
    #[tokio::test]
    async fn discover_as_uses_www_authenticate_resource_metadata() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let metadata_path = "/.well-known/oauth-protected-resource";

        // Step 0: unauthenticated GET /mcp returns 401 with WWW-Authenticate
        // pointing to the metadata URL on this same host.
        let www_authenticate = format!(
            r#"Bearer realm="MCP", resource_metadata="{}{}""#,
            server.uri(),
            metadata_path
        );
        Mock::given(method("GET"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header("WWW-Authenticate", www_authenticate.as_str()),
            )
            .mount(&server)
            .await;

        // Step 1: RFC 9728 resource metadata at the WWW-Authenticate-supplied URL
        // points at the AS URL, also on this same host.
        let resource_meta = serde_json::json!({
            "authorization_servers": [server.uri()]
        });
        Mock::given(method("GET"))
            .and(path(metadata_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(resource_meta))
            .mount(&server)
            .await;

        // Step 2: AS metadata at the origin-only RFC 8414 URL of the AS host.
        let as_meta = serde_json::json!({
            "authorization_endpoint": "https://auth.example.com/authorize",
            "token_endpoint":         "https://auth.example.com/token"
        });
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(as_meta))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let server_url = format!("{}/mcp", server.uri());
        let result = discover_as(&client, &server_url).await;

        let meta = result.expect("WWW-Authenticate-driven discovery must succeed");
        assert_eq!(meta.authorization_endpoint, "https://auth.example.com/authorize");
        assert_eq!(meta.token_endpoint, "https://auth.example.com/token");
    }
```

- [ ] **Step 2: Run; verify it fails**

Run: `cargo test -p right-agent discover_as_uses_www_authenticate -- --nocapture`

Expected: FAIL — `discover_as` does not currently send the unauthenticated probe; the 401 from `GET /mcp` plus the 200 at `/.well-known/oauth-protected-resource` (origin-only, no path suffix) is not enough for the existing chain to find AS metadata. Without WWW-Authenticate handling, the path-suffixed RFC 9728 URL `…/.well-known/oauth-protected-resource/mcp` returns 404 (default wiremock fallback) and the chain misses the real metadata.

- [ ] **Step 3: Implement the helper**

Add immediately above `pub async fn discover_as` (and below `parse_www_authenticate_resource_metadata`):

```rust
/// Step 0 of MCP Authorization-spec discovery: probe the MCP server URL
/// unauthenticated and read RFC 9728 metadata location from the
/// `WWW-Authenticate: Bearer resource_metadata="<url>"` challenge.
///
/// Returns `Some(url)` if the server returned a 401 with a parseable
/// `resource_metadata` parameter. Any other outcome (success, non-401
/// error, missing/malformed header, network error) yields `None` so
/// the caller can fall through to the well-known chain.
async fn probe_resource_metadata_via_www_authenticate(
    client: &reqwest::Client,
    server_url: &str,
) -> Option<String> {
    let resp = match client.get(server_url).send().await {
        Ok(r) => r,
        Err(e) => {
            debug!("WWW-Authenticate probe: request failed for {server_url}: {e}");
            return None;
        }
    };
    if resp.status().as_u16() != 401 {
        debug!(
            "WWW-Authenticate probe: {server_url} returned {} (not 401), skipping",
            resp.status()
        );
        return None;
    }
    let header = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)?
        .to_str()
        .ok()?;
    let url = parse_www_authenticate_resource_metadata(header)?;
    debug!("WWW-Authenticate probe: resource_metadata = {url}");
    Some(url)
}
```

- [ ] **Step 4: Wire Step 0 into `discover_as`**

In `crates/right-agent/src/mcp/oauth.rs`, modify the body of `discover_as`. Find the comment `// --- Step 1: RFC 9728 resource metadata ---` and replace the existing Step 1 block (the `let rfc9728_url = …` through the closing `};` of the `let as_url: Option<String> = match …`) with this version that prepends Step 0:

```rust
    // --- Step 0: WWW-Authenticate probe (MCP Authorization spec / RFC 9728 §5.1) ---
    // Send unauthenticated request to the MCP URL; the server should respond
    // 401 with a `WWW-Authenticate: Bearer resource_metadata="<url>"` header
    // pointing at its RFC 9728 metadata. This is the canonical mechanism;
    // well-known guesses below are best-effort fallbacks for servers that
    // don't follow the spec.
    let www_authenticate_url =
        probe_resource_metadata_via_www_authenticate(client, server_url).await;

    // --- Step 1: RFC 9728 resource metadata ---
    // Use the URL from Step 0 if we have it; otherwise synthesize the
    // path-aware/origin well-known URL.
    let rfc9728_url = if let Some(u) = www_authenticate_url {
        u
    } else if path.is_empty() || path == "/" {
        format!("{origin}/.well-known/oauth-protected-resource")
    } else {
        format!("{origin}/.well-known/oauth-protected-resource{path}")
    };

    debug!("discover_as: trying RFC 9728 resource metadata at {rfc9728_url}");
    let as_url: Option<String> = match client.get(&rfc9728_url).send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                let meta: ResourceMetadata = resp.json().await.map_err(|e| {
                    OAuthError::DiscoveryFailed(format!(
                        "failed to parse RFC 9728 response from {rfc9728_url}: {e}"
                    ))
                })?;
                debug!(
                    "discover_as: RFC 9728 succeeded, AS URL = {:?}",
                    meta.authorization_servers.first()
                );
                Some(
                    meta.authorization_servers
                        .into_iter()
                        .next()
                        .ok_or_else(|| {
                            OAuthError::DiscoveryFailed(
                                "RFC 9728 authorization_servers array is empty".to_string(),
                            )
                        })?,
                )
            } else {
                debug!(
                    "discover_as: RFC 9728 returned {status}, falling back to AS metadata"
                );
                None
            }
        }
        Err(e) => {
            debug!("discover_as: RFC 9728 request failed for {rfc9728_url}: {e}, falling back");
            None
        }
    };
```

- [ ] **Step 5: Run; verify all `discover_as_*` tests pass**

Run: `cargo test -p right-agent discover_as -- --nocapture`

Expected: ALL pass — the new `discover_as_uses_www_authenticate_resource_metadata` plus everything from previous tasks.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/mcp/oauth.rs
git commit -m "feat(oauth): probe WWW-Authenticate for resource_metadata URL

Implements the MCP Authorization spec's canonical discovery path:
unauthenticated request to the MCP URL, parse 401 response's
WWW-Authenticate header for resource_metadata=<url>, fetch RFC 9728
metadata from there. Existing well-known fallback chain stays as
backup for non-spec-compliant servers."
```

---

## Task 6: Build, lint, and full-suite verification

**Files:** none

- [ ] **Step 1: Full clippy on the workspace**

Run: `cargo clippy --workspace --all-targets -- -D warnings`

Expected: zero warnings. If anything in the new test code trips clippy, fix it inline (e.g., `clippy::needless_borrow`, `clippy::useless_format` are common in test bodies).

- [ ] **Step 2: Full debug build**

Run: `cargo build --workspace`

Expected: green.

- [ ] **Step 3: Run the full right-agent test suite**

Run: `cargo test -p right-agent`

Expected: green. Pay attention to anything in `mcp::` or `oauth::` — those are the modules you touched.

- [ ] **Step 4: Run the full workspace test suite (sanity)**

Run: `cargo test --workspace`

Expected: green. The handler-side `/mcp add` flow in `crates/bot` calls `discover_as` directly — its tests should still pass without any changes, because the public signature of `discover_as` is unchanged.

- [ ] **Step 5: Manual verification against the live Linear endpoint (optional but recommended)**

Run on a development machine with internet access:

```bash
cargo test -p right-agent discover_as_linear_pattern -- --nocapture
```

If you can spare 5 minutes, also run `right` in dev mode and execute `/mcp add linear https://mcp.linear.app/mcp` against a test agent. The expected reply should be `Browser flow: <auth_url>` (the OAuth authorization URL), not `Send the token for linear`.

- [ ] **Step 6: Final commit (only if Step 1 fixed any clippy warnings)**

If clippy required fixes:

```bash
git add crates/right-agent/src/mcp/oauth.rs
git commit -m "chore(oauth): satisfy clippy on new tests"
```

Otherwise skip — there's nothing to commit.

---

## Self-review notes (already addressed)

- **Spec coverage:** Tasks 2/3/5 each map 1-to-1 to the three bugs identified. Task 1 is the regression test; Task 4 is the helper that Task 5 depends on; Task 6 is verification.
- **Type consistency:** `parse_www_authenticate_resource_metadata` and `probe_resource_metadata_via_www_authenticate` are referenced consistently across Tasks 4 and 5. `as_metadata_urls` keeps its existing signature. `discover_as` keeps its public signature; only the body changes.
- **No placeholders:** every code step shows the full code to paste; every `Run:` step shows the exact command and expected outcome.
- **Test isolation:** the wiremock-based tests bind to ephemeral ports and run in parallel safely. The existing `discover_as_*` tests that use raw `TcpListener` are left intact (they test legitimately distinct scenarios — RFC 9728 happy path, all-404 chain — and translating them to wiremock is out of scope).
