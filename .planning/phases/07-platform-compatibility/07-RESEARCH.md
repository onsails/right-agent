# Phase 7: Platform Compatibility - Research

**Researched:** 2026-03-24
**Domain:** Platform-specific dependency detection, AppArmor diagnostics, install script updates
**Confidence:** HIGH

## Summary

Phase 7 updates `rightclaw doctor` and `install.sh` to handle the new CC native sandbox dependencies (bubblewrap + socat on Linux) and removes all OpenShell references from the install script. The doctor gains two new binary checks and a bwrap smoke test for AppArmor namespace restrictions. The install script gets Linux package installation for three distro families.

The most important finding: the CONTEXT.md smoke test `bwrap --ro-bind / / true` is **insufficient**. The Codex issue #12572 confirms that basic bwrap namespace creation can succeed while the actual CC sandbox invocation fails because `--unshare-net` triggers loopback network configuration inside the namespace, which AppArmor blocks separately. The smoke test MUST include `--unshare-net` to detect the actual failure mode: `bwrap --ro-bind / / --unshare-net --dev /dev true`.

**Primary recommendation:** Use `bwrap --ro-bind / / --unshare-net --dev /dev true` as the smoke test, not the simpler variant. No minimum bwrap version check needed -- any distro-packaged version works.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Doctor checks for `bwrap` (bubblewrap) and `socat` binaries on Linux via `which` crate
- **D-02:** Missing bubblewrap or socat on Linux is a **Fail** severity -- blocks `rightclaw up`
- **D-03:** macOS skips bubblewrap/socat checks entirely -- Seatbelt is built-in, no external deps needed
- **D-04:** Platform detection via `std::env::consts::OS` -- `"linux"` checks run, `"macos"` skips
- **D-05:** Run `bwrap --ro-bind / / true` as a smoke test instead of checking sysctl directly
- **D-06:** If bwrap smoke test passes -> sandbox compatible, no further checks
- **D-07:** If bwrap smoke test fails -> report failure with actionable fix guidance
- **D-08:** Fix guidance should include: `sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` (temporary) and `/etc/sysctl.d/` config (persistent), plus link to Ubuntu docs
- **D-09:** Smoke test only runs if bwrap binary is found (skip if binary check already failed)
- **D-10:** Doctor no longer checks for openshell binary (already removed in Phase 5 D-15)
- **D-11:** Doctor no longer checks for policy.yaml in agent directories (already removed in Phase 5 D-05)
- **D-12:** Support three package managers: apt (Ubuntu/Debian), dnf (Fedora/RHEL), pacman (Arch)
- **D-13:** Install bubblewrap and socat packages on Linux
- **D-14:** Package names: `bubblewrap socat` (apt), `bubblewrap socat` (dnf), `bubblewrap socat` (pacman)
- **D-15:** Auto-detect package manager via `command -v apt/dnf/pacman`
- **D-16:** No sandbox dependencies needed on macOS -- Seatbelt is built-in
- **D-17:** Still install process-compose and rightclaw binary on macOS
- **D-18:** Remove all OpenShell installation commands from install.sh
- **D-19:** Remove openshell from dependency list and post-install verification

### Claude's Discretion
- Exact doctor output formatting (consistent with existing check_binary pattern)
- Whether to check bubblewrap version (CC may require minimum version)
- Whether bwrap smoke test should use `--ro-bind / /` or a more minimal bind mount
- install.sh error handling for failed package installs

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| PLAT-01 | `rightclaw doctor` checks for bubblewrap and socat on Linux (not required on macOS) | Existing `check_binary()` pattern handles this. `which` crate already in workspace. Platform detection via `std::env::consts::OS`. |
| PLAT-02 | `rightclaw doctor` detects Ubuntu 24.04+ AppArmor restriction on unprivileged user namespaces and provides fix guidance | Smoke test `bwrap --ro-bind / / --unshare-net --dev /dev true` detects the restriction. Fix guidance documented below with AppArmor profile and sysctl commands. |
| PLAT-03 | `rightclaw doctor` no longer checks for OpenShell installation | Already removed in Phase 5. Verified: current doctor.rs has no openshell check. Test `run_doctor_always_checks_all_three_binaries` asserts `!binary_names.contains(&"openshell")`. |
| PLAT-04 | `install.sh` installs bubblewrap and socat on Linux (apt/dnf/pacman detection), skips on macOS | Package names verified identical across all three package managers (`bubblewrap socat`). Platform detection already in install.sh. |
| PLAT-05 | `install.sh` no longer installs OpenShell | `install_openshell()` function and its call in `main()` must be removed. |
</phase_requirements>

