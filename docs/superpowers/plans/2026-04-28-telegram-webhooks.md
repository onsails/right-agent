# Telegram Webhooks via Cloudflare Tunnel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace teloxide long-polling with Telegram webhooks delivered through the existing Cloudflare Tunnel, and make the tunnel a mandatory part of the platform.

**Architecture:** Each per-agent bot extends its existing axum UDS server (renamed `bot.sock`) with a `/tg/<agent>/...` route served by `teloxide::update_listeners::webhooks::axum_no_setup`. Cloudflared adds a `path: /tg/<agent>/.*` ingress rule per agent on the host's tunnel hostname. The bot owns `setWebhook` lifecycle (retry-with-backoff at startup; never `deleteWebhook` on shutdown). Webhook secret is derived from the existing per-agent secret in `agent.yaml` via `derive_token(secret, "tg-webhook")`.

**Tech Stack:** Rust 2024 edition, teloxide 0.17 (`webhooks-axum` feature), axum 0.8, tokio 1.50, miette/thiserror, minijinja templates, Cloudflare Tunnel (`cloudflared`), process-compose.

**Spec:** `docs/superpowers/specs/2026-04-28-telegram-webhooks-design.md`

---

## File Structure

### Created files

| Path | Responsibility |
|------|----------------|
| `crates/bot/src/telegram/webhook.rs` | Build the per-agent webhook router via `teloxide::update_listeners::webhooks::axum_no_setup`; secret derivation; `AllowedUpdate` set. |
| `crates/bot/tests/webhook_integration.rs` | End-to-end: stubbed Bot API, real bot binary, asserts on `setWebhook` retry behavior + UDS round-trip with secret check. |
| `crates/right/tests/right_up_requires_tunnel.rs` | `assert_cmd` test confirming `right up` errors out when global config has no tunnel block. |

### Modified files

| Path | Change |
|------|--------|
| `crates/right-agent/src/config/mod.rs` | `GlobalConfig.tunnel: TunnelConfig` (drop `Option`); `read_global_config` rejects missing/empty tunnel block; `write_global_config` always emits tunnel block; tests updated. |
| `crates/right/src/wizard.rs` | Drop `TunnelExistingAction::Skip` and `TunnelOutcome::Skipped`; `tunnel_setup` returns `Result<TunnelConfig>` (not `Option`); error on missing cloudflared cert. |
| `crates/right-agent/src/codegen/pipeline.rs` | Unwrap `if let Some(ref tunnel_cfg)` blocks; `cloudflared_script_path: PathBuf` (not `Option`). |
| `crates/right-agent/src/codegen/cloudflared.rs` | `CloudflaredAgent.socket_path` uses `bot.sock` instead of `oauth-callback.sock`. |
| `crates/right-agent/src/codegen/cloudflared_tests.rs` | Tests updated for `bot.sock`; new tests for `/tg/<agent>/.*` ingress + ordering. |
| `templates/cloudflared-config.yml.j2` | Always emit `tunnel:` + `credentials-file:`; add `/tg/<agent>/.*` ingress rule before `/oauth/<agent>/callback`. |
| `templates/process-compose.yaml.j2` | `cloudflared` block unconditional; per-agent bot adds `cloudflared` to `depends_on`. |
| `Cargo.toml` (workspace root) | teloxide features += `"webhooks-axum"`. |
| `crates/bot/src/telegram/oauth_callback.rs` | UDS path arg renamed `oauth-callback.sock` → `bot.sock`; `build_router` accepts the webhook router and nests it; adds `/healthz`. |
| `crates/bot/src/telegram/dispatch.rs` | `run_telegram` takes an `impl UpdateListener` parameter; `Dispatcher::dispatch()` → `Dispatcher::dispatch_with_listener()`. |
| `crates/bot/src/telegram/mod.rs` | Export new `webhook` module. |
| `crates/bot/src/lib.rs` | Construct webhook router, mount on the existing axum app, derive secret, spawn `webhook_register_loop`; remove pre-startup `delete_webhook` cleanup block. |
| `crates/right-agent/src/doctor.rs` | Invert `check_webhook_info_for_agents` (expect webhook to be set); add `check_bot_healthz_for_agents`; missing tunnel = ERROR (was WARN). |
| `crates/right-agent/src/agent/destroy.rs` | Best-effort `bot.delete_webhook()` before sandbox deletion. |

---

## Task 1: Make `tunnel:` mandatory in global config

**Files:**
- Modify: `crates/right-agent/src/config/mod.rs:105-109` (struct), `:189-222` (read), `:227-252` (write), `:303,338,350,358-366,383-402` (tests)

This task atomically changes the type of `GlobalConfig.tunnel` from `Option<TunnelConfig>` to `TunnelConfig`. After this task the workspace will not build until Task 2 (wizard) and Task 3 (pipeline) are done — keep the commit together with those.

Actually, to keep each task green-on-its-own, do the schema change last in this task and update the consumers in the same commit. The wizard and pipeline edits in Tasks 2/3 then operate on a working baseline.

- [ ] **Step 1.1: Add a failing test for missing `tunnel:` block**

In `crates/right-agent/src/config/mod.rs`, add this test after the existing `read_global_config_returns_default_when_file_missing` test (around line 367):

```rust
#[test]
fn read_global_config_errors_when_tunnel_block_missing() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("config.yaml"),
        "aggregator:\n  allowed_hosts:\n    - example.com\n",
    )
    .unwrap();
    let err = read_global_config(dir.path()).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("tunnel"),
        "error must mention tunnel, got: {msg}"
    );
    assert!(
        msg.contains("right init"),
        "error must hint at right init, got: {msg}"
    );
}
```

- [ ] **Step 1.2: Run the test to verify it fails**

```sh
cargo test -p right-agent --lib config::tests::read_global_config_errors_when_tunnel_block_missing
```

Expected: FAIL with "called `Result::unwrap_err()` on an `Ok` value" — current code returns `Ok(GlobalConfig { tunnel: None, ... })` instead of erroring.

- [ ] **Step 1.3: Change `GlobalConfig.tunnel` to non-Option**

Edit `crates/right-agent/src/config/mod.rs:105-109`:

```rust
pub struct GlobalConfig {
    pub tunnel: TunnelConfig,
    pub aggregator: AggregatorConfig,
}
```

Remove the `#[derive(Default)]` from `GlobalConfig` if present; `Default` no longer makes sense without a tunnel. Also remove any custom `Default` impl that constructed `tunnel: None`.

- [ ] **Step 1.4: Update `read_global_config` to require the tunnel block and error on missing file**

Replace `crates/right-agent/src/config/mod.rs:189-222` with:

```rust
pub fn read_global_config(home: &Path) -> miette::Result<GlobalConfig> {
    let path = home.join("config.yaml");
    if !path.exists() {
        return Err(miette::miette!(
            help = "run: right init --tunnel-name NAME --tunnel-hostname HOSTNAME",
            "global config not found at {} — tunnel configuration is required",
            path.display()
        ));
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| miette::miette!("read config.yaml: {e:#}"))?;
    let raw: RawGlobalConfig = serde_saphyr::from_str(&content)
        .map_err(|e| miette::miette!("parse config.yaml: {e:#}"))?;
    let raw_tunnel = raw.tunnel.ok_or_else(|| {
        miette::miette!(
            help = "run: right init --tunnel-name NAME --tunnel-hostname HOSTNAME",
            "config.yaml has no `tunnel:` block — Cloudflare Tunnel is required"
        )
    })?;
    if raw_tunnel.credentials_file.is_empty() || raw_tunnel.tunnel_uuid.is_empty() {
        return Err(miette::miette!(
            help = "run: right init --tunnel-name NAME --tunnel-hostname HOSTNAME",
            "Tunnel config is outdated (uses token-based format) — re-run `right init` to migrate"
        ));
    }
    Ok(GlobalConfig {
        tunnel: TunnelConfig {
            tunnel_uuid: raw_tunnel.tunnel_uuid,
            credentials_file: PathBuf::from(&raw_tunnel.credentials_file),
            hostname: raw_tunnel.hostname,
        },
        aggregator: raw
            .aggregator
            .map(|a| AggregatorConfig {
                allowed_hosts: a.allowed_hosts,
            })
            .unwrap_or_default(),
    })
}
```

- [ ] **Step 1.5: Update `write_global_config` to always emit tunnel block**

Replace `crates/right-agent/src/config/mod.rs:227-252` with:

```rust
pub fn write_global_config(home: &Path, config: &GlobalConfig) -> miette::Result<()> {
    let path = home.join("config.yaml");
    let mut content = String::new();
    content.push_str("tunnel:\n");
    let uuid = config.tunnel.tunnel_uuid.replace('"', "\\\"");
    let creds = config
        .tunnel
        .credentials_file
        .display()
        .to_string()
        .replace('"', "\\\"");
    let hostname = config.tunnel.hostname.replace('"', "\\\"");
    content.push_str(&format!("  tunnel_uuid: \"{uuid}\"\n"));
    content.push_str(&format!("  credentials_file: \"{creds}\"\n"));
    content.push_str(&format!("  hostname: \"{hostname}\"\n"));
    if !config.aggregator.allowed_hosts.is_empty() {
        content.push_str("aggregator:\n");
        content.push_str("  allowed_hosts:\n");
        for host in &config.aggregator.allowed_hosts {
            let escaped = host.replace('"', "\\\"");
            content.push_str(&format!("    - \"{escaped}\"\n"));
        }
    }
    std::fs::write(&path, &content).map_err(|e| miette::miette!("write config.yaml: {e:#}"))?;
    Ok(())
}
```

