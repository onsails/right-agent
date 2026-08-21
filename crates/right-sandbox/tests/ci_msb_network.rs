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
use microsandbox::{NetworkPolicy, NetworkProfile, Sandbox};
use rcgen::{CertificateParams, KeyPair};
use right_sandbox::{
    ExecRequest, SandboxHandle, SecretApplyDisposition, SecretBinding, SecretRemoveDisposition,
};
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

/// Second provider used to prove bindings remain independent.
const SECOND_SECRET_ENV: &str = "RIGHT_SECOND_PROVIDER_KEY";

/// Placeholder derived from [`SECOND_SECRET_ENV`].
const SECOND_SECRET_PLACEHOLDER: &str = "$MSB_RIGHT_SECOND_PROVIDER_KEY";

/// Host source references used by Right's production `apply_secret` path.
const FIRST_SOURCE_ENV: &str = "RT_MSB_FIRST_PROVIDER_SECRET";
const SECOND_SOURCE_ENV: &str = "RT_MSB_SECOND_PROVIDER_SECRET";

/// Writable-layer sentinel that must survive restart-backed additions.
const SECRET_APPLY_SENTINEL_PATH: &str = "/root/right-secret-apply-sentinel";
const SECRET_APPLY_SENTINEL: &str = "persistent-across-secret-restarts";

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
    parse_curl_output(&stdout, args)
}

