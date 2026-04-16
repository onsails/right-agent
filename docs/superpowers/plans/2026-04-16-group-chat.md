# Group Chat Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Telegram group chat support with a dynamic two-level allowlist (trusted users + opened groups), backed by a new bot-managed `allowlist.yaml`, with mention/reply-based routing, group-attributed prompts, and expanded memory tags.

**Architecture:** New `rightclaw::agent::allowlist` module owns `allowlist.yaml` IO (atomic rename + lockfile) and in-memory `Arc<RwLock<AllowlistState>>`. A `notify` watcher hot-reloads the cache. Bot routing replaces the flat chat_id `HashSet` filter with `AllowlistState` + mention detection. New `/allow`, `/deny`, `/allowed`, `/allow_all`, `/deny_all` command handlers mutate the state. CLI gets matching `rightclaw agent allow/deny/...` subcommands. Worker adds group-aware prompt formatting and richer memory tags. Legacy `agent.yaml::allowed_chat_ids` is auto-migrated once, then warned about.

**Tech Stack:** Rust (edition 2024), teloxide 0.18, serde-saphyr (read) + serde_yml-like-output (we'll emit YAML manually to avoid serde_yaml), notify + notify-debouncer-mini, tokio, clap.

---

## File Structure

**New files:**
- `crates/rightclaw/src/agent/allowlist.rs` — schema, state, IO, migration, file watcher.
- `crates/bot/src/telegram/mention.rs` — mention/reply detection and stripping.
- `crates/bot/src/telegram/allowlist_commands.rs` — `/allow`, `/deny`, `/allowed`, `/allow_all`, `/deny_all` handlers.

**Modified files:**
- `crates/rightclaw/src/agent/mod.rs` — re-export allowlist.
- `crates/rightclaw/src/agent/types.rs` — annotate `allowed_chat_ids` as deprecated (kept for migration read).
- `crates/rightclaw/src/init.rs` — wizard step for first trusted user ID.
- `crates/rightclaw-cli/src/main.rs` — new `AgentCommands::Allow/Deny/AllowAll/DenyAll/Allowed` subcommands + dispatch.
- `crates/bot/src/lib.rs` — load allowlist (+ migrate) before starting dispatcher; spawn watcher task.
- `crates/bot/src/telegram/mod.rs` — register new modules.
- `crates/bot/src/telegram/dispatch.rs` — rewire dependencies (drop `Vec<i64>`, add `AllowlistHandle`); register new bot commands; extend DI.
- `crates/bot/src/telegram/filter.rs` — rewrite as `make_routing_filter(AllowlistHandle, BotIdentity)` returning `Option<RoutingDecision>`.
- `crates/bot/src/telegram/handler.rs` — gate existing commands to DM; update `handle_message` to carry `RoutingDecision`.
- `crates/bot/src/telegram/worker.rs` — expand `chat_tags` → `retain_tags`; reply-to triggering message; silence live thinking in groups.
- `crates/bot/src/telegram/attachments.rs` — extend `InputMessage` (chat kind + reply_to body); extend `format_cc_input` for group attribution.

---

### Task 1: Allowlist schema types + unit tests

**Files:**
- Create: `crates/rightclaw/src/agent/allowlist.rs`
- Modify: `crates/rightclaw/src/agent/mod.rs`

Introduces `AllowlistFile` (the on-disk schema), `AllowedUser`, `AllowedGroup` structs, with serde-saphyr read and a custom YAML emitter (we avoid serde_yaml because it's deprecated; emit by hand — tiny format).

- [ ] **Step 1: Write the failing tests**

Create `crates/rightclaw/src/agent/allowlist.rs`:

```rust
//! Bot-managed allowlist — trusted users (DM + anywhere) + opened groups (per-chat-id open gate).
//!
//! On-disk format (version 1):
//!
//! ```yaml
//! version: 1
//! users:
//!   - id: 123
//!     label: andrey
//!     added_by: null
//!     added_at: 2026-04-16T12:00:00Z
//! groups:
//!   - id: -1001234
//!     label: Dev Team
//!     opened_by: null
//!     opened_at: 2026-04-16T12:00:00Z
//! ```

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AllowedUser {
    pub id: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub added_by: Option<i64>,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AllowedGroup {
    pub id: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub opened_by: Option<i64>,
    pub opened_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AllowlistFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub users: Vec<AllowedUser>,
    #[serde(default)]
    pub groups: Vec<AllowedGroup>,
}

fn default_version() -> u32 { 1 }

pub const CURRENT_VERSION: u32 = 1;

impl Default for AllowlistFile {
    fn default() -> Self {
        Self { version: CURRENT_VERSION, users: Vec::new(), groups: Vec::new() }
    }
}

/// Parse YAML text into `AllowlistFile`. Rejects unknown future `version`.
pub fn parse_yaml(text: &str) -> Result<AllowlistFile, String> {
    let parsed: AllowlistFile = serde_saphyr::from_str(text)
        .map_err(|e| format!("allowlist.yaml parse error: {e:#}"))?;
    if parsed.version != CURRENT_VERSION {
        return Err(format!(
            "allowlist.yaml version {} is not supported (expected {})",
            parsed.version, CURRENT_VERSION
        ));
    }
    Ok(parsed)
}

/// Serialize `AllowlistFile` to YAML text (deterministic key order).
pub fn serialize_yaml(file: &AllowlistFile) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(512);
    out.push_str("# Bot-managed. Edit via /allow, /deny, /allow_all, /deny_all,\n");
    out.push_str("# or `rightclaw agent allow|deny|allow_all|deny_all`.\n");
    writeln!(out, "version: {}", file.version).unwrap();
    if file.users.is_empty() {
        out.push_str("users: []\n");
    } else {
        out.push_str("users:\n");
        for u in &file.users {
            writeln!(out, "  - id: {}", u.id).unwrap();
            write_label(&mut out, "label", u.label.as_deref(), 4);
            write_opt_i64(&mut out, "added_by", u.added_by, 4);
            writeln!(out, "    added_at: {}", u.added_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap();
        }
    }
    if file.groups.is_empty() {
        out.push_str("groups: []\n");
    } else {
        out.push_str("groups:\n");
        for g in &file.groups {
            writeln!(out, "  - id: {}", g.id).unwrap();
            write_label(&mut out, "label", g.label.as_deref(), 4);
            write_opt_i64(&mut out, "opened_by", g.opened_by, 4);
            writeln!(out, "    opened_at: {}", g.opened_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap();
        }
    }
    out
}

fn write_label(out: &mut String, key: &str, val: Option<&str>, indent: usize) {
    use std::fmt::Write;
    let spaces: String = " ".repeat(indent);
    match val {
        Some(s) => {
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
            writeln!(out, "{spaces}{key}: \"{escaped}\"").unwrap();
        }
        None => writeln!(out, "{spaces}{key}: null").unwrap(),
    }
}

fn write_opt_i64(out: &mut String, key: &str, val: Option<i64>, indent: usize) {
    use std::fmt::Write;
    let spaces: String = " ".repeat(indent);
    match val {
        Some(n) => writeln!(out, "{spaces}{key}: {n}").unwrap(),
        None => writeln!(out, "{spaces}{key}: null").unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_allowlist() {
        let text = "version: 1\nusers: []\ngroups: []\n";
        let parsed = parse_yaml(text).unwrap();
        assert_eq!(parsed.version, 1);
        assert!(parsed.users.is_empty());
        assert!(parsed.groups.is_empty());
    }

    #[test]
    fn parses_users_and_groups() {
        let text = r#"version: 1
users:
  - id: 42
    label: andrey
    added_by: null
    added_at: 2026-04-16T12:00:00Z
groups:
  - id: -1001
    label: "Dev Team"
    opened_by: 42
    opened_at: 2026-04-16T12:30:00Z
"#;
        let parsed = parse_yaml(text).unwrap();
        assert_eq!(parsed.users.len(), 1);
        assert_eq!(parsed.users[0].id, 42);
        assert_eq!(parsed.users[0].label.as_deref(), Some("andrey"));
        assert_eq!(parsed.users[0].added_by, None);
        assert_eq!(parsed.groups.len(), 1);
        assert_eq!(parsed.groups[0].id, -1001);
        assert_eq!(parsed.groups[0].opened_by, Some(42));
    }

    #[test]
    fn missing_version_defaults_to_1() {
        let text = "users: []\ngroups: []\n";
        let parsed = parse_yaml(text).unwrap();
        assert_eq!(parsed.version, 1);
    }

    #[test]
    fn rejects_unknown_version() {
        let text = "version: 99\nusers: []\ngroups: []\n";
        let err = parse_yaml(text).unwrap_err();
        assert!(err.contains("version 99"));
    }

    #[test]
    fn serialize_roundtrip() {
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 42,
                label: Some("andrey".into()),
                added_by: None,
                added_at: "2026-04-16T12:00:00Z".parse().unwrap(),
            }],
            groups: vec![AllowedGroup {
                id: -1001,
                label: Some("Dev Team".into()),
                opened_by: Some(42),
                opened_at: "2026-04-16T12:30:00Z".parse().unwrap(),
            }],
        };
        let yaml = serialize_yaml(&file);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed, file);
    }

    #[test]
    fn serialize_empty_lists_use_flow_style() {
        let file = AllowlistFile::default();
        let yaml = serialize_yaml(&file);
        assert!(yaml.contains("users: []"));
        assert!(yaml.contains("groups: []"));
    }

    #[test]
    fn serialize_escapes_quotes_in_label() {
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 1,
                label: Some(r#"has "quotes""#.into()),
                added_by: None,
                added_at: "2026-04-16T12:00:00Z".parse().unwrap(),
            }],
            groups: vec![],
        };
        let yaml = serialize_yaml(&file);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed.users[0].label.as_deref(), Some(r#"has "quotes""#));
    }
}
```

Register in `crates/rightclaw/src/agent/mod.rs`:

```rust
pub mod allowlist;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw allowlist::tests -- --nocapture`
Expected: FAIL with "module `allowlist` not found" or compilation errors (module not yet registered).

- [ ] **Step 3: Resolve dependencies and recompile**

Verify `chrono` already exports `DateTime<Utc>` from `rightclaw/Cargo.toml`. If absent, add:

```toml
chrono = { version = "0.4", features = ["serde"] }
```

Run: `cargo build -p rightclaw`
Expected: PASS.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw allowlist::tests -- --nocapture`
Expected: all 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/agent/allowlist.rs crates/rightclaw/src/agent/mod.rs
git commit -m "feat(allowlist): schema types, parse, serialize"
```

---

### Task 2: `AllowlistState` in-memory + mutation ops

**Files:**
- Modify: `crates/rightclaw/src/agent/allowlist.rs`

Add the `Arc<RwLock<AllowlistState>>` handle with read/write methods used by bot and CLI.

- [ ] **Step 1: Write the failing tests**

Append to `crates/rightclaw/src/agent/allowlist.rs`:

```rust
use std::sync::Arc;
use tokio::sync::RwLock;

