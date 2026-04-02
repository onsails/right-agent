# Project Research Summary

**Project:** RightClaw v3.1 — Sandbox Fix & E2E Verification
**Domain:** CC native sandbox dependency detection in nix/devenv environments
**Researched:** 2026-04-02
**Confidence:** HIGH

## Executive Summary

RightClaw v3.0 shipped with CC native sandbox enabled per-agent via `settings.json`, but sandbox silently degrades to disabled in nix/devenv environments. The root cause is confirmed by direct CC source inspection: `checkDependencies()` calls `Bun.which()` on the vendored ripgrep binary, which has mode `r--r--r--` (444) in the nix store — no execute bit. `Bun.which()` returns null for non-executable files, CC pushes "ripgrep not found" to its errors array, and sandbox silently disables because `failIfUnavailable` defaults to false. There is also an existing bug in the codebase: `worker.rs` and `cron.rs` already set `USE_BUILTIN_RIPGREP=1`, which is the wrong value — `"1"` means "use bundled CC rg" (the broken one), `"0"` means "use system rg."

The fix has three complementary parts. First, change `USE_BUILTIN_RIPGREP` to `"0"` in worker.rs and cron.rs. Second, add `pkgs.ripgrep` to `devenv.nix` so a working system `rg` is in PATH. Third, inject the resolved system `rg` path into each agent's `settings.json` via the officially-supported `sandbox.ripgrep.command` field — this overrides CC's internal path resolution entirely, bypassing the vendored binary fallback. The architecture research confirms `sandbox.ripgrep.command` is present in CC v2.1.89's Zod schema and is the cleanest authoritative fix, not a workaround.

The primary risk is incomplete validation masquerading as a complete fix. Two failure modes to guard against: (1) fixing only ripgrep while leaving socat and bwrap unverified — sandbox may still be non-functional for network calls; (2) doctor checks passing because they run in the devenv shell while agent processes inherit a different PATH from process-compose and fail silently. The v3.1 milestone must address the fix, the diagnostics, and a full E2E verification pass that explicitly covers all three sandbox dependencies and uses a repeatable checklist (not a one-time manual run).

## Key Findings

### Recommended Stack

No new Rust crates are required. All changes use stdlib (`std::fs::metadata`, `std::os::unix::fs::PermissionsExt`) and `which::which()` already in the workspace. The `sandbox.ripgrep.command` field in `settings.json` is an officially-supported CC schema field confirmed in CC v2.1.89 cli.js Zod schema. The `devenv.nix` change is one line: `pkgs.ripgrep`.

**Change sites (no new files, no new crates):**
- `codegen/settings.rs`: inject `sandbox.ripgrep.command` via `which::which("rg")` + add `failIfUnavailable: true`
- `codegen/settings_tests.rs`: add tests for ripgrep field presence when rg found vs. not found
- `telegram/worker.rs` + `cron.rs`: change `USE_BUILTIN_RIPGREP` from `"1"` to `"0"`
- `doctor.rs`: add `check_sandbox_ripgrep()` DoctorCheck (Warn severity, Linux-only block)
- `runtime/deps.rs`: add non-fatal rg warning (matches existing git-warning pattern)
- `devenv.nix`: add `pkgs.ripgrep` to packages list

### Expected Features

**Must have (table stakes) for v3.1:**
- `devenv.nix` ripgrep addition — sandbox cannot start without system rg in PATH
- `USE_BUILTIN_RIPGREP=0` fix in worker.rs and cron.rs — current value actively forces the broken vendored binary
- `sandbox.ripgrep.command` injection in `generate_settings()` — official field, cleanest fix
- `sandbox.failIfUnavailable: true` in generated settings.json — turns silent degradation into visible hard failure
- Doctor `rg` check with Fail severity on Linux — same pattern as existing bwrap/socat checks
- E2E validation pass: full flow (rightclaw up → doctor green → sandbox ON → Telegram → cron) with all three deps verified

**Should have (differentiators) — add if E2E exposes issues:**
- Doctor `sandbox-config/<agent>` check — reads generated settings.json, verifies `sandbox.enabled: true` per agent
- Doctor warns when system rg is reachable only via devenv shell but not agent process PATH
- E2E test script in `tests/e2e/` as a repeatable checklist, not a one-time document

