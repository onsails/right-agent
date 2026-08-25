use super::{DEFAULT_EXEC_TIMEOUT, SANDBOX_HOME, exec_request, guest_exec_request};

#[test]
fn guest_exec_request_uses_unprivileged_user() {
    let request = guest_exec_request(&["true"], DEFAULT_EXEC_TIMEOUT);

    assert_eq!(request.user.as_deref(), Some(right_sandbox::GUEST_USER));
}

#[test]
fn exec_request_splits_program_from_arguments() {
    let request = exec_request(&["rm", "-rf", "/sandbox/x"], DEFAULT_EXEC_TIMEOUT);

    assert_eq!(request.cmd, "rm");
    assert_eq!(request.args, ["-rf", "/sandbox/x"]);
    assert_eq!(request.cwd.as_deref(), Some(SANDBOX_HOME));
    assert_eq!(request.timeout, Some(DEFAULT_EXEC_TIMEOUT));
}

#[test]
fn exec_request_carries_a_program_with_no_arguments() {
    let request = exec_request(&["true"], DEFAULT_EXEC_TIMEOUT);

    assert_eq!(request.cmd, "true");
    assert!(request.args.is_empty());
}