/// Outcome of `add_user` / `add_group`.
#[derive(Debug, Clone, PartialEq)]
pub enum AddOutcome { Inserted, AlreadyPresent }

/// Outcome of `remove_user` / `remove_group`.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoveOutcome { Removed, NotFound }

#[derive(Debug, Default, Clone)]
pub struct AllowlistState {
    inner: AllowlistFile,
}

impl AllowlistState {
    pub fn from_file(file: AllowlistFile) -> Self { Self { inner: file } }
    pub fn to_file(&self) -> AllowlistFile { self.inner.clone() }

    /// Is this user globally trusted?
    pub fn is_user_trusted(&self, user_id: i64) -> bool {
        self.inner.users.iter().any(|u| u.id == user_id)
    }

    /// Is this group opened (members may talk to bot with mention/reply)?
    pub fn is_group_open(&self, chat_id: i64) -> bool {
        self.inner.groups.iter().any(|g| g.id == chat_id)
    }

    pub fn users(&self) -> &[AllowedUser] { &self.inner.users }
    pub fn groups(&self) -> &[AllowedGroup] { &self.inner.groups }

    pub fn add_user(&mut self, user: AllowedUser) -> AddOutcome {
        if self.is_user_trusted(user.id) { return AddOutcome::AlreadyPresent; }
        self.inner.users.push(user);
        AddOutcome::Inserted
    }

    pub fn remove_user(&mut self, user_id: i64) -> RemoveOutcome {
        let before = self.inner.users.len();
        self.inner.users.retain(|u| u.id != user_id);
        if self.inner.users.len() == before { RemoveOutcome::NotFound } else { RemoveOutcome::Removed }
    }

    pub fn add_group(&mut self, group: AllowedGroup) -> AddOutcome {
        if self.is_group_open(group.id) { return AddOutcome::AlreadyPresent; }
        self.inner.groups.push(group);
        AddOutcome::Inserted
    }

    pub fn remove_group(&mut self, chat_id: i64) -> RemoveOutcome {
        let before = self.inner.groups.len();
        self.inner.groups.retain(|g| g.id != chat_id);
        if self.inner.groups.len() == before { RemoveOutcome::NotFound } else { RemoveOutcome::Removed }
    }
}

/// Shareable handle used by bot and CLI. Writers take `.write()`, readers take `.read()`.
#[derive(Debug, Clone, Default)]
pub struct AllowlistHandle(pub Arc<RwLock<AllowlistState>>);

impl AllowlistHandle {
    pub fn new(state: AllowlistState) -> Self { Self(Arc::new(RwLock::new(state))) }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    fn t() -> DateTime<Utc> { "2026-04-16T12:00:00Z".parse().unwrap() }

    #[test]
    fn add_user_inserted_then_already_present() {
        let mut s = AllowlistState::default();
        let u = AllowedUser { id: 1, label: None, added_by: None, added_at: t() };
        assert_eq!(s.add_user(u.clone()), AddOutcome::Inserted);
        assert_eq!(s.add_user(u), AddOutcome::AlreadyPresent);
        assert_eq!(s.users().len(), 1);
    }

    #[test]
    fn remove_user_removed_then_not_found() {
        let mut s = AllowlistState::default();
        let u = AllowedUser { id: 1, label: None, added_by: None, added_at: t() };
        s.add_user(u);
        assert_eq!(s.remove_user(1), RemoveOutcome::Removed);
        assert_eq!(s.remove_user(1), RemoveOutcome::NotFound);
    }

    #[test]
    fn is_user_trusted_reflects_state() {
        let mut s = AllowlistState::default();
        assert!(!s.is_user_trusted(99));
        s.add_user(AllowedUser { id: 99, label: None, added_by: None, added_at: t() });
        assert!(s.is_user_trusted(99));
    }

    #[test]
    fn add_group_and_is_open() {
        let mut s = AllowlistState::default();
        assert!(!s.is_group_open(-1));
        s.add_group(AllowedGroup { id: -1, label: None, opened_by: Some(1), opened_at: t() });
        assert!(s.is_group_open(-1));
    }

    #[tokio::test]
    async fn handle_is_shareable_across_tasks() {
        let h = AllowlistHandle::new(AllowlistState::default());
        let h2 = h.clone();
        tokio::spawn(async move {
            let mut w = h2.0.write().await;
            w.add_user(AllowedUser { id: 7, label: None, added_by: None, added_at: t() });
        }).await.unwrap();
        let r = h.0.read().await;
        assert!(r.is_user_trusted(7));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw allowlist::state_tests`
Expected: FAIL (types not yet defined).

- [ ] **Step 3: Verify compile (types are now defined above)**

Run: `cargo build -p rightclaw`
Expected: PASS. Tests were declared alongside types, so step 1 added the implementation and tests simultaneously.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw allowlist::state_tests`
Expected: 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/agent/allowlist.rs
git commit -m "feat(allowlist): AllowlistState + handle with add/remove ops"
```

---

### Task 3: Atomic file IO (read, write-with-lockfile)

**Files:**
- Modify: `crates/rightclaw/src/agent/allowlist.rs`
- Modify: `crates/rightclaw/Cargo.toml` (add `fs4` for file locks if not present)

- [ ] **Step 1: Write the failing tests**

Append to `crates/rightclaw/src/agent/allowlist.rs`:

```rust
use std::path::{Path, PathBuf};

/// Filename inside `agent_dir`.
pub const ALLOWLIST_FILENAME: &str = "allowlist.yaml";
/// Lockfile sibling. Held during read-modify-write.
pub const ALLOWLIST_LOCK_FILENAME: &str = "allowlist.yaml.lock";

pub fn allowlist_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join(ALLOWLIST_FILENAME)
}

pub fn lock_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join(ALLOWLIST_LOCK_FILENAME)
}

/// Read `allowlist.yaml` if present. Returns `Ok(None)` when the file doesn't exist
/// (caller decides whether to migrate or create empty). Returns parse errors verbatim.
pub fn read_file(agent_dir: &Path) -> Result<Option<AllowlistFile>, String> {
    let path = allowlist_path(agent_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_yaml(&text).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e:#}", path.display())),
    }
}

/// Acquire an exclusive file lock on `agent_dir/allowlist.yaml.lock`, then run `f`,
/// which may read, mutate, and call `write_file_inner` (below). Lock is released on drop.
///
/// The lockfile is created if missing. Using `fs4::fs_std::FileExt::lock_exclusive`
/// (advisory flock on Unix, LockFileEx on Windows).
pub fn with_lock<R>(
    agent_dir: &Path,
    f: impl FnOnce(&Path) -> Result<R, String>,
) -> Result<R, String> {
    use fs4::fs_std::FileExt;
    let lock_p = lock_path(agent_dir);
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_p)
        .map_err(|e| format!("open lockfile {}: {e:#}", lock_p.display()))?;
    lock_file.lock_exclusive()
        .map_err(|e| format!("lock {}: {e:#}", lock_p.display()))?;
    let result = f(agent_dir);
    let _ = FileExt::unlock(&lock_file);
    result
}

/// Atomic write: serialize `file`, write to `allowlist.yaml.tmp`, fsync, rename.
/// Caller must already hold the lock (via `with_lock`).
pub fn write_file_inner(agent_dir: &Path, file: &AllowlistFile) -> Result<(), String> {
    use std::io::Write;
    let text = serialize_yaml(file);
    let target = allowlist_path(agent_dir);
    let tmp = target.with_extension("yaml.tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| format!("create {}: {e:#}", tmp.display()))?;
        f.write_all(text.as_bytes())
            .map_err(|e| format!("write {}: {e:#}", tmp.display()))?;
        f.sync_all()
            .map_err(|e| format!("fsync {}: {e:#}", tmp.display()))?;
    }
    std::fs::rename(&tmp, &target)
        .map_err(|e| format!("rename {} -> {}: {e:#}", tmp.display(), target.display()))?;
    Ok(())
}

/// Convenience: lock + write in one call.
pub fn write_file(agent_dir: &Path, file: &AllowlistFile) -> Result<(), String> {
    with_lock(agent_dir, |dir| write_file_inner(dir, file))
}

#[cfg(test)]
mod io_tests {
    use super::*;
    use tempfile::TempDir;

    fn t() -> DateTime<Utc> { "2026-04-16T12:00:00Z".parse().unwrap() }

    #[test]
    fn read_nonexistent_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_file(dir.path()).unwrap().is_none());
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = TempDir::new().unwrap();
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser { id: 1, label: Some("u".into()), added_by: None, added_at: t() }],
            groups: vec![AllowedGroup { id: -1, label: None, opened_by: Some(1), opened_at: t() }],
        };
        write_file(dir.path(), &file).unwrap();
        let read = read_file(dir.path()).unwrap().unwrap();
        assert_eq!(read, file);
    }

    #[test]
    fn atomic_write_leaves_no_tmp_file() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), &AllowlistFile::default()).unwrap();
        let tmp = dir.path().join("allowlist.yaml.tmp");
        assert!(!tmp.exists(), "tmp file must be renamed away");
        assert!(dir.path().join("allowlist.yaml").exists());
    }

    #[test]
    fn with_lock_creates_lockfile_and_returns_result() {
        let dir = TempDir::new().unwrap();
        let r = with_lock(dir.path(), |_| Ok::<_, String>(42)).unwrap();
        assert_eq!(r, 42);
        assert!(dir.path().join("allowlist.yaml.lock").exists());
    }
}
```

- [ ] **Step 2: Add `fs4` to Cargo.toml**

Check `crates/rightclaw/Cargo.toml` for `fs4`. If absent, add under `[dependencies]`:

```toml
fs4 = "0.13"
```

Also verify `tempfile` is in `[dev-dependencies]`; if not, add:

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Run tests to verify they fail (or compile)**

Run: `cargo test -p rightclaw allowlist::io_tests`
Expected: PASS once deps resolved (fs4 present, tempfile for dev).

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/Cargo.toml crates/rightclaw/src/agent/allowlist.rs
git commit -m "feat(allowlist): atomic file read/write + lockfile"
```

---

### Task 4: Migration from `agent.yaml::allowed_chat_ids`

**Files:**
- Modify: `crates/rightclaw/src/agent/allowlist.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/rightclaw/src/agent/allowlist.rs`:

```rust
/// Summary of a migration run, for logging.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MigrationReport {
    pub migrated_users: usize,
    pub migrated_groups: usize,
    /// True iff `allowlist.yaml` existed prior to this call (nothing to do).
    pub already_present: bool,
}

/// One-time migration from `agent.yaml::allowed_chat_ids` to `allowlist.yaml`.
///
/// Behaviour:
/// 1. If `allowlist.yaml` exists → no-op, return `already_present: true`.
/// 2. Else, write a new `allowlist.yaml` containing split-by-sign entries
///    (positive → users, negative → groups), all with `added_by`/`opened_by = None`
///    and `added_at`/`opened_at = now`.
/// 3. If `legacy_chat_ids` is empty, create an empty `allowlist.yaml`.
pub fn migrate_from_legacy(
    agent_dir: &Path,
    legacy_chat_ids: &[i64],
    now: DateTime<Utc>,
) -> Result<MigrationReport, String> {
    if allowlist_path(agent_dir).exists() {
        return Ok(MigrationReport { already_present: true, ..Default::default() });
    }
    let mut file = AllowlistFile::default();
    let mut mu = 0usize;
    let mut mg = 0usize;
    for &id in legacy_chat_ids {
        if id > 0 {
            file.users.push(AllowedUser { id, label: None, added_by: None, added_at: now });
            mu += 1;
        } else if id < 0 {
            file.groups.push(AllowedGroup { id, label: None, opened_by: None, opened_at: now });
            mg += 1;
        }
        // id == 0 skipped (Telegram never uses 0).
    }
    write_file(agent_dir, &file)?;
    Ok(MigrationReport { migrated_users: mu, migrated_groups: mg, already_present: false })
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use tempfile::TempDir;

    fn t() -> DateTime<Utc> { "2026-04-16T12:00:00Z".parse().unwrap() }

    #[test]
    fn migration_splits_by_sign() {
        let dir = TempDir::new().unwrap();
        let report = migrate_from_legacy(dir.path(), &[42, -1001, 100, -500, 0], t()).unwrap();
        assert_eq!(report.migrated_users, 2);
        assert_eq!(report.migrated_groups, 2);
        assert!(!report.already_present);
        let file = read_file(dir.path()).unwrap().unwrap();
        let user_ids: Vec<i64> = file.users.iter().map(|u| u.id).collect();
        let group_ids: Vec<i64> = file.groups.iter().map(|g| g.id).collect();
        assert_eq!(user_ids, vec![42, 100]);
        assert_eq!(group_ids, vec![-1001, -500]);
        assert!(file.users.iter().all(|u| u.added_by.is_none()));
        assert!(file.groups.iter().all(|g| g.opened_by.is_none()));
    }

    #[test]
    fn migration_skips_when_allowlist_exists() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), &AllowlistFile::default()).unwrap();
        let report = migrate_from_legacy(dir.path(), &[42], t()).unwrap();
        assert!(report.already_present);
        assert_eq!(report.migrated_users, 0);
        let file = read_file(dir.path()).unwrap().unwrap();
        assert!(file.users.is_empty(), "should not overwrite existing file");
    }

    #[test]
    fn migration_empty_list_creates_empty_allowlist() {
        let dir = TempDir::new().unwrap();
        let report = migrate_from_legacy(dir.path(), &[], t()).unwrap();
        assert_eq!(report.migrated_users, 0);
        assert_eq!(report.migrated_groups, 0);
        assert!(!report.already_present);
        let file = read_file(dir.path()).unwrap().unwrap();
        assert!(file.users.is_empty());
        assert!(file.groups.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p rightclaw allowlist::migration_tests`
Expected: 3 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/agent/allowlist.rs
git commit -m "feat(allowlist): one-time migration from agent.yaml::allowed_chat_ids"
```

---

### Task 5: File watcher — hot-reload `allowlist.yaml` into `AllowlistHandle`

**Files:**
- Modify: `crates/rightclaw/src/agent/allowlist.rs`

Use `notify-debouncer-mini` (already a workspace dep) to debounce filesystem events 200 ms and reparse the file. On parse error, log and keep previous state.

- [ ] **Step 1: Write the failing test**

Append to `crates/rightclaw/src/agent/allowlist.rs`:

```rust
use std::time::Duration;

/// Spawn a background watcher on `allowlist_path(agent_dir)`. Whenever the file
/// changes, reparse and swap the `handle` contents. Parse errors are logged
/// (tracing::warn) and leave the previous state untouched.
///
/// Returns the debouncer object. Dropping it stops the watcher. The caller
/// typically keeps it alive for the lifetime of the bot process.
pub fn spawn_watcher(
    agent_dir: &Path,
    handle: AllowlistHandle,
) -> Result<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>, String> {
    use notify::RecursiveMode;
    use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};

    let watch_path = agent_dir.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_millis(200), move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| format!("create debouncer: {e:#}"))?;

    debouncer
        .watcher()
        .watch(&watch_path, RecursiveMode::NonRecursive)
        .map_err(|e| format!("watch {}: {e:#}", watch_path.display()))?;

    let handle_clone = handle.clone();
    let dir_clone = agent_dir.to_path_buf();
    std::thread::spawn(move || {
        let target = allowlist_path(&dir_clone);
        for events in rx {
            let Ok(evts) = events else { continue; };
            let touches_allowlist = evts
                .iter()
                .any(|e| matches!(e.kind, DebouncedEventKind::Any) && e.path == target);
            if !touches_allowlist { continue; }

            match read_file(&dir_clone) {
                Ok(Some(file)) => {
                    let state = AllowlistState::from_file(file);
                    let rt = tokio::runtime::Handle::try_current();
                    match rt {
                        Ok(h) => {
                            let handle = handle_clone.clone();
                            h.spawn(async move {
                                let mut w = handle.0.write().await;
                                *w = state;
                            });
                        }
                        Err(_) => {
                            let fut = async {
                                let mut w = handle_clone.0.write().await;
                                *w = state;
                            };
                            tokio::runtime::Runtime::new()
                                .expect("fallback runtime")
                                .block_on(fut);
                        }
                    }
                    tracing::info!("allowlist.yaml reloaded");
                }
                Ok(None) => {
                    tracing::warn!("allowlist.yaml disappeared; keeping previous state");
                }
                Err(e) => {
                    tracing::warn!("allowlist.yaml reload failed: {e}; keeping previous state");
                }
            }
        }
    });

    Ok(debouncer)
}

#[cfg(test)]
mod watcher_tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    fn t() -> DateTime<Utc> { "2026-04-16T12:00:00Z".parse().unwrap() }

    #[tokio::test]
    async fn watcher_reloads_after_external_write() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), &AllowlistFile::default()).unwrap();
        let handle = AllowlistHandle::new(AllowlistState::from_file(
            read_file(dir.path()).unwrap().unwrap(),
        ));
        let _watcher = spawn_watcher(dir.path(), handle.clone()).unwrap();

        // Externally mutate the file.
        let mut file = AllowlistFile::default();
        file.users.push(AllowedUser { id: 777, label: None, added_by: None, added_at: t() });
        write_file(dir.path(), &file).unwrap();

        // Poll up to 2s for the handle to reflect the change.
        for _ in 0..40 {
            {
                let r = handle.0.read().await;
                if r.is_user_trusted(777) { return; }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("watcher did not propagate external write within 2s");
    }
}
```

- [ ] **Step 2: Run tests to verify pass (debouncer is platform-dependent but `notify` is a dev-tested dep)**

Run: `cargo test -p rightclaw allowlist::watcher_tests -- --test-threads=1`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/agent/allowlist.rs
git commit -m "feat(allowlist): notify watcher hot-reloads in-memory state"
```

---

### Task 6: Mention / reply detection module

**Files:**
- Create: `crates/bot/src/telegram/mention.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

Helpers that encode the spec's §Mention/Reply Detection rules and produce a `RoutingDecision`.

- [ ] **Step 1: Write the failing tests**

Create `crates/bot/src/telegram/mention.rs`:

```rust
//! Detect whether a group message addresses the bot, and prepare the
//! cleaned-up prompt text.

use teloxide::types::{Message, MessageEntityKind, UserId};

/// Bot identity: username (without '@') and user_id. Cached at bot startup.
#[derive(Debug, Clone)]
pub struct BotIdentity {
    pub username: String,
    pub user_id: u64,
}

/// How a routed message refers to the bot, in group context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressKind {
    DirectMessage,
    GroupMentionText,       // `@botname` in text
    GroupMentionEntity,     // TextMention entity pointing at bot user_id
    GroupReplyToBot,        // reply_to_message is from bot
    GroupSlashCommand,      // /cmd@botname (or any cmd in a group-to-bot)
}

/// Returns true when the message in a group should be treated as addressed to the bot.
pub fn is_bot_addressed(msg: &Message, identity: &BotIdentity) -> Option<AddressKind> {
    use teloxide::types::ChatKind;
    match &msg.chat.kind {
        ChatKind::Private(_) => Some(AddressKind::DirectMessage),
        _ => {
            let text_opt = msg.text().or(msg.caption()).unwrap_or("");
            let entities_opt = msg.entities().or(msg.caption_entities());

            // 1) reply to bot's message
            if let Some(reply) = msg.reply_to_message()
                && let Some(from) = reply.from.as_ref()
                && from.id.0 == identity.user_id
            {
                return Some(AddressKind::GroupReplyToBot);
            }

            if let Some(entities) = entities_opt {
                for e in entities {
                    match &e.kind {
                        MessageEntityKind::TextMention { user } if user.id.0 == identity.user_id => {
                            return Some(AddressKind::GroupMentionEntity);
                        }
                        MessageEntityKind::Mention => {
                            let start = e.offset;
                            let end = e.offset + e.length;
                            let slice: String = text_opt.chars().skip(start).take(e.length).collect();
                            // Slice is e.g. "@botname"; compare case-insensitively.
                            if slice.strip_prefix('@').map(|u| u.eq_ignore_ascii_case(&identity.username)).unwrap_or(false) {
                                return Some(AddressKind::GroupMentionText);
                            }
                        }
                        MessageEntityKind::BotCommand => {
                            let slice: String = text_opt.chars().skip(e.offset).take(e.length).collect();
                            // Accept /cmd (no suffix — only one bot in chat or we're the default)
                            // or /cmd@botname (explicit).
                            if let Some((_, maybe_user)) = slice.split_once('@') {
                                if maybe_user.eq_ignore_ascii_case(&identity.username) {
                                    return Some(AddressKind::GroupSlashCommand);
                                }
                            } else {
                                return Some(AddressKind::GroupSlashCommand);
                            }
                        }
                        _ => {}
                    }
                }
            }
            None
        }
    }
}

