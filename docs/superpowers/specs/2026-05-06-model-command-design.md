# `/model` Telegram command — model picker for the agent

## Problem

The agent's Claude model is set once at bot startup via a CLI flag and
threaded through `AgentSettings.model: Option<String>` →
`WorkerContext.model` → `ClaudeInvocation::set_model`. Switching models
requires editing a CLI invocation (or process-compose config) and
restarting `right-bot` manually. There is no in-band UI for it.

`agent.yaml` is documented in `ARCHITECTURE.md` (Configuration Hierarchy,
MergedRMW row) as the home for `model`, but the field is not actually
implemented — the docs lead, the code lags.

Users want to pick between Claude variants the same way they do in the CC
CLI itself: a short menu of curated options (Default / Sonnet / Sonnet 1M
/ Haiku) with one-tap switching, working in DMs and groups.

## Goals

- New `/model` Telegram command that opens an inline keyboard with the
  same four options the CC `/model` slash command shows.
- Selection persists across bot restarts (written to `agent.yaml`).
- Selection takes effect on the **next** CC invocation in any chat —
  no bot restart, no killing of in-flight CC subprocesses.
- Works in private chats and group chats. In groups, restricted to
  allowlisted users via `crate::allowlist::is_allowed(user_id, &allowlist)`
  — the same trusted-user gate that protects every group-aware command.
- Support manually-edited custom model IDs in `agent.yaml` (do not
  silently overwrite them; show as "Custom" in the menu).

## Non-goals

- Fetching the model list from the Anthropic API. The four canonical
  options are hardcoded in a per-module array in `model_command.rs`.
  Model releases are infrequent enough that a `right-bot` release is
  the appropriate update path. (Consistent with the existing memory
  rule "avoid central enums/registries" — the array is local to the
  command module, not a project-wide registry.)
- Per-chat or per-thread model overrides. One agent → one model.
  Picking from any chat affects the whole agent.
