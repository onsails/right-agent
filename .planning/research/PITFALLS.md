# Pitfalls Research

**Domain:** Chrome browser MCP integration in a bubblewrap/Seatbelt-sandboxed headless agent runtime
**Researched:** 2026-04-05
**Confidence:** HIGH for bubblewrap nested sandbox failures (confirmed from chrome-devtools-mcp official docs + CC sandbox-runtime source); HIGH for npx startup issues (multiple independent sources, 2025-2026); MEDIUM for Chrome version compatibility (official docs + puppeteer issue tracker); HIGH for optionality recommendation (architectural reasoning from existing codebase patterns)

---

## Critical Pitfalls

### Pitfall 1: bubblewrap Sandbox Blocks Chrome's Own Sandbox — Nested Sandbox Deadlock

**What goes wrong:**
Chrome requires its own sandbox (seccomp + user namespaces) to start safely. bubblewrap itself uses Linux user namespaces. When Chrome runs *inside* a bubblewrap sandbox, it attempts to create child namespaces for each renderer process. This nested namespace creation fails because the bubblewrap container does not grant `CAP_SYS_ADMIN` or `clone(CLONE_NEWUSER)` to its children unless explicitly configured.

Result: Chrome starts, then immediately crashes with `Failed to move to new namespace: PID namespaces supported, Network namespace supported, but failed to unshare user namespace`. The MCP server's npx process exits with a non-zero code and CC sees the tool as unavailable.

The official chrome-devtools-mcp troubleshooting doc states explicitly: "If sandboxes are enabled, `chrome-devtools-mcp` is not able to start Chrome" when the MCP client itself runs inside an OS sandbox.

**Why it happens:**
chrome-devtools-mcp launches Chrome via Puppeteer. Puppeteer does not pass `--no-sandbox` by default. Chrome's sandbox requires Linux user namespace support at a level bubblewrap's default policy denies. It is not a bug in bubblewrap or Chrome — it is an architectural conflict between two sandboxes trying to own namespace isolation.

**How to avoid:**
There are two valid approaches:

1. **Run Chrome outside the sandbox (preferred):** Launch a Chrome process outside the bubblewrap sandbox with `--remote-debugging-port=9222 --headless --no-first-run --disable-features=VizDisplayCompositor`. Then configure chrome-devtools-mcp with `--browser-url=http://127.0.0.1:9222` so the MCP server connects to the existing instance rather than launching its own. The MCP stdio server itself runs inside the sandbox; only Chrome runs outside. This matches the threat model — Chrome is sandboxed by its own mechanisms; the agent gets browser access via CDP, not direct filesystem access.

2. **Use `--no-sandbox` flag with Chrome:** Pass `--chromeArg=--no-sandbox` to chrome-devtools-mcp. This disables Chrome's own sandbox. Only acceptable because the agent is already inside bubblewrap — the outer sandbox provides isolation. Do NOT use this approach without an outer sandbox — it exposes the host to renderer exploits.

**Warning signs:**
- `chrome-devtools-mcp` MCP entry in `.mcp.json` but CC reports the tool as failing to initialize.
- Chrome process in ps output but exits within 1-2 seconds.
- stderr from the MCP process contains "Failed to move to new namespace" or "No usable sandbox".

**Phase to address:** Detection phase (init / doctor). The approach (external Chrome vs. --no-sandbox) must be decided before implementation, as it affects how the Chrome process is configured in process-compose.

---

### Pitfall 2: /dev/shm Missing or Too Small — Chrome Crashes Silently After Start

**What goes wrong:**
Chrome uses `/dev/shm` as shared memory for IPC between its processes. The default bubblewrap sandbox does not bind-mount `/dev/shm` unless explicitly configured. Without `/dev/shm`, Chrome starts, renders a page, then crashes when renderer processes try to communicate. The crash may appear as a timeout on the MCP tool call, not a startup error.

Even with `/dev/shm` present, bubblewrap may impose a size limit (tmpfs `--size=`) that is smaller than Chrome's minimum. Chrome needs at minimum 128MB; the CC sandbox defaults to a smaller tmpfs.

