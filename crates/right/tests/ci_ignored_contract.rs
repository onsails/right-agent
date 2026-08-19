use std::fs;
use std::path::{Path, PathBuf};

const CI_IGNORE_PREFIXES: &[(&str, &str)] = &[
    ("ci-openshell", "ci_openshell_"),
    ("ci-claude", "ci_claude_"),
    ("ci-stt", "ci_stt_"),
    ("ci-msb", "ci_msb_"),
];

#[test]
fn ci_ignored_tests_have_workspace_filterable_names() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/right should have repo root parent")
        .to_path_buf();

    let mut rust_files = Vec::new();
    collect_rust_files(&repo.join("crates"), &mut rust_files);

    let mut violations = Vec::new();
    for path in rust_files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        check_file(&path, &source, &mut violations);
    }

    assert!(
        violations.is_empty(),
        "CI ignored test naming violations:\n{}",
        violations.join("\n")
    );
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|err| panic!("read {}: {err}", dir.display())) {
        let entry = entry.expect("read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn check_file(path: &Path, source: &str, violations: &mut Vec<String>) {
    let lines: Vec<_> = source.lines().collect();

    for (idx, line) in lines.iter().enumerate() {
        let Some(marker) = ci_ignore_marker(line) else {
            continue;
        };
        let Some(expected_prefix) = expected_test_prefix(marker) else {
            violations.push(format!(
                "{}:{} unknown CI ignore marker `{}`",
                path.display(),
                idx + 1,
                marker
            ));
            continue;
        };
        let Some((fn_line, fn_name)) = next_test_fn(&lines, idx + 1) else {
            violations.push(format!(
                "{}:{} `{}` ignore is not followed by a test function",
                path.display(),
                idx + 1,
                marker
            ));
            continue;
        };
        if !fn_name.starts_with(expected_prefix) {
            violations.push(format!(
                "{}:{} `{}` test `{}` must start with `{}` so workspace CI filters run it",
                path.display(),
                fn_line + 1,
                marker,
                fn_name,
                expected_prefix
            ));
        }
    }
}

fn ci_ignore_marker(line: &str) -> Option<&'static str> {
    CI_IGNORE_PREFIXES
        .iter()
        .map(|(marker, _)| *marker)
        .find(|marker| line.contains(&format!("#[ignore = \"{}:", marker)))
}

fn expected_test_prefix(marker: &str) -> Option<&'static str> {
    CI_IGNORE_PREFIXES
        .iter()
        .find_map(|(candidate, prefix)| (*candidate == marker).then_some(*prefix))
}

fn next_test_fn<'a>(lines: &'a [&str], start: usize) -> Option<(usize, &'a str)> {
    lines
        .iter()
        .enumerate()
        .skip(start)
        .take(8)
        .find_map(|(idx, line)| function_name(line).map(|name| (idx, name)))
}

fn function_name(line: &str) -> Option<&str> {
    let (_, rest) = line.split_once("fn ")?;
    rest.split(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
        .next()
        .filter(|name| !name.is_empty())
}
