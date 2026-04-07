# Remove Chrome/Browser Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove all Chrome DevTools MCP integration code, config, detection, and tests from rightclaw.

**Architecture:** Pure deletion task across 3 source files + 2 test files + docs. No new code except adjusting function signatures that lose their `chrome_config` parameter.

**Tech Stack:** Rust, cargo test, cargo clippy

---

### Task 1: Remove ChromeConfig from core library (`config.rs`)

**Files:**
- Modify: `crates/rightclaw/src/config.rs`

- [ ] **Step 1: Delete `ChromeConfig` struct and `RawChromeConfig` struct**

Remove lines 18-25 (`ChromeConfig`) and lines 52-58 (`RawChromeConfig`).

Remove `chrome: Option<ChromeConfig>` from `GlobalConfig` (line 31) and `chrome: Option<RawChromeConfig>` from `RawGlobalConfig` (line 49).

After edits, `GlobalConfig` should be:
```rust
#[derive(Debug, Clone, Default)]
pub struct GlobalConfig {
    pub tunnel: Option<TunnelConfig>,
}
```

And `RawGlobalConfig` should be:
```rust
#[derive(Debug, Deserialize)]
struct RawGlobalConfig {
    tunnel: Option<RawTunnelConfig>,
}
```

- [ ] **Step 2: Remove chrome from `read_global_config()`**

In `read_global_config()`, the return expression currently maps both `tunnel` and `chrome`. Remove lines 106-112 (the `chrome:` field mapping). Result should be:

```rust
    Ok(GlobalConfig {
        tunnel: raw
            .tunnel
            .map(|t| -> miette::Result<TunnelConfig> {
                // ... existing tunnel code unchanged ...
            })
            .transpose()?,
    })
```

- [ ] **Step 3: Remove chrome from `write_global_config()`**

Delete lines 131-137 (the `if let Some(ref chrome) = config.chrome { ... }` block).

- [ ] **Step 4: Delete all Chrome-related tests**

Delete these tests entirely (lines 281-405):
- `chrome_config_roundtrip`
- `write_global_config_emits_chrome_section`
- `read_config_no_chrome_section_is_none`
- `read_config_with_chrome_section_parses`
- `read_config_chrome_empty_fields_yields_none`
- `write_then_read_with_tunnel_and_chrome`
- `write_global_config_no_chrome_omits_section`

Also update existing tests that reference `chrome: None` in GlobalConfig construction — remove the `chrome` field:
- `write_then_read_roundtrips_new_fields` (line 195): remove `chrome: None,`
- `write_global_config_emits_tunnel_uuid_not_token` (line 214): remove `chrome: None,`

- [ ] **Step 5: Verify library crate compiles**

