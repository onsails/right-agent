# Stack Research: v3.1 Sandbox Fix & Verification

**Domain:** CC native sandbox dependency detection in nix environments
**Researched:** 2026-04-02
**Confidence:** HIGH — root cause confirmed by CC source inspection and Bun.which() live test

## Scope

Delta-research for the v3.1 milestone. Covers ONLY the sandbox fix:
1. Root cause of sandbox silently disabling in nix environments
2. Fix approaches ranked by correctness
3. Doctor diagnostics for sandbox dep detection
4. What NOT to try

Existing stack (tokio, serde, reqwest, rusqlite, teloxide, etc.) is NOT re-evaluated.

---

## Root Cause (Confirmed)

The nix CC package installs the vendored ripgrep binary at:

```
/nix/store/<hash>-claude-code-bun-2.1.89/lib/node_modules/@anthropic-ai/claude-code/vendor/ripgrep/x64-linux/rg
```

The nix store is immutable — all files get permissions `r--r--r--` (444). No execute bit.

CC's `checkDependencies()` calls `Bun.which(rgPath)` where `rgPath` is the absolute
path to the vendored binary. `Bun.which()` checks that a file exists AND is executable.
Since `444 & --x == 0`, `Bun.which()` returns `null`. CC pushes `"ripgrep not found"`
to the errors array and sandbox silently disables.

**Confirmed via live test:**
```
Bun.which("/nix/.../vendor/ripgrep/x64-linux/rg")  → null   (not executable)
Bun.which("rg")                                      → "/home/wb/.nix-profile/bin/rg"
```

**CC source path** (cli.js, G34 = checkDependencies):
```js
if (Zr(z.command) === null)   // Zr = Bun.which()
    K.push(`ripgrep (${z.command}) not found`);
```

**CC source path** (Bv8 = ripgrep config getter):
```js
Bv8 = $1(() => {
    if (A_(process.env.USE_BUILTIN_RIPGREP)) {   // A_("0") → true
        let { cmd: z } = tK4("rg", []);
        if (z !== "rg") return { mode: "system", command: "rg", args: [] };
    }
    // ... falls through to builtin/vendor path
    return { mode: "builtin", command: ".../vendor/ripgrep/x64-linux/rg", args: [] };
});
```

`A_(value)` returns `true` when value is `"0"`, `"false"`, `"no"`, or `"off"` — i.e. it
means "is USE_BUILTIN_RIPGREP disabled?" When `USE_BUILTIN_RIPGREP=0`, the vendored
path is SKIPPED and `rg` from PATH is used instead.

---

## Pre-existing Bug in Rightclaw

`crates/bot/src/telegram/worker.rs` line 399 has the wrong value:

```rust
// WRONG — "1" means "builtin is enabled" → still uses vendored path
cmd.env("USE_BUILTIN_RIPGREP", "1");
```

The comment says "Use system rg instead of CC's bundled vendor binary" but `"1"` does
the opposite. The correct fix is `"0"`.

---

## Fix: Approach 1 — Set USE_BUILTIN_RIPGREP=0 in CC subprocess env (RECOMMENDED)

**Where:** Every `tokio::process::Command` that invokes `claude`/`claude-bun`.
**Files:** `crates/bot/src/telegram/worker.rs` (invoke_cc), `crates/bot/src/cron.rs` (cron job invoker).

```rust
cmd.env("USE_BUILTIN_RIPGREP", "0");   // "0" → A_("0") = true → skip vendored binary, use rg from PATH
```

When `USE_BUILTIN_RIPGREP=0`, CC calls `tK4("rg", [])` to resolve the system `rg`. This
uses `Bun.which("rg")` — finds `/home/wb/.nix-profile/bin/rg` → returns `{ mode: "system", command: "rg" }`.
`checkDependencies` then calls `Bun.which("rg")` → non-null → no error → sandbox initializes.

**Prerequisite:** `rg` must be in PATH when CC runs. In devenv, ripgrep is already in
the devenv PATH (`devenv.nix` does NOT currently include ripgrep but the user's nix
profile has it). The devenv.nix should add ripgrep explicitly so it is always available:

```nix
packages = [
    pkgs.git
    pkgs.process-compose
    pkgs.socat
    pkgs.ripgrep          # ADD: required for CC sandbox rg check
] ++ lib.optionals pkgs.stdenv.isLinux [
    pkgs.bubblewrap
];
```

**Why this is correct:**
- Zero runtime cost — env var set at subprocess spawn time
- CC validates `rg` in PATH not a hard-coded path
- Works for both worker.rs (Telegram bot) and cron.rs (cron jobs)
- Mirrors the approach used by `sadjow/claude-code-nix` (authoritative nix CC package)

**Confidence:** HIGH — approach confirmed by CC source + nix package precedent.

---

## Fix: Approach 2 — Add ripgrep to devenv packages (REQUIRED COMPLEMENT)

Even with `USE_BUILTIN_RIPGREP=0`, CC must find `rg` in PATH. The rightclaw process-compose
entries run with the environment that rightclaw up was launched in. The devenv environment
currently gets ripgrep from the user's `~/.nix-profile`, not from devenv explicitly.

Add to `devenv.nix`:

```nix
packages = [
    pkgs.git
    pkgs.process-compose
    pkgs.socat
    pkgs.ripgrep
] ++ lib.optionals pkgs.stdenv.isLinux [
    pkgs.bubblewrap
];
```

This ensures ripgrep is in PATH in any devenv shell, not just when the user happens to
have it in their nix profile.

