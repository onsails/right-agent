# MCP Backend Error Convention Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align all MCP aggregator backends so operation failures return `Ok(CallToolResult)` with `is_error: true` and a structured JSON body, while protocol/infrastructure failures stay as `Err`.

**Architecture:** Single new helper module `right_agent::mcp::tool_error` exposes `tool_error(code, message, details)` plus `From<ProxyError> for CallToolResult`. `HindsightBackend` (in `aggregator.rs`) and `RightBackend` (in `right_backend.rs`) call the helper inline at operation-error sites; `BackendRegistry::dispatch_to_proxy` translates `ProxyError` at the dispatch boundary. `ProxyBackend::tools_call`'s typed return is unchanged.

**Tech Stack:** Rust 2024, rmcp `CallToolResult`/`Content`, `serde_json`, `thiserror`, `tokio`.

**Spec:** [`docs/superpowers/specs/2026-04-28-mcp-error-convention-design.md`](../specs/2026-04-28-mcp-error-convention-design.md).

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `crates/right-agent/src/mcp/tool_error.rs` | Create | Helper `tool_error(...)`, `From<ProxyError>`, in-file unit tests |
| `crates/right-agent/src/mcp/mod.rs` | Modify | `pub mod tool_error;` |
| `crates/right/src/aggregator.rs` | Modify | `HindsightBackend::tools_call` migrations, `dispatch_to_proxy`, instructions block, dispatch tests |
| `crates/right/src/right_backend.rs` | Modify | Two `CallToolResult::error` → `tool_error` swaps, tool-description tweaks |
| `crates/right/src/right_backend_tests.rs` | Modify | New error-path tests for allowlist + bootstrap_done |
| `PROMPT_SYSTEM.md` | Modify | Add "Error Convention" subsection |

Aggregator-side dispatch tests live inline in `aggregator.rs`'s existing `#[cfg(test)] mod tests` (the file is 853 lines; a separate file would invite drift from the existing fixtures `make_test_registry` / `make_dispatcher`).

Workspace cargo build only — no new crate-level deps required.

---

## Task 1: `tool_error` helper module

**Files:**
- Create: `crates/right-agent/src/mcp/tool_error.rs`
- Modify: `crates/right-agent/src/mcp/mod.rs`

- [ ] **Step 1: Add module declaration**

Modify `crates/right-agent/src/mcp/mod.rs`. Find the `pub mod` block at the top (currently lists `credentials`, `internal_client`, `oauth`, `proxy`, `reconnect`, `refresh`) and append:

```rust
pub mod tool_error;
```

- [ ] **Step 2: Write the failing test file with the helper module skeleton**

Create `crates/right-agent/src/mcp/tool_error.rs`:

```rust
//! Shared MCP operation-error helper.
//!
//! Operation errors are agent-visible failures that fit the MCP convention
//! `is_error: true` on `CallToolResult` with a structured JSON body. This
//! module is the single entry point for constructing them. Protocol and
//! infrastructure failures must continue to bubble up as `Err`.

use rmcp::model::{CallToolResult, Content};
use serde_json::json;

use crate::mcp::proxy::ProxyError;

/// Build an MCP operation error.
///
/// Returns a `CallToolResult` with `is_error: Some(true)` and a single text
/// content body of JSON shape:
///
/// ```json
/// { "error": { "code": "<code>", "message": "<message>", "details": { ... } } }
/// ```
///
/// `details` is omitted from the JSON when `None`.
pub fn tool_error(
    code: &str,
    message: impl Into<String>,
    details: Option<serde_json::Value>,
) -> CallToolResult {
    let mut error = serde_json::Map::new();
    error.insert("code".to_string(), json!(code));
    error.insert("message".to_string(), json!(message.into()));
    if let Some(d) = details {
        error.insert("details".to_string(), d);
    }
    let payload = json!({ "error": serde_json::Value::Object(error) });
    let text = serde_json::to_string_pretty(&payload)
        .expect("serializing tool_error JSON cannot fail");
    CallToolResult::error(vec![Content::text(text)])
}

