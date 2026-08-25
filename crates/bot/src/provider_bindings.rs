//! Authenticated provider binding resolution over the internal Unix socket.

use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct ProviderBindingResolver {
    client: Arc<right_mcp::internal_client::InternalClient>,
    agent: String,
    auth: secrecy::SecretString,
}

impl std::fmt::Debug for ProviderBindingResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderBindingResolver")
            .field("agent", &self.agent)
            .field("auth", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl ProviderBindingResolver {
    pub(crate) fn new(
        client: Arc<right_mcp::internal_client::InternalClient>,
        agent: impl Into<String>,
        agent_secret_b64: &str,
    ) -> miette::Result<Self> {
        let auth = right_mcp::internal_db::provider_binding_token(agent_secret_b64)?;
        Ok(Self {
            client,
            agent: agent.into(),
            auth: secrecy::SecretString::from(auth),
        })
    }

    pub(crate) async fn resolve_all(&self) -> miette::Result<Vec<right_sandbox::SecretBinding>> {
        let response = self
            .client
            .resolve_provider_bindings(&right_mcp::internal_db::ResolveProviderBindingsRequest {
                agent: self.agent.clone(),
                auth: self.auth.clone(),
            })
            .await
            .map_err(|error| miette::miette!("resolve provider bindings: {error:#}"))?;
        Ok(response
            .bindings
            .into_iter()
            .map(binding_from_dto)
            .collect())
    }

    pub(crate) async fn resolve_named(
        &self,
        provider: &str,
    ) -> miette::Result<right_sandbox::SecretBinding> {
        let response = self
            .client
            .resolve_named_provider_binding(
                &right_mcp::internal_db::ResolveNamedProviderBindingRequest {
                    agent: self.agent.clone(),
                    provider: provider.to_owned(),
                    auth: self.auth.clone(),
                },
            )
            .await
            .map_err(|error| miette::miette!("resolve provider binding {provider}: {error:#}"))?;
        Ok(binding_from_dto(response.binding))
    }
}

fn binding_from_dto(dto: right_mcp::internal_db::SecretBindingDto) -> right_sandbox::SecretBinding {
    let right_mcp::internal_db::SecretBindingDto {
        provider: _,
        env_var,
        source_env_var,
        placeholder,
        allowed_hosts,
        inject_query,
        value,
    } = dto;
    right_sandbox::SecretBinding::from_resolved_parts(
        env_var,
        source_env_var,
        placeholder,
        allowed_hosts,
        inject_query,
        value,
    )
}
