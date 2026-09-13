//! Tests for the Portal Configuration MCP server at `/api/v1/mcp` (T-0637, AG-60, AG-32, CC-45, CC-47).
//!
//! Asserts that:
//! 1. `initialize` and `server/discover` return protocol version and serverInfo.
//! 2. `tools/list` returns tools filtered by caller's effective permissions.
//! 3. `tools/call jc_catalog_search` returns structured content.
//! 4. A bearer token for another audience is rejected with 401 and WWW-Authenticate.
//! 5. A request with only a session cookie (no Bearer header) is rejected with 401.
//! 6. `GET /.well-known/oauth-protected-resource/api/v1/mcp` returns RFC 9728 metadata.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use p256::pkcs8::EncodePrivateKey;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const REALM_PATH: &str = "/realms/banskabystrica";
const PORTAL_AUDIENCE: &str = "joinedcontext-portal";

fn issuer_of(server: &MockServer) -> String {
    format!("{}{REALM_PATH}", server.uri().trim_end_matches('/'))
}

fn generate_keypair(kid: &str) -> (EncodingKey, Value) {
    let secret = p256::SecretKey::random(&mut rand_core::OsRng);
    let der = secret.to_pkcs8_der().expect("der");
    let signer = EncodingKey::from_ec_der(der.as_bytes());
    let mut jwk: Value = serde_json::from_str(&secret.public_key().to_jwk_string()).expect("jwk");
    jwk["kid"] = json!(kid);
    jwk["alg"] = json!("ES256");
    jwk["use"] = json!("sig");
    let jwks = json!({ "keys": [jwk] });
    (signer, jwks)
}

fn sign_token(
    signer: &EncodingKey,
    kid: &str,
    issuer: &str,
    audience: &str,
    subject: &str,
    roles: &[&str],
    groups: &[&str],
) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    let now = joinedcontext_portal::auth::session::now_unix();
    let claims = json!({
        "iss": issuer,
        "aud": audience,
        "sub": subject,
        "preferred_username": subject,
        "exp": now + 3600,
        "iat": now,
        "realm_access": { "roles": roles },
        "groups": groups,
    });
    encode(&header, &claims, signer).expect("sign token")
}

async fn setup_app_and_keys() -> (axum::Router, String, EncodingKey, String) {
    let mock_server = MockServer::start().await;
    let issuer = issuer_of(&mock_server);
    let (signer, jwks) = generate_keypair("key-mcp-test");

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
            "id_token_signing_alg_values_supported": ["ES256"]
        })))
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/certs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(jwks))
        .mount(&mock_server)
        .await;

    let mut config = Config::from_vars(|k| match k {
        "JC_OIDC_ISSUER" => Some(issuer.clone()),
        "JC_OIDC_CLIENT_ID" => Some(PORTAL_AUDIENCE.to_string()),
        "JC_OIDC_CLIENT_SECRET" => Some("secret".to_string()),
        "JC_PORTAL_COOKIE_KEY" => Some("k".repeat(64)),
        _ => None,
    })
    .expect("config");
    config.public_base_url = "https://portal.test".parse().unwrap();

    let mirror = std::sync::Arc::new(Mirror::new());
    // Seed some resources
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Endpoint".to_string(),
        metadata: ObjectMeta::new("public-air", "ovzdusie"),
        spec: json!({
            "slug": "publicair00000000000000000000",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        }),
        status: None,
    });

    // Add role for viewer
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Role".to_string(),
        metadata: ObjectMeta::new("read-only-role", "org"),
        spec: json!({
            "rules": [
                { "kinds": ["Endpoint", "ContextSpace"], "verbs": [] }
            ]
        }),
        status: None,
    });
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "RoleBinding".to_string(),
        metadata: ObjectMeta::new("viewer-binding", "org"),
        spec: json!({
            "subjects": [{ "group": "city-viewers" }],
            "role": "read-only-role",
            "scope": { "project": "ovzdusie" }
        }),
        status: None,
    });

    let state = AppState::from_config(config)
        .await
        .expect("state")
        .with_mirror(mirror);

    (
        server::app(state),
        issuer,
        signer,
        "key-mcp-test".to_string(),
    )
}