**Defer to v3.2+:**
- Automated CI E2E integration (manual UAT sufficient for v3.1)
- Per-agent HOME isolation (SEED-004, deferred for edge cases with trust files and git/SSH)
- `--check-agent-env` doctor flag to simulate agent launch environment exactly

**Anti-features to avoid:**
- `sandbox.failIfUnavailable: true` added before the rg fix — causes infinite restart loop in process-compose if rg still missing; both changes must land together
- `USE_BUILTIN_RIPGREP=0` in `settings.json` env section instead of `cmd.env()` — settings.json env section may not propagate to sandbox-runtime; set directly via process env
- Any `/nix/store/<hash>/` path hardcoded anywhere — rots on every CC update

### Architecture Approach

The fix is surgical: four existing files modified, no new files, no new crates. The `settings.rs` change is the primary fix (injects `sandbox.ripgrep.command` as an absolute path). The env var fix in worker.rs/cron.rs is belt-and-suspenders. Doctor and deps.rs changes provide pre-flight and post-hoc diagnostics. The architecture research confirms `sandbox.ripgrep.command` overrides CC's `oO6()` function which is exactly the path that `checkDependencies` consults — making this the authoritative fix, not a workaround.

**Modified components:**
1. `codegen/settings.rs` — inject `sandbox.ripgrep.command` + `failIfUnavailable: true`; if rg not found, omit field and rely on deps.rs/doctor.rs to warn
2. `codegen/settings_tests.rs` — unit tests: field present when `which("rg")` succeeds; field absent when not found
3. `runtime/deps.rs` — non-fatal tracing::warn when system rg absent and vendor rg also not executable
4. `doctor.rs` — `check_sandbox_ripgrep()` as a `DoctorCheck` struct in Linux-only block, placed after `check_bwrap_sandbox()`

**Data flow after fix (CC subprocess):**
```
claude-bun -p ... reads settings.json
  sandbox.ripgrep.command = "/nix/store/<hash>/bin/rg"  ← injected by generate_settings()
  oO6() returns {command: "/nix/.../bin/rg", args: []}
  checkDependencies: rg (runs command) → executable → success
  SandboxUnavailableReason = undefined → sandbox engages
  bwrap wraps bash subprocess
```

### Critical Pitfalls

1. **USE_BUILTIN_RIPGREP=1 is the wrong value** — `"1"` means use bundled rg (the broken one); `"0"` means use system rg. The current code has inverted semantics with a comment that says the opposite. This must be the first change — if left in place, all subsequent verification validates the broken state, not the fixed one.

2. **Agent launch PATH diverges from doctor PATH** — Doctor runs in the devenv shell (rg on PATH, all checks pass). Agents launched by process-compose inherit only what rightclaw explicitly sets. The `sandbox.ripgrep.command` field in settings.json solves rg specifically (absolute path, no PATH lookup). But socat and bwrap still depend on the inherited PATH. Doctor checks must validate what agents will see, not what the shell sees.

3. **Nix store is immutable** — `chmod +x` on `/nix/store/.../rg` fails with EROFS. Symlinking into nix store paths fails. Any hardcoded nix store path rots on every CC update/rebuild. Use `which::which("rg")` dynamically at `rightclaw up` time — it resolves to the current nix profile symlink, always current.

4. **macOS vs Linux divergence** — bwrap on macOS (installed via Homebrew) triggers the Linux sandbox path which fails on macOS. The `devenv.nix` already scopes bwrap to Linux only — keep it that way. The ripgrep fix applies identically to both platforms. E2E must be verified on both; a Linux-only fix that passes locally may silently break macOS.

5. **Partial sandbox fix — only ripgrep verified** — CC sandbox requires rg + socat + bwrap on Linux. Fixing ripgrep alone may leave network calls inside sandbox failing silently (socat) or the sandbox not engaging at all (bwrap). E2E must explicitly test: Grep tool (rg), network call inside sandbox (socat proxy), filesystem write isolation (bwrap bind-mount).

