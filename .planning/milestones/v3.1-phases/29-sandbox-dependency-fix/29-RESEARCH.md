# Phase 29: Sandbox Dependency Fix - Research

**Researched:** 2026-04-02
**Domain:** CC native sandbox dependency injection in nix/devenv environments
**Confidence:** HIGH — root cause confirmed by CC source inspection, live Bun.which test, and CC issue tracker

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** `generate_settings()` gains a new parameter `rg_path: Option<PathBuf>`. Caller (`cmd_up`) resolves `which::which("rg")` once and passes to each agent's settings generation. `settings.rs` stays pure (no IO).
- **D-02:** When rg is not found in PATH, `cmd_up` logs `tracing::warn` and passes `None`. `settings.json` gets no `sandbox.ripgrep.command` field. Agent will fail at CC level because `failIfUnavailable: true` prevents silent degradation.
- **D-03:** `sandbox.ripgrep.command` is set to the absolute path returned by `which::which("rg")` — resolved at `rightclaw up` time, not a relative or store path.
- **D-04:** `sandbox.failIfUnavailable: true` is unconditionally present in every generated `settings.json`, regardless of `--no-sandbox` flag. Zero branching in codegen.
- **D-05:** `USE_BUILTIN_RIPGREP` env var changed from `"1"` to `"0"` in both `worker.rs` and `cron.rs`.
- **D-06:** Comment updated to explain the counterintuitive naming: `"0"` = use system rg, `"1"` = use CC bundled rg.
- **D-07:** `pkgs.ripgrep` added to the packages list in `devenv.nix` (unconditional, not Linux-only).
- **D-08:** All four fix sites (settings.rs, worker.rs, cron.rs, devenv.nix) land in a single atomic commit.

### Claude's Discretion
None — all decisions are locked.

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SBOX-01 | `rightclaw up` injects `sandbox.ripgrep.command` with resolved system rg path into per-agent settings.json | D-01/D-03 + `generate_settings()` signature change; `sandbox.ripgrep` is an officially documented settings.json field in CC Zod schema |
| SBOX-02 | `USE_BUILTIN_RIPGREP` env var corrected from `"1"` to `"0"` in worker.rs and cron.rs CC subprocess invocations | D-05/D-06 + CC source A_() semantics confirmed: `A_("0") = true` → skip vendored binary → use PATH rg |
| SBOX-03 | `sandbox.failIfUnavailable: true` added to generated settings.json — sandbox failures become fatal instead of silent degradation | D-04 + CC docs confirm this field makes sandbox failure hard |
| SBOX-04 | devenv.nix includes `pkgs.ripgrep` in packages list for development environment | D-07 + standard nix package; ensures rg in PATH for all sessions |
</phase_requirements>

## Summary

The CC sandbox silently disables in nix environments because the vendored ripgrep binary at `/nix/store/<hash>-claude-code-bun-2.1.89/lib/node_modules/@anthropic-ai/claude-code/vendor/ripgrep/x64-linux/rg` has permissions `r--r--r--` (444) — no execute bit. `Bun.which()` checks executability and returns `null`, causing CC's `checkDependencies()` to push `"ripgrep not found"` to the errors array and silently degrade. The rightclaw codebase already attempted to fix this via `USE_BUILTIN_RIPGREP=1` in worker.rs and cron.rs, but the value is inverted: `"1"` keeps the bundled binary (default), `"0"` switches to system rg. This is the primary bug.

Four changes land atomically to close the sandbox gap. The order matters: `failIfUnavailable: true` must land simultaneously with `sandbox.ripgrep.command` injection and `USE_BUILTIN_RIPGREP=0` — enabling failIfUnavailable before the rg path fix causes a CC restart loop since sandbox would fail immediately. The atomic commit requirement in D-08 exists specifically to prevent this broken intermediate state.

The `generate_settings()` signature change (adding `rg_path: Option<PathBuf>`) requires updating both callers: `cmd_up` in `main.rs` (line 380) and `cmd_init` in `init.rs` (line 97). The `cmd_up` caller resolves `which::which("rg")` once before the per-agent loop. The `cmd_init` caller can pass `None` (init generates a template, not a live-environment file) or can also resolve rg — both approaches work given D-02's handling of `None`.

## Standard Stack

### Core (No New Dependencies)

All required tooling is already in the workspace:

| What | Where Available | Used For |
|------|----------------|----------|
| `which::which("rg")` | `which` crate — already in workspace deps | Resolve absolute rg path at launch time |
| `cmd.env("USE_BUILTIN_RIPGREP", "0")` | `tokio::process::Command` — already used | Override CC binary selection env var |
| `serde_json::json!` macro | `serde_json` — already in workspace | Add ripgrep and failIfUnavailable fields to settings JSON |
| `pkgs.ripgrep` | devenv/nix — standard package | Ensure rg in devenv PATH |

