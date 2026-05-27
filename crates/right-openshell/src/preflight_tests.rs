use super::{MIN_OPENSHELL_VERSION, parse_openshell_cli_version};
use semver::Version;

#[test]
fn min_openshell_version_is_v0_0_50() {
    assert_eq!(MIN_OPENSHELL_VERSION, Version::new(0, 0, 50));
}

#[test]
fn parse_openshell_cli_version_extracts_semver() {
    let v = parse_openshell_cli_version("openshell 0.0.50\n").unwrap();
    assert_eq!(v, Version::new(0, 0, 50));
}

#[test]
fn parse_openshell_cli_version_rejects_garbage() {
    let err = parse_openshell_cli_version("not a version line\n").unwrap_err();
    assert!(
        err.contains("could not parse"),
        "error should mention parse failure, got: {err}"
    );
}

#[test]
fn parse_openshell_cli_version_ignores_trailing_whitespace_and_lines() {
    let v = parse_openshell_cli_version("openshell 0.0.50  \n\nextra line\n").unwrap();
    assert_eq!(v, Version::new(0, 0, 50));
}
