# Stack Research: v3.4 Chrome Integration

**Domain:** Chrome browser MCP integration for RightClaw (Rust CLI, multi-agent runtime)
**Researched:** 2026-04-05
**Confidence:** HIGH for npm package identity and invocation; MEDIUM for Chrome path coverage

## Scope

Delta-research for the v3.4 milestone. Covers ONLY what is new:
1. chrome-devtools-mcp package identity, version, and invocation
2. Chrome binary paths for Linux (apt, snap) and macOS (standard, Homebrew)
3. CLI flags and env vars for Chrome path configuration
4. Whether a Rust crate exists for Chrome detection

Existing stack (tokio, serde, reqwest, rusqlite, teloxide, which, etc.) is NOT re-evaluated.

---

## Package Identity

**Package:** `chrome-devtools-mcp`
**npm org:** ChromeDevTools (official Google/Chrome team)
**Latest version:** 0.21.0 (published 2026-04-03, 3 days ago)
**Weekly downloads:** 447,568 — classified "Influential project" by npm
**Node.js requirement:** v20.19 or newer LTS
**GitHub:** https://github.com/ChromeDevTools/chrome-devtools-mcp

This is the authoritative package. There are forks (`@mcp-b/chrome-devtools-mcp`, `@daohoangson/chrome-devtools-mcp`, `mcp-chromedevtools`) but none have meaningful adoption or official backing. Use the unscoped `chrome-devtools-mcp`.

---

## Invocation Command

```json
{
  "command": "npx",
  "args": ["-y", "chrome-devtools-mcp@latest"]
}
```

The `-y` flag suppresses npm prompts. `@latest` ensures the most recent stable version. This is the canonical form confirmed by the official Chrome for Developers blog post and the package README.

**With Chrome path configured:**
```json
{
  "command": "npx",
  "args": [
    "-y",
    "chrome-devtools-mcp@latest",
    "--executablePath", "/usr/bin/google-chrome-stable",
    "--headless"
  ]
}
```

---

## CLI Flags (Relevant to RightClaw)

| Flag | Alias | Type | Default | Purpose |
|------|-------|------|---------|---------|
| `--executablePath` | `-e`, `--executable-path` | string | auto-detect | Absolute path to Chrome binary. Primary config point. |
| `--headless` | — | boolean | false | Run Chrome without UI. Set true for agent use. |
| `--userDataDir` | `--user-data-dir` | string | `$HOME/.cache/chrome-devtools-mcp/chrome-profile` | Chrome profile directory. |
| `--channel` | — | string | stable | Chrome channel when not using `--executablePath`. Values: stable, canary, beta, dev. |
| `--browserUrl` | `-u`, `--browser-url` | string | — | Connect to already-running Chrome (skip launch). Advanced use. |

**No `CHROME_PATH` env var.** The package does NOT read `CHROME_PATH` from the environment. Chrome path MUST be passed as `--executablePath` in the args array. Setting `CHROME_PATH` in the `.mcp.json` env section is silently ignored.

**Env vars that DO affect behavior:**
- `CHROME_DEVTOOLS_MCP_NO_USAGE_STATISTICS` — disables telemetry
- `CI` — disables telemetry automatically
- `DEBUG=*` — verbose logging

---

## Chrome Binary Paths

### Linux — Probe Order

| Path | Method | Notes |
|------|--------|-------|
| `/usr/bin/google-chrome-stable` | APT (google-chrome-stable deb) | Most common on Debian/Ubuntu with Google official repo |
| `/usr/bin/google-chrome` | APT (symlink) | Symlink to stable, present on most Google Chrome apt installs |
| `/usr/bin/chromium-browser` | APT (Ubuntu chromium-browser package) | Pre-20.04 Ubuntu standard |
| `/usr/bin/chromium` | APT (Debian, Fedora, Arch) | Alternate name; Fedora/Arch use `chromium` not `chromium-browser` |
| `/snap/bin/chromium` | Snap (Ubuntu 20.04+ default) | Ubuntu ships Chromium via snap since 20.04; `apt install chromium-browser` now redirects to snap |

**Snap caveat:** `/snap/bin/chromium` is a wrapper script, not the actual binary. It has its own `/tmp` and IPC namespace. `chrome-devtools-mcp` spawns Chrome as a child process, so snap Chromium should work. Flag it in doctor output when snap path is detected.

**PATH fallbacks via `which::which()`** (use after hardcoded list fails):
- `google-chrome-stable`
- `google-chrome`
- `chromium-browser`
- `chromium`

### macOS — Probe Order

