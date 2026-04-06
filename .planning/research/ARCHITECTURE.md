# Architecture Research: chrome-devtools-mcp Integration

**Domain:** RightClaw — chrome-devtools-mcp milestone
**Researched:** 2026-04-05
**Confidence:** HIGH

## Standard Architecture

### System Overview

```
config.yaml (GlobalConfig)
    │
    ├── tunnel: Option<TunnelConfig>   ← existing
    └── chrome: Option<ChromeConfig>  ← NEW (optional field)

cmd_up() per-agent loop
    │
    ├── generate_settings(...)
    ├── generate_agent_claude_json(...)
    ├── generate_mcp_config(...)       ← MODIFIED: inject chrome-devtools if chrome set
    │       │
    │       ├── mcpServers.rightmemory      ← existing
    │       └── mcpServers.chrome-devtools  ← NEW (conditional on chrome_path)
    └── ...

run_doctor(home) checks
    │
    ├── check_cloudflared_binary()     ← existing pattern to follow
    └── check_chrome()                 ← NEW: Warn severity
```

### Component Responsibilities

| Component | Responsibility | File:Line |
|-----------|---------------|-----------|
| `GlobalConfig` | Runtime config deserialized from config.yaml | `crates/rightclaw/src/config.rs:19` |
| `write_global_config` / `read_global_config` | YAML round-trip for config | `crates/rightclaw/src/config.rs:61,93` |
| `generate_mcp_config` | Write `.mcp.json` merging MCP server entries per agent | `crates/rightclaw/src/codegen/mcp_config.rs:12` |
| `cmd_up` agent loop | Calls `generate_mcp_config` per agent | `crates/rightclaw-cli/src/main.rs:701` |
| `cmd_init` | Resolves tunnel config after home init, writes GlobalConfig | `crates/rightclaw-cli/src/main.rs:250` |
| `run_doctor` | Runs all checks, collects DoctorCheck vec, never short-circuits | `crates/rightclaw/src/doctor.rs:46` |
| `check_binary` | Private fn: `which::which`, returns Pass/Fail DoctorCheck | `crates/rightclaw/src/doctor.rs:136` |

## Recommended Project Structure Changes

No new files needed. All changes are additive to existing files.

```
crates/rightclaw/src/
├── config.rs           # ADD: ChromeConfig struct, RawChromeConfig, extend GlobalConfig, read/write
├── codegen/
│   └── mcp_config.rs   # MODIFY: add chrome_path: Option<&Path> param, inject chrome-devtools entry
└── doctor.rs           # ADD: check_chrome() fn + check_chrome_path(), wire into run_doctor()

crates/rightclaw-cli/src/
└── main.rs             # MODIFY: cmd_init() chrome detection, move read_global_config before agent loop,
                        #         pass chrome path to generate_mcp_config at line 701

crates/bot/src/
└── lib.rs              # ADD: startup chrome path existence check (warn-only)
```

## Integration Points

### 1. ChromeConfig in config.rs

**Pattern to follow:** `TunnelConfig` — separate public struct, private `RawXxx` for deserialization, extend `GlobalConfig`, extend both `read_global_config` and `write_global_config`.

New structs:
```rust
pub struct ChromeConfig {
    pub chrome_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct RawChromeConfig {
    #[serde(default)]
    chrome_path: String,
}
```

Extend `GlobalConfig` at line 19:
```rust
pub struct GlobalConfig {
    pub tunnel: Option<TunnelConfig>,
    pub chrome: Option<ChromeConfig>,   // add
}
```

`read_global_config` (line 61): extend `RawGlobalConfig` with `chrome: Option<RawChromeConfig>`, map to `ChromeConfig` in the same `.transpose()?` pattern as tunnel. Return `Err` if `chrome_path` is present but empty.

