# Phase 30: Doctor Diagnostics - Research

**Researched:** 2026-04-02
**Domain:** Rust CLI diagnostic checks — PATH resolution, JSON file validation
**Confidence:** HIGH

## Summary

Phase 30 adds two targeted checks to the existing `run_doctor()` function in `crates/rightclaw/src/doctor.rs`. The codebase already contains all the primitives needed: `check_binary()` for PATH checks, `serde_json::Value` parsing from `check_managed_settings()`, and agent-directory traversal from `check_agent_structure()`. This is purely additive work — no new dependencies, no API changes, no coupling to codegen.

DOC-01 (rg PATH check, Linux-only) reuses `which::which("rg")` — identical to how `cmd_up` resolves the path. DOC-02 (settings.json ripgrep.command validation) iterates agent dirs, reads `.claude/settings.json` from disk, and checks `sandbox.ripgrep.command` points to a real executable. Both emit `CheckStatus::Warn` per the phase decisions.

**Primary recommendation:** Add two private functions `check_rg_in_path()` and `check_ripgrep_in_settings(home)` following the exact same shape as existing doctor check functions, then insert calls in `run_doctor()` after the bwrap/socat block.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Use `which::which("rg")` in the current environment — same approach as `cmd_up`. Doctor and `rightclaw up` run from the same shell session, so the PATH is identical. No subprocess spawning or process-compose.yaml parsing needed.
- **D-02:** Read each agent's `.claude/settings.json` from disk only. Do not call `generate_settings()` in the doctor path — no codegen coupling. If the file doesn't exist, emit Warn with "run `rightclaw up` first to generate settings".
- **D-03:** New checks run only in `run_doctor()`. No pre-flight checks in `cmd_up` — it already resolves `rg_path` inline and fails if missing. Doctor is the diagnostic tool, `up` is the launcher.
- **D-04:** Both checks emit `Warn` severity per DOC-01/DOC-02 requirements. Missing rg = Warn (Linux only). Invalid/absent `sandbox.ripgrep.command` in settings.json = Warn. Doctor remains non-blocking.

