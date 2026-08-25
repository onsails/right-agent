//! Offline legacy multiprocess-WAL repair (`right agent db-repair`).
//!
//! Recovers one agent's `data.db` from legacy experimental-multiprocess-WAL
//! state into a clean standalone snapshot. This module owns the COMPLETE
//! filesystem transaction; the CLI supplies validated paths and never
//! manipulates `data.db*` itself.
//!
//! Transaction shape (all under the bootstrap lock, all after the caller has
//! proven runtime quiescence):
//!
//! 1. Inventory `data.db` + every `data.db-*` artifact in the agent dir.
//! 2. Copy the full set byte-for-byte into
//!    `backups/<agent>/wal-recovery-<timestamp>/forensic/`, verifying hashes.
//! 3. Copy the set again into `staging/` and remove ONLY the staged
//!    `data.db-tshm`/`data.db-shm` coordination sidecars so the recovery open
//!    cold-rebuilds them (tursodatabase/turso#769).
//! 4. Open the staged copy with Turso standard-local mode so a valid WAL prefix
//!    replays, then `VACUUM INTO` a clean standalone
//!    replacement built next to the live database (same filesystem, so the
//!    final rename is atomic).
//! 5. Validate the replacement read-only: `PRAGMA quick_check`,
//!    `PRAGMA user_version`, and the fixed non-secret invariant report (table
//!    existence + row counts — never secret-bearing columns).
//! 6. Swap: move the original set into `live-pre-swap/`, then rename the
//!    replacement to `data.db`. Any failure before the first live move leaves
//!    the live set untouched; any failure after it restores the complete
//!    original set before the lock is released.
//! 7. Write `manifest.json`: schema version, file names/sizes/SHA-256, the
//!    non-secret counts, tool version, timestamp, swap status. Never row
//!    values, credentials, prompts, message text, or tokens.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{Connection, DbError, bootstrap_lock};

/// Name of the live database file inside an agent directory.
const DB_FILE_NAME: &str = "data.db";
/// Tables whose existence + row counts form the fixed non-secret invariant
/// report. Counts only — the query never touches a value column.
const INVARIANT_TABLES: [&str; 5] = [
    "auth_tokens",
    "cron_specs",
    "async_runs",
    "usage_events",
    "conversation_messages",
];
/// Staged coordination sidecars removed before the recovery open so Turso
/// cold-rebuilds them.
const STAGED_COORDINATION_SUFFIXES: [&str; 2] = ["-tshm", "-shm"];
/// Forensic copy buffer size.
const COPY_BUFFER_SIZE: usize = 64 * 1024;
/// quick_check failure rows folded into the error (they are structural
/// corruption descriptions, never row values).
const MAX_QUICK_CHECK_MESSAGES: usize = 5;
const OWNED_FTS_SHADOW_INDEXES: [&str; 2] = [
    "__turso_internal_fts_dir_idx_memories_turso_fts_key",
    "__turso_internal_fts_dir_idx_conversation_messages_turso_fts_key",
];

/// Validated inputs for one offline repair. Constructed by the CLI from the
/// agent name + home; contains no operator-supplied database path or SQL.
pub struct RepairRequest {
    /// Agent directory containing the live `data.db` (`<home>/agents/<name>`).
    pub agent_dir: PathBuf,
    /// Recovery artifacts root (`<home>/backups/<name>`); the timestamped
    /// `wal-recovery-<timestamp>/` directory is created beneath it.
    pub backups_dir: PathBuf,
    /// Label for the recovery directory. Callers pass a wall-clock stamp
    /// (`%Y%m%d-%H%M%S`); validated to be path-safe (ASCII alnum, `-`, `_`).
    pub timestamp: String,
}

/// Non-secret invariant for one table: existence and row count only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableInvariant {
    pub table: String,
    pub exists: bool,
    pub rows: Option<i64>,
}

/// File identity recorded in the report/manifest: name, size, SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileDigest {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// What a successful repair leaves behind. Carries paths, hashes, schema
/// version, and counts only — never row content.
#[derive(Debug)]
pub struct RepairReport {
    /// The new live database path (`<agent_dir>/data.db`).
    pub db_path: PathBuf,
    /// `backups/<agent>/wal-recovery-<timestamp>/`.
    pub recovery_dir: PathBuf,
    /// Byte-for-byte forensic copies of the original set.
    pub forensic_dir: PathBuf,
    /// The original live set after the swap (the rollback source).
    pub live_pre_swap_dir: PathBuf,
    /// `<recovery_dir>/manifest.json`.
    pub manifest_path: PathBuf,
    /// `PRAGMA user_version` of the replacement.
    pub schema_version: u32,
    /// `PRAGMA quick_check` result; `"ok"` (repair fails otherwise).
    pub quick_check: String,
    /// Fixed non-secret invariant report.
    pub tables: Vec<TableInvariant>,
    /// Digests of the preserved original artifacts (forensic == pre-swap).
    pub preserved_files: Vec<FileDigest>,
    /// Digest of the recovered standalone snapshot now live as `data.db`.
    pub recovered_db: FileDigest,
}

