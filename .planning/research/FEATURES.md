# Feature Research: Sandbox Fix & E2E Verification (v3.1)

**Domain:** Claude Code sandbox dependency detection — nix/devenv environments
**Researched:** 2026-04-02
**Confidence:** HIGH (CC official docs + GitHub issues verified; nix devenv.nix inspected directly)

---

## Context: What This Milestone Fixes

v3.0 shipped with sandbox enabled via `.claude/settings.json` per agent (`sandbox.enabled: true`).
The problem: CC's sandbox startup check looks for system `rg` (ripgrep) in PATH. In nix/devenv
environments, `rg` is either absent or lives in a nix store path that isn't in the PATH when
process-compose spawns agents.

**Current devenv.nix packages:** `git`, `process-compose`, `socat`, `bubblewrap` (Linux only).
**Missing:** `ripgrep`. This causes CC sandbox to fail its dependency check before any agent work runs.

**Separately:** V3.0 E2E flow (rightclaw up → bot → Telegram → cron) has never been validated with
`sandbox.enabled: true`. VER-01 was cancelled from v2.5. This milestone closes that gap.

**Existing features that must NOT be regreted:**
- `rightclaw up` generates `.claude/settings.json` with `sandbox.enabled: true` (default)
- `rightclaw doctor` checks `bwrap` + `socat` + runs bwrap smoke test
- `--no-sandbox` flag generates settings with `sandbox.enabled: false`
- Shell wrapper sets `HOME=$AGENT_DIR`, forwards 6 identity env vars
- `rightclaw doctor` checks managed-settings.json, Telegram webhooks, agent structure, sqlite3

---

## CC Sandbox Dependency Check — How It Works

**Source: CC official docs + GitHub issues (HIGH confidence)**

CC's sandbox startup checks for three system binaries on Linux/WSL2:

| Binary | Purpose | Where CC looks |
|--------|---------|----------------|
| `rg` (ripgrep) | File search, skill/command discovery | System PATH |
| `bwrap` (bubblewrap) | Filesystem + network isolation kernel primitive | System PATH |
| `socat` | Unix socket relay for network sandbox proxy | System PATH |

**CC uses vendored ripgrep for grep tool but checks system `rg` for sandbox startup.**
The vendored binary lives at `<cc-install>/vendor/ripgrep/<arch>/rg` and is controlled by
`USE_BUILTIN_RIPGREP` env var (default: use vendored). The sandbox dependency check is separate —
it requires system `rg` to be in PATH regardless of `USE_BUILTIN_RIPGREP`.

**Exact error when sandbox check fails:**
```
Error: Sandbox dependencies are not available on this system.
Required: ripgrep (rg), bubblewrap (bwrap), and socat.
```

**Fallback behavior:** By default CC shows a warning and runs without sandbox when dependencies are
missing. To make it a hard failure: `sandbox.failIfUnavailable: true` in settings.json.

**`/sandbox` command in CC:** Opens interactive menu showing current sandbox mode, dependency status,
and installation instructions for missing deps. Does not directly surface to rightclaw (CC TUI only).

**ENV vars that affect sandbox behavior:**
- `USE_BUILTIN_RIPGREP=0` — use system `rg` for grep tool (default: 1 = vendored); does NOT affect sandbox check
- `CLAUDE_DISABLE_NONESSENTIAL_TRAFFIC` — blocks feature flags, not sandbox
- No env var to skip sandbox dependency check specifically

---

## Feature Landscape

### Table Stakes

Features required for sandbox to actually work in the nix/devenv environment where rightclaw is
developed. Missing any = sandbox silently degrades to disabled.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| `ripgrep` in devenv.nix packages | CC sandbox startup requires `rg` in PATH. Without it, sandbox check fails (warning, not hard error by default). Agents run unsandboxed. | LOW | Add `pkgs.ripgrep` to `devenv.nix`. One line. |
| `rightclaw doctor` detects `rg` absence (Fail on Linux) | Doctor already checks `bwrap` and `socat` as Fail-severity on Linux. Same pattern must cover `rg`. | LOW | Add `check_binary("rg", ...)` in `doctor.rs` Linux block alongside existing bwrap/socat checks |
| `rightclaw doctor` verifies sandbox is actually enabled in generated settings.json | Sandbox might be disabled if `--no-sandbox` was used or settings.json is stale/corrupt | LOW | Read `agent/.claude/settings.json`, verify `sandbox.enabled: true`; Warn if false |
| E2E validation: sandbox-enabled bot receives Telegram message, responds | Core v3.0 flow must work with sandbox ON. Currently never validated with sandbox enabled. | MED | Manual UAT checklist + smoke test |
| E2E validation: cron fires under sandbox | `claude -p` subprocess spawned by cron runtime must work inside sandbox filesystem/network rules | MED | Cron execution with `sandbox.enabled: true` verified against allowed domains/paths |

