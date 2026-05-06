//! Integration test: ControlMaster directives in `generate_ssh_config`
//! actually engage multiplexing on a real sandbox.

#![cfg(unix)]

use right_agent::openshell;
use right_core::test_support::TestSandbox;

#[tokio::test]
async fn control_master_engages_after_first_ssh_call() {
    let _slot = openshell::acquire_sandbox_slot();
    let sandbox = TestSandbox::create("controlmaster").await;

    // Use /tmp directly so the ControlPath stays within the Unix domain socket
    // limit (104 bytes on macOS). The default temp dir on macOS expands to a
    // long /var/folders/... path that exceeds the limit.
    let tmp = tempfile::Builder::new()
        .tempdir_in("/tmp")
        .expect("tempdir in /tmp");
    let ssh_config_dir = tmp.path().to_path_buf();

    // Generate ssh-config with ControlMaster directives appended.
    let config_path = openshell::generate_ssh_config(sandbox.name(), &ssh_config_dir)
        .await
        .expect("generate_ssh_config");

    let host = openshell::ssh_host_for_sandbox(sandbox.name());
    let socket = openshell::control_master_socket_path(&ssh_config_dir, sandbox.name());

    // Pre-condition: no master yet.
    assert!(
        !socket.exists(),
        "control-master socket should not exist before any ssh call: {}",
        socket.display(),
    );
    assert!(
        !openshell::check_control_master(&config_path, &host).await,
        "check_control_master should be false before any ssh call",
    );

    // First ssh call — should establish the master.
    let out = openshell::ssh_exec(&config_path, &host, &["echo", "hello"], 10)
        .await
        .expect("first ssh_exec");
    assert!(out.contains("hello"), "unexpected stdout: {out:?}");

    // Master should now be alive.
    assert!(
        socket.exists(),
        "control-master socket should exist after first ssh call: {}",
        socket.display(),
    );
    assert!(
        openshell::check_control_master(&config_path, &host).await,
        "check_control_master should be true after first ssh call",
    );

    // Tear down — socket should be gone afterward.
    openshell::tear_down_control_master(&config_path, &host, &socket).await;
    assert!(
        !socket.exists(),
        "control-master socket should be gone after tear_down: {}",
        socket.display(),
    );
    assert!(
        !openshell::check_control_master(&config_path, &host).await,
        "check_control_master should be false after tear_down",
    );
}
