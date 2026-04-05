# Phase 10: Doctor & Managed Settings - Research

**Researched:** 2026-03-25
**Domain:** Rust CLI subcommand addition + filesystem writes + doctor check extension
**Confidence:** HIGH

## Summary

Phase 10 is a small, well-scoped addition to an existing, stable codebase. All patterns
already exist and are proven. The work is entirely in two files: `main.rs` (new `Config`
subcommand + dispatch) and `doctor.rs` (new `check_managed_settings()` function).

`serde_json` is a first-class direct dependency in both `rightclaw` and `rightclaw-cli`
crates — no new dependencies needed for JSON parsing. The `DoctorCheck` / `CheckStatus`
structures support `Warn` already, used for BOOTSTRAP.md and the git check in Phase 9.
The cross-platform assumption (`/etc/claude-code/managed-settings.json` on both Linux
and macOS) should be verified against CC docs before the planner finalizes the path
constant — the CONTEXT.md flags this explicitly.

**Primary recommendation:** Implement `check_managed_settings()` as a private function in
`doctor.rs` returning `Option<DoctorCheck>` (None when file absent, Some when present).
Add `Config { command: ConfigCommands }` to the `Commands` enum in `main.rs` with a sync
`cmd_config_strict_sandbox()` function.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**CLI Shape (TOOL-01)**

- **D-01:** `rightclaw config strict-sandbox` is a nested subcommand. `Commands` gets a `Config`
  variant with `#[command(subcommand)] command: ConfigCommands`. `ConfigCommands` has one variant:
  `StrictSandbox`.

- **D-02:** Cross-platform — write to `/etc/claude-code/managed-settings.json` on both Linux and
  macOS. Planner should verify the exact path CC uses before implementing.

**sudo / Privilege Handling (TOOL-01)**

- **D-03:** Attempt write, surface clear error. No re-exec via sudo. No uid check upfront. If
  `create_dir_all` or `write` fails with permission denied, return a miette error with hint:
  `Run with elevated privileges: sudo rightclaw config strict-sandbox`.

- **D-04:** Content written is exactly `{"allowManagedDomainsOnly": true}`. Overwrite unconditionally
  on every invocation (idempotent).

**Doctor Conflict Check (TOOL-02)**

- **D-05:** `rightclaw doctor` checks `/etc/claude-code/managed-settings.json` on both platforms.

- **D-06:** If file exists and `allowManagedDomainsOnly` is `true`: emit `CheckStatus::Warn` with
  detail `allowManagedDomainsOnly:true — per-agent allowedDomains may be overridden by system policy`.
  Fix hint: `Review /etc/claude-code/managed-settings.json — enabled via: sudo rightclaw config strict-sandbox`.

- **D-07:** If file exists but cannot be parsed or `allowManagedDomainsOnly` is absent or false:
  emit `CheckStatus::Warn` with detail `managed-settings.json found — content may affect agent
  sandbox behavior`. Same severity, non-fatal.

- **D-08:** If file does NOT exist: no check emitted. Silent skip — don't add Pass/Fail to output.

### Claude's Discretion

- Whether to use `check_managed_settings()` private function vs inline in `run_doctor()`. Prefer
  extracted function for testability.
- Exact content of the success print message for `config strict-sandbox`.
- JSON parsing strategy — `serde_json` already a direct dep in `rightclaw` crate. Use it.

### Deferred Ideas (OUT OF SCOPE)

- `rightclaw config unset strict-sandbox` — remove managed-settings.json
- Managed settings allowlist population
- `rightclaw agent init` subcommand
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| TOOL-01 | `rightclaw config strict-sandbox` writes `/etc/claude-code/managed-settings.json` with `allowManagedDomainsOnly: true` (opt-in, requires sudo) | New `Config` + `ConfigCommands` clap subcommands; `std::fs::create_dir_all` + `std::fs::write`; `serde_json::to_string` for output; `miette::miette!` with `help` for permission denied error |
| TOOL-02 | `rightclaw doctor` warns if `/etc/claude-code/managed-settings.json` exists and may conflict | New `check_managed_settings()` in `doctor.rs`; `serde_json::from_str` to parse; `CheckStatus::Warn`; conditionally push to checks vec in `run_doctor()` |
</phase_requirements>

