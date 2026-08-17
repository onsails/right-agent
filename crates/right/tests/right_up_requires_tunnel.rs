//! Integration test: `right up` must error out when the global config has no
//! tunnel block (post-mandatory-tunnel cutover).
//!
//! Both tests run `right up`, which probes a fixed TCP port (MCP_HTTP_PORT)
//! before reading config. To avoid races on that bind probe — within this
//! binary AND across parallel `cargo test` runs in different worktrees — we
//! serialize via acquire_test_name_lock on a shared logical name.

use assert_cmd::Command;
use predicates::prelude::*;
use right_openshell::openshell::acquire_test_name_lock;
use std::ffi::OsString;
use tempfile::TempDir;

fn write_minimal_agent(home: &std::path::Path) {
    let agent_dir = home.join("agents").join("test");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("agent.yaml"),
        "restart: never\nnetwork_policy: permissive\n",
    )
    .unwrap();
}

fn path_without_openshell() -> OsString {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::env::split_paths(&path)
        .filter(|dir| !dir.join("openshell").is_file())
        .collect::<Vec<_>>();
    std::env::join_paths(paths).unwrap()
}

#[test]
fn right_up_errors_when_global_config_missing() {
    let _lock = acquire_test_name_lock("right-up-fixed-port");
    let home = TempDir::new().unwrap();
    write_minimal_agent(home.path());

    Command::cargo_bin("right")
        .unwrap()
        .env("PATH", path_without_openshell())
        .env("OPENSHELL_MTLS_DIR", home.path().join("missing-mtls"))
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "up",
            "--non-interactive",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("tunnel").or(predicate::str::contains("right init")));
}

#[test]
fn right_up_errors_when_tunnel_block_missing_from_config() {
    let _lock = acquire_test_name_lock("right-up-fixed-port");
    let home = TempDir::new().unwrap();
    write_minimal_agent(home.path());
    std::fs::write(
        home.path().join("config.yaml"),
        "aggregator:\n  allowed_hosts:\n    - example.com\n",
    )
    .unwrap();

    Command::cargo_bin("right")
        .unwrap()
        .env("PATH", path_without_openshell())
        .env("OPENSHELL_MTLS_DIR", home.path().join("missing-mtls"))
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "up",
            "--non-interactive",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("tunnel"));
}
