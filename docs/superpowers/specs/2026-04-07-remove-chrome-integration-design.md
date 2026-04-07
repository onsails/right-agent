# Remove Chrome/Browser Integration

**Date:** 2026-04-07
**Status:** Approved
**Scope:** Delete all Chrome DevTools MCP integration code, config, detection, and tests.

## Motivation

Chrome DevTools MCP (`chrome-devtools-mcp`) only supports stdio transport. To run it as a shared service in process-compose (needed for OpenShell sandbox), an HTTP proxy is required — but no lightweight, well-maintained proxy exists. Rather than carry dead code or add a fragile dependency chain, remove Chrome integration entirely. Re-add when `chrome-devtools-mcp` gains native HTTP transport.

## What Gets Removed

### Source Code

1. **`crates/rightclaw/src/config.rs`**
   - `ChromeConfig` struct (lines 18-25)
   - `chrome: Option<ChromeConfig>` field in `GlobalConfig` (line 31)
   - `RawChromeConfig` struct (lines 53-58)
   - `chrome: Option<RawChromeConfig>` in `RawGlobalConfig` (line 49)
   - Chrome deserialization in `read_global_config()` (lines 106-112)
   - Chrome serialization in `write_global_config()` (lines 131-137)
   - All Chrome-related tests (~6 tests, lines 282-405)

2. **`crates/rightclaw/src/codegen/mcp_config.rs`**
   - `use crate::config::ChromeConfig` import
   - `chrome_config` parameter from `generate_mcp_config()` and `generate_mcp_config_http()`
   - Chrome-devtools entry injection block (lines 63-79)
   - `let _ = chrome_config;` in HTTP variant (line 135-136)
   - `.chrome-profile` directory logic
   - All Chrome-related tests (~6 tests, lines 338-535)

3. **`crates/rightclaw-cli/src/main.rs`**
   - `chrome_path` CLI arg from `Init` struct (line 113)
   - `detect_chrome_binary()` function (lines 478-512)
   - `detect_mcp_binary()` function (lines 533-560)
   - `detect_chrome_with_home()` function (lines 566-600)
   - `detect_chrome()` wrapper (lines 599-600)
   - `detect_chrome()` call in `cmd_init()` (line 363)
   - `chrome: chrome_cfg` in GlobalConfig construction (line 442)
   - Chrome revalidation block in `cmd_up()` (lines 764-783)
   - `chrome_cfg` passed to codegen functions
   - All Chrome detection tests (~4 tests, lines 1645-1737)
   - Chrome CLI integration tests (~2 tests, lines 322-371)

### Config

4. **`config.yaml`** — `chrome:` section no longer serialized/deserialized. Existing files with `chrome:` section are harmless (serde ignores unknown fields with `deny_unknown_fields` off, or we add `#[serde(flatten)]` if needed).

### Documentation

5. **`ARCHITECTURE.md`** — remove Chrome references from module map, data flow, config hierarchy.

### Planning docs

6. **`.planning/`** — leave as-is (historical record, not active code).

## What Stays

- Generic "browser" references in OAuth/login flow (not Chrome-specific)
- `.mcp.json` read-modify-write pattern (still used for `right` MCP entry)
- `generate_mcp_config()` / `generate_mcp_config_http()` functions themselves — just lose the chrome parameter

## Risk

None. Chrome MCP was optional with graceful degradation. No agent workflow depends on it. Removing it simplifies codegen and eliminates dead config paths.

## Migration

None required. Existing `config.yaml` files with `chrome:` sections will be silently ignored (unknown YAML keys are skipped by serde-saphyr default).
