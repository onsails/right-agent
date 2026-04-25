# Rename `rightclaw` → `right-agent`

**Date:** 2026-04-26
**Status:** Design

## Motivation

The project is being rebranded from `rightclaw` to `right-agent`. The GitHub
repository has already been renamed to `onsails/right-agent`. This spec
defines how the codebase, runtime layout, published artefacts, and existing
deployments transition to the new name without losing user data or breaking
already-running agents.

The rename touches three independent surfaces:

- **Code** — three Cargo crates, the binary name, internal identifiers, env
  vars, sandbox names, install scripts, release-plz config.
- **State on disk** — `~/.rightclaw/` (containing per-agent SQLite, agent
  configs, secrets, backups, OpenShell SSH config, runtime state).
- **Brand prose** — README, ARCHITECTURE, CLAUDE files, security docs,
  agent-facing templates.

Historical artefacts (commit history, `docs/plans/**`, `docs/superpowers/specs/**`,
older `CHANGELOG.md` entries) are deliberately left alone — they describe what
the project was called when those decisions were made.

## Naming map

| Old | New | Notes |
|---|---|---|
| Repo `onsails/rightclaw` | `onsails/right-agent` | Already done by user. |
| Brand prose "RightClaw" | "Right Agent" | Living docs only. |
| Binary `rightclaw` | `right` | Short, matches MCP namespace and `templates/right/`. |
| CLI crate `rightclaw-cli` (in `crates/rightclaw-cli/`) | crate `right` (in `crates/right/`) | `[[bin]] name = "right"`. |
| Library crate `rightclaw` (in `crates/rightclaw/`) | crate `right-agent` (in `crates/right-agent/`) | Matches repo name. |
| Bot crate `rightclaw-bot` (in `crates/bot/`) | crate `right-bot` (dir unchanged) | Both binaries (`right`, `right-bot`) share the `right-` prefix. |
| Runtime root `~/.rightclaw/` | `~/.right/` | Auto-migrated (see below). |
| Env var `RIGHTCLAW_HOME` | `RIGHT_HOME` | Hard rename — old reads return nothing. |
| Env var `RC_RIGHTCLAW_HOME` | `RC_RIGHT_HOME` | Hard rename, internal (process-compose passthrough). |
| New sandbox name `rightclaw-<agent>-<ts>` | `right-<agent>-<ts>` | Existing sandboxes keep their `rightclaw-*` names — opaque IDs. |
| Test sandbox `rightclaw-test-<name>` | `right-test-<name>` | `test_support` only. |
| Release-plz tags `rightclaw-v*`, `rightclaw-bot-v*` | `right-agent-v*`, `right-bot-v*` | Aggregate tag stays `v{{version}}` (now driven by crate `right`). |
| install.sh URL `onsails/rightclaw` | `onsails/right-agent` | |

**Untouched** (already correct or independent of brand):

- MCP namespace `right` (agent-facing tool prefix `mcp__right__*`).
- Skill IDs `rightmcp`, `rightcron`, `rightskills` — these are MCP-tool-name
  components, not the brand name.