## Standard Stack

### Core (all existing workspace deps — no new dependencies)

| Library | Version | Purpose | Why |
|---------|---------|---------|-----|
| `serde_json` | 1.0 | Parse managed-settings.json content | Direct dep in both `rightclaw` and `rightclaw-cli` crates. Use `serde_json::from_str::<serde_json::Value>` for flexible JSON parsing. |
| `std::fs` | stdlib | Write `/etc/claude-code/managed-settings.json` | `create_dir_all` + `write`. No external crate needed. |
| `miette` | 7.6 (fancy) | User-facing error for permission denied | Established pattern in codebase. `miette::miette!(help = "...", "...")` form already used in `cmd_up`. |
| `clap` | 4.6 | Nested subcommand structure | Derive API, already used. Add enum variant + `#[command(subcommand)]` field. |

**No new dependencies required.** `serde_json = "1.0"` is already in `[workspace.dependencies]`
and explicitly listed in both `crates/rightclaw/Cargo.toml` and `crates/rightclaw-cli/Cargo.toml`.

## Architecture Patterns

### Recommended Project Structure

No new files or modules required. Changes are contained to:

```
crates/rightclaw-cli/src/
└── main.rs          # Add Commands::Config variant, ConfigCommands enum, cmd_config_strict_sandbox()

crates/rightclaw/src/
└── doctor.rs        # Add check_managed_settings() private fn, wire into run_doctor()
```

### Pattern 1: Nested Subcommand (clap derive)

**What:** Add `Config` variant to `Commands` with an inner `ConfigCommands` subcommand enum.
**When to use:** Multiple config sub-verbs anticipated in future (D-01 is explicit about this shape).
**Example:**

```rust
// Source: Locked decision D-01 in 10-CONTEXT.md
#[derive(Subcommand)]
pub enum Commands {
    // ... existing variants ...
    /// Manage RightClaw configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Enable machine-wide domain blocking via managed settings (requires sudo)
    StrictSandbox,
}
```

Dispatch in `match cli.command`:
```rust
Commands::Config { command } => match command {
    ConfigCommands::StrictSandbox => cmd_config_strict_sandbox(),
},
```

### Pattern 2: Attempt-and-Surface-Error (fs write)

**What:** Call `std::fs::create_dir_all` then `std::fs::write`, map errors to miette with hint.
**When to use:** Any privileged filesystem write in a CLI. Established by `cmd_up` for settings.json.
**Example:**

```rust
// Source: Established pattern from cmd_up in main.rs
fn cmd_config_strict_sandbox() -> miette::Result<()> {
    const MANAGED_SETTINGS_PATH: &str = "/etc/claude-code/managed-settings.json";
    let path = std::path::Path::new(MANAGED_SETTINGS_PATH);

    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| {
        miette::miette!(
            help = "Run with elevated privileges: sudo rightclaw config strict-sandbox",
            "Permission denied writing {}: {e:#}", path.display()
        )
    })?;

    std::fs::write(path, r#"{"allowManagedDomainsOnly": true}"#).map_err(|e| {
        miette::miette!(
            help = "Run with elevated privileges: sudo rightclaw config strict-sandbox",
            "Permission denied writing {}: {e:#}", path.display()
        )
    })?;

    println!(
        "Wrote {} — machine-wide domain blocking enabled.",
        MANAGED_SETTINGS_PATH
    );
    Ok(())
}
```

### Pattern 3: Optional Doctor Check (returns Option<DoctorCheck>)

**What:** Function returns `None` when file absent (D-08 — no check emitted), `Some(DoctorCheck)`
otherwise. Caller uses `if let Some(check) = check_managed_settings()` before pushing.
**When to use:** Checks that should be silent when the feature is not in use.
**Example:**