#[derive(Serialize)]
struct RecoveryManifest {
    tool: &'static str,
    tool_version: &'static str,
    operation: &'static str,
    timestamp: String,
    agent_dir: String,
    recovery_dir: String,
    schema_version: Option<u32>,
    quick_check: Option<String>,
    tables: Vec<TableInvariant>,
    preserved_files: Vec<FileDigest>,
    recovered_db: Option<FileDigest>,
    swap_status: &'static str,
    error: Option<String>,
}

/// Repair one agent database per the module-level transaction shape.
///
/// Acquires the bootstrap migration lock for `agent_dir` and holds it through
/// forensic copy, staging, validation, swap, and any rollback. The caller
/// (CLI) must have already proven runtime quiescence; this function does not
/// stop processes.
pub async fn repair_legacy_wal(request: RepairRequest) -> Result<RepairReport, DbError> {
    validate_request(&request)?;
    let _guard = bootstrap_lock::acquire(&request.agent_dir).await?;

    let paths = RecoveryPaths::new(&request);
    // A prior repair attempt wrote its manifest; refuse to mix attempts under
    // one label (the CLI stamps `%Y%m%d-%H%M%S`, so retry = fresh label).
    if paths.manifest_path.exists() {
        return Err(repair_err(
            &request.agent_dir,
            format!(
                "recovery label {} already used by a prior repair attempt: {}",
                request.timestamp,
                paths.manifest_path.display()
            ),
        ));
    }
    // Creating the recovery directories is the first mutating step. It cannot
    // touch the live set, so a failure here needs no rollback.
    for dir in [
        &paths.recovery_dir,
        &paths.forensic_dir,
        &paths.staging_dir,
        &paths.live_pre_swap_dir,
    ] {
        std::fs::create_dir_all(dir).map_err(|source| {
            repair_err(
                &request.agent_dir,
                format!("create recovery directory {}: {source}", dir.display()),
            )
        })?;
    }
    sync_directory(&paths.recovery_dir, &request.agent_dir)?;
    sync_directory(&request.backups_dir, &request.agent_dir)?;

    match repair_under_lock(&request, &paths).await {
        Ok(report) => match write_manifest(&paths, &manifest(&report, "swapped", None), &request) {
            Ok(()) => Ok(report),
            Err(manifest_error) => {
                let rollback = rollback_completed_swap(&report);
                Err(repair_err(
                    &request.agent_dir,
                    format!(
                        "write successful recovery manifest: {manifest_error}; rollback: {rollback}"
                    ),
                ))
            }
        },
        Err(error) => {
            // A failed repair still leaves a manifest (swap status + error)
            // beside the forensic copy so the operator can audit the attempt.
            let mut error = error;
            if let Err(manifest_error) = write_manifest(
                &paths,
                &failure_manifest(&request, &paths, &error),
                &request,
            ) {
                error = repair_err(
                    &request.agent_dir,
                    format!(
                        "{error}; additionally the failure manifest could not be written: {manifest_error}"
                    ),
                );
            }
            Err(error)
        }
    }
}

struct RecoveryPaths {
    recovery_dir: PathBuf,
    forensic_dir: PathBuf,
    staging_dir: PathBuf,
    live_pre_swap_dir: PathBuf,
    manifest_path: PathBuf,
}

impl RecoveryPaths {
    fn new(request: &RepairRequest) -> Self {
        let recovery_dir = request
            .backups_dir
            .join(format!("wal-recovery-{}", request.timestamp));
        Self {
            forensic_dir: recovery_dir.join("forensic"),
            staging_dir: recovery_dir.join("staging"),
            live_pre_swap_dir: recovery_dir.join("live-pre-swap"),
            manifest_path: recovery_dir.join("manifest.json"),
            recovery_dir,
        }
    }
}

fn validate_request(request: &RepairRequest) -> Result<(), DbError> {
    let valid_timestamp = !request.timestamp.is_empty()
        && request
            .timestamp
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid_timestamp {
        return Err(DbError::InvalidParameter(format!(
            "repair timestamp label is not path-safe: {:?}",
            request.timestamp
        )));
    }
    if !request.agent_dir.is_dir() {
        return Err(repair_err(
            &request.agent_dir,
            "agent directory does not exist".to_string(),
        ));
    }
    let db_path = request.agent_dir.join(DB_FILE_NAME);
    if !db_path.is_file() {
        return Err(repair_err(
            &request.agent_dir,
            format!("data.db does not exist at {}", db_path.display()),
        ));
    }
    Ok(())
}