**No new Rust crates required.** This is purely a value-change + field-addition phase.

## Architecture Patterns

### Fix Site Map

```
Four atomic changes, one commit:

1. crates/rightclaw/src/codegen/settings.rs
   - generate_settings() signature: add rg_path: Option<PathBuf>
   - sandbox JSON: add failIfUnavailable: true (unconditional)
   - sandbox JSON: add ripgrep.command field when rg_path is Some

2. crates/rightclaw-cli/src/main.rs  (cmd_up, line 380)
   - resolve which::which("rg") once before per-agent loop
   - pass rg_path to generate_settings() per agent
   - tracing::warn! when rg not found

3. crates/rightclaw/src/init.rs  (cmd_init, line 97)
   - pass rg_path: None (or also resolve) to updated generate_settings()

4. crates/bot/src/telegram/worker.rs  (line 399)
   - change "1" → "0" for USE_BUILTIN_RIPGREP

5. crates/bot/src/cron.rs  (line 227)
   - change "1" → "0" for USE_BUILTIN_RIPGREP

6. devenv.nix
   - add pkgs.ripgrep to packages list (unconditional, not Linux-only)

7. crates/rightclaw/src/codegen/settings_tests.rs
   - new tests: failIfUnavailable present, ripgrep.command injected when Some, omitted when None
```

### Pattern 1: settings.rs — New Signature and JSON Fields

**What:** Add `rg_path: Option<PathBuf>` parameter. Inject `failIfUnavailable` unconditionally. Inject `ripgrep.command` conditionally on `rg_path`.

**Caller contract:** Caller owns IO (`which::which`). `settings.rs` stays pure — receives resolved path, emits JSON. No IO inside settings.rs.

```rust
// Source: CONTEXT.md D-01, D-03, D-04
pub fn generate_settings(
    agent: &AgentDef,
    no_sandbox: bool,
    host_home: &Path,
    rg_path: Option<PathBuf>,   // NEW: resolved by caller via which::which("rg")
) -> miette::Result<serde_json::Value> {
    // ... existing logic unchanged ...

    let mut settings = serde_json::json!({
        "skipDangerousModePermissionPrompt": true,
        "spinnerTipsEnabled": false,
        "prefersReducedMotion": true,
        "autoMemoryEnabled": false,
        "sandbox": {
            "enabled": !no_sandbox,
            "failIfUnavailable": true,   // NEW: D-04 — unconditional, fatal on sandbox failure
            "autoAllowBashIfSandboxed": true,
            "allowUnsandboxedCommands": false,
            "filesystem": { ... },
            "network": { ... },
        }
    });

    // Inject ripgrep path when available (D-01, D-03).
    // When None: omit field; CC fails at sandbox check because failIfUnavailable: true.
    if let Some(ref rg) = rg_path {
        settings["sandbox"]["ripgrep"] = serde_json::json!({
            "command": rg.display().to_string(),
            "args": []
        });
    }

    Ok(settings)
}
```

### Pattern 2: cmd_up — rg Resolution Before Per-Agent Loop

**What:** Resolve `which::which("rg")` once before the per-agent loop. Pass to each `generate_settings()` call.

```rust
// Source: CONTEXT.md D-01, D-02; main.rs ~line 368
// Resolve system rg path once — passed to each agent's settings generation (D-01).
// When absent: warn, pass None; agents will fail at CC sandbox check (failIfUnavailable: true).
let rg_path = match which::which("rg") {
    Ok(p) => {
        tracing::debug!(rg = %p.display(), "system ripgrep found");
        Some(p)
    }
    Err(_) => {
        tracing::warn!(
            "ripgrep (rg) not found in PATH — CC sandbox will fail to initialize. \
             Install ripgrep: nix profile install nixpkgs#ripgrep / apt install ripgrep / brew install ripgrep"
        );
        None
    }
};

// In the per-agent loop:
let settings = rightclaw::codegen::generate_settings(agent, no_sandbox, &host_home, rg_path.clone())?;
```

### Pattern 3: USE_BUILTIN_RIPGREP Correction

**What:** Single-character change in two files. Add explanatory comment per D-06.

```rust
// Source: CONTEXT.md D-05, D-06
// CC internal env var — "0" = skip bundled rg, use system rg from PATH (A_("0") = true).
// "1" = use CC's vendored rg (default; broken in nix — vendor binary lacks execute bit).
// UNDOCUMENTED: verify this env var still works after each CC version update.
// See: https://github.com/anthropics/claude-code/issues/6415
cmd.env("USE_BUILTIN_RIPGREP", "0");
```

### Pattern 4: devenv.nix

