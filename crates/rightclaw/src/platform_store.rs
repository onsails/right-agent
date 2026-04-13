//! Content-addressed platform store for atomic sandbox file deployment.

use futures::stream::{self, StreamExt};
use sha2::{Digest, Sha256};
use std::path::Path;

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
    let mut entries: Vec<_> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
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
    Ok(format!("{:08x}", u32::from_be_bytes(hash[..4].try_into().unwrap())))
}

/// Content-addressed name: `name.hash`
pub fn platform_path(name: &str, hash: &str) -> String {
    format!("{name}.{hash}")
}

/// A single file to deploy to /platform/.
pub struct FileEntry {
    pub name: String,
    pub host_path: std::path::PathBuf,
    pub hash: String,
    pub link_path: String,
    pub platform_prefix: String,
}

/// A directory to deploy to /platform/.
pub struct DirEntry {
    pub name: String,
    pub host_path: std::path::PathBuf,
    pub hash: String,
    pub link_path: String,
    pub platform_prefix: String,
}

/// Complete manifest of platform-managed files and directories.
pub struct Manifest {
    pub files: Vec<FileEntry>,
    pub dirs: Vec<DirEntry>,
}

/// Base path for platform store inside sandbox.
pub const PLATFORM_DIR: &str = "/platform";

/// Scan agent directory, build manifest of platform-managed files.
/// Excludes agent-owned files (IDENTITY.md, SOUL.md, USER.md, AGENTS.md, TOOLS.md).
pub fn build_manifest(agent_dir: &Path) -> miette::Result<Manifest> {
    let claude_dir = agent_dir.join(".claude");
    let mut files = Vec::new();
    let mut dirs = Vec::new();

    // Files in .claude/
    let claude_files: &[(&str, &str)] = &[
        ("settings.json", "/sandbox/.claude/settings.json"),
        ("reply-schema.json", "/sandbox/.claude/reply-schema.json"),
        ("cron-schema.json", "/sandbox/.claude/cron-schema.json"),
        ("system-prompt.md", "/sandbox/.claude/system-prompt.md"),
        ("bootstrap-schema.json", "/sandbox/.claude/bootstrap-schema.json"),
    ];

    for &(name, link) in claude_files {
        let path = claude_dir.join(name);
        if path.exists() {
            let content = std::fs::read(&path)
                .map_err(|e| miette::miette!("read {name}: {e:#}"))?;
            files.push(FileEntry {
                name: name.to_owned(),
                host_path: path,
                hash: content_hash(&content),
                link_path: link.to_owned(),
                platform_prefix: String::new(),
            });
        }
    }

    // Agent def files in .claude/agents/ (skip agent-owned AGENTS.md/TOOLS.md)
    let agents_dir = claude_dir.join("agents");
    if agents_dir.exists() {
        for entry in std::fs::read_dir(&agents_dir)
            .map_err(|e| miette::miette!("read agents dir: {e:#}"))?
        {
            let entry = entry.map_err(|e| miette::miette!("readdir: {e:#}"))?;
            let name_os = entry.file_name();
            let name = name_os.to_string_lossy();
            if name == "AGENTS.md" || name == "TOOLS.md" {
                continue;
            }
            let path = entry.path();
            if path.is_file() {
                let content = std::fs::read(&path)
                    .map_err(|e| miette::miette!("read agent def {name}: {e:#}"))?;
                files.push(FileEntry {
                    name: name.to_string(),
                    host_path: path,
                    hash: content_hash(&content),
                    link_path: format!("/sandbox/.claude/agents/{name}"),
                    platform_prefix: "agents/".to_owned(),
                });
            }
        }
    }

    // mcp.json at agent root
    let mcp_json = agent_dir.join("mcp.json");
    if mcp_json.exists() {
        let content = std::fs::read(&mcp_json)
            .map_err(|e| miette::miette!("read mcp.json: {e:#}"))?;
        files.push(FileEntry {
            name: "mcp.json".to_owned(),
            host_path: mcp_json,
            hash: content_hash(&content),
            link_path: "/sandbox/mcp.json".to_owned(),
            platform_prefix: String::new(),
        });
    }

    // Builtin skills (directories)
    let skills_dir = claude_dir.join("skills");
    for skill_name in &["rightskills", "rightcron", "rightmcp"] {
        let skill_path = skills_dir.join(skill_name);
        if skill_path.exists() && skill_path.is_dir() {
            let hash = directory_hash(&skill_path)?;
            dirs.push(DirEntry {
                name: skill_name.to_string(),
                host_path: skill_path,
                hash,
                link_path: format!("/sandbox/.claude/skills/{skill_name}"),
                platform_prefix: "skills/".to_owned(),
            });
        }
    }

    Ok(Manifest { files, dirs })
}

