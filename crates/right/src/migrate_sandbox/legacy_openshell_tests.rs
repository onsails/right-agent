//! Tests for the frozen OpenShell read path.
//!
//! Ported from the retired `right-openshell` crate's `openshell_tests.rs`,
//! narrowed to what the migration still vendors. The tar-argument tests are
//! the load-bearing ones: they pin the archive layout the microsandbox
//! restore in `right_agent::sandbox_migrate` expects.

use super::*;

/// The rebuildable-cache excludes the retired crate's backups used. Kept here
/// only so the exclude tests exercise a realistic multi-entry set.
const REBUILDABLE_EXCLUDES: &[&str] = &[".cache", ".venv", ".npm", ".uv"];

#[test]
fn sandbox_name_prefixes_agent_name() {
    assert_eq!(resolve_sandbox_name("brain", None), "right-brain");
    assert_eq!(resolve_sandbox_name("worker-1", None), "right-worker-1");
}

#[test]
fn sandbox_name_fits_within_upstream_routable_limit() {
    // `right-{agent}` would be 20 chars — over the cap.
    let name = resolve_sandbox_name("fourteenchars1", None);
    assert!(name.len() <= MAX_SANDBOX_NAME_LEN);
    assert!(name.starts_with("right-"));
    assert_eq!(name, resolve_sandbox_name("fourteenchars1", None));
    // Common prefixes must not collide.
    assert_ne!(
        resolve_sandbox_name("aaaaaaaaaaaaaaaa1", None),
        resolve_sandbox_name("aaaaaaaaaaaaaaaa2", None)
    );
}

#[test]
fn fit_sandbox_name_boundary() {
    let nineteen = "abcdefghijklmnopqrs";
    assert_eq!(fit_sandbox_name(nineteen), nineteen);
    let fitted = fit_sandbox_name("abcdefghijklmnopqrst");
    assert_eq!(fitted.len(), MAX_SANDBOX_NAME_LEN);
    assert!(fitted.starts_with("abcdefghijklmn-"));
}

#[test]
fn fit_sandbox_name_truncates_multibyte_on_byte_cap() {
    // Non-ASCII is outside the DNS-1123 charset and sanitizes to dashes.
    let fitted = fit_sandbox_name(&"é".repeat(15));
    assert!(fitted.len() <= MAX_SANDBOX_NAME_LEN);
    assert!(!fitted.contains('é'));
    let mixed = fit_sandbox_name("agent-émile-xxxxxxxxxxxxxxxx");
    assert!(mixed.len() <= MAX_SANDBOX_NAME_LEN);
    assert!(mixed.starts_with("agent-"));
}

#[test]
fn fit_sandbox_name_produces_dns1123_labels() {
    let assert_label = |name: &str| {
        assert!(!name.is_empty());
        assert!(name.len() <= MAX_SANDBOX_NAME_LEN);
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "bad chars: {name}"
        );
        assert!(
            !name.starts_with('-') && !name.ends_with('-'),
            "edge hyphen: {name}"
        );
        assert!(!name.contains("--"), "double hyphen: {name}");
    };
    // Hyphen at the truncation boundary must not produce `prefix--hash`.
    assert_label(&resolve_sandbox_name("abcdefgh-klmnop", None));
    assert_label(&resolve_sandbox_name("My_Agent X", None));
    assert_label(&resolve_sandbox_name("emile-long-agent-name-é", None));
    assert_label(&resolve_sandbox_name("a--b__c", None));
    assert_eq!(
        resolve_sandbox_name("My_Agent X", None),
        resolve_sandbox_name("My_Agent X", None)
    );
    assert_ne!(
        resolve_sandbox_name("My_Agent X1", None),
        resolve_sandbox_name("My_Agent X2", None)
    );
}

#[test]
fn resolve_sandbox_name_prefers_the_explicit_name_verbatim() {
    // Legacy over-long names were never rewritten upstream: reads are not
    // length-validated, so the migration must look the sandbox up as written.
    assert_eq!(
        resolve_sandbox_name("brain", Some("rightclaw-brain-20260415-1430")),
        "rightclaw-brain-20260415-1430"
    );
}