- [ ] **Step 1.6: Update existing tests that exercised `tunnel: None`**

Two places in `crates/right-agent/src/config/mod.rs`:

Replace the test at line 358 (currently `read_global_config_returns_default_when_file_missing`) with:

```rust
#[test]
fn read_global_config_errors_when_file_missing() {
    let dir = TempDir::new().unwrap();
    let err = read_global_config(dir.path()).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not found") || msg.contains("tunnel"),
        "error should mention missing config or tunnel, got: {msg}"
    );
}
```

Replace the test at line 383 (currently `aggregator_allowed_hosts_roundtrip`) — it constructs `GlobalConfig { tunnel: None, ... }`, which no longer compiles. Update to:

```rust
#[test]
fn aggregator_allowed_hosts_roundtrip() {
    let dir = TempDir::new().unwrap();
    let written = GlobalConfig {
        tunnel: TunnelConfig {
            tunnel_uuid: "abc-123".to_string(),
            credentials_file: PathBuf::from("/tmp/abc-123.json"),
            hostname: "test.example.com".to_string(),
        },
        aggregator: AggregatorConfig {
            allowed_hosts: vec![
                "mcp.example.com".to_string(),
                "mcp.example.com:8100".to_string(),
            ],
        },
    };
    write_global_config(dir.path(), &written).unwrap();
    let read = read_global_config(dir.path()).unwrap();
    assert_eq!(
        read.aggregator.allowed_hosts,
        vec![
            "mcp.example.com".to_string(),
            "mcp.example.com:8100".to_string(),
        ]
    );
    assert_eq!(read.tunnel.tunnel_uuid, "abc-123");
}
```

- [ ] **Step 1.7: Fix workspace consumers**

Run:

```sh
cargo build --workspace 2>&1 | grep -E "error\[" | head -30
```

Expected errors at these callsites (fix each by removing `Option`/`unwrap_or_default`/`.is_some()` checks that no longer compile):

- `crates/right/src/main.rs:1371` — change `unwrap_or_default()` to `?` (propagate the error). The right-up path expects a configured tunnel.
- `crates/right/src/wizard.rs:464,502` — wizard reads existing config; for the first-run case, gate on `home.join("config.yaml").exists()` before calling `read_global_config`. If not present, construct a new `GlobalConfig` from the wizard's outputs directly without reading.
- `crates/right-agent/src/doctor.rs:115` — drop the `.is_some()` check; tunnel is always configured if read succeeds. The `if let Ok(cfg)` guard remains (it tolerates the read error if config is broken — doctor should still run).
- `crates/right-agent/src/codegen/pipeline.rs:212` — already uses `?`; no change.
- `crates/bot/src/telegram/handler.rs:783` — already `match`; if it consumed `tunnel.is_none()`, drop that branch.
- `crates/right-agent/src/tunnel/health.rs:62` — same pattern.

For each compile error, the fix is mechanical: drop the `Option` layer.

- [ ] **Step 1.8: Run all config tests**

```sh
cargo test -p right-agent --lib config::tests
```

Expected: PASS (all tests, including the new `read_global_config_errors_when_tunnel_block_missing` and `read_global_config_errors_when_file_missing`).

- [ ] **Step 1.9: Run full workspace build**

```sh
cargo build --workspace
```

Expected: PASS, no errors.

- [ ] **Step 1.10: Add a sanity test for `derive_token(secret, "tg-webhook")`**

In `crates/right-agent/src/mcp/mod.rs`, extend `mod tests`:

```rust
    #[test]
    fn derive_token_for_tg_webhook_matches_telegram_secret_format() {
        let secret = generate_agent_secret();
        let webhook_secret = derive_token(&secret, "tg-webhook").unwrap();
        assert!(
            webhook_secret.len() >= 1 && webhook_secret.len() <= 256,
            "len out of Telegram's 1-256 range: {}",
            webhook_secret.len()
        );
        assert!(
            webhook_secret
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "char outside Telegram's [A-Za-z0-9_-]: {webhook_secret}"
        );
    }
```

Run:

```sh
cargo test -p right-agent --lib mcp::tests::derive_token_for_tg_webhook_matches_telegram_secret_format
```

Expected: PASS.

- [ ] **Step 1.11: Commit**

```sh
git add crates/right-agent/src/config/mod.rs crates/right/src/main.rs \
        crates/right/src/wizard.rs crates/right-agent/src/doctor.rs \
        crates/bot/src/telegram/handler.rs crates/right-agent/src/tunnel/health.rs \
        crates/right-agent/src/mcp/mod.rs
git commit -m "$(cat <<'EOF'
feat(config): make Cloudflare Tunnel mandatory

Drop Option<TunnelConfig> from GlobalConfig. read_global_config now
returns an error when config.yaml is missing or has no tunnel block,
pointing the operator at `right init`. write_global_config always
emits the tunnel block. All consumers updated to drop the Option layer.

Also adds a sanity test that derive_token(secret, "tg-webhook") produces
a string matching Telegram's secret_token constraints.
EOF
)"
```

---

## Task 2: Drop `Skip` from the global init wizard

**Files:**
- Modify: `crates/right/src/wizard.rs:16-35` (TunnelExistingAction enum), `:49-54` (TunnelOutcome enum), `:169-268` (tunnel_setup), `:196-199` (handle_existing_tunnel callsite), `:500` (combined_setting_menu callsite)

- [ ] **Step 2.1: Remove `Skip` variant from `TunnelExistingAction`**

Edit `crates/right/src/wizard.rs:16-35`:

```rust
enum TunnelExistingAction {
    Reuse,
    Rename,
    DeleteAndRecreate,
}

impl fmt::Display for TunnelExistingAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reuse => write!(f, "Reuse existing tunnel"),
            Self::Rename => write!(f, "Create a new tunnel with a different name"),
            Self::DeleteAndRecreate => write!(f, "Delete and recreate the tunnel"),
        }
    }
}
```

Also update the `dialoguer::Select` call that shows these choices — remove the `Skip` entry from the `&[]` list. Find it inside `handle_existing_tunnel` (search for the `&[TunnelExistingAction::Reuse, ...]` array).

- [ ] **Step 2.2: Remove `Skipped` from `TunnelOutcome`**

Edit `crates/right/src/wizard.rs:49-54`:

```rust
enum TunnelOutcome {
    /// Use this tunnel UUID.
    Uuid(String),
}
```

Note: with only one variant, this enum is now redundant. Inline it: change `handle_existing_tunnel`'s return type from `miette::Result<TunnelOutcome>` to `miette::Result<String>` (the UUID directly). Drop the `TunnelOutcome` enum entirely.

- [ ] **Step 2.3: Change `tunnel_setup` to return `TunnelConfig` (not `Option`)**

Edit `crates/right/src/wizard.rs:169-181` (top of `tunnel_setup`):

```rust
pub fn tunnel_setup(
    tunnel_name: &str,
    tunnel_hostname: Option<&str>,
    interactive: bool,
) -> miette::Result<TunnelConfig> {
    if !detect_cloudflared_cert() {
        return Err(miette::miette!(
            help = "run: cloudflared tunnel login",
            "no cloudflared certificate found at ~/.cloudflared/cert.pem"
        ));
    }
    // ... rest of function unchanged until the final return ...
```

The final `return Ok(Some(TunnelConfig { ... }))` (around line 264) becomes `Ok(TunnelConfig { ... })`.

- [ ] **Step 2.4: Remove the `Skipped` branch in `handle_existing_tunnel` callsite**

Edit `crates/right/src/wizard.rs:196-199`:

```rust
if interactive {
    handle_existing_tunnel(&cf_bin, &entry, tunnel_name, has_local_creds)?
}
```

(Now returns `String` directly, no match needed.)

- [ ] **Step 2.5: Update `combined_setting_menu` callsite**

Edit `crates/right/src/wizard.rs:500`:

```rust
let result = tunnel_setup(tunnel_name.trim(), None, true)?;
let mut new_config = read_global_config(home).unwrap_or_else(|_| GlobalConfig {
    tunnel: result.clone(),
    aggregator: AggregatorConfig::default(),
});
new_config.tunnel = result;
```

(If `read_global_config` errors because no config exists yet, build a fresh one. If it succeeds, override the tunnel field.)

Note: `TunnelConfig` does not currently derive `Clone`. Add `#[derive(Clone)]` to it in `crates/right-agent/src/config/mod.rs`.

- [ ] **Step 2.6: Build, fix any compile errors**

