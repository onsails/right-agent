# Remove RC_TELEGRAM_TOKEN env var Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the `RC_TELEGRAM_TOKEN` / `RC_TELEGRAM_TOKEN_FILE` env var indirection so the bot reads the Telegram token directly from `agent.yaml` on every startup — fixing the stale-token bug on config_watcher restart.

**Architecture:** Delete the env var round-trip (process-compose template → env → resolve_token). The bot already parses `agent.yaml` into `AgentConfig` at startup; `resolve_token()` just returns `config.telegram_token`. The process-compose template no longer emits the token.

**Tech Stack:** Rust, minijinja (process-compose template), serde

---

### Task 1: Simplify `resolve_token()` and remove `token_from_file_content()`

**Files:**
- Modify: `crates/bot/src/telegram/mod.rs:53-95` (resolve_token + token_from_file_content)
- Modify: `crates/bot/src/telegram/mod.rs:97-142` (tests)

- [ ] **Step 1: Rewrite `resolve_token()` — remove env var logic, remove `token_from_file_content()`**

Replace lines 53–95 of `crates/bot/src/telegram/mod.rs` with:

```rust
/// Resolve Telegram token from agent.yaml config.
///
/// Returns Err if `telegram_token` is absent or empty.
pub fn resolve_token(config: &AgentConfig) -> miette::Result<String> {
    if let Some(token) = &config.telegram_token
        && !token.is_empty()
    {
        return Ok(token.clone());
    }
    Err(miette::miette!(
        help = "Add telegram_token to agent.yaml",
        "No Telegram token found for this agent"
    ))
}
```

This removes:
- `token_from_file_content()` helper (lines 63–70)
- `RC_TELEGRAM_TOKEN` env var check (lines 73–78)
- `RC_TELEGRAM_TOKEN_FILE` env var check (lines 79–84)
- The unused `_agent_dir: &Path` parameter

- [ ] **Step 2: Fix all callers of `resolve_token` — remove the `&agent_dir` argument**

Search for `resolve_token(` across `crates/bot/src/`. The only production caller is in `crates/bot/src/lib.rs`:

```rust
// Before:
let token = telegram::resolve_token(&agent_dir, &config)?;

// After:
let token = telegram::resolve_token(&config)?;
```

Also remove the `use std::path::Path;` import in `mod.rs` if it becomes unused (it was only used by `resolve_token`'s `_agent_dir` parameter).

- [ ] **Step 3: Update tests in `crates/bot/src/telegram/mod.rs`**

Replace lines 97–142 with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rightclaw::agent::types::AgentConfig;
    use std::collections::HashMap;

    fn minimal_config() -> AgentConfig {
        AgentConfig {
            restart: Default::default(),
            max_restarts: 3,
            backoff_seconds: 5,
            model: None,
            sandbox: None,
            telegram_token: None,
            allowed_chat_ids: vec![],
            env: HashMap::new(),
            secret: None,
            attachments: Default::default(),
            network_policy: Default::default(),
            show_thinking: true,
            memory: None,
        }
    }

    #[test]
    fn resolve_token_from_config() {
        let mut config = minimal_config();
        config.telegram_token = Some("999:inline_token".to_string());
        let token = resolve_token(&config).unwrap();
        assert_eq!(token, "999:inline_token");
    }

    #[test]
    fn resolve_token_returns_err_when_nothing_configured() {
        let config = minimal_config();
        assert!(resolve_token(&config).is_err());
    }

    #[test]
    fn resolve_token_returns_err_when_empty_string() {
        let mut config = minimal_config();
        config.telegram_token = Some(String::new());
        assert!(resolve_token(&config).is_err());
    }
}
```

- [ ] **Step 4: Build and run tests**

Run: `devenv shell -- cargo test -p rightclaw-bot -- telegram::tests`
Expected: all 3 tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/mod.rs crates/bot/src/lib.rs
git commit -m "refactor(bot): resolve_token reads only from agent.yaml, remove env var indirection"
```

