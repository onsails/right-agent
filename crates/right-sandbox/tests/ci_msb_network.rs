//! Live-microVM egress, secret-substitution, and TLS-interception probes.
//!
//! Stage 1 assumption verification for issue #172. Every test here boots a
//! real microsandbox v0.6.10 microVM, so they are `#[ignore]`d behind the
//! `ci-msb` marker and run with:
//!
//!     devenv shell -- cargo nextest run -p right-sandbox --run-ignored all

mod common;

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use microsandbox::logs::{LogOptions, LogSource};
use microsandbox::sandbox::SandboxStatus;
use microsandbox::{
    ModificationDisposition, NetworkPolicy, NetworkProfile, PlannedChange, Sandbox,
    SecretPlannedChange, SecretSource,
};
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

/// Guest image: `curl` plus a CA bundle, with no package install step.
const CURL_IMAGE: &str = "mirror.gcr.io/curlimages/curl";

/// DNS alias the guest uses to reach a service on the sandbox host.
const HOST_ALIAS: &str = "host.microsandbox.internal";

/// Public destination used as the "reachable" control.
const PUBLIC_URL: &str = "https://example.com/";

/// Public destination on the restrictive-egress allowlist.
const ALLOWED_DOMAIN: &str = "cloudflare.com";

/// Fake credential. Never a real secret; it exists to be found in traffic.
const CANARY_SECRET: &str = "canary-not-a-real-secret-7f3a";

/// Guest-visible environment variable holding the placeholder.
const SECRET_ENV: &str = "RIGHT_PROVIDER_KEY";

/// Placeholder microsandbox derives from [`SECRET_ENV`] by default.
const SECRET_PLACEHOLDER: &str = "$MSB_RIGHT_PROVIDER_KEY";

/// Subject organization of the interception CA microsandbox mints.
const INTERCEPT_CA_MARKER: &str = "microsandbox";

/// How long a probe waits for the host fixture to observe a request.
const FIXTURE_TIMEOUT: Duration = Duration::from_secs(20);

//--------------------------------------------------------------------------------------------------
// Host fixtures
//--------------------------------------------------------------------------------------------------

/// What a host listener observed on one accepted connection.
#[derive(Debug)]
enum FixtureEvent {
    /// A complete HTTP request head, as received on the wire.
    Request(String),

    /// The connection was accepted but never produced a request head. The
    /// proxy dropping a blocked request lands here.
    Aborted(String),
}

impl FixtureEvent {
    /// The request head, or an empty string for an aborted connection.
    fn head(&self) -> &str {
        match self {
            Self::Request(head) => head,
            Self::Aborted(_) => "",
        }
    }
}

/// A loopback-only HTTP or HTTPS listener on an ephemeral port.
///
/// Binds `127.0.0.1` and `[::1]` on the same port because the guest may route
/// the host alias over either gateway family. It serves connections until it
/// is dropped, so a probe can observe several sequential requests.
struct HostServer {
    port: u16,
    events: mpsc::UnboundedReceiver<FixtureEvent>,
    task: JoinHandle<()>,
}

impl HostServer {
    /// Start a plain-HTTP listener.
    async fn start_plain() -> Result<Self> {
        Self::start(None).await
    }

    /// Start an HTTPS listener presenting a self-signed cert for [`HOST_ALIAS`].
    async fn start_tls() -> Result<Self> {
        Self::start(Some(TlsAcceptor::from(fixture_server_config()?))).await
    }

