//! Keeps a cookie session alive for as long as the Keycloak SSO session allows (CC-40, CC-42).
//!
//! The access token Keycloak issues lives five minutes; the session must not. Inside the leeway
//! before it expires, the first request that notices trades the refresh token for a new one,
//! rotates both cookies and carries on. Only a refused refresh ends the session: the cookies are
//! cleared and a browser navigation is sent to the login page, an API call gets `401`.

use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::PrivateCookieJar;

use crate::auth::session;
use crate::error::ApiError;
use crate::state::AppState;

/// Paths the refresh must leave alone: the login flow mints the session, logout ends it, the
/// back channel carries no session, and static assets carry nothing worth a token round trip.
fn is_exempt(path: &str) -> bool {
    path.starts_with("/assets/")
        || path.starts_with("/api/v1/auth/") && path != "/api/v1/auth/me"
        || path == "/login"
}

/// A top-level browser navigation, as opposed to a `fetch` from the SPA.
fn is_navigation(request: &Request) -> bool {
    request
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

/// The login page with the interrupted location to come back to.
pub fn login_redirect(path_and_query: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("redirect_to", path_and_query)
        .finish();
    format!("/login?{query}")
}

/// Middleware over the portal routes: refreshes a session that is due, ends one that cannot be.
pub async fn middleware(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    // A bearer caller is verified per request and never refreshed by the portal.
    if is_exempt(&path) || request.headers().contains_key(header::AUTHORIZATION) {
        return next.run(request).await;
    }
    let jar = PrivateCookieJar::from_headers(request.headers(), state.config.cookie_key.clone());
    let Some(current) = session::load(&jar) else {
        return next.run(request).await;
    };
    let now = session::now_unix();
    if !current.needs_refresh(now) {
        return next.run(request).await;
    }
    let refreshed = match state.oidc.as_deref() {
        Some(client) => client.refresh(&current).await,
        None => Err(ApiError::Unavailable(
            "no identity provider is configured".into(),
        )),
    };
    match refreshed {
        Ok(fresh) => {
            let mut request = request;
            request.extensions_mut().insert(fresh.clone());
            let response = next.run(request).await;
            match session::store(jar, &fresh) {
                Ok(jar) => (jar, response).into_response(),
                Err(err) => err.into_response(),
            }
        }
        // Still inside the leeway: this request runs on the token it has, the next one retries.
        // Parallel requests from one page share a refresh token this way without a lock.
        Err(_) if !current.access_expired(now) => next.run(request).await,
        Err(_) => {
            tracing::info!(subject = %current.identity.subject, "session ended: refresh refused");
            let jar = session::clear(jar);
            if is_navigation(&request) {
                let target = request
                    .uri()
                    .path_and_query()
                    .map(|pq| pq.as_str().to_string())
                    .unwrap_or_else(|| path.clone());
                (jar, Redirect::to(&login_redirect(&target))).into_response()
            } else {
                (jar, ApiError::Unauthorized).into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_flow_and_assets_are_left_alone() {
        assert!(is_exempt("/api/v1/auth/login"));
        assert!(is_exempt("/api/v1/auth/callback"));
        assert!(is_exempt("/api/v1/auth/logout"));
        assert!(is_exempt("/api/v1/auth/backchannel-logout"));
        assert!(is_exempt("/assets/index-abc123.js"));
        assert!(is_exempt("/login"));
        assert!(!is_exempt("/api/v1/auth/me"));
        assert!(!is_exempt("/api/v1/projects/helsinki/spaces"));
        assert!(!is_exempt("/projects/helsinki/spaces"));
    }

    #[test]
    fn the_login_redirect_keeps_the_interrupted_location() {
        assert_eq!(
            login_redirect("/projects/helsinki/spaces?page=2"),
            "/login?redirect_to=%2Fprojects%2Fhelsinki%2Fspaces%3Fpage%3D2"
        );
    }

    #[test]
    fn only_an_html_navigation_is_redirected() {
        let nav = Request::builder()
            .header(header::ACCEPT, "text/html,application/xhtml+xml")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(is_navigation(&nav));
        let fetch = Request::builder()
            .header(header::ACCEPT, "application/json")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(!is_navigation(&fetch));
    }
}
