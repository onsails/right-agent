use hex;
use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use std::io::Write as _;

/// Error type for credential operations.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error on credentials file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("backup rotation failed: {0}")]
    BackupFailed(String),
    #[error("credentials file parent directory not found")]
    InvalidPath,
    #[error("atomic write failed: {0}")]
    Persist(#[from] tempfile::PersistError),
}

/// MCP OAuth token as stored in ~/.claude/.credentials.json.
#[derive(Serialize, Deserialize, Clone)]
pub struct CredentialToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    /// Unix timestamp seconds. 0 = non-expiring (e.g. Linear).
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
    /// OAuth client_id used to obtain this token — stored for refresh grant.
    /// Absent in credentials written before Phase 35 (deserializes as None).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth client_secret (confidential clients only). None for public clients.
    /// Absent in credentials written before Phase 35 (deserializes as None).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

impl std::fmt::Debug for CredentialToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialToken")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &self.refresh_token.as_deref().map(|_| "[REDACTED]"))
            .field("token_type", &self.token_type)
            .field("scope", &self.scope)
            .field("expires_at", &self.expires_at)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_deref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Derive the key CC uses in ~/.claude/.credentials.json for an MCP OAuth token.
///
/// Formula: `serverName|sha256({"type":"<type>","url":"<url>","headers":{}}, compact)[:16 hex chars]`
/// Field order is FIXED (type → url → headers) — wrong order produces a wrong key.
///
/// IMPORTANT: serde_json::json! sorts keys alphabetically, which would produce the wrong hash.
/// We build the compact JSON string manually to guarantee the exact field order CC expects.
pub fn mcp_oauth_key(server_name: &str, server_type: &str, url: &str) -> String {
    // Manual compact JSON construction to guarantee field order: type → url → headers.
    // serde_json::json! sorts keys alphabetically (headers → type → url), producing a wrong key.
    // The type and url values are user-controlled strings — escape them properly.
    let type_escaped = server_type.replace('\\', "\\\\").replace('"', "\\\"");
    let url_escaped = url.replace('\\', "\\\\").replace('"', "\\\"");
    let compact = format!(r#"{{"type":"{type_escaped}","url":"{url_escaped}","headers":{{}}}}"#);

    let hash = Sha256::digest(compact.as_bytes());
    // First 8 bytes = 16 hex chars (D-03)
    let hex_str = hex::encode(&hash[..8]);
    format!("{server_name}|{hex_str}")
}

/// Rotate existing backups and copy current file to .bak.
/// Slot order: .bak.4 (oldest, dropped) ← .bak.3 ← .bak.2 ← .bak.1 ← .bak ← current file
fn rotate_backups(path: &Path) -> Result<(), CredentialError> {
    // Build slot paths in oldest-first order for the shift
    let slots: Vec<PathBuf> = [".bak.4", ".bak.3", ".bak.2", ".bak.1", ".bak"]
        .iter()
        .map(|ext| {
            let mut p = path.to_path_buf();
            let fname = p.file_name().unwrap().to_string_lossy().into_owned();
            p.set_file_name(format!("{fname}{ext}"));
            p
        })
        .collect();

    // slots[0] = .bak.4 (oldest) — drop it
    if slots[0].exists() {
        std::fs::remove_file(&slots[0])
            .map_err(|e| CredentialError::BackupFailed(format!("remove .bak.4: {e}")))?;
    }

    // Shift older slots first to avoid overwriting: .bak.3→.bak.4, .bak.2→.bak.3, ..., .bak→.bak.1
    // Must iterate ascending so we don't clobber a slot before moving it.
    // TOCTOU: under concurrent access a slot may disappear between exists() and rename().
    // Treat ENOENT on rename as benign — another thread already moved it.
    for i in 1..slots.len() {
        if slots[i].exists() {
            match std::fs::rename(&slots[i], &slots[i - 1]) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Another concurrent writer already moved this slot — not an error.
                }
                Err(e) => {
                    return Err(CredentialError::BackupFailed(format!("rename backup slot: {e}")));
                }
            }
        }
    }

    // Copy current file → .bak (last slot after shift)
    let bak_path = {
        let mut p = path.to_path_buf();
        let fname = p.file_name().unwrap().to_string_lossy().into_owned();
        p.set_file_name(format!("{fname}.bak"));
        p
    };
    std::fs::copy(path, &bak_path)
        .map_err(|e| CredentialError::BackupFailed(format!("copy to .bak: {e}")))?;

    Ok(())
}

