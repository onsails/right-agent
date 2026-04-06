# Feature Research

**Domain:** chrome-devtools-mcp integration — browser MCP wired into RightClaw agents
**Researched:** 2026-04-05
**Confidence:** HIGH (tool names from official docs/source; sandbox gotcha confirmed in issue tracker)

## What chrome-devtools-mcp Exposes

The official package is `chrome-devtools-mcp` by the `ChromeDevTools` org (not any fork).
Version at research time: **0.21.0** (npm, April 2026).
Invocation: `npx -y chrome-devtools-mcp@latest [flags]`

### Full Tool Inventory (29 tools in 6 categories)

**Input Automation (9 tools)**

| Tool | What It Does | Key Parameters |
|------|-------------|----------------|
| `click` | Activate element on page | `uid` (from snapshot), `dblClick?`, `includeSnapshot?` |
| `drag` | Move element onto another | `from_uid`, `to_uid` |
| `fill` | Populate input / textarea / select | `uid`, `value` |
| `fill_form` | Fill multiple fields at once | `elements[]` (array of uid+value pairs) |
| `handle_dialog` | Respond to alert/confirm/prompt | `action` (accept/dismiss), `promptText?` |
| `hover` | Position cursor over element | `uid` |
| `press_key` | Send keyboard input | `key` (e.g. "Enter", "Control+A") |
| `type_text` | Type into focused input | `text`, `submitKey?` |
| `upload_file` | Submit file via input element | `filePath`, `uid` |

**Navigation (6 tools)**

| Tool | What It Does | Key Parameters |
|------|-------------|----------------|
| `navigate_page` | Load URL / back / forward / reload | `type` (url/back/forward/reload), `url?`, `timeout?`, `initScript?` |
| `new_page` | Open new tab | `url`, `background?`, `isolatedContext?` |
| `list_pages` | List all open tabs | none |
| `select_page` | Switch active tab | `pageId`, `bringToFront?` |
| `close_page` | Close a tab | `pageId` |
| `wait_for` | Wait until text appears on page | `text[]`, `timeout?` |

**Emulation (2 tools)**

| Tool | What It Does | Key Parameters |
|------|-------------|----------------|
| `emulate` | Simulate device, network, geolocation | `viewport?`, `colorScheme?`, `networkConditions?`, `cpuThrottlingRate?` |
| `resize_page` | Set viewport dimensions | `width`, `height` |

**Performance (4 tools)**

| Tool | What It Does | Key Parameters |
|------|-------------|----------------|
| `performance_start_trace` | Begin recording performance metrics | `reload?`, `autoStop?`, `filePath?` |
| `performance_stop_trace` | End active trace recording | `filePath?` |
| `performance_analyze_insight` | Get detail on a specific metric | `insightName`, `insightSetId` |
| `take_memory_snapshot` | Capture heap snapshot | `filePath` (required, .heapsnapshot) |

**Network (2 tools)**

| Tool | What It Does | Key Parameters |
|------|-------------|----------------|
| `list_network_requests` | Show HTTP activity (paginated) | `resourceTypes[]?`, `pageIdx?`, `pageSize?` |
| `get_network_request` | Get full request/response detail | `reqid?`, `requestFilePath?`, `responseFilePath?` |

**Debugging (6 tools)**

| Tool | What It Does | Key Parameters |
|------|-------------|----------------|
| `take_snapshot` | Export accessibility tree (semantic DOM) | `verbose?`, `filePath?` |
| `take_screenshot` | Capture visual render | `uid?`, `fullPage?`, `format?`, `quality?`, `filePath?` |
| `evaluate_script` | Execute JS in page context | `function` (required), `args[]?` |
| `list_console_messages` | Get console output (paginated) | `types[]?`, `pageIdx?`, `pageSize?` |
| `get_console_message` | Get specific console entry | `msgid` |
| `lighthouse_audit` | Run Lighthouse accessibility/SEO/perf | `mode?` (navigation/snapshot), `device?`, `outputDirPath?` |

### Slim Mode (3 tools, ~359 tokens)

When `--slim --headless` flags are used, only 3 tools are exposed:

| Tool | What It Does |
|------|-------------|
| `navigate` | Load a URL |
| `screenshot` | Take a screenshot (no params required) |
| `evaluate` | Evaluate a JS script |

Slim mode is for token-budget-constrained agents doing basic verification only.

