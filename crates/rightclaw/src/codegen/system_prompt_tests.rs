use std::path::PathBuf;

use crate::agent::AgentDef;
use crate::codegen::generate_system_prompt;

/// Build a minimal AgentDef pointing at `path` as agent dir.
/// Optional files are set based on flags — paths are set but files only exist if you create them.
fn make_agent_at(
    path: PathBuf,
    with_soul: bool,
    with_user: bool,
    with_agents: bool,
) -> AgentDef {
    AgentDef {
        name: "testbot".to_owned(),
        path: path.clone(),
        identity_path: path.join("IDENTITY.md"),
        config: None,
        soul_path: if with_soul { Some(path.join("SOUL.md")) } else { None },
        user_path: if with_user { Some(path.join("USER.md")) } else { None },
        agents_path: if with_agents { Some(path.join("AGENTS.md")) } else { None },
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    }
}

fn write_file(dir: &std::path::Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

#[test]
fn identity_only_no_separator() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "# Identity Content\n");

    let agent = make_agent_at(tmp.path().to_path_buf(), false, false, false);
    let result = generate_system_prompt(&agent).unwrap();

    assert!(result.contains("# Identity Content"), "expected identity text");
    assert!(
        !result.contains("---"),
        "no separator expected when only IDENTITY.md present"
    );
}

#[test]
fn identity_and_soul_joined_by_separator() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");
    write_file(tmp.path(), "SOUL.md", "soul-text");

    let agent = make_agent_at(tmp.path().to_path_buf(), true, false, false);
    let result = generate_system_prompt(&agent).unwrap();

    assert_eq!(result, "identity-text\n\n---\n\nsoul-text");
}

#[test]
fn all_four_files_in_canonical_order() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "A");
    write_file(tmp.path(), "SOUL.md", "B");
    write_file(tmp.path(), "USER.md", "C");
    write_file(tmp.path(), "AGENTS.md", "D");

    let agent = make_agent_at(tmp.path().to_path_buf(), true, true, true);
    let result = generate_system_prompt(&agent).unwrap();

    assert_eq!(result, "A\n\n---\n\nB\n\n---\n\nC\n\n---\n\nD");
}

#[test]
fn absent_optional_paths_are_silently_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-only");
    // soul_path = None, user_path = None, agents_path = None
    let agent = make_agent_at(tmp.path().to_path_buf(), false, false, false);
    let result = generate_system_prompt(&agent).unwrap();

    assert_eq!(result, "identity-only");
}

#[test]
fn missing_identity_returns_err_with_path() {
    let tmp = tempfile::tempdir().unwrap();
    // Intentionally do NOT create IDENTITY.md.
    let agent = make_agent_at(tmp.path().to_path_buf(), false, false, false);
    let result = generate_system_prompt(&agent);

    assert!(result.is_err(), "expected Err when IDENTITY.md is missing");
    let err = format!("{:?}", result.unwrap_err());
    assert!(
        err.contains("IDENTITY.md"),
        "error should mention IDENTITY.md path, got: {err}"
    );
}

#[test]
fn soul_path_set_but_file_deleted_is_silently_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");
    // Create SOUL.md then delete it — path is Some but file does not exist.
    write_file(tmp.path(), "SOUL.md", "temp");
    std::fs::remove_file(tmp.path().join("SOUL.md")).unwrap();

    let agent = make_agent_at(tmp.path().to_path_buf(), true, false, false);
    let result = generate_system_prompt(&agent).unwrap();

    assert_eq!(result, "identity-text");
}

#[test]
fn no_hardcoded_startup_sections_in_output() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "IDENTITY.md", "identity-text");
    write_file(tmp.path(), "SOUL.md", "soul-text");

    let agent = make_agent_at(tmp.path().to_path_buf(), true, false, false);
    let result = generate_system_prompt(&agent).unwrap();

    assert!(
        !result.contains("Startup Instructions"),
        "output must not contain hardcoded 'Startup Instructions'"
    );
    assert!(
        !result.contains("rightcron"),
        "output must not contain hardcoded 'rightcron'"
    );
    assert!(
        !result.contains("Communication"),
        "output must not contain hardcoded 'Communication' section"
    );
    assert!(
        !result.contains("BOOTSTRAP"),
        "output must not contain hardcoded 'BOOTSTRAP'"
    );
}