async fn repair_under_lock(
    request: &RepairRequest,
    paths: &RecoveryPaths,
) -> Result<RepairReport, DbError> {
    let agent_dir = &request.agent_dir;
    let live_set = inventory_live_set(agent_dir)?;

    let preserved_files = forensic_copy(&live_set, &paths.forensic_dir, agent_dir)?;
    stage_working_copy(&live_set, &preserved_files, &paths.staging_dir, agent_dir)?;
    let replacement_path =
        agent_dir.join(format!("{DB_FILE_NAME}.recovering-{}", request.timestamp));
    let validation = match build_and_validate_replacement(
        &paths.staging_dir,
        &replacement_path,
        agent_dir,
    )
    .await
    {
        Ok(validation) => validation,
        Err(error) => {
            // Pre-swap: the live set is untouched; only the replacement
            // temp file (and any sidecars a validation open created) may
            // exist beside it.
            return Err(remove_replacement_artifacts(&replacement_path, error));
        }
    };

    swap_live_set(
        &live_set,
        &paths.live_pre_swap_dir,
        &replacement_path,
        agent_dir,
    )?;

    let recovered_db = digest_file(&agent_dir.join(DB_FILE_NAME), agent_dir)?;
    Ok(RepairReport {
        db_path: agent_dir.join(DB_FILE_NAME),
        recovery_dir: paths.recovery_dir.clone(),
        forensic_dir: paths.forensic_dir.clone(),
        live_pre_swap_dir: paths.live_pre_swap_dir.clone(),
        manifest_path: paths.manifest_path.clone(),
        schema_version: validation.schema_version,
        quick_check: validation.quick_check,
        tables: validation.tables,
        preserved_files,
        recovered_db,
    })
}

/// `data.db` first, then every `data.db-*` regular file, sorted. Directories
/// or other entries are not database artifacts and are never touched.
fn inventory_live_set(agent_dir: &Path) -> Result<Vec<PathBuf>, DbError> {
    let mut sidecars = Vec::new();
    let entries = std::fs::read_dir(agent_dir)
        .map_err(|source| repair_err(agent_dir, format!("list agent directory: {source}")))?;
    for entry in entries {
        let entry = entry
            .map_err(|source| repair_err(agent_dir, format!("list agent directory: {source}")))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&format!("{DB_FILE_NAME}-")) {
            continue;
        }
        let file_type = entry.file_type().map_err(|source| {
            repair_err(
                agent_dir,
                format!("stat {}: {source}", entry.path().display()),
            )
        })?;
        if file_type.is_file() {
            sidecars.push(entry.path());
        }
    }
    sidecars.sort();
    let mut live_set = vec![agent_dir.join(DB_FILE_NAME)];
    live_set.extend(sidecars);
    Ok(live_set)
}

/// Copy every live artifact into `forensic_dir` byte-for-byte, verifying each
/// copy by re-hashing the destination.
fn forensic_copy(
    live_set: &[PathBuf],
    forensic_dir: &Path,
    agent_dir: &Path,
) -> Result<Vec<FileDigest>, DbError> {
    let mut digests = Vec::with_capacity(live_set.len());
    for src in live_set {
        let dst = forensic_dir.join(file_name(src, agent_dir)?);
        let digest = copy_and_digest(src, &dst, agent_dir)?;
        let verify = digest_file(&dst, agent_dir)?;
        if verify != digest {
            return Err(repair_err(
                agent_dir,
                format!(
                    "forensic copy of {} failed verification (hash mismatch after copy)",
                    src.display()
                ),
            ));
        }
        digests.push(digest);
    }
    sync_directory(forensic_dir, agent_dir)?;
    Ok(digests)
}