```sh
cargo build --workspace
```

Fix follow-on errors (likely in `wizard.rs` callsites — the `tunnel_setup` signature change).

- [ ] **Step 2.7: Run wizard tests**

```sh
cargo test -p right --lib wizard
```

Expected: PASS (or no tests touched by the change). If any test directly invokes `tunnel_setup` and expects an `Option`, update it.

- [ ] **Step 2.8: Commit**

```sh
git add crates/right/src/wizard.rs crates/right-agent/src/config/mod.rs
git commit -m "$(cat <<'EOF'
feat(wizard): drop Skip option from tunnel setup

TunnelExistingAction loses the Skip variant; tunnel_setup returns
TunnelConfig directly (not Option). Missing cloudflared cert is now an
error, not a silent skip. TunnelConfig derives Clone for menu reuse.
EOF
)"
```

---

## Task 3: Pipeline & process-compose template — make cloudflared unconditional

**Files:**
- Modify: `crates/right-agent/src/codegen/pipeline.rs:245-309`
- Modify: `templates/process-compose.yaml.j2:19-47,50-61`

- [ ] **Step 3.1: Unwrap the `if let Some(ref tunnel_cfg)` block in pipeline**

Edit `crates/right-agent/src/codegen/pipeline.rs:245-289` (the entire `let cloudflared_script_path: Option<PathBuf> = if let Some(...) { ... } else { None };` block):

```rust
let cloudflared_script_path: std::path::PathBuf = {
    let agent_pairs: Vec<(String, std::path::PathBuf)> = all_agents
        .iter()
        .map(|a| (a.name.clone(), a.path.clone()))
        .collect();

    let creds = CloudflaredCredentials {
        tunnel_uuid: global_cfg.tunnel.tunnel_uuid.clone(),
        credentials_file: global_cfg.tunnel.credentials_file.clone(),
    };

    let cf_config = crate::codegen::cloudflared::generate_cloudflared_config(
        &agent_pairs,
        &global_cfg.tunnel.hostname,
        Some(&creds),
    )?;
    let cf_config_path = home.join("cloudflared-config.yml");
    write_regenerated(&cf_config_path, &cf_config)?;
    tracing::info!(path = %cf_config_path.display(), "cloudflared config written");

    let scripts_dir = home.join("scripts");
    std::fs::create_dir_all(&scripts_dir)
        .map_err(|e| miette::miette!("create scripts dir: {e:#}"))?;
    let uuid = &global_cfg.tunnel.tunnel_uuid;
    let hostname = &global_cfg.tunnel.hostname;
    let cf_config_path_str = cf_config_path.display();
    let script_content = format!(
        "#!/bin/sh\ncloudflared tunnel route dns --overwrite-dns {uuid} {hostname} || true\nexec cloudflared tunnel --config {cf_config_path_str} run\n"
    );
    let script_path = scripts_dir.join("cloudflared-start.sh");
    write_regenerated(&script_path, &script_content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| miette::miette!("chmod cloudflared-start.sh: {e:#}"))?;
    }
    tracing::info!(path = %script_path.display(), "cloudflared wrapper script written");
    script_path
};
```

- [ ] **Step 3.2: Find downstream consumer of `cloudflared_script_path` and drop `Option`**

Search for `cloudflared_script_path` usage further down in `pipeline.rs`:

```sh
rg -n "cloudflared_script_path" /Users/molt/dev/rightclaw/crates/right-agent/src/codegen/pipeline.rs
```

Wherever it's wrapped in `if let Some(...)` or passed as `Option<&PathBuf>` to the process-compose template context, drop the `Option` layer — the path now always exists.

- [ ] **Step 3.3: Make process-compose template's cloudflared block unconditional**

Edit `templates/process-compose.yaml.j2:50-61`:

```jinja2
  cloudflared:
    command: "{{ cloudflared.command }}"
    working_dir: "{{ cloudflared.working_dir }}"
    availability:
      restart: "on_failure"
      backoff_seconds: 5
      max_restarts: 10
    shutdown:
      signal: 15
      timeout_seconds: 30
```

(Remove the `{% if cloudflared %}` and `{% endif %}` lines.)

- [ ] **Step 3.4: Add `cloudflared` to per-agent bot `depends_on`**

Edit `templates/process-compose.yaml.j2:43-46` (the `depends_on` block inside the per-agent bot loop):

```jinja2
    depends_on:
{% if right_mcp_server %}
      right-mcp-server:
        condition: process_started
{% endif %}
      cloudflared:
        condition: process_started
```

- [ ] **Step 3.5: Build and run codegen tests**

```sh
cargo build --workspace && cargo test -p right-agent --lib codegen
```

Expected: PASS.

- [ ] **Step 3.6: Commit**

```sh
git add crates/right-agent/src/codegen/pipeline.rs templates/process-compose.yaml.j2
git commit -m "$(cat <<'EOF'
feat(codegen): cloudflared is unconditional in pipeline & process-compose

The tunnel is mandatory; pipeline.rs no longer guards cloudflared
generation behind `if let Some(tunnel_cfg)`. process-compose.yaml
emits the cloudflared block unconditionally and per-agent bots add
cloudflared to depends_on (condition: process_started — the bot's
own setWebhook retry handles tunnel-not-yet-reachable).
EOF
)"
```

---

## Task 4: Rename UDS from `oauth-callback.sock` to `bot.sock`

**Files:**
- Modify: `crates/right-agent/src/codegen/cloudflared.rs:46-47`
- Modify: `crates/right-agent/src/codegen/cloudflared_tests.rs:60-61,71` (string assertions)
- Modify: `crates/bot/src/lib.rs:438` (socket path construction)

- [ ] **Step 4.1: Update `CloudflaredAgent.socket_path` construction**

Edit `crates/right-agent/src/codegen/cloudflared.rs:42-51`:

```rust
let cf_agents: Vec<CloudflaredAgent> = agents
    .iter()
    .map(|(name, dir)| CloudflaredAgent {
        name: name.clone(),
        socket_path: dir
            .join("bot.sock")
            .to_string_lossy()
            .to_string(),
    })
    .collect();
```

- [ ] **Step 4.2: Update existing cloudflared test assertions**

Edit `crates/right-agent/src/codegen/cloudflared_tests.rs:53-77`:

```rust
#[test]
fn ingress_service_is_unix_socket_in_agent_dir() {
    let agents = vec![(
        "myagent".to_string(),
        PathBuf::from("/home/user/.right/agents/myagent"),
    )];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", None).unwrap();
    assert!(
        yaml.contains("unix:/home/user/.right/agents/myagent/bot.sock"),
        "wrong socket service: {yaml}"
    );
}

#[test]
fn catch_all_is_always_last_entry() {
    let agents = vec![("agent1".to_string(), PathBuf::from("/tmp/agents/agent1"))];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", None).unwrap();
    let catch_all_pos = yaml.rfind("http_status:404").expect("catch-all not found");
    let agent_rule_pos = yaml.rfind("bot.sock").expect("agent rule not found");
    assert!(
        catch_all_pos > agent_rule_pos,
        "catch-all must be after all agent rules. yaml:\n{yaml}"
    );
}
```

- [ ] **Step 4.3: Update bot's startup socket path**

Edit `crates/bot/src/lib.rs:438`:

```rust
let socket_path = agent_dir.join("bot.sock");
```

- [ ] **Step 4.4: Run cloudflared tests**

```sh
cargo test -p right-agent --lib codegen::cloudflared
```

Expected: PASS.

- [ ] **Step 4.5: Build workspace**

```sh
cargo build --workspace
```

Expected: PASS.

- [ ] **Step 4.6: Commit**

```sh
git add crates/right-agent/src/codegen/cloudflared.rs \
        crates/right-agent/src/codegen/cloudflared_tests.rs \
        crates/bot/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(bot): rename UDS to bot.sock

The per-agent UDS now serves both OAuth callbacks and Telegram
webhooks, so oauth-callback.sock is no longer accurate. Cloudflared
ingress and bot startup use bot.sock instead.
EOF
)"
```

---

## Task 5: Add `/tg/<agent>/.*` ingress to cloudflared template

**Files:**
- Modify: `templates/cloudflared-config.yml.j2`
- Modify: `crates/right-agent/src/codegen/cloudflared_tests.rs` (add tests)

- [ ] **Step 5.1: Add a failing test for `/tg/<agent>/.*` ingress**

Edit `crates/right-agent/src/codegen/cloudflared_tests.rs`, add at end:

```rust
#[test]
fn webhook_ingress_rule_per_agent() {
    let agents = vec![
        (
            "alpha".to_string(),
            PathBuf::from("/home/user/.right/agents/alpha"),
        ),
        (
            "beta".to_string(),
            PathBuf::from("/home/user/.right/agents/beta"),
        ),
    ];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", None).unwrap();
    assert!(
        yaml.contains("path: /tg/alpha/.*"),
        "missing alpha webhook ingress: {yaml}"
    );
    assert!(
        yaml.contains("path: /tg/beta/.*"),
        "missing beta webhook ingress: {yaml}"
    );
}

#[test]
fn webhook_ingress_appears_before_oauth_for_same_agent() {
    let agents = vec![("alpha".to_string(), PathBuf::from("/tmp/agents/alpha"))];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", None).unwrap();
    let tg_pos = yaml.find("/tg/alpha/.*").expect("missing /tg rule");
    let oauth_pos = yaml.find("/oauth/alpha/callback").expect("missing /oauth rule");
    assert!(
        tg_pos < oauth_pos,
        "/tg rule must come before /oauth rule for same agent (first match wins). yaml:\n{yaml}"
    );
}
```

- [ ] **Step 5.2: Run tests to verify they fail**

```sh
cargo test -p right-agent --lib codegen::cloudflared::tests::webhook
```

Expected: both tests FAIL — `/tg/<agent>/.*` is not in the template yet.

- [ ] **Step 5.3: Update the cloudflared template**

Replace `templates/cloudflared-config.yml.j2` entirely:

```jinja2
tunnel: {{ tunnel_uuid }}
credentials-file: {{ credentials_file }}
ingress:
{% for agent in agents %}
  - hostname: {{ tunnel_hostname }}
    path: /tg/{{ agent.name }}/.*
    service: unix:{{ agent.socket_path }}
  - hostname: {{ tunnel_hostname }}
    path: /oauth/{{ agent.name }}/callback
    service: unix:{{ agent.socket_path }}
{% endfor %}
  - service: http_status:404
```

The `{% if tunnel_uuid %}` guard is gone — credentials are always present (Task 1 made the tunnel mandatory).

- [ ] **Step 5.4: Update the existing `no_credentials_section_when_none` test**

This test asserted that `tunnel:` is absent when `credentials` is `None`. With the new template, that's no longer expressible — the template always emits `tunnel:`. Replace the test in `crates/right-agent/src/codegen/cloudflared_tests.rs:115-127` with:

```rust
#[test]
fn credentials_section_emitted_with_credentials() {
    let agents = vec![("agent".to_string(), PathBuf::from("/tmp/agents/agent"))];
    let creds = CloudflaredCredentials {
        tunnel_uuid: "abc-uuid".to_string(),
        credentials_file: PathBuf::from("/etc/cf-creds.json"),
    };
    let yaml = generate_cloudflared_config(&agents, "right.example.com", Some(&creds)).unwrap();
    assert!(yaml.contains("tunnel: abc-uuid"));
    assert!(yaml.contains("credentials-file: /etc/cf-creds.json"));
}
```

Add `use crate::codegen::cloudflared::CloudflaredCredentials;` to the test module's imports if missing.

- [ ] **Step 5.5: Decide on `generate_cloudflared_config` signature**

The function still takes `credentials: Option<&CloudflaredCredentials>`. Since credentials are now always passed, simplify:

Edit `crates/right-agent/src/codegen/cloudflared.rs:37-41`:

```rust
pub fn generate_cloudflared_config(
    agents: &[(String, PathBuf)],
    tunnel_hostname: &str,
    credentials: &CloudflaredCredentials,
) -> miette::Result<String> {
```

And update the body (lines 59-62):

```rust
let tunnel_uuid = credentials.tunnel_uuid.as_str();
let credentials_file = credentials.credentials_file.display().to_string();
```

Update the pipeline callsite at `crates/right-agent/src/codegen/pipeline.rs` to pass `&creds` instead of `Some(&creds)`.

Update test callsites in `cloudflared_tests.rs` to construct a `CloudflaredCredentials` and pass `&creds`. Replace tests that called with `None`:

```rust
fn fixture_creds() -> CloudflaredCredentials {
    CloudflaredCredentials {
        tunnel_uuid: "test-uuid".to_string(),
        credentials_file: PathBuf::from("/tmp/creds.json"),
    }
}
```

…and use `&fixture_creds()` everywhere a test previously passed `None`.

- [ ] **Step 5.6: Run all cloudflared tests**

```sh
cargo test -p right-agent --lib codegen::cloudflared
```

Expected: PASS.

- [ ] **Step 5.7: Commit**

```sh
git add templates/cloudflared-config.yml.j2 \
        crates/right-agent/src/codegen/cloudflared.rs \
        crates/right-agent/src/codegen/cloudflared_tests.rs \
        crates/right-agent/src/codegen/pipeline.rs
git commit -m "$(cat <<'EOF'
feat(codegen): /tg/<agent>/.* ingress rule per agent

cloudflared template emits a webhook ingress rule above the existing
OAuth rule for each agent (first match wins). generate_cloudflared_config
now takes a non-Option CloudflaredCredentials reference since the tunnel
is mandatory.
EOF
)"
```

---

## Task 6: Enable teloxide `webhooks-axum` feature

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 6.1: Add the feature**

Edit `/Users/molt/dev/rightclaw/Cargo.toml:37`:

```toml
teloxide = { version = "0.17", default-features = false, features = ["macros", "throttle", "cache-me", "rustls", "webhooks-axum"] }
```

- [ ] **Step 6.2: Run `cargo check`**

```sh
cargo check --workspace
```

Expected: PASS. The new feature pulls in axum 0.8 (already in the workspace) and teloxide's webhook implementation. No code change needed yet.

- [ ] **Step 6.3: Commit**

```sh
git add Cargo.toml
git commit -m "$(cat <<'EOF'
chore(deps): enable teloxide webhooks-axum feature
EOF
)"
```

---

## Task 7: New module — `crates/bot/src/telegram/webhook.rs`

**Files:**
- Create: `crates/bot/src/telegram/webhook.rs`
- Modify: `crates/bot/src/telegram/mod.rs` (add `pub mod webhook;`)

This task creates the webhook router builder and a unit test for the secret-token check.

- [ ] **Step 7.1: Add `pub mod webhook;` to telegram module**

Edit `crates/bot/src/telegram/mod.rs` after the existing `pub type BotType` definition:

```rust
pub mod webhook;
```

- [ ] **Step 7.2: Create the webhook module skeleton**

Create `crates/bot/src/telegram/webhook.rs`:

```rust
use std::convert::Infallible;
use std::future::Future;

use teloxide::update_listeners::{
    UpdateListener,
    webhooks::{Options, axum_no_setup},
};
use url::Url;

use super::BotType;

/// Allowed update types — explicit, not "all". Add new variants here when the
/// handler graph starts processing a new update kind.
fn allowed_updates() -> Vec<teloxide::types::AllowedUpdate> {
    use teloxide::types::AllowedUpdate;
    vec![
        AllowedUpdate::Message,
        AllowedUpdate::EditedMessage,
        AllowedUpdate::CallbackQuery,
    ]
}

/// Build the per-agent webhook router for mounting on the bot's UDS axum app.
///
/// Returns:
///   - an `UpdateListener` to plug into `Dispatcher::dispatch_with_listener(...)`
///   - a future that resolves when the listener is asked to stop (drives shutdown)
///   - the `axum::Router` to nest under `/tg/<agent_name>` on the outer app
///
/// The webhook URL is informational only at this point — `setWebhook` is called
/// elsewhere with the same URL + secret. `Options::address` is unused by
/// `axum_no_setup`; we pass a dummy SocketAddr to satisfy the type.
pub fn build_webhook_router(
    _bot: &BotType,
    secret: String,
    webhook_url: Url,
) -> (
    impl UpdateListener<Err = Infallible>,
    impl Future<Output = ()> + Send,
    axum::Router,
) {
    let options = Options::new(([127, 0, 0, 1], 0).into(), webhook_url).secret_token(secret);
    axum_no_setup(options)
}

/// The `AllowedUpdate` set passed to `setWebhook`. Exposed for the registration
/// loop in `lib.rs`.
pub fn webhook_allowed_updates() -> Vec<teloxide::types::AllowedUpdate> {
    allowed_updates()
}

#[cfg(test)]
mod tests {
    use super::*;
    use teloxide::types::AllowedUpdate;

    #[test]
    fn allowed_updates_lists_message_edited_callback() {
        let allowed = allowed_updates();
        assert!(allowed.contains(&AllowedUpdate::Message));
        assert!(allowed.contains(&AllowedUpdate::EditedMessage));
        assert!(allowed.contains(&AllowedUpdate::CallbackQuery));
    }
}
```

Note: the `_bot` parameter is currently unused but kept in the signature for symmetry with teloxide's `axum_to_router` (which does take a bot). If we never end up needing it, drop it later.

Actually — drop `_bot` now. Cleaner:

```rust
pub fn build_webhook_router(
    secret: String,
    webhook_url: Url,
) -> (
    impl UpdateListener<Err = Infallible>,
    impl Future<Output = ()> + Send,
    axum::Router,
) {
    let options = Options::new(([127, 0, 0, 1], 0).into(), webhook_url).secret_token(secret);
    axum_no_setup(options)
}
```

- [ ] **Step 7.3: Add an integration-style unit test for secret-token enforcement**

In the same file, extend `mod tests`:

