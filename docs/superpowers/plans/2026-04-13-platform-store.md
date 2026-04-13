# /platform Store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Atomic, content-addressed file deployment to sandbox via `/platform/` directory with symlinks, clear separation of platform-managed vs agent-owned files.

**Architecture:** Platform-managed files are uploaded to `/platform/` with content-hash suffixes, then symlinked from their expected locations in `/sandbox/.claude/`. Agent-owned files (IDENTITY.md, SOUL.md, USER.md, AGENTS.md, TOOLS.md) live directly in `/sandbox/` and are never overwritten by sync. GC removes stale content-addressed files after each sync cycle.

**Tech Stack:** Rust, sha2 (new dep), walkdir, futures (buffer_unordered), OpenShell CLI (upload, exec)

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `crates/rightclaw/src/platform_store.rs` | Content hashing, upload-to-platform, symlink creation, GC, file manifest |
| Create | `crates/rightclaw/src/platform_store_tests.rs` | Unit tests for hashing + integration tests with real sandbox |
| Modify | `crates/rightclaw/src/lib.rs` | Add `pub mod platform_store;` |
| Modify | `crates/rightclaw/src/openshell.rs` | New `exec_command()` CLI helper for running commands inside sandbox |
| Modify | `crates/bot/src/sync.rs` | Rewrite sync_cycle to use platform store; move AGENTS.md/TOOLS.md to /sandbox/ root |
| Modify | `crates/bot/src/telegram/worker.rs:587-596,649-662` | Read AGENTS.md/TOOLS.md from `/sandbox/` and `agent_dir/` root |
| Modify | `crates/rightclaw/src/codegen/agent_def.rs:9-16` | Remove AGENTS.md, TOOLS.md from CONTENT_MD_FILES |
| Modify | `crates/rightclaw/src/codegen/policy.rs:78-90` | Add `/platform` as read_only |
| Modify | `crates/rightclaw/src/openshell.rs` staging | Update prepare_staging_dir for /platform/ layout |
| Modify | `crates/rightclaw/src/init.rs` | AGENTS.md, TOOLS.md to agent_dir root (already there) |
| Modify | `Cargo.toml` | Add `sha2` workspace dep |
| Modify | `crates/rightclaw/Cargo.toml` | Add `sha2` crate dep |

---

### Task 1: Add sha2 dependency

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/rightclaw/Cargo.toml`

- [ ] **Step 1: Add sha2 to workspace deps**

In `Cargo.toml` (workspace root), add to `[workspace.dependencies]`:

```toml
sha2 = "0.10"
```

In `crates/rightclaw/Cargo.toml`, add to `[dependencies]`:

```toml
sha2 = { workspace = true }
```

- [ ] **Step 2: Verify it compiles**

Run: `devenv shell -- cargo check -p rightclaw`
Expected: clean build

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml crates/rightclaw/Cargo.toml Cargo.lock
git commit -m "chore: add sha2 dependency for content-addressed platform store"
```

---

### Task 2: Content hashing and exec_command helper

**Files:**
- Create: `crates/rightclaw/src/platform_store.rs`
- Create: `crates/rightclaw/src/platform_store_tests.rs`
- Modify: `crates/rightclaw/src/lib.rs`
- Modify: `crates/rightclaw/src/openshell.rs`

- [ ] **Step 1: Write unit tests for content hashing**

Create `crates/rightclaw/src/platform_store_tests.rs`:

```rust
use super::*;
use tempfile::tempdir;

#[test]
fn content_hash_deterministic() {
    let h1 = content_hash(b"hello world");
    let h2 = content_hash(b"hello world");
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 8, "hash should be 8 hex chars");
}

#[test]
fn content_hash_differs_for_different_input() {
    let h1 = content_hash(b"hello");
    let h2 = content_hash(b"world");
    assert_ne!(h1, h2);
}

#[test]
fn directory_hash_deterministic() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("a.txt"), "aaa").unwrap();
    std::fs::write(dir.path().join("sub/b.txt"), "bbb").unwrap();

    let h1 = directory_hash(dir.path()).unwrap();
    let h2 = directory_hash(dir.path()).unwrap();
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 8);
}

#[test]
fn directory_hash_changes_on_content_change() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "version1").unwrap();
    let h1 = directory_hash(dir.path()).unwrap();

    std::fs::write(dir.path().join("a.txt"), "version2").unwrap();
    let h2 = directory_hash(dir.path()).unwrap();
    assert_ne!(h1, h2);
}

#[test]
fn directory_hash_includes_filenames() {
    // Same content but different filenames should produce different hashes
    let dir1 = tempdir().unwrap();
    std::fs::write(dir1.path().join("a.txt"), "content").unwrap();

    let dir2 = tempdir().unwrap();
    std::fs::write(dir2.path().join("b.txt"), "content").unwrap();

    let h1 = directory_hash(dir1.path()).unwrap();
    let h2 = directory_hash(dir2.path()).unwrap();
    assert_ne!(h1, h2);
}

#[test]
fn directory_hash_empty_dir() {
    let dir = tempdir().unwrap();
    let h = directory_hash(dir.path()).unwrap();
    assert_eq!(h.len(), 8, "empty dir should still produce a hash");
}

#[test]
fn platform_name_for_file() {
    assert_eq!(
        platform_path("settings.json", "abcd1234"),
        "settings.json.abcd1234"
    );
}

#[test]
fn platform_name_for_directory() {
    assert_eq!(
        platform_path("rightmcp", "abcd1234"),
        "rightmcp.abcd1234"
    );
}

#[test]
fn build_manifest_from_files() {
    let dir = tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(claude_dir.join("skills/rightmcp")).unwrap();
    std::fs::write(claude_dir.join("settings.json"), r#"{"key":"val"}"#).unwrap();
    std::fs::write(claude_dir.join("skills/rightmcp/SKILL.md"), "# skill").unwrap();
    std::fs::write(dir.path().join("mcp.json"), "{}").unwrap();

    let manifest = build_manifest(dir.path()).unwrap();
    // Should have at least settings.json and rightmcp skill
    assert!(manifest.files.iter().any(|e| e.name == "settings.json"));
    assert!(manifest.dirs.iter().any(|e| e.name == "rightmcp"));
}
```

