use anyhow::{Context as _, Result};
use tempfile::TempDir;

use super::*;
use crate::REDACTION_SENTINEL;

/// A fresh store on a temp home. Returned together with the `TempDir` so the
/// directory outlives the store.
async fn store() -> Result<(TempDir, ProviderStore)> {
    let home = TempDir::new().context("create temp home")?;
    let store = ProviderStore::open(home.path())
        .await
        .context("open providers.db")?;
    Ok((home, store))
}

fn builtin(owner: &str, name: &str) -> NewProvider {
    NewProvider {
        owner_agent: owner.into(),
        name: name.into(),
        kind: ProviderKind::Builtin("right-fal".into()),
        label: "prod".into(),
    }
}

fn generic(owner: &str, name: &str, env_var: &str) -> NewProvider {
    NewProvider {
        owner_agent: owner.into(),
        name: name.into(),
        kind: ProviderKind::Generic(GenericSpec {
            env_var: env_var.into(),
            upstream_hosts: vec!["api.example.com".into()],
            upstream_path_prefix: Some("/v1".into()),
        }),
        label: "prod".into(),
    }
}

fn cred(value: &str) -> Credential {
    Credential::from(value.to_string())
}

#[tokio::test]
async fn open_is_idempotent_and_the_file_is_owner_only() -> Result<()> {
    let home = TempDir::new()?;
    let first = ProviderStore::open(home.path()).await?;
    let db_path = first.db_path().to_path_buf();
    drop(first);
    let second = ProviderStore::open(home.path()).await?;
    assert_eq!(second.db_path(), db_path);
    assert!(second.list("riskoff").await?.is_empty());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&db_path)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "providers.db holds plaintext credentials");
    }
    Ok(())
}

#[tokio::test]
async fn create_then_get_returns_a_ready_record_without_a_credential() -> Result<()> {
    let (_home, store) = store().await?;
    let created = store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("real-value"))
        .await?;
    assert_eq!(created.name, "fal-a1b2c3");
    assert_eq!(created.owner_agent, "riskoff");
    assert_eq!(created.env_var, "FAL_KEY");
    assert_eq!(created.status, ProviderStatus::Ready);
    assert!(created.borrower_agent.is_none());

    let fetched = store.get("riskoff", "fal-a1b2c3").await?;
    assert_eq!(fetched, created);
    assert!(
        !format!("{fetched:?}").contains("real-value"),
        "no credential value may reach a read API"
    );
    Ok(())
}

#[tokio::test]
async fn create_rejects_a_duplicate_name_and_a_duplicate_env_var() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("v1"))
        .await?;

    let err = store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("v2"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::NameCollision { name } if name == "fal-a1b2c3"),
        "got {err:?}"
    );

    let err = store
        .create(builtin("riskoff", "fal-d4e5f6"), cred("v2"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::EnvVarCollision { env_var } if env_var == "FAL_KEY"),
        "got {err:?}"
    );
    Ok(())
}

#[tokio::test]
async fn create_rejects_a_reserved_type_and_a_bare_generic_slug() -> Result<()> {
    let (_home, store) = store().await?;
    let mut reserved = builtin("riskoff", "claude-a1b2c3");
    reserved.kind = ProviderKind::Builtin("claude".into());
    assert!(store.create(reserved, cred("v")).await.is_err());

    let mut bare_generic = builtin("riskoff", "generic-a1b2c3");
    bare_generic.kind = ProviderKind::Builtin(GENERIC_SLUG.into());
    let err = store.create(bare_generic, cred("v")).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::InvalidName { reason, .. }
            if reason == "type \"generic\" requires a generic definition"),
        "got {err:?}"
    );
    Ok(())
}

/// `Credential::absent()` is how the migration seeds a provider it can define
/// but not fill in, so it must land in exactly the awaiting-credential state
/// the dashboard renders and the bot's bring-up skips.
#[tokio::test]
async fn a_credential_less_record_reports_needs_value() -> Result<()> {
    let (_home, store) = store().await?;
    let created = store
        .create(builtin("riskoff", "fal-a1b2c3"), Credential::absent())
        .await?;
    assert_eq!(created.status, ProviderStatus::NeedsValue);
    assert!(
        store
            .source_ref_binding("riskoff", "fal-a1b2c3")
            .await
            .is_err(),
        "a record awaiting its credential must never produce a binding"
    );

    store
        .rotate("riskoff", "fal-a1b2c3", cred("real-value"))
        .await?;
    assert_eq!(
        store.get("riskoff", "fal-a1b2c3").await?.status,
        ProviderStatus::Ready
    );
    Ok(())
}

#[tokio::test]
async fn rotate_and_remove_report_not_found_for_an_unknown_record() -> Result<()> {
    let (_home, store) = store().await?;
    let err = store
        .rotate("riskoff", "fal-a1b2c3", cred("v"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::NotFound { name } if name == "fal-a1b2c3"),
        "got {err:?}"
    );
    assert!(store.remove("riskoff", "fal-a1b2c3").await.is_err());
    Ok(())
}