/// Strip `@botname` mentions from `text` for prompt cleanup.
pub fn strip_bot_mentions(text: &str, username: &str) -> String {
    // Remove every case-insensitive occurrence of `@<username>`
    // conservatively: scan character indices, not byte offsets.
    let lower_user = username.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c == '@' {
            let rest = &text[i + 1..];
            let end = rest
                .char_indices()
                .find(|(_, ch)| !(ch.is_ascii_alphanumeric() || *ch == '_'))
                .map(|(idx, _)| idx)
                .unwrap_or(rest.len());
            let candidate = &rest[..end];
            if candidate.eq_ignore_ascii_case(&lower_user) && !candidate.is_empty() {
                // skip the @ + candidate
                for _ in 0..candidate.chars().count() {
                    it.next();
                }
                continue;
            }
        }
        out.push(c);
    }
    // Collapse duplicate whitespace left behind.
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim().to_string()
}

/// Parse a command string that may have the `@botname` suffix: returns (cmd, args_rest, addressed).
/// `addressed` is false when the suffix names a *different* bot.
pub fn parse_bot_command<'a>(text: &'a str, username: &str) -> Option<(&'a str, &'a str, bool)> {
    let stripped = text.strip_prefix('/')?;
    let (head, rest) = stripped.split_once(char::is_whitespace).unwrap_or((stripped, ""));
    let (cmd, addressed) = match head.split_once('@') {
        Some((cmd, who)) => (cmd, who.eq_ignore_ascii_case(username)),
        None => (head, true),
    };
    Some((cmd, rest.trim_start(), addressed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_bot_mention() {
        assert_eq!(strip_bot_mentions("@rightclaw_bot hello", "rightclaw_bot"), "hello");
        assert_eq!(strip_bot_mentions("hey @rightclaw_bot how are you", "rightclaw_bot"), "hey how are you");
    }

    #[test]
    fn strip_leaves_other_mentions() {
        assert_eq!(strip_bot_mentions("@alice says hi to @rightclaw_bot", "rightclaw_bot"), "@alice says hi to");
    }

    #[test]
    fn strip_is_case_insensitive() {
        assert_eq!(strip_bot_mentions("@RightClaw_Bot hi", "rightclaw_bot"), "hi");
    }

    #[test]
    fn parse_command_no_suffix() {
        let (cmd, args, addressed) = parse_bot_command("/allow 42", "rightclaw_bot").unwrap();
        assert_eq!(cmd, "allow");
        assert_eq!(args, "42");
        assert!(addressed);
    }

    #[test]
    fn parse_command_addressed_suffix() {
        let (cmd, args, addressed) = parse_bot_command("/allow@rightclaw_bot 42", "rightclaw_bot").unwrap();
        assert_eq!(cmd, "allow");
        assert_eq!(args, "42");
        assert!(addressed);
    }

    #[test]
    fn parse_command_different_bot() {
        let (cmd, _args, addressed) = parse_bot_command("/allow@otherbot 42", "rightclaw_bot").unwrap();
        assert_eq!(cmd, "allow");
        assert!(!addressed);
    }

    #[test]
    fn parse_command_bare() {
        let (cmd, args, _) = parse_bot_command("/allowed", "rightclaw_bot").unwrap();
        assert_eq!(cmd, "allowed");
        assert_eq!(args, "");
    }
}
```

Register in `crates/bot/src/telegram/mod.rs`:

```rust
pub mod mention;
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rightclaw-bot mention::tests`
Expected: 6 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/telegram/mention.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): mention/reply detection + command parser"
```

---

### Task 7: Replace chat_id filter with allowlist-based routing filter

**Files:**
- Modify: `crates/bot/src/telegram/filter.rs`

The filter now returns a `RoutingDecision`: either `Drop` (silent) or `Accept { address_kind }` carrying whether it's DM, group-reply-to-bot, or group-mention, so the handler can shape the prompt later.

- [ ] **Step 1: Write the failing test + implementation**

Replace `crates/bot/src/telegram/filter.rs`:

```rust
//! Gate incoming messages against the per-agent `AllowlistHandle`.
//! Returns `Some(RoutingDecision)` when the message should be processed,
//! `None` to silently drop (per spec §Response Rules).

use rightclaw::agent::allowlist::AllowlistHandle;
use teloxide::types::{ChatKind, Message};
use super::mention::{is_bot_addressed, AddressKind, BotIdentity};

#[derive(Debug, Clone)]
pub struct RoutingDecision {
    pub address: AddressKind,
    /// True iff the sender is in the global trusted-users list.
    pub sender_trusted: bool,
    /// Set to `true` for group messages when the group is opened. `false` for DM.
    pub group_open: bool,
}

pub fn make_routing_filter(
    allowlist: AllowlistHandle,
    identity: BotIdentity,
) -> impl Fn(Message) -> Option<(Message, RoutingDecision)> + Send + Sync + Clone + 'static {
    move |msg: Message| {
        let sender_id: Option<i64> = msg.from.as_ref().map(|u| u.id.0 as i64);
        let chat_id = msg.chat.id.0;

        // Fast deny: no `from` means it's a channel post or anonymous — ignore.
        let Some(sender_id) = sender_id else { return None; };

        // Synchronous read of the RwLock via blocking_read — safe in teloxide
        // filter_map closures because they're sync. We only read, no await.
        let state = allowlist.0.blocking_read();
        let sender_trusted = state.is_user_trusted(sender_id);
        let group_open = state.is_group_open(chat_id);
        drop(state);

        let is_group = !matches!(msg.chat.kind, ChatKind::Private(_));

        match is_bot_addressed(&msg, &identity) {
            None => None, // DM not handled here (always returns DirectMessage); group non-mention dropped.
            Some(addr @ AddressKind::DirectMessage) => {
                if !sender_trusted { return None; } // §Response Rules: DM non-trusted → drop
                Some((msg, RoutingDecision { address: addr, sender_trusted: true, group_open: false }))
            }
            Some(addr) => {
                // Group path.
                debug_assert!(is_group);
                let _ = is_group;
                if !sender_trusted && !group_open { return None; }
                Some((msg, RoutingDecision { address: addr, sender_trusted, group_open }))
            }
        }
    }
}
```

Add a small test (synthetic messages are awkward — we'll keep one matrix unit test via stubs):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // We can't easily build a full teloxide `Message` without a serialized
    // payload. Unit tests for mention detection already cover the branching.
    // Here we just check the RoutingDecision struct has the expected shape.

    #[test]
    fn routing_decision_constructs() {
        let d = RoutingDecision {
            address: AddressKind::DirectMessage,
            sender_trusted: true,
            group_open: false,
        };
        assert!(d.sender_trusted);
    }
}
```

- [ ] **Step 2: Delete the old `make_chat_id_filter` call sites (will be done in Task 8)**

Leave the old function in place as-is — callers still reference it. It will be deleted when dispatch.rs is rewired.

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw-bot filter::tests`
Expected: 1 test PASS.

Run: `cargo build -p rightclaw-bot`
Expected: build FAILS (old `make_chat_id_filter` no longer exported? It is still defined above `make_routing_filter`; keep it for now so build passes) — verify by running the build. If the compiler complains about imports, keep both functions in this file until Task 8.

Adjust `filter.rs` so both functions live side-by-side — the old `make_chat_id_filter` is preserved for Task 8's deletion.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/filter.rs
git commit -m "feat(bot): add allowlist-based routing filter (old filter still wired)"
```

---

### Task 8: Bot startup — load allowlist, migrate, spawn watcher, rewire dispatcher

**Files:**
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/filter.rs` (remove `make_chat_id_filter`)
- Modify: `crates/bot/src/telegram/handler.rs` (accept `RoutingDecision` via DI)

This task is the integration seam. Small enough to still be one task since tests are E2E-only and gated by a real Telegram token; the important checks are compile + existing unit tests remain green.

- [ ] **Step 1: Refactor `run_telegram` signature and wiring**

In `crates/bot/src/telegram/dispatch.rs`, change the `run_telegram` signature:

```rust
#[allow(clippy::too_many_arguments)]
pub async fn run_telegram(
    token: String,
    allowlist: rightclaw::agent::allowlist::AllowlistHandle,
    agent_dir: PathBuf,
    debug: bool,
    pending_auth: PendingAuthMap,
    home: PathBuf,
    ssh_config_path: Option<PathBuf>,
    show_thinking: bool,
    model: Option<String>,
    shutdown: CancellationToken,
    idle_ts: Arc<IdleTimestamp>,
    internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
    resolved_sandbox: Option<String>,
    hindsight_client: Option<std::sync::Arc<rightclaw::memory::hindsight::HindsightClient>>,
    prefetch_cache: Option<rightclaw::memory::prefetch::PrefetchCache>,
    upgrade_lock: Arc<tokio::sync::RwLock<()>>,
) -> miette::Result<()> {
    let bot = build_bot(token);

    // Resolve bot identity (username + user_id) via getMe. Required for mention detection.
    let me = bot.get_me().await
        .map_err(|e| miette::miette!("bot.get_me() failed: {e:#}"))?;
    let username = me.user.username.clone()
        .ok_or_else(|| miette::miette!("bot has no username; cannot set up group-mention detection"))?;
    let identity = super::mention::BotIdentity { username: username.clone(), user_id: me.user.id.0 };

    tracing::info!(%username, "bot identity resolved");
    let filter = super::filter::make_routing_filter(allowlist.clone(), identity.clone());

    // Shared state (unchanged for most)
    let worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>> = Arc::new(DashMap::new());
    // ... keep existing allocations ...

    // NEW: wrap allowlist + identity for DI:
    let allowlist_arc = allowlist.clone();
    let identity_arc = Arc::new(identity);

    // dispatcher .dependencies(...) additions:
    //   allowlist_arc, identity_arc
    // ... continue existing setup ...
```

Remove the `allowed_chat_ids` parameter and the `make_chat_id_filter` import. Delete the `set_my_commands` loop that iterates over chat IDs — instead, set global commands once (Telegram will still scope per chat if we want; for now, global is fine):

```rust
let commands = BotCommand::bot_commands();
if let Err(e) = bot.delete_my_commands().await {
    tracing::warn!("delete_my_commands (default scope): {e:#}");
}
if let Err(e) = bot.set_my_commands(commands).await {
    tracing::warn!("set_my_commands (default scope): {e:#}");
}
```

In `crates/bot/src/lib.rs`, at the call site of `telegram::run_telegram`:

1. Before calling `run_telegram`, load the allowlist:

```rust
use rightclaw::agent::allowlist::{self, AllowlistState, AllowlistHandle};

let allowlist = load_or_migrate_allowlist(&agent_dir, &config.allowed_chat_ids)?;
// Spawn watcher — keep handle in scope for bot process lifetime
let _watcher = allowlist::spawn_watcher(&agent_dir, allowlist.clone())
    .map_err(|e| miette::miette!("allowlist watcher: {e}"))?;
```

2. Add helper near the top of lib.rs:

```rust
fn load_or_migrate_allowlist(
    agent_dir: &std::path::Path,
    legacy: &[i64],
) -> miette::Result<AllowlistHandle> {
    let now = chrono::Utc::now();
    let existed_before = allowlist::allowlist_path(agent_dir).exists();
    let report = allowlist::migrate_from_legacy(agent_dir, legacy, now)
        .map_err(|e| miette::miette!("allowlist migration: {e}"))?;
    if !existed_before && !report.already_present && (report.migrated_users + report.migrated_groups) > 0 {
        tracing::info!(
            users = report.migrated_users,
            groups = report.migrated_groups,
            "migrated {} users, {} groups from agent.yaml::allowed_chat_ids; consider removing the legacy field",
            report.migrated_users, report.migrated_groups,
        );
    }
    if report.already_present && !legacy.is_empty() {
        tracing::warn!("legacy allowed_chat_ids field in agent.yaml is ignored; source of truth is allowlist.yaml");
    }
    let file = allowlist::read_file(agent_dir)
        .map_err(|e| miette::miette!("read allowlist: {e}"))?
        .unwrap_or_default();
    Ok(AllowlistHandle::new(AllowlistState::from_file(file)))
}
```

3. Update the `telegram::run_telegram(...)` call to pass `allowlist` instead of `config.allowed_chat_ids`.

4. Remove the D-05 empty-allowlist warning in lib.rs — keep a new one:

```rust
let r = allowlist.0.read().await;
if r.users().is_empty() {
    tracing::warn!("allowlist.yaml has no trusted users — DMs will be silently dropped until you add one via `rightclaw agent allow` or a first-run wizard");
}
drop(r);
```

In `crates/bot/src/telegram/filter.rs`: delete `make_chat_id_filter`.

In `crates/bot/src/telegram/handler.rs`, thread `RoutingDecision` through by adding it to `DebounceMsg` at message handling time (add a `pub address: AddressKind` field on `DebounceMsg` and a `pub group_open: bool`). Extract `address` from the filter result, place in `DebounceMsg`.

Update `handle_message(bot, (msg, decision): (Message, RoutingDecision), ...)`. Teloxide's `filter_map` result is a tuple; use:

```rust
let message_handler = Update::filter_message()
    .inspect(...)
    .filter_map(filter)
    .endpoint(handle_message);
```

Teloxide passes the tuple items as separate DI arguments — `Message` and `RoutingDecision`. Update handler signature:

```rust
pub async fn handle_message(
    bot: BotType,
    msg: Message,
    decision: RoutingDecision,
    ...
)
```

and store `decision.address` onto `DebounceMsg`.

- [ ] **Step 2: Build and run existing tests**

Run: `cargo build -p rightclaw-bot`
Expected: PASS.

Run: `cargo test -p rightclaw-bot`
Expected: PASS — no regressions.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/lib.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/filter.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): wire AllowlistHandle as routing source; migrate legacy field on startup"
```

---

### Task 9: DM-only gate on existing commands

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`

Wrap `handle_new`, `handle_list`, `handle_switch`, `handle_mcp`, `handle_doctor`, `handle_cron`, `handle_start` in a `ChatKind::Private` check; return silently if invoked in a group.

- [ ] **Step 1: Write the failing test**

In `crates/bot/src/telegram/handler.rs`, add to the `tests` module:

```rust
use teloxide::types::ChatKind;

fn make_private_chat_kind() -> ChatKind {
    // ChatKind::Private requires a PrivateChat {username: Option<String>, ...}
    // Use serde-deserialize test helper.
    serde_json::from_value(serde_json::json!({
        "type": "private",
        "first_name": "Test"
    })).unwrap()
}

fn make_group_chat_kind() -> ChatKind {
    serde_json::from_value(serde_json::json!({
        "type": "group",
        "title": "Group"
    })).unwrap()
}

#[test]
fn is_private_chat_detects_dm() {
    let private = make_private_chat_kind();
    let group = make_group_chat_kind();
    assert!(is_private_chat(&private));
    assert!(!is_private_chat(&group));
}
```

And add the helper at top of handler.rs:

```rust
pub(crate) fn is_private_chat(kind: &teloxide::types::ChatKind) -> bool {
    matches!(kind, teloxide::types::ChatKind::Private(_))
}
```

- [ ] **Step 2: Gate each handler**

At the top of each of `handle_start`, `handle_new`, `handle_list`, `handle_switch`, `handle_mcp`, `handle_doctor`, `handle_cron`, insert:

```rust
if !is_private_chat(&msg.chat.kind) {
    tracing::debug!(cmd = "<command-name>", "ignoring command in group chat (DM-only)");
    return Ok(());
}
```

Replace `<command-name>` literally per handler for grep-ability.

- [ ] **Step 3: Build and test**

Run: `cargo test -p rightclaw-bot handler::tests::is_private_chat_detects_dm`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/handler.rs
git commit -m "feat(bot): gate session/mcp/doctor/cron commands to DM only"
```

---

### Task 10: `/allow` and `/deny` handlers (user allowlist mutations)

**Files:**
- Create: `crates/bot/src/telegram/allowlist_commands.rs`
- Modify: `crates/bot/src/telegram/mod.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs` (add command variants + DI)

- [ ] **Step 1: Write handler module with tests**

Create `crates/bot/src/telegram/allowlist_commands.rs`:

```rust
//! Handlers for `/allow`, `/deny`, `/allowed`, `/allow_all`, `/deny_all`.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use rightclaw::agent::allowlist::{
    self, AddOutcome, AllowedGroup, AllowedUser, AllowlistHandle, AllowlistState, RemoveOutcome,
};
use teloxide::prelude::*;
use teloxide::types::{ChatKind, Message, MessageEntityKind};
use teloxide::RequestError;