### Resources and Prompts

chrome-devtools-mcp exposes **no MCP resources and no MCP prompts** beyond tools. It is tools-only.
The tool reference docs are external (GitHub docs), not served as MCP resources.

---

## Feature Landscape

### Table Stakes (Agents Expect These)

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Navigate to URL | Most basic browser operation | LOW | `navigate_page` with `type:"url"` |
| Take screenshot | Verify visual result of actions | LOW | `take_screenshot`, saves to `filePath` |
| Inspect page structure | Find elements to interact with | LOW | `take_snapshot` returns accessibility tree; `uid` values are required by all input tools |
| Click elements | Interact with buttons, links | LOW | `click` requires `uid` from snapshot |
| Fill forms | Submit data | LOW | `fill` or `fill_form` for batch |
| Execute JS | Scrape data, check state | MEDIUM | `evaluate_script` — runs in page context |
| Check console errors | Verify page health after action | LOW | `list_console_messages` + filter by `types: ["error"]` |
| Wait for async content | Handle dynamic page loads | LOW | `wait_for` with text array |

### Differentiators (Beyond Basic Automation)

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Performance tracing | Measure LCP, CLS, FID without external tools | MEDIUM | `performance_start_trace` → `performance_stop_trace` → `performance_analyze_insight` chain |
| Network request inspection | Debug API calls, check CORS, inspect payloads | MEDIUM | `list_network_requests` + `get_network_request` |
| Lighthouse audit | Accessibility + SEO + perf in one call | LOW | `lighthouse_audit` — one tool, comprehensive report |
| Heap snapshot | Memory leak detection | HIGH | `take_memory_snapshot` — agent must also analyze the file |
| Device/network emulation | Test mobile viewports + slow network conditions | LOW | `emulate` — useful for responsive-testing agents |
| Multi-tab management | Work across several pages simultaneously | MEDIUM | `new_page` + `select_page` + `close_page` |
| Isolated contexts | Separate cookie/storage per task | MEDIUM | `isolatedContext` param in `new_page` |

### Anti-Features (Do Not Build)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Auto-launch Chrome inside CC sandbox | Convenience | Chrome requires its own seccomp/namespace sandbox (bwrap blocks it). Chrome exits immediately or fails silently. | Launch Chrome outside agent sandbox — either as a dedicated process-compose process or let MCP server spawn it via `--executablePath` with sandbox exclusion for the Chrome binary |
| Storing Chrome path per-agent in agent.yaml | Per-agent customization | Path is system-level, not per-agent; creates config sprawl | Single path in `~/.rightclaw/config.yaml`, injected uniformly into all `.mcp.json` |
| Using `--autoConnect` in generated mcp.json | Simpler config | Requires Chrome 144+ with remote debugging enabled manually in `chrome://inspect`. Fails silently for users who haven't done this setup. | Use `--executablePath` + `--headless` so the MCP server controls Chrome lifecycle |
| Enabling CrUX telemetry (default-on) | Default behavior | Sends trace URLs to Google's CrUX API — privacy risk for agents browsing internal/sensitive URLs | Always inject `--no-performance-crux` into mcp.json args |
| Sharing the default user-data-dir across agents | Simpler | Browser state (cookies, localStorage) leaks across agents — breaks isolation | Use `--isolated` flag for ephemeral profiles, or `--userDataDir` pointing into agent dir |

---

## Feature Dependencies

```
take_snapshot (produces uid values)
    └──required before──> click, fill, fill_form, hover, drag, upload_file, type_text

navigate_page (load target URL)
    └──precedes──> take_snapshot, take_screenshot, list_console_messages

performance_start_trace
    └──requires completion by──> performance_stop_trace
                                     └──feeds──> performance_analyze_insight

new_page (produces pageId)
    └──required by──> select_page, close_page

Chrome process running outside bubblewrap
    └──required for──> ALL tools
      (MCP stdio process runs inside agent; Chrome itself cannot spawn inside bwrap)
```

### Dependency Notes

- **`take_snapshot` required before input tools:** All interaction tools (`click`, `fill`, etc.) take a `uid` — an opaque element identifier from the accessibility tree. Agents must call `take_snapshot` first to discover what is on the page.
- **`navigate_page` before everything else:** Chrome starts with about:blank or the last URL. Agents must navigate before inspecting.
- **Chrome outside sandbox:** The chrome-devtools-mcp stdio process (started by CC as an MCP server) can run inside the agent sandbox, but it must connect to a Chrome instance running outside bwrap. This is the central integration constraint for RightClaw.

