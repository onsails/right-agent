# Phase 7: Platform Compatibility - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.

**Date:** 2026-03-24
**Phase:** 07-platform-compatibility
**Areas discussed:** Doctor check scope, AppArmor detection, install.sh changes

---

## Doctor Check Scope — Severity

| Option | Description | Selected |
|--------|-------------|----------|
| Warn (not Fail) | Sandbox won't work but rightclaw still functions | |
| Fail | Block rightclaw up if bubblewrap/socat missing on Linux | ✓ |
| Platform-aware | Fail on Linux, skip on macOS | |

**User's choice:** Fail
**Notes:** macOS skips checks per PLAT-01. Platform-aware behavior implicit.

---

## AppArmor Detection

| Option | Description | Selected |
|--------|-------------|----------|
| Check + actionable fix | Detect via /proc/sys/kernel/apparmor_restrict_unprivileged_userns, print sysctl command | |
| Check + link to docs | Detect restriction, print warning with docs link | |
| Try bwrap, report result | Run bwrap --ro-bind / / true as smoke test. Catches all restriction sources. | |

**User's choice:** Other — suggested testing via claude itself, but bwrap smoke test is better (faster, no side effects, isolates the exact capability). Research phase will verify exact bwrap invocation.

---

## install.sh Changes — Distros

| Option | Description | Selected |
|--------|-------------|----------|
| apt + dnf + pacman | Ubuntu/Debian, Fedora/RHEL, Arch. ~95% coverage. | ✓ |
| apt + dnf only | Major distros only | |
| apt only + fallback msg | Minimal coverage | |

**User's choice:** apt + dnf + pacman

---

## Claude's Discretion

- Doctor output formatting
- bubblewrap version checking
- bwrap smoke test bind mount args
- install.sh error handling

## Deferred Ideas

None
