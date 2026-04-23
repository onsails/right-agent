use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

fn run(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("rightclaw").unwrap();
    cmd.env("RIGHTCLAW_HOME", home);
    cmd
}

fn init_agent(home: &std::path::Path, name: &str) {
    let dir = home.join("agents").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("agent.yaml"), "restart: never\n").unwrap();
    std::fs::write(dir.join("IDENTITY.md"), "# test agent\n").unwrap();
}

#[test]
fn allow_adds_user_and_allowed_lists_it() {
    let home = TempDir::new().unwrap();
    init_agent(home.path(), "testbot");

    run(home.path())
        .args(["agent", "allow", "testbot", "42", "--label", "alice"])
        .assert()
        .success()
        .stdout(contains("added user 42"));

    run(home.path())
        .args(["agent", "allowed", "testbot"])
        .assert()
        .success()
        .stdout(contains("42"))
        .stdout(contains("alice"));
}

#[test]
fn deny_removes_user() {
    let home = TempDir::new().unwrap();
    init_agent(home.path(), "testbot");

    run(home.path())
        .args(["agent", "allow", "testbot", "99"])
        .assert()
        .success();
    run(home.path())
        .args(["agent", "deny", "testbot", "99"])
        .assert()
        .success()
        .stdout(contains("removed user 99"));
    run(home.path())
        .args(["agent", "allowed", "testbot", "--json"])
        .assert()
        .success()
        .stdout(contains("\"users\": []"));
}

#[test]
fn allow_all_opens_group() {
    let home = TempDir::new().unwrap();
    init_agent(home.path(), "testbot");

    run(home.path())
        .args([
            "agent",
            "allow_all",
            "testbot",
            "-1001234",
            "--label",
            "Dev",
        ])
        .assert()
        .success()
        .stdout(contains("opened group -1001234"));

    run(home.path())
        .args(["agent", "allowed", "testbot"])
        .assert()
        .success()
        .stdout(contains("-1001234"))
        .stdout(contains("Dev"));
}
