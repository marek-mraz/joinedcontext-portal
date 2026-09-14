//! Basemap style and tile proxy routes for application map views (AP-67, T-0555).
//!
//! Sandboxed preview frames have no session and cannot reach third-party map providers
//! without leaking user activity and coordinates. All basemap requests flow through this
//! unauthenticated platform route, returning the configured style and cached raster tiles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use axum::extract::{Path as AxPath, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::config::Config;
use crate::error::ProblemDetails;
use crate::state::AppState;

/// Shared HTTP client for fetching upstream tiles.
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("joinedcontext-portal basemap")
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
});

// ponytail: the token buckets live in one replica's memory, so N replicas allow N times the cap;
// share them only when the Portal runs more than one pod. The client is the first
// X-Forwarded-For hop, which the ingress rewrites for untrusted callers.
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

static RATE_LIMITER: LazyLock<Mutex<HashMap<String, TokenBucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn check_rate_limit(client_ip: &str) -> bool {
    let mut map = RATE_LIMITER.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();

    if map.len() > 10_000 {
        map.retain(|_, b| now.duration_since(b.last_refill).as_secs() < 120);
    }

    let bucket = map
        .entry(client_ip.to_string())
        .or_insert_with(|| TokenBucket {
            tokens: 600.0,
            last_refill: now,
        });
    let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
    bucket.tokens = (bucket.tokens + elapsed * 10.0).min(600.0);
    bucket.last_refill = now;
    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        true
    } else {
        false
    }
}

fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

fn problem_response(
    status: StatusCode,
    slug: &str,
    title: &str,
    detail: Option<String>,
) -> Response {
    let problem = ProblemDetails {
        r#type: format!("https://joinedcontext.com/errors/{slug}"),
        title: title.to_string(),
        status: status.as_u16(),
        detail,
        instance: None,
        errors: None,
    };
    (
        status,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/problem+json"),
            ),
            (
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            ),
        ],
        serde_json::to_string(&problem).unwrap_or_default(),
    )
        .into_response()
}