/// Upload a content-addressed file to /platform/, create atomic symlink at `link_path`.
///
/// Returns the full platform path (for GC tracking).
pub async fn deploy_file(
    sandbox: &str,
    name: &str,
    content: &[u8],
    link_path: &str,
    platform_prefix: &str,
) -> miette::Result<String> {
    use crate::openshell::{exec_command, upload_file};

    let hash = content_hash(content);
    let addressed_name = platform_path(name, &hash);
    let full_platform_path = format!("{PLATFORM_DIR}/{platform_prefix}{addressed_name}");

    // Check if content-addressed file already exists (dedup).
    let (_, exit_code) = exec_command(sandbox, &["test", "-e", &full_platform_path]).await?;
    if exit_code != 0 {
        // File does not exist — upload it.
        let platform_dir = format!(
            "{PLATFORM_DIR}/{}",
            platform_prefix.trim_end_matches('/')
        );
        let (_, code) = exec_command(sandbox, &["mkdir", "-p", &platform_dir]).await?;
        if code != 0 {
            miette::bail!("mkdir -p {platform_dir} failed with exit code {code}");
        }

        // Write content to a temp file on host, upload, then rename.
        let tmp_dir = tempfile::tempdir()
            .map_err(|e| miette::miette!("create tempdir: {e:#}"))?;
        let tmp_file = tmp_dir.path().join(name);
        std::fs::write(&tmp_file, content)
            .map_err(|e| miette::miette!("write temp file {}: {e:#}", tmp_file.display()))?;

        // upload_file destination must end with '/'.
        let upload_dest = format!("{platform_dir}/");
        upload_file(sandbox, &tmp_file, &upload_dest).await?;

        // Rename from uploaded name to content-addressed name.
        let uploaded_path = format!("{platform_dir}/{name}");
        let (_, code) = exec_command(
            sandbox,
            &["mv", &uploaded_path, &full_platform_path],
        )
        .await?;
        if code != 0 {
            miette::bail!(
                "mv {uploaded_path} -> {full_platform_path} failed with exit code {code}"
            );
        }
    }

    // Ensure parent directory of link_path exists.
    if let Some(parent) = Path::new(link_path).parent() {
        let parent_str = parent.to_string_lossy();
        let (_, code) = exec_command(sandbox, &["mkdir", "-p", &parent_str]).await?;
        if code != 0 {
            miette::bail!("mkdir -p {parent_str} failed with exit code {code}");
        }
    }

    // Atomic symlink: create at temp location, remove old target, move into place.
    let link_name = Path::new(link_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_owned());
    let tmp_link = format!("/tmp/rightclaw-link-{link_name}");

    let (_, code) = exec_command(
        sandbox,
        &["ln", "-sf", &full_platform_path, &tmp_link],
    )
    .await?;
    if code != 0 {
        miette::bail!("ln -sf {full_platform_path} {tmp_link} failed with exit code {code}");
    }

    // Remove old target (handles migration from direct files/dirs).
    let (_, _) = exec_command(sandbox, &["rm", "-rf", link_path]).await?;

    let (_, code) = exec_command(
        sandbox,
        &["mv", "-fT", &tmp_link, link_path],
    )
    .await?;
    if code != 0 {
        miette::bail!("mv -fT {tmp_link} {link_path} failed with exit code {code}");
    }

    Ok(full_platform_path)
}

