//! Integration tests for brand-conformant CLI surfaces.
//! See docs/superpowers/specs/2026-04-28-init-wizard-brand-redesign-design.md.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn right() -> Command {
    Command::cargo_bin("right").unwrap()
}

fn isolated_home() -> tempfile::TempDir {
    tempdir().unwrap()
}

#[test]
fn doctor_renders_brand_block_in_mono() {
    let home = isolated_home();
    // The test process is non-TTY so detect() returns Ascii (| rail, - dashes).
    // We assert on the structural markers present under all theme tiers:
    // - a rail-prefixed "diagnostics" section header
    // - a "checks passed" summary line
    right()
        .env("NO_COLOR", "1")
        .env("TERM", "xterm-256color")
        .args(["--home", home.path().to_str().unwrap(), "doctor"])
        .assert()
        // doctor exits nonzero whenever any check fails — accept either outcome
        .stdout(predicate::str::contains("| diagnostics"))
        .stdout(predicate::str::contains("checks passed"));
}
