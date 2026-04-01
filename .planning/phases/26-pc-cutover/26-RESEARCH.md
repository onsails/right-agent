# Phase 26: PC Cutover - Research

**Researched:** 2026-04-01
**Domain:** process-compose codegen, teloxide webhook management, doctor check extension
**Confidence:** HIGH

## Summary

Phase 26 is a focused refactor of the process-compose code generation layer. The current
`generate_process_compose` function produces CC interactive session entries using shell wrappers.
After this phase it produces bot entries only — one `<agent>-bot` process per agent with a
Telegram token. All CC channels infrastructure (`ensure_bun_installed`, `ensure_telegram_plugin_installed`,
`generate_telegram_channel_config`, the `any_telegram` guard block) is removed from `cmd_up`.

The scope is narrow and all the relevant code is fully mapped. There are no new crate
dependencies — `reqwest` (already in `rightclaw`) handles the `getWebhookInfo` doctor check;
`teloxide`'s built-in `bot.delete_webhook()` handles PC-04; `minijinja` handles the refactored
template; `current_exe()` produces the binary path for the bot command.

**Primary recommendation:** Treat this as three independent sub-tasks: (1) refactor
`process_compose.rs` + template, (2) remove the channels block from `cmd_up` and delete or
de-export `codegen/telegram.rs`, (3) add `deleteWebhook` in `bot/src/lib.rs` and the doctor
webhook check in `doctor.rs`.

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**D-01:** After Phase 26, `process-compose.yaml` contains only bot entries — one `<agent>-bot`
process per agent with `telegram_token` / `telegram_token_file` set. Agents without a Telegram
token get no process-compose entry at all.

**D-02:** CC interactive session entries are removed entirely. The old CC persistent session is
not replaced — `claude -p` is invoked per-message by the bot process. `ProcessAgent` struct,
`process_compose.rs`, and the Jinja2 template are refactored to generate bot entries only.

**D-03:** Bot process entry shape (PC-01):
```yaml
<agent>-bot:
  command: "rightclaw bot --agent <name>"
  working_dir: "<agent_path>"
  environment:
    - RC_AGENT_DIR=<agent_path>
    - RC_AGENT_NAME=<agent_name>
    - RC_TELEGRAM_TOKEN=<token>       # or RC_TELEGRAM_TOKEN_FILE=<path>
  availability:
    restart: "on_failure"
    backoff_seconds: 5
    max_restarts: 3
  shutdown:
    signal: 15
    timeout_seconds: 30
```
No `is_interactive` field (PC-02). `RC_TELEGRAM_TOKEN` for inline; `RC_TELEGRAM_TOKEN_FILE`
for file path (absolute, resolved relative to agent dir).

**D-04:** `is_interactive` removed from the Jinja2 template entirely (PC-02). No entries use TTY.

**D-05:** `generate_process_compose` signature change detail is Claude's Discretion — function
may resolve `current_exe()` internally or receive it as a parameter.

**D-06:** Remove from `cmd_up`: `ensure_bun_installed()`, `ensure_telegram_plugin_installed()`,
`generate_telegram_channel_config(agent)`, and the `any_telegram` detection block.
Old `.claude/channels/telegram/` dirs on disk are left untouched.

**D-07:** Bot calls `deleteWebhook` in `run_async()` (in `crates/bot/src/lib.rs`) before
starting the teloxide dispatcher. Uses teloxide's built-in `bot.delete_webhook()`.

**D-08:** `deleteWebhook` failure is a fatal error — propagate `Err`, let process-compose
restart the bot.

**D-09:** `rightclaw doctor` checks each agent's configured Telegram token via Telegram API's
`getWebhookInfo` endpoint. Implementation uses `reqwest` with a blocking call via
`tokio::runtime::Runtime::new()`.

**D-10:** Check is non-fatal / warn — HTTP failure skips the check with Pass or Warn
"webhook check skipped". Only warns (not fails) when webhook IS found with a non-empty URL.

**D-11:** Token resolution for doctor check: reads `telegram_token` / `telegram_token_file`
from `agent.yaml` directly (same logic as `codegen::telegram::resolve_telegram_token`).
If no token configured — skip check for that agent.

### Claude's Discretion