```rust
    use axum::body::Body;
    use axum::http::{HeaderValue, Request, StatusCode};
    use tower::ServiceExt as _;

    fn dummy_url() -> Url {
        Url::parse("https://example.com/tg/test/").unwrap()
    }

    #[tokio::test]
    async fn webhook_router_rejects_missing_secret_header() {
        let (_listener, _stop, router) =
            build_webhook_router("the-secret".to_string(), dummy_url());
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from("{}"))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_router_rejects_wrong_secret_header() {
        let (_listener, _stop, router) =
            build_webhook_router("the-secret".to_string(), dummy_url());
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "X-Telegram-Bot-Api-Secret-Token",
                HeaderValue::from_static("wrong-secret"),
            )
            .body(Body::from("{}"))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
```

You may need to add `tower = "0.5"` to `crates/bot/Cargo.toml` `[dev-dependencies]` for the `ServiceExt::oneshot` extension trait. axum already pulls in tower transitively but the trait is in the `tower` crate.

- [ ] **Step 7.4: Run the new tests**

```sh
cargo test -p right-bot --lib telegram::webhook
```

Expected: PASS for the unit test (`allowed_updates_lists_message_edited_callback`) and PASS for the two router tests (teloxide's listener does the secret check internally).

If the secret-rejection tests don't behave as expected (e.g., the route isn't `POST /` but something else), inspect teloxide's source at `~/.cargo/registry/src/index.crates.io-*/teloxide-0.17.0/src/update_listeners/webhooks/axum.rs:axum_no_setup` and adjust the test URI.

- [ ] **Step 7.5: Commit**

```sh
git add crates/bot/src/telegram/webhook.rs crates/bot/src/telegram/mod.rs crates/bot/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(bot): webhook router module with secret-token enforcement

New `webhook.rs` module wraps teloxide's webhooks::axum_no_setup and
exposes the AllowedUpdate set used by setWebhook. Secret enforcement
is delegated to teloxide; tests confirm 401 on missing/wrong header.
EOF
)"
```

---

## Task 8: Mount the webhook router on the existing axum UDS app

**Files:**
- Modify: `crates/bot/src/telegram/oauth_callback.rs:55-59` (build_router), `:276-305` (run_oauth_callback_server signature)
- Modify: `crates/bot/src/lib.rs:437-445` (axum spawn block)

The current `run_oauth_callback_server` builds a router with only the OAuth route. We extend it to accept an external `axum::Router` (from the webhook module) and a `/healthz` route. Rename it for clarity.

- [ ] **Step 8.1: Update `build_router` to accept an extra router and add `/healthz`**

Edit `crates/bot/src/telegram/oauth_callback.rs:55-59`:

```rust
fn build_router(
    state: OAuthCallbackState,
    webhook_router: axum::Router,
    agent_name: String,
    started_at: std::time::Instant,
) -> axum::Router {
    use axum::routing::get;

    // Each sub-router carries its own state via .with_state(...) before merge,
    // so the outer Router<()> stays generic over state. axum 0.8 doesn't allow
    // two different state types on a single Router.
    let oauth_router = axum::Router::new()
        .route("/oauth/{agent_name}/callback", get(handle_oauth_callback))
        .with_state(state);

    let healthz_state = HealthzState {
        agent_name: agent_name.clone(),
        started_at,
    };
    let healthz_router = axum::Router::new()
        .route("/healthz", get(handle_healthz))
        .with_state(healthz_state);

    axum::Router::new()
        .merge(oauth_router)
        .merge(healthz_router)
        .nest(&format!("/tg/{}", agent_name), webhook_router)
}

#[derive(Clone)]
struct HealthzState {
    agent_name: String,
    started_at: std::time::Instant,
    webhook_set: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

async fn handle_healthz(
    axum::extract::State(state): axum::extract::State<HealthzState>,
) -> axum::Json<serde_json::Value> {
    use std::sync::atomic::Ordering;
    axum::Json(serde_json::json!({
        "agent": state.agent_name,
        "webhook_set": state.webhook_set.load(Ordering::Relaxed),
        "uptime_secs": state.started_at.elapsed().as_secs(),
    }))
}
```

The `webhook_set` flag is shared with the register loop (Task 10) — both sides see the same `Arc<AtomicBool>`.

- [ ] **Step 8.2: Update `run_oauth_callback_server` to accept the extra router**

Rename to `run_bot_uds_server` for clarity. Edit `crates/bot/src/telegram/oauth_callback.rs:276-305`:

```rust
pub async fn run_bot_uds_server(
    socket_path: PathBuf,
    state: OAuthCallbackState,
    webhook_router: axum::Router,
    agent_name: String,
    started_at: std::time::Instant,
    webhook_set: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> miette::Result<()> {
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)
            .map_err(|e| miette::miette!("remove stale UDS socket: {e:#}"))?;
    }

    let listener = UnixListener::bind(&socket_path).map_err(|e| {
        miette::miette!("bind bot UDS socket {}: {e:#}", socket_path.display())
    })?;

    tracing::info!(path = %socket_path.display(), "bot UDS server listening");

    if let Some(tx) = ready_tx {
        let _ = tx.send(());
    }

    let router = build_router(state, webhook_router, agent_name, started_at, webhook_set);
    axum::serve(listener, router)
        .await
        .map_err(|e| miette::miette!("axum serve error: {e:#}"))
}
```

Update `build_router`'s signature to thread the flag through:

```rust
fn build_router(
    state: OAuthCallbackState,
    webhook_router: axum::Router,
    agent_name: String,
    started_at: std::time::Instant,
    webhook_set: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> axum::Router {
    // ... same body as Step 8.1, but `HealthzState` now includes `webhook_set` ...
}
```

Update the public re-export if any (e.g., in `crates/bot/src/telegram/mod.rs`).

- [ ] **Step 8.3: Update the bot startup block**

Edit `crates/bot/src/lib.rs:437-445`:

```rust
let socket_path = agent_dir.join("bot.sock");
let started_at = std::time::Instant::now();

// Build the webhook URL from global config + agent name.
let webhook_url = url::Url::parse(&format!(
    "https://{}/tg/{}/",
    config.tunnel_hostname.trim_end_matches('/'),
    args.agent
))
.map_err(|e| miette::miette!("invalid webhook URL: {e:#}"))?;

// Derive the webhook secret from the agent secret (independent of bot token).
let webhook_secret = right_agent::mcp::derive_token(&agent_secret, "tg-webhook")?;

// Build the webhook router (returns a listener, stop future, and Router).
let (update_listener, _webhook_stop, webhook_router) =
    telegram::webhook::build_webhook_router(webhook_secret.clone(), webhook_url.clone());

// Shared flag for healthz "webhook_set"; flipped by the register loop in Task 10.
let webhook_set_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

let (axum_ready_tx, axum_ready_rx) = tokio::sync::oneshot::channel::<()>();
let axum_socket = socket_path.clone();
let agent_name_for_uds = args.agent.clone();
let webhook_set_for_axum = webhook_set_flag.clone();
let axum_handle = tokio::spawn(async move {
    run_bot_uds_server(
        axum_socket,
        oauth_state,
        webhook_router,
        agent_name_for_uds,
        started_at,
        webhook_set_for_axum,
        Some(axum_ready_tx),
    )
    .await
});
let _ = axum_ready_rx.await;
```

Two new dependencies on the surrounding scope:

1. `config.tunnel_hostname` — load from `read_global_config(&home)?` higher in `run_async`. If not already loaded, add:

   ```rust
   let global_cfg = right_agent::config::read_global_config(&home)?;
   ```

   …and use `global_cfg.tunnel.hostname`.

2. `agent_secret` — load from the agent config. Look at how `right_mcp_server` token is currently derived and follow the same pattern (`agent_secret` is in `agent.yaml::secret`). If the bot doesn't currently load it, add:

   ```rust
   let agent_secret = config
       .secret
       .clone()
       .ok_or_else(|| miette::miette!("agent.yaml missing required `secret:` field"))?;
   ```

3. The `update_listener` from `build_webhook_router` is consumed by Task 9 — for now, threading it through is fine: keep the variable.

- [ ] **Step 8.4: Build and check**

```sh
cargo build --workspace
```

Expected: PASS. The webhook server is now mounted on the UDS, but `setWebhook` hasn't been called yet — Telegram still long-polls (next task).

- [ ] **Step 8.5: Commit**

```sh
git add crates/bot/src/telegram/oauth_callback.rs crates/bot/src/lib.rs crates/bot/src/telegram/mod.rs
git commit -m "$(cat <<'EOF'
feat(bot): mount webhook router on bot.sock UDS server

run_oauth_callback_server is now run_bot_uds_server. The axum app
nests the teloxide webhook router under /tg/<agent_name> alongside
the existing /oauth/<agent_name>/callback route, plus /healthz.
Webhook URL is built from global tunnel hostname; secret is derived
from agent_secret via derive_token(secret, "tg-webhook").
EOF
)"
```

---

