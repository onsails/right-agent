# OpenShell Sandbox + HTTP MCP + OAuth Token Refresh — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace CC native sandbox with OpenShell containers, convert rightmemory to HTTP MCP server with per-agent auth, add OAuth token refresh.

**Architecture:** Each agent runs inside an OpenShell Docker sandbox with Landlock/seccomp/network isolation. Rightmemory becomes an HTTP MCP server on the host, authenticated per-agent via Bearer tokens. OAuth tokens are refreshed by a background tokio task before expiry.

**Tech Stack:** Rust (edition 2024), rmcp (with `transport-streamable-http-server-session` feature), axum 0.8, tokio 1.50, OpenShell CLI, serde, minijinja

---

### Task 1: Add rmcp HTTP transport feature flag

**Files:**
- Modify: `Cargo.toml` (root workspace)

- [ ] **Step 1: Add transport-streamable-http-server-session feature to rmcp**

```toml
# In [workspace.dependencies], change:
rmcp = { version = "1.3", default-features = false, features = ["server", "transport-io", "transport-streamable-http-server-session", "macros"] }
```

- [ ] **Step 2: Add rand dependency for token generation**

```toml
# In [workspace.dependencies], add:
rand = "0.9"
base64 = "0.22"
```

- [ ] **Step 3: Verify workspace compiles**

Run: `cargo check --workspace`
Expected: PASS (no new code yet, just features)

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add rmcp HTTP transport + rand/base64 deps for OpenShell migration"
```

---

### Task 2: Rewrite credential functions from `.claude.json` to `.mcp.json`

**Files:**
- Modify: `crates/rightclaw/src/mcp/credentials.rs`

- [ ] **Step 1: Write failing tests for flat `.mcp.json` structure**

Replace all existing tests. The new functions operate on a flat `mcpServers` structure without `projects.<key>` nesting. The `agent_path_key` parameter is removed from all functions.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // --- add_http_server tests ---

    #[test]
    fn add_creates_mcp_json_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");

        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["notion"]["type"], "http");
        assert_eq!(content["mcpServers"]["notion"]["url"], "https://mcp.notion.com/mcp");
    }

    #[test]
    fn add_merges_into_existing_mcp_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {
                    "rightmemory": { "type": "http", "url": "http://localhost:8100/mcp" }
                }
            })).unwrap(),
        ).unwrap();

        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["notion"]["url"], "https://mcp.notion.com/mcp");
        assert_eq!(content["mcpServers"]["rightmemory"]["url"], "http://localhost:8100/mcp");
    }

    // --- remove_http_server tests ---

    #[test]
    fn remove_deletes_named_server() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();
        add_http_server(&path, "linear", "https://mcp.linear.app/mcp").unwrap();

        remove_http_server(&path, "notion").unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content["mcpServers"]["notion"].is_null());
        assert_eq!(content["mcpServers"]["linear"]["url"], "https://mcp.linear.app/mcp");
    }

    #[test]
    fn remove_returns_not_found_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();

        let err = remove_http_server(&path, "nonexistent").unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }

    // --- list_http_servers tests ---

    #[test]
    fn list_returns_servers_sorted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        add_http_server(&path, "zebra", "https://zebra.example.com/mcp").unwrap();
        add_http_server(&path, "apple", "https://apple.example.com/mcp").unwrap();

        let servers = list_http_servers(&path).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].0, "apple");
        assert_eq!(servers[1].0, "zebra");
    }

    #[test]
    fn list_returns_empty_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.mcp.json");
        let servers = list_http_servers(&path).unwrap();
        assert!(servers.is_empty());
    }

    // --- set_server_header tests ---

    #[test]
    fn set_header_adds_authorization() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        add_http_server(&path, "notion", "https://mcp.notion.com/mcp").unwrap();

        set_server_header(&path, "notion", "Authorization", "Bearer tok-abc").unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            content["mcpServers"]["notion"]["headers"]["Authorization"],
            "Bearer tok-abc"
        );
    }

    #[test]
    fn set_header_returns_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, "{}").unwrap();

        let err = set_server_header(&path, "ghost", "Authorization", "Bearer x").unwrap_err();
        assert!(matches!(err, CredentialError::ServerNotFound(_)));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib mcp::credentials -- --nocapture`
Expected: FAIL — functions don't exist yet

- [ ] **Step 3: Implement new credential functions**

Replace the function bodies in `crates/rightclaw/src/mcp/credentials.rs`. Keep `CredentialError` and `write_json_atomic` unchanged. Replace the four public functions:

```rust
/// Add an HTTP MCP server to .mcp.json under `mcpServers.<name>`.
pub fn add_http_server(
    mcp_json_path: &Path,
    server_name: &str,
    url: &str,
) -> Result<(), CredentialError> {
    let mut root = read_mcp_json(mcp_json_path)?;
    let servers = ensure_mcp_servers(&mut root)?;
    servers.insert(
        server_name.to_string(),
        json!({ "type": "http", "url": url }),
    );
    write_json_atomic(mcp_json_path, &root)
}

/// Remove an HTTP MCP server from .mcp.json.
pub fn remove_http_server(
    mcp_json_path: &Path,
    server_name: &str,
) -> Result<(), CredentialError> {
    let mut root = read_mcp_json(mcp_json_path)?;
    let servers = ensure_mcp_servers(&mut root)?;
    if servers.remove(server_name).is_none() {
        return Err(CredentialError::ServerNotFound(server_name.to_string()));
    }
    write_json_atomic(mcp_json_path, &root)
}

/// List all HTTP MCP servers from .mcp.json.
pub fn list_http_servers(
    mcp_json_path: &Path,
) -> Result<Vec<(String, String)>, CredentialError> {
    let root = read_mcp_json(mcp_json_path)?;
    let servers = match root.get("mcpServers").and_then(|s| s.as_object()) {
        Some(s) => s,
        None => return Ok(vec![]),
    };
    let mut result: Vec<(String, String)> = servers
        .iter()
        .filter_map(|(name, entry)| {
            let url = entry.get("url")?.as_str()?;
            Some((name.clone(), url.to_string()))
        })
        .collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

/// Set a header on an HTTP MCP server in .mcp.json.
pub fn set_server_header(
    mcp_json_path: &Path,
    server_name: &str,
    header_name: &str,
    header_value: &str,
) -> Result<(), CredentialError> {
    let mut root = read_mcp_json(mcp_json_path)?;
    let servers = ensure_mcp_servers(&mut root)?;
    let server = servers
        .get_mut(server_name)
        .ok_or_else(|| CredentialError::ServerNotFound(server_name.to_string()))?;
    let headers = server
        .as_object_mut()
        .ok_or(CredentialError::InvalidPath)?
        .entry("headers")
        .or_insert_with(|| json!({}));
    headers
        .as_object_mut()
        .ok_or(CredentialError::InvalidPath)?
        .insert(header_name.to_string(), json!(header_value));
    write_json_atomic(mcp_json_path, &root)
}
```

Also rename the internal helper:

