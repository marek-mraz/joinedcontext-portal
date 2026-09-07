//! Live mirror synchronization status endpoint (MF-04, CC-08).

use axum::extract::State;
use axum::Json;

use crate::auth::CurrentUser;
use crate::error::ProblemDetails;
use crate::reconciler::SyncStatus;
use crate::state::AppState;

#[utoipa::path(
    get,
    path = "/api/v1/sync",
    tag = "system",
    responses(
        (status = 200, description = "Status of the background Git mirror synchronization", body = SyncStatus),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
    )
)]
pub async fn get_sync_status(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> Json<SyncStatus> {
    let status = state
        .syncer
        .as_ref()
        .map(|s| s.status())
        .unwrap_or_default();
    Json(status)
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route("/sync", axum::routing::get(get_sync_status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use axum_extra::extract::cookie::PrivateCookieJar;
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::auth::session::{self, Identity, Session};
    use crate::config::Config;
    use crate::git::GiteaClient;
    use crate::reconciler::Syncer;
    use crate::server;
    use crate::store::Mirror;

    fn make_session_cookie(config: &Config) -> String {
        use axum::response::IntoResponse;
        let now = session::now_unix();
        let s = Session {
            identity: Identity {
                subject: "f:1:demo.steward".into(),
                username: "demo.steward".into(),
                email: Some("demo.steward@banskabystrica.sk".into()),
                name: Some("Demo Steward".into()),
                roles: Vec::new(),
            },
            expires_at: now + 3600,
            issued_at: now,
            id_token: "id-token-placeholder".into(),
            access_expires_at: now + 3600,
            refresh_token: None,
        };
        let jar = PrivateCookieJar::new(config.cookie_key.clone());
        let jar = session::store(jar, &s).expect("store session");
        let response = (jar, StatusCode::OK).into_response();
        let mut parts = Vec::new();
        for value in response.headers().get_all(header::SET_COOKIE) {
            let raw = value.to_str().expect("cookie header");
            let pair = raw.split(';').next().unwrap_or_default();
            parts.push(pair.to_string());
        }
        parts.join("; ")
    }

    #[tokio::test]
    async fn sync_status_anonymous_returns_401() {
        let app = server::app(AppState::new(Config::for_tests(), None));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/sync")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
    }

    #[tokio::test]
    async fn sync_status_authenticated_without_syncer_returns_default() {
        let config = Config::for_tests();
        let cookie = make_session_cookie(&config);
        let app = server::app(AppState::new(config, None));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/sync")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let status: SyncStatus = serde_json::from_slice(&body).unwrap();
        assert_eq!(status.manifests, 0);
        assert!(status.last_sync.is_none());
        assert!(status.revision.is_none());
        assert!(status.last_error.is_none());
    }

    #[tokio::test]
    async fn sync_status_authenticated_with_syncer_returns_status() {
        let config = Config::for_tests();
        let cookie = make_session_cookie(&config);
        let gitea = Arc::new(
            GiteaClient::new(
                "http://localhost:3000".parse().unwrap(),
                "test-owner",
                "test-repo",
                "token",
            )
            .unwrap(),
        );
        let mirror = Arc::new(Mirror::new());
        let syncer = Arc::new(Syncer::new(gitea, mirror));
        let state = AppState::new(config, None).with_syncer(syncer);
        let app = server::app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/sync")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let status: SyncStatus = serde_json::from_slice(&body).unwrap();
        assert_eq!(status.manifests, 0);
    }
}
