# Phase 40: Wire Cloudflared into Process-Compose — Research

**Researched:** 2026-04-05
**Domain:** Rust codegen / minijinja template / process-compose YAML
**Confidence:** HIGH

## Summary

This phase is a small, surgical wiring task. All the hard work (cloudflared config generation,
wrapper script writing, credentials loading) was done in Phases 38 and 39. The only gap is that
`cloudflared_script_path` is computed in `cmd_up` but intentionally suppressed with `let _ =` and
never passed to `generate_process_compose`. This phase:

1. Adds a `cloudflared_script: Option<&Path>` parameter to `generate_process_compose`.
2. Adds a conditional cloudflared process block to `templates/process-compose.yaml.j2`.
3. Removes the `let _ = cloudflared_script_path;` placeholder in `cmd_up` and passes the value.
4. Adds a `which::which("cloudflared")` pre-flight check in `cmd_up` when `TunnelConfig` is present.
5. Adds two tests to `process_compose_tests.rs`: with and without cloudflared script.

No new dependencies are needed. All patterns already exist in the codebase.

**Primary recommendation:** Follow the decisions in CONTEXT.md verbatim — they are already concrete
and complete. The planner needs only to sequence the four file changes as a single plan.

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** Process name: `cloudflared` (single entry, no suffix)
- **D-02:** Restart policy: `on_failure`
- **D-03:** No `depends_on` from bot agents to cloudflared
- **D-04:** Shutdown: signal 15, timeout_seconds 30 — same as bot agents
- **D-05:** Backoff/max_restarts: hardcoded defaults (`backoff_seconds: 5`, `max_restarts: 10`)
- **D-06:** Conditional block via Jinja2 `{% if cloudflared %}...{% endif %}` in template
- **D-07:** `cloudflared.command` = absolute path to `cloudflared-start.sh`
- **D-08:** `generate_process_compose` gains `cloudflared_script: Option<&Path>` parameter
- **D-09:** Remove `let _ = cloudflared_script_path;`, pass `cloudflared_script_path.as_deref()`
- **D-10:** Fail fast in `cmd_up` if TunnelConfig present but `cloudflared` not in PATH — use `which::which("cloudflared")` before PC config generation

### Claude's Discretion

- Working directory for the cloudflared process
- Exact process-compose availability fields structure in the template

### Deferred Ideas (OUT OF SCOPE)

None.
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| TUNL-02 | Merged into TUNL-01: `rightclaw up` spawns cloudflared as a persistent process-compose entry | D-01..D-10 in CONTEXT.md fully specify the implementation; code in main.rs:686-733 already generates the script; gap is template wiring and function signature |
</phase_requirements>

---

## Standard Stack

No new dependencies. All tools already present in the workspace.

| Library | Purpose | Already in Use |
|---------|---------|----------------|
| minijinja | Jinja2 template rendering for process-compose.yaml | `[VERIFIED: crates/rightclaw/src/codegen/process_compose.rs]` |
| which | PATH binary lookup | `[VERIFIED: crates/rightclaw-cli/src/main.rs:269, crates/bot/src/cron.rs:187]` |
| miette | Error reporting | `[VERIFIED: throughout codebase]` |

## Architecture Patterns

### Existing Template Structure

The template at `templates/process-compose.yaml.j2` loops over `agents` context variable (a `Vec<BotProcessAgent>`). Conditional content already uses `{% if %}` for token variants. `[VERIFIED: templates/process-compose.yaml.j2]`

### Pattern: Optional Template Context Variable

The existing template uses `{% if agent.token_inline %}...{% else %}...{% endif %}` to branch on
optional values. The cloudflared block follows the same idiom at the top level:

```jinja2
{% if cloudflared %}
  cloudflared:
    command: "{{ cloudflared.command }}"
    working_dir: "{{ cloudflared.working_dir }}"
    availability:
      restart: "on_failure"
      backoff_seconds: 5
      max_restarts: 10
    shutdown:
      signal: 15
      timeout_seconds: 30
{% endif %}
```

`[VERIFIED: pattern derived from existing token_inline conditional in template]`

The `cloudflared` context variable is either absent/falsy (no TunnelConfig) or a struct/map with
`command` and `working_dir` fields.