```rust
// Source: D-05 through D-08 in 10-CONTEXT.md; DoctorCheck pattern from doctor.rs
fn check_managed_settings() -> Option<DoctorCheck> {
    const MANAGED_SETTINGS_PATH: &str = "/etc/claude-code/managed-settings.json";
    let content = match std::fs::read_to_string(MANAGED_SETTINGS_PATH) {
        Ok(c) => c,
        Err(_) => return None, // File absent — D-08: silent skip
    };

    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&content);
    let (detail, fix) = match parsed {
        Ok(v) if v.get("allowManagedDomainsOnly").and_then(|v| v.as_bool()) == Some(true) => (
            "allowManagedDomainsOnly:true — per-agent allowedDomains may be overridden by system policy".to_string(),
            Some("Review /etc/claude-code/managed-settings.json — enabled via: sudo rightclaw config strict-sandbox".to_string()),
        ),
        _ => (
            "managed-settings.json found — content may affect agent sandbox behavior".to_string(),
            Some("Review /etc/claude-code/managed-settings.json".to_string()),
        ),
    };

    Some(DoctorCheck {
        name: "managed-settings".to_string(),
        status: CheckStatus::Warn,
        detail,
        fix,
    })
}
```

Wire into `run_doctor()`:
```rust
// After existing checks.extend(check_agent_structure(home)):
if let Some(check) = check_managed_settings() {
    checks.push(check);
}
```

### Anti-Patterns to Avoid

- **Re-execing with sudo:** D-03 is explicit — do NOT use `std::process::Command::new("sudo")`.
  Simply attempt the write and return a helpful error.
- **Checking uid upfront:** `std::os::unix::fs::MetadataExt::uid() == 0` check is fragile.
  Attempt-and-fail is cleaner.
- **Adding the check unconditionally:** D-08 requires the check to be absent from output when
  the file doesn't exist. Do not push a `Pass` check for file-not-found.
- **Using `create_dir_all` on the file path instead of its parent:** Classic off-by-one.
  Use `path.parent().unwrap()` for the directory.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| JSON parsing | String `contains()` check | `serde_json::from_str::<serde_json::Value>` | Handles whitespace variations, key ordering, quoted vs unquoted values; `serde_json` already a dep |
| Sudo privilege check | `getuid() == 0` | Attempt write, map error | More robust: works under `sudo`, `doas`, capability bits. Pattern already established in codebase. |

## Common Pitfalls

### Pitfall 1: Wrong parent for create_dir_all

**What goes wrong:** `create_dir_all("/etc/claude-code/managed-settings.json")` creates a
**directory** named `managed-settings.json` inside `/etc/claude-code/`, then `write()` fails
because it's a directory.
**Why it happens:** Forgetting `.parent()`.
**How to avoid:** `std::fs::create_dir_all(path.parent().unwrap())` — always call `.parent()` before `create_dir_all` on a file path.
**Warning signs:** `write()` returns `IsADirectory` error.

### Pitfall 2: Both error arms are identical — extract to avoid duplication

**What goes wrong:** `create_dir_all` and `write` have the same error message and hint — copy-paste
drift.
**Why it happens:** Two separate `map_err` calls with identical text.
**How to avoid:** Extract a helper or write dir creation + file write in one closure. Or accept the
duplication given the function is only ~15 lines.

### Pitfall 3: Doctor check count changes in tests

**What goes wrong:** Existing tests that assert exact check counts (e.g., `checks.len() == N`)
break when the managed settings file happens to exist on the test host.
**Why it happens:** `/etc/claude-code/managed-settings.json` may or may not exist on CI/dev machines.
**How to avoid:** New tests for `check_managed_settings` should use a path parameter or test the
function in isolation (not through `run_doctor`). Existing `run_doctor` tests don't assert exact
count — they use `.find()` / `.filter()` — so this is not a problem in the current test suite.
Verify before adding count-based assertions.