/// Atomically write JSON value to path using same-dir NamedTempFile + rename.
fn write_json_atomic(path: &Path, value: &serde_json::Value) -> Result<(), CredentialError> {
    let content = serde_json::to_string_pretty(value)?;
    let dir = path.parent().ok_or(CredentialError::InvalidPath)?;
    // CRITICAL: new_in(dir) — same filesystem as target, avoids EXDEV on tmpfs
    let mut tmp = NamedTempFile::new_in(dir)?;
    tmp.write_all(content.as_bytes())?;
    // persist() = rename(2) — atomic, replaces target if it exists
    tmp.persist(path)?;
    Ok(())
}

/// Write an MCP OAuth token to ~/.claude/.credentials.json under the correct CC key.
///
/// - Derives the key using mcp_oauth_key(server_name, "http", server_url) per D-03.
/// - Merges with existing content — never removes other keys (D-08).
/// - Creates a rotating backup before modifying an existing file (D-06).
/// - Skips backup if the file does not yet exist (D-07).
/// - Write is atomic via NamedTempFile::persist() (D-05).
pub fn write_credential(
    credentials_path: &Path,
    server_name: &str,
    server_url: &str,
    token: &CredentialToken,
) -> Result<(), CredentialError> {
    let key = mcp_oauth_key(server_name, "http", server_url);

    // Read existing or start fresh
    let mut root: serde_json::Value = if credentials_path.exists() {
        let content = std::fs::read_to_string(credentials_path)?;
        match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                // Corrupt file: log warning, treat as empty (backup preserved prior good copy)
                tracing::warn!(
                    path = %credentials_path.display(),
                    error = %e,
                    "credentials file is corrupt — treating as empty, prior backup preserved"
                );
                serde_json::json!({})
            }
        }
    } else {
        serde_json::json!({})
    };

    // Ensure root is an object
    if !root.is_object() {
        root = serde_json::json!({});
    }

    // Backup before first modification (D-06, D-07)
    if credentials_path.exists() {
        rotate_backups(credentials_path)?;
    }

    // Upsert the token under the derived key (D-08)
    let token_value = serde_json::to_value(token)?;
    root.as_object_mut()
        .expect("root is always an object after normalization")
        .insert(key, token_value);

    // Atomic write (D-05)
    write_json_atomic(credentials_path, &root)?;

    Ok(())
}