/// Copy the live set into `staging_dir`, then remove ONLY the staged
/// coordination sidecars. The live `-tshm`/`-shm` are never touched here;
/// they move during the swap.
fn stage_working_copy(
    live_set: &[PathBuf],
    expected_digests: &[FileDigest],
    staging_dir: &Path,
    agent_dir: &Path,
) -> Result<(), DbError> {
    if live_set.len() != expected_digests.len() {
        return Err(repair_err(
            agent_dir,
            "staged-copy digest inventory length mismatch".into(),
        ));
    }
    for (src, expected) in live_set.iter().zip(expected_digests) {
        let live_digest = digest_file(src, agent_dir)?;
        if &live_digest != expected {
            return Err(repair_err(
                agent_dir,
                format!("live artifact changed before staging: {}", src.display()),
            ));
        }
        let dst = staging_dir.join(file_name(src, agent_dir)?);
        let copied = copy_and_digest(src, &dst, agent_dir)?;
        let staged_digest = digest_file(&dst, agent_dir)?;
        if copied != *expected || staged_digest != *expected {
            return Err(repair_err(
                agent_dir,
                format!(
                    "staged copy of {} failed digest verification",
                    src.display()
                ),
            ));
        }
    }
    sync_directory(staging_dir, agent_dir)?;
    for suffix in STAGED_COORDINATION_SUFFIXES {
        let staged = staging_dir.join(format!("{DB_FILE_NAME}{suffix}"));
        match std::fs::remove_file(&staged) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(DbError::SidecarRemove {
                    path: staged,
                    source,
                });
            }
        }
    }
    sync_directory(staging_dir, agent_dir)?;
    Ok(())
}

struct ReplacementValidation {
    schema_version: u32,
    quick_check: String,
    tables: Vec<TableInvariant>,
}

/// Open the staged copy with Turso standard-local mode (so a valid WAL prefix
/// replays), `VACUUM INTO` a clean standalone replacement next to the live
/// database, then validate the replacement read-only.
async fn build_and_validate_replacement(
    staging_dir: &Path,
    replacement_path: &Path,
    agent_dir: &Path,
) -> Result<ReplacementValidation, DbError> {
    let staged = Connection::open_local(staging_dir.join(DB_FILE_NAME), true).await?;
    staged.apply_connection_pragmas().await?;

    let destination = replacement_path.to_str().ok_or_else(|| {
        DbError::InvalidParameter(format!(
            "replacement path is not valid UTF-8: {}",
            replacement_path.display()
        ))
    })?;
    staged
        .execute_batch(&format!(
            "VACUUM INTO '{}'",
            destination.replace('\'', "''")
        ))
        .await?;
    drop(staged);
    sync_file(replacement_path, agent_dir)?;
    sync_directory(replacement_path.parent().unwrap_or(agent_dir), agent_dir)?;

    let check = Connection::open_local(replacement_path.to_path_buf(), false).await?;
    let result = validate_replacement(&check, agent_dir).await;
    drop(check);
    let validation = result?;
    remove_replacement_sidecars(replacement_path)?;
    sync_directory(replacement_path.parent().unwrap_or(agent_dir), agent_dir)?;
    Ok(validation)
}

async fn validate_replacement(
    conn: &Connection,
    agent_dir: &Path,
) -> Result<ReplacementValidation, DbError> {
    let messages: Vec<String> = conn
        .query_all("PRAGMA quick_check", (), |row| row.get(0))
        .await?;
    let failures: Vec<&String> = messages
        .iter()
        .filter(|message| message.as_str() != "ok" && !is_turso_fts_shadow_false_positive(message))
        .collect();
    let quick_check = if failures.is_empty() {
        "ok".to_string()
    } else {
        let shown: Vec<&String> = failures
            .iter()
            .take(MAX_QUICK_CHECK_MESSAGES)
            .copied()
            .collect();
        return Err(repair_err(
            agent_dir,
            format!(
                "replacement failed PRAGMA quick_check ({} message(s)): {}",
                failures.len(),
                shown
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        ));
    };

    let user_version: i64 = conn
        .query_row("PRAGMA user_version", (), |row| row.get(0))
        .await?;
    let schema_version = u32::try_from(user_version).map_err(|_| {
        repair_err(
            agent_dir,
            format!("replacement has out-of-range user_version {user_version}"),
        )
    })?;

    let mut tables = Vec::with_capacity(INVARIANT_TABLES.len());
    for table in INVARIANT_TABLES {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .await?;
        // Table names come from the fixed INVARIANT_TABLES list, never from
        // operator input; only counts are read, never value columns.
        let rows = if exists > 0 {
            Some(
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), (), |row| {
                    row.get(0)
                })
                .await?,
            )
        } else {
            None
        };
        tables.push(TableInvariant {
            table: table.to_string(),
            exists: exists > 0,
            rows,
        });
    }

    Ok(ReplacementValidation {
        schema_version,
        quick_check,
        tables,
    })
}

/// Turso 0.7.2 `quick_check` miscounts the two FTS backing-table indexes
/// derived from indexes owned by this schema. Accept only the exact message
/// shape and exact owned backing-index names; every other message is a repair
/// failure.
fn is_turso_fts_shadow_false_positive(message: &str) -> bool {
    let Some(index_name) = message
        .strip_prefix("wrong # of entries in index ")
        .filter(|name| !name.is_empty())
    else {
        return false;
    };
    OWNED_FTS_SHADOW_INDEXES.contains(&index_name)
}

