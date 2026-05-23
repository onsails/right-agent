use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Default)]
struct ReleasePlzPackage {
    name: Option<String>,
    changelog_include: Vec<String>,
    git_tag_enable: Option<String>,
    git_tag_name: Option<String>,
}

#[test]
fn changelog_included_packages_have_advancing_package_tags() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config_path = repo_root.join("release-plz.toml");
    let config = std::fs::read_to_string(&config_path).expect("release-plz.toml must be readable");
    let packages = parse_packages(&config);

    let aggregate = packages
        .get("right")
        .expect("right package must be configured");
    assert!(
        !aggregate.changelog_include.is_empty(),
        "right package must aggregate crate changelogs"
    );

    let mut failures = Vec::new();
    for package_name in &aggregate.changelog_include {
        let package = packages.get(package_name).unwrap_or_else(|| {
            panic!("changelog_include package {package_name:?} must have a [[package]] block")
        });

        if package.git_tag_enable.as_deref() != Some("true") {
            failures.push(format!("{package_name}: git_tag_enable must be true"));
        }

        let expected_tag_name = format!("{package_name}-v{{{{ version }}}}");
        if package.git_tag_name.as_deref() != Some(expected_tag_name.as_str()) {
            failures.push(format!(
                "{package_name}: git_tag_name must be {expected_tag_name:?}, got {:?}",
                package.git_tag_name
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "packages included in the right changelog need per-package tags so release-plz advances their changelog baseline:\n{}",
        failures.join("\n")
    );
}

fn parse_packages(config: &str) -> BTreeMap<String, ReleasePlzPackage> {
    let mut packages = BTreeMap::new();
    let mut current = ReleasePlzPackage::default();
    let mut in_package = false;

    for raw_line in config.lines() {
        let line = raw_line.trim();
        if line == "[[package]]" {
            if in_package {
                insert_package(&mut packages, current);
                current = ReleasePlzPackage::default();
            }
            in_package = true;
            continue;
        }

        if !in_package {
            continue;
        }

        if let Some(value) = line.strip_prefix("name = ") {
            current.name = parse_string(value);
        } else if let Some(value) = line.strip_prefix("changelog_include = ") {
            current.changelog_include = parse_string_array(value);
        } else if let Some(value) = line.strip_prefix("git_tag_enable = ") {
            current.git_tag_enable = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("git_tag_name = ") {
            current.git_tag_name = parse_string(value);
        }
    }

    if in_package {
        insert_package(&mut packages, current);
    }

    packages
}

fn insert_package(packages: &mut BTreeMap<String, ReleasePlzPackage>, package: ReleasePlzPackage) {
    if let Some(name) = package.name.clone() {
        packages.insert(name, package);
    }
}

fn parse_string(value: &str) -> Option<String> {
    value
        .trim()
        .strip_prefix('"')?
        .strip_suffix('"')
        .map(str::to_string)
}

fn parse_string_array(value: &str) -> Vec<String> {
    value
        .trim()
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or_default()
        .split(',')
        .filter_map(parse_string)
        .collect()
}