### Pitfall 4: macOS /etc/claude-code path assumption unverified

**What goes wrong:** `/etc/claude-code/managed-settings.json` is the assumed path, but Claude Code
may use a different path on macOS (e.g., `/Library/Application Support/Claude/managed-settings.json`
or `/etc/claude-code/` only on Linux).
**Why it happens:** CONTEXT.md D-02 notes "planner should verify" — it's an assumption, not confirmed.
**How to avoid:** Before hardcoding the path, verify against CC source or docs. If the path differs
by platform, use `cfg!(target_os = "macos")` conditional. Use a named constant regardless so the
path is a single edit point.

## Code Examples

### check_managed_settings signature

```rust
// Source: D-05 through D-08 pattern; returns Option<DoctorCheck>
fn check_managed_settings() -> Option<DoctorCheck>
```

This signature is optimal for D-08 (no check when file absent). Returning `Vec<DoctorCheck>` would
also work but adds unnecessary allocation.

### JSON value access pattern

```rust
// Source: serde_json docs; confirmed serde_json = "1.0" in workspace Cargo.toml
let val: serde_json::Value = serde_json::from_str(&content)?;
let flag = val.get("allowManagedDomainsOnly").and_then(|v| v.as_bool());
// flag == Some(true)  → D-06 warn (strict mode on)
// flag == Some(false) → D-07 warn (file exists, mode off)
// flag == None        → D-07 warn (key absent)
// from_str Err        → D-07 warn (invalid JSON)
```

### Managed settings path constant

```rust
// Define once, reference everywhere — makes cross-platform fix a single edit
const MANAGED_SETTINGS_PATH: &str = "/etc/claude-code/managed-settings.json";
```

## State of the Art

No state-of-the-art churn relevant to this phase. All patterns (clap nested subcommands,
serde_json value access, miette error with help hint) are stable and unchanged.

## Open Questions

1. **Is `/etc/claude-code/managed-settings.json` the correct path on macOS?**
   - What we know: D-02 assumes same path on both platforms. `/etc/` exists on macOS.
   - What's unclear: CC may have a macOS-specific managed settings location (e.g., under
     `/Library/Managed Preferences/` which is the standard macOS MDM location, or
     `/Library/Application Support/claude-code/`).
   - Recommendation: Planner should do a targeted web search or grep CC source for
     `managed-settings` / `managedSettings` before finalizing the constant. If platforms
     differ, use a private `fn managed_settings_path() -> &'static str` with `cfg!` branch.

## Sources

### Primary (HIGH confidence)

- `crates/rightclaw/Cargo.toml` — confirmed `serde_json = { workspace = true }` direct dep
- `crates/rightclaw-cli/Cargo.toml` — confirmed `serde_json = { workspace = true }` direct dep
- `Cargo.toml` (workspace) — `serde_json = "1.0"` in `[workspace.dependencies]`
- `crates/rightclaw/src/doctor.rs` — `DoctorCheck`, `CheckStatus::Warn`, `run_doctor()` pattern
- `crates/rightclaw-cli/src/main.rs` — `Commands` enum shape, `cmd_doctor()`, `cmd_up()` error pattern
- `crates/rightclaw/src/runtime/deps.rs` — `verify_dependencies()` non-fatal warn pattern
- `.planning/phases/10-doctor-managed-settings/10-CONTEXT.md` — all locked decisions

### Secondary (MEDIUM confidence)

- `.planning/REQUIREMENTS.md` — TOOL-01, TOOL-02 requirements text

### Tertiary (LOW confidence)

- macOS managed settings path — not verified against CC source or docs. Flagged in Open Questions.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all deps confirmed by direct file reads
- Architecture: HIGH — all patterns exist in codebase, no new patterns introduced
- Pitfalls: HIGH — derived from direct code inspection and locked decisions
- macOS path assumption: LOW — explicitly flagged, needs verification

**Research date:** 2026-03-25
**Valid until:** 2026-04-25 (stable domain — clap/serde_json/miette APIs don't churn)
