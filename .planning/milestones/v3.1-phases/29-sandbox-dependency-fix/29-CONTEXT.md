# Phase 29: Sandbox Dependency Fix - Context

**Gathered:** 2026-04-02
**Status:** Ready for planning

<domain>
## Phase Boundary

CC sandbox actually engages in nix/devenv environments. All four fix sites land atomically to avoid the failIfUnavailable restart-loop trap. No new features, no new CLI commands — purely fixing existing sandbox infrastructure.

</domain>

<decisions>
## Implementation Decisions

### rg Path Resolution
- **D-01:** `generate_settings()` gains a new parameter `rg_path: Option<PathBuf>`. Caller (`cmd_up`) resolves `which::which("rg")` once and passes to each agent's settings generation. `settings.rs` stays pure (no IO).
- **D-02:** When rg is not found in PATH, `cmd_up` logs `tracing::warn` and passes `None`. `settings.json` gets no `sandbox.ripgrep.command` field. Agent will fail at CC level because `failIfUnavailable: true` prevents silent degradation — this is the desired behavior (no agent runs without sandbox).
- **D-03:** `sandbox.ripgrep.command` is set to the absolute path returned by `which::which("rg")` — resolved at `rightclaw up` time, not a relative or store path.

### failIfUnavailable
- **D-04:** `sandbox.failIfUnavailable: true` is unconditionally present in every generated `settings.json`, regardless of `--no-sandbox` flag. When sandbox is disabled, CC ignores it (inert). Zero branching in codegen.

### USE_BUILTIN_RIPGREP
- **D-05:** `USE_BUILTIN_RIPGREP` env var changed from `"1"` to `"0"` in both `worker.rs` and `cron.rs`. Value `"1"` was a bug — forces CC to use its vendored rg (broken in nix). Value `"0"` forces system rg from PATH.
- **D-06:** Comment updated to explain the counterintuitive naming: `"0"` = use system rg, `"1"` = use CC bundled rg.

### devenv.nix
- **D-07:** `pkgs.ripgrep` added to the packages list in `devenv.nix` (unconditional, not Linux-only). Ensures `rg` is in PATH for all development sessions.

### Atomicity
- **D-08:** All four fix sites (settings.rs, worker.rs, cron.rs, devenv.nix) land in a single atomic commit. Research warns that enabling `failIfUnavailable` before fixing rg path causes a restart loop — no intermediate broken state allowed.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Sandbox Architecture
- `.planning/research/STACK.md` — CC internal USE_BUILTIN_RIPGREP semantics, A_() truthiness, sandbox.ripgrep Zod schema
- `.planning/research/ARCHITECTURE.md` — Fix site locations, CC sandbox dependency check flow
- `.planning/research/PITFALLS.md` — 7 pitfalls including USE_BUILTIN_RIPGREP inversion, failIfUnavailable trap
- `.planning/research/SUMMARY.md` — Consolidated research summary with execution order and pitfall cross-refs

### Requirements
- `.planning/REQUIREMENTS.md` — SBOX-01 through SBOX-04

### Existing Code
- `crates/rightclaw/src/codegen/settings.rs` — `generate_settings()` function to modify
- `crates/bot/src/telegram/worker.rs` line 399 — `USE_BUILTIN_RIPGREP=1` bug site
- `crates/bot/src/cron.rs` line 227 — `USE_BUILTIN_RIPGREP=1` bug site
- `devenv.nix` — packages list to extend

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `which` crate already in dependencies — no new dep needed for rg resolution
- `generate_settings()` already takes `agent: &AgentDef`, `no_sandbox: bool`, `host_home: &Path` — adding `rg_path: Option<PathBuf>` is a clean extension
- `settings_tests.rs` exists with existing test infrastructure

### Established Patterns
- `cmd.env("USE_BUILTIN_RIPGREP", "1")` pattern in both worker.rs:399 and cron.rs:227 — identical fix in both
- Settings JSON construction via `serde_json::json!` macro — ripgrep and failIfUnavailable fields are simple additions to the existing `"sandbox"` object

### Integration Points
- `cmd_up` in `main.rs` or `up.rs` calls `generate_settings()` per agent — rg resolution goes here (once, before the loop)
- `worker.rs` `invoke_cc()` function builds CC subprocess command — env var change site
- `cron.rs` CC subprocess builder — env var change site

</code_context>

<specifics>
## Specific Ideas

- User explicitly wants: if rg is missing, agent MUST fail (not silently run without sandbox). The combination of warn-on-up + failIfUnavailable:true achieves this.
- USE_BUILTIN_RIPGREP is an undocumented CC internal env var — add a comment with CC issue link for future maintainers.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 29-sandbox-dependency-fix*
*Context gathered: 2026-04-02*
