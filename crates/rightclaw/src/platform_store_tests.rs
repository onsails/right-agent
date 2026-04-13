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
    assert_eq!(h.len(), 8);
}

#[test]
fn platform_name_for_file() {
    assert_eq!(platform_path("settings.json", "abcd1234"), "settings.json.abcd1234");
}

#[test]
fn platform_name_for_directory() {
    assert_eq!(platform_path("rightmcp", "abcd1234"), "rightmcp.abcd1234");
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
    assert!(manifest.files.iter().any(|e| e.name == "settings.json"));
    assert!(manifest.files.iter().any(|e| e.name == "mcp.json"));
    assert!(manifest.dirs.iter().any(|e| e.name == "rightmcp"));
}

#[test]
fn build_manifest_skips_agent_owned_files() {
    let dir = tempdir().unwrap();
    let agents_dir = dir.path().join(".claude/agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(agents_dir.join("AGENTS.md"), "# agents").unwrap();
    std::fs::write(agents_dir.join("TOOLS.md"), "# tools").unwrap();
    std::fs::write(agents_dir.join("right.md"), "---\nname: right\n---").unwrap();
    let manifest = build_manifest(dir.path()).unwrap();
    // right.md should be included, AGENTS.md and TOOLS.md should not
    assert!(manifest.files.iter().any(|e| e.name == "right.md"));
    assert!(!manifest.files.iter().any(|e| e.name == "AGENTS.md"));
    assert!(!manifest.files.iter().any(|e| e.name == "TOOLS.md"));
}
