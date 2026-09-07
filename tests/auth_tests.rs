//! T-0181: Keycloak OIDC authorization code flow with PKCE and encrypted cookie sessions.
//! T-0508: the edge's `X-Access-Token` verified like a bearer, ES256 and RS256 (ADR-N-019).
//!
//! The realm is a wiremock stand-in: discovery is enough to exercise the flow start,
//! the state check and the failure paths without a live Keycloak, and its JWKS carries one
//! ES256 and one RS256 key, the way a realm with the `edge` client's override does.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Shaped like a Keycloak realm: the issuer carries the `/realms/{realm}` path.
const REALM_PATH: &str = "/realms/banskabystrica";

fn issuer_of(server: &MockServer) -> String {
    format!("{}{REALM_PATH}", server.uri().trim_end_matches('/'))
}

async fn realm() -> MockServer {
    let server = MockServer::start().await;
    let issuer = issuer_of(&server);
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM_PATH}/.well-known/openid-configuration"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "end_session_endpoint": format!("{issuer}/protocol/openid-connect/logout"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/certs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(keys::jwks()))
        .mount(&server)
        .await;
    server
}

async fn app_with_realm(server: &MockServer) -> axum::Router {
    app_behind(server, false).await
}

/// `edge` is what the deployment sets behind APISIX: `JC_TRUST_EDGE_TOKEN=true` (ADR-N-019).
async fn app_behind(server: &MockServer, edge: bool) -> axum::Router {
    let mut config = Config::from_vars(|k| match k {
        "JC_OIDC_ISSUER" => Some(issuer_of(server)),
        "JC_OIDC_CLIENT_ID" => Some("joinedcontext-portal".to_string()),
        "JC_OIDC_CLIENT_SECRET" => Some("test-secret".to_string()),
        "JC_PORTAL_COOKIE_KEY" => Some("k".repeat(64)),
        "JC_TRUST_EDGE_TOKEN" if edge => Some("true".to_string()),
        _ => None,
    })
    .expect("config");
    config.public_base_url = "https://portal.test".parse().expect("url");
    let state = AppState::from_config(config).await.expect("discovery");
    server::app(state)
}

/// The realm's two signing keys, generated once per test binary: ES256 for every client and
/// RS256 for the `edge` client (AP-27). Generated rather than committed: a private key literal
/// in the repository trips gitleaks, and rightly so.
mod keys {
    use std::sync::OnceLock;

    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use p256::pkcs8::EncodePrivateKey as _;
    use rsa::pkcs1::EncodeRsaPrivateKey as _;
    use rsa::traits::PublicKeyParts as _;
    use serde_json::{json, Value};

    struct Realm {
        es256: EncodingKey,
        rs256: EncodingKey,
        jwks: Value,
    }

    fn realm() -> &'static Realm {
        static REALM: OnceLock<Realm> = OnceLock::new();
        REALM.get_or_init(|| {
            let ec = p256::SecretKey::random(&mut rand_core::OsRng);
            let mut ec_jwk: Value =
                serde_json::from_str(&ec.public_key().to_jwk_string()).expect("a jwk");
            ec_jwk["kid"] = json!("realm-es256");
            ec_jwk["alg"] = json!("ES256");
            ec_jwk["use"] = json!("sig");

            let rsa = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048).expect("an rsa key");
            let rsa_jwk = json!({
                "kty": "RSA",
                "kid": "realm-rs256",
                "alg": "RS256",
                "use": "sig",
                "n": URL_SAFE_NO_PAD.encode(rsa.n().to_bytes_be()),
                "e": URL_SAFE_NO_PAD.encode(rsa.e().to_bytes_be()),
            });

            Realm {
                es256: EncodingKey::from_ec_der(ec.to_pkcs8_der().expect("der").as_bytes()),
                rs256: EncodingKey::from_rsa_der(rsa.to_pkcs1_der().expect("der").as_bytes()),
                jwks: json!({ "keys": [ec_jwk, rsa_jwk] }),
            }
        })
    }

    pub fn jwks() -> Value {
        realm().jwks.clone()
    }

    fn now() -> i64 {
        joinedcontext_portal::auth::session::now_unix()
    }

    /// An access token of `issuer` for the Portal, as Keycloak would mint it for `username`.
    pub fn token(algorithm: Algorithm, issuer: &str, username: &str, expires_in: i64) -> String {
        let (kid, key) = match algorithm {
            Algorithm::ES256 => ("realm-es256", &realm().es256),
            Algorithm::RS256 => ("realm-rs256", &realm().rs256),
            other => panic!("the realm does not sign {other:?}"),
        };
        let mut header = Header::new(algorithm);
        header.kid = Some(kid.into());
        encode(
            &header,
            &json!({
                "iss": issuer,
                "aud": "joinedcontext-portal",
                "sub": format!("f:1:{username}"),
                "preferred_username": username,
                "exp": now() + expires_in,
                "iat": now(),
                "realm_access": { "roles": ["portal-viewer"] },
            }),
            key,
        )
        .expect("a signed token")
    }
}