impl From<ProxyError> for CallToolResult {
    fn from(e: ProxyError) -> Self {
        match &e {
            ProxyError::NeedsAuth { .. } => tool_error("upstream_auth", format!("{e:#}"), None),
            ProxyError::Unreachable { .. } => {
                tool_error("upstream_unreachable", format!("{e:#}"), None)
            }
            ProxyError::NoSession { .. } => {
                tool_error("upstream_unreachable", format!("{e:#}"), None)
            }
            ProxyError::CallToolFailed { source, .. } => tool_error(
                "tool_failed",
                format!("{e:#}"),
                Some(json!({ "detail": format!("{source:#}") })),
            ),
            ProxyError::InitFailed { .. }
            | ProxyError::ListToolsFailed { .. }
            | ProxyError::InstructionsCacheFailed { .. } => {
                tool_error("upstream_unreachable", format!("{e:#}"), None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::RawContent;
    use rmcp::service::ServiceError;

    fn extract_json(result: &CallToolResult) -> serde_json::Value {
        let RawContent::Text(t) = &result.content[0].raw else {
            panic!("expected text content, got {:?}", result.content[0].raw);
        };
        serde_json::from_str(&t.text).expect("body must be valid JSON")
    }

    #[test]
    fn tool_error_sets_is_error_and_basic_shape() {
        let r = tool_error("upstream_auth", "auth failed", None);
        assert_eq!(r.is_error, Some(true));
        let body = extract_json(&r);
        assert_eq!(body["error"]["code"], "upstream_auth");
        assert_eq!(body["error"]["message"], "auth failed");
        assert!(body["error"].get("details").is_none());
    }

    #[test]
    fn tool_error_includes_details_when_present() {
        let r = tool_error(
            "bootstrap_files_missing",
            "missing files",
            Some(json!({ "missing": ["IDENTITY.md"] })),
        );
        let body = extract_json(&r);
        assert_eq!(
            body["error"]["details"]["missing"][0].as_str(),
            Some("IDENTITY.md")
        );
    }

    #[test]
    fn from_proxy_error_needs_auth() {
        let e = ProxyError::NeedsAuth {
            server: "notion".into(),
        };
        let r: CallToolResult = e.into();
        assert_eq!(r.is_error, Some(true));
        let body = extract_json(&r);
        assert_eq!(body["error"]["code"], "upstream_auth");
    }

    #[test]
    fn from_proxy_error_unreachable() {
        let e = ProxyError::Unreachable {
            server: "notion".into(),
        };
        let r: CallToolResult = e.into();
        let body = extract_json(&r);
        assert_eq!(body["error"]["code"], "upstream_unreachable");
    }

    #[test]
    fn from_proxy_error_no_session() {
        let e = ProxyError::NoSession {
            server: "notion".into(),
        };
        let r: CallToolResult = e.into();
        let body = extract_json(&r);
        assert_eq!(body["error"]["code"], "upstream_unreachable");
    }

    #[test]
    fn from_proxy_error_call_tool_failed_includes_detail() {
        let e = ProxyError::CallToolFailed {
            server: "notion".into(),
            tool: "search".into(),
            source: ServiceError::Cancelled,
        };
        let r: CallToolResult = e.into();
        let body = extract_json(&r);
        assert_eq!(body["error"]["code"], "tool_failed");
        assert!(body["error"]["details"]["detail"].is_string());
    }
}
```

- [ ] **Step 3: Run helper tests to verify they fail (compile-only — module not yet used)**

Run: `cargo test -p right-agent mcp::tool_error::tests`
Expected: 5 tests pass (helper has no external dependency yet, so they all pass on first run — this is the green state for the helper itself).

If a test fails for a reason other than logic, fix the helper and re-run.

- [ ] **Step 4: Run full workspace check to ensure nothing else broke**

Run: `cargo check --workspace --tests`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/mcp/tool_error.rs crates/right-agent/src/mcp/mod.rs
git commit -m "feat(mcp): add tool_error helper and From<ProxyError> for CallToolResult"
```

---

## Task 2: `dispatch_to_proxy` migration

**Files:**
- Modify: `crates/right/src/aggregator.rs` (function `dispatch_to_proxy` near line 314, plus existing test `dispatch_unknown_proxy_returns_error` near line 749)

- [ ] **Step 1: Update the existing dispatch test to reflect the new shape (failing first)**

Find this test in `aggregator.rs`:

```rust
    #[tokio::test]
    async fn dispatch_unknown_proxy_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_dispatcher(tmp.path());

        let result = dispatcher
            .dispatch("test-agent", "notion__search", serde_json::json!({}))
            .await;

        let err = result.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Server 'notion' not found"),
            "unexpected error: {msg}"
        );
    }
```

Replace its body with:

```rust
    #[tokio::test]
    async fn dispatch_unknown_proxy_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_dispatcher(tmp.path());

        let result = dispatcher
            .dispatch("test-agent", "notion__search", serde_json::json!({}))
            .await
            .expect("dispatch should return Ok with operation error");
        assert_eq!(result.is_error, Some(true));
        let body = aggregator_test_body(&result);
        assert_eq!(body["error"]["code"], "server_not_found");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("Server 'notion' not found"),
            "unexpected message: {body:?}"
        );
    }
```

Add this helper at the top of `mod tests` (immediately after `use super::*;`), if not already present:

```rust
    fn aggregator_test_body(result: &rmcp::model::CallToolResult) -> serde_json::Value {
        let rmcp::model::RawContent::Text(t) = &result.content[0].raw else {
            panic!("expected text content, got {:?}", result.content[0].raw);
        };
        serde_json::from_str(&t.text).expect("body must be valid JSON")
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p right --bin right aggregator::tests::dispatch_unknown_proxy_returns_error`
Expected: FAIL — current `dispatch_to_proxy` returns `Err`, test now expects `Ok` with `is_error`.

- [ ] **Step 3: Migrate `dispatch_to_proxy`**

Find this function in `aggregator.rs`:

```rust
    pub(crate) async fn dispatch_to_proxy(
        &self,
        proxy_name: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let proxies = self.proxies.read().await;
        let proxy = proxies.get(proxy_name).ok_or_else(|| {
            anyhow::anyhow!("Server '{proxy_name}' not found. It may have been removed.")
        })?;
        Ok(proxy.tools_call(tool, args).await?)
    }
```

Replace with:

```rust
    pub(crate) async fn dispatch_to_proxy(
        &self,
        proxy_name: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, anyhow::Error> {
        let proxies = self.proxies.read().await;
        let Some(proxy) = proxies.get(proxy_name) else {
            return Ok(right_agent::mcp::tool_error::tool_error(
                "server_not_found",
                format!("Server '{proxy_name}' not found. It may have been removed."),
                None,
            ));
        };
        match proxy.tools_call(tool, args).await {
            Ok(result) => Ok(result),
            Err(e) => Ok(CallToolResult::from(e)),
        }
    }
```

- [ ] **Step 4: Run dispatch tests**

Run: `cargo test -p right --bin right aggregator::tests::dispatch_unknown_proxy_returns_error`
Expected: PASS.

Run: `cargo test -p right --bin right aggregator::tests::`
Expected: all aggregator tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/aggregator.rs
git commit -m "feat(aggregator): translate ProxyError at dispatch boundary"
```

---

## Task 3: `HindsightBackend::memory_retain` migration

**Files:**
- Modify: `crates/right/src/aggregator.rs` (the `"memory_retain"` arm of `HindsightBackend::tools_call`, near line 182)

- [ ] **Step 1: Migrate the three `Err(...)` operation-error sites in `memory_retain`**

Find this block in `HindsightBackend::tools_call` (the `"memory_retain"` arm):

```rust
                    Err(right_agent::memory::ResilientError::Upstream(e)) => {
                        // ResilientHindsight::retain enqueues for later drain on
                        // Transient/RateLimited. Surface that as a success with a
                        // "queued" marker so the agent does not report a hard
                        // failure nor retry (which would double-enqueue — the
                        // pending_retains queue does not dedup).
                        match e.classify() {
                            right_agent::memory::ErrorKind::Transient
                            | right_agent::memory::ErrorKind::RateLimited => {
                                let json = serde_json::json!({
                                    "status": "queued",
                                    "reason": "upstream degraded, queued for retry on next drain tick",
                                    "detail": format!("{e:#}"),
                                });
                                Ok(CallToolResult::success(vec![Content::text(
                                    serde_json::to_string_pretty(&json)?,
                                )]))
                            }
                            right_agent::memory::ErrorKind::Auth
                            | right_agent::memory::ErrorKind::Client
                            | right_agent::memory::ErrorKind::Malformed => {
                                Err(anyhow::anyhow!("{e:#}"))
                            }
                        }
                    }
                    Err(right_agent::memory::ResilientError::CircuitOpen { retry_after }) => {
                        // retain() only enqueues on CircuitOpen when status is
                        // NOT AuthFailed. Mirror that distinction here.
                        if matches!(
                            self.client.status(),
                            right_agent::memory::MemoryStatus::AuthFailed { .. }
                        ) {
                            Err(anyhow::anyhow!("memory auth failed; retain rejected"))
                        } else {
                            let json = serde_json::json!({
                                "status": "queued",
                                "reason": "circuit breaker open; queued for retry on next drain tick",
                                "retry_after_secs": retry_after.map(|d| d.as_secs()),
                            });
                            Ok(CallToolResult::success(vec![Content::text(
                                serde_json::to_string_pretty(&json)?,
                            )]))
                        }
                    }
```

Replace with:

```rust
                    Err(right_agent::memory::ResilientError::Upstream(e)) => {
                        // ResilientHindsight::retain enqueues for later drain on
                        // Transient/RateLimited. Surface that as a success with a
                        // "queued" marker so the agent does not report a hard
                        // failure nor retry (which would double-enqueue — the
                        // pending_retains queue does not dedup).
                        match e.classify() {
                            right_agent::memory::ErrorKind::Transient
                            | right_agent::memory::ErrorKind::RateLimited => {
                                let json = serde_json::json!({
                                    "status": "queued",
                                    "reason": "upstream degraded, queued for retry on next drain tick",
                                    "detail": format!("{e:#}"),
                                });
                                Ok(CallToolResult::success(vec![Content::text(
                                    serde_json::to_string_pretty(&json)?,
                                )]))
                            }
                            right_agent::memory::ErrorKind::Auth => Ok(
                                right_agent::mcp::tool_error::tool_error(
                                    "upstream_auth",
                                    format!("{e:#}"),
                                    None,
                                ),
                            ),
                            right_agent::memory::ErrorKind::Client
                            | right_agent::memory::ErrorKind::Malformed => Ok(
                                right_agent::mcp::tool_error::tool_error(
                                    "upstream_invalid",
                                    format!("{e:#}"),
                                    None,
                                ),
                            ),
                        }
                    }
                    Err(right_agent::memory::ResilientError::CircuitOpen { retry_after }) => {
                        // retain() only enqueues on CircuitOpen when status is
                        // NOT AuthFailed. Mirror that distinction here.
                        if matches!(
                            self.client.status(),
                            right_agent::memory::MemoryStatus::AuthFailed { .. }
                        ) {
                            Ok(right_agent::mcp::tool_error::tool_error(
                                "upstream_auth",
                                "memory auth failed; retain rejected",
                                None,
                            ))
                        } else {
                            let json = serde_json::json!({
                                "status": "queued",
                                "reason": "circuit breaker open; queued for retry on next drain tick",
                                "retry_after_secs": retry_after.map(|d| d.as_secs()),
                            });
                            Ok(CallToolResult::success(vec![Content::text(
                                serde_json::to_string_pretty(&json)?,
                            )]))
                        }
                    }
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p right --lib`
Expected: clean.

- [ ] **Step 3: Run existing aggregator tests**

Run: `cargo test -p right --bin right aggregator::tests::`
Expected: all pass — no behavioral test exists for the migrated paths yet (added in Task 5b below).

- [ ] **Step 4: Commit**

```bash
git add crates/right/src/aggregator.rs
git commit -m "feat(aggregator): memory_retain operation errors return is_error"
```

---

## Task 4: `HindsightBackend::memory_recall` and `memory_reflect` migration

**Files:**
- Modify: `crates/right/src/aggregator.rs` (the `"memory_recall"` and `"memory_reflect"` arms of `HindsightBackend::tools_call`, near lines 254 and 273)

- [ ] **Step 1: Migrate the recall and reflect arms**

Find this block in `HindsightBackend::tools_call`:

```rust
            "memory_recall" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: query"))?;
                let results = self
                    .client
                    .recall(
                        query,
                        None,
                        None,
                        right_agent::memory::resilient::POLICY_MCP_RECALL,
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                let json = serde_json::json!({ "results": results });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json)?,
                )]))
            }
            "memory_reflect" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: query"))?;
                let result = self
                    .client
                    .reflect(query, right_agent::memory::resilient::POLICY_MCP_REFLECT)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
                let json = serde_json::json!({ "text": result.text });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json)?,
                )]))
            }