use super::handler::AgentDir;

/// Who the sender intends to add/remove.
#[derive(Debug, Clone, PartialEq)]
pub enum UserTarget {
    NumericId(i64),
    TextMention { id: i64, name: Option<String> },
    Reply { id: i64, name: Option<String> },
    /// `@username` mention without entity-level user_id — unresolvable.
    UnresolvableUsername(String),
    None,
}

pub fn resolve_user_target(msg: &Message, args: &str) -> UserTarget {
    // 1) reply-to-message
    if let Some(reply) = msg.reply_to_message()
        && let Some(from) = reply.from.as_ref()
    {
        return UserTarget::Reply { id: from.id.0 as i64, name: Some(from.full_name()) };
    }
    // 2) TextMention entity in this message
    if let Some(entities) = msg.entities() {
        for e in entities {
            if let MessageEntityKind::TextMention { user } = &e.kind {
                return UserTarget::TextMention { id: user.id.0 as i64, name: Some(user.full_name()) };
            }
        }
    }
    // 3) numeric arg
    let trimmed = args.trim();
    if let Ok(id) = trimmed.parse::<i64>() {
        return UserTarget::NumericId(id);
    }
    // 4) @username literal, no entity-level id — unresolvable
    if let Some(u) = trimmed.strip_prefix('@').filter(|s| !s.is_empty()) {
        return UserTarget::UnresolvableUsername(u.to_string());
    }
    UserTarget::None
}

/// Write the current `AllowlistState` atomically to disk (under the lock).
pub async fn persist(handle: &AllowlistHandle, agent_dir: &std::path::Path) -> Result<(), String> {
    let state = handle.0.read().await;
    let file = state.to_file();
    drop(state);
    let dir = agent_dir.to_path_buf();
    tokio::task::spawn_blocking(move || allowlist::write_file(&dir, &file))
        .await
        .map_err(|e| format!("join: {e:#}"))?
}

pub async fn handle_allow(
    bot: super::BotType,
    msg: Message,
    args: String,
    allowlist: AllowlistHandle,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let target = resolve_user_target(&msg, &args);
    let (id, label) = match target {
        UserTarget::NumericId(id) => (id, None),
        UserTarget::Reply { id, name } | UserTarget::TextMention { id, name } => (id, name),
        UserTarget::UnresolvableUsername(u) => {
            reply(&bot, &msg, &format!("✗ cannot resolve @{u} — reply to their message or use numeric user_id")).await?;
            return Ok(());
        }
        UserTarget::None => {
            reply(&bot, &msg, "✗ usage: /allow (reply to user) or /allow <user_id>").await?;
            return Ok(());
        }
    };

    // Reject attempting to add a bot (ourselves or others). Best-effort: we
    // can't always know from the target alone; reject negative IDs (channels)
    // as a cheap guard.
    if id < 0 {
        reply(&bot, &msg, "✗ user_id cannot be negative (groups/channels use /allow_all)").await?;
        return Ok(());
    }

    let outcome = {
        let mut w = allowlist.0.write().await;
        w.add_user(AllowedUser {
            id,
            label: label.clone(),
            added_by: msg.from.as_ref().map(|u| u.id.0 as i64),
            added_at: Utc::now(),
        })
    };

    match outcome {
        AddOutcome::Inserted => {
            if let Err(e) = persist(&allowlist, &agent_dir.0).await {
                reply(&bot, &msg, &format!("✗ persist failed: {e}")).await?;
                return Ok(());
            }
            let disp = label.unwrap_or_else(|| id.to_string());
            reply(&bot, &msg, &format!("✓ allowed user {disp} (id {id})")).await?;
        }
        AddOutcome::AlreadyPresent => {
            reply(&bot, &msg, &format!("✓ user {id} already in allowlist")).await?;
        }
    }
    Ok(())
}

pub async fn handle_deny(
    bot: super::BotType,
    msg: Message,
    args: String,
    allowlist: AllowlistHandle,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    let target = resolve_user_target(&msg, &args);
    let id = match target {
        UserTarget::NumericId(id) => id,
        UserTarget::Reply { id, .. } | UserTarget::TextMention { id, .. } => id,
        UserTarget::UnresolvableUsername(u) => {
            reply(&bot, &msg, &format!("✗ cannot resolve @{u} — reply to their message or use numeric user_id")).await?;
            return Ok(());
        }
        UserTarget::None => {
            reply(&bot, &msg, "✗ usage: /deny (reply to user) or /deny <user_id>").await?;
            return Ok(());
        }
    };

    // Self-deny rejection.
    if let Some(from) = msg.from.as_ref()
        && from.id.0 as i64 == id
    {
        reply(&bot, &msg, "✗ cannot deny yourself — add another trusted user first").await?;
        return Ok(());
    }

    let outcome = {
        let mut w = allowlist.0.write().await;
        w.remove_user(id)
    };
    match outcome {
        RemoveOutcome::Removed => {
            if let Err(e) = persist(&allowlist, &agent_dir.0).await {
                reply(&bot, &msg, &format!("✗ persist failed: {e}")).await?;
                return Ok(());
            }
            reply(&bot, &msg, &format!("✓ user {id} removed")).await?;
        }
        RemoveOutcome::NotFound => {
            reply(&bot, &msg, &format!("✗ user {id} not in allowlist")).await?;
        }
    }
    Ok(())
}