**Why it happens:**
CC's `settings.json` `sandbox` section controls bubblewrap configuration but does not automatically bind-mount `/dev/shm`. The developer tests Chrome outside the sandbox, sees it working, assumes the sandbox will work too.

**How to avoid:**
- Use the external-Chrome approach (Pitfall 1, option 1) — Chrome runs outside bubblewrap entirely, no `/dev/shm` issue inside the sandbox.
- If Chrome must run inside the sandbox: add `/dev/shm` to `SandboxOverrides.allow_write` paths AND add `--disable-dev-shm-usage` to Chrome flags so it falls back to `/tmp` (disk-backed shared memory). This flag avoids crashes at the cost of minor performance degradation — acceptable for agent use.
- Add `--disable-gpu` and `--disable-software-rasterizer` flags to eliminate GPU-related crash paths entirely (agents have no display anyway).

**Warning signs:**
- Chrome starts (npx process runs) but MCP tool calls time out or return empty results.
- Chrome process exits with signal (segfault or bus error) a few seconds after first CDP command.
- `--disable-dev-shm-usage` not present in Chrome args when running inside sandbox.

**Phase to address:** Implementation phase. Must be validated by actually running Chrome inside a real bubblewrap sandbox, not just testing in an uncontained environment.

---

### Pitfall 3: Chrome Path Saved at Init, Stale by Next Launch — Silent Failure

**What goes wrong:**
`rightclaw init` detects Chrome at a standard path (e.g., `/usr/bin/google-chrome`) and saves it to `~/.rightclaw/config.yaml`. Six months later: Chrome is updated and its binary moves (e.g., `/usr/bin/chromium-browser` on Ubuntu after a package rename), removed (user switched to Flatpak Chrome), or the saved path points to a symlink that was silently invalidated by a distro update.

On next `rightclaw up`, the path passes no validation at all (no check — rightclaw just writes it to `.mcp.json`), chrome-devtools-mcp silently fails to start, and agents have no browser tools. No error in logs — the MCP server just fails to initialize and CC treats it as an unavailable tool.

**Why it happens:**
Path validation happens once at init, then the saved value is trusted forever. Chrome paths are not stable across distro updates, major version bumps, or installation method changes (system package → Flatpak → snap → tarball).

**How to avoid:**
- Validate Chrome binary on every `rightclaw up`, not just at init. Use `which::which` as a fallback if the saved path is gone.
- Doctor check: verify Chrome at configured path exists AND is executable. Severity: Warn (not Error — agents still work without Chrome).
- On startup: if the saved path is absent, try a short list of standard fallback paths. If still missing, log a warning and skip chrome-devtools-mcp injection (do not fail the entire `rightclaw up`).
- Standard paths to probe (in order): `/usr/bin/google-chrome`, `/usr/bin/google-chrome-stable`, `/usr/bin/chromium-browser`, `/usr/bin/chromium`, `/snap/bin/chromium`, macOS: `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`, Homebrew: `$(brew --prefix)/bin/chromium`.

**Warning signs:**
- `~/.rightclaw/config.yaml` contains a `chrome_path` that points to a path that no longer exists.
- Agents report no browser tools available but no explicit error logged.
- Doctor shows "Chrome: not configured" after a system update even though Chrome is still installed (path changed).

**Phase to address:** Init detection phase AND `rightclaw up` generation phase. Both must validate path, not just init.

---

### Pitfall 4: npx Startup Adds 3–8 Seconds Per Agent Launch — Blocks process-compose Health Checks

**What goes wrong:**
`.mcp.json` entries using `npx chrome-devtools-mcp@latest` invoke npx on each agent startup. npx must:
1. Check the npm registry for the latest version (network call, 200-500ms).
2. Download the package if not cached (first run: 2-5 seconds; cached: 300-800ms).
3. Resolve and start the Node.js process.

When multiple agents start simultaneously in process-compose, each npx call makes a registry network call. In aggregate this delays agent initialization by 5-10 seconds. Worse: process-compose has health check timeouts. If MCP server startup exceeds the health check window, process-compose marks the process as unhealthy and restarts it — causing a restart loop.