/// Read an MCP OAuth token from ~/.claude/.credentials.json by server name and URL.
///
/// Returns Ok(None) if the file does not exist or the key is absent.
pub fn read_credential(
    credentials_path: &Path,
    server_name: &str,
    server_url: &str,
) -> Result<Option<CredentialToken>, CredentialError> {
    if !credentials_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(credentials_path)?;
    let root: serde_json::Value = serde_json::from_str(&content)?;
    let key = mcp_oauth_key(server_name, "http", server_url);

    match root.get(&key) {
        Some(v) => {
            let token: CredentialToken = serde_json::from_value(v.clone())?;
            Ok(Some(token))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_token(access: &str) -> CredentialToken {
        CredentialToken {
            access_token: access.to_string(),
            refresh_token: None,
            token_type: Some("Bearer".to_string()),
            scope: None,
            expires_at: 0,
            client_id: None,
            client_secret: None,
        }
    }

    // --- REFRESH-04: client_id / client_secret field tests ---

    #[test]
    fn client_id_none_not_serialized() {
        let token = make_token("tok");
        // client_id is None — skip_serializing_if = "Option::is_none" must suppress it
        let json_str = serde_json::to_string(&token).unwrap();
        assert!(!json_str.contains("client_id"), "client_id=None must NOT appear in JSON");
    }

    #[test]
    fn client_id_some_serialized() {
        let token = CredentialToken {
            client_id: Some("cli-abc".to_string()),
            ..make_token("tok")
        };
        let json_str = serde_json::to_string(&token).unwrap();
        assert!(
            json_str.contains(r#""client_id":"cli-abc""#),
            "client_id=Some must appear in JSON; got: {json_str}"
        );
    }

    #[test]
    fn old_json_round_trips_without_client_id() {
        // Simulate a credential written before Phase 35 (no client_id/client_secret fields)
        let json = r#"{"access_token":"t","expiresAt":0}"#;
        let token: CredentialToken = serde_json::from_str(json).unwrap();
        assert!(token.client_id.is_none(), "client_id must be None when absent in JSON");
        assert!(token.client_secret.is_none(), "client_secret must be None when absent in JSON");
    }

    #[test]
    fn debug_redacts_client_secret() {
        let token = CredentialToken {
            client_secret: Some("s3cr3t".to_string()),
            ..make_token("tok")
        };
        let debug_str = format!("{:?}", token);
        assert!(
            debug_str.contains("[REDACTED]"),
            "Debug must contain [REDACTED] for client_secret"
        );
        assert!(
            !debug_str.contains("s3cr3t"),
            "Debug must NOT expose actual client_secret value"
        );
    }

    // --- CRED-01: key formula tests ---
    #[test]
    fn notion_test_vector() {
        assert_eq!(
            mcp_oauth_key("notion", "http", "https://mcp.notion.com/mcp"),
            "notion|eac663db915250e7"
        );
    }

    #[test]
    fn key_is_deterministic() {
        let a = mcp_oauth_key("x", "http", "https://a.com");
        let b = mcp_oauth_key("x", "http", "https://a.com");
        assert_eq!(a, b);
    }

    #[test]
    fn credential_token_serializes_expires_at_camel_case() {
        let token = make_token("tok");
        let json_str = serde_json::to_string(&token).unwrap();
        assert!(json_str.contains("\"expiresAt\""), "must serialize as camelCase expiresAt");
        assert!(!json_str.contains("\"expires_at\""), "must NOT serialize as snake_case");
    }

    // --- CRED-02: write/read/backup tests ---
    #[test]
    fn write_creates_file_on_first_write() {
        let dir = tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");

        write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &make_token("tok1")).unwrap();

        assert!(creds.exists(), ".credentials.json must exist after first write");
        let content = std::fs::read_to_string(&creds).unwrap();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(v.get("notion|eac663db915250e7").is_some(), "key notion|eac663db915250e7 must be present");
    }

    #[test]
    fn write_preserves_unrelated_keys() {
        let dir = tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");
        std::fs::write(
            &creds,
            r#"{"claudeAiOauth": {"token": "existing"}}"#,
        ).unwrap();

        write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &make_token("tok")).unwrap();

        let content = std::fs::read_to_string(&creds).unwrap();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["claudeAiOauth"]["token"], "existing", "claudeAiOauth must survive");
        assert!(v.get("notion|eac663db915250e7").is_some(), "new MCP key must be present");
    }

    #[test]
    fn no_backup_on_first_write() {
        let dir = tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");

        write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &make_token("tok")).unwrap();

        assert!(!dir.path().join(".credentials.json.bak").exists(), "no .bak on first write");
    }

    #[test]
    fn backup_created_on_second_write() {
        let dir = tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");

        write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &make_token("tok1")).unwrap();
        write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &make_token("tok2")).unwrap();

        let bak = dir.path().join(".credentials.json.bak");
        assert!(bak.exists(), ".credentials.json.bak must exist after second write");
    }

    #[test]
    fn backup_rotation_max_five_slots() {
        let dir = tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");

        // 6 writes — should produce .bak through .bak.4 (5 slots), oldest dropped
        for i in 0..6u8 {
            write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &make_token(&format!("tok{i}"))).unwrap();
        }

        assert!(dir.path().join(".credentials.json.bak").exists());
        assert!(dir.path().join(".credentials.json.bak.1").exists());
        assert!(dir.path().join(".credentials.json.bak.2").exists());
        assert!(dir.path().join(".credentials.json.bak.3").exists());
        assert!(dir.path().join(".credentials.json.bak.4").exists());
        // No .bak.5 should exist
        assert!(!dir.path().join(".credentials.json.bak.5").exists());
    }

    #[test]
    fn read_roundtrip() {
        let dir = tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");

        write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &make_token("mytoken")).unwrap();
        let result = read_credential(&creds, "notion", "https://mcp.notion.com/mcp").unwrap();

        assert!(result.is_some(), "read must return Some after write");
        assert_eq!(result.unwrap().access_token, "mytoken");
    }

    #[test]
    fn read_returns_none_for_missing_key() {
        let dir = tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");

        write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &make_token("tok")).unwrap();
        let result = read_credential(&creds, "linear", "https://mcp.linear.app/mcp").unwrap();

        assert!(result.is_none(), "missing key must return None");
    }

    #[test]
    fn read_returns_none_when_file_absent() {
        let dir = tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");

        let result = read_credential(&creds, "notion", "https://mcp.notion.com/mcp").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn concurrent_writes_produce_valid_json() {
        use std::sync::Arc;
        let dir = tempdir().unwrap();
        let creds = Arc::new(dir.path().join(".credentials.json"));

        // Write initial file
        write_credential(&creds, "notion", "https://mcp.notion.com/mcp", &make_token("init")).unwrap();

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let creds = Arc::clone(&creds);
                std::thread::spawn(move || {
                    write_credential(
                        &creds,
                        "notion",
                        "https://mcp.notion.com/mcp",
                        &make_token(&format!("tok{i}")),
                    )
                    .unwrap();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // File must be valid JSON after all concurrent writes
        let content = std::fs::read_to_string(&*creds).unwrap();
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&content);
        // If this panics, the concurrent writes corrupted the file
        assert!(parsed.is_ok(), "concurrent writes must not corrupt the file");
    }
}
