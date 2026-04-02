# Architecture Research

**Domain:** CC sandbox fix and end-to-end verification for nix environments
**Researched:** 2026-04-02
**Confidence:** HIGH (direct codebase inspection + CC cli.js source analysis)

## Standard Architecture

### System Overview

```
rightclaw up
    │
    ├── runtime/deps.rs → verify_dependencies()
    │     checks: process-compose, claude/claude-bun
    │     NEW: warn if rg absent AND CC vendor rg not executable
    │
    ├── for each agent:
    │     codegen/settings.rs → generate_settings()
    │         writes agent/.claude/settings.json
    │         existing: sandbox.enabled, filesystem, network
    │         NEW: sandbox.ripgrep.command → absolute system rg path
    │
    └── codegen/process_compose.rs → generate_process_compose()
          no changes — bot-only template already complete

doctor.rs → run_doctor()
    existing: bwrap, socat, bwrap-sandbox checks
    NEW: check_sandbox_ripgrep() DoctorCheck

process-compose (bot processes)
    └── crates/bot/ → rightclaw bot --agent <name>
          ├── telegram/worker.rs → invoke_cc()
          │     USE_BUILTIN_RIPGREP=1  (already set — belt-and-suspenders)
          │     HOME=agent_dir
          │     spawns: claude-bun -p --dangerously-skip-permissions ...
          │         CC reads agent/.claude/settings.json
          │         sandbox.ripgrep.command tells CC which rg to use
          │         CC's bwrap wraps the subprocess
          │
          └── cron.rs → execute_job()
                USE_BUILTIN_RIPGREP=1  (already set — belt-and-suspenders)
                HOME=agent_dir
                spawns: claude-bun -p --dangerously-skip-permissions ...
                    same settings.json read path
```

### Root Cause: CC Vendor rg Has No Execute Bit in Nix Store

CC resolves its rg binary in this order (from `cli.js` `$aA` lazy value):

1. `USE_BUILTIN_RIPGREP` set → find system `rg` via `findActualExecutable("rg", [])`
2. bun embedded mode → use `process.execPath --ripgrep`
3. default fallback → `vendor/ripgrep/<arch>-<platform>/rg` (absolute nix store path)

The vendored rg in the current CC nix package has mode `.r--r--r--` — **no execute bit**:
```
.r--r--r-- 5.6M root  1 Jan  1970
  /nix/store/hhydcyh1z6h2fyznlqagb1p66l07yhp6-claude-code-bun-2.1.89/
  lib/node_modules/@anthropic-ai/claude-code/vendor/ripgrep/x64-linux/rg
```
Confirmed: attempting to execute it gives `permission denied`.

`USE_BUILTIN_RIPGREP=1` is already set in `worker.rs` and `cron.rs`. This tells CC
to prefer system `rg` via PATH lookup. However, CC's `checkDependencies` function
(`zT8` in cli.js) runs the resolved rg binary to verify it works. If that test fails
(e.g., system rg not in the subprocess PATH, or resolved to vendor path which has
no execute bit), CC sets `SandboxUnavailableReason` and **silently degrades** to
unsandboxed mode — no error exit, no diagnostic output visible to rightclaw.

**`failIfUnavailable`** (a `settings.json` flag) defaults to `false`, so the silent
degradation happens with no observable failure. Agents keep running, but unsandboxed.

### The Fix: sandbox.ripgrep in settings.json

CC's `settings.json` schema supports an explicit `sandbox.ripgrep` override:
```json
"sandbox": {
  "ripgrep": {
    "command": "/nix/store/.../bin/rg",
    "args": []
  }
}
```

This field is documented in the CC settings schema (confirmed in `cli.js` Zod schema):
```
sandbox.ripgrep: {command: string, args?: string[]}
  "Custom ripgrep configuration for bundled ripgrep"
```

When `sandbox.ripgrep.command` is set, CC's `oO6()` function returns it directly
instead of running `$aA()` (the path that falls through to the vendor binary).
The `checkDependencies` call uses this path for its executable test — if it points
to a working system `rg`, the sandbox check passes.

**Implementation:** In `generate_settings()`, resolve `which::which("rg")` to get
the absolute system rg path, then inject it into the sandbox JSON. This is done
at `rightclaw up` time so the path is always current (no hardcoded nix store hash).

