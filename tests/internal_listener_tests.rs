//! Who may call the Portal's internal listener (T-1500; PF-46, AG-52).
//!
//! The listener carries no session and no CSRF guard: APISIX routes nothing to it and a
//! NetworkPolicy admits one workload to its port. That policy is the second control, so every
//! route here asks for the identity it belongs to as well — a pod that reaches port 9090 through
//! a policy mistake presents no token of the gateway's and reads nothing.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use p256::pkcs8::EncodePrivateKey;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const REALM_PATH: &str = "/realms/banskabystrica";
/// The Portal's own API audience: a token for `/api/v1` is not a token for this listener.
const PORTAL_AUDIENCE: &str = "portal-api";
/// What the internal listener's tokens are issued for, and the client that may hold one.
const INTERNAL_AUDIENCE: &str = "portal-internal";
const GATEWAY_CLIENT: &str = "context-gateway";

fn keypair(kid: &str) -> (EncodingKey, Value) {
    let secret = p256::SecretKey::random(&mut rand_core::OsRng);
    let der = secret.to_pkcs8_der().expect("der");
    let signer = EncodingKey::from_ec_der(der.as_bytes());
    let mut jwk: Value = serde_json::from_str(&secret.public_key().to_jwk_string()).expect("jwk");
    jwk["kid"] = json!(kid);
    jwk["alg"] = json!("ES256");
    jwk["use"] = json!("sig");
    (signer, json!({ "keys": [jwk] }))
}

fn token(signer: &EncodingKey, issuer: &str, audience: &str, client: &str) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("key-internal-test".to_owned());
    let now = joinedcontext_portal::auth::session::now_unix();
    let claims = json!({
        "iss": issuer,
        "aud": audience,
        "sub": format!("service-account-{client}"),
        "azp": client,
        "exp": now + 300,
        "iat": now,
    });
    encode(&header, &claims, signer).expect("sign")
}

/// The internal listener of a Portal that knows the realm and the gateway's client, and the key
/// that realm signs with.
async fn listener(gateway_client: Option<&str>) -> (axum::Router, EncodingKey, String) {
    let realm: &'static MockServer = Box::leak(Box::new(MockServer::start().await));
    let issuer = format!("{}{REALM_PATH}", realm.uri().trim_end_matches('/'));
    let (signer, jwks) = keypair("key-internal-test");
    Mock::given(method("GET"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/certs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(jwks))
        .mount(realm)
        .await;

    let client = gateway_client.map(str::to_owned);
    let issuer_for_config = issuer.clone();
    let config = Config::from_vars(move |key| match key {
        "JC_OIDC_ISSUER" => Some(issuer_for_config.clone()),
        "JC_OIDC_CLIENT_ID" => Some(PORTAL_AUDIENCE.to_owned()),
        "JC_OIDC_CLIENT_SECRET" => Some("secret".to_owned()),
        "JC_PORTAL_COOKIE_KEY" => Some("k".repeat(64)),
        "JC_PORTAL_GATEWAY_CLIENT_ID" => client.clone(),
        _ => None,
    })
    .expect("config");

    let state = AppState::new(config, None);
    // The realm's keys, as the Portal warms them at startup.
    state
        .bearer
        .as_ref()
        .expect("a realm")
        .refresh()
        .await
        .expect("jwks");
    (server::internal_app(state), signer, issuer)
}

async fn previews(app: &axum::Router, bearer: Option<&str>) -> StatusCode {
    let mut request = Request::builder().method("GET").uri("/internal/previews");
    if let Some(token) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response")
        .status()
}

/// PF-46, AG-52: the preview list answers the Context Gateway's own ServiceAccount token and
/// nothing else. It holds every running preview's manifests, which are a project's configuration.
#[tokio::test]
async fn the_preview_list_refuses_a_call_without_the_gateways_token() {
    let (app, signer, issuer) = listener(Some(GATEWAY_CLIENT)).await;

    // No credential at all: what a pod that reached the port through a policy mistake sends.
    assert_eq!(previews(&app, None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        previews(&app, Some("not-a-token")).await,
        StatusCode::UNAUTHORIZED
    );

    // Another workload of the same realm, with a token for this very listener: the audience is
    // right and the client is not, so it reads nothing either.
    let other = token(&signer, &issuer, INTERNAL_AUDIENCE, "helsinki-agent-proxy");
    assert_eq!(previews(&app, Some(&other)).await, StatusCode::UNAUTHORIZED);

    // The gateway's own token, but issued for the public API: a token a person's app may also
    // hold must not open an internal route.
    let for_api = token(&signer, &issuer, PORTAL_AUDIENCE, GATEWAY_CLIENT);
    assert_eq!(
        previews(&app, Some(&for_api)).await,
        StatusCode::UNAUTHORIZED
    );

    // And the one call that belongs here.
    let gateway = token(&signer, &issuer, INTERNAL_AUDIENCE, GATEWAY_CLIENT);
    assert_eq!(previews(&app, Some(&gateway)).await, StatusCode::OK);
}

/// A Portal that was not told whose token to expect refuses every call rather than falling back
/// to the NetworkPolicy: configuration is fail-closed here as everywhere else.
#[tokio::test]
async fn a_portal_without_a_configured_gateway_client_answers_nobody() {
    let (app, signer, issuer) = listener(None).await;
    let gateway = token(&signer, &issuer, INTERNAL_AUDIENCE, GATEWAY_CLIENT);
    assert_eq!(
        previews(&app, Some(&gateway)).await,
        StatusCode::UNAUTHORIZED
    );
}
