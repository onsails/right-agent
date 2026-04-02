# Requirements — v3.1 Sandbox Fix & Verification

**Milestone:** v3.1
**Status:** Active
**Defined:** 2026-04-02
**Core Value:** Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, orchestrated by a single CLI command.

---

## SBOX — Sandbox Dependency Fix

- [ ] **SBOX-01**: `rightclaw up` injects `sandbox.ripgrep.command` with resolved system rg path into per-agent settings.json (works in any environment — nix, apt, brew, etc.)
- [ ] **SBOX-02**: `USE_BUILTIN_RIPGREP` env var corrected from `"1"` to `"0"` in worker.rs and cron.rs CC subprocess invocations
- [ ] **SBOX-03**: `sandbox.failIfUnavailable: true` added to generated settings.json — sandbox failures become fatal instead of silent degradation
- [ ] **SBOX-04**: devenv.nix includes `pkgs.ripgrep` in packages list for development environment

## DOC — Doctor Diagnostics

- [ ] **DOC-01**: `rightclaw doctor` checks ripgrep availability in the PATH that agent processes will inherit (not just the developer's shell PATH)
- [ ] **DOC-02**: `rightclaw doctor` validates that generated settings.json contains correct `sandbox.ripgrep.command` pointing to an existing executable

## VER — E2E Verification

- [ ] **VER-01**: Bot subprocess (teloxide → `claude -p`) runs with sandbox enabled — CC stderr shows no sandbox warning
- [ ] **VER-02**: Cron subprocess (`claude -p --agent`) runs with sandbox enabled — CC stderr shows no sandbox warning
- [ ] **VER-03**: Repeatable verification script checks rg + socat + bwrap availability in agent launch environment

---

## Future Requirements

- Real-time streaming response (edit-in-place via `--output-format stream-json` + `editMessageText`) — v3.2
- `/fast` prefix for `--bare` mode per message — v3.2
- File watching hot-reload for cron specs (notify-debouncer-full) — v3.2
- File/image message handling from Telegram — v3.x
- Webhook support — v3.x

## Out of Scope

| Feature | Reason |
|---------|--------|
| macOS-specific sandbox fix | Seatbelt is built-in, no vendored binaries needed — macOS works already |
| Replacing bubblewrap/socat | These are CC's chosen sandbox deps — not our problem to replace |
| CC source code patches | We work around CC's behavior, don't fork CC |
| Automated CI sandbox test | Requires bwrap (unprivileged userns) which most CI runners block |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| SBOX-01 | — | Pending |
| SBOX-02 | — | Pending |
| SBOX-03 | — | Pending |
| SBOX-04 | — | Pending |
| DOC-01 | — | Pending |
| DOC-02 | — | Pending |
| VER-01 | — | Pending |
| VER-02 | — | Pending |
| VER-03 | — | Pending |

**Coverage:**
- v3.1 requirements: 9 total
- Mapped to phases: 0
- Unmapped: 9 ⚠️

---
*Requirements defined: 2026-04-02*
*Last updated: 2026-04-02 after initial definition*