fn set_cookie_values(response: &axum::response::Response) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn login_starts_a_pkce_flow_and_parks_the_verifier_in_a_secure_cookie() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(location.contains("code_challenge="), "{location}");
    assert!(
        location.contains("code_challenge_method=S256"),
        "{location}"
    );
    assert!(location.contains("state="), "{location}");
    assert!(location.contains("nonce="), "{location}");
    assert!(location.contains("scope=openid"), "{location}");
    assert!(
        location.contains("redirect_uri=https%3A%2F%2Fportal.test%2Fapi%2Fv1%2Fauth%2Fcallback"),
        "{location}"
    );

    let cookies = set_cookie_values(&response);
    let flow = cookies
        .iter()
        .find(|c| c.starts_with("jc_oidc_flow="))
        .expect("flow cookie");
    assert!(flow.contains("HttpOnly"), "{flow}");
    assert!(flow.contains("Secure"), "{flow}");
    assert!(flow.contains("SameSite=Lax"), "{flow}");
}

#[tokio::test]
async fn callback_without_a_login_in_flight_is_refused() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/callback?code=abc&state=xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn callback_with_a_foreign_state_is_refused() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let started = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let flow_cookie = set_cookie_values(&started)
        .into_iter()
        .find(|c| c.starts_with("jc_oidc_flow="))
        .map(|c| c.split(';').next().unwrap_or_default().to_string())
        .expect("flow cookie");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/callback?code=abc&state=not-the-state-we-issued")
                .header(header::COOKIE, flow_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem["type"],
        "https://joinedcontext.com/errors/invalid-request"
    );
    assert!(problem["detail"].as_str().unwrap().contains("state"));
}

#[tokio::test]
async fn callback_reports_a_provider_side_error() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/callback?error=access_denied&error_description=user%20said%20no")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn me_without_a_session_is_unauthorized() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
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
async fn login_without_a_configured_realm_is_unavailable() {
    let app = server::app(AppState::new(Config::for_tests(), None));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn a_mutation_without_the_csrf_token_is_forbidden() {
    let app = server::app(AppState::new(Config::for_tests(), None));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn a_mutation_with_a_matching_csrf_token_passes_the_gate() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let token = "double-submit-token";

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(header::COOKIE, format!("jc_csrf={token}"))
                .header("x-csrf-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "logout answers once CSRF passes"
    );
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );

    // ADR-N-019, AP-29: no session cookie came with the request, and all three Portal cookies
    // are still told to go, because the edge's own logout never reaches this handler.
    let cookies = set_cookie_values(&response);
    for name in ["jc_session", "jc_refresh", "jc_csrf"] {
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with(&format!("{name}=")) && c.contains("Max-Age=0")),
            "{name} is not cleared: {cookies:?}"
        );
    }

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let target: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert!(
        target["endSessionUrl"]
            .as_str()
            .is_some_and(|u| !u.is_empty()),
        "the SPA needs somewhere to navigate: {target}"
    );
}

/// The authorization request names each scope once (the client adds `openid` itself).
#[tokio::test]
async fn the_login_asks_for_each_scope_once() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    let scope = url::Url::parse(location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "scope")
        .map(|(_, v)| v.into_owned())
        .expect("a scope");
    assert_eq!(scope, "openid profile email", "{location}");
}

/// A token response the way Keycloak answers `grant_type=refresh_token`: the refresh token is a
/// JWT whose `exp` is the SSO session's remaining life. Nothing is signed; the portal reads the
/// refresh token's `exp` as a hint and never verifies it.
fn refresh_token_with_exp(exp: i64) -> String {
    use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine};
    format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"HS512"}"#),
        URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp},"typ":"Refresh"}}"#).as_bytes()),
        URL_SAFE_NO_PAD.encode(b"not-a-signature"),
    )
}