### Pattern: Additive Function Signature

`generate_process_compose` is called in exactly one place: `main.rs:746`. Adding a trailing
`cloudflared_script: Option<&Path>` parameter is a straightforward additive change. All existing
callers (tests) pass `None` or are updated explicitly.

`[VERIFIED: grep output — single call site in main.rs:746, tests import via crate::codegen::generate_process_compose]`

### Pattern: which::which Pre-flight

Existing pattern from `cmd_init` (line 269):

```rust
let cf_bin = which::which("cloudflared")
    .map_err(|_| miette::miette!("cloudflared not found in PATH — install it first"))?;
```

`cmd_up` reuses the identical pattern before PC config generation, guarded by `if tunnel_cfg.is_some()`.

`[VERIFIED: crates/rightclaw-cli/src/main.rs:269]`

### Working Directory for Cloudflared Process

**Claude's discretion.** The script is at `~/.rightclaw/scripts/cloudflared-start.sh`. The most
natural working directory is `~/.rightclaw/` (the home dir). The script uses absolute paths
throughout (`route dns <uuid> <hostname>`, `--config <abs_path>`), so working_dir is irrelevant to
correctness. Recommendation: use `home.display().to_string()` — the same directory already used as
the rightclaw home in `cmd_up`.

### Template Context Struct (Discretion)

Two options for passing cloudflared data to the template:

**Option A: dedicated `CloudflaredContext` struct** — explicit, typed, Serialize-derives cleanly.
**Option B: pass script path as a string in `context!`** — simpler, fewer types.

Since only two fields are needed (`command` and `working_dir`), a small anonymous struct or inline
struct works. Recommendation: use a minimal `#[derive(Serialize)] struct CloudflaredEntry` with
`command: String` and `working_dir: String`, constructed in `generate_process_compose` from the
`Option<&Path>` argument.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead |
|---------|-------------|-------------|
| Optional YAML block | String concatenation with `if` guards | minijinja `{% if cloudflared %}` block |
| Binary PATH check | Manual `std::env::var("PATH").split(':')` scan | `which::which("cloudflared")` |

## Common Pitfalls

### Pitfall 1: Test Signature Mismatch After Parameter Addition

**What goes wrong:** Existing tests call `generate_process_compose(&agents, exe, false)` — adding a
required parameter breaks all 18 existing tests.

**How to avoid:** Add `cloudflared_script: Option<&Path>` as the last parameter. Update all test
call sites to pass `None`. The 2 new tests pass `Some(Path::new(...))` for the cloudflared case.

### Pitfall 2: Template Renders Cloudflared When Script Is None

**What goes wrong:** If `cloudflared` is passed as `None` to the template context as a Rust
`Option`, minijinja may not treat it as falsy.

**How to avoid:** Only insert the `cloudflared` key into the template context when the script path
is `Some`. When `None`, omit it from `context!{}` entirely so the variable is undefined/falsy in
the template. Alternatively, pass `Option<CloudflaredEntry>` — minijinja serializes `None` as
`null` which is falsy in Jinja2.

**Verification:** Test the `None` path explicitly: assert cloudflared process block is absent from
output.

### Pitfall 3: Pre-flight Check Placed After Script Generation

**What goes wrong:** `which::which("cloudflared")` fails but the script has already been written to
disk, leaving a stale artifact.

**How to avoid:** Place the `which::which` check before cloudflared script generation (before
line 686 in the current flow), not after. The check is gated on `TunnelConfig` presence.

### Pitfall 4: Script Path Not Absolute in Template

**What goes wrong:** If the script path is passed as relative, process-compose can't find it when
spawning.

**How to avoid:** The script path is always built as `scripts_dir.join("cloudflared-start.sh")`
where `scripts_dir = home.join("scripts")` — already absolute. Verify with `assert!(path.is_absolute())` in test.

## Code Examples

### Minimal CloudflaredEntry Struct

```rust
// In crates/rightclaw/src/codegen/process_compose.rs
#[derive(Debug, Serialize)]
struct CloudflaredEntry {
    command: String,
    working_dir: String,
}
```

`[ASSUMED: derived from existing BotProcessAgent pattern in same file]`