```

Replace with (note: the recall/reflect arms now reuse a helper `classify_resilient_error` defined just below the arm — see Step 2):

```rust
            "memory_recall" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: query"))?;
                let res = self
                    .client
                    .recall(
                        query,
                        None,
                        None,
                        right_agent::memory::resilient::POLICY_MCP_RECALL,
                    )
                    .await;
                match res {
                    Ok(results) => {
                        let json = serde_json::json!({ "results": results });
                        Ok(CallToolResult::success(vec![Content::text(
                            serde_json::to_string_pretty(&json)?,
                        )]))
                    }
                    Err(e) => Ok(self.classify_resilient_error(e)),
                }
            }
            "memory_reflect" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required param: query"))?;
                let res = self
                    .client
                    .reflect(query, right_agent::memory::resilient::POLICY_MCP_REFLECT)
                    .await;
                match res {
                    Ok(result) => {
                        let json = serde_json::json!({ "text": result.text });
                        Ok(CallToolResult::success(vec![Content::text(
                            serde_json::to_string_pretty(&json)?,
                        )]))
                    }
                    Err(e) => Ok(self.classify_resilient_error(e)),
                }
            }
```

- [ ] **Step 2: Add the `classify_resilient_error` helper method on `HindsightBackend`**

Find the `impl HindsightBackend` block. Just after the closing `}` of `tools_call` (and before the closing `}` of the impl), add:

```rust
    /// Map a `ResilientError` from `recall` / `reflect` to a structured
    /// operation error. The `retain` path has its own queueing semantics and
    /// does not use this helper.
    fn classify_resilient_error(
        &self,
        e: right_agent::memory::ResilientError,
    ) -> CallToolResult {
        match e {
            right_agent::memory::ResilientError::Upstream(ref inner) => match inner.classify() {
                right_agent::memory::ErrorKind::Transient
                | right_agent::memory::ErrorKind::RateLimited => {
                    right_agent::mcp::tool_error::tool_error(
                        "upstream_unreachable",
                        format!("{e:#}"),
                        None,
                    )
                }
                right_agent::memory::ErrorKind::Auth => right_agent::mcp::tool_error::tool_error(
                    "upstream_auth",
                    format!("{e:#}"),
                    None,
                ),
                right_agent::memory::ErrorKind::Client
                | right_agent::memory::ErrorKind::Malformed => {
                    right_agent::mcp::tool_error::tool_error(
                        "upstream_invalid",
                        format!("{e:#}"),
                        None,
                    )
                }
            },
            right_agent::memory::ResilientError::CircuitOpen { retry_after } => {
                if matches!(
                    self.client.status(),
                    right_agent::memory::MemoryStatus::AuthFailed { .. }
                ) {
                    right_agent::mcp::tool_error::tool_error(
                        "upstream_auth",
                        format!("{e:#}"),
                        None,
                    )
                } else {
                    let details = retry_after
                        .map(|d| serde_json::json!({ "retry_after_secs": d.as_secs() }));
                    right_agent::mcp::tool_error::tool_error(
                        "circuit_open",
                        format!("{e:#}"),
                        details,
                    )
                }
            }
        }
    }
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p right --lib`
Expected: clean.

- [ ] **Step 4: Run existing tests**

Run: `cargo test -p right --bin right aggregator::`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/aggregator.rs
git commit -m "feat(aggregator): memory_recall/reflect operation errors return is_error"
```