- Whether `generate_process_compose` resolves `current_exe()` internally or receives it as a parameter.
- Whether token is injected as `RC_TELEGRAM_TOKEN` (inline) or `RC_TELEGRAM_TOKEN_FILE` (file path)
  in the env block — follow the precedence already established in `telegram::resolve_token`.
- Test structure for the reworked `process_compose_tests.rs`.

### Deferred Ideas (OUT OF SCOPE)

- `telegram-core` crate refactor
- Cron runtime (Phase 27)
- Cronsync SKILL rewrite (Phase 28)
- Any new agent capabilities
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| PC-01 | PC config entry `<agent>-bot` generated for agents with `telegram_token` set; entry uses `rightclaw bot --agent <name>` with env block `RC_AGENT_DIR`, `RC_AGENT_NAME`, `RC_TELEGRAM_TOKEN` (or `RC_TELEGRAM_TOKEN_FILE`) | `ProcessAgent` struct replaced with `BotProcessAgent`; Jinja2 template rewritten; `generate_process_compose` filters to telegram-only agents |
| PC-02 | `is_interactive` removed from PC template (bots don't need TTY) | Template field deleted; `ProcessAgent` struct never had it in the bot path |
| PC-03 | CC channels flag (`--channels plugin:telegram@...`) removed from all agent launch code paths | Three calls + `any_telegram` guard in `cmd_up` removed atomically; `codegen/plugin.rs` exports de-published or left with dead-code warning addressed |
| PC-04 | Bot process calls `deleteWebhook` on startup to clear any prior Telegram webhook registration | `teloxide` Bot has `delete_webhook()` method; call before `telegram::run_telegram()` in `run_async()` |
| PC-05 | `rightclaw doctor` warns when a configured Telegram token has an active webhook | New `check_webhook_info` function in `doctor.rs`; uses `reqwest` blocking runtime; `getWebhookInfo` JSON parsed for non-empty `result.url` |
</phase_requirements>

---

## Standard Stack

### Core (no new dependencies)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| minijinja | 2.18 | Jinja2 template for bot-only PC YAML | Already in use; rewrite in-place |
| reqwest | 0.13 | HTTP call to Telegram `getWebhookInfo` | Already in `rightclaw` crate deps (workspace) |
| tokio | 1.50 | `Runtime::new().block_on(...)` for blocking doctor check | Already in workspace |
| teloxide | 0.17 | `bot.delete_webhook()` in PC-04 | Already in `bot` crate |

**No new Cargo.toml entries required.** All dependencies are already present in the workspace.

### reqwest Feature Check — IMPORTANT

The workspace `reqwest` is configured with `default-features = false, features = ["json", "rustls"]`.
The `blocking` feature is NOT enabled. Per D-09, the doctor check uses
`tokio::runtime::Runtime::new().block_on(...)` to drive the async `reqwest` client from the
sync `run_doctor()` context. This is correct — do not add the `blocking` feature.

```toml
# Workspace Cargo.toml — CURRENT (no change needed)
reqwest = { version = "0.13", default-features = false, features = ["json", "rustls"] }
```

---

## Architecture Patterns

### Recommended Project Structure (after Phase 26)

```
crates/
├── rightclaw/src/
│   ├── codegen/
│   │   ├── mod.rs              # remove telegram export; keep process_compose export
│   │   ├── process_compose.rs  # PRIMARY CHANGE: BotProcessAgent, bot-only template logic
│   │   ├── process_compose_tests.rs  # FULL REWRITE: all old tests invalid
│   │   ├── telegram.rs         # REVIEW: delete or keep (needed by resolve_telegram_token for doctor)
│   │   └── plugin.rs           # kept but exports removed from mod.rs
│   └── doctor.rs               # ADD: check_webhook_info per-agent
├── bot/src/
│   └── lib.rs                  # ADD: delete_webhook before run_telegram
templates/
└── process-compose.yaml.j2     # REWRITE: bot entry format, no is_interactive
```

### Pattern 1: BotProcessAgent struct replacement

The current `ProcessAgent` struct carries `wrapper_path` and `working_dir`. Replace with a
struct that carries bot-entry fields:

```rust
// crates/rightclaw/src/codegen/process_compose.rs
#[derive(Debug, Serialize)]
struct BotProcessAgent {
    name: String,           // entry key: "<agent-name>-bot"
    agent_name: String,     // "--agent <name>" arg
    exe_path: String,       // current_exe() resolved to absolute path string
    working_dir: String,    // agent.path
    token_env: TokenEnv,    // RC_TELEGRAM_TOKEN or RC_TELEGRAM_TOKEN_FILE
    restart_policy: String,
    backoff_seconds: u32,
    max_restarts: u32,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value")]
enum TokenEnv {
    Inline(String),   // -> RC_TELEGRAM_TOKEN=<value>
    File(String),     // -> RC_TELEGRAM_TOKEN_FILE=<abs-path>
}
```

The template uses `{% if agent.token_env.kind == "Inline" %}` or simpler: use two separate
optional string fields in the struct and let the template branch on `agent.token_inline` vs
`agent.token_file`. Simpler approach for minijinja: two optional string fields.

**Claude's Discretion:** Internal struct shape — pick whatever makes the Jinja template cleanest.

### Pattern 2: Jinja2 template rewrite

```yaml
# templates/process-compose.yaml.j2
# Generated by rightclaw -- do not edit
version: "0.5"
is_strict: true

processes:
{% for agent in agents %}
  {{ agent.name }}-bot:
    command: "{{ agent.exe_path }} bot --agent {{ agent.agent_name }}"
    working_dir: "{{ agent.working_dir }}"
    environment:
      - RC_AGENT_DIR={{ agent.working_dir }}
      - RC_AGENT_NAME={{ agent.agent_name }}
{% if agent.token_inline %}
      - RC_TELEGRAM_TOKEN={{ agent.token_inline }}
{% else %}
      - RC_TELEGRAM_TOKEN_FILE={{ agent.token_file }}
{% endif %}
    availability:
      restart: "{{ agent.restart_policy }}"
      backoff_seconds: {{ agent.backoff_seconds }}
      max_restarts: {{ agent.max_restarts }}
    shutdown:
      signal: 15
      timeout_seconds: 30
{% endfor %}
```

No `is_interactive` field anywhere (PC-02).

### Pattern 3: deleteWebhook in bot/src/lib.rs

Insert immediately before `telegram::run_telegram(...)` call in `run_async`:

```rust
// crates/bot/src/lib.rs — in run_async(), after token is resolved
use teloxide::Bot;

let bot_for_webhook = Bot::new(token.clone());
bot_for_webhook
    .delete_webhook()
    .await
    .map_err(|e| miette::miette!("deleteWebhook failed: {e:#}"))?;
tracing::info!(agent = %args.agent, "deleteWebhook succeeded");
```

Note: the bot adaptor is built inside `telegram::run_telegram` via `build_bot(token)`.
For `deleteWebhook` we can use a plain `Bot::new(token.clone())` — no adaptors needed for
a one-shot API call before the dispatcher starts.

### Pattern 4: Doctor webhook check (getWebhookInfo)

The Telegram `getWebhookInfo` endpoint: `GET https://api.telegram.org/bot<TOKEN>/getWebhookInfo`

Response shape:
```json
{"ok": true, "result": {"url": "", "has_custom_certificate": false, ...}}
```
When no webhook is set, `result.url` is `""`. When a webhook is active, `result.url` is
the HTTPS endpoint.

Doctor check integration in `doctor.rs`:

```rust
// Called from run_doctor() after agent structure checks
checks.extend(check_webhook_info_for_agents(home));
```

Implementation outline:

```rust
fn check_webhook_info_for_agents(home: &Path) -> Vec<DoctorCheck> {
    // discover_agents equivalent or direct read of agent dirs
    // For each agent with a telegram token:
    //   resolve token via resolve_telegram_token equivalent
    //   call getWebhookInfo via reqwest (blocking via Runtime::new)
    //   if HTTP fails: DoctorCheck { status: Warn, detail: "webhook check skipped: {e}" }
    //   if url non-empty: DoctorCheck { status: Warn, detail: "active webhook found: {url}" }
    //   if url empty: DoctorCheck { status: Pass, detail: "no active webhook" }
}
```

**Token resolution for doctor:** `codegen/telegram.rs::resolve_telegram_token` already exists
with the correct logic. Two options:
1. Keep `codegen/telegram.rs` (don't delete it) and call `resolve_telegram_token` from doctor.
2. Duplicate the tiny resolution logic inline in doctor.rs.

Option 1 is cleaner. The `generate_telegram_channel_config` export can be removed from
`codegen/mod.rs` without deleting the file if `resolve_telegram_token` is needed by doctor.
Or make `resolve_telegram_token` pub and re-export from `codegen/mod.rs`.

### Pattern 5: generate_process_compose signature change

Current signature: `generate_process_compose(agents: &[AgentDef], run_dir: &Path) -> miette::Result<String>`

After Phase 26: `run_dir` is no longer needed (no more shell wrapper paths). The function only
needs `agents: &[AgentDef]` plus a way to get `current_exe()`.

Options (Claude's Discretion):
- **Internal resolution:** `std::env::current_exe()` called inside the function. Simple, no
  signature change at callsite. Downside: harder to unit test without spawning a real exe.
- **Parameter:** `exe_path: &Path` passed in. Caller does `current_exe()`. Makes tests inject
  a known path. This is the same pattern used for the rightmemory MCP path in Phase 17.

**Recommendation:** Pass `exe_path: &Path` as a parameter. Tests can inject a stable path
(e.g. `/usr/bin/rightclaw`). Callsite in `cmd_up` already has `self_exe` resolved.

New signature:
```rust
pub fn generate_process_compose(agents: &[AgentDef], exe_path: &Path) -> miette::Result<String>
```

The `run_dir` parameter is dropped entirely.

### Pattern 6: Filtering agents in generate_process_compose

The function must only emit entries for agents that have a Telegram token. The filter:

```rust
let bot_agents: Vec<BotProcessAgent> = agents
    .iter()
    .filter(|a| {
        a.config.as_ref().map(|c|
            c.telegram_token.is_some() || c.telegram_token_file.is_some()
        ).unwrap_or(false)
    })
    .map(|agent| { /* build BotProcessAgent */ })
    .collect();
```

If `bot_agents` is empty, the template produces a valid but process-less YAML (processes block
with no entries). This is fine — process-compose handles empty process sets.

### Anti-Patterns to Avoid

- **Do not use `reqwest::blocking`** — it is not in the feature set and adding it would
  conflict with the tokio runtime in doctor tests. Use `Runtime::new().block_on(...)`.
- **Do not construct a nested tokio runtime** if the doctor check is ever called from an async
  context — `run_doctor` is sync (called from sync `cmd_doctor`), so `Runtime::new()` is safe.
- **Do not call `resolve_token` from `bot/src/telegram/mod.rs`** for the doctor check — that
  function has a different priority chain (env vars first). Doctor needs static config reading
  only (`resolve_telegram_token` from `codegen/telegram.rs`).
- **Do not leave dead `ensure_bun_installed` / `ensure_telegram_plugin_installed` exports** in
  `codegen/mod.rs` — remove the re-exports when the calls are removed from `cmd_up`, or cargo
  will emit dead_code warnings.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Telegram webhook deletion | Custom HTTP DELETE call | `teloxide bot.delete_webhook()` | teloxide wraps the API correctly including error types |
| Async from sync context | `std::thread::spawn` + channel | `tokio::runtime::Runtime::new().block_on()` | Clean pattern, no thread overhead for one-shot HTTP |
| Current exe path | String constant | `std::env::current_exe()` | Works even when binary is not on PATH (process-compose env) |

---

## Common Pitfalls

### Pitfall 1: `run_dir` still passed to generate_process_compose callsite

**What goes wrong:** `cmd_up` in `main.rs` currently calls
`generate_process_compose(&agents, &run_dir)`. After the signature change to `(agents, exe_path)`,
the callsite must be updated to pass `&self_exe` instead of `&run_dir`.
**Why it happens:** Two variables in scope, easy to pass the wrong one.
**How to avoid:** Update callsite atomically with the function signature change.
**Warning signs:** Compile error (type mismatch) — Path is Path so this would silently compile
correctly but produce wrong paths. Add a compile-time guard if possible, or name it `bot_exe`.

### Pitfall 2: `codegen/telegram.rs` delete removes `resolve_telegram_token` used by doctor

**What goes wrong:** Doctor's webhook check needs `resolve_telegram_token`. If `telegram.rs` is
deleted, doctor can't resolve tokens.
**Why it happens:** CONTEXT.md says "may be deleted or kept — review whether
`generate_telegram_channel_config` is called from anywhere else".
**How to avoid:** Either keep `telegram.rs` (just remove the pub re-export of
`generate_telegram_channel_config` from `mod.rs`) OR extract `resolve_telegram_token` to a
shared location before deleting the file. The keep-and-de-export approach is simpler.

### Pitfall 3: process_compose_tests.rs contains 8 tests that all become invalid

**What goes wrong:** All existing tests assert on `wrapper_path` (`.sh` suffix) and
`is_interactive` fields. After the refactor, none of these assertions are valid.
**Why it happens:** The test file documents the OLD PC template shape completely.
**How to avoid:** Treat the test file as a full rewrite. New tests should assert:
- `<agent>-bot:` process key
- `rightclaw bot --agent <name>` command
- `RC_AGENT_DIR` in environment block
- Agents without telegram token are absent from output
- `is_interactive` does NOT appear in output

### Pitfall 4: `reqwest` runtime collision in doctor tests

**What goes wrong:** Unit tests for `check_webhook_info` try to create a new tokio Runtime
from within an existing test runtime (if tests use `#[tokio::test]`).
**Why it happens:** `run_doctor` is sync; doctor.rs tests are sync; but `Runtime::new()` panics
if called from within an existing runtime.
**How to avoid:** Keep doctor.rs tests as plain sync `#[test]` (not `#[tokio::test]`).
For webhook check tests, mock the HTTP call or use a test helper that skips the actual
network call (pass a dummy token that resolves to None).

### Pitfall 5: Empty `processes:` block when no agents have telegram token

**What goes wrong:** `process-compose up` may behave unexpectedly with zero processes.
**Why it happens:** D-01 says agents without a Telegram token get no entry. If all agents
lack tokens, the config has `processes:` with nothing under it.
**How to avoid:** Add an early check in `cmd_up` — if `bot_agents` is empty after filtering,
warn the user ("no agents have Telegram tokens configured; nothing to start") and exit with
a useful error rather than passing an empty config to process-compose.

### Pitfall 6: `telegram_token_file` path in env block must be absolute

**What goes wrong:** D-03 says `RC_TELEGRAM_TOKEN_FILE` is a file path "resolved to absolute,
relative to agent dir". If the relative path from `agent.yaml` is emitted as-is, the bot
process won't find it (working_dir is the agent dir, but env vars are set at launch time).
**Why it happens:** `telegram_token_file` in `agent.yaml` is stored as a relative path.
**How to avoid:** In `generate_process_compose`, call `agent.path.join(rel_path).canonicalize()`
or `agent.path.join(rel_path).display().to_string()` to produce an absolute path before
writing it into the env block. Prefer `agent.path.join(rel)` (no `canonicalize`) to avoid
errors when the file doesn't exist yet at codegen time.

---

## Code Examples

### getWebhookInfo HTTP call

```rust
// Source: Telegram Bot API docs (https://core.telegram.org/bots/api#getwebhookinfo)
// Called from check_webhook_info() in doctor.rs

fn fetch_webhook_url(token: &str) -> Result<String, String> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to create runtime: {e}"))?;
    rt.block_on(async {
        let url = format!("https://api.telegram.org/bot{token}/getWebhookInfo");
        let resp = reqwest::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;
        let body: serde_json::Value = resp.json().await
            .map_err(|e| format!("JSON parse error: {e}"))?;
        Ok(body["result"]["url"]
            .as_str()
            .unwrap_or("")
            .to_string())
    })
}
```

### teloxide delete_webhook

```rust
// Source: teloxide docs — Bot::delete_webhook() returns ResponseResult<True>
// In bot/src/lib.rs run_async(), after token resolved, before run_telegram()

let webhook_bot = teloxide::Bot::new(token.clone());
webhook_bot
    .delete_webhook()
    .await
    .map_err(|e| miette::miette!("deleteWebhook failed — long polling would compete with active webhook: {e:#}"))?;
tracing::info!(agent = %args.agent, "deleteWebhook succeeded");
```

### Revised cmd_up channels block removal

```rust
// REMOVE from cmd_up (approximately lines 413-422 of main.rs):
//
//   let any_telegram = agents.iter().any(|a| { ... });
//   if any_telegram {
//       rightclaw::codegen::ensure_bun_installed()?;
//       rightclaw::codegen::ensure_telegram_plugin_installed()?;
//   }
//
// REMOVE from per-agent loop (approximately line 487):
//   rightclaw::codegen::generate_telegram_channel_config(agent)?;
//
// These are the three calls D-06 identifies for removal.
```

### Updated generate_process_compose callsite

```rust
// In cmd_up, BEFORE:
let pc_config = rightclaw::codegen::generate_process_compose(&agents, &run_dir)?;

// AFTER (self_exe already resolved earlier in cmd_up):
let pc_config = rightclaw::codegen::generate_process_compose(&agents, &self_exe)?;
```

---

## Runtime State Inventory

> Not applicable — this is not a rename/migration phase. No stored data, OS-registered state,
> or build artifacts contain identifiers being changed. The old `.claude/channels/telegram/`
> directories are intentionally left on disk per D-06.

---

## Environment Availability

> Skip — Phase 26 is purely code/config changes. No new external tool dependencies. All
> required tools (teloxide, reqwest, minijinja) are already in the workspace.

---

## Deferred Tracking

### PROMPT-03 Resolution (from CONTEXT.md deferred section)

CONTEXT.md notes: Phase 26 removes the last user of `wrapper_path` (the old PC template),
which effectively completes PROMPT-03 (`codegen/shell_wrapper.rs` removed).

Current state: `codegen/shell_wrapper.rs` does NOT exist in the codebase (verified by
inspecting `codegen/mod.rs` — no `shell_wrapper` module). PROMPT-03 is already complete in
code (Phase 24 cleaned it up). The planner should mark PROMPT-03 complete in REQUIREMENTS.md
as a bookkeeping task in Wave 0.

---

## Open Questions

1. **`codegen/telegram.rs` fate — delete or keep?**
   - What we know: `generate_telegram_channel_config` is called in one place (`cmd_up` per-agent
     loop, line 487). `resolve_telegram_token` is the internal function needed by the doctor check.
   - What's unclear: Whether to keep the file intact, or delete it and inline the token resolution
     in `doctor.rs`.
   - Recommendation: Keep `telegram.rs`, make `resolve_telegram_token` pub(crate), remove the
     `generate_telegram_channel_config` pub re-export from `mod.rs`. Deleting the file saves
     ~300 lines but requires duplicating logic elsewhere.

2. **`codegen/plugin.rs` fate — delete or keep?**
   - What we know: `ensure_bun_installed` and `ensure_telegram_plugin_installed` are only called
     from the `any_telegram` block being removed. No other callers.
   - Recommendation: Delete the re-exports from `mod.rs`; keep or delete the file based on
     whether there are dead_code warnings (it's an internal module, so deleting it and its
     `pub mod plugin` entry in `mod.rs` is clean).

3. **Doctor check placement in run_doctor() — before or after agent structure checks?**
   - What we know: `run_doctor()` collects checks sequentially; webhook check needs agent
     discovery to find tokens.
   - Recommendation: Add webhook checks after `check_agent_structure(home)` — structurally
     similar to how sqlite3 and managed-settings checks follow. Pass `home` to the new helper.

---

## Sources

### Primary (HIGH confidence)
- Direct code inspection: `crates/rightclaw/src/codegen/process_compose.rs` — current struct and template
- Direct code inspection: `crates/rightclaw/src/codegen/telegram.rs` — `resolve_telegram_token` logic
- Direct code inspection: `crates/rightclaw/src/doctor.rs` — `DoctorCheck` pattern, `run_doctor` structure
- Direct code inspection: `crates/bot/src/lib.rs` — `run_async` insertion point for deleteWebhook
- Direct code inspection: `crates/bot/src/telegram/mod.rs` — `resolve_token` priority chain
- Direct code inspection: `crates/rightclaw-cli/src/main.rs` — `cmd_up` channels block (lines 413-488)
- Direct code inspection: `Cargo.toml` — reqwest features (no `blocking`), teloxide version
- Telegram Bot API docs (canonical): `getWebhookInfo` returns `result.url` empty string when no webhook

### Secondary (MEDIUM confidence)
- teloxide 0.17 API: `Bot::delete_webhook()` — standard method in teloxide since early versions,
  returns `ResponseResult<True>`. Method name unchanged across versions.

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all dependencies already in workspace, no new additions
- Architecture: HIGH — all change targets identified from direct code inspection
- Pitfalls: HIGH — derived from reading the actual existing code, not speculation

**Research date:** 2026-04-01
**Valid until:** 2026-05-01 (teloxide API stable; minijinja stable)
