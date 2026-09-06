//! The React build, embedded in the binary so the image is one artifact (AP-25).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use crate::App;

#[derive(RustEmbed)]
#[folder = "ui/dist"]
pub struct Assets;

/// Shown when the binary was built without a UI bundle, so the reason is on the page instead
/// of in a log nobody reads.
pub const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8" /><title>Air quality</title></head>
<body><p>UI bundle not built. Run <code>pnpm build</code> in <code>ui/</code>.</p></body>
</html>
"#;

pub async fn static_handler(State(app): State<Arc<App>>, uri: Uri) -> Response {
    let base = app.config.base_path.trim_end_matches('/');
    let path = uri
        .path()
        .strip_prefix(base)
        .unwrap_or(uri.path())
        .trim_start_matches('/');
    if path.is_empty() || path == "index.html" {
        return index();
    }
    let Some(file) = Assets::get(path) else {
        // Anything with an extension is a real miss; everything else is a client route.
        return if path.rsplit('/').next().unwrap_or(path).contains('.') {
            StatusCode::NOT_FOUND.into_response()
        } else {
            index()
        };
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref()),
            (header::CACHE_CONTROL, cache),
        ],
        Body::from(file.data),
    )
        .into_response()
}

fn index() -> Response {
    let body = Assets::get("index.html")
        .map(|file| Body::from(file.data))
        .unwrap_or_else(|| Body::from(PLACEHOLDER_HTML));
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}
