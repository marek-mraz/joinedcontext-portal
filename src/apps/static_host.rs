//! Static apps served under the platform host at `/apps/{name}/` (AP-14).
//!
//! A `static` app is a built bundle in the app artifact root, one directory per app. Three
//! things decide what leaves this module: the app's manifest must say `lifecycle: published`
//! (AP-18), every file must match the Subresource Integrity digest its build lane recorded in
//! `integrity.json` (AP-12), and every response carries the app's own Content Security Policy
//! instead of the Portal's (AP-12).

use std::path::{Path as FsPath, PathBuf};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::Engine;
use jc_core::kinds::{AppLifecycle, AppSpec};
use sha2::{Digest, Sha384};

use crate::error::ApiError;
use crate::state::AppState;

/// Digest map written by the build lane beside the bundle: bundle-relative path →
/// `sha384-<base64>`. An app directory without it serves nothing; the manifest is what makes a
/// bundle publishable, not an optional extra (AP-12).
pub const INTEGRITY_MANIFEST: &str = "integrity.json";

/// Serves `/apps/{name}/` — the app's own index.
async fn serve_index(user: OptionalUser, state: State<AppState>, name: Path<String>) -> Response {
    serve(user, state, Path((name.0, "index.html".to_string()))).await
}

/// Serves `/apps/{name}/{path}`.
async fn serve(
    OptionalUser(user): OptionalUser,
    State(state): State<AppState>,
    Path((name, path)): Path<(String, String)>,
) -> Response {
    // Every refusal below is the same 404. A draft app, a retired app, a name that was never
    // created and a caller who may not see it are indistinguishable from outside, so the host
    // never discloses what is being worked on (AP-18).
    let not_found = || ApiError::NotFound(format!("app '{name}' not found")).into_response();

    let Some(spec) = published_app(&state, &name) else {
        return not_found();
    };
    if !may_read(&spec, user.is_some()) {
        return not_found();
    }
    let Some(root) = state.config.apps_dir.as_deref() else {
        return not_found();
    };
    let Some(file) = resolve(FsPath::new(root), &name, &path) else {
        return not_found();
    };
    let Ok(bytes) = std::fs::read(&file) else {
        return not_found();
    };

    let Some(expected) = integrity_of(FsPath::new(root), &name, &path) else {
        // No recorded digest means the file is not part of the published bundle, even if it
        // sits in the directory.
        return not_found();
    };
    if expected != sri_sha384(&bytes) {
        // A bundle that no longer matches what CI signed is never served under a published
        // app's name; the operator gets the log line, the caller gets a plain refusal.
        tracing::error!(app = %name, path = %path, "app asset fails its recorded integrity digest");
        return ApiError::Unavailable("the app bundle failed its integrity check".into())
            .into_response();
    }

    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        // The bundle is immutable per publish, but a republish reuses the path, so assets are
        // revalidated rather than pinned for a year.
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());

    let headers = response.headers_mut();
    if let Ok(csp) = HeaderValue::from_str(&content_security_policy(&spec)) {
        headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static_str_or_deny(spec.embeddable),
    );
    response
}

/// `X-Frame-Options` follows the app's own `frame-ancestors`: an app that may not be framed is
/// refused by both headers, an embeddable one is left to the CSP alone.
trait FrameOptions {
    fn from_static_str_or_deny(embeddable: bool) -> HeaderValue;
}

impl FrameOptions for HeaderValue {
    fn from_static_str_or_deny(embeddable: bool) -> HeaderValue {
        if embeddable {
            HeaderValue::from_static("SAMEORIGIN")
        } else {
            HeaderValue::from_static("DENY")
        }
    }
}

/// The published App manifest of this name, whatever project owns it. App names are
/// DNS-1123 labels and the app URL has no project segment (AP-14), so the name is what
/// identifies it here.
fn published_app(state: &AppState, name: &str) -> Option<AppSpec> {
    if !crate::resource::is_dns1123(name) {
        return None;
    }
    let envelope = state
        .mirror
        .find(|env| env.kind == "App" && env.metadata.name == name)?;
    let spec: AppSpec = serde_json::from_value(envelope.spec).ok()?;
    (spec.lifecycle == AppLifecycle::Published).then_some(spec)
}

/// Who may read a published app. `visibility` beyond "is there a session" is the endpoint
/// authorization's job; the host only refuses to serve a non-public app to an anonymous caller.
fn may_read(spec: &AppSpec, authenticated: bool) -> bool {
    matches!(spec.visibility, jc_core::kinds::AppVisibility::Public) || authenticated
}

/// The app's Content Security Policy (AP-12). `default-src` and `connect-src` stay on `'self'`
/// plus whatever the manifest adds; `frame-ancestors` is `'none'` unless the app is embeddable.
pub fn content_security_policy(spec: &AppSpec) -> String {
    let csp = spec.csp.as_ref();
    let mut connect = vec!["'self'".to_string()];
    if let Some(csp) = csp {
        connect.extend(csp.connect_src.iter().filter(|s| *s != "self").map(quoted));
    }

    let declared: Vec<String> = csp
        .map(|c| c.frame_ancestors.iter().map(quoted).collect())
        .unwrap_or_default();
    let frame_ancestors = if !declared.is_empty() && spec.embeddable {
        declared.join(" ")
    } else if spec.embeddable {
        "'self'".to_string()
    } else {
        "'none'".to_string()
    };

    format!(
        "default-src 'self'; base-uri 'self'; object-src 'none'; script-src 'self'; \
         style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; \
         form-action 'self'; connect-src {}; frame-ancestors {frame_ancestors}",
        connect.join(" ")
    )
}

