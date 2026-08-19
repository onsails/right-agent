//! Content-addressed platform store for atomic sandbox file deployment.
//!
//! Platform-managed files are uploaded to `/platform/` with content-hash suffixes,
//! then symlinked from their expected locations in `/sandbox/.claude/`.
//! Agent-owned files live directly in `/sandbox/` and are never overwritten.

#![warn(unreachable_pub)]

use futures::stream::{self, StreamExt};
use right_sandbox::{ExecRequest, SandboxHandle};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

/// Timeout for the small shell operations this module runs in the guest
/// (`mkdir`, `ln`, `mv`, `find`). Deployment itself moves bytes over the fs
/// API, which is not bounded by this.
const EXEC_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `argv` in the guest and return `(stdout, exit_code)`.
///
/// A non-zero exit code is data; only a transport failure is an `Err`.
async fn exec(sbox: &SandboxHandle, argv: &[&str]) -> miette::Result<(String, i32)> {
    let (cmd, args) = argv
        .split_first()
        .ok_or_else(|| miette::miette!("exec argv must name a program"))?;
    let request = ExecRequest {
        cmd: (*cmd).to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        timeout: Some(EXEC_TIMEOUT),
        ..ExecRequest::default()
    };
    let outcome = sbox
        .exec(&request)
        .await
        .map_err(|e| miette::miette!("sandbox exec {argv:?} failed: {e:#}"))?;
    Ok((
        String::from_utf8_lossy(&outcome.stdout).into_owned(),
        outcome.code,
    ))
}

#[cfg(test)]
#[path = "platform_store_tests.rs"]
mod tests;

/// 8-char hex hash of content bytes (first 4 bytes of SHA-256).
pub fn content_hash(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    format!("{:08x}", u32::from_be_bytes(hash[..4].try_into().unwrap()))
}

/// Hash of a directory's contents. Walks files sorted by relative path,
/// hashes (path + content) for each.
pub fn directory_hash(dir: &Path) -> miette::Result<String> {
    let mut hasher = Sha256::new();
    let all_entries: Vec<walkdir::DirEntry> = walkdir::WalkDir::new(dir)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| miette::miette!("walkdir error in {}: {e:#}", dir.display()))?;
    let mut entries: Vec<_> = all_entries
        .into_iter()
        .filter(|e| e.file_type().is_file())
        .collect();
    entries.sort_by_key(|e| e.path().to_path_buf());
    for entry in entries {
        let rel = entry
            .path()
            .strip_prefix(dir)
            .map_err(|e| miette::miette!("strip_prefix: {e:#}"))?;
        hasher.update(rel.to_string_lossy().as_bytes());
        let content = std::fs::read(entry.path())
            .map_err(|e| miette::miette!("read {}: {e:#}", entry.path().display()))?;
        hasher.update(&content);
    }
    let hash = hasher.finalize();
    Ok(format!(
        "{:08x}",
        u32::from_be_bytes(hash[..4].try_into().unwrap())
    ))
}

/// Content-addressed name: `name.hash`
pub fn platform_path(name: &str, hash: &str) -> String {
    format!("{name}.{hash}")
}

/// A platform-managed entry to deploy to /platform/.
pub struct ManifestEntry {
    pub name: String,
    pub host_path: std::path::PathBuf,
    pub hash: String,
    pub link_path: String,
    pub platform_prefix: String,
    /// Cached file content (avoids double-read during deploy). None for directories.
    pub content: Option<Vec<u8>>,
    pub is_dir: bool,
}

/// Complete manifest of platform-managed entries.
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
}

/// Base path for platform store inside sandbox.
pub const PLATFORM_DIR: &str = "/sandbox/.platform";