/// Returns the absolute style URL for a given project if basemap proxying is configured.
pub fn style_url(config: &Config, project: &str) -> Option<String> {
    config.basemap.as_ref()?;
    let base = config.public_base_url.as_str().trim_end_matches('/');
    Some(format!(
        "{base}/api/v1/projects/{project}/basemap/default/style.json"
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/basemap/{style}/style.json",
    tag = "basemap",
    params(
        ("project" = String, Path, description = "Project name"),
        ("style" = String, Path, description = "Basemap style (default)"),
    ),
    responses(
        (status = 200, description = "MapLibre style v8 JSON", content_type = "application/json"),
        (status = 404, description = "Not found or basemap not configured", body = ProblemDetails)
    )
)]
pub async fn get_style(
    State(state): State<AppState>,
    AxPath((project, style)): AxPath<(String, String)>,
) -> Response {
    let Some(basemap) = state.config.basemap.as_ref() else {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some("basemap is not configured on this platform".to_string()),
        );
    };

    if style != "default" {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some(format!("basemap style '{style}' not found")),
        );
    }

    if !state.mirror.namespaces().iter().any(|p| p == &project) {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some(format!("project '{project}' not found")),
        );
    }

    let base = state.config.public_base_url.as_str().trim_end_matches('/');
    let tile_url =
        format!("{base}/api/v1/projects/{project}/basemap/default/{{z}}/{{x}}/{{y}}.png");
    let style_json = serde_json::json!({
        "version": 8,
        "sources": {
            "raster-tiles": {
                "type": "raster",
                "tiles": [tile_url],
                "tileSize": 256,
                "maxzoom": basemap.max_zoom,
                "attribution": basemap.attribution,
            }
        },
        "layers": [
            {
                "id": "raster-layer",
                "type": "raster",
                "source": "raster-tiles",
                "minzoom": 0,
                "maxzoom": basemap.max_zoom,
            }
        ]
    });

    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
            (
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            ),
        ],
        serde_json::to_string_pretty(&style_json).unwrap_or_default(),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/basemap/{style}/{z}/{x}/{tile}",
    tag = "basemap",
    params(
        ("project" = String, Path, description = "Project name"),
        ("style" = String, Path, description = "Basemap style (default)"),
        ("z" = u8, Path, description = "Zoom level"),
        ("x" = u32, Path, description = "Tile X coordinate"),
        ("tile" = String, Path, description = "Tile Y coordinate with extension, e.g. 0.png"),
    ),
    responses(
        (status = 200, description = "Tile image", content_type = "image/png"),
        (status = 400, description = "Invalid tile coordinates", body = ProblemDetails),
        (status = 404, description = "Not found or basemap not configured", body = ProblemDetails),
        (status = 429, description = "Rate limit exceeded", body = ProblemDetails),
        (status = 502, description = "Bad gateway from upstream", body = ProblemDetails)
    )
)]
pub async fn get_tile(
    State(state): State<AppState>,
    AxPath((project, style, z_str, x_str, tile)): AxPath<(String, String, String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(basemap) = state.config.basemap.as_ref() else {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some("basemap is not configured on this platform".to_string()),
        );
    };

    if style != "default" {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some(format!("basemap style '{style}' not found")),
        );
    }

    if !state.mirror.namespaces().iter().any(|p| p == &project) {
        return problem_response(
            StatusCode::NOT_FOUND,
            "resource-not-found",
            "Resource Not Found",
            Some(format!("project '{project}' not found")),
        );
    }

    let z = match z_str.parse::<u8>() {
        Ok(val) => val,
        Err(_) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_REQUEST,
                "invalid-request",
                "Invalid Request",
                Some(format!("invalid zoom level: {z_str}")),
            );
        }
    };
    let x = match x_str.parse::<u32>() {
        Ok(val) => val,
        Err(_) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_REQUEST,
                "invalid-request",
                "Invalid Request",
                Some(format!("invalid tile x coordinate: {x_str}")),
            );
        }
    };
    let (y_str, ext) = match tile.split_once('.') {
        Some((y, ext)) => (y, ext),
        None => (tile.as_str(), "png"),
    };
    let y = match y_str.parse::<u32>() {
        Ok(val) => val,
        Err(_) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_REQUEST,
                "invalid-request",
                "Invalid Request",
                Some(format!("invalid tile y coordinate: {y_str}")),
            );
        }
    };

    if ext != "png" && ext != "jpg" && ext != "jpeg" {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_REQUEST,
            "invalid-request",
            "Invalid Request",
            Some(format!("unsupported tile extension: {ext}")),
        );
    }

    if z > basemap.max_zoom {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_REQUEST,
            "invalid-request",
            "Invalid Request",
            Some(format!(
                "zoom level {z} exceeds maximum zoom {}",
                basemap.max_zoom
            )),
        );
    }

    let limit = 1u64 << z;
    if (x as u64) >= limit || (y as u64) >= limit {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_REQUEST,
            "invalid-request",
            "Invalid Request",
            Some(format!(
                "coordinates ({x}, {y}) out of range for zoom level {z} (max {limit})"
            )),
        );
    }

    let ip = client_ip(&headers);
    if !check_rate_limit(&ip) {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::TOO_MANY_REQUESTS,
            "rate-limit-exceeded",
            "Too Many Requests",
            Some("rate limit of 600 requests per minute exceeded".to_string()),
        );
    }

    let cache_key = format!("{style}_{z}_{x}_{y}.{ext}");
    let cache_path = basemap.cache_dir.join(&cache_key);

    if let Ok(metadata) = std::fs::metadata(&cache_path) {
        if let Ok(modified) = metadata.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                if elapsed.as_secs() < basemap.cache_ttl_secs {
                    if let Ok(bytes) = std::fs::read(&cache_path) {
                        metrics::counter!("jc_basemap_tiles_total", "result" => "hit").increment(1);
                        let content_type = if ext == "jpg" || ext == "jpeg" {
                            "image/jpeg"
                        } else {
                            "image/png"
                        };
                        return (
                            StatusCode::OK,
                            [
                                (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
                                (
                                    header::CACHE_CONTROL,
                                    HeaderValue::from_static("public, max-age=3600"),
                                ),
                                (
                                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                                    HeaderValue::from_static("*"),
                                ),
                            ],
                            bytes,
                        )
                            .into_response();
                    }
                }
            }
        }
    }

    let mut upstream_url = basemap.url_template.clone();
    upstream_url = upstream_url
        .replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string())
        .replace("{ext}", ext)
        .replace("{key}", basemap.key.as_deref().unwrap_or(""));

    let is_https = upstream_url.starts_with("https://");
    let is_allowed_http = basemap.allow_http && upstream_url.starts_with("http://");
    if !is_https && !is_allowed_http {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_GATEWAY,
            "bad-gateway",
            "Bad Gateway",
            Some("upstream URL scheme must be https".to_string()),
        );
    }

    let response = match HTTP_CLIENT.get(&upstream_url).send().await {
        Ok(res) => res,
        Err(e) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_GATEWAY,
                "bad-gateway",
                "Bad Gateway",
                // A reqwest error names its URL, and the URL carries the key.
                Some(format!("upstream fetch failed: {}", e.without_url())),
            );
        }
    };

    if !response.status().is_success() {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_GATEWAY,
            "bad-gateway",
            "Bad Gateway",
            Some(format!("upstream returned HTTP {}", response.status())),
        );
    }

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let is_valid_image =
        content_type.starts_with("image/png") || content_type.starts_with("image/jpeg");
    if !is_valid_image {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_GATEWAY,
            "bad-gateway",
            "Bad Gateway",
            Some(format!(
                "upstream returned non-image content: {content_type}"
            )),
        );
    }

    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
            return problem_response(
                StatusCode::BAD_GATEWAY,
                "bad-gateway",
                "Bad Gateway",
                Some(format!("failed to read upstream body: {}", e.without_url())),
            );
        }
    };

    if bytes.len() > 1024 * 1024 {
        metrics::counter!("jc_basemap_tiles_total", "result" => "refused").increment(1);
        return problem_response(
            StatusCode::BAD_GATEWAY,
            "bad-gateway",
            "Bad Gateway",
            Some("upstream tile exceeds 1 MiB limit".to_string()),
        );
    }

    // The cache walk reads the whole directory, so it runs off the async workers.
    let (dir, max_bytes, stored) = (
        basemap.cache_dir.clone(),
        basemap.cache_max_bytes,
        bytes.clone(),
    );
    if let Err(err) =
        tokio::task::spawn_blocking(move || save_and_evict(&dir, &cache_path, &stored, max_bytes))
            .await
    {
        tracing::warn!(error = %err, "basemap cache write did not finish");
    }
    metrics::counter!("jc_basemap_tiles_total", "result" => "fetch").increment(1);

    let final_ct = if content_type.starts_with("image/jpeg") {
        "image/jpeg"
    } else {
        "image/png"
    };

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(final_ct)),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
            (
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            ),
        ],
        bytes,
    )
        .into_response()
}