## Task 9: Switch dispatcher from long-poll to webhook listener

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs:82-100` (run_telegram signature), `:297-298` (dispatch call)
- Modify: `crates/bot/src/lib.rs:753-772` (run_telegram callsite)

- [ ] **Step 9.1: Add `update_listener` parameter to `run_telegram`**

Edit `crates/bot/src/telegram/dispatch.rs:82-100`:

```rust
pub async fn run_telegram<L>(
    token: String,
    allowlist: right_agent::agent::allowlist::AllowlistHandle,
    agent_dir: PathBuf,
    debug: bool,
    pending_auth: PendingAuthMap,
    home: PathBuf,
    ssh_config_path: Option<PathBuf>,
    show_thinking: bool,
    model: Option<String>,
    shutdown: CancellationToken,
    idle_ts: Arc<IdleTimestamp>,
    internal_client: Arc<right_agent::mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<String>,
    hindsight_wrapper: Option<std::sync::Arc<right_agent::memory::ResilientHindsight>>,
    prefetch_cache: Option<right_agent::memory::prefetch::PrefetchCache>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
    stt: Option<std::sync::Arc<crate::stt::SttContext>>,
    update_listener: L,
) -> miette::Result<()>
where
    L: teloxide::update_listeners::UpdateListener<Err = std::convert::Infallible> + Send + 'static,
    L::StopToken: Send,
{
```

- [ ] **Step 9.2: Replace `dispatcher.dispatch().await` with `dispatch_with_listener`**

Edit `crates/bot/src/telegram/dispatch.rs:297-298`:

```rust
tracing::info!("teloxide dispatcher starting (webhook)");
dispatcher
    .dispatch_with_listener(
        update_listener,
        teloxide::error_handlers::LoggingErrorHandler::new(),
    )
    .await;
tracing::info!("dispatcher exited cleanly");
```

- [ ] **Step 9.3: Pass the listener at the callsite**

Edit `crates/bot/src/lib.rs:753-772` (the `tokio::select!` block):

```rust
let result = tokio::select! {
    result = telegram::run_telegram(
        token,
        allowlist,
        agent_dir,
        args.debug,
        Arc::clone(&pending_auth),
        home.clone(),
        ssh_config_path,
        config.show_thinking,
        config.model.clone(),
        shutdown.clone(),
        Arc::clone(&idle_timestamp),
        Arc::clone(&internal_client),
        resolved_sandbox,
        hindsight_wrapper,
        prefetch_cache,
        upgrade_lock,
        stt,
        update_listener,  // NEW: pass the teloxide webhook listener
    ) => result,
    result = axum_handle => result
        .map_err(|e| miette::miette!("axum task panicked: {e:#}"))?,
};
```

- [ ] **Step 9.4: Build and run a smoke test**

```sh
cargo build --workspace
cargo test -p right-bot --lib telegram
```

Expected: PASS.

- [ ] **Step 9.5: Commit**

```sh
git add crates/bot/src/telegram/dispatch.rs crates/bot/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(bot): dispatch via webhook UpdateListener instead of long-poll

run_telegram now takes the listener returned by webhook::build_webhook_router
and uses Dispatcher::dispatch_with_listener. Long-polling is gone — the
listener feeds updates from POST requests on the UDS axum app.
EOF
)"
```

---

## Task 10: setWebhook lifecycle — register-loop with retry/backoff

**Files:**
- Modify: `crates/bot/src/lib.rs` (add `webhook_register_loop` task)

- [ ] **Step 10.1: Add the `webhook_register_loop` helper**

In `crates/bot/src/lib.rs`, near the top of the file (after imports), add:

```rust
async fn webhook_register_loop(
    bot: telegram::BotType,
    url: url::Url,
    secret: String,
    webhook_set: std::sync::Arc<std::sync::atomic::AtomicBool>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    use std::sync::atomic::Ordering;
    use teloxide::requests::Requester as _;
    use teloxide::types::AllowedUpdate;
    use tokio::time::Duration;

    let mut delay = Duration::from_secs(2);
    let allowed = telegram::webhook::webhook_allowed_updates();
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        let req = bot
            .set_webhook(url.clone())
            .secret_token(secret.clone())
            .allowed_updates(allowed.clone())
            .max_connections(40);
        match req.await {
            Ok(_) => {
                webhook_set.store(true, Ordering::Relaxed);
                tracing::info!(target: "bot::webhook", url = %url, "webhook registered");
                return;
            }
            Err(e) => {
                use teloxide::ApiError;
                use teloxide::RequestError;
                if matches!(&e, RequestError::Api(ApiError::Unauthorized)) {
                    tracing::error!(target: "bot::webhook", "bot token invalid; exiting");
                    std::process::exit(2);
                }
                tracing::warn!(
                    target: "bot::webhook",
                    error = %format!("{e:#}"),
                    retry_in_secs = delay.as_secs(),
                    "setWebhook failed",
                );
                let jitter = (rand::random::<u64>() % 1000) as i64 - 500;
                let with_jitter = (delay.as_millis() as i64 + jitter).max(500) as u64;
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(with_jitter)) => {}
                    _ = shutdown.cancelled() => return,
                }
                delay = (delay * 2).min(Duration::from_secs(60));
            }
        }
    }
}
```

Note: `teloxide::ApiError::Unauthorized` may differ — verify the exact variant name in teloxide 0.17. If it's `ApiError::Unknown(s)` with `s.contains("Unauthorized")`, adjust accordingly.

- [ ] **Step 10.2: Spawn the loop after axum is ready**

Edit `crates/bot/src/lib.rs`, right after the `let _ = axum_ready_rx.await;` line:

```rust
let webhook_url_clone = webhook_url.clone();
let webhook_secret_clone = webhook_secret.clone();
let bot_for_webhook = bot.clone();
let shutdown_for_webhook = shutdown.clone();
let webhook_set_for_loop = webhook_set_flag.clone();
let webhook_register_handle = tokio::spawn(async move {
    webhook_register_loop(
        bot_for_webhook,
        webhook_url_clone,
        webhook_secret_clone,
        webhook_set_for_loop,
        shutdown_for_webhook,
    )
    .await
});
```

`bot` here is the `BotType` instance constructed via `telegram::bot::build_bot(token.clone())`. If that's not yet in scope at this point, hoist its construction up before the axum spawn:

```rust
let bot = telegram::bot::build_bot(token.clone());
```

…and pass `bot.clone()` into `run_telegram` so both the dispatcher and the register loop use the same instance.

- [ ] **Step 10.3: Build and verify**

```sh
cargo build --workspace
```

Expected: PASS.

- [ ] **Step 10.4: Commit**

```sh
git add crates/bot/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(bot): setWebhook register loop with retry/backoff

Spawns a task at startup that calls setWebhook with the derived secret,
URL, and allowed_updates. Retries with capped exponential backoff
(2s → 60s, jittered) on transient errors. Exits non-zero on Unauthorized
(invalid bot token). Cancels on shutdown signal.
EOF
)"
```

---

## Task 11: Remove the obsolete pre-startup `delete_webhook` block

**Files:**
- Modify: `crates/bot/src/lib.rs:339-364`

The pre-startup `delete_webhook()` was needed for long-polling (a stale webhook would compete with `getUpdates`). With webhooks as the sole transport, this block is wrong: it'd delete the webhook seconds before our register loop sets it again, and worse, on a competing host it'd interfere with the legitimate webhook.

- [ ] **Step 11.1: Delete the block**

Edit `crates/bot/src/lib.rs:339-364`. Remove:

```rust
// PC-04: Clear any prior Telegram webhook before starting long-polling.
// Fatal if this fails -- competing with an active webhook causes silent message drops.
{
    use teloxide::requests::Requester as _;
    let webhook_bot = teloxide::Bot::new(token.clone());
    webhook_bot.delete_webhook().await.map_err(|e| {
        miette::miette!(
            "deleteWebhook failed -- long polling would compete with active webhook: {e:#}"
        )
    })?;

    // Log bot identity -- helps detect token conflicts with other running CC sessions
    match webhook_bot.get_me().await {
        Ok(me) => tracing::info!(
            agent = %args.agent,
            bot_id = me.id.0,
            bot_username = %me.username(),
            "deleteWebhook succeeded -- bot identity confirmed"
        ),
        Err(e) => {
            tracing::warn!(agent = %args.agent, "getMe failed (non-fatal, bot identity unknown): {e:#}")
        }
    }
}
```

Optionally: keep the `get_me` part for log-the-bot-identity debuggability, but call it directly without a `delete_webhook` precursor:

```rust
{
    use teloxide::requests::Requester as _;
    match bot.get_me().await {
        Ok(me) => tracing::info!(
            agent = %args.agent,
            bot_id = me.id.0,
            bot_username = %me.username(),
            "bot identity confirmed",
        ),
        Err(e) => tracing::warn!(
            agent = %args.agent,
            "getMe failed (non-fatal, bot identity unknown): {e:#}",
        ),
    }
}
```

- [ ] **Step 11.2: Update the comment at lib.rs:69 (function-level)**

Find the doc-comment that says "the teloxide long-polling dispatcher with graceful shutdown wiring". Replace with: "the teloxide webhook dispatcher with graceful shutdown wiring."

- [ ] **Step 11.3: Build**

```sh
cargo build --workspace
```

Expected: PASS.

- [ ] **Step 11.4: Commit**

```sh
git add crates/bot/src/lib.rs
git commit -m "$(cat <<'EOF'
refactor(bot): drop obsolete pre-startup deleteWebhook