#[test]
fn ssh_host_for_sandbox_formats_correctly() {
    assert_eq!(
        ssh_host_for_sandbox("right-brain"),
        "openshell-right-brain.default"
    );
}

#[test]
fn sandbox_tar_download_args_reads_sandbox_dir_and_preserves_archive_root() {
    assert_eq!(
        sandbox_tar_download_args("sandbox", &[]).unwrap(),
        vec![
            "tar",
            "czpf",
            "-",
            "-C",
            "/sandbox",
            "--transform=flags=rh;s,^\\.$,sandbox,",
            "--transform=flags=rh;s,^\\./,sandbox/,",
            ".",
        ]
    );
}

#[test]
fn sandbox_tar_download_args_emits_directory_and_child_excludes() {
    let args = sandbox_tar_download_args("sandbox", REBUILDABLE_EXCLUDES).unwrap();

    assert_eq!(&args[0..5], ["tar", "czpf", "-", "-C", "/sandbox"]);
    assert!(args.contains(&"--transform=flags=rh;s,^\\.$,sandbox,".to_string()));
    assert!(args.contains(&"--transform=flags=rh;s,^\\./,sandbox/,".to_string()));
    assert_eq!(args.last().unwrap(), ".");

    for path in REBUILDABLE_EXCLUDES {
        assert!(
            args.contains(&format!("--exclude=./{path}")),
            "missing directory exclude for {path}: {args:?}"
        );
        assert!(
            args.contains(&format!("--exclude=./{path}/*")),
            "missing child exclude for {path}: {args:?}"
        );
    }
}

#[test]
fn sandbox_tar_download_args_without_excludes_emits_no_exclude_flags() {
    let args = sandbox_tar_download_args("sandbox", &[]).unwrap();

    assert_eq!(&args[0..5], ["tar", "czpf", "-", "-C", "/sandbox"]);
    assert_eq!(args.last().unwrap(), ".");

    for path in REBUILDABLE_EXCLUDES {
        assert!(
            !args.contains(&format!("--exclude=./{path}")),
            "forensic mode should not exclude {path}: {args:?}"
        );
        assert!(
            !args.contains(&format!("--exclude=./{path}/*")),
            "forensic mode should not exclude children of {path}: {args:?}"
        );
    }
}

#[test]
fn sandbox_tar_download_args_rejects_an_empty_sandbox_path() {
    // An empty archive root would transform every member to `/...`.
    assert!(sandbox_tar_download_args("/", &[]).is_err());
}