| Path | Method | Notes |
|------|--------|-------|
| `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` | Standard DMG / `brew install --cask google-chrome` | Universal — both Intel and ARM. Homebrew cask installs here regardless of Homebrew prefix. |
| `/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary` | Canary DMG | Developer machines |
| `/Applications/Chromium.app/Contents/MacOS/Chromium` | `brew install --cask chromium` | Less common |
| `$HOME/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` | Per-user install | When user installs to ~/Applications |

**Homebrew Intel vs ARM:** `brew install --cask google-chrome` places the `.app` in `/Applications/` on both Intel (`/usr/local/`) and ARM (`/opt/homebrew/`). The binary path is identical — no platform split needed for macOS Chrome paths.

---

## No Rust Crate for Chrome Detection

There is no dedicated Rust crate for Chrome path detection. The Node.js ecosystem has `chrome-launcher` (by Google) which does this, but it has no Rust equivalent as a standalone crate.

Existing Rust browser automation crates (`chromiumoxide`, `headless_chrome`, `spider_chrome`) all implement manual path probing internally — a hardcoded list + system PATH lookup. This is the standard pattern.

RightClaw already has `which` as a workspace dependency. Use it for PATH fallbacks after probing the hardcoded list. No new crates required.

---

## .mcp.json Integration Shape

Extends `generate_mcp_config()` in `crates/rightclaw/src/codegen/mcp_config.rs`. The Chrome entry follows the same merge-and-upsert pattern as `rightmemory`.

```json
{
  "mcpServers": {
    "rightmemory": { "...existing entry..." },
    "chrome-devtools": {
      "command": "npx",
      "args": [
        "-y",
        "chrome-devtools-mcp@latest",
        "--executablePath", "<chrome_path from config.yaml>",
        "--headless"
      ]
    }
  }
}
```

**When Chrome is not configured:** omit the `chrome-devtools` entry entirely. An entry with a missing `--executablePath` causes `npx` to fail and produces confusing agent errors.

---

## Alternatives Considered

| Recommended | Alternative | Why Not |
|-------------|-------------|---------|
| `chrome-devtools-mcp` | `@mcp-b/chrome-devtools-mcp` | Fork with no unique value, far lower adoption |
| `chrome-devtools-mcp` | `mcp-chromedevtools` | Unrelated third-party, no official backing |
| `npx -y chrome-devtools-mcp@latest` | Pin `chrome-devtools-mcp@0.21.0` | Agents stuck on stale APIs; the package evolves frequently (0.21.0 just dropped) |
| `--executablePath` flag | `--channel stable` | `--channel` relies on package's own detection which is unreliable on Linux snap/nix. Explicit path is safer. |

---

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `CHROME_PATH` in `.mcp.json` env section | Package ignores this env var. Silently does nothing. | `--executablePath` in `args` array |
| Injecting entry when Chrome path is unconfigured | npx invocation fails, agent sees broken MCP tool | Skip injection; doctor warns instead |
| Flatpak Chrome path | Chrome is not officially on Flathub; path is user-specific and unreliable | Probe standard APT/snap paths; allow `--chrome-path` override |

---

## No New Rust Crates Required

| What | How |
|------|-----|
| Chrome path probing (hardcoded list) | `std::path::Path::exists()` — stdlib |
| Chrome PATH fallbacks | `which::which()` — already in workspace |
| Config persistence | `~/.rightclaw/config.yaml` — already exists via serde-saphyr |
| .mcp.json injection | `generate_mcp_config()` in mcp_config.rs — extend existing function |

---

## Sources

- [chrome-devtools-mcp npm](https://www.npmjs.com/package/chrome-devtools-mcp) — v0.21.0, 447k downloads/week, HIGH confidence
- [ChromeDevTools/chrome-devtools-mcp GitHub README](https://github.com/ChromeDevTools/chrome-devtools-mcp) — `--executablePath`, `--headless`, `--channel` flags, HIGH confidence
- [Chrome DevTools MCP blog](https://developer.chrome.com/blog/chrome-devtools-mcp) — canonical `npx -y chrome-devtools-mcp@latest` invocation, HIGH confidence
- [chrome-devtools-mcp troubleshooting.md](https://github.com/ChromeDevTools/chrome-devtools-mcp/blob/main/docs/troubleshooting.md) — `--executablePath` WSL usage patterns, MEDIUM confidence
- [GoogleChrome/chrome-launcher chrome-finder.ts](https://github.com/GoogleChrome/chrome-launcher/blob/main/src/chrome-finder.ts) — Linux/macOS probe path list reference, MEDIUM confidence (chrome-launcher uses puppeteer-core not chrome-launcher internally, but platform path landscape identical)
- [Ubuntu snap Chromium issue tracking](https://github.com/SeleniumHQ/selenium/issues/7788) — `/snap/bin/chromium` path confirmed, MEDIUM confidence

---
*Stack research for: RightClaw v3.4 Chrome Integration*
*Researched: 2026-04-05*
