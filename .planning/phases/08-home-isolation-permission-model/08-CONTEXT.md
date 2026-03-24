# Phase 8: HOME Isolation & Permission Model - Context

**Gathered:** 2026-03-24
**Status:** Ready for planning (pending OAuth validation gate — see D-10)

<domain>
## Phase Boundary

Per-agent HOME directory isolation: shell wrapper sets `HOME=$AGENT_DIR`, `rightclaw up`
generates per-agent `.claude.json`, credential symlink, git/SSH env forwarding, corrected
sandbox paths with `allowRead`/`denyRead` using absolute host HOME paths, and automated
integration tests covering all security assumptions.

New capabilities that belong in other phases: Telegram channel copy (Phase 9), git init
in agent dir (Phase 9), settings.local.json scaffold (Phase 9).

</domain>

<decisions>
## Implementation Decisions

### Shell Wrapper

- **D-01:** Shell wrapper sets `HOME=$AGENT_DIR` before `exec` (HOME-01). Use an early
  `export HOME="$WORKING_DIR"` line in the template so all subsequent shell expansion
  uses agent HOME.

- **D-02:** Shell wrapper forwards git/SSH identity env vars (HOME-04):
  `GIT_CONFIG_GLOBAL`, `SSH_AUTH_SOCK`, `GIT_SSH_COMMAND`, `GIT_AUTHOR_NAME`,
  `GIT_AUTHOR_EMAIL`. Capture values from the real environment BEFORE `HOME` is
  overridden (or use `export VAR="$VAR"` explicit propagation to be explicit).

- **D-03:** Shell wrapper forwards `ANTHROPIC_API_KEY` from environment if set. Use
  `export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"` — no-op if unset, forward if
  present. Required because OAuth breaks under HOME override on Linux (credentials
  file is handled by symlink, but key forwarding is an additional safety net).

- **D-04:** `--dangerously-skip-permissions` stays in wrapper (PERM-01). Already
  implemented. No change.

### Per-Agent .claude.json

- **D-05:** `rightclaw up` generates per-agent `.claude.json` inside `$AGENT_DIR`
  (HOME-02). Content must include:
  - `projects[agent_abs_path].hasTrustDialogAccepted: true` — workspace trust
  - `bypassPermissionsAccepted: true` (or whatever key suppresses the bypass warning
    dialog — verify field name empirically)

- **D-06:** `rightclaw up` STOPS writing to host `~/.claude.json`. Under HOME override,
  CC reads `$AGENT_DIR/.claude.json` — host entries are irrelevant. This also affects
  `init.rs`: `pre_trust_directory()` must be updated to write per-agent `.claude.json`
  instead of (or in addition to) host file. Keep writing host file from `init` ONLY
  as fallback if HOME override is experimentally reverted (see D-10).

### Credential Symlink

- **D-07:** `rightclaw up` creates symlink: `$AGENT_DIR/.claude/.credentials.json`
  → `[absolute_real_host_home]/.claude/.credentials.json`. CRITICAL: use
  `dirs::home_dir()` captured BEFORE any HOME override — never use `~/` here as it
  would create a self-referential symlink if HOME is already overridden. HOME-03.
  **Symlink is mandatory — without it agents cannot authenticate.**

- **D-08:** If host credentials file does not exist (new user, macOS Keychain-only
  auth): warn prominently and skip the symlink — do NOT fail fast. Message:
  "Warning: no OAuth credentials found at [path] — agents will need ANTHROPIC_API_KEY
  to authenticate." Without the symlink, agents are completely non-functional.

### Sandbox Path Resolution

- **D-09:** All sandbox paths in generated `settings.json` use absolute paths resolved
  at `rightclaw up` time (HOME-05):
  - `allowWrite`: already uses `agent.path.display()` — absolute, no change needed
  - `denyRead`: switch from `~/`-relative to `[resolved_host_home]/.ssh`,
    `[resolved_host_home]/.aws`, `[resolved_host_home]/.gnupg`
  - Use `dirs::home_dir()` captured BEFORE any HOME override to get real host HOME

- **D-09b:** Add `allowRead` support to sandbox config. Agent read access:
  - Default: `allowRead: [absolute_agent_path]` — agents read only their own dir
  - `denyRead: [absolute_real_host_home/]` — blocks entire host HOME (belt)
  - Users add extra `allowRead` paths via `agent.yaml sandbox.allow_read: []`
  - **IMPORTANT**: `SandboxOverrides` struct needs new `allow_read: Vec<String>` field
  - Use absolute paths (not `.`) for `allowRead` — avoids conflict when agent dir
    is inside denied host HOME. Use `agent.path` resolved at generation time.
  - Verify empirically: does CC `allowRead` specificity override parent `denyRead`?
    (e.g., does `allowRead: /home/user/.rightclaw/agents/right` beat
    `denyRead: /home/user/`?) — note for planner to test.

### OAuth Validation Gate

- **D-10: VALIDATED 2026-03-24.**

  Test results:
  - Baseline (no symlink): FAILS — "Not logged in · Please run /login"
  - Symlink to absolute real credentials path: **WORKS** — full API response

  **Conclusion: symlink is REQUIRED.** Without it, agents cannot authenticate at all.
  HOME override approach is confirmed valid. Proceed with current plan.