Separately, `npx ... @latest` version anchoring means each launch resolves the "latest" tag, which can pull a different version than what was running before (version drift). A breaking change in chrome-devtools-mcp could silently break all agents simultaneously on next launch.

**Why it happens:**
`npx` is designed for convenience in interactive use, not for per-session daemon startup. It is routinely recommended in MCP documentation for simplicity, but this convenience hides significant startup cost and version instability in production runtimes.

**How to avoid:**
- Do NOT use `npx` in the generated `.mcp.json` entry. Use a globally installed binary path instead.
- Install chrome-devtools-mcp globally as part of `rightclaw init`: `npm install -g chrome-devtools-mcp@<pinned-version>`.
- Write the resolved binary path (from `which::which("chrome-devtools-mcp")` or `npm prefix -g`) into the `.mcp.json` `command` field — same pattern as rightclaw uses `current_exe()` for the rightmemory entry.
- Pin the version in `config.yaml` and doctor-check that the installed version matches.
- If global install is undesirable: at minimum use `npx --no` (offline, fail if not cached) and pin `chrome-devtools-mcp@X.Y.Z` not `@latest`.

**Warning signs:**
- `.mcp.json` entry has `"command": "npx"` with `"args": ["chrome-devtools-mcp@latest", ...]`.
- Agent startup takes noticeably longer after adding Chrome MCP (5-10 seconds per agent).
- process-compose shows chrome-devtools-mcp process restarting repeatedly.
- Agents work on Monday, break on Wednesday — version drift via `@latest`.

**Phase to address:** MCP config generation phase. The `generate_mcp_config` pattern in `mcp_config.rs` should be followed: use absolute binary path, not npx.

---

### Pitfall 5: Chrome Version Incompatibility with chrome-devtools-mcp — Silent CDP Protocol Mismatch

**What goes wrong:**
chrome-devtools-mcp targets the latest Extended Stable Chrome release. It uses Chrome DevTools Protocol (CDP) commands that may not exist in older Chrome versions. If the system Chrome is significantly behind (e.g., Chrome 110 on an old Ubuntu LTS while chrome-devtools-mcp targets Chrome 135+), CDP commands fail silently or return unexpected results.

The inverse also happens: Chrome auto-updates on macOS/Windows, advancing beyond what chrome-devtools-mcp was tested against. The tool officially supports "Other Chromium-based browsers" only with caveats — Chromium from distro packages may lag months behind Google Chrome's release cadence.

**Why it happens:**
Chrome versions are not pinned by the system package manager on most distros. Google Chrome auto-updates silently on Linux (if installed from Google's repo). Chromium from distro packages (e.g., Ubuntu's `chromium-browser`) lags 3-6 months behind Chrome stable. The mismatch is not detected at init time.

**How to avoid:**
- During `rightclaw init` Chrome detection, record the detected Chrome version alongside the path.
- Doctor check: verify Chrome version is >= minimum required (document this constant; as of 2026-04, Chrome 144+ is needed for Remote Debugging connections via chrome-devtools-mcp).
- Prefer Google Chrome over Chromium in the path probe order — Chrome is more current.
- Add version to doctor output so the operator can see what Chrome version agents are using.
- If Chromium is detected and its version is too old, emit a Warn with a link to instructions for installing Google Chrome.

**Warning signs:**
- Chrome binary found and functional but `screenshot` or `navigate` MCP tools return errors.
- Chromium from distro packages detected (binary name is `chromium-browser` not `google-chrome`).
- Doctor reports Chrome found but does not report version.

**Phase to address:** Init detection phase (detect and record version) + Doctor phase (validate version).

---

### Pitfall 6: Seatbelt (macOS) Blocks Chrome Profile Directory Write — Different Failure Than bubblewrap

**What goes wrong:**
On macOS, CC's sandbox uses Seatbelt (`sandbox-exec`). Seatbelt's deny rules restrict which directories the sandboxed process can write to. chrome-devtools-mcp defaults the Chrome profile directory to `~/.cache/chrome-devtools-mcp/chrome-profile`.

