//! Integration test: `rebootstrap::execute` against a live OpenShell sandbox.

use std::path::Path;

use right_agent::rebootstrap::{self, IDENTITY_FILES, RebootstrapPlan};
use right_core::test_support::TestSandbox;

/// Write a host-side agent dir with `agent.yaml` pointing at `sandbox_name`,
/// the three identity files, and a stamped active session row in data.db.
fn seed_agent_dir(agent_dir: &Path, sandbox_name: &str) {
    std::fs::create_dir_all(agent_dir).unwrap();
    let yaml = format!(
        "sandbox:\n  mode: openshell\n  name: {sandbox_name}\n  policy_file: policy.yaml\n"
    );
    std::fs::write(agent_dir.join("agent.yaml"), yaml).unwrap();
    // policy.yaml content irrelevant — we never apply it; agent.yaml just
    // needs to parse as a sandboxed agent.
    std::fs::write(agent_dir.join("policy.yaml"), "version: 1\n").unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "host id\n").unwrap();
    std::fs::write(agent_dir.join("SOUL.md"), "host soul\n").unwrap();
    std::fs::write(agent_dir.join("USER.md"), "host user\n").unwrap();

    let conn = right_db::open_connection(agent_dir, true).unwrap();
    conn.execute(
        "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
         VALUES (1, 0, 'sandbox-session-uuid', 1)",
        [],
    )
    .unwrap();
}

/// Verify a path inside the sandbox does not exist via `[ -e <path> ]`.
async fn assert_absent_in_sandbox(sandbox: &TestSandbox, path: &str) {
    let (_, exit) = sandbox.exec(&["test", "-e", path]).await;
    assert_ne!(exit, 0, "expected {path} to be absent in sandbox");
}

#[tokio::test]
async fn execute_against_live_sandbox() {
    let _slot = right_core::openshell::acquire_sandbox_slot();
    let sandbox = TestSandbox::create("rebootstrap").await;

    // Seed sandbox-side identity files via in-sandbox shell. echo into
    // /sandbox/ avoids the openshell upload code path entirely.
    for &f in IDENTITY_FILES {
        let (_, exit) = sandbox
            .exec(&["sh", "-c", &format!("echo sandbox-{f} > /sandbox/{f}")])
            .await;
        assert_eq!(exit, 0, "failed to seed /sandbox/{f}");
    }

    // Set up a temp home with the agent dir under it.
    let home = tempfile::tempdir().unwrap();
    let agent_name = "rb-test";
    let agent_dir = home.path().join("agents").join(agent_name);
    seed_agent_dir(&agent_dir, sandbox.name());

    // Build plan manually — the standard `plan()` would resolve a sandbox
    // name from `agent.yaml`, but our agent.yaml doesn't know about
    // TestSandbox's randomised name. We override via direct construction.
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let p = RebootstrapPlan {
        agent_name: agent_name.to_string(),
        agent_dir: agent_dir.clone(),
        backup_dir: home
            .path()
            .join("backups")
            .join(agent_name)
            .join(format!("rebootstrap-{timestamp}")),
        sandbox_mode: right_agent::agent::types::SandboxMode::Openshell,
        sandbox_name: Some(sandbox.name().to_string()),
    };

    let report = rebootstrap::execute(&p).await.expect("execute failed");

    // Host: identity files removed
    for &f in IDENTITY_FILES {
        assert!(
            !agent_dir.join(f).exists(),
            "host {f} should be removed"
        );
    }
    // Host: BOOTSTRAP.md created
    let bootstrap = std::fs::read_to_string(agent_dir.join("BOOTSTRAP.md")).unwrap();
    assert_eq!(bootstrap, right_codegen::BOOTSTRAP_INSTRUCTIONS);

    // Backup: host copies (use concrete content map — what seed_agent_dir wrote)
    let expected_host: &[(&str, &str)] = &[
        ("IDENTITY.md", "host id\n"),
        ("SOUL.md", "host soul\n"),
        ("USER.md", "host user\n"),
    ];
    for (name, content) in expected_host {
        let host_copy = report.backup_dir.join(name);
        assert!(host_copy.exists(), "backup of host {name} missing");
        assert_eq!(&std::fs::read_to_string(&host_copy).unwrap(), content);
    }

    // Backup: sandbox copies
    for &f in IDENTITY_FILES {
        let sb_copy = report.backup_dir.join("sandbox").join(f);
        assert!(sb_copy.exists(), "backup of sandbox {f} missing");
        let content = std::fs::read_to_string(&sb_copy).unwrap();
        assert_eq!(content, format!("sandbox-{f}\n"));
    }

    // Sandbox: identity files removed
    for &f in IDENTITY_FILES {
        assert_absent_in_sandbox(&sandbox, &format!("/sandbox/{f}")).await;
    }

    assert_eq!(report.sessions_deactivated, 1);
    assert_eq!(report.host_backed_up.to_vec(), IDENTITY_FILES.to_vec());
    assert_eq!(report.sandbox_backed_up.to_vec(), IDENTITY_FILES.to_vec());
    assert_eq!(
        report.sandbox_status,
        right_agent::rebootstrap::SandboxStatus::Cleaned
    );
}
