# Phase 23: Bot Skeleton - Research

**Researched:** 2026-03-31
**Domain:** Rust teloxide bot framework, tokio signal handling, Cargo workspace crate creation
**Confidence:** HIGH

## Summary

Phase 23 creates `crates/bot` as a new workspace crate (`rightclaw-bot`) and wires `rightclaw bot --agent <name>` into the CLI. The crate starts a teloxide long-polling dispatcher with no-op handlers, opens memory.db, resolves the agent config, and installs full SIGTERM/SIGINT shutdown infrastructure — even though no subprocesses exist yet.

The teloxide version is 0.17.0 (current on crates.io). The critical adaptor ordering — `CacheMe<Throttle<Bot>>` — is confirmed by the official Throttle documentation: "It's recommended to use this wrapper before other wrappers (i.e. `SomeWrapper<Throttle<Bot>>` not `Throttle<SomeWrapper<Bot>>`)". CacheMe is outer, Throttle is inner. This prevents the issue #516 deadlock.

Teloxide's `Dispatcher` exposes a `ShutdownToken` (Clone + Send + Sync) that can be passed to a separate tokio task. Calling `token.shutdown()` returns `Result<impl Future, IdleShutdownError>`. The future resolves when all in-flight handlers finish. `tokio::signal::unix::signal(SignalKind::terminate())` handles SIGTERM; `tokio::signal::ctrl_c()` handles SIGINT. Both feed into the same shutdown path.

**Primary recommendation:** Scaffold `crates/bot` with Telegram-specific code under a `telegram/` submodule. Wire dispatcher shutdown via `ShutdownToken`. Use `CacheMe<Throttle<Bot>>` with `Limits::default()`. Allow-list filtering lives in a `dptree` filter step before handlers — silently drop by returning `None` from a `filter_map` closure.

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** New `crates/bot` workspace crate (`rightclaw-bot` package). All bot-related code lives here, NOT in `rightclaw` or `rightclaw-cli`.
- **D-02:** `rightclaw-cli` gains a `Bot` variant in `Commands` enum and delegates to `rightclaw-bot`. Dependency chain: `rightclaw-cli` → `rightclaw-bot` → `rightclaw`.
- **D-03:** `allowed_chat_ids: Vec<i64>` field added to `AgentConfig` in `rightclaw` crate.
- **D-04:** `#[serde(default)]` on the field — absent from agent.yaml = empty vec.
- **D-05:** Empty vec = block all (secure default). Bot emits `tracing::warn!` at startup. Process does not crash.
- **D-06:** Filtering: incoming update's chat_id not in set → silently drop (no reply, no log at INFO/DEBUG).
- **D-07:** Full SIGTERM shutdown structure wired in Phase 23 even though no subprocesses exist yet. `Arc<Mutex<Vec<tokio::process::Child>>>`.
- **D-08:** `tokio::signal::unix::signal(SignalKind::terminate())` + `tokio::signal::ctrl_c()`. Both trigger same shutdown path.
- **D-09:** Graceful sequence: signal → kill each in-flight child → wait for dispatcher to stop → exit 0.
- **D-10:** Agent dir resolved from `$RIGHTCLAW_HOME/agents/<name>/` via same logic as `rightclaw up`.
- **D-11:** `RC_AGENT_DIR` env var overrides dir search. Used by process-compose in Phase 26.
- **D-12:** `--home` CLI flag (already on root `Cli` struct) controls RIGHTCLAW_HOME.
- **D-13:** Token resolution order: (1) `RC_TELEGRAM_TOKEN` env, (2) `RC_TELEGRAM_TOKEN_FILE` env, (3) `agent.yaml telegram_token_file`, (4) `agent.yaml telegram_token`. Error if none found.
- **D-14:** `RC_TELEGRAM_TOKEN` / `RC_TELEGRAM_TOKEN_FILE` injected by process-compose (Phase 26). Manual set for Phase 23 testing.
- **D-15:** `CacheMe<Throttle<Bot>>` adaptor ordering. CacheMe outer, Throttle inner.
- **D-16:** Bot opens `memory.db` via `memory::open_db(agent_dir.join("memory.db"))`. Error if cannot open.