---

## MVP Definition

### Launch With (v3.4 — this milestone)

- [ ] Chrome path detection in `rightclaw init` — standard paths on Linux + macOS incl. Homebrew; `--chrome-path` CLI override
- [ ] Chrome path saved to `~/.rightclaw/config.yaml`
- [ ] `chrome-devtools-mcp` entry injected into agent `.mcp.json` on `rightclaw up` when Chrome is configured
- [ ] Generated entry uses `--executablePath` + `--headless` + `--no-performance-crux` + `--isolated` flags
- [ ] `rightclaw doctor` checks Chrome binary at configured path (Warn severity if absent/unconfigured)
- [ ] AGENTS.md template gets browser tool usage section (see below)

### Add After Validation (v3.4.x)

- [ ] Bot startup validates Chrome path — log error if absent but don't fail startup
- [ ] `--browserUrl` connect-to-running mode for debugging with visible DevTools UI
- [ ] Per-agent `browser_disabled: true` in `agent.yaml` to opt out of Chrome injection

### Future Consideration (v3.5+)

- [ ] Chrome as dedicated process-compose process (persistent, faster reconnect, cleanly outside all agent sandboxes)
- [ ] Slim mode option for agents that only need navigate/screenshot/evaluate

---

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Chrome path in config + .mcp.json injection | HIGH | LOW | P1 |
| Safe default flags (headless, isolated, no-crux) | HIGH | LOW | P1 |
| `rightclaw doctor` Chrome check | HIGH | LOW | P1 |
| AGENTS.md browser section | HIGH | LOW | P1 |
| Bot startup Chrome validation | MEDIUM | LOW | P2 |
| `--browserUrl` connect mode | MEDIUM | MEDIUM | P2 |
| Per-agent `browser_disabled` opt-out | LOW | MEDIUM | P3 |
| Slim mode option | LOW | LOW | P3 |

---

## System Prompt Section for AGENTS.md

The browser section should instruct agents with these exact workflow patterns:

```markdown
## Browser Automation

Use the `chrome-devtools-mcp` MCP tools for all browser tasks. Chrome runs headless.

**Standard workflow:**
1. `navigate_page` — load the target URL
2. `take_snapshot` — inspect page structure and get element `uid` values
3. `click` / `fill` / `fill_form` — interact using `uid` from the snapshot
4. `take_screenshot` — verify visual result (saves to filePath)
5. `list_console_messages` with `types: ["error"]` — check for JS errors

**Key rules:**
- All input tools (`click`, `fill`, `hover`, etc.) require a `uid` from `take_snapshot` — never guess UIDs, always snapshot first
- Call `wait_for` before inspecting content that loads asynchronously
- Save screenshots and traces to files via `filePath` param — raw data is not returned inline
- `evaluate_script` takes a function string, not a bare expression: `"function() { return document.title; }"`
- `uid` values are not stable — re-snapshot after navigation or significant DOM changes

**Debugging patterns:**
- Page blank or unexpected? → `list_console_messages` with `types: ["error"]`
- Need to inspect an API call? → `list_network_requests` then `get_network_request`
- Performance issue? → `performance_start_trace` (with `reload: true`) → interact → `performance_stop_trace` → `performance_analyze_insight`
- Accessibility/SEO check? → `lighthouse_audit` with `mode: "snapshot"`
```

---

## Known Limitations and Gotchas

### Critical: Chrome Cannot Launch Inside Bubblewrap

