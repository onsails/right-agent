# Memory System Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable CC native memory for conversation continuity, rename MCP record tools to signal "structured data", add memory prompt instructions to AGENTS.md.

**Architecture:** Two-layer memory — CC auto-memory for fuzzy continuity (system prompt injection), SQLite `right` MCP for structured tagged records (on-demand tool calls). One settings.rs flip, tool renames in both MCP servers, prompt updates in agent templates.

**Tech Stack:** Rust, rmcp, schemars, serde, rusqlite

---

### Task 1: Enable CC native memory in settings.rs

**Files:**
- Modify: `crates/rightclaw/src/codegen/settings.rs:19`
- Modify: `crates/rightclaw/src/codegen/settings_tests.rs:30`

- [ ] **Step 1: Update the settings test to expect `true`**

In `crates/rightclaw/src/codegen/settings_tests.rs`, change line 30:

```rust
    assert_eq!(settings["autoMemoryEnabled"], true);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib codegen::settings::tests::generates_behavioral_flags`
Expected: FAIL — assertion `false == true`

- [ ] **Step 3: Flip the setting**

In `crates/rightclaw/src/codegen/settings.rs`, change line 19:

```rust
        "autoMemoryEnabled": true,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rightclaw --lib codegen::settings::tests`
Expected: all settings tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/settings.rs crates/rightclaw/src/codegen/settings_tests.rs
git commit -m "feat: enable CC native memory (autoMemoryEnabled: true)

CC's auto-memory handles conversation continuity — preferences,
decisions, context persisted across sessions. Our SQLite records
serve structured/programmatic storage only."
```

---

### Task 2: Rename param structs in memory_server.rs

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server.rs:15-39`

- [ ] **Step 1: Rename all four param structs and their field descriptions**