    async fn start(acceptor: Option<TlsAcceptor>) -> Result<Self> {
        let v4 = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .context("bind fixture on 127.0.0.1")?;
        let port = v4.local_addr().context("fixture local addr")?.port();
        let v6 = TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, port)))
            .await
            .context("bind fixture on [::1]")?;

        let (tx, events) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    accepted = v4.accept() => accepted,
                    accepted = v6.accept() => accepted,
                };
                let stream = match accepted {
                    Ok((stream, _)) => stream,
                    Err(err) => {
                        if tx
                            .send(FixtureEvent::Aborted(format!("accept: {err}")))
                            .is_err()
                        {
                            tracing::debug!("fixture receiver dropped before {err} was reported");
                        }
                        return;
                    }
                };
                let event = match &acceptor {
                    Some(acceptor) => match acceptor.accept(stream).await {
                        Ok(tls) => serve_one(tls).await,
                        Err(err) => Err(err),
                    },
                    None => serve_one(stream).await,
                };
                let event = match event {
                    Ok(head) => FixtureEvent::Request(head),
                    Err(err) => FixtureEvent::Aborted(err.to_string()),
                };
                if tx.send(event).is_err() {
                    return;
                }
            }
        });

        Ok(Self { port, events, task })
    }

    fn port(&self) -> u16 {
        self.port
    }

    /// Wait for the next observed connection, or `None` on timeout.
    async fn next_event(&mut self, wait: Duration) -> Option<FixtureEvent> {
        tokio::time::timeout(wait, self.events.recv())
            .await
            .ok()
            .flatten()
    }

    /// Wait for the next request head, failing if the connection aborted or
    /// nothing arrived.
    async fn next_request(&mut self) -> Result<String> {
        match self.next_event(FIXTURE_TIMEOUT).await {
            Some(FixtureEvent::Request(head)) => Ok(head),
            Some(FixtureEvent::Aborted(err)) => {
                bail!("host fixture accepted a connection but got no request: {err}")
            }
            None => bail!("host fixture received nothing within {FIXTURE_TIMEOUT:?}"),
        }
    }
}

impl Drop for HostServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Read one HTTP request head, answer `200 ok`, and return the head.
async fn serve_one<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S) -> io::Result<String> {
    let mut head = Vec::new();
    loop {
        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before the request head",
            ));
        }
        head.extend_from_slice(&buf[..read]);
        if head.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }

    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
        .await?;
    stream.shutdown().await?;
    Ok(String::from_utf8_lossy(&head).into_owned())
}

/// A self-signed server config for [`HOST_ALIAS`].
///
/// Sandboxes that talk to the fixture set `verify_upstream(false)`, so the
/// interception proxy accepts this cert; the guest passes `-k` for the same
/// reason on the guest-facing leg.
fn fixture_server_config() -> Result<Arc<rustls::ServerConfig>> {
    // Installing twice is expected: only the first install in a process wins
    // and the error carries no other information.
    if rustls::crypto::ring::default_provider()
        .install_default()
        .is_err()
    {
        tracing::debug!("rustls ring provider was already installed");
    }

    let key_pair = KeyPair::generate().context("generate fixture key")?;
    let params =
        CertificateParams::new(vec![HOST_ALIAS.to_string()]).context("fixture cert params")?;
    let cert = params
        .self_signed(&key_pair)
        .context("self-sign fixture cert")?;
    let chain = vec![CertificateDer::from(cert.der().to_vec())];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

    Ok(Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .context("fixture server config")?,
    ))
}

//--------------------------------------------------------------------------------------------------
// Guest probes
//--------------------------------------------------------------------------------------------------

/// The outcome of one guest-side `curl` invocation.
#[derive(Debug)]
struct CurlOutcome {
    /// `%{http_code}`; `000` when no HTTP response arrived.
    code: String,

    /// `curl`'s exit status.
    status: i32,

    /// `curl`'s stderr, including `-v` handshake tracing.
    trace: String,
}

impl CurlOutcome {
    /// True when the destination answered with an HTTP status line.
    fn reached_server(&self) -> bool {
        self.status == 0 && self.code.len() == 3 && self.code != "000"
    }

    /// The certificate issuer `curl -v` reported for the TLS peer.
    ///
    /// Verbose lines are prefixed with `* `, and the certificate block indents
    /// its fields further: `*  issuer: C=US; O=Let's Encrypt; CN=E5`.
    fn issuer(&self) -> Option<&str> {
        self.trace.lines().find_map(|line| {
            line.trim_start()
                .trim_start_matches('*')
                .trim_start()
                .strip_prefix("issuer:")
                .map(str::trim)
        })
    }
}

