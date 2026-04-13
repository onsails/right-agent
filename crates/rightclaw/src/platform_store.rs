//! Content-addressed platform store for atomic sandbox file deployment.

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
