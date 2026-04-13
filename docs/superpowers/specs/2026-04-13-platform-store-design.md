# /platform Store: Atomic Sync with Symlinks

## Problem

1. **Directory uploads silently drop files** — OpenShell CLI bug, workaround (parallel single-file uploads) is fragile
2. **No atomicity** — sync overwrites files in place, agent can see partial writes
3. **No ownership boundary** — platform-managed and agent-owned files mixed in same directories, sync overwrites agent edits (AGENTS.md, TOOLS.md get clobbered every 5 min)
4. **Corrupted state blocks restart** — if a previous upload left garbage (e.g. file as directory), subsequent syncs fail and bot enters restart loop

## Solution

Introduce `/platform/` directory in sandbox for all platform-managed files. Content-addressed storage with symlinks from expected paths.

## Layout

```
/platform/                              ← platform-managed, read-only for agent
├── settings.json.a1b2c3d4             ← content hash suffix (sha256[:8])
├── reply-schema.json.f5e6d7c8
├── cron-schema.json.11223344
├── bootstrap-schema.json.aabbccdd
├── mcp.json.55667788
├── agents/
│   └── right.md.deadbeef              ← agent def (@ references, dead code but CC expects it)
└── skills/
    ├── rightmcp.b3c4d5e6/             ← directory hash = hash of all files concatenated
    │   └── SKILL.md
    ├── rightcron.9a8b7c6d/
    │   └── SKILL.md
    └── rightskills.1122aabb/
        └── SKILL.md

/sandbox/                               ← agent workspace, writable
├── IDENTITY.md                         ← agent-owned
├── SOUL.md                             ← agent-owned
├── USER.md                             ← agent-owned
├── AGENTS.md                           ← agent-owned (MOVED from .claude/agents/)
├── TOOLS.md                            ← agent-owned (MOVED from .claude/agents/)
├── inbox/
├── outbox/
└── .claude/
    ├── settings.json       → /platform/settings.json.a1b2c3d4
    ├── reply-schema.json   → /platform/reply-schema.json.f5e6d7c8
    ├── cron-schema.json    → /platform/cron-schema.json.11223344
    ├── bootstrap-schema.json → /platform/bootstrap-schema.json.aabbccdd
    ├── .mcp.json           → /platform/mcp.json.55667788
    ├── agents/
    │   └── right.md        → /platform/agents/right.md.deadbeef
    └── skills/
        ├── rightmcp        → /platform/skills/rightmcp.b3c4d5e6
        ├── rightcron       → /platform/skills/rightcron.9a8b7c6d
        └── rightskills     → /platform/skills/rightskills.1122aabb
```

## Sync Flow

### Initial sync (blocking, before bot starts)

1. Upload all platform files to `/platform/` with content-hash suffixes
   - Files: compute sha256[:8] of content, upload as `name.hash`
   - Directories: compute sha256[:8] of all files (sorted by relative path, concatenated), upload as `dirname.hash/`
   - Parallel uploads: `buffer_unordered(10)`
   - Skip upload if `/platform/name.hash` already exists (content-addressed dedup)
2. Create/update symlinks from `/sandbox/.claude/` → `/platform/`
   - Atomic swap: `ln -sf <target> /tmp/rightclaw-link && mv -f /tmp/rightclaw-link <link-path>`
   - Create parent directories if needed
3. Upload agent-owned files (AGENTS.md, TOOLS.md) to `/sandbox/` **only if they don't exist**
   - First boot: upload from host templates
   - Subsequent boots: agent's edits preserved
4. GC: list all files/dirs in `/platform/`, remove any not targeted by a current symlink
5. `chmod -R a-w /platform/` — prevent agent from modifying platform files

### Periodic sync (every 5 min)

Same as initial sync steps 1-2 and 4. Step 3 (agent-owned files) skipped — never overwrite agent edits. Step 5 (chmod) only needed if new files were uploaded.

### Failure handling

- Upload failure → error propagates via `?`, sync_cycle fails, bot reports error
- Symlink failure → same, error propagates
- GC failure → log warning, don't fail sync (GC is best-effort cleanup)