#[tokio::test]
async fn mcp_initialize_and_discover() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        // The bootstrap group (Config::DEFAULT_BOOTSTRAP_ADMINS): the steward holds every
        // operation, the viewer below only what its binding grants.
        &["platform-admins"],
    );

    // 1. initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2026-07-28",
            "capabilities": {},
            "clientInfo": { "name": "claude-code", "version": "1.0.0" }
        }
    });

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&init_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let init_res: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(init_res["jsonrpc"], "2.0");
    assert_eq!(init_res["id"], 1);
    assert_eq!(init_res["result"]["protocolVersion"], "2026-07-28");
    assert_eq!(
        init_res["result"]["serverInfo"]["name"],
        "joinedcontext-portal"
    );

    // 2. server/discover
    let discover_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "server/discover"
    });

    let resp_disc = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&discover_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_disc.status(), StatusCode::OK);
    let bytes_disc = resp_disc.into_body().collect().await.unwrap().to_bytes();
    let disc_res: Value = serde_json::from_slice(&bytes_disc).unwrap();
    assert_eq!(disc_res["jsonrpc"], "2.0");
    assert_eq!(disc_res["id"], 2);
    assert_eq!(disc_res["result"]["protocolVersion"], "2026-07-28");
    assert!(disc_res["result"]["tools"]["count"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn mcp_tools_list_filtered_by_caller() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;

    // 1. Steward bearer token
    let steward_token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        // The bootstrap group (Config::DEFAULT_BOOTSTRAP_ADMINS): the steward holds every
        // operation, the viewer below only what its binding grants.
        &["platform-admins"],
    );

    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/list",
        "params": {
            "project": "ovzdusie"
        }
    });

    let resp_steward = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {steward_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&list_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_steward.status(), StatusCode::OK);
    let bytes_steward = resp_steward.into_body().collect().await.unwrap().to_bytes();
    let res_steward: Value = serde_json::from_slice(&bytes_steward).unwrap();
    let tools = res_steward["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(names.contains(&"jc_catalog_search"));
    assert!(names.contains(&"jc_datasource_propose"));
    assert!(names.contains(&"jc_change_approve"));

    // Check annotations and schemas
    let search_tool = tools
        .iter()
        .find(|t| t["name"] == "jc_catalog_search")
        .unwrap();
    assert_eq!(search_tool["annotations"]["readOnlyHint"], true);
    assert_eq!(search_tool["annotations"]["destructiveHint"], false);
    assert_eq!(search_tool["annotations"]["idempotentHint"], true);

    // 2. Viewer bearer token
    let viewer_token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "viewer.user",
        &[],
        &["city-viewers"],
    );

    let resp_viewer = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {viewer_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&list_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_viewer.status(), StatusCode::OK);
    let bytes_viewer = resp_viewer.into_body().collect().await.unwrap().to_bytes();
    let res_viewer: Value = serde_json::from_slice(&bytes_viewer).unwrap();
    let v_tools = res_viewer["result"]["tools"].as_array().unwrap();
    let v_names: Vec<&str> = v_tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(v_names.contains(&"jc_catalog_search"));
    assert!(
        !v_names.contains(&"jc_datasource_propose"),
        "viewer must not see propose tools"
    );
    assert!(
        !v_names.contains(&"jc_change_approve"),
        "viewer must not see approve tools"
    );
}

#[tokio::test]
async fn mcp_tools_call_jc_catalog_search() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        // The bootstrap group (Config::DEFAULT_BOOTSTRAP_ADMINS): the steward holds every
        // operation, the viewer below only what its binding grants.
        &["platform-admins"],
    );

    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 20,
        "method": "tools/call",
        "params": {
            "name": "jc_catalog_search",
            "arguments": {
                "project": "ovzdusie",
                "q": "air"
            }
        }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&call_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let call_res: Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(call_res["jsonrpc"], "2.0");
    assert_eq!(call_res["id"], 20);
    assert_eq!(call_res["result"]["isError"], false);

    let structured = &call_res["result"]["structuredContent"];
    let items = structured["items"].as_array().expect("items in catalog");
    assert!(!items.is_empty());
    assert_eq!(items[0]["name"], "public-air");

    let content = call_res["result"]["content"].as_array().expect("content");
    assert_eq!(content[0]["type"], "text");
    assert!(content[0]["text"].as_str().unwrap().contains("public-air"));
}

#[tokio::test]
async fn mcp_wrong_audience_bearer_returns_401_with_www_authenticate() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;

    // Token for a different audience
    let foreign_token = sign_token(
        &signer,
        &kid,
        &issuer,
        "context-gateway",
        "attacker.user",
        &["portal-approver"],
        &[],
    );

    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 30,
        "method": "ping"
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {foreign_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&call_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let auth_header = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate header");
    let auth_str = auth_header.to_str().unwrap();
    assert!(auth_str.starts_with("Bearer resource_metadata="));
    assert!(auth_str.contains("/.well-known/oauth-protected-resource/api/v1/mcp"));
}