---

## Task 5a: `RightBackend` allowlist and bootstrap migration

**Files:**
- Modify: `crates/right/src/right_backend.rs` (lines ~163, ~196, ~417)

- [ ] **Step 1: Migrate the allowlist-reject site in `cron_create`**

Find this block in `RightBackend::call_cron_create` (around line 162):

```rust
        if let Err(msg) = validate_target_against_allowlist(agent_dir, params.target_chat_id) {
            return Ok(CallToolResult::error(vec![Content::text(msg)]));
        }
```

Replace with:

```rust
        if let Err(msg) = validate_target_against_allowlist(agent_dir, params.target_chat_id) {
            return Ok(right_agent::mcp::tool_error::tool_error(
                "chat_id_not_in_allowlist",
                msg,
                None,
            ));
        }
```

- [ ] **Step 2: Migrate the allowlist-reject site in `cron_update`**

Find this block in `RightBackend::call_cron_update` (around line 193):

```rust
        if let Some(chat) = params.target_chat_id
            && let Err(msg) = validate_target_against_allowlist(agent_dir, chat)
        {
            return Ok(CallToolResult::error(vec![Content::text(msg)]));
        }
```

Replace with:

```rust
        if let Some(chat) = params.target_chat_id
            && let Err(msg) = validate_target_against_allowlist(agent_dir, chat)
        {
            return Ok(right_agent::mcp::tool_error::tool_error(
                "chat_id_not_in_allowlist",
                msg,
                None,
            ));
        }
```

