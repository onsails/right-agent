//! Construction of complete, unpublished per-agent Aggregator runtimes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use right_mcp::proxy::{AuthMethod, BackendStatus, ProxyBackend};
use right_mcp::refresh::{OAuthServerState, RefreshMessage};

use crate::aggregator::{BackendRegistry, HindsightBackend};
use crate::db_owner::{AgentDbOwner, AgentRuntimeBundle};

const REFRESH_CHANNEL_CAPACITY: usize = 32;
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PARTIAL_BUILD_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct BuiltAgentRuntime {
    pub(crate) bundle: Arc<AgentRuntimeBundle>,
    pub(crate) registry: BackendRegistry,
    pub(crate) refresh_sender: tokio::sync::mpsc::Sender<RefreshMessage>,
    pub(crate) reconnect_manager: right_mcp::reconnect::ReconnectManager,
}

#[cfg(test)]
pub(crate) struct RuntimeBuildTestHook {
    pub(crate) fail_after_tasks: bool,
    pub(crate) task_dropped: Option<Arc<std::sync::atomic::AtomicBool>>,
}

pub(crate) async fn build_agent_runtime(
    agent_name: &str,
    agent_dir: PathBuf,
    agents_dir: &Path,
    providers: Arc<right_providers::ProviderStore>,
    http_client: reqwest::Client,
) -> anyhow::Result<BuiltAgentRuntime> {
    build_agent_runtime_impl(
        agent_name,
        agent_dir,
        agents_dir,
        providers,
        http_client,
        #[cfg(test)]
        None,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn build_agent_runtime_with_test_hook(
    agent_name: &str,
    agent_dir: PathBuf,
    agents_dir: &Path,
    providers: Arc<right_providers::ProviderStore>,
    http_client: reqwest::Client,
    hook: RuntimeBuildTestHook,
) -> anyhow::Result<BuiltAgentRuntime> {
    build_agent_runtime_impl(
        agent_name,
        agent_dir,
        agents_dir,
        providers,
        http_client,
        Some(hook),
    )
    .await
}

async fn build_agent_runtime_impl(
    agent_name: &str,
    agent_dir: PathBuf,
    agents_dir: &Path,
    providers: Arc<right_providers::ProviderStore>,
    http_client: reqwest::Client,
    #[cfg(test)] test_hook: Option<RuntimeBuildTestHook>,
) -> anyhow::Result<BuiltAgentRuntime> {
    let owner = Arc::new(AgentDbOwner::starting(agent_name, agent_dir.clone()));
    owner
        .open_and_migrate()
        .await
        .with_context(|| format!("open database owner for {agent_name}"))?;
    let bundle = Arc::new(AgentRuntimeBundle::new(Arc::clone(&owner)));

    let result = assemble_initialized_runtime(
        agent_name,
        &agent_dir,
        agents_dir,
        providers,
        http_client,
        Arc::clone(&owner),
        Arc::clone(&bundle),
        #[cfg(test)]
        test_hook.as_ref(),
    )
    .await;

    match result {
        Ok(runtime) => Ok(runtime),
        Err((error, mut reconnect_manager)) => {
            reconnect_manager.cancel_all();
            bundle
                .drain(PARTIAL_BUILD_DRAIN_TIMEOUT)
                .await
                .with_context(|| {
                    format!(
                        "cleanup partial runtime for {agent_name} after build failure: {error:#}"
                    )
                })?;
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn assemble_initialized_runtime(
    agent_name: &str,
    agent_dir: &Path,
    agents_dir: &Path,
    providers: Arc<right_providers::ProviderStore>,
    http_client: reqwest::Client,
    owner: Arc<AgentDbOwner>,
    bundle: Arc<AgentRuntimeBundle>,
    #[cfg(test)] test_hook: Option<&RuntimeBuildTestHook>,
) -> Result<BuiltAgentRuntime, (anyhow::Error, right_mcp::reconnect::ReconnectManager)> {
    let persistence: Arc<dyn right_mcp::persistence::McpPersistence> = Arc::new(
        crate::mcp_persistence::OwnerMcpPersistence::new(Arc::clone(&owner)),
    );
    let agent_config = match right_agent::agent::discovery::parse_agent_config(agent_dir) {
        Ok(config) => config,
        Err(error) => {
            let (refresh_sender, _) = tokio::sync::mpsc::channel(REFRESH_CHANNEL_CAPACITY);
            let manager = right_mcp::reconnect::ReconnectManager::new(
                refresh_sender,
                Arc::clone(&persistence),
            );
            return Err((anyhow!(error).context("parse agent config"), manager));
        }
    };

    let restored_servers = match owner.restore_mcp_servers().await {
        Ok(servers) => servers,
        Err(error) => {
            let (refresh_sender, _) = tokio::sync::mpsc::channel(REFRESH_CHANNEL_CAPACITY);
            let manager = right_mcp::reconnect::ReconnectManager::new(
                refresh_sender,
                Arc::clone(&persistence),
            );
            return Err((anyhow!(error).context("restore MCP servers"), manager));
        }
    };

    let mut proxies = HashMap::new();
    let mut oauth_entries = HashMap::new();
    let mut oauth_server_names = HashSet::new();
    for server in restored_servers {
        let http_headers = if server.auth_type.as_deref() == Some("headers") {
            match owner.restore_http_headers(&server.name).await {
                Ok(headers) => Some(headers),
                Err(error) => {
                    let (refresh_sender, _) = tokio::sync::mpsc::channel(REFRESH_CHANNEL_CAPACITY);
                    let manager = right_mcp::reconnect::ReconnectManager::new(
                        refresh_sender,
                        Arc::clone(&persistence),
                    );
                    return Err((anyhow!(error).context("restore MCP HTTP headers"), manager));
                }
            }
        } else {
            None
        };
        let auth_method = restored_mcp_auth_method(
            server.auth_type.as_deref(),
            server.auth_header.as_deref(),
            http_headers,
        );
        let needs_auth = auth_method.is_none();
        let token = Arc::new(tokio::sync::RwLock::new(server.auth_token.clone()));

        if server.auth_type.as_deref() == Some("oauth") {
            oauth_server_names.insert(server.name.clone());
            if let (Some(token_endpoint), Some(client_id), Some(expires_at)) = (
                server.token_endpoint.as_ref(),
                server.client_id.as_ref(),
                server.expires_at.as_ref(),
            ) {
                let expires_at = chrono::DateTime::parse_from_rfc3339(expires_at)
                    .map(|value| value.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());
                oauth_entries.insert(
                    server.name.clone(),
                    (
                        OAuthServerState {
                            refresh_token: server.refresh_token.clone(),
                            token_endpoint: token_endpoint.clone(),
                            client_id: client_id.clone(),
                            client_secret: server.client_secret.clone(),
                            expires_at,
                            server_url: server.url.clone(),
                            resource: server
                                .oauth_resource
                                .as_deref()
                                .filter(|resource| !resource.trim().is_empty())
                                .map(ToOwned::to_owned)
                                .unwrap_or_else(|| {
                                    right_mcp::oauth::canonical_resource_uri(&server.url)
                                        .unwrap_or_else(|_| server.url.clone())
                                }),
                        },
                        Arc::clone(&token),
                    ),
                );
            }
        }

        let backend = Arc::new(ProxyBackend::new(
            server.name.clone(),
            Arc::clone(&persistence),
            server.url,
            token,
            auth_method.unwrap_or_default(),
        ));
        if needs_auth {
            backend.set_status(BackendStatus::NeedsAuth).await;
        }
        proxies.insert(server.name, backend);
    }

    let hindsight = build_hindsight(agent_name, agent_config.as_ref(), Arc::clone(&owner));
    let proxies = Arc::new(tokio::sync::RwLock::new(proxies));
    let registry = BackendRegistry {
        right: crate::right_backend::RightBackend::new(agents_dir.to_path_buf(), Some(providers))
            .with_db_owner(owner),
        proxies: Arc::clone(&proxies),
        agent_dir: agent_dir.to_path_buf(),
        hindsight,
    };

    let (refresh_sender, refresh_receiver) = tokio::sync::mpsc::channel(REFRESH_CHANNEL_CAPACITY);
    bundle
        .spawn(right_mcp::refresh::run_refresh_scheduler(
            Arc::clone(&persistence),
            refresh_receiver,
        ))
        .await;
    bundle
        .spawn(right_mcp::health::run_health_reconciler(
            Arc::clone(&proxies),
            http_client.clone(),
        ))
        .await;

    #[cfg(test)]
    if let Some(dropped) = test_hook.and_then(|hook| hook.task_dropped.as_ref()) {
        let dropped = Arc::clone(dropped);
        bundle
            .spawn(async move {
                struct DropProbe(Arc<std::sync::atomic::AtomicBool>);
                impl Drop for DropProbe {
                    fn drop(&mut self) {
                        self.0.store(true, std::sync::atomic::Ordering::Release);
                    }
                }
                let _probe = DropProbe(dropped);
                std::future::pending::<()>().await;
            })
            .await;
        tokio::task::yield_now().await;
    }

    let proxies_snapshot: Vec<_> = proxies
        .read()
        .await
        .iter()
        .map(|(name, backend)| (name.clone(), Arc::clone(backend)))
        .collect();
    let mut reconnect_manager = right_mcp::reconnect::ReconnectManager::new(
        refresh_sender.clone(),
        Arc::clone(&persistence),
    );

    for (name, (state, token)) in &oauth_entries {
        if state.refresh_token.is_some()
            && right_mcp::refresh::refresh_due_in(state) != Duration::ZERO
        {
            let Some((_, backend)) = proxies_snapshot.iter().find(|(server, _)| server == name)
            else {
                return Err((
                    anyhow!("OAuth server '{name}' has no restored proxy backend"),
                    reconnect_manager,
                ));
            };
            if let Err(error) = refresh_sender
                .send(RefreshMessage::NewEntry {
                    server_name: name.clone(),
                    state: state.clone(),
                    token: Arc::clone(token),
                    backend: Arc::clone(backend),
                })
                .await
            {
                return Err((
                    anyhow!(error).context("schedule restored OAuth refresh"),
                    reconnect_manager,
                ));
            }
        }
    }

    for (server_name, backend) in proxies_snapshot {
        if backend.status().await == BackendStatus::NeedsAuth {
            continue;
        }
        if let Some((oauth_state, token)) = oauth_entries.get(&server_name) {
            if right_mcp::refresh::refresh_due_in(oauth_state) == Duration::ZERO {
                if oauth_state.refresh_token.is_some() {
                    let handle = reconnect_manager.start_reconnect(
                        server_name.clone(),
                        Arc::clone(&backend),
                        oauth_state.clone(),
                        Arc::clone(token),
                        http_client.clone(),
                    );
                    bundle
                        .spawn(log_reconnect_result(
                            agent_name.to_owned(),
                            server_name,
                            handle,
                        ))
                        .await;
                } else {
                    backend.set_status(BackendStatus::NeedsAuth).await;
                }
            } else {
                bundle
                    .spawn(connect_backend(
                        agent_name.to_owned(),
                        server_name,
                        backend,
                        http_client.clone(),
                    ))
                    .await;
            }
        } else if oauth_server_names.contains(&server_name) {
            backend.set_status(BackendStatus::NeedsAuth).await;
        } else {
            bundle
                .spawn(connect_backend(
                    agent_name.to_owned(),
                    server_name,
                    backend,
                    http_client.clone(),
                ))
                .await;
        }
    }

    #[cfg(test)]
    if test_hook.is_some_and(|hook| hook.fail_after_tasks) {
        return Err((
            anyhow!("injected late runtime build failure"),
            reconnect_manager,
        ));
    }

    Ok(BuiltAgentRuntime {
        bundle,
        registry,
        refresh_sender,
        reconnect_manager,
    })
}

async fn connect_backend(
    agent_name: String,
    server_name: String,
    backend: Arc<ProxyBackend>,
    http_client: reqwest::Client,
) {
    if let Err(error) = backend.connect(http_client).await {
        tracing::warn!(
            agent = agent_name.as_str(),
            server = server_name.as_str(),
            "startup reconnect failed: {error:#}",
        );
    }
}

async fn log_reconnect_result(
    agent_name: String,
    server_name: String,
    handle: tokio::task::JoinHandle<Result<(), right_mcp::reconnect::ReconnectError>>,
) {
    match handle.await {
        Ok(Ok(())) => {}
        Ok(Err(right_mcp::reconnect::ReconnectError::Cancelled)) => {}
        Ok(Err(error)) => tracing::warn!(
            agent = agent_name.as_str(),
            server = server_name.as_str(),
            "startup OAuth reconnect failed: {error:#}",
        ),
        Err(error) if error.is_cancelled() => {}
        Err(error) => tracing::error!(
            agent = agent_name.as_str(),
            server = server_name.as_str(),
            "startup OAuth reconnect task failed: {error:#}",
        ),
    }
}

fn build_hindsight(
    agent_name: &str,
    agent_config: Option<&right_agent::agent::types::AgentConfig>,
    owner: Arc<AgentDbOwner>,
) -> Option<Arc<HindsightBackend>> {
    let memory = agent_config?.memory.as_ref()?;
    if memory.provider != right_agent::agent::types::MemoryProvider::Hindsight {
        return None;
    }
    let api_key = std::env::var("HINDSIGHT_API_KEY")
        .ok()
        .or_else(|| memory.api_key.clone());
    let Some(api_key) = api_key else {
        tracing::warn!(
            agent = agent_name,
            "Hindsight provider configured without an API key; memory tools disabled"
        );
        return None;
    };
    let bank_id = memory.bank_id.as_deref().unwrap_or(agent_name);
    let budget = memory.recall_budget.to_string();
    let client = right_memory::hindsight::HindsightClient::new(
        &api_key,
        bank_id,
        &budget,
        memory.recall_max_tokens,
        None,
    );
    let wrapper = Arc::new(right_memory::ResilientHindsight::new(
        client,
        Arc::new(crate::retain_owner::OwnerRetainQueue::new(owner)),
        "aggregator",
    ));
    Some(Arc::new(HindsightBackend::new(wrapper)))
}

pub(crate) fn restored_mcp_auth_method(
    auth_type: Option<&str>,
    auth_header: Option<&str>,
    http_headers: Option<Vec<right_mcp::credentials::HttpHeaderSecret>>,
) -> Option<AuthMethod> {
    if auth_type == Some("headers") {
        let headers = http_headers?;
        if headers.is_empty() {
            None
        } else {
            Some(AuthMethod::from_db_with_headers(
                auth_type,
                auth_header,
                headers,
            ))
        }
    } else {
        Some(AuthMethod::from_db_with_headers(
            auth_type,
            auth_header,
            Vec::new(),
        ))
    }
}

pub(crate) fn build_http_client() -> anyhow::Result<reqwest::Client> {
    right_mcp::ssrf::hardened_client_builder(right_mcp::ssrf::NetworkPolicy::AllowPrivate)
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_REQUEST_TIMEOUT)
        .build()
        .context("build Aggregator MCP HTTP client")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{RuntimeBuildTestHook, build_agent_runtime_with_test_hook, build_http_client};

    async fn test_store(home: &std::path::Path) -> Arc<right_providers::ProviderStore> {
        Arc::new(right_providers::ProviderStore::open(home).await.unwrap())
    }

    #[tokio::test]
    async fn complete_builder_restores_hindsight_and_runtime_components() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");
        let agent_dir = agents_dir.join("alpha");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("agent.yaml"),
            "memory:\n  provider: hindsight\n  api_key: hs_test\n",
        )
        .unwrap();

        let runtime = build_agent_runtime_with_test_hook(
            "alpha",
            agent_dir,
            &agents_dir,
            test_store(tmp.path()).await,
            build_http_client().unwrap(),
            RuntimeBuildTestHook {
                fail_after_tasks: false,
                task_dropped: None,
            },
        )
        .await
        .unwrap();

        assert!(runtime.registry.hindsight.is_some());
        assert!(!runtime.refresh_sender.is_closed());
        runtime
            .bundle
            .drain(std::time::Duration::from_secs(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn late_builder_failure_awaits_partial_task_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join("agents");
        let agent_dir = agents_dir.join("alpha");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let task_dropped = Arc::new(AtomicBool::new(false));

        let result = build_agent_runtime_with_test_hook(
            "alpha",
            agent_dir,
            &agents_dir,
            test_store(tmp.path()).await,
            build_http_client().unwrap(),
            RuntimeBuildTestHook {
                fail_after_tasks: true,
                task_dropped: Some(Arc::clone(&task_dropped)),
            },
        )
        .await;

        assert!(result.is_err());
        assert!(
            task_dropped.load(Ordering::Acquire),
            "late failure returned before the partial task was cancelled and joined"
        );
    }
}