---

### Task 2: Remove `token_inline` from process-compose codegen and template

**Files:**
- Modify: `crates/rightclaw/src/codegen/process_compose.rs:13-35` (BotProcessAgent struct)
- Modify: `crates/rightclaw/src/codegen/process_compose.rs:107-156` (generate_process_compose mapping)
- Modify: `templates/process-compose.yaml.j2:26-28` (RC_TELEGRAM_TOKEN block)

- [ ] **Step 1: Remove `token_inline` field from `BotProcessAgent`**

In `crates/rightclaw/src/codegen/process_compose.rs`, remove line 23:

```rust
    /// Inline Telegram token value.
    token_inline: Option<String>,
```

- [ ] **Step 2: Remove `token_inline` from the struct construction in `generate_process_compose()`**

In `crates/rightclaw/src/codegen/process_compose.rs`, the `filter_map` closure (lines 109–155) currently reads the token and uses it for two purposes: (a) skip agents without a token, (b) pass to template. Keep (a), remove (b).

Replace lines 112–114:

```rust
            // No telegram token configured — skip this agent.
            let token_inline = config.telegram_token.clone();
            token_inline.as_ref()?;
```

With:

```rust
            // No telegram token configured — skip this agent.
            config.telegram_token.as_ref()?;
```

And remove `token_inline,` from the `Some(BotProcessAgent { ... })` construction (line 146).

- [ ] **Step 3: Remove `RC_TELEGRAM_TOKEN` block from template**

In `templates/process-compose.yaml.j2`, delete lines 26–28:

```jinja2
{% if agent.token_inline %}
      - RC_TELEGRAM_TOKEN={{ agent.token_inline }}
{% endif %}
```

- [ ] **Step 4: Build and run tests**

Run: `devenv shell -- cargo test -p rightclaw -- codegen::process_compose`
Expected: `inline_token_uses_rc_telegram_token` FAILS (expected — we'll fix it next step). All other tests pass.

- [ ] **Step 5: Rewrite `inline_token_uses_rc_telegram_token` test**

In `crates/rightclaw/src/codegen/process_compose_tests.rs`, replace lines 182–191:

```rust
#[test]
fn token_not_leaked_to_process_compose_env() {
    let agents = vec![make_bot_agent("myagent", "999:mytoken")];
    let exe = Path::new(EXE_PATH);
    let output = generate_process_compose(&agents, exe, &default_config()).unwrap();
    assert!(
        !output.contains("RC_TELEGRAM_TOKEN"),
        "RC_TELEGRAM_TOKEN must not appear in process-compose output:\n{output}"
    );
    // Agent with token still produces a process entry
    assert!(
        output.contains("myagent-bot:"),
        "agent with token must still appear in output:\n{output}"
    );
}
```

- [ ] **Step 6: Build and run all process-compose tests**

Run: `devenv shell -- cargo test -p rightclaw -- codegen::process_compose`
Expected: all tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/codegen/process_compose.rs crates/rightclaw/src/codegen/process_compose_tests.rs templates/process-compose.yaml.j2
git commit -m "refactor(codegen): remove RC_TELEGRAM_TOKEN from process-compose template"
```

---

### Task 3: Update error message and full workspace verification

**Files:**
- Modify: `crates/bot/src/error.rs:8` (NoToken message)

- [ ] **Step 1: Update `NoToken` error message**

In `crates/bot/src/error.rs`, replace line 8:

```rust
    #[error("no Telegram token found; set RC_TELEGRAM_TOKEN or configure agent.yaml")]
```

With:

```rust
    #[error("no Telegram token found; add telegram_token to agent.yaml")]
```

- [ ] **Step 2: Full workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: compiles cleanly

- [ ] **Step 3: Full workspace tests**

Run: `devenv shell -- cargo test --workspace`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/error.rs
git commit -m "fix(bot): update NoToken error message — env var no longer supported"
```