## Content Hash

```rust
fn content_hash(data: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    let hash = Sha256::digest(data);
    format!("{:x}", &hash[..4])  // 8 hex chars
}

fn directory_hash(dir: &Path) -> miette::Result<String> {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    let mut entries: Vec<_> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    entries.sort_by_key(|e| e.path().to_path_buf());
    for entry in entries {
        let rel = entry.path().strip_prefix(dir)?;
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(&std::fs::read(entry.path())?);
    }
    let hash = hasher.finalize();
    Ok(format!("{:x}", &hash[..4]))
}
```

## GC

```
1. Walk /platform/ recursively, collect all file paths and directory paths
2. Walk /sandbox/ recursively, collect all symlink targets (resolve relative → absolute)
3. Delete anything in /platform/ not in the symlink target set
4. Remove empty directories in /platform/
```

GC runs at end of every sync cycle. Errors logged but don't fail sync.

## Changes by Component

### `crates/rightclaw/src/openshell.rs`
- New: `upload_to_platform(sandbox, name, content) -> hash` — upload content-addressed file
- New: `upload_dir_to_platform(sandbox, name, host_dir) -> hash` — upload content-addressed directory
- New: `create_symlink(sandbox, link_path, target)` — atomic symlink via exec
- New: `gc_platform(sandbox, active_targets)` — remove stale files
- New: `check_platform_exists(sandbox, name_with_hash)` — check if already uploaded (dedup)
- Existing `upload_file` — unchanged, still used for agent-owned files (one-time upload)

### `crates/bot/src/sync.rs`
- `sync_cycle` rewritten: upload to `/platform/`, create symlinks, GC
- Agent-owned files (AGENTS.md, TOOLS.md) uploaded only in initial_sync, only if missing in sandbox

### `crates/rightclaw/src/openshell.rs` staging
- Staging dir layout changed: platform files go to `staging/platform/`, agent-owned to `staging/sandbox/`
- Symlinks created in staging (or via exec after sandbox creation)

### `crates/bot/src/telegram/worker.rs` prompt assembly
- Sandbox: `cat /sandbox/AGENTS.md` (was `/sandbox/.claude/agents/AGENTS.md`)
- Sandbox: `cat /sandbox/TOOLS.md` (was `/sandbox/.claude/agents/TOOLS.md`)
- Host: read from `agent_dir/AGENTS.md` (was `agent_dir/.claude/agents/AGENTS.md`)
- Host: read from `agent_dir/TOOLS.md` (was `agent_dir/.claude/agents/TOOLS.md`)

### `crates/rightclaw/src/codegen/pipeline.rs`
- AGENTS.md written to `agent_dir/AGENTS.md` (was `agent_dir/.claude/agents/AGENTS.md`)
- TOOLS.md written to `agent_dir/TOOLS.md` (was `agent_dir/.claude/agents/TOOLS.md`)

### `crates/rightclaw/src/init.rs`
- AGENTS.md, TOOLS.md written to agent_dir root

### OpenShell policy
- Add `/platform/` as read-only in filesystem_policy (agent can read, not write)

## Migration (existing agents)

Existing sandbox has files in old locations. On first sync with new code:
1. Upload to `/platform/`, create symlinks — symlinks overwrite existing files
2. Move AGENTS.md, TOOLS.md from `.claude/agents/` to `/sandbox/` root if not already there
3. GC cleans up old `.claude/agents/AGENTS.md` and `.claude/agents/TOOLS.md`

No manual intervention needed.

## Dependencies

- `sha2` crate — for content hashing (add to workspace)

## Testing

1. **Unit test**: `content_hash` and `directory_hash` produce stable hashes
2. **Unit test**: `gc_platform` removes stale, keeps active
3. **Integration test**: full sync cycle — upload to /platform/, symlinks work, files readable, overwrite works
4. **Integration test**: agent-owned files not overwritten on second sync
5. **Integration test**: GC removes old versions after content change