### Differentiators

Features that make sandbox failures visible and diagnosable before they silently degrade security.

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Doctor detects sandbox actually active (not just deps present) | Presence of `bwrap` + `rg` + `socat` does not guarantee sandbox ran. Doctor should also check that generated settings.json has `sandbox.enabled: true`. | LOW | Read settings.json per agent, report `sandbox-config/<agent>` pass/warn |
| `sandbox.failIfUnavailable: true` in generated settings.json | Turns sandbox degradation from silent to hard failure. Operators immediately see that sandbox is broken instead of agents silently running unsandboxed. | LOW | Add `failIfUnavailable: true` to `generate_settings()` output. Docs must explain this makes `--no-sandbox` the explicit escape hatch. |
| Doctor check for USE_BUILTIN_RIPGREP env var conflict | Setting `USE_BUILTIN_RIPGREP=0` in agent env does NOT fix sandbox check — users may misunderstand and set it thinking it helps. Detect and warn. | LOW | If `USE_BUILTIN_RIPGREP=0` set in agent.yaml env and `rg` not in PATH, warn: "USE_BUILTIN_RIPGREP=0 does not substitute for system rg needed by sandbox check" |
| E2E test script with sandbox enabled | Automated smoke test: `rightclaw up`, verify `sandbox.enabled: true` in settings.json, send test Telegram message, verify response, wait for cron tick, verify cron ran | MED | Shell script in `tests/e2e/`; not full automation but repeatable manual UAT checklist |

### Anti-Features

| Anti-Feature | Why Requested | Why Avoid | Alternative |
|--------------|---------------|-----------|-------------|
| `sandbox.failIfUnavailable: false` as default (current behavior) | "Agents still run even if sandbox fails" | Silent sandbox degradation is a security foot-gun. Users think they have sandbox protection, don't. | Set `failIfUnavailable: true` in generated settings. `--no-sandbox` is the explicit opt-out. |
| Injecting `rg` path directly into CC via env var | "Fix without touching devenv.nix — just point CC at the nix store path" | No CC env var accepts a custom `rg` path for the sandbox check. The check uses `which`/PATH lookup. Path injection into PATH is the only supported mechanism. | Add `pkgs.ripgrep` to devenv.nix; PATH injection handled by devenv shell |
| USE_BUILTIN_RIPGREP=1 in agent env as sandbox fix | "Just tell CC to use its vendored ripgrep" | Vendored rg controls grep tool only. Sandbox check is a separate code path that always checks system PATH. Setting this does nothing for sandbox. | System ripgrep via devenv.nix is the only fix. |
| Patching CC's vendor/ripgrep permissions | "chmod +x the vendored binary" | Fixes skill/command discovery (known issue #42068) but is orthogonal to sandbox check. Sandbox needs system rg. | Fix the right problem: add system ripgrep to devenv.nix |
| Doctor suppressing rg Fail when sandbox.enabled is false | "If sandbox is off, rg isn't needed" | Operator may not realize sandbox is degraded. Always report missing `rg` on Linux; context (sandbox disabled) is informational only. | Report Fail for missing rg, add detail "sandbox will degrade to unsandboxed mode" |

---

## Feature Dependencies

```
[pkgs.ripgrep in devenv.nix]
    └──satisfies──> [CC sandbox rg dependency check]
    └──unblocks──> [sandbox.enabled: true agents actually run sandboxed]
    └──required before──> [E2E verification with sandbox enabled]

[doctor rg check (Linux Fail)]
    └──depends on──> [existing check_binary() infrastructure in doctor.rs]
    └──parallel to──> [existing bwrap check, socat check]
    └──surfaces before launch──> [sandbox degradation risk]

[sandbox.failIfUnavailable: true in generate_settings()]
    └──depends on──> [generate_settings() in codegen/settings.rs]
    └──changes behavior on dep failure──> [CC errors instead of silently running unsandboxed]
    └──requires devenv.nix ripgrep fix to work correctly, otherwise rightclaw up fails hard]
    └──escape hatch is──> [--no-sandbox flag, which sets sandbox.enabled: false]

[doctor sandbox-config check per agent]
    └──depends on──> [agent settings.json exists (generated by rightclaw up)]
    └──reads──> [agent/.claude/settings.json]
    └──independent of──> [rg binary check (structural check, not dep check)]
    └──reports──> [sandbox-config/<agent> Pass/Warn]

[E2E verification: bot + cron with sandbox]
    └──depends on──> [ripgrep in PATH (devenv fix)]
    └──depends on──> [bwrap + socat already present in devenv.nix]
    └──depends on──> [v3.0 bot runtime (Phase 23-28, already built)]
    └──depends on──> [cron runtime (Phase 27, already built)]
    └──validates end-to-end: rightclaw up → sandbox ON → Telegram msg → CC subprocess → response]
```

