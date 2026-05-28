use include_dir::{Dir, include_dir};

static DASHBOARD_ASSETS: Dir<'_> = include_dir!("$OUT_DIR/dashboard");

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

#[cfg(test)]
mod tests {
    use super::DASHBOARD_ASSETS;
    use include_dir::{Dir, DirEntry};

    fn contains_providers_view(dir: &Dir<'_>) -> bool {
        for entry in dir.entries() {
            match entry {
                DirEntry::File(f) => {
                    let path = f.path();
                    let is_js_or_html = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e == "js" || e == "html")
                        .unwrap_or(false);
                    if is_js_or_html {
                        if let Ok(s) = std::str::from_utf8(f.contents()) {
                            if s.contains("ProvidersView") {
                                return true;
                            }
                        }
                    }
                }
                DirEntry::Dir(d) => {
                    if contains_providers_view(d) {
                        return true;
                    }
                }
            }
        }
        false
    }

    #[test]
    fn dashboard_bundle_contains_providers_view() {
        assert!(
            contains_providers_view(&DASHBOARD_ASSETS),
            "DASHBOARD_ASSETS has no JS/HTML file containing 'ProvidersView' \
             — bundle is stale (vite build did not run for current source)",
        );
    }
}