```rust
/// Read and parse .mcp.json. Returns empty object if file absent.
fn read_mcp_json(path: &Path) -> Result<serde_json::Value, CredentialError> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let content = std::fs::read_to_string(path)?;
    let root: serde_json::Value = serde_json::from_str(&content)?;
    Ok(root)
}

/// Ensure mcpServers key exists as object, return mutable ref.
fn ensure_mcp_servers(
    root: &mut serde_json::Value,
) -> Result<&mut serde_json::Map<String, serde_json::Value>, CredentialError> {
    let obj = root.as_object_mut().ok_or(CredentialError::InvalidPath)?;
    obj.entry("mcpServers").or_insert_with(|| json!({}));
    obj.get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .ok_or(CredentialError::InvalidPath)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib mcp::credentials -- --nocapture`
Expected: all PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/mcp/credentials.rs
git commit -m "refactor: rewrite credential functions for flat .mcp.json structure"
```

---

### Task 3: Update callers of old credential functions

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server.rs` (lines 274-412 — mcp_add, mcp_remove, mcp_list, mcp_auth)
- Modify: `crates/bot/src/telegram/oauth_callback.rs` (lines 24, 206-222)
- Modify: `crates/bot/src/telegram/handler.rs` (if it calls credential functions)

- [ ] **Step 1: Update memory_server.rs mcp_add/mcp_remove/mcp_list**

In `mcp_add()`: replace `add_http_server_to_claude_json()` with `add_http_server()`. Remove `agent_path_key` computation.
In `mcp_remove()`: replace `remove_http_server_from_claude_json()` with `remove_http_server()`.
In `mcp_list()`: replace `list_http_servers_from_claude_json()` with `list_http_servers()`.

Change the path from `.claude.json` to `.mcp.json`:
```rust
// Old:
let claude_json_path = self.agent_dir.join(".claude.json");
// New:
let mcp_json_path = self.agent_dir.join(".mcp.json");
```

- [ ] **Step 2: Update oauth_callback.rs**

In `OAuthCallbackState`: rename `claude_json_path` → `mcp_json_path`, remove `agent_path_key`.

In `complete_oauth_flow()`:
```rust
// Old:
add_http_server_to_claude_json(&cb_state.claude_json_path, &cb_state.agent_path_key, ...)?;
set_server_header(&cb_state.claude_json_path, &cb_state.agent_path_key, ...)?;

// New:
add_http_server(&cb_state.mcp_json_path, &pending.server_name, &pending.server_url)?;
set_server_header(&cb_state.mcp_json_path, &pending.server_name, "Authorization", &format!("Bearer {}", token_resp.access_token))?;
```

- [ ] **Step 3: Update bot/lib.rs where OAuthCallbackState is constructed**

Find where `OAuthCallbackState` is built (around line 163-170 of `crates/bot/src/lib.rs`) and update field names.

- [ ] **Step 4: Verify compilation**

Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 5: Run existing tests**

Run: `cargo test --workspace`
Expected: all PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server.rs crates/bot/src/telegram/oauth_callback.rs crates/bot/src/lib.rs
git commit -m "refactor: update all callers to use flat .mcp.json credential functions"
```

---

### Task 4: Convert rightmemory to HTTP MCP server

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server.rs` (lines 462-508 — `run_memory_server()`)
- Modify: `crates/rightclaw-cli/Cargo.toml` (add rand, base64)

- [ ] **Step 1: Write failing test for HTTP server with Bearer auth**

Create test in `crates/rightclaw-cli/src/memory_server_tests.rs` (or add to existing):

```rust
#[tokio::test]
async fn http_server_rejects_missing_bearer() {
    // Start HTTP memory server on random port
    // Send request without Authorization header
    // Expect 401 Unauthorized
}

#[tokio::test]
async fn http_server_rejects_invalid_bearer() {
    // Start HTTP memory server on random port
    // Send request with wrong Bearer token
    // Expect 401 Unauthorized
}

#[tokio::test]
async fn http_server_accepts_valid_bearer() {
    // Start HTTP memory server on random port with known token→agent mapping
    // Send valid MCP initialize request with correct Bearer
    // Expect 200 OK
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-cli --lib memory_server -- http_server --nocapture`
Expected: FAIL

- [ ] **Step 3: Add HTTP server mode to memory_server.rs**

Add a new function `run_memory_server_http()` alongside the existing `run_memory_server()`. The existing stdio function stays for backward compatibility during migration.

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Token → (agent_name, agent_dir) mapping for multi-agent HTTP mode.
pub type AgentTokenMap = Arc<RwLock<HashMap<String, (String, std::path::PathBuf)>>>;