- [ ] **Step 2: Write the platform_store module**

Create `crates/rightclaw/src/platform_store.rs`:

```rust
//! Content-addressed platform store for atomic sandbox file deployment.
//!
//! Platform-managed files are uploaded to `/platform/` with content-hash suffixes,
//! then symlinked from their expected locations in `/sandbox/.claude/`.
//! Agent-owned files live directly in `/sandbox/` and are never overwritten.

use sha2::{Digest, Sha256};
use std::path::Path;

#[cfg(test)]
#[path = "platform_store_tests.rs"]
mod tests;

/// 8-char hex hash of content bytes.
pub fn content_hash(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    format!("{:08x}", u32::from_be_bytes(hash[..4].try_into().unwrap()))
}

/// Hash of a directory's contents: sorted (relative_path + file_content) for each file.
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
    /// Logical name (e.g. "settings.json")
    pub name: String,
    /// Host path to the source file
    pub host_path: std::path::PathBuf,
    /// Content hash
    pub hash: String,
    /// Symlink path inside sandbox (e.g. "/sandbox/.claude/settings.json")
    pub link_path: String,
    /// Path prefix inside /platform/ (e.g. "" for root, "agents/" for subdirs)
    pub platform_prefix: String,
}

/// A directory to deploy to /platform/.
pub struct DirEntry {
    /// Logical name (e.g. "rightmcp")
    pub name: String,
    /// Host path to the source directory
    pub host_path: std::path::PathBuf,
    /// Content hash of directory
    pub hash: String,
    /// Symlink path inside sandbox (e.g. "/sandbox/.claude/skills/rightmcp")
    pub link_path: String,
    /// Path prefix inside /platform/ (e.g. "skills/")
    pub platform_prefix: String,
}

/// Complete manifest of files and directories to deploy.
pub struct Manifest {
    pub files: Vec<FileEntry>,
    pub dirs: Vec<DirEntry>,
}

/// Scan the agent directory and build a manifest of platform-managed files.
///
/// Only includes files that exist on disk. Agent-owned files (IDENTITY.md,
/// SOUL.md, USER.md, AGENTS.md, TOOLS.md) are NOT included.
pub fn build_manifest(agent_dir: &Path) -> miette::Result<Manifest> {
    let claude_dir = agent_dir.join(".claude");
    let mut files = Vec::new();
    let mut dirs = Vec::new();

    // Individual files in .claude/
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

    // Agent def files in .claude/agents/ (platform-managed, NOT agent-owned AGENTS.md/TOOLS.md)
    let agents_dir = claude_dir.join("agents");
    if agents_dir.exists() {
        for entry in std::fs::read_dir(&agents_dir)
            .map_err(|e| miette::miette!("read agents dir: {e:#}"))?
        {
            let entry = entry.map_err(|e| miette::miette!("readdir: {e:#}"))?;
            let name_os = entry.file_name();
            let name = name_os.to_string_lossy();
            // Skip AGENTS.md and TOOLS.md — they are agent-owned
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

    // mcp.json (at agent root, not inside .claude/)
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
```

- [ ] **Step 3: Add exec_command helper to openshell.rs**

In `crates/rightclaw/src/openshell.rs`, add after `upload_file`:

```rust
/// Execute a command inside a sandbox via CLI. Returns (stdout, exit_code).
pub async fn exec_command(sandbox: &str, cmd: &[&str]) -> miette::Result<(String, i32)> {
    let mut command = Command::new("openshell");
    command.args(["sandbox", "exec", sandbox, "--"]);
    command.args(cmd);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let output = command
        .output()
        .await
        .map_err(|e| miette::miette!("openshell exec failed: {e:#}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let code = output.status.code().unwrap_or(-1);
    Ok((stdout, code))
}
```

- [ ] **Step 4: Add module to lib.rs**

In `crates/rightclaw/src/lib.rs`, add:

```rust
pub mod platform_store;
```

- [ ] **Step 5: Run tests**

Run: `devenv shell -- cargo test -p rightclaw platform_store`
Expected: all 9 unit tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/platform_store.rs crates/rightclaw/src/platform_store_tests.rs crates/rightclaw/src/lib.rs crates/rightclaw/src/openshell.rs
git commit -m "feat: add platform_store module with content hashing and manifest builder"
```

---

### Task 3: Platform deploy functions (upload + symlink + GC)

**Files:**
- Modify: `crates/rightclaw/src/platform_store.rs`
- Modify: `crates/rightclaw/src/platform_store_tests.rs`

- [ ] **Step 1: Write integration test for deploy cycle**

Add to `crates/rightclaw/src/platform_store_tests.rs`:

```rust
use serial_test::serial;

#[tokio::test]
#[serial]
#[ignore = "requires live OpenShell sandbox"]
async fn deploy_file_creates_platform_entry_and_symlink() {
    let sbox = crate::openshell::tests::TestSandbox::create("platform-deploy").await;

    // Deploy a file
    deploy_file(
        sbox.name(),
        "test.json",
        b"{}",
        "/sandbox/.claude/test.json",
        "",
    )
    .await
    .expect("deploy_file should succeed");

    // Verify: file exists in /platform/
    let (out, code) = sbox.exec(&["ls", "/platform/"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("test.json."), "should have content-addressed file");

    // Verify: symlink exists and resolves
    let (content, code) = sbox.exec(&["cat", "/sandbox/.claude/test.json"]).await;
    assert_eq!(code, 0);
    assert_eq!(content, "{}");

    // Verify: symlink target
    let (target, code) = sbox.exec(&["readlink", "/sandbox/.claude/test.json"]).await;
    assert_eq!(code, 0);
    assert!(target.trim().starts_with("/platform/"), "symlink must point to /platform/");

    sbox.destroy().await;
}

#[tokio::test]
#[serial]
#[ignore = "requires live OpenShell sandbox"]
async fn deploy_file_overwrites_cleanly() {
    let sbox = crate::openshell::tests::TestSandbox::create("platform-overwrite").await;

    // First deploy
    deploy_file(
        sbox.name(),
        "data.json",
        b"version1",
        "/sandbox/.claude/data.json",
        "",
    )
    .await
    .unwrap();

    let (content, _) = sbox.exec(&["cat", "/sandbox/.claude/data.json"]).await;
    assert_eq!(content, "version1");

    // Second deploy with different content
    deploy_file(
        sbox.name(),
        "data.json",
        b"version2",
        "/sandbox/.claude/data.json",
        "",
    )
    .await
    .unwrap();

    let (content, _) = sbox.exec(&["cat", "/sandbox/.claude/data.json"]).await;
    assert_eq!(content, "version2");

    // Old version should still exist until GC
    let (ls, _) = sbox.exec(&["ls", "/platform/"]).await;
    let json_files: Vec<&str> = ls.lines().filter(|l| l.starts_with("data.json.")).collect();
    assert_eq!(json_files.len(), 2, "both versions should exist before GC");

    sbox.destroy().await;
}

#[tokio::test]
#[serial]
#[ignore = "requires live OpenShell sandbox"]
async fn deploy_directory_and_symlink() {
    let sbox = crate::openshell::tests::TestSandbox::create("platform-dir").await;

    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().join("testskill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# test skill content").unwrap();

    deploy_directory(
        sbox.name(),
        "testskill",
        &skill_dir,
        "/sandbox/.claude/skills/testskill",
        "skills/",
    )
    .await
    .expect("deploy_directory should succeed");

    // Verify via symlink
    let (content, code) = sbox
        .exec(&["cat", "/sandbox/.claude/skills/testskill/SKILL.md"])
        .await;
    assert_eq!(code, 0);
    assert_eq!(content, "# test skill content");

    // Verify symlink target
    let (target, _) = sbox
        .exec(&["readlink", "/sandbox/.claude/skills/testskill"])
        .await;
    assert!(target.trim().starts_with("/platform/skills/testskill."));

    sbox.destroy().await;
}

#[tokio::test]
#[serial]
#[ignore = "requires live OpenShell sandbox"]
async fn gc_removes_stale_files() {
    let sbox = crate::openshell::tests::TestSandbox::create("platform-gc").await;

    // Deploy v1
    deploy_file(sbox.name(), "f.txt", b"v1", "/sandbox/f.txt", "").await.unwrap();

    // Deploy v2 (v1 still on disk)
    deploy_file(sbox.name(), "f.txt", b"v2", "/sandbox/f.txt", "").await.unwrap();

    let (ls_before, _) = sbox.exec(&["find", "/platform", "-type", "f"]).await;
    let count_before = ls_before.lines().count();
    assert_eq!(count_before, 2, "both versions before GC");

    // Collect active targets
    let targets = vec![
        format!("/platform/{}", platform_path("f.txt", &content_hash(b"v2")))
    ];
    gc_platform(sbox.name(), &targets).await.unwrap();

    let (ls_after, _) = sbox.exec(&["find", "/platform", "-type", "f"]).await;
    let count_after = ls_after.lines().count();
    assert_eq!(count_after, 1, "only current version after GC");

    // Symlink still works
    let (content, code) = sbox.exec(&["cat", "/sandbox/f.txt"]).await;
    assert_eq!(code, 0);
    assert_eq!(content, "v2");

    sbox.destroy().await;
}

#[tokio::test]
#[serial]
#[ignore = "requires live OpenShell sandbox"]
async fn deploy_skips_upload_when_hash_matches() {
    let sbox = crate::openshell::tests::TestSandbox::create("platform-dedup").await;

    // Deploy same content twice
    deploy_file(sbox.name(), "dup.txt", b"same", "/sandbox/dup.txt", "").await.unwrap();
    deploy_file(sbox.name(), "dup.txt", b"same", "/sandbox/dup.txt", "").await.unwrap();

    // Should only have one file (same hash = same path, no duplicate)
    let (ls, _) = sbox.exec(&["find", "/platform", "-type", "f"]).await;
    let count = ls.lines().filter(|l| l.contains("dup.txt.")).count();
    assert_eq!(count, 1, "identical content should produce single entry");

    sbox.destroy().await;
}
```

- [ ] **Step 2: Implement deploy functions**

Add to `crates/rightclaw/src/platform_store.rs`:

```rust
use futures::stream::{self, StreamExt};

/// Base path for platform store inside sandbox.
pub const PLATFORM_DIR: &str = "/platform";

/// Deploy a single file to /platform/ with content-addressed name, create symlink.
///
/// If a file with the same hash already exists in /platform/, skips upload (dedup).
/// Symlink is always updated (atomic swap via tmp link + mv).
pub async fn deploy_file(
    sandbox: &str,
    name: &str,
    content: &[u8],
    link_path: &str,
    platform_prefix: &str,
) -> miette::Result<()> {
    let hash = content_hash(content);
    let platform_name = platform_path(name, &hash);
    let full_platform_path = if platform_prefix.is_empty() {
        format!("{PLATFORM_DIR}/{platform_name}")
    } else {
        format!("{PLATFORM_DIR}/{platform_prefix}{platform_name}")
    };

    // Check if already exists (dedup)
    let (_, code) = crate::openshell::exec_command(
        sandbox,
        &["test", "-e", &full_platform_path],
    )
    .await?;

    if code != 0 {
        // Upload: write content to temp file, upload, move into place
        let tmp = tempfile::NamedTempFile::new()
            .map_err(|e| miette::miette!("tempfile: {e:#}"))?;
        std::fs::write(tmp.path(), content)
            .map_err(|e| miette::miette!("write temp: {e:#}"))?;

        // Ensure parent directory exists
        let parent = std::path::Path::new(&full_platform_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if !parent.is_empty() {
            crate::openshell::exec_command(sandbox, &["mkdir", "-p", &parent]).await?;
        }

        crate::openshell::upload_file(sandbox, tmp.path(), &format!("{parent}/"))
            .await
            .map_err(|e| miette::miette!("upload {name}: {e:#}"))?;

        // Rename from temp filename to content-addressed name
        let uploaded_name = tmp.path().file_name().unwrap().to_string_lossy();
        let uploaded_path = format!("{parent}/{uploaded_name}");
        crate::openshell::exec_command(
            sandbox,
            &["mv", &uploaded_path, &full_platform_path],
        )
        .await?;
    }

    // Create/update symlink (atomic: create tmp link, then mv over target)
    let link_parent = std::path::Path::new(link_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    if !link_parent.is_empty() {
        crate::openshell::exec_command(sandbox, &["mkdir", "-p", &link_parent]).await?;
    }

    let tmp_link = format!("/tmp/rightclaw-link-{}", name.replace('/', "-"));
    crate::openshell::exec_command(
        sandbox,
        &["ln", "-sf", &full_platform_path, &tmp_link],
    )
    .await?;
    crate::openshell::exec_command(
        sandbox,
        &["mv", "-fT", &tmp_link, link_path],
    )
    .await?;

    Ok(())
}

/// Deploy a directory to /platform/ with content-addressed name, create symlink.
pub async fn deploy_directory(
    sandbox: &str,
    name: &str,
    host_dir: &Path,
    link_path: &str,
    platform_prefix: &str,
) -> miette::Result<()> {
    let hash = directory_hash(host_dir)?;
    let platform_name = platform_path(name, &hash);
    let full_platform_path = if platform_prefix.is_empty() {
        format!("{PLATFORM_DIR}/{platform_name}")
    } else {
        format!("{PLATFORM_DIR}/{platform_prefix}{platform_name}")
    };

    // Check if already exists (dedup)
    let (_, code) = crate::openshell::exec_command(
        sandbox,
        &["test", "-d", &full_platform_path],
    )
    .await?;

    if code != 0 {
        // Ensure parent dir
        let parent = std::path::Path::new(&full_platform_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if !parent.is_empty() {
            crate::openshell::exec_command(sandbox, &["mkdir", "-p", &parent]).await?;
        }

        // Upload all files in the directory individually (parallel)
        let mut uploads = Vec::new();
        for entry in walkdir::WalkDir::new(host_dir) {
            let entry = entry.map_err(|e| miette::miette!("walkdir: {e:#}"))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(host_dir)
                .map_err(|e| miette::miette!("strip_prefix: {e:#}"))?;
            let dest_dir = std::path::Path::new(&full_platform_path)
                .join(rel.parent().unwrap_or(std::path::Path::new("")));
            let dest = format!("{}/", dest_dir.display());
            uploads.push((entry.path().to_path_buf(), dest));
        }

        let results: Vec<miette::Result<()>> = stream::iter(
            uploads.into_iter().map(|(path, dest)| {
                let sandbox = sandbox.to_owned();
                async move {
                    // Ensure dest dir exists
                    crate::openshell::exec_command(&sandbox, &["mkdir", "-p", dest.trim_end_matches('/')]).await?;
                    crate::openshell::upload_file(&sandbox, &path, &dest).await
                }
            }),
        )
        .buffer_unordered(10)
        .collect()
        .await;

        for r in results {
            r?;
        }
    }

    // Create/update symlink (atomic)
    let link_parent = std::path::Path::new(link_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    if !link_parent.is_empty() {
        crate::openshell::exec_command(sandbox, &["mkdir", "-p", &link_parent]).await?;
    }

    let tmp_link = format!("/tmp/rightclaw-link-{}", name);
    crate::openshell::exec_command(
        sandbox,
        &["ln", "-sfn", &full_platform_path, &tmp_link],
    )
    .await?;
    crate::openshell::exec_command(
        sandbox,
        &["mv", "-fT", &tmp_link, link_path],
    )
    .await?;

    Ok(())
}

/// Remove stale files from /platform/ that are not in the active targets set.
///
/// Best-effort: logs errors but does not fail.
pub async fn gc_platform(sandbox: &str, active_targets: &[String]) -> miette::Result<()> {
    let (listing, code) = crate::openshell::exec_command(
        sandbox,
        &["find", PLATFORM_DIR, "-type", "f", "-o", "-type", "d", "-mindepth", "1"],
    )
    .await?;

    if code != 0 {
        tracing::warn!("gc: failed to list /platform/ contents");
        return Ok(());
    }

    // Collect all paths to keep: active targets + their parent dirs
    let mut keep: std::collections::HashSet<String> = std::collections::HashSet::new();
    for target in active_targets {
        keep.insert(target.clone());
        // Keep parent dirs
        let mut p = std::path::Path::new(target);
        while let Some(parent) = p.parent() {
            let s = parent.to_string_lossy().to_string();
            if s == PLATFORM_DIR || s == "/" || s.is_empty() {
                break;
            }
            keep.insert(s);
            p = parent;
        }
    }

    // Find stale entries (files and leaf directories)
    let mut stale: Vec<String> = Vec::new();
    for line in listing.lines() {
        let path = line.trim();
        if path.is_empty() || path == PLATFORM_DIR {
            continue;
        }
        if !keep.contains(path) {
            stale.push(path.to_owned());
        }
    }

    if !stale.is_empty() {
        let mut rm_args = vec!["rm", "-rf"];
        let stale_refs: Vec<&str> = stale.iter().map(|s| s.as_str()).collect();
        rm_args.extend(stale_refs);
        let (_, code) = crate::openshell::exec_command(sandbox, &rm_args).await?;
        if code != 0 {
            tracing::warn!("gc: rm failed for some stale entries");
        } else {
            tracing::debug!(count = stale.len(), "gc: removed stale platform entries");
        }
    }

    Ok(())
}

/// Run a complete deploy cycle: upload manifest to /platform/, create symlinks, GC.
pub async fn deploy_manifest(sandbox: &str, manifest: &Manifest) -> miette::Result<()> {
    // Ensure /platform/ exists
    crate::openshell::exec_command(sandbox, &["mkdir", "-p", PLATFORM_DIR]).await?;

    let mut active_targets: Vec<String> = Vec::new();

    // Deploy files
    for entry in &manifest.files {
        let content = std::fs::read(&entry.host_path)
            .map_err(|e| miette::miette!("read {}: {e:#}", entry.name))?;
        deploy_file(
            sandbox,
            &entry.name,
            &content,
            &entry.link_path,
            &entry.platform_prefix,
        )
        .await?;
        let platform_name = platform_path(&entry.name, &entry.hash);
        let target = if entry.platform_prefix.is_empty() {
            format!("{PLATFORM_DIR}/{platform_name}")
        } else {
            format!("{PLATFORM_DIR}/{}/{platform_name}", entry.platform_prefix.trim_end_matches('/'))
        };
        active_targets.push(target);
    }

    // Deploy directories
    for entry in &manifest.dirs {
        deploy_directory(
            sandbox,
            &entry.name,
            &entry.host_path,
            &entry.link_path,
            &entry.platform_prefix,
        )
        .await?;
        let platform_name = platform_path(&entry.name, &entry.hash);
        let target = if entry.platform_prefix.is_empty() {
            format!("{PLATFORM_DIR}/{platform_name}")
        } else {
            format!("{PLATFORM_DIR}/{}/{platform_name}", entry.platform_prefix.trim_end_matches('/'))
        };
        active_targets.push(target);
    }

    // GC stale entries
    gc_platform(sandbox, &active_targets).await?;

    // Make /platform/ read-only
    crate::openshell::exec_command(sandbox, &["chmod", "-R", "a-w", PLATFORM_DIR]).await?;

    Ok(())
}
```

- [ ] **Step 3: Run unit tests**

Run: `devenv shell -- cargo test -p rightclaw platform_store -- --skip ignore`
Expected: all unit tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw/src/platform_store.rs crates/rightclaw/src/platform_store_tests.rs
git commit -m "feat: add platform deploy functions (upload, symlink, GC)"
```

---

### Task 4: Add /platform to OpenShell policy

**Files:**
- Modify: `crates/rightclaw/src/codegen/policy.rs`

- [ ] **Step 1: Add /platform to read_only list**

In `crates/rightclaw/src/codegen/policy.rs`, in the `read_write` section (around line 88-90), add `/platform` to `read_only` and keep `/sandbox` in `read_write`:

Change:

```rust
  read_only:
    - /usr
    - /lib
    - /lib64
    - /etc
    - /proc
    - /dev/urandom
    - /dev/null
  read_write:
    - /tmp
    - /sandbox
```

To:

```rust
  read_only:
    - /usr
    - /lib
    - /lib64
    - /etc
    - /proc
    - /dev/urandom
    - /dev/null
    - /platform
  read_write:
    - /tmp
    - /sandbox
```

Note: `/platform` must be read_write initially (for sync uploads), then sync makes it read-only via `chmod`. But OpenShell policy applies to the sandbox user, and uploads happen from outside via CLI. So `read_only` in policy is correct — the agent process inside the sandbox can only read, while `openshell sandbox upload` and `openshell sandbox exec` bypass the policy.

Actually — check this. `exec_command` runs commands AS the sandbox user. If `/platform` is read_only in policy, `mkdir -p /platform` from exec_command will fail. We need `/platform` to be read_write in the policy, and enforce read-only via `chmod` after deployment.

Change `/platform` to `read_write` instead:

```rust
  read_write:
    - /tmp
    - /sandbox
    - /platform
```

- [ ] **Step 2: Update existing policy test**

Run: `devenv shell -- cargo test -p rightclaw policy`
Expected: if tests check exact policy output, update them to include `/platform`

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/policy.rs
git commit -m "feat: add /platform to sandbox filesystem policy"
```

---

### Task 5: Move AGENTS.md and TOOLS.md to agent root

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs:9-16`
- Modify: `crates/bot/src/telegram/worker.rs:587-596,649-662`
- Modify: `crates/bot/src/sync.rs:142-148`

- [ ] **Step 1: Remove AGENTS.md, TOOLS.md from CONTENT_MD_FILES**

In `crates/rightclaw/src/codegen/agent_def.rs`, change `CONTENT_MD_FILES` (line 9-16):

```rust
pub const CONTENT_MD_FILES: &[&str] = &[
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
];
```

Remove `"AGENTS.md"` and `"TOOLS.md"` — they are no longer synced to `.claude/agents/`, they live at `/sandbox/` root.

- [ ] **Step 2: Update sandbox prompt assembly**

In `crates/bot/src/telegram/worker.rs`, change the AGENTS.md and TOOLS.md sections in the sandbox shell script (lines 587-596). Change:

```rust
if [ -f /sandbox/.claude/agents/AGENTS.md ]; then
  printf '\n## Agent Configuration\n'
  cat /sandbox/.claude/agents/AGENTS.md
  printf '\n'
fi
if [ -f /sandbox/.claude/agents/TOOLS.md ]; then
  printf '\n## Environment and Tools\n'
  cat /sandbox/.claude/agents/TOOLS.md
  printf '\n'
fi
```

To:

```rust
if [ -f /sandbox/AGENTS.md ]; then
  printf '\n## Agent Configuration\n'
  cat /sandbox/AGENTS.md
  printf '\n'
fi
if [ -f /sandbox/TOOLS.md ]; then
  printf '\n## Environment and Tools\n'
  cat /sandbox/TOOLS.md
  printf '\n'
fi
```

- [ ] **Step 3: Update host prompt assembly**

In `crates/bot/src/telegram/worker.rs`, change the host-side AGENTS.md and TOOLS.md reads (lines 649-662). Change:

```rust
let agents_path = agent_dir.join(".claude").join("agents").join("AGENTS.md");
```

To:

```rust
let agents_path = agent_dir.join("AGENTS.md");
```

And change:

```rust
let tools_path = agent_dir.join(".claude").join("agents").join("TOOLS.md");
```

To:

```rust
let tools_path = agent_dir.join("TOOLS.md");
```

- [ ] **Step 4: Update REVERSE_SYNC_FILES to include AGENTS.md**

In `crates/bot/src/sync.rs`, add `"AGENTS.md"` to `REVERSE_SYNC_FILES` (since the agent can now edit it in `/sandbox/`):

```rust
const REVERSE_SYNC_FILES: &[&str] = &[
    "AGENTS.md",
    "TOOLS.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "MEMORY.md",
];
```

- [ ] **Step 5: Run existing tests, fix assertions**

Run: `devenv shell -- cargo test --workspace --lib`
Expected: some tests will fail due to changed paths. Fix assertion strings:
- Tests checking `script.contains("cat /sandbox/.claude/agents/AGENTS.md")` → change to `cat /sandbox/AGENTS.md`
- Tests checking `agents_path` → adjust for new location

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs crates/bot/src/telegram/worker.rs crates/bot/src/sync.rs
git commit -m "refactor: move AGENTS.md and TOOLS.md to agent root, out of .claude/agents/"
```

---

### Task 6: Rewrite sync_cycle to use platform store

**Files:**
- Modify: `crates/bot/src/sync.rs`

- [ ] **Step 1: Rewrite sync_cycle**

Replace the entire `sync_cycle` function in `crates/bot/src/sync.rs`:

```rust
async fn sync_cycle(agent_dir: &Path, sandbox: &str) -> miette::Result<()> {
    // Build manifest of platform-managed files
    let manifest = rightclaw::platform_store::build_manifest(agent_dir)?;

    // Deploy to /platform/ with content-addressed names + symlinks
    rightclaw::platform_store::deploy_manifest(sandbox, &manifest).await?;

    // Verify .claude.json (separate flow — not content-addressed)
    verify_claude_json(agent_dir, sandbox).await?;

    tracing::debug!("sync: cycle complete");
    Ok(())
}
```

- [ ] **Step 2: Rewrite initial_sync**

Update `initial_sync` to handle agent-owned files:

```rust
pub async fn initial_sync(agent_dir: &Path, sandbox_name: &str) -> miette::Result<()> {
    tracing::info!(sandbox = sandbox_name, "sync: initial cycle (blocking)");

    // Deploy platform-managed files
    sync_cycle(agent_dir, sandbox_name).await?;

    // Upload agent-owned files ONLY if they don't exist in sandbox.
    // These are writable by the agent — never overwrite their edits.
    let agent_owned: &[(&str, &str)] = &[
        ("AGENTS.md", "/sandbox/AGENTS.md"),
        ("TOOLS.md", "/sandbox/TOOLS.md"),
    ];

    for &(filename, sandbox_path) in agent_owned {
        let host_path = agent_dir.join(filename);
        if !host_path.exists() {
            continue;
        }
        // Check if already exists in sandbox
        let (_, code) = rightclaw::openshell::exec_command(
            sandbox_name,
            &["test", "-f", sandbox_path],
        )
        .await?;
        if code != 0 {
            // Does not exist — upload initial version
            let parent = std::path::Path::new(sandbox_path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let dest = format!("{parent}/");
            rightclaw::openshell::upload_file(sandbox_name, &host_path, &dest)
                .await
                .map_err(|e| miette::miette!("initial sync {filename}: {e:#}"))?;
            tracing::info!(file = filename, "sync: uploaded agent-owned file (first time)");
        }
    }

    // Upload content .md files (IDENTITY, SOUL, USER, MEMORY) into /sandbox/
    // Only on startup — sandbox is source of truth after this point.
    for &filename in rightclaw::codegen::CONTENT_MD_FILES {
        let host_path = agent_dir.join(filename);
        if host_path.exists() {
            rightclaw::openshell::upload_file(sandbox_name, &host_path, "/sandbox/")
                .await
                .map_err(|e| miette::miette!("sync {filename}: {e:#}"))?;
            tracing::debug!(file = filename, "sync: uploaded content file");
        }
    }

    Ok(())
}
```

- [ ] **Step 3: Remove old imports**

Remove `use rightclaw::codegen::CONTENT_MD_FILES;` from the top of sync.rs if it's no longer used in initial_sync... actually it IS used in the content .md loop. Keep it.

Wait — CONTENT_MD_FILES is now only `["IDENTITY.md", "SOUL.md", "USER.md", "MEMORY.md"]`. The initial_sync loop uploads these to `/sandbox/`. But these are synced only on initial startup, then reverse-synced back. That's the existing behavior — correct.

- [ ] **Step 4: Run tests**

Run: `devenv shell -- cargo test --workspace --lib`
Expected: all unit tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/sync.rs
git commit -m "feat: rewrite sync_cycle to use platform store with content-addressed deployment"
```

---

### Task 7: Update staging for new sandbox creation

**Files:**
- Modify: `crates/rightclaw/src/openshell.rs` (prepare_staging_dir)

- [ ] **Step 1: Update prepare_staging_dir**

The staging dir is used when creating a NEW sandbox. It needs to include `/platform/` layout. But staging is uploaded as-is to `/sandbox/` root by OpenShell. We can't easily create `/platform/` in staging because OpenShell uploads everything under the sandbox workdir.

Instead, after sandbox creation (in the bot), we run `initial_sync` which does the platform deploy. So staging only needs the minimal files for CC to start — `.claude.json`, `.claude/settings.json`.

Simplify `prepare_staging_dir` to only include `.claude.json` and the minimal CC bootstrap files. Remove skills and agents from staging — initial_sync handles them via platform store.

In `crates/rightclaw/src/openshell.rs`, update `prepare_staging_dir`:

```rust
pub fn prepare_staging_dir(agent_dir: &Path, upload_dir: &Path) -> miette::Result<()> {
    let staging_claude = upload_dir.join(".claude");
    if staging_claude.exists() {
        std::fs::remove_dir_all(&staging_claude)
            .map_err(|e| miette::miette!("failed to clean staging/.claude: {e:#}"))?;
    }
    std::fs::create_dir_all(&staging_claude)
        .map_err(|e| miette::miette!("failed to create staging/.claude: {e:#}"))?;

    let src_claude = agent_dir.join(".claude");

    // Only copy minimal files needed for CC to start.
    // Platform-managed files (settings, schemas, skills, agent defs) are deployed
    // via /platform/ store during initial_sync — not in staging.
    let copy_items: &[&str] = &[
        "settings.json",
        "reply-schema.json",
    ];

    for item in copy_items {
        let src = src_claude.join(item);
        let dst = staging_claude.join(item);
        if src.exists() {
            std::fs::copy(&src, &dst)
                .map_err(|e| miette::miette!("failed to copy {} to staging: {e:#}", item))?;
        }
    }

    // Copy .claude.json (trust/onboarding — at agent root, not inside .claude/)
    let claude_json_src = agent_dir.join(".claude.json");
    let claude_json_dst = upload_dir.join(".claude.json");
    if claude_json_src.exists() {
        std::fs::copy(&claude_json_src, &claude_json_dst)
            .map_err(|e| miette::miette!("failed to copy .claude.json to staging: {e:#}"))?;
    }

    // Copy mcp.json (needed for --mcp-config flag at startup)
    let mcp_json_src = agent_dir.join("mcp.json");
    let mcp_json_dst = upload_dir.join("mcp.json");
    if mcp_json_src.exists() {
        std::fs::copy(&mcp_json_src, &mcp_json_dst)
            .map_err(|e| miette::miette!("failed to copy mcp.json to staging: {e:#}"))?;
    }

    tracing::info!("prepared staging dir for sandbox upload");
    Ok(())
}
```

- [ ] **Step 2: Run tests, fix staging-related assertions**

Run: `devenv shell -- cargo test --workspace --lib`
Expected: fix tests that check staging contents (e.g. tests expecting skills in staging)

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/openshell.rs
git commit -m "refactor: simplify staging dir, platform files deployed via /platform/ store"
```

---

### Task 8: Update documentation

**Files:**
- Modify: `PROMPT_SYSTEM.md`
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update PROMPT_SYSTEM.md**

Update the file locations table and sync description:
- AGENTS.md: `/sandbox/AGENTS.md` (was `/sandbox/.claude/agents/AGENTS.md`)
- TOOLS.md: `/sandbox/TOOLS.md` (was `/sandbox/.claude/agents/TOOLS.md`)
- Add `/platform/` store description

- [ ] **Step 2: Update ARCHITECTURE.md**

Update:
- Directory layout to show `/platform/` in sandbox
- Sync flow description
- Data flow section

- [ ] **Step 3: Commit**

```bash
git add PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "docs: update architecture for /platform store and agent-owned file locations"
```

---

### Task 9: Full build, all tests, end-to-end verification

**Files:** (none — verification only)

- [ ] **Step 1: Run all unit tests**

Run: `devenv shell -- cargo test --workspace --lib`
Expected: all tests pass

- [ ] **Step 2: Full workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: clean build

- [ ] **Step 3: Verify binary content**

Run: `devenv shell -- rg "/platform" target/debug/rightclaw`
Expected: binary file matches

- [ ] **Step 4: Run integration tests (if sandbox available)**

Run: `devenv shell -- cargo test -p rightclaw platform_store -- --ignored --nocapture`
Expected: all integration tests pass (deploy, overwrite, symlink, GC, dedup)

- [ ] **Step 5: Migration test on live sandbox**

Manual verification:
1. `devenv shell -- cargo build --workspace`
2. Restart bot (rightclaw down + up)
3. Check sandbox: `ssh ... "ls -la /platform/"` — should have content-addressed files
4. Check sandbox: `ssh ... "readlink /sandbox/.claude/settings.json"` — should point to /platform/
5. Check sandbox: `ssh ... "cat /sandbox/AGENTS.md"` — agent-owned file in root
6. Check sandbox: `ssh ... "cat /sandbox/.claude/skills/rightmcp/SKILL.md"` — via symlink
7. Ask bot to add Composio MCP — should activate /rightmcp skill

- [ ] **Step 6: Commit any fixups**

Only if previous steps required fixes.