/// Move the original live set into `live_pre_swap_dir`, then rename the
/// replacement to `data.db`. On any failure after the first live move,
/// restore the complete original set before returning.
fn swap_live_set(
    live_set: &[PathBuf],
    live_pre_swap_dir: &Path,
    replacement_path: &Path,
    agent_dir: &Path,
) -> Result<(), DbError> {
    let db_path = agent_dir.join(DB_FILE_NAME);

    // The replacement is a fresh inode; copy the original data.db mode and,
    // when different, Unix owner/group before it becomes the live file.
    preserve_file_identity(&db_path, replacement_path, agent_dir)?;
    sync_file(replacement_path, agent_dir)?;

    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut failure: Option<DbError> = None;
    for src in live_set {
        let dst = live_pre_swap_dir.join(file_name(src, agent_dir)?);
        if let Err(source) = std::fs::rename(src, &dst) {
            failure = Some(repair_err(
                agent_dir,
                format!("move {} into live-pre-swap: {source}", src.display()),
            ));
            break;
        }
        sync_directory(live_pre_swap_dir, agent_dir)?;
        sync_directory(agent_dir, agent_dir)?;
        moved.push((src.clone(), dst));
    }
    if failure.is_none()
        && let Err(source) = std::fs::rename(replacement_path, &db_path)
    {
        failure = Some(repair_err(
            agent_dir,
            format!("install replacement as {}: {source}", db_path.display()),
        ));
    }
    if failure.is_none() {
        sync_directory(agent_dir, agent_dir)?;
        sync_directory(live_pre_swap_dir, agent_dir)?;
    }
    if let Some(error) = failure {
        let rollback = rollback_live_set(&moved);
        let error = swap_rollback_error(agent_dir, error, rollback);
        return Err(remove_replacement_artifacts(replacement_path, error));
    }
    Ok(())
}