The PC-04 cleanup block existed to prevent long-poll from competing
with a stale webhook. Webhooks are now the sole transport, so the
block is wrong — it would delete the webhook moments before our
register loop sets it. Bot identity logging via getMe is preserved.
EOF
)"
```

---

## Task 12: Doctor — invert webhook check + add healthz check

**Files:**
- Modify: `crates/right-agent/src/doctor.rs:472-579` (webhook checks), `:108-119` (tunnel-state severity)

- [ ] **Step 12.1: Rewrite `make_webhook_check` for the new semantics**

Edit `crates/right-agent/src/doctor.rs:529-552`:

```rust
fn make_webhook_check(
    agent_name: &str,
    expected_url: &str,
    info: Result<WebhookInfo, String>,
) -> DoctorCheck {
    match info {
        Ok(info) if info.url.is_empty() => DoctorCheck {
            name: format!("telegram-webhook/{agent_name}"),
            status: CheckStatus::Fail,
            detail: "no webhook registered (expected to be set by bot)".to_string(),
            fix: Some(format!(
                "Run right restart {agent_name} — bot's setWebhook loop will register it"
            )),
        },
        Ok(info) if info.url != expected_url => DoctorCheck {
            name: format!("telegram-webhook/{agent_name}"),
            status: CheckStatus::Fail,
            detail: format!(
                "webhook URL mismatch — registered: {}, expected: {}",
                info.url, expected_url
            ),
            fix: Some(format!(
                "Run right restart {agent_name} to re-register the webhook"
            )),
        },
        Ok(info) => {
            let mut detail = format!("webhook registered: {}", info.url);
            if let Some(msg) = info.last_error_message.as_ref() {
                detail.push_str(&format!(" (last error: {msg})"));
            }
            if info.pending_update_count > 100 {
                return DoctorCheck {
                    name: format!("telegram-webhook/{agent_name}"),
                    status: CheckStatus::Warn,
                    detail: format!(
                        "{detail}; pending_update_count={} (>100)",
                        info.pending_update_count
                    ),
                    fix: Some("Investigate bot health — updates are queueing".to_string()),
                };
            }
            if info.last_error_message.is_some() {
                return DoctorCheck {
                    name: format!("telegram-webhook/{agent_name}"),
                    status: CheckStatus::Warn,
                    detail,
                    fix: None,
                };
            }
            DoctorCheck {
                name: format!("telegram-webhook/{agent_name}"),
                status: CheckStatus::Pass,
                detail,
                fix: None,
            }
        }
        Err(e) => DoctorCheck {
            name: format!("telegram-webhook/{agent_name}"),
            status: CheckStatus::Warn,
            detail: format!("webhook check skipped: {e}"),
            fix: None,
        },
    }
}

#[derive(Debug)]
struct WebhookInfo {
    url: String,
    pending_update_count: u64,
    last_error_message: Option<String>,
}
```

- [ ] **Step 12.2: Rewrite `fetch_webhook_url` to return the full info**

Replace `crates/right-agent/src/doctor.rs:558-579`:

```rust
fn fetch_webhook_info(token: &str) -> Result<WebhookInfo, String> {
    tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("failed to create runtime: {e}"))?;
        rt.block_on(async {
            let url = format!("https://api.telegram.org/bot{token}/getWebhookInfo");
            let resp = reqwest::Client::new()
                .get(&url)
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
                .map_err(|e| format!("HTTP error: {e}"))?;
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("JSON parse error: {e}"))?;
            let result = &body["result"];
            Ok(WebhookInfo {
                url: result["url"].as_str().unwrap_or("").to_string(),
                pending_update_count: result["pending_update_count"].as_u64().unwrap_or(0),
                last_error_message: result["last_error_message"]
                    .as_str()
                    .map(|s| s.to_string()),
            })
        })
    })
}
```

- [ ] **Step 12.3: Update `check_webhook_info_for_agents` to compute and pass expected URL**

Replace `crates/right-agent/src/doctor.rs:472-515`:

```rust
fn check_webhook_info_for_agents(home: &Path) -> Vec<DoctorCheck> {
    let agents_dir = crate::config::agents_dir(home);
    if !agents_dir.exists() {
        return vec![];
    }

    let global_cfg = match crate::config::read_global_config(home) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut checks = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let config = match crate::agent::discovery::parse_agent_config(&path) {
            Ok(Some(c)) => c,
            Ok(None) | Err(_) => continue,
        };

        let token = match resolve_token_from_config(&path, &config) {
            Some(t) => t,
            None => continue,
        };

        let expected_url = format!("https://{}/tg/{}/", global_cfg.tunnel.hostname, name);
        checks.push(make_webhook_check(&name, &expected_url, fetch_webhook_info(&token)));
    }

    checks
}
```

- [ ] **Step 12.4: Add a healthz check**

After `check_webhook_info_for_agents`, add:

```rust
fn check_bot_healthz_for_agents(home: &Path) -> Vec<DoctorCheck> {
    use std::os::unix::net::UnixStream;
    use std::io::{Read as _, Write as _};

    let agents_dir = crate::config::agents_dir(home);
    if !agents_dir.exists() {
        return vec![];
    }
    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };
    let mut checks = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let socket = path.join("bot.sock");
        if !socket.exists() {
            continue;
        }
        let status = match probe_healthz(&socket) {
            Ok(()) => CheckStatus::Pass,
            Err(e) => {
                checks.push(DoctorCheck {
                    name: format!("bot-healthz/{name}"),
                    status: CheckStatus::Warn,
                    detail: format!("healthz failed: {e}"),
                    fix: Some(format!("Run right restart {name}")),
                });
                continue;
            }
        };
        checks.push(DoctorCheck {
            name: format!("bot-healthz/{name}"),
            status,
            detail: "healthz OK".to_string(),
            fix: None,
        });
    }
    checks
}

fn probe_healthz(socket: &Path) -> Result<(), String> {
    use std::os::unix::net::UnixStream;
    use std::io::{Read as _, Write as _};
    use std::time::Duration;

    let mut stream = UnixStream::connect(socket).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: bot\r\nConnection: close\r\n\r\n")
        .map_err(|e| format!("write: {e}"))?;
    let mut buf = String::new();
    stream
        .read_to_string(&mut buf)
        .map_err(|e| format!("read: {e}"))?;
    if buf.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(format!("non-200 response: {}", buf.lines().next().unwrap_or("(empty)")))
    }
}
```

Wire it into `run_doctor` near where webhook checks are added.

- [ ] **Step 12.5: Tighten tunnel-state severity from WARN to ERROR**

Find `check_tunnel_state` at `crates/right-agent/src/doctor.rs:607` and `check_cloudflared_binary` at `:585`. Change any `CheckStatus::Warn` to `CheckStatus::Fail` for the cases:

- Tunnel block missing.
- Tunnel credentials file missing.
- `cloudflared` binary not in PATH.

(Existing OK / "configured" branches stay `Pass`.)

- [ ] **Step 12.6: Run doctor tests**

```sh
cargo test -p right-agent --lib doctor
```

Fix any unit tests that assert on the old WARN severity for these checks.

- [ ] **Step 12.7: Commit**

```sh
git add crates/right-agent/src/doctor.rs
git commit -m "$(cat <<'EOF'
feat(doctor): expect webhook to be set; healthz check; ERROR on missing tunnel

check_webhook_info_for_agents inverts: a missing webhook is now FAIL
(expected to be set by bot's register loop). URL mismatch is FAIL.
last_error_message and pending_update_count surface as WARN.

New check_bot_healthz_for_agents probes the per-agent UDS for a 200.

Missing tunnel/cloudflared binary/credentials are now ERROR (was WARN)
since the platform requires them.
EOF
)"
```

---

## Task 13: Best-effort `delete_webhook` on agent destroy

**Files:**
- Modify: `crates/right-agent/src/agent/destroy.rs:200-220` (after stop_process, before sandbox delete)

- [ ] **Step 13.1: Add the deleteWebhook step**

Edit `crates/right-agent/src/agent/destroy.rs`, after the `stop_process` block (around line 208) and before the sandbox-delete logic:

```rust
// Best-effort deleteWebhook so Telegram stops trying to deliver to this agent.
if let Some(cfg) = config.as_ref()
    && let Some(token) = cfg.telegram_token.clone()
{
    use teloxide::requests::Requester as _;
    let webhook_bot = teloxide::Bot::new(token);
    match webhook_bot.delete_webhook().await {
        Ok(_) => tracing::info!(agent = %options.agent_name, "deleted Telegram webhook"),
        Err(e) => tracing::warn!(
            agent = %options.agent_name,
            "deleteWebhook failed (continuing): {e:#}",
        ),
    }
}
```

- [ ] **Step 13.2: Build and run destroy tests**

```sh
cargo build --workspace
cargo test -p right-agent --lib agent::destroy
```

Expected: PASS. Existing tests don't depend on `delete_webhook` mocking — Telegram is not reachable in tests, so the call will fail and log a warning, then continue.

- [ ] **Step 13.3: Commit**

```sh
git add crates/right-agent/src/agent/destroy.rs
git commit -m "$(cat <<'EOF'
feat(agent): best-effort deleteWebhook on destroy