6. **USE_BUILTIN_RIPGREP is undocumented** — it is an internal CC env var with no stable API guarantee. Future CC updates can change its semantics without notice. The E2E verification checklist must be designed as repeatable, specifically to catch regressions after CC version bumps.

7. **failIfUnavailable + missing rg = infinite restart loop** — setting `failIfUnavailable: true` before the rg fix lands causes CC to exit on startup, process-compose restarts it, infinite loop. Both changes must land in the same `rightclaw up` invocation. The fix (settings.rs + devenv.nix) must precede or coincide with failIfUnavailable addition.

## Implications for Roadmap

Based on research, suggested phase structure:

### Phase 1: Sandbox Dependency Fix
**Rationale:** Unblocks all subsequent work. The USE_BUILTIN_RIPGREP bug must be corrected before any testing, or verification validates the wrong state. All change sites are small and independent; this can be a single commit.
**Delivers:** CC sandbox actually engages in nix/devenv; `rightclaw up` generates settings.json with `sandbox.ripgrep.command` pointing to working system rg; `failIfUnavailable: true` in generated settings; devenv.nix includes ripgrep explicitly
**Addresses:** devenv.nix ripgrep, USE_BUILTIN_RIPGREP=0, sandbox.ripgrep.command injection, failIfUnavailable
**Avoids:** Pitfall 1 (wrong env var value), Pitfall 3 (nix store immutability), Pitfall 7 (restart loop — both changes land together)

### Phase 2: Doctor Diagnostics
**Rationale:** Doctor must accurately surface sandbox state before launch and reflect what agents will see — not the developer's interactive shell. Must complete before E2E verification so the verification pass can use doctor to confirm pre-conditions.
**Delivers:** `check_sandbox_ripgrep()` DoctorCheck in Linux block (Warn severity); non-fatal rg warning in deps.rs; sandbox-config check reading generated settings.json per agent
**Addresses:** Doctor rg check, sandbox-config/agent check, devenv-only dep warning
**Avoids:** Pitfall 2 (doctor PATH divergence)

### Phase 3: E2E Verification
**Rationale:** Only valid after Phase 1 (fix) and Phase 2 (diagnostics are accurate). Must validate all three sandbox deps explicitly, not just ripgrep. Must produce a repeatable checklist committed to the repo.
**Delivers:** Documented UAT pass confirming sandbox-enabled full flow on Linux; repeatable checklist in `tests/e2e/` covering all three deps; macOS verification documented separately
**Addresses:** Full flow (rightclaw up → doctor green → sandbox ON → Telegram → CC subprocess → cron), all three sandbox deps tested
**Avoids:** Pitfall 4 (macOS divergence — explicit separate verification), Pitfall 5 (partial sandbox fix), Pitfall 6 (one-time validation)

### Phase Ordering Rationale

- Phase 1 must precede Phase 2 and Phase 3: diagnostics and verification are meaningless until the underlying fix is in place
- Phase 2 must precede Phase 3: doctor is the primary pre-condition check tool during E2E runs
- Phase 1 is small enough to be a single PR (4-5 files, ~40 lines total change)
- failIfUnavailable and sandbox.ripgrep.command must land together in Phase 1 to avoid the restart loop pitfall
- Phase 3 E2E checklist must be committed as a living document and re-run after CC version bumps

### Research Flags

All phases have sufficient detail to proceed directly to implementation. No phase requires `/gsd:research-phase`.

Phases with standard patterns (skip research-phase):
- **Phase 1:** CC source inspected directly. All change sites identified by file + function. No new abstractions.
- **Phase 2:** Follows exact existing DoctorCheck pattern. No novel patterns needed.
- **Phase 3:** Manual UAT — execution only, no research needed.