If the agent's HOME is overridden to the agent dir (e.g., `~/.rightclaw/agents/right/`), then `~/.cache/` resolves to `~/.rightclaw/agents/right/.cache/` — a path that may or may not be in Seatbelt's allowed write set. Chrome fails to initialize its profile, exits with a non-zero code, and the MCP server dies silently.

**Why it happens:**
macOS Seatbelt rules are defined in `settings.json` `sandbox.allowWrite` (array of paths). The agent dir is permitted. But `~/.cache/chrome-devtools-mcp/` under a HOME-overridden agent dir is a non-obvious path that no one added to the allow list.

**How to avoid:**
- Pass `--userDataDir` to chrome-devtools-mcp pointing to a path inside the agent directory (e.g., `$AGENT_DIR/.chrome-profile`). This keeps Chrome's profile within the already-permitted agent dir.
- In the MCP config generation, inject `--userDataDir` as an arg pointing to a resolved absolute path inside the agent dir.
- On macOS: add the Chrome profile dir to `SandboxOverrides.allow_write` if using external Chrome approach.
- Use `--isolated` flag in chrome-devtools-mcp for agent use — creates a temporary profile that is cleaned up, avoiding profile directory drift.

**Warning signs:**
- Chrome starts on macOS but exits immediately after the first CDP command.
- Seatbelt deny log: `deny(1) file-write-create` on a path under `~/.cache/chrome-devtools-mcp/`.
- Agent HOME is overridden (HOME=$AGENT_DIR set in shell wrapper) but Chrome profile is not redirected.

**Phase to address:** MCP config generation phase. `--userDataDir` must be injected as part of the generated `.mcp.json` args.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| `npx chrome-devtools-mcp@latest` in .mcp.json | Zero setup, always current | 3-8s startup delay, version drift, registry dependency at launch | Never in production; only for manual one-off testing |
| Save Chrome path at init, never revalidate | Simple code | Silent failure after Chrome update/move | Never — always validate on `up` |
| Inject chrome-devtools-mcp even when Chrome absent | Uniform agent config | MCP server fails to start on every launch, pollutes logs | Never — make injection conditional |
| Use `--no-sandbox` without outer bubblewrap sandbox | Fixes nested sandbox issue | Renderer exploits escape to host filesystem | Only acceptable inside a bubblewrap or Seatbelt outer sandbox |
| Don't pin chrome-devtools-mcp version | Always latest features | Breaking change hits all agents simultaneously on next launch | Never for the global install; acceptable only with a lock file |
| Profile dir under ~/.cache (default) | No config needed | Seatbelt denies write on macOS with HOME override | Never when HOME is overridden to agent dir |

---

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| bubblewrap + Chrome | Launch Chrome inside bwrap, no extra flags | Run Chrome outside bwrap with `--browser-url`, or pass `--no-sandbox` to Chrome inside bwrap |
| bubblewrap + /dev/shm | Assume /dev/shm is available inside sandbox | Pass `--disable-dev-shm-usage` to Chrome; add /dev/shm bind-mount if needed |
| Seatbelt + Chrome profile | Default profile dir under ~/.cache | Use `--userDataDir` pointing inside the permitted agent dir |
| .mcp.json + npx | Use `npx chrome-devtools-mcp@latest` | Install globally, use absolute binary path (mirror rightmemory pattern) |
| Chrome path | Validate only at init | Validate on every `rightclaw up`, probe fallback paths if saved path gone |
| Chrome version | Detect binary, skip version check | Detect + record + validate version in doctor |
| External Chrome process | Add to process-compose as a separate process | Launch as a process-compose entry with `--remote-debugging-port=9222`; healthcheck via `curl http://127.0.0.1:9222/json/version` |
| Agents without Chrome configured | Inject chrome-devtools-mcp entry anyway | Skip MCP entry entirely when Chrome path absent; log info-level message |