## Component Responsibilities

| Component | Responsibility | Change |
|-----------|----------------|--------|
| `codegen/settings.rs` | Generate `settings.json` per agent | **MODIFY** |
| `codegen/settings_tests.rs` | Unit tests for settings generation | **MODIFY** |
| `runtime/deps.rs` | Pre-flight dependency check in `rightclaw up` | **MODIFY** |
| `doctor.rs` | Comprehensive diagnostics | **MODIFY** |
| `telegram/worker.rs` | Spawn CC for Telegram messages | unchanged (already has `USE_BUILTIN_RIPGREP=1`) |
| `cron.rs` | Spawn CC for cron jobs | unchanged (already has `USE_BUILTIN_RIPGREP=1`) |
| `codegen/process_compose.rs` | Bot process PC config | unchanged |
| `agent/types.rs` | `SandboxOverrides` struct | unchanged |
| `templates/process-compose.yaml.j2` | Bot process template | unchanged |

## Recommended Project Structure

No new files. All changes in existing files:

```
crates/rightclaw/src/
├── codegen/
│   ├── settings.rs          ← MODIFY: add sandbox.ripgrep to JSON output
│   └── settings_tests.rs    ← MODIFY: add tests for ripgrep field presence
├── runtime/
│   └── deps.rs              ← MODIFY: add rg warning (non-fatal)
└── doctor.rs                ← MODIFY: add check_sandbox_ripgrep() DoctorCheck
```

### Structure Rationale

- **settings.rs** is the natural home for the rg path injection — it already builds
  the complete `settings.json` JSON value. No new abstraction needed.
- **deps.rs** is the `verify_dependencies()` fast-fail path. Adding an rg warning
  here surfaces the issue before any process starts, not after agents fail silently.
- **doctor.rs** provides post-hoc diagnostics via `rightclaw doctor`. Adding a
  `check_sandbox_ripgrep()` here lets users debug without running `rightclaw up`.

## Architectural Patterns

### Pattern 1: Inject sandbox.ripgrep at settings.json Generation Time

**What:** Resolve `which::which("rg")` in `generate_settings()` and embed the
absolute path into the `sandbox.ripgrep` field.

**When to use:** Always — even when `USE_BUILTIN_RIPGREP=1` is set, the
`checkDependencies` path in CC needs a working rg to enable the sandbox.

**Trade-offs:**
- Pro: CC uses this path for both `checkDependencies` and runtime operations
- Pro: Absolute path survives HOME override (no relative PATH lookup needed)
- Pro: Officially supported settings.json field — not a hack
- Pro: Settings regenerated on every `rightclaw up` — always current path
- Con: If system rg absent: field omitted, sandbox degrades, must warn caller

**Example:**
```rust
// In generate_settings(), after building the sandbox block:
if let Ok(rg_path) = which::which("rg") {
    settings["sandbox"]["ripgrep"] = serde_json::json!({
        "command": rg_path.display().to_string(),
        "args": []
    });
    // If rg not found: omit field; deps.rs/doctor.rs warn separately
}
```

### Pattern 2: Non-Fatal rg Warning in verify_dependencies()

**What:** After the claude binary check in `verify_dependencies()`, add a
non-fatal check: if system rg is absent AND the CC vendor rg is not executable,
emit a `tracing::warn!` — not an error. Agent launch proceeds; sandbox just
won't work.

**When to use:** In `cmd_up` fast-fail path — catches the issue before launch
so the operator sees it in the terminal, not in CC's silent degradation.

**Trade-offs:**
- Pro: Consistent with existing git warning pattern (warn-only, never return Err)
- Pro: Actionable: user sees the warning before agents start
- Con: Soft warning is easy to miss — doctor.rs provides the explicit check

**Example:**
```rust
// Non-fatal: sandbox degrades silently without rg; warn the operator.
let rg_available = which::which("rg").is_ok();
if !rg_available {
    // Check CC vendor path as fallback detection
    let vendor_rg_executable = resolve_cc_vendor_rg()
        .map(|p| std::fs::metadata(&p)
            .map(|m| { use std::os::unix::fs::PermissionsExt; m.permissions().mode() & 0o111 != 0 })
            .unwrap_or(false))
        .unwrap_or(false);
    if !vendor_rg_executable {
        tracing::warn!(
            "rg not found in PATH and CC vendor ripgrep lacks execute permission — \
             CC sandbox will be silently disabled. \
             Install ripgrep: sudo apt install ripgrep (or pacman/dnf)"
        );
    }
}
```

