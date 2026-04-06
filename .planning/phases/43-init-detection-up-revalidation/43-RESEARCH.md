# Phase 43: Init Detection + Up Revalidation - Research

**Researched:** 2026-04-06
**Domain:** Rust CLI — Chrome/MCP binary auto-detection at init, `--chrome-path` override, config write restructuring, per-run revalidation in cmd_up
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Chrome binary detection (CHROME-01)**
- D-01: Auto-detect Chrome at init by checking standard paths in order — Linux: `/usr/bin/google-chrome-stable`, `/usr/bin/google-chrome`, `/usr/bin/chromium-browser`, `/usr/bin/chromium`, `/snap/bin/chromium`; macOS: `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`, `~/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`
- D-02: `--chrome-path <path>` takes precedence; used as-is, no existence check at arg parse time

**chrome-devtools-mcp binary detection**
- D-03: `which::which("chrome-devtools-mcp")` first, then fallback to `/usr/local/bin/chrome-devtools-mcp`, `~/.npm-global/bin/chrome-devtools-mcp`; macOS additionally checks `$(brew --prefix)/bin/chrome-devtools-mcp`
- D-04: No npx — absolute path to globally-installed binary only

**Partial detection handling (CHROME-03)**
- D-05: Chrome not found → `tracing::warn!`, skip chrome section, init continues
- D-06: Chrome found but MCP not found → same as D-05, skip chrome section
- D-07: `--chrome-path` provided but MCP not found → same as D-06

**Config write restructuring**
- D-08: Chrome detection early in `cmd_init()`, before tunnel block; result in `let chrome_cfg: Option<ChromeConfig>`
- D-09: Tunnel block no longer calls `write_global_config`. Collect `let tunnel_cfg: Option<TunnelConfig>` instead
- D-10: Single `write_global_config(home, &GlobalConfig { tunnel: tunnel_cfg, chrome: chrome_cfg })` at end of `cmd_init()`
- D-11: Write config even if both are None (empty `GlobalConfig::default()`)

**Up revalidation (INJECT-03)**
- D-12: In `cmd_up()`, check `chrome_cfg.chrome_path.exists() && chrome_cfg.mcp_binary_path.exists()`
- D-13: Either path missing → `tracing::warn!` with missing path, effective `chrome_cfg = None` for this run, agents start normally
- D-14: Per-run revalidation, not cached

### Claude's Discretion
- Exact warning message wording for each detection failure case
- Whether to extract Chrome/MCP detection into helper functions in `init.rs` or keep inline in `cmd_init()`
- Brew prefix detection: whether to cache the result or call brew once per detection attempt

### Deferred Ideas (OUT OF SCOPE)
- Per-agent `chrome.enabled: false` opt-out in `agent.yaml` — deferred to v3.5
- Chrome process as a separate process-compose service — v3.5 (CHROME-EXT-01)
- Using `npx chrome-devtools-mcp` instead of installed binary — explicitly rejected
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CHROME-01 | Auto-detect Chrome at standard paths; save to `~/.rightclaw/config.yaml` under `chrome.chrome_path` | Detection helper pattern verified in codebase (detect_cloudflared_cert_with_home); standard paths locked in D-01 |
| CHROME-02 | `--chrome-path <path>` override to `rightclaw init`; saved to config | `Init` struct in main.rs ~line 93 confirmed; `Option<PathBuf>` arg pattern consistent with existing args |
| CHROME-03 | Non-fatal detection — no Chrome found logs warn and continues | `tracing::warn!` + `return Ok(())` / continue pattern already used in cloudflared detection |
| INJECT-03 | Chrome path revalidated on every `rightclaw up`; missing path logs warn and skips injection | `chrome_cfg` extraction point already at line 619 in cmd_up(); existence check slots in before per-agent loop |
</phase_requirements>

---

## Summary

Phase 43 adds two behaviors to an existing, well-structured codebase: (1) `rightclaw init` gains Chrome + MCP binary auto-detection with a `--chrome-path` override, writing results to the already-capable `GlobalConfig`/`write_global_config()` machinery; (2) `rightclaw up` revalidates the stored paths on each run before passing `chrome_cfg` into the per-agent generator loop.