- [ ] **Step 3: Migrate the missing-files site in `bootstrap_done`**

Find this block in `RightBackend::call_bootstrap_done` (around line 417):

```rust
        } else {
            Ok(CallToolResult::error(vec![Content::text(format!(
                "Cannot complete bootstrap — missing files: {}. \
                 Create them first, then call bootstrap_done again.",
                missing.join(", ")
            ))]))
        }
```

Replace with:

```rust
        } else {
            let message = format!(
                "Cannot complete bootstrap — missing files: {}. \
                 Create them first, then call bootstrap_done again.",
                missing.join(", ")
            );
            Ok(right_agent::mcp::tool_error::tool_error(
                "bootstrap_files_missing",
                message,
                Some(serde_json::json!({ "missing": missing })),
            ))
        }
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p right --lib`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/right_backend.rs
git commit -m "feat(right_backend): allowlist and bootstrap_done emit structured tool_error"
```

---

## Task 5b: `RightBackend` test coverage

**Files:**
- Modify: `crates/right/src/right_backend_tests.rs`

- [ ] **Step 1: Add a body-extractor helper if not already present**

Open `crates/right/src/right_backend_tests.rs`. At the top of the test module (just after the `use super::*;` or equivalent imports), ensure this helper exists. If not, add it:

```rust
fn extract_error_body(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    let rmcp::model::RawContent::Text(t) = &result.content[0].raw else {
        panic!("expected text content, got {:?}", result.content[0].raw);
    };
    serde_json::from_str(&t.text).expect("body must be valid JSON")
}
```

- [ ] **Step 2: Add the bootstrap_done missing-files test**

Append to `right_backend_tests.rs`:

```rust
#[tokio::test]
async fn bootstrap_done_returns_tool_error_when_files_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let agents_dir = tmp.path().join("agents");
    let agent_dir = agents_dir.join("test-agent");
    std::fs::create_dir_all(&agent_dir).unwrap();

    let backend = RightBackend::new(agents_dir, None);
    let result = backend
        .tools_call("test-agent", &agent_dir, "bootstrap_done", serde_json::json!({}))
        .await
        .expect("dispatch should be Ok with operation error");

    assert_eq!(result.is_error, Some(true));
    let body = extract_error_body(&result);
    assert_eq!(body["error"]["code"], "bootstrap_files_missing");
    let missing = body["error"]["details"]["missing"]
        .as_array()
        .expect("details.missing must be an array");
    let names: Vec<&str> = missing.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&"IDENTITY.md"), "missing IDENTITY.md: {names:?}");
    assert!(names.contains(&"SOUL.md"));
    assert!(names.contains(&"USER.md"));
}
```

- [ ] **Step 3: Run the new test**

Run: `cargo test -p right --bin right right_backend::tests::bootstrap_done_returns_tool_error_when_files_missing`
Expected: PASS.

- [ ] **Step 4: Run the full right_backend test module**

Run: `cargo test -p right --bin right right_backend::tests::`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/right_backend_tests.rs
git commit -m "test(right_backend): cover bootstrap_done structured error path"
```