### Pattern 3: DoctorCheck for Sandbox ripgrep

**What:** Add `check_sandbox_ripgrep()` in `run_doctor()` — a `DoctorCheck`
with `Warn` severity when rg is unavailable. Placed in the Linux-only block
after `check_bwrap_sandbox()`.

**When to use:** `rightclaw doctor` diagnostic path. Also acts as documentation
for operators who need to understand sandbox requirements.

**Trade-offs:**
- Pro: Follows exact existing DoctorCheck pattern (name, status, detail, fix)
- Pro: `Warn` severity matches the fact that agents still run (just unsandboxed)
- Con: Requires resolving CC binary path, which is slightly involved

**Check shape:**
```
  sandbox-ripgrep      warn   rg not found in PATH and CC vendor rg not executable
    fix: Install ripgrep: sudo apt install ripgrep (or dnf/pacman)
```

## Data Flow

### cmd_up — Sandbox Fix Flow

```
cmd_up()
    │
    ├── verify_dependencies()  [deps.rs]
    │     ├── which("process-compose") → ok/fail
    │     ├── which("claude"/"claude-bun") → ok/fail
    │     └── NEW: if which("rg").is_err() && vendor_rg_not_executable
    │               → tracing::warn! (non-fatal, continue)
    │
    ├── for each agent:
    │     generate_settings(agent, no_sandbox, &host_home)  [settings.rs]
    │         existing: sandbox.enabled, filesystem, network, etc.
    │         NEW: if which("rg").is_ok()
    │               → settings["sandbox"]["ripgrep"] = {command: rg_path, args: []}
    │
    └── generate_process_compose()  [unchanged]
          bot processes use USE_BUILTIN_RIPGREP=1 (belt-and-suspenders)
          but settings.json ripgrep field is the primary fix
```

### CC Subprocess — Sandbox Enable Flow (after fix)

```
claude-bun -p --dangerously-skip-permissions ...
    reads agent/.claude/settings.json
    sandbox.enabled = true
    sandbox.ripgrep.command = "/nix/store/.../bin/rg"  ← NEW
         │
         oO6() returns {command: "/nix/store/.../bin/rg", args: []}
         zT8() checkDependencies:
             bwrap → found ✓
             socat → found ✓
             rg (runs command) → /nix/store/.../bin/rg works ✓
         SandboxUnavailableReason() → undefined (no reason = sandbox enabled)
         bwrap wraps bash subprocess ✓
```

### doctor — New Check Flow

```
run_doctor(home)
    ├── check_binary("bwrap")         ← existing
    ├── check_binary("socat")         ← existing
    ├── check_bwrap_sandbox()         ← existing
    └── NEW: check_sandbox_ripgrep()
              step 1: which("rg") → if found, Pass
              step 2: else: resolve CC binary → find vendor rg path
              step 3: check vendor rg mode & 0o111 != 0 → if ok, Pass
              step 4: else Warn with fix hint
```

## Integration Points

### External Service: CC settings.json sandbox.ripgrep field

| Field | Schema | Notes |
|-------|--------|-------|
| `sandbox.ripgrep.command` | `string` | Absolute path to rg binary |
| `sandbox.ripgrep.args` | `string[]` optional | Extra rg args (use `[]`) |

This field was confirmed present in CC v2.1.89 cli.js Zod schema. It overrides
CC's internal `$aA()` path resolution, including the vendor fallback.

**Confidence: HIGH** — confirmed by source inspection of active CC version.

### Internal Boundary: settings.rs ↔ deps.rs

Both independently call `which::which("rg")`. There is no shared state needed —
settings.rs uses the result to populate JSON, deps.rs uses it to decide whether
to warn. No refactoring needed; the two calls are independent.

If code reuse becomes desirable in the future, extract a `resolve_system_rg() -> Option<PathBuf>`
helper in a shared module. Not needed for this milestone.

### Internal Boundary: deps.rs ↔ doctor.rs

Both check rg availability. The doctor check is richer (resolves CC vendor path as
fallback, emits DoctorCheck struct). The deps.rs check is simpler (just warn).
Code can be duplicated (small amount) or shared via an `is_rg_available_for_sandbox()`
helper. Given the existing pattern (doctor.rs is standalone), duplication is fine.