/// Scan agent directory, build manifest of platform-managed files.
/// Excludes agent-owned files (IDENTITY.md, SOUL.md, USER.md, TOOLS.md).
/// File content is cached in the manifest to avoid double-reads during deploy.
///
/// `builtin_skill_names` enumerates the platform-managed skill directories
/// to deploy (host `.claude/skills/<name>/` → sandbox `/sandbox/.claude/skills/<name>/`).
/// The list is owned by `right-codegen` (`BUILTIN_SKILL_NAMES`) so the installer
/// and the deployer share a single source of truth.
pub fn build_manifest(agent_dir: &Path, builtin_skill_names: &[&str]) -> miette::Result<Manifest> {
    let claude_dir = agent_dir.join(".claude");
    let mut entries = Vec::new();

    // Files in .claude/
    let claude_files: &[(&str, &str)] = &[
        ("settings.json", "/sandbox/.claude/settings.json"),
        ("reply-schema.json", "/sandbox/.claude/reply-schema.json"),
        ("cron-schema.json", "/sandbox/.claude/cron-schema.json"),
        ("system-prompt.md", "/sandbox/.claude/system-prompt.md"),
        (
            "bootstrap-schema.json",
            "/sandbox/.claude/bootstrap-schema.json",
        ),
    ];

    for &(name, link) in claude_files {
        let path = claude_dir.join(name);
        if path.exists() {
            let content =
                std::fs::read(&path).map_err(|e| miette::miette!("read {name}: {e:#}"))?;
            let hash = content_hash(&content);
            entries.push(ManifestEntry {
                name: name.to_owned(),
                host_path: path,
                hash,
                link_path: link.to_owned(),
                platform_prefix: String::new(),
                content: Some(content),
                is_dir: false,
            });
        }
    }

    // mcp.json at agent root
    let mcp_json = agent_dir.join("mcp.json");
    if mcp_json.exists() {
        let content =
            std::fs::read(&mcp_json).map_err(|e| miette::miette!("read mcp.json: {e:#}"))?;
        let hash = content_hash(&content);
        entries.push(ManifestEntry {
            name: "mcp.json".to_owned(),
            host_path: mcp_json,
            hash,
            link_path: "/sandbox/mcp.json".to_owned(),
            platform_prefix: String::new(),
            content: Some(content),
            is_dir: false,
        });
    }

    // Builtin skills (directories)
    let skills_dir = claude_dir.join("skills");
    for skill_name in builtin_skill_names {
        let skill_path = skills_dir.join(skill_name);
        if skill_path.exists() && skill_path.is_dir() {
            let hash = directory_hash(&skill_path)?;
            entries.push(ManifestEntry {
                name: skill_name.to_string(),
                host_path: skill_path,
                hash,
                link_path: format!("/sandbox/.claude/skills/{skill_name}"),
                platform_prefix: "skills/".to_owned(),
                content: None,
                is_dir: true,
            });
        }
    }

    Ok(Manifest { entries })
}

/// Create an atomic symlink: tmp link → rm old → mv into place.
///
/// Uses `ln -sfn` (works for both files and directories).
async fn atomic_symlink(sbox: &SandboxHandle, target: &str, link_path: &str) -> miette::Result<()> {
    // Ensure parent directory exists.
    if let Some(parent) = Path::new(link_path).parent() {
        let parent_str = parent.to_string_lossy();
        let (_, code) = exec(sbox, &["mkdir", "-p", &parent_str]).await?;
        if code != 0 {
            miette::bail!("mkdir -p {parent_str} failed with exit code {code}");
        }
    }

    let link_name = Path::new(link_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "link".to_owned());
    let tmp_link = format!("/tmp/right-link-{link_name}");

    let (_, code) = exec(sbox, &["ln", "-sfn", target, &tmp_link]).await?;
    if code != 0 {
        miette::bail!("ln -sfn {target} {tmp_link} failed with exit code {code}");
    }

    // Remove old target (handles migration from direct files/dirs).
    exec(sbox, &["rm", "-rf", link_path]).await?;

    let (_, code) = exec(sbox, &["mv", "-fT", &tmp_link, link_path]).await?;
    if code != 0 {
        miette::bail!("mv -fT {tmp_link} {link_path} failed with exit code {code}");
    }

    Ok(())
}

