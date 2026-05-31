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