**What:** Add `pkgs.ripgrep` unconditionally to the packages list.

```nix
# Source: CONTEXT.md D-07
packages = [
    pkgs.git
    pkgs.process-compose
    pkgs.socat
    pkgs.ripgrep          # Required: CC sandbox rg check; devenv rg must be in agent launch PATH
] ++ lib.optionals pkgs.stdenv.isLinux [
    pkgs.bubblewrap
];
```

### Anti-Patterns to Avoid

- **`USE_BUILTIN_RIPGREP=1`:** Wrong direction — keeps vendored binary (the broken one). Must be `"0"`.
- **Resolving rg inside `settings.rs`:** Violates D-01 — settings.rs must stay pure, no IO.
- **Hardcoding nix store paths:** Rots on every CC update; `which::which` is dynamic.
- **`chmod +x` on nix store path:** Nix store is read-only (`EROFS`). Never works.
- **Conditional `failIfUnavailable`:** D-04 is unconditional. Branching on `no_sandbox` adds complexity with no benefit — CC ignores the field when sandbox is disabled.
- **Enabling `failIfUnavailable` in a separate commit from rg path fix:** Causes CC restart loop. D-08 requires atomic landing.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Find rg binary | PATH-walking loop | `which::which("rg")` | Already in workspace; handles PATH, permissions, cross-platform |
| JSON settings fields | String concatenation | `serde_json::json!` macro | Already used throughout settings.rs; type-safe, correct escaping |

## Common Pitfalls

### Pitfall 1: USE_BUILTIN_RIPGREP Polarity Inversion
**What goes wrong:** Setting `"1"` thinking it enables system rg. It does the opposite — `"1"` enables CC's builtin (vendored) rg.
**Root cause:** `A_(value)` in CC's cli.js means "is USE_BUILTIN_RIPGREP disabled?" — inverted semantics. `A_("0") = true` → disabled → use system rg. `A_("1") = false` → enabled → use vendored rg.
**How to avoid:** Always `"0"` to use system rg. Both bug sites (worker.rs:399, cron.rs:227) have `"1"` — both need changing.
**Verification:** `grep -r USE_BUILTIN_RIPGREP crates/` must show only `"0"`.

### Pitfall 2: failIfUnavailable Before rg Fix = Restart Loop
**What goes wrong:** If `failIfUnavailable: true` lands before `sandbox.ripgrep.command` + `USE_BUILTIN_RIPGREP=0`, CC fails its sandbox check on every launch. process-compose restarts agents in a tight loop.
**Root cause:** failIfUnavailable makes CC exit non-zero when sandbox can't initialize. Without the rg fix, the sandbox still can't initialize.
**How to avoid:** D-08 — all four changes in one atomic commit. No intermediate state where failIfUnavailable is true but rg is still broken.

### Pitfall 3: init.rs Caller Not Updated
**What goes wrong:** `generate_settings()` signature change breaks `cmd_init` caller in `init.rs` (line 97). Build fails. Easy to miss since `cmd_up` in `main.rs` is the primary focus.
**Root cause:** Two callers: `main.rs:380` (cmd_up) and `init.rs:97` (cmd_init). Both must be updated.
**How to avoid:** After changing signature, compile immediately — Rust will point at both broken call sites.

### Pitfall 4: test Callers Not Updated
**What goes wrong:** `settings_tests.rs` has 14 calls to `generate_settings()` with 3 arguments. After signature change to 4 arguments, all tests fail to compile.
**Root cause:** Test file uses the old 3-arg signature throughout.
**How to avoid:** After signature change, update all test calls. Most can pass `None` for rg_path. Add new dedicated tests for Some(rg_path) behavior.

### Pitfall 5: Agent Launch PATH vs. Developer PATH Divergence
**What goes wrong:** `which::which("rg")` in cmd_up resolves rg from the operator's shell PATH (devenv active). Agent processes launched by process-compose inherit a potentially different PATH.
**Root cause:** process-compose inherits `rightclaw up`'s environment. If run from devenv shell, devenv PATH propagates. If run from a bare terminal, only system PATH.
**How to avoid:** The rg absolute path injected into `sandbox.ripgrep.command` is the resolved path at launch time — it's an absolute path, so the agent doesn't need rg in its own PATH for the settings.json check. `USE_BUILTIN_RIPGREP=0` still requires rg to be findable at CC subprocess spawn time, but since the absolute path is already in `sandbox.ripgrep.command`, that field overrides PATH lookup.
**Note:** This is a Phase 30 (doctor) concern more than a Phase 29 concern. Phase 29 fix is correct as specified.

## Code Examples

### Current Bug State (both files)

`crates/bot/src/telegram/worker.rs` line 398-399:
```rust
// Use system rg instead of CC's bundled vendor binary (nix store rg lacks execute bit).
cmd.env("USE_BUILTIN_RIPGREP", "1");   // BUG: "1" forces bundled binary, not system rg
```