## Standard Stack

No new crates needed. All required functionality exists in the workspace.

### Existing Crates Used

| Crate | Purpose in This Phase |
|-------|----------------------|
| `which` | Binary detection for `bwrap` and `socat` in PATH |
| `std::process::Command` | Execute bwrap smoke test |
| `std::env::consts::OS` | Platform detection ("linux" vs "macos") |

### External Dependencies Checked

| Package | Binary | Package Name (apt) | Package Name (dnf) | Package Name (pacman) |
|---------|--------|-------------------|--------------------|-----------------------|
| bubblewrap | `bwrap` | `bubblewrap` | `bubblewrap` | `bubblewrap` |
| socat | `socat` | `socat` | `socat` | `socat` |

Package names are identical across all three package managers. Confidence: HIGH (verified via pkgs.org and official CC sandboxing docs).

### Version Requirements

**No minimum bwrap version check needed.** Research finding: CC's sandbox-runtime (`linux-sandbox-utils.ts`) performs NO version check -- it only checks PATH presence via `whichSync('bwrap')`. Any distro-packaged version works (0.4+ supports user namespaces, which is all that's needed). All major distros ship bwrap >= 0.8.0 in current releases.

**No minimum socat version check needed.** Same rationale -- CC only checks PATH presence.

## Architecture Patterns

### Doctor Check Flow (Linux)

```
run_doctor(home)
  |
  +-- check_binary("rightclaw", ...)        // existing
  +-- check_binary("process-compose", ...)  // existing
  +-- check_binary("claude" / "claude-bun") // existing
  +-- [NEW] if OS == "linux":
  |     +-- check_binary("bwrap", ...)      // Fail if missing
  |     +-- check_binary("socat", ...)      // Fail if missing
  |     +-- if bwrap found:
  |           +-- check_bwrap_sandbox()     // smoke test
  +-- check_agent_structure(home)           // existing
```

### Doctor Check Flow (macOS)

```
run_doctor(home)
  |
  +-- check_binary("rightclaw", ...)
  +-- check_binary("process-compose", ...)
  +-- check_binary("claude" / "claude-bun")
  +-- [SKIP bwrap/socat checks]
  +-- check_agent_structure(home)
```

### Smoke Test Pattern

```rust
fn check_bwrap_sandbox() -> DoctorCheck {
    let output = std::process::Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--unshare-net", "--dev", "/dev", "true"])
        .output();

    match output {
        Ok(o) if o.status.success() => DoctorCheck {
            name: "bwrap-sandbox".to_string(),
            status: CheckStatus::Pass,
            detail: "bubblewrap sandbox functional".to_string(),
            fix: None,
        },
        _ => DoctorCheck {
            name: "bwrap-sandbox".to_string(),
            status: CheckStatus::Fail,
            detail: "bubblewrap sandbox test failed (likely AppArmor restriction)".to_string(),
            fix: Some(BWRAP_FIX_GUIDANCE.to_string()),
        },
    }
}
```

### install.sh Pattern

```bash
install_sandbox_deps() {
  if [ "$PLATFORM" = "darwin" ]; then
    ok "macOS uses built-in Seatbelt sandbox (no additional deps needed)"
    return 0
  fi

  info "Installing sandbox dependencies (bubblewrap, socat)..."

  if command -v bwrap >/dev/null 2>&1 && command -v socat >/dev/null 2>&1; then
    ok "bubblewrap and socat already installed"
    return 0
  fi

  if command -v apt-get >/dev/null 2>&1; then
    sudo apt-get install -y bubblewrap socat
  elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y bubblewrap socat
  elif command -v pacman >/dev/null 2>&1; then
    sudo pacman -S --noconfirm bubblewrap socat
  else
    die "No supported package manager found (need apt, dnf, or pacman)"
  fi
}
```

### Anti-Patterns to Avoid