One area requiring live validation during Phase 1 execution:
- **socat PATH inheritance:** Verify socat is reachable from the env that agent processes inherit from process-compose (not just devenv shell). The `sandbox.ripgrep.command` field handles rg specifically; socat and bwrap still depend on inherited PATH. If not present, explicit PATH injection in `cmd_up` is required.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | CC source inspected directly (cli.js active build). `sandbox.ripgrep` Zod schema confirmed. No speculation. |
| Features | HIGH | CC official docs + 5 GitHub issues (42068, 6415, 26282, 32275, 1843) verified. devenv.nix and rightclaw source inspected directly. |
| Architecture | HIGH | All change sites identified by file + function. Data flow confirmed via CC cli.js source. `sandbox.ripgrep` override path confirmed. |
| Pitfalls | HIGH | All 7 pitfalls sourced from confirmed CC GitHub issues or verified nix behavior. USE_BUILTIN_RIPGREP semantics confirmed via source + live Bun.which test. |

**Overall confidence:** HIGH

### Gaps to Address

- **socat agent PATH inheritance:** The `sandbox.ripgrep.command` fix handles rg by absolute path. Socat and bwrap still depend on agent process PATH. Confirm socat is in the PATH process-compose agents inherit; if not, add explicit PATH injection in `cmd_up` as part of Phase 1.
- **macOS E2E:** Linux fix is confirmed. macOS Seatbelt path (no bwrap, sandbox-exec) needs explicit E2E verification in Phase 3. Expected to work identically (same env var fix, same settings.json change) but untested.
- **CC update stability:** `USE_BUILTIN_RIPGREP` is an undocumented internal env var. After any CC version bump in devenv.lock, the Phase 3 E2E checklist must be re-run. Consider adding a note in `devenv.nix` or a doctor check that triggers when CC version changes.

## Sources

### Primary (HIGH confidence)
- CC cli.js source (inspected 2026-04-02, build 2.1.89): `checkDependencies` (zT8/G34), `USE_BUILTIN_RIPGREP` value semantics (A_/Bv8), `sandbox.ripgrep` Zod schema (oO6), `failIfUnavailable` default false, `SandboxUnavailableReason` silent degradation path
- [CC Sandboxing docs](https://code.claude.com/docs/en/sandboxing) — dependency requirements (bwrap, socat, rg), failIfUnavailable, sandbox modes
- [CC Issue #42068](https://github.com/anthropics/claude-code/issues/42068) — vendored rg permissions bug, USE_BUILTIN_RIPGREP=0 workaround documented
- [CC Issue #6415](https://github.com/anthropics/claude-code/issues/6415) — USE_BUILTIN_RIPGREP value semantics: 0=system rg, 1=bundled rg
- [CC Issue #26282](https://github.com/anthropics/claude-code/issues/26282) — exact error message for sandbox dep failure
- [CC Issue #32275](https://github.com/anthropics/claude-code/issues/32275) — bwrap on macOS triggers wrong sandbox path (closed March 2026)
- [sadjow/claude-code-nix package.nix](https://github.com/sadjow/claude-code-nix) — USE_BUILTIN_RIPGREP=0 + ripgrep in PATH as confirmed nix packaging pattern
- RightClaw codebase (inspected directly): devenv.nix, doctor.rs, codegen/settings.rs, telegram/worker.rs, cron.rs
- Live Bun.which test: `Bun.which("/nix/.../vendor/ripgrep/x64-linux/rg")` → null; vendor rg mode confirmed r--r--r-- (444)

### Secondary (MEDIUM confidence)
- [CC Issue #1843](https://github.com/anthropics/claude-code/issues/1843) — USE_BUILTIN_RIPGREP controls grep tool vs. sandbox check (separate code paths)
- [CC Issue #25418](https://github.com/anthropics/claude-code/issues/25418) — CC agent teams shadow binary issue on NixOS; DISABLE_AUTOUPDATER workaround
- [anthropic-experimental/sandbox-runtime](https://github.com/anthropic-experimental/sandbox-runtime) — confirmed dep chain: bwrap + socat + rg
- [NixOS Discourse: Packaging Claude Code](https://discourse.nixos.org/t/packaging-claude-code-on-nixos/61072) — nix store immutability, chmod failure patterns
- [Nix store immutability](https://nixos.org/guides/nix-pills/the-nix-store.html) — content-addressed, read-only by design

---
*Research completed: 2026-04-02*
*Ready for roadmap: yes*
