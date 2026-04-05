# Phase 10: Doctor & Managed Settings - Context

**Gathered:** 2026-03-25
**Status:** Ready for planning

<domain>
## Phase Boundary

Add `rightclaw config strict-sandbox` command that writes `/etc/claude-code/managed-settings.json`
with `allowManagedDomainsOnly: true` (opt-in, requires sudo), and extend `rightclaw doctor` to
detect that file and warn about conflicts with per-agent sandbox config.

Out of scope: any change to per-agent settings.json generation, sandbox defaults, or how `rightclaw up` works.

</domain>

<decisions>
## Implementation Decisions

### CLI Shape (TOOL-01)

- **D-01:** `rightclaw config strict-sandbox` is implemented as a **nested subcommand**. Add a `Config`
  variant to the top-level `Commands` enum with its own `#[command(subcommand)] command: ConfigCommands`
  field. `ConfigCommands` enum has one variant: `StrictSandbox`.

  ```rust
  pub enum Commands {
      // ... existing variants ...
      /// Manage RightClaw configuration
      Config {
          #[command(subcommand)]
          command: ConfigCommands,
      },
  }

  pub enum ConfigCommands {
      /// Enable machine-wide domain blocking via managed settings (requires sudo)
      StrictSandbox,
  }
  ```

- **D-02:** The command is **cross-platform** (Linux and macOS). Write to
  `/etc/claude-code/managed-settings.json` on both platforms. `/etc/` exists on macOS. Planner
  should verify the exact path CC uses (may differ) — but default assumption is same path.

### sudo / Privilege Handling (TOOL-01)

- **D-03:** **Attempt write, surface clear error.** Do not re-exec via sudo. Do not check uid upfront.
  Just call `std::fs::create_dir_all("/etc/claude-code")` + `std::fs::write(...)`. If either fails with
  permission denied, return a miette error:
  ```
  Permission denied writing /etc/claude-code/managed-settings.json
  hint: Run with elevated privileges: sudo rightclaw config strict-sandbox
  ```
  On success, print confirmation: `Wrote /etc/claude-code/managed-settings.json — machine-wide domain
  blocking enabled.`

- **D-04:** The managed-settings.json content written is exactly:
  ```json
  {"allowManagedDomainsOnly": true}
  ```
  Nothing more. Idempotent — overwrite unconditionally on every invocation.

### Doctor Conflict Check (TOOL-02)

- **D-05:** `rightclaw doctor` checks for `/etc/claude-code/managed-settings.json` existence. This
  check runs on both Linux and macOS (same path assumption, same cross-platform policy as D-02).

- **D-06:** **Rich warning** — not just file existence. Read the file content, parse it, and check
  if `allowManagedDomainsOnly` is `true`. If it is, emit a `CheckStatus::Warn` with detail:
  ```
  allowManagedDomainsOnly:true — per-agent allowedDomains may be overridden by system policy
  ```
  Fix hint: `Review /etc/claude-code/managed-settings.json — enabled via: sudo rightclaw config strict-sandbox`

- **D-07:** If the file exists but cannot be parsed as JSON or `allowManagedDomainsOnly` is absent
  or false: emit `CheckStatus::Warn` with detail `managed-settings.json found — content may affect
  agent sandbox behavior`. Same Warn severity, non-fatal. Do not Fail on unrecognized content.

- **D-08:** If `/etc/claude-code/managed-settings.json` does NOT exist: no check emitted (skip
  silently, not a Pass/Fail). This avoids polluting the doctor output for users who never used
  `config strict-sandbox`.

### Claude's Discretion

- Whether to add a `check_managed_settings()` private function in `doctor.rs` or inline the logic
  in `run_doctor()`. Prefer extracted function for testability.
- Exact content of the success print message for `config strict-sandbox`.
- JSON parsing strategy — `serde_json` already a transitive dep via reqwest, or use manual string
  check if crate not available. Planner confirms dep availability.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Core implementation files
- `crates/rightclaw-cli/src/main.rs` — `Commands` enum (add `Config` variant + dispatch), existing
  `cmd_doctor()` call pattern
- `crates/rightclaw/src/doctor.rs` — `run_doctor()`, `DoctorCheck`, `CheckStatus` (add managed
  settings check here)
- `crates/rightclaw/src/runtime/deps.rs` — `verify_dependencies()` pattern (Warn-only checks)
- `crates/rightclaw/Cargo.toml` — confirm if `serde_json` is available as a dep for JSON parsing

### Requirements
- `.planning/REQUIREMENTS.md` — Phase 10 requirements: TOOL-01, TOOL-02

### Prior phase context
- `.planning/phases/09-agent-environment-setup/09-CONTEXT.md` — D-03 git Warn pattern (non-fatal
  check in doctor, same severity model as D-06/D-07 here)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `doctor.rs` `check_binary()` pattern — returns `DoctorCheck` with name/status/detail/fix. New
  `check_managed_settings()` follows same shape.
- `doctor.rs` `run_doctor()` — `checks.extend(...)` pattern for adding new check groups. Add
  `check_managed_settings()` result here (conditionally push — only if file exists).
- `DoctorCheck` + `CheckStatus::Warn` — already exists, used for BOOTSTRAP.md and git. Reuse as-is.

### Established Patterns
- Linux-only gating: `if std::env::consts::OS == "linux" { ... }` in `run_doctor()`. The managed
  settings check does NOT use this gate — it's cross-platform (D-02, D-05).
- Attempt-and-surface-error: existing miette error pattern in `cmd_up()` — `std::fs::write()` mapped
  to `miette::miette!("failed to write {}: {e:#}", path.display())`. Same pattern for D-03.
- `cmd_*` functions in `main.rs` — sync functions returning `miette::Result<()>`.
  `cmd_config_strict_sandbox()` will be a new sync function (no async needed — just fs writes).

### Integration Points
- `Commands` enum in `main.rs`: add `Config { command: ConfigCommands }` variant + match arm
  that calls `cmd_config_strict_sandbox()`
- `run_doctor()` in `doctor.rs`: add optional managed settings check after existing agent checks
- No new crates expected — `serde_json` should be available transitively, or use simple string
  contains check as fallback

</code_context>

<specifics>
## Specific Ideas

- User confirmed: ship the feature as planned despite low solo-dev value. Rationale: it completes
  v2.1 cleanly and is useful for teams running agents in production.
- Cross-platform assumption: `/etc/claude-code/managed-settings.json` is the CC managed settings
  path on both Linux and macOS. Planner should verify against CC docs/source before implementing.
- The command is intentionally spartan — no `--dry-run`, no `--remove`, no undo. One-shot opt-in.
  Future phases can add `rightclaw config unset strict-sandbox` if needed.

</specifics>

<deferred>
## Deferred Ideas

- **`rightclaw config unset strict-sandbox`** — remove managed-settings.json. Out of scope for now.
- **Managed settings allowlist population** — if `allowManagedDomainsOnly: true`, users need to
  populate the domain allowlist too. RightClaw doesn't manage the allowlist — that's a CC
  configuration concern. Future integration possibility.
- **`rightclaw agent init` subcommand** — guided multi-agent setup (from Phase 9 deferred list).
  Still deferred.

</deferred>

---

*Phase: 10-doctor-managed-settings*
*Context gathered: 2026-03-25*
