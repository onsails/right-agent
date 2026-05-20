use include_dir::{Dir, include_dir};

static DASHBOARD_ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/static/dashboard");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DashboardAsset {
    pub bytes: &'static [u8],
    pub content_type: &'static str,
}

pub fn asset(path: &str) -> Option<DashboardAsset> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    DASHBOARD_ASSETS.get_file(path).map(|file| DashboardAsset {
        bytes: file.contents(),
        content_type: content_type(path),
    })
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}