### Updated Function Signature

```rust
pub fn generate_process_compose(
    agents: &[AgentDef],
    exe_path: &Path,
    debug: bool,
    cloudflared_script: Option<&Path>,
) -> miette::Result<String>
```

`[ASSUMED: derived from D-08 decision and existing signature]`

### Template Context Construction

```rust
let cf_entry = cloudflared_script.map(|script| CloudflaredEntry {
    command: script.display().to_string(),
    working_dir: script
        .parent()
        .unwrap_or(script)
        .parent()          // scripts/ -> home/
        .unwrap_or(script)
        .display()
        .to_string(),
});

tmpl.render(context! {
    agents => bot_agents,
    cloudflared => cf_entry,
})
```

`[ASSUMED: based on minijinja context! macro pattern and existing code]`

### Pre-flight Check in cmd_up (main.rs)

```rust
// Before cloudflared script generation (currently line ~686):
if global_cfg.tunnel.is_some() {
    which::which("cloudflared")
        .map_err(|_| miette::miette!(
            "TunnelConfig is present but `cloudflared` is not in PATH — install cloudflared first"
        ))?;
}
```

`[VERIFIED: pattern matches cmd_init usage at line 269]`

### Removing the Suppression in cmd_up

```rust
// Remove:
let _ = cloudflared_script_path; // used by process-compose template in future phases

// Replace call at line 746 with:
let pc_config = rightclaw::codegen::generate_process_compose(
    &agents, &self_exe, debug, cloudflared_script_path.as_deref()
)?;
```

`[VERIFIED: suppression at main.rs:733, call site at main.rs:746]`

## State of the Art

No state-of-the-art changes needed. This is a gap closure, not a new capability introduction.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `CloudflaredEntry` struct with `command` + `working_dir` is the right template context shape | Code Examples | Low — struct can be reshaped without API breakage; template and struct must match |
| A2 | Working directory for cloudflared process = `~/.rightclaw/` (home dir) | Architecture Patterns | Low — cloudflared script uses absolute paths throughout; any valid dir works |
| A3 | minijinja serializes `None` as falsy `null` in template | Pitfall 2 | Medium — if wrong, cloudflared block renders even when no tunnel configured; caught by test |

## Open Questions

1. **Working directory in template**
   - What we know: script uses absolute paths, so correctness is not affected
   - What's unclear: whether process-compose logs show the working_dir; cosmetic only
   - Recommendation: use `home` dir (parent of `scripts/`) — consistent with script location

2. **should the pre-flight check return early vs error?**
   - What we know: D-10 says "fails fast with a clear error"
   - Recommendation: propagate as `Err` with `?` — fail the whole `cmd_up` call

## Environment Availability

Step 2.6: SKIPPED — phase is pure Rust code/template changes; external dependency (cloudflared binary) is checked at runtime by the new pre-flight added in this phase, not at build time.

## Sources

### Primary (HIGH confidence)
- `[VERIFIED: crates/rightclaw/src/codegen/process_compose.rs]` — current function signature, BotProcessAgent pattern, minijinja usage
- `[VERIFIED: templates/process-compose.yaml.j2]` — existing template structure, conditional pattern
- `[VERIFIED: crates/rightclaw-cli/src/main.rs:686-746]` — cloudflared_script_path generation and suppression, call site
- `[VERIFIED: crates/rightclaw/src/codegen/process_compose_tests.rs]` — 18 existing tests, all call generate_process_compose with 3 args
- `[VERIFIED: crates/rightclaw-cli/src/main.rs:269]` — which::which("cloudflared") pattern in cmd_init
- `[VERIFIED: .planning/phases/40-wire-cloudflared-into-process-compose/40-CONTEXT.md]` — all locked decisions D-01..D-10

### Secondary (MEDIUM confidence)
- `[VERIFIED: .planning/phases/39-cloudflared-auto-tunnel/39-UAT.md]` — UAT gap description and root cause analysis

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new deps, all tools verified in codebase
- Architecture: HIGH — patterns directly observed in source files
- Pitfalls: HIGH — derived from code inspection, not speculation

**Research date:** 2026-04-05
**Valid until:** N/A — internal codebase research, not library-version-sensitive