- **Checking sysctl directly instead of smoke test:** The sysctl `kernel.apparmor_restrict_unprivileged_userns` does not exist on non-Ubuntu systems (Fedora, Arch, Debian without AppArmor). A smoke test is universally correct.
- **Using `bwrap --ro-bind / / true` without `--unshare-net`:** This tests basic namespace creation but NOT network namespace + loopback setup, which is what actually fails on restricted systems. CC uses `--unshare-net` internally, so the smoke test must include it.
- **Checking bwrap version:** CC does not check version. Any distro-packaged version works.
- **Recommending global sysctl disable as primary fix:** The AppArmor profile approach is targeted and secure. System-wide sysctl disable weakens overall security.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Binary detection | Custom PATH search | `which::which()` | Already in workspace, handles edge cases (symlinks, permissions) |
| Platform detection | uname parsing in Rust | `std::env::consts::OS` | Compile-time constant, zero cost |
| Package manager detection | Complex distro detection | `command -v apt/dnf/pacman` | Simple, reliable, matches how install.sh already works |

## Common Pitfalls

### Pitfall 1: Smoke Test Too Weak

**What goes wrong:** `bwrap --ro-bind / / true` passes but CC sandbox still fails at runtime because `--unshare-net` triggers loopback RTM_NEWADDR which AppArmor blocks separately.
**Why it happens:** AppArmor can allow basic user namespace creation but block capabilities within the namespace (like configuring loopback). The `--unshare-net` flag triggers `RTM_NEWADDR` for loopback setup which requires CAP_NET_ADMIN inside the namespace.
**How to avoid:** Include `--unshare-net --dev /dev` in the smoke test: `bwrap --ro-bind / / --unshare-net --dev /dev true`. This exercises the same code path CC's sandbox-runtime uses.
**Warning signs:** Doctor reports "pass" but `rightclaw up` fails with sandbox errors.
**Confidence:** HIGH (confirmed by OpenAI Codex issue #12572 and sandbox-runtime issue #74)

### Pitfall 2: Fix Guidance Recommends Sysctl Disable as Primary Fix

**What goes wrong:** Recommending `sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` as the primary fix disables user namespace restrictions system-wide, weakening security for all applications.
**Why it happens:** It is the simplest fix but not the most secure.
**How to avoid:** Primary fix should be the per-application AppArmor profile. Sysctl disable should be mentioned as an alternative/temporary workaround.
**Warning signs:** Users disable security restrictions unnecessarily.
**Recommended fix order:**
1. Create AppArmor profile for bwrap (targeted, secure)
2. System-wide sysctl disable (temporary workaround only)

### Pitfall 3: Smoke Test Hangs or Blocks

**What goes wrong:** `bwrap` command hangs if `true` is not found inside the namespace, or if something blocks.
**Why it happens:** If `/usr/bin/true` is not available at `/true` inside the bind mount, the command fails. Since `--ro-bind / /` mounts the entire root, `true` should be at its normal location.
**How to avoid:** Use `Command::output()` (not `spawn()`), which captures stdout/stderr and waits. Consider adding a timeout (though bwrap should exit fast -- typically <10ms).
**Warning signs:** `rightclaw doctor` appears to hang.

### Pitfall 4: Debian kernel.unprivileged_userns_clone

**What goes wrong:** On older Debian systems (or Debian-derivative distros like Kicksecure), bwrap fails with "No permissions to creating new namespace" due to `kernel.unprivileged_userns_clone=0` (a separate sysctl from AppArmor's).
**Why it happens:** Debian patches the kernel to allow disabling unprivileged user namespaces via this sysctl. Modern Debian packages ship a sysctl conf that enables it, but it may be overridden.
**How to avoid:** The smoke test catches this too (bwrap will fail). The fix guidance should mention both: the AppArmor profile fix (Ubuntu) and the `kernel.unprivileged_userns_clone=1` sysctl (Debian).
**Warning signs:** bwrap error message says "No permissions to creating new namespace" (different from the AppArmor "RTM_NEWADDR" error).

### Pitfall 5: install.sh Removes OpenShell But Existing Users Have It

**What goes wrong:** Nothing -- removing `install_openshell()` from install.sh is clean. Existing openshell installations are not affected (the script does not uninstall anything).
**How to avoid:** No special handling needed. The install script only adds, never removes.

## Code Examples

### Bwrap Smoke Test (Rust)

```rust
// Source: research synthesis from sandbox-runtime source + issue #12572 + issue #74
fn check_bwrap_sandbox() -> DoctorCheck {
    // Must include --unshare-net to test the actual code path CC uses.
    // Basic bwrap without --unshare-net can pass even when CC sandbox would fail.
    let result = std::process::Command::new("bwrap")
        .args([
            "--ro-bind", "/", "/",
            "--unshare-net",
            "--dev", "/dev",
            "true",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();

    match result {
        Ok(output) if output.status.success() => DoctorCheck {
            name: "bwrap-sandbox".to_string(),
            status: CheckStatus::Pass,
            detail: "bubblewrap sandbox functional".to_string(),
            fix: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = if stderr.contains("RTM_NEWADDR") || stderr.contains("Operation not permitted") {
                "AppArmor restricts bubblewrap user namespaces".to_string()
            } else if stderr.contains("No permissions") {
                "unprivileged user namespaces disabled".to_string()
            } else {
                format!("bubblewrap sandbox test failed: {}", stderr.trim())
            };
            DoctorCheck {
                name: "bwrap-sandbox".to_string(),
                status: CheckStatus::Fail,
                detail,
                fix: Some(bwrap_fix_guidance()),
            }
        },
        Err(e) => DoctorCheck {
            name: "bwrap-sandbox".to_string(),
            status: CheckStatus::Fail,
            detail: format!("failed to run bwrap smoke test: {e}"),
            fix: Some(bwrap_fix_guidance()),
        },
    }
}
```

### Fix Guidance String

```rust
fn bwrap_fix_guidance() -> String {
    "\
Create an AppArmor profile for bwrap:

  sudo tee /etc/apparmor.d/bwrap << 'PROFILE'
  abi <abi/4.0>,
  include <tunables/global>

  profile bwrap /usr/bin/bwrap flags=(unconfined) {
    userns,
    include if exists <local/bwrap>
  }
  PROFILE

  sudo apparmor_parser -r /etc/apparmor.d/bwrap

Or temporarily disable the restriction:

  sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0

For persistent fix, add to /etc/sysctl.d/60-bwrap-userns.conf:

  kernel.apparmor_restrict_unprivileged_userns=0

See: https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces"
        .to_string()
}
```

### Platform-Conditional Doctor Checks (Rust)

```rust
pub fn run_doctor(home: &Path) -> Vec<DoctorCheck> {
    let mut checks = vec![
        check_binary("rightclaw", Some("https://github.com/onsails/rightclaw")),
        check_binary("process-compose", Some("https://f1bonacc1.github.io/process-compose/installation/")),
        check_binary("claude", Some("https://docs.anthropic.com/en/docs/claude-code")),
    ];

    // Linux-only sandbox dependency checks
    if std::env::consts::OS == "linux" {
        let bwrap_check = check_binary(
            "bwrap",
            Some("Install bubblewrap: sudo apt install bubblewrap (or dnf/pacman)"),
        );
        let bwrap_found = bwrap_check.status == CheckStatus::Pass;
        checks.push(bwrap_check);

        checks.push(check_binary(
            "socat",
            Some("Install socat: sudo apt install socat (or dnf/pacman)"),
        ));

        // Only run smoke test if bwrap binary was found
        if bwrap_found {
            checks.push(check_bwrap_sandbox());
        }
    }

    checks.extend(check_agent_structure(home));
    checks
}
```

### install.sh: Linux Sandbox Dependencies

```bash
install_sandbox_deps() {
  if [ "$PLATFORM" = "darwin" ]; then
    ok "macOS uses built-in Seatbelt sandbox (no additional deps needed)"
    return 0
  fi

  info "Installing sandbox dependencies..."

  local need_bwrap=false need_socat=false
  command -v bwrap  >/dev/null 2>&1 || need_bwrap=true
  command -v socat  >/dev/null 2>&1 || need_socat=true

  if [ "$need_bwrap" = false ] && [ "$need_socat" = false ]; then
    ok "bubblewrap and socat already installed"
    return 0
  fi

  local pkgs=""
  [ "$need_bwrap" = true ] && pkgs="bubblewrap"
  [ "$need_socat" = true ] && pkgs="$pkgs socat"

  if command -v apt-get >/dev/null 2>&1; then
    sudo apt-get install -y $pkgs
  elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y $pkgs
  elif command -v pacman >/dev/null 2>&1; then
    sudo pacman -S --noconfirm $pkgs
  else
    die "No supported package manager found (need apt, dnf, or pacman).
    Install manually: bubblewrap socat"
  fi

  ok "sandbox dependencies installed"
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| OpenShell sandbox (external) | CC native sandbox (bubblewrap/Seatbelt) | v2.0 (2026-03) | No external sandbox runtime needed |
| Doctor checks openshell | Doctor checks bwrap + socat (Linux only) | Phase 5/7 | Different dependency set per platform |
| No AppArmor detection | Smoke test detects namespace restriction | Phase 7 | Users get actionable fix guidance |
| install.sh installs openshell | install.sh installs bubblewrap + socat | Phase 7 | Simpler install, fewer external deps |

## Open Questions

1. **Smoke test: `true` binary location**
   - What we know: `--ro-bind / /` mounts entire root, so `/usr/bin/true` (or `/bin/true`) is at its normal path.
   - What's unclear: On NixOS, `true` may be at `/run/current-system/sw/bin/true` with `/usr/bin/true` not existing. The command `true` may need to be specified as an absolute path.
   - Recommendation: Use `"/bin/true"` explicitly (should exist on all non-NixOS Linux). For NixOS, the smoke test may need `which true` first. LOW priority -- NixOS users typically know their way around this.

2. **Fix guidance: AppArmor profile path**
   - What we know: The profile goes in `/etc/apparmor.d/bwrap` with `userns,` permission. Works on Ubuntu 24.04.
   - What's unclear: Whether the `abi <abi/4.0>` line is correct for all Ubuntu/Debian versions. Older AppArmor versions may not support ABI 4.0.
   - Recommendation: Use `abi <abi/4.0>` as shown in official Ubuntu docs and the Codex issue. Ubuntu 24.04+ has AppArmor 4.0. Older distros don't have the restriction, so they don't need the profile.

3. **Smoke test timeout**
   - What we know: bwrap typically completes in <10ms.
   - What's unclear: Whether there are edge cases where it hangs (e.g., broken namespace cleanup, deadlocked kernel).
   - Recommendation: Don't add explicit timeout for now. `Command::output()` will block but bwrap is designed to exit quickly. If it becomes an issue, use `tokio::process::Command` with timeout in a future fix.

## Sources

### Primary (HIGH confidence)
- [Claude Code Sandboxing Docs](https://code.claude.com/docs/en/sandboxing) -- official prerequisites, install commands for apt/dnf
- [sandbox-runtime source: linux-sandbox-utils.ts](https://github.com/anthropic-experimental/sandbox-runtime) -- confirms CC checks `whichSync('bwrap')` only, no version check, no smoke test
- [sandbox-runtime issue #74](https://github.com/anthropic-experimental/sandbox-runtime/issues/74) -- confirmed bwrap AppArmor failure on Ubuntu 24.04+
- [Ubuntu blog: Restricted unprivileged user namespaces](https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces) -- official AppArmor restriction docs
- [OpenAI Codex issue #12572](https://github.com/openai/codex/issues/12572) -- confirms basic bwrap works but --unshare-net fails, same fix applies

### Secondary (MEDIUM confidence)
- [ArchWiki: Bubblewrap](https://wiki.archlinux.org/title/Bubblewrap) -- AppArmor profile pattern, package info
- [Julia Evans: Notes on bubblewrap](https://jvns.ca/blog/2022/06/28/some-notes-on-bubblewrap/) -- bwrap invocation patterns and testing
- [bubblewrap GitHub](https://github.com/containers/bubblewrap) -- version history, capabilities documentation

### Tertiary (LOW confidence)
- [Qualys: Three bypasses of Ubuntu userns restrictions](https://www.qualys.com/2025/three-bypasses-of-Ubuntu-unprivileged-user-namespace-restrictions.txt) -- security context for why per-app profiles are safer than global sysctl disable

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- no new dependencies, existing crate usage verified
- Architecture: HIGH -- extends well-understood existing patterns (check_binary, DoctorCheck)
- Pitfalls: HIGH -- confirmed via multiple upstream bug reports with reproducer steps
- Smoke test design: HIGH -- validated against sandbox-runtime source and real-world failure reports

**Research date:** 2026-03-24
**Valid until:** 2026-06-24 (stable domain -- AppArmor restrictions unlikely to change in 90 days)