/// Run the MCP memory server as HTTP on the given port.
///
/// Each request must include `Authorization: Bearer <token>`.
/// Token is looked up in `token_map` to determine which agent's memory.db to use.
pub async fn run_memory_server_http(
    port: u16,
    token_map: AgentTokenMap,
    rightclaw_home: std::path::PathBuf,
) -> miette::Result<()> {
    use rmcp::transport::streamable_http_server::StreamableHttpService;

    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let app = StreamableHttpService::new(move |req| {
        // Extract Bearer token from request
        // Look up agent in token_map
        // Create MemoryServer for that agent
        // Handle MCP request
    });

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| miette::miette!("bind failed on port {port}: {e:#}"))?;

    tracing::info!(port, "rightmemory HTTP server listening");

    axum::serve(listener, app)
        .await
        .map_err(|e| miette::miette!("HTTP server error: {e:#}"))
}
```

Note: The exact integration with rmcp's `StreamableHttpService` needs to follow the rmcp docs. Use context7 MCP tool to fetch current rmcp documentation for the `transport-streamable-http-server-session` feature during implementation.

- [ ] **Step 4: Add CLI subcommand for HTTP mode**

In `crates/rightclaw-cli/src/main.rs`, update the `memory-server` subcommand to accept `--http --port <PORT>`:

```rust
/// MCP memory server
MemoryServer {
    /// Run as HTTP server instead of stdio
    #[arg(long)]
    http: bool,
    /// Port for HTTP mode (default: 8100)
    #[arg(long, default_value = "8100")]
    port: u16,
    /// Path to agents directory (HTTP mode only)
    #[arg(long)]
    agents_dir: Option<std::path::PathBuf>,
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw-cli --lib memory_server -- http_server --nocapture`
Expected: all PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server.rs crates/rightclaw-cli/src/main.rs crates/rightclaw-cli/Cargo.toml
git commit -m "feat: add HTTP mode to rightmemory MCP server with per-agent Bearer auth"
```

---

### Task 5: Generate per-agent Bearer tokens and update `.mcp.json` generation

**Files:**
- Modify: `crates/rightclaw/src/codegen/mcp_config.rs`
- Modify: `crates/rightclaw-cli/src/main.rs` (cmd_up, around line 876)

- [ ] **Step 1: Write failing test for HTTP rightmemory entry in `.mcp.json`**

In `crates/rightclaw/src/codegen/mcp_config.rs` tests:

```rust
#[test]
fn generates_http_rightmemory_entry() {
    let dir = tempdir().unwrap();
    let token = "test-bearer-token-abc123";

    generate_mcp_config_http(
        dir.path(),
        "brain",
        "http://host.docker.internal:8100/mcp",
        token,
        None, // no chrome
    ).unwrap();

    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(content["mcpServers"]["rightmemory"]["type"], "http");
    assert_eq!(content["mcpServers"]["rightmemory"]["url"], "http://host.docker.internal:8100/mcp");
    assert_eq!(
        content["mcpServers"]["rightmemory"]["headers"]["Authorization"],
        "Bearer test-bearer-token-abc123"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib codegen::mcp_config -- generates_http --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement `generate_mcp_config_http()`**

Add new function alongside existing `generate_mcp_config()`:

```rust
/// Generate .mcp.json with rightmemory as HTTP MCP server entry.
///
/// Used when agents run inside OpenShell sandbox and connect to host rightmemory via HTTP.
pub fn generate_mcp_config_http(
    agent_path: &Path,
    agent_name: &str,
    rightmemory_url: &str,
    bearer_token: &str,
    chrome_config: Option<&ChromeConfig>,
) -> miette::Result<()> {
    let mcp_path = agent_path.join(".mcp.json");

    let mut root: serde_json::Value = if mcp_path.exists() {
        let content = std::fs::read_to_string(&mcp_path)
            .map_err(|e| miette::miette!("failed to read .mcp.json: {e:#}"))?;
        serde_json::from_str(&content)
            .map_err(|e| miette::miette!("failed to parse .mcp.json: {e:#}"))?
    } else {
        serde_json::json!({})
    };

    let obj = root.as_object_mut()
        .ok_or_else(|| miette::miette!(".mcp.json root is not a JSON object"))?;
    if !obj.contains_key("mcpServers") {
        obj.insert("mcpServers".to_string(), serde_json::json!({}));
    }
    let servers = obj.get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| miette::miette!("mcpServers is not a JSON object"))?;

    servers.insert(
        "rightmemory".to_string(),
        serde_json::json!({
            "type": "http",
            "url": rightmemory_url,
            "headers": {
                "Authorization": format!("Bearer {bearer_token}")
            }
        }),
    );

    // Chrome devtools stays as stdio — not available inside sandbox.
    // Omit chrome_config for now (sandbox doesn't have chrome).

    let output = serde_json::to_string_pretty(&root)
        .map_err(|e| miette::miette!("failed to serialize .mcp.json: {e:#}"))?;
    std::fs::write(&mcp_path, output)
        .map_err(|e| miette::miette!("failed to write .mcp.json: {e:#}"))?;

    Ok(())
}
```

- [ ] **Step 4: Add token generation helper**

In `crates/rightclaw/src/codegen/mcp_config.rs`:

```rust
/// Generate a random 32-byte Bearer token, base64url-encoded.
pub fn generate_agent_token() -> String {
    use rand::Rng as _;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rightclaw --lib codegen::mcp_config -- --nocapture`
Expected: all PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/mcp_config.rs
git commit -m "feat: generate HTTP rightmemory entry with per-agent Bearer token"
```

---

### Task 6: Create OpenShell sandbox module

**Files:**
- Create: `crates/rightclaw/src/codegen/sandbox.rs`
- Modify: `crates/rightclaw/src/codegen/mod.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_name_is_deterministic() {
        assert_eq!(sandbox_name("brain"), "rightclaw-brain");
        assert_eq!(sandbox_name("worker"), "rightclaw-worker");
    }

    #[test]
    fn upload_command_builds_correctly() {
        let cmd = build_upload_command("rightclaw-brain", "/host/file.txt", "/sandbox/file.txt");
        assert_eq!(cmd[0], "openshell");
        assert_eq!(cmd[1], "sandbox");
        assert_eq!(cmd[2], "upload");
        assert_eq!(cmd[3], "rightclaw-brain");
        assert_eq!(cmd[4], "/host/file.txt");
        assert_eq!(cmd[5], "/sandbox/file.txt");
    }

    #[test]
    fn exec_command_builds_correctly() {
        let cmd = build_exec_command("rightclaw-brain", &["claude", "-p", "--", "hello"]);
        assert_eq!(cmd[0], "openshell");
        assert_eq!(cmd[1], "sandbox");
        assert_eq!(cmd[2], "exec");
        assert_eq!(cmd[3], "rightclaw-brain");
        assert_eq!(cmd[4], "--");
        assert_eq!(cmd[5], "claude");
        assert_eq!(cmd[6], "-p");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib codegen::sandbox -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement sandbox module**

```rust
//! OpenShell sandbox lifecycle: create, upload, exec, delete.

use std::path::Path;
use std::process::Stdio;

/// Generate deterministic sandbox name from agent name.
pub fn sandbox_name(agent_name: &str) -> String {
    format!("rightclaw-{agent_name}")
}

/// Build the openshell upload command args.
pub fn build_upload_command(sandbox: &str, host_path: &str, sandbox_path: &str) -> Vec<String> {
    vec![
        "openshell".into(),
        "sandbox".into(),
        "upload".into(),
        sandbox.into(),
        host_path.into(),
        sandbox_path.into(),
    ]
}

/// Build the openshell sandbox exec command args.
pub fn build_exec_command(sandbox: &str, cmd: &[&str]) -> Vec<String> {
    let mut args = vec![
        "openshell".into(),
        "sandbox".into(),
        "exec".into(),
        sandbox.into(),
        "--".into(),
    ];
    args.extend(cmd.iter().map(|s| (*s).to_string()));
    args
}

/// Create an OpenShell sandbox with the given policy.
pub async fn create_sandbox(
    name: &str,
    policy_path: &Path,
) -> miette::Result<()> {
    let status = tokio::process::Command::new("openshell")
        .args(["sandbox", "create", "--policy"])
        .arg(policy_path)
        .args(["--name", name, "--", "sleep", "infinity"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .await
        .map_err(|e| miette::miette!("openshell sandbox create failed: {e:#}"))?;

    if !status.success() {
        return Err(miette::miette!("openshell sandbox create exited with {status}"));
    }
    tracing::info!(sandbox = name, "sandbox created");
    Ok(())
}

/// Upload a file from host into a running sandbox.
pub async fn upload_file(
    sandbox: &str,
    host_path: &Path,
    sandbox_path: &str,
) -> miette::Result<()> {
    let output = tokio::process::Command::new("openshell")
        .args(["sandbox", "upload", sandbox])
        .arg(host_path)
        .arg(sandbox_path)
        .output()
        .await
        .map_err(|e| miette::miette!("openshell upload failed: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("openshell upload failed: {stderr}"));
    }
    Ok(())
}

/// Execute a command inside a running sandbox, capturing output.
pub async fn exec_in_sandbox(
    sandbox: &str,
    cmd: &[&str],
    timeout_secs: u64,
) -> miette::Result<std::process::Output> {
    let child = tokio::process::Command::new("openshell")
        .args(["sandbox", "exec", sandbox, "--"])
        .args(cmd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| miette::miette!("openshell exec spawn failed: {e:#}"))?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| miette::miette!("openshell exec timed out after {timeout_secs}s"))?
    .map_err(|e| miette::miette!("openshell exec failed: {e:#}"))?;

    Ok(output)
}

/// Delete a sandbox.
pub async fn delete_sandbox(name: &str) -> miette::Result<()> {
    let status = tokio::process::Command::new("openshell")
        .args(["sandbox", "delete", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .await
        .map_err(|e| miette::miette!("openshell sandbox delete failed: {e:#}"))?;

    if !status.success() {
        tracing::warn!(sandbox = name, "sandbox delete returned non-zero (may already be gone)");
    }
    tracing::info!(sandbox = name, "sandbox deleted");
    Ok(())
}
```

- [ ] **Step 4: Register module in mod.rs**

Add to `crates/rightclaw/src/codegen/mod.rs`:
```rust
pub mod sandbox;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rightclaw --lib codegen::sandbox -- --nocapture`
Expected: all PASS (unit tests for command building, not integration)

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/sandbox.rs crates/rightclaw/src/codegen/mod.rs
git commit -m "feat: add OpenShell sandbox lifecycle module (create/upload/exec/delete)"
```

---

### Task 7: Create OpenShell policy generator

**Files:**
- Create: `crates/rightclaw/src/codegen/policy.rs`
- Modify: `crates/rightclaw/src/codegen/mod.rs`
- Create: `templates/right/policy.yaml`

- [ ] **Step 1: Write failing test for policy generation**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_policy_with_rightmemory_port() {
        let policy = generate_policy(8100, &[]);
        assert!(policy.contains("host.docker.internal"));
        assert!(policy.contains("8100"));
        assert!(policy.contains("api.anthropic.com"));
        assert!(policy.contains("hard_requirement"));
    }

    #[test]
    fn adds_external_mcp_domains() {
        let policy = generate_policy(8100, &[
            ("notion", "mcp.notion.com"),
            ("linear", "mcp.linear.app"),
        ]);
        assert!(policy.contains("mcp.notion.com"));
        assert!(policy.contains("mcp.linear.app"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib codegen::policy -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement policy generator**

```rust
//! Generate OpenShell policy.yaml from agent configuration.

/// Generate an OpenShell policy YAML string.
///
/// `rightmemory_port`: TCP port for the host-side rightmemory HTTP server.
/// `external_mcp_servers`: (name, domain) pairs for external MCP servers the agent needs.
pub fn generate_policy(
    rightmemory_port: u16,
    external_mcp_servers: &[(&str, &str)],
) -> String {
    let mut policy = format!(
        r#"version: 1

filesystem_policy:
  include_workdir: true
  read_only:
    - /usr
    - /lib
    - /lib64
    - /etc
    - /proc
    - /dev/urandom
    - /dev/null
  read_write:
    - /tmp
    - /sandbox

landlock:
  compatibility: hard_requirement

process:
  run_as_user: sandbox
  run_as_group: sandbox

network_policies:
  anthropic_api:
    endpoints:
      - host: "api.anthropic.com"
        port: 443
        protocol: rest
        access: full
      - host: "statsig.anthropic.com"
        port: 443
        protocol: rest
        access: full
    binaries:
      - path: "/sandbox/**"

  rightmemory:
    endpoints:
      - host: "host.docker.internal"
        port: {rightmemory_port}
        protocol: rest
        access: full
    binaries:
      - path: "/sandbox/**"
"#
    );

    for (name, domain) in external_mcp_servers {
        policy.push_str(&format!(
            r#"
  {name}:
    endpoints:
      - host: "{domain}"
        port: 443
        protocol: rest
        access: full
    binaries:
      - path: "/sandbox/**"
"#
        ));
    }

    policy
}
```

- [ ] **Step 4: Register module**

Add to `crates/rightclaw/src/codegen/mod.rs`:
```rust
pub mod policy;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rightclaw --lib codegen::policy -- --nocapture`
Expected: all PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/policy.rs crates/rightclaw/src/codegen/mod.rs
git commit -m "feat: add OpenShell policy.yaml generator with dynamic MCP domains"
```

---

### Task 8: Simplify settings.rs

**Files:**
- Modify: `crates/rightclaw/src/codegen/settings.rs`
- Modify: `crates/rightclaw/src/codegen/settings_tests.rs`

- [ ] **Step 1: Write test for minimal settings**

Replace tests in `settings_tests.rs`:

```rust
#[test]
fn generates_minimal_settings() {
    let settings = generate_settings_minimal();
    let obj = settings.as_object().unwrap();
    assert_eq!(obj["skipDangerousModePermissionPrompt"], true);
    assert_eq!(obj["autoMemoryEnabled"], false);
    // Must NOT contain sandbox key
    assert!(obj.get("sandbox").is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib codegen::settings -- minimal --nocapture`
Expected: FAIL

- [ ] **Step 3: Add `generate_settings_minimal()` function**

```rust
/// Generate minimal settings.json for agents inside OpenShell sandbox.
///
/// OpenShell handles all sandboxing — no CC sandbox config needed.
/// Only CC behavioral flags remain.
pub fn generate_settings_minimal() -> serde_json::Value {
    serde_json::json!({
        "skipDangerousModePermissionPrompt": true,
        "autoMemoryEnabled": false,
        "spinnerTipsEnabled": false,
        "prefersReducedMotion": true,
    })
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rightclaw --lib codegen::settings -- --nocapture`
Expected: all PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/settings.rs crates/rightclaw/src/codegen/settings_tests.rs
git commit -m "feat: add minimal settings generator for OpenShell-sandboxed agents"
```

---

### Task 9: Delete legacy symlink code

**Files:**
- Modify: `crates/rightclaw/src/codegen/claude_json.rs`
- Modify: `crates/rightclaw/src/codegen/mod.rs`

- [ ] **Step 1: Delete `create_plugins_symlink()` and its tests**

Remove the function (lines 116-155) and all `test_plugins_*` tests from `claude_json.rs`.

- [ ] **Step 2: Replace `create_credential_symlink()` with `copy_credentials()`**

```rust
/// Copy host credentials into agent staging directory.
///
/// Copies `~/.claude/.credentials.json` to `staging_dir/.claude/.credentials.json`.
/// No symlink — prevents CC from discovering the host's real ~/.claude/ path.
pub fn copy_credentials(staging_dir: &Path, host_home: &Path) -> miette::Result<()> {
    let host_creds = host_home.join(".claude").join(".credentials.json");
    let target_dir = staging_dir.join(".claude");
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| miette::miette!("failed to create .claude dir: {e:#}"))?;
    let target = target_dir.join(".credentials.json");

    if host_creds.exists() {
        std::fs::copy(&host_creds, &target)
            .map_err(|e| miette::miette!("failed to copy credentials: {e:#}"))?;
        tracing::debug!("copied credentials to staging");
    } else {
        tracing::warn!("no OAuth credentials at {} — agent needs ANTHROPIC_API_KEY", host_creds.display());
    }
    Ok(())
}
```

- [ ] **Step 3: Update mod.rs exports**

Remove `create_plugins_symlink` from pub use. Replace `create_credential_symlink` with `copy_credentials`.

- [ ] **Step 4: Verify compilation**

Run: `cargo check --workspace`
Expected: PASS (callers updated in Task 10)

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/claude_json.rs crates/rightclaw/src/codegen/mod.rs
git commit -m "refactor: delete plugins symlink, replace credential symlink with copy"
```

---

### Task 10: Create OAuth state persistence module

**Files:**
- Create: `crates/rightclaw/src/mcp/refresh.rs`
- Modify: `crates/rightclaw/src/mcp/mod.rs`

- [ ] **Step 1: Write failing tests for OAuth state serde**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_oauth_state() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("oauth-state.json");

        let entry = OAuthServerState {
            refresh_token: Some("rt-abc".into()),
            token_endpoint: "https://accounts.notion.com/oauth/token".into(),
            client_id: "client123".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            server_url: "https://mcp.notion.com/mcp".into(),
        };

        let mut state = OAuthState::default();
        state.servers.insert("notion".into(), entry);
        save_oauth_state(&path, &state).unwrap();

        let loaded = load_oauth_state(&path).unwrap();
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers["notion"].client_id, "client123");
        assert_eq!(loaded.servers["notion"].refresh_token.as_deref(), Some("rt-abc"));
    }

    #[test]
    fn load_returns_empty_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.json");
        let state = load_oauth_state(&path).unwrap();
        assert!(state.servers.is_empty());
    }

    #[test]
    fn refresh_due_in_returns_correct_duration() {
        let entry = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: "https://example.com/token".into(),
            client_id: "c".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(30),
            server_url: "https://example.com/mcp".into(),
        };
        // Should refresh 10 minutes before expiry = 20 minutes from now
        let due = refresh_due_in(&entry);
        assert!(due.as_secs() > 1100 && due.as_secs() < 1300); // ~20 min
    }

    #[test]
    fn refresh_due_in_returns_zero_when_expired() {
        let entry = OAuthServerState {
            refresh_token: Some("rt".into()),
            token_endpoint: "https://example.com/token".into(),
            client_id: "c".into(),
            client_secret: None,
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
            server_url: "https://example.com/mcp".into(),
        };
        let due = refresh_due_in(&entry);
        assert_eq!(due, std::time::Duration::ZERO);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib mcp::refresh -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Add chrono dependency**

In root `Cargo.toml` workspace dependencies:
```toml
chrono = { version = "0.4", features = ["serde"] }
```

In `crates/rightclaw/Cargo.toml`:
```toml
chrono.workspace = true
```

- [ ] **Step 4: Implement OAuth state persistence**

```rust
//! OAuth token refresh: state persistence and background scheduler.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Refresh margin: refresh token 10 minutes before expiry.
const REFRESH_MARGIN: Duration = Duration::from_secs(600);

/// Maximum retry attempts before notifying user.
const MAX_RETRIES: u32 = 3;

/// Per-server OAuth state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthServerState {
    pub refresh_token: Option<String>,
    pub token_endpoint: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub server_url: String,
}

/// All OAuth state for an agent.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct OAuthState {
    pub servers: HashMap<String, OAuthServerState>,
}

/// Message sent from OAuth callback to refresh scheduler.
#[derive(Debug)]
pub struct RefreshEntry {
    pub server_name: String,
    pub state: OAuthServerState,
}

pub fn load_oauth_state(path: &Path) -> miette::Result<OAuthState> {
    if !path.exists() {
        return Ok(OAuthState::default());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("failed to read oauth state: {e:#}"))?;
    let state: OAuthState = serde_json::from_str(&content)
        .map_err(|e| miette::miette!("failed to parse oauth state: {e:#}"))?;
    Ok(state)
}

pub fn save_oauth_state(path: &Path, state: &OAuthState) -> miette::Result<()> {
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| miette::miette!("failed to serialize oauth state: {e:#}"))?;
    std::fs::write(path, content)
        .map_err(|e| miette::miette!("failed to write oauth state: {e:#}"))?;
    Ok(())
}

/// Calculate how long until refresh should fire.
///
/// Returns Duration::ZERO if already expired or past margin.
pub fn refresh_due_in(entry: &OAuthServerState) -> Duration {
    let now = chrono::Utc::now();
    let refresh_at = entry.expires_at - chrono::Duration::from_std(REFRESH_MARGIN).unwrap();
    if refresh_at <= now {
        Duration::ZERO
    } else {
        (refresh_at - now).to_std().unwrap_or(Duration::ZERO)
    }
}
```

- [ ] **Step 5: Register module**

In `crates/rightclaw/src/mcp/mod.rs`:
```rust
pub mod refresh;
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p rightclaw --lib mcp::refresh -- --nocapture`
Expected: all PASS

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/mcp/refresh.rs crates/rightclaw/src/mcp/mod.rs Cargo.toml crates/rightclaw/Cargo.toml
git commit -m "feat: add OAuth state persistence + refresh timing for MCP tokens"
```

---

### Task 11: Implement refresh scheduler

**Files:**
- Modify: `crates/rightclaw/src/mcp/refresh.rs`

- [ ] **Step 1: Implement the scheduler function**

Add to `refresh.rs`:

```rust
/// Run the OAuth refresh scheduler.
///
/// Loads state from `oauth_state_path`, schedules refresh for each server.
/// Listens on `rx` for new entries from OAuth callback.
/// On successful refresh: updates Bearer in `mcp_json_path`, re-uploads into sandbox,
/// updates `oauth_state_path`.
pub async fn run_refresh_scheduler(
    oauth_state_path: std::path::PathBuf,
    mcp_json_path: std::path::PathBuf,
    sandbox_name: String,
    mut rx: tokio::sync::mpsc::Receiver<RefreshEntry>,
    notify_tx: tokio::sync::mpsc::Sender<String>, // send error messages for Telegram notification
) {
    let http_client = reqwest::Client::new();

    // Load existing state
    let mut state = match load_oauth_state(&oauth_state_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to load oauth state: {e:#}");
            OAuthState::default()
        }
    };

    // Build initial timer set
    let mut timers: HashMap<String, tokio::time::Instant> = HashMap::new();
    for (name, entry) in &state.servers {
        if entry.refresh_token.is_none() {
            tracing::warn!(server = %name, "no refresh_token — skipping auto-refresh");
            continue;
        }
        let due = refresh_due_in(entry);
        timers.insert(name.clone(), tokio::time::Instant::now() + due);
        tracing::info!(server = %name, due_secs = due.as_secs(), "scheduled refresh");
    }

    loop {
        // Find the next timer to fire
        let next = timers.iter().min_by_key(|(_, &instant)| instant);

        tokio::select! {
            // New entry from OAuth callback
            Some(entry) = rx.recv() => {
                let due = refresh_due_in(&entry.state);
                timers.insert(entry.server_name.clone(), tokio::time::Instant::now() + due);
                state.servers.insert(entry.server_name.clone(), entry.state);
                let _ = save_oauth_state(&oauth_state_path, &state);
                tracing::info!(server = %entry.server_name, due_secs = due.as_secs(), "new refresh scheduled");
            }

            // Timer fires
            _ = async {
                match next {
                    Some((_, &instant)) => tokio::time::sleep_until(instant).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let name = next.unwrap().0.clone();
                let entry = match state.servers.get(&name) {
                    Some(e) => e.clone(),
                    None => continue,
                };

                tracing::info!(server = %name, "refreshing OAuth token");

                match do_refresh(&http_client, &entry, MAX_RETRIES).await {
                    // Returns (updated_state, new_access_token)
                    Ok((new_entry, access_token)) => {
                        // Update Bearer in .mcp.json with the NEW access token
                        if let Err(e) = crate::mcp::credentials::set_server_header(
                            &mcp_json_path,
                            &name,
                            "Authorization",
                            &format!("Bearer {access_token}"),
                        ) {
                            tracing::error!(server = %name, "failed to update .mcp.json: {e:#}");
                        }

                        // Re-upload .mcp.json into sandbox
                        if let Err(e) = crate::codegen::sandbox::upload_file(
                            &sandbox_name,
                            &mcp_json_path,
                            "/sandbox/.mcp.json",
                        ).await {
                            tracing::error!(server = %name, "failed to re-upload .mcp.json: {e:#}");
                        }

                        // Schedule next refresh
                        let due = refresh_due_in(&new_entry);
                        timers.insert(name.clone(), tokio::time::Instant::now() + due);
                        state.servers.insert(name.clone(), new_entry);
                        let _ = save_oauth_state(&oauth_state_path, &state);
                    }
                    Err(e) => {
                        tracing::error!(server = %name, "token refresh failed after retries: {e:#}");
                        timers.remove(&name);
                        let _ = notify_tx.send(format!("OAuth refresh failed for {name}: {e:#}")).await;
                    }
                }
            }
        }
    }
}

/// Attempt token refresh with retries.
/// Returns (updated_state, new_access_token).
async fn do_refresh(
    client: &reqwest::Client,
    entry: &OAuthServerState,
    max_retries: u32,
) -> miette::Result<(OAuthServerState, String)> {
    let refresh_token = entry.refresh_token.as_deref()
        .ok_or_else(|| miette::miette!("no refresh_token"))?;

    let backoffs = [30, 60, 120]; // seconds

    for attempt in 0..max_retries {
        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &entry.client_id),
        ];
        if let Some(ref secret) = entry.client_secret {
            form.push(("client_secret", secret));
        }

        let resp = client
            .post(&entry.token_endpoint)
            .form(&form)
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let token_resp: crate::mcp::oauth::TokenResponse = r.json().await
                    .map_err(|e| miette::miette!("failed to parse token response: {e:#}"))?;

                let expires_at = chrono::Utc::now()
                    + chrono::Duration::seconds(token_resp.expires_in.unwrap_or(3600) as i64);

                let access_token = token_resp.access_token.clone();
                return Ok((OAuthServerState {
                    refresh_token: token_resp.refresh_token.or(entry.refresh_token.clone()),
                    token_endpoint: entry.token_endpoint.clone(),
                    client_id: entry.client_id.clone(),
                    client_secret: entry.client_secret.clone(),
                    expires_at,
                    server_url: entry.server_url.clone(),
                }, access_token));
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                tracing::warn!(attempt, %status, %body, "refresh attempt failed");
            }
            Err(e) => {
                tracing::warn!(attempt, "refresh request error: {e:#}");
            }
        }

        if attempt < max_retries - 1 {
            let delay = backoffs.get(attempt as usize).copied().unwrap_or(120);
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
    }

    Err(miette::miette!("token refresh failed after {max_retries} attempts"))
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/mcp/refresh.rs
git commit -m "feat: implement OAuth refresh scheduler with retry and sandbox re-upload"
```

---

### Task 12: Rewrite `invoke_cc()` for OpenShell sandbox exec

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Update `invoke_cc()` to use `openshell sandbox exec`**

In `crates/bot/src/telegram/worker.rs`, the `invoke_cc()` function (line 338) currently spawns `claude -p` directly. Change it to use `openshell sandbox exec`:

Replace the command construction (lines 373-408) with:

```rust
    let sandbox = crate::codegen::sandbox::sandbox_name(&ctx.agent_name);

    // Build claude -p args for sandbox exec
    let mut claude_args: Vec<String> = vec![
        "claude".into(),
        "-p".into(),
        "--dangerously-skip-permissions".into(),
    ];
    for arg in &cmd_args {
        claude_args.push(arg.clone());
    }
    claude_args.push("--output-format".into());
    claude_args.push("json".into());

    if is_first_call {
        claude_args.push("--agent".into());
        claude_args.push(ctx.agent_name.clone());
    }

    let reply_schema = std::fs::read_to_string(&reply_schema_path)
        .map_err(|e| format_error_reply(-1, &format!("reply-schema.json read failed: {:#}", e)))?;
    claude_args.push("--json-schema".into());
    claude_args.push(reply_schema);

    claude_args.push("--".into());
    claude_args.push(xml.to_string());

    // Build openshell sandbox exec command
    let mut cmd = tokio::process::Command::new("openshell");
    cmd.args(["sandbox", "exec", &sandbox, "--"]);
    for arg in &claude_args {
        cmd.arg(arg);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);
```

Remove `cmd.env("HOME", ...)` and `cmd.env("USE_BUILTIN_RIPGREP", ...)` — these are handled inside the sandbox.
Remove `cmd.current_dir(...)` — cwd is `/sandbox/` inside the container.

- [ ] **Step 2: Add sandbox_name to WorkerContext**

In worker.rs `WorkerContext` struct, it should already have `agent_name`. Verify that `sandbox_name()` can be called from it.

- [ ] **Step 3: Verify compilation**

Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "refactor: invoke_cc uses openshell sandbox exec instead of direct claude -p"
```

---

### Task 13: Rewrite `cmd_up()` for OpenShell lifecycle

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs` (cmd_up, lines 676-1027)

This is the largest single change. The per-agent loop needs to:
1. Generate files into a staging directory
2. Create OpenShell sandbox
3. Upload staging files into sandbox
4. Generate process-compose.yaml with rightmemory HTTP server + bot processes

- [ ] **Step 1: Add staging directory creation and agent token generation**

In the per-agent loop (after line 793), replace the current codegen calls with staging-based flow:

```rust
    // Create staging directory
    let staging_dir = agent.path.join("staging");
    std::fs::create_dir_all(&staging_dir)
        .map_err(|e| miette::miette!("failed to create staging dir: {e:#}"))?;

    // Generate minimal settings.json (OpenShell handles sandbox)
    let settings = rightclaw::codegen::settings::generate_settings_minimal();
    let settings_dir = staging_dir.join(".claude");
    std::fs::create_dir_all(&settings_dir)
        .map_err(|e| miette::miette!("create .claude staging dir: {e:#}"))?;
    std::fs::write(
        settings_dir.join("settings.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    ).map_err(|e| miette::miette!("write staging settings.json: {e:#}"))?;

    // Copy credentials (not symlink)
    rightclaw::codegen::claude_json::copy_credentials(&staging_dir, &host_home)?;

    // Generate .claude.json (trust + onboarding)
    // Need to generate into staging_dir, not agent.path
    // ... adapt generate_agent_claude_json to accept a target dir

    // Generate per-agent Bearer token
    let agent_token = rightclaw::codegen::mcp_config::generate_agent_token();
    std::fs::write(agent.path.join("agent-secret.key"), &agent_token)
        .map_err(|e| miette::miette!("write agent secret: {e:#}"))?;

    // Generate .mcp.json with HTTP rightmemory entry
    let rightmemory_url = format!("http://host.docker.internal:{RIGHTMEMORY_PORT}/mcp");
    rightclaw::codegen::mcp_config::generate_mcp_config_http(
        &staging_dir, &agent.name, &rightmemory_url, &agent_token, None,
    )?;

    // Copy agent identity files to staging
    for file in ["IDENTITY.md", "SOUL.md", "USER.md", "AGENTS.md", "agent.yaml"] {
        let src = agent.path.join(file);
        if src.exists() {
            std::fs::copy(&src, staging_dir.join(file))
                .map_err(|e| miette::miette!("copy {file} to staging: {e:#}"))?;
        }
    }
```

- [ ] **Step 2: Add OpenShell sandbox creation and file upload**

After the per-agent staging loop:

```rust
    // Generate policy
    let external_mcps = vec![]; // TODO: extract from agent's MCP config
    let policy_yaml = rightclaw::codegen::policy::generate_policy(RIGHTMEMORY_PORT, &external_mcps);
    let policy_path = agent.path.join("policy.yaml");
    std::fs::write(&policy_path, &policy_yaml)
        .map_err(|e| miette::miette!("write policy.yaml: {e:#}"))?;

    // Create sandbox
    let sandbox = rightclaw::codegen::sandbox::sandbox_name(&agent.name);
    rightclaw::codegen::sandbox::create_sandbox(&sandbox, &policy_path).await?;

    // Upload staging files
    for entry in std::fs::read_dir(&staging_dir)
        .map_err(|e| miette::miette!("read staging dir: {e:#}"))?
    {
        let entry = entry.map_err(|e| miette::miette!("read staging entry: {e:#}"))?;
        let sandbox_path = format!("/sandbox/{}", entry.file_name().to_string_lossy());
        rightclaw::codegen::sandbox::upload_file(
            &sandbox, &entry.path(), &sandbox_path,
        ).await?;
    }
    // Upload .claude/ subdirectory
    rightclaw::codegen::sandbox::upload_file(
        &sandbox, &staging_dir.join(".claude"), "/sandbox/.claude",
    ).await?;
    rightclaw::codegen::sandbox::upload_file(
        &sandbox, &staging_dir.join(".mcp.json"), "/sandbox/.mcp.json",
    ).await?;
```

- [ ] **Step 3: Update process-compose generation**

Add rightmemory HTTP server as a process-compose entry. Update `generate_process_compose()` or the template to include:

```yaml
rightmemory:
  command: {{ rightmemory_exe }} memory-server --http --port {{ rightmemory_port }} --agents-dir {{ agents_dir }}
  availability:
    restart: on_failure
    backoff_seconds: 5
    max_restarts: 10
```

- [ ] **Step 4: Remove old codegen calls**

Remove from the per-agent loop:
- `generate_settings()` call (replaced by `generate_settings_minimal()`)
- `create_credential_symlink()` call (replaced by `copy_credentials()`)
- `create_plugins_symlink()` call (deleted)
- Old `generate_mcp_config()` call (replaced by `generate_mcp_config_http()`)

- [ ] **Step 5: Verify compilation**

Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat: rewrite cmd_up for OpenShell sandbox lifecycle with staging + upload"
```

---

### Task 14: Rewrite `cmd_down()` for OpenShell cleanup

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs` (cmd_down, lines 1029-1051)

- [ ] **Step 1: Add sandbox deletion to cmd_down**

```rust
async fn cmd_down(home: &Path) -> miette::Result<()> {
    let run_dir = home.join("run");
    let state_path = run_dir.join("state.json");

    let state = rightclaw::runtime::read_state(&state_path).map_err(|_| {
        miette::miette!("No running instance found. Is rightclaw running?")
    })?;

    // Delete OpenShell sandboxes
    for agent in &state.agents {
        let sandbox = rightclaw::codegen::sandbox::sandbox_name(&agent.name);
        if let Err(e) = rightclaw::codegen::sandbox::delete_sandbox(&sandbox).await {
            tracing::warn!(agent = %agent.name, "sandbox cleanup failed: {e:#}");
        }
    }

    // Stop process-compose
    match rightclaw::runtime::PcClient::new(rightclaw::runtime::PC_PORT) {
        Ok(client) => {
            if let Err(e) = client.shutdown().await {
                tracing::warn!("process-compose shutdown failed: {e:#}");
            }
        }
        Err(e) => {
            tracing::warn!("could not connect to process-compose: {e:#}");
        }
    }

    println!("All agents stopped.");
    Ok(())
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat: cmd_down deletes OpenShell sandboxes before stopping process-compose"
```

---

### Task 15: Spawn refresh scheduler in bot dispatch

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Add refresh scheduler spawn in bot entry**

In `crates/bot/src/lib.rs`, before `tokio::select!` (around line 186), spawn the refresh scheduler:

```rust
    // Spawn OAuth refresh scheduler
    let (refresh_tx, refresh_rx) = tokio::sync::mpsc::channel::<rightclaw::mcp::refresh::RefreshEntry>(32);
    let (notify_refresh_tx, mut notify_refresh_rx) = tokio::sync::mpsc::channel::<String>(32);

    let oauth_state_path = agent_dir.join("oauth-state.json");
    let mcp_json_path = agent_dir.join("staging").join(".mcp.json");
    let sandbox = rightclaw::codegen::sandbox::sandbox_name(&agent_name);

    tokio::spawn(rightclaw::mcp::refresh::run_refresh_scheduler(
        oauth_state_path,
        mcp_json_path,
        sandbox,
        refresh_rx,
        notify_refresh_tx,
    ));

    // Forward refresh error notifications to Telegram
    let notify_bot = bot.clone();
    let notify_chat_ids = allowed_chat_ids.clone();
    tokio::spawn(async move {
        while let Some(msg) = notify_refresh_rx.recv().await {
            for &chat_id in &notify_chat_ids {
                let _ = notify_bot.send_message(teloxide::types::ChatId(chat_id), &msg).await;
            }
        }
    });
```

- [ ] **Step 2: Pass `refresh_tx` to OAuthCallbackState**

Add `refresh_tx` to `OAuthCallbackState` so that `complete_oauth_flow()` can notify the scheduler of new tokens.

- [ ] **Step 3: Update `complete_oauth_flow()` to persist state and notify scheduler**

In `oauth_callback.rs`, after writing Bearer token:

```rust
    // Persist OAuth state
    let oauth_entry = rightclaw::mcp::refresh::OAuthServerState {
        refresh_token: token_resp.refresh_token.clone(),
        token_endpoint: pending.token_endpoint.clone(),
        client_id: pending.client_id.clone(),
        client_secret: pending.client_secret.clone(),
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(
            token_resp.expires_in.unwrap_or(3600) as i64
        ),
        server_url: pending.server_url.clone(),
    };

    // Notify refresh scheduler
    let _ = cb_state.refresh_tx.send(rightclaw::mcp::refresh::RefreshEntry {
        server_name: pending.server_name.clone(),
        state: oauth_entry,
    }).await;
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/lib.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/oauth_callback.rs
git commit -m "feat: spawn OAuth refresh scheduler in bot, wire to OAuth callback"
```

---

### Task 16: Update doctor checks

**Files:**
- Modify: `crates/rightclaw/src/doctor.rs`

- [ ] **Step 1: Replace bubblewrap/socat checks with OpenShell/Docker checks**

Find the Linux-only bwrap checks (around lines 63-82) and replace:

```rust
    // OpenShell binary check
    checks.push(check_binary_exists("openshell", Severity::Fail));

    // Docker daemon check
    checks.push(check_docker_running());
```

Add:

```rust
fn check_docker_running() -> DoctorCheck {
    match std::process::Command::new("docker").args(["info"]).output() {
        Ok(output) if output.status.success() => DoctorCheck {
            name: "Docker daemon".into(),
            status: CheckStatus::Pass,
            message: "Docker is running".into(),
            severity: Severity::Fail,
        },
        _ => DoctorCheck {
            name: "Docker daemon".into(),
            status: CheckStatus::Fail,
            message: "Docker daemon not running — OpenShell requires Docker".into(),
            severity: Severity::Fail,
        },
    }
}
```

- [ ] **Step 2: Remove bwrap smoke test**

Delete the `check_bwrap_smoke()` function and its call.

- [ ] **Step 3: Verify compilation**

Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/doctor.rs
git commit -m "refactor: replace bubblewrap doctor checks with OpenShell + Docker"
```

---

### Task 17: Update process-compose template

**Files:**
- Modify: `templates/process-compose.yaml.j2`

- [ ] **Step 1: Add rightmemory HTTP server entry**

Add before the agent loop:

```yaml
  rightmemory:
    command: {{ rightmemory.exe_path }} memory-server --http --port {{ rightmemory.port }}
    environment:
      - RC_AGENTS_DIR={{ rightmemory.agents_dir }}
    availability:
      restart: on_failure
      backoff_seconds: 5
      max_restarts: 10
    shutdown:
      signal: 15
      timeout_seconds: 10
```

- [ ] **Step 2: Remove `RC_TELEGRAM_TOKEN_FILE` from template**

Remove line 16: `- RC_TELEGRAM_TOKEN_FILE={{ agent.token_file }}` and the `{% else %}` branch.
Keep only `RC_TELEGRAM_TOKEN={{ agent.token_inline }}`.

- [ ] **Step 3: Update process-compose generation code**

In `crates/rightclaw/src/codegen/process_compose.rs`, add `RightmemoryProcess` struct to the template context:

```rust
#[derive(Serialize)]
struct RightmemoryProcess {
    exe_path: String,
    port: u16,
    agents_dir: String,
}
```

- [ ] **Step 4: Verify template renders correctly**

Run: `cargo test -p rightclaw --lib codegen::process_compose -- --nocapture`
Expected: PASS (update tests as needed)

- [ ] **Step 5: Commit**

```bash
git add templates/process-compose.yaml.j2 crates/rightclaw/src/codegen/process_compose.rs
git commit -m "feat: add rightmemory HTTP server to process-compose template"
```

---

### Task 18: Remove `telegram_token_file` from agent config

**Files:**
- Modify: `crates/rightclaw/src/agent/types.rs`
- Modify: `crates/rightclaw/src/init.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Remove `telegram_token_file` field from `AgentConfig`**

In `crates/rightclaw/src/agent/types.rs`, remove the `telegram_token_file` field from `AgentConfig`.

- [ ] **Step 2: Remove file-based token resolution from bot**

In `crates/bot/src/telegram/mod.rs`, remove priority 3 (`telegram_token_file`) from `resolve_token()`. Keep priorities 1 (`RC_TELEGRAM_TOKEN` env var), 2 (`RC_TELEGRAM_TOKEN_FILE` env var), and 4 (inline `telegram_token`).

- [ ] **Step 3: Remove `.env` file writing from init.rs**

In `crates/rightclaw/src/init.rs`, remove the block that writes `TELEGRAM_BOT_TOKEN=...` to `.claude/channels/telegram/.env` and appends `telegram_token_file` to agent.yaml.

- [ ] **Step 4: Fix all compilation errors**

Run: `cargo check --workspace`
Fix any remaining references to `telegram_token_file`.

- [ ] **Step 5: Update tests**

Fix tests in init.rs, bot/telegram/mod.rs that reference `telegram_token_file`.

- [ ] **Step 6: Run tests**

Run: `cargo test --workspace`
Expected: all PASS

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/agent/types.rs crates/rightclaw/src/init.rs crates/bot/src/telegram/mod.rs
git commit -m "refactor: remove telegram_token_file — token via RC_TELEGRAM_TOKEN env var only"
```

---

### Task 19: Full workspace build and test

**Files:** None (verification only)

- [ ] **Step 1: Build entire workspace**

Run: `cargo build --workspace`
Expected: PASS

- [ ] **Step 2: Run all tests**

Run: `cargo test --workspace`
Expected: all PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "chore: fix clippy warnings and test failures from OpenShell migration"
```

---

### Task 20: Update init.rs for new agent layout

**Files:**
- Modify: `crates/rightclaw/src/init.rs`

- [ ] **Step 1: Update `init_rightclaw_home()` to create staging dir**

Add staging directory creation for each agent:

```rust
    let staging_dir = agents_dir.join("staging");
    std::fs::create_dir_all(&staging_dir)
        .map_err(|e| miette::miette!("create staging dir: {e:#}"))?;
```

- [ ] **Step 2: Store telegram token in global config**

Instead of writing to per-agent `.env`, store in `~/.rightclaw/config.yaml`:

```rust
    if let Some(token) = telegram_token {
        // Token stored globally, passed via RC_TELEGRAM_TOKEN env var in process-compose
        global_config.telegram_token = Some(token.to_string());
        write_global_config(home, &global_config)?;
    }
```

- [ ] **Step 3: Update tests**

Fix init tests that expect `.claude/channels/telegram/.env` or `telegram_token_file` in agent.yaml.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rightclaw --lib init -- --nocapture`
Expected: all PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/init.rs
git commit -m "refactor: update init for staging dir layout + global telegram token storage"
```

---

### Task 21: Integration test — end-to-end sandbox lifecycle

**Files:**
- Create: `crates/rightclaw-cli/tests/sandbox_lifecycle.rs`

- [ ] **Step 1: Write integration test**

```rust
//! Integration test: requires OpenShell + Docker running.
//! Run with: cargo test --test sandbox_lifecycle -- --ignored

#[tokio::test]
#[ignore] // requires OpenShell + Docker
async fn sandbox_create_upload_exec_delete() {
    use rightclaw::codegen::sandbox::*;

    let sandbox = "rightclaw-integration-test";

    // Create sandbox with minimal policy
    let policy_dir = tempfile::tempdir().unwrap();
    let policy_path = policy_dir.path().join("policy.yaml");
    let policy = rightclaw::codegen::policy::generate_policy(8100, &[]);
    std::fs::write(&policy_path, &policy).unwrap();

    create_sandbox(sandbox, &policy_path).await.unwrap();

    // Upload a test file
    let test_file = policy_dir.path().join("hello.txt");
    std::fs::write(&test_file, "hello from host").unwrap();
    upload_file(sandbox, &test_file, "/sandbox/hello.txt").await.unwrap();

    // Exec cat inside sandbox
    let output = exec_in_sandbox(sandbox, &["cat", "/sandbox/hello.txt"], 10).await.unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello from host");

    // Delete sandbox
    delete_sandbox(sandbox).await.unwrap();
}
```

- [ ] **Step 2: Run test (requires OpenShell)**

Run: `cargo test --test sandbox_lifecycle -- --ignored --nocapture`
Expected: PASS if OpenShell + Docker available

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/tests/sandbox_lifecycle.rs
git commit -m "test: add integration test for OpenShell sandbox lifecycle"
```