/// The cookies a browser would hold after a login, with the access token `access_in` seconds
/// from expiry, encrypted with the test key `app_with_realm` configures.
fn session_cookies(access_in: i64) -> String {
    use axum::response::IntoResponse;
    use axum_extra::extract::cookie::{Key, PrivateCookieJar};
    use joinedcontext_portal::auth::session::{now_unix, store, Identity, Session};

    let now = now_unix();
    let session = Session {
        identity: Identity {
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: None,
            name: None,
            roles: vec!["portal-viewer".into()],
            groups: Vec::new(),
        },
        expires_at: now + 3600,
        issued_at: now - 600,
        id_token: "opaque-id-token-for-tests".into(),
        access_expires_at: now + access_in,
        refresh_token: Some("opaque-refresh-token-for-tests".into()),
    };
    let key = Key::from("k".repeat(64).as_bytes());
    let jar = store(PrivateCookieJar::new(key), &session).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    set_cookie_values(&response)
        .iter()
        .filter_map(|c| c.split(';').next().map(str::to_string))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn mount_token_endpoint(realm: &MockServer, template: ResponseTemplate, expected: u64) {
    Mock::given(method("POST"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/token")))
        .and(wiremock::matchers::body_string_contains(
            "grant_type=refresh_token",
        ))
        .and(wiremock::matchers::body_string_contains(
            "refresh_token=opaque-refresh-token-for-tests",
        ))
        .respond_with(template)
        .expect(expected)
        .mount(realm)
        .await;
}

#[tokio::test]
async fn a_session_near_access_expiry_is_refreshed_and_both_cookies_rotate() {
    let realm = realm().await;
    let sso_ends = joinedcontext_portal::auth::session::now_unix() + 3000;
    mount_token_endpoint(
        &realm,
        ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access-token",
            "token_type": "Bearer",
            "expires_in": 300,
            "refresh_token": refresh_token_with_exp(sso_ends),
            "refresh_expires_in": 3000,
            "session_state": "keycloak-adds-fields",
        })),
        1,
    )
    .await;
    let app = app_with_realm(&realm).await;

    // Past the access token, inside the leeway: the only thing that keeps the user signed in.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .header(header::ACCEPT, "application/json")
                .header(header::COOKIE, session_cookies(-5))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the refreshed session serves the request"
    );
    let cookies = set_cookie_values(&response);
    let session = cookies
        .iter()
        .find(|c| c.starts_with("jc_session="))
        .expect("rotated session cookie");
    let refresh = cookies
        .iter()
        .find(|c| c.starts_with("jc_refresh="))
        .expect("rotated refresh cookie");
    for cookie in [session, refresh] {
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("Secure"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(!cookie.contains("Max-Age=0"), "{cookie}");
        assert!(
            cookie.len() < 4096,
            "a cookie must stay under the browser limit"
        );
    }
    // The session now lives as long as the SSO session: roughly 3000 s, not the 300 s token.
    let max_age: i64 = session
        .split(';')
        .find_map(|p| p.trim().strip_prefix("Max-Age="))
        .and_then(|v| v.parse().ok())
        .expect("max-age");
    assert!((2990..=3000).contains(&max_age), "{max_age}");

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let me: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(me["username"], "demo.steward");
}

#[tokio::test]
async fn a_refused_refresh_ends_the_session_with_401_for_fetch_and_login_for_navigation() {
    let realm = realm().await;
    mount_token_endpoint(
        &realm,
        ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "Session not active"
        })),
        2,
    )
    .await;
    let app = app_with_realm(&realm).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .header(header::ACCEPT, "application/json")
                .header(header::COOKIE, session_cookies(-5))
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
    let cookies = set_cookie_values(&response);
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("jc_session=") && c.contains("Max-Age=0")),
        "the session cookie is cleared: {cookies:?}"
    );
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("jc_refresh=") && c.contains("Max-Age=0")),
        "the refresh cookie is cleared: {cookies:?}"
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/helsinki/spaces?page=2")
                .header(header::ACCEPT, "text/html,application/xhtml+xml")
                .header(header::COOKIE, session_cookies(-5))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "/login?redirect_to=%2Fprojects%2Fhelsinki%2Fspaces%3Fpage%3D2"
    );
}

