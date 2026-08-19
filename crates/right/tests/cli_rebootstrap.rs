//! CLI surface tests for `right agent rebootstrap`.
//!
//! The full library-level happy path is covered by
//! `right-agent`'s `rebootstrap_sandbox` integration test, so here we only
//! exercise the CLI-level concerns: argument validation, missing-agent
//! errors, and the abort-on-cancel path.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn rebootstrap_unknown_agent_errors_with_name() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("right")
        .unwrap()
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "rebootstrap",
            "ghost",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ghost"));
}

#[test]
fn rebootstrap_help_lists_yes_flag() {
    Command::cargo_bin("right")
        .unwrap()
        .args(["agent", "rebootstrap", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--yes"));
}

/// Regression for the 2026-04-29 incident: when state.json is present but
/// process-compose is unreachable (auth-broken, dead, port stolen), the
/// command MUST refuse rather than proceed with file ops. Previously this
/// was silently swallowed and the agent was left bootstrapped on disk
/// while the still-running bot served the old persona.
#[test]
fn rebootstrap_errors_when_state_present_but_pc_unreachable() {
    let home = tempfile::tempdir().unwrap();

    // Set up a minimal agent dir so `rebootstrap::plan` succeeds.
    let agent_dir = home.path().join("agents").join("ghosty");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "# ghosty\n").unwrap();
    std::fs::write(agent_dir.join("agent.yaml"), "sandbox:\n  name: right-test\n").unwrap();

    // state.json points at a port nothing listens on. Reserved port 1 is
    // unused by anything reasonable; any TCP connect attempt will fail
    // immediately, mimicking "PC is dead". The token is irrelevant since
    // no server will accept the connection.
    let run_dir = home.path().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(
        run_dir.join("state.json"),
        r#"{"agents":[{"name":"ghosty"}],"socket_path":"/tmp/x.sock","started_at":"2026-04-29T00:00:00Z","pc_port":1,"pc_api_token":"any"}"#,
    )
    .unwrap();

    Command::cargo_bin("right")
        .unwrap()
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "rebootstrap",
            "ghosty",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Refusing to rebootstrap"));

    // File ops MUST NOT have run. IDENTITY.md must still be present and
    // BOOTSTRAP.md must NOT have been (re)written.
    assert!(
        agent_dir.join("IDENTITY.md").exists(),
        "IDENTITY.md should be untouched when PC is unreachable",
    );
    assert!(
        !agent_dir.join("BOOTSTRAP.md").exists(),
        "BOOTSTRAP.md should NOT have been written when PC is unreachable",
    );
}

#[tokio::test]
async fn rebootstrap_unavailable_sandbox_preserves_host_sessions_and_answers() {
    let home = tempfile::tempdir().unwrap();
    let agent_dir = home.path().join("agents").join("sandboxed");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("agent.yaml"),
        "sandbox:\n  mode: openshell\n  name: missing-sandbox\n  policy_file: policy.yaml\n",
    )
    .unwrap();
    std::fs::write(agent_dir.join("policy.yaml"), "version: 1\n").unwrap();
    for name in right_agent::rebootstrap::IDENTITY_FILES {
        std::fs::write(agent_dir.join(name), format!("original {name}\n")).unwrap();
    }

    let conn = right_db::open_connection(&agent_dir, true).await.unwrap();
    conn.execute(
        "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
         VALUES (7, 3, 'preserved-session', 1)",
        [],
    )
    .await
    .unwrap();
    right_db::bootstrap_answers::claim_owner(&conn, 7, 3)
        .await
        .unwrap();
    right_db::bootstrap_answers::record_question_issue(&conn, "user_name", 7, 3, 98)
        .await
        .unwrap();
    right_db::conversation::archive_message(
        &conn,
        right_db::conversation::ConversationMessage {
            platform: "telegram",
            chat_id: 7,
            thread_id: 3,
            message_id: Some(99),
            sender_user_id: Some(1),
            sender_name: Some("Ada"),
            addressed_to_bot: true,
            routed_to_agent: true,
            root_session_id: Some("preserved-session"),
            turn_id: None,
            role: right_db::conversation::ConversationRole::User,
            content: "Ada",
        },
    )
    .await
    .unwrap();
    assert_eq!(
        right_db::bootstrap_answers::record_answer(&conn, "user_name", "Ada", 7, 3, 99)
            .await
            .unwrap(),
        right_db::bootstrap_answers::RecordAnswerOutcome::Recorded
    );
    drop(conn);

    Command::cargo_bin("right")
        .unwrap()
        .env("PATH", "")
        .env("OPENSHELL_MTLS_DIR", home.path().join("missing-mtls"))
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "agent",
            "rebootstrap",
            "sandboxed",
            "-y",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing to reset host state"));

    for name in right_agent::rebootstrap::IDENTITY_FILES {
        assert_eq!(
            std::fs::read_to_string(agent_dir.join(name)).unwrap(),
            format!("original {name}\n")
        );
    }
    assert!(!agent_dir.join("BOOTSTRAP.md").exists());

    let conn = right_db::open_connection(&agent_dir, false).await.unwrap();
    let active_sessions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE is_active = 1",
            [],
            |row| row.get(0),
        )
        .await
        .unwrap();
    assert_eq!(active_sessions, 1);
    assert_eq!(
        right_db::bootstrap_answers::missing_stages(&conn, 7, 3)
            .await
            .unwrap(),
        vec!["agent_name", "nature", "vibe", "emoji"]
    );
}