/// Upload a content-addressed directory to /platform/, create atomic symlink at `link_path`.
///
/// Returns the full platform path (for GC tracking).
pub async fn deploy_directory(
    sandbox: &str,
    name: &str,
    host_dir: &std::path::Path,
    link_path: &str,
    platform_prefix: &str,
) -> miette::Result<String> {
    use crate::openshell::{exec_command, upload_file};

    let hash = directory_hash(host_dir)?;
    let addressed_name = platform_path(name, &hash);
    let full_platform_path = format!("{PLATFORM_DIR}/{platform_prefix}{addressed_name}");

    // Check if content-addressed directory already exists (dedup).
    let (_, exit_code) = exec_command(sandbox, &["test", "-d", &full_platform_path]).await?;
    if exit_code != 0 {
        // Directory does not exist — upload all files individually in parallel.
        let (_, code) = exec_command(sandbox, &["mkdir", "-p", &full_platform_path]).await?;
        if code != 0 {
            miette::bail!("mkdir -p {full_platform_path} failed with exit code {code}");
        }

        // Collect all files with their relative paths.
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
            file_entries.push((entry.path().to_path_buf(), rel.to_string_lossy().to_string()));
        }

        // Create all subdirectories first.
        let mut subdirs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (_, rel) in &file_entries {
            if let Some(parent) = Path::new(rel).parent() {
                let parent_str = parent.to_string_lossy().to_string();
                if !parent_str.is_empty() {
                    subdirs.insert(parent_str);
                }
            }
        }
        for subdir in &subdirs {
            let dir_path = format!("{full_platform_path}/{subdir}");
            let (_, code) = exec_command(sandbox, &["mkdir", "-p", &dir_path]).await?;
            if code != 0 {
                miette::bail!("mkdir -p {dir_path} failed with exit code {code}");
            }
        }

        // Upload files in parallel (buffer_unordered(10)).
        let sandbox_owned = sandbox.to_owned();
        let platform_path_owned = full_platform_path.clone();

        let results: Vec<miette::Result<()>> = stream::iter(file_entries)
            .map(|(host_path, rel_path)| {
                let sandbox = sandbox_owned.clone();
                let platform_base = platform_path_owned.clone();
                async move {
                    let dest_dir = if let Some(parent) = Path::new(&rel_path).parent() {
                        let p = parent.to_string_lossy();
                        if p.is_empty() {
                            format!("{platform_base}/")
                        } else {
                            format!("{platform_base}/{p}/")
                        }
                    } else {
                        format!("{platform_base}/")
                    };
                    upload_file(&sandbox, &host_path, &dest_dir).await?;
                    Ok(())
                }
            })
            .buffer_unordered(10)
            .collect()
            .await;

        for result in results {
            result?;
        }
    }

    // Ensure parent directory of link_path exists.
    if let Some(parent) = Path::new(link_path).parent() {
        let parent_str = parent.to_string_lossy();
        let (_, code) = exec_command(sandbox, &["mkdir", "-p", &parent_str]).await?;
        if code != 0 {
            miette::bail!("mkdir -p {parent_str} failed with exit code {code}");
        }
    }

    // Atomic symlink for directory: ln -sfn (not ln -sf).
    let link_name = Path::new(link_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_owned());
    let tmp_link = format!("/tmp/rightclaw-link-{link_name}");

    let (_, code) = exec_command(
        sandbox,
        &["ln", "-sfn", &full_platform_path, &tmp_link],
    )
    .await?;
    if code != 0 {
        miette::bail!("ln -sfn {full_platform_path} {tmp_link} failed with exit code {code}");
    }

    // Remove old target (handles migration from direct dirs).
    let (_, _) = exec_command(sandbox, &["rm", "-rf", link_path]).await?;

    let (_, code) = exec_command(
        sandbox,
        &["mv", "-fT", &tmp_link, link_path],
    )
    .await?;
    if code != 0 {
        miette::bail!("mv -fT {tmp_link} {link_path} failed with exit code {code}");
    }

    Ok(full_platform_path)
}