`write_global_config` (line 93): serde-saphyr is deserialize-only — YAML is written manually. Add a `chrome:` block after the tunnel block, mirroring lines 97-103:
```
chrome:
  chrome_path: "/usr/bin/google-chrome"
```

**Config YAML shape:**
```yaml
chrome:
  chrome_path: "/usr/bin/google-chrome"
```

**Where `GlobalConfig` is used in cmd_up:** Currently read at line 706 (after the per-agent loop). Chrome path must be available *inside* the per-agent loop at line 701. Move `read_global_config` to before the loop (alongside `self_exe` and `rg_path` at line 595 area), extract `chrome_cfg` from it, then pass `chrome_cfg.as_ref().map(|c| c.chrome_path.as_path())` to `generate_mcp_config`.

### 2. generate_mcp_config extension

**Current signature (mcp_config.rs:12):**
```rust
pub fn generate_mcp_config(
    agent_path: &Path,
    binary: &Path,
    agent_name: &str,
    rightclaw_home: &Path,
) -> miette::Result<()>
```

**New signature — add at end to minimize call-site churn:**
```rust
pub fn generate_mcp_config(
    agent_path: &Path,
    binary: &Path,
    agent_name: &str,
    rightclaw_home: &Path,
    chrome_path: Option<&Path>,
) -> miette::Result<()>
```

**Injection logic** — after the `rightmemory` insert at line 39:
```rust
if chrome_path.is_some() {
    servers.insert(
        "chrome-devtools".to_string(),
        serde_json::json!({
            "command": "npx",
            "args": ["-y", "chrome-devtools-mcp@latest"],
            "env": {
                "CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS": "1"
            }
        }),
    );
}
```

Note: `chrome-devtools-mcp` discovers Chrome automatically. The `chrome_path` gates whether the entry is injected — it is not passed as `--executablePath` unless agents consistently fail Chrome discovery (defer that complexity). If explicit path forwarding is needed later, add `"--executablePath", chrome_path.to_str().unwrap()` to the args array.

**Call site at main.rs:701** — update to:
```rust
rightclaw::codegen::generate_mcp_config(
    &agent.path, &self_exe, &agent.name, home,
    chrome_cfg.as_ref().map(|c| c.chrome_path.as_path()),
)?;
```

Adding the param triggers a compile error at the call site, which is the correct way to catch all callers (tests included).

### 3. Chrome detection in cmd_init

**Where to insert:** After tunnel setup (line 352 `write_global_config` call) but before `Ok(())`. Chrome detection is non-interactive and non-fatal.

**Detection logic (new private fn `detect_chrome_binary() -> Option<PathBuf>`):**
```rust
fn detect_chrome_binary() -> Option<PathBuf> {
    for name in &["google-chrome", "chromium-browser", "chromium", "chrome"] {
        if let Ok(p) = which::which(name) { return Some(p); }
    }
    // macOS hardcoded path
    let mac = std::path::Path::new(
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    );
    if mac.exists() { return Some(mac.to_path_buf()); }
    None
}
```

**Critical:** `cmd_init` currently writes `GlobalConfig` only when tunnel is configured (line 349). To avoid a double-write footgun, accumulate both `tunnel` and `chrome` into a single `GlobalConfig` before calling `write_global_config` once. Concretely:

1. Compute `tunnel_config: Option<TunnelConfig>` from existing tunnel setup logic.
2. Compute `chrome_config: Option<ChromeConfig>` from `detect_chrome_binary()`.
3. If either is `Some`, call `write_global_config` once with both fields populated.

This avoids a read-back-to-merge step and is consistent with how the function currently works.

### 4. Bot startup Chrome check

**Location:** `crates/bot/src/lib.rs` in the `run()` fn (called via `rightclaw_bot::run(...)` at main.rs:240).

**Pattern:** Non-fatal warn. Use `tracing::warn!` — the codebase uses `tracing`, not the `log` crate directly.