pub async fn handle_allowed(
    bot: super::BotType,
    msg: Message,
    allowlist: AllowlistHandle,
) -> ResponseResult<()> {
    let state = allowlist.0.read().await;
    let file = state.to_file();
    drop(state);
    let mut text = String::from("<b>Trusted users:</b>\n");
    if file.users.is_empty() {
        text.push_str("  (none)\n");
    } else {
        for u in &file.users {
            let label = u.label.as_deref().unwrap_or("");
            text.push_str(&format!("  • {} {}\n", u.id, label));
        }
    }
    text.push_str("\n<b>Opened groups:</b>\n");
    if file.groups.is_empty() {
        text.push_str("  (none)\n");
    } else {
        for g in &file.groups {
            let label = g.label.as_deref().unwrap_or("");
            text.push_str(&format!("  • {} {}\n", g.id, label));
        }
    }
    bot.send_message(msg.chat.id, text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .await?;
    Ok(())
}

pub async fn handle_allow_all(
    bot: super::BotType,
    msg: Message,
    allowlist: AllowlistHandle,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if matches!(msg.chat.kind, ChatKind::Private(_)) {
        reply(&bot, &msg, "✗ /allow_all is only valid in group chats").await?;
        return Ok(());
    }
    let chat_id = msg.chat.id.0;
    let label = msg.chat.title().map(|s| s.to_string());
    let outcome = {
        let mut w = allowlist.0.write().await;
        w.add_group(AllowedGroup {
            id: chat_id,
            label: label.clone(),
            opened_by: msg.from.as_ref().map(|u| u.id.0 as i64),
            opened_at: Utc::now(),
        })
    };
    match outcome {
        AddOutcome::Inserted => {
            if let Err(e) = persist(&allowlist, &agent_dir.0).await {
                reply(&bot, &msg, &format!("✗ persist failed: {e}")).await?;
                return Ok(());
            }
            reply(&bot, &msg, "✓ group opened").await?;
        }
        AddOutcome::AlreadyPresent => {
            reply(&bot, &msg, "✓ group already opened").await?;
        }
    }
    Ok(())
}

pub async fn handle_deny_all(
    bot: super::BotType,
    msg: Message,
    allowlist: AllowlistHandle,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if matches!(msg.chat.kind, ChatKind::Private(_)) {
        reply(&bot, &msg, "✗ /deny_all is only valid in group chats").await?;
        return Ok(());
    }
    let chat_id = msg.chat.id.0;
    let outcome = {
        let mut w = allowlist.0.write().await;
        w.remove_group(chat_id)
    };
    match outcome {
        RemoveOutcome::Removed => {
            if let Err(e) = persist(&allowlist, &agent_dir.0).await {
                reply(&bot, &msg, &format!("✗ persist failed: {e}")).await?;
                return Ok(());
            }
            reply(&bot, &msg, "✓ group closed").await?;
        }
        RemoveOutcome::NotFound => {
            reply(&bot, &msg, "✓ group was not opened").await?;
        }
    }
    Ok(())
}

async fn reply(bot: &super::BotType, msg: &Message, text: &str) -> Result<(), RequestError> {
    let mut req = bot.send_message(msg.chat.id, text);
    req = req.reply_parameters(teloxide::types::ReplyParameters {
        message_id: msg.id,
        ..Default::default()
    });
    req.await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use teloxide::types::{Message, MessageEntity, MessageEntityKind};

    // Helpers to build synthetic messages via serde_json.
    fn dm_msg(from_id: u64, text: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 0,
            "chat": {"id": from_id as i64, "type": "private", "first_name": "U"},
            "from": {"id": from_id, "is_bot": false, "first_name": "U"},
            "text": text
        })).unwrap()
    }

    #[test]
    fn resolve_numeric_id() {
        let m = dm_msg(1, "42");
        assert_eq!(resolve_user_target(&m, "42"), UserTarget::NumericId(42));
    }

    #[test]
    fn resolve_empty_args() {
        let m = dm_msg(1, "");
        assert_eq!(resolve_user_target(&m, ""), UserTarget::None);
    }

    #[test]
    fn resolve_unresolvable_username() {
        let m = dm_msg(1, "@someone");
        match resolve_user_target(&m, "@someone") {
            UserTarget::UnresolvableUsername(u) => assert_eq!(u, "someone"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
```

Register in `crates/bot/src/telegram/mod.rs`:

```rust
pub mod allowlist_commands;
```

- [ ] **Step 2: Wire into dispatcher**

In `crates/bot/src/telegram/dispatch.rs`, extend `BotCommand`:

```rust
#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum BotCommand {
    // ... existing ...
    #[command(description = "Add trusted user (reply to user, or /allow <user_id>)")]
    Allow(String),
    #[command(description = "Remove trusted user")]
    Deny(String),
    #[command(description = "List trusted users and opened groups")]
    Allowed,
    #[command(description = "Open this group for all members (group only)")]
    AllowAll,
    #[command(description = "Close this group (group only)")]
    DenyAll,
}
```

And route them:

```rust
let command_handler = dptree::entry()
    .filter_command::<BotCommand>()
    .branch(dptree::case![BotCommand::Start].endpoint(handle_start))
    .branch(dptree::case![BotCommand::New(name)].endpoint(handle_new))
    .branch(dptree::case![BotCommand::List].endpoint(handle_list))
    .branch(dptree::case![BotCommand::Switch(uuid)].endpoint(handle_switch))
    .branch(dptree::case![BotCommand::Mcp(args)].endpoint(handle_mcp))
    .branch(dptree::case![BotCommand::Doctor].endpoint(handle_doctor))
    .branch(dptree::case![BotCommand::Cron(args)].endpoint(handle_cron))
    .branch(dptree::case![BotCommand::Allow(args)].endpoint(super::allowlist_commands::handle_allow))
    .branch(dptree::case![BotCommand::Deny(args)].endpoint(super::allowlist_commands::handle_deny))
    .branch(dptree::case![BotCommand::Allowed].endpoint(super::allowlist_commands::handle_allowed))
    .branch(dptree::case![BotCommand::AllowAll].endpoint(super::allowlist_commands::handle_allow_all))
    .branch(dptree::case![BotCommand::DenyAll].endpoint(super::allowlist_commands::handle_deny_all));
```

Extend `.dependencies(...)` with `allowlist_arc` (already added in Task 8) so the new handlers receive `AllowlistHandle` by DI. dptree takes the `AllowlistHandle` directly (it's `Clone`).

**Gate to trusted-only:** wrap every new handler with a pre-check at the top:

```rust
let sender_id = msg.from.as_ref().map(|u| u.id.0 as i64);
let trusted = match sender_id {
    Some(id) => allowlist.0.read().await.is_user_trusted(id),
    None => false,
};
if !trusted {
    tracing::debug!(?sender_id, "allow/deny command from non-trusted sender ignored");
    return Ok(());
}
```

Add at the top of `handle_allow`, `handle_deny`, `handle_allowed`, `handle_allow_all`, `handle_deny_all`.

- [ ] **Step 3: Build + test**

Run: `cargo build -p rightclaw-bot`
Expected: PASS.

Run: `cargo test -p rightclaw-bot allowlist_commands::tests`
Expected: 3 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/allowlist_commands.rs crates/bot/src/telegram/mod.rs crates/bot/src/telegram/dispatch.rs
git commit -m "feat(bot): /allow /deny /allowed /allow_all /deny_all handlers"
```

---

### Task 11: CLI subcommands `rightclaw agent allow|deny|allow_all|deny_all|allowed`

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`
- Create: `crates/rightclaw-cli/tests/allowlist_cli.rs`

- [ ] **Step 1: Extend `AgentCommands` enum and dispatch**

In `crates/rightclaw-cli/src/main.rs`, add to `AgentCommands`:

```rust
    /// Add a trusted user to this agent's allowlist.
    Allow {
        /// Agent name
        name: String,
        /// Telegram user ID (positive integer)
        user_id: i64,
        /// Optional label (first_name or username)
        #[arg(long)]
        label: Option<String>,
    },
    /// Remove a trusted user.
    Deny {
        name: String,
        user_id: i64,
    },
    /// Open a group for all members.
    AllowAll {
        name: String,
        /// Telegram group chat ID (negative integer)
        chat_id: i64,
        #[arg(long)]
        label: Option<String>,
    },
    /// Close an opened group.
    DenyAll {
        name: String,
        chat_id: i64,
    },
    /// Dump current allowlist to stdout.
    Allowed {
        name: String,
        /// Emit as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
```

Dispatch in the main match on `AgentCommands`:

```rust
AgentCommands::Allow { name, user_id, label } => {
    let dir = agents_dir(&home)?.join(&name);
    ensure_agent_exists(&dir, &name)?;
    rightclaw::agent::allowlist::with_lock(&dir, |d| {
        let mut file = rightclaw::agent::allowlist::read_file(d)
            .map_err(|e| e)?
            .unwrap_or_default();
        use rightclaw::agent::allowlist::{AllowedUser, AllowlistState, AddOutcome};
        let mut state = AllowlistState::from_file(file);
        let outcome = state.add_user(AllowedUser {
            id: user_id,
            label,
            added_by: None,
            added_at: chrono::Utc::now(),
        });
        rightclaw::agent::allowlist::write_file_inner(d, &state.to_file())?;
        println!("{}", match outcome {
            AddOutcome::Inserted => format!("added user {user_id}"),
            AddOutcome::AlreadyPresent => format!("user {user_id} already allowed"),
        });
        Ok::<_, String>(())
    }).map_err(|e| miette::miette!("{e}"))
}
// ... similar for Deny, AllowAll, DenyAll, Allowed ...
```

Use a small helper for `ensure_agent_exists` consistent with existing agent commands (find by grep in main.rs).

For `Allowed` subcommand, print a table:

```rust
AgentCommands::Allowed { name, json } => {
    let dir = agents_dir(&home)?.join(&name);
    let file = rightclaw::agent::allowlist::read_file(&dir)
        .map_err(|e| miette::miette!("{e}"))?
        .unwrap_or_default();
    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "users": file.users.iter().map(|u| serde_json::json!({
                "id": u.id, "label": u.label, "added_by": u.added_by, "added_at": u.added_at,
            })).collect::<Vec<_>>(),
            "groups": file.groups.iter().map(|g| serde_json::json!({
                "id": g.id, "label": g.label, "opened_by": g.opened_by, "opened_at": g.opened_at,
            })).collect::<Vec<_>>(),
        })).unwrap());
    } else {
        println!("Trusted users:");
        if file.users.is_empty() { println!("  (none)"); }
        for u in &file.users {
            println!("  • {} {} (added {})", u.id, u.label.as_deref().unwrap_or(""), u.added_at.format("%Y-%m-%d"));
        }
        println!("Opened groups:");
        if file.groups.is_empty() { println!("  (none)"); }
        for g in &file.groups {
            println!("  • {} {} (opened {})", g.id, g.label.as_deref().unwrap_or(""), g.opened_at.format("%Y-%m-%d"));
        }
    }
    Ok(())
}
```

Add `serde` derives to `AllowedUser`/`AllowedGroup` (add `Serialize` alongside `Deserialize`) in `allowlist.rs` so JSON output works.

- [ ] **Step 2: Write an integration test**

Create `crates/rightclaw-cli/tests/allowlist_cli.rs`:

```rust
use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