---

## MVP Definition

### Must Ship in v3.1

1. **devenv.nix: add `pkgs.ripgrep`** — one-line fix. Unblocks everything else.
2. **doctor.rs: add `rg` check on Linux** — Fail severity, same pattern as bwrap/socat.
3. **generate_settings(): add `failIfUnavailable: true`** — hard failure when sandbox deps missing.
4. **E2E validation pass: sandbox-enabled agents** — run full flow (up → bot → Telegram → cron) with sandbox ON; document results in UAT checklist.

### Add If E2E Exposes Issues

- Doctor checks sandbox-config per agent (reads settings.json, verifies enabled)
- Doctor detects USE_BUILTIN_RIPGREP=0 misuse when rg absent
- Cron runs need allowWrite to `crons/` dir in sandbox settings (verify current SandboxOverrides are sufficient)

### Out of Scope for v3.1

- Automated E2E tests (CI integration) — manual UAT is sufficient for now
- Sandbox for the Rust bot process itself (teloxide binary is not a CC subprocess — sandbox does not apply)
- Webhook or network isolation for the bot's Telegram long-polling calls — bot runs outside CC sandbox

---

## What E2E Verification Must Check

A sandbox-enabled E2E pass is considered valid when all of the following hold:

| Check | Pass Criteria |
|-------|---------------|
| `rightclaw doctor` | All Linux checks pass: `bwrap` ok, `socat` ok, `rg` ok, `bwrap-sandbox` ok |
| `rightclaw up` | Succeeds; no sandbox dep error in CC startup logs |
| `agent/.claude/settings.json` | `sandbox.enabled: true`, `failIfUnavailable: true` |
| Bot receives Telegram message | Message arrives, `claude -p` subprocess spawned |
| CC subprocess runs under sandbox | No "sandbox unavailable" warning in CC stderr/debug output |
| Bot sends response | Response arrives in Telegram thread |
| `crons/*.yaml` spec exists | Cron task fires at scheduled time under sandbox |
| Cron output logged | `cron_runs` table entry written; no sandbox-related failure in logs |
| `rightclaw doctor` post-up | All checks still pass; no webhook conflicts |

---

## Sources

- [CC Sandboxing docs](https://code.claude.com/docs/en/sandboxing) — dependency requirements (bwrap, socat), failIfUnavailable setting, sandbox modes (HIGH)
- [CC Troubleshooting docs](https://code.claude.com/docs/en/troubleshooting) — WSL2 sandbox setup, rg for search/discovery, USE_BUILTIN_RIPGREP (HIGH)
- [CC issue #26282](https://github.com/anthropics/claude-code/issues/26282) — exact error message "Required: ripgrep (rg), bubblewrap (bwrap), and socat" (HIGH)
- [CC issue #42068](https://github.com/anthropics/claude-code/issues/42068) — vendored ripgrep path `vendor/ripgrep/<arch>/rg`, permissions issue (HIGH)
- [CC issue #1843](https://github.com/anthropics/claude-code/issues/1843) — USE_BUILTIN_RIPGREP controls grep tool only, not sandbox check (MEDIUM)
- [RightClaw devenv.nix](../../../devenv.nix) — current packages: missing ripgrep confirmed (HIGH — direct inspection)
- [RightClaw doctor.rs](../../../crates/rightclaw/src/doctor.rs) — existing check_binary pattern, bwrap/socat Linux block (HIGH — direct inspection)
- [RightClaw codegen/settings.rs](../../../crates/rightclaw/src/codegen/settings.rs) — generate_settings(), current sandbox JSON structure (HIGH — direct inspection)
- [RightClaw PROJECT.md](../../PROJECT.md) — VER-01 validation gap, active milestone context (HIGH)

---
*Feature research for: RightClaw v3.1 Sandbox Fix & E2E Verification*
*Researched: 2026-04-02*