```rust
let global_cfg = rightclaw::config::read_global_config(&home_path)?;
if let Some(ref chrome) = global_cfg.chrome {
    if !chrome.chrome_path.exists() {
        tracing::warn!(
            path = %chrome.chrome_path.display(),
            "chrome_path configured but binary not found — chrome-devtools-mcp will fail to launch"
        );
    }
} else {
    tracing::debug!("no chrome configured — chrome-devtools-mcp not injected into agents");
}
```

Use `debug!` for the "not configured" case — it is normal and noisy to warn on every bot start.

### 5. check_chrome() in doctor.rs

**Pattern:** `check_cloudflared_binary()` (line 617) — calls `check_binary()`, wraps to downgrade Fail to Warn.

But `check_binary` takes a single name with `claude-bun` alternatives only (line 137). For Chrome we need multiple names. Write `check_chrome` directly without delegating to `check_binary`:

```rust
fn check_chrome() -> DoctorCheck {
    let candidates = ["google-chrome", "chromium-browser", "chromium", "chrome"];
    for name in candidates {
        if let Ok(path) = which::which(name) {
            return DoctorCheck {
                name: "chrome".to_string(),
                status: CheckStatus::Pass,
                detail: path.display().to_string(),
                fix: None,
            };
        }
    }
    let mac = std::path::Path::new(
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    );
    if mac.exists() {
        return DoctorCheck {
            name: "chrome".to_string(),
            status: CheckStatus::Pass,
            detail: mac.display().to_string(),
            fix: None,
        };
    }
    DoctorCheck {
        name: "chrome".to_string(),
        status: CheckStatus::Warn,   // optional capability, not hard dependency
        detail: "not found in PATH or standard macOS path".to_string(),
        fix: Some(
            "install Google Chrome or Chromium; or manually set chrome.chrome_path in ~/.rightclaw/config.yaml"
                .to_string(),
        ),
    }
}
```

**Wire into `run_doctor` (doctor.rs:115):** Add after `check_cloudflared_binary()`:
```rust
checks.push(check_chrome());

// If chrome is configured, verify the path still exists on disk
if let Ok(global_cfg) = crate::config::read_global_config(home)
    && let Some(ref chrome_cfg) = global_cfg.chrome
{
    checks.push(check_chrome_path(chrome_cfg));
}
```

`check_chrome_path` mirrors `check_tunnel_credentials_file` (line 665): check `.exists()`, return Pass or Warn with a fix hint.

## Data Flow

### rightclaw up — Chrome MCP injection

```
cmd_up()
    │
    ├── resolve rg_path, self_exe (existing, line ~596)
    ├── read_global_config(home)          ← MOVE here from line 706
    │       └── chrome_cfg: Option<ChromeConfig>
    │
    └── for agent in agents:
            └── generate_mcp_config(
                    &agent.path, &self_exe, &agent.name, home,
                    chrome_cfg.as_ref().map(|c| c.chrome_path.as_path())
                )
                    ├── mcpServers.rightmemory    (always)
                    └── mcpServers.chrome-devtools (if chrome_path is Some)
```

### rightclaw init — Chrome detection

```
cmd_init()
    ├── init_rightclaw_home(...)          // home skeleton, agent dirs
    ├── tunnel setup (existing)           // computes tunnel_config: Option<TunnelConfig>
    ├── detect_chrome_binary()            // NEW: returns Option<PathBuf>
    │       ├── which::which("google-chrome") / "chromium-browser" / "chromium" / "chrome"
    │       └── macOS hardcoded path fallback
    └── if tunnel.is_some() || chrome.is_some():
            write_global_config(home, &GlobalConfig { tunnel, chrome })  // single write
```

## Build Order

Dependencies are strictly additive. Each step compiles independently:

**Step 1** — `config.rs`: Add `ChromeConfig`, `RawChromeConfig`, extend `GlobalConfig`, extend `read_global_config` / `write_global_config`. Write tests: roundtrip with chrome field, absent chrome returns `None`, empty chrome_path returns `Err`.