fn save_and_evict(cache_dir: &Path, file_path: &Path, bytes: &[u8], max_bytes: u64) {
    if let Err(e) = std::fs::create_dir_all(cache_dir) {
        tracing::warn!("failed to create basemap cache dir: {e}");
        return;
    }
    if let Err(e) = std::fs::write(file_path, bytes) {
        tracing::warn!("failed to write basemap cache tile: {e}");
        return;
    }

    struct Entry {
        path: PathBuf,
        size: u64,
        modified: SystemTime,
    }

    let mut entries = Vec::new();
    let mut total_size = 0u64;

    if let Ok(read_dir) = std::fs::read_dir(cache_dir) {
        for entry in read_dir.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    let size = meta.len();
                    total_size += size;
                    let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    entries.push(Entry {
                        path: entry.path(),
                        size,
                        modified,
                    });
                }
            }
        }
    }

    if total_size > max_bytes {
        entries.sort_by_key(|e| e.modified);
        for entry in entries {
            if total_size <= max_bytes {
                break;
            }
            if let Ok(()) = std::fs::remove_file(&entry.path) {
                total_size = total_size.saturating_sub(entry.size);
            }
        }
    }
}

/// The basemap routes for application views.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/basemap/{style}/style.json",
            get(get_style),
        )
        .route(
            "/projects/{project}/basemap/{style}/{z}/{x}/{tile}",
            get(get_tile),
        )
}
