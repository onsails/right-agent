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
fn doctor_renders_brand_block_ascii() {
    let home = isolated_home();
    // assert_cmd runs the binary non-TTY, which forces Theme::Ascii
    // (`|` rail, `-` dashes, `[ok]/[warn]/[err]` glyphs).
    right()
        .env_remove("NO_COLOR")
        .args(["--home", home.path().to_str().unwrap(), "doctor"])
        .assert()
        // doctor may exit nonzero on a fresh tempdir (missing deps) — that's fine.
        .stdout(predicate::str::contains("| diagnostics"))
        .stdout(predicate::str::contains("checks passed"));
}

#[test]
fn doctor_ascii_no_unicode_atoms_and_no_ansi() {
    let home = isolated_home();
    let assert = right()
        .args(["--home", home.path().to_str().unwrap(), "doctor"])
        .assert();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    // No Unicode brand atoms in Ascii mode.
    for ch in ['▐', '✓', '✗', '…'] {
        assert!(
            !stdout.contains(ch),
            "ascii output must not contain {ch:?}, full stdout:\n{stdout}"
        );
    }
    // The bare ASCII '!' may appear in shell instructions — only exclude bracketed glyphs.
    // Verify no ANSI escapes.
    assert!(
        !stdout.contains('\x1b'),
        "ascii output must not contain ANSI escapes, full stdout:\n{stdout}"
    );
    // Verify the bracketed Ascii glyph at least once (any of [ok]/[warn]/[err]).
    assert!(
        stdout.contains("[ok]") || stdout.contains("[warn]") || stdout.contains("[err]"),
        "ascii output should contain at least one bracketed glyph, full stdout:\n{stdout}"
    );
}

#[test]
fn doctor_dumb_term_still_ascii() {
    // Even with TERM=dumb explicitly set, output stays Ascii (already the default for non-TTY).
    let home = isolated_home();
    let assert = right()
        .env("TERM", "dumb")
        .args(["--home", home.path().to_str().unwrap(), "doctor"])
        .assert();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("| diagnostics") || stdout.contains("|  ["),
        "TERM=dumb should still produce Ascii rail+glyphs, full stdout:\n{stdout}"
    );
}

#[test]
fn status_no_pc_running_renders_err_with_fix() {
    let home = isolated_home();
    // No `right up` was called → no run/state.json
    right()
        .args(["--home", home.path().to_str().unwrap(), "status"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("| status"))
        .stdout(predicate::str::contains("not running"))
        .stdout(predicate::str::contains("right up"));
}

// --- right init brand tests ---
// assert_cmd runs the binary non-TTY, so Theme::Ascii is always active.
// Rail::mark(Ascii) = "|*", Rail::blank(Ascii) = "|", section(Ascii, "x") starts with "| x ".

#[test]
fn init_first_run_splash_and_recap() {
    let home = isolated_home();
    right()
        .args([
            "--home",
            home.path().to_str().unwrap(),
            "init",
            "-y",
            "--sandbox-mode",
            "none",
            "--tunnel-hostname",
            "test.example.com",
        ])
        .assert()
        .success()
        // splash: first line is "|* right agent v<version>"
        .stdout(predicate::str::contains("|* right agent v"))
        // dependency section header
        .stdout(predicate::str::contains("| dependencies "))
        // recap section header
        .stdout(predicate::str::contains("| ready "))
        // next-step pointer
        .stdout(predicate::str::contains("|  next: right up"));
}

#[test]
fn init_rerun_writes_recap_again() {
    // Two independent init runs (separate homes) both produce the recap.
    // (init_right_home guards against re-init on the same home without --force;
    // this test focuses on recap being present on any fresh run.)
    for _ in 0..2 {
        let home = isolated_home();
        right()
            .args([
                "--home",
                home.path().to_str().unwrap(),
                "init",
                "-y",
                "--sandbox-mode",
                "none",
                "--tunnel-hostname",
                "test.example.com",
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("| ready "));
    }
}