## Scaling Considerations

Not applicable — this is a local CLI tool. The fix affects only per-agent
`settings.json` generation at `rightclaw up` time.

## Anti-Patterns

### Anti-Pattern 1: Symlinking rg into the Agent Dir

**What people do:** Create a symlink at `agent_dir/.bin/rg` → system rg,
then add `agent_dir/.bin` to the PATH or `allowWrite`.

**Why it's wrong:** Bubblewrap's `--bind agent_dir agent_dir` is for write
access, not PATH injection. The CC subprocess inherits PATH from the spawning
process (rightclaw/process-compose), not from `allowWrite`. The symlink approach
adds complexity for zero benefit.

**Do this instead:** Set `sandbox.ripgrep.command` in settings.json — CC reads
this before constructing the bwrap call. Clean, supported, one line.

### Anti-Pattern 2: Hardcoding the Nix Store Hash

**What people do:** Hardcode `/nix/store/<hash>-ripgrep-.../bin/rg` in settings.rs
or a constant.

**Why it's wrong:** The nix store hash changes on every `rg` version update.
After a `nix profile upgrade`, the hardcoded path is stale.

**Do this instead:** `which::which("rg")` at `rightclaw up` time always resolves
to the current nix profile symlink target — automatically correct after upgrades.

### Anti-Pattern 3: Setting failIfUnavailable to Make Problems Visible

**What people do:** Add `"failIfUnavailable": true` to `settings.json` to force
CC to exit loudly when sandbox can't start, making the problem visible.

**Why it's wrong:** CC exits on startup with an error, the bot process crashes,
process-compose restarts it, infinite restart loop. The operator only sees the
restart counter climbing with no useful output. This is for managed enterprise
deployments, not debugging.

**Do this instead:** Fix the root cause (inject working rg path in settings.json).
Use `rightclaw doctor` to diagnose before `rightclaw up`.

### Anti-Pattern 4: Adding /nix or /nix/store to allowRead

**What people do:** Add `/nix` or `/nix/store` to `settings.json` `allowRead`,
assuming sandbox can't read the rg binary.

**Why it's wrong:** The bwrap sandbox already does `--ro-bind / /` — the entire
filesystem is bind-mounted read-only. The nix store is already readable. The
problem is the execute bit being absent on the vendored binary, not read access.
`allowRead` does not affect execute semantics.

**Do this instead:** Point CC to a different rg binary via `sandbox.ripgrep.command`.

## Sources

- CC source: `/nix/store/hhydcyh1z6h2fyznlqagb1p66l07yhp6-claude-code-bun-2.1.89/lib/node_modules/@anthropic-ai/claude-code/cli.js` (inspected 2026-04-02)
  - `$aA` lazy: rg resolution order (USE_BUILTIN_RIPGREP → system → vendor)
  - `zT8` / `checkDependencies`: verifies bwrap + socat + rg at sandbox init
  - `vx_` / `SandboxUnavailableReason`: returns reason string when sandbox can't start (silent degradation when `failIfUnavailable=false`)
  - `oO6()`: returns `{rgPath, rgArgs}` — `sandbox.ripgrep` field overrides `$aA()`
  - `sandbox.ripgrep` Zod schema: `{command: string, args?: string[]}` — confirmed documented field
  - `failIfUnavailable`: opt-in hard failure (default false)
  - `z34`: linux dep check function (bwrap + socat only — rg tested separately via `checkDependencies`)
- UAT failure record: `.planning/phases/28.2-v3-0-uat-fix-teloxide-native-tls-and-doctor-async-runtime/28.2-UAT.md` lines 101-111
- Existing fix attempt: `crates/bot/src/telegram/worker.rs:399` + `cron.rs:227` — `USE_BUILTIN_RIPGREP=1` already set (insufficient alone)
- Vendor rg permissions: mode `.r--r--r--` confirmed via `ls -la` on active CC nix build
- System rg path: `/nix/store/l327dgzc03fl423swhgkqnrb76ymsd9f-ripgrep-15.1.0/bin/rg` (.r-xr-xr-x — executable)

---
*Architecture research for: v3.1 CC sandbox nix rg fix*
*Researched: 2026-04-02*