**Step 2** — `mcp_config.rs`: Add `chrome_path: Option<&Path>` param. All existing tests break at compile time (missing 5th arg) — update them to pass `None`. Write new tests: chrome entry injected when `Some`, absent when `None`, idempotent on second call with same `Some`.

**Step 3** — `doctor.rs`: Add `check_chrome()` and `check_chrome_path()`, wire into `run_doctor`. Depends on step 1 for `GlobalConfig`. Write tests.

**Step 4** — `main.rs`: Update `generate_mcp_config` call site (line 701) to pass `chrome_cfg`. Move `read_global_config` before the agent loop. Add `detect_chrome_binary()` and chrome accumulation to `cmd_init`. Depends on steps 1 and 2.

**Step 5** — `bot/src/lib.rs`: Add startup chrome check. Depends on step 1 only. Purely additive.

## Anti-Patterns

### Anti-Pattern 1: Passing chrome_path as env var to chrome-devtools-mcp

**What people do:** Set `CHROME_PATH` env var in the MCP entry, expecting the server to use it.
**Why it's wrong:** `chrome-devtools-mcp` uses `--executablePath` as a CLI arg, not `CHROME_PATH`. Unrecognized env vars are silently ignored.
**Do this instead:** If explicit path forwarding is ever needed, add `"--executablePath"` and the path string to the `args` array. Default: let `chrome-devtools-mcp` discover Chrome automatically.

### Anti-Pattern 2: Writing GlobalConfig twice in cmd_init

**What people do:** Write tunnel config, detect chrome, write config a second time.
**Why it's wrong:** The manual YAML serializer in `write_global_config` only emits fields it knows about. A second write without reading back first will miss the first write's fields.
**Do this instead:** Accumulate both `tunnel: Option<TunnelConfig>` and `chrome: Option<ChromeConfig>` before calling `write_global_config` once.

### Anti-Pattern 3: Fail severity for missing Chrome in doctor

**What people do:** Mirror the `bwrap` check with `CheckStatus::Fail` when Chrome is absent.
**Why it's wrong:** Chrome is an optional capability enhancement. Missing Chrome doesn't break `rightclaw up`, agent launch, memory, tunnel, or Telegram.
**Do this instead:** `CheckStatus::Warn` (same as `cloudflared` and `sqlite3`). Only use `Fail` if Chrome is explicitly configured in config.yaml but the path no longer exists on disk.

### Anti-Pattern 4: Reading global config inside the per-agent loop

**What people do:** Call `read_global_config` inside the `for agent in agents` loop to get chrome_path.
**Why it's wrong:** Reads the same file N times (once per agent), and the current code structure already calls `read_global_config` after the loop at line 706. Splitting the read creates two callsites that must stay in sync.
**Do this instead:** Read global config once before the loop, extract `chrome_cfg`, pass it into `generate_mcp_config` on each iteration.

## Sources

- `crates/rightclaw/src/config.rs` — GlobalConfig, TunnelConfig pattern (inspected directly)
- `crates/rightclaw/src/codegen/mcp_config.rs` — generate_mcp_config signature and JSON merge logic
- `crates/rightclaw/src/doctor.rs` — DoctorCheck, check_binary, check_cloudflared_binary, run_doctor wiring
- `crates/rightclaw-cli/src/main.rs` — cmd_init flow (lines 250-356), cmd_up agent loop (lines 526-703), generate_mcp_config call at line 701
- [chrome-devtools-mcp npm](https://www.npmjs.com/package/chrome-devtools-mcp) — command/args/env var configuration
- [ChromeDevTools/chrome-devtools-mcp GitHub](https://github.com/ChromeDevTools/chrome-devtools-mcp) — --executablePath, --channel, --headless options

---
*Architecture research for: chrome-devtools-mcp integration in RightClaw*
*Researched: 2026-04-05*
