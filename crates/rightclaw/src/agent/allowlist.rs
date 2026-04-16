//! Bot-managed allowlist — trusted users (DM + anywhere) + opened groups (per-chat-id open gate).
//!
//! On-disk format (version 1):
//!
//! ```yaml
//! version: 1
//! users:
//!   - id: 123
//!     label: andrey
//!     added_by: null
//!     added_at: 2026-04-16T12:00:00Z
//! groups:
//!   - id: -1001234
//!     label: Dev Team
//!     opened_by: null
//!     opened_at: 2026-04-16T12:00:00Z
//! ```

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AllowedUser {
    pub id: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub added_by: Option<i64>,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AllowedGroup {
    pub id: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub opened_by: Option<i64>,
    pub opened_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AllowlistFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub users: Vec<AllowedUser>,
    #[serde(default)]
    pub groups: Vec<AllowedGroup>,
}

fn default_version() -> u32 { 1 }

pub const CURRENT_VERSION: u32 = 1;

impl Default for AllowlistFile {
    fn default() -> Self {
        Self { version: CURRENT_VERSION, users: Vec::new(), groups: Vec::new() }
    }
}

/// Parse YAML text into `AllowlistFile`. Rejects unknown future `version`.
pub fn parse_yaml(text: &str) -> Result<AllowlistFile, String> {
    let parsed: AllowlistFile = serde_saphyr::from_str(text)
        .map_err(|e| format!("allowlist.yaml parse error: {e:#}"))?;
    if parsed.version != CURRENT_VERSION {
        return Err(format!(
            "allowlist.yaml version {} is not supported (expected {})",
            parsed.version, CURRENT_VERSION
        ));
    }
    Ok(parsed)
}

/// Serialize `AllowlistFile` to YAML text (deterministic key order).
pub fn serialize_yaml(file: &AllowlistFile) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(512);
    out.push_str("# Bot-managed. Edit via /allow, /deny, /allow_all, /deny_all,\n");
    out.push_str("# or `rightclaw agent allow|deny|allow_all|deny_all`.\n");
    writeln!(out, "version: {}", file.version).unwrap();
    if file.users.is_empty() {
        out.push_str("users: []\n");
    } else {
        out.push_str("users:\n");
        for u in &file.users {
            writeln!(out, "  - id: {}", u.id).unwrap();
            write_label(&mut out, "label", u.label.as_deref(), 4);
            write_opt_i64(&mut out, "added_by", u.added_by, 4);
            writeln!(out, "    added_at: {}", u.added_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap();
        }
    }
    if file.groups.is_empty() {
        out.push_str("groups: []\n");
    } else {
        out.push_str("groups:\n");
        for g in &file.groups {
            writeln!(out, "  - id: {}", g.id).unwrap();
            write_label(&mut out, "label", g.label.as_deref(), 4);
            write_opt_i64(&mut out, "opened_by", g.opened_by, 4);
            writeln!(out, "    opened_at: {}", g.opened_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap();
        }
    }
    out
}

fn write_label(out: &mut String, key: &str, val: Option<&str>, indent: usize) {
    use std::fmt::Write;
    let spaces: String = " ".repeat(indent);
    match val {
        Some(s) => {
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
            writeln!(out, "{spaces}{key}: \"{escaped}\"").unwrap();
        }
        None => writeln!(out, "{spaces}{key}: null").unwrap(),
    }
}

fn write_opt_i64(out: &mut String, key: &str, val: Option<i64>, indent: usize) {
    use std::fmt::Write;
    let spaces: String = " ".repeat(indent);
    match val {
        Some(n) => writeln!(out, "{spaces}{key}: {n}").unwrap(),
        None => writeln!(out, "{spaces}{key}: null").unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_allowlist() {
        let text = "version: 1\nusers: []\ngroups: []\n";
        let parsed = parse_yaml(text).unwrap();
        assert_eq!(parsed.version, 1);
        assert!(parsed.users.is_empty());
        assert!(parsed.groups.is_empty());
    }

    #[test]
    fn parses_users_and_groups() {
        let text = r#"version: 1
users:
  - id: 42
    label: andrey
    added_by: null
    added_at: 2026-04-16T12:00:00Z
groups:
  - id: -1001
    label: "Dev Team"
    opened_by: 42
    opened_at: 2026-04-16T12:30:00Z
"#;
        let parsed = parse_yaml(text).unwrap();
        assert_eq!(parsed.users.len(), 1);
        assert_eq!(parsed.users[0].id, 42);
        assert_eq!(parsed.users[0].label.as_deref(), Some("andrey"));
        assert_eq!(parsed.users[0].added_by, None);
        assert_eq!(parsed.groups.len(), 1);
        assert_eq!(parsed.groups[0].id, -1001);
        assert_eq!(parsed.groups[0].opened_by, Some(42));
    }

    #[test]
    fn missing_version_defaults_to_1() {
        let text = "users: []\ngroups: []\n";
        let parsed = parse_yaml(text).unwrap();
        assert_eq!(parsed.version, 1);
    }

    #[test]
    fn rejects_unknown_version() {
        let text = "version: 99\nusers: []\ngroups: []\n";
        let err = parse_yaml(text).unwrap_err();
        assert!(err.contains("version 99"));
    }

    #[test]
    fn serialize_roundtrip() {
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 42,
                label: Some("andrey".into()),
                added_by: None,
                added_at: "2026-04-16T12:00:00Z".parse().unwrap(),
            }],
            groups: vec![AllowedGroup {
                id: -1001,
                label: Some("Dev Team".into()),
                opened_by: Some(42),
                opened_at: "2026-04-16T12:30:00Z".parse().unwrap(),
            }],
        };
        let yaml = serialize_yaml(&file);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed, file);
    }

    #[test]
    fn serialize_empty_lists_use_flow_style() {
        let file = AllowlistFile::default();
        let yaml = serialize_yaml(&file);
        assert!(yaml.contains("users: []"));
        assert!(yaml.contains("groups: []"));
    }

    #[test]
    fn serialize_escapes_quotes_in_label() {
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 1,
                label: Some(r#"has "quotes""#.into()),
                added_by: None,
                added_at: "2026-04-16T12:00:00Z".parse().unwrap(),
            }],
            groups: vec![],
        };
        let yaml = serialize_yaml(&file);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed.users[0].label.as_deref(), Some(r#"has "quotes""#));
    }
}
