use rightclaw::memory::ResilientHindsight;
use rightclaw::memory::hindsight::HindsightClient;

pub mod mock {
    pub async fn always(status: u16, body: &str) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let body = body.to_owned();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return; };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body,
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (handle, url)
    }
}

pub async fn wrap(url: &str, source: &str) -> ResilientHindsight {
    // `into_path()` is deprecated in current tempfile; use `.keep()`.
    let dir = tempfile::tempdir().unwrap().keep();
    let _ = rightclaw::memory::open_connection(&dir, true).unwrap();
    let client = HindsightClient::new("hs_x", "b", "high", 1024, Some(url));
    ResilientHindsight::new(client, dir, source)
}