/// Run `curl <args>` in the guest and capture code, exit status, and stderr.
///
/// `set +e` keeps the script alive after a curl failure so the failure reason
/// reaches the assertion instead of collapsing into an opaque exec error.
async fn curl(sandbox: &Sandbox, args: &str) -> Result<CurlOutcome> {
    let script = format!(
        r#"set +e
err=$(mktemp)
code=$(curl -sS --max-time 30 -o /dev/null -w '%{{http_code}}' {args} 2>"$err")
status=$?
printf 'code=%s status=%s\n' "$code" "$status"
cat "$err"
rm -f "$err"
"#
    );
    let output = sandbox
        .shell(script)
        .await
        .with_context(|| format!("exec curl {args}"))?;
    let stdout = output.stdout().context("curl probe stdout is not utf-8")?;
    let (summary, trace) = stdout
        .split_once('\n')
        .ok_or_else(|| anyhow!("curl probe produced no summary line: {stdout:?}"))?;

    let mut code = None;
    let mut status = None;
    for field in summary.split_whitespace() {
        if let Some(value) = field.strip_prefix("code=") {
            code = Some(value.to_string());
        } else if let Some(value) = field.strip_prefix("status=") {
            status = Some(value.parse::<i32>().context("parse curl exit status")?);
        }
    }

    Ok(CurlOutcome {
        code: code.ok_or_else(|| anyhow!("curl probe summary lacks code=: {summary:?}"))?,
        status: status.ok_or_else(|| anyhow!("curl probe summary lacks status=: {summary:?}"))?,
        trace: trace.to_string(),
    })
}

/// Retry a success-expecting public-internet probe.
///
/// The policy under test never changes across attempts, so only a dropped
/// handshake to a shared CDN edge can flake here.
async fn curl_with_retry(sandbox: &Sandbox, args: &str) -> Result<CurlOutcome> {
    let mut last = curl(sandbox, args).await?;
    for _ in 0..2 {
        if last.reached_server() {
            return Ok(last);
        }
        last = curl(sandbox, args).await?;
    }
    Ok(last)
}

/// Read one environment variable inside the guest.
async fn guest_env(sandbox: &Sandbox, var: &str) -> Result<String> {
    let output = sandbox
        .shell(format!("printf '%s' \"${var}\""))
        .await
        .with_context(|| format!("read guest env {var}"))?;
    output.stdout().context("guest env value is not utf-8")
}

/// Every guest environment variable, for canary-leak assertions.
async fn guest_env_dump(sandbox: &Sandbox) -> Result<String> {
    let output = sandbox.shell("env").await.context("dump guest env")?;
    output.stdout().context("guest env dump is not utf-8")
}

