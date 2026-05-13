//! Integration tests confirming that Claude Code produces the per-session
//! files that the /rightreflect skill depends on.
//!
//! Tests 14-16 of the self-introspection implementation plan.
//!
//! All tests run against a live OpenShell sandbox — no #[ignore].
//! Requires a running OpenShell gateway (dev machines have it).

use right_openshell::test_support::TestSandbox;

/// Confirm that `claude -p --debug --debug-file=<path>` creates a non-empty
/// file inside the sandbox at the specified path.
///
/// This is the load-bearing assumption of /rightreflect: that enabling debug
/// mode produces an agent-readable per-session log at a known path.
///
/// Claude will exit non-zero (no auth token), but it should still create
/// the debug file before bailing.
#[tokio::test]
async fn cc_debug_file_lands_inside_sandbox() {
    let _slot = right_openshell::openshell::acquire_sandbox_slot();
    let sandbox = TestSandbox::create("rightreflect-debugfile").await;

    let session_id = "rightreflect-test-00000000-0000-0000-0000-000000000001";
    let log_path = format!("/sandbox/.claude/logs/{session_id}.log");

    // Pre-create the logs directory; CC will write to it.
    let (_, exit) = sandbox
        .exec(&["mkdir", "-p", "/sandbox/.claude/logs"])
        .await;
    assert_eq!(exit, 0, "mkdir /sandbox/.claude/logs failed");

    // Minimal claude -p invocation. We do NOT assert success — claude will
    // fail without a real auth token. We only care that the --debug-file
    // is created before claude exits.
    let _ignored = sandbox
        .exec(&[
            "claude",
            "-p",
            "--dangerously-skip-permissions",
            "--debug",
            &format!("--debug-file={log_path}"),
            "--session-id",
            session_id,
            "--",
            "hello",
        ])
        .await;

    // The debug file must exist at the specified path.
    let (ls_out, ls_exit) = sandbox.exec(&["ls", "-la", &log_path]).await;
    assert_eq!(
        ls_exit, 0,
        "debug file not found at {log_path}; ls output: {ls_out}"
    );
    assert!(
        ls_out.contains(session_id),
        "session id not in ls output for {log_path}: {ls_out}"
    );

    // The debug file must be non-empty — it should contain at least a header.
    let (wc_out, wc_exit) = sandbox.exec(&["wc", "-c", &log_path]).await;
    assert_eq!(wc_exit, 0, "wc -c failed on {log_path}: {wc_out}");
    let bytes: u64 = wc_out
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .expect("parse byte count from wc output");
    assert!(bytes > 0, "debug file is empty (0 bytes) at {log_path}");
}

/// Confirm that the JSONL project directory CC uses for `/sandbox` as cwd is
/// accessible and writable from inside the sandbox, and that files written
/// there survive a subsequent CC invocation (CC does not wipe pre-existing
/// JSONL files on unauthenticated startup).
///
/// Background: CC uses the working directory to derive the project path:
/// `/sandbox` → `/sandbox/.claude/projects/-sandbox/`. The /rightreflect
/// skill reads JSONL files from this directory. This test verifies:
///
/// 1. The path is writable (the agent can write JSONL there if needed).
/// 2. CC does not wipe the directory or its contents on startup — a pre-seeded
///    file is still present after a failed (no-auth) CC invocation.
///
/// Note: CC only writes its own JSONL transcript after a successful session;
/// it exits before writing anything when there is no auth token. That is why
/// this test seeds a synthetic file rather than relying on CC to create one.
#[tokio::test]
async fn jsonl_project_dir_is_accessible_and_cc_preserves_contents() {
    let _slot = right_openshell::openshell::acquire_sandbox_slot();
    let sandbox = TestSandbox::create("rightreflect-jsonl").await;

    let session_id = "rightreflect-test-00000000-0000-0000-0000-000000000002";
    let project_dir = "/sandbox/.claude/projects/-sandbox";
    let jsonl_path = format!("{project_dir}/{session_id}.jsonl");
    let marker = "RIGHTREFLECT-PRESERVED-MARKER";

    // Seed the project directory with a synthetic JSONL file, simulating a
    // file that CC would have written in a real (authenticated) session.
    let (_, mkdir_exit) = sandbox.exec(&["mkdir", "-p", project_dir]).await;
    assert_eq!(mkdir_exit, 0, "mkdir {project_dir} failed");

    let write_cmd = format!(
        "printf '%s\\n' '{{\"type\":\"assistant\",\"marker\":\"{marker}\"}}' > {}",
        jsonl_path
    );
    let (write_out, write_exit) = sandbox.exec(&["sh", "-c", &write_cmd]).await;
    assert_eq!(write_exit, 0, "write synthetic jsonl failed: {write_out}");

    // Run CC with the matching session-id. It exits non-zero (no auth), but it
    // must not wipe the project directory or the pre-seeded JSONL file.
    let _ignored = sandbox
        .exec(&[
            "claude",
            "-p",
            "--dangerously-skip-permissions",
            "--session-id",
            session_id,
            "--",
            "hello",
        ])
        .await;

    // The file must still be present after CC exits.
    let (cat_out, cat_exit) = sandbox.exec(&["cat", &jsonl_path]).await;
    assert_eq!(
        cat_exit, 0,
        "jsonl file missing after CC invocation at {jsonl_path}: {cat_out}"
    );
    assert!(
        cat_out.contains(marker),
        "file content was modified or wiped by CC; expected marker '{marker}' in: {cat_out}"
    );
}

/// Confirm that a synthetic JSONL file in the projects directory can be
/// located and grepped from inside the sandbox — the filesystem-access
/// primitive that /rightreflect relies on.
///
/// This test is filesystem-only: no claude binary needed.
#[tokio::test]
async fn skill_can_grep_jsonl() {
    let _slot = right_openshell::openshell::acquire_sandbox_slot();
    let sandbox = TestSandbox::create("rightreflect-grep").await;

    let (_, exit) = sandbox
        .exec(&["mkdir", "-p", "/sandbox/.claude/projects/-sandbox"])
        .await;
    assert_eq!(exit, 0, "mkdir projects dir failed");

    let marker = "RIGHTREFLECT-MARKER-XYZZY";
    // A realistic JSONL line with a tool_use call referencing the marker.
    let jsonl_line = format!(
        r#"{{"type":"assistant","uuid":"abc","message":{{"content":[{{"type":"tool_use","name":"mcp__right__cron_create","input":{{"job_name":"{marker}"}}}}]}}}}"#
    );
    let target = "/sandbox/.claude/projects/-sandbox/synthetic-session.jsonl";

    // Write the synthetic JSONL file via a shell printf.
    // Single quotes in the line are escaped using '\''.
    let write_cmd = format!(
        "printf '%s\\n' '{}' > {}",
        jsonl_line.replace('\'', "'\\''"),
        target
    );
    let (write_out, write_exit) = sandbox.exec(&["sh", "-c", &write_cmd]).await;
    assert_eq!(write_exit, 0, "write synthetic jsonl failed: {write_out}");

    // grep -l returns the filename when the pattern is found.
    let grep_cmd = format!("grep -l {marker} /sandbox/.claude/projects/-sandbox/*.jsonl");
    let (grep_out, grep_exit) = sandbox.exec(&["sh", "-c", &grep_cmd]).await;
    assert_eq!(
        grep_exit, 0,
        "grep did not find marker '{marker}' in jsonl files: {grep_out}"
    );
    assert!(
        grep_out.contains("synthetic-session.jsonl"),
        "grep output didn't contain expected filename: {grep_out}"
    );
}
