use super::*;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

#[test]
fn bin_dir_returns_parent() {
    assert_eq!(bin_dir(Path::new("/x/y/right")), PathBuf::from("/x/y"));
}

#[test]
fn rc_targets_selects_per_shell() {
    let home = Path::new("/home/u");
    assert_eq!(
        rc_targets(Some("/bin/bash"), home),
        vec![home.join(".bashrc"), home.join(".profile")]
    );
    assert_eq!(
        rc_targets(Some("/usr/bin/zsh"), home),
        vec![home.join(".zshrc")]
    );
    assert_eq!(
        rc_targets(Some("/usr/bin/fish"), home),
        vec![home.join(".config/fish/config.fish")]
    );
    assert_eq!(rc_targets(None, home), vec![home.join(".profile")]);
    assert_eq!(
        rc_targets(Some("/usr/bin/dash"), home),
        vec![home.join(".profile")]
    );
}

#[test]
fn standard_dir_is_persistently_on_path() {
    let home = tempdir().unwrap();
    assert!(is_persistently_on_path(
        Path::new("/usr/local/bin"),
        home.path(),
        Some("/bin/bash")
    ));
}

#[test]
fn fresh_home_is_not_on_path() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    assert!(!is_persistently_on_path(
        &bindir,
        home.path(),
        Some("/bin/bash")
    ));
}

#[test]
fn rc_mentioning_bindir_is_on_path() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    std::fs::write(
        home.path().join(".bashrc"),
        format!("export PATH=\"{}:$PATH\"\n", bindir.display()),
    )
    .unwrap();
    assert!(is_persistently_on_path(
        &bindir,
        home.path(),
        Some("/bin/bash")
    ));
}

#[test]
fn ensure_writes_block_then_is_idempotent() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    let rc = home.path().join(".bashrc");

    let first = ensure_on_path(&bindir, home.path(), Some("/bin/bash")).unwrap();
    assert_eq!(first, EnsureOutcome::Wrote { file: rc.clone() });
    let content = std::fs::read_to_string(&rc).unwrap();
    assert_eq!(content.matches(BLOCK_START).count(), 1);
    assert_eq!(content.matches(BLOCK_END).count(), 1);
    assert!(content.contains(&*bindir.to_string_lossy()));

    let second = ensure_on_path(&bindir, home.path(), Some("/bin/bash")).unwrap();
    assert_eq!(second, EnsureOutcome::AlreadyOnPath);
    assert_eq!(std::fs::read_to_string(&rc).unwrap(), content);
}

#[test]
fn apply_block_normalizes_orphan_and_duplicate_markers() {
    let bindir = Path::new("/opt/bin");
    let messy =
        format!("line1\n{BLOCK_START}\nold\n{BLOCK_END}\nline2\n{BLOCK_START}\norphan-no-end\n");
    let once = apply_block(&messy, bindir, Some("/bin/bash"));
    assert_eq!(once.matches(BLOCK_START).count(), 1);
    assert_eq!(once.matches(BLOCK_END).count(), 1);
    assert!(once.contains("line1") && once.contains("line2"));
    assert!(!once.contains("old") && !once.contains("orphan-no-end"));
    assert_eq!(apply_block(&once, bindir, Some("/bin/bash")), once);
}

#[test]
fn ensure_returns_could_not_write_when_target_is_a_dir() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    std::fs::create_dir(home.path().join(".bashrc")).unwrap(); // .bashrc is a dir
    match ensure_on_path(&bindir, home.path(), Some("/bin/bash")).unwrap() {
        EnsureOutcome::CouldNotWrite { file, .. } => {
            assert_eq!(file, home.path().join(".bashrc"))
        }
        other => panic!("expected CouldNotWrite, got {other:?}"),
    }
}

#[test]
fn ensure_writes_fish_config_with_fish_syntax() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    let rc = home.path().join(".config/fish/config.fish");
    let outcome = ensure_on_path(&bindir, home.path(), Some("/usr/bin/fish")).unwrap();
    assert_eq!(outcome, EnsureOutcome::Wrote { file: rc.clone() });
    let content = std::fs::read_to_string(&rc).unwrap();
    assert!(content.contains("fish_add_path"));
    assert!(content.contains(&*bindir.to_string_lossy()));
}

#[test]
fn ensure_skips_when_profile_already_mentions_bindir() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    std::fs::write(
        home.path().join(".profile"),
        format!("export PATH={}:$PATH\n", bindir.display()),
    )
    .unwrap();
    assert_eq!(
        ensure_on_path(&bindir, home.path(), Some("/bin/bash")).unwrap(),
        EnsureOutcome::AlreadyOnPath
    );
}

#[test]
fn ensure_writes_both_bashrc_and_profile_when_both_exist() {
    let home = tempdir().unwrap();
    let bindir = home.path().join(".local/bin");
    std::fs::write(home.path().join(".bashrc"), "# existing bashrc\n").unwrap();
    std::fs::write(home.path().join(".profile"), "# existing profile\n").unwrap();

    let outcome = ensure_on_path(&bindir, home.path(), Some("/bin/bash")).unwrap();
    // Primary (first written) is .bashrc.
    assert_eq!(
        outcome,
        EnsureOutcome::Wrote {
            file: home.path().join(".bashrc")
        }
    );

    for rc in [".bashrc", ".profile"] {
        let content = std::fs::read_to_string(home.path().join(rc)).unwrap();
        assert!(
            content.contains(BLOCK_START),
            "{rc} should contain the managed block"
        );
        assert!(
            content.contains(&*bindir.to_string_lossy()),
            "{rc} should mention bindir"
        );
    }
}