**Confidence:** HIGH — ripgrep is a standard nix package.

---

## Fix: Approach 3 — Doctor check for vendored rg executability (DIAGNOSTIC)

The doctor should detect the nix sandbox issue before launch and surface a clear message.

Add to `crates/rightclaw/src/doctor.rs`:

```rust
// Check: CC vendored ripgrep executability (nix issue)
checks.push(check_cc_vendored_rg());
```

The check should:
1. Find the CC binary path (using `which::which("claude").or_else(|_| which::which("claude-bun"))`)
2. Resolve the node_modules parent: `cc_bin.parent()?.parent()?.join("lib/node_modules/@anthropic-ai/claude-code")`
3. For Linux, build the vendored path: `cc_root/vendor/ripgrep/x64-linux/rg`
4. Check if the file exists AND is executable (`metadata.permissions().mode() & 0o111 != 0`)
5. If not executable: emit Warn with message "CC vendored ripgrep not executable (nix store). Set USE_BUILTIN_RIPGREP=0 in agent env — rightclaw up handles this automatically."
6. If `USE_BUILTIN_RIPGREP=0` is already set: emit Pass "CC using system ripgrep (nix-compatible)"
7. If no vendored rg found at all (non-nix install): emit Pass "CC vendored ripgrep not present (non-nix install)"

**Severity:** Warn (not Fail) — rightclaw up automatically injects `USE_BUILTIN_RIPGREP=0`
so after the fix the doctor check becomes informational.

---

## What NOT to Try

| Approach | Why Not |
|----------|---------|
| `chmod +x .../vendor/ripgrep/x64-linux/rg` | Nix store is immutable — chmod fails with EROFS. Re-applied on every nix upgrade anyway. |
| Nix overlay to patch CC package | Overkill — the fix is a single env var. Nix overlays require nix expertise and pin the CC version. |
| Symlink system rg over vendored path | Nix store is read-only. Would require a writable CC install. |
| `sandbox.enabled: false` in settings.json | Defeats the entire security model. The goal is to ENABLE sandbox, not disable it. |
| Set `USE_BUILTIN_RIPGREP=1` | WRONG — value "1" passes `A_("1") = false`, keeping the vendored path. The existing code in worker.rs has this bug. |
| Wrapper script around claude binary | More complexity than needed. env var approach is cleaner and already works. |
| `SANDBOX_RUNTIME=1` env var | That's an internal CC env var set INSIDE the sandbox. Not for external use. |

---

## Integration Points in Rightclaw

### worker.rs (invoke_cc) — BROKEN, needs fix
```rust
// Current (wrong):
cmd.env("USE_BUILTIN_RIPGREP", "1");

// Fixed:
cmd.env("USE_BUILTIN_RIPGREP", "0");
```

### cron.rs (cron job invoker) — needs same fix
The cron job invoker spawns `claude -p` similarly. Verify it also sets
`USE_BUILTIN_RIPGREP=0` or add it.

### devenv.nix — add ripgrep
```nix
pkgs.ripgrep
```

### doctor.rs — add check_cc_vendored_rg
New check function that detects nix install + non-executable vendored rg + advises.

---

## No New Rust Crates Required

The fix requires no new dependencies:

| What | How |
|------|-----|
| `USE_BUILTIN_RIPGREP=0` in subprocess env | `std::process::Command::env()` / `tokio::process::Command::env()` — already used |
| Vendored rg executability check | `std::fs::metadata()` + `std::os::unix::fs::PermissionsExt` — stdlib |
| Find CC binary for doctor check | `which::which()` — already in workspace deps |
| ripgrep in devenv PATH | `pkgs.ripgrep` in devenv.nix — devenv package, not a Rust crate |

---

## Verification Sequence

After applying the fix, the sandbox status can be verified:

1. `rightclaw doctor` — should show Pass for new `cc-sandbox-rg` check
2. `rightclaw up` — launch agent
3. Inside agent session: `/sandbox` — should show sandbox enabled, not disabled
4. Inside agent session: `/doctor` — CC's own doctor should report ripgrep working
5. `rightclaw doctor` in a fresh shell (without devenv active) — ensure rg is still found

---

## Sources

- CC cli.js source, G34 (`checkDependencies`) — extracted from `/nix/store/.../cli.js`, line 780
- CC cli.js source, Bv8 (`ripgrep config getter`) — `USE_BUILTIN_RIPGREP` logic confirmed
- CC cli.js source, Zr (`Bun.which` wrapper) — used for dependency detection
- CC cli.js source, A_ — value semantics: `"0"` → builtin disabled → use system rg
- [sadjow/claude-code-nix package.nix](https://github.com/sadjow/claude-code-nix/blob/main/package.nix) — `USE_BUILTIN_RIPGREP=0` + `--prefix PATH ripgrep` pattern confirmed HIGH confidence
- [CC issue #42068](https://github.com/anthropics/claude-code/issues/42068) — vendored rg permissions bug (same root issue), April 2026
- [CC sandboxing docs](https://code.claude.com/docs/en/sandboxing) — `sandbox.failIfUnavailable` behavior
- Live Bun.which test: `Bun.which("/nix/.../vendor/ripgrep/x64-linux/rg")` → `null` (confirmed)
- Live file check: `/nix/.../vendor/ripgrep/x64-linux/rg` has mode `r--r--r--` (444)

---
*Stack research for: RightClaw v3.1 Sandbox Fix & Verification*
*Researched: 2026-04-02*
