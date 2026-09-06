use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use crate::error::ApiError;

#[derive(RustEmbed)]
#[folder = "ui/dist"]
pub struct Assets;

/// Fallback HTML served when the UI bundle has not been built yet.
pub const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>joinedcontext Portal</title>
</head>
<body>
    <div id="root">
        <h1>joinedcontext Portal</h1>
        <p>UI bundle not built. Run <code>pnpm build</code> in <code>ui/</code> to produce production assets.</p>
    </div>
</body>
</html>
"#;

/// Serves static assets from the embedded Vite build or falls back to SPA routing.
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.is_empty() || path == "index.html" {
        return serve_index();
    }

    if let Some(file) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let cache_control = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::CACHE_CONTROL, cache_control)
            .body(Body::from(file.data))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    } else {
        let last_segment = path.rsplit('/').next().unwrap_or(path);
        if last_segment.contains('.') {
            ApiError::NotFound(format!("Asset '{path}' not found")).into_response()
        } else {
            serve_index()
        }
    }
}

fn serve_index() -> Response {
    if let Some(file) = Assets::get("index.html") {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(file.data))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    } else {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(PLACEHOLDER_HTML))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_html_is_valid() {
        assert!(PLACEHOLDER_HTML.contains("<!doctype html>"));
        assert!(PLACEHOLDER_HTML.contains("UI bundle not built"));
        assert!(PLACEHOLDER_HTML.contains("</html>"));
    }

    #[test]
    fn mime_of_app_js_is_javascript() {
        let mime = mime_guess::from_path("app.js").first_or_octet_stream();
        assert!(
            mime.as_ref().contains("javascript"),
            "expected mime to contain 'javascript', got {mime}"
        );
    }
}