/// Where the runtime gets a secret's real value.
enum SecretMaterial<'a> {
    /// Inline value stored in the sandbox config.
    Value(&'a str),

    /// Host environment variable, resolved by the SDK at spawn and at apply.
    HostEnv(&'a str),
}

/// Boot a curl sandbox that carries one host-bound secret and talks to the
/// HTTPS fixture on `port`.
///
/// `intercepted_ports` must name the fixture port: interception defaults to
/// 443 only. `verify_upstream(false)` covers the fixture's self-signed cert.
async fn create_secret_sandbox(
    name: &str,
    port: u16,
    material: SecretMaterial<'_>,
    allow_host: &str,
) -> Result<Sandbox> {
    Sandbox::builder(name)
        .image(CURL_IMAGE)
        .cpus(common::PROBE_CPUS)
        .memory(common::PROBE_MEMORY_MIB)
        .user("0")
        .secret(|s| {
            let s = s.env(SECRET_ENV).allow_host(allow_host);
            match material {
                SecretMaterial::Value(value) => s.value(value),
                SecretMaterial::HostEnv(var) => s.source(SecretSource::Env {
                    var: var.to_string(),
                }),
            }
        })
        .network(|n| {
            n.policy(NetworkPolicy::from_profiles([
                NetworkProfile::Public,
                NetworkProfile::Host,
            ]))
            .tls(|t| t.intercepted_ports(vec![port]).verify_upstream(false))
        })
        .create()
        .await
        .with_context(|| format!("create secret sandbox {name}"))
}

//--------------------------------------------------------------------------------------------------
// Probes: egress policy
//--------------------------------------------------------------------------------------------------

/// Assumption 3, permissive half: `[Public, Host]` reaches the internet.
#[tokio::test]
#[ignore = "ci-msb: boots a live microVM"]
async fn ci_msb_permissive_egress_reaches_public_internet() -> Result<()> {
    common::ensure_runtime_installed().await?;
    let _slot = common::acquire_vm_slot();
    let guard = common::SandboxGuard::new("net");

    let sandbox = Sandbox::builder(guard.name())
        .image(CURL_IMAGE)
        .cpus(common::PROBE_CPUS)
        .memory(common::PROBE_MEMORY_MIB)
        .user("0")
        .network(|n| {
            n.policy(NetworkPolicy::from_profiles([
                NetworkProfile::Public,
                NetworkProfile::Host,
            ]))
        })
        .create()
        .await
        .context("create permissive-egress sandbox")?;

    let outcome = curl_with_retry(&sandbox, PUBLIC_URL).await?;
    assert!(
        outcome.reached_server(),
        "permissive egress must reach {PUBLIC_URL}: {outcome:?}"
    );

    guard.destroy().await
}

/// Assumption 3, host-isolation half: the host group is denied unless asked
/// for, even while public egress is wide open.
#[tokio::test]
#[ignore = "ci-msb: boots a live microVM"]
async fn ci_msb_host_group_denied_by_default() -> Result<()> {
    common::ensure_runtime_installed().await?;
    let mut fixture = HostServer::start_plain().await?;
    let port = fixture.port();
    let _slot = common::acquire_vm_slot();
    let guard = common::SandboxGuard::new("net");

    let sandbox = Sandbox::builder(guard.name())
        .image(CURL_IMAGE)
        .cpus(common::PROBE_CPUS)
        .memory(common::PROBE_MEMORY_MIB)
        .user("0")
        .network(|n| n.policy(NetworkPolicy::from_profiles([NetworkProfile::Public])))
        .create()
        .await
        .context("create public-only sandbox")?;

    // Public egress still works, so a failure below is host-group denial and
    // not a dead network.
    let public = curl_with_retry(&sandbox, PUBLIC_URL).await?;
    assert!(
        public.reached_server(),
        "public egress must still work under the Public profile: {public:?}"
    );

    let host = curl(&sandbox, &format!("http://{HOST_ALIAS}:{port}/")).await?;
    assert!(
        !host.reached_server(),
        "Public profile alone must not reach the host listener: {host:?}"
    );
    assert!(
        fixture.next_event(Duration::from_secs(5)).await.is_none(),
        "denied guest traffic must never reach the host listener"
    );

    guard.destroy().await
}

/// Assumption 5, MCP-aggregator path: with `[Public, Host]` the guest reaches
/// a host service bound to loopback through the host alias.
#[tokio::test]
#[ignore = "ci-msb: boots a live microVM"]
async fn ci_msb_guest_reaches_loopback_host_service_via_alias() -> Result<()> {
    common::ensure_runtime_installed().await?;
    let mut fixture = HostServer::start_plain().await?;
    let port = fixture.port();
    let _slot = common::acquire_vm_slot();
    let guard = common::SandboxGuard::new("net");

    let sandbox = Sandbox::builder(guard.name())
        .image(CURL_IMAGE)
        .cpus(common::PROBE_CPUS)
        .memory(common::PROBE_MEMORY_MIB)
        .user("0")
        .network(|n| {
            n.policy(NetworkPolicy::from_profiles([
                NetworkProfile::Public,
                NetworkProfile::Host,
            ]))
        })
        .create()
        .await
        .context("create host-access sandbox")?;

    let outcome = curl(&sandbox, &format!("http://{HOST_ALIAS}:{port}/mcp")).await?;
    assert_eq!(
        outcome.code, "200",
        "guest must reach the loopback host service via {HOST_ALIAS}: {outcome:?}"
    );

    let head = fixture.next_request().await?;
    assert!(
        head.starts_with("GET /mcp "),
        "host listener must observe the guest request: {head:?}"
    );
    assert!(
        head.contains(&format!("{HOST_ALIAS}:{port}")),
        "guest request must carry the host alias: {head:?}"
    );

    guard.destroy().await
}

/// Assumption 3, restrictive half: a deny-by-default policy plus a one-domain
/// allowlist admits exactly that domain.
#[tokio::test]
#[ignore = "ci-msb: boots a live microVM"]
async fn ci_msb_restrictive_egress_allows_only_listed_destinations() -> Result<()> {
    common::ensure_runtime_installed().await?;
    let mut fixture = HostServer::start_plain().await?;
    let port = fixture.port();
    let _slot = common::acquire_vm_slot();
    let guard = common::SandboxGuard::new("net");

    // Host keeps the aggregator path open; the allowlist is the only public
    // egress. `from_profiles` already permits DNS to the gateway resolver.
    let policy = NetworkPolicy::from_profiles([NetworkProfile::Host])
        .allow_domain_suffix(ALLOWED_DOMAIN)
        .with_context(|| format!("allow {ALLOWED_DOMAIN}"))?;

    let sandbox = Sandbox::builder(guard.name())
        .image(CURL_IMAGE)
        .cpus(common::PROBE_CPUS)
        .memory(common::PROBE_MEMORY_MIB)
        .user("0")
        .network(|n| n.policy(policy))
        .create()
        .await
        .context("create restrictive-egress sandbox")?;

    let allowed = curl_with_retry(&sandbox, &format!("https://{ALLOWED_DOMAIN}/")).await?;
    assert!(
        allowed.reached_server(),
        "allowlisted {ALLOWED_DOMAIN} must be reachable: {allowed:?}"
    );

    let unlisted = curl(&sandbox, PUBLIC_URL).await?;
    assert!(
        !unlisted.reached_server(),
        "unlisted {PUBLIC_URL} must be refused: {unlisted:?}"
    );

    // The Host profile is still in force, so the allowlist restricts public
    // egress without cutting the host service off.
    let host = curl(&sandbox, &format!("http://{HOST_ALIAS}:{port}/")).await?;
    assert_eq!(
        host.code, "200",
        "host alias must stay reachable under a restrictive public allowlist: {host:?}"
    );
    fixture.next_request().await?;

    guard.destroy().await
}

//--------------------------------------------------------------------------------------------------
// Probes: secret substitution
//--------------------------------------------------------------------------------------------------

/// Assumption 2: the guest holds only a placeholder and the bound destination
/// receives the real credential.
#[tokio::test]
#[ignore = "ci-msb: boots a live microVM"]
async fn ci_msb_secret_substitution_reaches_bound_destination() -> Result<()> {
    common::ensure_runtime_installed().await?;
    let mut fixture = HostServer::start_tls().await?;
    let port = fixture.port();
    let _slot = common::acquire_vm_slot();
    let guard = common::SandboxGuard::new("net");

    let sandbox = create_secret_sandbox(
        guard.name(),
        port,
        SecretMaterial::Value(CANARY_SECRET),
        HOST_ALIAS,
    )
    .await?;

    // The guest sees a placeholder, not the credential.
    let visible = guest_env(&sandbox, SECRET_ENV).await?;
    assert_eq!(
        visible, SECRET_PLACEHOLDER,
        "guest must see only the placeholder for {SECRET_ENV}"
    );
    let env_dump = guest_env_dump(&sandbox).await?;
    assert!(
        !env_dump.contains(CANARY_SECRET),
        "canary must not appear anywhere in the guest environment"
    );

    let outcome = curl(
        &sandbox,
        &format!(
            "-k -H \"authorization: Bearer ${SECRET_ENV}\" https://{HOST_ALIAS}:{port}/v1/messages"
        ),
    )
    .await?;
    assert_eq!(
        outcome.code, "200",
        "substituted request must reach the bound destination: {outcome:?}"
    );

    let head = fixture.next_request().await?;
    assert!(
        head.contains(&format!("Bearer {CANARY_SECRET}")),
        "bound destination must receive the real credential: {head:?}"
    );
    assert!(
        !head.contains(SECRET_PLACEHOLDER),
        "placeholder must not survive substitution: {head:?}"
    );

    guard.destroy().await
}

/// One host-visible log line mentioning a secret, and where it was found.
#[derive(Debug)]
struct LogHit {
    /// On-disk log file, or `sandbox.logs()` for the SDK reader.
    origin: String,
    line: String,
}

/// Every host-visible log line that mentions a secret, from both the on-disk
/// log directory and the SDK's `System` log source.
///
/// Right needs a host-side signal to raise a Telegram operator alert when the
/// guest aims a placeholder at an unbound destination; this is the search for
/// that signal.
async fn secret_log_hits(sandbox: &Sandbox, name: &str) -> Result<Vec<LogHit>> {
    let mut hits = Vec::new();

    let dir = microsandbox::logs::log_dir_for(name);
    if dir.is_dir() {
        let entries =
            std::fs::read_dir(&dir).with_context(|| format!("read log dir {}", dir.display()))?;
        for entry in entries {
            let path = entry
                .with_context(|| format!("read log dir entry in {}", dir.display()))?
                .path();
            if !path.is_file() {
                continue;
            }
            let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            collect_secret_lines(
                &path.display().to_string(),
                &String::from_utf8_lossy(&bytes),
                &mut hits,
            );
        }
    }

    let entries = sandbox
        .logs(&LogOptions {
            sources: vec![LogSource::System],
            ..LogOptions::default()
        })
        .await
        .context("read sandbox system logs")?;
    for entry in entries {
        collect_secret_lines(
            "sandbox.logs(System)",
            &String::from_utf8_lossy(&entry.data),
            &mut hits,
        );
    }

    Ok(hits)
}

fn collect_secret_lines(origin: &str, text: &str, hits: &mut Vec<LogHit>) {
    for line in text.lines() {
        if line.to_ascii_lowercase().contains("secret") {
            hits.push(LogHit {
                origin: origin.to_string(),
                line: line.trim_end().to_string(),
            });
        }
    }
}

/// The log directory the probes search, reported alongside evidence.
fn log_dir_display(name: &str) -> PathBuf {
    microsandbox::logs::log_dir_for(name)
}

/// Assumption 2, negative half: a placeholder aimed at an unbound destination
/// is blocked rather than leaked, the sandbox survives, and the host gets a
/// log line it can alert on.
#[tokio::test]
#[ignore = "ci-msb: boots a live microVM"]
async fn ci_msb_secret_blocked_toward_unbound_destination() -> Result<()> {
    common::ensure_runtime_installed().await?;
    let mut fixture = HostServer::start_tls().await?;
    let port = fixture.port();
    let _slot = common::acquire_vm_slot();
    let guard = common::SandboxGuard::new("net");

    // The secret is bound to a provider host the probe never contacts, so the
    // fixture is an unbound destination for it.
    let sandbox = create_secret_sandbox(
        guard.name(),
        port,
        SecretMaterial::Value(CANARY_SECRET),
        "api.provider.invalid",
    )
    .await?;

    let outcome = curl(
        &sandbox,
        &format!("-k -H \"authorization: Bearer ${SECRET_ENV}\" https://{HOST_ALIAS}:{port}/exfil"),
    )
    .await?;
    assert!(
        !outcome.reached_server(),
        "request carrying a placeholder to an unbound destination must be blocked: {outcome:?}"
    );
    assert!(
        !outcome.trace.contains(CANARY_SECRET),
        "curl output must never echo the canary: {outcome:?}"
    );

    // Nothing that carries secret material may reach the unbound destination.
    // An aborted connection is fine; a request head is not.
    while let Some(event) = fixture.next_event(Duration::from_secs(5)).await {
        let head = event.head();
        assert!(
            !head.contains(CANARY_SECRET) && !head.contains(SECRET_PLACEHOLDER),
            "no secret material may reach an unbound destination: {event:?}"
        );
    }

    assert_eq!(
        sandbox.status().await.context("status after violation")?,
        SandboxStatus::Running,
        "a blocked secret must not tear the sandbox down"
    );

    let hits = secret_log_hits(&sandbox, guard.name()).await?;
    // Assert canary-free BEFORE printing: if the runtime ever logged a real
    // credential, we must fail before echoing it into the test log.
    for hit in &hits {
        assert!(
            !hit.line.contains(CANARY_SECRET),
            "log lines must not carry the credential: {hit:?}"
        );
        println!("[secret-log] {} :: {}", hit.origin, hit.line);
    }
    assert!(
        !hits.is_empty(),
        "expected a host-visible secret-violation log line under {}",
        log_dir_display(guard.name()).display()
    );

    guard.destroy().await
}

/// Host environment variable backing the rotation probe's source-ref secret.
const ROTATION_HOST_VAR: &str = "RT_MSB_ROTATION_SECRET";

/// Read the guest's boot id, which changes if and only if the VM rebooted.
async fn guest_boot_id(sandbox: &Sandbox) -> Result<String> {
    let output = sandbox
        .shell("cat /proc/sys/kernel/random/boot_id")
        .await
        .context("read guest boot id")?;
    Ok(output
        .stdout()
        .context("guest boot id is not utf-8")?
        .trim()
        .to_string())
}

/// Send the placeholder to the fixture and return the credential the fixture
/// actually received.
async fn observed_credential(
    sandbox: &Sandbox,
    fixture: &mut HostServer,
    port: u16,
) -> Result<String> {
    let outcome = curl(
        sandbox,
        &format!(
            "-k -H \"authorization: Bearer ${SECRET_ENV}\" https://{HOST_ALIAS}:{port}/rotate"
        ),
    )
    .await?;
    assert_eq!(
        outcome.code, "200",
        "rotation probe request must reach the fixture: {outcome:?}"
    );

    let head = fixture.next_request().await?;
    let value = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().trim_start_matches("Bearer ").to_string())
        })
        .ok_or_else(|| anyhow!("fixture saw no authorization header: {head:?}"))?;
    Ok(value)
}