---

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| `--no-sandbox` Chrome without outer sandbox | Renderer exploits escape to host | Only use `--no-sandbox` when bubblewrap/Seatbelt is the outer layer; doctor should verify sandbox is active when `--no-sandbox` is passed to Chrome |
| Exposing `--remote-debugging-port=9222` on 0.0.0.0 | Other processes on the machine can control the agent's Chrome session | Bind only to `127.0.0.1`; verify Chrome launch args include `--remote-debugging-address=127.0.0.1` |
| Chrome profile dir shared across agents | One agent can read another agent's cookies, credentials, browsing history via Chrome profile | Each agent must have its own `--userDataDir` path; never share a Chrome profile between agents |
| `chrome-devtools-mcp@latest` unverified at runtime | Malicious npm publish could inject code into agent sessions | Pin version; use `npm audit` in doctor check; prefer signed releases |
| Allowing Chrome access to agent dir via `--userDataDir` | Chrome writes profiling, crash dumps, cache inside agent dir — potentially sensitive data | Use `--isolated` flag for temporary profiles, or scope `--userDataDir` to a subdirectory outside the agent's sensitive directories |

---

## "Looks Done But Isn't" Checklist

- [ ] **Nested sandbox tested**: Chrome integration tested inside a real bubblewrap sandbox, not just in an uncontained shell — verify Chrome actually starts without "No usable sandbox" crash.
- [ ] **Path validation on `up`**: Chrome path checked on every `rightclaw up`, not just at init — test with a path that was valid at init but deleted before `up`.
- [ ] **MCP entry skipped when Chrome absent**: `rightclaw up` with no Chrome configured produces `.mcp.json` without a chrome-devtools-mcp entry — agents launch successfully.
- [ ] **No npx in generated .mcp.json**: Inspect the generated `.mcp.json` — `command` must be an absolute binary path, not `"npx"`.
- [ ] **userDataDir is per-agent**: Two agents do not share the same Chrome profile directory.
- [ ] **External Chrome process registered in process-compose**: If using `--browser-url` approach, Chrome has its own process-compose entry with a working healthcheck.
- [ ] **Doctor checks Chrome version**: `rightclaw doctor` output includes Chrome version string alongside the path.
- [ ] **macOS profile write confirmed**: On macOS, verify Chrome can write its profile dir by checking Seatbelt deny logs after a test agent launch.
- [ ] **`/dev/shm` or `--disable-dev-shm-usage` present**: If Chrome runs inside bubblewrap, verify one of these is configured.
- [ ] **Port 9222 not already in use**: Doctor or preflight check verifies port 9222 is available before launching Chrome process.

---

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Nested sandbox deadlock | LOW | Switch to `--browser-url` approach or add `--no-sandbox` to Chrome args; no code rewrite needed |
| /dev/shm crash | LOW | Add `--disable-dev-shm-usage` to Chrome launch args |
| Stale Chrome path | LOW | `rightclaw init --chrome-path <new-path>` to re-detect; or manually update `~/.rightclaw/config.yaml` |
| npx version drift breaking change | MEDIUM | Pin to last known good version globally; update config.yaml chrome-devtools-mcp version field |
| Chrome profile locked by Seatbelt | LOW | Add `--userDataDir` arg pointing inside agent dir to generated .mcp.json args |
| Chrome version too old | MEDIUM | Install Google Chrome (not Chromium) from official repo; update saved path |
| Port 9222 conflict | LOW | Allow `--remote-debugging-port` to be configurable in `config.yaml`; expose as a `rightclaw init` flag |
| Shared Chrome profile across agents | HIGH | Requires .mcp.json regeneration for all agents with per-agent `--userDataDir`; no data corruption but isolation was void |