/// Parse the stable summary emitted by the curl probe script.
fn parse_curl_output(stdout: &str, args: &str) -> Result<CurlOutcome> {
    let (summary, trace) = stdout
        .split_once('\n')
        .ok_or_else(|| anyhow!("curl probe produced no summary line for {args}: {stdout:?}"))?;

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

/// Run the same curl probe through Right's production sandbox handle.
async fn right_curl(sandbox: &SandboxHandle, args: &str) -> Result<CurlOutcome> {
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
    let mut request = ExecRequest::new("/bin/sh");
    request.args = vec!["-c".to_owned(), script];
    request.user = Some("0".to_owned());
    let output = sandbox
        .exec(&request)
        .await
        .with_context(|| format!("exec curl {args} through Right handle"))?;
    let stdout = std::str::from_utf8(&output.stdout).context("curl probe stdout is not utf-8")?;
    parse_curl_output(stdout, args)
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

/// Read one guest environment variable through Right's production handle.
async fn right_guest_env(sandbox: &SandboxHandle, var: &str) -> Result<String> {
    let mut request = ExecRequest::new("/bin/sh");
    request.args = vec!["-c".to_owned(), format!("printf '%s' \"${var}\"")];
    request.user = Some("0".to_owned());
    let output = sandbox
        .exec(&request)
        .await
        .with_context(|| format!("read guest env {var} through Right handle"))?;
    assert_eq!(output.code, 0, "read guest env {var}: {output:?}");
    String::from_utf8(output.stdout).context("guest env value is not utf-8")
}

/// Every guest environment variable, for canary-leak assertions.
async fn guest_env_dump(sandbox: &Sandbox) -> Result<String> {
    let output = sandbox.shell("env").await.context("dump guest env")?;
    output.stdout().context("guest env dump is not utf-8")
}

/// Inline secret material used by assumption probes that bypass Right's API.
enum SecretMaterial<'a> {
    /// Inline value stored in the sandbox config.
    Value(&'a str),
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

/// A complete source-ref binding for one logical provider host.
fn source_binding(
    env_var: &str,
    source_env_var: &str,
    allowed_host: &str,
    value: &str,
) -> SecretBinding {
    let mut binding =
        SecretBinding::new(env_var, source_env_var, secrecy::SecretString::from(value));
    binding.allowed_hosts = vec![allowed_host.to_owned()];
    binding
}

/// Read the guest's boot id, which changes if and only if the VM rebooted.
async fn right_guest_boot_id(sandbox: &SandboxHandle) -> Result<String> {
    let mut request = ExecRequest::new("cat");
    request.args = vec!["/proc/sys/kernel/random/boot_id".to_owned()];
    request.user = Some("0".to_owned());
    let output = sandbox.exec(&request).await.context("read guest boot id")?;
    assert_eq!(output.code, 0, "read guest boot id: {output:?}");
    Ok(String::from_utf8(output.stdout)
        .context("guest boot id is not utf-8")?
        .trim()
        .to_owned())
}

/// Verify both persisted and running configurations carry the expected TLS state.
async fn assert_tls_state(name: &str, port: u16, expected: bool) -> Result<()> {
    let handle = Sandbox::get(name)
        .await
        .with_context(|| format!("inspect sandbox {name}"))?;
    let persisted = handle.config().context("parse persisted sandbox config")?;
    let persisted_tls = persisted
        .spec
        .network
        .tls
        .context("persisted sandbox config must carry explicit TLS state")?;
    assert_eq!(persisted_tls.enabled, expected, "persisted TLS state");
    assert!(
        persisted_tls.intercepted_ports.contains(&port),
        "secret apply must retain the fixture interception port"
    );
    let active = handle
        .active_config()
        .context("parse active sandbox config")?
        .context("running sandbox must expose an active config snapshot")?;
    let active_tls = active
        .spec
        .network
        .tls
        .context("active sandbox config must carry explicit TLS state")?;
    assert_eq!(active_tls.enabled, expected, "active TLS state");
    assert!(
        active_tls.intercepted_ports.contains(&port),
        "active config must retain the fixture interception port"
    );
    Ok(())
}

/// Verify persisted desired TLS is disabled after the final secret removal,
/// while the current VM truthfully remains TLS-enabled until restart.
async fn assert_last_removal_tls_state(name: &str) -> Result<()> {
    let handle = Sandbox::get(name)
        .await
        .with_context(|| format!("inspect sandbox {name}"))?;
    let persisted = handle.config().context("parse persisted sandbox config")?;
    assert!(
        !persisted
            .spec
            .network
            .tls
            .context("persisted TLS config")?
            .enabled
    );
    let active = handle
        .active_config()
        .context("parse active sandbox config")?
        .context("running sandbox must expose an active config snapshot")?;
    assert!(
        active
            .spec
            .network
            .tls
            .context("active TLS config")?
            .enabled,
        "SDK 0.6.10 has no live TLS disable toggle"
    );
    Ok(())
}

/// Send one provider placeholder to its logical TLS host and return what arrived.
async fn observed_right_credential(
    sandbox: &SandboxHandle,
    fixture: &mut HostServer,
    port: u16,
    env_var: &str,
    logical_host: &str,
) -> Result<(String, String)> {
    let args = format!(
        "-k -H \"authorization: Bearer ${env_var}\" https://{logical_host}:{port}/provider"
    );
    let outcome = right_curl(sandbox, &args).await?;
    assert_eq!(
        outcome.code, "200",
        "provider {env_var} request for {logical_host} must reach the fixture: {outcome:?}"
    );

    let head = fixture.next_request().await?;
    let value = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().trim_start_matches("Bearer ").to_owned())
        })
        .ok_or_else(|| anyhow!("fixture saw no authorization header: {head:?}"))?;
    Ok((value, head))
}

