//! Shared fail-closed guard for offline database commands.

use std::path::Path;

pub(crate) async fn require_runtime_quiesced(
    home: &Path,
) -> miette::Result<right_agent::runtime::RuntimeExclusionGuard> {
    right_agent::runtime::require_runtime_quiesced(home).await
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn absent_state_is_quiesced() {
        let home = tempfile::tempdir().unwrap();
        require_runtime_quiesced(home.path()).await.unwrap();
    }
    #[tokio::test]
    async fn retained_state_fails_closed() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("run")).unwrap();
        std::fs::write(
            home.path().join("run/state.json"),
            r#"{"agents":[],"socket_path":"","started_at":"x","pc_port":1,"pc_api_token":"x"}"#,
        )
        .unwrap();
        assert!(require_runtime_quiesced(home.path()).await.is_err());
    }
}
