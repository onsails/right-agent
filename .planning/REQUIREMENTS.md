# Requirements: RightClaw v3.4 Chrome Integration

**Defined:** 2026-04-06
**Core Value:** Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, each with its own sandbox configuration and identity, orchestrated by a single CLI command.

## v3.4 Requirements

### CHROME — Detection & Config

- [x] **CHROME-01**: Operator can run `rightclaw init` and have Chrome auto-detected at standard paths (Linux: `/usr/bin/google-chrome-stable`, `/usr/bin/google-chrome`, `/usr/bin/chromium-browser`, `/usr/bin/chromium`, `/snap/bin/chromium`; macOS: `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`, `~/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`); detected path saved to `~/.rightclaw/config.yaml` under `chrome.chrome_path`
- [x] **CHROME-02**: Operator can pass `--chrome-path <path>` to `rightclaw init` to override auto-detection; provided path saved to config
- [x] **CHROME-03**: Chrome detection is non-fatal — no Chrome found logs a warn and skips saving chrome config; init continues normally

### INJECT — MCP Entry Generation

- [x] **INJECT-01**: `rightclaw up` injects a `chrome-devtools` entry into per-agent `.mcp.json` when `chrome.chrome_path` is set in `~/.rightclaw/config.yaml`; entry uses absolute path to globally-installed `chrome-devtools-mcp` binary (never `npx`)
- [x] **INJECT-02**: Generated `.mcp.json` chrome-devtools entry passes `--executablePath <chrome_path>`, `--headless`, `--isolated`, `--no-sandbox` (bubblewrap outer sandbox), and `--userDataDir <agent_dir>/.chrome-profile` as args
- [x] **INJECT-03**: Chrome path is revalidated on every `rightclaw up`; if configured path no longer exists, logs warn and skips injection for that run (does not abort)

### SBOX — Sandbox Settings

- [x] **SBOX-01**: `generate_settings()` adds Chrome sandbox overrides to per-agent `settings.json` when Chrome is configured: Chrome binary path added to `allowedCommands`; agent `chrome-profile` dir added to `allowWrite`
- [x] **SBOX-02**: Chrome sandbox overrides are additive — merged with existing `SandboxOverrides` from `agent.yaml` using the same `Vec::extend` pattern as existing overrides

### VALID — Doctor + Bot + Up Validation

- [ ] **VALID-01**: `rightclaw doctor` includes a `check_chrome()` check — verifies Chrome binary exists at configured path; Warn severity if missing or unconfigured; skipped if chrome not in config
- [ ] **VALID-02**: Bot process startup validates Chrome configuration — logs `tracing::warn!` if Chrome is configured but binary missing; logs `tracing::debug!` if Chrome is not configured at all

### AGENT — System Prompt Template

- [ ] **AGENT-01**: `templates/right/AGENTS.md` and `identity/AGENTS.md` include a "Browser Automation" section instructing agents to: use ChromeDevTools MCP for all browser tasks; call `navigate_page` then `take_snapshot` before any interaction; use `uid` from snapshot for `click`/`fill`/`hover`; use `take_screenshot` to verify results

## Future Requirements

### External Chrome Process

- **CHROME-EXT-01**: Chrome runs as a dedicated process-compose process outside all agent sandboxes; agents connect via `--browserUrl http://127.0.0.1:9222` — deferred to v3.5 (cleaner architecture, requires process-compose integration work)

### Per-Agent Chrome Toggle

- **CHROME-AGENT-01**: Per-agent `agent.yaml` `chrome.enabled: false` field to opt individual agents out of Chrome MCP injection — deferred, global on/off is sufficient for v3.4

## Out of Scope

| Feature | Reason |
|---------|--------|
| Chrome as process-compose entry | v3.5 work — external Chrome approach requires additional integration |
| ngrok / other tunnel for Chrome redirect | Not relevant to browser automation |
| Chrome version enforcement / pinning | Warn-only for v3.4; hard requirement deferred |
| Slim mode (3-tool subset) | Full tool set is correct default; slim mode adds config complexity |
| `--autoConnect` mode | Requires manual Chrome setup; not compatible with headless agent model |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| CHROME-01 | Phase 43 | Complete |
| CHROME-02 | Phase 43 | Complete |
| CHROME-03 | Phase 43 | Complete |
| INJECT-01 | Phase 42 | Complete |
| INJECT-02 | Phase 42 | Complete |
| INJECT-03 | Phase 43 | Complete |
| SBOX-01 | Phase 42 | Complete |
| SBOX-02 | Phase 42 | Complete |
| VALID-01 | Phase 44 | Pending |
| VALID-02 | Phase 44 | Pending |
| AGENT-01 | Phase 44 | Pending |

**Coverage:**
- v3.4 requirements: 11 total
- Mapped to phases: 11 ✓
- Unmapped: 0

---
*Requirements defined: 2026-04-06*
