//! Bot-managed allowlist — trusted users (DM + anywhere) + opened groups (per-chat-id open gate).
//!
//! On-disk format (version 2):
//!
//! ```yaml
//! version: 2
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
//!     mode: all
//!     topics:
//!       - thread_id: 8
//!         mode: addressed
//! ```

use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AllowedUser {
    pub id: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub added_by: Option<i64>,
    pub added_at: DateTime<Utc>,
}

/// Per-scope response mode. Default `Addressed` preserves the historical
/// "must address the bot" behaviour.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ResponseMode {
    /// Respond only when addressed (mention / reply-to-bot / command).
    #[default]
    Addressed,
    /// Respond to every message in the scope, from any participant.
    All,
}

/// What a `groups` entry actually is. Default `Group` keeps every existing
/// allowlist.yaml valid.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum GroupKind {
    #[default]
    Group,
    Channel,
}

/// A per-topic override of the group's default mode. `thread_id` is the
/// normalised effective thread id (General = 0).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TopicMode {
    pub thread_id: i64,
    pub mode: ResponseMode,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AllowedGroup {
    pub id: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub opened_by: Option<i64>,
    pub opened_at: DateTime<Utc>,
    #[serde(default)]
    pub mode: ResponseMode,
    #[serde(default)]
    pub topics: Vec<TopicMode>,
    #[serde(default)]
    pub kind: GroupKind,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AllowlistFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub users: Vec<AllowedUser>,
    #[serde(default)]
    pub groups: Vec<AllowedGroup>,
}

fn default_version() -> u32 {
    1
}

pub const CURRENT_VERSION: u32 = 2;

impl Default for AllowlistFile {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            users: Vec::new(),
            groups: Vec::new(),
        }
    }
}

/// Parse YAML text into `AllowlistFile`. Rejects unknown future `version`.
pub fn parse_yaml(text: &str) -> Result<AllowlistFile, String> {
    let mut parsed: AllowlistFile =
        serde_saphyr::from_str(text).map_err(|e| format!("allowlist.yaml parse error: {e:#}"))?;
    if parsed.version != 1 && parsed.version != CURRENT_VERSION {
        return Err(format!(
            "allowlist.yaml version {} is not supported (expected 1 or {})",
            parsed.version, CURRENT_VERSION
        ));
    }
    parsed.version = CURRENT_VERSION;
    Ok(parsed)
}