#[tokio::test]
async fn a_refused_refresh_inside_the_leeway_lets_the_live_token_serve_the_request() {
    let realm = realm().await;
    mount_token_endpoint(
        &realm,
        ResponseTemplate::new(400).set_body_json(json!({ "error": "invalid_grant" })),
        1,
    )
    .await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .header(header::COOKIE, session_cookies(30))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        set_cookie_values(&response).is_empty(),
        "nothing rotates and nothing is cleared while the access token still stands"
    );
}

async fn me_with(app: axum::Router, headers: &[(&str, String)]) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .uri("/api/v1/auth/me")
        .header(header::ACCEPT, "application/json");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(json!(null)))
}

/// ADR-N-019, AP-28: behind the edge the user's token arrives as `X-Access-Token` and is
/// verified exactly as `Authorization: Bearer` would be; `me` says which front it came through.
#[tokio::test]
async fn the_edge_token_is_verified_like_a_bearer_when_the_deployment_trusts_the_edge() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;
    let token = keys::token(
        jsonwebtoken::Algorithm::RS256,
        &issuer_of(&realm),
        "demo.steward",
        300,
    );

    let (status, me) = me_with(app.clone(), &[("x-access-token", token.clone())]).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["username"], "demo.steward");
    assert_eq!(me["subject"], "f:1:demo.steward");
    assert_eq!(me["roles"], json!(["portal-viewer"]));
    assert_eq!(me["front"], "edge", "the UI routes logout by this: {me}");

    // The same token as a bearer is the same person through the other front.
    let (status, me) = me_with(app.clone(), &[("authorization", format!("Bearer {token}"))]).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["front"], "bearer");

    // A protected resource route takes the edge token through the same extractor.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .header("x-access-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "the edge token authenticates every protected route, not only /me"
    );
}

/// ADR-N-019 §3.4: a Portal without APISIX in front never trusts the header, whatever it says.
#[tokio::test]
async fn the_edge_header_is_ignored_when_the_deployment_does_not_trust_the_edge() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    let token = keys::token(
        jsonwebtoken::Algorithm::RS256,
        &issuer_of(&realm),
        "demo.steward",
        300,
    );

    let (status, body) = me_with(app.clone(), &[("x-access-token", token.clone())]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // The flag is about the header, not the algorithm: the same token is still a good bearer.
    let (status, me) = me_with(app, &[("authorization", format!("Bearer {token}"))]).await;
    assert_eq!(status, StatusCode::OK, "{me}");
}

/// AP-27: the `edge` client signs RS256 by per-client override while the realm stays ES256, so
/// the verifier takes both and nothing else.
#[tokio::test]
async fn rs256_and_es256_tokens_are_both_accepted_and_hs256_is_not() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;
    for algorithm in [
        jsonwebtoken::Algorithm::ES256,
        jsonwebtoken::Algorithm::RS256,
    ] {
        let token = keys::token(algorithm, &issuer_of(&realm), "demo.viewer", 300);
        let (status, me) = me_with(app.clone(), &[("x-access-token", token)]).await;
        assert_eq!(status, StatusCode::OK, "{algorithm:?}: {me}");
        assert_eq!(me["username"], "demo.viewer");
    }

    // A token signed with a shared secret names one of the realm's kids and is still refused.
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some("realm-rs256".into());
    let forged = jsonwebtoken::encode(
        &header,
        &json!({
            "iss": issuer_of(&realm), "aud": "joinedcontext-portal", "sub": "f:1:mallory",
            "exp": joinedcontext_portal::auth::session::now_unix() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(b"guessable"),
    )
    .unwrap();
    let (status, _) = me_with(app, &[("x-access-token", forged)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// A bad edge token is 401 like a bad bearer, and never a look at the cookie beside it.
#[tokio::test]
async fn a_bad_edge_token_is_refused_and_does_not_fall_back_to_the_cookie() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;
    let expired = keys::token(
        jsonwebtoken::Algorithm::RS256,
        &issuer_of(&realm),
        "demo.steward",
        -300,
    );

    for bad in [expired, "not-a-jwt".to_string(), String::new()] {
        let (status, body) = me_with(
            app.clone(),
            &[
                ("x-access-token", bad.clone()),
                ("cookie", session_cookies(300)),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{bad:?}: {body}");
    }
}

/// The Portal's own cookie session is the `portal` front, and `me` says so.
#[tokio::test]
async fn a_cookie_session_reports_the_portal_front() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;

    let (status, me) = me_with(app, &[("cookie", session_cookies(300))]).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["username"], "demo.steward");
    assert_eq!(me["front"], "portal");
}