- Process name `right-mcp-server` in `process-compose.yaml`.
- Template directory `templates/right/`.
- Internal sync target `/sandbox/.platform/` inside sandboxes.
- `~/.claude/` host paths.
- SQLite `data.db` schema — column values and table names are neutral.
- Agent-owned files (`AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `MEMORY.md`,
  `TOOLS.md`, `USER.md`) — written by users / bootstrap, not the codebase.

## Migration semantics

### Home directory auto-rename

Resolved in `right_agent::config::resolve_home()` (called by every CLI
command). Runs before any state is read or written.

```
1. if RIGHT_HOME env set → use it. No migration.
2. if ~/.right/ exists → use it. No migration.
3. if ~/.rightclaw/ exists:
   ├─ check if process-compose is running:
   │   ├─ read ~/.rightclaw/run/state.json (if present)
   │   ├─ probe http://127.0.0.1:<pc_port>/ with bearer token
   │   └─ if 200 OK: ERROR. Fail with:
   │      "Detected ~/.rightclaw/ with a running process-compose. Stop it
   │       before upgrade — run `<old binary path> down` (or kill the PC),
   │       then re-run."
   ├─ atomic std::fs::rename(~/.rightclaw, ~/.right)
   ├─ INFO log: "Migrated ~/.rightclaw/ → ~/.right/"
   └─ use ~/.right/.
4. else → ~/.right/ (fresh install).
```

**Concurrency**: two `right` invocations racing on rename are idempotent —
the second sees `~/.right/` already exists at step 2 and proceeds. The
underlying `rename(2)` is also atomic on POSIX.

**Cross-filesystem failure**: `std::fs::rename` returns `EXDEV` if `$HOME`
and the target are on different mounts (rare). Catch and report manually:
"Migration failed (cross-filesystem move). Run `mv ~/.rightclaw ~/.right`
yourself, then re-run." No automatic copy-and-delete fallback — too risky.

**Permissions**: if rename fails with `EACCES`/`EPERM`, report the OS error
verbatim and exit. User fixes their own permissions.

### OpenShell sandboxes

Sandboxes are persistent and named at creation time. Rule: **never recreate
a sandbox to change its name**. Two consequences:

- Existing `rightclaw-<agent>-<ts>` sandboxes keep their names. Bot startup
  finds them by reading `agent.yaml`'s `sandbox.name` field — unchanged.
- New sandboxes (created by `agent init` or sandbox migration after
  filesystem-policy changes) use the new prefix `right-<agent>-<ts>`.
- The `pkill_test_orphans` cleanup matches both `rightclaw-test-*` and
  `right-test-*` patterns until all old test orphans have aged out.

### Env vars — hard rename

`RIGHT_HOME` is read; `RIGHTCLAW_HOME` is not. Users with a custom
`RIGHTCLAW_HOME` export in their shell rc file will silently fall through to
the auto-migration path — fine if their custom path was the default
`~/.rightclaw/`, broken if it pointed elsewhere. The blast radius is small
(power users only) and a clear path forward exists (re-export as
`RIGHT_HOME`).

`RC_RIGHTCLAW_HOME` and `RC_RIGHT_HOME` flip together: generated
`process-compose.yaml` writes `RC_RIGHT_HOME`, and `right-bot` reads
`RC_RIGHT_HOME`. No dual-read.

### SQLite schema

No migration needed. Schema columns and values are neutral
(`telegram_sessions`, `cron_specs`, `mcp_servers`, etc., contain no
`rightclaw` strings).

### Sandbox state

Inside the sandbox, `/sandbox/.platform/` and `/sandbox/.claude/` are
unaffected. The host-side platform-store sync continues to redeploy these
paths every 5 minutes; the sync target is the sandbox name (which hasn't
changed for old sandboxes).

## Codebase changes

### Workspace

`Cargo.toml`:

```toml
[workspace]
members = ["crates/right-agent", "crates/right", "crates/bot"]
resolver = "3"
```

### Per-crate

- `crates/right-agent/Cargo.toml`: `name = "right-agent"`. Module name in
  Rust source: `right_agent` (Cargo converts `-` to `_`).
- `crates/right/Cargo.toml`: `name = "right"`, `[[bin]] name = "right"`. The
  CLI crate.
- `crates/bot/Cargo.toml`: `name = "right-bot"`. Directory stays `crates/bot/`
  to mirror the existing short-dir convention.
- All inter-crate path deps update accordingly.
- All `use rightclaw::...` → `use right_agent::...`.

### String replacement table

Mechanical replacements, scoped to code + living docs (see
"Files in / out of scope" below).

| Pattern | Replacement | Context |
|---|---|---|
| `rightclaw` (lowercase identifier) | `right_agent` (Rust modules), `right-agent` (TOML/YAML/CLI), `right` (binary name) | Context-dependent. |
| `RightClaw` | `Right Agent` | Living docs prose. |
| `RIGHTCLAW_HOME` | `RIGHT_HOME` | Code + docs. |
| `RC_RIGHTCLAW_HOME` | `RC_RIGHT_HOME` | Code + docs. |
| `rightclaw-cli` (Cargo name) | `right` | Cargo + release-plz. |
| `rightclaw-bot` (Cargo name) | `right-bot` | Cargo + release-plz. |
| `~/.rightclaw/` | `~/.right/` | Path literals + docs. |
| `rightclaw-<agent>-` (sandbox prefix) | `right-<agent>-` | Only at sandbox-creation sites. |
| `rightclaw-test-` | `right-test-` | `test_support` only. |
| `onsails/rightclaw` | `onsails/right-agent` | install.sh, README badges, doc links. |
| `RIGHTCLAW_VERSION` (install.sh env var) | `RIGHT_VERSION` | install.sh only. |

### Files needing structural review (not just `sed`)

- `Cargo.toml` (workspace root) — workspace member paths.
- `crates/right-agent/Cargo.toml`, `crates/right/Cargo.toml`,
  `crates/bot/Cargo.toml` — name, bin, deps.
- `release-plz.toml` — package definitions, tag templates, changelog group
  and aggregate tag.
- `templates/process-compose.yaml.j2` — `RC_RIGHT_HOME` env var, process
  names that mention `rightclaw` (verify there are none).
- `install.sh` — repo URL, env var, banner.
- `README.md`, `ARCHITECTURE.md`, `CLAUDE.md`, `CLAUDE.rust.md`,
  `PROMPT_SYSTEM.md`, `docs/SECURITY.md`, `docs/brand-guidelines.html` —
  prose.
  Paths in this list use the **pre-rename** crate layout — it's where the
  implementer will be editing.
- `crates/rightclaw-cli/src/main.rs` — `clap` `#[command(name = "...")]`,
  help text, any error message that names the binary.
- `crates/rightclaw/src/config.rs` (or wherever `RIGHTCLAW_HOME` is read)
  — env var name + add the auto-migration helper.
- `crates/rightclaw/src/test_support.rs` — `rightclaw-test-` prefix +
  `pkill_test_orphans` patterns.
- `templates/right/agent/BOOTSTRAP.md`, `templates/right/agent/agent.yaml`
  — agent-facing references (verify and update).
- `crates/bot/src/stt/whisper.rs`, `crates/bot/src/stt/mod.rs`,
  `crates/bot/src/telegram/memory_alerts.rs`, `crates/rightclaw/src/init.rs`,
  `crates/rightclaw/src/doctor.rs` — env var reads, log messages,
  user-facing error strings.
- `tests/e2e/verify-sandbox.sh` — env var name.
- `Cargo.lock` — regenerate after Cargo.toml changes.

### Files explicitly out of scope

- `docs/superpowers/specs/**` (164 files) — historical specs.
- `docs/plans/**` — historical plans.
- `CHANGELOG.md` — historical entries unchanged. New entries from this
  release onward use new names.
- Git commit messages — left as-is (rewriting history is high-risk and the
  project value is in current-state docs, not log archaeology).

## Verification

All must pass before merge:

1. `cargo build --workspace` (debug) — confirms compile.
2. `cargo test --workspace` — unit + integration tests, including
   `crates/bot/tests/sandbox_upgrade.rs`.
3. `cargo clippy --workspace --all-targets -- -D warnings`.
4. **Grep gate**:

   ```
   rg -i 'rightclaw' --no-ignore \
     -g '!target' -g '!.git' \
     -g '!docs/superpowers/specs/**' \
     -g '!docs/plans/**' \
     -g '!CHANGELOG.md'
   ```

   Must return zero hits. The exclude list is the canonical "out of scope"
   set from this spec.

5. `Cargo.lock` regeneration — confirm no stale `rightclaw*` entries.

6. **Auto-migrate unit test** in `crates/right/tests/migration_test.rs`
   (new file, post-rename path):

   - Set up tmpdir as fake `$HOME` with `tmp/.rightclaw/agents/foo/agent.yaml`.
   - No live PC. Run a `right`-style entry that calls `resolve_home()`.
   - Assert: `tmp/.rightclaw/` no longer exists; `tmp/.right/` exists with
     same contents; INFO log emitted.

7. **PC-running-guard test** in the same file:

   - Set up tmpdir with `tmp/.rightclaw/run/state.json` pointing at a
     `httptest`-style local server returning 200.
   - Assert: rename does not happen; error message names the path.

8. **End-to-end smoke** — `tests/e2e/verify-sandbox.sh` updated for new env
   var, runs against live OpenShell. Tests `right init` → `right agent init`
   → `right up` → message round-trip → `right down`.

9. **Manual upgrade test** (one-time, documented):

   - Take a workstation with a populated `~/.rightclaw/` running prod agents.
   - `rightclaw down` (old binary).
   - `cargo install --path crates/right --force`.
   - `right up`.
   - Verify: `~/.rightclaw/` is gone, `~/.right/` exists with full contents,
     OpenShell sandboxes (with their old `rightclaw-*` names) reconnect via
     gRPC, bot resumes Telegram polling, an inbound message produces a reply.

## Risks

- **Cross-filesystem rename failure (`EXDEV`)** — caught with explicit error
  message and manual fallback instructions. No automatic copy.
- **Permissions on `$HOME`** — caught with raw OS error + exit. User fixes.
- **Concurrent `right` invocations during migration** — idempotent by
  design; second caller sees `~/.right/` exists.
- **Old `state.json` in migrated dir** — carries stale `pc_port` /
  `pc_api_token`. `right up` regenerates them; no action required.
- **User has `RIGHTCLAW_HOME` set in shell rc pointing somewhere
  non-default** — silently broken (their override no longer applies). Only
  noticeable to power users; small blast radius. Documented in CHANGELOG.
- **Stale `git remote` on local clones** — `origin` still points at
  `onsails/rightclaw.git`. GitHub redirects work, but the user should run
  `git remote set-url origin <new>`. Documented in CHANGELOG.

## Out of scope (user-side actions)

- Local working directory rename (`/Users/molt/dev/rightclaw` → `right-agent`).
- `git remote set-url`.
- User shell rc files exporting `RIGHTCLAW_HOME`.
- Renaming the user's existing OpenShell sandboxes.
- Pre-existing `~/.rightclaw/` deployments on machines that don't run the
  new binary (no action — they continue to work with the old binary they
  installed).