- Effort-level toggle (the `xHigh effort` row in CC's menu). Out of
  scope; only the model dimension.
- Non-Claude models. `--model` accepts any string and forwards it to
  `claude -p` verbatim, but the curated menu only lists CC-supported
  variants.
- A `/model <model-id>` argument form. YAGNI for v1; if requested
  later, parse `BotCommand::Model(String)` arg and skip the menu.

## Decisions

| Question | Answer | Reasoning |
|---|---|---|
| Storage: agent.yaml, in-memory map, or SQLite? | `agent.yaml` (`MergedRMW`) | Single source of truth, survives restart, hand-editable, already documented as the home for this field. |
| Model list: hardcoded, dynamic from API, or hybrid? | Hardcoded 4 options | Match CC's curated menu. API call is overkill for 3-4 buttons; release cadence is fine. |
| "Default" semantics | Absent `model` field in yaml (`None`) | Matches CC behavior: don't pin a model, let CC pick its own default. Future-proof against CC changing its default. |
| Group permissions | Allowlist only | Same gate as `/allow`/`/deny`. Trusted-user pattern is established. |
| Apply behavior: restart vs hot-reload | Hot-reload | `/model` will be used often; killing in-flight CC subprocesses every switch is bad UX. Justifies a smart-diff watcher. |
| Callback data format | Short alias (`model:sonnet`) | 64-byte limit on Telegram callback_data; survives model-ID renames; stable across rebrand. |
| Custom model in yaml | Show "Current: \<id\> (custom)" without ✓ | Don't clobber user's manual choice; give explicit path back to canonical. |

## Architecture

### Components

1. **`AgentConfig.model: Option<String>`** — new field in
   `crates/right-agent/src/config/agent_config.rs`. `None` = field
   absent in yaml = CC default. Canonical values in v1: `claude-sonnet-4-6`,
   `claude-sonnet-4-6[1m]`, `claude-haiku-4-5`. (Default is represented
   by absence, not by a sentinel string.)

2. **`write_agent_yaml_model(path, Option<&str>) -> Result<()>`** —
   MergedRMW helper in the same module. Reads → mutates `model` key →
   writes via `tempfile` + `fs::rename` (atomic on the same filesystem).
   `None` removes the key entirely (does not write `model: null`).
   Preserves all unknown fields. Yaml comments are best-effort:
   `serde_yaml` round-trips lose trailing comments, which is acceptable
   for `agent.yaml` since the template's comments are leading-only.

3. **`BotCommand::Model`** — new variant (no arg) in
   `crates/bot/src/telegram/dispatch.rs`. Registered in all three
   `BotCommandScope`s (Default, AllPrivateChats, AllGroupChats) for
   autocomplete in DMs and groups.

4. **`crates/bot/src/telegram/model_command.rs`** — new module:
   - `MODEL_CHOICES: &[ModelChoice]` — local array of 4 entries
     (alias, label, model_id, description)
   - `handle_model` — opens the menu (with allowlist gate in groups)
   - `handle_model_callback` — handles button clicks
   - menu rendering helpers (label-with-checkmark, keyboard layout)

5. **`AgentSettings.model: Arc<ArcSwap<Option<String>>>`** — refactor
   in `crates/bot/src/telegram/handler.rs`. Lock-free read on every
   CC invocation, lock-free swap on `/model` callback or watcher
   hot-reload. Adds `arc-swap = "1.7"` to `crates/bot/Cargo.toml`.

6. **Smart-diff `config_watcher`** — `crates/bot/src/config_watcher.rs`
   gains a parameter `model_swap: Arc<ArcSwap<Option<String>>>` and a
   `diff_classify(old_yaml, new_yaml) -> ChangeKind` helper. If the
   only diff is the `model` field, parse new value and `model_swap.store`
   it; do not cancel the shutdown token. Otherwise, current behavior:
   set `config_changed = true` and cancel.

7. **`WorkerContext.model: Arc<ArcSwap<Option<String>>>`** —
   `crates/bot/src/telegram/worker.rs` holds the same `Arc` as
   `AgentSettings`. Both call sites that build `ClaudeInvocation`
   (`worker.rs` lines ~1154 and ~1652) call `.load()` immediately
   before `set_model(...)`. No snapshotting at worker spawn.

### Boundary of responsibility

- `right-agent` crate owns the `AgentConfig` schema and the yaml
  read/modify/write helper. Single source of truth for the file format.
- `right-bot` crate owns the UI (command, menu, callbacks), the
  in-memory `ArcSwap` cell, and the watcher's hot-reload classification.
- `ClaudeInvocation` API does not change. It still takes
  `set_model(Option<String>)`. Hot-reload is invisible to it.

## UI flow

User types `/model` (`/model@username` in groups) in any chat or thread.

**DM:** open menu unconditionally.

**Group:** check allowlist via `crate::allowlist::is_allowed(user_id,
&allowlist)`. Not allowed → reply `Not allowed` (existing pattern), exit.

**Menu message:**

```
🤖 Choose Claude model

✓ Default — Opus 4.7 (1M context) · Most capable
   Sonnet — Sonnet 4.6 · Best for everyday tasks
   Sonnet 1M — Sonnet 4.6 (1M context) · Extra usage billing
   Haiku — Haiku 4.5 · Fastest

[ ✓ Default ]  [ Sonnet ]
[ Sonnet 1M ]  [ Haiku ]
```

The body lists all four with descriptions so users see trade-offs.
The `✓` appears both on the active row and on the active button
(matched via the current `AgentSettings.model.load()`). In a topic
thread, the message is sent to the same `thread_id`.

**Callback data:** `model:default`, `model:sonnet`, `model:sonnet1m`,
`model:haiku`. Short aliases — survive model-ID renames.

**On click:**

1. Telegram callback dispatched by `dispatch.rs:453+` callback router
   (new branch matching prefix `"model:"`).
2. In groups: re-check allowlist (callback can come from any user who
   sees the keyboard).
3. Resolve alias → `ModelChoice`. Unknown alias → `warn!` log,
   `answer_callback_query("Unknown option")`, return.
4. Write to `agent.yaml` via `write_agent_yaml_model`.
5. Update `AgentSettings.model` via `ArcSwap::store`.
6. Edit the menu message (`bot.edit_message_text` +
   `bot.edit_message_reply_markup`) — refreshed checkmarks.
7. `bot.answer_callback_query("Switched to <label>")` — toast.

**Custom-model edge case:** if `agent.yaml` has a `model` value not in
`MODEL_CHOICES`, the menu shows `Current: <id> (custom)` above the four
canonical options, no `✓` on any button. Clicking a button transitions
to the canonical value (and overwrites the custom one — the user
explicitly chose the canonical option, intent is clear).

## Data flow

**Read path (every CC invocation):**

```
worker builds ClaudeInvocation
  → settings.model.load()  (ArcSwap::Guard<Arc<Option<String>>>)
  → invocation.set_model(guard.as_ref().clone())
  → invocation.into_args()  → [..., "--model", "<id>", ...]
```

**Write path (callback `model:sonnet`):**

```
① write_agent_yaml_model(yaml_path, Some("claude-sonnet-4-6"))
   - read → serde_yaml::Value
   - set value["model"] = "claude-sonnet-4-6"  (or remove for None)
   - serialize → temp file → atomic rename
② settings.model.store(Arc::new(Some("claude-sonnet-4-6".into())))
③ bot.edit_message_text + edit_message_reply_markup  (UI refresh)
④ bot.answer_callback_query("Switched to Sonnet")  (toast)
```

Order ①→② is load-bearing: if the yaml write fails, the in-memory state
is not changed, so on restart the visible state matches what was
persisted.

**Hot-reload path (watcher fires on yaml change):**

```rust
fn diff_classify(old: &Value, new: &Value) -> ChangeKind {
    let mut o = old.clone();
    let mut n = new.clone();
    if let Some(m) = o.as_mapping_mut() { m.remove("model"); }
    if let Some(m) = n.as_mapping_mut() { m.remove("model"); }
    if o == n { ChangeKind::HotReloadable } else { ChangeKind::RestartRequired }
}
```

- `RestartRequired` → existing path: `config_changed.store(true)` +
  `token.cancel()`.
- `HotReloadable` → parse `new["model"]`, `model_swap.store(Arc::new(parsed))`,
  `info!` log, do not cancel.
- Parse failure on either side → `RestartRequired` (fail-safe).
- Watcher caches the last-seen yaml as `old` for the next event.

When the user clicks a button, the watcher will fire after our own
write. It computes `HotReloadable` (only `model` changed), parses, and
stores — but the parsed value equals what `②` already stored. Idempotent.

## Error handling

Per `CLAUDE.rust.md` FAIL FAST: every error propagates. No `unwrap_or_default`,
no `.ok()`, no silent fallbacks.

| Failure | Response |
|---|---|
| `agent.yaml` read/parse fail in callback | `error!` log; `answer_callback_query("Failed to read config")`; ArcSwap not touched |
| Atomic write fail (disk, perms) | `error!` log; toast as above; ArcSwap not touched (consistency) |
| `bot.edit_message_text` fail | `warn!` log; still call `answer_callback_query` (model is already switched, just UI refresh failed) |
| Allowlist check fail in group | `answer_callback_query("Not allowed")`; no write, no swap |
| Unknown callback alias | `warn!` log; `answer_callback_query("Unknown option")` |
| Watcher: parse fail on new yaml | `warn!` log; fall back to `RestartRequired` (integrity over liveness) |
| Watcher: cached old-yaml missing on first run | `warn!` log; treat next event as `RestartRequired` |
| ArcSwap store | infallible — atomic pointer swap |

**Concurrency: two users click different buttons simultaneously.**
`agent.yaml` write is atomic-rename (last write wins). `ArcSwap::store`
is lock-free (last store wins). Watcher fires twice; second event sees
final state. Both users see toasts; final menu reflects the surviving
write. No locking needed — the operation is idempotent and the user
can always click again.

**Concurrency: switch during in-flight CC.**
Current CC subprocess was spawned with `--model <old>` and runs to
completion. Next message in the chat → fresh `settings.model.load()` →
new model. No kill, no retry. Documented behavior.

**Logging:** every switch logs at `info`:

```
INFO model switched: from=<old or "default"> to=<new or "default"> chat_id=… user_id=…
```

Trace lives in `~/.right/logs/<agent>.log`.

## Testing

**Unit (in-crate `#[cfg(test)]`):**

| Test | Crate / file | Coverage |
|---|---|---|
| `model_choices_aliases_unique` | bot / `model_command.rs` | all 4 aliases distinct |
| `model_choice_lookup_by_alias` | same | resolve known + unknown alias |
| `model_choice_active_match` | same | `None` → `default`; canonical → matching alias; unknown string → no match (custom) |
| `write_agent_yaml_model_round_trip` | right-agent / `agent_config.rs` | yaml with unknown fields + comments survives RMW |
| `write_agent_yaml_model_clear` | same | `Some(...)` → `None` removes key, no `model: null` left over |
| `write_agent_yaml_atomicity` | same | mocked write fail leaves original file intact |
| `config_watcher_diff_model_only` | bot / `config_watcher.rs` | `HotReloadable` |
| `config_watcher_diff_other_field` | same | `RestartRequired` |
| `config_watcher_diff_model_and_other` | same | `RestartRequired` |
| `config_watcher_diff_parse_fail` | same | `RestartRequired` |
| `agent_settings_arcswap_visibility` | bot / `handler.rs` | swap-store visible across tasks |

**Integration (`crates/bot/tests/`):**

| Test | Coverage |
|---|---|
| `model_command_dm_flow` | `/model` in DM → mock-Telegram receives sendMessage with inline keyboard; callback `model:sonnet` → yaml updated, message edited |
| `model_command_group_allowlisted` | same flow in group, allowlisted user — works |
| `model_command_group_not_allowlisted` | non-allowlisted user — toast `Not allowed`, yaml untouched |
| `model_hot_reload_no_restart` | yaml-only change to `model` → watcher does NOT cancel shutdown_token; ArcSwap reflects new value |
| `model_change_applied_next_invocation` | in-flight CC + switch → current keeps old `--model`, next fresh CC invocation has new `--model` |

**Out of scope:** real Anthropic API calls, real `claude` subprocess.
The integration tests assert against the args passed to a mocked
invocation builder.

## Files touched

| File | Change |
|---|---|
| `crates/right-agent/src/config/agent_config.rs` | + `pub model: Option<String>` field on `AgentConfig`; + `write_agent_yaml_model` MergedRMW helper |
| `crates/right-agent/templates/right/agent/agent.yaml` | + commented `# model: claude-sonnet-4-6` example with explanatory comment |
| `crates/bot/Cargo.toml` | + `arc-swap = "1.7"` |
| `crates/bot/src/telegram/dispatch.rs` | + `BotCommand::Model` variant; + `dptree::case![...]` branch → `handle_model`; + callback router branch on prefix `"model:"` → `handle_model_callback`; register command in all three scopes |
| `crates/bot/src/telegram/handler.rs` | `AgentSettings.model: Option<String>` → `Arc<ArcSwap<Option<String>>>`; adapt `dispatch.rs:147` callsite to wrap on construct |
| `crates/bot/src/telegram/worker.rs` | `WorkerContext.model` typed as `Arc<ArcSwap<Option<String>>>`; both `ClaudeInvocation` build sites call `.load()` immediately before `set_model` |
| `crates/bot/src/telegram/model_command.rs` | **new file** — `MODEL_CHOICES`, `handle_model`, `handle_model_callback`, menu rendering, helpers |
| `crates/bot/src/telegram/mod.rs` | + `pub(crate) mod model_command;` |
| `crates/bot/src/config_watcher.rs` | accept `Arc<ArcSwap<Option<String>>>` parameter; add `diff_classify` + `ChangeKind` enum; branch hot-reload vs restart |
| `crates/bot/src/lib.rs` | callsite of `spawn_config_watcher` (~line 449) — pass `settings_arc.model.clone()` |
| `crates/bot/tests/model_command.rs` | **new file** — integration tests above |
| `PROMPT_SYSTEM.md` | brief paragraph: model is now configurable via `/model` and `agent.yaml` `model:`, hot-reloaded |
| `ARCHITECTURE.md` | Add a brief note under Configuration Hierarchy that `model` in `agent.yaml` is hot-reloadable while all other fields remain restart-on-change. (`model` is already named in the table.) |
| `docs/architecture/lifecycle.md` (or appropriate satellite) | one paragraph documenting the `/model` flow and the hot-reload watcher path |

**Estimated size:** ~150 LoC for `model_command.rs`, ~50 for the
agent_config helper, ~40 for watcher diff, ~30 across other Rust files,
~200 LoC of tests. ~30 lines of doc edits. Total ≈ 500 LoC. No file
crosses the 900-LoC ceiling; new files stay well below.

## Dependencies and risks

- **`arc-swap`** is a small, well-maintained crate (used by `tracing`
  and many others); adding it is low-risk. Alternative would be
  `tokio::sync::RwLock<Option<String>>`, which is fine but adds an
  await on every CC invocation read path. Sync `ArcSwap::load` is the
  better fit.
- **`[1m]` model-ID syntax** must be verified to work with `claude -p
  --model claude-sonnet-4-6[1m]` before implementation lands. The
  system prompt of the current Opus 4.7 session shows
  `claude-opus-4-7[1m]` as the canonical model ID, so the syntax is
  expected to work; spec author has not run the literal command yet.
  If shell-quoting issues arise, fall back to a different encoding
  (e.g., `claude-sonnet-4-6-1m` if CC supports it). Decided at
  implementation time, not in the spec.
- **Race window between yaml-write and watcher fire** is real but
  idempotent (covered in Data flow). No mitigation needed.
- **`config_watcher` change touches a process-wide subsystem.** The
  `RestartRequired` path stays bit-identical; the new `HotReloadable`
  path is purely additive. Risk of regressing existing graceful-restart
  flow is low if the diff helper is well-tested.

## Open questions deferred to implementation

- Exact menu text wording in Russian/English. Default to English to
  match other bot copy; add Russian later if requested.
- Whether to include a "Cancel" button in the menu (CC's terminal
  picker does not have one — Esc cancels; in Telegram, sending
  another message naturally moves on). YAGNI for v1.
- Whether `/model` should optionally accept a positional model-ID
  argument (`/model claude-haiku-4-5`). YAGNI for v1; documented as a
  non-goal.