/// Garbage-collect stale entries from /platform/.
///
/// Lists all files/dirs in /platform/, removes anything not in `active_targets`.
/// Best-effort: logs warnings but does not fail.
pub async fn gc_platform(sandbox: &str, active_targets: &[String]) -> miette::Result<()> {
    use crate::openshell::exec_command;

    let active_set: std::collections::HashSet<&str> =
        active_targets.iter().map(|s| s.as_str()).collect();

    // List all entries under /platform/ (depth 2 to catch prefix/name.hash).
    let (stdout, exit_code) = exec_command(
        sandbox,
        &["find", PLATFORM_DIR, "-mindepth", "1", "-maxdepth", "2"],
    )
    .await?;

    if exit_code != 0 {
        tracing::warn!("gc_platform: find {PLATFORM_DIR} exited with code {exit_code}");
        return Ok(());
    }

    // Collect paths that are NOT prefixes of any active target.
    // We need to keep both the direct entries and their parent prefix dirs.
    for line in stdout.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }

        // Keep if this path IS an active target.
        if active_set.contains(path) {
            continue;
        }

        // Keep if any active target starts with this path (it's a prefix directory).
        let is_prefix = active_targets
            .iter()
            .any(|t| t.starts_with(path) && t.len() > path.len());
        if is_prefix {
            continue;
        }

        tracing::info!("gc_platform: removing stale {path}");
        let (_, code) = exec_command(sandbox, &["rm", "-rf", path]).await?;
        if code != 0 {
            tracing::warn!("gc_platform: rm -rf {path} exited with code {code}");
        }
    }

    Ok(())
}

/// Deploy all files and directories from a manifest, then GC stale entries.
pub async fn deploy_manifest(sandbox: &str, manifest: &Manifest) -> miette::Result<()> {
    use crate::openshell::exec_command;

    // Ensure /platform/ exists.
    let (_, code) = exec_command(sandbox, &["mkdir", "-p", PLATFORM_DIR]).await?;
    if code != 0 {
        miette::bail!("mkdir -p {PLATFORM_DIR} failed with exit code {code}");
    }

    // Make writable (previous run made it a-w). Best-effort — may not exist on first run.
    let (_, _) = exec_command(sandbox, &["chmod", "-R", "u+w", PLATFORM_DIR]).await?;

    let mut active_targets: Vec<String> = Vec::new();

    // Deploy all files.
    for file_entry in &manifest.files {
        let content = std::fs::read(&file_entry.host_path)
            .map_err(|e| miette::miette!("read {}: {e:#}", file_entry.host_path.display()))?;
        let target = deploy_file(
            sandbox,
            &file_entry.name,
            &content,
            &file_entry.link_path,
            &file_entry.platform_prefix,
        )
        .await?;
        active_targets.push(target);
    }

    // Deploy all directories.
    for dir_entry in &manifest.dirs {
        let target = deploy_directory(
            sandbox,
            &dir_entry.name,
            &dir_entry.host_path,
            &dir_entry.link_path,
            &dir_entry.platform_prefix,
        )
        .await?;
        active_targets.push(target);
    }

    // GC stale entries.
    gc_platform(sandbox, &active_targets).await?;

    // Make read-only to prevent agent modification.
    let (_, code) = exec_command(sandbox, &["chmod", "-R", "a-w", PLATFORM_DIR]).await?;
    if code != 0 {
        miette::bail!("chmod -R a-w {PLATFORM_DIR} failed with exit code {code}");
    }

    Ok(())
}
