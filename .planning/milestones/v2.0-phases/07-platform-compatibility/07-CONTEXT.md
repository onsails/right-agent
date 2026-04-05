# Phase 7: Platform Compatibility - Context

**Gathered:** 2026-03-24
**Status:** Ready for planning

<domain>
## Phase Boundary

Update `rightclaw doctor` and `install.sh` for new CC native sandbox dependencies. Doctor checks bubblewrap + socat on Linux (fail if missing), runs bwrap smoke test to detect AppArmor restrictions, skips sandbox checks on macOS (Seatbelt built-in). Install.sh installs bubblewrap + socat via apt/dnf/pacman. Remove all OpenShell installation references.

</domain>

<decisions>
## Implementation Decisions

### Doctor Checks — Sandbox Dependencies
- **D-01:** Doctor checks for `bwrap` (bubblewrap) and `socat` binaries on Linux via `which` crate
- **D-02:** Missing bubblewrap or socat on Linux is a **Fail** severity — blocks `rightclaw up`
- **D-03:** macOS skips bubblewrap/socat checks entirely — Seatbelt is built-in, no external deps needed
- **D-04:** Platform detection via `std::env::consts::OS` — `"linux"` checks run, `"macos"` skips

### Doctor Checks — AppArmor Detection
- **D-05:** Run `bwrap --ro-bind / / true` as a smoke test instead of checking sysctl directly
- **D-06:** If bwrap smoke test passes → sandbox compatible, no further checks
- **D-07:** If bwrap smoke test fails → report failure with actionable fix guidance
- **D-08:** Fix guidance should include: `sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` (temporary) and `/etc/sysctl.d/` config (persistent), plus link to Ubuntu docs
- **D-09:** Smoke test only runs if bwrap binary is found (skip if binary check already failed)

### Doctor Checks — Cleanup
- **D-10:** Doctor no longer checks for openshell binary (already removed in Phase 5 D-15)
- **D-11:** Doctor no longer checks for policy.yaml in agent directories (already removed in Phase 5 D-05)

### install.sh — Linux Dependencies
- **D-12:** Support three package managers: apt (Ubuntu/Debian), dnf (Fedora/RHEL), pacman (Arch)
- **D-13:** Install bubblewrap and socat packages on Linux
- **D-14:** Package names: `bubblewrap socat` (apt), `bubblewrap socat` (dnf), `bubblewrap socat` (pacman)
- **D-15:** Auto-detect package manager via `command -v apt/dnf/pacman`

### install.sh — macOS
- **D-16:** No sandbox dependencies needed on macOS — Seatbelt is built-in
- **D-17:** Still install process-compose and rightclaw binary on macOS

### install.sh — Cleanup
- **D-18:** Remove all OpenShell installation commands from install.sh
- **D-19:** Remove openshell from dependency list and post-install verification

### Claude's Discretion
- Exact doctor output formatting (consistent with existing check_binary pattern)
- Whether to check bubblewrap version (CC may require minimum version)
- Whether bwrap smoke test should use `--ro-bind / /` or a more minimal bind mount
- install.sh error handling for failed package installs

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Doctor implementation
- `crates/rightclaw/src/doctor.rs` — current doctor checks, check_binary pattern, DoctorCheck Display impl

### Install script
- `install.sh` — current installation script with platform detection

### Dependency checking
- `crates/rightclaw/src/runtime/deps.rs` — verify_dependencies() (openshell already removed)

### Research
- `.planning/research/PITFALLS.md` — Ubuntu 24.04+ AppArmor restriction details
- `.planning/research/STACK.md` — bubblewrap/socat install commands per distro

### Prior phase context
- `.planning/phases/05-remove-openshell/05-CONTEXT.md` — D-14/D-15 (openshell already removed from deps/doctor)
- `.planning/phases/06-sandbox-configuration/06-CONTEXT.md` — sandbox settings schema

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `check_binary()` pattern in doctor.rs — takes binary name, returns DoctorCheck with pass/fail + message
- `DoctorCheck` enum with `Pass`, `Warn`, `Fail` variants and Display impl
- `which` crate already in workspace for binary detection
- install.sh already has platform detection (`uname -s`) and package manager detection

### Established Patterns
- Doctor checks return Vec<DoctorCheck>, display formatted results
- install.sh uses `command -v` for tool detection
- install.sh uses `sudo` for package installs with user confirmation

### Integration Points
- `doctor.rs::run_doctor()` — add bubblewrap/socat checks + bwrap smoke test
- `install.sh` — add bubblewrap/socat to Linux install section, remove openshell

</code_context>

<specifics>
## Specific Ideas

No specific requirements — standard tooling updates following established patterns.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 07-platform-compatibility*
*Context gathered: 2026-03-24*