The codebase is ready. `ChromeConfig` struct, `write_global_config()`, and `read_global_config()` all handle the `chrome` section already (Phase 42). The detection pattern to mirror (`detect_cloudflared_cert_with_home`) is already in `main.rs`. The `which::which` crate is already imported in `cmd_up()`. The only structural change is the config write refactor: the tunnel block currently calls `write_global_config` inline; it must be refactored to collect `tunnel_cfg: Option<TunnelConfig>` and defer to a single write at the end of `cmd_init()`.

**Primary recommendation:** Implement detection as a `detect_chrome_with_home(home: &Path, chrome_path_override: Option<&Path>) -> Option<ChromeConfig>` helper in `main.rs` (same file as cloudflared helpers, mirrors the established pattern). Keep brew prefix call simple — one `Command::new("brew").arg("--prefix")` call inline, no caching needed (runs once per init).

---

## Standard Stack

### Core (already in use — no new dependencies needed)

[VERIFIED: codebase grep]

| Library | Version | Purpose | Status |
|---------|---------|---------|--------|
| `which` | workspace | `which::which("chrome-devtools-mcp")` for MCP binary search | Already used in `cmd_up()` for cloudflared check |
| `dirs` | workspace | `dirs::home_dir()` to expand `~` in standard paths | Already used throughout |
| `tracing` | workspace | `tracing::warn!` for non-fatal detection failures | Already used; established pattern |
| `std::path::Path::exists()` | stdlib | Path existence checks in detection and revalidation | No new dep |
| `std::process::Command` | stdlib | `brew --prefix` resolution on macOS | Already used for cloudflared commands |

**No new Cargo dependencies required for this phase.**

---

## Architecture Patterns

### Pattern 1: Testable Detection Helper (mirrors cloudflared)

**What:** `detect_chrome_with_home(home: &Path, override_path: Option<&Path>) -> Option<ChromeConfig>` in `main.rs`. Takes explicit home so tests can pass a `TempDir` path. The non-testable wrapper `detect_chrome()` calls it with `dirs::home_dir()`.

**When to use:** Any binary or file detection that depends on the filesystem — keeps test isolation clean without `std::env::set_var()` (forbidden by CLAUDE.rust.md).

[VERIFIED: existing codebase — `detect_cloudflared_cert_with_home` / `detect_cloudflared_cert` pair at lines 368-377 of main.rs]

```rust
// Pattern from existing codebase (main.rs lines 368-377):
fn detect_cloudflared_cert_with_home(home: &std::path::Path) -> bool {
    home.join(".cloudflared").join("cert.pem").exists()
}

fn detect_cloudflared_cert() -> bool {
    dirs::home_dir()
        .map(|h| detect_cloudflared_cert_with_home(&h))
        .unwrap_or(false)
}

// Phase 43: same pattern for Chrome detection
fn detect_chrome_with_home(
    home: &Path,
    override_path: Option<&Path>,
) -> Option<ChromeConfig> {
    // 1. Resolve chrome_path
    let chrome_path = if let Some(p) = override_path {
        p.to_path_buf()
    } else {
        detect_chrome_binary(home)?  // returns Option<PathBuf>
    };

    // 2. Resolve mcp_binary_path
    let mcp_path = detect_mcp_binary(home)?;  // returns Option<PathBuf>

    Some(ChromeConfig { chrome_path, mcp_binary_path: mcp_path })
}
```

### Pattern 2: Standard Path Iteration

**What:** Iterate a static list of candidate paths, return first that `.exists()`. Expand `~` manually via `home.join(relative_part)` — no shell expansion needed.

