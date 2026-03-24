---
phase: 07-platform-compatibility
verified: 2026-03-24T15:30:00Z
status: passed
score: 8/8 must-haves verified
re_verification: false
---

# Phase 7: Platform Compatibility Verification Report

**Phase Goal:** Users on Linux and macOS get correct dependency guidance and automated installation for the new sandbox stack
**Verified:** 2026-03-24T15:30:00Z
**Status:** passed
**Re-verification:** No -- initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | rightclaw doctor checks for bwrap and socat binaries on Linux | VERIFIED | doctor.rs:59-76 -- `if std::env::consts::OS == "linux"` block calls `check_binary("bwrap", ...)` and `check_binary("socat", ...)` |
| 2 | rightclaw doctor runs bwrap smoke test with --unshare-net on Linux when bwrap is found | VERIFIED | doctor.rs:208-249 -- `check_bwrap_sandbox()` runs `Command::new("bwrap").args(["--ro-bind", "/", "/", "--unshare-net", "--dev", "/dev", "true"])`, only invoked when `bwrap_found` is true (line 73) |
| 3 | rightclaw doctor skips bwrap/socat checks on macOS | VERIFIED | doctor.rs:59 -- platform guard `std::env::consts::OS == "linux"` ensures non-Linux platforms skip entirely. Test `run_doctor_skips_bwrap_socat_on_non_linux` (line 506) validates |
| 4 | rightclaw doctor does not check for openshell | VERIFIED | 0 matches for "openshell" (case-insensitive) in doctor.rs. Test `run_doctor_always_checks_all_three_binaries` (line 449) asserts `!binary_names.contains(&"openshell")` |
| 5 | install.sh installs bubblewrap and socat on Linux | VERIFIED | install.sh:162-195 -- `install_sandbox_deps()` builds package list from missing deps, installs via apt-get/dnf/pacman |
| 6 | install.sh supports apt, dnf, and pacman package managers | VERIFIED | install.sh:183-189 -- three branches for `apt-get install -y`, `dnf install -y`, `pacman -S --noconfirm`, with die() fallback for unsupported managers |
| 7 | install.sh skips sandbox deps on macOS | VERIFIED | install.sh:163-166 -- `if [ "$PLATFORM" = "darwin" ]` returns early with Seatbelt message |
| 8 | install.sh no longer installs OpenShell | VERIFIED | 0 matches for "openshell" or "OpenShell" (case-insensitive) in install.sh. No `install_openshell` function exists |

