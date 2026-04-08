# Bootstrap Onboarding & Reverse Sync

## Summary

Two related features:

1. **Bootstrap onboarding** -- include BOOTSTRAP.md in the CC agent definition so the agent sees the onboarding script, conducts the first-run conversation, rewrites IDENTITY/SOUL/USER, then deletes BOOTSTRAP.md.
2. **Reverse sync** -- after every `claude -p` invocation, sync a fixed list of `.md` files from sandbox back to host so changes persist across restarts.

## Current State

- `templates/right/BOOTSTRAP.md` exists with full onboarding content (4 questions: name, nature, vibe, emoji).
- `rightclaw init` creates BOOTSTRAP.md in agent dir.
- `discovery.rs` discovers `bootstrap_path`.
- `doctor.rs` warns when BOOTSTRAP.md still exists (onboarding pending).
- `agent_def.rs` does NOT include BOOTSTRAP.md in the agent definition -- CC never sees it.
- `sync.rs` is host-to-sandbox only. No reverse sync exists.

## Design

### 1. Bootstrap in Agent Definition

**File:** `crates/rightclaw/src/codegen/agent_def.rs`

Add `bootstrap_path` to the optional files array in `generate_agent_definition()`. Insert it between IDENTITY and SOUL in the section order:

```
IDENTITY -> BOOTSTRAP -> SOUL -> USER -> AGENTS
```

BOOTSTRAP goes after IDENTITY because it contains instructions to rewrite IDENTITY. CC needs to read the current (template) IDENTITY first, then see the rewrite instructions.

Change: add `agent.bootstrap_path.as_ref()` to the `optional` array at index 0 (before `soul_path`).

### 2. Reverse Sync

**File:** `crates/bot/src/sync.rs`

New public function:

```rust
pub async fn reverse_sync_md(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()>
```

#### What to sync

Fixed list of files CC is allowed to modify:

```rust
const REVERSE_SYNC_FILES: &[&str] = &[
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
    "BOOTSTRAP.md",
];
```

#### Algorithm

For each file in `REVERSE_SYNC_FILES`:

1. Attempt `download_file(sandbox, "/sandbox/{file}", tmp_dir)`.
2. **Download succeeds:** compare content with `agent_dir/{file}`.
   - If different (or host file absent): atomic write via tempfile + rename into `agent_dir/`.
   - If identical: skip.
3. **Download fails** (file not in sandbox): check if `agent_dir/{file}` exists on host.
   - If it exists: delete it (CC removed the file -- the BOOTSTRAP.md deletion case).
   - If absent: skip (file never existed).

Per-file atomic writes make concurrent reverse syncs idempotent. Two parallel syncs download the same content and write the same result. No mutex needed.

#### Call Sites

**Worker** (`crates/bot/src/telegram/worker.rs`): call `reverse_sync_md()` after `invoke_cc()` returns, before sending the reply to Telegram. Only when sandboxed.

**Cron** (`crates/bot/src/cron.rs`): call `reverse_sync_md()` after `child.wait_with_output()` succeeds, before parsing the reply. Only when sandboxed.

#### Error Handling

`reverse_sync_md` itself propagates errors normally (returns `Result`). Per-file download failures are collected and the function returns an error summarizing any failures. **Callers** (worker, cron) log the error but do not propagate it -- reverse sync is not on the critical path. A failed download must not block the Telegram reply or cron completion. This is an intentional exception to the fail-fast rule: the caller makes the decision to swallow, not the sync function.

### 3. What Is NOT Reverse-Synced

These files have source of truth on the host and must NOT be pulled from sandbox:

- `.claude.json` -- managed by codegen + verify_claude_json
- `settings.json` -- managed by codegen
- `.mcp.json` / `mcp.json` -- managed by bot (refresh scheduler, `/mcp add/remove`)
- `skills/` -- managed by codegen
- `crons/` -- managed by user
- `agent.yaml` -- managed by user

### 4. Files Changed

| File | Change |
|------|--------|
| `crates/rightclaw/src/codegen/agent_def.rs` | Add `bootstrap_path` to optional sections array |
| `crates/rightclaw/src/codegen/agent_def_tests.rs` | Tests for bootstrap inclusion/exclusion in agent def |
| `crates/bot/src/sync.rs` | Add `reverse_sync_md()` function |
| `crates/bot/src/telegram/worker.rs` | Call `reverse_sync_md()` after `invoke_cc()` |
| `crates/bot/src/cron.rs` | Call `reverse_sync_md()` after CC subprocess completes |

### 5. Testing

- **agent_def**: test that when `bootstrap_path` is `Some`, BOOTSTRAP.md content appears in output between IDENTITY and SOUL sections.
- **agent_def**: test that when `bootstrap_path` is `None`, output is unchanged.
- **reverse_sync_md**: unit test with tempdir -- write files, call function, verify host files updated.
- **reverse_sync_md**: test deletion case -- file exists on host, absent in "sandbox" (download fails), verify host file removed.
- **reverse_sync_md**: test identical content -- file unchanged, verify no write (check mtime or use a spy).