/// Serialize `AllowlistFile` to YAML text (deterministic key order).
pub fn serialize_yaml(file: &AllowlistFile) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(512);
    out.push_str("# Bot-managed. Edit via /allow, /deny, /allow_all, /deny_all,\n");
    out.push_str("# or `right agent allow|deny|allow_all|deny_all`.\n");
    writeln!(out, "version: {}", file.version).unwrap();
    if file.users.is_empty() {
        out.push_str("users: []\n");
    } else {
        out.push_str("users:\n");
        for u in &file.users {
            writeln!(out, "  - id: {}", u.id).unwrap();
            write_label(&mut out, "label", u.label.as_deref(), 4);
            write_opt_i64(&mut out, "added_by", u.added_by, 4);
            writeln!(
                out,
                "    added_at: {}",
                u.added_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            )
            .unwrap();
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
            writeln!(
                out,
                "    opened_at: {}",
                g.opened_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            )
            .unwrap();
            if g.mode != ResponseMode::default() {
                writeln!(out, "    mode: {}", response_mode_str(g.mode)).unwrap();
            }
            if g.kind == GroupKind::Channel {
                writeln!(out, "    kind: channel").unwrap();
            }
            if !g.topics.is_empty() {
                out.push_str("    topics:\n");
                for t in &g.topics {
                    writeln!(out, "      - thread_id: {}", t.thread_id).unwrap();
                    writeln!(out, "        mode: {}", response_mode_str(t.mode)).unwrap();
                }
            }
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

fn response_mode_str(m: ResponseMode) -> &'static str {
    match m {
        ResponseMode::Addressed => "addressed",
        ResponseMode::All => "all",
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

#[derive(Debug, Default, Clone, PartialEq)]
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

    /// Opened channels (kind == Channel).
    pub fn opened_channels(&self) -> Vec<&AllowedGroup> {
        self.inner
            .groups
            .iter()
            .filter(|g| g.kind == GroupKind::Channel)
            .collect()
    }

    /// Is this chat id an opened channel?
    pub fn is_channel_open(&self, chat_id: i64) -> bool {
        self.inner
            .groups
            .iter()
            .any(|g| g.id == chat_id && g.kind == GroupKind::Channel)
    }

    /// True iff the given chat id is either a trusted user or an opened group.
    /// Used by cron target validation and by anything else that needs a
    /// uniform "is this a valid Telegram destination for this agent?" check.
    pub fn is_chat_allowed(&self, chat_id: i64) -> bool {
        self.is_user_trusted(chat_id) || self.is_group_open(chat_id)
    }

    /// Effective response mode for a scope. Precedence: explicit topic entry →
    /// group default → built-in `Addressed`. Unknown (closed) group → `Addressed`.
    pub fn response_mode(&self, chat_id: i64, thread_id: i64) -> ResponseMode {
        let Some(g) = self.inner.groups.iter().find(|g| g.id == chat_id) else {
            return ResponseMode::Addressed;
        };
        if let Some(t) = g.topics.iter().find(|t| t.thread_id == thread_id) {
            return t.mode;
        }
        g.mode
    }

    /// Set the group-level default mode. Returns false if the group is not open.
    pub fn set_group_mode(&mut self, chat_id: i64, mode: ResponseMode) -> bool {
        match self.inner.groups.iter_mut().find(|g| g.id == chat_id) {
            Some(g) => {
                g.mode = mode;
                true
            }
            None => false,
        }
    }

    /// Set (or overwrite) a per-topic mode override. Returns false if the group
    /// is not open.
    pub fn set_topic_mode(&mut self, chat_id: i64, thread_id: i64, mode: ResponseMode) -> bool {
        let Some(g) = self.inner.groups.iter_mut().find(|g| g.id == chat_id) else {
            return false;
        };
        match g.topics.iter_mut().find(|t| t.thread_id == thread_id) {
            Some(t) => t.mode = mode,
            None => g.topics.push(TopicMode { thread_id, mode }),
        }
        true
    }

    /// Remove a per-topic override. Returns true iff an override was removed.
    pub fn clear_topic_mode(&mut self, chat_id: i64, thread_id: i64) -> bool {
        let Some(g) = self.inner.groups.iter_mut().find(|g| g.id == chat_id) else {
            return false;
        };
        let before = g.topics.len();
        g.topics.retain(|t| t.thread_id != thread_id);
        g.topics.len() != before
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
/// The lockfile is created if missing. Using `fs4::FileExt::lock`
/// (advisory flock on Unix, LockFileEx on Windows).
pub fn with_lock<R>(
    agent_dir: &Path,
    f: impl FnOnce(&Path) -> Result<R, String>,
) -> Result<R, String> {
    use fs4::FileExt;
    let lock_p = lock_path(agent_dir);
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_p)
        .map_err(|e| format!("open lockfile {}: {e:#}", lock_p.display()))?;
    lock_file
        .lock()
        .map_err(|e| format!("lock {}: {e:#}", lock_p.display()))?;
    let result = f(agent_dir);
    if let Err(e) = FileExt::unlock(&lock_file) {
        tracing::warn!("allowlist lockfile unlock failed: {e:#}");
    }
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
        let mut f =
            std::fs::File::create(&tmp).map_err(|e| format!("create {}: {e:#}", tmp.display()))?;
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
        return Ok(MigrationReport {
            already_present: true,
            ..Default::default()
        });
    }
    let mut file = AllowlistFile::default();
    let mut mu = 0usize;
    let mut mg = 0usize;
    for &id in legacy_chat_ids {
        if id > 0 {
            file.users.push(AllowedUser {
                id,
                label: None,
                added_by: None,
                added_at: now,
            });
            mu += 1;
        } else if id < 0 {
            file.groups.push(AllowedGroup {
                id,
                label: None,
                opened_by: None,
                opened_at: now,
                mode: ResponseMode::Addressed,
                topics: Vec::new(),
                kind: GroupKind::Group,
            });
            mg += 1;
        }
        // id == 0 skipped (Telegram never uses 0).
    }
    write_file(agent_dir, &file)?;
    Ok(MigrationReport {
        migrated_users: mu,
        migrated_groups: mg,
        already_present: false,
    })
}

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
    use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};

    let watch_path = agent_dir.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_millis(200), move |res| {
        if let Err(e) = tx.send(res) {
            tracing::warn!("allowlist watcher channel send failed: {e:#}");
        }
    })
    .map_err(|e| format!("create debouncer: {e:#}"))?;

    debouncer
        .watcher()
        .watch(&watch_path, RecursiveMode::NonRecursive)
        .map_err(|e| format!("watch {}: {e:#}", watch_path.display()))?;

    let handle_clone = handle.clone();
    let dir_clone = agent_dir.to_path_buf();
    std::thread::spawn(move || {
        // macOS FSEvents reports canonical paths (e.g. /private/var/... for /var/...),
        // so we compare by filename rather than full path. Safe because the watcher
        // is scoped to `agent_dir` with `NonRecursive`.
        let target_filename = std::ffi::OsStr::new(ALLOWLIST_FILENAME);
        for events in rx {
            let Ok(evts) = events else {
                continue;
            };
            let touches_allowlist = evts.iter().any(|e| {
                matches!(e.kind, DebouncedEventKind::Any)
                    && e.path.file_name() == Some(target_filename)
            });
            if !touches_allowlist {
                continue;
            }

            match read_file(&dir_clone) {
                Ok(Some(file)) => {
                    let new_state = AllowlistState::from_file(file);
                    let mut w = handle_clone.0.write().expect("allowlist lock poisoned");
                    if *w == new_state {
                        continue;
                    }
                    *w = new_state;
                    drop(w);
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
#[path = "allowlist_tests.rs"]
mod tests;