`crates/bot/src/cron.rs` line 226-227:
```rust
// Use system rg instead of CC's bundled vendor binary (nix store rg lacks execute bit).
cmd.env("USE_BUILTIN_RIPGREP", "1");   // BUG: same inversion
```

### Fixed State

```rust
// CC internal env var — "0" = skip bundled rg, use system rg from PATH.
// Counterintuitive: A_("0")=true means "builtin disabled" → falls through to system rg.
// Confirmed working: https://github.com/anthropics/claude-code/issues/6415
// UNDOCUMENTED: re-verify after CC version bumps.
cmd.env("USE_BUILTIN_RIPGREP", "0");
```

### settings.json Output (with rg injected)

```json
{
  "skipDangerousModePermissionPrompt": true,
  "spinnerTipsEnabled": false,
  "prefersReducedMotion": true,
  "autoMemoryEnabled": false,
  "sandbox": {
    "enabled": true,
    "failIfUnavailable": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    "ripgrep": {
      "command": "/home/wb/.nix-profile/bin/rg",
      "args": []
    },
    "filesystem": { ... },
    "network": { ... }
  }
}
```

### New Tests to Add (settings_tests.rs)

```rust
#[test]
fn includes_fail_if_unavailable_unconditionally() {
    let agent = make_test_agent("test-agent", None);
    // With sandbox enabled
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();
    assert_eq!(settings["sandbox"]["failIfUnavailable"], true);
    // With sandbox disabled (--no-sandbox) — still present, CC ignores it
    let settings = generate_settings(&agent, true, Path::new("/home/user"), None).unwrap();
    assert_eq!(settings["sandbox"]["failIfUnavailable"], true);
}

#[test]
fn injects_ripgrep_command_when_path_provided() {
    let agent = make_test_agent("test-agent", None);
    let rg = Some(PathBuf::from("/usr/bin/rg"));
    let settings = generate_settings(&agent, false, Path::new("/home/user"), rg).unwrap();
    assert_eq!(settings["sandbox"]["ripgrep"]["command"], "/usr/bin/rg");
    assert_eq!(settings["sandbox"]["ripgrep"]["args"], serde_json::json!([]));
}

#[test]
fn omits_ripgrep_when_path_not_provided() {
    let agent = make_test_agent("test-agent", None);
    let settings = generate_settings(&agent, false, Path::new("/home/user"), None).unwrap();
    assert!(
        settings["sandbox"].get("ripgrep").is_none(),
        "ripgrep field must be absent when rg_path is None"
    );
}
```

## Environment Availability

Step 2.6: SKIPPED (no new external tool dependencies — `which` crate and `pkgs.ripgrep` already in ecosystem; the fix adds them to the codegen output, not to rightclaw's own runtime).

## Sources

### Primary (HIGH confidence)
- CC cli.js source G34 (`checkDependencies`) — confirmed Bun.which() executability check; live test: `Bun.which("/nix/.../vendor/ripgrep/x64-linux/rg") → null`
- CC cli.js source Bv8 (ripgrep config getter) — `USE_BUILTIN_RIPGREP` value semantics: `A_("0") = true → skip vendored → system rg`
- `.planning/research/STACK.md` — root cause analysis, fix approaches ranked
- `.planning/research/PITFALLS.md` — 7 pitfalls including restart loop trap, nix immutability
- `.planning/research/ARCHITECTURE.md` — fix site locations, `sandbox.ripgrep` Zod schema
- CONTEXT.md — all decisions locked (D-01 through D-08)
- Live file check: `/nix/.../vendor/ripgrep/x64-linux/rg` mode `r--r--r--` (444) — confirmed

### Secondary (MEDIUM confidence)
- [CC Issue #6415](https://github.com/anthropics/claude-code/issues/6415) — USE_BUILTIN_RIPGREP semantics, fixed August 2025
- [CC Issue #42068](https://github.com/anthropics/claude-code/issues/42068) — vendored rg permissions bug, April 2026
- [sadjow/claude-code-nix](https://github.com/sadjow/claude-code-nix) — `USE_BUILTIN_RIPGREP=0` in nix package

### Tertiary (LOW confidence)
None — all critical claims verified against primary sources.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new deps, existing patterns only
- Architecture: HIGH — all fix sites confirmed via direct source inspection
- Pitfalls: HIGH — confirmed against CC source, live tests, and issue tracker
- Atomicity requirement: HIGH — restart loop trap documented and understood

**Research date:** 2026-04-02
**Valid until:** 2026-05-02 (stable; USE_BUILTIN_RIPGREP could change with CC updates but that's Phase 30+ concern)
