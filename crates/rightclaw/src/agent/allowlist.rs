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

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AllowedUser {
    pub id: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub added_by: Option<i64>,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AllowedGroup {
    pub id: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub opened_by: Option<i64>,
    pub opened_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
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
            let escaped = escape_double_quoted(s);
            writeln!(out, "{spaces}{key}: \"{escaped}\"").unwrap();
        }
        None => writeln!(out, "{spaces}{key}: null").unwrap(),
    }
}

/// Escape a string for embedding inside a YAML 1.2 double-quoted scalar.
///
/// Escapes: `\`, `"`, common control whitespace (`\n`, `\r`, `\t`), NUL, and any
/// other byte in the C0 control range (< 0x20) or DEL (0x7F). Other Unicode
/// code points pass through untouched — double-quoted YAML scalars are UTF-8
/// and emoji/CJK/RTL marks are valid as-is.
fn escape_double_quoted(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7F => {
                write!(out, "\\x{:02X}", c as u32).unwrap();
            }
            c => out.push(c),
        }
    }
    out
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

    #[test]
    fn allowed_user_rejects_unknown_fields() {
        let yaml = r#"
id: 42
opned_by: 7
added_at: 2026-04-16T12:00:00Z
"#;
        let result: Result<AllowedUser, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown field"),
            "expected 'unknown field' in error: {err}"
        );
    }

    #[test]
    fn allowed_group_rejects_unknown_fields() {
        let yaml = r#"
id: -1001
opend_by: 7
opened_at: 2026-04-16T12:00:00Z
"#;
        let result: Result<AllowedGroup, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown field"),
            "expected 'unknown field' in error: {err}"
        );
    }

    #[test]
    fn allowlist_file_rejects_unknown_fields() {
        let yaml = r#"
version: 1
usrs: []
groups: []
"#;
        let result: Result<AllowlistFile, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown field"),
            "expected 'unknown field' in error: {err}"
        );
    }

    #[test]
    fn serialize_escapes_control_chars_in_label() {
        // Literal backslash, double quote, newline, carriage return, tab,
        // NUL, \x05 (arbitrary control char), plus non-ASCII (emoji + Cyrillic).
        let tricky = "a\\b\"c\nd\re\tf\0g\x05h \u{1F600} \u{0410}\u{0411}";
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 1,
                label: Some(tricky.to_string()),
                added_by: None,
                added_at: "2026-04-16T12:00:00Z".parse().unwrap(),
            }],
            groups: vec![],
        };
        let yaml = serialize_yaml(&file);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed.users[0].label.as_deref(), Some(tricky));
    }

    #[test]
    fn serialize_null_label_roundtrips_as_none() {
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 1,
                label: None,
                added_by: None,
                added_at: "2026-04-16T12:00:00Z".parse().unwrap(),
            }],
            groups: vec![AllowedGroup {
                id: -1001,
                label: None,
                opened_by: None,
                opened_at: "2026-04-16T12:30:00Z".parse().unwrap(),
            }],
        };
        let yaml = serialize_yaml(&file);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed.users[0].label, None);
        assert_eq!(parsed.groups[0].label, None);
    }
}

/// Outcome of `add_user` / `add_group`.
#[derive(Debug, Clone, PartialEq)]
pub enum AddOutcome {
    Inserted,
    AlreadyPresent,
}

/// Outcome of `remove_user` / `remove_group`.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoveOutcome {
    Removed,
    NotFound,
}

#[derive(Debug, Default, Clone)]
pub struct AllowlistState {
    inner: AllowlistFile,
}

impl AllowlistState {
    pub fn from_file(file: AllowlistFile) -> Self {
        Self { inner: file }
    }

    pub fn to_file(&self) -> AllowlistFile {
        self.inner.clone()
    }

    /// Is this user globally trusted?
    pub fn is_user_trusted(&self, user_id: i64) -> bool {
        self.inner.users.iter().any(|u| u.id == user_id)
    }

    /// Is this group opened (members may talk to bot with mention/reply)?
    pub fn is_group_open(&self, chat_id: i64) -> bool {
        self.inner.groups.iter().any(|g| g.id == chat_id)
    }

    pub fn users(&self) -> &[AllowedUser] {
        &self.inner.users
    }

    pub fn groups(&self) -> &[AllowedGroup] {
        &self.inner.groups
    }

    pub fn add_user(&mut self, user: AllowedUser) -> AddOutcome {
        if self.is_user_trusted(user.id) {
            return AddOutcome::AlreadyPresent;
        }
        self.inner.users.push(user);
        AddOutcome::Inserted
    }

    pub fn remove_user(&mut self, user_id: i64) -> RemoveOutcome {
        let before = self.inner.users.len();
        self.inner.users.retain(|u| u.id != user_id);
        if self.inner.users.len() == before {
            RemoveOutcome::NotFound
        } else {
            RemoveOutcome::Removed
        }
    }

    pub fn add_group(&mut self, group: AllowedGroup) -> AddOutcome {
        if self.is_group_open(group.id) {
            return AddOutcome::AlreadyPresent;
        }
        self.inner.groups.push(group);
        AddOutcome::Inserted
    }

    pub fn remove_group(&mut self, chat_id: i64) -> RemoveOutcome {
        let before = self.inner.groups.len();
        self.inner.groups.retain(|g| g.id != chat_id);
        if self.inner.groups.len() == before {
            RemoveOutcome::NotFound
        } else {
            RemoveOutcome::Removed
        }
    }
}

/// Shareable handle used by bot and CLI. Writers take `.write()`, readers take `.read()`.
#[derive(Debug, Clone, Default)]
pub struct AllowlistHandle(pub Arc<RwLock<AllowlistState>>);

impl AllowlistHandle {
    pub fn new(state: AllowlistState) -> Self {
        Self(Arc::new(RwLock::new(state)))
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    fn t() -> DateTime<Utc> {
        "2026-04-16T12:00:00Z".parse().unwrap()
    }

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
        })
        .await
        .unwrap();
        let r = h.0.read().await;
        assert!(r.is_user_trusted(7));
    }
}

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
    lock_file
        .lock_exclusive()
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

    fn t() -> DateTime<Utc> {
        "2026-04-16T12:00:00Z".parse().unwrap()
    }

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
            users: vec![AllowedUser {
                id: 1,
                label: Some("u".into()),
                added_by: None,
                added_at: t(),
            }],
            groups: vec![AllowedGroup {
                id: -1,
                label: None,
                opened_by: Some(1),
                opened_at: t(),
            }],
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