**Score:** 8/8 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw/src/doctor.rs` | Platform-conditional sandbox dependency checks + bwrap smoke test | VERIFIED | 520 lines, contains `check_bwrap_sandbox`, `bwrap_fix_guidance`, platform-conditional block, 15 tests |
| `install.sh` | Platform-aware installer with Linux sandbox deps and no OpenShell | VERIFIED | 267 lines, contains `install_sandbox_deps()`, apt/dnf/pacman branches, macOS early return, zero OpenShell references |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| doctor.rs | std::process::Command | bwrap smoke test invocation | WIRED | Line 209: `std::process::Command::new("bwrap")` with full args |
| doctor.rs | std::env::consts::OS | platform conditional | WIRED | Line 59: `if std::env::consts::OS == "linux"` |
| doctor.rs | CLI main.rs | run_doctor() call | WIRED | `crates/rightclaw-cli/src/main.rs:176` calls `rightclaw::doctor::run_doctor(home)` |
| install.sh | bubblewrap/socat packages | apt-get/dnf/pacman install | WIRED | Lines 183-188: all three package managers with `$pkgs` variable containing "bubblewrap" and/or "socat" |
| install.sh main() | install_sandbox_deps | function call | WIRED | Line 241: `install_sandbox_deps` called in main flow between `install_process_compose` and `check_bun` |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| PLAT-01 | 07-01 | `rightclaw doctor` checks for bubblewrap and socat on Linux | SATISFIED | doctor.rs:60-70 checks both binaries inside Linux guard |
| PLAT-02 | 07-01 | `rightclaw doctor` detects AppArmor restriction and provides fix guidance | SATISFIED | doctor.rs:208-281 -- smoke test with `--unshare-net`, stderr parsing for RTM_NEWADDR/Operation not permitted, fix guidance with AppArmor profile + sysctl + Ubuntu docs link |
| PLAT-03 | 07-01 | `rightclaw doctor` no longer checks for OpenShell | SATISFIED | 0 openshell references in doctor.rs, existing test validates absence |
| PLAT-04 | 07-02 | `install.sh` installs bubblewrap and socat on Linux (apt/dnf/pacman), skips on macOS | SATISFIED | install.sh:162-195 -- selective install with apt-get, dnf, pacman; macOS early return with Seatbelt message |
| PLAT-05 | 07-02 | `install.sh` no longer installs OpenShell | SATISFIED | 0 openshell/OpenShell matches in install.sh, no `install_openshell` function |

No orphaned requirements found -- all 5 PLAT requirements from REQUIREMENTS.md are claimed and satisfied by plans 07-01 and 07-02.

### Success Criteria from ROADMAP.md

| # | Criterion | Status | Evidence |
|---|-----------|--------|----------|
| 1 | `rightclaw doctor` checks for bubblewrap and socat on Linux, skips on macOS | VERIFIED | Platform-conditional block in run_doctor() with cfg(test) coverage for both cases |
| 2 | `rightclaw doctor` detects Ubuntu 24.04+ AppArmor restriction and prints fix guidance | VERIFIED | check_bwrap_sandbox() with --unshare-net flag, stderr parsing, bwrap_fix_guidance() with AppArmor profile, sysctl, Ubuntu docs link |
| 3 | `rightclaw doctor` no longer checks for OpenShell | VERIFIED | Zero openshell references, test assertion confirms |
| 4 | `install.sh` installs bubblewrap/socat on Linux (apt/dnf/pacman), skips on macOS, no OpenShell | VERIFIED | install_sandbox_deps() with three package managers, macOS Seatbelt early return, zero OpenShell references |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|

No anti-patterns detected. Both files are clean of TODOs, FIXMEs, placeholders, empty implementations, and hardcoded empty data.

### Commits Verified

| Commit | Message | Status |
|--------|---------|--------|
| 6401a06 | test(07-01): add failing tests for bwrap/socat doctor checks | VERIFIED |
| d7a6e1e | feat(07-01): add bwrap/socat binary checks and bwrap smoke test to doctor | VERIFIED |
| 53761aa | feat(07-02): replace OpenShell install with sandbox deps in install.sh | VERIFIED |

### Human Verification Required

### 1. bwrap smoke test on Ubuntu 24.04+ with AppArmor restriction

**Test:** Run `rightclaw doctor` on an Ubuntu 24.04+ system with default AppArmor settings (kernel.apparmor_restrict_unprivileged_userns=1)
**Expected:** bwrap-sandbox check shows FAIL with fix guidance containing AppArmor profile creation instructions and sysctl workaround
**Why human:** Requires a specific OS version with AppArmor restriction active; cannot verify programmatically on this host

### 2. install.sh end-to-end on fresh Linux

**Test:** Run install.sh on a fresh Linux system without bwrap/socat installed
**Expected:** Script detects platform, installs bubblewrap and socat via system package manager, completes without error
**Why human:** Requires fresh system with sudo access and package manager; destructive/stateful operation

### 3. install.sh macOS path

**Test:** Run install.sh on macOS
**Expected:** Sandbox deps step prints "macOS uses built-in Seatbelt sandbox (no additional deps needed)" and proceeds
**Why human:** Requires macOS host

### Documentation Note

ROADMAP.md progress table shows Phase 7 as "0/2 plans complete" with status "Planning complete". The actual work is done and committed. This is a ROADMAP update lag, not a code gap.

### Gaps Summary

No gaps found. All 8 observable truths verified. All 5 requirements satisfied. All key links wired. All 3 commits verified. No anti-patterns detected. Shell syntax validates cleanly.

---

_Verified: 2026-03-24T15:30:00Z_
_Verifier: Claude (gsd-verifier)_
