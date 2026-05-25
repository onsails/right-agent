// Shared integration-test helpers. `#[allow(dead_code)]` silences per-test-file
// dead_code warnings that fire when a helper is used in some test files but
// not others.
#![allow(dead_code)]

use right_memory::ResilientHindsight;
use right_memory::hindsight::HindsightClient;

pub mod mock {
    pub async fn always(status: u16, body: &str) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let body = body.to_owned();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body,
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (handle, url)
    }
}

/// Install ring as the rustls process-level crypto provider. Idempotent —
/// safe to call from multiple tests in the same binary.
pub fn setup_crypto() {
    // install_default returns Err(existing provider Arc) when already
    // installed by another test in the same binary — that's not a failure.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub async fn wrap(url: &str, source: &str) -> ResilientHindsight {
    setup_crypto();
    // `into_path()` is deprecated in current tempfile; use `.keep()`.
    let dir = tempfile::tempdir().unwrap().keep();
    let _ = right_db::open_connection(&dir, true).await.unwrap();
    let client = HindsightClient::new("hs_x", "b", "high", 1024, Some(url));
    ResilientHindsight::new(client, dir, source)
}

pub mod switch {
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Clone)]
    pub struct ResponseSwitch {
        pub inner: Arc<Mutex<(u16, String)>>,
    }

    impl ResponseSwitch {
        pub fn new(status: u16, body: &str) -> Self {
            Self {
                inner: Arc::new(Mutex::new((status, body.to_owned()))),
            }
        }

        pub async fn set(&self, status: u16, body: &str) {
            *self.inner.lock().await = (status, body.to_owned());
        }
    }

    pub async fn server(switch: ResponseSwitch) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = s.read(&mut buf).await;
                let (status, body) = switch.inner.lock().await.clone();
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body,
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (handle, url)
    }
}