#[tokio::test]
async fn mcp_session_cookie_only_returns_401() {
    let (app, _issuer, _signer, _kid) = setup_app_and_keys().await;

    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 40,
        "method": "ping"
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::COOKIE, "jc_session=some_session_value")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&call_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_oauth_protected_resource_metadata() {
    let (app, issuer, _signer, _kid) = setup_app_and_keys().await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/oauth-protected-resource/api/v1/mcp")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let meta: Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(meta["resource"], "https://portal.test/api/v1/mcp");
    assert_eq!(meta["authorization_servers"], json!([issuer]));
    assert_eq!(meta["bearer_methods_supported"], json!(["header"]));
    assert_eq!(meta["scopes_supported"], json!(["mcp:portal"]));
}

async fn rpc(app: axum::Router, token: &str, body: Value) -> Value {
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn mcp_resources_list_and_read_manifests_drafts_and_schemas() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        &["platform-admins"],
    );

    // A draft filed through the registry shows up as a resource too.
    let put = rpc(
        app.clone(),
        &token,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "jc_draft_put", "arguments": {
                "project": "ovzdusie", "kind": "DataSource", "name": "hsy-air",
                "manifest": { "apiVersion": "joinedcontext.com/v1", "kind": "DataSource",
                    "metadata": { "name": "hsy-air", "namespace": "ovzdusie" },
                    "spec": { "protocol": "http", "url": "https://example.org/air" } }
            } }
        }),
    )
    .await;
    assert_eq!(put["result"]["isError"], false, "{put}");

    let listed = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "resources/list", "params": { "project": "ovzdusie" } }),
    )
    .await;
    let uris: Vec<&str> = listed["result"]["resources"]
        .as_array()
        .expect("resources")
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    assert!(uris.contains(&"jc://ovzdusie/endpoints/public-air"), "{uris:?}");
    assert!(uris.contains(&"jc://ovzdusie/drafts/DataSource/hsy-air"), "{uris:?}");
    assert!(uris.contains(&"jc://schemas/Endpoint"), "{uris:?}");

    for (uri, needle) in [
        ("jc://ovzdusie/endpoints/public-air", "publicair00000000000000000000"),
        ("jc://ovzdusie/drafts/DataSource/hsy-air", "https://example.org/air"),
        ("jc://schemas/Endpoint", "\"properties\""),
    ] {
        let read = rpc(
            app.clone(),
            &token,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/read", "params": { "uri": uri } }),
        )
        .await;
        let text = read["result"]["contents"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{uri}: {read}"));
        assert!(text.contains(needle), "{uri}: {text}");
    }

    let missing = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/read", "params": { "uri": "jc://ovzdusie/endpoints/nope" } }),
    )
    .await;
    assert_eq!(missing["error"]["code"], -32002);

    // A caller without a grant on the project sees nothing and reads nothing.
    let stranger = sign_token(&signer, &kid, &issuer, PORTAL_AUDIENCE, "nobody", &[], &[]);
    let listed = rpc(
        app.clone(),
        &stranger,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list", "params": { "project": "ovzdusie" } }),
    )
    .await;
    assert_eq!(listed["result"]["resources"].as_array().map(Vec::len), Some(0));
    let read = rpc(
        app,
        &stranger,
        json!({ "jsonrpc": "2.0", "id": 6, "method": "resources/read", "params": { "uri": "jc://ovzdusie/endpoints/public-air" } }),
    )
    .await;
    assert_eq!(read["error"]["code"], -32002);
}

#[tokio::test]
async fn mcp_prompts_name_the_units_and_their_operations() {
    let (app, issuer, signer, kid) = setup_app_and_keys().await;
    let token = sign_token(
        &signer,
        &kid,
        &issuer,
        PORTAL_AUDIENCE,
        "steward.user",
        &["portal-approver"],
        &["platform-admins"],
    );
    let listed = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "prompts/list" }),
    )
    .await;
    let names: Vec<&str> = listed["result"]["prompts"]
        .as_array()
        .expect("prompts")
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert_eq!(names, ["load", "share", "analyse", "model"]);

    let got = rpc(
        app.clone(),
        &token,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "prompts/get",
            "params": { "name": "share", "arguments": { "project": "ovzdusie" } } }),
    )
    .await;
    let text = got["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("Project: ovzdusie"), "{text}");
    assert!(text.contains("jc_endpoint_propose"), "{text}");

    let unknown = rpc(
        app,
        &token,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "prompts/get", "params": { "name": "dance" } }),
    )
    .await;
    assert_eq!(unknown["error"]["code"], -32602);
}