fn preserve_file_identity(src: &Path, dst: &Path, agent_dir: &Path) -> Result<(), DbError> {
    let src_metadata = std::fs::metadata(src)
        .map_err(|source| repair_err(agent_dir, format!("stat live data.db: {source}")))?;
    std::fs::set_permissions(dst, src_metadata.permissions()).map_err(|source| {
        repair_err(
            agent_dir,
            format!("apply original file mode to replacement: {source}"),
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let dst_metadata = std::fs::metadata(dst).map_err(|source| {
            repair_err(agent_dir, format!("stat replacement database: {source}"))
        })?;
        if src_metadata.uid() != dst_metadata.uid() || src_metadata.gid() != dst_metadata.gid() {
            std::os::unix::fs::chown(dst, Some(src_metadata.uid()), Some(src_metadata.gid()))
                .map_err(|source| {
                    repair_err(
                        agent_dir,
                        format!("apply original owner/group to replacement: {source}"),
                    )
                })?;
        }
    }
    Ok(())
}

/// Filesystem operations used by rollback. Keeping this boundary narrow makes
/// durability failures deterministic in tests without changing production IO.
trait RollbackFs {
    fn remove_file(&self, path: &Path) -> std::io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn sync_file(&self, path: &Path) -> std::io::Result<()>;
    fn sync_directory(&self, path: &Path) -> std::io::Result<()>;
}

struct StdRollbackFs;

impl RollbackFs for StdRollbackFs {
    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        std::fs::remove_file(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }

    fn sync_file(&self, path: &Path) -> std::io::Result<()> {
        std::fs::File::open(path)?.sync_all()
    }

    fn sync_directory(&self, path: &Path) -> std::io::Result<()> {
        std::fs::File::open(path)?.sync_all()
    }
}

/// A final-manifest failure means the operation cannot report a successful,
/// auditable repair. Remove the recovered live snapshot and restore the
/// complete original set before releasing the bootstrap lock.
fn rollback_completed_swap(report: &RepairReport) -> String {
    rollback_completed_swap_with_fs(report, &StdRollbackFs)
}

fn rollback_completed_swap_with_fs(report: &RepairReport, fs: &impl RollbackFs) -> String {
    let agent_dir = report.db_path.parent().unwrap_or(Path::new(""));
    let mut failures = Vec::new();
    rollback_remove_file(&report.db_path, agent_dir, fs, &mut failures);

    match std::fs::read_dir(&report.live_pre_swap_dir) {
        Ok(entries) => {
            let mut originals: Vec<PathBuf> = entries
                .filter_map(|entry| match entry {
                    Ok(entry) => Some(entry.path()),
                    Err(source) => {
                        failures.push(format!("list live-pre-swap entry: {source}"));
                        None
                    }
                })
                .collect();
            originals.sort();
            for preserved in originals {
                let Some(name) = preserved.file_name() else {
                    failures.push(format!(
                        "preserved path has no file name: {}",
                        preserved.display()
                    ));
                    continue;
                };
                let destination = agent_dir.join(name);
                rollback_rename_file(&preserved, &destination, fs, &mut failures);
            }
        }
        Err(source) => failures.push(format!(
            "list {}: {source}",
            report.live_pre_swap_dir.display()
        )),
    }
    rollback_summary(failures, "restored complete original set")
}

/// Move already-moved originals back, newest first. Returns a human summary
/// folded into the swap error; a rollback or durability failure names every
/// affected file or directory.
fn rollback_live_set(moved: &[(PathBuf, PathBuf)]) -> String {
    rollback_live_set_with_fs(moved, &StdRollbackFs)
}

fn rollback_live_set_with_fs(moved: &[(PathBuf, PathBuf)], fs: &impl RollbackFs) -> String {
    let mut failures = Vec::new();
    for (src, dst) in moved.iter().rev() {
        rollback_rename_file(dst, src, fs, &mut failures);
    }
    rollback_summary(
        failures,
        &format!("restored {} original file(s)", moved.len()),
    )
}

fn rollback_remove_file(
    path: &Path,
    agent_dir: &Path,
    fs: &impl RollbackFs,
    failures: &mut Vec<String>,
) {
    match fs.remove_file(path) {
        Ok(()) => sync_rollback_directory(path.parent().unwrap_or(agent_dir), fs, failures),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => failures.push(format!("remove recovered {}: {source}", path.display())),
    }
}

fn rollback_rename_file(from: &Path, to: &Path, fs: &impl RollbackFs, failures: &mut Vec<String>) {
    if let Err(source) = fs.rename(from, to) {
        failures.push(format!("restore {}: {source}", to.display()));
        return;
    }

    if let Err(source) = fs.sync_file(to) {
        failures.push(format!("sync restored file {}: {source}", to.display()));
    }
    match from.parent() {
        Some(parent) => sync_rollback_directory(parent, fs, failures),
        None => failures.push(format!(
            "sync rollback source parent: {} has no parent",
            from.display()
        )),
    }
    match to.parent() {
        Some(parent) => sync_rollback_directory(parent, fs, failures),
        None => failures.push(format!(
            "sync rollback destination parent: {} has no parent",
            to.display()
        )),
    }
}

fn sync_rollback_directory(path: &Path, fs: &impl RollbackFs, failures: &mut Vec<String>) {
    if let Err(source) = fs.sync_directory(path) {
        failures.push(format!(
            "sync rollback directory {}: {source}",
            path.display()
        ));
    }
}

fn rollback_summary(failures: Vec<String>, success: &str) -> String {
    if failures.is_empty() {
        success.to_string()
    } else {
        format!("ROLLBACK INCOMPLETE — {}", failures.join("; "))
    }
}

fn swap_rollback_error(agent_dir: &Path, original: DbError, rollback: String) -> DbError {
    repair_err(agent_dir, format!("{original:#}; rollback: {rollback}"))
}

/// Delete the replacement temp file and any sidecars a connection created
/// next to it after a failed build/validate. A cleanup failure never masks
/// the primary error; it is folded into the returned message instead (the
/// temp file never shadows the live set: distinct `data.db.recovering-*`
/// name).
fn remove_replacement_artifacts(replacement_path: &Path, primary: DbError) -> DbError {
    let cleanup = || -> Result<(), DbError> {
        match std::fs::remove_file(replacement_path) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(DbError::SidecarRemove {
                    path: replacement_path.to_path_buf(),
                    source,
                });
            }
        }
        remove_replacement_sidecars(replacement_path)
    };
    match cleanup() {
        Ok(()) => primary,
        Err(cleanup_error) => repair_err(
            replacement_path.parent().unwrap_or(Path::new("")),
            format!("{primary}; additionally replacement cleanup failed: {cleanup_error}"),
        ),
    }
}