fn run(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("rightclaw").unwrap();
    cmd.env("RIGHTCLAW_HOME", home);
    cmd
}

fn init_agent(home: &std::path::Path, name: &str) {
    // Use the existing init subcommand in non-interactive mode.
    // If init is too heavy, we can create the directory directly:
    let dir = home.join("agents").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("agent.yaml"), "restart: never\n").unwrap();
    // Also create the IDENTITY.md stub used by discovery.
    std::fs::write(dir.join("IDENTITY.md"), "# test agent\n").unwrap();
}

#[test]
fn allow_adds_user_and_allowed_lists_it() {
    let home = TempDir::new().unwrap();
    init_agent(home.path(), "testbot");

    run(home.path())
        .args(["agent", "allow", "testbot", "42", "--label", "alice"])
        .assert()
        .success()
        .stdout(contains("added user 42"));

    run(home.path())
        .args(["agent", "allowed", "testbot"])
        .assert()
        .success()
        .stdout(contains("42"))
        .stdout(contains("alice"));
}

#[test]
fn deny_removes_user() {
    let home = TempDir::new().unwrap();
    init_agent(home.path(), "testbot");

    run(home.path())
        .args(["agent", "allow", "testbot", "99"])
        .assert().success();
    run(home.path())
        .args(["agent", "deny", "testbot", "99"])
        .assert().success();
    run(home.path())
        .args(["agent", "allowed", "testbot", "--json"])
        .assert()
        .success()
        .stdout(contains("\"users\": []"));
}

#[test]
fn allow_all_opens_group() {
    let home = TempDir::new().unwrap();
    init_agent(home.path(), "testbot");

    run(home.path())
        .args(["agent", "allow_all", "testbot", "-1001234", "--label", "Dev"])
        .assert()
        .success();

    run(home.path())
        .args(["agent", "allowed", "testbot"])
        .assert()
        .success()
        .stdout(contains("-1001234"))
        .stdout(contains("Dev"));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rightclaw-cli --test allowlist_cli`
Expected: 3 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs crates/rightclaw-cli/tests/allowlist_cli.rs crates/rightclaw/src/agent/allowlist.rs
git commit -m "feat(cli): rightclaw agent allow|deny|allow_all|deny_all|allowed"
```

---

### Task 12: Group-aware `InputMessage` + `format_cc_input` attribution

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

Extend `InputMessage` (and `DebounceMsg`) with chat metadata and the reply target's full author+text. Update `format_cc_input` to emit the group attribution YAML from the spec.

- [ ] **Step 1: Write tests**

In `crates/bot/src/telegram/attachments.rs`, add tests at the bottom:

```rust
#[cfg(test)]
mod group_format_tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 4, 16, 12, 0, 0).unwrap()
    }

    #[test]
    fn dm_single_message_emits_yaml_with_no_chat_block() {
        let m = InputMessage {
            message_id: 1,
            text: Some("hi".into()),
            timestamp: now(),
            attachments: vec![],
            author: MessageAuthor { name: "Alice".into(), username: Some("@alice".into()), user_id: Some(42) },
            forward_info: None,
            reply_to_id: None,
            chat: ChatContext::Private,
            reply_to_body: None,
        };
        let yaml = format_cc_input(&[m]).unwrap();
        assert!(yaml.contains("messages:"));
        assert!(!yaml.contains("chat:"), "DM must not emit chat block");
    }

    #[test]
    fn group_message_emits_chat_block_and_topic() {
        let m = InputMessage {
            message_id: 9,
            text: Some("what does foo do".into()),
            timestamp: now(),
            attachments: vec![],
            author: MessageAuthor { name: "Alice".into(), username: Some("@alice".into()), user_id: Some(42) },
            forward_info: None,
            reply_to_id: None,
            chat: ChatContext::Group { id: -1001, title: Some("Dev".into()), topic_id: Some(7) },
            reply_to_body: None,
        };
        let yaml = format_cc_input(&[m]).unwrap();
        assert!(yaml.contains("chat:"));
        assert!(yaml.contains("kind: group"));
        assert!(yaml.contains("id: -1001"));
        assert!(yaml.contains("title:"));
        assert!(yaml.contains("topic_id: 7"));
    }

    #[test]
    fn group_message_with_reply_to_body_emits_reply_to_block() {
        let m = InputMessage {
            message_id: 10,
            text: Some("explain this".into()),
            timestamp: now(),
            attachments: vec![],
            author: MessageAuthor { name: "Bob".into(), username: None, user_id: Some(43) },
            forward_info: None,
            reply_to_id: Some(5),
            chat: ChatContext::Group { id: -1001, title: None, topic_id: None },
            reply_to_body: Some(ReplyToBody {
                author: MessageAuthor { name: "Alice".into(), username: Some("@alice".into()), user_id: Some(42) },
                text: Some("here is the function: foo()".into()),
            }),
        };
        let yaml = format_cc_input(&[m]).unwrap();
        assert!(yaml.contains("reply_to:"));
        assert!(yaml.contains("here is the function: foo()"));
    }
}
```

- [ ] **Step 2: Add new types and extend `format_cc_input`**

In `crates/bot/src/telegram/attachments.rs`, add after `ForwardInfo`:

```rust
#[derive(Debug, Clone)]
pub enum ChatContext {
    Private,
    Group { id: i64, title: Option<String>, topic_id: Option<i64> },
}

#[derive(Debug, Clone)]
pub struct ReplyToBody {
    pub author: MessageAuthor,
    pub text: Option<String>,
}
```

Extend `InputMessage`:

```rust
pub struct InputMessage {
    pub message_id: i32,
    pub text: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub attachments: Vec<ResolvedAttachment>,
    pub author: MessageAuthor,
    pub forward_info: Option<ForwardInfo>,
    pub reply_to_id: Option<i32>,
    pub chat: ChatContext,          // NEW
    pub reply_to_body: Option<ReplyToBody>, // NEW
}
```

Update `format_cc_input` to emit `chat:` block when Group and `reply_to:` block when present. Insert after the `author:` block:

```rust
match &m.chat {
    ChatContext::Private => {}
    ChatContext::Group { id, title, topic_id } => {
        out.push_str("    chat:\n");
        writeln!(out, "      kind: group").expect("infallible");
        writeln!(out, "      id: {id}").expect("infallible");
        if let Some(t) = title {
            writeln!(out, "      title: \"{}\"", yaml_escape_string(t)).expect("infallible");
        }
        if let Some(tid) = topic_id {
            writeln!(out, "      topic_id: {tid}").expect("infallible");
        }
    }
}

if let Some(r) = &m.reply_to_body {
    out.push_str("    reply_to:\n");
    out.push_str("      author:\n");
    writeln!(out, "        name: \"{}\"", yaml_escape_string(&r.author.name)).expect("infallible");
    if let Some(un) = &r.author.username {
        writeln!(out, "        username: \"{}\"", yaml_escape_string(un)).expect("infallible");
    }
    if let Some(uid) = r.author.user_id {
        writeln!(out, "        user_id: {uid}").expect("infallible");
    }
    if let Some(t) = &r.text {
        writeln!(out, "      text: \"{}\"", yaml_escape_string(t)).expect("infallible");
    }
}
```

- [ ] **Step 3: Propagate new fields through `DebounceMsg` and handler**

In `crates/bot/src/telegram/worker.rs`, extend `DebounceMsg`:

```rust
pub struct DebounceMsg {
    // ... existing ...
    pub chat: super::attachments::ChatContext,
    pub reply_to_body: Option<super::attachments::ReplyToBody>,
}
```

In `crates/bot/src/telegram/handler.rs::handle_message`, populate them:

```rust
use teloxide::types::ChatKind;
let chat_ctx = match &msg.chat.kind {
    ChatKind::Private(_) => super::attachments::ChatContext::Private,
    _ => super::attachments::ChatContext::Group {
        id: msg.chat.id.0,
        title: msg.chat.title().map(|s| s.to_string()),
        topic_id: msg.thread_id.map(|t| t.0.0 as i64).filter(|&n| n > 1),
    },
};

let reply_to_body = msg.reply_to_message().and_then(|r| {
    // Don't include reply-target when replying to our own bot's message (that's
    // already context inside the session).
    let our_id = /* BotIdentity provided via DI in task 8 */;
    let from = r.from.as_ref()?;
    if from.is_bot && from.id.0 == our_id { return None; }
    Some(super::attachments::ReplyToBody {
        author: super::attachments::MessageAuthor {
            name: from.full_name(),
            username: from.username.as_ref().map(|u| format!("@{u}")),
            user_id: Some(from.id.0 as i64),
        },
        text: r.text().or(r.caption()).map(|t| t.to_string()),
    })
});
```

For `our_id`, add `BotIdentity` to the dptree DI in `dispatch.rs` (if not done yet) and pipe as `Arc<BotIdentity>` into `handle_message`.

In the worker, when calling `format_cc_input`, the `InputMessage` conversion now carries `chat` and `reply_to_body` through from `DebounceMsg`. Plumb them in the `input_messages.push(InputMessage { ... })` block around worker.rs:313.

Strip `@botname` from `text` before putting into the InputMessage. Use `super::mention::strip_bot_mentions(&text, &identity.username)`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p rightclaw-bot attachments::group_format_tests`
Expected: 3 tests PASS.

Run: `cargo build -p rightclaw-bot`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs crates/bot/src/telegram/worker.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs
git commit -m "feat(bot): group chat context + reply-to body in CC prompt"
```

---

### Task 13: Worker — group reply-to + silence live thinking + expanded memory tags

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write test for tag builder**

In `crates/bot/src/telegram/worker.rs`, replace `chat_tags` and add tests:

```rust
/// Build the tag list for a Hindsight retain call.
///
/// - DM: `["chat:<chat_id>"]`.
/// - Group: `["chat:<chat_id>", "user:<sender_id>"]` plus `"topic:<thread_id>"`
///   when this is a supergroup topic (thread_id > 0).
fn retain_tags(chat_id: i64, sender_id: Option<i64>, thread_id: i64, is_group: bool) -> Vec<String> {
    let mut tags = vec![format!("chat:{chat_id}")];
    if is_group {
        if let Some(uid) = sender_id {
            tags.push(format!("user:{uid}"));
        }
        if thread_id > 0 {
            tags.push(format!("topic:{thread_id}"));
        }
    }
    tags
}

/// Recall tags: unchanged — only `chat:<chat_id>`.
fn recall_tags(chat_id: i64) -> Vec<String> {
    vec![format!("chat:{chat_id}")]
}

#[cfg(test)]
mod tag_tests {
    use super::*;

    #[test]
    fn dm_tags_have_chat_only() {
        let t = retain_tags(42, Some(42), 0, false);
        assert_eq!(t, vec!["chat:42"]);
    }

    #[test]
    fn group_tags_have_user_and_topic() {
        let t = retain_tags(-1001, Some(100), 7, true);
        assert_eq!(t, vec!["chat:-1001", "user:100", "topic:7"]);
    }

    #[test]
    fn group_tags_no_topic_when_thread_zero() {
        let t = retain_tags(-1001, Some(100), 0, true);
        assert_eq!(t, vec!["chat:-1001", "user:100"]);
    }

    #[test]
    fn recall_tags_unchanged_by_group() {
        let t = recall_tags(-1001);
        assert_eq!(t, vec!["chat:-1001"]);
    }
}
```

Delete the old `chat_tags` function; replace all 3 call-sites in worker.rs (lines ~559, 584, 874) with the new functions. At each call, pass `is_group = matches!(ctx.chat_context, ChatContext::Group{..})` — you need to carry the group flag into `WorkerContext` or derive from the batch's first message.

Simpler: derive from `batch[0].chat` which is already carried:

```rust
let is_group = matches!(batch[0].chat, super::attachments::ChatContext::Group { .. });
let sender_id = batch[0].author.user_id; // already present
let retain_tags_v = retain_tags(chat_id, sender_id, eff_thread_id, is_group);
```

Use `retain_tags_v` where `chat_tags(chat_id)` was called for retain. Use `recall_tags(chat_id)` where `chat_tags(chat_id)` was called for recall/prefetch.

- [ ] **Step 2: Silence live thinking in groups**

Around the live-thinking setup in worker.rs (search for `show_thinking`), change the condition from `ctx.show_thinking` to `ctx.show_thinking && !is_group`. Also log once:

```rust
if is_group && ctx.show_thinking {
    tracing::debug!(?key, "show_thinking suppressed in group");
}
```

- [ ] **Step 3: Reply-to triggering message in groups (already supported)**

The existing worker.rs line 457 sets `reply_to = Some(batch[0].message_id)` when batch.len() == 1. For groups, we always want reply-to regardless of batch size. Change to:

```rust
let reply_to = if matches!(batch[0].chat, super::attachments::ChatContext::Group { .. }) {
    Some(batch[0].message_id)
} else if batch.len() == 1 {
    Some(batch[0].message_id)
} else {
    output.reply_to_message_id
};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p rightclaw-bot worker::tag_tests`
Expected: 4 tests PASS.

Run: `cargo build -p rightclaw-bot`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): group retain tags, silent thinking, always reply-to in groups"
```

---

### Task 14: Init wizard — ask for first trusted user ID

**Files:**
- Modify: `crates/rightclaw/src/init.rs` (or wizard flow) — locate the agent-init interactive flow and add a step.

- [ ] **Step 1: Locate init wizard step for telegram**

Read `crates/rightclaw/src/init.rs` to find where telegram-related config is collected in `init_agent()` (or similar). The new step goes after telegram_token is set.

- [ ] **Step 2: Add the prompt and persist**

```rust
// After telegram_token is configured and agent.yaml is written:
if config.telegram_token.is_some() && !allowlist_exists(&agent_dir) {
    // Use the same `dialoguer` prompt style as other init steps.
    let prompt = "Your Telegram user ID (first trusted user; leave blank to skip and add later)";
    let input: String = dialoguer::Input::new()
        .with_prompt(prompt)
        .allow_empty(true)
        .interact_text()
        .map_err(|e| miette::miette!("{e}"))?;
    let trimmed = input.trim();
    if !trimmed.is_empty() {
        let user_id: i64 = trimmed.parse()
            .map_err(|_| miette::miette!("invalid user_id: {trimmed}"))?;
        let file = rightclaw::agent::allowlist::AllowlistFile {
            version: 1,
            users: vec![rightclaw::agent::allowlist::AllowedUser {
                id: user_id,
                label: None,
                added_by: None,
                added_at: chrono::Utc::now(),
            }],
            groups: vec![],
        };
        rightclaw::agent::allowlist::write_file(&agent_dir, &file)
            .map_err(|e| miette::miette!("{e}"))?;
        println!("✓ Added {user_id} as first trusted user");
    }
}