/// Upload a content-addressed file to /platform/, create atomic symlink at `link_path`.
/// Uses cached content from manifest to avoid re-reading from disk.
///
/// Returns the full platform path (for GC tracking).
pub async fn deploy_file(
    sbox: &SandboxHandle,
    name: &str,
    content: &[u8],
    hash: &str,
    link_path: &str,
    platform_prefix: &str,
) -> miette::Result<String> {
    let addressed_name = platform_path(name, hash);
    let full_platform_path = format!("{PLATFORM_DIR}/{platform_prefix}{addressed_name}");

    // Check if content-addressed file already exists (dedup).
    let exists = sbox
        .fs_exists(&full_platform_path)
        .await
        .map_err(|e| miette::miette!("stat {full_platform_path} failed: {e:#}"))?;
    if !exists {
        let platform_dir = format!("{PLATFORM_DIR}/{}", platform_prefix.trim_end_matches('/'));
        sbox.fs_mkdir(&platform_dir)
            .await
            .map_err(|e| miette::miette!("mkdir -p {platform_dir} failed: {e:#}"))?;

        // The fs API writes straight to the content-addressed name, so there
        // is no host temp file and no upload-then-rename step.
        sbox.fs_write(&full_platform_path, content)
            .await
            .map_err(|e| miette::miette!("write {full_platform_path} failed: {e:#}"))?;
    }

    atomic_symlink(sbox, &full_platform_path, link_path).await?;
    Ok(full_platform_path)
}

/// Upload a content-addressed directory to /platform/, create atomic symlink at `link_path`.
///
/// Returns the full platform path (for GC tracking).
pub async fn deploy_directory(
    sbox: &SandboxHandle,
    name: &str,
    host_dir: &std::path::Path,
    hash: &str,
    link_path: &str,
    platform_prefix: &str,
) -> miette::Result<String> {
    let addressed_name = platform_path(name, hash);
    let full_platform_path = format!("{PLATFORM_DIR}/{platform_prefix}{addressed_name}");

    // Check if content-addressed directory already exists (dedup).
    let exists = sbox
        .fs_exists(&full_platform_path)
        .await
        .map_err(|e| miette::miette!("stat {full_platform_path} failed: {e:#}"))?;
    if !exists {
        sbox.fs_mkdir(&full_platform_path)
            .await
            .map_err(|e| miette::miette!("mkdir -p {full_platform_path} failed: {e:#}"))?;

        // Collect files with relative paths.
        let mut file_entries: Vec<(std::path::PathBuf, String)> = Vec::new();
        for entry in walkdir::WalkDir::new(host_dir) {
            let entry = entry.map_err(|e| miette::miette!("walkdir: {e:#}"))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(host_dir)
                .map_err(|e| miette::miette!("strip_prefix: {e:#}"))?;
            file_entries.push((
                entry.path().to_path_buf(),
                rel.to_string_lossy().to_string(),
            ));
        }

        // Create subdirectories.
        let mut subdirs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (_, rel) in &file_entries {
            if let Some(parent) = Path::new(rel).parent() {
                let p = parent.to_string_lossy().to_string();
                if !p.is_empty() {
                    subdirs.insert(p);
                }
            }
        }
        for subdir in &subdirs {
            let dir_path = format!("{full_platform_path}/{subdir}");
            sbox.fs_mkdir(&dir_path)
                .await
                .map_err(|e| miette::miette!("mkdir -p {dir_path} failed: {e:#}"))?;
        }

        // Upload files in parallel.
        let platform_base = full_platform_path.as_str();

        let results: Vec<miette::Result<()>> = stream::iter(file_entries)
            .map(|(host_path, rel_path)| async move {
                let dest = format!("{platform_base}/{rel_path}");
                sbox.fs_copy_from_host(&host_path, &dest).await.map_err(|e| {
                    miette::miette!("upload {} -> {dest} failed: {e:#}", host_path.display())
                })
            })
            .buffer_unordered(10)
            .collect()
            .await;

        for result in results {
            result?;
        }
    }

    atomic_symlink(sbox, &full_platform_path, link_path).await?;
    Ok(full_platform_path)
}