/// Assumption 5: a source-ref secret can be rotated on a running sandbox, and
/// the plan's disposition tells the truth about whether that needs a restart.
#[tokio::test]
#[ignore = "ci-msb: boots a live microVM"]
async fn ci_msb_source_ref_secret_rotates_live() -> Result<()> {
    common::ensure_runtime_installed().await?;
    let first = format!("{CANARY_SECRET}-v1");
    let second = format!("{CANARY_SECRET}-v2");

    // The SDK resolves `SecretSource::Env` in this process, at create and at
    // apply, so rotating the credential means rewriting this variable. Test
    // binaries are single sandbox processes under nextest, so nothing else
    // reads it concurrently. This is the only mechanism upstream exposes for
    // source-ref rotation, so the no-set_var-in-tests rule is bent here;
    // cleanup removes the variable before the probe returns.
    unsafe { std::env::set_var(ROTATION_HOST_VAR, &first) };

    let mut fixture = HostServer::start_tls().await?;
    let port = fixture.port();
    let _slot = common::acquire_vm_slot();
    let guard = common::SandboxGuard::new("net");

    let sandbox = create_secret_sandbox(
        guard.name(),
        port,
        SecretMaterial::HostEnv(ROTATION_HOST_VAR),
        HOST_ALIAS,
    )
    .await?;

    let before = observed_credential(&sandbox, &mut fixture, port).await?;
    assert_eq!(
        before, first,
        "source-ref secret must resolve from the host environment at spawn"
    );
    let boot_before = guest_boot_id(&sandbox).await?;

    unsafe { std::env::set_var(ROTATION_HOST_VAR, &second) };

    let plan = sandbox
        .modify()
        .secret(|s| {
            s.env(SECRET_ENV).source(SecretSource::Env {
                var: ROTATION_HOST_VAR.to_string(),
            })
        })
        .dry_run()
        .await
        .context("dry-run secret rotation")?;

    let changes: Vec<&SecretPlannedChange> = plan
        .changes
        .iter()
        .filter_map(|change| match change {
            PlannedChange::Secret(secret) => Some(secret),
            PlannedChange::Config(_) => None,
        })
        .collect();
    assert_eq!(
        changes.len(),
        1,
        "rotation must plan exactly one secret change: {:?}",
        plan.changes
    );
    let disposition = changes[0].disposition;
    println!("[rotation] status={} plan={:?}", plan.status, changes[0]);
    for warning in &plan.warnings {
        println!(
            "[rotation] warning: {} :: {}",
            warning.field, warning.message
        );
    }

    // The planner only reports `Live` for a running sandbox when the runtime
    // advertises the `secrets_update` control capability, so the disposition
    // is the capability report.
    let live_capability = disposition == ModificationDisposition::Live;

    let applied = sandbox
        .modify()
        .secret(|s| {
            s.env(SECRET_ENV).source(SecretSource::Env {
                var: ROTATION_HOST_VAR.to_string(),
            })
        })
        .apply()
        .await
        .context("apply secret rotation")?;
    assert!(applied.applied, "apply must report the plan as applied");

    let boot_after = guest_boot_id(&sandbox).await?;
    let after = observed_credential(&sandbox, &mut fixture, port).await?;
    assert_eq!(
        after, second,
        "the rotated credential must reach the bound destination"
    );

    if live_capability {
        assert_eq!(
            boot_before, boot_after,
            "a Live rotation must not reboot the guest"
        );
    } else {
        assert_ne!(
            boot_before, boot_after,
            "a non-Live rotation only propagates because apply restarted the guest"
        );
    }

    // Leave no residue for a later test process that reuses this binary under
    // plain `cargo test`.
    unsafe { std::env::remove_var(ROTATION_HOST_VAR) };

    guard.destroy().await
}