---

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| Nested sandbox deadlock (bubblewrap) | Chrome config generation phase | Run `rightclaw up` and verify chrome-devtools-mcp MCP tool initializes inside a real bwrap session |
| /dev/shm missing inside bubblewrap | Chrome config generation phase | Chrome process survives first CDP navigation command inside sandbox |
| Stale Chrome path | Both `rightclaw up` AND doctor phases | Delete Chrome binary, run `rightclaw up` — must warn and skip, not crash |
| npx startup delay + version drift | MCP config generation phase | Inspect generated `.mcp.json` — no `"command": "npx"` present |
| Chrome version incompatibility | Init detection + doctor phase | `rightclaw doctor` outputs Chrome version; old Chromium produces Warn |
| Seatbelt profile write failure (macOS) | MCP config generation phase | macOS agent launch with HOME override — Chrome tool succeeds |
| Chrome MCP optional when absent | `rightclaw up` generation phase | `rightclaw up` with no Chrome configured — agents start, no chrome-devtools-mcp entry in .mcp.json |

---

## Optionality Decision

**Chrome MCP must be optional. Recommendation: Warn, never block.**

Rationale:
1. RightClaw's existing pattern for all optional dependencies (git, sqlite3, socat, rg) is Warn severity in doctor — agent runtime continues without them. Chrome is no different.
2. The majority of rightclaw use cases are text/code agents (memory, cron, Telegram). Requiring Chrome for all agents would break existing setups on Chrome-less servers.
3. Linux headless servers (common deployment target) often have no Chrome available and installing it has significant overhead (100MB+ package with system dependencies).
4. The `--browser-url` approach allows agents to use Chrome only when an operator has explicitly set it up, matching the self-service model.

**Implementation:** `chrome_path` is absent from `config.yaml` by default. `rightclaw init` probes for Chrome and offers to configure it if found, but skips silently if not. `rightclaw up` injects the chrome-devtools-mcp entry only when `chrome_path` is present and valid. `rightclaw doctor` emits a `Warn` (not `Fail`) when Chrome path is configured but the binary is gone.

---

## Sources

- [chrome-devtools-mcp troubleshooting.md — official sandbox guidance](https://github.com/ChromeDevTools/chrome-devtools-mcp/blob/main/docs/troubleshooting.md) — explicit statement that sandboxed MCP clients cannot start Chrome; `--browser-url` as workaround
- [chrome-devtools-mcp GitHub README](https://github.com/ChromeDevTools/chrome-devtools-mcp) — `--executablePath`, `--browser-url`, `--userDataDir`, `--isolated`, `--headless`, `--chromeArg` flag documentation
- [Chromium /dev/shm issue #736452](https://bugs.chromium.org/p/chromium/issues/detail?id=736452) — `--disable-dev-shm-usage` flag origin; confirmed fix for tmpfs-constrained environments
- [Puppeteer issue #10367: Headless fails without sandbox](https://github.com/puppeteer/puppeteer/issues/10367) — nested sandbox + `--no-sandbox` behavior documented
- [Matt Ferrante on npx MCP server latency](https://x.com/ferrants/status/1920703234249032039) — real-world warning: npx is unacceptable for production MCP servers
- [MCP server MySQL issue #108: npx timeout with health check](https://github.com/benborla/mcp-server-mysql/issues/108) — process-compose health check timeout caused by npx startup delay
- [opencode issue #820: slow npx MCP startup blocks chat](https://github.com/sst/opencode/issues/820) — npx holds main thread during agent startup
- [chrome-devtools-mcp issue #140: automatic connection to existing Chrome session](https://github.com/ChromeDevTools/chrome-devtools-mcp/issues/140) — `--browser-url` usage confirmed for connecting to externally-managed Chrome
- [bubblewrap containers/bubblewrap](https://github.com/containers/bubblewrap) — user namespace inheritance; no automatic /dev/shm bind-mount
- [Simpleit.rocks: Chrome GPU process error on Ubuntu](https://simpleit.rocks/linux/ubuntu/fixing-common-google-chrome-gpu-process-error-message-in-linux/) — `--disable-gpu` + `--disable-software-rasterizer` flags for headless servers
- [zenika/alpine-chrome Docker image](https://hub.docker.com/r/zenika/alpine-chrome) — `--disable-dev-shm-usage` + `--no-sandbox` pattern for containerized Chrome

---
*Pitfalls research for: v3.4 Chrome Browser MCP Integration — chrome-devtools-mcp in bubblewrap/Seatbelt sandboxed headless agent runtime*
*Researched: 2026-04-05*