/// Garbage-collect stale entries from /platform/.
///
/// Best-effort: logs warnings but does not fail.
pub async fn gc_platform(sbox: &SandboxHandle, active_targets: &[String]) -> miette::Result<()> {
    let active_set: std::collections::HashSet<&str> =
        active_targets.iter().map(|s| s.as_str()).collect();

    let (stdout, exit_code) = exec(
        sbox,
        &["find", PLATFORM_DIR, "-mindepth", "1", "-maxdepth", "2"],
    )
    .await?;

    if exit_code != 0 {
        tracing::warn!("gc_platform: find {PLATFORM_DIR} exited with code {exit_code}");
        return Ok(());
    }

    let mut stale: Vec<&str> = Vec::new();
    for line in stdout.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        if active_set.contains(path) {
            continue;
        }
        // Keep if any active target starts with this path (prefix directory).
        let is_prefix = active_targets
            .iter()
            .any(|t| t.starts_with(path) && t.len() > path.len());
        if is_prefix {
            continue;
        }
        stale.push(path);
    }

    if !stale.is_empty() {
        let mut rm_args: Vec<&str> = vec!["rm", "-rf"];
        rm_args.extend(&stale);
        let (_, code) = exec(sbox, &rm_args).await?;
        if code != 0 {
            tracing::warn!("gc_platform: rm -rf failed with exit code {code}");
        } else {
            tracing::debug!(count = stale.len(), "gc_platform: removed stale entries");
        }
    }

    Ok(())
}

/// Deploy all entries from a manifest, then GC stale entries.
pub async fn deploy_manifest(sbox: &SandboxHandle, manifest: &Manifest) -> miette::Result<()> {
    // Ensure /platform/ exists. The parent stays root-owned; agent-owned
    // content lives under the agent's own subdirectory.
    sbox.fs_mkdir(PLATFORM_DIR).await.map_err(|e| {
        miette::miette!(
            "mkdir -p {PLATFORM_DIR} failed: {e:#}. \
             Recreate the sandbox if this persists."
        )
    })?;

    // Make writable (previous run made it a-w). Best-effort on first run.
    if let Err(e) = exec(sbox, &["chmod", "-R", "u+w", PLATFORM_DIR]).await {
        tracing::warn!("chmod u+w {PLATFORM_DIR} failed (may be first run): {e:#}");
    }

    let mut active_targets: Vec<String> = Vec::new();

    // Deploy all entries (files use cached content, directories walk host_path).
    for entry in &manifest.entries {
        let target = if entry.is_dir {
            deploy_directory(
                sbox,
                &entry.name,
                &entry.host_path,
                &entry.hash,
                &entry.link_path,
                &entry.platform_prefix,
            )
            .await?
        } else {
            let content = entry.content.as_ref().ok_or_else(|| {
                miette::miette!("file entry {} has no cached content", entry.name)
            })?;
            deploy_file(
                sbox,
                &entry.name,
                content,
                &entry.hash,
                &entry.link_path,
                &entry.platform_prefix,
            )
            .await?
        };
        active_targets.push(target);
    }

    // GC stale entries.
    gc_platform(sbox, &active_targets).await?;

    // Make read-only to prevent agent modification.
    let (_, code) = exec(sbox, &["chmod", "-R", "a-w", PLATFORM_DIR]).await?;
    if code != 0 {
        miette::bail!("chmod -R a-w {PLATFORM_DIR} failed with exit code {code}");
    }

    Ok(())
}