//--------------------------------------------------------------------------------------------------
// Probes: TLS interception scope
//--------------------------------------------------------------------------------------------------

/// Anthropic API host Claude Code talks to.
const ANTHROPIC_HOST: &str = "api.anthropic.com";

/// Assumption 3: with interception on, bypassed hosts keep the real upstream
/// certificate, and every host that is *not* bypassed is intercepted.
///
/// The second half is the load-bearing one: `TlsConfig.bypass` is a deny-list,
/// so the scope of interception is "everything except these", not "only
/// provider hosts".
#[tokio::test]
#[ignore = "ci-msb: boots a live microVM"]
async fn ci_msb_claude_traffic_bypasses_interception() -> Result<()> {
    common::ensure_runtime_installed().await?;
    let _slot = common::acquire_vm_slot();
    let guard = common::SandboxGuard::new("net");

    // One secret is enough to turn interception on globally; it is bound to a
    // host this probe never contacts.
    let sandbox = Sandbox::builder(guard.name())
        .image(CURL_IMAGE)
        .cpus(common::PROBE_CPUS)
        .memory(common::PROBE_MEMORY_MIB)
        .user("0")
        .secret(|s| {
            s.env(SECRET_ENV)
                .value(CANARY_SECRET)
                .allow_host("api.provider.invalid")
        })
        .network(|n| {
            n.policy(NetworkPolicy::from_profiles([NetworkProfile::Public]))
                .tls(|t| t.bypass(ANTHROPIC_HOST).bypass("*.anthropic.com"))
        })
        .create()
        .await
        .context("create bypass sandbox")?;

    // Bypassed: no `-k`, no CA configuration, so success proves the guest
    // validated a real public chain rather than a minted leaf.
    let bypassed = curl_with_retry(&sandbox, &format!("-v https://{ANTHROPIC_HOST}/")).await?;
    assert!(
        bypassed.reached_server(),
        "bypassed {ANTHROPIC_HOST} must handshake with no trust configuration: {bypassed:?}"
    );
    let bypassed_issuer = bypassed
        .issuer()
        .ok_or_else(|| anyhow!("curl -v reported no issuer for {ANTHROPIC_HOST}: {bypassed:?}"))?;
    println!("[bypass] {ANTHROPIC_HOST} issuer: {bypassed_issuer}");
    assert!(
        !bypassed_issuer.contains(INTERCEPT_CA_MARKER),
        "bypassed host must present the real upstream cert, got issuer {bypassed_issuer:?}"
    );

    // Not bypassed: `-k` so the handshake completes far enough for curl to
    // report the issuer of whatever certificate it was served.
    let intercepted = curl_with_retry(&sandbox, &format!("-v -k {PUBLIC_URL}")).await?;
    let intercepted_issuer = intercepted
        .issuer()
        .ok_or_else(|| anyhow!("curl -v reported no issuer for {PUBLIC_URL}: {intercepted:?}"))?;
    println!("[bypass] {PUBLIC_URL} issuer: {intercepted_issuer}");
    assert!(
        intercepted_issuer.contains(INTERCEPT_CA_MARKER),
        "a non-bypassed host must be intercepted once any secret exists, got issuer \
         {intercepted_issuer:?}"
    );

    // Whether the guest trusts the interception CA without help decides how
    // much trust plumbing Right owes a sandboxed process.
    let unpinned = curl(&sandbox, &format!("-v {PUBLIC_URL}")).await?;
    println!(
        "[bypass] non-bypassed host without -k: code={} status={}",
        unpinned.code, unpinned.status
    );
    assert!(
        !unpinned.reached_server(),
        "an intercepted host must fail verification without the interception CA: {unpinned:?}"
    );

    guard.destroy().await
}