**What goes wrong:** Chrome requires its own seccomp/namespace sandbox. Bubblewrap (CC's sandbox on Linux) blocks Chrome's sandbox creation. Chrome exits immediately with "Running as root without --no-sandbox is not supported" or silently fails to connect.

**Resolution (PR #338, merged Oct 2025):** The MCP server now supports passing arbitrary Chrome args. This allows `--no-sandbox` to be threaded through, but the maintainers explicitly warn this is a security tradeoff and cannot recommend it by default.

**RightClaw approach:** Chrome binary path must be added to `allowedCommands` in the agent's sandbox settings, or Chrome must be launched as a process-compose process outside all agent sandboxes. The `.mcp.json` approach (MCP server spawns Chrome) requires the Chrome binary to be reachable from inside the MCP server's process — which runs inside bwrap. Test required to determine if bwrap allows child processes to escape to a separate namespace.

Source: [Issue #261](https://github.com/ChromeDevTools/chrome-devtools-mcp/issues/261), resolved in PR #338.

### UID Instability

Snapshot `uid` values are not stable across page navigations or significant DOM changes. Agents must re-call `take_snapshot` after navigation. Never cache `uid` between steps or across tool calls.

### Browser Not Started on MCP Connection

chrome-devtools-mcp does NOT launch Chrome when the MCP connection opens. Chrome starts on the first tool call that requires it. First browser tool call has cold-start latency (~1-3s). Subsequent calls are fast.

### Node.js Version Requirement

Requires Node.js v20.19+. v22.12.0+ and v24.9.0+ confirmed working. Older node versions cause silent failures at MCP connection time. RightClaw `rightclaw doctor` should check node version when Chrome is configured.

### `evaluate_script` Takes a Function, Not a Statement

The `function` parameter must be a function expression: `"function() { return document.title; }"` — not a bare JS expression. Agents passing bare expressions get a parse error.

### CrUX Telemetry Is On By Default

Performance tools send trace URLs to Google's CrUX API by default. For agents browsing internal/sensitive URLs this is a privacy leak. Always inject `--no-performance-crux` into the generated `.mcp.json` args.

### Screenshots and Traces Are File-Based

`take_screenshot`, `performance_stop_trace`, `take_memory_snapshot` write results to files (`filePath` param). They do not return inline data by default. Agents must specify a `filePath` and then read that file. This is by design ("Reference over Value" in design principles).

### macOS: Seatbelt Interaction

On macOS, CC uses Seatbelt (Apple sandbox-exec). Chrome also uses Seatbelt internally. Two nested Seatbelt invocations may conflict. The `--browserUrl` connect-to-running approach (Chrome started outside any sandbox) is safer than auto-spawn on macOS.

### `--autoConnect` Requires Chrome 144+ Remote Debugging Manually Enabled

`--autoConnect` flag requires the user to have enabled "Enable remote debugging" in `chrome://inspect/#remote-debugging`. This is not a default Chrome state. Do not use this flag in generated `.mcp.json` without user setup.

### Plugin Cache vs Generated .mcp.json

When installed via Claude Code's plugin system, chrome-devtools-mcp config lives in the plugin cache and gets overwritten on plugin updates. RightClaw's approach — injecting via `.mcp.json` on `rightclaw up` — avoids this. The source of truth is `agent.yaml` → config.yaml → generated `.mcp.json`.

---

## Sources

- [chrome-devtools-mcp tool reference (official)](https://github.com/ChromeDevTools/chrome-devtools-mcp/blob/main/docs/tool-reference.md) — HIGH confidence
- [chrome-devtools-mcp README + CLI flags](https://github.com/ChromeDevTools/chrome-devtools-mcp) — HIGH confidence
- [chrome-devtools-mcp slim tool reference](https://github.com/ChromeDevTools/chrome-devtools-mcp/blob/main/docs/slim-tool-reference.md) — HIGH confidence
- [Chrome for Developers blog: DevTools MCP announcement](https://developer.chrome.com/blog/chrome-devtools-mcp) — HIGH confidence
- [design-principles.md](https://github.com/ChromeDevTools/chrome-devtools-mcp/blob/main/docs/design-principles.md) — HIGH confidence (uid/snapshot workflow, file-based output design)
- [Issue #261: Headless isolated launch fails as root](https://github.com/ChromeDevTools/chrome-devtools-mcp/issues/261) — HIGH confidence (sandbox conflict confirmed + PR #338 resolution)
- [Issue #182: Cannot connect with Claude Code](https://github.com/ChromeDevTools/chrome-devtools-mcp/issues/182) — HIGH confidence (Node.js version requirement confirmed)
- [samwize.com: Chrome DevTools MCP for Claude Code](https://samwize.com/2026/03/26/how-to-set-up-chrome-devtools-mcp-for-claude-code/) — MEDIUM confidence (practical config patterns, `--autoConnect` gotcha)

---
*Feature research for: chrome-devtools-mcp integration — RightClaw v3.4*
*Researched: 2026-04-05*
