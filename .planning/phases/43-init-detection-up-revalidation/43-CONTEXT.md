# Phase 43: Init Detection + Up Revalidation - Context

**Gathered:** 2026-04-06
**Status:** Ready for planning

<domain>
## Phase Boundary

Chrome path is discovered at `rightclaw init` and revalidated silently on every `rightclaw up` — operators never lose injection silently. Covers: Chrome + MCP binary auto-detection at init, `--chrome-path` CLI override, non-fatal detection, config write restructuring, and per-run revalidation in cmd_up.

Doctor check and AGENTS.md template are Phase 44. Per-agent Chrome opt-out (`chrome.enabled: false` in agent.yaml) is deferred to v3.5.

</domain>

<decisions>
## Implementation Decisions

### Chrome binary detection (CHROME-01)
- **D-01:** At `rightclaw init`, auto-detect Chrome by checking standard paths in order:
  - Linux: `/usr/bin/google-chrome-stable`, `/usr/bin/google-chrome`, `/usr/bin/chromium-browser`, `/usr/bin/chromium`, `/snap/bin/chromium`
  - macOS: `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`, `~/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`
- **D-02:** `--chrome-path <path>` CLI override takes precedence over auto-detection; the provided path is used as-is without existence check at arg parse time (validated implicitly when written to config)

### chrome-devtools-mcp binary detection
- **D-03:** `mcp_binary_path` discovered via `which::which("chrome-devtools-mcp")` first, then fallback to standard paths:
  - Linux + macOS: `/usr/local/bin/chrome-devtools-mcp`, `~/.npm-global/bin/chrome-devtools-mcp`
  - macOS additionally: `$(brew --prefix)/bin/chrome-devtools-mcp` (resolved via `std::process::Command::new("brew").arg("--prefix")`)
- **D-04:** No `npx` — absolute path to globally-installed binary only (prior decision: "Never use npx in .mcp.json")

### Partial detection handling (CHROME-03)
- **D-05:** If Chrome binary not found: `tracing::warn!`, skip writing chrome section entirely, init continues normally
- **D-06:** If Chrome binary found but `chrome-devtools-mcp` not found: `tracing::warn!` about missing MCP binary, skip writing chrome section entirely — same behavior as no Chrome. Chrome config only written when BOTH paths are resolved.
- **D-07:** If `--chrome-path` provided but `chrome-devtools-mcp` not found: same as D-06 — warn, skip chrome section. The `--chrome-path` override only overrides the Chrome binary search; MCP binary is always auto-detected.

### Config write restructuring
- **D-08:** Detect Chrome early in `cmd_init()` (before entering the tunnel block), store in `let chrome_cfg: Option<ChromeConfig>`.
- **D-09:** The tunnel block no longer writes config itself. Remove the `write_global_config` call from inside the tunnel block. Instead, collect `let tunnel_cfg: Option<TunnelConfig>` from the tunnel block.
- **D-10:** Write `GlobalConfig { tunnel: tunnel_cfg, chrome: chrome_cfg }` once at the very end of `cmd_init()` — single write path regardless of which combination of tunnel/chrome was detected.
- **D-11:** If neither tunnel nor chrome was detected, still write the config (empty `GlobalConfig::default()`) so the file exists for future `rightclaw up` reads. No-op functionally but avoids first-run surprises.

### Up revalidation (INJECT-03)
- **D-12:** In `cmd_up()`, after extracting `chrome_cfg = global_cfg.chrome.as_ref()`: check that both `chrome_cfg.chrome_path.exists()` AND `chrome_cfg.mcp_binary_path.exists()`.
- **D-13:** If either path is missing: `tracing::warn!` with the missing path, set effective `chrome_cfg = None` for this run. Agents start normally, injection is skipped.
- **D-14:** Revalidation is per-run (every `rightclaw up`), not cached. The config.yaml stored paths are trusted as the source of truth; disk existence is re-checked each time.

### Claude's Discretion
- Exact warning message wording for each detection failure case
- Whether to extract Chrome/MCP detection into helper functions in `init.rs` or keep inline in `cmd_init()`
- Brew prefix detection: whether to cache the result or call brew once per detection attempt

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — CHROME-01, CHROME-02, CHROME-03, INJECT-03 (exact standard path lists, override behavior, non-fatal spec, revalidation spec)

### Init command
- `crates/rightclaw-cli/src/main.rs` — `cmd_init()` (~line 250): current tunnel detection + config write flow to restructure; `Init` struct (~line 93): add `--chrome-path` arg here; `detect_cloudflared_cert()` pattern to mirror for Chrome detection

### Init logic
- `crates/rightclaw/src/init.rs` — `init_rightclaw_home()`: Chrome detection helper may live here or in a new `chrome_detect` module in the rightclaw crate

### Config types
- `crates/rightclaw/src/config.rs` — `GlobalConfig`, `ChromeConfig`, `TunnelConfig`, `write_global_config()`: ChromeConfig already has both fields; write function already handles both sections

### Up command
- `crates/rightclaw-cli/src/main.rs` — `cmd_up()` lines ~616-620: `chrome_cfg` extraction point where revalidation check slots in before the per-agent loop

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `detect_cloudflared_cert()` / `detect_cloudflared_cert_with_home()` in `main.rs`: exact pattern to mirror for Chrome binary detection — testable variant takes explicit home dir
- `which::which()` already used in `cmd_up()` for `cloudflared` binary check; same crate for MCP binary detection
- `write_global_config()` in `config.rs`: already serializes both `tunnel` and `chrome` sections; no changes needed to this function
- `ChromeConfig` struct in `config.rs`: already has both required fields, already deserialized from YAML

### Established Patterns
- **Testable detection helpers**: `detect_cloudflared_cert_with_home(home: &Path)` pattern — Chrome detection should follow this (pass home dir explicitly for test isolation)
- **Non-fatal warn**: `tracing::warn!` + continue, never `return Err(...)` for optional feature detection
- **Single config write at end**: tunnel block collects `TunnelConfig`, main body collects `ChromeConfig`, `GlobalConfig` assembled and written once

### Integration Points
- `cmd_init()` in `main.rs`: two changes — (1) add Chrome detection before tunnel block, (2) restructure config write from inside-tunnel to after-tunnel
- `cmd_up()` in `main.rs` ~line 619: two-path check (`chrome_path.exists() && mcp_binary_path.exists()`) before passing `chrome_cfg` to generators

</code_context>

<specifics>
## Specific Ideas

- MCP binary standard path list (Linux + macOS): `/usr/local/bin/chrome-devtools-mcp`, `~/.npm-global/bin/chrome-devtools-mcp`; macOS additionally checks brew prefix
- Detection order for Chrome binary: check paths in the order listed in CHROME-01 requirements, stop at first found
- `--chrome-path` wires into `Init` struct as `Option<PathBuf>` — if `Some`, skip auto-detection entirely, use the provided path directly as `chrome_path`

</specifics>

<deferred>
## Deferred Ideas

- Per-agent `chrome.enabled: false` opt-out in `agent.yaml` — deferred to v3.5 (out of scope per PROJECT.md)
- Chrome process as a separate process-compose service — v3.5 (CHROME-EXT-01)
- Using `npx chrome-devtools-mcp` instead of installed binary — explicitly rejected (never use npx in .mcp.json)

### Reviewed Todos (not folded)
- "Document CC gotcha — Telegram messages dropped while agent is streaming" — unrelated to Phase 43 Chrome detection work

</deferred>

---

*Phase: 43-init-detection-up-revalidation*
*Context gathered: 2026-04-06*