/// CSP keywords are quoted, origins are not.
fn quoted(source: &String) -> String {
    match source.as_str() {
        "self" | "none" => format!("'{source}'"),
        other => other.to_string(),
    }
}

/// Resolves a bundle-relative path inside one app's directory, or `None` if it would leave it.
/// `canonicalize` is what decides: it resolves `..` and every symlink, so a link pointing out of
/// the root is caught as surely as a literal traversal.
fn resolve(root: &FsPath, app: &str, path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.ends_with('/') {
        return None;
    }
    let app_root = root.join(app).canonicalize().ok()?;
    let file = app_root.join(path).canonicalize().ok()?;
    (file.starts_with(&app_root) && file.is_file()).then_some(file)
}

/// The digest the build lane recorded for this file, if any.
fn integrity_of(root: &FsPath, app: &str, path: &str) -> Option<String> {
    let manifest = resolve(root, app, INTEGRITY_MANIFEST)?;
    let digests: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(manifest).ok()?).ok()?;
    digests.get(path).cloned()
}

/// The Subresource Integrity form of a file's digest, `sha384-<base64>` (AP-12).
pub fn sri_sha384(bytes: &[u8]) -> String {
    format!(
        "sha384-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha384::digest(bytes))
    )
}

/// A session if the caller has one, nothing if not: a public app is served to anyone, and a
/// non-public one needs a login without the route itself being a login wall.
struct OptionalUser(Option<crate::auth::CurrentUser>);

impl axum::extract::FromRequestParts<AppState> for OptionalUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(
            crate::auth::CurrentUser::from_request_parts(parts, state)
                .await
                .ok(),
        ))
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/apps/{name}/", get(serve_index))
        .route("/apps/{name}/{*path}", get(serve))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jc_core::kinds::{AppVisibility, ContentSecurityPolicy};

    fn spec() -> AppSpec {
        serde_json::from_value(serde_json::json!({
            "kind": "static",
            "source": { "path": "." },
            "build": { "node": "22" },
            "visibility": "public",
            "lifecycle": "published",
            "dataNeeds": []
        }))
        .expect("a minimal static app")
    }

    #[test]
    fn a_plain_app_may_not_be_framed_and_talks_only_to_the_platform() {
        let csp = content_security_policy(&spec());
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert!(csp.contains("connect-src 'self';"), "{csp}");
        assert!(
            !csp.contains('*'),
            "a wildcard never reaches the header: {csp}"
        );
    }

    #[test]
    fn an_embeddable_app_relaxes_only_frame_ancestors() {
        let mut spec = spec();
        spec.embeddable = true;
        spec.csp = Some(ContentSecurityPolicy {
            connect_src: vec!["self".into()],
            frame_ancestors: vec!["https://portal.example.sk".into()],
        });
        let csp = content_security_policy(&spec);
        assert!(
            csp.contains("frame-ancestors https://portal.example.sk"),
            "{csp}"
        );
        assert!(csp.contains("connect-src 'self';"), "{csp}");
    }

    #[test]
    fn a_declared_frame_ancestor_is_ignored_while_the_app_is_not_embeddable() {
        // spec.embeddable is the switch (AP-12); a frameAncestors list alone must not open it.
        let mut spec = spec();
        spec.csp = Some(ContentSecurityPolicy {
            connect_src: Vec::new(),
            frame_ancestors: vec!["https://elsewhere.example".into()],
        });
        assert!(content_security_policy(&spec).contains("frame-ancestors 'none'"));
    }

    #[test]
    fn a_public_app_needs_no_session_and_a_project_app_does() {
        assert!(may_read(&spec(), false));
        let mut private = spec();
        private.visibility = AppVisibility::Project;
        assert!(!may_read(&private, false));
        assert!(may_read(&private, true));
    }

    #[test]
    fn the_digest_is_the_subresource_integrity_form() {
        // sha384 of the empty input, the value a browser would compute for an empty asset.
        assert_eq!(
            sri_sha384(b""),
            "sha384-OLBgp1GsljhM2TJ+sbHjaiH9txEUvgdDTAzHv2P24donTt6/529l+9Ua0vFImLlb"
        );
    }

    #[test]
    fn a_path_leaving_the_app_root_does_not_resolve() {
        let dir = std::env::temp_dir().join(format!("jc-apps-{}", std::process::id()));
        let app = dir.join("demo");
        std::fs::create_dir_all(&app).expect("app dir");
        std::fs::write(dir.join("secret.txt"), b"not yours").expect("neighbour file");
        std::fs::write(app.join("index.html"), b"<!doctype html>").expect("index");

        assert!(resolve(&dir, "demo", "index.html").is_some());
        assert!(resolve(&dir, "demo", "../secret.txt").is_none());
        assert!(resolve(&dir, "demo", "/etc/passwd").is_none());
        assert!(
            resolve(&dir, "demo", "").is_none(),
            "a directory is not a file"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