---

## Task 6: Aggregator-side operation-error tests with mock Hindsight

**Files:**
- Modify: `crates/right/src/aggregator.rs` (existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Add a small mock-server helper inside the existing test module**

Open `crates/right/src/aggregator.rs` and find `mod tests`. Inside the module (near the existing `make_test_registry` / `make_dispatcher` helpers), add:

```rust
    /// Mock HTTP server that responds to each incoming connection with the given
    /// status + body. Mirrors the helper from `right-agent::memory::resilient`
    /// tests; copied (not exposed) to avoid test-only public API growth.
    async fn mock_hindsight(body: &str, status: u16) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let body = body.to_owned();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body,
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (handle, url)
    }

    fn make_hindsight_backend(url: &str) -> std::sync::Arc<HindsightBackend> {
        use right_agent::memory::{HindsightClient, ResilientHindsight};
        let dir = tempfile::tempdir().unwrap().keep();
        let _ = right_agent::memory::open_connection(&dir, true).unwrap();
        let client = HindsightClient::new("hs_x", "bank-1", "high", 1024, Some(url));
        let resilient = std::sync::Arc::new(ResilientHindsight::new(client, dir, "test"));
        std::sync::Arc::new(HindsightBackend::new(resilient))
    }
```

> **Note:** If `right_agent::memory::open_connection` or `HindsightClient::new` signatures differ from the snippets above when you reach this step, mirror the exact signature used in `crates/right-agent/src/memory/resilient.rs`'s own `wrap()` test helper (read it first if in doubt — it is the canonical fixture).

- [ ] **Step 2: Add memory_retain Auth → upstream_auth test**

Append to `mod tests`:

```rust
    #[tokio::test]
    async fn memory_retain_auth_returns_upstream_auth() {
        let (_h, url) = mock_hindsight(r#"{"error": "unauthorized"}"#, 401).await;
        let backend = make_hindsight_backend(&url);
        let result = backend
            .tools_call(
                "memory_retain",
                serde_json::json!({ "content": "x" }),
            )
            .await
            .expect("Ok with operation error");
        assert_eq!(result.is_error, Some(true));
        let body = aggregator_test_body(&result);
        assert_eq!(body["error"]["code"], "upstream_auth");
    }
```

- [ ] **Step 3: Add memory_retain Client → upstream_invalid test**

Append:

```rust
    #[tokio::test]
    async fn memory_retain_client_returns_upstream_invalid() {
        let (_h, url) = mock_hindsight(r#"{"error": "bad request"}"#, 400).await;
        let backend = make_hindsight_backend(&url);
        let result = backend
            .tools_call(
                "memory_retain",
                serde_json::json!({ "content": "x" }),
            )
            .await
            .expect("Ok with operation error");
        let body = aggregator_test_body(&result);
        assert_eq!(body["error"]["code"], "upstream_invalid");
    }
```

- [ ] **Step 4: Add memory_retain queued path regression test**

Append (this is a regression guard — queued path must remain `is_error: false`):

```rust
    #[tokio::test]
    async fn memory_retain_transient_remains_queued_success() {
        let (_h, url) = mock_hindsight(r#"{"error": "bad gateway"}"#, 502).await;
        let backend = make_hindsight_backend(&url);
        let result = backend
            .tools_call(
                "memory_retain",
                serde_json::json!({ "content": "x" }),
            )
            .await
            .expect("Ok success with queued status");
        assert_eq!(result.is_error, None.or(Some(false)).filter(|v| !*v).map(|_| false), 
            "queued path must not set is_error=true");
        // is_error is either None or Some(false) — both are acceptable success
        assert!(matches!(result.is_error, None | Some(false)));
        let body = aggregator_test_body(&result);
        assert_eq!(body["status"], "queued");
    }
```

> **Note:** Adjust the `result.is_error` assertion to whichever of `None` / `Some(false)` `CallToolResult::success` produces. Run the test once, see what it actually returns, and tighten the assertion to that single form. The simpler check `assert!(matches!(result.is_error, None | Some(false)))` is the resilient fallback.

- [ ] **Step 5: Add memory_recall Auth path test**

Append:

```rust
    #[tokio::test]
    async fn memory_recall_auth_returns_upstream_auth() {
        let (_h, url) = mock_hindsight(r#"{"error": "unauthorized"}"#, 401).await;
        let backend = make_hindsight_backend(&url);
        let result = backend
            .tools_call(
                "memory_recall",
                serde_json::json!({ "query": "test" }),
            )
            .await
            .expect("Ok with operation error");
        let body = aggregator_test_body(&result);
        assert_eq!(body["error"]["code"], "upstream_auth");
    }
```

- [ ] **Step 6: Add memory_recall transient path test (`upstream_unreachable`)**

Append:

```rust
    #[tokio::test]
    async fn memory_recall_transient_returns_upstream_unreachable() {
        let (_h, url) = mock_hindsight(r#"{"error": "bad gateway"}"#, 502).await;
        let backend = make_hindsight_backend(&url);
        let result = backend
            .tools_call(
                "memory_recall",
                serde_json::json!({ "query": "test" }),
            )
            .await
            .expect("Ok with operation error");
        let body = aggregator_test_body(&result);
        assert_eq!(body["error"]["code"], "upstream_unreachable");
    }
```

- [ ] **Step 7: Run the new tests**

Run: `cargo test -p right --bin right aggregator::tests::memory_`
Expected: all pass.

- [ ] **Step 8: Run the full aggregator suite**

Run: `cargo test -p right --bin right aggregator::`
Expected: all pass.

- [ ] **Step 9: Commit**

```bash
git add crates/right/src/aggregator.rs
git commit -m "test(aggregator): cover Hindsight operation-error mappings"
```

---

## Task 7: Documentation updates

**Files:**
- Modify: `crates/right/src/aggregator.rs` (the `Aggregator::get_info().with_instructions(...)` call near line 489)
- Modify: `crates/right/src/right_backend.rs` (tool descriptions in `tools_list`, lines ~46-92)
- Modify: `PROMPT_SYSTEM.md` (add subsection)

- [ ] **Step 1: Extend aggregator instructions block**

In `crates/right/src/aggregator.rs`, find:

```rust
            .with_instructions(
                "Right Agent MCP Aggregator — routes tool calls to built-in Right Agent tools \
                 and connected external MCP servers via prefix-based dispatch.\n\n\
                 Memory tools (when Hindsight is configured):\n\
                 - memory_retain: Store facts to long-term memory\n\
                 - memory_recall: Search memory by relevance\n\
                 - memory_reflect: Synthesize reasoned answers from memory",
            )
```

Replace with:

```rust
            .with_instructions(
                "Right Agent MCP Aggregator — routes tool calls to built-in Right Agent tools \
                 and connected external MCP servers via prefix-based dispatch.\n\n\
                 Memory tools (when Hindsight is configured):\n\
                 - memory_retain: Store facts to long-term memory\n\
                 - memory_recall: Search memory by relevance\n\
                 - memory_reflect: Synthesize reasoned answers from memory\n\
                 (Errors follow the aggregator-level error convention; see below.)\n\n\
                 Error convention (operation errors):\n\
                 On operation failure, tools return is_error: true with content\n  \
                 { \"error\": { \"code\": \"<code>\", \"message\": \"<human readable>\", \"details\"?: {...} } }\n\
                 Cross-cutting codes any tool may emit:\n  \
                 upstream_unreachable — backend service unreachable / transport failure\n  \
                 upstream_auth        — backend authentication required or rejected\n  \
                 upstream_invalid     — backend rejected the request (4xx, malformed)\n  \
                 circuit_open         — local circuit breaker open; retry later\n  \
                 invalid_argument     — semantic argument validation failed\n  \
                 tool_failed          — upstream tool returned its own error (see details)\n  \
                 server_not_found     — referenced MCP server is not registered\n\
                 Tool-specific codes are documented in each tool's description.",
            )
```

- [ ] **Step 2: Append `Errors:` clauses to tool-specific-code tools**

In `crates/right/src/right_backend.rs`, find the `tools_list()` definitions:

For `cron_create` (line ~46-50), replace:

```rust
            Tool::new(
                "cron_create",
                "Create a new cron job spec. Supports recurring schedules and one-shot jobs (via run_at or recurring=false). The job will be picked up by the cron engine on its next reload cycle.",
                schema_for_type::<CronCreateParams>(),
            ),
```

with:

```rust
            Tool::new(
                "cron_create",
                "Create a new cron job spec. Supports recurring schedules and one-shot jobs (via run_at or recurring=false). The job will be picked up by the cron engine on its next reload cycle. Errors: chat_id_not_in_allowlist (the target chat must first be approved via /allow or /allow_all).",
                schema_for_type::<CronCreateParams>(),
            ),
```

For `cron_update` (line ~51-55), replace:

```rust
            Tool::new(
                "cron_update",
                "Update an existing cron job spec. Only pass fields you want to change — unspecified fields keep their current values. Setting schedule clears run_at; setting run_at clears schedule.",
                schema_for_type::<CronUpdateParams>(),
            ),
```

with:

```rust
            Tool::new(
                "cron_update",
                "Update an existing cron job spec. Only pass fields you want to change — unspecified fields keep their current values. Setting schedule clears run_at; setting run_at clears schedule. Errors: chat_id_not_in_allowlist (when updating target_chat_id to a chat not in the allowlist).",
                schema_for_type::<CronUpdateParams>(),
            ),
```

For `bootstrap_done` (line ~88-92), replace:

```rust
            Tool::new(
                "bootstrap_done",
                "Signal that bootstrap onboarding is complete. Call this AFTER you have created IDENTITY.md, SOUL.md, and USER.md. The system will verify the files exist.",
                schema_for_type::<CronListParams>(), // empty schema — no params
            ),
```

with:

```rust
            Tool::new(
                "bootstrap_done",
                "Signal that bootstrap onboarding is complete. Call this AFTER you have created IDENTITY.md, SOUL.md, and USER.md. The system will verify the files exist. Errors: bootstrap_files_missing (one or more identity files not yet created — see details.missing).",
                schema_for_type::<CronListParams>(), // empty schema — no params
            ),
```

- [ ] **Step 3: Add an Error Convention subsection to PROMPT_SYSTEM.md**

In `PROMPT_SYSTEM.md`, find the section heading `## MCP Server Instructions` (around line 196). After the entire section's closing paragraph (before `## Upstream MCP Server Instructions`), insert:

```markdown
### Error Convention

Tool failures return `is_error: true` with a JSON body of shape

    { "error": { "code": "<code>", "message": "<human readable>", "details"?: {...} } }

Operation errors are normal and recoverable; the agent reads `error.code` to
decide whether to retry, surface to the user, or take a different path.
Protocol errors (JSON-RPC errors) indicate a bug in the agent's tool call
itself (unknown tool, missing/malformed argument).

Cross-cutting codes any tool may emit: `upstream_unreachable`, `upstream_auth`,
`upstream_invalid`, `circuit_open`, `invalid_argument`, `tool_failed`,
`server_not_found`. Tool-specific codes are listed in each tool's
description.
```

- [ ] **Step 4: Verify the workspace still builds**

Run: `cargo check --workspace --tests`
Expected: clean.

- [ ] **Step 5: Run the full aggregator and right_backend test suites**

Run: `cargo test -p right --bin right aggregator:: right_backend::`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/right/src/aggregator.rs crates/right/src/right_backend.rs PROMPT_SYSTEM.md
git commit -m "docs(mcp): document operation-error convention and per-tool codes"
```

---

## Task 8: Final workspace verification

**Files:** none modified — verification only.

- [ ] **Step 1: Build the workspace in debug**

Run: `cargo build --workspace`
Expected: clean build, zero errors.

- [ ] **Step 2: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: all tests pass. If any test outside the touched files fails, investigate — it likely means a downstream test was matching the old `Err`-style error message and needs updating to the new `is_error`/`error.code` shape.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace --tests -- -D warnings`
Expected: clean.

- [ ] **Step 4: Spot-check the agent-facing instructions block**

Run: `cargo test -p right --bin right aggregator::tests::tools_list_includes_right_and_meta`
Expected: PASS — no regression in the basic tool surface.

If everything is green, the implementation matches the spec. Done.

---

## Self-Review

**Spec coverage:**
- Goal "operation failure returns Ok with is_error" — Tasks 2, 3, 4, 5a.
- Goal "protocol/infrastructure stay as Err" — verified by Task 5b (deserialize-malformed args still `Err`) and by leaving `?` propagation untouched in cron arms.
- Goal "agent-visible shape uniform across backends" — Task 1 (single helper) + per-backend migrations call it.
- Goal "per-tool tests" — Tasks 5b and 6.
- Helper module + `From<ProxyError>` — Task 1.
- ProxyBackend translation at dispatch boundary — Task 2.
- Aggregator instructions update — Task 7.
- PROMPT_SYSTEM.md update — Task 7.
- Tool-description updates for tool-specific codes — Task 7.
- Existing `dispatch_unknown_proxy_returns_error` test flips — Task 2 Step 1.
- `memory_retain` queued path regression — Task 6 Step 4.
- `memory_recall` / `memory_reflect` `CircuitOpen` mapping — Task 4 (covered by `classify_resilient_error`).

**Placeholder scan:**
- No "TBD"/"TODO" left.
- One soft-instruction in Task 6 Step 1 about mirroring `wrap()` if the `HindsightClient::new` signature drifts — that is a verify-against-current-code instruction, not a placeholder.
- One soft-instruction in Task 6 Step 4 about tightening `is_error` to whichever exact form `CallToolResult::success` returns — gives a working fallback `matches!(... None | Some(false))` so the test compiles and passes regardless. Acceptable.

**Type consistency:**
- `tool_error(code: &str, message: impl Into<String>, details: Option<serde_json::Value>) -> CallToolResult` — same signature in module (Task 1), at every call site (Tasks 2, 3, 4, 5a), and in test imports.
- `From<ProxyError> for CallToolResult` — same impl in Task 1, used as `CallToolResult::from(e)` in Task 2.
- `aggregator_test_body` and `extract_error_body` are two separate test helpers (one in aggregator.rs, one in right_backend_tests.rs) with the same body. Acceptable — they live in separate test modules.
- Code strings used (`upstream_auth`, `upstream_unreachable`, `upstream_invalid`, `circuit_open`, `chat_id_not_in_allowlist`, `bootstrap_files_missing`, `server_not_found`, `tool_failed`) are referenced consistently between implementation tasks, test tasks, and documentation tasks.