Run: `cargo check -p rightclaw`
Expected: compiler errors in `mcp_config.rs` (uses `ChromeConfig` — fixed in Task 2)

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/config.rs
git commit -m "refactor: remove ChromeConfig from GlobalConfig"
```

---

### Task 2: Remove chrome from MCP codegen (`mcp_config.rs`)

**Files:**
- Modify: `crates/rightclaw/src/codegen/mcp_config.rs`

- [ ] **Step 1: Remove `chrome_config` parameter from `generate_mcp_config()`**

Remove `use crate::config::ChromeConfig;` import (line 3).

Change function signature from:
```rust
pub fn generate_mcp_config(
    agent_path: &Path,
    binary: &Path,
    agent_name: &str,
    rightclaw_home: &Path,
    chrome_config: Option<&ChromeConfig>,
) -> miette::Result<()> {
```
to:
```rust
pub fn generate_mcp_config(
    agent_path: &Path,
    binary: &Path,
    agent_name: &str,
    rightclaw_home: &Path,
) -> miette::Result<()> {
```

Remove the doc line mentioning chrome (line 14): `/// - When \`chrome_config\` is Some, injects a \`chrome-devtools\` MCP entry (INJECT-01, INJECT-02).`

Delete the chrome injection block (lines 63-79):
```rust
    // Inject chrome-devtools MCP entry when Chrome is configured (per D-07, INJECT-01, INJECT-02).
    if let Some(chrome) = chrome_config {
        ...
    }
```

- [ ] **Step 2: Remove `chrome_config` parameter from `generate_mcp_config_http()`**

Change signature from:
```rust
pub fn generate_mcp_config_http(
    agent_path: &Path,
    _agent_name: &str,
    right_mcp_url: &str,
    bearer_token: &str,
    chrome_config: Option<&ChromeConfig>,
) -> miette::Result<()> {
```
to:
```rust
pub fn generate_mcp_config_http(
    agent_path: &Path,
    _agent_name: &str,
    right_mcp_url: &str,
    bearer_token: &str,
) -> miette::Result<()> {
```

Delete lines 135-136:
```rust
    // Chrome devtools not available inside OpenShell sandbox -- skip chrome_config
    let _ = chrome_config;
```

- [ ] **Step 3: Delete all Chrome-related tests**

Delete these tests entirely (lines 335-535):
- `chrome_devtools_injected_when_chrome_config_some`
- `chrome_devtools_not_injected_when_none`
- `chrome_devtools_uses_absolute_binary_path_not_npx`
- `chrome_devtools_user_data_dir_is_agent_chrome_profile`
- `chrome_devtools_coexists_with_right`
- `chrome_devtools_idempotent`
- `chrome_devtools_overwrites_stale_entry`

- [ ] **Step 4: Update existing tests that pass `None` as chrome_config**

All non-Chrome tests in this file pass `None` as the last argument to `generate_mcp_config()`. Remove that argument from every call. There are ~10 calls like:
```rust
generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/home/user"), None).unwrap();
```
Change to:
```rust
generate_mcp_config(dir.path(), Path::new("rightclaw"), "test-agent", Path::new("/home/user")).unwrap();
```

Same for `generate_mcp_config_http()` calls — remove the trailing `None` argument.

- [ ] **Step 5: Verify library crate compiles**

Run: `cargo check -p rightclaw`
Expected: success (or errors in CLI crate only — fixed in Task 3)

- [ ] **Step 6: Run library tests**

Run: `cargo test -p rightclaw -- codegen::mcp_config`
Expected: all remaining tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/codegen/mcp_config.rs
git commit -m "refactor: remove chrome_config parameter from MCP codegen"
```

---

### Task 3: Remove Chrome from CLI (`main.rs`)

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`

- [ ] **Step 1: Remove `--chrome-path` CLI arg**

In the `Init` variant of `Commands` enum, remove:
```rust
        /// Path to Chrome binary (overrides auto-detection)
        #[arg(long)]
        chrome_path: Option<std::path::PathBuf>,
```

Update the match arm at line 280 that destructures `Commands::Init { ... chrome_path }` — remove `chrome_path` from the pattern and from the `cmd_init()` call.

- [ ] **Step 2: Remove `chrome_path` parameter from `cmd_init()`**

Remove `chrome_path: Option<&std::path::Path>,` from the function signature (line 334).

Delete the Chrome detection block (lines 361-367):
```rust
    // Chrome + MCP binary detection (CHROME-01, CHROME-02, CHROME-03).
    // Non-fatal: warn and continue if Chrome or MCP binary not found.
    let chrome_cfg = detect_chrome(chrome_path);
    if chrome_cfg.is_none() && chrome_path.is_none() {
        // Auto-detection found nothing — informational, not an error.
        tracing::debug!("No Chrome installation found at standard paths — Chrome injection disabled");
    }
```

Remove `chrome: chrome_cfg,` from the `GlobalConfig` construction (line 442). The config should be:
```rust
    let config = rightclaw::config::GlobalConfig {
        tunnel: tunnel_cfg,
    };
```

- [ ] **Step 3: Delete all Chrome detection functions**

Delete these functions entirely:
- `detect_chrome_binary()` — all 3 platform variants (lines 478-514)
- `brew_prefix()` — macOS only (lines 517-531)
- `detect_mcp_binary()` (lines 533-564)
- `detect_chrome_with_home()` (lines 571-596)
- `detect_chrome()` (lines 599-601)

- [ ] **Step 4: Remove Chrome revalidation block from `cmd_up()`**

Delete the comment and revalidation block (lines 760-783):
```rust
    // Read global config early — needed for Chrome revalidation before any agent work.
    // Also reused by the cloudflared tunnel block after the per-agent loop.
    let global_cfg = rightclaw::config::read_global_config(home)?;

    // Revalidate Chrome paths on every up — ...
    let chrome_cfg: Option<&rightclaw::config::ChromeConfig> = match global_cfg.chrome.as_ref() {
        ...
    };
```

**Keep** the `read_global_config()` call if it's used later for tunnel config. Check if `global_cfg` is referenced elsewhere in `cmd_up()`. If only used for Chrome, remove it. If used for tunnel, keep it but remove the Chrome revalidation block.

- [ ] **Step 5: Remove `chrome_cfg` from codegen calls in `cmd_up()`**

At line 965, `chrome_cfg` is passed to `generate_mcp_config_http()`. Remove that argument:
```rust
        rightclaw::codegen::generate_mcp_config_http(
            &agent.path,
            &agent.name,
            &right_mcp_url,
            &bearer_token,
        )?;
```

Search for any other `chrome_cfg` references in `cmd_up()` and remove them.

- [ ] **Step 6: Delete Chrome detection unit tests**

Delete the test section "detect_chrome helpers" (lines 1645-1738):
- `detect_chrome_binary_with_home_returns_none_for_empty_tmp`
- `detect_mcp_binary_returns_some_when_npm_global_binary_present`
- `detect_mcp_binary_returns_none_when_no_binary_present`
- `detect_chrome_with_home_returns_some_when_both_paths_exist`
- `detect_chrome_with_home_returns_none_when_mcp_missing`
- `detect_chrome_with_home_returns_none_when_mcp_absent_from_tmp`

- [ ] **Step 7: Verify CLI compiles**

Run: `cargo check -p rightclaw-cli`
Expected: success

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "refactor: remove Chrome detection and injection from CLI"
```

---

### Task 4: Remove Chrome integration tests

**Files:**
- Modify: `crates/rightclaw-cli/tests/cli_integration.rs`

- [ ] **Step 1: Delete Chrome-specific integration tests**

Delete these tests:
- `test_init_chrome_path_arg_warns_when_mcp_missing` (lines 321-339)
- `test_up_warns_when_chrome_path_missing` (lines 343-373)

Also delete the section comment: `// --- Phase 43 Plan 01: Chrome detection + single config write ---` (line 301).

Update `test_init_always_writes_config` — remove Chrome references from comments:
- Line 305: change `// D-11: config.yaml must be written even when no cloudflared cert and no Chrome detected.` to `// D-11: config.yaml must be written even when no cloudflared cert is detected.`
- Line 317: change `"config.yaml must exist after init even with no tunnel and no chrome"` to `"config.yaml must exist after init even with no tunnel configured"`

- [ ] **Step 2: Run integration tests**

Run: `cargo test -p rightclaw-cli --test cli_integration`
Expected: all remaining tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/tests/cli_integration.rs
git commit -m "test: remove Chrome integration tests"
```

---

### Task 5: Update ARCHITECTURE.md

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Remove Chrome references**

Four lines to update:
1. Line 23: `config.rs` description — change `GlobalConfig (tunnel, chrome), RIGHTCLAW_HOME resolution` to `GlobalConfig (tunnel), RIGHTCLAW_HOME resolution`
2. Line 93: data flow — remove `├─ Detect Telegram token, cloudflared tunnel, Chrome binary` → `├─ Detect Telegram token, cloudflared tunnel`
3. Line 197: config table — change `Tunnel, Chrome config` to `Tunnel config`
4. Line 219: key types — change `GlobalConfig    // From config.yaml: tunnel, chrome` to `GlobalConfig    // From config.yaml: tunnel`

- [ ] **Step 2: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: remove Chrome references from ARCHITECTURE.md"
```

---

### Task 6: Full workspace build + test

- [ ] **Step 1: Build full workspace**

Run: `cargo build --workspace`
Expected: success

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: all tests pass

- [ ] **Step 4: Final commit (if any fixups needed)**

If clippy or tests revealed issues, fix and commit.