### Integration Tests

- **D-11:** All security assumptions must have automated integration tests using
  `claude -p ... --output-format json`. Required test scenarios:
  1. OAuth under bare HOME override (baseline — no symlink)
  2. OAuth with credential symlink in place (HOME-03 approach)
  3. Git env vars forwarded — verify `git config user.name` returns host identity
     inside agent session under HOME override
  4. `denyRead` enforcement — attempt to read a blocked file (e.g., host `.ssh/config`)
     via `claude -p`, expect refusal/error
  5. `allowRead` enforcement — attempt to read a file outside agent dir that isn't
     in allowRead, expect refusal
  6. Any other security boundary verification needed for sandbox assumptions

  Tests use `assert_cmd` (already in dev deps) + `claude -p --output-format json`
  pattern. Tests are integration tests in `crates/rightclaw-cli/tests/`.

### Claude's Discretion

- Exact `bypassPermissionsAccepted` field name in `.claude.json` — verify empirically
  or from CC source. The intent is to suppress the "bypass permissions" warning dialog.
- Whether to retain or remove `pre_trust_directory()` host-level writes from `init.rs`
  entirely vs. keeping them as a non-HOME-override fallback.
- `allowRead`/`denyRead` conflict resolution semantics — test and implement accordingly.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Core implementation files
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — wrapper generator (D-01, D-02, D-03)
- `templates/agent-wrapper.sh.j2` — wrapper template to extend with HOME + env vars
- `crates/rightclaw/src/codegen/settings.rs` — sandbox settings generation (D-09, D-09b)
- `crates/rightclaw/src/agent/types.rs` — SandboxOverrides struct (needs allow_read field)
- `crates/rightclaw/src/init.rs` — pre_trust_directory() + .claude.json generation (D-05, D-06)
- `crates/rightclaw-cli/src/main.rs` — cmd_up() flow (D-05, D-06, D-07)

### Requirements and planning
- `.planning/REQUIREMENTS.md` — Phase 8 requirements: HOME-01, HOME-02, HOME-03, HOME-04, HOME-05, PERM-01, PERM-02

### Testing
- `crates/rightclaw-cli/tests/cli_integration.rs` — existing integration tests (pattern reference for D-11)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `pre_trust_directory()` in `init.rs`: already generates `.claude.json` trust entries and writes `skipDangerousModePermissionPrompt` to settings. Needs refactoring to target agent HOME rather than host HOME.
- `generate_settings()` in `settings.rs`: existing sandbox codegen. Extend with `allowRead` support and switch denyRead to absolute paths.
- `SandboxOverrides` struct in `agent/types.rs`: add `allow_read: Vec<String>` field (same pattern as existing `allow_write`).
- `cmd_up()` in `main.rs:297-341`: existing per-agent generation loop — add `.claude.json` generation and credential symlink here, same pattern as `settings.json`.

### Established Patterns
- Per-agent generation in `cmd_up()`: for each agent, generate files into `agent.path.join(".claude")/`. Settings.json is already regenerated on every `up`. Same pattern for `.claude.json`.
- `agent.path.display().to_string()` for absolute paths — already correct for `allowWrite`. Use same pattern for absolute `allowRead` and `denyRead` using `dirs::home_dir()` for the real host home.
- `dirs::home_dir()` already used in `init.rs` for finding host home — reuse in `settings.rs`.

### Integration Points
- Shell wrapper template: add `HOME`, env var exports before `exec` line
- `generate_settings()`: add `allowRead` to filesystem section, change `denyRead` to absolute paths (signature change: needs real host HOME path as parameter)
- `cmd_up()`: add credential symlink step + `.claude.json` generation step in the per-agent loop
- `SandboxOverrides`: backward-compatible addition of `allow_read` field (default empty vec)

</code_context>

<specifics>
## Specific Ideas

- User confirmed: `allowRead` and `denyRead` both exist in CC sandbox API (user referenced docs showing `"allowRead": ["."]` usage). Planner should verify the exact API shape and test allowRead/denyRead precedence.
- User wants `allowRead: ["."]` semantics — restrict agent reads to agent dir by default, with user-extensible paths via agent.yaml. This is a tight, clean sandbox.
- OAuth test: use `claude -p "hello" --output-format json` as the test vector. JSON output makes pass/fail programmatic.
- ANTHROPIC_API_KEY forwarding: `export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"` pattern — no-op when not set, explicit forward when set.

</specifics>

<deferred>
## Deferred Ideas

- Agent-level `env:` section in agent.yaml for arbitrary env var forwarding — would be a clean way to support GITHUB_TOKEN, NPM_TOKEN etc. Phase 9 or later.
- `allowRead` for project repos outside agent dir — users can add via sandbox.allow_read today, but a common-patterns list might help. Future UX improvement.
- Strict mode: require ANTHROPIC_API_KEY, error if absent under HOME override. Deferred until OAuth test results are known.
- macOS Keychain credential strategy — if symlink doesn't help on macOS (credentials in Keychain, not file), need separate approach. Post-test decision.

</deferred>

---

*Phase: 08-home-isolation-permission-model*
*Context gathered: 2026-03-24*