#[tokio::test]
async fn update_generic_is_owner_only_generic_only_and_env_var_stable() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(
            generic("riskoff", "generic-a1b2c3", "EXAMPLE_KEY"),
            cred("v"),
        )
        .await?;
    store
        .create(builtin("riskoff", "fal-d4e5f6"), cred("v"))
        .await?;

    // Built-ins have no endpoints to edit.
    let err = store
        .update_generic(
            "riskoff",
            "fal-d4e5f6",
            GenericSpec {
                env_var: "FAL_KEY".into(),
                upstream_hosts: vec!["fal.run".into()],
                upstream_path_prefix: None,
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::InvalidName { reason, .. }
            if reason == "config-update only valid on generic providers"),
        "got {err:?}"
    );

    // Rebinding to a different env var is a create, not a config edit.
    let err = store
        .update_generic(
            "riskoff",
            "generic-a1b2c3",
            GenericSpec {
                env_var: "OTHER_KEY".into(),
                upstream_hosts: vec!["api.example.com".into()],
                upstream_path_prefix: None,
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::GenericEnvVarChangeRequiresCredential { name }
            if name == "generic-a1b2c3"),
        "got {err:?}"
    );

    // Hosts are normalized and the cleared prefix sticks.
    store
        .update_generic(
            "riskoff",
            "generic-a1b2c3",
            GenericSpec {
                env_var: "EXAMPLE_KEY".into(),
                upstream_hosts: vec![
                    " api.example.com ".into(),
                    "cdn.example.com".into(),
                    "api.example.com".into(),
                ],
                upstream_path_prefix: None,
            },
        )
        .await?;
    let record = store.get("riskoff", "generic-a1b2c3").await?;
    let spec = record.kind.generic().expect("generic record");
    assert_eq!(
        spec.upstream_hosts,
        vec!["api.example.com", "cdn.example.com"]
    );
    assert_eq!(spec.upstream_path_prefix, None);
    Ok(())
}

#[tokio::test]
async fn share_creates_a_read_only_borrowed_reference() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("real-value"))
        .await?;

    let borrowed = store.share("riskoff", "fal-a1b2c3", "right").await?;
    assert_eq!(borrowed.owner_agent, "riskoff");
    assert_eq!(borrowed.borrower_agent.as_deref(), Some("right"));
    assert_eq!(borrowed.holder_agent(), "right");
    assert_eq!(borrowed.env_var, "FAL_KEY");

    for err in [
        store
            .rotate("right", "fal-a1b2c3", cred("v2"))
            .await
            .unwrap_err(),
        store.remove("right", "fal-a1b2c3").await.unwrap_err(),
    ] {
        assert!(
            matches!(&err, StoreError::BorrowedReadOnly { name, owner }
                if name == "fal-a1b2c3" && owner == "riskoff"),
            "got {err:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn resharing_points_at_the_true_owner_not_the_intermediary() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("real-value"))
        .await?;
    store.share("riskoff", "fal-a1b2c3", "right").await?;

    let third = store.share("right", "fal-a1b2c3", "scout").await?;
    assert_eq!(
        third.owner_agent, "riskoff",
        "a re-share resolves to the owning agent, never the intermediary"
    );
    Ok(())
}

#[tokio::test]
async fn share_rejects_self_and_duplicate_destinations() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("v"))
        .await?;

    let err = store
        .share("riskoff", "fal-a1b2c3", "riskoff")
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::ShareConflict { .. }),
        "got {err:?}"
    );

    store.share("riskoff", "fal-a1b2c3", "right").await?;
    let err = store
        .share("riskoff", "fal-a1b2c3", "right")
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::ShareConflict { .. }),
        "got {err:?}"
    );
    Ok(())
}

#[tokio::test]
async fn unshare_drops_the_reference_and_never_the_record() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("v"))
        .await?;
    store.share("riskoff", "fal-a1b2c3", "right").await?;

    store.unshare("right", "fal-a1b2c3").await?;
    assert!(store.list("right").await?.is_empty());
    assert_eq!(
        store.list("riskoff").await?.len(),
        1,
        "owner keeps the record"
    );

    let err = store.unshare("riskoff", "fal-a1b2c3").await.unwrap_err();
    assert!(
        matches!(&err, StoreError::ShareConflict { reason }
            if reason.contains("use remove, not unshare")),
        "got {err:?}"
    );
    Ok(())
}

#[tokio::test]
async fn owner_removal_re_homes_to_a_surviving_borrower() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("real-value"))
        .await?;
    store.share("riskoff", "fal-a1b2c3", "alpha").await?;
    store.share("riskoff", "fal-a1b2c3", "beta").await?;

    store.remove("riskoff", "fal-a1b2c3").await?;

    assert!(store.list("riskoff").await?.is_empty(), "owner detached");
    let alpha = store.get("alpha", "fal-a1b2c3").await?;
    assert_eq!(
        alpha.owner_agent, "alpha",
        "first survivor becomes the owner"
    );
    assert!(alpha.is_owned());
    let beta = store.get("beta", "fal-a1b2c3").await?;
    assert_eq!(
        beta.owner_agent, "alpha",
        "remaining borrower follows the new owner"
    );
    assert_eq!(beta.borrower_agent.as_deref(), Some("beta"));

    // The credential survived the re-home: the new owner can bind it.
    assert_eq!(alpha.status, ProviderStatus::Ready);
    Ok(())
}

