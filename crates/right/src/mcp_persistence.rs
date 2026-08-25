//! `right-mcp` persistence adapter over the Aggregator's [`AgentDbOwner`].
//!
//! Runtime proxy/reconnect/refresh code never opens `data.db`. Each operation
//! is executed inside the agent owner's serialized connection and preserves
//! the transaction boundaries in `right_mcp::credentials`.

use std::sync::Arc;

use right_mcp::persistence::{BoxFuture, McpPersistError, McpPersistence, OAuthStatePersist};

use crate::db_owner::AgentDbOwner;

#[derive(Debug, Clone)]
pub(crate) struct OwnerMcpPersistence {
    owner: Arc<AgentDbOwner>,
}

impl OwnerMcpPersistence {
    pub(crate) fn new(owner: Arc<AgentDbOwner>) -> Self {
        Self { owner }
    }
}

fn map_owner_error(error: crate::db_owner::DbOwnerError) -> McpPersistError {
    match error {
        crate::db_owner::DbOwnerError::Unavailable { .. }
        | crate::db_owner::DbOwnerError::NotOpened { .. }
        | crate::db_owner::DbOwnerError::NotFound { .. }
        | crate::db_owner::DbOwnerError::DrainTimeout { .. } => {
            McpPersistError::Unavailable(format!("{error:#}"))
        }
        _ => McpPersistError::Operation(format!("{error:#}")),
    }
}

impl McpPersistence for OwnerMcpPersistence {
    fn update_instructions(
        &self,
        server: &str,
        instructions: Option<&str>,
    ) -> BoxFuture<'static, Result<(), McpPersistError>> {
        let owner = Arc::clone(&self.owner);
        let server = server.to_owned();
        let instructions = instructions.map(str::to_owned);
        Box::pin(async move {
            let _mutation_guard = owner.lock_mcp_mutation(&server).await;
            owner
                .with_connection(move |connection| {
                    Box::pin(async move {
                        right_mcp::credentials::db_update_instructions(
                            connection,
                            &server,
                            instructions.as_deref(),
                        )
                        .await
                        .map_err(Into::into)
                    })
                })
                .await
                .map_err(map_owner_error)
        })
    }

    fn set_oauth_state(
        &self,
        server: &str,
        state: OAuthStatePersist,
    ) -> BoxFuture<'static, Result<(), McpPersistError>> {
        let owner = Arc::clone(&self.owner);
        let server = server.to_owned();
        Box::pin(async move {
            let _mutation_guard = owner.lock_mcp_mutation(&server).await;
            owner
                .with_connection(move |connection| {
                    Box::pin(async move {
                        right_mcp::credentials::db_set_oauth_state(
                            connection,
                            &server,
                            &state.access_token,
                            state.refresh_token.as_deref(),
                            &state.token_endpoint,
                            &state.client_id,
                            state.client_secret.as_deref(),
                            &state.expires_at,
                            &state.oauth_resource,
                        )
                        .await
                        .map_err(Into::into)
                    })
                })
                .await
                .map_err(map_owner_error)
        })
    }

    fn update_oauth_token(
        &self,
        server: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_at: &str,
    ) -> BoxFuture<'static, Result<(), McpPersistError>> {
        let owner = Arc::clone(&self.owner);
        let server = server.to_owned();
        let access_token = access_token.to_owned();
        let refresh_token = refresh_token.map(str::to_owned);
        let expires_at = expires_at.to_owned();
        Box::pin(async move {
            let _mutation_guard = owner.lock_mcp_mutation(&server).await;
            owner
                .with_connection(move |connection| {
                    Box::pin(async move {
                        right_mcp::credentials::db_update_oauth_token(
                            connection,
                            &server,
                            &access_token,
                            refresh_token.as_deref(),
                            &expires_at,
                        )
                        .await
                        .map_err(Into::into)
                    })
                })
                .await
                .map_err(map_owner_error)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn owner_persistence_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Arc::new(AgentDbOwner::starting("alpha", dir.path().to_path_buf()));
        owner.open_and_migrate().await.unwrap();
        owner
            .with_connection(|connection| {
                Box::pin(async move {
                    right_mcp::credentials::db_add_server(
                        connection,
                        "srv",
                        "https://example.com/mcp",
                    )
                    .await
                    .map_err(Into::into)
                })
            })
            .await
            .unwrap();
        let persistence = OwnerMcpPersistence::new(Arc::clone(&owner));

        persistence
            .update_instructions("srv", Some("instructions"))
            .await
            .unwrap();
        let instructions = owner
            .with_connection(|connection| {
                Box::pin(async move {
                    right_mcp::credentials::db_list_servers(connection)
                        .await
                        .map(|servers| servers[0].instructions.clone())
                        .map_err(Into::into)
                })
            })
            .await
            .unwrap();
        assert_eq!(instructions.as_deref(), Some("instructions"));
    }
}
