# Pitfalls Research

**Domain:** Fixing CC sandbox dependency detection in nix environments + end-to-end verification of rightclaw sandbox pipeline
**Researched:** 2026-04-02
**Confidence:** HIGH for ripgrep/vendor env var behavior (confirmed GitHub issues, official docs); HIGH for nix immutability constraints (nix design docs); MEDIUM for multi-vendor dep gaps (extrapolated from sandbox-runtime source observations); HIGH for macOS/Linux divergence (confirmed CC issues #32275, #19996); MEDIUM for env var env-var-value polarity bug (code inspection + issue #6415)

---

## Critical Pitfalls

### Pitfall 1: USE_BUILTIN_RIPGREP=1 Forces Bundled Binary — The Exact Opposite of the Stated Intent

**What goes wrong:**
The current codebase sets `cmd.env("USE_BUILTIN_RIPGREP", "1")` with the comment "Use system rg instead of CC's bundled vendor binary." The comment and value are inverted.

`USE_BUILTIN_RIPGREP=1` means: **use the bundled CC binary** (this is the default — the "1" enables the built-in). Setting it to `"0"` forces CC to use the system `rg` from `PATH`.

So the current code is actively forcing the broken bundled binary, not the system one. The nix store vendor binary lacks the execute bit (reported across Linux and macOS in CC issue #42068), so CC's sandbox silently degrades — skills don't load, grep tool fails — and the code's "fix" is actually reinforcing the failure.

**Why it happens:**
The naming is counterintuitive. "USE_BUILTIN_RIPGREP" sounds like "if 1, use system builtin rg," but in CC's implementation it means "if 1, use CC's own bundled rg." The bug is easy to introduce when reading the variable name without checking the issue tracker.

**How to avoid:**
- Set `cmd.env("USE_BUILTIN_RIPGREP", "0")` to force system `rg`.
- Add system `rg` to `PATH` before spawning `claude -p` (devenv already provides it via `pkgs.ripgrep` through rightclaw's `devenv.nix`, but not necessarily in the agent launch environment).
- Doctor check: verify `rg` is in the `PATH` that agents will inherit. The `which::which("rg")` call in doctor.rs should validate the exact PATH the agents receive, not the operator's interactive PATH.

**Warning signs:**
- Skills from `.claude/skills/` silently absent — CC doesn't list or invoke them.
- Grep tool (Search tool in CC) returns empty results or EACCES error in stderr.
- `claude --debug` shows `spawn .../vendor/ripgrep/.../rg EACCES` buried in debug output.
- `USE_BUILTIN_RIPGREP=1` anywhere in the agent environment.

**Phase to address:** Phase 1 (sandbox dependency fix). Single-line fix — change `"1"` to `"0"` — but must be caught before testing or the entire verification effort validates the wrong behavior.

---

### Pitfall 2: Nix Store Paths Are Immutable — Symlinking Into Them Silently Fails or Is Dangerous

**What goes wrong:**
A naive "fix" for the missing execute bit is to `chmod +x` the vendor ripgrep or symlink a working binary into the nix store path. Both approaches fail:

1. **chmod +x on nix store**: the nix store is mounted read-only. `chmod +x /nix/store/<hash>/...` returns `EROFS: read-only file system`. Even if it were possible, the hash in the store path encodes the file contents and permissions — changing permissions invalidates the store derivation. This is not a valid fix.

2. **Symlinking vendor/ripgrep into nix store path**: the vendor directory lives inside the CC npm package, which lives inside the nix store derivation for `claude-code`. The directory is immutable. You cannot write into it.

3. **Symlinking nix store rg into a mutable path**: this works but creates a fragile dependency on a specific nix store hash. When the nix derivation for ripgrep is garbage collected or updated, the symlink breaks silently without error until the next CC invocation fails.

**Why it happens:**
Developers familiar with mutable package managers (apt, brew, npm) assume they can patch installed files. Nix's content-addressed, immutable store invalidates this assumption entirely.

**How to avoid:**
- Do NOT attempt to symlink into the nix store.
- Do NOT attempt to chmod files in the nix store.
- The correct fix is env var: set `USE_BUILTIN_RIPGREP=0` so CC uses `rg` from `PATH`, and ensure `rg` is in the PATH that agents receive at launch time (not just the developer's interactive shell PATH).
- In `rightclaw up`, explicitly prepend the devenv/nix-provided `rg` to the agent's PATH via `cmd.env("PATH", ...)` if the system `rg` is only available through a devenv shell.

**Warning signs:**
- A "fix" that involves `chmod +x` on a path containing `/nix/store/` — this will silently fail or corrupt the derivation.
- A symlink from the project directory into `/nix/store/<hash>/...` — works until GC or update.
- Doctor passes in the developer's devenv shell but fails on a fresh `rightclaw up` from a non-devenv terminal.

**Phase to address:** Phase 1 (sandbox dependency fix). Must explicitly decide "no nix store mutation" before starting implementation, or it will be re-attempted as a quick fix when the env var approach seems more involved.

---

### Pitfall 3: Nix Store Hash Changes on Every CC Update — Hardcoded Paths Rot Immediately

**What goes wrong:**
Any path containing a nix store hash is version-specific: `/nix/store/abc123...-claude-code-1.2.3/...`. The hash changes every time the derivation changes — every CC update, every rebuild with different compiler flags, every devenv rebuild.

If the sandbox fix hardcodes a nix store path anywhere (in doctor.rs, in shell wrappers, in test assertions, in any generated configuration), those paths break silently on the next update. The fix appears to work on the current version but regresses without warning on every subsequent CC update.

This is particularly treacherous for doctor checks: if `check_binary("rg", ...)` uses `which::which` with the developer's PATH but the agent launch uses a different PATH (no devenv activation), doctor passes while `rightclaw up` agents fail.

**Why it happens:**
Developers test in their devenv shell where everything is on PATH. The agent processes launched by process-compose inherit a different environment — only what `rightclaw up` explicitly forwards.

**How to avoid:**
- Never hardcode nix store paths in any generated or compiled artifact.
- Use `which::which("rg")` dynamically at agent launch time, not at build time.
- The doctor check for `rg` should validate that `rg` is reachable from the environment that agents will actually receive — consider testing with the exact env vars rightclaw sets on agent processes.
- Add a dedicated doctor check: `sandbox-rg-accessible` — actually spawns a minimal CC command with the same env as agent processes and verifies rg works.

**Warning signs:**
- Any `/nix/store/<hash>/` substring in generated files, doctor output, or runtime config.
- Doctor passes in devenv shell but not in a fresh login terminal.
- Tests that assert specific rg paths rather than testing behavior.

**Phase to address:** Phase 1 (sandbox dependency fix) + Phase 2 (doctor diagnostics). The doctor check design must account for env divergence.

---

### Pitfall 4: macOS and Linux Have Different Vendor Dep Chains — Fix One, Break the Other

**What goes wrong:**
CC uses different sandboxing mechanisms per OS:
- **Linux**: bubblewrap + socat + ripgrep. The CC sandbox-runtime checks for all three.
- **macOS**: Seatbelt (built-in `sandbox-exec`) + socat + ripgrep. No bwrap needed.

The nix environment pitfalls differ between platforms:
- On Linux/devenv: `bwrap` from `pkgs.bubblewrap`, `socat` from `pkgs.socat`, `rg` from devenv PATH. These may or may not be in the agent launch PATH.
- On macOS/devenv: no `bwrap` installed. **But if `bwrap` is installed via Homebrew on macOS (e.g., a Linux developer's laptop), CC detects it on PATH and switches to the Linux sandbox path — which fails on macOS** (confirmed CC issue #32275, locked March 2026).

So the risk cuts both ways: bwrap missing on Linux = fail; bwrap present on macOS = also fail.

**Why it happens:**
CC's platform detection checks binary availability before OS type in some code paths. The dev environment (Homebrew on macOS, devenv on Linux) may install binaries that confuse CC's runtime detection.

**How to avoid:**
- On macOS: do NOT install bwrap via Homebrew or include it in macOS devenv. The CLAUDE.md `devenv.nix` already scopes bwrap to Linux only (`lib.optionals pkgs.stdenv.isLinux`). This is correct — do not change it.
- On Linux: ensure socat is reachable from the agent launch PATH, not just the devenv shell. The bwrap smoke test in doctor.rs currently tests with `--unshare-net --dev /dev` — this correctly detects AppArmor restrictions.
- The `USE_BUILTIN_RIPGREP=0` fix applies identically to both platforms. Verify on both.
- Doctor checks should emit different checks per platform, not a single cross-platform list.

**Warning signs:**
- A macOS developer testing fixes that work locally but break on Linux CI (or vice versa).
- Any fix that involves `pkgs.bubblewrap` on macOS (wrongly added to non-Linux devenv).
- `rightclaw doctor` showing "bwrap: pass" on macOS — this means bwrap is installed and may trigger the wrong sandbox path.

**Phase to address:** Phase 1 (sandbox fix must be tested on both platforms) + Phase 2 (doctor must distinguish macOS vs Linux vendor dep chains).

---

### Pitfall 5: Fixing Ripgrep Misses Other Vendor Dependencies — Sandbox Still Broken

**What goes wrong:**
CC's sandbox-runtime checks for multiple dependencies, not just ripgrep. Known deps:
- `rg` (ripgrep) — for skill/command discovery and Grep tool
- `socat` — for network proxying inside the bubblewrap sandbox (Linux)
- `bwrap` (Linux only) — the container runtime itself

If the fix addresses only ripgrep, the sandbox may partially work (skills load) but silently fail on network calls (socat missing) or fail entirely on bwrap invocation. The failure mode differs: missing socat causes network tool calls inside sandbox to fail or hang, not a startup error. Missing bwrap causes sandbox to disable silently (CC falls back to unsandboxed mode without a clear error).

**Why it happens:**
The issue that surfaces first (ripgrep, because it blocks skill loading, which is visible immediately) gets fixed, but the subtler dependencies (socat, bwrap) are not validated in the same pass.

**How to avoid:**
- The fix phase must validate ALL sandbox dependencies end-to-end, not just ripgrep.
- E2E verification must include: a tool call that uses the Grep tool (exercises rg), a Bash command that makes a network call inside sandbox (exercises socat + bwrap network proxy), and a filesystem write to the agent dir (exercises bwrap bind-mount).
- Doctor must explicitly check all three deps against the agent launch environment, not just the shell PATH.
- Add a doctor check that actually runs `claude -p "test" --max-turns 1` with sandbox enabled and checks for sandbox-related errors in stderr.

**Warning signs:**
- "Sandbox fixed" declared after only validating skill loading / grep tool.
- Telegram bot responds (proving basic CC works) but network tool calls inside agent fail silently.
- No E2E test that exercises a sandboxed network call.

**Phase to address:** Phase 2 (E2E verification). The verification phase must have explicit test cases for each vendor dep, not just smoke tests.

---

### Pitfall 6: CC Updates Can Silently Revert the Env Var Fix — No Stable Contract

**What goes wrong:**
`USE_BUILTIN_RIPGREP` is an undocumented internal env var. It is not part of CC's stable API. Anthropic has historically changed behavior of undocumented env vars without deprecation notices.

Two failure modes:
1. A future CC update changes the semantics: `USE_BUILTIN_RIPGREP=0` stops working, or the variable is removed, or the variable is renamed.
2. A future CC update fixes the nix store permission issue (e.g., via a postinstall chmod), making `USE_BUILTIN_RIPGREP=0` redundant — but now the system `rg` version may not match what CC expects (if CC adds a version check).

Neither failure is detectable without running tests after each CC update.

**Why it happens:**
Undocumented env vars are implementation details. CC issue #6415 confirmed the setting worked as of August 2025, but it was not documented in any public API.

**How to avoid:**
- Document the dependency explicitly in the codebase: "CC uses USE_BUILTIN_RIPGREP=0 to skip its bundled rg. This is undocumented. Verify after each CC update."
- Add a CI-friendly doctor check that validates sandbox is actually engaged (not just that deps are present).
- The E2E verification phase should be designed as a repeatable test (not a one-time manual check) specifically so it can be re-run after CC updates.
- Consider wrapping the env var in a named constant in the Rust code with a comment linking to the CC issue.

**Warning signs:**
- CC version bumped in devenv.lock but no re-verification run.
- `USE_BUILTIN_RIPGREP` removed from CC CHANGELOG with no note in rightclaw.
- Doctor passes but skills silently absent after CC update.

**Phase to address:** Phase 2 (E2E verification must be designed as repeatable, not one-time) + ongoing maintenance awareness.

---

### Pitfall 7: Agent Launch PATH Diverges From Doctor PATH — Doctor Lies

**What goes wrong:**
`rightclaw doctor` runs in the operator's interactive shell (which has devenv activated, so `rg`, `bwrap`, `socat` are all on PATH). Agent processes launched by process-compose inherit a different environment — only what rightclaw explicitly sets.

If rightclaw does not explicitly inject the devenv PATH or the system PATH into agent env, the agent may fail to find `rg` or `socat` even though doctor reports them as present.

Concretely: a developer runs `devenv shell`, then `rightclaw doctor` — all green. They exit the shell and run `rightclaw up` from a regular terminal where devenv PATH is not active. Agents fail to find `rg`. Doctor passed, agents broken.

**Why it happens:**
Doctor uses `which::which` against the current process's PATH. Agent processes inherit only what `std::process::Command` explicitly sets via `.env()`. If PATH is not forwarded, the agent sees the minimal system PATH, not the devenv PATH.

**How to avoid:**
- In `rightclaw up`, explicitly resolve the paths to `rg`, `socat`, and `bwrap` at launch time and include them in the PATH forwarded to agents.
- Or: inject the devenv PATH explicitly — detect devenv activation and capture `DEVENV_PROFILE/bin` for injection.
- Doctor should warn when `rg` is found but is only accessible via a devenv shell (heuristic: path contains `/nix/store/` or `/devenv/`).
- Consider adding a `--check-agent-env` flag to doctor that simulates agent launch environment.

**Warning signs:**
- Doctor passes but agents fail immediately after launch.
- `rg` path in doctor output contains `/nix/store/` or `devenv` — only available in devenv context.
- Agent stderr contains "rg not found" or "EACCES" despite doctor showing rg present.

**Phase to address:** Phase 1 (sandbox fix) — the PATH injection must be part of the fix, not the doctor. Phase 2 (doctor diagnostics) — doctor must detect devenv-only deps.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Set USE_BUILTIN_RIPGREP=1 (the current bug) | Looks like a fix, compiles | Forced bundled rg, sandbox broken | Never — correct value is 0 |
| chmod +x on nix store | Quick unblock | Breaks on rebuild, violates nix invariants | Never |
| Hardcode nix store path | Avoids dynamic PATH resolution | Rots on every CC update | Never |
| Test only in devenv shell | Tests pass quickly | Doctor/agent PATH divergence goes undetected | Never for E2E tests |
| Only fix rg, skip socat/bwrap validation | One failing thing fixed | Partial sandbox — network calls broken | Never for "sandbox verified" claim |
| USE_BUILTIN_RIPGREP without a constant + comment | Shorter code | Future maintainer won't know what it does | Acceptable if documented with CC issue link |

---

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| nix + CC vendor rg | Set USE_BUILTIN_RIPGREP=1 thinking it enables system rg | Set =0 to use system rg; =1 (default) uses bundled |
| nix store + chmod | chmod +x /nix/store/.../rg | Don't touch nix store; use env var instead |
| devenv + process-compose | Agent inherits developer's devenv PATH | Explicitly set PATH in agent env during `rightclaw up` |
| macOS + bwrap | Install bwrap via Homebrew "to match Linux" | Never install bwrap on macOS; CC uses Seatbelt there |
| doctor + agent env | Doctor uses shell PATH, agents use process PATH | Doctor must check both or warn when deps are devenv-only |
| CC update + USE_BUILTIN_RIPGREP | Assume env var persists unchanged | Re-verify after CC version bump; env var is undocumented |
| sandbox verification | Only test that skill loads (rg present) | Also test network call inside sandbox (socat) and write isolation (bwrap) |
| USE_BUILTIN_RIPGREP in settings.json env section | Setting it in settings.json but not cmd env | Set in the process env directly via `cmd.env()` — settings.json env section may not propagate to sandbox-runtime |

---

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Disabling sandbox to work around dep issues (`--no-sandbox`) | Agents run with unrestricted filesystem + network access; defeats the security model | Fix deps instead of disabling; `--no-sandbox` should only be a debug flag, never production |
| Passing `DISABLE_INSTALLATION_CHECKS=1` without `DISABLE_AUTOUPDATER=1` | CC agent teams may still download unpatched binaries to ~/.local/bin/, shadowing nix-managed ones | Pass both env vars together; add doctor check for ~/.local/bin/claude shadow |
| bwrap AppArmor fix via `sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` | Disables protection globally, not just for bwrap | Use targeted AppArmor profile (already in doctor.rs guidance) |
| Injecting full HOST PATH into agent env | Agent may find and execute host binaries outside allowedDomains | Inject minimal PATH: only dirs containing rg, socat, bwrap, git |

---

## "Looks Done But Isn't" Checklist

- [ ] **USE_BUILTIN_RIPGREP value**: Is it `"0"` (use system rg) not `"1"` (use bundled)? Check worker.rs and cron.rs.
- [ ] **System rg in agent PATH**: Not just in devenv shell — verify `which rg` from a non-devenv terminal returns a valid binary, and that rightclaw up injects it into agent env.
- [ ] **Sandbox actually engaged**: Not just "rg found." Run `claude -p "use bash to run: id" --max-turns 1` inside a sandboxed agent and verify sandbox constraints appear in stderr or behavior.
- [ ] **Socat validated**: Not just that socat binary exists — verify a network tool call inside an agent sandbox succeeds (exercises socat proxy path).
- [ ] **macOS verified separately**: A Linux fix that passes on Linux may break macOS (bwrap vs Seatbelt divergence). Test explicitly.
- [ ] **Doctor uses agent env, not shell env**: Doctor check for rg should reflect what the agent process sees, not the operator's interactive PATH.
- [ ] **No nix store paths in generated files**: grep for `/nix/store/` in all generated config, doctor output, and test assertions.
- [ ] **CC update re-verification**: After any claude-code version bump, the sandbox E2E test must be re-run explicitly.
- [ ] **Sandbox not silently disabled**: Verify `settings.json` still has `"sandbox": {"enabled": true}` after each `rightclaw up`. The `--no-sandbox` flag should not silently persist.
- [ ] **~/.local/bin/claude shadow**: On NixOS/nix environments, check that CC agent teams has not downloaded an unpatched binary that shadows the nix-managed one.

---

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| USE_BUILTIN_RIPGREP wrong value | LOW | Change "1" to "0" in worker.rs and cron.rs, rebuild |
| Nix store mutation attempted | MEDIUM | `nix-store --verify --check-contents` to detect corruption; `nix-store --repair-path` if needed; revert to env var approach |
| PATH divergence (doctor passes, agents fail) | MEDIUM | Add explicit PATH injection in cmd_up; re-run E2E verification |
| macOS bwrap installed accidentally | LOW | `brew uninstall bubblewrap`; verify CC returns to Seatbelt sandbox |
| Partial sandbox (rg fixed, socat/bwrap not) | MEDIUM | Run full E2E suite against all three deps; add socat/bwrap to agent PATH injection |
| ~/.local/bin/claude shadows nix binary | LOW | `rm -rf ~/.local/bin/claude ~/.local/share/claude*`; add doctor check to detect this |
| USE_BUILTIN_RIPGREP semantics changed in CC update | HIGH | Reverify against new CC version; may need to find replacement env var or alternative approach |

---

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| USE_BUILTIN_RIPGREP wrong value (=1 vs =0) | Phase 1: sandbox fix | `grep USE_BUILTIN_RIPGREP crates/**/*.rs` shows only `"0"` |
| Nix store mutation temptation | Phase 1: pre-implementation constraint | Code review: no `/nix/store/` in any changed file |
| Nix store hash rot | Phase 1 + Phase 2 | `grep -r '/nix/store/' .planning/ crates/` finds nothing |
| macOS vs Linux divergence | Phase 1 + Phase 2 | Both platforms tested in E2E; macOS CI job exists |
| Missing socat/bwrap in fix scope | Phase 2: E2E verification | Test suite has explicit network-inside-sandbox and write-isolation tests |
| CC update invalidates env var | Phase 2: test design | E2E suite is automated and re-runnable, not one-time manual check |
| Doctor PATH divergence | Phase 1 (agent PATH injection) + Phase 2 (doctor check) | Doctor validates rg reachable from agent env, not just shell env |

---

## Sources

- [CC Issue #42068: Bundled ripgrep loses execute permission](https://github.com/anthropics/claude-code/issues/42068) — macOS + Linux, confirmed permissions bug, `USE_BUILTIN_RIPGREP=0` workaround documented in comments
- [CC Issue #6415: USE_BUILTIN_RIPGREP ignored](https://github.com/anthropics/claude-code/issues/6415) — bug fixed August 2025; value semantics: `0` = system rg, `1` = built-in rg
- [CC Issue #32275: bwrap on macOS via Homebrew triggers wrong sandbox path](https://github.com/anthropics/claude-code/issues/32275) — closed March 2026, root cause was missing rg; bwrap-on-macOS risk documented
- [CC Issue #25418: Agent teams installs incompatible binary on NixOS, shadows nix binary](https://github.com/anthropics/claude-code/issues/25418) — `DISABLE_AUTOUPDATER` + `DISABLE_INSTALLATION_CHECKS` bypass in agent teams code path
- [CC Issue #26282: Cowork sessions fail — sandbox dependency check fails inside VM](https://github.com/anthropics/claude-code/issues/26282) — sandbox dep check failure patterns
- [anthropic-experimental/sandbox-runtime](https://github.com/anthropic-experimental/sandbox-runtime) — requires bwrap (Linux), sandbox-exec (macOS), socat; no env var override for dep paths
- [sadjow/claude-code-nix](https://github.com/sadjow/claude-code-nix) — nix packaging approach; USE_BUILTIN_RIPGREP=0 used in nix overlays
- [NixOS Discourse: Packaging Claude Code](https://discourse.nixos.org/t/packaging-claude-code-on-nixos/61072) — immutability constraints, chmod failures on nix store
- [Nix store immutability](https://nixos.org/guides/nix-pills/the-nix-store.html) — store paths are content-addressed and immutable by design
- [CC Advanced Setup docs — USE_BUILTIN_RIPGREP](https://code.claude.com/docs/en/setup) — `USE_BUILTIN_RIPGREP=0` listed as env var for system ripgrep

---
*Pitfalls research for: v3.1 Sandbox Fix & Verification — nix + CC sandbox dependency detection + end-to-end verification*
*Researched: 2026-04-02*
