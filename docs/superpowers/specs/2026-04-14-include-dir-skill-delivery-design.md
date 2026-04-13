# include_dir! for built-in skill delivery

## Context

Built-in skills (rightmcp, rightcron, rightskills) are compiled into the binary
via `include_str!` in `crates/rightclaw/src/codegen/skills.rs`. Only `SKILL.md`
is included per skill. When `known-endpoints.yaml` was added to rightmcp, it
wasn't delivered to the sandbox because it wasn't in the `include_str!` list.

This is a structural problem: every new file in a skill directory requires a
manual Rust code change. `include_dir!` eliminates this by embedding entire
directories at compile time.

## Changes

### 1. Add `include_dir` dependency

- `include_dir = "0.7"` in `[workspace.dependencies]` (root `Cargo.toml`)
- `include_dir = { workspace = true }` in `crates/rightclaw/Cargo.toml`

### 2. Rewrite `skills.rs`

Replace:
```rust
const SKILL_RIGHTSKILLS: &str = include_str!("../../../../skills/rightskills/SKILL.md");
const SKILL_RIGHTCRON: &str = include_str!("../../../../skills/rightcron/SKILL.md");
const SKILL_RIGHTMCP: &str = include_str!("../../../../skills/rightmcp/SKILL.md");
```

With:
```rust
use include_dir::{include_dir, Dir};

const SKILL_RIGHTSKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../skills/rightskills");
const SKILL_RIGHTCRON: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../skills/rightcron");
const SKILL_RIGHTMCP: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../skills/rightmcp");
```

Rewrite `install_builtin_skills()` to iterate `Dir::files()` recursively,
creating subdirectories as needed. Same semantics: always overwrite built-in
skill files, preserve user dirs, create-if-absent `installed.json`.

### 3. Update tests

Existing tests check for `SKILL.md` existence — keep those.
Add: `rightmcp_includes_known_endpoints_yaml` — asserts `known-endpoints.yaml`
is written alongside `SKILL.md`.

### Not changed

- `platform_store.rs` — already uses `walkdir` + `directory_hash`, picks up
  new files automatically
- `build_manifest()` — directory hash changes when files change → redeploy

## Files to modify

| File | Change |
|------|--------|
| `Cargo.toml` (root) | Add `include_dir = "0.7"` to workspace deps |
| `crates/rightclaw/Cargo.toml` | Add `include_dir = { workspace = true }` |
| `crates/rightclaw/src/codegen/skills.rs` | Rewrite to use `include_dir!` |