fn remove_replacement_sidecars(replacement_path: &Path) -> Result<(), DbError> {
    for suffix in ["-wal", "-shm", "-tshm"] {
        let sidecar = sidecar_path(replacement_path, suffix);
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(DbError::SidecarRemove {
                    path: sidecar,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn manifest(
    report: &RepairReport,
    swap_status: &'static str,
    error: Option<String>,
) -> RecoveryManifest {
    RecoveryManifest {
        tool: "right",
        tool_version: env!("CARGO_PKG_VERSION"),
        operation: "wal-recovery",
        timestamp: chrono::Utc::now().to_rfc3339(),
        agent_dir: report
            .db_path
            .parent()
            .unwrap_or(Path::new(""))
            .display()
            .to_string(),
        recovery_dir: report.recovery_dir.display().to_string(),
        schema_version: Some(report.schema_version),
        quick_check: Some(report.quick_check.clone()),
        tables: report.tables.clone(),
        preserved_files: report.preserved_files.clone(),
        recovered_db: Some(report.recovered_db.clone()),
        swap_status,
        error,
    }
}

/// Failure manifest: whatever was learned before the failure, swap status,
/// and the error text (error chains carry paths/structural detail, never row
/// values).
fn failure_manifest(
    request: &RepairRequest,
    paths: &RecoveryPaths,
    error: &DbError,
) -> RecoveryManifest {
    RecoveryManifest {
        tool: "right",
        tool_version: env!("CARGO_PKG_VERSION"),
        operation: "wal-recovery",
        timestamp: chrono::Utc::now().to_rfc3339(),
        agent_dir: request.agent_dir.display().to_string(),
        recovery_dir: paths.recovery_dir.display().to_string(),
        schema_version: None,
        quick_check: None,
        tables: Vec::new(),
        preserved_files: Vec::new(),
        recovered_db: None,
        swap_status: "failed",
        error: Some(error.to_string()),
    }
}

fn write_manifest(
    paths: &RecoveryPaths,
    manifest: &RecoveryManifest,
    request: &RepairRequest,
) -> Result<(), DbError> {
    use std::io::Write;

    let json = serde_json::to_string_pretty(manifest).map_err(|source| {
        repair_err(
            &request.agent_dir,
            format!("serialize recovery manifest: {source}"),
        )
    })?;
    let mut file = std::fs::File::create(&paths.manifest_path).map_err(|source| {
        repair_err(
            &request.agent_dir,
            format!(
                "write recovery manifest {}: {source}",
                paths.manifest_path.display()
            ),
        )
    })?;
    file.write_all(json.as_bytes()).map_err(|source| {
        repair_err(
            &request.agent_dir,
            format!("write recovery manifest bytes: {source}"),
        )
    })?;
    file.sync_all().map_err(|source| {
        repair_err(
            &request.agent_dir,
            format!("sync recovery manifest: {source}"),
        )
    })?;
    sync_directory(&paths.recovery_dir, &request.agent_dir)
}

/// Copy `src` to `dst` while hashing; returns the digest of what was read.
fn copy_and_digest(src: &Path, dst: &Path, agent_dir: &Path) -> Result<FileDigest, DbError> {
    use std::io::{Read, Write};

    let mut reader = std::fs::File::open(src).map_err(|source| {
        repair_err(
            agent_dir,
            format!("open {} for copy: {source}", src.display()),
        )
    })?;
    let mut writer = std::fs::File::create(dst).map_err(|source| {
        repair_err(
            agent_dir,
            format!("create copy {}: {source}", dst.display()),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| repair_err(agent_dir, format!("read {}: {source}", src.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        writer.write_all(&buffer[..read]).map_err(|source| {
            repair_err(agent_dir, format!("write {}: {source}", dst.display()))
        })?;
        size += read as u64;
    }
    writer
        .flush()
        .map_err(|source| repair_err(agent_dir, format!("flush {}: {source}", dst.display())))?;
    writer
        .sync_all()
        .map_err(|source| repair_err(agent_dir, format!("sync {}: {source}", dst.display())))?;
    Ok(FileDigest {
        name: file_name(src, agent_dir)?,
        size,
        sha256: hex_lower(&hasher.finalize()),
    })
}

fn digest_file(path: &Path, agent_dir: &Path) -> Result<FileDigest, DbError> {
    use std::io::Read;

    let mut reader = std::fs::File::open(path).map_err(|source| {
        repair_err(
            agent_dir,
            format!("open {} for hashing: {source}", path.display()),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    loop {
        let read = reader.read(&mut buffer).map_err(|source| {
            repair_err(agent_dir, format!("read {}: {source}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok(FileDigest {
        name: file_name(path, agent_dir)?,
        size,
        sha256: hex_lower(&hasher.finalize()),
    })
}

fn sync_file(path: &Path, agent_dir: &Path) -> Result<(), DbError> {
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|source| repair_err(agent_dir, format!("sync {}: {source}", path.display())))
}

fn sync_directory(path: &Path, agent_dir: &Path) -> Result<(), DbError> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| {
            repair_err(
                agent_dir,
                format!("sync directory {}: {source}", path.display()),
            )
        })
}

fn file_name(path: &Path, agent_dir: &Path) -> Result<String, DbError> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| {
            repair_err(
                agent_dir,
                format!("path has no file name: {}", path.display()),
            )
        })
}

fn sidecar_path(file: &Path, suffix: &str) -> PathBuf {
    let mut path = file.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

fn repair_err(agent_dir: &Path, message: String) -> DbError {
    DbError::Repair {
        path: agent_dir.join(DB_FILE_NAME),
        message,
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io;

    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum FsCall {
        Remove(PathBuf),
        Rename(PathBuf, PathBuf),
        SyncFile(PathBuf),
        SyncDirectory(PathBuf),
    }

    #[derive(Default)]
    struct RecordingRollbackFs {
        calls: RefCell<Vec<FsCall>>,
        fail_file_sync: Option<PathBuf>,
        fail_directory_syncs: Vec<PathBuf>,
    }

    impl RollbackFs for RecordingRollbackFs {
        fn remove_file(&self, path: &Path) -> io::Result<()> {
            self.calls.borrow_mut().push(FsCall::Remove(path.into()));
            Ok(())
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.calls
                .borrow_mut()
                .push(FsCall::Rename(from.into(), to.into()));
            Ok(())
        }

        fn sync_file(&self, path: &Path) -> io::Result<()> {
            self.calls.borrow_mut().push(FsCall::SyncFile(path.into()));
            if self.fail_file_sync.as_deref() == Some(path) {
                return Err(io::Error::other("injected restored-file sync failure"));
            }
            Ok(())
        }

        fn sync_directory(&self, path: &Path) -> io::Result<()> {
            self.calls
                .borrow_mut()
                .push(FsCall::SyncDirectory(path.into()));
            if self
                .fail_directory_syncs
                .iter()
                .any(|failed| failed == path)
            {
                return Err(io::Error::other("injected directory sync failure"));
            }
            Ok(())
        }
    }

    #[test]
    fn rollback_rename_syncs_file_and_both_parent_directories() {
        let agent_dir = PathBuf::from("/agent");
        let preserved_dir = PathBuf::from("/backup/live-pre-swap");
        let restored = agent_dir.join(DB_FILE_NAME);
        let preserved = preserved_dir.join(DB_FILE_NAME);
        let fs = RecordingRollbackFs::default();

        let summary = rollback_live_set_with_fs(&[(restored.clone(), preserved.clone())], &fs);

        assert_eq!(summary, "restored 1 original file(s)");
        assert_eq!(
            *fs.calls.borrow(),
            [
                FsCall::Rename(preserved, restored.clone()),
                FsCall::SyncFile(restored),
                FsCall::SyncDirectory(preserved_dir),
                FsCall::SyncDirectory(agent_dir),
            ]
        );
    }

    #[test]
    fn rollback_sync_failure_keeps_original_swap_error_context() {
        let agent_dir = PathBuf::from("/agent");
        let preserved_dir = PathBuf::from("/backup/live-pre-swap");
        let restored = agent_dir.join(DB_FILE_NAME);
        let preserved = preserved_dir.join(DB_FILE_NAME);
        let fs = RecordingRollbackFs {
            fail_file_sync: Some(restored.clone()),
            fail_directory_syncs: vec![preserved_dir.clone(), agent_dir.clone()],
            ..RecordingRollbackFs::default()
        };
        let original = repair_err(
            &agent_dir,
            "install replacement as /agent/data.db: injected swap failure".to_string(),
        );

        let rollback = rollback_live_set_with_fs(&[(restored.clone(), preserved.clone())], &fs);
        let error = swap_rollback_error(&agent_dir, original, rollback);
        let message = error.to_string();

        assert!(message.contains("injected swap failure"), "{message}");
        assert!(
            message.contains("injected restored-file sync failure"),
            "{message}"
        );
        assert!(
            message.contains(
                "sync rollback directory /backup/live-pre-swap: injected directory sync failure"
            ),
            "{message}"
        );
        assert!(
            message.contains("sync rollback directory /agent: injected directory sync failure"),
            "{message}"
        );
        assert_eq!(
            *fs.calls.borrow(),
            [
                FsCall::Rename(preserved, restored.clone()),
                FsCall::SyncFile(restored),
                FsCall::SyncDirectory(preserved_dir),
                FsCall::SyncDirectory(agent_dir),
            ],
            "directory durability must still be attempted after file sync fails"
        );
    }

    #[test]
    fn rollback_remove_syncs_affected_parent_directory() {
        let agent_dir = PathBuf::from("/agent");
        let recovered = agent_dir.join(DB_FILE_NAME);
        let fs = RecordingRollbackFs::default();
        let mut failures = Vec::new();

        rollback_remove_file(&recovered, &agent_dir, &fs, &mut failures);

        assert!(failures.is_empty());
        assert_eq!(
            *fs.calls.borrow(),
            [FsCall::Remove(recovered), FsCall::SyncDirectory(agent_dir),]
        );
    }
}