In `crates/rightclaw-cli/src/memory_server.rs`, replace lines 15-39:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StoreRecordParams {
    #[schemars(description = "Content to store as a record")]
    pub content: String,
    #[schemars(description = "Comma-separated tags for categorization")]
    pub tags: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryRecordsParams {
    #[schemars(description = "Tag or keyword to search by")]
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchRecordsParams {
    #[schemars(description = "Full-text search query")]
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteRecordParams {
    #[schemars(description = "Record ID to soft-delete")]
    pub id: i64,
}
```

- [ ] **Step 2: Verify it compiles (will fail — dependents not yet updated)**

Run: `cargo check -p rightclaw-cli 2>&1 | head -20`
Expected: errors about `StoreParams`, `RecallParams`, `SearchParams`, `ForgetParams` not found. This confirms the rename propagated.

- [ ] **Step 3: Commit (partial — struct renames only)**

```bash
git add crates/rightclaw-cli/src/memory_server.rs
git commit -m "refactor: rename memory param structs to record terminology

StoreParams → StoreRecordParams, RecallParams → QueryRecordsParams,
SearchParams → SearchRecordsParams, ForgetParams → DeleteRecordParams"
```

---

### Task 3: Rename tool functions in memory_server.rs (stdio)

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server.rs:107-192`
- Modify: `crates/rightclaw-cli/src/memory_server.rs:394-406` (ServerHandler instructions)

- [ ] **Step 1: Rename `store` → `store_record` with new description and param type**

In `crates/rightclaw-cli/src/memory_server.rs`, replace the `store` function (lines 107-132):

```rust
    #[tool(description = "Store a tagged record. Content is scanned for prompt injection. Use for structured data (cron results, audit entries, explicit facts) — not for general conversation context. Returns record ID.")]
    async fn store_record(
        &self,
        Parameters(params): Parameters<StoreRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        match rightclaw::memory::store::store_memory(
            &conn,
            &params.content,
            params.tags.as_deref(),
            Some(self.agent_name.as_str()),
            Some("mcp:store_record"),
        ) {
            Ok(id) => Ok(CallToolResult::success(vec![Content::text(format!(
                "stored record id={id}"
            ))])),
            Err(rightclaw::memory::MemoryError::InjectionDetected) => Err(McpError::invalid_params(
                "content rejected: possible prompt injection detected",
                None,
            )),
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }
```

- [ ] **Step 2: Rename `recall` → `query_records` with new description and param type**

Replace the `recall` function (lines 134-149):

```rust
    #[tool(description = "Look up records by tag or keyword. Returns matching active records.")]
    async fn query_records(
        &self,
        Parameters(params): Parameters<QueryRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let entries = rightclaw::memory::store::recall_memories(&conn, &params.query)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let json_values: Vec<serde_json::Value> = entries.iter().map(entry_to_json).collect();
        let output = serde_json::to_string_pretty(&json_values)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
```

- [ ] **Step 3: Rename `search` → `search_records` with new description and param type**

Replace the `search` function (lines 151-166):

```rust
    #[tool(description = "Full-text search records using FTS5. Returns BM25-ranked results.")]
    async fn search_records(
        &self,
        Parameters(params): Parameters<SearchRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let entries = rightclaw::memory::store::search_memories(&conn, &params.query)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let json_values: Vec<serde_json::Value> = entries.iter().map(entry_to_json).collect();
        let output = serde_json::to_string_pretty(&json_values)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
```

- [ ] **Step 4: Rename `forget` → `delete_record` with new description and param type**

Replace the `forget` function (lines 168-192):

```rust
    #[tool(description = "Soft-delete a record by ID. Entry is excluded from queries but preserved in audit log.")]
    async fn delete_record(
        &self,
        Parameters(params): Parameters<DeleteRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = params.id;
        let conn = self
            .conn
            .lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        match rightclaw::memory::store::forget_memory(
            &conn,
            id,
            Some(self.agent_name.as_str()),
        ) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "deleted record id={id}"
            ))])),
            Err(rightclaw::memory::MemoryError::NotFound(_)) => Err(McpError::invalid_params(
                format!("record id={id} not found or already deleted"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }
```

- [ ] **Step 5: Update ServerHandler instructions string**

In the `ServerHandler` impl (line 403), replace the instructions string:

```rust
            .with_instructions(
                "RightClaw tools: store_record, query_records, search_records, delete_record, cron_list_runs, cron_show_run, mcp_add, mcp_remove, mcp_list, mcp_auth",
            )
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p rightclaw-cli 2>&1 | head -20`
Expected: errors from `memory_server_http.rs` (still uses old imports), but `memory_server.rs` itself should be clean.

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server.rs
git commit -m "refactor: rename MCP tool functions in stdio server

store → store_record, recall → query_records,
search → search_records, forget → delete_record.
Updated descriptions, response messages, and source_tool tag."
```

---

### Task 4: Rename tool functions in memory_server_http.rs

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server_http.rs:27-31` (imports)
- Modify: `crates/rightclaw-cli/src/memory_server_http.rs:103-184` (tool functions)
- Modify: `crates/rightclaw-cli/src/memory_server_http.rs:381-393` (ServerHandler instructions)

- [ ] **Step 1: Update imports**

In `crates/rightclaw-cli/src/memory_server_http.rs`, replace lines 27-31:

```rust
use crate::memory_server::{
    CronListRunsParams, CronShowRunParams, DeleteRecordParams, McpAddParams, McpAuthParams,
    McpListParams, McpRemoveParams, QueryRecordsParams, SearchRecordsParams, StoreRecordParams,
    cron_run_to_json, entry_to_json,
};
```

- [ ] **Step 2: Rename `store` → `store_record`**

Replace the `store` function (lines 103-127):

```rust
    #[tool(description = "Store a tagged record. Content is scanned for prompt injection. Use for structured data (cron results, audit entries, explicit facts) — not for general conversation context. Returns record ID.")]
    async fn store_record(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<StoreRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        match rightclaw::memory::store::store_memory(
            &conn,
            &params.content,
            params.tags.as_deref(),
            Some(agent.name.as_str()),
            Some("mcp:store_record"),
        ) {
            Ok(id) => Ok(CallToolResult::success(vec![Content::text(format!("stored record id={id}"))])),
            Err(rightclaw::memory::MemoryError::InjectionDetected) => Err(McpError::invalid_params(
                "content rejected: possible prompt injection detected",
                None,
            )),
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }
```

- [ ] **Step 3: Rename `recall` → `query_records`**

Replace the `recall` function (lines 129-145):

```rust
    #[tool(description = "Look up records by tag or keyword. Returns matching active records.")]
    async fn query_records(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<QueryRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let entries = rightclaw::memory::store::recall_memories(&conn, &params.query)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let json_values: Vec<serde_json::Value> = entries.iter().map(entry_to_json).collect();
        let output = serde_json::to_string_pretty(&json_values)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
```

- [ ] **Step 4: Rename `search` → `search_records`**

Replace the `search` function (lines 147-163):

```rust
    #[tool(description = "Full-text search records using FTS5. Returns BM25-ranked results.")]
    async fn search_records(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<SearchRecordsParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        let entries = rightclaw::memory::store::search_memories(&conn, &params.query)
            .map_err(|e| McpError::internal_error(format!("{e:#}"), None))?;
        let json_values: Vec<serde_json::Value> = entries.iter().map(entry_to_json).collect();
        let output = serde_json::to_string_pretty(&json_values)
            .map_err(|e| McpError::internal_error(format!("serialization error: {e:#}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
```

- [ ] **Step 5: Rename `forget` → `delete_record`**

Replace the `forget` function (lines 165-184):

```rust
    #[tool(description = "Soft-delete a record by ID. Entry is excluded from queries but preserved in audit log.")]
    async fn delete_record(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<DeleteRecordParams>,
    ) -> Result<CallToolResult, McpError> {
        let agent = Self::agent_from_parts(&parts)?;
        let id = params.id;
        let conn_arc = self.get_conn_for_agent(&agent)?;
        let conn = conn_arc.lock()
            .map_err(|e| McpError::internal_error(format!("mutex poisoned: {e}"), None))?;
        match rightclaw::memory::store::forget_memory(&conn, id, Some(agent.name.as_str())) {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!("deleted record id={id}"))])),
            Err(rightclaw::memory::MemoryError::NotFound(_)) => Err(McpError::invalid_params(
                format!("record id={id} not found or already deleted"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(format!("{e:#}"), None)),
        }
    }
```

- [ ] **Step 6: Update ServerHandler instructions string**

In the `ServerHandler` impl (line 390), replace the instructions string:

```rust
            .with_instructions(
                "RightClaw tools: store_record, query_records, search_records, delete_record, cron_list_runs, cron_show_run, mcp_add, mcp_remove, mcp_list, mcp_auth",
            )
```

- [ ] **Step 7: Verify compilation**

Run: `cargo check -p rightclaw-cli`
Expected: PASS — no more references to old types.

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server_http.rs
git commit -m "refactor: rename MCP tool functions in HTTP server

Mirror stdio server renames: store → store_record,
recall → query_records, search → search_records, forget → delete_record."
```

---

### Task 5: Update test that checks ServerHandler instructions

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server_mcp_tests.rs:308-328`

- [ ] **Step 1: Update the `test_get_info_mentions_mcp_tools` test**

In `crates/rightclaw-cli/src/memory_server_mcp_tests.rs`, replace the test at lines 307-328:

```rust
#[test]
fn test_get_info_mentions_record_tools() {
    let (server, _dir) = setup_server_with_dir();
    let info = server.get_info();
    let instructions = info.instructions.unwrap_or_default();
    assert!(
        instructions.contains("store_record"),
        "instructions should mention store_record: {instructions}"
    );
    assert!(
        instructions.contains("query_records"),
        "instructions should mention query_records: {instructions}"
    );
    assert!(
        instructions.contains("search_records"),
        "instructions should mention search_records: {instructions}"
    );
    assert!(
        instructions.contains("delete_record"),
        "instructions should mention delete_record: {instructions}"
    );
    assert!(
        instructions.contains("mcp_add"),
        "instructions should mention mcp_add: {instructions}"
    );
}
```

- [ ] **Step 2: Run all tests**

Run: `cargo test -p rightclaw-cli`
Expected: all tests PASS

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server_mcp_tests.rs
git commit -m "test: update ServerHandler instructions test for renamed tools"
```

---

### Task 6: Add Memory section to AGENTS.md templates

**Files:**
- Modify: `templates/right/AGENTS.md`
- Modify: `identity/AGENTS.md`

- [ ] **Step 1: Add Memory section to `templates/right/AGENTS.md`**

Insert the following **before** the existing `## MCP Management` section (before line 23). The file currently starts with `## Core Skills`:

In `templates/right/AGENTS.md`, insert before `## MCP Management`:

```markdown
## Memory

Claude Code manages your conversation memory automatically.
Important context, user preferences, and decisions persist across sessions
without any action from you.

For **structured data** that needs tags or search later, use the `right` MCP tools:

- `store_record(content, tags)` — store a tagged record (cron results, audit entries, explicit facts)
- `query_records(query)` — look up records by tag or keyword
- `search_records(query)` — full-text search across all records (BM25-ranked)
- `delete_record(id)` — soft-delete a record by ID

Use these for data you or cron jobs need to retrieve programmatically —
not for general conversation context (Claude handles that).

```

- [ ] **Step 2: Add Memory section to `identity/AGENTS.md`**

Insert the following **before** the existing `## MCP Management` section (before line 1). The file currently starts with `## MCP Management`:

In `identity/AGENTS.md`, insert before `## MCP Management`:

```markdown
## Memory

Claude Code manages your conversation memory automatically.
Important context, user preferences, and decisions persist across sessions
without any action from you.

For **structured data** that needs tags or search later, use the `right` MCP tools:

- `store_record(content, tags)` — store a tagged record (cron results, audit entries, explicit facts)
- `query_records(query)` — look up records by tag or keyword
- `search_records(query)` — full-text search across all records (BM25-ranked)
- `delete_record(id)` — soft-delete a record by ID

Use these for data you or cron jobs need to retrieve programmatically —
not for general conversation context (Claude handles that).

```

- [ ] **Step 3: Commit**

```bash
git add templates/right/AGENTS.md identity/AGENTS.md
git commit -m "docs: add Memory section to AGENTS.md templates

Guides agents on the two-layer memory split: CC native memory
for conversation continuity, right MCP tools for structured records."
```

---

### Task 7: Build full workspace and verify

**Files:** None (verification only)

- [ ] **Step 1: Build full workspace**

Run: `cargo build --workspace`
Expected: PASS — no compilation errors.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: PASS — no warnings.

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: all tests PASS.
