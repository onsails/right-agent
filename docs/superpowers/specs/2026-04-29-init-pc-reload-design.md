# `right agent init` — auto-reload running process-compose

## Problem

`right agent init <name>` always ends with `next: right up`, regardless of whether process-compose (PC) is currently running. When PC *is* running, this is misleading on two counts:

1. The just-created agent's bot doesn't appear in PC. The user has to run `right down && right up` to pick it up — destructive for any other running bots.
2. The "next" line directs the user at the wrong action.

The infrastructure to hot-add a bot already exists. `right agent destroy` does the symmetric thing: detect PC via `PcClient::from_home(home)` + `health_check`, regenerate cross-agent codegen, then call `pc_client.reload_configuration()` (PC's `POST /project/configuration` diffs the new config against running state and adds/updates/removes processes).

## Goal

After `right agent init` finishes, if PC is running, the new agent's bot is added to PC live — no `right up` needed. The recap's tail line reflects what actually happened.

## Behavior

| Scenario | What happens after wizard ends | Recap tail |
|---|---|---|
| PC not running | Skip reload entirely. Detected via `PcClient::from_home(home)` returning `None`, or `health_check` failing. | `next: right up` *(unchanged)* |
| PC running, reload OK | Regenerate cross-agent codegen → `pc_client.reload_configuration()`. If `--force-recreate` on a pre-existing agent, also `pc_client.restart_process("<name>-bot")`. | `next: send /start to your bot in Telegram` |
| PC running, reload failed | Agent dir + sandbox stay (config is valid). Underlying error logged via `tracing::warn!`. | `⚠ reload  failed to add to running right` row + `next: right restart` |

### Why `restart_process` only on recreate

For a brand-new agent, the reload spawns a new process — no extra restart needed. For a pre-existing agent that was wiped and recreated (`--force-recreate`), PC sees no spec change for `<name>-bot` (same exec, same env), so a config reload alone wouldn't restart the bot — its working directory was just wiped under it. We force a clean restart explicitly in that case.

### Why we don't bail on reload failure

The agent is fully on disk and fully usable; only the live PC didn't pick it up. Bailing with an error after a successful interactive wizard would be jarring. Render the warn line, return `Ok(())`. Details go to `tracing::warn!` so they land in `~/.right/logs/` without cluttering the recap.

## Design

### New library helper

New module `crates/right-agent/src/agent/register.rs`, mirroring `destroy.rs`.

```rust
pub struct RegisterOptions {
    pub agent_name: String,
    /// True when init wiped a pre-existing agent dir (`--force-recreate` on
    /// an existing agent). Drives the post-reload `restart_process` call.
    pub recreated: bool,
}

pub struct RegisterResult {
    /// True if PC was alive and the reload succeeded.
    /// False if PC was not running (no `state.json`, stale port, health-check fail).
    pub pc_running: bool,
}

pub async fn register_with_running_pc(
    home: &Path,
    options: RegisterOptions,
) -> miette::Result<RegisterResult>;
```

Behavior:

1. `PcClient::from_home(home)?` returns `None` ⇒ `Ok(RegisterResult { pc_running: false })`, no other side effects. **This is the runtime-isolation guard mandated by `ARCHITECTURE.md` ("Runtime isolation — mandatory").**
2. `health_check()` fails ⇒ same: `Ok(RegisterResult { pc_running: false })` (stale `state.json` from a crashed PC).
3. PC alive ⇒ discover all agents, run `run_agent_codegen`, call `reload_configuration`. On error, return `Err` with the underlying message preserved. Caller renders the warn row + logs.
4. On reload success + `options.recreated == true`, call `restart_process("<name>-bot")`. Log via `tracing::warn!` and continue if restart fails — config is already correct on disk and in PC; only the live process didn't bounce. Then return `Ok(RegisterResult { pc_running: true })`.

Restart-failure is intentionally invisible in the recap. It's a niche edge case (PC alive enough to accept config-reload but not to bounce a process) that is much better surfaced via `~/.right/logs/` than via end-of-wizard chrome.

### CLI integration

**Flag rename** on the `agent init` subcommand:

- `--force` → `--force-recreate`. Hard rename. No clap alias for the old flag. Existing test callers updated.

**Wiring in `cmd_agent_init` (main.rs).** After the wizard's "✓ sandbox ready" output, before composing the recap:

```rust
let recreated = agent_existed && force_recreate;

// ... existing wizard / codegen / sandbox creation ...

let outcome = right_agent::agent::register::register_with_running_pc(
    home,
    RegisterOptions { agent_name: name.to_string(), recreated },
).await;

let recap = build_init_recap(&cfg, &mode, &chat_ids_detail, ..., outcome);
println!("{}", recap.render(theme));
```

Recap composition (caller-side, no `Recap` API change):

- `Ok(RegisterResult { pc_running: false })` → `.next("right up")`
- `Ok(RegisterResult { pc_running: true })` → `.next("send /start to your bot in Telegram")`
- `Err(e)` → `tracing::warn!(error = format!("{e:#}"), "PC reload failed")` + `.warn("reload", "failed to add to running right")` + `.next("right restart")`

### Recap rendering — examples

PC not running (today's behavior, unchanged):
```
▐  ✓ memory    hindsight
▐
▐  next: right up
```

PC running, reload OK:
```
▐  ✓ memory    hindsight
▐
▐  next: send /start to your bot in Telegram
```

PC running, reload failed:
```
▐  ✓ memory    hindsight
▐  ⚠ reload    failed to add to running right
▐
▐  next: right restart
```

The reload-failed wording deliberately doesn't summarize the underlying error — that goes to `tracing::warn!`.

## Tests

### Unit — `right-agent::agent::register`

1. `register_with_running_pc` against a tempdir home with no `state.json` returns `Ok(RegisterResult { pc_running: false, .. })` and produces no side effects.
2. Malformed `state.json` (`from_home` errors) propagates the error.
3. Stale `state.json` pointing at a closed port: `pc_running: false` returned (health_check fails cleanly).

### Unit — recap rendering

4. Three string-comparison tests assert the recap output for each path. The builder is fed a synthetic `RegisterResult`/`Err`.

### CLI integration — flag rename

5. Update `wizard_brand.rs` and `cli_integration.rs` callers from `--force` to `--force-recreate`.
6. Add one negative test: bare `--force` errors with clap's "unexpected argument" (no hidden alias).
7. Existing `wizard_brand.rs` assertions that match `next: right up` keep matching — those tests run with no `state.json`, so the PC-not-running path holds.

### Integration — live PC

Out of scope for CI (requires `right up --detach` running). The design notes the manual test sequence:

1. `right up --detach`.
2. `right agent init test2` (new agent).
3. Expect: new `test2-bot` process appears in PC; recap ends with `next: send /start...`.
4. `right agent init --force-recreate test2`.
5. Expect: `test2-bot` is restarted (verifiable from PC logs).

## Non-goals

- Replacing the `--force-recreate` wipe with a full `destroy::run` call. That's a larger surgery and would also handle the Telegram webhook, sandbox cleanup, etc. — out of scope here. We only add the post-reload `restart_process` call so the user-visible recreate behavior remains correct under a running PC.
- Cloudflared hot-reload semantics. Cross-agent codegen rewrites the cloudflared config; PC's reload picks up any spec changes. If cloudflared needs an explicit restart, that's a pre-existing question shared with `destroy` and out of scope.
- Adding alias support for the old `--force` flag. RightClaw is alpha; clean rename.

## References

- `crates/right-agent/src/agent/destroy.rs` — symmetric pattern; the model for `register.rs`.
- `crates/right-agent/src/runtime/pc_client.rs` — `reload_configuration`, `restart_process`, `health_check`, `from_home`.
- `ARCHITECTURE.md` § "Runtime isolation — mandatory" — the `from_home` contract this design depends on.
- `crates/right/src/main.rs` `cmd_agent_init` — caller to be modified.