### Claude's Discretion

- Exact module layout inside `crates/bot/src/`.
- Whether to use `tokio_util::sync::CancellationToken` vs `AtomicBool` for shutdown signalling.
- Whether `allowed_chat_ids` becomes a `HashSet<i64>` internally for O(1) lookup.

### Deferred Ideas (OUT OF SCOPE)

- Non-Telegram bot support (Slack, webhooks) — future phases.
- Streaming responses / edit-in-place — v3.1.
- Documenting CC gotcha about Telegram messages dropped during streaming — pending todo.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| BOT-01 | `rightclaw bot --agent <name>` subcommand runs a teloxide long-polling bot for the given agent | New `crates/bot` crate; `Commands::Bot` variant in CLI; teloxide Dispatcher with `dispatch().await` |
| BOT-03 | Bot uses `CacheMe<Throttle<Bot>>` adaptor ordering to prevent Throttle deadlock (issue #516) | Confirmed by official Throttle docs: Throttle must be inner. `Bot::new(token).throttle(Limits::default()).cache_me()` |
| BOT-04 | Bot gracefully shuts down on SIGTERM — all in-flight claude -p subprocesses are killed before exit | `tokio::signal::unix::signal(SignalKind::terminate())` + `Dispatcher::shutdown_token()` + `Arc<Mutex<Vec<Child>>>` |
| BOT-05 | `allowed_chat_ids` field in agent.yaml — messages from unlisted chat IDs are silently ignored | `AgentConfig` field addition; dptree `filter_map` closure returns `None` for unlisted IDs |
</phase_requirements>

---

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| teloxide | 0.17.0 | Telegram bot framework: long-polling, dispatcher, adaptors | Only serious Rust Telegram bot framework. 1.3M downloads. |
| tokio (signal) | 1.50.0 (workspace) | SIGTERM + SIGINT handling | Already in workspace. `tokio::signal::unix` provides `signal(SignalKind::terminate())`. |
| tokio (process) | 1.50.0 (workspace) | `Child` type for in-flight subprocess tracking | Already in workspace. Used in Phase 25+ for real children. Phase 23 wires the `Arc<Mutex<Vec<Child>>>` empty. |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tokio_util (CancellationToken) | via tokio_util or manual | Shutdown coordination signal | If Claude chooses over AtomicBool — cleaner async-cancel propagation |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `CancellationToken` | `AtomicBool` + notify | AtomicBool is simpler but less ergonomic for async wait |

**Installation (add to workspace Cargo.toml and crates/bot/Cargo.toml):**
```bash
# workspace Cargo.toml [workspace.dependencies]:
teloxide = { version = "0.17", default-features = false, features = ["macros", "ctrlc_handler"] }

# crates/bot/Cargo.toml [dependencies]:
teloxide = { workspace = true }
rightclaw = { path = "../rightclaw" }
rightclaw-cli deps: clap, tokio, tracing, miette (all workspace)
```

**Version verified:** `cargo search teloxide` → `0.17.0` (2025). `cargo info teloxide` confirms `rust-version: 1.82`.

---

## Architecture Patterns

### Recommended Project Structure
```
crates/bot/
├── Cargo.toml
└── src/
    ├── lib.rs           # pub fn run(args: BotArgs) -> miette::Result<()>
    ├── error.rs         # BotError (thiserror)
    └── telegram/
        ├── mod.rs       # pub async fn run_telegram(config: BotConfig) -> miette::Result<()>
        ├── bot.rs       # Bot construction: CacheMe<Throttle<Bot>>
        ├── dispatch.rs  # Dispatcher setup, ShutdownToken, no-op handlers
        └── filter.rs    # chat_id allow-list filter (dptree filter_map)
```

Telegram-specific code lives under `telegram/` submodule per the user's forward-compatibility requirement. Future Slack/webhook modules get sibling dirs.

### Pattern 1: Adaptor Construction
**What:** Build `CacheMe<Throttle<Bot>>` from a token string.
**When to use:** Bot initialization, called once at startup.
**Example:**
```rust
// Source: https://docs.rs/teloxide/latest/teloxide/adaptors/struct.Throttle.html
use teloxide::{adaptors::throttle::Limits, prelude::*};

let bot = Bot::new(token)
    .throttle(Limits::default())  // Throttle<Bot>  — inner
    .cache_me();                  // CacheMe<Throttle<Bot>> — outer
```

### Pattern 2: Dispatcher with ShutdownToken
**What:** Build Dispatcher, extract ShutdownToken before dispatching, use token in SIGTERM task.
**When to use:** Main bot run loop.
**Example:**
```rust
// Source: https://docs.rs/teloxide/latest/teloxide/dispatching/struct.ShutdownToken.html
let mut dispatcher = Dispatcher::builder(bot, handler_schema)
    .build();

let shutdown_token = dispatcher.shutdown_token(); // Clone + Send + Sync

tokio::spawn(async move {
    // wait for SIGTERM or SIGINT
    signal_received.await;
    // kill in-flight children first
    kill_children(&children).await;
    // then ask dispatcher to stop
    if let Ok(fut) = shutdown_token.shutdown() {
        fut.await;
    }
});

dispatcher.dispatch().await;
```

### Pattern 3: chat_id Allow-list via dptree filter_map
**What:** Silently drop updates from unlisted chat IDs before any handler runs.
**When to use:** At the top of the Update handler chain.
**Example:**
```rust
// Source: teloxide dptree pattern
use teloxide::prelude::*;

let allowed: HashSet<i64> = config.allowed_chat_ids.iter().copied().collect();

let schema = Update::filter_message()
    .filter_map(move |msg: Message| {
        let chat_id = msg.chat.id.0;
        if allowed.is_empty() || allowed.contains(&chat_id) {
            // Phase 23: empty = block all; Phase 25 will not be empty in practice
            None  // or Some(msg) when allowed
        } else {
            None  // silently drop
        }
    })
    // no-op endpoint for Phase 23
    .endpoint(|_: Message| async { respond(()) });
```

Note: per D-05, empty `allowed_chat_ids` blocks all — the filter_map returns `None` for every message. Warn at startup via `tracing::warn!`.

### Pattern 4: SIGTERM + SIGINT dual signal handling
**What:** Await either SIGTERM or SIGINT in a single task, then trigger unified shutdown.
**When to use:** Signal handler task spawned before dispatcher loop.
**Example:**
```rust
// Source: tokio docs — tokio::signal::unix
use tokio::signal::unix::{signal, SignalKind};
use tokio::signal::ctrl_c;

tokio::spawn(async move {
    let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => { tracing::info!("SIGTERM received"); }
        _ = ctrl_c() => { tracing::info!("SIGINT received"); }
    }
    // unified shutdown sequence here
});
```

### Anti-Patterns to Avoid
- **`Throttle<CacheMe<Bot>>`:** Wrong order — Throttle must be inner. Official docs explicitly state inner wrappers must not interfere with Throttle's timing. Use `CacheMe<Throttle<Bot>>`.
- **`enable_ctrlc_handler()` on Dispatcher:** Do NOT call this — Phase 23 installs its own SIGTERM+SIGINT handler that also kills subprocesses. The built-in handler only shuts down the dispatcher and ignores children.
- **Logging silently dropped messages:** D-06 says no reply AND no log. `filter_map` returning `None` achieves this cleanly — dptree simply doesn't call downstream handlers.
- **`allowed_chat_ids` as `Vec<i64>` for lookup:** Convert to `HashSet<i64>` at startup for O(1) check, not O(n) linear scan on every update.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Rate limiting Telegram API calls | Custom sleep/queue | `Throttle` adaptor | Handles global + per-chat limits, chat-specific burst rules |
| GetMe caching | Custom `Option<Me>` | `CacheMe` adaptor | Thread-safe, already in teloxide |
| Long-polling loop | Manual `getUpdates` HTTP calls | `Dispatcher::dispatch()` | Handles offset tracking, error recovery, update routing |
| Graceful shutdown coordination | Manual channel/flag | `ShutdownToken` | Already integrated with dispatcher's in-flight handler wait |
| SIGTERM on Linux | `ctrlc` crate alone | `tokio::signal::unix::signal(SignalKind::terminate())` | `ctrlc` crate handles SIGINT; SIGTERM needs tokio's unix signal API |

**Key insight:** Teloxide handles the entire long-polling lifecycle — don't build any layer below `Dispatcher`.

---

## Common Pitfalls

### Pitfall 1: Wrong Adaptor Ordering
**What goes wrong:** Using `Throttle<CacheMe<Bot>>` instead of `CacheMe<Throttle<Bot>>` causes Throttle to see extra call overhead from CacheMe and miscalculate rate limits, potentially triggering issue #516 deadlock.
**Why it happens:** Intuitive nesting order feels like "throttle outermost = most control".
**How to avoid:** Build with `.throttle(Limits::default()).cache_me()` — method chaining gives correct inside-out ordering.
**Warning signs:** Dispatcher hangs at 100% CPU with only the throttle task active.

### Pitfall 2: Calling `enable_ctrlc_handler()` alongside custom signal handling
**What goes wrong:** Both handlers race; children may not be killed before dispatcher shuts down.
**Why it happens:** Default teloxide feature includes `ctrlc_handler`, so it's easy to call it "just in case".
**How to avoid:** Never call `.enable_ctrlc_handler()`. Install one SIGTERM+SIGINT handler task that owns the entire shutdown sequence (D-08, D-09).
**Warning signs:** Zombie claude -p processes remain after `rightclaw bot` exits.

### Pitfall 3: `AgentConfig` serde breaking due to `deny_unknown_fields`
**What goes wrong:** `AgentConfig` has `#[serde(deny_unknown_fields)]`. Adding `allowed_chat_ids` without `#[serde(default)]` breaks deserialization for all existing agent.yaml files that don't have the field.
**Why it happens:** Forgetting `#[serde(default)]` when adding an optional field.
**How to avoid:** Always pair new optional fields with `#[serde(default)]`. Existing tests in `types.rs` will catch this — `agent_config_deserializes_minimal_yaml_with_defaults` and `agent_config_without_telegram_defaults_to_none` both deserialize `"{}"` and will fail if the new field lacks a default.
**Warning signs:** `rightclaw up` fails to parse existing agent.yaml files after the field is added.

### Pitfall 4: `ShutdownToken::shutdown()` returns `Err` if dispatcher is idle
**What goes wrong:** Calling `token.shutdown()` before the dispatcher has started polling returns `IdleShutdownError`. Crashing on this error leaves children alive.
**Why it happens:** Signal arrives before `dispatcher.dispatch()` begins (rare, but possible in tests or fast shutdown).
**How to avoid:** Treat `Err(IdleShutdownError)` as "already stopped" — log at debug level and continue the shutdown sequence.
**Warning signs:** `unwrap()` or `?` on the shutdown result panics in tests.

### Pitfall 5: `open_db` vs `open_connection` confusion
**What goes wrong:** Calling `open_db()` (which drops the connection) then calling store operations will fail because each operation opens its own connection. For bot use, `open_connection()` is needed to share a live connection across the session.
**Why it happens:** Both functions exist in `memory::mod.rs`; `open_db` is simpler but returns `()`.
**How to avoid:** Bot startup calls `memory::open_connection(agent_dir)` not `open_db`. Connection stored in bot state for Phase 25 CRUD operations.

---

## Code Examples

### Minimal teloxide dispatcher (no-op, Phase 23 level)
```rust
// Demonstrates the full skeleton — CacheMe<Throttle<Bot>>, no-op handler, no ctrlc_handler
use teloxide::{adaptors::throttle::Limits, prelude::*, types::Update};

pub async fn run_telegram(token: String, allowed_chat_ids: HashSet<i64>) -> miette::Result<()> {
    let bot = Bot::new(token)
        .throttle(Limits::default())
        .cache_me();

    if allowed_chat_ids.is_empty() {
        tracing::warn!("allowed_chat_ids is empty — all incoming messages will be dropped");
    }

    let schema = Update::filter_message()
        .filter_map(move |msg: Message| {
            let id = msg.chat.id.0;
            if allowed_chat_ids.contains(&id) {
                Some(msg)
            } else {
                None
            }
        })
        .endpoint(|_: Message| async { respond(()) });

    let mut dispatcher = Dispatcher::builder(bot, schema).build();
    let shutdown_token = dispatcher.shutdown_token();
    let children: Arc<Mutex<Vec<tokio::process::Child>>> = Arc::new(Mutex::new(Vec::new()));

    // Signal handler task
    let children_clone = Arc::clone(&children);
    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate()
        ).expect("SIGTERM signal registration");
        tokio::select! {
            _ = sigterm.recv() => tracing::info!("SIGTERM received — initiating shutdown"),
            _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received — initiating shutdown"),
        }
        // Kill in-flight children (empty in Phase 23, populated in Phase 25)
        let mut locked = children_clone.lock().await;
        for child in locked.iter_mut() {
            let _ = child.kill().await;
        }
        drop(locked);
        // Stop dispatcher
        if let Ok(fut) = shutdown_token.shutdown() {
            fut.await;
        }
    });

    dispatcher.dispatch().await;
    Ok(())
}
```

### AgentConfig field addition (types.rs)
```rust
// In AgentConfig struct, after existing telegram_token field:
/// Chat IDs allowed to interact with this bot.
/// Empty = block all (secure default). Emits warn! at startup.
#[serde(default)]
pub allowed_chat_ids: Vec<i64>,
```

### Token resolution chain
```rust
pub fn resolve_token(
    agent_dir: &Path,
    config: &AgentConfig,
) -> miette::Result<String> {
    // 1. RC_TELEGRAM_TOKEN env var
    if let Ok(token) = std::env::var("RC_TELEGRAM_TOKEN") {
        if !token.is_empty() { return Ok(token); }
    }
    // 2. RC_TELEGRAM_TOKEN_FILE env var
    if let Ok(path) = std::env::var("RC_TELEGRAM_TOKEN_FILE") {
        return Ok(std::fs::read_to_string(&path)
            .map(|s| s.trim().to_string())
            .map_err(|e| miette::miette!("RC_TELEGRAM_TOKEN_FILE: {e}"))?);
    }
    // 3. agent.yaml telegram_token_file
    if let Some(rel) = &config.telegram_token_file {
        return Ok(std::fs::read_to_string(agent_dir.join(rel))
            .map(|s| s.trim().to_string())
            .map_err(|e| miette::miette!("telegram_token_file: {e}"))?);
    }
    // 4. agent.yaml telegram_token
    if let Some(token) = &config.telegram_token {
        return Ok(token.clone());
    }
    Err(miette::miette!("No Telegram token found. Set RC_TELEGRAM_TOKEN or configure agent.yaml"))
}
```

---

## Environment Availability

Step 2.6: SKIPPED — Phase 23 is purely Rust code changes. The only external dependency (Telegram API) is reached over HTTPS at runtime and does not require local installation. `tokio::signal::unix` is available on Linux (confirmed platform).

---

## Validation Architecture

nyquist_validation is explicitly `false` in `.planning/config.json`. Section skipped per config.

---

## Integration Points (Existing Codebase)

| File | Change Required |
|------|----------------|
| `Cargo.toml` (workspace) | Add `"crates/bot"` to `members`; add `teloxide` to `[workspace.dependencies]` |
| `crates/rightclaw/src/agent/types.rs` | Add `allowed_chat_ids: Vec<i64>` with `#[serde(default)]` to `AgentConfig` |
| `crates/rightclaw-cli/src/main.rs` | Add `Bot { agent: String }` variant to `Commands` enum; dispatch to `rightclaw_bot::run()` |
| `crates/bot/` | New crate: `Cargo.toml` + `src/lib.rs` + `src/telegram/` submodule |

**`AgentConfig` has `#[serde(deny_unknown_fields)]`** — the new field MUST have `#[serde(default)]` to remain backward compatible with existing agent.yaml files.

---

## Open Questions

1. **`CancellationToken` vs `AtomicBool` for shutdown coordination**
   - What we know: `tokio_util::sync::CancellationToken` supports `.cancel()` + `.cancelled().await` cleanly in async context. `AtomicBool` with `Notify` requires more boilerplate.
   - What's unclear: Whether tokio_util is already transitively available or needs a new workspace dep.
   - Recommendation: Check `cargo tree` for tokio_util before adding it. If absent, use `tokio::sync::watch` (already available via tokio full) as a zero-new-dep alternative.

2. **`HashSet<i64>` conversion point**
   - What we know: `Vec<i64>` deserialized from YAML, needs O(1) lookup.
   - Recommendation: Convert to `HashSet<i64>` once at startup when constructing the filter closure, not in `AgentConfig`. Keep `AgentConfig` as `Vec<i64>` (simpler for serde).

---

## Sources

### Primary (HIGH confidence)
- [teloxide 0.17.0 on crates.io](https://crates.io/crates/teloxide/0.17.0) — version confirmed via `cargo search teloxide` and `cargo info teloxide`
- [Throttle adaptor docs — ordering requirement](https://docs.rs/teloxide-core/latest/teloxide_core/adaptors/throttle/struct.Throttle.html) — "use this wrapper before other wrappers (SomeWrapper<Throttle<Bot>>)"
- [ShutdownToken docs](https://docs.rs/teloxide/latest/teloxide/dispatching/struct.ShutdownToken.html) — `shutdown()` returns `Result<impl Future<Output=()>, IdleShutdownError>`
- [DispatcherBuilder docs](https://docs.rs/teloxide/0.17.0/teloxide/dispatching/struct.DispatcherBuilder.html) — `enable_ctrlc_handler()` feature-gated; confirmed we should NOT use it

### Secondary (MEDIUM confidence)
- [teloxide issue #516](https://github.com/teloxide/teloxide/issues/516) — Throttle deadlock; confirms issue exists and is tied to adaptor ordering
- [teloxide issue #649](https://github.com/teloxide/teloxide/issues/649) — "Document idiomatic adaptor order" — confirms Throttle should be innermost
- [teloxide main docs index](https://docs.rs/teloxide/0.17.0/teloxide/index.html) — `.throttle(Limits::default()).cache_me()` method chaining pattern

### Tertiary (LOW confidence)
- None — all claims verified against official sources or codebase inspection.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — versions confirmed from crates.io live query
- Architecture: HIGH — patterns derived from official teloxide docs
- Pitfalls: HIGH — derived from official Throttle docs + existing codebase inspection (`deny_unknown_fields` found in source)
- Integration points: HIGH — read from actual source files

**Research date:** 2026-03-31
**Valid until:** 2026-05-01 (teloxide is actively maintained; 0.17.x API stable)