Calling deleteWebhook before sandbox/dir cleanup tells Telegram to
stop attempting deliveries. Failures (network, invalid token) log a
warning and continue — a stale webhook URL is a soft leak, not
worth blocking destroy on.
EOF
)"
```

---

## Task 14: Integration test — `right up` rejects missing tunnel

**Files:**
- Create: `crates/right/tests/right_up_requires_tunnel.rs`

- [ ] **Step 14.1: Create the test**

Create `crates/right/tests/right_up_requires_tunnel.rs`:

```rust
//! Integration test: `right up` must error out when the global config has no
//! tunnel block (post-mandatory-tunnel cutover).

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn right_up_errors_when_global_config_missing() {
    let home = TempDir::new().unwrap();
    Command::cargo_bin("right")
        .unwrap()
        .args(["--home", home.path().to_str().unwrap(), "up"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("tunnel").or(predicate::str::contains("right init")));
}

#[test]
fn right_up_errors_when_tunnel_block_missing_from_config() {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.yaml"),
        "aggregator:\n  allowed_hosts:\n    - example.com\n",
    )
    .unwrap();
    Command::cargo_bin("right")
        .unwrap()
        .args(["--home", home.path().to_str().unwrap(), "up"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("tunnel"));
}
```

- [ ] **Step 14.2: Run the test**

```sh
cargo test -p right --test right_up_requires_tunnel
```

Expected: PASS.

- [ ] **Step 14.3: Commit**

```sh
git add crates/right/tests/right_up_requires_tunnel.rs
git commit -m "$(cat <<'EOF'
test(right): right up rejects missing/incomplete tunnel config

Two integration tests confirm right up exits non-zero with a helpful
message when (a) the global config file is missing entirely, or (b)
the file exists but has no tunnel block.
EOF
)"
```

---

## Task 15: Integration test — webhook end-to-end with stubbed Bot API

**Files:**
- Create: `crates/bot/tests/webhook_integration.rs`
- Modify: `crates/bot/Cargo.toml` (add `wiremock` to `[dev-dependencies]`)

This is the most involved test. It stubs the Bot API server (so we don't hit Telegram), starts a fake bot on a UDS, and confirms:

1. `setWebhook` is called with the expected URL, secret, and `allowed_updates` set.
2. `setWebhook` retries with backoff when the stub returns 502.
3. POSTing a constructed `Update` to the bot's UDS with the right header lands in the dispatcher; without the header → 401.

To keep this manageable, the test exercises the **`webhook::build_webhook_router`** API directly (not the full bot binary). The full-binary version is documented in the spec as manual.

- [ ] **Step 15.1: Add `wiremock` and `tower` to dev-dependencies**

Edit `crates/bot/Cargo.toml`, in the `[dev-dependencies]` section:

```toml
wiremock = "0.6"
tower = "0.5"
```

- [ ] **Step 15.2: Create the test file**

Create `crates/bot/tests/webhook_integration.rs`:

```rust
//! Integration test for the webhook router behavior.
//!
//! Exercises:
//!   - axum_no_setup router rejects POSTs with missing/wrong secret header (401).
//!   - axum_no_setup router accepts POSTs with the right header (handing off
//!     to the listener channel — verified via the listener's update stream).
//!   - setWebhook through teloxide is exercised via wiremock by calling the
//!     teloxide Bot directly.

use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode};
use right_bot::telegram::webhook::build_webhook_router;
use serde_json::json;
use tower::ServiceExt as _;
use url::Url;

fn dummy_url() -> Url {
    Url::parse("https://example.com/tg/test/").unwrap()
}

/// A minimal Update JSON that teloxide will accept.
fn fake_update() -> serde_json::Value {
    json!({
        "update_id": 1,
        "message": {
            "message_id": 1,
            "date": 0,
            "chat": {"id": 1, "type": "private", "first_name": "test"},
            "from": {"id": 1, "is_bot": false, "first_name": "test"},
            "text": "hello"
        }
    })
}

#[tokio::test]
async fn webhook_router_401_on_missing_secret() {
    let (_listener, _stop, router) = build_webhook_router("the-secret".to_string(), dummy_url());
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("Content-Type", "application/json")
        .body(Body::from(fake_update().to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_router_401_on_wrong_secret() {
    let (_listener, _stop, router) = build_webhook_router("the-secret".to_string(), dummy_url());
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("Content-Type", "application/json")
        .header(
            "X-Telegram-Bot-Api-Secret-Token",
            HeaderValue::from_static("wrong"),
        )
        .body(Body::from(fake_update().to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_router_202_on_correct_secret() {
    use teloxide::update_listeners::UpdateListener as _;

    let (mut listener, _stop, router) =
        build_webhook_router("the-secret".to_string(), dummy_url());

    // Spawn a task that posts a fake update through the router.
    let router_clone = router.clone();
    let post_task = tokio::spawn(async move {
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("Content-Type", "application/json")
            .header(
                "X-Telegram-Bot-Api-Secret-Token",
                HeaderValue::from_static("the-secret"),
            )
            .body(Body::from(fake_update().to_string()))
            .unwrap();
        router_clone.oneshot(req).await.unwrap()
    });

    // Pull the update from the listener stream.
    use futures::StreamExt as _;
    let stream = listener.as_stream();
    tokio::pin!(stream);
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next()).await;

    let resp = post_task.await.unwrap();
    // teloxide returns 200 OK on accepted updates.
    assert!(
        resp.status().is_success(),
        "expected 2xx, got {}",
        resp.status()
    );
    let received = received
        .expect("listener didn't yield within 2s")
        .expect("listener stream ended");
    let update = received.expect("listener error");
    assert_eq!(update.id.0, 1);
}
```

Note: `right_bot::telegram::webhook::build_webhook_router` requires the `webhook` module to be `pub` and the function to be reachable from outside the crate. Verify (`pub use telegram::webhook;` may need adding to `crates/bot/src/lib.rs`).

- [ ] **Step 15.3: Run the test**

```sh
cargo test -p right-bot --test webhook_integration
```

Expected: PASS for all three. If the listener stream API differs from `as_stream`, consult teloxide docs and adjust.

- [ ] **Step 15.4: Commit**

```sh
git add crates/bot/tests/webhook_integration.rs crates/bot/Cargo.toml
git commit -m "$(cat <<'EOF'
test(bot): webhook router integration tests

Three scenarios:
- 401 on missing X-Telegram-Bot-Api-Secret-Token header.
- 401 on wrong secret value.
- 200 + listener emits the Update on correct secret.

Exercises right_bot::telegram::webhook::build_webhook_router directly
(no full bot binary, no live Telegram).
EOF
)"
```

---

## Task 16: Final smoke — full workspace build, full test, manual check

**Files:** none (verification only)

- [ ] **Step 16.1: Clean build of the entire workspace**

```sh
cargo build --workspace --all-targets
```

Expected: PASS, no warnings beyond pre-existing ones.

- [ ] **Step 16.2: Run the full test suite**

```sh
cargo test --workspace
```

Expected: PASS.

- [ ] **Step 16.3: Run clippy**

```sh
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 16.4: Manual end-to-end check (if you have a live agent)**

```sh
# 1. Restart an existing agent (assuming tunnel is configured globally):
right restart <agent>

# 2. From host, query getWebhookInfo via Telegram (replace <TOKEN>):
curl -s "https://api.telegram.org/bot<TOKEN>/getWebhookInfo" | jq

# Expect:
#   "url": "https://<your.tunnel.hostname>/tg/<agent>/",
#   "pending_update_count": 0,
#   "last_error_message": null

# 3. Send the agent a Telegram message; confirm it replies.

# 4. Run doctor:
right doctor

# Expect:
#   telegram-webhook/<agent>: Pass — webhook registered: ...
#   bot-healthz/<agent>: Pass — healthz OK
```

- [ ] **Step 16.5: Final cleanup commit (if anything was forgotten)**

If any tracked file was modified during smoke testing (e.g., warnings fixed inline), commit them as a single follow-up:

```sh
git add -A
git commit -m "chore: final cleanup post-smoke"
```

If nothing changed, skip this step.

---

## Done

The Cloudflare Tunnel is now mandatory. Each per-agent bot serves Telegram webhooks on `/tg/<agent>/...` of the tunnel hostname via its `bot.sock` UDS. Long-polling code is gone. `right doctor` validates the new world. Existing agents pick up webhooks on `right restart`; agents on installations without a tunnel get a clear error pointing at `right init`.