[VERIFIED: pattern consistent with existing code; confirmed `~` expansion via `dirs::home_dir()` is the project's established approach]

```rust
fn detect_chrome_binary(home: &Path) -> Option<PathBuf> {
    // Linux paths
    #[cfg(target_os = "linux")]
    let candidates = [
        PathBuf::from("/usr/bin/google-chrome-stable"),
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/chromium-browser"),
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/snap/bin/chromium"),
    ];

    // macOS paths
    #[cfg(target_os = "macos")]
    let candidates = [
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        home.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
    ];

    candidates.into_iter().find(|p| p.exists())
}
```

Note: `#[cfg(target_os)]` for platform branching is standard and appropriate here. The alternative (runtime `std::env::consts::OS`) is also viable but cfg is idiomatic for static path lists.

### Pattern 3: Config Write Refactor

**What:** Currently `cmd_init()` calls `write_global_config` with `chrome: None` inside the tunnel block (line 349-353). After Phase 43, the tunnel block must produce a value instead of writing, and the single write happens at function end.

[VERIFIED: current code at main.rs lines 344-353]

**Current structure (to change):**
```rust
// Inside tunnel block (line 344-353) — currently writes with chrome: None
let config = rightclaw::config::GlobalConfig {
    tunnel: Some(tunnel_config),
    chrome: None,               // <-- WRONG after Phase 43
};
rightclaw::config::write_global_config(home, &config)?;  // <-- move to end
```

**Target structure:**
```rust
// In cmd_init() before tunnel block:
let chrome_cfg = detect_chrome(/* home, override */);
if chrome_cfg.is_none() { tracing::warn!(...); }

// Tunnel block now sets variable, doesn't write:
let tunnel_cfg: Option<TunnelConfig> = if detect_cloudflared_cert() {
    // ... existing tunnel logic ...
    Some(tunnel_config)
} else {
    println!("No cloudflared login found...");
    None
};

// Single write at end:
let config = rightclaw::config::GlobalConfig {
    tunnel: tunnel_cfg,
    chrome: chrome_cfg,
};
rightclaw::config::write_global_config(home, &config)?;
```

Note: The current tunnel block uses `return Ok(())` early exits in the non-cloudflared path (line 287). After refactor, those early returns are gone — the block must fall through. This is the main structural risk; all early returns inside the tunnel block need to become `None`-returning expressions or be restructured.

### Pattern 4: Up Revalidation

**What:** After `chrome_cfg` is extracted from `global_cfg` at line 619, add a two-path existence check. If either path is gone, warn and shadow `chrome_cfg` with `None`.

[VERIFIED: insertion point at main.rs lines 616-619]

```rust
// Existing (line 617-619):
let global_cfg = rightclaw::config::read_global_config(home)?;
let chrome_cfg = global_cfg.chrome.as_ref();

// After Phase 43:
let global_cfg = rightclaw::config::read_global_config(home)?;
let revalidated_chrome: Option<rightclaw::config::ChromeConfig>;
let chrome_cfg = match global_cfg.chrome.as_ref() {
    Some(cfg) if !cfg.chrome_path.exists() => {
        tracing::warn!(
            path = %cfg.chrome_path.display(),
            "configured Chrome binary no longer exists — skipping injection for this run"
        );
        None
    }
    Some(cfg) if !cfg.mcp_binary_path.exists() => {
        tracing::warn!(
            path = %cfg.mcp_binary_path.display(),
            "configured chrome-devtools-mcp binary no longer exists — skipping injection for this run"
        );
        None
    }
    other => other,
};
```

Or more concisely using a helper closure — either is acceptable per discretion.

### Pattern 5: Brew Prefix Detection (macOS only)

**What:** Call `brew --prefix` once via `std::process::Command`, parse stdout, append `/bin/chrome-devtools-mcp`.

[ASSUMED: brew --prefix outputs the prefix path on one line; standard brew behavior. No caching needed for a one-shot init command.]

```rust
fn brew_prefix() -> Option<PathBuf> {
    let out = std::process::Command::new("brew")
        .arg("--prefix")
        .output()
        .ok()?;
    if out.status.success() {
        let prefix = std::str::from_utf8(&out.stdout).ok()?.trim().to_string();
        Some(PathBuf::from(prefix))
    } else {
        None
    }
}
```

### Anti-Patterns to Avoid

- **Writing config inside the tunnel block** — breaks D-10's single-write guarantee; chrome will always be None if tunnel block writes early
- **Existence-checking `--chrome-path` at arg parse time** — locked decision D-02 says validate implicitly; miette will produce a clear error at write time if needed
- **Using `std::env::set_var()` in tests** — CLAUDE.rust.md forbids this; pass home/paths explicitly to testable helper variants
- **`return Ok(())` early exit after chrome detection** — detection is non-fatal; log and continue, never abort init
- **`npx` in MCP binary resolution** — explicitly rejected in D-04

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Find binary in PATH | Custom PATH scanning | `which::which(name)` | Already used in project; handles platform edge cases |
| Home directory | `std::env::var("HOME")` | `dirs::home_dir()` | Cross-platform; already project convention |

---

## Common Pitfalls

### Pitfall 1: Tunnel Block Early Returns Break Config Write Refactor

**What goes wrong:** `cmd_init()` currently has `return Ok(())` at line 287 (no cloudflared) and several other early returns inside the tunnel block. If these aren't converted to produce `None` values, the single config write at function end is never reached on the no-cloudflared path.

**Why it happens:** The tunnel block was written as an "all or nothing" flow. After refactor it needs to be a value-producing block.

**How to avoid:** Restructure the tunnel block as an `Option<TunnelConfig>`-returning closure or inline `if/else` block. The `return Ok(())` at line 287 becomes `None` (no tunnel, keep going). Early error returns (hostname validation failures) remain `return Err(...)`.

**Warning signs:** Test coverage — test "init with no cloudflared should still write empty config" will catch this.

### Pitfall 2: macOS-Only Brew Call on Linux

**What goes wrong:** Calling `brew --prefix` on Linux where brew isn't installed causes `Command` to fail; if not handled with `.ok()` or error suppression, it panics or produces a spurious error.

**How to avoid:** Gate brew prefix lookup with `#[cfg(target_os = "macos")]` or check `which::which("brew").is_ok()` before calling. The `detect_mcp_binary` function should return `None` for the brew path on Linux without error.

### Pitfall 3: `~` in Standard Path Strings

**What goes wrong:** Writing `PathBuf::from("~/.npm-global/bin/chrome-devtools-mcp")` doesn't expand `~` — `Path::exists()` will always return false.

**How to avoid:** Expand manually: `home.join(".npm-global/bin/chrome-devtools-mcp")` where `home = dirs::home_dir()`. Pass `home` explicitly to testable variants.

### Pitfall 4: cmd_init Signature Change Needed

**What goes wrong:** `cmd_init()` currently doesn't accept a `chrome_path` parameter. Adding `--chrome-path` to the `Init` struct requires threading it through the dispatch in `main()` and into `cmd_init()`.

**How to avoid:** Add `chrome_path: Option<PathBuf>` field to `Init` struct (in `Commands::Init` enum variant) and add `chrome_path: Option<&Path>` parameter to `cmd_init()`. This is a 3-line change in the enum + signature.

---

## Code Examples

### Chrome Binary Standard Path Lists (locked by D-01)

```rust
// Linux candidates — check in order, return first that exists
const CHROME_PATHS_LINUX: &[&str] = &[
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium-browser",
    "/usr/bin/chromium",
    "/snap/bin/chromium",
];

// macOS candidates — note: second path requires home expansion
// "/Applications/..." is absolute; "~/Applications/..." needs dirs::home_dir()
```

### MCP Binary Standard Path Lists (locked by D-03)

```rust
// Common paths (Linux + macOS):
//   /usr/local/bin/chrome-devtools-mcp
//   ~/.npm-global/bin/chrome-devtools-mcp   (requires home expansion)
// macOS additionally:
//   $(brew --prefix)/bin/chrome-devtools-mcp  (requires brew call)
```

### Revalidation in cmd_up() [VERIFIED: insertion point main.rs line 619]

The existing `let chrome_cfg = global_cfg.chrome.as_ref();` at line 619 is the insertion point. The revalidation check returns `None` when either path is missing, shadowing the variable. The per-agent loop at line 622 and generator calls at lines 624/707 already accept `Option<&ChromeConfig>` — no downstream changes needed.

---

## Integration Points Summary

[VERIFIED: codebase inspection]

| Location | File | What Changes |
|----------|------|-------------|
| `Commands::Init` enum, ~line 93 | `main.rs` | Add `chrome_path: Option<PathBuf>` arg |
| `cmd_init()` dispatch, ~line 206 | `main.rs` | Pass `chrome_path.as_deref()` to `cmd_init()` |
| `cmd_init()` signature, ~line 250 | `main.rs` | Add `chrome_path: Option<&Path>` param |
| `cmd_init()` body — before tunnel block | `main.rs` | Add Chrome detection, store `chrome_cfg: Option<ChromeConfig>` |
| `cmd_init()` tunnel block | `main.rs` | Refactor to produce `tunnel_cfg: Option<TunnelConfig>` (no write inside) |
| `cmd_init()` end | `main.rs` | Single `write_global_config` with both tunnel + chrome |
| `cmd_up()` ~line 619 | `main.rs` | Add revalidation block after `chrome_cfg` extraction |
| New helpers | `main.rs` | `detect_chrome_with_home()`, `detect_chrome()`, `detect_chrome_binary()`, `detect_mcp_binary()` |

`config.rs` and `init.rs` require **no changes** — all infrastructure is already in place.

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `brew --prefix` outputs the brew prefix on stdout, one line, no trailing slash issues | Code Examples | Brew prefix path format varies; wrap with `.trim()` to handle trailing newline |
| A2 | No Chrome detection helper currently exists in `init.rs` or `main.rs` | Integration Points | If one exists, would be duplicate; grep search shows none |

---

## Open Questions

1. **Tunnel block refactor scope**
   - What we know: The block currently has several early `return Ok(())` and `return Err(...)` paths (lines 287-356)
   - What's unclear: Whether to restructure as a closure, a separate `fn detect_tunnel() -> Option<TunnelConfig>` helper, or inline `if/else`
   - Recommendation: Inline `if/else` returning `Option<TunnelConfig>` — consistent with how Chrome detection will be structured; avoids introducing a new helper for existing behavior

---

## Environment Availability

Step 2.6: SKIPPED (phase is pure code change — no external dependencies beyond what's already required by the project)

---

## Security Domain

> `security_enforcement` not set to false in config.json — section required.

Chrome binary detection involves path traversal and filesystem probing. No user-controlled input reaches `PathBuf::exists()` calls in the standard-path detection — all paths are hardcoded literals or derived from `dirs::home_dir()`. The `--chrome-path` CLI override is `Option<PathBuf>` from clap — no shell expansion, no injection surface. MCP binary detection via `which::which` is safe (resolves against PATH, no shell). No ASVS categories apply beyond V5 (input validation is moot for static path lists). No cryptographic operations.

---

## Sources

### Primary (HIGH confidence)
- `crates/rightclaw-cli/src/main.rs` — cmd_init() lines 250-357, cmd_up() lines 600-624, detect_cloudflared_cert pattern lines 368-377, Init struct lines 93-110
- `crates/rightclaw/src/config.rs` — ChromeConfig, GlobalConfig, write_global_config, read_global_config — full file inspected
- `crates/rightclaw/src/init.rs` — init_rightclaw_home signature and structure — lines 1-80 inspected
- `.planning/phases/43-init-detection-up-revalidation/43-CONTEXT.md` — all locked decisions

### Secondary (MEDIUM confidence)
- `.planning/REQUIREMENTS.md` — CHROME-01 through INJECT-03 exact specifications

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all libraries already in use; no new deps needed
- Architecture: HIGH — established patterns in codebase; exact insertion points identified
- Pitfalls: HIGH — structural risk (tunnel block refactor) identified from direct code inspection

**Research date:** 2026-04-06
**Valid until:** 2026-05-06 (stable codebase, no fast-moving dependencies)
