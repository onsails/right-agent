//! Shared `#[cfg(test)]` fake for the Aggregator's internal database API.
//!
//! Production bot code reaches `data.db` exclusively through typed
//! `InternalClient` calls over `run/internal.sock`. Tests that previously
//! seeded and asserted against a direct `right_db::Connection` now stand up
//! this fake owner instead: it binds a real Unix socket, speaks the same
//! HTTP/JSON wire protocol (`POST <route>` with a JSON body, JSON response,
//! `DbErrorResponse` on failure), and records every request so tests can
//! assert exactly which typed operations the bot issued.
//!
//! Semantic mapping: SQL-fixture assertions became (a) recorded-request
//! assertions here (bot → wire contract) and (b) owner-side tests in
//! `crates/right/src/internal_api_db_tests.rs` (wire → real SQL semantics).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::Mutex;

/// One request the fake owner received: the route path and the parsed JSON body.
#[derive(Debug, Clone)]
pub(crate) struct RecordedRequest {
    pub route: String,
    pub body: serde_json::Value,
}

use futures::future::BoxFuture;

/// Request dispatcher: route path + parsed body → `(status, response body)`.
pub(crate) type Handler = Arc<
    dyn Fn(&str, serde_json::Value) -> BoxFuture<'static, (u16, serde_json::Value)> + Send + Sync,
>;

/// Adapt a synchronous dispatcher into a [`Handler`].
pub(crate) fn sync_handler(
    f: impl Fn(&str, serde_json::Value) -> (u16, serde_json::Value) + Send + Sync + 'static,
) -> Handler {
    Arc::new(move |route, body| {
        let response = f(route, body);
        Box::pin(async move { response })
    })
}

/// A fake internal API owner bound on a real Unix socket.
pub(crate) struct FakeInternalApi {
    _dir: Option<tempfile::TempDir>,
    socket_path: PathBuf,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    _handle: tokio::task::JoinHandle<()>,
}

impl FakeInternalApi {
    /// Bind the fake at a fresh `<tempdir>/internal.sock`.
    pub(crate) fn start(handler: Handler) -> Self {
        let dir = tempfile::tempdir().expect("fake internal API tempdir");
        let socket_path = dir.path().join("internal.sock");
        Self::bind(socket_path, handler, Some(dir))
    }

    /// Bind the fake at an explicit socket path (its parent must exist).
    ///
    /// Used when the code under test derives the socket location itself, e.g.
    /// `crate::db::client_for_agent_dir` maps `<home>/agents/<name>` to
    /// `<home>/run/internal.sock`.
    pub(crate) fn start_at(socket_path: PathBuf, handler: Handler) -> Self {
        Self::bind(socket_path, handler, None)
    }

    fn bind(socket_path: PathBuf, handler: Handler, dir: Option<tempfile::TempDir>) -> Self {
        let listener =
            tokio::net::UnixListener::bind(&socket_path).expect("bind fake internal API");
        let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let handler = Arc::clone(&handler);
                let recorded = Arc::clone(&recorded);
                tokio::spawn(async move {
                    serve_connection(stream, handler, recorded).await;
                });
            }
        });
        Self {
            _dir: dir,
            socket_path,
            requests,
            _handle: handle,
        }
    }

    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Snapshot of every request received so far, in arrival order.
    pub(crate) async fn recorded(&self) -> Vec<RecordedRequest> {
        self.requests.lock().await.clone()
    }

    /// Poll until at least `n` requests arrived (or the timeout elapses) and
    /// return the snapshot. Tests use this instead of sleeping fixed amounts
    /// when the bot writes asynchronously (e.g. spawned archive tasks).
    pub(crate) async fn wait_for_requests(
        &self,
        n: usize,
        timeout: Duration,
    ) -> Vec<RecordedRequest> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let snapshot = self.recorded().await;
            if snapshot.len() >= n || tokio::time::Instant::now() >= deadline {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// Wrap a serializable response DTO into a `200 OK` reply.
pub(crate) fn ok(body: impl serde::Serialize) -> (u16, serde_json::Value) {
    (
        200,
        serde_json::to_value(body).expect("serialize fake response"),
    )
}

/// Build a server-side typed error reply matching `DbErrorResponse`, so
/// `classify_transport_error` maps it into the typed category.
pub(crate) fn db_error(
    status: u16,
    category: right_mcp::internal_db::DbErrorCategory,
) -> (u16, serde_json::Value) {
    (
        status,
        serde_json::json!({
            "category": category,
            "message": format!("fake owner error: {category:?}"),
        }),
    )
}

async fn serve_connection(
    mut stream: tokio::net::UnixStream,
    handler: Handler,
    recorded: Arc<Mutex<Vec<RecordedRequest>>>,
) {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).await.expect("read request");
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let content_length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buf.len() < body_start + content_length {
        let n = stream.read(&mut chunk).await.expect("read request body");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let route = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_owned();
    let body: serde_json::Value =
        serde_json::from_slice(&buf[body_start..body_start + content_length])
            .expect("request body is JSON");
    recorded.lock().await.push(RecordedRequest {
        route: route.clone(),
        body: body.clone(),
    });

    let (status, response) = handler(&route, body).await;
    let payload = response.to_string();
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream
        .write_all(head.as_bytes())
        .await
        .expect("write response head");
    stream
        .write_all(payload.as_bytes())
        .await
        .expect("write response body");
}