#[tokio::test]
async fn owner_removal_deletes_the_record_when_nobody_borrows_it() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("v"))
        .await?;
    store.remove("riskoff", "fal-a1b2c3").await?;
    assert!(store.list("riskoff").await?.is_empty());
    assert!(store.get("riskoff", "fal-a1b2c3").await.is_err());
    Ok(())
}

#[tokio::test]
async fn list_returns_owned_then_borrowed() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("v"))
        .await?;
    store
        .create(generic("other", "generic-d4e5f6", "EXAMPLE_KEY"), cred("v"))
        .await?;
    store.share("other", "generic-d4e5f6", "riskoff").await?;

    let listed = store.list("riskoff").await?;
    let shape: Vec<(&str, Option<&str>)> = listed
        .iter()
        .map(|r| (r.name.as_str(), r.borrower_agent.as_deref()))
        .collect();
    assert_eq!(
        shape,
        vec![("fal-a1b2c3", None), ("generic-d4e5f6", Some("riskoff"))]
    );
    Ok(())
}

#[tokio::test]
async fn source_ref_binding_carries_names_and_hosts_only() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(builtin("riskoff", "fal-a1b2c3"), cred("real-value"))
        .await?;

    let binding = store.source_ref_binding("riskoff", "fal-a1b2c3").await?;
    assert_eq!(binding.env_var, "FAL_KEY");
    assert_eq!(binding.source_env_var, "RIGHT_PROVIDER_FAL_A1B2C3");
    assert_eq!(binding.placeholder, "$MSB_FAL_KEY");
    assert_eq!(
        binding.allowed_hosts,
        vec!["fal.run", "queue.fal.run", "rest.fal.ai"]
    );
    assert!(!binding.inject_query, "query injection is opt-in per entry");

    let rendered = format!("{binding:?}");
    assert!(
        !rendered.contains("real-value"),
        "a binding must never carry the value: {rendered}"
    );

    // The value is resolvable by the spawning process under the source name,
    // which is the whole point of a source-ref secret.
    assert_eq!(
        std::env::var("RIGHT_PROVIDER_FAL_A1B2C3").ok().as_deref(),
        Some("real-value")
    );
    // Leave the process environment as we found it (AGENTS.rust.md §5).
    crate::store::remove_source_value("RIGHT_PROVIDER_FAL_A1B2C3");
    Ok(())
}

#[tokio::test]
async fn a_borrower_can_bind_the_owner_s_credential() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(
            generic("riskoff", "generic-b0rr0w", "BORROW_KEY"),
            cred("shared-value"),
        )
        .await?;
    store.share("riskoff", "generic-b0rr0w", "right").await?;

    let binding = store.source_ref_binding("right", "generic-b0rr0w").await?;
    assert_eq!(binding.env_var, "BORROW_KEY");
    assert_eq!(binding.allowed_hosts, vec!["api.example.com"]);
    crate::store::remove_source_value("RIGHT_PROVIDER_GENERIC_B0RR0W");
    Ok(())
}

#[tokio::test]
async fn an_unusable_credential_is_a_hard_error_not_an_empty_binding() -> Result<()> {
    let (_home, store) = store().await?;
    store
        .create(generic("riskoff", "generic-empty1", "EMPTY_KEY"), cred(""))
        .await?;
    let err = store
        .source_ref_binding("riskoff", "generic-empty1")
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::SourceCredentialUnreadable { source_provider }
            if source_provider == "generic-empty1"),
        "got {err:?}"
    );

    store
        .create(
            generic("riskoff", "generic-redact", "REDACT_KEY"),
            cred(REDACTION_SENTINEL),
        )
        .await?;
    let err = store
        .source_ref_binding("riskoff", "generic-redact")
        .await
        .unwrap_err();
    assert!(
        matches!(err, StoreError::SourceCredentialUnreadable { .. }),
        "the redaction sentinel is never a usable credential"
    );
    Ok(())
}

#[test]
fn source_env_var_is_deterministic_and_shell_safe() {
    assert_eq!(source_env_var("fal-a1b2c3"), "RIGHT_PROVIDER_FAL_A1B2C3");
    assert_eq!(
        source_env_var("riskoff-generic-1"),
        "RIGHT_PROVIDER_RISKOFF_GENERIC_1"
    );
}

#[tokio::test]
async fn the_per_agent_lock_serializes_callers() -> Result<()> {
    let (_home, store) = store().await?;
    let store = std::sync::Arc::new(store);

    let first = store.agent_lock("riskoff").await;
    let contender = tokio::spawn({
        let store = std::sync::Arc::clone(&store);
        async move {
            let _guard = store.agent_lock("riskoff").await;
        }
    });
    // A different agent is never blocked by this guard.
    let _other = store.agent_lock("right").await;
    assert!(!contender.is_finished(), "same-agent callers queue up");

    drop(first);
    contender.await.context("contender task")?;
    Ok(())
}
