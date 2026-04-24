# Upgrade & Migration Model — Design Spec

**Date:** 2026-04-24
**Status:** Design approved, implementation pending
**Scope:** Codify how codegen changes propagate to already-deployed RightClaw agents, and apply the model to fix the `tls: terminate` deprecation in OpenShell policies.

## Context

Two forces motivate this spec:

1. **Concrete bug.** OpenShell v0.0.28+ (PR [#544](https://github.com/NVIDIA/OpenShell/pull/544)) deprecated `tls: terminate` / `tls: passthrough` in policy YAML. TLS termination is now automatic by peeking ClientHello bytes. RightClaw's `codegen/policy.rs` still emits `tls: terminate`, producing per-request `WARN 'tls: terminate' is deprecated` in sandbox logs. Not broken yet, but will be once OpenShell removes the field.

2. **Systemic gap.** RightClaw has production agents running right now. Any codegen or config change must be deployable to them *without* sandbox recreation, `rightclaw agent init`, or manual migration. Today this rule lives as scattered bullet points in `CLAUDE.md` (`Upgrade-friendly design`, `Never delete sandboxes for recovery`, `Self-healing platform`). There is no executable contract, no type-level enforcement, no test that catches a regression. The `tls: terminate` deprecation is the first concrete example — we need the rule encoded so the next one is caught by `cargo test`.

The handoff doc at `docs/superpowers/plans/2026-04-24-mcp-sandbox-403-and-tls-deprecation.md` covers the full investigation. This spec implements Problem 2 (deprecation cleanup) plus the general migration model.

Problem 1 (rmcp DNS-rebinding 403) is out of scope for this spec and will be addressed separately.

## Design

### Codegen categories

Every per-agent codegen output belongs to exactly one of the following categories:

| Category | Semantics | Examples |
|---|---|---|
| `Regenerated(BotRestart)` | Unconditional overwrite on every bot start. Takes effect on next CC invocation. | `settings.json`, `mcp.json`, schemas, `system-prompt.md` |
| `Regenerated(SandboxPolicyApply)` | Overwrite + `openshell policy set --wait`. Network-only policy, hot-reloadable. | `policy.yaml` (network section) |
| `Regenerated(SandboxRecreate)` | Overwrite + triggers sandbox migration flow. Filesystem/landlock and other boot-time-only changes. | `policy.yaml` (filesystem section) |
| `MergedRMW` | Read existing, merge codegen fields in, write back. Preserves unknown fields. | `.claude.json`, `agent.yaml` (secret injection) |
| `AgentOwned` | Created by init with an initial payload. Never touched by codegen again. | `TOOLS.md`, `AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`, `MEMORY.md`, `settings.local.json` |

Cross-agent outputs (`process-compose.yaml`, `agent-tokens.json`, cloudflared config) are all `Regenerated(BotRestart)` — reread on `rightclaw up`.

**Note on `policy.yaml`.** A single file contains both a network section (hot-reloadable) and a filesystem section (requires recreate). The file is registered as `Regenerated(SandboxRecreate)` — the strictest applicable variant. At runtime, `openshell::filesystem_policy_changed` discriminates the two cases (existing function at `crates/rightclaw/src/openshell.rs:1299`): if filesystem-section drift is detected, sandbox migration (backup → new sandbox → restore → swap) runs; otherwise a plain `apply_policy` hot-reloads the network section.

**Existing gap — disclosed, not hidden by this spec.** `maybe_migrate_sandbox` (at `crates/rightclaw-cli/src/main.rs:3904`) is today called *only* from `rightclaw agent config`, not from bot startup. Which means a pure `rightclaw restart <agent>` with a new codegen-emitted filesystem policy will silently leave landlock rules drifted. This spec adds a minimal safeguard (see **Bot startup drift check** below) but does not automatically migrate on restart. Automatic migration on restart is out of scope here and tracked as a follow-up.

### Helper API

New module `crates/rightclaw/src/codegen/contract.rs`:

```rust
pub enum CodegenKind {
    Regenerated(HotReload),
    MergedRMW,
    AgentOwned,
}

pub enum HotReload {
    BotRestart,
    SandboxPolicyApply,
    SandboxRecreate,
}

pub struct CodegenFile {
    pub kind: CodegenKind,
    pub path: PathBuf,
}

/// Unconditional overwrite for `Regenerated(BotRestart | SandboxRecreate)` outputs.
/// The `HotReload` category is registry metadata — the writer itself just writes.
/// `SandboxPolicyApply` outputs do NOT use this function — they MUST go through
/// `write_and_apply_sandbox_policy` (enforced by there being no other writer).
pub fn write_regenerated(path: &Path, content: &str) -> miette::Result<()>;

/// Read-modify-write. `merge_fn` receives Some(existing) or None (file absent)
/// and returns the final content. Merger must preserve unknown fields.
pub fn write_merged_rmw<F>(path: &Path, merge_fn: F) -> miette::Result<()>
where
    F: FnOnce(Option<&str>) -> miette::Result<String>;

/// No-op if file exists. Otherwise writes `initial`.
pub fn write_agent_owned(path: &Path, initial: &str) -> miette::Result<()>;

/// The ONLY way to update policy for a running sandbox.
/// Writes file + applies via `openshell policy set --wait` atomically.
pub async fn write_and_apply_sandbox_policy(
    sandbox: &str,
    path: &Path,
    content: &str,
) -> miette::Result<()>;

/// Central registry of per-agent codegen outputs.
pub fn codegen_registry(agent_dir: &Path) -> Vec<CodegenFile>;

/// Central registry of cross-agent codegen outputs.
pub fn crossagent_codegen_registry(home: &Path) -> Vec<CodegenFile>;
```

Direct `std::fs::write` inside codegen modules is a review-blocking defect after this change.

### Call-site refactor

**`crates/rightclaw/src/codegen/pipeline.rs` — `run_single_agent_codegen`:**

Each row has two independent actions: replace the `std::fs::write` call with the helper, AND register the file in `codegen_registry()` with the listed category.

| Current line | File written | Helper | Registry category |
|---|---|---|---|
| 30 | `agent.yaml` (secret injection) | `write_merged_rmw` | `MergedRMW` |
| 68 | `.claude/reply-schema.json` | `write_regenerated` | `Regenerated(BotRestart)` |
| 80 | `.claude/cron-schema.json` | `write_regenerated` | `Regenerated(BotRestart)` |
| 98 | `.claude/system-prompt.md` | `write_regenerated` | `Regenerated(BotRestart)` |
| 110 | `.claude/bootstrap-schema.json` | `write_regenerated` | `Regenerated(BotRestart)` |
| 130 | `.claude/settings.json` | `write_regenerated` | `Regenerated(BotRestart)` |
| 178 | `.claude/settings.local.json` | `write_agent_owned` | `AgentOwned` |
| 204 | `policy.yaml` (seed, `host_ip=None`) | `write_regenerated` | `Regenerated(SandboxRecreate)` |
| `claude_json.rs:67` | `.claude.json` | `write_merged_rmw` | `MergedRMW` |
| `mcp_config.rs:61, 95` | `.mcp.json` | `write_regenerated` | `Regenerated(BotRestart)` |
| `skills.rs:44, 58` | `.claude/skills/*` | content-addressed deploy path preserved | `Regenerated(BotRestart)` |

Cross-agent writes (`pipeline.rs:265, 317, 332, 358`) switch to `write_regenerated` and are listed in `crossagent_codegen_registry` as `Regenerated(BotRestart)`.

**`crates/bot/src/lib.rs:524-532`** collapses to one call:

```rust
rightclaw::codegen::contract::write_and_apply_sandbox_policy(
    &sandbox, &policy_path, &policy_content,
).await?;
```

`write_and_apply_sandbox_policy` internally does the same write + `apply_policy` pair that exists today. Since `apply_policy` is a network-only hot-reload, this is unchanged in behaviour for network policy.

### Bot startup drift check

Immediately after `write_and_apply_sandbox_policy`, call `openshell::filesystem_policy_changed` with (active policy, new policy). If drift is detected, emit a `WARN`:

```
Filesystem policy drift detected for '<agent>'. Landlock rules in the running
sandbox do not match policy.yaml. Run `rightclaw agent config <agent>` (accept
defaults) to trigger sandbox migration, or `rightclaw agent backup <agent>
--sandbox-only` first if you want a recovery point.
```

No automatic migration — migrations are disruptive (tar-copy of `/sandbox/`, 10-60s downtime). Operator acts when ready. This turns a silent drift into a visible one and is the minimum acceptable behaviour for the `SandboxRecreate` category.

**Out of scope for refactor:** `wizard.rs` and `init.rs` template-file writes (AGENTS.md, BOOTSTRAP.md, etc.) — those are `AgentOwned` files being seeded at init time; the helper `write_agent_owned` is the right tool there, and we migrate them as part of the same PR since they share the registry.

### Fix for `tls: terminate` deprecation

Three concrete edits in `crates/rightclaw/src/codegen/policy.rs`:

1. **`restrictive_endpoints()` (line 16-30).** Drop the `tls: terminate` line from the `format!` template. Endpoint block becomes:
   ```
         - host: "{host}"
           port: 443
           protocol: rest
           access: full
   ```
2. **`generate_policy` permissive branch (line 48-62).** Drop `tls: terminate` from the `**.*:443` entry. Same pattern as above.
3. **Existing test `allows_all_outbound_https_and_http` (line 134-141).** Invert assertion:
   ```rust
   assert!(!policy.contains("tls: terminate"),
       "deprecated OpenShell field must not be emitted");
   ```

### Guard tests

**Unit — `codegen/policy.rs`:**
- `policy_has_no_deprecated_openshell_fields`: parse the generated YAML through `serde_saphyr`, walk every endpoint block, fail if any contains `tls: terminate` or `tls: passthrough`. Run matrix: `{Permissive, Restrictive} × {host_ip=None, host_ip=Some}` — 4 cases.

**Unit — `codegen/contract.rs` (or `contract_tests.rs` if file exceeds 800 LoC):**
- `regenerated_files_are_idempotent`: run `run_single_agent_codegen` twice on the same `tempfile::TempDir` agent layout. For each `Regenerated(_)` entry in `codegen_registry()`, compare SHA-256 of file contents between run 1 and run 2. Must match.
- `agent_owned_files_preserved`: seed `TOOLS.md` with marker `"__AGENT_WROTE_THIS__"`, run codegen, assert marker intact. Repeat for one representative per `AgentOwned` in the registry.
- `merged_rmw_preserves_unknown_fields`: pre-create `.claude.json` with `{"mcpServers": {...codegen fields...}, "customField": "xyz"}`, run codegen, assert `customField` still present.
- `registry_covers_all_per_agent_writes`: snapshot agent dir file set before and after `run_single_agent_codegen`; for every newly-created file, assert it appears in `codegen_registry()` OR in the documented `KNOWN_EXCEPTIONS` list. Initial exception set:
  - `.git/**` (git init)
  - `data.db`, `data.db-shm`, `data.db-wal` (SQLite)
  - `.claude/shell-snapshots/` (pre-created dir, CC populates)
  - `.claude/.credentials.json` (symlink created by `create_credential_symlink`, target is host-owned)
  - `inbox/`, `outbox/`, `tmp/inbox/`, `tmp/outbox/` (runtime dirs created by bot, not codegen — test fixture may or may not trigger these)
  New additions require both a registry entry and a review-reasoned exception.

**Integration — `crates/rightclaw/tests/policy_apply.rs` (new file):**
- `generated_policy_applies_to_live_openshell`: create a `TestSandbox`, generate policy via `generate_policy()` using both `Permissive` and `Restrictive`, apply via `apply_policy`. Assert exit code 0 and scan the sandbox's agent-container log for any `WARN` line containing `deprecated`. Slot-aware (uses `acquire_sandbox_slot`). No `#[ignore]`. Single test covers future deprecations and policy-syntax regressions in one pass.

**Deliberately not added:**
- Compile-time / AST linter forbidding `std::fs::write` in codegen modules — too brittle; `registry_covers_all_per_agent_writes` catches the same class of bug inductively.
- A test asserting `write_and_apply_sandbox_policy` is called whenever policy changes — enforced via type: there is no alternate writer for `SandboxPolicyApply`.

### ARCHITECTURE.md changes

Insert new top-level section `## Upgrade & Migration Model` after `## SQLite Rules`. Full content outlined in the Appendix below.

Minor edits elsewhere:
- `Configuration Hierarchy` table: add a `Category` column referencing the new taxonomy (no duplication, one-word entries linking to the main section).
- `OpenShell Policy Gotchas`: remove the `tls: terminate is required` entry; add a one-liner noting it's now deprecated and unused.

## Manual verification (after merge)

```bash
cargo build --workspace
rightclaw restart right   # or rightclaw down && rightclaw up

# No more deprecation warnings in agent container log:
docker exec openshell-cluster-openshell \
  kubectl -n openshell logs rightclaw-right -c agent --tail=200 \
  | grep -i 'deprecated'
# expect: no matches

# Policy.yaml on disk has no deprecated fields:
grep -c 'tls: terminate' ~/.rightclaw/agents/*/policy.yaml
# expect: 0
```

## Rollout

- Change lands in one PR (helper module + refactor + `tls: terminate` removal + ARCHITECTURE.md update).
- Existing agents adopt on next bot restart. No user-facing migration step.
- Sandboxes are not touched (network-only policy change, hot-reloadable).

## Non-goals

- Problem 1 from the handoff doc (rmcp `.disable_allowed_hosts()`). Separate spec.
- OpenShell server upgrades — covered by `OpenShell Integration Conventions`.
- SQLite schema migrations — handled by `rusqlite_migration` (see `SQLite Rules`).
- Runtime enforcement that an agent's on-disk file matches codegen expectations — files are regenerated every bot start, so drift is self-correcting.
- Changing the content of `AgentOwned` files once created. Those are the agent's property.

## Appendix: full ARCHITECTURE.md section

````markdown
## Upgrade & Migration Model

Every change that touches codegen, sandbox config, or on-disk state must be
deployable to already-running production agents. Manual migration steps,
`rightclaw agent init`, or sandbox recreation are NEVER acceptable as upgrade
paths.

### Codegen categories

Every per-agent codegen output belongs to exactly one category:

| Category | Semantics | Examples |
|---|---|---|
| `Regenerated(BotRestart)` | Unconditional overwrite every bot start. Takes effect on next CC invocation. | settings.json, mcp.json, schemas, system-prompt.md |
| `Regenerated(SandboxPolicyApply)` | Overwrite + `openshell policy set --wait`. Network-only. | policy.yaml (network section) |
| `Regenerated(SandboxRecreate)` | Overwrite + triggers sandbox migration. Filesystem/landlock and other boot-time-only changes. | policy.yaml (filesystem section) |
| `MergedRMW` | Read, merge, write. Preserves unknown fields. | .claude.json, agent.yaml (secret injection) |
| `AgentOwned` | Created by init. Never touched again. | TOOLS.md, AGENTS.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md, settings.local.json |

Cross-agent outputs (process-compose.yaml, agent-tokens.json, cloudflared
config) are all `Regenerated(BotRestart)` — reread on `rightclaw up`.

`policy.yaml` mixes a hot-reloadable network section and a recreate-only
filesystem section. It's registered as the stricter `Regenerated(SandboxRecreate)`;
runtime downgrades to hot-reload when the filesystem section is unchanged.

### Helper API

`crates/rightclaw/src/codegen/contract.rs` provides the only sanctioned writers:

- `write_regenerated(path, content, HotReload)` — all `Regenerated` outputs
  except `SandboxPolicyApply`.
- `write_merged_rmw(path, merge_fn)` — read-modify-write with unknown-field
  preservation.
- `write_agent_owned(path, initial)` — no-op if file exists.
- `write_and_apply_sandbox_policy(sandbox, path, content).await` — the ONLY
  way to update policy for a running sandbox. Writes + applies atomically.

Direct `std::fs::write` inside codegen modules is a review-blocking defect.

### Rules for adding a new codegen output

1. Pick a category. Add a `CodegenFile` entry to the matching registry
   (`codegen_registry()` or `crossagent_codegen_registry()`).
2. Use the matching helper. No bare `std::fs::write`.
3. Run `cargo test registry_covers_all_per_agent_writes` to verify the
   registry is complete.
4. If `Regenerated(SandboxRecreate)` — exercise the migration path manually
   and update `Sandbox migration` subsection under Data Flow if the trigger
   condition changed.
5. If the new output is policy-related, apply via
   `write_and_apply_sandbox_policy` only. Adding a new network endpoint is
   fine; adding a new filesystem rule requires `SandboxRecreate` treatment.
6. Never require `rightclaw agent init` for existing agents to adopt the
   change. They upgrade via `rightclaw restart <agent>`.

### Upgrade flow for a typical codegen change

1. Code change merged.
2. User runs `rightclaw restart <agent>` (or the bot restarts naturally via
   process-compose `on_failure`).
3. `run_single_agent_codegen` rewrites every `Regenerated` file.
4. Hot-reload machinery applies per category:
   - `BotRestart`: nothing extra — CC picks up the new file on next invocation.
   - `SandboxPolicyApply`: `write_and_apply_sandbox_policy` hot-reloads via
     `openshell policy set --wait`.
   - `SandboxRecreate`: bot startup compares active vs on-disk policy via
     `filesystem_policy_changed`. On drift, logs a WARN telling the operator
     to run `rightclaw agent config <agent>`, which invokes
     `maybe_migrate_sandbox` (backup → new sandbox → restore → swap). No
     automatic migration — it's disruptive and requires operator consent.
5. For `BotRestart` / `SandboxPolicyApply`: zero manual steps.
6. For `SandboxRecreate`: one follow-up command from the operator.

### Non-goals

- Agent-owned content (`AgentOwned` files) — agent property; codegen never
  mutates them.
- OpenShell server upgrades — covered by `OpenShell Integration Conventions`.
- SQLite schema — handled by `rusqlite_migration` (see `SQLite Rules`).

### Cross-references

- `CLAUDE.md` → `Upgrade-friendly design`, `Never delete sandboxes for
  recovery`, `Self-healing platform` — conventions this model implements.
- Data Flow → `Sandbox migration (filesystem policy change)` — the migration
  flow used by `Regenerated(SandboxRecreate)`.
````