/// Production contract for provider addition, rotation, and revocation.
///
/// Starts with no secrets and explicit TLS-off state, then drives only Right's
/// public `apply_secret`/`remove_secret` APIs. It proves live credential
/// revocation, survivor isolation, durable restart state, last-removal
/// credential invalidation, and TLS-off after restart without real credentials.
#[tokio::test]
#[ignore = "ci-msb: boots a live microVM"]
async fn ci_msb_right_apply_secret_activates_tls_and_preserves_provider_contracts() -> Result<()> {
    common::ensure_runtime_installed().await?;
    let first_v1 = format!("{CANARY_SECRET}-first-v1");
    let first_v2 = format!("{CANARY_SECRET}-first-v2");
    let second = format!("{CANARY_SECRET}-second");

    let mut fixture = HostServer::start_tls().await?;
    let port = fixture.port();
    let _slot = common::acquire_vm_slot();
    let guard = common::SandboxGuard::new("net");

    // Raw creation is intentional: Right's public create-time spec does not
    // expose the test-only interception-port override. The resulting sandbox
    // has production-equivalent no-secret/TLS-off state, and every mutation
    // below goes through Right's public handle.
    let created = Sandbox::builder(guard.name())
        .detached(true)
        .image(CURL_IMAGE)
        .cpus(common::PROBE_CPUS)
        .memory(common::PROBE_MEMORY_MIB)
        .user("0")
        .network(|network| {
            network
                .policy(NetworkPolicy::from_profiles([
                    NetworkProfile::Public,
                    NetworkProfile::Host,
                ]))
                .tls(|tls| {
                    tls.enabled(false)
                        .intercepted_ports(vec![port])
                        .verify_upstream(false)
                })
        })
        .create()
        .await
        .context("create empty Right sandbox")?;
    drop(created);
    let sandbox = SandboxHandle::attach(guard.name())
        .await
        .context("attach Right handle")?;
    assert_tls_state(guard.name(), port, false).await?;
    let mut write_sentinel = ExecRequest::new("/bin/sh");
    write_sentinel.args = vec![
        "-c".to_owned(),
        format!("printf '%s' '{SECRET_APPLY_SENTINEL}' > {SECRET_APPLY_SENTINEL_PATH}"),
    ];
    write_sentinel.user = Some("0".to_owned());
    let written = sandbox
        .exec(&write_sentinel)
        .await
        .context("write restart-persistence sentinel")?;
    assert_eq!(
        written.code, 0,
        "write restart-persistence sentinel: {written:?}"
    );

    let first_binding = source_binding(SECRET_ENV, FIRST_SOURCE_ENV, HOST_ALIAS, &first_v1);
    let boot_before_first = right_guest_boot_id(&sandbox).await?;
    let first_apply = sandbox.apply_secret(&first_binding).await?;
    assert_eq!(
        first_apply.disposition,
        SecretApplyDisposition::AddedWithRestart
    );
    assert_ne!(
        boot_before_first,
        right_guest_boot_id(&sandbox).await?,
        "first secret addition must take the restart-backed path"
    );
    let mut read_sentinel = ExecRequest::new("cat");
    read_sentinel.args = vec![SECRET_APPLY_SENTINEL_PATH.to_owned()];
    read_sentinel.user = Some("0".to_owned());
    let first_sentinel = sandbox
        .exec(&read_sentinel)
        .await
        .context("read sentinel after first addition")?;
    assert_eq!(first_sentinel.code, 0, "read sentinel after first addition");
    assert_eq!(
        String::from_utf8(first_sentinel.stdout).context("first sentinel is not utf-8")?,
        SECRET_APPLY_SENTINEL,
        "first addition must preserve the writable layer"
    );
    assert_tls_state(guard.name(), port, true).await?;
    assert_eq!(
        right_guest_env(&sandbox, SECRET_ENV).await?,
        SECRET_PLACEHOLDER,
        "the guest must retain the first provider placeholder"
    );
    let (observed_first, _) =
        observed_right_credential(&sandbox, &mut fixture, port, SECRET_ENV, HOST_ALIAS).await?;
    assert_eq!(observed_first, first_v1);

    let second_binding = source_binding(SECOND_SECRET_ENV, SECOND_SOURCE_ENV, HOST_ALIAS, &second);
    let boot_before_second = right_guest_boot_id(&sandbox).await?;
    let second_apply = sandbox.apply_secret(&second_binding).await?;
    assert_eq!(
        second_apply.disposition,
        SecretApplyDisposition::AddedWithRestart
    );
    assert_ne!(
        boot_before_second,
        right_guest_boot_id(&sandbox).await?,
        "second secret addition must take the restart-backed path"
    );
    let mut map_second_host = ExecRequest::new("/bin/sh");
    map_second_host.args = vec![
        "-c".to_owned(),
        "ip=$(getent hosts host.microsandbox.internal | awk 'NR == 1 { print $1 }'); test -n \"$ip\" && printf '%s second.provider.test\\n' \"$ip\" >> /etc/hosts".to_owned(),
    ];
    map_second_host.user = Some("0".to_owned());
    let mapped = sandbox
        .exec(&map_second_host)
        .await
        .context("map second provider fixture host")?;
    assert_eq!(
        mapped.code, 0,
        "map second provider fixture host: {mapped:?}"
    );
    let second_sentinel = sandbox
        .exec(&read_sentinel)
        .await
        .context("read sentinel after second addition")?;
    assert_eq!(
        second_sentinel.code, 0,
        "read sentinel after second addition"
    );
    assert_eq!(
        String::from_utf8(second_sentinel.stdout).context("second sentinel is not utf-8")?,
        SECRET_APPLY_SENTINEL,
        "second addition must preserve the writable layer"
    );
    assert_eq!(
        right_guest_env(&sandbox, SECOND_SECRET_ENV).await?,
        SECOND_SECRET_PLACEHOLDER,
        "the guest must retain the second provider placeholder"
    );
    let (observed_second, _) =
        observed_right_credential(&sandbox, &mut fixture, port, SECOND_SECRET_ENV, HOST_ALIAS)
            .await?;
    assert_eq!(observed_second, second);
    let (observed_first_again, _) =
        observed_right_credential(&sandbox, &mut fixture, port, SECRET_ENV, HOST_ALIAS).await?;
    assert_eq!(
        observed_first_again, first_v1,
        "adding a second provider must not disturb the first"
    );

    let cross_host = right_curl(
        &sandbox,
        &format!(
            "-k -H 'host: second.provider.test' \
             -H \"authorization: Bearer ${SECRET_ENV}\" \
             https://{HOST_ALIAS}:{port}/wrong-provider"
        ),
    )
    .await?;
    assert!(
        !cross_host.reached_server(),
        "the first provider placeholder must be blocked at the second provider host: {cross_host:?}"
    );
    while let Some(event) = fixture.next_event(Duration::from_secs(5)).await {
        assert!(
            !event.head().contains(&first_v1) && !event.head().contains(SECRET_PLACEHOLDER),
            "no first-provider material may reach the second host: {event:?}"
        );
    }

    let boot_before_rotation = right_guest_boot_id(&sandbox).await?;
    // Re-asserting the full host list exercises the live rotation path while
    // retaining the production fixture host.
    let proposed_host_update = source_binding(SECRET_ENV, FIRST_SOURCE_ENV, HOST_ALIAS, &first_v2);
    let rotated = sandbox.apply_secret(&proposed_host_update).await?;
    assert_eq!(rotated.disposition, SecretApplyDisposition::RotatedLive);
    assert_eq!(
        boot_before_rotation,
        right_guest_boot_id(&sandbox).await?,
        "credential rotation must remain live"
    );

    let (observed_rotated, rotated_head) =
        observed_right_credential(&sandbox, &mut fixture, port, SECRET_ENV, HOST_ALIAS).await?;
    assert_eq!(observed_rotated, first_v2);
    assert!(
        !rotated_head.contains(&first_v1),
        "the old source value must not reach the replacement host"
    );
    assert_eq!(
        right_guest_env(&sandbox, SECRET_ENV).await?,
        SECRET_PLACEHOLDER,
        "rotation must never replace the guest placeholder"
    );

    sandbox.stop().await.context("stop after live rotation")?;
    drop(sandbox);
    let restarted = Sandbox::get(guard.name())
        .await
        .context("load stopped sandbox after live rotation")?
        .start_detached()
        .await
        .context("restart sandbox after live rotation")?;
    drop(restarted);
    let sandbox = SandboxHandle::attach(guard.name())
        .await
        .context("attach after live-rotation restart")?;
    assert_tls_state(guard.name(), port, true).await?;
    let restarted_sentinel = sandbox
        .exec(&read_sentinel)
        .await
        .context("read sentinel after rotation restart")?;
    assert_eq!(
        restarted_sentinel.code, 0,
        "read sentinel after rotation restart"
    );
    assert_eq!(
        String::from_utf8(restarted_sentinel.stdout).context("restarted sentinel is not utf-8")?,
        SECRET_APPLY_SENTINEL,
        "rotation restart must preserve the writable layer"
    );
    assert_eq!(
        right_guest_env(&sandbox, SECRET_ENV).await?,
        SECRET_PLACEHOLDER
    );
    assert_eq!(
        right_guest_env(&sandbox, SECOND_SECRET_ENV).await?,
        SECOND_SECRET_PLACEHOLDER
    );
    let (restarted_first, restarted_first_head) =
        observed_right_credential(&sandbox, &mut fixture, port, SECRET_ENV, HOST_ALIAS).await?;
    assert_eq!(restarted_first, first_v2);
    assert!(
        !restarted_first_head.contains(&first_v1),
        "old value must remain absent after restart"
    );
    let (restarted_second, _) =
        observed_right_credential(&sandbox, &mut fixture, port, SECOND_SECRET_ENV, HOST_ALIAS)
            .await?;
    assert_eq!(restarted_second, second);

    let boot_before_remove = right_guest_boot_id(&sandbox).await?;
    let removed_first = sandbox.remove_secret(SECRET_ENV).await?;
    assert_eq!(
        removed_first.disposition,
        SecretRemoveDisposition::RemovedLive
    );
    assert_eq!(
        boot_before_remove,
        right_guest_boot_id(&sandbox).await?,
        "removing one of two bindings must be live"
    );
    assert_eq!(sandbox.secret_env_vars().await?, [SECOND_SECRET_ENV]);
    assert_tls_state(guard.name(), port, true).await?;
    let removed_request = right_curl(
        &sandbox,
        &format!(
            "-k -H 'authorization: Bearer {SECRET_PLACEHOLDER}' https://{HOST_ALIAS}:{port}/removed"
        ),
    )
    .await?;
    assert!(
        removed_request.reached_server(),
        "without a binding the obsolete placeholder is ordinary opaque data"
    );
    let removed_head = fixture.next_request().await?;
    assert!(
        removed_head
            .to_ascii_lowercase()
            .contains(&SECRET_PLACEHOLDER.to_ascii_lowercase()),
        "removed placeholder must stay opaque: {removed_head:?}"
    );
    assert!(!removed_head.contains(&first_v2));
    let (surviving_second, _) =
        observed_right_credential(&sandbox, &mut fixture, port, SECOND_SECRET_ENV, HOST_ALIAS)
            .await?;
    assert_eq!(surviving_second, second);

    sandbox.stop().await.context("stop after first removal")?;
    drop(sandbox);
    let restarted = Sandbox::get(guard.name())
        .await
        .context("load sandbox after first removal")?
        .start_detached()
        .await
        .context("restart sandbox after first removal")?;
    drop(restarted);
    let sandbox = SandboxHandle::attach(guard.name())
        .await
        .context("attach after first-removal restart")?;
    assert_eq!(sandbox.secret_env_vars().await?, [SECOND_SECRET_ENV]);
    let persisted_removed = right_curl(
        &sandbox,
        &format!(
            "-k -H 'authorization: Bearer {SECRET_PLACEHOLDER}' https://{HOST_ALIAS}:{port}/removed-after-restart"
        ),
    )
    .await?;
    assert!(persisted_removed.reached_server());
    let persisted_removed_head = fixture.next_request().await?;
    assert!(persisted_removed_head.contains(SECRET_PLACEHOLDER));
    assert!(!persisted_removed_head.contains(&first_v2));

    let removed_last = sandbox.remove_secret(SECOND_SECRET_ENV).await?;
    assert_eq!(
        removed_last.disposition,
        SecretRemoveDisposition::RemovedLive
    );
    assert!(sandbox.secret_env_vars().await?.is_empty());
    assert_last_removal_tls_state(guard.name()).await?;
    let last_removed = right_curl(
        &sandbox,
        &format!(
            "-k -H 'authorization: Bearer {SECOND_SECRET_PLACEHOLDER}' https://{HOST_ALIAS}:{port}/last-removed"
        ),
    )
    .await?;
    assert!(last_removed.reached_server());
    let last_removed_head = fixture.next_request().await?;
    assert!(last_removed_head.contains(SECOND_SECRET_PLACEHOLDER));
    assert!(!last_removed_head.contains(&second));

    sandbox.stop().await.context("stop after last removal")?;
    drop(sandbox);
    let restarted = Sandbox::get(guard.name())
        .await
        .context("load sandbox after last removal")?
        .start_detached()
        .await
        .context("restart sandbox after last removal")?;
    drop(restarted);
    let sandbox = SandboxHandle::attach(guard.name())
        .await
        .context("attach after last-removal restart")?;
    assert!(sandbox.secret_env_vars().await?.is_empty());
    assert_tls_state(guard.name(), port, false).await?;
    drop(sandbox);
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