#[test]
fn sandbox_tar_download_remote_command_quotes_transform_semicolons_for_shell() {
    use std::process::Command;

    let args = sandbox_tar_download_args("sandbox", REBUILDABLE_EXCLUDES).unwrap();
    let remote_command = quote_ssh_remote_args(args.iter().map(String::as_str)).unwrap();
    let probe = format!(
        "tar() {{ for arg in \"$@\"; do printf '<%s>\\n' \"$arg\"; done; }}; {remote_command}"
    );

    let output = Command::new("sh").arg("-c").arg(&probe).output().unwrap();
    assert!(
        output.status.success(),
        "remote command must not be split at transform semicolons; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed_args: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let expected_args: Vec<String> = args[1..].iter().map(|arg| format!("<{arg}>")).collect();
    assert_eq!(parsed_args, expected_args);
}

#[test]
fn quote_ssh_remote_args_preserves_shell_metacharacters_as_data() {
    use std::process::Command;

    let remote_command = quote_ssh_remote_args([
        "probe_cmd",
        "alpha beta",
        "$(nope)",
        "semi;colon",
        "quote'arg",
    ])
    .unwrap();
    let probe = format!(
        "probe_cmd() {{ for arg in \"$@\"; do command printf '<%s>\\n' \"$arg\"; done; }}; {remote_command}"
    );

    let output = Command::new("sh").arg("-c").arg(probe).output().unwrap();
    assert!(
        output.status.success(),
        "quoted command should parse under sh; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "<alpha beta>\n<$(nope)>\n<semi;colon>\n<quote'arg>\n"
    );
}

#[test]
fn sandbox_tar_download_args_preserves_relative_symlink_targets() {
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::path::PathBuf;
    use std::process::Command;

    let tar_version = Command::new("tar")
        .arg("--version")
        .output()
        .expect("tar --version should run");
    if !String::from_utf8_lossy(&tar_version.stdout).contains("GNU tar") {
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let out = tmp.path().join("out");
    let archive = tmp.path().join("archive.tar.gz");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(src.join("target"), "data").unwrap();
    symlink("./target", src.join("link")).unwrap();
    std::fs::hard_link(src.join("target"), src.join("hard")).unwrap();

    let mut args = sandbox_tar_download_args("sandbox", &[]).unwrap();
    assert_eq!(args.remove(0), "tar");
    let archive_arg = args.iter().position(|arg| arg == "-").unwrap();
    args[archive_arg] = archive.to_string_lossy().into_owned();
    let source_arg = args.iter().position(|arg| arg == "/sandbox").unwrap();
    args[source_arg] = src.to_string_lossy().into_owned();

    let status = Command::new("tar").args(&args).status().unwrap();
    assert!(status.success(), "tar create failed with {status}");

    let status = Command::new("tar")
        .arg("xzpf")
        .arg(&archive)
        .arg("-C")
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success(), "tar extract failed with {status}");

    assert_eq!(
        std::fs::read_link(out.join("sandbox/link")).unwrap(),
        PathBuf::from("./target")
    );
    assert_eq!(
        std::fs::metadata(out.join("sandbox/target")).unwrap().ino(),
        std::fs::metadata(out.join("sandbox/hard")).unwrap().ino(),
    );
}

// ---------------------------------------------------------------------------
// CLI output parsing — the surface that replaced the retired gRPC client.
// Both shapes are captured from a live `openshell` v0.0.105 gateway.
// ---------------------------------------------------------------------------

#[test]
fn sandbox_phase_reads_the_top_level_phase_field() {
    let json = r#"{"name":"right-brain","phase":"Ready","policy":{"landlock":{}}}"#;
    assert_eq!(sandbox_phase(json).unwrap(), "Ready");
}

#[test]
fn sandbox_phase_fails_rather_than_guessing() {
    // A gateway that answers without a phase must not read as "not ready yet"
    // and burn the whole readiness timeout.
    assert!(sandbox_phase(r#"{"name":"right-brain"}"#).is_err());
    assert!(sandbox_phase("not json at all").is_err());
}

#[test]
fn attached_provider_names_come_from_the_first_table_column() {
    // The real bytes: the CLI renders the header bold even into a pipe with
    // NO_COLOR set, so it arrives wrapped in SGR escapes. A fixture that lost
    // them to copy-paste is why this parser shipped returning an empty list
    // for every sandbox — the test passed while production found no header.
    let table = "\u{1b}[1mNAME\u{1b}[0m        \u{1b}[1mTYPE\u{1b}[0m                 \u{1b}[1mCREDENTIAL_KEYS\u{1b}[0m  \u{1b}[1mCONFIG_KEYS\u{1b}[0m\n\
                 agent-a-provider   right-fal                          1                0\n\
                 agent-a-twitterapi  right-provider-agent-a-service   1                0\n";
    assert_eq!(
        parse_attached_provider_names(table),
        vec!["agent-a-provider", "agent-a-twitterapi"]
    );
}

#[test]
fn attached_provider_names_parse_without_escapes_too() {
    let table = "NAME        TYPE\n\
                 agent-a-provider   right-fal\n";
    assert_eq!(
        parse_attached_provider_names(table),
        vec!["agent-a-provider"]
    );
}

#[test]
fn no_attached_providers_parses_as_empty_not_as_a_row() {
    // The CLI prints a sentence, not an empty table, when nothing is attached.
    assert!(
        parse_attached_provider_names("No providers attached to sandbox right-brain.\n").is_empty()
    );
    assert!(parse_attached_provider_names("").is_empty());
}
