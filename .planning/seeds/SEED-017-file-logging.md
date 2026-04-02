---
id: SEED-017
status: dormant
planted: 2026-04-02
planted_during: v3.0 Teloxide Bot Runtime (phase 28.2 UAT)
trigger_when: Next milestone — any work on observability, diagnostics, or rightclaw up/bot startup
scope: small
---

# SEED-017: File logging for rightclaw, process-compose, and agents

## Why This Matters

Currently everything goes to stdout/stderr and disappears when the TUI is closed or the
terminal scrolls. In production (detached mode, systemd, server deploys) there's no way
to debug what happened. Three log streams need to land in files:

1. **rightclaw bot process** — tracing output from the Rust bot (message routing, CC invocation
   errors, debounce decisions) → rolling file via `tracing-appender`
2. **process-compose** — PC has native `log_location` per-process config; agent stdout/stderr
   can be redirected there with one field in the generated YAML
3. **PC orchestrator itself** — PC writes its own log; expose its location via `rightclaw doctor`

Without file logs, post-mortem debugging of headless agents is guesswork.

## When to Surface

**Trigger:** Next milestone — surface whenever touching startup, `rightclaw up`, or the
`bot` subcommand.

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches:
- Observability / diagnostics improvements
- Production deployment / detached mode work
- Any touch of `main.rs` tracing init or process-compose codegen

## Scope Estimate

**Small** — Three independent sub-tasks, each a few hours:

1. `tracing-appender` rolling file for rightclaw bot process (`main.rs:167`)
   - Log dir: `~/.rightclaw/logs/rightclaw.log` (rolling daily)
   - Keep stdout for interactive use; add file writer in parallel
   - `tracing_subscriber::fmt().with_writer(non_blocking)` pattern

2. `log_location` in generated `process-compose.yaml.j2` per agent
   - PC natively supports `log_location: /path/to/agent.log` per process
   - Path: `~/.rightclaw/logs/<agent-name>.log`
   - One field in `BotAgent` template context + one line in the Jinja2 template

3. Expose log paths in `rightclaw doctor` output
   - Print log file locations so user knows where to look

## Breadcrumbs

- `crates/rightclaw-cli/src/main.rs:167` — `tracing_subscriber::fmt()` init — add file appender here
- `templates/process-compose.yaml.j2:8` — process entry — add `log_location:` field here
- `crates/rightclaw/src/codegen/process_compose.rs` — `BotAgent` struct — add `log_path: String` field
- `crates/rightclaw/src/doctor.rs` — add log path check/display
- `crates/rightclaw/src/config.rs` — log dir config (default `~/.rightclaw/logs/`)

## Notes

- `tracing-appender` crate: use `rolling::daily` for automatic rotation, `NonBlocking` writer
  to avoid blocking async tasks on disk I/O
- PC `log_location` path: must be absolute. Derive from `~/.rightclaw/logs/<agent>.log` at
  codegen time (resolved at `rightclaw up` invocation, not in template)
- Consider: `rightclaw logs <agent>` subcommand that tails the log file (future seed)