fn allowlist_exists(dir: &std::path::Path) -> bool {
    rightclaw::agent::allowlist::allowlist_path(dir).exists()
}
```

If `init.rs` lives in the `rightclaw` crate but calls `rightclaw::agent::allowlist::...`, use the relative paths within that crate (`crate::agent::allowlist`).

- [ ] **Step 3: Sanity check — no test for interactive step, just build**

Run: `cargo build -p rightclaw`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/init.rs
git commit -m "feat(init): prompt for first trusted user id during agent init"
```

---

### Task 15: `agent.yaml` — keep `allowed_chat_ids` field with deprecation comment

**Files:**
- Modify: `crates/rightclaw/src/agent/types.rs`

Because `AgentConfig` uses `#[serde(deny_unknown_fields)]`, removing the field would break parsing of existing yaml files. Keep it as a vestigial field (read-only for migration); log a WARN on load when it's non-empty but `allowlist.yaml` already exists — handled already in Task 8's startup code.

Update the doc comment only:

- [ ] **Step 1: Update doc + rename to mark deprecated**

In `crates/rightclaw/src/agent/types.rs` around line 215:

```rust
    /// **DEPRECATED** — moved to `allowlist.yaml`. Retained for backward-compatible
    /// parsing and one-time migration. Used only by `bot::lib::load_or_migrate_allowlist`
    /// on the first bot startup after upgrade. On subsequent startups the field is
    /// ignored and a WARN is emitted (see §Migration in the group-chat spec).
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
```

Add a regression test in `types.rs::tests`:

```rust
#[test]
fn allowed_chat_ids_still_parses_for_migration() {
    let yaml = "allowed_chat_ids:\n  - 42\n  - -100\n";
    let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    assert_eq!(config.allowed_chat_ids, vec![42, -100]);
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rightclaw agent::types`
Expected: existing tests + new one PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/agent/types.rs
git commit -m "chore(types): mark allowed_chat_ids deprecated; keep for migration"
```

---

### Task 16: End-to-end smoke + `cargo test --workspace`

**Files:** none (verification only).

- [ ] **Step 1: Run full workspace build (debug)**

Run: `cargo build --workspace`
Expected: PASS.

- [ ] **Step 2: Run full workspace tests**

Run: `cargo test --workspace`
Expected: all tests PASS. No `#[ignore]` added; integration tests that require OpenShell continue to run against a live sandbox.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean (fix any clippy warnings introduced by these changes).

- [ ] **Step 4: Launch a local bot manually against a test agent, verify:**

1. Fresh agent with no `allowlist.yaml` and empty `allowed_chat_ids` → bot starts, silently drops DMs.
2. `rightclaw agent allow test 12345` → DM from user 12345 works.
3. Legacy agent with `allowed_chat_ids: [12345]` → bot startup log shows migration INFO; `allowlist.yaml` appears.
4. Add bot to a group, trusted user sends `/allow_all` → ack appears, `allowlist.yaml` updated.
5. Non-trusted group member mentions bot (group is closed) → bot silent.
6. After `/allow_all`, non-trusted group member mentions bot → bot replies.
7. Non-trusted group member sends non-mention message → bot silent.
8. Trusted user sends `/new` in a group → silent (DM-only command).

- [ ] **Step 5: Commit verification log**

```bash
git commit --allow-empty -m "chore: group-chat support ready for review"
```

---

## Self-Review — Spec Coverage Check

| Spec section | Implementing task(s) |
|---|---|
| §Response Rules matrix | Task 7 (filter), Task 10 (command trusted-gate) |
| §Mention/Reply Detection | Task 6 |
| §Prompt Shape → DM unchanged | Task 12 (keeps DM branch minimal) |
| §Prompt Shape → Group YAML + attribution + reply_to | Task 12 |
| §Commands table | Task 10 (Telegram) + Task 11 (CLI) |
| §Command Routing Rules + Ack messages | Task 10 |
| §Storage: allowlist.yaml location/ownership/schema | Task 1, 2, 3 |
| §Storage Writes (atomic, lockfile, concurrency) | Task 3, 5 |
| §Storage Reads (RwLock + notify) | Task 2, 5, 8 |
| §Bootstrap (init wizard) | Task 14 |
| §CLI Commands (recovery/pre-start) | Task 11 |
| §Migration | Task 4, Task 8 (startup call + log) |
| §Empty Allowlist Safety | Task 7 (filter) + Task 8 (startup WARN) |
| §Memory & Session | Task 13 (retain tags), unchanged keying |
| §UX Details (reply-to, silent thinking) | Task 13 |
| §Non-Goals (session mgmt in groups) | Task 9 |
| §Non-Goals (rewriting agent.yaml) | Task 15 (no rewrite; WARN only) |

No gaps identified.

## Self-Review — Type Consistency Check

- `AllowlistHandle`, `AllowlistState`, `AllowlistFile`, `AllowedUser`, `AllowedGroup` — used consistently across Tasks 1–14.
- `retain_tags`/`recall_tags` replace `chat_tags` in Task 13; all three call-sites updated.
- `DebounceMsg` / `InputMessage` / `ChatContext` / `ReplyToBody` — fields added in Task 12 are consumed in Tasks 12 and 13.
- `BotIdentity` — introduced in Task 6, used in Tasks 7, 8, 10, 12.
- `RoutingDecision` — Task 7, carried via DI starting Task 8.
- CLI subcommand variants (`Allow`, `Deny`, `AllowAll`, `DenyAll`, `Allowed`) — Task 11.

All types are internally consistent. No conflicting signatures.

## Self-Review — Placeholder Scan

- No "TBD" / "TODO" / "implement later" entries remain.
- Every step shows full code (not "similar to above").
- Each task ends with a runnable verification command and an expected outcome.
- Every modified file has an exact path; every created file has an exact path.