### Claude's Discretion
- Check ordering within `run_doctor()` — place new checks logically near existing sandbox checks (after bwrap/socat, before agent structure)
- Fix hint wording for both new checks
- Whether to validate `sandbox.ripgrep.args` field or just `command`

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| DOC-01 | `rightclaw doctor` checks ripgrep availability in the PATH that agent processes will inherit (not just the developer's shell PATH) | `which::which("rg")` already in deps; mirrors `cmd_up` pattern at main.rs:379; Linux-only gate pattern already used for bwrap/socat |
| DOC-02 | `rightclaw doctor` validates that generated settings.json contains correct `sandbox.ripgrep.command` pointing to an existing executable | `serde_json::Value` parsing pattern exists in `check_managed_settings()`; settings.json path is `<agent_dir>/.claude/settings.json`; agent dir traversal pattern exists in `check_agent_structure()` |
</phase_requirements>

## Standard Stack

### Core (no new dependencies needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `which` | already in Cargo.toml | Resolve `rg` in PATH | Same crate used by `cmd_up`; already a workspace dep |
| `serde_json` | already in Cargo.toml | Parse settings.json | Already used in `check_managed_settings()`; no new dep |
| `std::path::Path::exists()` / `std::fs::metadata()` | stdlib | Verify executable exists at path | Standard — no external dep needed |

**Installation:** None required. All needed crates are already in the workspace.

## Architecture Patterns

### Pattern 1: Linux-only check gate (DOC-01)
**What:** Guard rg PATH check with `std::env::consts::OS == "linux"` identical to bwrap/socat block.
**When to use:** DOC-01 specifies Linux-only Warn. macOS has Seatbelt built-in and doesn't need rg check enforcement.
**Example:**
```rust
// From doctor.rs lines 61-78 — existing pattern
if std::env::consts::OS == "linux" {
    // ...bwrap checks...
    checks.push(check_binary("socat", Some("Install socat: ...")));

    // DOC-01 goes here, same gate
    let rg_check = check_rg_in_path();
    checks.push(rg_check);
}
```

### Pattern 2: which::which() for binary resolution (DOC-01)
**What:** Call `which::which("rg")`, emit Warn (not Fail) on Err.
**When to use:** DOC-01 requires Warn severity. Unlike core binaries (rightclaw/process-compose/claude) which emit Fail, rg is a sandbox dependency — non-fatal at doctor level.

```rust
fn check_rg_in_path() -> DoctorCheck {
    match which::which("rg") {
        Ok(path) => DoctorCheck {
            name: "rg".to_string(),
            status: CheckStatus::Pass,
            detail: path.display().to_string(),
            fix: None,
        },
        Err(_) => DoctorCheck {
            name: "rg".to_string(),
            status: CheckStatus::Warn,
            detail: "ripgrep not found in PATH — CC sandbox will fail at agent launch".to_string(),
            fix: Some(
                "Install ripgrep: nix profile install nixpkgs#ripgrep / apt install ripgrep"
                    .to_string(),
            ),
        },
    }
}
```

### Pattern 3: Per-agent JSON file validation (DOC-02)
**What:** Iterate agents dir (same pattern as `check_agent_structure`), read `.claude/settings.json`, parse with `serde_json`, navigate to `sandbox.ripgrep.command`, verify the path exists and is executable.
**When to use:** DOC-02 needs per-agent granularity — each agent gets its own check entry.

```rust
fn check_ripgrep_in_settings(home: &Path) -> Vec<DoctorCheck> {
    let agents_dir = home.join("agents");
    // ... same iteration as check_agent_structure() ...

    for agent_dir in agent_dirs {
        let settings_path = agent_dir.join(".claude").join("settings.json");

        if !settings_path.exists() {
            checks.push(DoctorCheck {
                name: format!("sandbox-rg/{agent_name}"),
                status: CheckStatus::Warn,
                detail: "settings.json not found — sandbox rg config unknown".to_string(),
                fix: Some("Run `rightclaw up` to generate agent settings".to_string()),
            });
            continue;
        }

        let content = std::fs::read_to_string(&settings_path)?;
        let parsed: serde_json::Value = serde_json::from_str(&content)?;

        match parsed["sandbox"]["ripgrep"]["command"].as_str() {
            None => {
                // Field absent or null
                checks.push(warn_missing_rg_command(agent_name));
            }
            Some(cmd_path) => {
                if std::path::Path::new(cmd_path).is_file() {
                    checks.push(pass_rg_command(agent_name, cmd_path));
                } else {
                    checks.push(warn_invalid_rg_command(agent_name, cmd_path));
                }
            }
        }
    }
    checks
}
```

### Pattern 4: Warn override (existing sqlite3 pattern)
**What:** Call `check_binary()` (returns Fail when absent) then override status to Warn.
**When to use:** DOC-01 — rg should never be Fail. This avoids reimplementing the find-in-PATH logic.

```rust
// Alternative to pattern 2 — reuse check_binary + override (lines 88-98 of doctor.rs)
let raw = check_binary("rg", Some("Install ripgrep: ..."));
checks.push(DoctorCheck {
    status: if raw.status == CheckStatus::Pass {
        CheckStatus::Pass
    } else {
        CheckStatus::Warn
    },
    ..raw
});
```
Note: Pattern 2 (direct `which::which`) and Pattern 4 (check_binary + override) are both valid. Pattern 4 is DRYer; Pattern 2 allows custom detail message. Planner chooses.

### Recommended Project Structure
No structural changes. Both new functions go in `crates/rightclaw/src/doctor.rs`.

### Anti-Patterns to Avoid
- **Calling `generate_settings()` in doctor:** Locked by D-02. Codegen coupling means doctor output diverges from what's actually written to disk. Read the file, don't regenerate.
- **Using `std::fs::metadata()` to check executability:** Prefer `Path::is_file()` — sufficient for the use case. If the path exists as a regular file, it's installed. Executability bit checking adds complexity with no practical benefit (rg from nix store is always executable).
- **Emitting Fail for missing rg:** D-04 locks Warn severity. Doctor must remain non-blocking.
- **Platform-gating DOC-02 to Linux-only:** settings.json ripgrep validation is cross-platform — macOS agents also have settings.json. Only DOC-01 (PATH check) is Linux-only.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Binary path resolution | Custom PATH parsing | `which::which()` | Already in deps; handles PATH splits, symlinks, permissions |
| JSON navigation | Manual string parsing | `serde_json::Value` with `["key"]` indexing | Already used in `check_managed_settings()`; returns `Value::Null` for missing keys (safe to `.as_str()`) |
| Agent directory iteration | Custom walker | Same `std::fs::read_dir` + `.flatten()` + `.is_dir()` pattern as `check_agent_structure()` | Already established, tested pattern |

## Common Pitfalls

### Pitfall 1: serde_json null vs absent field
**What goes wrong:** `parsed["sandbox"]["ripgrep"]["command"]` returns `Value::Null` both when the key is absent AND when it's explicitly `null`. `.as_str()` returns `None` for both — this is correct behavior for the validation (both cases = missing config).
**Why it happens:** JSON path indexing with `[]` on missing keys produces `Value::Null`, not an error.
**How to avoid:** Use `.as_str()` which gracefully returns `None` for null. No special handling needed.
**Warning signs:** Using `.get("command").is_none()` — misses the null case.

### Pitfall 2: settings.json parse failure handling
**What goes wrong:** If settings.json is corrupted/invalid JSON, `serde_json::from_str` returns Err. Propagating this error would panic in a non-Result function.
**Why it happens:** Doctor check functions return `Vec<DoctorCheck>`, not `Result`. Errors must be converted to Warn checks.
**How to avoid:** Match on the parse result: `Err(_) => push Warn("settings.json could not be parsed")`.

### Pitfall 3: Cross-platform settings.json check scope
**What goes wrong:** Gating DOC-02 to Linux-only (mistakenly following the DOC-01 Linux gate).
**Why it happens:** DOC-01 and DOC-02 are added near the bwrap/socat block which is Linux-only.
**How to avoid:** DOC-02 goes OUTSIDE the `if std::env::consts::OS == "linux"` block. Settings validation is cross-platform.

### Pitfall 4: Check name collision with existing checks
**What goes wrong:** Naming the rg check `"rg"` when there's already a `check_binary("rg", ...)` equivalent somewhere.
**Why it happens:** Doctor accumulates all checks in a Vec — no deduplication.
**How to avoid:** Check `run_doctor()` exhaustively before adding. Currently: rightclaw, process-compose, claude, bwrap, socat, bwrap-sandbox, agents/*, telegram-webhook/*, sqlite3, managed-settings. No `rg` check exists yet.

## Code Examples

### DOC-01: rg PATH check (Linux-only Warn)
```rust
// Pattern: check_binary + status override (mirrors sqlite3 pattern at doctor.rs:88-98)
if std::env::consts::OS == "linux" {
    // ... existing bwrap/socat checks ...

    let raw_rg = check_binary(
        "rg",
        Some("Install ripgrep: nix profile install nixpkgs#ripgrep / apt install ripgrep / brew install ripgrep"),
    );
    checks.push(DoctorCheck {
        status: if raw_rg.status == CheckStatus::Pass {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        ..raw_rg
    });
}
```

### DOC-02: settings.json ripgrep.command validation
```rust
fn check_ripgrep_in_settings(home: &Path) -> Vec<DoctorCheck> {
    let agents_dir = home.join("agents");
    if !agents_dir.exists() {
        return vec![];
    }

    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut checks = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let settings_path = path.join(".claude").join("settings.json");
        if !settings_path.exists() {
            checks.push(DoctorCheck {
                name: format!("sandbox-rg/{name}"),
                status: CheckStatus::Warn,
                detail: "settings.json not found".to_string(),
                fix: Some("Run `rightclaw up` to generate agent settings".to_string()),
            });
            continue;
        }

        let content = match std::fs::read_to_string(&settings_path) {
            Ok(c) => c,
            Err(e) => {
                checks.push(DoctorCheck {
                    name: format!("sandbox-rg/{name}"),
                    status: CheckStatus::Warn,
                    detail: format!("cannot read settings.json: {e}"),
                    fix: None,
                });
                continue;
            }
        };

        let parsed: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => {
                checks.push(DoctorCheck {
                    name: format!("sandbox-rg/{name}"),
                    status: CheckStatus::Warn,
                    detail: "settings.json is not valid JSON".to_string(),
                    fix: Some("Run `rightclaw up` to regenerate settings".to_string()),
                });
                continue;
            }
        };

        match parsed["sandbox"]["ripgrep"]["command"].as_str() {
            None => {
                checks.push(DoctorCheck {
                    name: format!("sandbox-rg/{name}"),
                    status: CheckStatus::Warn,
                    detail: "sandbox.ripgrep.command absent — CC sandbox will fail at launch".to_string(),
                    fix: Some(
                        "Ensure ripgrep is installed, then run `rightclaw up` to regenerate settings"
                            .to_string(),
                    ),
                });
            }
            Some(cmd) => {
                if std::path::Path::new(cmd).is_file() {
                    checks.push(DoctorCheck {
                        name: format!("sandbox-rg/{name}"),
                        status: CheckStatus::Pass,
                        detail: cmd.to_string(),
                        fix: None,
                    });
                } else {
                    checks.push(DoctorCheck {
                        name: format!("sandbox-rg/{name}"),
                        status: CheckStatus::Warn,
                        detail: format!("sandbox.ripgrep.command points to non-existent path: {cmd}"),
                        fix: Some(
                            "Reinstall ripgrep and run `rightclaw up` to regenerate settings"
                                .to_string(),
                        ),
                    });
                }
            }
        }
    }

    checks
}
```

### run_doctor() insertion points
```rust
pub fn run_doctor(home: &Path) -> Vec<DoctorCheck> {
    // ... existing binary checks ...

    if std::env::consts::OS == "linux" {
        // ... existing bwrap_check, socat, bwrap-sandbox ...

        // DOC-01: rg PATH check (after socat, before bwrap-sandbox or after)
        let raw_rg = check_binary("rg", Some("Install ripgrep: ..."));
        checks.push(DoctorCheck {
            status: if raw_rg.status == CheckStatus::Pass { CheckStatus::Pass } else { CheckStatus::Warn },
            ..raw_rg
        });
    }

    // Agent structure checks
    checks.extend(check_agent_structure(home));

    // ... telegram webhook checks, sqlite3, managed-settings ...

    // DOC-02: settings.json ripgrep.command validation (cross-platform, after agent structure)
    checks.extend(check_ripgrep_in_settings(home));

    checks
}
```

## State of the Art

This is purely internal implementation — no external API or ecosystem state changes relevant.

## Open Questions

1. **`sandbox.ripgrep.args` validation (Claude's Discretion)**
   - What we know: `generate_settings()` always writes `"args": []`. The field is an array.
   - What's unclear: Is there a real failure mode from wrong `args`? The command field is the critical part.
   - Recommendation: Skip `args` validation. DOC-02 requirement says "pointing to an existing executable" — only `command` matters. Keep the check minimal.

2. **Check name for DOC-02 entries**
   - What we know: Existing names follow patterns like `agents/{name}/`, `telegram-webhook/{name}`, `bwrap-sandbox`.
   - What's unclear: Whether `sandbox-rg/{name}` or `rg-config/{name}` reads better in doctor output.
   - Recommendation: Use `sandbox-rg/{name}` — mirrors the `telegram-webhook/{name}` prefix pattern and makes the sandbox context clear.

## Environment Availability

Step 2.6: SKIPPED — no external dependencies. Both checks use `which` (already in Cargo.toml) and stdlib `std::fs`/`std::path`. No new tools or services required.

## Sources

### Primary (HIGH confidence)
- `crates/rightclaw/src/doctor.rs` — All existing check patterns inspected directly
- `crates/rightclaw/src/codegen/settings.rs` — `rg_path` injection logic, `sandbox.ripgrep.command` key name confirmed
- `crates/rightclaw-cli/src/main.rs:379` — `which::which("rg")` pattern in `cmd_up` confirmed
- `.planning/phases/30-doctor-diagnostics/30-CONTEXT.md` — All decisions D-01 through D-04 from discussion session

### Secondary (MEDIUM confidence)
- `crates/rightclaw/src/codegen/settings_tests.rs` — Confirms `settings["sandbox"]["ripgrep"]["command"]` JSON path (lines 290-305)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new deps, all existing code inspected
- Architecture: HIGH — exact patterns identified with line references
- Pitfalls: HIGH — derived from direct code inspection, not speculation

**Research date:** 2026-04-02
**Valid until:** 90 days (stable internal Rust code — no external dependencies)
